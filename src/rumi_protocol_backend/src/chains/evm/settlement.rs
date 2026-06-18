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
//! tick (Timer D, wired in Task 15). It mirrors `run_observer`'s read → await
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

use crate::chains::config::{ChainId, ChainStatus};
use crate::chains::monad::chain_vault::ChainVaultStatus;
use crate::chains::multi_chain_state::MultiChainStateV4;
use crate::chains::settlement_queue::{SettlementOpKind, SettlementOpStatus, SettlementQueueV1};
use crate::chains::supply::{apply_supply_delta, SupplyDelta};
use crate::logs::INFO;
use crate::state::{mutate_state, read_state};
use crate::Mode;

use super::{evm_rpc, hardening, tecdsa, tx};

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
    state: &mut MultiChainStateV4,
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

/// Task 12 (Option B): confirm an on-chain interest mint — grow the REAL vault's
/// `debt_e8s` and the chain supply by `observed_e8s` TOGETHER (invariant exact),
/// advance `last_interest_accrual_ns` to the harvest snapshot, and clear the
/// pending. `observed_e8s` (from the on-chain Mint log) must equal the vault's
/// `pending_interest_mint_e8s`. `pre_total` is the PRE-mint
/// `total_chain_vault_debt_e8s()`. STUB — real body in GREEN.
pub fn confirm_interest_mint_in_state(
    state: &mut MultiChainStateV4,
    chain: ChainId,
    vault_id: u64,
    observed_e8s: u128,
    accrual_through_ns: u64,
    pre_total: u128,
) -> Result<(), String> {
    // Step 1: validate (read-only — no mutation on failure).
    {
        let v = state
            .chain_vaults
            .get(&vault_id)
            .ok_or_else(|| format!("confirm_interest_mint: unknown vault {vault_id}"))?;
        if v.pending_interest_mint_e8s != observed_e8s {
            return Err(format!(
                "confirm_interest_mint: observed {} != pending {}",
                observed_e8s, v.pending_interest_mint_e8s
            ));
        }
    }

    // Step 2: supply delta. Debt and supply grow together by exactly the minted
    // amount, so the foreign-chain invariant stays exact. Rejects (no mutation)
    // on divergence/halt.
    let post_total = pre_total.saturating_add(observed_e8s);
    apply_supply_delta(state, chain, SupplyDelta::Increase(observed_e8s), post_total)
        .map_err(|e| format!("{e:?}"))?;

    // Step 3: only reached when the supply delta succeeded. Realize the interest
    // into debt and advance the accrual window to the harvest snapshot time (NOT
    // confirm time), so the harvest->confirm sliver accrues next round.
    let v = state
        .chain_vaults
        .get_mut(&vault_id)
        .expect("vault present: checked above");
    v.debt_e8s = v.debt_e8s.saturating_add(observed_e8s);
    v.pending_interest_mint_e8s = 0;
    v.last_interest_accrual_ns = accrual_through_ns;
    Ok(())
}

// ─── Timer D tick (fan-out) ─────────────────────────────────────────────────

/// Timer D entry point: run one settlement cycle for every registered+enabled
/// chain. The per-chain `run_settlement` carries its own mode/halt/re-entrancy
/// guards, so this fan-out just snapshots the chain-id list and calls each in
/// turn. NO state borrow is held across the awaits — the chain-id Vec is cloned
/// out of state up front.
///
/// No-op when no chain is registered (the Vec is empty), so it is safe to
/// register on the staging canister before Monad is configured (Task 15 PocketIC
/// smoke test asserts this).
///
/// SUPERSEDED (M2 Task 8): the live settlement timer now calls the chain-kind
/// dispatcher `main::run_all_settlements`, which calls `run_settlement(chain)`
/// directly per registered chain (Monad always, Solana when enabled). This
/// Monad-only fan-out is retained for any direct caller but is no longer on the
/// timer path; behavior is identical for Monad chains.
pub async fn settlement_tick() {
    let chains: Vec<ChainId> = read_state(|s| {
        s.multi_chain
            .chain_configs
            .iter()
            .filter(|(_, c)| matches!(c.status, ChainStatus::Registered))
            .map(|(id, _)| *id)
            .collect()
    });
    for chain in chains {
        run_settlement(chain).await;
    }
}

// ─── Per-chain re-entrancy guard (Task 13 review; wired Task 15) ───────────────
//
// Once Timer D runs at a short interval, a slow RPC tick can still be awaiting
// when the next timer fires, which would spawn a SECOND `run_settlement(chain)`
// concurrently. Both would `select_next_op` the SAME op and double-process it
// (double-submit -> potential double-mint; double-confirm). This per-chain guard
// ensures only one `run_settlement` per chain runs at a time. The RAII guard is
// a local held across all awaits, so it releases when the async fn returns on
// ANY path (success, early return, trap-unwind).
//
// Self-healing (B2 hardening): the map stores the nanosecond timestamp the
// guard was acquired at. On the IC, a trap in a post-await continuation does
// NOT run `Drop`, so a stale entry would otherwise block that chain forever.
// `hardening::inflight_should_acquire` reclaims entries older than
// `hardening::INFLIGHT_STALE_NS` (10 min), self-healing after a trapped tick.

thread_local! {
    static SETTLEMENT_INFLIGHT: std::cell::RefCell<std::collections::BTreeMap<ChainId, u64>> =
        const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
}

struct SettlementGuard(ChainId);
impl Drop for SettlementGuard {
    fn drop(&mut self) {
        SETTLEMENT_INFLIGHT.with(|s| {
            s.borrow_mut().remove(&self.0);
        });
    }
}

// ─── Async worker ─────────────────────────────────────────────────────────────

