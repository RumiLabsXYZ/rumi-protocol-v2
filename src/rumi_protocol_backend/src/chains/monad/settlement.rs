//! Outbound settlement worker (Timer D) for Monad (Phase 1b, Task 10).
//!
//! ## Pure helpers (unit-tested in `tests_settlement`)
//!
//! - `select_next_op`: picks the next op to act on, enforcing
//!   one-in-flight-per-queue (Confirm an Inflight op before submitting a new
//!   Queued one).
//! - `confirm_mint_in_state`: on a confirmed on-chain mint, moves the vault's
//!   `pending_mint_e8s` into `debt_e8s`, flips it to `Open`, and increments the
//!   chain supply via `apply_supply_delta`.
//!
//! ## Supply-invariant total convention (mirrors `apply_burn_to_state`)
//!
//! The Phase 1b supply invariant is FOREIGN-CHAIN-ONLY:
//!   `sum(chain_supplies) == MultiChainStateV2::total_chain_vault_debt_e8s()`.
//! The ICP-native `State::total_borrowed_icusd_amount()` is a SEPARATE pool and
//! is NEVER consulted here. `confirm_mint_in_state` takes the PRE-mint
//! `total_chain_vault_debt_e8s()` and computes the post-mint total internally
//! (`pre + observed`), passing THAT to `apply_supply_delta` — exactly the
//! convention `apply_burn_to_state` uses with `pre - amount`.
//!
//! ## Async worker (run_settlement)
//!
//! `run_settlement` drains one chain's `settlement_queues` entry one op per
//! tick (Timer D, wired in Task 12). It mirrors `run_observer`'s read → await
//! RPC → mutate pattern; no `read_state`/`mutate_state` borrow is held across
//! an `.await`. The async path is NOT unit-tested — PocketIC covers it in
//! Task 17.
//!
//! ## Task 11 seams (NOT implemented here)
//!
//! Two hardening hooks land in Task 11 (`hardening.rs`, which does not yet
//! exist): a hot-wallet gas gate before submitting, and a stuck-tx
//! bump-gas/resubmit on the Confirm path. Both are marked with `TASK 11 SEAM:`
//! comments. This task references no hardening symbol.

use ic_canister_log::log;

use crate::chains::adapter::{ChainAdapter, MintInstruction, WithdrawalRequest};
use crate::chains::config::ChainId;
use crate::chains::monad::chain_vault::ChainVaultStatus;
use crate::chains::multi_chain_state::MultiChainStateV2;
use crate::chains::settlement_queue::{SettlementOpKind, SettlementOpStatus, SettlementQueueV1};
use crate::chains::supply::{apply_supply_delta, SupplyDelta};
use crate::logs::INFO;
use crate::state::{mutate_state, read_state};
use crate::Mode;

use super::evm_rpc;
use super::MonadAdapter;

// ─── Pure helpers ─────────────────────────────────────────────────────────────

/// What the drain loop should do with the op `select_next_op` returns.
#[derive(Debug, PartialEq, Eq)]
pub enum OpAction {
    /// A `Queued` op: sign and broadcast it, then mark it `Inflight`.
    Submit,
    /// An `Inflight` op: check its receipt and (on a confirmed mint) finalize.
    Confirm,
}

/// Pick the next op to act on. Enforces one-in-flight-per-queue: if ANY op is
/// `Inflight`, only that op (action `Confirm`) is actionable; otherwise the
/// lowest-op_id `Queued` op (action `Submit`). `pending` is a
/// `BTreeMap<u64, SettlementOp>`, so iteration is op_id-ascending and the
/// drain is FIFO. Returns `None` when nothing is actionable.
pub fn select_next_op(q: &SettlementQueueV1) -> Option<(u64, OpAction)> {
    for (&id, op) in q.pending.iter() {
        if matches!(op.status, SettlementOpStatus::Inflight { .. }) {
            return Some((id, OpAction::Confirm));
        }
    }
    for (&id, op) in q.pending.iter() {
        if matches!(op.status, SettlementOpStatus::Queued) {
            return Some((id, OpAction::Submit));
        }
    }
    None
}

