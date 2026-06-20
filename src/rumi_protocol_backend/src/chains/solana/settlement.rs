//! Outbound settlement worker for Solana (M2, Task 8).
//!
//! Mirrors `chains::monad::settlement`, pared down for Solana's M2 surface and
//! its durable-nonce idempotency model. The chain-agnostic pure helpers
//! (`select_next_op`, `OpAction`, `confirm_mint_in_state`) are REUSED from the
//! Monad module rather than duplicated; only the async `run_settlement` worker
//! (which talks to the Solana adapter + SOL RPC) is Solana-specific.
//!
//! ## Durable-nonce idempotency (the correctness keystone - avoid double-mint)
//!
//! A durable-nonce transaction advances the nonce EXACTLY ONCE on success, and
//! its signature is DETERMINISTIC from the signed bytes. We therefore NEVER
//! re-sign a "maybe sent" op with a fresh nonce: if the first tx landed, the
//! nonce advanced, and a fresh re-sign would be a DIFFERENT transaction (a
//! second mint). Instead:
//!
//! - **Submit**: the adapter builds + signs the durable-nonce tx, yielding
//!   `raw_tx`. We compute the tx signature LOCALLY from those bytes
//!   (`tx::first_signature_base58`) and store it in `op.last_tx_hash`, then
//!   `mark_inflight`, BEFORE/REGARDLESS of the `send_transaction` outcome.
//!   Whether `send_transaction` returns Ok or Err ("maybe sent"), the op is now
//!   Inflight tracked by its deterministic signature, so the Confirm path
//!   reconciles it via `getTransaction(sig)`. This is the playbook "treat a send
//!   decode-failure as maybe-sent; reconcile, never blind-retry" applied
//!   correctly: we never re-sign, we only confirm by the known signature.
//!
//! - **Stuck-tx resend** (re-broadcasting the SAME stored bytes if the tx never
//!   lands) is a DEFERRED SEAM for M2 (design Section 10). We do NOT build a
//!   resend/replace-by-fee path. An op that never confirms stays Inflight,
//!   pending that seam / an operator. Because the bytes are nonce-pinned, a
//!   future same-bytes resend (and the existing multi-provider broadcast) is
//!   inherently idempotent - re-sending cannot mint twice.
//!
//! ## Borrow-across-await discipline (mirrors run_observer / Monad run_settlement)
//!
//! No `read_state`/`mutate_state` borrow is EVER held across an `.await`. Every
//! value an await depends on is snapshotted OUT of state under a synchronous
//! `read_state`/`mutate_state` closure first; durable state is mutated ONLY after
//! a clean RPC decode (playbook #2). The per-chain re-entrancy RAII guard is a
//! local held across all awaits, so it releases on ANY return path.

use ic_canister_log::log;

use crate::chains::config::ChainId;
use crate::chains::settlement_queue::{SettlementOp, SettlementOpKind, SettlementOpStatus};
use crate::chains::vault::ChainVaultStatus;
use crate::logs::INFO;
use crate::state::{mutate_state, read_state};
use crate::Mode;

// Reuse the chain-agnostic pure helpers from the Monad settlement module (the
// FIFO one-in-flight selector and the supply-invariant mint-confirm). These are
// chain-independent: `select_next_op` scans a `SettlementQueueV1` and
// `confirm_mint_in_state` operates on `MultiChainState`, neither of which is
// Monad-specific.
use crate::chains::monad::settlement::{confirm_mint_in_state, select_next_op, OpAction};

use super::adapter::SolanaAdapter;
use super::{hardening, sol_rpc, ted25519, tx};
use crate::chains::adapter::{ChainAdapter, MintInstruction, WithdrawalRequest};

// ─── Per-chain re-entrancy guard (mirrors monad::settlement) ─────────────────
//
// Once the Task-8 timer runs `run_settlement` at a short interval, a slow RPC
// tick can still be awaiting when the next timer fires, which would spawn a
// SECOND `run_settlement(chain)` concurrently. Both would `select_next_op` the
// SAME op and double-process it (double-submit / double-confirm). This per-chain
// guard ensures only one `run_settlement` per chain runs at a time. The RAII
// guard is a local held across all awaits, so it releases on ANY return path.
//
// Self-healing: the map stores the nanosecond timestamp the guard was acquired
// at. On the IC, a trap in a post-await continuation does NOT run `Drop`, so a
// stale entry would otherwise block that chain forever.
// `hardening::inflight_should_acquire` reclaims entries older than
// `hardening::INFLIGHT_STALE_NS` (10 min), self-healing after a trapped tick.

