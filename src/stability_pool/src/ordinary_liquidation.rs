//! Pure planning and receipt checks for ordinary Stability Pool liquidations.
//!
//! The durable request lifecycle belongs to `StabilityPoolState` and the
//! backend's request-ID rail.  This module deliberately keeps the arithmetic
//! and immutable-identity checks independent from those journals so they can
//! be called both by the normal liquidation worker and by recovery after an
//! upgrade.  In particular, it never derives a second request ID from a
//! changed vault or fee quote.

use candid::{CandidType, Principal};
use rumi_protocol_backend::sp_liquidation::{
    SpLiquidationPayoutTuple, SpLiquidationReceipt,
};
use rumi_protocol_backend::state::SpLiquidationPayment;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::types::{normalize_from_e8s, normalize_to_e8s};

/// Domain separator for ordinary SP request identities.
pub const ORDINARY_REQUEST_DOMAIN: &[u8] = b"RUMI-SP-ORDINARY-LIQUIDATION-V1";

/// A configured non-LP stablecoin candidate.  `available_raw` must already be
/// the sum of balances eligible for the frozen collateral type.  The caller
/// must take that snapshot before reserving the request.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StableCandidate {
    pub ledger: Principal,
    pub payment: SpLiquidationPayment,
    pub decimals: u8,
    pub priority: u8,
    pub active: bool,
    pub is_lp: bool,
    pub available_raw: u64,
}

/// One selected ordinary stablecoin leg.  The current rail intentionally
/// chooses one ledger per attempt; a later retry must retain this choice.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StableSelection {
    pub ledger: Principal,
    pub payment: SpLiquidationPayment,
    pub decimals: u8,
    pub amount_raw: u64,
    pub covered_debt_e8s: u64,
}

/// Choose one eligible non-LP ledger using the same descending-priority rule
/// as `compute_token_draw`, with principal bytes as the deterministic tie-break.
/// The amount is floored in native units, so the selected leg never claims to
/// cover more debt than its exact balance/value snapshot.
pub fn choose_single_stable(
    candidates: &[StableCandidate],
    target_debt_e8s: u64,
) -> Option<StableSelection> {
    if target_debt_e8s == 0 {
        return None;
    }
    let candidate = candidates
        .iter()
        .filter(|candidate| {
            candidate.active && !candidate.is_lp && candidate.available_raw > 0
        })
        .filter(|candidate| normalize_to_e8s(candidate.available_raw, candidate.decimals) > 0)
        .max_by(|left, right| {
            left.priority
                .cmp(&right.priority)
                .then_with(|| right.ledger.as_slice().cmp(left.ledger.as_slice()))
        })?;
    let available_e8s = normalize_to_e8s(candidate.available_raw, candidate.decimals);
    let covered_debt_e8s = target_debt_e8s.min(available_e8s);
    let amount_raw = normalize_from_e8s(covered_debt_e8s, candidate.decimals)
        .min(candidate.available_raw);
    let covered_debt_e8s = normalize_to_e8s(amount_raw, candidate.decimals).min(target_debt_e8s);
    (amount_raw > 0 && covered_debt_e8s > 0).then_some(StableSelection {
        ledger: candidate.ledger,
        payment: candidate.payment,
        decimals: candidate.decimals,
        amount_raw,
        covered_debt_e8s,
    })
}

/// Derive the one stable request identity for a vault and monotonic nonce.
/// The nonce is allocated by the persistent state owner; callers must never
/// replace it after an await or an upgrade.
pub fn ordinary_request_id(vault_id: u64, monotonic_nonce: u64) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(ORDINARY_REQUEST_DOMAIN);
    hasher.update(vault_id.to_be_bytes());
    hasher.update(monotonic_nonce.to_be_bytes());
    hasher.finalize().into()
}

/// The exact raw stable amount that must be removed from participant balances.
/// `approve_fee_raw` is charged once when approval succeeds; `revoke_fee_raw`
/// is included only when a revoke actually charged the pool.  Backend
/// principal, surcharge, and pull fee come from the immutable backend quote.
pub fn exact_stable_debit(
    principal_raw: u64,
    surcharge_raw: u64,
    pull_fee_raw: u64,
    approve_fee_raw: u64,
    revoke_fee_raw: u64,
) -> Option<u64> {
    principal_raw
        .checked_add(surcharge_raw)?
        .checked_add(pull_fee_raw)?
        .checked_add(approve_fee_raw)?
        .checked_add(revoke_fee_raw)
}