/// Run one settlement cycle for the given chain.
///
/// Called by Timer D (configured in Task 15). Acts on at most one op per tick
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
    // Re-entrancy guard (acquired BEFORE any other work): if a tick for this
    // chain is still in flight (and not stale), skip this one entirely. The
    // RAII guard releases on the future completing (any return path). A stale
    // entry (> INFLIGHT_STALE_NS old) means the previous holder trapped in a
    // post-await continuation and its `Drop` never ran — the later tick
    // reclaims it, self-healing the permanent-block scenario.
    let now_ns = ic_cdk::api::time();
    let _guard = match SETTLEMENT_INFLIGHT.with(|s| {
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

    // Guard: skip if in read-only mode, the supply invariant has halted, or this
    // chain is reorg-halted (Task 11). A reorg-halted chain stops BOTH the
    // observer and the settlement worker until `clear_reorg_halt` (Task 14).
    let should_skip = read_state(|s| {
        s.mode == Mode::ReadOnly
            || s.multi_chain.invariant_halted
            || s.multi_chain.reorg_halted.get(&chain).copied().unwrap_or(false)
    });
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
    // monotonically (Task-10 review follow-up). Live ops are untouched, so the
    // next tick's `select_next_op` is unaffected; `seen_idempotency_keys` is
    // preserved as the dup guard.
    mutate_state(|s| {
        if let Some(q) = s.multi_chain.settlement_queues.get_mut(&chain) {
            q.prune_terminal();
        }
    });
}

/// What kind of settlement tx a `TxPlan` carries, so the submit path can emit
/// the right event (`ChainMintSubmitted` vs `WithdrawalSigned`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum TxPlanKind {
    Mint,
    NativeWithdrawal,
    /// Task 12: an interest-realization mint (to the treasury). Submit-path logs
    /// it; the authoritative record is `ChainInterestMinted` on confirm.
    InterestMint,
}

/// Per-op-kind tx shape mirroring `MonadAdapter`'s choices, so the submit and
/// the stuck-tx resubmit paths build identical transactions (only nonce + fees
/// differ on a resubmit). `vault_id`/`recipient`/`amount` are the values the
/// submit path needs for the `ChainMintSubmitted` / `WithdrawalSigned` event;
/// `amount` is e8s for a mint and e18 (wei) for a native withdrawal.
struct TxPlan {
    fields: tx::Eip1559Fields,
    vault_id: u64,
    recipient: String,
    amount: u128,
    kind: TxPlanKind,
}

/// Build the EIP-1559 fields for a settlement op at an EXPLICIT nonce + fees.
///
/// Mirrors `MonadAdapter::sign_mint`/`sign_withdrawal` exactly:
/// - Mint: `to` = icUSD contract, `value` = 0, calldata =
///   `encode_mint_calldata`, gas_limit 120_000.
/// - NativeWithdrawal: `to` = recipient, `value` = `amount_e18` (wei), empty
///   data, gas_limit 21_000; the plan carries the op's real `vault_id` so the
///   submit path can emit `WithdrawalSigned` and the confirm path can finalize
///   the close.
/// - Burn: never signed in Phase 1b — returns `Err`.
///
/// The caller supplies `prio`/`max_fee` (a submit uses the live estimate; a
/// resubmit uses the bumped values) and the contract address for mints.
fn build_tx_plan(
    chain: ChainId,
    kind: &SettlementOpKind,
    nonce: u64,
    prio: u128,
    max_fee: u128,
    contract: Option<&str>,
    // For NativeWithdrawal only: the value to actually send, already capped so
    // the custody signer can also pay gas (see `fundable_withdrawal_value`).
    // `None` (or a Mint op) sends the op's full declared amount.
    withdrawal_value: Option<u128>,
) -> Result<TxPlan, String> {
    match kind {
        SettlementOpKind::Mint { recipient, amount_e8s, vault_id } => {
            let contract = contract.ok_or_else(|| "icUSD contract not set".to_string())?;
            // Delegate the per-op-kind field shape to the single source of truth.
            let fields = tx::build_eip1559_fields(
                chain.0 as u64,
                tx::MonadTxKind::Mint {
                    contract,
                    recipient,
                    amount_e8s: *amount_e8s,
                    vault_id: *vault_id,
                },
                nonce,
                prio,
                max_fee,
            )?;
            Ok(TxPlan {
                fields,
                vault_id: *vault_id,
                recipient: recipient.clone(),
                amount: *amount_e8s,
                kind: TxPlanKind::Mint,
            })
        }
        SettlementOpKind::NativeWithdrawal { recipient, amount_e18, vault_id } => {
            // `value` is wei (1e18 scale) — it goes straight into the EIP-1559
            // `value` of a native transfer. The withdrawal is signed by the
            // vault's own custody address, so `withdrawal_value` is the
            // gas-netted amount (full `amount_e18` for a partial withdrawal that
            // leaves a buffer; `custody_balance - gas` for a full close).
            let value = withdrawal_value.unwrap_or(*amount_e18);
            let fields = tx::build_eip1559_fields(
                chain.0 as u64,
                tx::MonadTxKind::NativeWithdrawal { recipient, amount_wei: value },
                nonce,
                prio,
                max_fee,
            )?;
            Ok(TxPlan {
                fields,
                vault_id: *vault_id,
                recipient: recipient.clone(),
                amount: value,
                kind: TxPlanKind::NativeWithdrawal,
            })
        }
        SettlementOpKind::InterestMint { vault_id, mint_id, amount_e8s, recipient, .. } => {
            // Task 12: identical IcUSD.mint calldata to a normal Mint, but the
            // on-chain `vault_id` arg is the SYNTHETIC `mint_id` (the real vault
            // already minted once at open and IcUSD.mint reverts a repeat id),
            // and the calldata `to:` is the interest-treasury `recipient`. Signed
            // by the minter (resolve_op_signer). The `TxPlan.vault_id` carries the
            // REAL vault for event attribution.
            let contract = contract.ok_or_else(|| "icUSD contract not set".to_string())?;
            let fields = tx::build_eip1559_fields(
                chain.0 as u64,
                tx::MonadTxKind::Mint {
                    contract,
                    recipient,
                    amount_e8s: *amount_e8s,
                    vault_id: *mint_id,
                },
                nonce,
                prio,
                max_fee,
            )?;
            Ok(TxPlan {
                fields,
                vault_id: *vault_id,
                recipient: recipient.clone(),
                amount: *amount_e8s,
                kind: TxPlanKind::InterestMint,
            })
        }
        SettlementOpKind::Burn { .. } => Err("burn not signable in Phase 1b".to_string()),
    }
}

