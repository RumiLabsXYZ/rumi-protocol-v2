//! Pure semantic quorum rules for independent Stellar Horizon observations.
//!
//! Callers must place each distinct provider in a separate fixed slot. This
//! module does not establish provider independence or perform network I/O.

use super::horizon_parse::{AccountSnapshot, FinalizedDeposit, LedgerReserve};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuorumError {
    NoQuorum,
    ConflictingAccountState,
    ConflictingTransactionLedger,
    ConflictingDepositReceipt,
}

/// Accept two identical complete snapshots, including their deterministic
/// `account_last_modified_ledger`. A newer dissent or a same-ledger conflict
/// makes the observation ambiguous and is rejected even when the other two
/// slots match.
pub fn account_snapshot_quorum(
    observations: [Option<AccountSnapshot>; 3],
) -> Result<AccountSnapshot, QuorumError> {
    for left in 0..3 {
        let Some(candidate) = observations[left].as_ref() else {
            continue;
        };
        for right in (left + 1)..3 {
            let Some(other) = observations[right].as_ref() else {
                continue;
            };
            if candidate != other {
                continue;
            }

            for (slot, dissent) in observations.iter().enumerate() {
                if slot == left || slot == right {
                    continue;
                }
                if let Some(dissent) = dissent {
                    if dissent.account_id != candidate.account_id
                        || dissent.account_last_modified_ledger
                            > candidate.account_last_modified_ledger
                        || (dissent.account_last_modified_ledger
                            == candidate.account_last_modified_ledger
                            && dissent != candidate)
                    {
                        return Err(QuorumError::ConflictingAccountState);
                    }
                }
            }
            return Ok(candidate.clone());
        }
    }
    Err(QuorumError::NoQuorum)
}

/// Require two providers to agree on the base reserve. Return the maximum
/// reserve observed in any valid slot so disagreement can never lower the
/// reserve used by a caller.
pub fn ledger_reserve_quorum(observations: [Option<LedgerReserve>; 3]) -> Result<u64, QuorumError> {
    let valid: Vec<&LedgerReserve> = observations.iter().flatten().collect();
    let has_pair = valid.iter().enumerate().any(|(index, candidate)| {
        valid[index + 1..]
            .iter()
            .any(|other| candidate.base_reserve_in_stroops == other.base_reserve_in_stroops)
    });
    if !has_pair {
        return Err(QuorumError::NoQuorum);
    }
    valid
        .iter()
        .map(|reserve| reserve.base_reserve_in_stroops)
        .max()
        .ok_or(QuorumError::NoQuorum)
}

/// Require two matching affirmative transaction-ledger observations. Any
/// third affirmative observation for a different ledger is a conflict.
pub fn transaction_ledger_quorum(observations: [Option<u32>; 3]) -> Result<u32, QuorumError> {
    let matching = observations
        .iter()
        .enumerate()
        .find_map(|(left, candidate)| {
            let candidate = (*candidate)?;
            observations[left + 1..]
                .iter()
                .any(|other| *other == Some(candidate))
                .then_some(candidate)
        });
    let Some(ledger) = matching.filter(|ledger| *ledger > 0) else {
        return Err(QuorumError::NoQuorum);
    };
    if observations.iter().flatten().any(|other| *other != ledger) {
        return Err(QuorumError::ConflictingTransactionLedger);
    }
    Ok(ledger)
}