/// Returns the largest backend gross payment that can be admitted after
/// reserving the live ledger approval fee. A zero result cannot fund an
/// ordinary attempt and is therefore rejected before durable admission.
pub fn affordable_max_gross_raw(available_raw: u64, approval_fee_raw: u64) -> Option<u64> {
    available_raw
        .checked_sub(approval_fee_raw)
        .filter(|gross_raw| *gross_raw > 0)
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParticipantBalance {
    pub owner: Principal,
    pub balance_raw: u64,
    pub opted_in: bool,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StableDebitAllocation {
    pub owner: Principal,
    pub debit_raw: u64,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq)]
pub enum AllocationError {
    ZeroAmount,
    NoEligibleBalance,
    InsufficientBalance { requested: u64, available: u64 },
    ArithmeticOverflow,
}

fn sorted_eligible(participants: &[ParticipantBalance]) -> Vec<ParticipantBalance> {
    let mut eligible: Vec<_> = participants
        .iter()
        .filter(|participant| participant.opted_in && participant.balance_raw > 0)
        .cloned()
        .collect();
    eligible.sort_by(|left, right| left.owner.as_slice().cmp(right.owner.as_slice()));
    eligible
}

/// Allocate an exact stable debit proportionally to the frozen participant
/// balances.  Floors are assigned first and the residual raw units are then
/// distributed in principal-byte order, preserving both per-user caps and the
/// exact physical outflow.
pub fn allocate_exact_debit(
    participants: &[ParticipantBalance],
    debit_raw: u64,
) -> Result<Vec<StableDebitAllocation>, AllocationError> {
    if debit_raw == 0 {
        return Err(AllocationError::ZeroAmount);
    }
    let eligible = sorted_eligible(participants);
    if eligible.is_empty() {
        return Err(AllocationError::NoEligibleBalance);
    }
    let available = eligible.iter().try_fold(0u64, |sum, participant| {
        sum.checked_add(participant.balance_raw)
    }).ok_or(AllocationError::ArithmeticOverflow)?;
    if available < debit_raw {
        return Err(AllocationError::InsufficientBalance {
            requested: debit_raw,
            available,
        });
    }

    let mut allocations = Vec::with_capacity(eligible.len());
    let mut allocated = 0u64;
    for participant in &eligible {
        let share = (u128::from(debit_raw) * u128::from(participant.balance_raw)
            / u128::from(available)) as u64;
        allocated = allocated
            .checked_add(share)
            .ok_or(AllocationError::ArithmeticOverflow)?;
        allocations.push(StableDebitAllocation {
            owner: participant.owner,
            debit_raw: share,
        });
    }
    let mut remainder = debit_raw
        .checked_sub(allocated)
        .ok_or(AllocationError::ArithmeticOverflow)?;
    for allocation in &mut allocations {
        if remainder == 0 {
            break;
        }
        let capacity = eligible
            .iter()
            .find(|participant| participant.owner == allocation.owner)
            .map(|participant| participant.balance_raw.saturating_sub(allocation.debit_raw))
            .unwrap_or(0);
        let extra = capacity.min(remainder);
        allocation.debit_raw = allocation
            .debit_raw
            .checked_add(extra)
            .ok_or(AllocationError::ArithmeticOverflow)?;
        remainder -= extra;
    }
    if remainder != 0 {
        return Err(AllocationError::InsufficientBalance {
            requested: debit_raw,
            available,
        });
    }
    Ok(allocations)
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollateralGainAllocation {
    pub owner: Principal,
    pub gain_raw: u64,
}

/// Allocate a verified collateral net payout against the exact stable debit
/// allocations.  This avoids `process_liquidation_gains_at`, whose historical
/// floor path can leave stable debit dust uncharged.  The first sorted owner
/// receives the deterministic collateral remainder.
pub fn allocate_exact_gain(
    debit_allocations: &[StableDebitAllocation],
    collateral_gain_raw: u64,
) -> Result<Vec<CollateralGainAllocation>, AllocationError> {
    if collateral_gain_raw == 0 {
        return Err(AllocationError::ZeroAmount);
    }
    let mut weights: Vec<_> = debit_allocations
        .iter()
        .filter(|allocation| allocation.debit_raw > 0)
        .cloned()
        .collect();
    weights.sort_by(|left, right| left.owner.as_slice().cmp(right.owner.as_slice()));
    let total_debit = weights.iter().try_fold(0u64, |sum, allocation| {
        sum.checked_add(allocation.debit_raw)
    }).ok_or(AllocationError::ArithmeticOverflow)?;
    if total_debit == 0 || weights.is_empty() {
        return Err(AllocationError::NoEligibleBalance);
    }

    let mut allocations = Vec::with_capacity(weights.len());
    let mut allocated = 0u64;
    for allocation in &weights {
        let gain = (u128::from(collateral_gain_raw) * u128::from(allocation.debit_raw)
            / u128::from(total_debit)) as u64;
        allocated = allocated
            .checked_add(gain)
            .ok_or(AllocationError::ArithmeticOverflow)?;
        allocations.push(CollateralGainAllocation {
            owner: allocation.owner,
            gain_raw: gain,
        });
    }
    let remainder = collateral_gain_raw
        .checked_sub(allocated)
        .ok_or(AllocationError::ArithmeticOverflow)?;
    if let Some(first) = allocations.first_mut() {
        first.gain_raw = first
            .gain_raw
            .checked_add(remainder)
            .ok_or(AllocationError::ArithmeticOverflow)?;
    }
    Ok(allocations)
}

/// Check every backend receipt field that can influence SP accounting.  A
/// caller must additionally authenticate the collateral transfer against the
/// direct ledger block before applying the allocation plan.
pub fn validate_receipt_identity<'a>(
    receipt: &'a SpLiquidationReceipt,
    request_id: &[u8; 32],
    vault_id: u64,
    payment: SpLiquidationPayment,
    stable_ledger: Principal,
    collateral_ledger: Principal,
    expected_debt_e8s: u64,
    expected_principal_raw: u64,
    expected_surcharge_raw: u64,
    expected_pull_fee_raw: u64,
    expected_gross_payment_raw: u64,
    expected_collateral_gross_raw: u64,
    expected_collateral_net_raw: u64,
) -> Result<&'a SpLiquidationPayoutTuple, String> {
    if receipt.request_id.as_slice() != request_id
        || receipt.vault_id != vault_id
        || receipt.payment != payment
        || receipt.payment_tuple.ledger != stable_ledger
        || receipt.debt_e8s != expected_debt_e8s
        || receipt.principal_raw != expected_principal_raw
        || receipt.surcharge_raw != expected_surcharge_raw
        || receipt.pull_fee_raw != expected_pull_fee_raw
        || receipt.gross_payment_raw != expected_gross_payment_raw
        || receipt.collateral_ledger != collateral_ledger
        || receipt.collateral_gross_raw != expected_collateral_gross_raw
        || receipt.collateral_net_raw != expected_collateral_net_raw
        || receipt.collateral_net_raw > receipt.collateral_gross_raw
        || receipt.collateral_payout_net_raw > receipt.collateral_net_raw
    {
        return Err("backend receipt conflicts with the immutable SP liquidation quote".into());
    }
    let pull_amount_raw = expected_principal_raw
        .checked_add(expected_surcharge_raw)
        .ok_or_else(|| "ordinary payment principal plus surcharge overflows".to_string())?;
    let gross_pull_raw = pull_amount_raw
        .checked_add(expected_pull_fee_raw)
        .ok_or_else(|| "ordinary payment gross amount overflows".to_string())?;
    if receipt.payment_tuple.amount_raw != pull_amount_raw
        || receipt.payment_tuple.ledger_fee_raw != expected_pull_fee_raw
        || receipt.gross_payment_raw != gross_pull_raw
    {
        return Err("backend payment tuple conflicts with the frozen payment quote".into());
    }
    let payout = receipt
        .payout_tuple
        .as_ref()
        .ok_or_else(|| "backend receipt has no collateral payout tuple".to_string())?;
    let payout_gross_raw = payout
        .amount_raw
        .checked_add(payout.ledger_fee_raw)
        .ok_or_else(|| "backend payout gross amount overflows".to_string())?;
    if payout.ledger != collateral_ledger
        || payout.amount_raw != receipt.collateral_payout_net_raw
        // The backend's payout tuple is the SP leg after the protocol cut;
        // the full seized collateral gross is retained separately in the
        // immutable receipt quote.
        || payout.gross_raw != expected_collateral_net_raw
        || payout.gross_raw != payout_gross_raw
        || receipt.payout_operation_id.is_none()
        || receipt.payout_block_index.is_none()
        || receipt.payout_net_raw != Some(payout.amount_raw)
    {
        return Err("backend payout tuple conflicts with the immutable collateral quote".into());
    }
    Ok(payout)
}