/// Resolve the `(derivation_path, from_address)` that signs an op's tx.
///
/// - **Mint** → the per-chain settlement (minter) hot wallet. It only ever pays
///   icUSD-mint gas (tiny), never user collateral.
/// - **NativeWithdrawal** → the VAULT'S OWN per-vault custody address, which
///   holds the deposited collateral. Collateral is paid back out of the same
///   address it was deposited to — never commingled into the hot wallet — so
///   there is no custody-sweep dependency. The custody path is re-derived from
///   the vault's `(chain, owner, vault_id)` exactly as `open_chain_vault` derived
///   it; the stored `custody_address` is the matching signer address, so no
///   extra tECDSA derive is needed here.
async fn resolve_op_signer(
    chain: ChainId,
    kind: &SettlementOpKind,
) -> Result<(Vec<Vec<u8>>, String), String> {
    match kind {
        SettlementOpKind::Mint { .. } => tecdsa::cached_settlement_address(chain).await,
        // Task 12: interest mints are signed by the minter, same as a vault mint.
        SettlementOpKind::InterestMint { .. } => tecdsa::cached_settlement_address(chain).await,
        SettlementOpKind::NativeWithdrawal { vault_id, .. } => {
            let vid = *vault_id;
            let info = read_state(|s| {
                s.multi_chain
                    .chain_vaults
                    .get(&vid)
                    .map(|v| (v.owner, v.custody_address.clone()))
            });
            let (owner, custody_addr) =
                info.ok_or_else(|| format!("withdrawal signer: unknown vault {vid}"))?;
            // The op's `chain` IS the vault's collateral chain (settlement queues
            // are keyed by chain), so re-derive the custody path on it.
            let path = tecdsa::custody_derivation_path(chain, owner, vid);
            Ok((path, custody_addr))
        }
        SettlementOpKind::Burn { .. } => Err("burn not signable in Phase 1b".to_string()),
    }
}

/// The native value to actually send for a withdrawal, capped so the custody
/// signer can ALSO pay its own gas. For a full close the requested amount equals
/// the entire custody balance, so the worst-case gas (`gas_limit * max_fee`) is
/// netted out — the withdrawer bears their own gas, as on any native chain, and
/// a tiny dust (`max_fee - actual_fee`) is left behind. A partial withdrawal
/// that leaves a buffer sends the full requested amount.
pub(crate) fn fundable_withdrawal_value(amount_e18: u128, custody_balance: u128, max_fee: u128) -> u128 {
    let gas_reserve = (tx::NATIVE_WITHDRAWAL_GAS_LIMIT as u128).saturating_mul(max_fee);
    amount_e18.min(custody_balance.saturating_sub(gas_reserve))
}