thread_local! {
    static SOLANA_SETTLEMENT_INFLIGHT: std::cell::RefCell<std::collections::BTreeMap<ChainId, u64>> =
        const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
}

struct SettlementGuard(ChainId);
impl Drop for SettlementGuard {
    fn drop(&mut self) {
        SOLANA_SETTLEMENT_INFLIGHT.with(|s| {
            s.borrow_mut().remove(&self.0);
        });
    }
}

// ─── Async worker ─────────────────────────────────────────────────────────────

/// Run one settlement cycle for the given Solana chain.
///
/// Called by the Task-8 timer dispatch. Acts on at most one op per tick (the one
/// chosen by `select_next_op`):
///
/// - **Submit** (a `Queued` op): build + sign via `SolanaAdapter`, compute the
///   deterministic tx signature LOCALLY, mark the op Inflight with that
///   signature, then broadcast via `sol_rpc::send_transaction`. The Inflight
///   state is set BEFORE the send and is NOT reverted on a send Err (durable-
///   nonce idempotency: the op is reconciled by its known signature).
/// - **Confirm** (an `Inflight` op): `getTransaction(sig)` at finalized; on
///   `Confirmed` a mint is finalized through `confirm_mint_in_state` and a native
///   withdrawal flips its vault `Closing -> Closed`; on `Failed` the op is marked
///   Failed (and a reverted withdrawal restores the reserved collateral);
///   `NotFound` / RPC-error leave the op Inflight for the next tick.
///
/// Borrow discipline mirrors `run_observer`: clone the op out of state for the
/// async RPC calls, then re-acquire a `mutate_state` borrow to write the
/// resulting status back. No `read_state`/`mutate_state` borrow is ever held
/// across an `.await`.
pub async fn run_settlement(chain: ChainId) {
    // Re-entrancy guard (acquired BEFORE any other work): if a tick for this
    // chain is still in flight (and not stale), skip this one entirely. A stale
    // entry (> INFLIGHT_STALE_NS old) means the previous holder trapped in a
    // post-await continuation and its `Drop` never ran; the later tick reclaims
    // it, self-healing the permanent-block scenario.
    let now_ns = ic_cdk::api::time();
    let _guard = match SOLANA_SETTLEMENT_INFLIGHT.with(|s| {
        let existing = s.borrow().get(&chain).copied();
        if hardening::inflight_should_acquire(existing, now_ns, hardening::INFLIGHT_STALE_NS) {
            s.borrow_mut().insert(chain, now_ns);
            Some(SettlementGuard(chain))
        } else {
            None
        }
    }) {
        Some(g) => g,
        None => return, // a fresh tick for this chain is already running; skip
    };

    // Guard: skip if in read-only mode or the supply invariant has halted. (No
    // reorg-halt check: Solana reads at `finalized`, so the reorg circuit breaker
    // does not apply here, exactly as in the Solana observer.)
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

    // Reap terminal (Succeeded/Failed) ops so `pending` does not grow
    // monotonically. Live ops are untouched, so the next tick's `select_next_op`
    // is unaffected; `seen_idempotency_keys` is preserved as the dup guard.
    mutate_state(|s| {
        if let Some(q) = s.multi_chain.settlement_queues.get_mut(&chain) {
            q.prune_terminal();
        }
    });
}

