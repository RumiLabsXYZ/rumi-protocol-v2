//! Timer orchestration and supply reconciliation.
//!
//! One self-rescheduling timer runs all background work in a fixed order. It uses the
//! active interval while anything is pending and the idle interval otherwise, so an
//! idle bridge makes few paid outcalls. A guard prevents overlapping ticks; a stale
//! guard (left by a trap after an await) is reclaimed after ten minutes.

use crate::accounting::{check_base_supply, check_coverage};
use crate::env;
use crate::evm;
use crate::flows;
use crate::ids::LaneKind;
use crate::ledger;
use crate::observer;
use crate::state::{self, AuditCtx, BalanceSnapshot};
use crate::types::*;
use crate::views;
use std::cell::Cell;

const STALE_GUARD_NS: u64 = 10 * 60 * 1_000_000_000;
/// Minimum spacing between automatic supply reconciliations (each reads the finalized
/// head with a large response cap, so it is not run every tick).
const RECONCILE_EVERY_NS: u64 = 30 * 60 * 1_000_000_000;

thread_local! {
    static RUNNING_SINCE: Cell<Option<u64>> = const { Cell::new(None) };
    static LAST_RECONCILE_NS: Cell<u64> = const { Cell::new(0) };
    /// Due time of the pending tick, if one is armed.
    static NEXT_DUE_NS: Cell<Option<u64>> = const { Cell::new(None) };
    #[cfg(target_arch = "wasm32")]
    static NEXT_TIMER: std::cell::RefCell<Option<ic_cdk_timers::TimerId>> = const { std::cell::RefCell::new(None) };
}

/// Decide whether a request to tick at `wanted_due` should replace the pending tick.
/// A pending tick that is already due no later than requested is kept, so repeated
/// requests (for example many user calls) can never push the next tick later.
pub fn should_reschedule(pending_due: Option<u64>, wanted_due: u64) -> bool {
    match pending_due {
        Some(due) => wanted_due < due,
        None => true,
    }
}

struct TickGuard;

impl TickGuard {
    fn acquire() -> Option<TickGuard> {
        let now = env::now_ns();
        RUNNING_SINCE.with(|r| match r.get() {
            Some(t) if now.saturating_sub(t) < STALE_GUARD_NS => None,
            _ => {
                r.set(Some(now));
                Some(TickGuard)
            }
        })
    }
}

impl Drop for TickGuard {
    fn drop(&mut self) {
        RUNNING_SINCE.with(|r| r.set(None));
    }
}

/// Whether anything is in flight, so the next tick should come soon. User hints are
/// deliberately excluded: anyone signed in can submit one, and they must not be able
/// to force fast, paid RPC polling.
pub fn has_pending_work() -> bool {
    state::active_deposit_count() > 0 || state::active_release_count() > 0 || state::read_core(|c| {
        // A halted lane (nonce anomaly) is parked for manual investigation; it must not
        // keep the timer at the fast interval forever.
        (c.minter_lane.halted.is_none()
                && (c.minter_lane.unincluded_tx.is_some() || !c.minter_lane.unfinalized_txs.is_empty()))
            || (c.admin_lane.halted.is_none()
                && (c.admin_lane.unincluded_tx.is_some()
                    || !c.admin_lane.unfinalized_txs.is_empty()
                    || !c.admin_lane.queued_txs.is_empty()))
            || c.sweeps.iter().any(|s| !flows::ledger_ops::is_terminal(&s.op))
    })
}

#[cfg(target_arch = "wasm32")]
fn schedule_in(secs: u64) {
    let wanted = env::now_ns().saturating_add(secs.saturating_mul(1_000_000_000));
    if !should_reschedule(NEXT_DUE_NS.with(|d| d.get()), wanted) {
        return;
    }
    NEXT_TIMER.with(|t| {
        if let Some(id) = t.borrow_mut().take() {
            ic_cdk_timers::clear_timer(id);
        }
        let id = ic_cdk_timers::set_timer(std::time::Duration::from_secs(secs), || {
            // This timer has fired. Arm a fallback tick first, in this message, so the
            // chain survives even if the spawned tick traps after one of its awaits.
            NEXT_TIMER.with(|t| *t.borrow_mut() = None);
            NEXT_DUE_NS.with(|d| d.set(None));
            let idle = state::read_core(|c| c.config.observer.idle_interval_secs);
            schedule_in(idle);
            ic_cdk::spawn(async {
                run_tick().await;
                schedule_next();
            })
        });
        *t.borrow_mut() = Some(id);
    });
    NEXT_DUE_NS.with(|d| d.set(Some(wanted)));
}

