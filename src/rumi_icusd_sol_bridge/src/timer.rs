//! The timer (49a): one self-rescheduling timer runs the whole background pass in a fixed order:
//! derive keys and bootstrap what is missing (`bootstrap::bootstrap_step`), then reconcile the
//! nonce lanes (`lane::tick`). Once everything is bound and no operation is active the pass makes
//! no calls, so an idle bridge spends nothing.
//!
//! [`run_once`] is generic over the transport and the signer and is what the tests drive. The wasm
//! half only builds the real [`CandidRpc`] and [`SchnorrSigner`] from the stored configuration and
//! spawns [`run_once`]. A guard stops two passes overlapping; a guard left behind by a trap after
//! an await is reclaimed after ten minutes, and the next timer is armed BEFORE the pass is spawned,
//! so a trapped pass cannot end the chain of ticks.

use crate::bootstrap::{bootstrap_step, BootstrapEvent};
use crate::lane::{tick, TickReport, TxSigner};
use crate::sol_rpc::SolRpc;
use crate::state;
use std::cell::Cell;

const STALE_GUARD_NS: u64 = 10 * 60 * 1_000_000_000;
/// Smallest interval between passes, whatever the configuration says.
pub const MIN_INTERVAL_SECS: u64 = 5;

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
    pub bootstrap: Vec<BootstrapEvent>,
    pub lane: TickReport,
}

/// One background pass.
pub async fn run_once<R: SolRpc, S: TxSigner>(rpc: &R, signer: &S, now: u64) -> RunReport {
    if !state::read_core(|c| c.initialized) {
        return RunReport::default();
    }
    let Some(_guard) = RunGuard::acquire(now) else {
        return RunReport { skipped: true, ..RunReport::default() };
    };
    let bootstrap = bootstrap_step(rpc, signer, now).await;
    let lane = tick(rpc, signer, now).await;
    RunReport { skipped: false, bootstrap, lane }
}

/// Seconds between passes: the rebroadcast interval of the lane settings, at least
/// [`MIN_INTERVAL_SECS`].
pub fn interval_secs() -> u64 {
    state::read_core(|c| c.config.lane.rebroadcast_interval_secs).max(MIN_INTERVAL_SECS)
}

#[cfg(target_arch = "wasm32")]
async fn pass() {
    use crate::schnorr::SchnorrSigner;
    use crate::sol_rpc::CandidRpc;
    let (config, now) = (state::read_core(|c| c.config.clone()), crate::env::now_ns());
    // An instance that is not fully configured (no cluster, an anonymous SOL RPC id, a key name that
    // breaks the rule) does nothing; it never falls back to a default.
    let (Ok(rpc), Ok(signer)) = (CandidRpc::from_config(config.sol_rpc, config.rpc.clone()), SchnorrSigner::from_config(&config)) else {
        return;
    };
    run_once(&rpc, &signer, now).await;
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
            schedule_in(interval_secs());
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
    use crate::testkit::{block_on, MockChain, MockSigner};

    #[test]
    fn an_uninitialized_instance_does_nothing() {
        state::reset_for_tests();
        let r = block_on(run_once(&MockChain::new(), &MockSigner::new(), 5));
        assert_eq!(r, RunReport::default());
    }

    #[test]
    fn overlapping_passes_are_refused_and_a_stale_guard_is_reclaimed() {
        state::reset_for_tests();
        state::mutate_core(|c| c.initialized = true);
        let chain = MockChain::new();
        let signer = MockSigner::new();
        let now = 1_000_000_000_000u64;
        let g = RunGuard::acquire(now).unwrap();
        assert!(block_on(run_once(&chain, &signer, now + 1)).skipped, "a pass is running");
        // A guard left behind by a trapped pass is reclaimed after ten minutes.
        assert!(!block_on(run_once(&chain, &signer, now + STALE_GUARD_NS + 1)).skipped);
        drop(g);
        assert!(!block_on(run_once(&chain, &signer, now + 2 * STALE_GUARD_NS)).skipped);
    }

    #[test]
    fn the_interval_follows_the_lane_settings_with_a_floor() {
        state::reset_for_tests();
        assert_eq!(interval_secs(), 30);
        state::mutate_core(|c| c.config.lane.rebroadcast_interval_secs = 1);
        assert_eq!(interval_secs(), MIN_INTERVAL_SECS);
    }
}