/// Durable phase owned by the Stability Pool.  The approval and backend
/// phases deliberately distinguish “submitted or unknown” from a proven
/// no-effect response: an unknown callback may only retry the exact persisted
/// tuple and may never allocate another request identity.
#[derive(CandidType, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrdinaryLiquidationPhase {
    Prepared,
    ApprovalSubmitted,
    ApprovalConfirmed,
    BackendPrepared,
    CancellationSubmitted,
    BackendSubmitted,
    PaymentConfirmed,
    PayoutHeld,
    Completed,
    PaymentNoEffect,
    PayoutNoEffect,
    Cancelled,
}

impl Default for OrdinaryLiquidationPhase {
    fn default() -> Self {
        Self::Completed
    }
}

impl OrdinaryLiquidationPhase {
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::PaymentNoEffect | Self::PayoutNoEffect | Self::Cancelled
        )
    }

    pub fn blocks_new_spend(self) -> bool {
        !matches!(self, Self::Completed | Self::PaymentNoEffect | Self::Cancelled)
    }
}

/// The backend's exact collateral payout tuple, copied into SP state so
/// recovery never needs to reconstruct it from a mutable backend journal.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrdinaryPayoutTuple {
    pub ledger: Principal,
    pub source: icrc_ledger_types::icrc1::account::Account,
    pub destination: icrc_ledger_types::icrc1::account::Account,
    pub amount_raw: u64,
    pub gross_raw: u64,
    pub ledger_fee_raw: u64,
    pub memo: Vec<u8>,
    pub created_at_time_ns: u64,
    pub op_nonce: u128,
}