/// On a confirmed on-chain mint: move `pending_mint_e8s` into `debt_e8s`, flip
/// the vault to `Open`, and increment the chain supply.
///
/// `pre_mint_total_debt` is the PRE-mint `total_chain_vault_debt_e8s()` (the
/// sum of all chain-vault debt BEFORE this mint counts — under Design B the
/// vault's `debt_e8s` is still 0 while the mint is pending, so its pending
/// amount is NOT yet in this total). This helper computes the post-mint total
/// internally as `pre_mint_total_debt + observed_e8s` and passes THAT to
/// `apply_supply_delta`, mirroring `apply_burn_to_state`. `observed_e8s` (read
/// from the on-chain Mint log) must equal the vault's `pending_mint_e8s`.
///
/// ## Mutation ordering (no-mutation-on-rejection guarantee)
///
/// 1. Vault lookup + amount match — reject (no mutation) on failure.
/// 2. `apply_supply_delta` — validates and mutates `chain_supplies`, or rejects
///    entirely (no mutation on `Err`, e.g. divergence/halt).
/// 3. Only after (2) succeeds: move `pending_mint_e8s` -> `debt_e8s`, set
///    status `Open`.
pub fn confirm_mint_in_state(
    state: &mut MultiChainStateV2,
    chain: ChainId,
    vault_id: u64,
    observed_e8s: u128,
    pre_mint_total_debt: u128,
) -> Result<(), String> {
    // Step 1: validate (read-only — no mutation on failure).
    {
        let v = state
            .chain_vaults
            .get(&vault_id)
            .ok_or_else(|| format!("confirm_mint: unknown vault {vault_id}"))?;
        if v.pending_mint_e8s != observed_e8s {
            return Err(format!(
                "confirm_mint: observed {} != pending {}",
                observed_e8s, v.pending_mint_e8s
            ));
        }
    }

    // Step 2: supply delta. The post-mint total is the pre-mint total plus the
    // amount this mint adds to the foreign-chain debt pool.
    let post_mint_total = pre_mint_total_debt.saturating_add(observed_e8s);
    apply_supply_delta(state, chain, SupplyDelta::Increase(observed_e8s), post_mint_total)
        .map_err(|e| format!("{e:?}"))?;

    // Step 3: only reached when the supply delta succeeded — move pending -> debt.
    let v = state
        .chain_vaults
        .get_mut(&vault_id)
        .expect("vault present: checked above");
    v.debt_e8s = v.debt_e8s.saturating_add(observed_e8s);
    v.pending_mint_e8s = 0;
    v.status = ChainVaultStatus::Open;
    Ok(())
}

// ─── Async worker ─────────────────────────────────────────────────────────────

