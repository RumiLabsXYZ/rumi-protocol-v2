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
        // Receipt attempts retain this fence across a callback trap/upgrade.
        // Ordinary unpause cannot authorize reserve movement while unresolved.
        if crate::receipts::fenced()
            || crate::storage::payouts::fenced()
            || crate::storage::liquidity_receipt_fence().is_some()
        {
            return Err(ThreePoolError::PoolLocked);
        }
        Self::acquire()
    }

    /// Acquire the global pool admission lock for the asynchronous, read-only
    /// preflight of a fresh liquidity receipt.  The lock is deliberately held
    /// across the ledger balance/allowance queries so two fresh intents cannot
    /// both pass against the same reserve snapshot and then consume receipt
    /// capacity.  The caller must hand this guard directly to the receipt
    /// operation before dropping it.
    pub(crate) fn new_liquidity_receipt_admission() -> Result<Self, ThreePoolError> {
        if crate::receipts::fenced()
            || crate::storage::payouts::fenced()
            || crate::storage::liquidity_receipt_fence().is_some()
        {
            return Err(ThreePoolError::PoolLocked);
        }
        Self::acquire()
    }

    /// Recovery for an ordinary payout can cross its own durable payout fence,
    /// but never a receipt-backed swap fence.
    pub(crate) fn new_payout_recovery() -> Result<Self, ThreePoolError> {
        if crate::receipts::fenced()
            || crate::storage::liquidity_receipt_fence().is_some()
        {
            return Err(ThreePoolError::PoolLocked);
        }
        Self::acquire()
    }

    /// Acquire the pool lock for the exact durable add-liquidity receipt that
    /// owns the persisted fence.  This is the only bypass for that fence; swap
    /// and unrelated payout fences remain blocking.
    pub(crate) fn new_for_liquidity_receipt(
        receipt: &crate::liquidity_receipts::LiquidityReceiptV1,
    ) -> Result<Self, ThreePoolError> {
        Self::validate_for_liquidity_receipt(receipt)?;
        Self::acquire()
    }

    /// Convert a held fresh-receipt admission guard into the exact durable
    /// receipt guard without an await or an unlock/relock gap.  The durable
    /// receipt fence and any owned payout fence must already be present.
    pub(crate) fn handoff_to_liquidity_receipt(
        self,
        receipt: &crate::liquidity_receipts::LiquidityReceiptV1,
    ) -> Result<Self, ThreePoolError> {
        Self::validate_for_liquidity_receipt(receipt)?;
        Ok(self)
    }

    fn validate_for_liquidity_receipt(
        receipt: &crate::liquidity_receipts::LiquidityReceiptV1,
    ) -> Result<(), ThreePoolError> {
        if crate::receipts::fenced() {
            return Err(ThreePoolError::PoolLocked);
        }
        let intent_id: [u8; 32] = receipt
            .request
            .intent_id
            .as_slice()
            .try_into()
            .map_err(|_| ThreePoolError::PoolLocked)?;
        if crate::storage::liquidity_receipt_fence()
            != Some((receipt.owner, intent_id))
        {
            return Err(ThreePoolError::PoolLocked);
        }
        if crate::storage::payouts::fenced() {
            // A receipt may cross only the payout fence belonging to one of
            // its own prebound add-liquidity refund identities.  The payout
            // row is checked as well so an ID collision or a stale receipt
            // cannot bypass an unrelated swap/output fence.
            let Some(fence_id) = crate::storage::payouts::fence_id() else {
                return Err(ThreePoolError::PoolLocked);
            };
            let owns_fence = receipt
                .input_ids
                .iter()
                .chain(receipt.refund_ids.iter())
                .flatten()
                .any(|id| *id == fence_id);
            let Some(entitlement) = crate::payouts::get(fence_id) else {
                return Err(ThreePoolError::PoolLocked);
            };
            if !owns_fence
                || entitlement.owner != receipt.owner
                || entitlement.kind != crate::payouts::PayoutKind::AddLiquidityRefund
                || entitlement.input_action
                    != Some(crate::payouts::PayoutInputAction::AddLiquidity)
            {
                return Err(ThreePoolError::PoolLocked);
            }
        }
        Ok(())
    }

    fn acquire() -> Result<Self, ThreePoolError> {
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
    use crate::liquidity_receipts::{self, AddLiquidityRequestV1, LiquidityReceiptV1};
    use crate::payouts::{
        PayoutAttempt, PayoutEntitlement, PayoutInputAction, PayoutInputOutcome, PayoutInputTransfer,
        PayoutKind, PayoutOutcome, PayoutTransfer,
    };
    use crate::storage;
    use candid::Principal;
    use icrc_ledger_types::icrc1::account::Account;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn receipt(owner: Principal, intent_id: [u8; 32]) -> LiquidityReceiptV1 {
        liquidity_receipts::reserve(
            owner,
            AddLiquidityRequestV1 {
                intent_id: intent_id.to_vec(),
                amounts: vec![100, 0, 0],
                min_lp: 1,
                source_subaccount: None,
                max_transfer_fee: Some(10),
            },
        )
        .expect("test receipt reservation")
        .0
    }

    fn refund_entitlement(id: u64, owner: Principal, kind: PayoutKind) -> PayoutEntitlement {
        let ledger = Principal::self_authenticating(b"guard test ledger");
        let source = Account {
            owner,
            subaccount: None,
        };
        let pool = Account {
            owner: Principal::self_authenticating(b"guard test pool"),
            subaccount: None,
        };
        PayoutEntitlement {
            id,
            owner,
            token_index: 0,
            ledger,
            symbol: "TEST".into(),
            gross: 100,
            kind,
            swap_context: None,
            compensation_id: None,
            compensation_for: None,
            dispatch_ready: Some(false),
            input_transfer: Some(PayoutInputTransfer {
                ledger,
                from: source.clone(),
                to: pool.clone(),
                amount: 100,
                fee: Some(1),
                memo: Some(vec![1; 32]),
                created_at_time: 123,
            }),
            input_outcome: Some(PayoutInputOutcome::Prepared),
            input_action: Some(PayoutInputAction::AddLiquidity),
            settled: false,
            attempts: vec![PayoutAttempt {
                number: 0,
                replay_count: 0,
                transfer: PayoutTransfer {
                    ledger,
                    from: pool,
                    to: source,
                    gross: 100,
                    net: 99,
                    fee: 1,
                    memo: vec![2; 32],
                    created_at_time: 123,
                },
                outcome: PayoutOutcome::Prepared,
            }],
        }
    }

    #[test]
    fn guard_is_exclusive() {
        let _serial = TEST_LOCK.lock().expect("test lock");
        let g1 = PoolGuard::new().expect("first acquire");
        assert!(matches!(PoolGuard::new(), Err(ThreePoolError::PoolLocked)));
        drop(g1);
        let _g2 = PoolGuard::new().expect("second acquire after drop");
    }

    #[test]
    fn fresh_receipt_admission_blocks_generic_pool_mutation_until_handoff() {
        let _serial = TEST_LOCK.lock().expect("test lock");
        let owner = Principal::self_authenticating(b"admission owner");
        let intent_id = [43; 32];
        let receipt = receipt(owner, intent_id);
        let admission = PoolGuard::new_liquidity_receipt_admission()
            .expect("fresh receipt admission");
        assert!(matches!(PoolGuard::new(), Err(ThreePoolError::PoolLocked)));
        liquidity_receipts::claim_active(owner, &intent_id).expect("claim receipt fence");
        let handed = admission
            .handoff_to_liquidity_receipt(&receipt)
            .expect("handoff retains global lock");
        assert!(matches!(PoolGuard::new(), Err(ThreePoolError::PoolLocked)));
        drop(handed);
        assert!(storage::clear_liquidity_receipt_fence(owner, intent_id));
    }

    #[test]
    fn durable_receipt_fence_blocks_generic_and_allows_exact_owner_recovery() {
        let _serial = TEST_LOCK.lock().expect("test lock");
        let owner = Principal::self_authenticating(b"receipt fence owner");
        let intent_id = [41; 32];
        let receipt = receipt(owner, intent_id);
        liquidity_receipts::claim_active(owner, &intent_id).expect("claim receipt fence");

        assert!(matches!(PoolGuard::new(), Err(ThreePoolError::PoolLocked)));
        let guard = PoolGuard::new_for_liquidity_receipt(&receipt).expect("own recovery guard");
        drop(guard);
        assert!(storage::clear_liquidity_receipt_fence(owner, intent_id));
    }

    #[test]
    fn receipt_payout_fence_requires_exact_id_owner_and_kind() {
        let _serial = TEST_LOCK.lock().expect("test lock");
        let owner = Principal::self_authenticating(b"payout fence owner");
        let intent_id = [42; 32];
        let payout_id = 9_100_001;
        let mut receipt = receipt(owner, intent_id);
        receipt.input_ids[0] = Some(payout_id);
        receipt.refund_ids[0] = Some(payout_id);
        liquidity_receipts::claim_active(owner, &intent_id).expect("claim receipt fence");
        storage::payouts::insert(refund_entitlement(payout_id, owner, PayoutKind::AddLiquidityRefund));
        storage::payouts::set_fence_for(payout_id);

        let guard = PoolGuard::new_for_liquidity_receipt(&receipt).expect("own payout fence");
        drop(guard);

        let mut wrong_id = receipt.clone();
        wrong_id.input_ids[0] = Some(payout_id + 1);
        wrong_id.refund_ids[0] = Some(payout_id + 1);
        assert!(matches!(
            PoolGuard::new_for_liquidity_receipt(&wrong_id),
            Err(ThreePoolError::PoolLocked)
        ));
        let wrong_owner = Principal::self_authenticating(b"different payout owner");
        let mut wrong_owner_receipt = receipt.clone();
        wrong_owner_receipt.owner = wrong_owner;
        assert!(matches!(
            PoolGuard::new_for_liquidity_receipt(&wrong_owner_receipt),
            Err(ThreePoolError::PoolLocked)
        ));

        storage::payouts::clear_fence_for(payout_id);
        storage::payouts::remove_no_effect(payout_id, owner);
        storage::clear_liquidity_receipt_fence(owner, intent_id);
    }

    #[test]
    fn receipt_payout_fence_rejects_unrelated_kind_and_missing_row() {
        let _serial = TEST_LOCK.lock().expect("test lock");
        let owner = Principal::self_authenticating(b"unrelated fence owner");
        let intent_id = [43; 32];
        let payout_id = 9_100_002;
        let mut receipt = receipt(owner, intent_id);
        receipt.input_ids[0] = Some(payout_id);
        receipt.refund_ids[0] = Some(payout_id);
        liquidity_receipts::claim_active(owner, &intent_id).expect("claim receipt fence");

        storage::payouts::insert(refund_entitlement(payout_id, owner, PayoutKind::SwapOutput));
        storage::payouts::set_fence_for(payout_id);
        assert!(matches!(
            PoolGuard::new_for_liquidity_receipt(&receipt),
            Err(ThreePoolError::PoolLocked)
        ));

        storage::payouts::remove_no_effect(payout_id, owner);
        assert!(matches!(
            PoolGuard::new_for_liquidity_receipt(&receipt),
            Err(ThreePoolError::PoolLocked)
        ));
        storage::payouts::clear_fence_for(payout_id);
        storage::clear_liquidity_receipt_fence(owner, intent_id);
    }

    #[test]
    fn stale_terminal_copy_cannot_clear_nonterminal_receipt_fence() {
        let _serial = TEST_LOCK.lock().expect("test lock");
        let owner = Principal::self_authenticating(b"terminal fence owner");
        let intent_id = [44; 32];
        let mut durable = receipt(owner, intent_id);
        liquidity_receipts::claim_active(owner, &intent_id).expect("claim receipt fence");

        let mut stale = durable.clone();
        stale.status = crate::liquidity_receipts::LiquidityReceiptStatusV1::Completed;
        assert!(!liquidity_receipts::clear_if_terminal(&stale));
        assert!(storage::liquidity_receipt_fence().is_some());

        durable.status = crate::liquidity_receipts::LiquidityReceiptStatusV1::Completed;
        liquidity_receipts::save(&durable);
        assert!(liquidity_receipts::clear_if_terminal(&durable));
        assert!(storage::liquidity_receipt_fence().is_none());
    }
}