impl From<&SpLiquidationPayoutTuple> for OrdinaryPayoutTuple {
    fn from(tuple: &SpLiquidationPayoutTuple) -> Self {
        Self {
            ledger: tuple.ledger,
            source: tuple.source.clone(),
            destination: tuple.destination.clone(),
            amount_raw: tuple.amount_raw,
            gross_raw: tuple.gross_raw,
            ledger_fee_raw: tuple.ledger_fee_raw,
            memo: tuple.memo.clone(),
            created_at_time_ns: tuple.created_at_time_ns,
            op_nonce: tuple.op_nonce,
        }
    }
}

/// Full incoming ICRC-2 pull tuple copied from the backend receipt. Keeping
/// the memo, timestamp, and operation nonce in the SP journal makes a retry
/// compare the same backend payment identity after an upgrade.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrdinaryPaymentTuple {
    pub ledger: Principal,
    pub source: icrc_ledger_types::icrc1::account::Account,
    pub destination: icrc_ledger_types::icrc1::account::Account,
    pub amount_raw: u64,
    pub ledger_fee_raw: u64,
    pub memo: Vec<u8>,
    pub created_at_time_ns: u64,
    pub op_nonce: u128,
}

impl From<&rumi_protocol_backend::state::SpLiquidationPaymentTuple> for OrdinaryPaymentTuple {
    fn from(tuple: &rumi_protocol_backend::state::SpLiquidationPaymentTuple) -> Self {
        Self {
            ledger: tuple.ledger,
            source: tuple.source.clone(),
            destination: tuple.destination.clone(),
            amount_raw: tuple.amount_raw,
            ledger_fee_raw: tuple.ledger_fee_raw,
            memo: tuple.memo.to_vec(),
            created_at_time_ns: tuple.created_at_time_ns,
            op_nonce: tuple.op_nonce,
        }
    }
}

/// Quote values copied from the backend receipt after request admission.  All
/// later approval, retry, and settlement paths use this row rather than
/// reading a fresh vault, price, or ledger-fee quote.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrdinaryLiquidationQuote {
    pub debt_e8s: u64,
    pub principal_raw: u64,
    pub surcharge_raw: u64,
    pub pull_fee_raw: u64,
    pub gross_payment_raw: u64,
    pub payment_tuple: OrdinaryPaymentTuple,
    pub collateral_ledger: Principal,
    pub collateral_price_e8s: u64,
    pub collateral_gross_raw: u64,
    pub collateral_net_raw: u64,
    pub collateral_fee_raw: u64,
    pub payout_operation_id: Option<u128>,
    pub payout_tuple: Option<OrdinaryPayoutTuple>,
    pub payment_block_index: Option<u64>,
    pub payout_block_index: Option<u64>,
    pub payout_net_raw: Option<u64>,
}

