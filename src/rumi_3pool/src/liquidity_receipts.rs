//! Durable caller-scoped add-liquidity intents.
//!
//! The receipt is the operation identity.  It is written before the first
//! fee lookup, and every input/refund reservation is recorded before its
//! ICRC-2 call.  A retry therefore resumes the exact payout identities rather
//! than allocating another pull or another LP mint.

use crate::{payouts, storage};
use candid::{CandidType, Nat, Principal};
use icrc_ledger_types::icrc1::account::Account;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::BTreeSet;

pub const MAX_RECEIPTS: u64 = 10_000;

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AddLiquidityRequestV1 {
    pub intent_id: Vec<u8>,
    pub amounts: Vec<u128>,
    pub min_lp: u128,
    /// Optional caller-owned source subaccount.  The owner is always the
    /// authenticated caller; the subaccount is bound into the receipt.
    pub source_subaccount: Option<[u8; 32]>,
    /// Maximum fee that any initial or recovery refund transfer may consume.
    /// `None` preserves the uncapped legacy policy.
    pub max_transfer_fee: Option<u128>,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum LiquidityReceiptStatusV1 {
    Prepared,
    Executing,
    RecoveryPending,
    Completed,
    Refunded,
    Rejected,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum LiquidityLegStatusV1 {
    Prepared,
    Submitted,
    Confirmed { block: Nat },
    RejectedNoTransfer { reason: String },
    Unresolved { reason: String },
    Refunded,
    Skipped,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LiquidityReceiptV1 {
    pub version: u16,
    pub owner: Principal,
    pub request: AddLiquidityRequestV1,
    pub status: LiquidityReceiptStatusV1,
    /// Exact stable payout identities.  For add-liquidity each identity
    /// contains both the input pull and its held refund tuple.
    pub input_ids: [Option<u64>; 3],
    pub refund_ids: [Option<u64>; 3],
    pub input_status: [Option<LiquidityLegStatusV1>; 3],
    pub refund_status: [Option<LiquidityLegStatusV1>; 3],
    /// Exact input and refund tuples retained for accounting and recovery
    /// review, including the actual fee and bound source subaccount.
    pub input_transfers: [Option<payouts::PayoutInputTransfer>; 3],
    pub refund_transfers: [Option<payouts::PayoutTransfer>; 3],
    /// Immutable accounting plan captured before the first input dispatch.
    pub old_balances: Option<[u128; 3]>,
    pub old_lp_supply: Option<u128>,
    pub old_caller_lp: Option<u128>,
    pub balances_after: Option<[u128; 3]>,
    pub lp_supply_after: Option<u128>,
    pub caller_lp_after: Option<u128>,
    pub lp_minted: Option<u128>,
    pub fee_bps: Option<u16>,
    pub imbalance_before: Option<u64>,
    pub imbalance_after: Option<u64>,
    pub is_rebalancing: Option<bool>,
    pub mint_event_id: Option<u64>,
    /// Set once any confirmed input is selected for compensation.  A saga
    /// that began refunding must never later mint if a delayed input callback
    /// arrives and makes every pull appear confirmed.
    pub compensation_started: bool,
    pub error: Option<String>,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum LiquidityReceiptErrorV1 {
    InvalidIntentId,
    IntentConflict,
    CapacityExceeded,
    Unauthorized,
    InvalidRequest,
    NotFound,
    Concurrent,
    PoolLocked,
    RecoveryRequired,
}

pub fn key(owner: Principal, intent_id: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + owner.as_slice().len() + intent_id.len());
    out.push(owner.as_slice().len() as u8);
    out.extend_from_slice(owner.as_slice());
    out.extend_from_slice(intent_id);
    out
}

pub fn get(owner: Principal, intent_id: &[u8]) -> Option<LiquidityReceiptV1> {
    if intent_id.len() != 32 {
        return None;
    }
    storage::LIQUIDITY_RECEIPTS_V1.with(|map| map.borrow().get(&key(owner, intent_id)))
}

/// Check capacity without reserving a durable row.  Fresh public requests use
/// this while holding the global admission guard, so an invalid or unfunded
/// request cannot consume one of the bounded receipt slots.
pub fn capacity_available() -> bool {
    storage::LIQUIDITY_RECEIPTS_V1.with(|map| map.borrow().len() < MAX_RECEIPTS)
}

/// Pure funding gate used before a fresh receipt is reserved.  The source
/// balance must cover both the requested pull and its pinned ledger fee; the
/// ICRC-2 allowance must cover the full debit requested by this pool,
/// including the ledger fee.
pub fn preflight_leg_funded(
    amount: u128,
    fee: u128,
    max_transfer_fee: Option<u128>,
    source_balance: u128,
    allowance: u128,
) -> bool {
    amount > fee
        && max_transfer_fee.map_or(true, |cap| fee <= cap)
        && source_balance >= amount.saturating_add(fee)
        && allowance >= amount.saturating_add(fee)
}

pub fn save(receipt: &LiquidityReceiptV1) {
    storage::LIQUIDITY_RECEIPTS_V1.with(|map| {
        let key = key(receipt.owner, &receipt.request.intent_id);
        let mut map = map.borrow_mut();
        if map.get(&key).is_some_and(|existing| {
            matches!(
                &existing.status,
                LiquidityReceiptStatusV1::Completed
                    | LiquidityReceiptStatusV1::Refunded
                    | LiquidityReceiptStatusV1::Rejected
            ) && existing != *receipt
        }) {
            // Terminal rows are immutable tombstones.  A stale callback may
            // still hold an older local copy after an upgrade/recovery, but it
            // cannot reopen the saga or replace its durable result.
            return;
        }
        map.insert(key, receipt.clone());
    });
}

/// Persist the single active receipt fence before the first fee or ledger
/// await.  A later recovery may claim the same identity after an upgrade;
/// another receipt can never displace it.
pub fn claim_active(owner: Principal, intent_id: &[u8]) -> Result<(), LiquidityReceiptErrorV1> {
    let intent_id: [u8; 32] = intent_id
        .try_into()
        .map_err(|_| LiquidityReceiptErrorV1::InvalidIntentId)?;
    storage::claim_liquidity_receipt_fence(owner, intent_id)
        .map_err(|_| LiquidityReceiptErrorV1::PoolLocked)
}

/// Clear only the exact receipt's durable fence.  Terminal cleanup is
/// idempotent and a stale callback cannot unlock a different receipt.
pub fn clear_active(receipt: &LiquidityReceiptV1) -> bool {
    let Ok(intent_id) = receipt.request.intent_id.as_slice().try_into() else {
        return false;
    };
    storage::clear_liquidity_receipt_fence(receipt.owner, intent_id)
}

pub fn clear_if_terminal(receipt: &LiquidityReceiptV1) -> bool {
    if matches!(
        &receipt.status,
        LiquidityReceiptStatusV1::Completed
            | LiquidityReceiptStatusV1::Refunded
            | LiquidityReceiptStatusV1::Rejected
    ) {
        // A stale callback may carry a terminal local copy while recovery has
        // already moved the durable row back to RecoveryPending.  Only the
        // durable terminal row may release the persisted pool fence.
        let durable_terminal = get(receipt.owner, &receipt.request.intent_id)
            .is_some_and(|current| {
                matches!(
                    &current.status,
                    LiquidityReceiptStatusV1::Completed
                        | LiquidityReceiptStatusV1::Refunded
                        | LiquidityReceiptStatusV1::Rejected
                )
            });
        if durable_terminal {
            return clear_active(receipt);
        }
    }
    false
}

pub fn reserve(
    owner: Principal,
    request: AddLiquidityRequestV1,
) -> Result<(LiquidityReceiptV1, bool), LiquidityReceiptErrorV1> {
    if owner == Principal::anonymous() {
        return Err(LiquidityReceiptErrorV1::Unauthorized);
    }
    if request.intent_id.len() != 32 {
        return Err(LiquidityReceiptErrorV1::InvalidIntentId);
    }
    if request.amounts.len() != 3 || request.amounts.iter().all(|amount| *amount == 0) {
        return Err(LiquidityReceiptErrorV1::InvalidRequest);
    }
    if let Some(existing) = get(owner, &request.intent_id) {
        return if existing.request == request {
            Ok((existing, false))
        } else {
            Err(LiquidityReceiptErrorV1::IntentConflict)
        };
    }
    if storage::LIQUIDITY_RECEIPTS_V1.with(|map| map.borrow().len()) >= MAX_RECEIPTS {
        return Err(LiquidityReceiptErrorV1::CapacityExceeded);
    }
    let receipt = LiquidityReceiptV1 {
        version: 1,
        owner,
        request,
        status: LiquidityReceiptStatusV1::Prepared,
        input_ids: [None; 3],
        refund_ids: [None; 3],
        input_status: [None, None, None],
        refund_status: [None, None, None],
        input_transfers: std::array::from_fn(|_| None),
        refund_transfers: std::array::from_fn(|_| None),
        old_balances: None,
        old_lp_supply: None,
        old_caller_lp: None,
        balances_after: None,
        lp_supply_after: None,
        caller_lp_after: None,
        lp_minted: None,
        fee_bps: None,
        imbalance_before: None,
        imbalance_after: None,
        is_rebalancing: None,
        mint_event_id: None,
        compensation_started: false,
        error: None,
    };
    save(&receipt);
    Ok((receipt, true))
}

pub fn source_account(owner: Principal, source_subaccount: Option<[u8; 32]>) -> Account {
    Account {
        owner,
        subaccount: source_subaccount,
    }
}

pub fn reject(receipt: &mut LiquidityReceiptV1, reason: impl Into<String>) {
    receipt.status = LiquidityReceiptStatusV1::Rejected;
    receipt.error = Some(reason.into().chars().take(512).collect());
    save(receipt);
}

pub fn recovery_pending(receipt: &mut LiquidityReceiptV1, reason: impl Into<String>) {
    receipt.status = LiquidityReceiptStatusV1::RecoveryPending;
    receipt.error = Some(reason.into().chars().take(512).collect());
    save(receipt);
}

pub fn plan(
    receipt: &mut LiquidityReceiptV1,
    old_balances: [u128; 3],
    old_lp_supply: u128,
    old_caller_lp: u128,
    balances_after: [u128; 3],
    lp_supply_after: u128,
    caller_lp_after: u128,
    lp_minted: u128,
    fee_bps: u16,
    imbalance_before: u64,
    imbalance_after: u64,
    is_rebalancing: bool,
) {
    receipt.old_balances = Some(old_balances);
    receipt.old_lp_supply = Some(old_lp_supply);
    receipt.old_caller_lp = Some(old_caller_lp);
    receipt.balances_after = Some(balances_after);
    receipt.lp_supply_after = Some(lp_supply_after);
    receipt.caller_lp_after = Some(caller_lp_after);
    receipt.lp_minted = Some(lp_minted);
    receipt.fee_bps = Some(fee_bps);
    receipt.imbalance_before = Some(imbalance_before);
    receipt.imbalance_after = Some(imbalance_after);
    receipt.is_rebalancing = Some(is_rebalancing);
    save(receipt);
}

pub fn set_prepared_id(receipt: &mut LiquidityReceiptV1, index: usize, id: u64) {
    receipt.input_ids[index] = Some(id);
    receipt.refund_ids[index] = Some(id);
    receipt.input_status[index] = Some(LiquidityLegStatusV1::Prepared);
    receipt.refund_status[index] = Some(LiquidityLegStatusV1::Prepared);
    if let Some(entitlement) = payouts::get(id) {
        receipt.input_transfers[index] = entitlement.input_transfer.clone();
        receipt.refund_transfers[index] = entitlement.attempts.last().map(|attempt| attempt.transfer.clone());
    }
    save(receipt);
}

pub fn set_executing(receipt: &mut LiquidityReceiptV1) {
    receipt.status = LiquidityReceiptStatusV1::Executing;
    save(receipt);
}

pub fn sync_leg(receipt: &mut LiquidityReceiptV1, index: usize, entitlement: &payouts::PayoutEntitlement) {
    receipt.input_transfers[index] = entitlement.input_transfer.clone();
    receipt.refund_transfers[index] = entitlement.attempts.last().map(|attempt| attempt.transfer.clone());
    receipt.input_status[index] = entitlement.input_outcome.as_ref().map(input_status);
    receipt.refund_status[index] = entitlement.attempts.last().map(|attempt| payout_status(&attempt.outcome));
    save(receipt);
}

/// Persist a definitive no-transfer leg before its prepared payout identity
/// is removed.  The durable status is the tombstone that lets recovery accept
/// the intentionally missing payout row without treating an unknown identity
/// as proof of no effect.
pub fn mark_rejected_no_transfer(
    receipt: &mut LiquidityReceiptV1,
    index: usize,
    reason: impl Into<String>,
) {
    receipt.input_status[index] = Some(LiquidityLegStatusV1::RejectedNoTransfer {
        reason: reason.into().chars().take(512).collect(),
    });
    receipt.refund_status[index] = Some(LiquidityLegStatusV1::Skipped);
    save(receipt);
}

/// Persist that a prepared leg was never dispatched before canceling its
/// payout identity.  If cancellation fails, the caller must resync the live
/// payout row and leave the receipt retryable.
pub fn mark_skipped(receipt: &mut LiquidityReceiptV1, index: usize) {
    receipt.input_status[index] = Some(LiquidityLegStatusV1::Skipped);
    receipt.refund_status[index] = Some(LiquidityLegStatusV1::Skipped);
    save(receipt);
}

pub fn mark_refunded(receipt: &mut LiquidityReceiptV1, index: usize) {
    receipt.refund_status[index] = Some(LiquidityLegStatusV1::Refunded);
    save(receipt);
}

/// Return the terminal receipt status only when every nonzero request leg has
/// a durable, conservative terminal outcome.  Unknown or missing legs never
/// become terminal by absence.
pub fn terminal_status(receipt: &LiquidityReceiptV1) -> Option<LiquidityReceiptStatusV1> {
    let all_terminal = receipt.request.amounts.len() == 3
        && receipt.request.amounts.iter().enumerate().all(|(index, amount)| {
            *amount == 0
                || (receipt.input_ids[index].is_some()
                    && receipt.refund_ids[index].is_some()
                    && receipt.input_status[index].as_ref().is_some_and(|status| {
                    matches!(
                        status,
                        LiquidityLegStatusV1::Confirmed { .. }
                            | LiquidityLegStatusV1::RejectedNoTransfer { .. }
                            | LiquidityLegStatusV1::Skipped
                    )
                    }) && receipt.refund_status[index].as_ref().is_some_and(|status| {
                        matches!(
                            status,
                            LiquidityLegStatusV1::Refunded | LiquidityLegStatusV1::Skipped
                        )
                    }))
        });
    if !all_terminal {
        return None;
    }
    let any_confirmed = receipt.input_status.iter().any(|status| {
        matches!(status, Some(LiquidityLegStatusV1::Confirmed { .. }))
    });
    Some(if any_confirmed {
        LiquidityReceiptStatusV1::Refunded
    } else {
        LiquidityReceiptStatusV1::Rejected
    })
}

fn input_status(status: &payouts::PayoutInputOutcome) -> LiquidityLegStatusV1 {
    match status {
        payouts::PayoutInputOutcome::Prepared => LiquidityLegStatusV1::Prepared,
        payouts::PayoutInputOutcome::Submitted => LiquidityLegStatusV1::Submitted,
        payouts::PayoutInputOutcome::Confirmed { block } => LiquidityLegStatusV1::Confirmed { block: block.clone() },
        payouts::PayoutInputOutcome::RejectedNoTransfer { reason } => LiquidityLegStatusV1::RejectedNoTransfer { reason: reason.clone() },
        payouts::PayoutInputOutcome::Unresolved { reason } => LiquidityLegStatusV1::Unresolved { reason: reason.clone() },
    }
}

fn payout_status(status: &payouts::PayoutOutcome) -> LiquidityLegStatusV1 {
    match status {
        payouts::PayoutOutcome::Prepared => LiquidityLegStatusV1::Prepared,
        payouts::PayoutOutcome::Submitted => LiquidityLegStatusV1::Submitted,
        payouts::PayoutOutcome::Confirmed { block } => LiquidityLegStatusV1::Confirmed { block: block.clone() },
        payouts::PayoutOutcome::RejectedNoTransfer { reason } => LiquidityLegStatusV1::RejectedNoTransfer { reason: reason.clone() },
        payouts::PayoutOutcome::Unresolved { reason } => LiquidityLegStatusV1::Unresolved { reason: reason.clone() },
        payouts::PayoutOutcome::HeldLegacyUnbound => LiquidityLegStatusV1::Unresolved { reason: "legacy payout identity is unbound".into() },
    }
}

pub fn all_inputs_confirmed(receipt: &LiquidityReceiptV1) -> bool {
    receipt.request.amounts.len() == 3
        && receipt.request.amounts.iter().enumerate().all(|(index, amount)| {
            *amount == 0
                || matches!(
                    (receipt.input_ids[index], &receipt.input_status[index]),
                    (Some(_), Some(LiquidityLegStatusV1::Confirmed { .. }))
                )
        })
}

pub fn no_input_dispatched(receipt: &LiquidityReceiptV1) -> bool {
    receipt.input_ids.iter().flatten().all(|id| {
        matches!(
            storage::payouts::get(*id).and_then(|p| p.input_outcome),
            Some(payouts::PayoutInputOutcome::Prepared) | None
        )
    })
}

thread_local! {
    static LIVE_RECEIPTS: RefCell<BTreeSet<Vec<u8>>> = const { RefCell::new(BTreeSet::new()) };
}

#[derive(Debug)]
pub struct LiveGuard {
    key: Vec<u8>,
}

pub fn acquire_live(owner: Principal, intent_id: &[u8]) -> Result<LiveGuard, LiquidityReceiptErrorV1> {
    let key = key(owner, intent_id);
    LIVE_RECEIPTS.with(|live| {
        let mut live = live.borrow_mut();
        if !live.insert(key.clone()) {
            return Err(LiquidityReceiptErrorV1::Concurrent);
        }
        Ok(LiveGuard { key })
    })
}

impl Drop for LiveGuard {
    fn drop(&mut self) {
        LIVE_RECEIPTS.with(|live| {
            live.borrow_mut().remove(&self.key);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> AddLiquidityRequestV1 {
        AddLiquidityRequestV1 {
            intent_id: vec![7; 32],
            amounts: vec![100, 200, 300],
            min_lp: 1,
            source_subaccount: Some([9; 32]),
            max_transfer_fee: Some(10),
        }
    }

    #[test]
    fn request_collision_is_caller_scoped_and_parameter_bound() {
        let alice = Principal::self_authenticating(b"alice");
        let bob = Principal::self_authenticating(b"bob");
        let (row, fresh) = reserve(alice, request()).unwrap();
        assert!(fresh);
        let (same, fresh) = reserve(alice, request()).unwrap();
        assert!(!fresh);
        assert_eq!(same, row);
        let mut conflict = request();
        conflict.min_lp += 1;
        assert_eq!(reserve(alice, conflict).unwrap_err(), LiquidityReceiptErrorV1::IntentConflict);
        assert!(get(bob, &[7; 32]).is_none());
    }

    #[test]
    fn validation_precedes_allocation_and_source_is_bound() {
        let alice = Principal::self_authenticating(b"alice");
        assert_eq!(reserve(Principal::anonymous(), request()).unwrap_err(), LiquidityReceiptErrorV1::Unauthorized);
        let mut bad = request();
        bad.intent_id.push(0);
        assert_eq!(reserve(alice, bad).unwrap_err(), LiquidityReceiptErrorV1::InvalidIntentId);
        assert_eq!(source_account(alice, Some([3; 32])).subaccount, Some([3; 32]));
    }

    #[test]
    fn live_guard_is_transient_and_exclusive() {
        let owner = Principal::self_authenticating(b"owner");
        let first = acquire_live(owner, &[1; 32]).unwrap();
        assert_eq!(acquire_live(owner, &[1; 32]).unwrap_err(), LiquidityReceiptErrorV1::Concurrent);
        drop(first);
        assert!(acquire_live(owner, &[1; 32]).is_ok());
    }

    #[test]
    fn completed_receipt_roundtrips_and_duplicate_never_allocates() {
        let owner = Principal::self_authenticating(b"completed owner");
        let (mut row, fresh) = reserve(owner, request()).unwrap();
        assert!(fresh);
        row.status = LiquidityReceiptStatusV1::Completed;
        row.lp_minted = Some(777);
        row.mint_event_id = Some(12);
        save(&row);
        let encoded = candid::encode_one(&row).unwrap();
        let decoded: LiquidityReceiptV1 = candid::decode_one(&encoded).unwrap();
        assert_eq!(decoded, row);
        let (again, fresh) = reserve(owner, request()).unwrap();
        assert!(!fresh);
        assert_eq!(again, row);

        let mut stale = row.clone();
        stale.status = LiquidityReceiptStatusV1::RecoveryPending;
        save(&stale);
        assert_eq!(get(owner, &row.request.intent_id).unwrap(), row);
    }

    #[test]
    fn capacity_is_bounded_and_never_evicts_existing_rows() {
        assert_eq!(MAX_RECEIPTS, 10_000);
        let owner = Principal::self_authenticating(b"capacity owner");
        let (row, _) = reserve(owner, request()).unwrap();
        assert_eq!(reserve(owner, request()).unwrap().0, row);
        assert!(get(owner, &request().intent_id).is_some());
    }

    #[test]
    fn preflight_funding_requires_full_debit_and_fee_cap() {
        assert!(preflight_leg_funded(100, 10, Some(10), 110, 110));
        assert!(!preflight_leg_funded(100, 10, Some(10), 109, 110));
        assert!(!preflight_leg_funded(100, 10, Some(10), 110, 109));
        assert!(!preflight_leg_funded(100, 10, Some(9), 110, 110));
        assert!(!preflight_leg_funded(10, 10, None, 20, 20));
    }

    #[test]
    fn confirmed_without_identity_never_vacuously_confirms() {
        let owner = Principal::self_authenticating(b"confirmed without id");
        let (mut row, _) = reserve(owner, AddLiquidityRequestV1 {
            intent_id: vec![8; 32],
            amounts: vec![100, 0, 0],
            min_lp: 1,
            source_subaccount: None,
            max_transfer_fee: None,
        }).unwrap();
        row.input_status[0] = Some(LiquidityLegStatusV1::Confirmed { block: Nat::from(1u8) });
        assert!(!all_inputs_confirmed(&row));
        row.input_ids[0] = Some(42);
        assert!(all_inputs_confirmed(&row));
    }

    #[test]
    fn definitive_no_transfer_leg_is_terminal_rejection() {
        let owner = Principal::self_authenticating(b"no funds terminal");
        let (mut row, _) = reserve(
            owner,
            AddLiquidityRequestV1 {
                intent_id: vec![9; 32],
                amounts: vec![100, 0, 0],
                min_lp: 1,
                source_subaccount: None,
                max_transfer_fee: None,
            },
        )
        .unwrap();
        row.input_ids[0] = Some(101);
        row.refund_ids[0] = Some(101);
        mark_rejected_no_transfer(&mut row, 0, "allowance too low");
        assert_eq!(
            terminal_status(&row),
            Some(LiquidityReceiptStatusV1::Rejected)
        );
    }

    #[test]
    fn mixed_known_and_unknown_legs_remain_recovery_pending() {
        let owner = Principal::self_authenticating(b"mixed terminal");
        let (mut row, _) = reserve(
            owner,
            AddLiquidityRequestV1 {
                intent_id: vec![10; 32],
                amounts: vec![100, 200, 0],
                min_lp: 1,
                source_subaccount: None,
                max_transfer_fee: None,
            },
        )
        .unwrap();
        row.input_ids[0] = Some(102);
        row.refund_ids[0] = Some(102);
        row.input_ids[1] = Some(103);
        row.refund_ids[1] = Some(103);
        mark_rejected_no_transfer(&mut row, 0, "allowance too low");
        row.input_status[1] = Some(LiquidityLegStatusV1::Unresolved {
            reason: "ledger reply lost".into(),
        });
        row.refund_status[1] = Some(LiquidityLegStatusV1::Prepared);
        assert_eq!(terminal_status(&row), None);

        row.input_status[1] = Some(LiquidityLegStatusV1::Confirmed {
            block: Nat::from(77u8),
        });
        mark_refunded(&mut row, 1);
        assert_eq!(
            terminal_status(&row),
            Some(LiquidityReceiptStatusV1::Refunded)
        );
    }
}