#[cfg(not(target_arch = "wasm32"))]
fn schedule_in(secs: u64) {
    let wanted = env::now_ns().saturating_add(secs.saturating_mul(1_000_000_000));
    if should_reschedule(NEXT_DUE_NS.with(|d| d.get()), wanted) {
        NEXT_DUE_NS.with(|d| d.set(Some(wanted)));
    }
}

/// Make sure a tick is armed (used by admin endpoints, so an operator can always revive
/// the timer without an upgrade).
pub fn ensure_scheduled() {
    schedule_soon();
}

#[cfg(test)]
pub fn pending_due_for_tests() -> Option<u64> {
    NEXT_DUE_NS.with(|d| d.get())
}

/// Run a tick within about a second (used after user actions and on install).
pub fn schedule_soon() {
    schedule_in(1);
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn schedule_next() {
    let (active, idle) = state::read_core(|c| (c.config.observer.active_interval_secs, c.config.observer.idle_interval_secs));
    schedule_in(if has_pending_work() { active } else { idle });
}

#[cfg(target_arch = "wasm32")]
async fn fetch_minting_account(ledger_id: candid::Principal) -> Result<Option<icrc_ledger_types::icrc1::account::Account>, String> {
    let (acc,): (Option<icrc_ledger_types::icrc1::account::Account>,) =
        ic_cdk::call(ledger_id, "icrc1_minting_account", ()).await.map_err(|(c, m)| format!("{c:?}: {m}"))?;
    Ok(acc)
}

#[cfg(not(target_arch = "wasm32"))]
async fn fetch_minting_account(_ledger_id: candid::Principal) -> Result<Option<icrc_ledger_types::icrc1::account::Account>, String> {
    Err("unavailable on host".into())
}

async fn refresh_ledger_basics() {
    let ledger_id = state::read_core(|c| c.config.icusd_ledger);
    if state::read_core(|c| c.ledger_minting_account.is_none()) {
        if let Ok(Some(acc)) = fetch_minting_account(ledger_id).await {
            state::mutate_core(|c| c.ledger_minting_account = Some(acc));
        }
    }
    if let Ok(fee) = ledger::fee(ledger_id).await {
        state::mutate_core(|c| c.ledger_fee_e8s = Some(fee));
    }
    if let Ok(len) = ledger::log_length(ledger_id).await {
        state::mutate_core(|c| c.ledger_tip_lower_bound = c.ledger_tip_lower_bound.max(len));
    }
}

/// All background work, in dependency order.
pub async fn run_tick() {
    let Some(_guard) = TickGuard::acquire() else { return };
    if !state::read_core(|c| c.initialized) {
        return;
    }
    refresh_ledger_basics().await;
    flows::deposit::reconcile_pulls().await;
    flows::deposit::retry_due_holds();
    // Lanes first: a mint and the burn of its tokens can finalize in the same head step,
    // and the burn must not be recorded before the mint has credited outstanding supply.
    evm::lane::run_lane(LaneKind::Minter).await;
    evm::lane::run_lane(LaneKind::Admin).await;
    if !state::read_core(|c| c.config.pauses.observer) {
        if let Err(e) = observer::run_pass().await {
            state::mutate_core(|c| c.observer.last_error = Some(e));
        }
    }
    flows::release::start_remints();
    flows::release::process_releases().await;
    flows::deposit::process_refunds().await;
    flows::deposit::process_sweeps().await;
    let now = env::now_ns();
    let due = LAST_RECONCILE_NS.with(|l| now.saturating_sub(l.get()) >= RECONCILE_EVERY_NS);
    if due {
        let _ = reconcile_supply().await;
    }
}

/// Check invariant C1 against the live escrow balance and C2/C3 against the finalized
/// Base supply. Violations halt the bridge.
pub async fn reconcile_supply() -> BridgeResult<()> {
    LAST_RECONCILE_NS.with(|l| l.set(env::now_ns()));
    let canister = env::canister_id();
    let (ledger_id, escrow) = state::read_core(|c| (c.config.icusd_ledger, c.escrow_account(canister)));
    let ledger_total = ledger::total_supply(ledger_id).await.ok();
    // The balance read is an await, and other messages may change the buckets while it
    // is in flight. Inflows only add surplus, and every escrow outflow is reserved in
    // `unknown_outflow` before it is sent, so comparing the balance with the buckets as
    // they were before AND after the read can only fail both ways for a real deficit.
    let before = state::read_core(|c| c.buckets);
    let balance = ledger::balance_of(ledger_id, escrow).await.map_err(BridgeError::Ledger)?;
    let after = state::read_core(|c| c.buckets);
    let r_before = check_coverage(&before, balance).map_err(|e| BridgeError::Internal(e.to_string()))?;
    let r_after = check_coverage(&after, balance).map_err(|e| BridgeError::Internal(e.to_string()))?;
    let ok = r_before.ok || r_after.ok;
    let required = r_before.required.min(r_after.required);
    let now = env::now_ns();
    state::mutate_core(|c| {
        c.escrow_snapshot =
            Some(BalanceSnapshot { balance, at_ns: now, coverage_ok: ok, ledger_total_supply: ledger_total, required })
    });
    if !ok {
        state::halt(format!(
            "escrow coverage violated: balance {balance} < required {} (before) and {} (after)",
            r_before.required, r_after.required
        ));
    }
    if state::read_core(|c| c.config.contract.is_some()) {
        match observer::check_base_supply().await {
            Ok(snap) => {
                let within = snap.within_bound;
                let total = snap.total_supply;
                let block = snap.block_number;
                state::mutate_core(|c| c.base_supply_snapshot = Some(snap));
                if !within {
                    state::halt(format!("Base totalSupply {total} at block {block} exceeds outstanding plus in-flight mints"));
                }
            }
            Err(e) => {
                state::append_audit(None, AuditCtx::note(format!("Base supply check failed: {e}")));
            }
        }
    }
    Ok(())
}

/// Admin: re-run both supply checks now and clear the global halt only if both pass on
/// fresh data. Lane halts (nonce anomalies) are separate and are not cleared here.
pub async fn clear_halt_after_checks() -> BridgeResult<SupplyAudit> {
    let Some(halt) = state::read_core(|c| c.halted.clone()) else {
        return Ok(views::supply_audit());
    };
    let started = env::now_ns();
    reconcile_supply().await?;
    let (escrow_ok, base_ok, still) = state::read_core(|c| {
        (
            c.escrow_snapshot.as_ref().map(|s| s.at_ns >= started && s.coverage_ok).unwrap_or(false),
            match (&c.config.contract, &c.base_supply_snapshot) {
                (None, _) => true,
                (Some(_), Some(s)) => s.at_ns >= started && s.within_bound,
                (Some(_), None) => false,
            },
            c.halted.clone(),
        )
    });
    if !escrow_ok || !base_ok {
        return Err(BridgeError::Halted(format!(
            "checks did not pass on fresh data (escrow_ok={escrow_ok}, base_ok={base_ok}); halt remains: {}",
            still.map(|h| h.reason).unwrap_or(halt.reason)
        )));
    }
    // Anything that halted during the checks keeps its own reason; only clear the halt
    // that existed before the checks started.
    let cleared = state::mutate_core(|c| {
        if c.halted.as_ref().map(|h| h.at_ns <= started).unwrap_or(false) {
            c.halted = None;
            true
        } else {
            false
        }
    });
    if !cleared {
        return Err(BridgeError::Halted("a new halt was raised during the checks".into()));
    }
    state::append_audit(None, AuditCtx::note(format!("halt cleared after fresh checks: {}", halt.reason)));
    Ok(views::supply_audit())
}

/// Evaluate C2/C3 for a finalized Base `totalSupply`. Shared with the observer.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_never_push_a_pending_tick_later() {
        assert!(should_reschedule(None, 100));
        assert!(should_reschedule(Some(100), 50), "an earlier request wins");
        assert!(!should_reschedule(Some(100), 100));
        assert!(!should_reschedule(Some(100), 150), "a later request never delays the pending tick");
    }

    #[test]
    fn many_schedule_soon_calls_keep_the_first_due_time() {
        crate::state::reset_for_tests();
        crate::env::test_env::set_now_ns(1_000_000_000_000);
        schedule_soon();
        let first = pending_due_for_tests().unwrap();
        for _ in 0..100 {
            crate::env::test_env::advance_secs(0);
            crate::env::test_env::set_now_ns(1_000_000_000_000 + 500_000_000);
            schedule_soon();
        }
        assert_eq!(pending_due_for_tests(), Some(first));
    }
}

pub fn base_supply_snapshot(total_supply: u128, block_number: u64, quiescent: bool) -> state::BaseSupplySnapshot {
    let buckets = state::read_core(|c| c.buckets);
    let inflight = state::inflight_mint_e8s();
    let r = check_base_supply(&buckets, total_supply, inflight, quiescent);
    state::BaseSupplySnapshot {
        total_supply,
        block_number,
        inflight_mint: inflight,
        quiescent,
        within_bound: r.within_bound,
        exact_match: r.exact_match,
        at_ns: env::now_ns(),
    }
}