/// Submit path: sign + broadcast a `Queued` op, then mark it `Inflight` with the
/// op's DETERMINISTIC signature.
///
/// The signature is computed LOCALLY from the signed bytes and stored BEFORE the
/// `send_transaction` outcall, so a send Err ("maybe sent") leaves the op
/// Inflight tracked by that signature rather than re-queued for a fresh-nonce
/// re-sign. See the module doc for the durable-nonce idempotency rationale.
async fn submit_op(chain: ChainId, op_id: u64, op: SettlementOp) {
    // A Burn op is never signable in M2 - burns are user-initiated on-chain. Fail
    // it up front (no RPC), mirroring Monad. Task 12: an InterestMint can never be
    // enqueued on a Solana queue (interest harvest is EVM-only), but fail it
    // defensively too so a stray op can never wedge the worker.
    if matches!(op.kind, SettlementOpKind::Burn { .. } | SettlementOpKind::InterestMint { .. }) {
        let now = ic_cdk::api::time();
        let reason = "op not signable on Solana in M2 (burns/interest-mints handled elsewhere)".to_string();
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
        log!(INFO, "[solana settlement chain={:?}] op {} is a Burn; marked Failed (burns are user-initiated in M2)", chain, op_id);
        return;
    }

    // GAS GATE: refuse a new outbound op when the settlement (mint-authority +
    // fee-payer) address lacks enough SOL for fees + a fresh-ATA rent touch.
    // Unlike Monad (which reads a cached balance the observer refreshes), the
    // Solana observer does not populate a balance cache, so we read the balance
    // LIVE here. Derive the settlement address, read its lamports, and gate on
    // `hardening::hot_wallet_ok`. A derive/read failure logs and leaves the op
    // Queued (retry next tick); it does NOT fail the op.
    let settlement_path = ted25519::settlement_derivation_path(chain);
    let (_settlement_pk, settlement_addr) =
        match ted25519::derive_solana_address(settlement_path).await {
            Ok(pair) => pair,
            Err(e) => {
                log!(INFO, "[solana settlement chain={:?}] derive settlement address failed for op {}: {}; will retry", chain, op_id, e);
                return;
            }
        };
    let balance = match sol_rpc::get_balance(&settlement_addr).await {
        Ok(b) => b,
        Err(e) => {
            log!(INFO, "[solana settlement chain={:?}] get_balance failed for op {}: {}; will retry", chain, op_id, e);
            return;
        }
    };
    if !hardening::hot_wallet_ok(balance) {
        let now = ic_cdk::api::time();
        crate::storage::record_event(&crate::event::Event::ChainHotWalletLow {
            chain_id: chain,
            balance_e18: balance as u128, // lamports carried in the e18 field (unit wart)
            threshold_e18: hardening::SOLANA_HOT_WALLET_MIN_LAMPORTS as u128,
            timestamp: now,
        });
        log!(INFO, "[solana settlement chain={:?}] settlement balance {} lamports < threshold {}; skipping submit of op {} (reads continue)", chain, balance, hardening::SOLANA_HOT_WALLET_MIN_LAMPORTS, op_id);
        return;
    }

    // Build + sign via the adapter. The adapter reads the durable nonce, derives
    // its own keys, and returns the signed wire bytes (`raw_tx`); `tx_hash` is
    // left empty by the adapter (we compute the deterministic signature here).
    // NO state borrow is held across these awaits.
    let adapter = SolanaAdapter::new(chain);
    let (raw_tx, vault_id, recipient, amount, kind) = match &op.kind {
        SettlementOpKind::Mint { recipient, amount_e8s, vault_id } => {
            let instr = MintInstruction {
                recipient: recipient.clone(),
                amount_e8s: *amount_e8s,
                vault_id: *vault_id,
                idempotency_key: op.idempotency_key.clone(),
                op_id: op.op_id,
            };
            match adapter.sign_mint(instr).await {
                Ok(signed) => (
                    signed.raw_tx,
                    *vault_id,
                    recipient.clone(),
                    *amount_e8s,
                    SubmitKind::Mint,
                ),
                Err(e) => return handle_adapter_error(chain, op_id, "sign_mint", e),
            }
        }
        SettlementOpKind::NativeWithdrawal { recipient, amount_e18, vault_id } => {
            // `amount_e18` carries SOL lamports here (the e18 field is the shared
            // amount wart); the adapter does the checked u128 -> u64 conversion.
            let req = WithdrawalRequest {
                recipient: recipient.clone(),
                amount_e8s: *amount_e18,
                idempotency_key: op.idempotency_key.clone(),
            };
            match adapter.sign_withdrawal(req).await {
                Ok(signed) => (
                    signed.raw_tx,
                    *vault_id,
                    recipient.clone(),
                    *amount_e18,
                    SubmitKind::NativeWithdrawal,
                ),
                Err(e) => return handle_adapter_error(chain, op_id, "sign_withdrawal", e),
            }
        }
        SettlementOpKind::Burn { .. } | SettlementOpKind::InterestMint { .. } => {
            // Unreachable: handled (marked Failed) at the top of this fn.
            return;
        }
    };

    // Compute the deterministic transaction signature LOCALLY from the signed
    // bytes (it is the tx's first signature, base58-encoded). This is the value
    // `sendTransaction` would return; computing it here lets us track the op by
    // signature regardless of the send outcome. A parse failure means the adapter
    // produced malformed bytes (a bug) - log + leave Queued (do NOT broadcast or
    // mark Inflight without a tracking id).
    let signature = match tx::first_signature_base58(&raw_tx) {
        Ok(sig) => sig,
        Err(e) => {
            log!(INFO, "[solana settlement chain={:?}] first_signature_base58 failed for op {}: {}; will retry", chain, op_id, e);
            return;
        }
    };

    // Mark Inflight + record the signature BEFORE broadcasting. After this point
    // the op is reconciled by `getTransaction(signature)` on the Confirm path,
    // whether or not the send below succeeds. We guard the mutation on the op
    // still being Queued so a rare overlapping tick cannot double-stamp it.
    let now = ic_cdk::api::time();
    let stamped = mutate_state(|s| {
        if let Some(q) = s.multi_chain.settlement_queues.get_mut(&chain) {
            if let Some(o) = q.pending.get_mut(&op_id) {
                if matches!(o.status, SettlementOpStatus::Queued) {
                    o.mark_inflight(now);
                    o.last_tx_hash = Some(signature.clone());
                    return true;
                }
            }
        }
        false
    });
    if !stamped {
        // Another tick already moved this op past Queued; do not broadcast again
        // (our guard plus durable-nonce idempotency keep this safe, but skipping
        // the duplicate send avoids a wasted outcall).
        log!(INFO, "[solana settlement chain={:?}] op {} no longer Queued at stamp time; skipping duplicate submit", chain, op_id);
        return;
    }

    // Emit the submit event (the op is now Inflight regardless of the send).
    match kind {
        SubmitKind::Mint => {
            crate::storage::record_event(&crate::event::Event::ChainMintSubmitted {
                chain_id: chain,
                vault_id,
                op_id,
                recipient,
                amount_e8s: amount,
                tx_hash: signature.clone(),
                timestamp: now,
            });
        }
        SubmitKind::NativeWithdrawal => {
            crate::storage::record_event(&crate::event::Event::WithdrawalSigned {
                chain_id: chain,
                vault_id,
                op_id,
                recipient,
                amount_e18: amount, // lamports (unit wart, mirrors the SettlementOpKind field)
                tx_hash: signature.clone(),
                timestamp: now,
            });
        }
    }

    // Broadcast. The op is ALREADY Inflight; a send Err is "maybe sent" and is
    // logged but does NOT change state (the Confirm path reconciles by the stored
    // signature). Durable-nonce semantics make a future same-bytes resend (a
    // deferred M2 seam) and the multi-provider broadcast idempotent: re-sending
    // the SAME nonce-pinned bytes can never mint twice.
    match sol_rpc::send_transaction(&raw_tx).await {
        Ok(returned_sig) => {
            // Sanity check: the provider's signature should match what we computed
            // locally (both derive from the same signed bytes). A mismatch is not
            // fatal (we track by our local value), but log it loudly.
            if !returned_sig.eq_ignore_ascii_case(&signature) {
                log!(INFO, "[solana settlement chain={:?}] WARNING op {} provider signature {} != local {}", chain, op_id, returned_sig, signature);
            }
            match kind {
                SubmitKind::Mint => log!(INFO, "[solana settlement chain={:?}] mint submitted: op={} vault={} amount_e8s={} sig={}", chain, op_id, vault_id, amount, signature),
                SubmitKind::NativeWithdrawal => log!(INFO, "[solana settlement chain={:?}] withdrawal submitted: op={} vault={} lamports={} sig={}", chain, op_id, vault_id, amount, signature),
            }
        }
        Err(e) => {
            // "Maybe sent": leave the op Inflight (tracked by `signature`); the
            // Confirm path will reconcile via getTransaction. NEVER re-sign with a
            // fresh nonce (that would risk a second mint).
            log!(INFO, "[solana settlement chain={:?}] send_transaction returned Err for op {} (maybe sent): {}; op stays Inflight, will confirm by sig {}", chain, op_id, e, signature);
        }
    }
}