/// Run one settlement cycle for the given chain.
///
/// Called by Timer D (configured in Task 12). Acts on at most one op per tick
/// (the one chosen by `select_next_op`):
///
/// - **Submit** (a `Queued` op): sign via the adapter and broadcast via
///   `eth_sendRawTransaction`, then mark the op `Inflight` with the tx hash.
/// - **Confirm** (an `Inflight` op): check the receipt; once mined AND final,
///   a successful mint is finalized through `confirm_mint_in_state`, the op is
///   marked `Succeeded`, and `ChainMintConfirmed` is emitted. A reverted mint
///   is marked `Failed` and the vault's `pending_mint_e8s` is cleared (Design
///   B: no debt was counted, so no supply reversal is needed).
///
/// Borrow discipline mirrors `run_observer`: clone the op out of state for the
/// async RPC calls, then re-acquire a `mutate_state` borrow to write the
/// resulting status back. No `read_state`/`mutate_state` borrow is ever held
/// across an `.await`.
pub async fn run_settlement(chain: ChainId) {
    // Guard: skip if in read-only mode or the supply invariant has halted.
    let should_skip = read_state(|s| s.mode == Mode::ReadOnly || s.multi_chain.invariant_halted);
    if should_skip {
        return;
    }

    // Snapshot this chain's queue and pick the next actionable op.
    let queue = read_state(|s| s.multi_chain.settlement_queues.get(&chain).cloned());
    let queue = match queue {
        Some(q) => q,
        None => return, // chain not registered / no queue
    };
    let (op_id, action) = match select_next_op(&queue) {
        Some(pair) => pair,
        None => return, // nothing to do
    };
    // Clone the op out so we can drop the queue snapshot before awaiting.
    let op = match queue.pending.get(&op_id).cloned() {
        Some(o) => o,
        None => return,
    };

    match action {
        OpAction::Submit => submit_op(chain, op_id, op).await,
        OpAction::Confirm => confirm_op(chain, op_id, op).await,
    }
}