/// Submit path: sign + broadcast a `Queued` op, then mark it `Inflight`.
///
/// Approach A (Task 11): the worker fetches the nonce itself and builds/signs
/// the tx directly via `build_tx_plan` + `tx::sign_eip1559` (NOT through the
/// adapter), so it KNOWS the exact nonce used and can store it on the op. The
/// stuck-tx resubmit (`confirm_op`) re-signs on this stored nonce, making a
/// bumped-gas resubmit a true replace-by-fee rather than a second mint.
async fn submit_op(
    chain: ChainId,
    op_id: u64,
    op: crate::chains::settlement_queue::SettlementOp,
) {
    // A Burn op is never signable in Phase 1b — fail it up front (no RPC).
    if matches!(op.kind, SettlementOpKind::Burn { .. }) {
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

    // DEPOSIT FINALITY GATE (Mint ops only): the deposit-watch flips a vault to
    // MintPending on a `"latest"` (chain-tip, depth-0) custody balance for
    // liveness (the Gate-4 design keeps DETECTION robust to block-height read
    // failures). But an icUSD mint is irreversible, so before BROADCASTING the
    // mint we RE-VERIFY the custody balance still covers the declared collateral
    // at the observer's finalized cursor (`last_observed_block`, which the
    // observer advances only to confirmed-final blocks — the same value the mint
    // CONFIRM path treats as final). If the deposit cannot be shown final yet
    // (cursor unseeded, balance not yet reflected at the cursor, or the read
    // fails) we DEFER: leave the op Queued and retry next tick. This is
    // fail-closed — a deposit that reorgs out after the tip observation can never
    // back a mint. (Withdrawals/Burns are unaffected.)
    if let SettlementOpKind::Mint { vault_id, .. } = &op.kind {
        let vid = *vault_id;
        let (cursor, vinfo) = read_state(|s| {
            let cursor = s.multi_chain.last_observed_block.get(&chain).copied().unwrap_or(0);
            let v = s
                .multi_chain
                .chain_vaults
                .get(&vid)
                .map(|v| (v.custody_address.clone(), v.collateral_amount_native));
            (cursor, v)
        });
        let (custody, declared) = match vinfo {
            Some(p) => p,
            None => {
                log!(INFO, "[settlement chain={:?}] mint op {}: unknown vault {}; will retry", chain, op_id, vid);
                return;
            }
        };
        if cursor == 0 {
            log!(INFO, "[settlement chain={:?}] mint op {} vault {}: finalized cursor unseeded; cannot verify deposit finality — call set_last_observed_block(chain, <current tip>) to enable mints", chain, op_id, vid);
            return;
        }
        match evm_rpc::get_balance_at_block(chain, &custody, cursor).await {
            Ok(final_balance) if final_balance >= declared => {
                log!(INFO, "[settlement chain={:?}] mint op {} vault {}: deposit verified final ({} >= declared {} at block {})", chain, op_id, vid, final_balance, declared, cursor);
            }
            Ok(final_balance) => {
                log!(INFO, "[settlement chain={:?}] mint op {} vault {}: deposit not yet final (finalized balance {} < declared {} at block {}); deferring broadcast", chain, op_id, vid, final_balance, declared, cursor);
                return;
            }
            Err(e) => {
                log!(INFO, "[settlement chain={:?}] mint op {} vault {}: get_balance_at_block failed ({}); deferring broadcast", chain, op_id, vid, e);
                return;
            }
        }
    }

    // GAS GATE (Task 11): MINTS ONLY. A mint is paid by the per-chain settlement
    // hot wallet, so refuse a new mint when the cached settlement balance is
    // below the hot-wallet floor. FAIL OPEN when the cache is unset (`None`): an
    // unpopulated cache (fresh chain / observer hasn't run yet) must NEVER block
    // a legitimate mint. The observer refreshes the cache each tick
    // (deposit_watch::refresh_hot_wallet_balance). Native WITHDRAWALS are signed
    // by the vault's own custody address (which holds the collateral) and net
    // their gas out of the transfer, so the settlement-wallet floor is irrelevant
    // to them — never gate a withdrawal on it.
    // Task 12: interest mints are ALSO paid by the settlement hot wallet, so they
    // are gated on its balance too (a vault open mint and an interest mint cost
    // the same gas).
    if matches!(op.kind, SettlementOpKind::Mint { .. } | SettlementOpKind::InterestMint { .. }) {
        let cached = read_state(|s| s.multi_chain.hot_wallet_balance_e18.get(&chain).copied());
        if let Some(bal) = cached {
            if !hardening::hot_wallet_ok(bal) {
                let now = ic_cdk::api::time();
                crate::storage::record_event(&crate::event::Event::ChainHotWalletLow {
                    chain_id: chain,
                    balance_e18: bal,
                    threshold_e18: hardening::HOT_WALLET_MIN_E18,
                    timestamp: now,
                });
                log!(INFO, "[settlement chain={:?}] hot-wallet balance {} e18 < threshold {} e18; skipping submit of op {} (reads/observer continue)", chain, bal, hardening::HOT_WALLET_MIN_E18, op_id);
                return;
            }
        }
    }

    // 1. Resolve the signer: mints are signed by the per-chain settlement
    //    (minter) hot wallet; native withdrawals by the vault's own custody
    //    address. Returns the derivation PATH too, which the signer needs.
    let (path, signer_addr) = match resolve_op_signer(chain, &op.kind).await {
        Ok(pair) => pair,
        Err(e) => {
            log!(INFO, "[settlement chain={:?}] resolve_op_signer failed for op {}: {}; will retry", chain, op_id, e);
            return;
        }
    };

    // 2. Fetch the nonce ("latest") for the SIGNER — we store it on the op so a
    //    stuck-tx resubmit can replace-by-fee on the SAME nonce.
    let nonce = match evm_rpc::get_transaction_count(chain, &signer_addr).await {
        Ok(n) => n,
        Err(e) => {
            log!(INFO, "[settlement chain={:?}] get_transaction_count failed for op {}: {}; will retry", chain, op_id, e);
            return;
        }
    };

    // 3. Fetch fee estimate; max_fee mirrors the adapter (2*base + prio).
    let (base_fee, prio) = match evm_rpc::fetch_fees(chain).await {
        Ok(pair) => pair,
        Err(e) => {
            log!(INFO, "[settlement chain={:?}] fetch_fees failed for op {}: {}; will retry", chain, op_id, e);
            return;
        }
    };
    let max_fee = base_fee.saturating_mul(2).saturating_add(prio);

    // 4. For a native withdrawal, cap the transfer value so the custody signer
    //    can also pay gas (a full close requests the entire custody balance).
    //    Read the custody balance ONCE here; the value is recomputed identically
    //    on a stuck-tx resubmit. Mints don't carry a value, so skip the read.
    let withdrawal_value = if matches!(op.kind, SettlementOpKind::NativeWithdrawal { .. }) {
        match evm_rpc::get_balance(chain, &signer_addr).await {
            Ok(bal) => {
                if let SettlementOpKind::NativeWithdrawal { amount_e18, .. } = &op.kind {
                    Some(fundable_withdrawal_value(*amount_e18, bal, max_fee))
                } else {
                    None
                }
            }
            Err(e) => {
                log!(INFO, "[settlement chain={:?}] get_balance(custody) failed for op {}: {}; will retry", chain, op_id, e);
                return;
            }
        }
    } else {
        None
    };

    // 5. Resolve the contract (mints only) and build the tx plan.
    let contract = read_state(|s| s.multi_chain.chain_contracts.get(&chain).cloned());
    let plan = match build_tx_plan(chain, &op.kind, nonce, prio, max_fee, contract.as_deref(), withdrawal_value) {
        Ok(p) => p,
        Err(e) => {
            log!(INFO, "[settlement chain={:?}] build_tx_plan failed for op {}: {}; will retry", chain, op_id, e);
            return;
        }
    };
    let TxPlan { fields, vault_id, recipient, amount, kind } = plan;

    // 6. Sign with the resolved signer (settlement for mints, custody for withdrawals).
    let raw_hex = match tx::sign_eip1559(&fields, path, &signer_addr).await {
        Ok(h) => h,
        Err(e) => {
            log!(INFO, "[settlement chain={:?}] sign_eip1559 failed for op {}: {}; will retry", chain, op_id, e);
            return;
        }
    };

    // 7. Broadcast. A transient send error is logged and retried next tick — we
    //    do NOT mark the op Failed (it may yet be re-signable / re-broadcastable).
    //
    //    ON-CHAIN DOUBLE-MINT DEPENDENCY: if send_raw_transaction returns Err but
    //    the tx actually landed (an RPC false negative), this op stays Queued
    //    with no submit_nonce recorded, so the next tick re-reads "latest" and
    //    signs a NEW tx at nonce+1 — a genuine second on-chain mint. The
    //    canister's supply accounting stays correct (confirm requires
    //    observed_e8s == pending_mint_e8s and credits exactly once), but on-chain
    //    icUSD could be minted twice. Protection lives OUTSIDE this function:
    //    (a) IcUSD.mint MUST guard per vault_id (Task 19: `mapping(uint64 => bool)
    //    minted`, revert on repeat — asserted by a Task 20 test), and (b) once an
    //    op IS Inflight, Task-11's stuck-tx path resubmits on the SAME stored
    //    nonce (true replace-by-fee). The unguarded window is only the
    //    transient-error-before-Inflight case.
    let tx_hash = match evm_rpc::send_raw_transaction(chain, &raw_hex).await {
        Ok(h) => h,
        Err(e) => {
            log!(INFO, "[settlement chain={:?}] send_raw_transaction failed for op {}: {}; will retry", chain, op_id, e);
            return;
        }
    };

    // 8. Mark Inflight + record the tx hash AND the submit nonce. Emit
    //    ChainMintSubmitted for mints.
    let now = ic_cdk::api::time();
    mutate_state(|s| {
        if let Some(q) = s.multi_chain.settlement_queues.get_mut(&chain) {
            if let Some(o) = q.pending.get_mut(&op_id) {
                o.mark_inflight(now);
                o.last_tx_hash = Some(tx_hash.clone());
                o.submit_nonce = Some(nonce);
            }
        }
    });

    match kind {
        TxPlanKind::Mint => {
            crate::storage::record_event(&crate::event::Event::ChainMintSubmitted {
                chain_id: chain,
                vault_id,
                op_id,
                recipient,
                amount_e8s: amount,
                tx_hash: tx_hash.clone(),
                timestamp: now,
            });
            log!(INFO, "[settlement chain={:?}] mint submitted: op={} vault={} amount_e8s={} tx={}", chain, op_id, vault_id, amount, tx_hash);
        }
        TxPlanKind::NativeWithdrawal => {
            crate::storage::record_event(&crate::event::Event::WithdrawalSigned {
                chain_id: chain,
                vault_id,
                op_id,
                recipient,
                amount_e18: amount,
                tx_hash: tx_hash.clone(),
                timestamp: now,
            });
            log!(INFO, "[settlement chain={:?}] withdrawal submitted: op={} vault={} amount_e18={} tx={}", chain, op_id, vault_id, amount, tx_hash);
        }
        TxPlanKind::InterestMint => {
            // Submit-path log only; the authoritative event is ChainInterestMinted
            // on confirm (after the on-chain mint is observed at finality).
            log!(INFO, "[settlement chain={:?}] interest mint submitted: op={} vault={} amount_e8s={} treasury={} tx={}", chain, op_id, vault_id, amount, recipient, tx_hash);
        }
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
            // Not mined yet. ADVANCE `tries` on EVERY not-mined tick (the prior
            // code only advanced on submit / successful-resubmit, so `is_stuck`
            // (>= 2) was unreachable and replace-by-fee never fired). The pure
            // `on_not_mined_tick` decides the new tries count and whether to
            // replace-by-fee THIS tick (only when stuck AND we stored the submit
            // nonce — resubmitting without it would risk a fresh-nonce 2nd mint).
            let tries = match &op.status {
                SettlementOpStatus::Inflight { tries, .. } => *tries,
                _ => {
                    // Defensive: confirm_op is only entered for an Inflight op.
                    log!(INFO, "[settlement chain={:?}] confirm op {} not Inflight on not-mined tick; skipping", chain, op_id);
                    return;
                }
            };
            let finality_depth = read_state(|s| {
                s.multi_chain.chain_configs.get(&chain).map(|c| c.finality_depth)
            })
            .unwrap_or(1);
            let has_nonce = op.submit_nonce.is_some();
            let (new_tries, do_resubmit) =
                hardening::on_not_mined_tick(tries, finality_depth, has_nonce);

            // Persist the advanced tries directly onto the Inflight status so the
            // stuck threshold is actually reachable across ticks. Keep
            // last_attempt_ns as-is here (a non-resubmit tick is not an attempt);
            // the resubmit path updates last_tx_hash on a successful rebroadcast.
            mutate_state(|s| {
                if let Some(q) = s.multi_chain.settlement_queues.get_mut(&chain) {
                    if let Some(o) = q.pending.get_mut(&op_id) {
                        if let SettlementOpStatus::Inflight { tries, .. } = &mut o.status {
                            *tries = new_tries;
                        }
                    }
                }
            });

            // Once on_not_mined_tick says so, replace-by-fee on the STORED nonce
            // (NOT a fresh `latest` read). resubmit_if_stuck does the bumped-gas
            // re-sign + rebroadcast and does NOT advance tries again (confirm_op
            // owns the per-tick advance now), avoiding a double-advance.
            if do_resubmit {
                resubmit_if_stuck(chain, op_id, &op, &tx_hash, new_tries, finality_depth).await;
            }
            return;
        }
    };

    let now = ic_cdk::api::time();

    // 2. Reverted tx: mark the op Failed. The per-op-kind state reversal differs:
    //
    //    - Mint (Design B): no debt was counted, so NO supply reversal. Clear the
    //      vault's pending mint (the mint will not happen). Do NOT advance the
    //      vault status — `Closed` here means "fully repaid + collateral returned",
    //      so stamping it would mislabel a vault that still holds deposited
    //      collateral; the failed-mint resolution (return collateral, then close)
    //      is the Task 12/13 flow's job. The vault is left MintPending with a
    //      Failed op + a ChainSettlementFailed event for that path to act on.
    //
    //    - NativeWithdrawal: the transfer did not happen, so the reserved
    //      collateral was not paid out from the settlement hot wallet. ADD it
    //      back to `collateral_amount_native` (undo the reserve-at-enqueue) and, if
    //      the vault had gone `Closing` (full withdrawal / close), revert it to
    //      `Open` — it is no longer empty. Never touches debt/supply (withdraw
    //      moves only collateral).
    //
    //    IDEMPOTENCY: the per-kind reversal AND the `mark_failed` run in a SINGLE
    //    `mutate_state` guarded on the op still being `Inflight` (compare-and-
    //    swap). If two overlapping `run_settlement` ticks snapshot the same
    //    Inflight op before either mutates (possible once Timer D runs at a short
    //    interval — Task 15), only the first to observe it Inflight performs the
    //    reversal; the second sees the op already Failed and is a no-op. Without
    //    this CAS the NativeWithdrawal add-back could credit `2 × amount_e18` for
    //    one reverted withdrawal (phantom collateral). The broader run_settlement
    //    re-entrancy guard is deferred to Task 15; this per-op CAS is the local
    //    defense-in-depth.
    if !status_ok {
        let reason = "tx reverted".to_string();
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
                    // Design B: no debt was counted, so NO supply reversal. Clear
                    // pending mint; do NOT change status (Task-10 behavior).
                    if let Some(v) = s.multi_chain.chain_vaults.get_mut(vault_id) {
                        v.pending_mint_e8s = 0;
                    }
                }
                SettlementOpKind::NativeWithdrawal { vault_id, amount_e18, .. } => {
                    if let Some(v) = s.multi_chain.chain_vaults.get_mut(vault_id) {
                        v.collateral_amount_native =
                            v.collateral_amount_native.saturating_add(*amount_e18);
                        if v.status == ChainVaultStatus::Closing {
                            v.status = ChainVaultStatus::Open;
                        }
                    }
                }
                SettlementOpKind::InterestMint { vault_id, .. } => {
                    // Task 12: no debt/supply was credited (the mint reverted), so
                    // just clear the reservation. last_interest_accrual_ns is left
                    // unchanged, so the same window is retried at the next harvest
                    // (no double-charge, no loss).
                    if let Some(v) = s.multi_chain.chain_vaults.get_mut(vault_id) {
                        v.pending_interest_mint_e8s = 0;
                    }
                }
                SettlementOpKind::Burn { .. } => {}
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
        // Gate the event + log on the CAS so a rare double-tick does not emit a
        // duplicate failure event (the state mutation is already idempotent; this
        // just keeps the event log clean).
        if did_revert {
            crate::storage::record_event(&crate::event::Event::ChainSettlementFailed {
                chain_id: chain,
                op_id,
                reason,
                timestamp: now,
            });
            log!(INFO, "[settlement chain={:?}] op {} tx {} reverted; marked Failed", chain, op_id, tx_hash);
        }
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
            // `log_index` is threaded through from get_logs but not needed here
            // (the settlement path selects by vault_id + tx_hash, not by log
            // identity — each mint op maps to exactly one Mint event).
            let mut matched: Option<(u128, String)> = None;
            for (topics, data, log_tx, log_block, _log_index) in &logs {
                match evm_rpc::decode_mint_log(topics, data, log_tx, *log_block) {
                    Ok(m) if m.vault_id == vault_id => {
                        let exact = log_tx.eq_ignore_ascii_case(&tx_hash);
                        // Prefer an exact tx-hash match; otherwise take the first
                        // vault-id match as a fallback (safe: IcUSD.mint reverts a
                        // repeat vault_id, so at most one Mint log per vault exists).
                        if exact {
                            matched = Some((m.amount_e8s, m.recipient));
                            break;
                        } else if matched.is_none() {
                            matched = Some((m.amount_e8s, m.recipient));
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        log!(INFO, "[settlement chain={:?}] decode_mint_log failed confirming op {}: {}", chain, op_id, e);
                    }
                }
            }

            let (observed_e8s, observed_recipient) = match matched {
                Some(pair) => pair,
                None => {
                    log!(INFO, "[settlement chain={:?}] no Mint log for vault {} in block {} confirming op {}; will retry", chain, vault_id, block_number, op_id);
                    return;
                }
            };

            // Verify the on-chain Mint recipient matches the vault's intended
            // `mint_recipient` (case-insensitive). The supply invariant is
            // recipient-agnostic, so without this check a mint to the WRONG
            // address would still balance and be marked Succeeded. A mismatch is a
            // real divergence: leave the op Inflight and do NOT credit.
            let intended_recipient =
                read_state(|s| s.multi_chain.chain_vaults.get(&vault_id).map(|v| v.mint_recipient.clone()));
            match intended_recipient {
                Some(intended) if intended.eq_ignore_ascii_case(&observed_recipient) => {}
                Some(intended) => {
                    log!(INFO, "[settlement chain={:?}] mint-confirm recipient mismatch op {} vault {}: on-chain {} != intended {}; left Inflight (NOT credited)", chain, op_id, vault_id, observed_recipient, intended);
                    return;
                }
                None => {
                    log!(INFO, "[settlement chain={:?}] mint-confirm: unknown vault {} for op {}; left Inflight", chain, vault_id, op_id);
                    return;
                }
            }

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
        SettlementOpKind::InterestMint { vault_id, mint_id, accrual_through_ns, recipient, .. } => {
            // Task 12: same Mint-log read as a vault mint, but matched by the
            // SYNTHETIC `mint_id` and recipient-verified against the interest
            // treasury. On confirm, debt + supply grow together for the REAL vault.
            let vault_id = *vault_id;
            let mint_id = *mint_id;
            let accrual_through_ns = *accrual_through_ns;
            let intended_recipient = recipient.clone();

            let contract = read_state(|s| s.multi_chain.chain_contracts.get(&chain).cloned());
            let contract = match contract {
                Some(c) => c,
                None => {
                    log!(INFO, "[settlement chain={:?}] no contract configured; cannot confirm interest mint op {}", chain, op_id);
                    return;
                }
            };

            let logs = match evm_rpc::get_logs(chain, &contract, evm_rpc::MINT_EVENT_TOPIC0, block_number, block_number).await {
                Ok(l) => l,
                Err(e) => {
                    log!(INFO, "[settlement chain={:?}] get_logs(interest mint) failed confirming op {}: {}; will retry", chain, op_id, e);
                    return;
                }
            };

            let mut matched: Option<(u128, String)> = None;
            for (topics, data, log_tx, log_block, _log_index) in &logs {
                match evm_rpc::decode_mint_log(topics, data, log_tx, *log_block) {
                    Ok(m) if m.vault_id == mint_id => {
                        let exact = log_tx.eq_ignore_ascii_case(&tx_hash);
                        if exact {
                            matched = Some((m.amount_e8s, m.recipient));
                            break;
                        } else if matched.is_none() {
                            matched = Some((m.amount_e8s, m.recipient));
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        log!(INFO, "[settlement chain={:?}] decode_mint_log failed confirming interest op {}: {}", chain, op_id, e);
                    }
                }
            }

            let (observed_e8s, observed_recipient) = match matched {
                Some(pair) => pair,
                None => {
                    log!(INFO, "[settlement chain={:?}] no interest Mint log for mint_id {} in block {} confirming op {}; will retry", chain, mint_id, block_number, op_id);
                    return;
                }
            };

            // The interest mint MUST land at the per-chain interest-treasury
            // address (the recipient stored on the op). A mismatch is a real
            // divergence: leave Inflight, do NOT credit.
            if !intended_recipient.eq_ignore_ascii_case(&observed_recipient) {
                log!(INFO, "[settlement chain={:?}] interest-mint recipient mismatch op {} mint_id {}: on-chain {} != treasury {}; left Inflight (NOT credited)", chain, op_id, mint_id, observed_recipient, intended_recipient);
                return;
            }

            let pre_total = read_state(|s| s.multi_chain.total_chain_vault_debt_e8s());
            let result = mutate_state(|s| {
                confirm_interest_mint_in_state(
                    &mut s.multi_chain, chain, vault_id, observed_e8s, accrual_through_ns, pre_total,
                )
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
                    crate::storage::record_event(&crate::event::Event::ChainInterestMinted {
                        chain_id: chain,
                        vault_id,
                        mint_id,
                        amount_e8s: observed_e8s,
                        tx_hash: tx_hash.clone(),
                        block_number,
                        timestamp: now,
                    });
                    log!(INFO, "[settlement chain={:?}] interest mint confirmed: op={} vault={} mint_id={} amount_e8s={} block={} tx={}", chain, op_id, vault_id, mint_id, observed_e8s, block_number, tx_hash);
                }
                Err(e) => {
                    log!(INFO, "[settlement chain={:?}] confirm_interest_mint_in_state FAILED for op {} vault {}: {}; left Inflight", chain, op_id, vault_id, e);
                }
            }
        }
        SettlementOpKind::NativeWithdrawal { vault_id, .. } => {
            // A confirmed (mined + ok + final) native transfer-out: the
            // collateral has been paid out from the vault's own custody address.
            // Mark the op Succeeded,
            // then if the vault is `Closing` (a full withdrawal / close) flip it
            // to `Closed` — collateral is gone and (close required) debt is 0, so
            // the vault is fully settled. A partial withdrawal leaves the vault
            // `Open` (it still holds collateral); nothing extra to do there.
            let vid = *vault_id;
            mutate_state(|s| {
                if let Some(q) = s.multi_chain.settlement_queues.get_mut(&chain) {
                    if let Some(o) = q.pending.get_mut(&op_id) {
                        o.mark_succeeded(tx_hash.clone(), now);
                    }
                }
                if let Some(v) = s.multi_chain.chain_vaults.get_mut(&vid) {
                    if v.status == ChainVaultStatus::Closing {
                        v.status = ChainVaultStatus::Closed;
                    }
                }
            });
            log!(INFO, "[settlement chain={:?}] withdrawal op {} vault {} confirmed tx={} (Closing->Closed if applicable)", chain, op_id, vid, tx_hash);
        }
        SettlementOpKind::Burn { .. } => {
            // Unreachable: a Burn op is marked Failed on the submit path and
            // never goes Inflight. Log defensively rather than panic.
            log!(INFO, "[settlement chain={:?}] inflight Burn op {} reached confirm path unexpectedly", chain, op_id);
        }
    }
}

/// Stuck-tx replace-by-fee broadcast (Task 11). Called from the `confirm_op`
/// NOT-MINED branch ONLY when `hardening::on_not_mined_tick` has already decided
/// this op is stuck and a resubmit is warranted (the caller advanced and
/// persisted `tries` first). Re-signs and rebroadcasts the SAME transaction on
/// the SAME stored nonce (`submit_nonce`) with fees bumped +25% — a true EVM
/// replace-by-fee, NOT a second mint.
///
/// `tries`/`finality_depth` are passed for logging only — the stuck decision is
/// the caller's. On a successful rebroadcast: update `last_tx_hash` (and
/// `last_attempt_ns`) but do NOT advance `tries` (confirm_op already advanced it
/// once for this tick; advancing again would double-count). On any error
/// (derive/fees/sign/send): log and leave the op Inflight as-is for the next
/// tick. When `submit_nonce` is `None`, this refuses to resubmit (a resubmit
/// would risk a fresh nonce — a second mint) and leaves the op Inflight.
///
/// Borrow discipline mirrors `submit_op`: read → clone → await → mutate; no
/// `read_state`/`mutate_state` borrow is held across an `.await`.
async fn resubmit_if_stuck(
    chain: ChainId,
    op_id: u64,
    op: &crate::chains::settlement_queue::SettlementOp,
    tx_hash: &str,
    tries: u32,
    finality_depth: u32,
) {
    // We can only replace-by-fee if we know the nonce the op was first submitted
    // at. Without it, a resubmit would risk a fresh nonce (a second mint), so we
    // do NOT resubmit — leave Inflight for the next tick.
    let nonce = match op.submit_nonce {
        Some(n) => n,
        None => {
            log!(INFO, "[settlement chain={:?}] op {} stuck (tries={}, finality_depth={}) but no submit_nonce; leaving Inflight (cannot safely replace-by-fee)", chain, op_id, tries, finality_depth);
            return;
        }
    };

    // Resolve the signer (settlement for mints, vault custody for withdrawals).
    let (path, signer_addr) = match resolve_op_signer(chain, &op.kind).await {
        Ok(pair) => pair,
        Err(e) => {
            log!(INFO, "[settlement chain={:?}] resubmit resolve_op_signer failed for op {}: {}; leaving Inflight", chain, op_id, e);
            return;
        }
    };

    // Recompute fees and bump +25%. Mirror the adapter's max-fee formula
    // (2*base + prio) before bumping, so the bumped ceiling tracks current gas.
    let (base_fee, prio) = match evm_rpc::fetch_fees(chain).await {
        Ok(pair) => pair,
        Err(e) => {
            log!(INFO, "[settlement chain={:?}] resubmit fetch_fees failed for op {}: {}; leaving Inflight", chain, op_id, e);
            return;
        }
    };
    let base_max_fee = base_fee.saturating_mul(2).saturating_add(prio);
    let (bumped_prio, bumped_max) = hardening::bump_gas(prio, base_max_fee);

    // Re-net the withdrawal value at the BUMPED fee so the replacement still
    // fits within the custody balance (a higher gas reserve sends marginally
    // less collateral — fine for a replace-by-fee). Mints carry no value.
    let withdrawal_value = if matches!(op.kind, SettlementOpKind::NativeWithdrawal { .. }) {
        match evm_rpc::get_balance(chain, &signer_addr).await {
            Ok(bal) => {
                if let SettlementOpKind::NativeWithdrawal { amount_e18, .. } = &op.kind {
                    Some(fundable_withdrawal_value(*amount_e18, bal, bumped_max))
                } else {
                    None
                }
            }
            Err(e) => {
                log!(INFO, "[settlement chain={:?}] resubmit get_balance(custody) failed for op {}: {}; leaving Inflight", chain, op_id, e);
                return;
            }
        }
    } else {
        None
    };

    // Resolve the contract (mints only) and rebuild the SAME tx at the stored
    // nonce with the bumped fees.
    let contract = read_state(|s| s.multi_chain.chain_contracts.get(&chain).cloned());
    let plan = match build_tx_plan(chain, &op.kind, nonce, bumped_prio, bumped_max, contract.as_deref(), withdrawal_value) {
        Ok(p) => p,
        Err(e) => {
            log!(INFO, "[settlement chain={:?}] resubmit build_tx_plan failed for op {}: {}; leaving Inflight", chain, op_id, e);
            return;
        }
    };

    // Re-sign on the stored nonce with the resolved signer.
    let raw_hex = match tx::sign_eip1559(&plan.fields, path, &signer_addr).await {
        Ok(h) => h,
        Err(e) => {
            log!(INFO, "[settlement chain={:?}] resubmit sign_eip1559 failed for op {}: {}; leaving Inflight", chain, op_id, e);
            return;
        }
    };

    // Rebroadcast.
    let new_tx_hash = match evm_rpc::send_raw_transaction(chain, &raw_hex).await {
        Ok(h) => h,
        Err(e) => {
            log!(INFO, "[settlement chain={:?}] resubmit send_raw_transaction failed for op {}: {}; leaving Inflight", chain, op_id, e);
            return;
        }
    };

    // Success: record the new tx hash and refresh last_attempt_ns. Do NOT bump
    // `tries` here — confirm_op already advanced it once for this tick (this
    // function is only called after on_not_mined_tick decided to resubmit), so
    // bumping again would double-advance per tick. The nonce is unchanged (the
    // whole point of replace-by-fee).
    let now = ic_cdk::api::time();
    mutate_state(|s| {
        if let Some(q) = s.multi_chain.settlement_queues.get_mut(&chain) {
            if let Some(o) = q.pending.get_mut(&op_id) {
                o.last_tx_hash = Some(new_tx_hash.clone());
                if let SettlementOpStatus::Inflight { last_attempt_ns, .. } = &mut o.status {
                    *last_attempt_ns = now;
                }
            }
        }
    });
    log!(
        INFO,
        "[settlement chain={:?}] STUCK op {} (tries={}, finality_depth={}) replaced-by-fee on nonce {}: prio {}->{}, max_fee {}->{}, old_tx={} new_tx={}",
        chain, op_id, tries, finality_depth, nonce, prio, bumped_prio, base_max_fee, bumped_max, tx_hash, new_tx_hash
    );
}