/// Confirm path: check an `Inflight` op's finalized status and finalize on
/// success.
async fn confirm_op(chain: ChainId, op_id: u64, op: SettlementOp) {
    // The submit path always sets last_tx_hash before going Inflight; a None here
    // is defensive (e.g. a manually-poked state) - log and bail.
    let signature = match &op.last_tx_hash {
        Some(h) => h.clone(),
        None => {
            log!(INFO, "[solana settlement chain={:?}] inflight op {} has no last_tx_hash; skipping", chain, op_id);
            return;
        }
    };

    // Look up the tx at `finalized`. NO state borrow across this await.
    let status = match sol_rpc::get_transaction(&signature).await {
        Ok(s) => s,
        Err(e) => {
            // RPC / decode failure: leave Inflight, retry next tick (playbook #2:
            // mutate durable state ONLY after a good decode).
            log!(INFO, "[solana settlement chain={:?}] get_transaction failed for op {} sig {}: {}; will retry", chain, op_id, signature, e);
            return;
        }
    };

    let now = ic_cdk::api::time();
    match status {
        sol_rpc::TxStatus::NotFound => {
            // Not finalized yet (or never landed). Leave Inflight; the next tick
            // re-confirms. A same-bytes resend for a truly-stuck tx is the
            // deferred M2 seam - we do NOT re-broadcast or re-sign here.
            log!(INFO, "[solana settlement chain={:?}] op {} sig {} not finalized yet; leaving Inflight", chain, op_id, signature);
        }
        sol_rpc::TxStatus::Confirmed { slot } => {
            confirm_succeeded(chain, op_id, &op, &signature, slot, now);
        }
        sol_rpc::TxStatus::Failed => {
            confirm_reverted(chain, op_id, &op, &signature, now);
        }
    }
}