/// Submit path: sign + broadcast a `Queued` op, then mark it `Inflight`.
async fn submit_op(
    chain: ChainId,
    op_id: u64,
    op: crate::chains::settlement_queue::SettlementOp,
) {
    // TASK 11 SEAM: gas-gate (hot_wallet_ok) check goes here before signing/submitting.

    let adapter = MonadAdapter::new(chain);

    // 1. Sign the op into a 0x-hex signed-tx string (the adapter returns the
    //    hex bytes in `raw_tx`; `tx_hash` is empty until the broadcaster fills
    //    it). The mint vault_id is carried so the Confirm path can match the log.
    let (signed_bytes, vault_id, recipient, amount_e8s) = match &op.kind {
        SettlementOpKind::Mint { recipient, amount_e8s, vault_id } => {
            let instr = MintInstruction {
                recipient: recipient.clone(),
                amount_e8s: *amount_e8s,
                vault_id: *vault_id,
                idempotency_key: op.idempotency_key.clone(),
            };
            match adapter.sign_mint(instr).await {
                Ok(signed) => (signed.raw_tx, *vault_id, recipient.clone(), *amount_e8s),
                Err(e) => {
                    log!(INFO, "[settlement chain={:?}] sign_mint failed for op {}: {:?}; will retry", chain, op_id, e);
                    return;
                }
            }
        }
        SettlementOpKind::Withdrawal { recipient, amount_e8s } => {
            // Task 13: withdrawal ops are not enqueued until close-vault lands;
            // the close/WithdrawalSigned-with-vault_id bookkeeping is built
            // there. The signing/broadcast mechanism below is reused as-is.
            let req = WithdrawalRequest {
                recipient: recipient.clone(),
                amount_e8s: *amount_e8s,
                idempotency_key: op.idempotency_key.clone(),
            };
            match adapter.sign_withdrawal(req).await {
                // vault_id 0 is a placeholder; Task 13 threads the real vault id.
                Ok(signed) => (signed.raw_tx, 0u64, recipient.clone(), *amount_e8s),
                Err(e) => {
                    log!(INFO, "[settlement chain={:?}] sign_withdrawal failed for op {}: {:?}; will retry", chain, op_id, e);
                    return;
                }
            }
        }
        SettlementOpKind::Burn { .. } => {
            // The canister never signs burns in Phase 1b (burns are
            // user-initiated on-chain). Mark this op Failed rather than panic.
            let now = ic_cdk::api::time();
            let reason = "burn not signable in Phase 1b".to_string();
            mutate_state(|s| {
                if let Some(q) = s.multi_chain.settlement_queues.get_mut(&chain) {
                    if let Some(o) = q.pending.get_mut(&op_id) {
                        o.mark_failed(reason.clone(), now);
                    }
                }
            });
            crate::storage::record_event(&crate::event::Event::ChainSettlementFailed {
                chain_id: chain,
                op_id,
                reason,
                timestamp: now,
            });
            log!(INFO, "[settlement chain={:?}] op {} is a Burn; marked Failed (burns are user-initiated in Phase 1b)", chain, op_id);
            return;
        }
    };

    // 2. Reconstruct the 0x-hex string from the signed-tx UTF-8 bytes.
    let raw_hex = match String::from_utf8(signed_bytes) {
        Ok(h) => h,
        Err(e) => {
            log!(INFO, "[settlement chain={:?}] op {} signed tx not valid UTF-8: {}; will retry", chain, op_id, e);
            return;
        }
    };

    // 3. Broadcast. A transient send error is logged and retried next tick — we
    //    do NOT mark the op Failed (it may yet be re-signable / re-broadcastable).
    //
    //    ON-CHAIN DOUBLE-MINT DEPENDENCY: if send_raw_transaction returns Err but
    //    the tx actually landed (an RPC false negative), the next tick re-reads
    //    the "latest" nonce (adapter.rs::sign_mint), so it signs a NEW tx at
    //    nonce+1 — a genuine second on-chain mint, not a replacement. The
    //    canister's supply accounting stays correct (confirm requires
    //    observed_e8s == pending_mint_e8s and credits exactly once), but on-chain
    //    icUSD could be minted twice. Protection lives OUTSIDE this function:
    //    (a) IcUSD.mint MUST guard per vault_id (Task 19: `mapping(uint64 => bool)
    //    minted`, revert on repeat — asserted by a Task 20 test), and (b) Task
    //    11's stuck-tx path resubmits on the SAME stored nonce. Until both land,
    //    the resubmit-after-transient-error case is unguarded at the canister layer.
    let tx_hash = match evm_rpc::send_raw_transaction(chain, &raw_hex).await {
        Ok(h) => h,
        Err(e) => {
            log!(INFO, "[settlement chain={:?}] send_raw_transaction failed for op {}: {}; will retry", chain, op_id, e);
            return;
        }
    };

    // 4. Mark Inflight + record the tx hash. Emit ChainMintSubmitted for mints.
    let now = ic_cdk::api::time();
    let is_mint = matches!(op.kind, SettlementOpKind::Mint { .. });
    mutate_state(|s| {
        if let Some(q) = s.multi_chain.settlement_queues.get_mut(&chain) {
            if let Some(o) = q.pending.get_mut(&op_id) {
                o.mark_inflight(now);
                o.last_tx_hash = Some(tx_hash.clone());
            }
        }
    });

    if is_mint {
        crate::storage::record_event(&crate::event::Event::ChainMintSubmitted {
            chain_id: chain,
            vault_id,
            op_id,
            recipient,
            amount_e8s,
            tx_hash: tx_hash.clone(),
            timestamp: now,
        });
        log!(INFO, "[settlement chain={:?}] mint submitted: op={} vault={} amount_e8s={} tx={}", chain, op_id, vault_id, amount_e8s, tx_hash);
    } else {
        log!(INFO, "[settlement chain={:?}] op {} submitted inflight tx={}", chain, op_id, tx_hash);
    }
}

