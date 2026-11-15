//! Explicit operator-funded subsidy budget. Claim proceeds remain separate.
//! Allocation and payment preparation are one durable BotState transition.
use candid::CandidType;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::state::{BotClaimJournalStatus, BotPaymentJournal, BotPaymentStatus, BotState};

pub const MAX_BUDGET_E6: u64 = 1_000_000;
pub const MAX_PER_CLAIM_E6: u64 = 25_000;
const MAX_ALLOCATIONS: usize = 1_024;

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperatorReserveConfig {
    pub budget_e6: u64,
    pub per_claim_cap_e6: u64,
    /// Untouchable shared-wallet balance; excludes this explicitly funded budget.
    pub protected_balance_e6: u64,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReserveAllocation {
    pub vault_id: u64,
    pub claim_generation: u64,
    pub amount_e6: u64,
    pub payment_created_at_time: u64,
    pub payment_memo: Vec<u8>,
    pub payment_amount_e6: u64,
    pub payment_fee_e6: u64,
    pub backend_principal: candid::Principal,
    pub ledger_principal: candid::Principal,
    pub settled: bool,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperatorReserve {
    pub config: OperatorReserveConfig,
    pub enabled: bool,
    /// Includes unresolved allocations. Never refunded on timeout or retry.
    pub allocated_e6: u64,
    pub settled_e6: u64,
    /// History record IDs are immutable allocation identities, including after cleanup.
    pub allocations: BTreeMap<u64, ReserveAllocation>,
}

impl OperatorReserve {
    pub fn new(config: OperatorReserveConfig, balance: u64) -> Result<Self, String> {
        if config.budget_e6 == 0
            || config.budget_e6 > MAX_BUDGET_E6
            || config.per_claim_cap_e6 == 0
            || config.per_claim_cap_e6 > MAX_PER_CLAIM_E6
            || config.per_claim_cap_e6 > config.budget_e6
            || config
                .protected_balance_e6
                .checked_add(config.budget_e6)
                .is_none_or(|required| required > balance)
        {
            return Err("reserve budget/cap/protected-balance bounds are invalid".into());
        }
        Ok(Self {
            config,
            enabled: false,
            allocated_e6: 0,
            settled_e6: 0,
            allocations: BTreeMap::new(),
        })
    }
}

/// Only ledger fees plus the configured slippage allowance can be subsidized.
pub fn economic_subsidy_cap(debt_e8s: u64, ledger_fee_e6: u64, slippage_bps: u16) -> u64 {
    let net = debt_e8s / 100 + u64::from(debt_e8s % 100 != 0);
    let slippage = (u128::from(net) * u128::from(slippage_bps.min(10_000)) / 10_000) as u64;
    ledger_fee_e6.saturating_mul(2).saturating_add(slippage)
}

/// Conservative pre-claim admission: pool slippage, pool withdrawal fee and
/// the separate backend payment fee all fit owned proceeds + authorized reserve.
pub fn can_admit_quote(
    s: &BotState,
    debt_e8s: u64,
    fee: u64,
    quote: u64,
    slippage_bps: u16,
    balance: u64,
) -> bool {
    if quote == 0 || slippage_bps > 10_000 {
        return false;
    }
    let minimum_gross =
        (u128::from(quote) * u128::from(10_000u16.saturating_sub(slippage_bps)) / 10_000) as u64;
    let minimum_net = minimum_gross.saturating_sub(fee);
    let net_debt = debt_e8s / 100 + u64::from(debt_e8s % 100 != 0);
    let Some(payment_gross) = net_debt.checked_add(fee) else {
        return false;
    };
    let deficit = payment_gross.saturating_sub(minimum_net);
    can_cover(
        s,
        deficit,
        balance.saturating_add(minimum_net),
        payment_gross,
        economic_subsidy_cap(debt_e8s, fee, slippage_bps),
    )
}

fn allocation_matches(a: &ReserveAllocation, j: &BotPaymentJournal) -> bool {
    a.vault_id == j.vault_id
        && a.claim_generation == j.claim_generation
        && a.payment_created_at_time == j.created_at_time
        && a.payment_memo == j.memo
        && a.payment_amount_e6 == j.amount_e6
        && a.payment_fee_e6 == j.fee_e6
        && a.backend_principal == j.backend_principal
        && a.ledger_principal == j.ledger_principal
        && j.amount_e6.checked_add(j.fee_e6) == Some(j.gross_amount_e6)
        && a.amount_e6 == j.gross_amount_e6.saturating_sub(j.ckusdc_received_e6)
}

pub fn can_cover(s: &BotState, deficit: u64, balance: u64, gross: u64, economic_cap: u64) -> bool {
    if deficit == 0 {
        return true;
    }
    let Some(r) = &s.operator_reserve else {
        return false;
    };
    r.enabled
        && deficit <= r.config.per_claim_cap_e6
        && deficit <= economic_cap
        && r.allocated_e6
            .checked_add(deficit)
            .is_some_and(|v| v <= r.config.budget_e6)
        && r.allocations.len() < MAX_ALLOCATIONS
        && balance
            .checked_sub(gross)
            .is_some_and(|v| v >= r.config.protected_balance_e6)
}

/// Caller must save state before dispatch. No mutation occurs on refusal.
pub fn commit_payment(
    s: &mut BotState,
    journal: BotPaymentJournal,
    balance: u64,
    economic_cap: u64,
) -> Result<(), String> {
    let claim = s
        .pending_claims
        .get(&journal.vault_id)
        .ok_or("claim disappeared")?;
    let record_id = journal
        .record_id
        .ok_or("payment has no durable history identity")?;
    let required_net =
        journal.debt_covered_e8s / 100 + u64::from(journal.debt_covered_e8s % 100 != 0);
    if s.pending_payments.contains_key(&journal.vault_id)
        || s.pending_treasury.contains_key(&record_id)
        || claim.claim_generation != journal.claim_generation
        || claim.payment_memo != journal.memo
        || claim.debt_covered_e8s != journal.debt_covered_e8s
        || claim.collateral_amount_e8s != journal.collateral_amount_e8s
        || claim.collateral_received_amount_e8s != journal.collateral_received_amount_e8s
        || !matches!(
            claim.status,
            BotClaimJournalStatus::PaymentShortfall | BotClaimJournalStatus::SwapMayHaveStarted
        )
        || claim.collateral_return.is_some()
        || !claim.failed_return_attempts.is_empty()
        || journal.amount_e6 != required_net
        || journal.created_at_time == 0
        || journal.amount_e6.checked_add(journal.fee_e6) != Some(journal.gross_amount_e6)
        || journal.status != BotPaymentStatus::Prepared
        || journal.receipt.is_some()
        || journal.shortfall_receipt_observed
        || journal.shortfall_topup.is_some()
        || !s.config.as_ref().is_some_and(|c| {
            c.backend_principal == journal.backend_principal
                && c.ckusdc_ledger == journal.ledger_principal
        })
    {
        return Err("claim/payment identity changed before reserve allocation".into());
    }
    let deficit = journal
        .gross_amount_e6
        .saturating_sub(journal.ckusdc_received_e6);
    if !can_cover(s, deficit, balance, journal.gross_amount_e6, economic_cap) {
        return Err("reserve disabled, capped, exhausted, or protected balance unavailable".into());
    }
    if s.operator_reserve
        .as_ref()
        .is_some_and(|r| r.allocations.contains_key(&record_id))
    {
        return Err("this history identity already consumed its reserve allocation".into());
    }
    if deficit > 0 {
        let reserve = s
            .operator_reserve
            .as_mut()
            .expect("can_cover checked reserve");
        reserve.allocated_e6 += deficit;
        reserve.allocations.insert(
            record_id,
            ReserveAllocation {
                vault_id: journal.vault_id,
                claim_generation: journal.claim_generation,
                amount_e6: deficit,
                payment_created_at_time: journal.created_at_time,
                payment_memo: journal.memo.clone(),
                payment_amount_e6: journal.amount_e6,
                payment_fee_e6: journal.fee_e6,
                backend_principal: journal.backend_principal,
                ledger_principal: journal.ledger_principal,
                settled: false,
            },
        );
    }
    s.pending_claims.remove(&journal.vault_id);
    s.pending_payments.insert(journal.vault_id, journal);
    Ok(())
}

/// A confirmed tuple consumes its allocation once. Disabling new reserve use
/// does not invalidate an already allocated payment or its reconciliation.
pub fn settle_once(s: &mut BotState, journal: &BotPaymentJournal) -> bool {
    let Some(id) = journal.record_id else {
        return true;
    };
    let Some(r) = &mut s.operator_reserve else {
        return true;
    };
    let Some(a) = r.allocations.get_mut(&id) else {
        return true;
    };
    if !allocation_matches(a, journal) {
        return false;
    }
    if !a.settled {
        r.settled_e6 += a.amount_e6;
        a.settled = true;
    }
    true
}

pub fn has_allocation(s: &BotState, j: &BotPaymentJournal) -> bool {
    j.record_id.is_some_and(|id| {
        s.operator_reserve
            .as_ref()
            .is_some_and(|r| r.allocations.contains_key(&id))
    })
}

pub fn payment_allocation_matches(s: &BotState, j: &BotPaymentJournal) -> bool {
    let Some(id) = j.record_id else { return true };
    s.operator_reserve
        .as_ref()
        .and_then(|r| r.allocations.get(&id))
        .is_none_or(|a| allocation_matches(a, j))
}

/// Explicit retry of a definitive no-effect payment keeps the same tuple and
/// committed budget. A fee change/expired window requires reconciliation.
pub fn rearm_no_effect(
    s: &mut BotState,
    j: &BotPaymentJournal,
    fee: u64,
    balance: u64,
    now: u64,
) -> Result<(), String> {
    let id = j.record_id.ok_or("reserve payment lacks record identity")?;
    let r = s.operator_reserve.as_ref().ok_or("reserve missing")?;
    let a = r.allocations.get(&id).ok_or("reserve allocation missing")?;
    let current = s
        .pending_payments
        .get(&j.vault_id)
        .ok_or("payment missing")?;
    if a.settled
        || !allocation_matches(a, j)
        || !allocation_matches(a, current)
        || current.status != BotPaymentStatus::NoEffect
        || current.receipt.is_some()
        || j.status != BotPaymentStatus::NoEffect
        || j.receipt.is_some()
        || fee != j.fee_e6
        || now < j.created_at_time
        || now - j.created_at_time >= 23 * 60 * 60 * 1_000_000_000
        || balance
            .checked_sub(j.gross_amount_e6)
            .is_none_or(|v| v < r.config.protected_balance_e6)
    {
        return Err(
            "reserve no-effect retry cannot change its tuple, fee, allocation or protected funds"
                .into(),
        );
    }
    s.pending_payments
        .get_mut(&j.vault_id)
        .expect("validated pending payment")
        .status = BotPaymentStatus::Prepared;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured() -> BotState {
        let mut r = OperatorReserve::new(
            OperatorReserveConfig {
                budget_e6: 1_000_000,
                per_claim_cap_e6: 25_000,
                protected_balance_e6: 25_621_238,
            },
            26_621_238,
        )
        .unwrap();
        r.enabled = true;
        BotState {
            operator_reserve: Some(r),
            ..Default::default()
        }
    }

    fn payment_fixture() -> (BotState, BotPaymentJournal) {
        let (config, claim, _) = crate::process::fixture202();
        let journal = BotPaymentJournal {
            record_id: Some(231),
            vault_id: 202,
            backend_principal: config.backend_principal,
            ledger_principal: config.ckusdc_ledger,
            claim_generation: 3,
            debt_covered_e8s: 10_090_702,
            collateral_amount_e8s: claim.collateral_amount_e8s,
            collateral_received_amount_e8s: claim.collateral_received_amount_e8s,
            collateral_price_e8s: claim.collateral_price_e8s,
            icp_swapped_e8s: 4_918_638,
            swap_input_fee_e8s: 10_000,
            ckusdc_received_e6: 95_482,
            held_surplus_e6: 0,
            gross_amount_e6: 110_908,
            amount_e6: 100_908,
            fee_e6: 10_000,
            created_at_time: 123,
            memo: claim.payment_memo.clone(),
            status: BotPaymentStatus::Prepared,
            receipt: None,
            shortfall_receipt_observed: false,
            shortfall_topup: None,
        };
        let mut s = configured();
        s.config = Some(config);
        s.pending_claims.insert(202, claim);
        (s, journal)
    }

    #[test]
    fn allocation_is_atomic_and_replay_or_changed_tuple_cannot_spend_again() {
        let (mut s, j) = payment_fixture();
        let before = serde_json::to_vec(&s).unwrap();
        let mut wrong = j.clone();
        wrong.claim_generation += 1;
        assert!(commit_payment(&mut s, wrong, 26_716_720, 22_522).is_err());
        assert_eq!(before, serde_json::to_vec(&s).unwrap());
        commit_payment(&mut s, j.clone(), 26_716_720, 22_522).unwrap();
        assert!(s.pending_claims.is_empty());
        assert_eq!(s.operator_reserve.as_ref().unwrap().allocated_e6, 15_426);
        assert!(commit_payment(&mut s, j.clone(), 26_716_720, 22_522).is_err());
        // Lost reply + restart retains the original tuple and its committed budget.
        let mut restarted: BotState =
            serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
        assert_eq!(restarted.pending_payments[&202].created_at_time, 123);
        assert_eq!(
            restarted.operator_reserve.as_ref().unwrap().allocated_e6,
            15_426
        );
        restarted.operator_reserve.as_mut().unwrap().enabled = false;
        let mut changed = j.clone();
        changed.created_at_time += 1;
        assert!(!settle_once(&mut restarted, &changed));
        assert!(settle_once(&mut restarted, &j));
        assert!(settle_once(&mut restarted, &j));
        assert_eq!(
            restarted.operator_reserve.as_ref().unwrap().settled_e6,
            15_426
        );
        assert_eq!(
            restarted.operator_reserve.as_ref().unwrap().allocated_e6,
            15_426
        );
    }

    #[test]
    fn ordinary_claim_without_shortfall_needs_no_reserve_and_keeps_own_proceeds() {
        let (mut s, mut j) = payment_fixture();
        s.operator_reserve = None;
        j.ckusdc_received_e6 = 120_908;
        j.held_surplus_e6 = 10_000;
        commit_payment(&mut s, j.clone(), 0, 0).unwrap();
        assert!(s.operator_reserve.is_none());
        assert!(settle_once(&mut s, &j));
        assert_eq!(s.pending_payments[&202].held_surplus_e6, 10_000);
    }

    #[test]
    fn no_effect_retry_reuses_original_allocation_tuple_and_rejects_fee_time_or_floor_changes() {
        let (mut s, j) = payment_fixture();
        commit_payment(&mut s, j.clone(), 26_716_720, 22_522).unwrap();
        s.pending_payments.get_mut(&202).unwrap().status = BotPaymentStatus::NoEffect;
        let retry = s.pending_payments[&202].clone();
        let original = serde_json::to_vec(&s).unwrap();
        for (fee, balance, now) in [
            (10_001, 26_716_720, 124),
            (10_000, 25_621_238, 124),
            (10_000, 26_716_720, 23 * 60 * 60 * 1_000_000_000 + 123),
        ] {
            assert!(rearm_no_effect(&mut s, &retry, fee, balance, now).is_err());
            assert_eq!(serde_json::to_vec(&s).unwrap(), original);
        }
        rearm_no_effect(&mut s, &retry, 10_000, 26_716_720, 124).unwrap();
        assert_eq!(
            s.pending_payments[&202].created_at_time,
            retry.created_at_time
        );
        assert_eq!(s.pending_payments[&202].amount_e6, retry.amount_e6);
        assert_eq!(s.pending_payments[&202].memo, retry.memo);
        assert_eq!(s.operator_reserve.as_ref().unwrap().allocated_e6, 15_426);
        assert!(payment_allocation_matches(&s, &retry));
        let mut changed = retry.clone();
        changed.created_at_time += 1;
        assert!(!payment_allocation_matches(&s, &changed));
    }

    #[test]
    fn exact_202_shortfall_fits_funded_budget_but_default_has_no_authority() {
        assert!(!can_cover(
            &BotState::default(),
            15_426,
            26_716_720,
            110_908,
            22_522
        ));
        assert!(can_cover(
            &configured(),
            15_426,
            26_716_720,
            110_908,
            22_522
        ));
        assert_eq!(economic_subsidy_cap(10_090_702, 10_000, 250), 22_522);
    }

    #[test]
    fn fee_budget_cannot_spend_protected_claim_funds_or_exceed_caps() {
        let mut s = configured();
        for (deficit, balance, cap) in [
            (25_001, 26_716_720, 30_000),
            (15_426, 25_700_000, 22_522),
            (15_426, 26_716_720, 15_425),
        ] {
            assert!(!can_cover(&s, deficit, balance, 110_908, cap));
        }
        s.operator_reserve.as_mut().unwrap().allocated_e6 = 990_000;
        assert!(!can_cover(&s, 15_426, 26_716_720, 110_908, 22_522));
        s.operator_reserve.as_mut().unwrap().enabled = false;
        assert!(!can_cover(&s, 1, 26_716_720, 110_908, 22_522));
    }

    #[test]
    fn small_vault_is_not_claimed_without_fee_coverage_or_after_budget_exhaustion() {
        let mut s = configured();
        assert!(can_admit_quote(
            &s, 10_090_702, 10_000, 106_023, 250, 26_621_238
        ));
        assert!(!can_admit_quote(
            &BotState::default(),
            10_090_702,
            10_000,
            106_023,
            250,
            26_621_238
        ));
        s.operator_reserve.as_mut().unwrap().allocated_e6 = 1_000_000;
        assert!(!can_admit_quote(
            &s, 10_090_702, 10_000, 106_023, 250, 26_621_238
        ));
        // A larger economically covered claim needs no reserve authority.
        assert!(can_admit_quote(
            &BotState::default(),
            100_000_000,
            10_000,
            1_060_000,
            250,
            0
        ));
    }

    #[test]
    fn legacy_state_has_no_reserve_and_new_budget_survives_serialization() {
        let s = configured();
        let mut json = serde_json::to_value(&s).unwrap();
        json.as_object_mut().unwrap().remove("operator_reserve");
        let old: BotState = serde_json::from_value(json).unwrap();
        assert!(old.operator_reserve.is_none());
        let roundtrip: BotState = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
        assert_eq!(s.operator_reserve, roundtrip.operator_reserve);
        assert!(OperatorReserve::new(
            OperatorReserveConfig {
                budget_e6: 1_000_001,
                per_claim_cap_e6: 25_000,
                protected_balance_e6: 0
            },
            u64::MAX
        )
        .is_err());
    }
}