/// Finalize a `Confirmed` op (mint or native withdrawal). Mutates durable state
/// ONLY here, after the clean `Confirmed` decode.
fn confirm_succeeded(
    chain: ChainId,
    op_id: u64,
    op: &SettlementOp,
    signature: &str,
    slot: u64,
    now: u64,
) {
    match &op.kind {
        SettlementOpKind::Mint { vault_id, amount_e8s, .. } => {
            let vault_id = *vault_id;
            let amount_e8s = *amount_e8s;
            // PRE-mint total: sum of foreign-chain vault debt BEFORE this mint
            // counts (under Design B this vault's debt_e8s is still 0). NEVER the
            // ICP-native total. The Solana confirm path has no on-chain Mint-log
            // decode (the canister is the sole minter via a deterministic
            // amount); the op's recorded `amount_e8s` IS the observed amount, and
            // `confirm_mint_in_state` re-validates it equals the vault's
            // `pending_mint_e8s` before crediting.
            let pre_total = read_state(|s| s.multi_chain.total_chain_vault_debt_e8s());
            // CAS: credit + mark Succeeded in ONE mutate_state, gated on the op
            // still being Inflight. Mirrors the Monad reverted-path CAS so two
            // overlapping ticks (possible under the 10-min stale-guard reclaim)
            // cannot both credit: the first flips the op to Succeeded, the second
            // sees it non-Inflight and no-ops. `confirm_mint_in_state`'s
            // pending==observed check is the backstop; this makes the guard
            // explicit and symmetric with `confirm_reverted`.
            enum MintConfirm {
                Credited,
                AlreadyHandled,
                Failed(String),
            }
            let outcome = mutate_state(|s| {
                let still_inflight = s
                    .multi_chain
                    .settlement_queues
                    .get(&chain)
                    .and_then(|q| q.pending.get(&op_id))
                    .map(|o| matches!(o.status, SettlementOpStatus::Inflight { .. }))
                    .unwrap_or(false);
                if !still_inflight {
                    return MintConfirm::AlreadyHandled;
                }
                match confirm_mint_in_state(&mut s.multi_chain, chain, vault_id, amount_e8s, pre_total, now) {
                    Ok(()) => {
                        if let Some(q) = s.multi_chain.settlement_queues.get_mut(&chain) {
                            if let Some(o) = q.pending.get_mut(&op_id) {
                                o.mark_succeeded(signature.to_string(), now);
                            }
                        }
                        MintConfirm::Credited
                    }
                    Err(e) => MintConfirm::Failed(e),
                }
            });
            match outcome {
                MintConfirm::Credited => {
                    crate::storage::record_event(&crate::event::Event::ChainMintConfirmed {
                        chain_id: chain,
                        vault_id,
                        op_id,
                        amount_e8s,
                        tx_hash: signature.to_string(),
                        block_number: slot, // Solana slot fills the block_number field
                        timestamp: now,
                    });
                    log!(INFO, "[solana settlement chain={:?}] mint confirmed: op={} vault={} amount_e8s={} slot={} sig={}", chain, op_id, vault_id, amount_e8s, slot, signature);
                }
                MintConfirm::AlreadyHandled => {
                    log!(INFO, "[solana settlement chain={:?}] op {} already finalized by a concurrent tick; skipping double-credit", chain, op_id);
                }
                MintConfirm::Failed(e) => {
                    // A confirm failure here is a protocol-level condition
                    // (divergence/halt/amount mismatch), NOT a tx failure. Leave
                    // the op Inflight for retry; do NOT mark it Failed.
                    log!(INFO, "[solana settlement chain={:?}] confirm_mint_in_state FAILED for op {} vault {}: {}; left Inflight", chain, op_id, vault_id, e);
                }
            }
        }
        SettlementOpKind::NativeWithdrawal { vault_id, .. } => {
            // A confirmed (finalized + successful) native SOL transfer-out: the
            // collateral has been paid out from the settlement hot wallet. Mark
            // the op Succeeded, then if the vault is `Closing` (a full withdrawal /
            // close) flip it to `Closed`. A partial withdrawal leaves the vault
            // `Open` (it still holds collateral); nothing extra to do.
            let vid = *vault_id;
            mutate_state(|s| {
                if let Some(q) = s.multi_chain.settlement_queues.get_mut(&chain) {
                    if let Some(o) = q.pending.get_mut(&op_id) {
                        o.mark_succeeded(signature.to_string(), now);
                    }
                }
                if let Some(v) = s.multi_chain.chain_vaults.get_mut(&vid) {
                    if v.status == ChainVaultStatus::Closing {
                        v.status = ChainVaultStatus::Closed;
                    }
                }
            });
            log!(INFO, "[solana settlement chain={:?}] withdrawal op {} vault {} confirmed slot={} sig={} (Closing->Closed if applicable)", chain, op_id, vid, slot, signature);
        }
        SettlementOpKind::Burn { .. } | SettlementOpKind::InterestMint { .. } => {
            // Unreachable: Burn/InterestMint ops are marked Failed on the submit
            // path and never go Inflight. Log defensively rather than panic.
            log!(INFO, "[solana settlement chain={:?}] inflight non-signable op {} reached confirm path unexpectedly", chain, op_id);
        }
    }
}