/// Confirm path: check an `Inflight` op's receipt and finalize on success.
async fn confirm_op(
    chain: ChainId,
    op_id: u64,
    op: crate::chains::settlement_queue::SettlementOp,
) {
    // The submit path always sets last_tx_hash before going Inflight; a None
    // here is defensive (e.g. a manually-poked state) — log and bail.
    let tx_hash = match &op.last_tx_hash {
        Some(h) => h.clone(),
        None => {
            log!(INFO, "[settlement chain={:?}] inflight op {} has no last_tx_hash; skipping", chain, op_id);
            return;
        }
    };

    // 1. Fetch the receipt.
    let receipt = match evm_rpc::get_transaction_receipt(chain, &tx_hash).await {
        Ok(r) => r,
        Err(e) => {
            log!(INFO, "[settlement chain={:?}] get_transaction_receipt failed for op {} tx {}: {}; will retry", chain, op_id, tx_hash, e);
            return;
        }
    };

    let (status_ok, block_number) = match receipt {
        Some(pair) => pair,
        None => {
            // Not mined yet — leave Inflight and retry next tick.
            // TASK 11 SEAM: is_stuck + bump_gas + resubmit on same nonce goes here.
            return;
        }
    };

    let now = ic_cdk::api::time();

    // 2. Reverted tx: mark the op Failed. Under Design B no debt was counted, so
    //    there is NO supply reversal. Clear the vault's pending mint (the mint
    //    will not happen). Do NOT advance the vault status: per the plan (Task
    //    10) a reverted mint changes no vault status, and `Closed` in this
    //    codebase means "fully repaid + collateral returned" — stamping it here
    //    would mislabel a vault that still holds deposited collateral as
    //    returned, and the Task-13 close path (which returns collateral by
    //    enqueuing a Withdrawal and going Closing -> Closed) keys off `Closing`,
    //    not `Closed`, so the collateral would be stranded. The failed-mint
    //    resolution (return collateral, then close) is defined by the Task 12/13
    //    flow design; the vault is left in its current (MintPending) status with
    //    a Failed op + a ChainSettlementFailed event for that path to act on.
    if !status_ok {
        let reason = "tx reverted".to_string();
        if let SettlementOpKind::Mint { vault_id, .. } = &op.kind {
            let vid = *vault_id;
            mutate_state(|s| {
                if let Some(v) = s.multi_chain.chain_vaults.get_mut(&vid) {
                    v.pending_mint_e8s = 0;
                }
            });
        }
        mutate_state(|s| {
            if let Some(q) = s.multi_chain.settlement_queues.get_mut(&chain) {
                if let Some(o) = q.pending.get_mut(&op_id) {
                    o.mark_failed(reason.clone(), now);
                }
            }
        });
        crate::storage::record_event(&crate::event::Event::ChainSettlementFailed {
            chain_id: chain,
            op_id,
            reason,
            timestamp: now,
        });
        log!(INFO, "[settlement chain={:?}] op {} tx {} reverted; marked Failed", chain, op_id, tx_hash);
        return;
    }

    // 3. Mined + ok — require finality before confirming.
    let finalized = match evm_rpc::fetch_block_numbers(chain).await {
        Ok((_latest, fin)) => fin,
        Err(e) => {
            log!(INFO, "[settlement chain={:?}] fetch_block_numbers failed confirming op {}: {}; will retry", chain, op_id, e);
            return;
        }
    };
    if block_number > finalized {
        // Mined but not yet final — leave Inflight, retry next tick.
        return;
    }

    // 4. Mint confirm path: read the confirmed on-chain amount from the Mint
    //    log, then finalize through confirm_mint_in_state.
    match &op.kind {
        SettlementOpKind::Mint { vault_id, .. } => {
            let vault_id = *vault_id;

            let contract = read_state(|s| s.multi_chain.chain_contracts.get(&chain).cloned());
            let contract = match contract {
                Some(c) => c,
                None => {
                    log!(INFO, "[settlement chain={:?}] no contract address configured; cannot confirm mint op {}", chain, op_id);
                    return;
                }
            };

            let logs = match evm_rpc::get_logs(chain, &contract, evm_rpc::MINT_EVENT_TOPIC0, block_number, block_number).await {
                Ok(l) => l,
                Err(e) => {
                    log!(INFO, "[settlement chain={:?}] get_logs(mint) failed confirming op {}: {}; will retry", chain, op_id, e);
                    return;
                }
            };

            // Find the Mint log for this vault, preferring the one whose tx hash
            // matches this op's submission (case-insensitive).
            let mut matched: Option<u128> = None;
            for (topics, data, log_tx, log_block) in &logs {
                match evm_rpc::decode_mint_log(topics, data, log_tx, *log_block) {
                    Ok(m) if m.vault_id == vault_id => {
                        let exact = log_tx.eq_ignore_ascii_case(&tx_hash);
                        // Prefer an exact tx-hash match; otherwise take the first
                        // vault-id match as a fallback.
                        if exact {
                            matched = Some(m.amount_e8s);
                            break;
                        } else if matched.is_none() {
                            matched = Some(m.amount_e8s);
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        log!(INFO, "[settlement chain={:?}] decode_mint_log failed confirming op {}: {}", chain, op_id, e);
                    }
                }
            }

            let observed_e8s = match matched {
                Some(a) => a,
                None => {
                    log!(INFO, "[settlement chain={:?}] no Mint log for vault {} in block {} confirming op {}; will retry", chain, vault_id, block_number, op_id);
                    return;
                }
            };

            // PRE-mint total: sum of foreign-chain vault debt BEFORE this mint
            // counts (this vault's debt_e8s is still 0 under Design B). NEVER
            // total_borrowed_icusd_amount (separate ICP-native pool).
            let pre_total = read_state(|s| s.multi_chain.total_chain_vault_debt_e8s());

            let result = mutate_state(|s| {
                confirm_mint_in_state(&mut s.multi_chain, chain, vault_id, observed_e8s, pre_total)
            });

            match result {
                Ok(()) => {
                    mutate_state(|s| {
                        if let Some(q) = s.multi_chain.settlement_queues.get_mut(&chain) {
                            if let Some(o) = q.pending.get_mut(&op_id) {
                                o.mark_succeeded(tx_hash.clone(), now);
                            }
                        }
                    });
                    crate::storage::record_event(&crate::event::Event::ChainMintConfirmed {
                        chain_id: chain,
                        vault_id,
                        op_id,
                        amount_e8s: observed_e8s,
                        tx_hash: tx_hash.clone(),
                        block_number,
                        timestamp: now,
                    });
                    log!(INFO, "[settlement chain={:?}] mint confirmed: op={} vault={} amount_e8s={} block={} tx={}", chain, op_id, vault_id, observed_e8s, block_number, tx_hash);
                    // The op stays in `pending` with status Succeeded. There is
                    // no existing drain/remove helper on SettlementQueueV1 (head
                    // is advanced lazily); leaving it Succeeded is the queue's
                    // current convention and is safe — select_next_op never
                    // re-selects a Succeeded op.
                }
                Err(e) => {
                    // A confirm failure here is a protocol-level condition
                    // (divergence/halt/amount mismatch), NOT a tx failure. Leave
                    // the op Inflight for retry; do NOT mark it Failed.
                    log!(INFO, "[settlement chain={:?}] confirm_mint_in_state FAILED for op {} vault {}: {}; left Inflight", chain, op_id, vault_id, e);
                }
            }
        }
        SettlementOpKind::Withdrawal { .. } => {
            // Task 13: a confirmed withdrawal finalizes the vault close +
            // emits WithdrawalSigned bookkeeping. The mechanism (receipt +
            // finality check above) is in place; the close logic lands there.
            mutate_state(|s| {
                if let Some(q) = s.multi_chain.settlement_queues.get_mut(&chain) {
                    if let Some(o) = q.pending.get_mut(&op_id) {
                        o.mark_succeeded(tx_hash.clone(), now);
                    }
                }
            });
            log!(INFO, "[settlement chain={:?}] withdrawal op {} confirmed tx={} (Task 13 finalizes the close)", chain, op_id, tx_hash);
        }
        SettlementOpKind::Burn { .. } => {
            // Unreachable: a Burn op is marked Failed on the submit path and
            // never goes Inflight. Log defensively rather than panic.
            log!(INFO, "[settlement chain={:?}] inflight Burn op {} reached confirm path unexpectedly", chain, op_id);
        }
    }
}
