//! Pure XLM receipt-to-credit decision logic.
//!
//! This module does not fetch evidence, mutate vault state, or enable Stellar
//! admission. Its caller must provide a proven finalized operation receipt, an
//! account snapshot, and an independently observed current ledger reserve.

use super::{
    amount::{minimum_balance_stroops, spendable_stroops, AmountError},
    horizon_parse::{AccountSnapshot, FinalizedDeposit, LedgerReserve},
};

/// Smallest net XLM amount eligible for credit under the native collateral plan.
pub const MIN_CREDIT_STROOPS: u64 = 10_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeferReason {
    AccountBehindReceipt,
    ReserveBehindEvidence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RejectReason {
    ZeroReceipt,
    ReceiptDestinationMismatch,
    AccountBelowMinimum,
    NoSpendableBalance,
    BelowMinimumCredit,
    PartialCreditNotFirstFunding,
    InvalidReserve,
    ArithmeticOverflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreditDecision {
    Credit { amount_stroops: u64 },
    Defer(DeferReason),
    Reject(RejectReason),
}

/// Decide how much of one finalized deposit can be credited while preserving
/// the account's live reserve, selling liabilities, pending claims, and fee
/// budget. Partial credit is allowed; any remainder is intentionally not
/// carried forward by this pure decision. The caller must persistently
/// journal/idempotently bind `is_first_funding` to this receipt before applying
/// a credit. The caller must also establish that the supplied reserve has
/// current age and quorum; sequence comparisons here only ensure it is not
/// behind the account or receipt evidence.
pub fn decide_credit(
    receipt: &FinalizedDeposit,
    account: &AccountSnapshot,
    current_reserve: &LedgerReserve,
    fee_budget_stroops: u64,
    outstanding_claims_stroops: &[u64],
    is_first_funding: bool,
) -> CreditDecision {
    if receipt.amount_stroops == 0 {
        return CreditDecision::Reject(RejectReason::ZeroReceipt);
    }
    if receipt.destination != account.account_id {
        return CreditDecision::Reject(RejectReason::ReceiptDestinationMismatch);
    }
    if account.account_last_modified_ledger < receipt.ledger {
        return CreditDecision::Defer(DeferReason::AccountBehindReceipt);
    }
    if current_reserve.sequence < receipt.ledger
        || current_reserve.sequence < account.account_last_modified_ledger
    {
        return CreditDecision::Defer(DeferReason::ReserveBehindEvidence);
    }

    let minimum = match minimum_balance_stroops(
        current_reserve.base_reserve_in_stroops,
        account.subentry_count,
        account.num_sponsoring,
        account.num_sponsored,
    ) {
        Ok(minimum) => minimum,
        Err(AmountError::Overflow) => {
            return CreditDecision::Reject(RejectReason::ArithmeticOverflow)
        }
        Err(_) => return CreditDecision::Reject(RejectReason::InvalidReserve),
    };
    if account.native_balance_stroops < minimum {
        return CreditDecision::Reject(RejectReason::AccountBelowMinimum);
    }

    let pending_obligations = outstanding_claims_stroops.iter().try_fold(
        account.native_selling_liabilities_stroops,
        |total, amount| total.checked_add(*amount),
    );
    let Some(pending_obligations) = pending_obligations else {
        return CreditDecision::Reject(RejectReason::ArithmeticOverflow);
    };
    let spendable = match spendable_stroops(
        account.native_balance_stroops,
        minimum,
        &[pending_obligations],
        fee_budget_stroops,
    ) {
        Ok(spendable) => spendable,
        Err(AmountError::Overflow) => {
            return CreditDecision::Reject(RejectReason::ArithmeticOverflow)
        }
        Err(AmountError::InsufficientBalance) => {
            return CreditDecision::Reject(RejectReason::NoSpendableBalance)
        }
        Err(_) => return CreditDecision::Reject(RejectReason::InvalidReserve),
    };
    let credit = receipt.amount_stroops.min(spendable);
    if credit < MIN_CREDIT_STROOPS {
        CreditDecision::Reject(RejectReason::BelowMinimumCredit)
    } else if credit < receipt.amount_stroops && !is_first_funding {
        CreditDecision::Reject(RejectReason::PartialCreditNotFirstFunding)
    } else {
        CreditDecision::Credit {
            amount_stroops: credit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chains::stellar::horizon_parse::DepositKind;

    fn receipt(ledger: u32, amount_stroops: u64) -> FinalizedDeposit {
        FinalizedDeposit {
            transaction_hash: "tx".into(),
            ledger,
            operation_id: format!("{}", u64::from(ledger) << 32 | 1),
            destination: "GDEST".into(),
            amount_stroops,
            kind: DepositKind::NativePayment,
        }
    }

    fn account(ledger: u32, balance: u64) -> AccountSnapshot {
        AccountSnapshot {
            account_id: "GDEST".into(),
            sequence: 1,
            account_last_modified_ledger: ledger,
            native_balance_stroops: balance,
            native_selling_liabilities_stroops: 2_000,
            subentry_count: 0,
            num_sponsoring: 0,
            num_sponsored: 0,
        }
    }

    fn reserve(sequence: u32, base_reserve_in_stroops: u64) -> LedgerReserve {
        LedgerReserve {
            sequence,
            base_reserve_in_stroops,
        }
    }

    #[test]
    fn credits_at_most_receipt_and_never_spends_reserve_or_obligations() {
        let decision = decide_credit(
            &receipt(10, 10_000_000),
            &account(10, 30_000_000),
            &reserve(10, 5_000_000),
            1_000,
            &[3_000],
            true,
        );
        // 2 reserve units + liabilities + fee budget leave more than the
        // receipt amount, so the receipt caps credit at 1 XLM.
        assert_eq!(
            decision,
            CreditDecision::Credit {
                amount_stroops: MIN_CREDIT_STROOPS
            }
        );
    }

    #[test]
    fn partial_credit_is_capped_by_spendable_balance() {
        let decision = decide_credit(
            &receipt(10, 50_000_000),
            &account(10, 20_020_000),
            &reserve(10, 5_000_000),
            1_000,
            &[3_000],
            true,
        );
        assert_eq!(
            decision,
            CreditDecision::Credit {
                amount_stroops: 10_014_000
            }
        );
    }

    #[test]
    fn defers_stale_account_or_reserve_evidence() {
        assert_eq!(
            decide_credit(
                &receipt(11, 10_000),
                &account(10, 30_000),
                &reserve(11, 5_000),
                0,
                &[],
                true
            ),
            CreditDecision::Defer(DeferReason::AccountBehindReceipt)
        );
        assert_eq!(
            decide_credit(
                &receipt(10, 10_000),
                &account(11, 30_000),
                &reserve(10, 5_000_000),
                0,
                &[],
                true
            ),
            CreditDecision::Defer(DeferReason::ReserveBehindEvidence)
        );
    }

    #[test]
    fn rejects_zero_mismatch_below_minimum_and_overflow() {
        assert_eq!(
            decide_credit(
                &receipt(10, 0),
                &account(10, 30_000),
                &reserve(10, 5_000),
                0,
                &[],
                true
            ),
            CreditDecision::Reject(RejectReason::ZeroReceipt)
        );
        let mut mismatch = receipt(10, 1);
        mismatch.destination = "GOTHER".into();
        assert_eq!(
            decide_credit(
                &mismatch,
                &account(10, 30_000),
                &reserve(10, 5_000),
                0,
                &[],
                true
            ),
            CreditDecision::Reject(RejectReason::ReceiptDestinationMismatch)
        );
        assert_eq!(
            decide_credit(
                &receipt(10, 1),
                &account(10, 9_999),
                &reserve(10, 5_000),
                0,
                &[],
                true
            ),
            CreditDecision::Reject(RejectReason::AccountBelowMinimum)
        );
        assert_eq!(
            decide_credit(
                &receipt(10, 1),
                &account(10, 30_000),
                &reserve(10, u64::MAX),
                0,
                &[],
                true
            ),
            CreditDecision::Reject(RejectReason::ArithmeticOverflow)
        );
    }

    #[test]
    fn never_credits_when_reserve_and_obligations_leave_nothing_spendable() {
        assert_eq!(
            decide_credit(
                &receipt(10, 1),
                &account(10, 10_000),
                &reserve(10, 5_000),
                0,
                &[],
                true,
            ),
            CreditDecision::Reject(RejectReason::NoSpendableBalance)
        );
    }

    #[test]
    fn rejects_credit_below_plan_minimum_and_non_first_partial_credit() {
        assert_eq!(
            decide_credit(
                &receipt(10, MIN_CREDIT_STROOPS - 1),
                &account(10, 30_000_000),
                &reserve(10, 5_000),
                0,
                &[],
                true,
            ),
            CreditDecision::Reject(RejectReason::BelowMinimumCredit)
        );
        assert_eq!(
            decide_credit(
                &receipt(10, 15_000_000),
                &account(10, MIN_CREDIT_STROOPS + 12_000_000),
                &reserve(10, 5_000_000),
                0,
                &[],
                false,
            ),
            CreditDecision::Reject(RejectReason::PartialCreditNotFirstFunding)
        );
    }
}
