//! Pure decisions over finalized native-XLM deposit and reserve evidence.
//!
//! Provider slot independence, HTTPS reads, freshness-policy values, and all
//! state writes remain caller responsibilities. This module only reduces
//! already-parsed evidence and produces a credit decision.

use super::{
    credit::{decide_credit, CreditDecision, RejectReason},
    horizon_parse::{AccountSnapshot, DepositKind, FinalizedDeposit, LatestLedgerReserve},
};
use crate::state::{
    State, XlmClaimStatus, XlmCustodyAttribution, XlmPendingDeposit, XlmPendingDepositStatus,
};

const NANOS_PER_SECOND: u128 = 1_000_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReceiptListQuorumError {
    NoQuorum,
    ConflictingDissent,
    DuplicateOperation,
}

/// Canonicalize each complete native receipt list before voting. Two lists
/// must agree exactly; any third affirmative but different list makes the
/// evidence ambiguous. Provider identity and slot independence are not checked
/// here and must be established by the caller.
pub fn finalized_deposit_list_quorum(
    observations: [Option<Vec<FinalizedDeposit>>; 3],
) -> Result<Vec<FinalizedDeposit>, ReceiptListQuorumError> {
    let mut normalized: [Option<Vec<FinalizedDeposit>>; 3] = [None, None, None];
    for (index, list) in observations.into_iter().enumerate() {
        if let Some(mut list) = list {
            list.sort_by(|left, right| {
                (&left.transaction_hash, &left.operation_id)
                    .cmp(&(&right.transaction_hash, &right.operation_id))
            });
            if list.windows(2).any(|pair| {
                pair[0].transaction_hash == pair[1].transaction_hash
                    && pair[0].operation_id == pair[1].operation_id
            }) {
                return Err(ReceiptListQuorumError::DuplicateOperation);
            }
            normalized[index] = Some(list);
        }
    }

    let matching = (0..3).find_map(|left| {
        let candidate = normalized[left].as_ref()?;
        ((left + 1)..3)
            .find(|right| normalized[*right].as_ref() == Some(candidate))
            .map(|right| (left, right, candidate))
    });
    let Some((left, right, receipts)) = matching else {
        return Err(ReceiptListQuorumError::NoQuorum);
    };
    if normalized.iter().enumerate().any(|(index, other)| {
        index != left && index != right && other.as_ref().is_some_and(|other| other != receipts)
    }) {
        return Err(ReceiptListQuorumError::ConflictingDissent);
    }
    Ok(receipts.clone())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvidenceDeferReason {
    AccountBehindReceipt,
    ReserveBehindEvidence,
    InvalidReserveTimestamp,
    ReserveTimestampInFuture,
    ReserveTooOld,
}

/// Check ledger ordering plus caller-selected wall-clock freshness for the
/// latest reserve observation. This compares `closed_at` with `now_unix_ns`;
/// it does not establish provider quorum or independent provider identities.
pub fn decide_evidence_freshness(
    receipt_ledger: u32,
    account: &AccountSnapshot,
    reserve: &LatestLedgerReserve,
    now_unix_ns: u128,
    max_reserve_age_secs: u64,
) -> Result<(), EvidenceDeferReason> {
    if account.account_last_modified_ledger < receipt_ledger {
        return Err(EvidenceDeferReason::AccountBehindReceipt);
    }
    if reserve.reserve.sequence == 0
        || reserve.reserve.base_reserve_in_stroops == 0
        || reserve.reserve.sequence < receipt_ledger
        || reserve.reserve.sequence < account.account_last_modified_ledger
    {
        return Err(EvidenceDeferReason::ReserveBehindEvidence);
    }
    let closed_at_ns = parse_utc_timestamp_ns(&reserve.closed_at)
        .ok_or(EvidenceDeferReason::InvalidReserveTimestamp)?;
    if closed_at_ns > now_unix_ns {
        return Err(EvidenceDeferReason::ReserveTimestampInFuture);
    }
    let max_age_ns = u128::from(max_reserve_age_secs)
        .checked_mul(NANOS_PER_SECOND)
        .ok_or(EvidenceDeferReason::ReserveTooOld)?;
    if now_unix_ns - closed_at_ns > max_age_ns {
        return Err(EvidenceDeferReason::ReserveTooOld);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitialCreditRejectReason {
    MissingPendingDeposit,
    InactivePendingDeposit,
    MissingOrConflictingAttribution,
    NetworkMismatch,
    ReceiptNotInQuorum,
    ReceiptList(ReceiptListQuorumError),
    ReceiptAlreadyRecorded,
    VaultAlreadyFunded,
    ReceiptAccountMismatch,
    ArithmeticOverflow,
    Credit(RejectReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitialCreditDecision {
    Credit { amount_stroops: u64 },
    Defer(EvidenceDeferReason),
    Reject(InitialCreditRejectReason),
}

/// Reduce quorum-backed receipt lists and current reserve evidence into a
/// first-funding credit decision for one pending XLM vault. The caller must
/// journal the returned receipt and credit atomically/idempotently before
/// retrying. `extra_reserved_headroom_stroops` is an additional amount kept
/// uncredited, in addition to the fee budget and live minimum reserve;
/// outstanding claims are read from State for the exact custody owner/nonce.
/// Account snapshot quorum is expected to have been established by the caller.
#[allow(clippy::too_many_arguments)]
pub fn decide_initial_funding_credit(
    state: &State,
    vault_id: u64,
    network_id: [u8; 32],
    receipt_lists: [Option<Vec<FinalizedDeposit>>; 3],
    receipt: &FinalizedDeposit,
    account: &AccountSnapshot,
    reserve: &LatestLedgerReserve,
    now_unix_ns: u128,
    max_reserve_age_secs: u64,
    extra_reserved_headroom_stroops: u64,
    fee_budget_stroops: u64,
) -> InitialCreditDecision {
    let quorum_receipts = match finalized_deposit_list_quorum(receipt_lists) {
        Ok(receipts) => receipts,
        Err(reason) => {
            return InitialCreditDecision::Reject(InitialCreditRejectReason::ReceiptList(reason))
        }
    };
    if !quorum_receipts.contains(receipt) {
        return InitialCreditDecision::Reject(InitialCreditRejectReason::ReceiptNotInQuorum);
    }
    if let Err(reason) = decide_evidence_freshness(
        receipt.ledger,
        account,
        reserve,
        now_unix_ns,
        max_reserve_age_secs,
    ) {
        return InitialCreditDecision::Defer(reason);
    }

    let Some(pending) = state.xlm_pending_deposits.get(&vault_id) else {
        return InitialCreditDecision::Reject(InitialCreditRejectReason::MissingPendingDeposit);
    };
    if !matches!(
        pending.status,
        XlmPendingDepositStatus::Open | XlmPendingDepositStatus::Pending
    ) {
        return InitialCreditDecision::Reject(InitialCreditRejectReason::InactivePendingDeposit);
    }
    if pending.network_id != network_id {
        return InitialCreditDecision::Reject(InitialCreditRejectReason::NetworkMismatch);
    }
    if !attribution_matches(
        state.xlm_custody_attributions.get(&pending.custody_address),
        pending,
    ) {
        return InitialCreditDecision::Reject(
            InitialCreditRejectReason::MissingOrConflictingAttribution,
        );
    }
    if receipt.destination != pending.custody_address
        || account.account_id != pending.custody_address
    {
        return InitialCreditDecision::Reject(InitialCreditRejectReason::ReceiptAccountMismatch);
    }
    // FinalizedDeposit has only these two variants. Keep the accepted set
    // explicit so a future parser expansion cannot silently enter first funding.
    if !matches!(
        receipt.kind,
        DepositKind::CreateAccount | DepositKind::NativePayment
    ) {
        return InitialCreditDecision::Reject(InitialCreditRejectReason::ReceiptNotInQuorum);
    }
    let receipt_key = (
        receipt.transaction_hash.clone(),
        receipt.operation_id.clone(),
    );
    if state.xlm_deposit_receipts.contains_key(&receipt_key) {
        return InitialCreditDecision::Reject(InitialCreditRejectReason::ReceiptAlreadyRecorded);
    }
    if state
        .xlm_deposit_receipts
        .values()
        .any(|record| record.vault_id == vault_id)
        || state.vault_id_to_vaults.contains_key(&vault_id)
    {
        return InitialCreditDecision::Reject(InitialCreditRejectReason::VaultAlreadyFunded);
    }

    let mut claim_obligations = 0_u64;
    for claim in state.xlm_claims.values().filter(|claim| {
        claim.custody_owner == pending.owner && claim.custody_nonce == pending.derivation_nonce
    }) {
        if claim.status != XlmClaimStatus::Settled {
            let Some(total) = claim_obligations
                .checked_add(claim.gross_stroops)
                .and_then(|value| value.checked_add(claim.reserved_fee_stroops))
            else {
                return InitialCreditDecision::Reject(
                    InitialCreditRejectReason::ArithmeticOverflow,
                );
            };
            claim_obligations = total;
        }
    }
    let Some(total_fee_budget) = extra_reserved_headroom_stroops.checked_add(fee_budget_stroops)
    else {
        return InitialCreditDecision::Reject(InitialCreditRejectReason::ArithmeticOverflow);
    };
    let credit = decide_credit(
        receipt,
        account,
        &reserve.reserve,
        total_fee_budget,
        &[claim_obligations],
        true,
    );
    match credit {
        CreditDecision::Credit { amount_stroops } => {
            InitialCreditDecision::Credit { amount_stroops }
        }
        CreditDecision::Defer(_) => {
            InitialCreditDecision::Defer(EvidenceDeferReason::ReserveBehindEvidence)
        }
        CreditDecision::Reject(reason) => {
            InitialCreditDecision::Reject(InitialCreditRejectReason::Credit(reason))
        }
    }
}

fn attribution_matches(
    attribution: Option<&XlmCustodyAttribution>,
    pending: &XlmPendingDeposit,
) -> bool {
    attribution.is_some_and(|attribution| {
        attribution.owner == pending.owner
            && attribution.derivation_nonce == pending.derivation_nonce
            && attribution.network_id == pending.network_id
    })
}

/// Parse Horizon's UTC `closed_at` form into Unix nanoseconds. Sub-nanosecond
/// fractional digits are rounded down, which makes freshness conservative.
fn parse_utc_timestamp_ns(value: &str) -> Option<u128> {
    let value = value.strip_suffix('Z')?;
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.len() != 19
        || whole.as_bytes().get(4) != Some(&b'-')
        || whole.as_bytes().get(7) != Some(&b'-')
        || whole.as_bytes().get(10) != Some(&b'T')
        || whole.as_bytes().get(13) != Some(&b':')
        || whole.as_bytes().get(16) != Some(&b':')
        || (value.contains('.') && fraction.is_empty())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let parse = |start, end| whole.get(start..end)?.parse::<u32>().ok();
    let (year, month, day, hour, minute, second) = (
        parse(0, 4)?,
        parse(5, 7)?,
        parse(8, 10)?,
        parse(11, 13)?,
        parse(14, 16)?,
        parse(17, 19)?,
    );
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if day == 0 || day > days_in_month || hour >= 24 || minute >= 60 || second > 60 {
        return None;
    }

    let mut adjusted_year = i64::from(year);
    if month <= 2 {
        adjusted_year -= 1;
    }
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let month_prime = i64::from(month) + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days_since_epoch = era * 146_097 + day_of_era - 719_468;
    if days_since_epoch < 0 {
        return None;
    }
    let seconds = u128::try_from(days_since_epoch)
        .ok()?
        .checked_mul(86_400)?
        .checked_add(u128::from(hour) * 3_600)?
        .checked_add(u128::from(minute) * 60)?
        .checked_add(u128::from(second))?;
    let fractional_nanos = if fraction.is_empty() {
        0
    } else {
        let nanos = fraction.chars().take(9).collect::<String>();
        let parsed = nanos.parse::<u32>().ok()?;
        u128::from(parsed) * 10_u128.pow((9 - nanos.len()) as u32)
    };
    seconds
        .checked_mul(NANOS_PER_SECOND)?
        .checked_add(fractional_nanos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{
        XlmClaim, XlmClaimStatus, XlmCustodyAttribution, XlmDepositKind, XlmDepositReceipt,
    };
    use crate::{numeric::ICUSD, vault::Vault};

    const DEST: &str = "GDEST";
    const NETWORK: [u8; 32] = [7; 32];
    const CLOSED_AT: &str = "2026-10-27T12:00:00.000000000Z";

    fn receipt(kind: DepositKind, operation_id: &str, amount_stroops: u64) -> FinalizedDeposit {
        FinalizedDeposit {
            transaction_hash: "TX".into(),
            ledger: 42,
            operation_id: operation_id.into(),
            destination: DEST.into(),
            amount_stroops,
            kind,
        }
    }

    fn account() -> AccountSnapshot {
        AccountSnapshot {
            account_id: DEST.into(),
            sequence: 100,
            account_last_modified_ledger: 42,
            native_balance_stroops: 20_000_000,
            native_selling_liabilities_stroops: 0,
            subentry_count: 0,
            num_sponsoring: 0,
            num_sponsored: 0,
        }
    }

    fn latest_reserve() -> LatestLedgerReserve {
        LatestLedgerReserve {
            reserve: super::super::horizon_parse::LedgerReserve {
                sequence: 42,
                base_reserve_in_stroops: 5_000_000,
            },
            closed_at: CLOSED_AT.into(),
        }
    }

    fn state_with_pending(vault_id: u64) -> State {
        let mut state = State::default();
        let owner = candid::Principal::anonymous();
        state.xlm_pending_deposits.insert(
            vault_id,
            XlmPendingDeposit {
                owner,
                custody_address: DEST.into(),
                derivation_nonce: vault_id,
                network_id: NETWORK,
                opened_at_ns: 1,
                expires_at_ns: u128::MAX as u64,
                status: XlmPendingDepositStatus::Open,
            },
        );
        state.xlm_custody_attributions.insert(
            DEST.into(),
            XlmCustodyAttribution {
                owner,
                derivation_nonce: vault_id,
                network_id: NETWORK,
            },
        );
        state
    }

    fn now_ns() -> u128 {
        parse_utc_timestamp_ns(CLOSED_AT).unwrap() + 5 * NANOS_PER_SECOND
    }

    #[test]
    fn receipt_lists_are_sorted_before_exact_two_of_three_vote() {
        let first = receipt(DepositKind::CreateAccount, "op-a", 11_000_000);
        let second = receipt(DepositKind::NativePayment, "op-b", 12_000_000);
        let result = finalized_deposit_list_quorum([
            Some(vec![first.clone(), second.clone()]),
            Some(vec![second.clone(), first.clone()]),
            None,
        ])
        .unwrap();
        assert_eq!(result, vec![first, second]);
    }

    #[test]
    fn any_affirmative_dissent_and_duplicate_identity_fail_closed() {
        let item = receipt(DepositKind::NativePayment, "op-a", 11_000_000);
        let dissent = receipt(DepositKind::NativePayment, "op-b", 12_000_000);
        assert_eq!(
            finalized_deposit_list_quorum([
                Some(vec![item.clone()]),
                Some(vec![item.clone()]),
                Some(vec![dissent]),
            ]),
            Err(ReceiptListQuorumError::ConflictingDissent)
        );
        assert_eq!(
            finalized_deposit_list_quorum([
                Some(vec![item.clone(), item.clone()]),
                Some(vec![item.clone()]),
                None,
            ]),
            Err(ReceiptListQuorumError::DuplicateOperation)
        );
    }

    #[test]
    fn freshness_checks_sequence_closed_at_future_and_age() {
        let account = account();
        let reserve = latest_reserve();
        assert_eq!(
            decide_evidence_freshness(42, &account, &reserve, now_ns(), 5),
            Ok(())
        );
        assert_eq!(
            decide_evidence_freshness(43, &account, &reserve, now_ns(), 5),
            Err(EvidenceDeferReason::AccountBehindReceipt)
        );
        assert_eq!(
            decide_evidence_freshness(42, &account, &reserve, now_ns(), 4),
            Err(EvidenceDeferReason::ReserveTooOld)
        );
        assert_eq!(
            decide_evidence_freshness(42, &account, &reserve, now_ns() - 1, 5),
            Err(EvidenceDeferReason::ReserveTimestampInFuture)
        );
        let mut invalid = reserve;
        invalid.closed_at = "not-a-time".into();
        assert_eq!(
            decide_evidence_freshness(42, &account, &invalid, now_ns(), 5),
            Err(EvidenceDeferReason::InvalidReserveTimestamp)
        );
    }

    #[test]
    fn initial_wrapper_accepts_create_account_or_native_payment_and_applies_credit() {
        let state = state_with_pending(1);
        for kind in [DepositKind::CreateAccount, DepositKind::NativePayment] {
            let receipt = receipt(kind, "op-1", 10_000_000);
            let lists = [
                Some(vec![receipt.clone()]),
                Some(vec![receipt.clone()]),
                None,
            ];
            assert_eq!(
                decide_initial_funding_credit(
                    &state,
                    1,
                    NETWORK,
                    lists,
                    &receipt,
                    &account(),
                    &latest_reserve(),
                    now_ns(),
                    30,
                    0,
                    0,
                ),
                InitialCreditDecision::Credit {
                    amount_stroops: 10_000_000
                }
            );
        }
    }

    #[test]
    fn wrapper_rejects_replayed_or_unattributed_receipt() {
        let mut state = state_with_pending(1);
        let receipt = receipt(DepositKind::NativePayment, "op-1", 10_000_000);
        let lists = [
            Some(vec![receipt.clone()]),
            Some(vec![receipt.clone()]),
            None,
        ];
        state.xlm_deposit_receipts.insert(
            (
                receipt.transaction_hash.clone(),
                receipt.operation_id.clone(),
            ),
            XlmDepositReceipt {
                vault_id: 1,
                owner: candid::Principal::anonymous(),
                destination: DEST.into(),
                ledger: 42,
                gross_stroops: 10_000_000,
                credited_stroops: 10_000_000,
                kind: XlmDepositKind::NativePayment,
                confirmed_at_ns: 1,
            },
        );
        let result = decide_initial_funding_credit(
            &state,
            1,
            NETWORK,
            lists,
            &receipt,
            &account(),
            &latest_reserve(),
            now_ns(),
            30,
            0,
            0,
        );
        assert_eq!(
            result,
            InitialCreditDecision::Reject(InitialCreditRejectReason::ReceiptAlreadyRecorded)
        );

        let mut unattributed = state_with_pending(2);
        unattributed.xlm_custody_attributions.clear();
        let fresh = receipt(DepositKind::NativePayment, "op-2", 10_000_000);
        let result = decide_initial_funding_credit(
            &unattributed,
            2,
            NETWORK,
            [Some(vec![fresh.clone()]), Some(vec![fresh.clone()]), None],
            &fresh,
            &account(),
            &latest_reserve(),
            now_ns(),
            30,
            0,
            0,
        );
        assert_eq!(
            result,
            InitialCreditDecision::Reject(
                InitialCreditRejectReason::MissingOrConflictingAttribution
            )
        );
    }

    #[test]
    fn in_flight_claim_reserves_gross_amount_and_reserved_fee() {
        let mut state = state_with_pending(3);
        let owner = candid::Principal::anonymous();
        state.xlm_claims.insert(
            1,
            XlmClaim {
                vault_id: 3,
                custody_owner: owner,
                custody_nonce: 3,
                claimant: owner,
                gross_stroops: 6_000_000,
                destination: "GCLAIMANT".into(),
                network_id: NETWORK,
                reserved_fee_stroops: 1_000_000,
                created_at_ns: 1,
                status: XlmClaimStatus::InFlight,
                settlement_intent: None,
            },
        );
        let receipt = receipt(DepositKind::NativePayment, "op-3", 15_000_000);
        let mut account = account();
        account.native_balance_stroops = 28_000_000;
        let result = decide_initial_funding_credit(
            &state,
            3,
            NETWORK,
            [
                Some(vec![receipt.clone()]),
                Some(vec![receipt.clone()]),
                None,
            ],
            &receipt,
            &account,
            &latest_reserve(),
            now_ns(),
            30,
            0,
            0,
        );
        // 28M balance - 10M reserve - (6M gross + 1M fee) leaves 11M.
        assert_eq!(
            result,
            InitialCreditDecision::Credit {
                amount_stroops: 11_000_000
            }
        );
    }

    #[test]
    fn existing_vault_is_not_treated_as_free_first_funding_balance() {
        let mut state = state_with_pending(4);
        state.vault_id_to_vaults.insert(
            4,
            Vault {
                owner: candid::Principal::anonymous(),
                borrowed_icusd_amount: ICUSD::new(0),
                collateral_amount: 10_000_000,
                vault_id: 4,
                collateral_type: candid::Principal::anonymous(),
                last_accrual_time: 0,
                accrued_interest: ICUSD::new(0),
                bot_processing: false,
            },
        );
        let receipt = receipt(DepositKind::NativePayment, "op-4", 10_000_000);
        let result = decide_initial_funding_credit(
            &state,
            4,
            NETWORK,
            [
                Some(vec![receipt.clone()]),
                Some(vec![receipt.clone()]),
                None,
            ],
            &receipt,
            &account(),
            &latest_reserve(),
            now_ns(),
            30,
            0,
            0,
        );
        assert_eq!(
            result,
            InitialCreditDecision::Reject(InitialCreditRejectReason::VaultAlreadyFunded)
        );
    }
}