/// Exact approval tuple persisted before the first `icrc2_approve` await.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrdinaryApprovalTuple {
    pub ledger: Principal,
    pub spender: Principal,
    pub amount_raw: u64,
    pub fee_raw: u64,
    pub memo: Vec<u8>,
    pub created_at_time_ns: u64,
    pub expires_at_time_ns: u64,
}

/// Stable Pool journal for one ordinary liquidation.  `quote` and
/// `approval` are populated by compare-and-swap transitions after backend
/// preparation, while the request identity and participant snapshot exist
/// before any external await.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrdinaryLiquidationIntent {
    pub request_id: [u8; 32],
    pub vault_id: u64,
    pub collateral_type: Principal,
    pub payment: SpLiquidationPayment,
    pub stable_ledger: Principal,
    pub stable_decimals: u8,
    pub requested_debt_e8s: u64,
    pub max_gross_raw: u64,
    pub participant_snapshot: Vec<ParticipantBalance>,
    pub quote: Option<OrdinaryLiquidationQuote>,
    pub approval: Option<OrdinaryApprovalTuple>,
    pub approval_attempted: bool,
    pub approval_block_index: Option<u64>,
    /// Charged only after an authoritative revoke receipt; unknown revoke
    /// outcomes keep this row held rather than guessing a second fee.
    pub revoke_fee_raw: u64,
    pub backend_attempted: bool,
    pub phase: OrdinaryLiquidationPhase,
    pub last_error: Option<String>,
    pub created_at_ns: u64,
    pub updated_at_ns: u64,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrdinaryLiquidationTombstone {
    pub request_id: [u8; 32],
    pub vault_id: u64,
    pub payment: SpLiquidationPayment,
    pub stable_ledger: Principal,
    pub collateral_type: Principal,
    /// The terminal disposition is retained separately from the result so a
    /// status query distinguishes cancellation/no-effect from a successful
    /// completion after the pending row has been released.
    #[serde(default)]
    pub terminal_phase: OrdinaryLiquidationPhase,
    pub quote: Option<OrdinaryLiquidationQuote>,
    pub result: crate::types::LiquidationResult,
    pub completed_at_ns: u64,
}

