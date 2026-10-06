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
        Self::acquire(None)
    }

    /// Resume the sole active receipt that owns the durable fence. Every other
    /// unresolved receipt/ingress still blocks this operation.
    pub fn new_for_receipt(owner: candid::Principal, intent_id: &[u8]) -> Result<Self, ThreePoolError> {
        Self::acquire(Some((owner, intent_id)))
    }

    fn acquire(receipt: Option<(candid::Principal, &[u8])>) -> Result<Self, ThreePoolError> {
        let acquired = POOL_LOCK.with(|lock| {
            let mut held = lock.borrow_mut();
            if *held { return false; }
            *held = true;
            true
        });
        if !acquired { return Err(ThreePoolError::PoolLocked); }

        // Check after taking the synchronous lock, so no update can change
        // receipt ownership between authorization and acquisition.
        let allowed = match receipt {
            Some((owner, intent_id)) => crate::receipts::is_only_active_receipt(owner, intent_id),
            None => !crate::receipts::fenced(),
        };
        if !allowed {
            POOL_LOCK.with(|lock| *lock.borrow_mut() = false);
            return Err(ThreePoolError::PoolLocked);
        }
        Ok(Self)
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

    #[test]
    fn receipt_guard_resumes_only_its_own_active_fence() {
        let owner = candid::Principal::self_authenticating(b"guard-owner");
        let mut request = crate::receipts::SwapRequestV1 {
            intent_id: vec![0; 32], i: 0, j: 1, dx: 1, min_dy: 0,
        };
        request.intent_id[7] = 1;
        let (mut receipt, _) = crate::receipts::reserve(owner, request.clone()).unwrap();
        assert!(crate::receipts::fenced());
        {
            let _guard = PoolGuard::new_for_receipt(owner, &request.intent_id).unwrap();
            assert!(PoolGuard::new().is_err());
        }
        receipt.status = crate::receipts::SwapReceiptStatusV1::Completed;
        crate::receipts::save(&receipt);
        crate::receipts::set_fence(false);
        assert!(!crate::receipts::fenced());
    }

    #[test]
    fn receipt_guard_rejects_other_active_receipt_or_ingress() {
        let owner = candid::Principal::self_authenticating(b"guard-owner-a");
        let other = candid::Principal::self_authenticating(b"guard-owner-b");
        let mut request = crate::receipts::SwapRequestV1 {
            intent_id: vec![0; 32], i: 0, j: 1, dx: 1, min_dy: 0,
        };
        request.intent_id[7] = 1;
        let (mut swap, _) = crate::receipts::reserve(owner, request.clone()).unwrap();
        let mut ingress_id = vec![0; 32];
        ingress_id[7] = 1;
        let (mut ingress, _) = crate::receipts::reserve_ingress(
            other,
            ingress_id.clone(),
            crate::receipts::IngressRequestV1::Donate { token_index: 0, amount: 1 },
        ).unwrap();
        assert!(crate::receipts::is_only_active_receipt(owner, &request.intent_id) == false);
        assert!(PoolGuard::new_for_receipt(owner, &request.intent_id).is_err());
        swap.status = crate::receipts::SwapReceiptStatusV1::Completed;
        ingress.status = crate::receipts::IngressStatusV1::Completed;
        crate::receipts::save(&swap);
        crate::receipts::save_ingress(&ingress);
        crate::receipts::set_fence(false);
        assert!(!crate::receipts::fenced());
    }
}
