//! Per-pool reentrancy guard for the stability pool's liquidation path.
//!
//! SP-102 (audit 2026-06-05): a liquidation snapshots depositor balances,
//! `await`s the backend (which pulls the consumed stables), then re-reads the
//! LIVE depositor set to apportion the burn and collateral gains. `deposit` /
//! `withdraw` / `claim_*` run across that await window, so a withdraw that lands
//! mid-liquidation escapes its share of the burn while the remaining depositors
//! over-absorb, and the tracked aggregate can end up above the real ledger
//! balance. This guard serializes the liquidation against the balance-mutating
//! user ops: the liquidation holds it across its await; `deposit` / `withdraw` /
//! `claim_*` reject with `SystemBusy` while it is held.
//!
//! Thread-local + RAII: ic-cdk's `call_on_cleanup` drops the guard (releasing
//! the lock) even when a post-`await` continuation traps, so the pool never
//! wedges. Same pattern as `rumi_3pool::PoolGuard`.
//!
//! This is a CONCURRENCY guard, not a liquidation retry. A rejected concurrent
//! liquidation simply does not run this round and falls through to the next
//! notification / manual handling; the SP never auto-retries a liquidation
//! (project rule).

use crate::types::StabilityPoolError;
use std::cell::RefCell;

thread_local! {
    static LIQUIDATION_ACTIVE: RefCell<bool> = const { RefCell::new(false) };
}

#[must_use]
pub struct SpLiquidationGuard;

impl SpLiquidationGuard {
    /// Acquire the exclusive liquidation lock. Returns `SystemBusy` if another
    /// liquidation is already in flight (no auto-retry — the caller or the next
    /// notification round handles it).
    pub fn new() -> Result<Self, StabilityPoolError> {
        LIQUIDATION_ACTIVE.with(|f| {
            let mut held = f.borrow_mut();
            if *held {
                return Err(StabilityPoolError::SystemBusy);
            }
            *held = true;
            Ok(Self)
        })
    }
}

impl Drop for SpLiquidationGuard {
    fn drop(&mut self) {
        LIQUIDATION_ACTIVE.with(|f| *f.borrow_mut() = false);
    }
}

/// True while a liquidation holds the lock. `deposit` / `withdraw` / `claim_*`
/// reject (with `SystemBusy`) when this is true, so they cannot race the
/// liquidation's snapshot -> await -> apportion sequence.
pub fn liquidation_in_progress() -> bool {
    LIQUIDATION_ACTIVE.with(|f| *f.borrow())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sp_liquidation_guard_is_exclusive_and_blocks_ops() {
        assert!(!liquidation_in_progress());
        let g = SpLiquidationGuard::new().expect("first acquire");
        assert!(liquidation_in_progress(), "deposit/withdraw/claim see the lock as held");
        assert!(
            SpLiquidationGuard::new().is_err(),
            "a second concurrent liquidation must be rejected (SystemBusy)",
        );
        drop(g);
        assert!(!liquidation_in_progress(), "lock released on drop");
        let _g2 = SpLiquidationGuard::new().expect("re-acquire after release");
    }
}