/// Handle a `Failed` (on-chain reverted) op. Marks the op Failed (NO auto-retry,
/// per the project no-retries rule) and, for a reverted native withdrawal,
/// RESTORES the reserved collateral to the vault so a failed payout does not
/// strand the user's collateral. Mirrors Monad's restore-on-revert.
///
/// The state reversal + `mark_failed` run in a SINGLE `mutate_state` guarded on
/// the op still being `Inflight` (compare-and-swap), so two overlapping ticks
/// cannot double-restore collateral (a phantom `2 × amount` credit). The event +
/// log are gated on that CAS so a rare double-tick does not emit a duplicate
/// failure event.
fn confirm_reverted(chain: ChainId, op_id: u64, op: &SettlementOp, signature: &str, now: u64) {
    let reason = "tx reverted on-chain".to_string();
    let did_revert = mutate_state(|s| {
        // CAS: only the first tick to observe this op Inflight does the work.
        let still_inflight = s
            .multi_chain
            .settlement_queues
            .get(&chain)
            .and_then(|q| q.pending.get(&op_id))
            .map(|o| matches!(o.status, SettlementOpStatus::Inflight { .. }))
            .unwrap_or(false);
        if !still_inflight {
            return false;
        }
        match &op.kind {
            SettlementOpKind::Mint { vault_id, .. } => {
                // Design B: no debt was counted, so NO supply reversal. Clear the
                // vault's pending mint (the mint will not happen). Do NOT change
                // status - the vault is left MintPending with a Failed op + a
                // ChainSettlementFailed event for the failed-mint resolution path
                // to act on (mirrors Monad: stamping Closed here would mislabel a
                // vault that still holds deposited collateral). Logged loudly below.
                if let Some(v) = s.multi_chain.chain_vaults.get_mut(vault_id) {
                    v.pending_mint_e8s = 0;
                }
            }
            SettlementOpKind::NativeWithdrawal { vault_id, amount_e18, .. } => {
                // The transfer did not happen, so the reserved collateral was not
                // paid out. ADD it back (undo the reserve-at-enqueue) and, if the
                // vault had gone `Closing` (full withdrawal / close), revert it to
                // `Open` - it is no longer empty. Never touches debt/supply.
                if let Some(v) = s.multi_chain.chain_vaults.get_mut(vault_id) {
                    v.collateral_amount_native =
                        v.collateral_amount_native.saturating_add(*amount_e18);
                    if v.status == ChainVaultStatus::Closing {
                        v.status = ChainVaultStatus::Open;
                    }
                }
            }
            SettlementOpKind::Burn { .. } | SettlementOpKind::InterestMint { .. } => {}
        }
        if let Some(o) = s
            .multi_chain
            .settlement_queues
            .get_mut(&chain)
            .and_then(|q| q.pending.get_mut(&op_id))
        {
            o.mark_failed(reason.clone(), now);
        }
        true
    });

    if did_revert {
        crate::storage::record_event(&crate::event::Event::ChainSettlementFailed {
            chain_id: chain,
            op_id,
            reason,
            timestamp: now,
        });
        match &op.kind {
            SettlementOpKind::Mint { vault_id, .. } => {
                log!(INFO, "[solana settlement chain={:?}] MINT op {} vault {} REVERTED on-chain (sig {}); marked Failed, pending_mint cleared, vault left MintPending for manual resolution", chain, op_id, vault_id, signature);
            }
            SettlementOpKind::NativeWithdrawal { vault_id, amount_e18, .. } => {
                log!(INFO, "[solana settlement chain={:?}] withdrawal op {} vault {} reverted on-chain (sig {}); marked Failed, restored {} lamports of reserved collateral", chain, op_id, vault_id, signature, amount_e18);
            }
            SettlementOpKind::Burn { .. } | SettlementOpKind::InterestMint { .. } => {}
        }
    }
}

