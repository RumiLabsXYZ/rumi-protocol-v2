//! Phase 1c notify-then-verify: turn a verified tx receipt into applied burns.
use crate::chains::config::ChainId;
use crate::chains::monad::deposit_watch::apply_burn_to_state;
use crate::chains::monad::evm_rpc::{
    decode_burn_log, get_transaction_receipt_with_logs, is_block_final, BurnLog,
    TxReceiptWithLogs, BURN_EVENT_TOPIC0,
};
use crate::chains::multi_chain_state::MultiChainStateV4;
use crate::logs::INFO;
use crate::state::{mutate_state, read_state};
use ic_canister_log::log;

/// Apply every `Burn` log in `receipt` that was emitted by `contract` to protocol
/// state, deduped on `(tx_hash, log_index)` via `processed_burn_keys`. Returns the
/// burns newly applied (the caller emits a `ChainBurnObserved` event per entry and
/// uses the count). Caller MUST have verified `receipt.success` and finality, and
/// MUST pass a normalized (lowercased) `tx_hash`, before calling. Trust rules:
///  - only logs whose `address == contract` (case-insensitive) and whose
///    `topics[0] == BURN_EVENT_TOPIC0` are considered (a user cannot forge a burn
///    by emitting their own event);
///  - amount + vault_id come FROM the log, never from caller input;
///  - already-seen `(tx_hash, log_index)` is skipped (idempotent re-submit);
///  - a halt-class `apply_burn_to_state` error aborts the whole call (Err) so the
///    invariant machinery halts; an InvalidBurn (unknown vault / over-repay) is
///    skipped-and-recorded (cannot ever succeed), matching the poll path.
///
/// On a halt-class abort, burns already applied earlier in this receipt stay
/// committed with their keys recorded (no `.await` in this synchronous loop, so
/// apply+record commit in one message slice) — a retry re-skips them and
/// re-attempts the halting burn. This matches the audited poll-path C-1 semantics.
pub fn apply_receipt_burns_to_state(
    state: &mut MultiChainStateV4,
    _chain: ChainId,
    contract: &str,
    tx_hash: &str,
    receipt: &TxReceiptWithLogs,
) -> Result<Vec<BurnLog>, String> {
    let mut applied: Vec<BurnLog> = Vec::new();
    for (address, topics, data, log_index) in &receipt.logs {
        if !address.eq_ignore_ascii_case(contract) {
            continue;
        }
        if topics
            .first()
            .map(|t| !t.eq_ignore_ascii_case(BURN_EVENT_TOPIC0))
            .unwrap_or(true)
        {
            continue;
        }
        let burn = match decode_burn_log(topics, data, tx_hash, receipt.block_number) {
            Ok(b) => b,
            Err(e) => {
                log!(INFO, "[burn_proof] decode failed (skip): {}", e);
                continue;
            }
        };
        let key = format!("{}:{}", burn.tx_hash, log_index);
        let seen = state
            .processed_burn_keys
            .get(&burn.block_number)
            .map(|set| set.contains(&key))
            .unwrap_or(false);
        if seen {
            continue;
        }
        let total = state.total_chain_vault_debt_e8s();
        match apply_burn_to_state(state, &burn, total) {
            Ok(()) => {
                state
                    .processed_burn_keys
                    .entry(burn.block_number)
                    .or_default()
                    .insert(key);
                applied.push(burn.clone());
            }
            Err(crate::chains::monad::deposit_watch::BurnApplyError::InvalidBurn(msg)) => {
                log!(INFO, "[burn_proof] invalid burn skipped: {}", msg);
                state
                    .processed_burn_keys
                    .entry(burn.block_number)
                    .or_default()
                    .insert(key);
            }
            Err(crate::chains::monad::deposit_watch::BurnApplyError::SupplyInvariant(e)) => {
                return Err(format!("halt-class supply invariant: {:?}", e));
            }
        }
    }
    Ok(applied)
}

#[derive(Debug)]
pub enum BurnProofError {
    NoContract,
    Pending,      // receipt not yet mined
    Reverted,     // status != 0x1
    NotFinal,     // mined but not buried under finality_depth
    Rpc(String),
    Halt(String), // halt-class invariant failure
}

/// Fetch the receipt for `tx_hash`, verify success + finality, and apply any Burn
/// logs from the configured contract. Returns the count newly applied. No state
/// borrow is held across the awaits.
///
/// The notify path must emit a `ChainBurnObserved` event per applied burn so that
/// analytics sees notify-path burns the same way it sees poll-path burns (the poll
/// loop in `deposit_watch.rs` emits one per applied burn). We capture the applied
/// `Vec<BurnLog>` from the (synchronous) `apply_receipt_burns_to_state` and emit the
/// events OUTSIDE the `mutate_state` closure, after a successful apply.
pub async fn verify_and_apply_burn_proof(
    chain: ChainId,
    tx_hash: &str,
) -> Result<u32, BurnProofError> {
    // Lowercase the caller-supplied hash before it reaches the core: it becomes the
    // dedup key, so mixed casing must not be able to bypass dedup and double-apply.
    let tx = tx_hash.to_ascii_lowercase();

    let contract = read_state(|s| s.multi_chain.chain_contracts.get(&chain).cloned())
        .ok_or(BurnProofError::NoContract)?;
    let finality_depth = read_state(|s| {
        s.multi_chain
            .chain_configs
            .get(&chain)
            .map(|c| c.finality_depth as u64)
    })
    .unwrap_or(1);

    let receipt = match get_transaction_receipt_with_logs(chain, &tx)
        .await
        .map_err(BurnProofError::Rpc)?
    {
        Some(r) => r,
        None => return Err(BurnProofError::Pending),
    };
    if !receipt.success {
        return Err(BurnProofError::Reverted);
    }
    if !is_block_final(chain, receipt.block_number, finality_depth)
        .await
        .map_err(BurnProofError::Rpc)?
    {
        return Err(BurnProofError::NotFinal);
    }

    // Apply synchronously (no `.await` inside), capturing the burns newly applied.
    let applied: Vec<BurnLog> = mutate_state(|s| {
        apply_receipt_burns_to_state(&mut s.multi_chain, chain, &contract, &tx, &receipt)
    })
    .map_err(BurnProofError::Halt)?;

    // Emit one ChainBurnObserved event per applied burn, mirroring the poll path
    // (deposit_watch.rs line 686). record_event is its own call, done outside the
    // mutate_state closure above, only after a successful apply.
    let now = ic_cdk::api::time();
    for burn in &applied {
        crate::storage::record_event(&crate::event::Event::ChainBurnObserved {
            chain_id: chain,
            vault_id: burn.vault_id,
            amount_e8s: burn.amount_e8s,
            tx_hash: burn.tx_hash.clone(),
            block_number: burn.block_number,
            timestamp: now,
        });
    }

    Ok(applied.len() as u32)
}