/// Require exact agreement on the complete parsed deposit operation receipt.
/// Field-wise voting is deliberately not performed; any affirmative dissent
/// from a third provider makes the receipt ambiguous.
pub fn finalized_deposit_quorum(
    observations: [Option<FinalizedDeposit>; 3],
) -> Result<FinalizedDeposit, QuorumError> {
    let matching = observations
        .iter()
        .enumerate()
        .find_map(|(left, candidate)| {
            let candidate = candidate.as_ref()?;
            observations[left + 1..]
                .iter()
                .any(|other| other.as_ref() == Some(candidate))
                .then_some(candidate)
        });
    let Some(receipt) = matching else {
        return Err(QuorumError::NoQuorum);
    };
    if observations.iter().flatten().any(|other| other != receipt) {
        return Err(QuorumError::ConflictingDepositReceipt);
    }
    Ok(receipt.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chains::stellar::horizon_parse::DepositKind;

    fn account(ledger: u32, balance: u64, sequence: u64) -> AccountSnapshot {
        AccountSnapshot {
            account_id: "GACCOUNT".to_string(),
            sequence,
            account_last_modified_ledger: ledger,
            native_balance_stroops: balance,
            native_selling_liabilities_stroops: 0,
            subentry_count: 2,
            num_sponsoring: 0,
            num_sponsored: 0,
        }
    }

    fn reserve(sequence: u32, value: u64) -> LedgerReserve {
        LedgerReserve {
            sequence,
            base_reserve_in_stroops: value,
        }
    }

    fn deposit(ledger: u32, operation_id: &str, amount_stroops: u64) -> FinalizedDeposit {
        FinalizedDeposit {
            transaction_hash: "TX_HASH".to_string(),
            ledger,
            operation_id: operation_id.to_string(),
            destination: "GACCOUNT".to_string(),
            amount_stroops,
            kind: DepositKind::NativePayment,
        }
    }

    #[test]
    fn accepts_two_identical_complete_account_snapshots() {
        let expected = account(42, 900, 7);
        assert_eq!(
            account_snapshot_quorum([Some(expected.clone()), Some(expected.clone()), None]),
            Ok(expected)
        );
    }

    #[test]
    fn rejects_newer_account_dissent_even_when_other_two_match() {
        let stale = account(42, 900, 7);
        assert_eq!(
            account_snapshot_quorum([Some(stale.clone()), Some(stale), Some(account(43, 800, 8)),]),
            Err(QuorumError::ConflictingAccountState)
        );
    }

    #[test]
    fn rejects_conflicting_account_at_same_modified_ledger() {
        let majority = account(42, 900, 7);
        assert_eq!(
            account_snapshot_quorum([
                Some(majority.clone()),
                Some(majority),
                Some(account(42, 901, 7)),
            ]),
            Err(QuorumError::ConflictingAccountState)
        );
    }

    #[test]
    fn rejects_dissent_for_another_account_id() {
        let majority = account(42, 900, 7);
        let mut wrong_account = account(41, 900, 7);
        wrong_account.account_id = "GWRONG".to_string();
        assert_eq!(
            account_snapshot_quorum([Some(majority.clone()), Some(majority), Some(wrong_account)]),
            Err(QuorumError::ConflictingAccountState)
        );
    }

    #[test]
    fn does_not_fall_back_to_one_account_provider() {
        assert_eq!(
            account_snapshot_quorum([Some(account(42, 900, 7)), None, None]),
            Err(QuorumError::NoQuorum)
        );
    }

    #[test]
    fn reserve_quorum_uses_conservative_maximum() {
        assert_eq!(
            ledger_reserve_quorum([
                Some(reserve(42, 5_000_000)),
                Some(reserve(43, 5_000_000)),
                Some(reserve(42, 6_000_000)),
            ]),
            Ok(6_000_000)
        );
    }

    #[test]
    fn does_not_fall_back_to_one_reserve_provider() {
        assert_eq!(
            ledger_reserve_quorum([Some(reserve(42, 5_000_000)), None, None]),
            Err(QuorumError::NoQuorum)
        );
    }

    #[test]
    fn accepts_two_matching_transaction_ledgers() {
        assert_eq!(
            transaction_ledger_quorum([Some(42), Some(42), None]),
            Ok(42)
        );
    }

    #[test]
    fn does_not_fall_back_to_one_transaction_ledger_provider() {
        assert_eq!(
            transaction_ledger_quorum([Some(42), None, None]),
            Err(QuorumError::NoQuorum)
        );
    }

    #[test]
    fn rejects_conflicting_third_transaction_ledger() {
        assert_eq!(
            transaction_ledger_quorum([Some(42), Some(42), Some(43)]),
            Err(QuorumError::ConflictingTransactionLedger)
        );
    }

    #[test]
    fn rejects_zero_transaction_ledger_even_if_two_sources_agree() {
        assert_eq!(
            transaction_ledger_quorum([Some(0), Some(0), None]),
            Err(QuorumError::NoQuorum)
        );
    }

    #[test]
    fn accepts_two_identical_complete_deposit_receipts() {
        let expected = deposit(42, "op-7", 20_000_000);
        assert_eq!(
            finalized_deposit_quorum([Some(expected.clone()), Some(expected.clone()), None]),
            Ok(expected)
        );
    }

    #[test]
    fn does_not_fall_back_to_one_deposit_provider() {
        assert_eq!(
            finalized_deposit_quorum([Some(deposit(42, "op-7", 20_000_000)), None, None]),
            Err(QuorumError::NoQuorum)
        );
    }

    #[test]
    fn rejects_conflicting_third_deposit_receipt() {
        let majority = deposit(42, "op-7", 20_000_000);
        assert_eq!(
            finalized_deposit_quorum([
                Some(majority.clone()),
                Some(majority),
                Some(deposit(43, "op-8", 20_000_000)),
            ]),
            Err(QuorumError::ConflictingDepositReceipt)
        );
    }
}