/// What kind of settlement tx a submit is building, so the submit path emits the
/// right event after marking the op Inflight.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SubmitKind {
    Mint,
    NativeWithdrawal,
}

/// Map a `ChainAdapterError` from the adapter's sign path to a log + state
/// decision. A permanent `InvalidPayload` (malformed address / oversized amount)
/// can never succeed on a retry, so the op is marked Failed (mirroring Monad's
/// judgment); any other error (signature subnet hiccup, RPC failure reading the
/// nonce) is transient - log and leave the op Queued to retry next tick.
fn handle_adapter_error(
    chain: ChainId,
    op_id: u64,
    what: &str,
    e: crate::chains::adapter::ChainAdapterError,
) {
    use crate::chains::adapter::ChainAdapterError;
    match e {
        ChainAdapterError::InvalidPayload(msg) => {
            let now = ic_cdk::api::time();
            let reason = format!("{what}: invalid payload: {msg}");
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
                reason: reason.clone(),
                timestamp: now,
            });
            log!(INFO, "[solana settlement chain={:?}] {} returned permanent InvalidPayload for op {}: {}; marked Failed", chain, what, op_id, reason);
        }
        other => {
            log!(INFO, "[solana settlement chain={:?}] {} failed (transient) for op {}: {:?}; will retry", chain, what, op_id, other);
        }
    }
}
