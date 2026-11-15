//! The timer (53a): one self-rescheduling timer runs the whole background pass in a fixed order.
//! In 53a the pass is the ICP-side sagas (`ledger_pass`: ledger facts, pulls, preflight holds,
//! refunds, releases, fee sweeps); the mint lane, the return observer and the monitors join the same
//! pass in 53b and 53c. Once nothing is active the pass makes only the three cheap ledger reads, and
//! only every few minutes, so an idle bridge spends almost nothing.
//!
//! [`run_once`] is generic over the ledger and is what the tests drive. The wasm half only builds the
//! real [`CanisterLedger`] from the stored configuration and spawns [`run_once`]. A guard stops two
//! passes overlapping; a guard left behind by a trap after an await is reclaimed after ten minutes,
//! and the next timer is armed BEFORE the pass is spawned, so a trapped pass cannot end the chain of
//! ticks. Nothing here signs, broadcasts or reads Sui.

use crate::flows::{deposit, release};
use crate::icp::IcpLedger;
use crate::state;
use std::cell::Cell;

const STALE_GUARD_NS: u64 = 10 * 60 * 1_000_000_000;
/// Smallest interval between passes, whatever the configuration says.
pub const MIN_INTERVAL_SECS: u64 = 5;
/// Seconds between passes.
pub const INTERVAL_SECS: u64 = 30;

thread_local! {
    static RUNNING_SINCE: Cell<Option<u64>> = const { Cell::new(None) };
    #[cfg(target_arch = "wasm32")]
    static NEXT_TIMER: std::cell::RefCell<Option<ic_cdk_timers::TimerId>> = const { std::cell::RefCell::new(None) };
}

struct RunGuard {
    token: u64,
}

impl RunGuard {
    fn acquire(now: u64) -> Option<RunGuard> {
        RUNNING_SINCE.with(|r| match r.get() {
            Some(t) if now.saturating_sub(t) < STALE_GUARD_NS => None,
            _ => {
                r.set(Some(now));
                Some(RunGuard { token: now })
            }
        })
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        // Clear only our own claim: a stale guard may have been reclaimed by a newer pass.
        RUNNING_SINCE.with(|r| {
            if r.get() == Some(self.token) {
                r.set(None);
            }
        });
    }
}

/// What one pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunReport {
    pub skipped: bool,
}

/// How long the ledger facts are trusted while nothing is waiting on them.
pub const LEDGER_FACTS_TTL_NS: u64 = 5 * 60 * 1_000_000_000;

thread_local! {
    /// When the ledger facts were last read. Heap only: after an upgrade the next pass reads them.
    static FACTS_READ_AT: Cell<Option<u64>> = const { Cell::new(None) };
}

fn ledger_work_pending() -> bool {
    state::active_deposit_count() > 0
        || state::active_release_count() > 0
        || state::read_runtime(|c| c.sweeps.iter().any(|s| !crate::flows::ledger_ops::is_terminal(&s.op)))
}

/// Refresh the ledger facts the sagas depend on: the minting account (once), the transfer fee and a
/// lower bound on the log length (where an ICRC-3 search of an operation created later starts). An
/// idle bridge reads them every few minutes; with ledger work pending, every pass.
async fn refresh_ledger_basics<L: IcpLedger>(ledger: &L) {
    let now = crate::env::now_ns();
    let known = state::read_runtime(|c| c.ledger_minting_account.is_some() && c.ledger_fee_e8s.is_some());
    let fresh = FACTS_READ_AT.with(|t| t.get()).is_some_and(|t| now.saturating_sub(t) < LEDGER_FACTS_TTL_NS);
    if known && fresh && !ledger_work_pending() {
        return;
    }
    FACTS_READ_AT.with(|t| t.set(Some(now)));
    if state::read_runtime(|c| c.ledger_minting_account.is_none()) {
        if let Ok(Some(acc)) = ledger.minting_account().await {
            state::mutate_runtime(|c| c.ledger_minting_account = Some(acc));
        }
    }
    if let Ok(fee) = ledger.fee().await {
        state::mutate_runtime(|c| c.ledger_fee_e8s = Some(fee));
    }
    if let Ok(len) = ledger.log_length().await {
        state::mutate_runtime(|c| c.ledger_tip_lower_bound = c.ledger_tip_lower_bound.max(len));
    }
}

/// The ICP-side sagas, in dependency order. Each is bounded per pass.
pub async fn ledger_pass<L: IcpLedger>(ledger: &L) {
    refresh_ledger_basics(ledger).await;
    deposit::reconcile_pulls(ledger).await;
    deposit::retry_due_holds();
    deposit::process_refunds(ledger).await;
    release::process_releases(ledger).await;
    deposit::process_sweeps(ledger).await;
}