pub fn ordinary_failure_result(
    vault_id: u64,
    collateral_type: Principal,
    reason: impl Into<String>,
    admitted: bool,
) -> crate::types::LiquidationResult {
    crate::types::LiquidationResult {
        vault_id,
        stables_consumed: std::collections::BTreeMap::new(),
        collateral_gained: 0,
        collateral_type,
        success: false,
        error_message: Some(reason.into()),
        admitted: Some(admitted),
    }
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrdinaryLiquidationStatus {
    pub request_id: [u8; 32],
    pub vault_id: u64,
    pub payment: SpLiquidationPayment,
    pub stable_ledger: Principal,
    pub collateral_type: Principal,
    pub phase: OrdinaryLiquidationPhase,
    pub quote: Option<OrdinaryLiquidationQuote>,
    pub approval: Option<OrdinaryApprovalTuple>,
    pub approval_attempted: bool,
    pub backend_attempted: bool,
    pub last_error: Option<String>,
}

impl From<&OrdinaryLiquidationIntent> for OrdinaryLiquidationStatus {
    fn from(intent: &OrdinaryLiquidationIntent) -> Self {
        Self {
            request_id: intent.request_id,
            vault_id: intent.vault_id,
            payment: intent.payment,
            stable_ledger: intent.stable_ledger,
            collateral_type: intent.collateral_type,
            phase: intent.phase,
            quote: intent.quote.clone(),
            approval: intent.approval.clone(),
            approval_attempted: intent.approval_attempted,
            backend_attempted: intent.backend_attempted,
            last_error: intent.last_error.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn principal(byte: u8) -> Principal {
        Principal::from_slice(&[byte])
    }

    fn participant(byte: u8, balance_raw: u64, opted_in: bool) -> ParticipantBalance {
        ParticipantBalance {
            owner: principal(byte),
            balance_raw,
            opted_in,
        }
    }

    #[test]
    fn exact_debit_distributes_floor_dust_without_exceeding_balances() {
        let allocations = allocate_exact_debit(
            &[participant(2, 10_000_000, true), participant(1, 10_000_000, true)],
            10_000_001,
        )
        .unwrap();
        assert_eq!(allocations[0].owner, principal(1));
        assert_eq!(allocations[0].debit_raw, 5_000_001);
        assert_eq!(allocations[1].debit_raw, 5_000_000);
        assert_eq!(allocations.iter().map(|a| a.debit_raw).sum::<u64>(), 10_000_001);
    }

    #[test]
    fn gain_remainder_goes_to_first_sorted_depositor() {
        let gains = allocate_exact_gain(
            &[
                StableDebitAllocation { owner: principal(2), debit_raw: 1 },
                StableDebitAllocation { owner: principal(1), debit_raw: 1 },
            ],
            5,
        )
        .unwrap();
        assert_eq!(gains, vec![
            CollateralGainAllocation { owner: principal(1), gain_raw: 3 },
            CollateralGainAllocation { owner: principal(2), gain_raw: 2 },
        ]);
    }

    #[test]
    fn opt_out_is_excluded_from_debit_and_gain_weight() {
        let allocations = allocate_exact_debit(
            &[participant(1, 100, true), participant(2, 900, false)],
            100,
        )
        .unwrap();
        assert_eq!(allocations, vec![StableDebitAllocation { owner: principal(1), debit_raw: 100 }]);
        assert_eq!(
            allocate_exact_gain(&allocations, 7).unwrap(),
            vec![CollateralGainAllocation { owner: principal(1), gain_raw: 7 }]
        );
    }

    #[test]
    fn insufficient_budget_is_rejected_before_any_allocation() {
        assert_eq!(
            allocate_exact_debit(&[participant(1, 10, true)], 11),
            Err(AllocationError::InsufficientBalance { requested: 11, available: 10 })
        );
    }

    #[test]
    fn exact_debit_charges_approval_and_pull_fees_once() {
        assert_eq!(exact_stable_debit(100, 3, 2, 4, 0), Some(109));
        assert_eq!(exact_stable_debit(100, 3, 2, 4, 5), Some(114));
    }

    #[test]
    fn affordable_gross_reserves_live_approval_fee_exactly() {
        assert_eq!(affordable_max_gross_raw(10_000_000, 10_000), Some(9_990_000));
        assert_eq!(affordable_max_gross_raw(10_000, 10_000), None);
        assert_eq!(affordable_max_gross_raw(9_999, 10_000), None);
    }

    #[test]
    fn payout_gross_is_sp_net_after_protocol_cut() {
        let request_id = [7; 32];
        let stable = principal(1);
        let collateral = principal(2);
        let mut receipt = SpLiquidationReceipt {
            request_id: request_id.to_vec(),
            vault_id: 9,
            payment: SpLiquidationPayment::Icusd,
            payment_tuple: rumi_protocol_backend::state::SpLiquidationPaymentTuple {
                ledger: stable,
                source: icrc_ledger_types::icrc1::account::Account {
                    owner: principal(3),
                    subaccount: None,
                },
                destination: icrc_ledger_types::icrc1::account::Account {
                    owner: principal(4),
                    subaccount: None,
                },
                amount_raw: 100,
                ledger_fee_raw: 2,
                memo: [8; 16],
                created_at_time_ns: 11,
                op_nonce: 12,
            },
            phase: rumi_protocol_backend::sp_liquidation::SpLiquidationPhaseView::PayoutSettled,
            debt_e8s: 100,
            principal_raw: 100,
            surcharge_raw: 0,
            pull_fee_raw: 2,
            gross_payment_raw: 102,
            collateral_ledger: collateral,
            collateral_price_e8s: 1,
            collateral_price_decimal: Vec::new(),
            collateral_gross_raw: 100,
            collateral_net_raw: 90,
            collateral_payout_net_raw: 89,
            payout_operation_id: Some(13),
            payout_tuple: Some(SpLiquidationPayoutTuple {
                ledger: collateral,
                source: icrc_ledger_types::icrc1::account::Account {
                    owner: principal(4),
                    subaccount: None,
                },
                destination: icrc_ledger_types::icrc1::account::Account {
                    owner: principal(3),
                    subaccount: None,
                },
                amount_raw: 89,
                gross_raw: 90,
                ledger_fee_raw: 1,
                memo: vec![9; 16],
                created_at_time_ns: 14,
                op_nonce: 13,
            }),
            payment_block_index: Some(15),
            payout_block_index: Some(16),
            payout_net_raw: Some(89),
        };
        assert!(validate_receipt_identity(
            &receipt,
            &request_id,
            9,
            SpLiquidationPayment::Icusd,
            stable,
            collateral,
            100,
            100,
            0,
            2,
            102,
            100,
            90,
        )
        .is_ok());
        receipt.payout_tuple.as_mut().unwrap().gross_raw = 100;
        assert!(validate_receipt_identity(
            &receipt,
            &request_id,
            9,
            SpLiquidationPayment::Icusd,
            stable,
            collateral,
            100,
            100,
            0,
            2,
            102,
            100,
            90,
        )
        .is_err());
    }

    #[test]
    fn single_ledger_selection_excludes_lp_and_prefers_priority() {
        let selected = choose_single_stable(
            &[
                StableCandidate {
                    ledger: principal(1), payment: SpLiquidationPayment::Icusd,
                    decimals: 8, priority: 1, active: true, is_lp: false,
                    available_raw: 50_000_000,
                },
                StableCandidate {
                    ledger: principal(2), payment: SpLiquidationPayment::Ckusdc,
                    decimals: 6, priority: 2, active: true, is_lp: false,
                    available_raw: 10_000_000,
                },
                StableCandidate {
                    ledger: principal(3), payment: SpLiquidationPayment::Icusd,
                    decimals: 8, priority: 9, active: true, is_lp: true,
                    available_raw: 100_000_000,
                },
            ],
            700_000_000,
        )
        .unwrap();
        assert_eq!(selected.ledger, principal(2));
        assert_eq!(selected.amount_raw, 7_000_000);
        assert_eq!(selected.covered_debt_e8s, 700_000_000);
    }

    #[test]
    fn request_id_is_stable_and_nonce_bound() {
        let first = ordinary_request_id(42, 7);
        assert_eq!(first, ordinary_request_id(42, 7));
        assert_ne!(first, ordinary_request_id(42, 8));
        assert_ne!(first, ordinary_request_id(43, 7));
    }

    #[test]
    fn principal_tie_break_is_byte_ordered() {
        let mut candidates = vec![
            StableCandidate {
                ledger: principal(2), payment: SpLiquidationPayment::Icusd,
                decimals: 8, priority: 1, active: true, is_lp: false,
                available_raw: 1,
            },
            StableCandidate {
                ledger: principal(1), payment: SpLiquidationPayment::Icusd,
                decimals: 8, priority: 1, active: true, is_lp: false,
                available_raw: 1,
            },
        ];
        let selected = choose_single_stable(&candidates, 1).unwrap();
        assert_eq!(selected.ledger, principal(1));
        candidates.swap(0, 1);
        assert_eq!(choose_single_stable(&candidates, 1).unwrap().ledger, principal(1));
    }

    #[test]
    fn zero_inputs_are_rejected() {
        assert_eq!(allocate_exact_debit(&[participant(1, 1, true)], 0), Err(AllocationError::ZeroAmount));
        assert_eq!(allocate_exact_gain(&[], 1), Err(AllocationError::NoEligibleBalance));
        assert!(choose_single_stable(&[], 1).is_none());
    }

    #[test]
    fn unresolved_phases_block_fresh_spend_but_terminal_no_effect_does_not() {
        assert!(OrdinaryLiquidationPhase::ApprovalSubmitted.blocks_new_spend());
        assert!(OrdinaryLiquidationPhase::CancellationSubmitted.blocks_new_spend());
        assert!(OrdinaryLiquidationPhase::BackendSubmitted.blocks_new_spend());
        assert!(OrdinaryLiquidationPhase::PayoutHeld.blocks_new_spend());
        assert!(!OrdinaryLiquidationPhase::PaymentNoEffect.blocks_new_spend());
        assert!(!OrdinaryLiquidationPhase::Cancelled.blocks_new_spend());
        assert!(OrdinaryLiquidationPhase::Completed.terminal());
    }
}
