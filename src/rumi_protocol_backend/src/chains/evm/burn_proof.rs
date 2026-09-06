//! Phase 1c notify-then-verify: turn a verified tx receipt into applied burns.
use crate::chains::config::ChainId;
use crate::chains::monad::deposit_watch::apply_burn_to_state;
use crate::chains::monad::evm_rpc::{
    decode_burn_log, get_transaction_receipt_with_logs, is_block_final, BurnLog,
    TxReceiptWithLogs, BURN_EVENT_TOPIC0,
};
use crate::chains::multi_chain_state::MultiChainState;
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
///
/// F-02 residual (2026-06-18 audit): this is the ONLY place that mutates state
/// or inserts into `processed_burn_keys` on the notify path, so the
/// `reorg_halted` guard MUST live here, as the very first check, rather than
/// only in the caller. `run_observer` (deposit_watch.rs) already refuses to
/// scan while `reorg_halted[chain]` is set; this mirrors that check for the
/// independent `submit_burn_proof` path, which was not gated on it. Refusing
/// here means: this function is always called synchronously inside a single
/// `mutate_state` closure (see `verify_and_apply_burn_proof` below), so
/// checking `reorg_halted` as the first statement is equivalent to checking it
/// "immediately before the mutation, with no `.await` in between" (there is
/// no `.await` anywhere in this function).
pub fn apply_receipt_burns_to_state(
    state: &mut MultiChainState,
    chain: ChainId,
    contract: &str,
    tx_hash: &str,
    receipt: &TxReceiptWithLogs,
) -> Result<Vec<BurnLog>, ApplyBurnsError> {
    if state.reorg_halted.get(&chain).copied().unwrap_or(false) {
        // Refuse outright: no key inserted, no debt/supply mutation. Retryable
        // once an operator clears the halt (`clear_reorg_halt`); nothing was
        // consumed, so the identical proof can be resubmitted and will apply
        // exactly once via the existing `processed_burn_keys` dedup.
        return Err(ApplyBurnsError::ReorgHalted);
    }
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
                return Err(ApplyBurnsError::Halt(format!(
                    "halt-class supply invariant: {:?}",
                    e
                )));
            }
            Err(crate::chains::monad::deposit_watch::BurnApplyError::DeferredLiquidation) => {
                // Vault mid-liquidation (findings #11/#19): skip without recording
                // the key so a later proof submission re-applies it once the
                // liquidation marker clears. Do not abort the whole proof.
                log!(
                    INFO,
                    "[burn_proof] deferring burn for vault {} (mid-liquidation); will retry on a later proof",
                    burn.vault_id
                );
            }
        }
    }
    Ok(applied)
}

/// Error from the synchronous apply step. Kept distinct from `BurnProofError`
/// so the sync core (`apply_receipt_burns_to_state`) has no dependency on the
/// async wrapper's error shape; `verify_and_apply_burn_proof` maps it 1:1.
#[derive(Debug, PartialEq, Eq)]
pub enum ApplyBurnsError {
    /// Chain is currently `reorg_halted`. Nothing was mutated or recorded.
    /// Retryable: resubmit the identical proof after `clear_reorg_halt`.
    ReorgHalted,
    /// Halt-class invariant failure (e.g. supply invariant violation). Burns
    /// applied earlier in this same receipt, before the failing log, stay
    /// committed (see the doc comment on `apply_receipt_burns_to_state`).
    Halt(String),
}

#[derive(Debug)]
pub enum BurnProofError {
    NoContract,
    Pending,  // receipt not yet mined
    Reverted, // status != 0x1
    NotFinal, // mined but not buried under finality_depth
    Rpc(String),
    Halt(String), // halt-class invariant failure
    /// Chain is currently `reorg_halted` (Task 11 circuit breaker). Distinct
    /// from `Halt`: this is NOT terminal/corrupt data, it is a temporary
    /// operator-controlled gate. Nothing was consumed or recorded, so the
    /// caller should retry the identical proof after `clear_reorg_halt`.
    ReorgHalted,
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

    // Cheap fail-fast BEFORE any RPC await: a chain already `reorg_halted`
    // before we spend outcall cycles has no chance of applying anyway (the
    // check inside `apply_receipt_burns_to_state` below would refuse it after
    // paying for the receipt fetch + finality probe). This is purely a cost
    // optimization; the authoritative guard is the re-check after the awaits,
    // immediately below.
    if read_state(|s| s.multi_chain.reorg_halted.get(&chain).copied().unwrap_or(false)) {
        return Err(BurnProofError::ReorgHalted);
    }

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
    // `apply_receipt_burns_to_state` re-checks `reorg_halted` as its first
    // statement, inside this same closure with no `.await` between the check
    // and the mutation, so a halt raised during the awaits above (receipt
    // fetch / finality probe) cannot race a burn through.
    let applied: Vec<BurnLog> = mutate_state(|s| {
        apply_receipt_burns_to_state(&mut s.multi_chain, chain, &contract, &tx, &receipt)
    })
    .map_err(|e| match e {
        ApplyBurnsError::ReorgHalted => BurnProofError::ReorgHalted,
        ApplyBurnsError::Halt(msg) => BurnProofError::Halt(msg),
    })?;

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
