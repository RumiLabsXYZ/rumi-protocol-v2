//! Phase 1c notify-then-verify: turn a verified tx receipt into applied burns.
use crate::chains::config::ChainId;
use crate::chains::monad::deposit_watch::apply_burn_to_state;
use crate::chains::monad::evm_rpc::{decode_burn_log, BurnLog, TxReceiptWithLogs, BURN_EVENT_TOPIC0};
use crate::chains::multi_chain_state::MultiChainStateV2;
use crate::logs::INFO;
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
    state: &mut MultiChainStateV2,
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