/// One background pass over the ICP side only (the ledger sagas and the re-mint driver). The mint
/// lane is not run: see [`run_full`].
pub async fn run_once<L: IcpLedger>(ledger: &L, now: u64) -> RunReport {
    if !state::read_runtime(|c| c.initialized) {
        return RunReport::default();
    }
    let Some(_guard) = RunGuard::acquire(now) else {
        return RunReport { skipped: true };
    };
    ledger_pass(ledger).await;
    crate::flows::remint::process_remints();
    RunReport { skipped: false }
}

/// One full background pass: the ICP side, the re-mint driver, one step of the mint lane, then the return
/// observer (which opens releases from verified returns and runs the monitors). The
/// lane only runs when its own conditions hold (`flows::mint::blocked_by` is empty), so an
/// unconfigured instance, a disabled mirror or a halt makes this equal to [`run_once`].
pub async fn run_full<L: IcpLedger, T: crate::mirror::SuiTransport, S: crate::schnorr::RoleSigner>(ledger: &L, transport: &T, signer: &S, now: u64) -> RunReport {
    if !state::read_runtime(|c| c.initialized) {
        return RunReport::default();
    }
    let Some(_guard) = RunGuard::acquire(now) else {
        return RunReport { skipped: true };
    };
    ledger_pass(ledger).await;
    crate::flows::remint::process_remints();
    crate::flows::mint::mint_pass(transport, signer).await;
    crate::flows::observer::observer_pass(transport, ledger).await;
    RunReport { skipped: false }
}

/// Run one pass now (the admin's "run tick"). `false` when another pass holds the guard.
#[cfg(target_arch = "wasm32")]
pub async fn run_now() -> bool {
    !run_wasm(crate::env::now_ns()).await.skipped
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn run_now() -> bool {
    false
}

/// The real pass: the real ledger, the HTTPS-outcall transport and the threshold signer. An instance
/// whose network cannot build a signer runs the ICP side only.
#[cfg(target_arch = "wasm32")]
async fn run_wasm(now: u64) -> RunReport {
    let ledger = crate::icp::CanisterLedger::from_config();
    let (network, chain) = state::read_core(|c| (c.network, c.chain_identifier));
    match crate::schnorr::SuiSigner::new(network, chain) {
        Ok(signer) => run_full(&ledger, &crate::mirror::CanisterTransport, &signer, now).await,
        Err(_) => run_once(&ledger, now).await,
    }
}

#[cfg(target_arch = "wasm32")]
async fn pass() {
    run_wasm(crate::env::now_ns()).await;
}

/// Arm the next pass `secs` from now (replacing any pending one).
#[cfg(target_arch = "wasm32")]
pub fn schedule_in(secs: u64) {
    NEXT_TIMER.with(|slot| {
        if let Some(old) = slot.borrow_mut().take() {
            ic_cdk_timers::clear_timer(old);
        }
        let id = ic_cdk_timers::set_timer(std::time::Duration::from_secs(secs), || {
            // Re-arm first: the chain survives even if the spawned pass traps after one of its awaits.
            schedule_in(INTERVAL_SECS);
            ic_cdk::spawn(pass());
        });
        *slot.borrow_mut() = Some(id);
    });
}

#[cfg(not(target_arch = "wasm32"))]
pub fn schedule_in(_secs: u64) {}

/// Arm the first pass shortly after install or upgrade.
pub fn schedule_soon() {
    schedule_in(MIN_INTERVAL_SECS);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{block_on, MockLedger};

    #[test]
    fn an_uninitialized_instance_does_nothing() {
        state::reset_for_tests();
        let r = block_on(run_once(&MockLedger::new(), 5));
        assert_eq!(r, RunReport::default());
    }

    #[test]
    fn overlapping_passes_are_refused_and_a_stale_guard_is_reclaimed() {
        state::reset_for_tests();
        state::mutate_runtime(|c| c.initialized = true);
        let now = 1_000_000_000_000u64;
        let g = RunGuard::acquire(now).unwrap();
        let ledger = MockLedger::new();
        assert!(block_on(run_once(&ledger, now + 1)).skipped, "a pass is running");
        // A guard left behind by a trapped pass is reclaimed after ten minutes.
        assert!(!block_on(run_once(&ledger, now + STALE_GUARD_NS + 1)).skipped);
        drop(g);
        assert!(!block_on(run_once(&ledger, now + 2 * STALE_GUARD_NS)).skipped);
    }
}
