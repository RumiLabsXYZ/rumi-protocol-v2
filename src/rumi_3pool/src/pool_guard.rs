//! Per-canister reentrancy guard for the 3pool's mutating async paths.
//!
//! On the IC, messages interleave at every `await` point. Without
//! serialization, two concurrent callers of `swap` can both read
//! `s.balances` before either updates state, both compute the same output,
//! and both transfer that output to the user. The same hazard applies to
//! `add_liquidity`, `remove_liquidity`, `remove_one_coin`, `donate`, and
//! `authorized_redeem_and_burn`.
//!
//! `PoolGuard::new()` succeeds at most once at a time per canister. The
//! guard is released via `Drop`, which runs even if the callback traps
//! (since ic-cdk 0.5.1).
//!
//! Audit fence: B-01 (Wave 14a). Mirrors `rumi_amm::PoolGuard` with a
//! single-flag lock since this canister hosts exactly one pool.

use crate::types::ThreePoolError;
use std::cell::RefCell;

thread_local! {
    static POOL_LOCK: RefCell<bool> = const { RefCell::new(false) };
}

pub struct PoolGuard;

impl PoolGuard {
    /// Acquire the canister-wide pool lock. Returns `Err(PoolLocked)` if
    /// another mutating operation is already in flight.
    pub fn new() -> Result<Self, ThreePoolError> {
        POOL_LOCK.with(|lock| {
            let mut held = lock.borrow_mut();
            if *held {
                return Err(ThreePoolError::PoolLocked);
            }
            *held = true;
            Ok(Self)
        })
    }
}

impl Drop for PoolGuard {
    fn drop(&mut self) {
        POOL_LOCK.with(|lock| {
            *lock.borrow_mut() = false;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_is_exclusive() {
        let g1 = PoolGuard::new().expect("first acquire");
        assert!(matches!(PoolGuard::new(), Err(ThreePoolError::PoolLocked)));
        drop(g1);
        let _g2 = PoolGuard::new().expect("second acquire after drop");
    }
}
