//! Treasury inter-canister helpers.
//!
//! Mint/transfer protocol revenue to the treasury canister and call
//! `treasury.deposit()` for categorized bookkeeping.
//!
//! All treasury operations are **non-critical**: failures are logged but
//! never block user-facing operations (borrow, repay, liquidation).

use candid::{CandidType, Deserialize, Nat, Principal};
use ic_canister_log::log;
use num_traits::ToPrimitive;
use serde::Serialize;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use crate::logs::INFO;
use crate::management;
use crate::numeric::ICUSD;
use crate::state::read_state;

const BORROWING_FEE_ONCE_METHOD: &str = "deposit_borrowing_fee_once";
const INTEREST_REVENUE_ONCE_METHOD: &str = "deposit_interest_revenue_once";
const MAX_PENDING_TREASURY_INTEREST_MINTS: usize = 2048;
const TREASURY_INTEREST_RETRY_BATCH: usize = 16;
/// Stop automatic dispatch one hour before the standard 24-hour ICRC-1
/// deduplication horizon. Old/ambiguous rows enter positive-receipt recovery.
const TREASURY_STABLE_AUTO_RETRY_MAX_AGE_NS: u64 = 23 * 60 * 60 * 1_000_000_000;
const TREASURY_INTEREST_AUTO_RETRY_MAX_AGE_NS: u64 = 24 * 60 * 60 * 1_000_000_000;
const MAX_TREASURY_INTEREST_RECOVERY_PAGE: u16 = 100;

#[derive(CandidType, Clone, Debug, PartialEq, Eq)]
pub struct TreasuryInterestMintRecoveryRow {
    pub tuple: crate::state::BorrowMintTuple,
    pub treasury: Principal,
    pub phase: crate::state::TreasuryInterestMintPhase,
    pub history_floor: Option<u64>,
    pub expiry_proven: bool,
    pub absence_scan: Option<crate::state::BorrowMintAbsenceScan>,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq)]
pub struct TreasuryInterestMintRecoveryPage {
    pub items: Vec<TreasuryInterestMintRecoveryRow>,
    pub next_cursor: Option<u128>,
}

pub fn treasury_interest_mint_recovery_page(
    after_op_nonce: Option<u128>,
    limit: u16,
) -> Result<TreasuryInterestMintRecoveryPage, String> {
    if limit == 0 {
        return Err("page limit must be greater than zero".into());
    }
    let limit = usize::from(limit.min(MAX_TREASURY_INTEREST_RECOVERY_PAGE));
    crate::state::read_state(|state| {
        use std::ops::Bound::{Excluded, Unbounded};
        let start = after_op_nonce.map_or(Unbounded, Excluded);
        let mut rows = state.pending_treasury_interest_mints.range((start, Unbounded));
        let mut items = Vec::with_capacity(limit);
        for (_, row) in rows.by_ref().take(limit) {
            items.push(TreasuryInterestMintRecoveryRow {
                tuple: row.tuple.clone(),
                treasury: row.treasury,
                phase: row.phase.clone(),
                history_floor: row.history_floor,
                expiry_proven: row.expiry_proven,
                absence_scan: row.absence_scan.clone(),
            });
        }
        let next_cursor = if rows.next().is_some() {
            items.last().map(|row| row.tuple.op_nonce)
        } else {
            None
        };
        Ok(TreasuryInterestMintRecoveryPage { items, next_cursor })
    })
}

// ---------------------------------------------------------------------------
// Mirror types matching rumi_treasury::types (can't depend on cdylib crate)
// ---------------------------------------------------------------------------

/// Mirrors `rumi_treasury::types::DepositType`.
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum DepositType {
    BorrowingFee,
    RedemptionFee,
    LiquidationFee,
    InterestRevenue,
}

/// Mirrors `rumi_treasury::types::AssetType`.
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum AssetType {
    ICUSD,
    ICP,
    CKBTC,
    CKUSDT,
    CKUSDC,
}

/// Mirror the Stability Pool error wire shape without depending on its cdylib
/// crate. A Candid `unknown` payload cannot decode the pool's typed error
/// variant, so keep this list aligned with `stability_pool::StabilityPoolError`.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
enum StabilityPoolInterestNotificationError {
    InsufficientBalance {
        token: Principal,
        required: u64,
        available: u64,
    },
    AmountTooLow { minimum_e8s: u64 },
    NoPositionFound,
    InsufficientPoolBalance,
    Unauthorized,
    TokenNotAccepted { ledger: Principal },
    TokenNotActive { ledger: Principal },
    CollateralNotFound { ledger: Principal },
    LedgerTransferFailed { reason: String },
    InterCanisterCallFailed { target: String, method: String },
    LiquidationFailed { vault_id: u64, reason: String },
    EmergencyPaused,
    SystemBusy,
    AlreadyOptedOut { collateral: Principal },
    AlreadyOptedIn { collateral: Principal },
    PayoutAddressRequired { collateral: Principal },
    InvalidPayoutAddress { reason: String },
    XrpClaimStillOutstanding { claim_id: u64 },
    XrpClaimStatusCheckFailed { reason: String },
    RefundClaimNotFound,
}

/// Only `Ok` acknowledges a post-mint receipt. Every typed error remains
/// pending and can be retried with the same mint-block idempotency key.
#[derive(CandidType, Deserialize)]
enum StabilityPoolInterestNotificationResult {
    Ok,
    Err(StabilityPoolInterestNotificationError),
}

type InterestNotificationKey = (Principal, Principal, u64);

thread_local! {
    /// Prevents the inline post-mint delivery and timer flush from issuing the
    /// same inter-canister notification concurrently in one backend instance.
    static IN_FLIGHT_INTEREST_NOTIFICATIONS: RefCell<BTreeSet<InterestNotificationKey>> =
        RefCell::new(BTreeSet::new());
    static IN_FLIGHT_TREASURY_INTEREST_MINTS: RefCell<BTreeSet<u128>> =
        RefCell::new(BTreeSet::new());
    static IN_FLIGHT_TREASURY_STABLE_TRANSFERS: RefCell<BTreeSet<crate::state::TreasuryStableTransferSource>> =
        RefCell::new(BTreeSet::new());
}

struct TreasuryStableTransferGuard(crate::state::TreasuryStableTransferSource);
impl TreasuryStableTransferGuard {
    fn try_new(source: &crate::state::TreasuryStableTransferSource) -> Option<Self> {
        IN_FLIGHT_TREASURY_STABLE_TRANSFERS.with(|busy| {
            busy.borrow_mut().insert(source.clone()).then_some(Self(source.clone()))
        })
    }
}
impl Drop for TreasuryStableTransferGuard {
    fn drop(&mut self) {
        IN_FLIGHT_TREASURY_STABLE_TRANSFERS.with(|busy| { busy.borrow_mut().remove(&self.0); });
    }
}

struct TreasuryInterestMintGuard(u128);

impl TreasuryInterestMintGuard {
    fn try_new(op_nonce: u128) -> Option<Self> {
        IN_FLIGHT_TREASURY_INTEREST_MINTS.with(|in_flight| {
            in_flight.borrow_mut().insert(op_nonce).then_some(Self(op_nonce))
        })
    }
}

impl Drop for TreasuryInterestMintGuard {
    fn drop(&mut self) {
        IN_FLIGHT_TREASURY_INTEREST_MINTS.with(|in_flight| {
            in_flight.borrow_mut().remove(&self.0);
        });
    }
}

struct InterestNotificationGuard(InterestNotificationKey);

fn has_receipt_safe_delivery_protocol(
    notification: &crate::state::PendingStabilityPoolInterestNotification,
) -> bool {
    notification.receipt_protocol_version == Some(1)
}

fn notification_delivery_acknowledged(
    notification: &crate::state::PendingStabilityPoolInterestNotification,
    delivered: bool,
) -> bool {
    delivered && has_receipt_safe_delivery_protocol(notification)
}

impl InterestNotificationGuard {
    fn try_new(key: InterestNotificationKey) -> Option<Self> {
        IN_FLIGHT_INTEREST_NOTIFICATIONS.with(|in_flight| {
            let inserted = in_flight.borrow_mut().insert(key);
            inserted.then(|| Self(key))
        })
    }
}

impl Drop for InterestNotificationGuard {
    fn drop(&mut self) {
        IN_FLIGHT_INTEREST_NOTIFICATIONS.with(|in_flight| {
            in_flight.borrow_mut().remove(&self.0);
        });
    }
}

/// Mirrors `rumi_treasury::types::DepositArgs`.
#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct DepositArgs {
    pub deposit_type: DepositType,
    pub asset_type: AssetType,
    pub amount: u64,
    pub block_index: u64,
    pub memo: Option<String>,
}

/// Mirrors the new replay-safe Treasury endpoint. It is deliberately distinct
/// from legacy `deposit`, whose block handling is not exact-replay safe.
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ExternalTokenDepositArgs {
    pub ledger: Principal,
    pub deposit_type: DepositType,
    pub asset_type: AssetType,
    pub amount: u64,
    pub block_index: u64,
    pub memo: Option<String>,
}

#[cfg(test)]
mod external_token_treasury_candid_compatibility_tests {
    use super::{AssetType, DepositType, ExternalTokenDepositArgs};
    use candid::{CandidType, Principal};

    // Frozen counterpart of src/rumi_treasury/src/types.rs on origin/main
    // 4c824fb1. Keep the structural byte comparison so backend mirrors cannot
    // silently drift from the receiver's new method argument.
    #[derive(CandidType)]
    enum ReceiverDepositType { BorrowingFee, RedemptionFee, LiquidationFee, InterestRevenue }
    #[derive(CandidType)]
    enum ReceiverAssetType { ICUSD, ICP, CKBTC, CKUSDT, CKUSDC }
    #[derive(CandidType)]
    struct ReceiverExternalTokenDepositArgs {
        ledger: Principal,
        deposit_type: ReceiverDepositType,
        asset_type: ReceiverAssetType,
        amount: u64,
        block_index: u64,
        memo: Option<String>,
    }

    #[test]
    fn outgoing_method_argument_matches_merged_receiver_candid_shape() {
        let ledger = Principal::from_slice(&[1]);
        for (backend_deposit, receiver_deposit, backend_asset, receiver_asset) in [
            (DepositType::InterestRevenue, ReceiverDepositType::InterestRevenue,
                AssetType::CKUSDT, ReceiverAssetType::CKUSDT),
            (DepositType::InterestRevenue, ReceiverDepositType::InterestRevenue,
                AssetType::CKUSDC, ReceiverAssetType::CKUSDC),
            (DepositType::LiquidationFee, ReceiverDepositType::LiquidationFee,
                AssetType::CKUSDT, ReceiverAssetType::CKUSDT),
            (DepositType::LiquidationFee, ReceiverDepositType::LiquidationFee,
                AssetType::CKUSDC, ReceiverAssetType::CKUSDC),
        ] {
            let backend = ExternalTokenDepositArgs {
                ledger,
                deposit_type: backend_deposit,
                asset_type: backend_asset,
                amount: 123_456,
                block_index: 789,
                memo: Some("rumi-stable-v1:source".into()),
            };
            let receiver = ReceiverExternalTokenDepositArgs {
                ledger,
                deposit_type: receiver_deposit,
                asset_type: receiver_asset,
                amount: 123_456,
                block_index: 789,
                memo: Some("rumi-stable-v1:source".into()),
            };
            assert_eq!(candid::encode_one(backend).unwrap(), candid::encode_one(receiver).unwrap());
        }
    }
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq)]
pub struct TreasuryStableTransferRecoveryPage {
    pub items: Vec<crate::state::TreasuryStableTransferJournal>,
    pub next_cursor: Option<crate::state::TreasuryStableTransferSource>,
}

pub fn treasury_stable_transfer_recovery_page(
    after_source: Option<crate::state::TreasuryStableTransferSource>, limit: u16,
) -> Result<TreasuryStableTransferRecoveryPage, String> {
    if limit == 0 { return Err("page limit must be greater than zero".into()); }
    let limit = usize::from(limit.min(100));
    crate::state::read_state(|s| {
        use std::ops::Bound::{Excluded, Unbounded};
        let mut rows: Vec<_> = match after_source {
            Some(cursor) => s.pending_treasury_stable_transfers.range((Excluded(cursor), Unbounded))
                .take(limit + 1).map(|(_, row)| row.clone()).collect(),
            None => s.pending_treasury_stable_transfers.iter().take(limit + 1)
                .map(|(_, row)| row.clone()).collect(),
        };
        let has_more = rows.len() > limit;
        rows.truncate(limit);
        let next_cursor = if has_more { rows.last().map(|row| row.source.clone()) } else { None };
        Ok(TreasuryStableTransferRecoveryPage { items: rows, next_cursor })
    })
}

/// Developer-authorized resolution can bind missing configuration before
/// preflight, resume a confirmed receipt when its frozen configuration returns,
/// or reissue only after a complete positive absence proof. That last action
/// preserves the source obligation and payload while assigning a fresh ledger
/// attempt tuple.
pub fn resolve_pending_treasury_stable_transfer(op_nonce: u128, caller: Principal) -> Result<(), String> {
    crate::state::mutate_state(|s| {
        if caller == Principal::anonymous() || caller != s.developer_principal {
            return Err("developer authorization required".into());
        }
        let Some((source, expected)) = s.pending_treasury_stable_transfers.iter()
            .find(|(_, row)| row.op_nonce == op_nonce).map(|(source, row)| (source.clone(), row.clone()))
        else { return Err("no pending Treasury stable transfer exists".into()); };
        let configured_ledger = match &expected.asset_type {
            AssetType::CKUSDT => s.ckusdt_ledger_principal,
            AssetType::CKUSDC => s.ckusdc_ledger_principal,
            _ => None,
        };
        if configured_ledger != Some(expected.ledger) {
            return Err("configured stable ledger does not match the frozen transfer tuple".into());
        }
        let mut updated = expected.clone();
        match expected.phase {
            crate::state::TreasuryStableTransferPhase::ReceiptRecoveryRequired => {
                rearm_stable_transfer_after_proven_absence(s, &source, &expected, ic_cdk::api::time())?;
                return Ok(());
            }
            crate::state::TreasuryStableTransferPhase::ConfigurationHold
                if expected.fee_e6s.is_none() && expected.history_floor.is_none() => {
                    let configured_treasury = s.treasury_principal.ok_or_else(|| "Treasury is not configured".to_string())?;
                    if expected.treasury.is_some_and(|pinned| pinned != configured_treasury) {
                        return Err("configured Treasury differs from the frozen destination".into());
                    }
                    updated.treasury = Some(configured_treasury);
                    updated.phase = crate::state::TreasuryStableTransferPhase::AwaitingPreflight;
                }
            crate::state::TreasuryStableTransferPhase::ConfigurationReviewRequired { block_index } => {
                if expected.treasury != s.treasury_principal {
                    return Err("configured Treasury differs from the frozen destination".into());
                }
                updated.phase = crate::state::TreasuryStableTransferPhase::NotifyPending { block_index };
            }
            _ => return Err("row is not in an explicitly resolvable phase".into()),
        }
        crate::storage::record_pending_payout_event(
            &crate::event::PendingPayoutEvent::TreasuryStableTransferUpdated { operation: updated.clone() },
        );
        s.pending_treasury_stable_transfers.insert(source, updated);
        Ok(())
    })
}

/// A new ledger attempt is permitted only after the old exact tuple has aged
/// out, a typed TooOld response was persisted, and every direct block in the
/// pinned history prefix was scanned without an exact receipt. This is an
/// explicit developer action; age alone never authorizes a second payout.
fn rearm_stable_transfer_after_proven_absence(
    s: &mut crate::state::State,
    source: &crate::state::TreasuryStableTransferSource,
    expected: &crate::state::TreasuryStableTransferJournal,
    now: u64,
) -> Result<(), String> {
    let scan = expected.recovery_scan.as_ref()
        .ok_or_else(|| "absence proof is incomplete".to_string())?;
    if !expected.expiry_proven
        || expected.phase != crate::state::TreasuryStableTransferPhase::ReceiptRecoveryRequired
        || expected.fee_e6s.is_none()
        || expected.history_floor.is_none()
        || scan.fixed_tip.is_none()
        || scan.next_block_index != scan.fixed_tip.unwrap_or_default()
        || scan.candidate_block_index.is_some()
    {
        return Err("a complete expiry and no-receipt proof is required".into());
    }
    if s.pending_treasury_stable_transfers.get(source) != Some(expected) {
        return Err("stable transfer changed during resolution".into());
    }
    let nonce = s.next_op_nonce_at(now);
    let mut updated = expected.clone();
    updated.op_nonce = nonce;
    updated.memo = management::nonce_to_memo(nonce).0.as_slice()
        .try_into().expect("nonce memo is 16 bytes");
    updated.created_at_time_ns = management::nonce_to_created_at_time(nonce);
    updated.fee_e6s = None;
    updated.history_floor = None;
    updated.recovery_scan = None;
    updated.expiry_proven = false;
    updated.phase = crate::state::TreasuryStableTransferPhase::AwaitingPreflight;
    crate::storage::record_pending_payout_event(
        &crate::event::PendingPayoutEvent::TreasuryStableTransferUpdated { operation: updated.clone() },
    );
    s.pending_treasury_stable_transfers.insert(source.clone(), updated);
    Ok(())
}

pub async fn advance_pending_treasury_stable_transfer_recovery(
    op_nonce: u128, caller: Principal,
) -> Result<(), String> {
    let Some(source) = crate::state::read_state(|s| s.pending_treasury_stable_transfers.iter()
        .find(|(_, row)| row.op_nonce == op_nonce).map(|(source, _)| source.clone())) else {
        return Err("no pending Treasury stable transfer exists".into());
    };
    if caller == Principal::anonymous()
        || caller != crate::state::read_state(|s| s.developer_principal) {
        return Err("developer authorization required".into());
    }
    process_treasury_stable_transfer(source).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Wave-8e LIQ-005: fee → deficit routing planner
// ---------------------------------------------------------------------------

/// Outcome of routing a fee through the deficit-repayment path.
///
/// `to_repay` is the icUSD applied to deficit repayment (mint foregone for
/// borrowing fees, supply already reduced for redemption fees).
/// `to_remainder` is what flows to the existing destination — for borrowing
/// fees that's the treasury mint amount; for redemption fees that's the
/// portion of fee revenue that accrues as protocol equity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeeRoutingOutcome {
    pub to_repay: ICUSD,
    pub to_remainder: ICUSD,
}

/// Plan how a fee splits between deficit repayment and its existing
/// destination. Mutates state to apply the repayment + emit
/// `DeficitRepaid` (so callers stay one-liners), then returns the split
/// for the caller to act on.
///
/// `anchor_block_index` on the emitted event is `None`; the ledger op
/// (treasury mint for borrowing fee, the redeemer's burn for redemption
/// fee) hasn't happened yet at this point. Callers can correlate via the
/// op_nonce on subsequent ledger entries.
///
/// In production the canister wrapper [`plan_fee_routing`] passes
/// `ic_cdk::api::time()`; the inner `_at` form takes the timestamp
/// explicitly so unit tests can drive it without a canister context.
pub fn plan_fee_routing_at(
    state: &mut crate::state::State,
    fee: crate::numeric::ICUSD,
    source: crate::event::FeeSource,
    timestamp: u64,
) -> FeeRoutingOutcome {
    if fee.0 == 0 {
        return FeeRoutingOutcome {
            to_repay: crate::numeric::ICUSD::new(0),
            to_remainder: crate::numeric::ICUSD::new(0),
        };
    }
    let to_repay = state.compute_deficit_repay_amount(fee);
    if to_repay.0 > 0 {
        crate::event::record_deficit_repaid(state, to_repay, source, None, timestamp);
    }
    let to_remainder = crate::numeric::ICUSD::new(fee.0 - to_repay.0);
    FeeRoutingOutcome {
        to_repay,
        to_remainder,
    }
}

/// Production wrapper around [`plan_fee_routing_at`] that captures the
/// canister time. Call sites stay one-liners.
pub fn plan_fee_routing(
    state: &mut crate::state::State,
    fee: crate::numeric::ICUSD,
    source: crate::event::FeeSource,
) -> FeeRoutingOutcome {
    plan_fee_routing_at(state, fee, source, ic_cdk::api::time())
}

// ---------------------------------------------------------------------------
// Helper: map collateral ledger principal → AssetType
// ---------------------------------------------------------------------------

/// Map a collateral ledger principal to the treasury's AssetType enum.
/// Uses known ckStable ledger principals from state config, plus ICP ledger.
pub fn collateral_to_asset_type(ct: &Principal) -> AssetType {
    read_state(|s| {
        if *ct == s.icp_ledger_principal {
            return AssetType::ICP;
        }
        if let Some(ckusdt) = s.ckusdt_ledger_principal {
            if *ct == ckusdt {
                return AssetType::CKUSDT;
            }
        }
        if let Some(ckusdc) = s.ckusdc_ledger_principal {
            if *ct == ckusdc {
                return AssetType::CKUSDC;
            }
        }
        // For any other collateral (ckBTC, ckETH, etc.), default to ICP
        // since treasury AssetType only has ICP/CKBTC/CKUSDT/CKUSDC.
        // TODO: Expand AssetType when new collaterals are added.
        AssetType::ICP
    })
}

// ---------------------------------------------------------------------------
// Inter-canister call to treasury.deposit()
// ---------------------------------------------------------------------------

/// Notify the treasury canister about a deposit (for bookkeeping).
/// Non-critical: failures are logged but don't affect protocol operation.
pub async fn notify_treasury_deposit(
    treasury: Principal,
    deposit_type: DepositType,
    asset_type: AssetType,
    amount: u64,
    block_index: u64,
) -> Result<u64, String> {
    let args = DepositArgs {
        deposit_type,
        asset_type,
        amount,
        block_index,
        memo: None,
    };
    let result: Result<(Result<u64, String>,), _> =
        ic_cdk::call(treasury, "deposit", (args,)).await;
    match result {
        Ok((Ok(deposit_id),)) => {
            log!(INFO, "[treasury] Deposit recorded: id={}", deposit_id);
            Ok(deposit_id)
        }
        Ok((Err(e),)) => {
            log!(INFO, "[treasury] WARNING: deposit recording failed: {}", e);
            Err(e)
        }
        Err((code, msg)) => {
            log!(
                INFO,
                "[treasury] WARNING: inter-canister call failed: {:?} {}",
                code,
                msg
            );
            Err(msg)
        }
    }
}

/// Create an immutable source-keyed obligation synchronously with the source
/// event/state transition. Repeated source callbacks return the original row.
pub fn admit_treasury_stable_transfer(
    source: crate::state::TreasuryStableTransferSource,
    transfer_ledger: Principal,
    amount_e6s: u64,
    asset_type: AssetType,
    deposit_type: DepositType,
) -> Option<u128> {
    if amount_e6s == 0 { return None; }
    crate::state::mutate_state(|s| {
        let treasury = s.treasury_principal;
        admit_treasury_stable_transfer_in_state(
            s, source, transfer_ledger, treasury, amount_e6s, asset_type, deposit_type,
        )
    })
}

pub fn admit_treasury_stable_transfer_in_state(
    s: &mut crate::state::State,
    source: crate::state::TreasuryStableTransferSource,
    transfer_ledger: Principal,
    frozen_treasury: Option<Principal>,
    amount_e6s: u64,
    asset_type: AssetType,
    deposit_type: DepositType,
) -> Option<u128> {
    admit_treasury_stable_transfer_in_state_at(
        s, source, transfer_ledger, frozen_treasury, amount_e6s, asset_type,
        deposit_type, ic_cdk::api::time(),
    )
}

fn admit_treasury_stable_transfer_in_state_at(
    s: &mut crate::state::State,
    source: crate::state::TreasuryStableTransferSource,
    transfer_ledger: Principal,
    frozen_treasury: Option<Principal>,
    amount_e6s: u64,
    asset_type: AssetType,
    deposit_type: DepositType,
    now: u64,
) -> Option<u128> {
    if amount_e6s == 0 { return None; }
    if s.completed_treasury_stable_transfer_sources.contains_key(&source) { return None; }
    if let Some(existing) = s.pending_treasury_stable_transfers.get(&source) {
        if existing.amount_e6s == amount_e6s && existing.asset_type == asset_type
            && existing.deposit_type == deposit_type && existing.ledger == transfer_ledger
            && existing.treasury == frozen_treasury {
            return Some(existing.op_nonce);
        }
        let mut held = existing.clone();
        held.phase = crate::state::TreasuryStableTransferPhase::ReceiptRecoveryRequired;
        held.recovery_scan = None;
        crate::storage::record_pending_payout_event(
            &crate::event::PendingPayoutEvent::TreasuryStableTransferUpdated { operation: held.clone() },
        );
        s.pending_treasury_stable_transfers.insert(source, held);
        return None;
    }
    let treasury = frozen_treasury;
    let configured_ledger = match &asset_type {
            AssetType::CKUSDT => s.ckusdt_ledger_principal,
            AssetType::CKUSDC => s.ckusdc_ledger_principal,
            _ => None,
        };
    let ledger = transfer_ledger;
    let configuration_matches = treasury.is_some()
        && s.treasury_principal == treasury && configured_ledger == Some(ledger);
    let nonce = s.next_op_nonce_at(now);
    let operation = crate::state::TreasuryStableTransferJournal {
        source: source.clone(), ledger, treasury, asset_type, deposit_type, amount_e6s,
        fee_e6s: None, history_floor: None, recovery_scan: None, expiry_proven: false,
        memo: management::nonce_to_memo(nonce).0.as_slice().try_into().expect("nonce memo is 16 bytes"),
        created_at_time_ns: management::nonce_to_created_at_time(nonce), op_nonce: nonce,
        phase: if configuration_matches { crate::state::TreasuryStableTransferPhase::AwaitingPreflight }
            else { crate::state::TreasuryStableTransferPhase::ConfigurationHold },
    };
    crate::storage::record_pending_payout_event(
        &crate::event::PendingPayoutEvent::TreasuryStableTransferStarted { operation: operation.clone() },
    );
    s.pending_treasury_stable_transfers.insert(source, operation);
    Some(nonce)
}

fn persist_stable_transfer(
    expected: &crate::state::TreasuryStableTransferJournal,
    operation: &crate::state::TreasuryStableTransferJournal,
) -> bool {
    crate::state::mutate_state(|s| {
        if s.pending_treasury_stable_transfers.get(&operation.source) != Some(expected) { return false; }
        crate::storage::record_pending_payout_event(
            &crate::event::PendingPayoutEvent::TreasuryStableTransferUpdated { operation: operation.clone() },
        );
        s.pending_treasury_stable_transfers.insert(operation.source.clone(), operation.clone());
        true
    })
}

fn complete_stable_transfer(
    expected: &crate::state::TreasuryStableTransferJournal,
    block_index: u64,
) -> bool {
    crate::state::mutate_state(|s| {
        if s.pending_treasury_stable_transfers.get(&expected.source) != Some(expected)
            || expected.phase != (crate::state::TreasuryStableTransferPhase::NotifyPending { block_index }) {
            return false;
        }
        crate::storage::record_pending_payout_event(
            &crate::event::PendingPayoutEvent::TreasuryStableTransferCompleted {
                source: expected.source.clone(), block_index,
            },
        );
        s.pending_treasury_stable_transfers.remove(&expected.source);
        s.completed_treasury_stable_transfer_sources.insert(expected.source.clone(), Some(block_index));
        true
    })
}

fn stable_transfer_with_initial_scan(
    operation: &crate::state::TreasuryStableTransferJournal,
    fixed_tip: u64,
) -> Option<crate::state::TreasuryStableTransferJournal> {
    let floor = operation.history_floor?;
    if fixed_tip < floor { return None; }
    let mut updated = operation.clone();
    updated.recovery_scan = Some(crate::state::BorrowMintAbsenceScan {
        next_block_index: floor,
        fixed_tip: Some(fixed_tip),
        candidate_block_index: None,
    });
    Some(updated)
}

fn treasury_external_memo(source: &crate::state::TreasuryStableTransferSource) -> String {
    // ASCII, bounded (<128 bytes), stable across retries and independent from
    // process-local nonce generation.
    format!("rumi-stable-v1:{}:{}:{}:{}", source.ledger.to_text(), source.block_index, source.lane, source.slot)
}

async fn notify_external_stable_transfer(
    operation: &crate::state::TreasuryStableTransferJournal,
    block_index: u64,
) -> Result<(), String> {
    let configured = crate::state::read_state(|s| {
        let ledger = match operation.asset_type {
            AssetType::CKUSDT => s.ckusdt_ledger_principal,
            AssetType::CKUSDC => s.ckusdc_ledger_principal,
            _ => None,
        };
        operation.treasury.is_some_and(|treasury| s.treasury_principal == Some(treasury))
            && ledger == Some(operation.ledger)
    });
    if !configured {
        let mut held = operation.clone();
        held.phase = crate::state::TreasuryStableTransferPhase::ConfigurationReviewRequired { block_index };
        persist_stable_transfer(operation, &held);
        return Err("Treasury or stablecoin ledger configuration differs from the frozen transfer".into());
    }
    let args = ExternalTokenDepositArgs {
        ledger: operation.ledger,
        deposit_type: operation.deposit_type.clone(),
        asset_type: operation.asset_type.clone(),
        amount: operation.amount_e6s,
        block_index,
        memo: Some(treasury_external_memo(&operation.source)),
    };
    let result: Result<(Result<u64, String>,), _> =
        ic_cdk::call(operation.treasury.expect("configuration checked"), "record_external_token_deposit_once", (args,)).await;
    match result {
        Ok((Ok(_),)) => {
            complete_stable_transfer(operation, block_index);
            Ok(())
        }
        Ok((Err(error),)) => Err(error),
        Err((_, error)) => Err(error),
    }
}

async fn advance_stable_transfer_receipt_scan(
    operation: &crate::state::TreasuryStableTransferJournal,
) {
    if operation.history_floor.is_none() { return; }
    let Some(treasury) = operation.treasury else { return; };
    let mut current = operation.clone();
    if !current.expiry_proven {
        let tuple = crate::state::BorrowMintTuple {
            ledger: current.ledger, destination: treasury, amount_e8s: current.amount_e6s,
            memo: current.memo, created_at_time_ns: current.created_at_time_ns,
            op_nonce: current.op_nonce,
        };
        if probe_stable_transfer_expiry(&tuple).await.is_err() { return; }
        let mut expired = current.clone();
        expired.expiry_proven = true;
        if !persist_stable_transfer(&current, &expired) { return; }
        current = expired;
    }
    let scan = if let Some(scan) = current.recovery_scan.clone() {
        scan
    } else {
        let Ok(tip) = crate::icrc3_proof::icrc3_history_floor(current.ledger).await else { return; };
        let Some(updated) = stable_transfer_with_initial_scan(&current, tip) else { return; };
        if !persist_stable_transfer(&current, &updated) { return; }
        current = updated.clone();
        updated.recovery_scan.unwrap()
    };
    let Some(tip) = scan.fixed_tip else { return; };
    if scan.next_block_index >= tip { return; }
    let start = scan.next_block_index;
    let end = tip.min(start.saturating_add(64));
    let request = vec![icrc_ledger_types::icrc3::blocks::GetBlocksRequest {
        start: candid::Nat::from(start), length: candid::Nat::from(end - start),
    }];
    let result: Result<(icrc_ledger_types::icrc3::blocks::GetBlocksResult,), _> =
        ic_cdk::call(operation.ledger, "icrc3_get_blocks", (request,)).await;
    let Ok((response,)) = result else { return; };
    let tuple = crate::state::BorrowMintTuple {
        ledger: operation.ledger, destination: treasury, amount_e8s: operation.amount_e6s,
        memo: operation.memo, created_at_time_ns: operation.created_at_time_ns,
        op_nonce: operation.op_nonce,
    };
    let Some(expected_fee) = operation.fee_e6s else { return; };
    let Ok(candidate) = crate::icrc3_proof::validate_treasury_stable_transfer_scan_page(
        start, end, tip, &response, ic_cdk::id(), &tuple, expected_fee,
    ) else { return; };
    let mut advanced_scan = scan;
    if let Some(block_index) = candidate {
        if advanced_scan.candidate_block_index.is_some_and(|prior| prior != block_index) {
            return;
        }
        advanced_scan.candidate_block_index = Some(block_index);
    }
    advanced_scan.next_block_index = end;
    let mut advanced = current.clone();
    if end == tip {
        if let Some(block_index) = advanced_scan.candidate_block_index {
            advanced.phase = crate::state::TreasuryStableTransferPhase::NotifyPending { block_index };
            advanced.recovery_scan = None;
        } else {
            // Complete, archive-free scan with typed TooOld expiry and no
            // exact tuple. Retain the proof for developer-authorized reissue.
            advanced.recovery_scan = Some(advanced_scan);
        }
    } else {
        advanced.recovery_scan = Some(advanced_scan);
    }
    if persist_stable_transfer(&current, &advanced) {
        if let crate::state::TreasuryStableTransferPhase::NotifyPending { block_index } = advanced.phase {
            let _ = notify_external_stable_transfer(&advanced, block_index).await;
        }
    }
}

struct StableTransferExpiryProbeArgs {
    subaccount: [u8; 32],
    transfer: icrc_ledger_types::icrc1::transfer::TransferArg,
}

fn stable_transfer_expiry_probe_args(
    tuple: &crate::state::BorrowMintTuple,
    entropy: [u8; 32],
) -> Result<StableTransferExpiryProbeArgs, String> {
    use icrc_ledger_types::icrc1::account::Account;
    use icrc_ledger_types::icrc1::transfer::{Memo, TransferArg};
    use sha2::{Digest, Sha256};

    let mut subaccount_hash = Sha256::new();
    subaccount_hash.update(b"RUMI-STABLE-TRANSFER-EXPIRY-SUBACCOUNT-V1");
    subaccount_hash.update(tuple.ledger.as_slice());
    subaccount_hash.update(tuple.destination.as_slice());
    subaccount_hash.update(tuple.op_nonce.to_be_bytes());
    subaccount_hash.update(tuple.created_at_time_ns.to_be_bytes());
    subaccount_hash.update(entropy);
    let subaccount: [u8; 32] = subaccount_hash.finalize().into();

    let mut sink_seed = b"RUMI-STABLE-TRANSFER-EXPIRY-SINK-V2".to_vec();
    sink_seed.extend_from_slice(tuple.ledger.as_slice());
    sink_seed.extend_from_slice(tuple.destination.as_slice());
    sink_seed.extend_from_slice(&tuple.op_nonce.to_be_bytes());
    sink_seed.extend_from_slice(&tuple.created_at_time_ns.to_be_bytes());
    sink_seed.extend_from_slice(&entropy);
    let sink = Principal::self_authenticating(&sink_seed);
    if sink == tuple.destination {
        return Err("expiry sink collides with Treasury".into());
    }
    let mut hash = Sha256::new();
    hash.update(b"RUMI-STABLE-TRANSFER-EXPIRY-PROBE-MEMO-V2");
    hash.update(tuple.ledger.as_slice());
    hash.update(tuple.destination.as_slice());
    hash.update(tuple.op_nonce.to_be_bytes());
    hash.update(tuple.created_at_time_ns.to_be_bytes());
    hash.update(entropy);
    let memo = hash.finalize().to_vec();
    if memo.as_slice() == tuple.memo {
        return Err("expiry probe memo collides with transfer memo".into());
    }
    Ok(StableTransferExpiryProbeArgs {
        subaccount,
        transfer: TransferArg {
            from_subaccount: Some(subaccount),
            to: Account {
                owner: sink,
                subaccount: None,
            },
            fee: None,
            created_at_time: Some(tuple.created_at_time_ns),
            memo: Some(Memo::from(memo)),
            amount: Nat::from(1u8),
        },
    })
}

fn stable_transfer_expiry_was_proven(
    result: &Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError>,
) -> bool {
    matches!(
        result,
        Err(icrc_ledger_types::icrc1::transfer::TransferError::TooOld)
    )
}

async fn probe_stable_transfer_expiry(tuple: &crate::state::BorrowMintTuple) -> Result<(), String> {
    use icrc_ledger_types::icrc1::account::Account;
    use icrc_ledger_types::icrc1::transfer::TransferError;

    // The pinned ICRC ledger checks a supplied fee before transaction age.
    // Use fee=None so its current fee is accepted. A fresh one-unit transfer
    // from a private, empty subaccount cannot debit anything, even at zero fee;
    // only a typed TooOld response may establish that the original timestamp
    // has aged out. A separate exact-history scan still proves absence.
    let entropy: [u8; 32] = ic_cdk::api::management_canister::main::raw_rand()
        .await
        .map_err(|(code, error)| format!("expiry probe entropy unavailable ({code:?}): {error}"))?
        .0
        .try_into()
        .map_err(|_| "expiry probe entropy had an invalid length".to_string())?;
    let args = stable_transfer_expiry_probe_args(tuple, entropy)?;
    let source = Account {
        owner: ic_cdk::id(),
        subaccount: Some(args.subaccount),
    };
    let balance: Result<(Nat,), _> =
        ic_cdk::call(tuple.ledger, "icrc1_balance_of", (source,)).await;
    let (balance,) = balance.map_err(|(code, error)| {
        format!("expiry probe source balance unavailable ({code:?}): {error}")
    })?;
    if balance != Nat::from(0u8) {
        return Err("expiry probe source subaccount was not empty; held".into());
    }
    let minting: Result<(Option<Account>,), _> =
        ic_cdk::call(tuple.ledger, "icrc1_minting_account", ()).await;
    let (minting,) = minting.map_err(|(code, error)| {
        format!("expiry probe minting account unavailable ({code:?}): {error}")
    })?;
    if minting.as_ref() == Some(&source) || minting.as_ref() == Some(&args.transfer.to) {
        return Err("expiry probe account collides with ledger minting account; held".into());
    }
    let outcome: Result<(Result<Nat, TransferError>,), _> =
        ic_cdk::call(tuple.ledger, "icrc1_transfer", (args.transfer,)).await;
    match outcome {
        Ok((result,)) if stable_transfer_expiry_was_proven(&result) => Ok(()),
        Ok((Ok(_),)) => Err("expiry probe unexpectedly succeeded; held".into()),
        Ok((Err(error),)) => Err(format!("expiry probe was not typed TooOld: {error:?}")),
        Err((code, error)) => Err(format!("expiry probe call rejected ({code:?}): {error}")),
    }
}

#[cfg(test)]
mod stable_transfer_expiry_probe_tests {
    use super::{stable_transfer_expiry_probe_args, stable_transfer_expiry_was_proven};
    use crate::state::BorrowMintTuple;
    use candid::{Nat, Principal};
    use icrc_ledger_types::icrc1::transfer::TransferError;

    fn tuple() -> BorrowMintTuple {
        BorrowMintTuple {
            ledger: Principal::from_slice(&[1]),
            destination: Principal::from_slice(&[2]),
            amount_e8s: 123_456,
            memo: [0xA5; 16],
            created_at_time_ns: 42,
            op_nonce: 7,
        }
    }

    #[test]
    fn probe_uses_randomized_empty_source_and_distinct_destination_and_memo() {
        let tuple = tuple();
        let entropy = [0x5A; 32];
        let args = stable_transfer_expiry_probe_args(&tuple, entropy).unwrap();

        assert_eq!(args.subaccount.len(), 32);
        assert_eq!(args.transfer.from_subaccount, Some(args.subaccount));
        assert_eq!(args.transfer.amount, Nat::from(1u8));
        assert_eq!(args.transfer.fee, None);
        assert_eq!(
            args.transfer.created_at_time,
            Some(tuple.created_at_time_ns)
        );
        assert_ne!(args.transfer.to.owner, tuple.destination);
        let probe_memo = args.transfer.memo.as_ref().unwrap();
        assert_ne!(probe_memo.0.as_ref(), tuple.memo.as_slice());

        let other_entropy = [0x5B; 32];
        let other_args = stable_transfer_expiry_probe_args(&tuple, other_entropy).unwrap();
        assert_ne!(args.subaccount, other_args.subaccount);
    }

    #[test]
    fn only_typed_too_old_proves_expiry() {
        assert!(stable_transfer_expiry_was_proven(&Err(
            TransferError::TooOld
        )));
        assert!(!stable_transfer_expiry_was_proven(&Ok(Nat::from(1u8))));
        assert!(!stable_transfer_expiry_was_proven(&Err(
            TransferError::BadFee {
                expected_fee: Nat::from(10u8),
            }
        )));
        assert!(!stable_transfer_expiry_was_proven(&Err(
            TransferError::InsufficientFunds {
                balance: Nat::from(0u8)
            }
        )));
    }
}

pub async fn process_treasury_stable_transfer(source: crate::state::TreasuryStableTransferSource) {
    let Some(_guard) = TreasuryStableTransferGuard::try_new(&source) else { return; };
    let Some(mut operation) = crate::state::read_state(|s| s.pending_treasury_stable_transfers.get(&source).cloned()) else { return; };
    if matches!(operation.phase, crate::state::TreasuryStableTransferPhase::ReceiptRecoveryRequired) {
        advance_stable_transfer_receipt_scan(&operation).await;
        return;
    }
    if operation.treasury.is_none() { return; }
    let config_matches = crate::state::read_state(|s| {
        let configured_ledger = match &operation.asset_type {
            AssetType::CKUSDT => s.ckusdt_ledger_principal,
            AssetType::CKUSDC => s.ckusdc_ledger_principal,
            _ => None,
        };
        s.treasury_principal == operation.treasury && configured_ledger == Some(operation.ledger)
    });
    if !config_matches {
        if !matches!(operation.phase, crate::state::TreasuryStableTransferPhase::ConfigurationHold |
            crate::state::TreasuryStableTransferPhase::ConfigurationReviewRequired { .. }) {
            let old = operation.clone();
            operation.phase = crate::state::TreasuryStableTransferPhase::ConfigurationHold;
            persist_stable_transfer(&old, &operation);
        }
        return;
    }
    if operation.phase == crate::state::TreasuryStableTransferPhase::AwaitingPreflight {
        let result: Result<(Nat,), _> = ic_cdk::call(operation.ledger, "icrc1_fee", ()).await;
        let Ok((fee_nat,)) = result else { return; };
        let Some(fee) = fee_nat.0.to_u64() else { return; };
        let Ok(history_floor) = crate::icrc3_proof::icrc3_history_floor(operation.ledger).await else { return; };
        let previous = operation.clone();
        operation.fee_e6s = Some(fee);
        operation.history_floor = Some(history_floor);
        operation.phase = crate::state::TreasuryStableTransferPhase::TransferPending;
        if !persist_stable_transfer(&previous, &operation) { return; }
    }
    if let crate::state::TreasuryStableTransferPhase::NotifyPending { block_index } = operation.phase {
        let _ = notify_external_stable_transfer(&operation, block_index).await;
        return;
    }
    if operation.phase != crate::state::TreasuryStableTransferPhase::TransferPending { return; }
    if ic_cdk::api::time().saturating_sub(operation.created_at_time_ns)
        >= TREASURY_STABLE_AUTO_RETRY_MAX_AGE_NS {
        let previous = operation.clone();
        operation.phase = crate::state::TreasuryStableTransferPhase::ReceiptRecoveryRequired;
        if persist_stable_transfer(&previous, &operation) {
            // The timer will pick up the persisted held row on its next fair
            // pass. Do not use the stale pre-transition local snapshot here.
        }
        return;
    }
    let Some(fee) = operation.fee_e6s else { return; };
    match management::transfer_collateral_with_nonce_and_fee_status(
        operation.amount_e6s, Some(fee), operation.treasury.expect("preflight requires Treasury"), operation.ledger, operation.op_nonce,
    ).await {
        Ok(block_index) => {
            let previous = operation.clone();
            operation.phase = crate::state::TreasuryStableTransferPhase::NotifyPending { block_index };
            if persist_stable_transfer(&previous, &operation) {
                let _ = notify_external_stable_transfer(&operation, block_index).await;
            }
        }
        Err(management::DurableTransferError::LedgerNoEffect(
            icrc_ledger_types::icrc1::transfer::TransferError::TooOld,
        )) => {
            let previous = operation.clone();
            operation.phase = crate::state::TreasuryStableTransferPhase::ReceiptRecoveryRequired;
            persist_stable_transfer(&previous, &operation);
        }
        Err(management::DurableTransferError::LedgerNoEffect(
            icrc_ledger_types::icrc1::transfer::TransferError::BadFee { .. },
        )) => {
            // Explicit fee was rejected with typed no-effect. Do not mutate
            // the frozen tuple; require operator resolution.
            let previous = operation.clone();
            operation.phase = crate::state::TreasuryStableTransferPhase::ReceiptRecoveryRequired;
            persist_stable_transfer(&previous, &operation);
        }
        Err(_) => {} // exact tuple remains retryable; ambiguous replies never mint a new nonce
    }
}

/// Bounded recovery of pending stablecoin transfers and notification receipts.
pub async fn flush_pending_treasury_stable_transfers() {
    const BATCH: usize = 8;
    let sources = crate::state::mutate_state(|s| {
        use std::ops::Bound::{Excluded, Unbounded};
        let mut selected: Vec<_> = match &s.treasury_stable_transfer_retry_cursor {
            Some(cursor) => s.pending_treasury_stable_transfers
                .range((Excluded(cursor.clone()), Unbounded)).take(BATCH)
                .map(|(source, _)| source.clone()).collect(),
            None => s.pending_treasury_stable_transfers.iter().take(BATCH)
                .map(|(source, _)| source.clone()).collect(),
        };
        if selected.len() < BATCH {
            let remaining = BATCH - selected.len();
            if let Some(cursor) = &s.treasury_stable_transfer_retry_cursor {
                selected.extend(s.pending_treasury_stable_transfers.range(..=cursor.clone())
                    .take(remaining).map(|(source, _)| source.clone()));
            }
        }
        if let Some(last) = selected.last() { s.treasury_stable_transfer_retry_cursor = Some(last.clone()); }
        selected
    });
    for source in sources { process_treasury_stable_transfer(source).await; }
}

/// Notify Treasury through the versioned, idempotent borrowing-fee API.
/// This uses a separate method name so an old Treasury can only reject the
/// call as method-not-found instead of processing a retry through its legacy
/// non-idempotent `deposit` entry point.
pub async fn notify_treasury_borrowing_fee_once(
    treasury: Principal,
    amount: u64,
    block_index: u64,
) -> Result<u64, String> {
    let args = borrowing_fee_once_args(amount, block_index);
    let result: Result<(Result<u64, String>,), _> =
        ic_cdk::call(treasury, BORROWING_FEE_ONCE_METHOD, (args,)).await;
    match result {
        Ok((Ok(deposit_id),)) => {
            log!(INFO, "[treasury] Borrowing-fee deposit recorded: id={}", deposit_id);
            Ok(deposit_id)
        }
        Ok((Err(error),)) => {
            log!(INFO, "[treasury] Borrowing-fee deposit rejected: {}", error);
            Err(error)
        }
        Err((code, message)) => {
            log!(INFO, "[treasury] Borrowing-fee deposit call failed: {:?} {}", code, message);
            Err(message)
        }
    }
}

fn borrowing_fee_once_args(amount: u64, block_index: u64) -> DepositArgs {
    DepositArgs {
        deposit_type: DepositType::BorrowingFee,
        asset_type: AssetType::ICUSD,
        amount,
        block_index,
        memo: None,
    }
}

/// An old Treasury cannot accidentally accept a replay through legacy
/// `deposit`: the versioned method must exist before this receipt is cleared.
async fn notify_treasury_interest_revenue_once(
    treasury: Principal,
    amount: u64,
    block_index: u64,
) -> Result<u64, String> {
    let args = DepositArgs {
        deposit_type: DepositType::InterestRevenue,
        asset_type: AssetType::ICUSD,
        amount,
        block_index,
        memo: None,
    };
    let result: Result<(Result<u64, String>,), _> =
        ic_cdk::call(treasury, INTEREST_REVENUE_ONCE_METHOD, (args,)).await;
    match result {
        Ok((Ok(id),)) => Ok(id),
        Ok((Err(error),)) => Err(error),
        Err((code, message)) => Err(format!("{code:?} {message}")),
    }
}

#[cfg(test)]
mod borrowing_fee_endpoint_tests {
    use super::{borrowing_fee_once_args, AssetType, DepositType, BORROWING_FEE_ONCE_METHOD};

    #[test]
    fn retries_use_only_the_versioned_idempotent_method_and_exact_deposit_key() {
        assert_eq!(BORROWING_FEE_ONCE_METHOD, "deposit_borrowing_fee_once");
        let args = borrowing_fee_once_args(75, 912);
        assert!(matches!(args.deposit_type, DepositType::BorrowingFee));
        assert!(matches!(args.asset_type, AssetType::ICUSD));
        assert_eq!(args.amount, 75);
        assert_eq!(args.block_index, 912);
        assert_eq!(args.memo, None);
    }
}

#[cfg(test)]
mod treasury_stable_transfer_admission_tests {
    use super::{
        admit_treasury_stable_transfer_in_state_at, persist_stable_transfer,
        rearm_stable_transfer_after_proven_absence, stable_interest_share_e6s,
        stable_transfer_with_initial_scan,
        AssetType, DepositType,
    };
    use crate::state::{self, State, TreasuryStableTransferPhase, TreasuryStableTransferSource};
    use candid::Principal;

    fn source() -> TreasuryStableTransferSource {
        TreasuryStableTransferSource {
            ledger: Principal::from_slice(&[1]),
            block_index: 77,
            lane: "stable_repay_surcharge".into(),
            slot: 0,
        }
    }

    fn configured() -> (Principal, Principal) {
        let treasury = Principal::from_slice(&[2]);
        let ledger = Principal::from_slice(&[3]);
        let mut s = State::default();
        s.treasury_principal = Some(treasury);
        s.ckusdt_ledger_principal = Some(ledger);
        state::replace_state(s);
        (treasury, ledger)
    }

    #[test]
    fn exact_source_retry_returns_same_nonce_and_payload_conflict_holds() {
        let (treasury, ledger) = configured();
        let key = source();
        let nonce = state::mutate_state(|s| admit_treasury_stable_transfer_in_state_at(
            s, key.clone(), ledger, Some(treasury), 123, AssetType::CKUSDT,
            DepositType::InterestRevenue, 99,
        )).unwrap();
        let same = state::mutate_state(|s| admit_treasury_stable_transfer_in_state_at(
            s, key.clone(), ledger, Some(treasury), 123, AssetType::CKUSDT,
            DepositType::InterestRevenue, 100,
        ));
        assert_eq!(same, Some(nonce));
        let conflict = state::mutate_state(|s| admit_treasury_stable_transfer_in_state_at(
            s, key.clone(), ledger, Some(treasury), 124, AssetType::CKUSDT,
            DepositType::InterestRevenue, 101,
        ));
        assert_eq!(conflict, None);
        assert_eq!(state::read_state(|s| s.pending_treasury_stable_transfers[&key].phase.clone()),
            TreasuryStableTransferPhase::ReceiptRecoveryRequired);
    }

    #[test]
    fn stable_interest_conversion_preserves_legacy_floor_and_sub_e6_dust() {
        assert_eq!(stable_interest_share_e6s(99), 0);
        assert_eq!(stable_interest_share_e6s(100), 1);
        assert_eq!(stable_interest_share_e6s(199), 1);
        assert_eq!(stable_interest_share_e6s(200), 2);
    }

    #[test]
    fn expiry_hold_initializes_first_scan_from_the_post_transition_cas_row() {
        let (treasury, ledger) = configured();
        let key = source();
        let nonce = state::mutate_state(|s| admit_treasury_stable_transfer_in_state_at(
            s, key.clone(), ledger, Some(treasury), 123, AssetType::CKUSDT,
            DepositType::InterestRevenue, 99,
        )).unwrap();
        let awaiting = state::read_state(|s| s.pending_treasury_stable_transfers[&key].clone());
        assert_eq!(awaiting.op_nonce, nonce);
        let mut original = awaiting.clone();
        original.fee_e6s = Some(7);
        original.history_floor = Some(100);
        original.phase = TreasuryStableTransferPhase::TransferPending;
        assert!(persist_stable_transfer(&awaiting, &original));
        let mut held = original.clone();
        held.phase = TreasuryStableTransferPhase::ReceiptRecoveryRequired;
        held.expiry_proven = true;
        assert!(persist_stable_transfer(&original, &held));

        let mut first_scan = stable_transfer_with_initial_scan(&held, 120).unwrap();
        assert!(persist_stable_transfer(&held, &first_scan));
        let scan = first_scan.recovery_scan.take().unwrap();
        assert_eq!(scan.next_block_index, 100);
        assert_eq!(scan.fixed_tip, Some(120));
        assert_eq!(scan.candidate_block_index, None);
        // The stale pre-hold snapshot must never be allowed to initialize or
        // overwrite this recovery cursor.
        assert!(!persist_stable_transfer(&original, &held));
    }

    #[test]
    fn absence_reissue_requires_complete_clean_scan_and_changes_only_attempt_tuple() {
        let (treasury, ledger) = configured();
        let key = source();
        state::mutate_state(|s| admit_treasury_stable_transfer_in_state_at(
            s, key.clone(), ledger, Some(treasury), 123, AssetType::CKUSDT,
            DepositType::InterestRevenue, 99,
        ));
        let mut held = state::read_state(|s| s.pending_treasury_stable_transfers[&key].clone());
        held.phase = TreasuryStableTransferPhase::ReceiptRecoveryRequired;
        held.fee_e6s = Some(7);
        held.history_floor = Some(100);
        held.expiry_proven = true;
        held.recovery_scan = Some(crate::state::BorrowMintAbsenceScan {
            next_block_index: 120, fixed_tip: Some(120), candidate_block_index: None,
        });
        assert!(persist_stable_transfer(
            &state::read_state(|s| s.pending_treasury_stable_transfers[&key].clone()), &held,
        ));
        let original = held.clone();

        // Incomplete and candidate-bearing scans cannot authorize reissue.
        let incomplete = state::mutate_state(|s| {
            let mut partial = original.clone();
            partial.recovery_scan.as_mut().unwrap().next_block_index = 119;
            s.pending_treasury_stable_transfers.insert(key.clone(), partial.clone());
            rearm_stable_transfer_after_proven_absence(s, &key, &partial, 500)
        });
        assert!(incomplete.is_err());
        let candidate = state::mutate_state(|s| {
            let mut found = original.clone();
            found.recovery_scan.as_mut().unwrap().candidate_block_index = Some(110);
            s.pending_treasury_stable_transfers.insert(key.clone(), found.clone());
            rearm_stable_transfer_after_proven_absence(s, &key, &found, 500)
        });
        assert!(candidate.is_err());

        state::mutate_state(|s| {
            s.pending_treasury_stable_transfers.insert(key.clone(), original.clone());
            rearm_stable_transfer_after_proven_absence(s, &key, &original, 500).unwrap();
        });
        let rearmed = state::read_state(|s| s.pending_treasury_stable_transfers[&key].clone());
        assert_eq!(rearmed.phase, TreasuryStableTransferPhase::AwaitingPreflight);
        assert_eq!(rearmed.source, original.source);
        assert_eq!(rearmed.ledger, original.ledger);
        assert_eq!(rearmed.treasury, original.treasury);
        assert_eq!(rearmed.asset_type, original.asset_type);
        assert_eq!(rearmed.deposit_type, original.deposit_type);
        assert_eq!(rearmed.amount_e6s, original.amount_e6s);
        assert_ne!(rearmed.op_nonce, original.op_nonce);
        assert_ne!(rearmed.memo, original.memo);
        assert_ne!(rearmed.created_at_time_ns, original.created_at_time_ns);
        assert_eq!(rearmed.fee_e6s, None);
        assert_eq!(rearmed.history_floor, None);
        assert_eq!(rearmed.recovery_scan, None);
        assert!(!rearmed.expiry_proven);
    }
}

// ---------------------------------------------------------------------------
// Public helpers — mint/transfer + notify
// ---------------------------------------------------------------------------

/// Reserve the precise ledger mint and Treasury destination before the first
/// await. `Err` means no row was admitted and the caller may restore its
/// aggregate bucket. Once admitted, the row owns the obligation exclusively.
fn start_treasury_interest_mint_at(amount: ICUSD, now: u64) -> Result<u128, ICUSD> {
    crate::state::mutate_state(|state| {
        let Some(treasury) = state.treasury_principal else { return Err(amount); };
        start_interest_mint_in_state(
            state,
            amount,
            treasury,
            crate::state::TreasuryInterestMintRoute::Treasury,
            now,
        )
    })
}

fn start_routed_interest_mint_at(
    amount: ICUSD,
    destination: Principal,
    route: crate::state::TreasuryInterestMintRoute,
    now: u64,
) -> Result<u128, ICUSD> {
    crate::state::mutate_state(|state| {
        start_interest_mint_in_state(state, amount, destination, route, now)
    })
}

fn start_interest_mint_in_state(
    state: &mut crate::state::State,
    amount: ICUSD,
    destination: Principal,
    route: crate::state::TreasuryInterestMintRoute,
    now: u64,
) -> Result<u128, ICUSD> {
    if state.pending_treasury_interest_mints.len() >= MAX_PENDING_TREASURY_INTEREST_MINTS {
        return Err(amount);
    }
    let nonce = state.next_op_nonce_at(now);
    if state.pending_treasury_interest_mints.contains_key(&nonce) {
        return Err(amount);
    }
    let operation = crate::state::TreasuryInterestMintJournal {
        tuple: crate::state::BorrowMintTuple {
            ledger: state.icusd_ledger_principal,
            destination,
            amount_e8s: amount.to_u64(),
            memo: nonce.to_be_bytes(),
            created_at_time_ns: management::nonce_to_created_at_time(nonce),
            op_nonce: nonce,
        },
        treasury: destination,
        route,
        phase: crate::state::TreasuryInterestMintPhase::AwaitingHistoryFloor,
        history_floor: None,
        expiry_proven: false,
        absence_scan: None,
    };
    crate::storage::record_pending_payout_event(
        &crate::event::PendingPayoutEvent::TreasuryInterestMintStarted {
            operation: operation.clone(),
        },
    );
    state.pending_treasury_interest_mints.insert(nonce, operation);
    Ok(nonce)
}

fn start_stability_pool_interest_mint_at(
    amount: ICUSD,
    collateral_type: Principal,
    destination: Principal,
    now: u64,
) -> Result<u128, ICUSD> {
    crate::state::mutate_state(|state| {
        start_interest_mint_in_state(
            state,
            amount,
            destination,
            crate::state::TreasuryInterestMintRoute::StabilityPool { collateral_type },
            now,
        )
    })
}

/// Atomically move a bounded source aggregate into an exact SP mint journal.
/// One replay event carries both effects so a trap cannot leave the row and
/// source aggregate simultaneously live (which would duplicate the liability).
fn admit_stability_pool_interest_fallback_at(
    collateral_type: Principal,
    amount: ICUSD,
    destination: Principal,
    now: u64,
) -> Result<u128, ICUSD> {
    crate::state::mutate_state(|state| {
        let Some(current) = state
            .pending_stability_pool_interest_fallback_e8s
            .get(&collateral_type)
            .copied()
        else {
            return Err(amount);
        };
        if current < u128::from(amount.to_u64())
            || state.pending_treasury_interest_mints.len() >= MAX_PENDING_TREASURY_INTEREST_MINTS
        {
            return Err(amount);
        }
        let nonce = state.next_op_nonce_at(now);
        if state.pending_treasury_interest_mints.contains_key(&nonce) {
            return Err(amount);
        }
        let operation = crate::state::TreasuryInterestMintJournal {
            tuple: crate::state::BorrowMintTuple {
                ledger: state.icusd_ledger_principal,
                destination,
                amount_e8s: amount.to_u64(),
                memo: nonce.to_be_bytes(),
                created_at_time_ns: management::nonce_to_created_at_time(nonce),
                op_nonce: nonce,
            },
            treasury: destination,
            route: crate::state::TreasuryInterestMintRoute::StabilityPool {
                collateral_type,
            },
            phase: crate::state::TreasuryInterestMintPhase::AwaitingHistoryFloor,
            history_floor: None,
            expiry_proven: false,
            absence_scan: None,
        };
        crate::storage::record_pending_payout_event(
            &crate::event::PendingPayoutEvent::StabilityPoolInterestFallbackAdmitted {
                collateral_type,
                amount_e8s: amount.to_u64(),
                operation: operation.clone(),
            },
        );
        state
            .pending_treasury_interest_mints
            .insert(nonce, operation);
        let remaining = current - u128::from(amount.to_u64());
        if remaining == 0 {
            state
                .pending_stability_pool_interest_fallback_e8s
                .remove(&collateral_type);
        } else {
            state
                .pending_stability_pool_interest_fallback_e8s
                .insert(collateral_type, remaining);
        }
        Ok(nonce)
    })
}

fn queue_treasury_interest_fallback(amount_e8s: u64) {
    if amount_e8s == 0 { return; }
    crate::state::mutate_state(|s| {
        let next = s.pending_treasury_interest_fallback_e8s
            .checked_add(u128::from(amount_e8s))
            .expect("Treasury fallback aggregate overflow");
        crate::storage::record_pending_payout_event(
            &crate::event::PendingPayoutEvent::TreasuryInterestFallbackCredited { amount_e8s },
        );
        s.pending_treasury_interest_fallback_e8s = next;
    });
}

fn queue_stability_pool_interest_fallback(collateral_type: Principal, amount_e8s: u64) {
    if amount_e8s == 0 {
        return;
    }
    crate::state::mutate_state(|s| {
        let next = s
            .pending_stability_pool_interest_fallback_e8s
            .get(&collateral_type)
            .copied()
            .unwrap_or_default()
            .checked_add(u128::from(amount_e8s))
            .expect("Stability Pool interest fallback aggregate overflow");
        crate::storage::record_pending_payout_event(
            &crate::event::PendingPayoutEvent::StabilityPoolInterestFallbackCredited {
                collateral_type,
                amount_e8s,
            },
        );
        s.pending_stability_pool_interest_fallback_e8s
            .insert(collateral_type, next);
    });
}

#[derive(Debug)]
enum TreasuryInterestMintFailure {
    Retry(icrc_ledger_types::icrc1::transfer::TransferError),
    Hold,
}

async fn process_treasury_interest_mint(op_nonce: u128) {
    let Some(operation) = crate::state::read_state(|s| {
        s.pending_treasury_interest_mints.get(&op_nonce).cloned()
    }) else { return; };
    if operation.phase == crate::state::TreasuryInterestMintPhase::AwaitingHistoryFloor {
        if operation.tuple.ledger
            != crate::state::read_state(|s| s.icusd_ledger_principal)
        {
            log!(INFO, "[treasury] Interest mint {op_nonce} is held: configured ledger differs from its pinned tuple");
            return;
        }
        let floor = match crate::icrc3_proof::icrc3_history_floor(operation.tuple.ledger).await {
            Ok(floor) => floor,
            Err(error) => {
                log!(INFO, "[treasury] Could not capture pre-mint ICRC-3 history floor for {op_nonce}: {error}");
                return;
            }
        };
        let armed = crate::state::mutate_state(|s| {
            if s.icusd_ledger_principal != operation.tuple.ledger
                || s.pending_treasury_interest_mints.get(&op_nonce) != Some(&operation)
            {
                return false;
            }
            let mut updated = operation.clone();
            updated.history_floor = Some(floor);
            updated.phase = crate::state::TreasuryInterestMintPhase::MintPending;
            crate::storage::record_pending_payout_event(
                &crate::event::PendingPayoutEvent::TreasuryInterestMintUpdated {
                    operation: updated.clone(),
                },
            );
            s.pending_treasury_interest_mints.insert(op_nonce, updated);
            true
        });
        if !armed {
            log!(INFO, "[treasury] Interest journal changed while capturing pre-mint history floor for {op_nonce}; no mint sent");
            return;
        }
    }
    process_treasury_interest_mint_with_now(
        op_nonce,
        ic_cdk::api::time(),
        |operation| async move {
            management::mint_icusd_with_exact_tuple(&operation.tuple)
                .await
                .map_err(|error| match error {
                    icrc_ledger_types::icrc1::transfer::TransferError::TooOld =>
                        TreasuryInterestMintFailure::Hold,
                    other => TreasuryInterestMintFailure::Retry(other),
                })
        },
        |operation, block_index| async move {
            match operation.route {
                crate::state::TreasuryInterestMintRoute::Treasury => {
                    notify_treasury_interest_revenue_once(
                        operation.treasury,
                        operation.tuple.amount_e8s,
                        block_index,
                    ).await.map(|_| ())
                }
                crate::state::TreasuryInterestMintRoute::StabilityPool { collateral_type } => {
                    let notification = crate::state::PendingStabilityPoolInterestNotification {
                        pool_principal: operation.treasury,
                        token_ledger: operation.tuple.ledger,
                        amount_e8s: operation.tuple.amount_e8s,
                        collateral_type,
                        source_mint_block: block_index,
                        receipt_protocol_version: Some(1),
                    };
                    let acknowledged = notification_delivery_acknowledged(
                        &notification,
                        deliver_stability_pool_interest_notification(&notification).await,
                    );
                    if acknowledged {
                        Ok(())
                    } else {
                        Err("Stability Pool interest receipt was not acknowledged".into())
                    }
                }
                crate::state::TreasuryInterestMintRoute::ThreePool => {
                    // The pool atomically persists this caller/nonce/payload
                    // before ACK. Reply loss is safe because retry returns the
                    // same positive receipt without incrementing reserves twice.
                    notify_three_pool_donation_once(
                        operation.treasury,
                        operation.tuple.op_nonce,
                        block_index,
                        operation.tuple.amount_e8s,
                    )
                    .await
                }
            }
        },
    ).await;
}

async fn notify_three_pool_donation_once(
    pool_canister: Principal,
    op_nonce: u128,
    source_mint_block: u64,
    amount_e8s: u64,
) -> Result<(), String> {
    let result: Result<(Result<(), ThreePoolDonationReceiptError>,), _> = ic_cdk::call(
        pool_canister,
        "receive_donation_v2",
        (op_nonce, source_mint_block, 0u8, u128::from(amount_e8s)),
    )
    .await;
    match result {
        Ok((Ok(()),)) => {
            log!(INFO, "[treasury] 3pool acknowledged donation {} (nonce {}, block {})", amount_e8s, op_nonce, source_mint_block);
            Ok(())
        }
        Ok((Err(error),)) => Err(format!("3pool donation receipt rejected: {error:?}")),
        Err((code, message)) => Err(format!("3pool donation receipt call failed: {code:?} {message}")),
    }
}

/// Reconcile only a positive exact ICRC-3 mint receipt. The verifier is
/// injectable so the journal transition can be tested without a ledger call.
pub async fn reconcile_treasury_interest_mint_from_block(
    op_nonce: u128,
    candidate_block_index: u64,
) -> Result<(), String> {
    reconcile_treasury_interest_mint_from_block_with(
        op_nonce,
        candidate_block_index,
        ic_cdk::api::caller(),
        |ledger, block, tuple| async move {
            crate::icrc3_proof::verify_icrc3_borrow_mint_block(ledger, block, &tuple).await
        },
    ).await
}

/// Advance one bounded step of held Treasury-interest recovery. The scan can
/// only begin after the original timestamp gets a typed TooOld response and
/// a fixed post-expiry tip is durably recorded. Legacy rows without a floor
/// stay eligible for positive receipt reconciliation only.
pub async fn advance_pending_treasury_interest_mint_recovery(
    op_nonce: u128,
) -> Result<(), String> {
    advance_pending_treasury_interest_mint_recovery_with(
        op_nonce,
        ic_cdk::api::caller(),
        ic_cdk::api::time(),
        |tuple, sink, memo| async move {
            probe_treasury_interest_mint_expiry(&tuple, sink, memo).await
        },
        |ledger| async move { crate::icrc3_proof::icrc3_history_floor(ledger).await },
        |ledger, start, end| async move {
            let request = vec![icrc_ledger_types::icrc3::blocks::GetBlocksRequest {
                start: candid::Nat::from(start),
                length: candid::Nat::from(end - start),
            }];
            let (response,): (icrc_ledger_types::icrc3::blocks::GetBlocksResult,) =
                ic_cdk::call(ledger, "icrc3_get_blocks", (request,))
                    .await
                    .map_err(|(code, message)| {
                        format!("direct icUSD history page [{start}, {end}) failed ({code:?}): {message}")
                    })?;
            Ok(response)
        },
    ).await
}

async fn advance_pending_treasury_interest_mint_recovery_with<P, PF, T, TF, B, BF>(
    op_nonce: u128,
    caller: Principal,
    now: u64,
    probe: P,
    history_floor: T,
    page: B,
) -> Result<(), String>
where
    P: FnOnce(crate::state::BorrowMintTuple, Principal, Vec<u8>) -> PF,
    PF: std::future::Future<Output = Result<(), String>>,
    T: FnOnce(Principal) -> TF,
    TF: std::future::Future<Output = Result<u64, String>>,
    B: FnOnce(Principal, u64, u64) -> BF,
    BF: std::future::Future<Output = Result<icrc_ledger_types::icrc3::blocks::GetBlocksResult, String>>,
{
    let mut operation = crate::state::read_state(|s| {
        s.pending_treasury_interest_mints.get(&op_nonce).cloned()
    }).ok_or_else(|| "no active treasury-interest mint row exists".to_string())?;
    if operation.tuple.op_nonce != op_nonce {
        return Err("treasury-interest map key differs from the pinned tuple nonce".into());
    }
    validate_treasury_interest_recovery_authority(caller, &operation)?;
    if operation.phase != crate::state::TreasuryInterestMintPhase::ReceiptRecoveryRequired {
        return Err("treasury-interest mint is not awaiting receipt recovery".into());
    }
    let Some(floor) = operation.history_floor else {
        return Err("legacy treasury-interest row has no durable history floor; positive receipt recovery only".into());
    };
    if !operation.expiry_proven && operation.absence_scan.is_some() {
        return Err("treasury-interest scan exists without a persisted typed TooOld proof".into());
    }

    if !operation.expiry_proven {
        let (sink, memo) = treasury_interest_expiry_probe_identity(&operation)?;
        probe(operation.tuple.clone(), sink, memo).await?;

        // Persist the typed TooOld before the next await so a restart can
        // resume at the fixed-tip read without repeating the probe.
        validate_treasury_interest_recovery_authority(caller, &operation)?;
        let mut expired = operation.clone();
        expired.expiry_proven = true;
        if !cas_treasury_interest_mint_update(caller, &operation, &expired) {
            return Err("treasury-interest journal or developer authority changed during expiry probe".into());
        }
        operation = expired;
    }

    if operation.absence_scan.is_none() {
        let tip = history_floor(operation.tuple.ledger).await?;
        validate_treasury_interest_recovery_authority(caller, &operation)?;
        if tip < floor {
            return Err("post-expiry ledger tip precedes the durable history floor".into());
        }
        let mut pinned = operation.clone();
        pinned.absence_scan = Some(crate::state::BorrowMintAbsenceScan {
            next_block_index: floor,
            fixed_tip: Some(tip),
            candidate_block_index: None,
        });
        if !cas_treasury_interest_mint_update(caller, &operation, &pinned) {
            return Err("treasury-interest journal or developer authority changed while pinning history tip".into());
        }
        operation = pinned;
    }

    let mut scan = operation.absence_scan.clone()
        .ok_or_else(|| "treasury-interest scan was not pinned".to_string())?;
    let tip = scan.fixed_tip
        .ok_or_else(|| "treasury-interest scan has no fixed tip".to_string())?;
    if scan.next_block_index < floor || scan.next_block_index > tip {
        return Err("persisted treasury-interest cursor is outside its pinned range".into());
    }

    if scan.next_block_index < tip {
        let start = scan.next_block_index;
        let end = tip.min(start.saturating_add(64));
        let response = page(operation.tuple.ledger, start, end).await?;
        validate_treasury_interest_recovery_authority(caller, &operation)?;
        let found = crate::icrc3_proof::validate_borrow_mint_scan_page(
            start, end, tip, &response, &operation.tuple,
        ).map_err(|error| format!("icUSD history page is not complete proof: {error}"))?;
        crate::state::record_borrow_mint_scan_page(&mut scan, start, end, found)?;
        let mut advanced = operation.clone();
        advanced.absence_scan = Some(scan.clone());

        if scan.next_block_index == tip {
            if let Some(block_index) = scan.candidate_block_index {
                return commit_treasury_interest_mint_receipt(
                    caller, &operation, &advanced, block_index,
                ).then_some(()).ok_or_else(|| "treasury-interest journal changed before receipt commit".into());
            }
            return replace_proven_absent_treasury_interest_mint(
                caller, &operation, &advanced, now,
            ).then_some(()).ok_or_else(|| "treasury-interest journal changed before absence replacement".into());
        }

        if !cas_treasury_interest_mint_update(caller, &operation, &advanced) {
            return Err("treasury-interest journal or developer authority changed before scan cursor CAS".into());
        }
        return Ok(());
    }

    if let Some(block_index) = scan.candidate_block_index {
        return commit_treasury_interest_mint_receipt(caller, &operation, &operation, block_index)
            .then_some(())
            .ok_or_else(|| "treasury-interest journal changed before receipt commit".into());
    }
    replace_proven_absent_treasury_interest_mint(caller, &operation, &operation, now)
        .then_some(())
        .ok_or_else(|| "treasury-interest journal changed before absence replacement".into())
}

fn treasury_interest_expiry_probe_identity(
    operation: &crate::state::TreasuryInterestMintJournal,
) -> Result<(Principal, Vec<u8>), String> {
    if operation.treasury != operation.tuple.destination {
        return Err("treasury-interest journal destination differs from its pinned tuple".into());
    }
    let mut sink_seed = b"RUMI-TREASURY-INTEREST-EXPIRY-SINK-V1".to_vec();
    sink_seed.extend_from_slice(operation.tuple.ledger.as_slice());
    sink_seed.extend_from_slice(operation.treasury.as_slice());
    sink_seed.extend_from_slice(&operation.tuple.op_nonce.to_be_bytes());
    let sink = Principal::self_authenticating(&sink_seed);
    if sink == operation.treasury {
        return Err("could not derive a Treasury-interest expiry sink distinct from Treasury".into());
    }
    use sha2::{Digest, Sha256};
    let mut memo = Sha256::new();
    memo.update(b"RUMI-TREASURY-INTEREST-EXPIRY-PROBE-V1");
    memo.update(operation.tuple.ledger.as_slice());
    memo.update(operation.treasury.as_slice());
    memo.update(operation.tuple.op_nonce.to_be_bytes());
    memo.update(operation.tuple.created_at_time_ns.to_be_bytes());
    Ok((sink, memo.finalize().to_vec()))
}

async fn probe_treasury_interest_mint_expiry(
    tuple: &crate::state::BorrowMintTuple,
    sink: Principal,
    memo: Vec<u8>,
) -> Result<(), String> {
    if sink == tuple.destination || memo.as_slice() == tuple.memo.as_slice() {
        return Err("Treasury-interest expiry probe is not domain-separated from the pinned mint".into());
    }
    use icrc_ledger_types::icrc1::account::Account;
    use icrc_ledger_types::icrc1::transfer::{Memo, TransferArg, TransferError};
    let outcome: Result<(Result<candid::Nat, TransferError>,), _> = ic_cdk::call(
        tuple.ledger,
        "icrc1_transfer",
        (TransferArg {
            from_subaccount: None,
            to: Account { owner: sink, subaccount: None },
            fee: Some(candid::Nat::from(0u8)),
            created_at_time: Some(tuple.created_at_time_ns),
            memo: Some(Memo::from(memo)),
            amount: candid::Nat::from(0u8),
        },),
    ).await;
    classify_treasury_interest_expiry_probe_outcome(outcome.map(|(inner,)| inner))
}

fn classify_treasury_interest_expiry_probe_outcome(
    outcome: Result<
        Result<candid::Nat, icrc_ledger_types::icrc1::transfer::TransferError>,
        (ic_cdk::api::call::RejectionCode, String),
    >,
) -> Result<(), String> {
    use icrc_ledger_types::icrc1::transfer::TransferError;
    match outcome {
        Ok(Err(TransferError::TooOld)) => Ok(()),
        Ok(Ok(_)) => Err("zero-value Treasury-interest expiry probe succeeded; row remains held".into()),
        Ok(Err(TransferError::Duplicate { .. })) => Err("zero-value Treasury-interest expiry probe was Duplicate; row remains held".into()),
        Ok(Err(error)) => Err(format!("zero-value Treasury-interest expiry probe returned {error:?}; row remains held")),
        Err((code, message)) => Err(format!("zero-value Treasury-interest expiry probe call rejected ({code:?}): {message}")),
    }
}

fn cas_treasury_interest_mint_update(
    caller: Principal,
    expected: &crate::state::TreasuryInterestMintJournal,
    updated: &crate::state::TreasuryInterestMintJournal,
) -> bool {
    crate::state::mutate_state(|s| {
        if caller == Principal::anonymous()
            || caller != s.developer_principal
            || s.icusd_ledger_principal != expected.tuple.ledger
            || expected.phase != crate::state::TreasuryInterestMintPhase::ReceiptRecoveryRequired
            || expected.treasury != expected.tuple.destination
            || updated.tuple != expected.tuple
            || updated.treasury != expected.treasury
            || updated.route != expected.route
            || updated.phase != crate::state::TreasuryInterestMintPhase::ReceiptRecoveryRequired
            || s.pending_treasury_interest_mints.get(&expected.tuple.op_nonce) != Some(expected)
        {
            return false;
        }
        crate::storage::record_pending_payout_event(
            &crate::event::PendingPayoutEvent::TreasuryInterestMintUpdated {
                operation: updated.clone(),
            },
        );
        s.pending_treasury_interest_mints.insert(updated.tuple.op_nonce, updated.clone());
        true
    })
}

fn commit_treasury_interest_mint_receipt(
    caller: Principal,
    expected: &crate::state::TreasuryInterestMintJournal,
    completed: &crate::state::TreasuryInterestMintJournal,
    block_index: u64,
) -> bool {
    let Some(scan) = completed.absence_scan.as_ref() else { return false; };
    if !completed.expiry_proven
        || completed.phase != crate::state::TreasuryInterestMintPhase::ReceiptRecoveryRequired
        || scan.fixed_tip != Some(scan.next_block_index)
        || scan.candidate_block_index != Some(block_index)
        || expected.tuple != completed.tuple
        || expected.treasury != completed.treasury
        || expected.route != completed.route
        || expected.history_floor != completed.history_floor
    {
        return false;
    }
    crate::state::mutate_state(|s| {
        if caller == Principal::anonymous()
            || caller != s.developer_principal
            || s.icusd_ledger_principal != expected.tuple.ledger
            || expected.phase != crate::state::TreasuryInterestMintPhase::ReceiptRecoveryRequired
            || expected.treasury != expected.tuple.destination
            || s.pending_treasury_interest_mints.get(&expected.tuple.op_nonce) != Some(expected)
        {
            return false;
        }
        crate::storage::record_pending_payout_event(
            &crate::event::PendingPayoutEvent::TreasuryInterestMintAccepted {
                op_nonce: expected.tuple.op_nonce,
                block_index,
            },
        );
        if let Some(current) = s.pending_treasury_interest_mints.get_mut(&expected.tuple.op_nonce) {
            current.phase = crate::state::TreasuryInterestMintPhase::NotifyPending {
                mint_block_index: block_index,
            };
        }
        true
    })
}

fn replace_proven_absent_treasury_interest_mint(
    caller: Principal,
    expected: &crate::state::TreasuryInterestMintJournal,
    completed: &crate::state::TreasuryInterestMintJournal,
    now: u64,
) -> bool {
    let Some(scan) = completed.absence_scan.as_ref() else { return false; };
    if scan.fixed_tip != Some(scan.next_block_index) || scan.candidate_block_index.is_some()
        || !completed.expiry_proven || completed.phase != crate::state::TreasuryInterestMintPhase::ReceiptRecoveryRequired
        || completed.history_floor.is_none_or(|floor| floor > scan.next_block_index)
        || expected.tuple != completed.tuple
        || expected.treasury != completed.treasury
        || expected.route != completed.route
        || expected.history_floor != completed.history_floor
        || !expected.expiry_proven
    {
        return false;
    }
    crate::state::mutate_state(|s| {
        if caller == Principal::anonymous()
            || caller != s.developer_principal
            || s.icusd_ledger_principal != expected.tuple.ledger
            || s.pending_treasury_interest_mints.get(&expected.tuple.op_nonce) != Some(expected)
            || expected.phase != crate::state::TreasuryInterestMintPhase::ReceiptRecoveryRequired
            || expected.treasury != expected.tuple.destination
        {
            return false;
        }
        let nonce = s.next_op_nonce_at(now);
        if s.pending_treasury_interest_mints.contains_key(&nonce) {
            return false;
        }
        let replacement = crate::state::TreasuryInterestMintJournal {
            tuple: crate::state::BorrowMintTuple {
                ledger: expected.tuple.ledger,
                destination: expected.treasury,
                amount_e8s: expected.tuple.amount_e8s,
                memo: nonce.to_be_bytes(),
                created_at_time_ns: management::nonce_to_created_at_time(nonce),
                op_nonce: nonce,
            },
            treasury: expected.treasury,
            route: expected.route.clone(),
            phase: crate::state::TreasuryInterestMintPhase::AwaitingHistoryFloor,
            history_floor: None,
            expiry_proven: false,
            absence_scan: None,
        };
        crate::storage::record_pending_payout_event(
            &crate::event::PendingPayoutEvent::TreasuryInterestMintReplaced {
                old_nonce: expected.tuple.op_nonce,
                operation: replacement.clone(),
            },
        );
        s.pending_treasury_interest_mints.remove(&expected.tuple.op_nonce);
        s.pending_treasury_interest_mints.insert(nonce, replacement);
        true
    })
}

async fn reconcile_treasury_interest_mint_from_block_with<V, VF>(
    op_nonce: u128,
    candidate_block_index: u64,
    caller: Principal,
    verify: V,
) -> Result<(), String>
where
    V: FnOnce(Principal, u64, crate::state::BorrowMintTuple) -> VF,
    VF: std::future::Future<Output = Result<(), String>>,
{
    let operation = crate::state::read_state(|s| {
        s.pending_treasury_interest_mints.get(&op_nonce).cloned()
    }).ok_or_else(|| "no active treasury-interest mint row exists".to_string())?;
    if operation.tuple.op_nonce != op_nonce {
        return Err("treasury-interest map key differs from the pinned tuple nonce".into());
    }
    validate_treasury_interest_recovery_authority(caller, &operation)?;
    if let crate::state::TreasuryInterestMintPhase::NotifyPending { mint_block_index } = &operation.phase {
        return if *mint_block_index == candidate_block_index {
            Ok(())
        } else {
            Err("treasury-interest mint is already reconciled to a different block".into())
        };
    }
    if operation.phase != crate::state::TreasuryInterestMintPhase::ReceiptRecoveryRequired {
        return Err("treasury-interest mint is not awaiting receipt recovery".into());
    }
    let ledger = operation.tuple.ledger;
    verify(ledger, candidate_block_index, operation.tuple.clone()).await?;

    // Await may change both developer authority and the persisted operation.
    // Recheck them in the same state mutation that records the accepted receipt.
    let committed = crate::state::mutate_state(|s| {
        if caller == Principal::anonymous()
            || caller != s.developer_principal
            || s.icusd_ledger_principal != ledger
            || s.pending_treasury_interest_mints.get(&op_nonce) != Some(&operation)
            || operation.phase != crate::state::TreasuryInterestMintPhase::ReceiptRecoveryRequired
        {
            return false;
        }
        crate::storage::record_pending_payout_event(
            &crate::event::PendingPayoutEvent::TreasuryInterestMintAccepted {
                op_nonce,
                block_index: candidate_block_index,
            },
        );
        if let Some(current) = s.pending_treasury_interest_mints.get_mut(&op_nonce) {
            current.phase = crate::state::TreasuryInterestMintPhase::NotifyPending {
                mint_block_index: candidate_block_index,
            };
        }
        true
    });
    if committed {
        Ok(())
    } else {
        Err("treasury-interest journal or developer authority changed during receipt recovery".into())
    }
}

fn validate_treasury_interest_recovery_authority(
    caller: Principal,
    operation: &crate::state::TreasuryInterestMintJournal,
) -> Result<(), String> {
    if caller == Principal::anonymous()
        || caller != crate::state::read_state(|s| s.developer_principal)
    {
        return Err("only the developer principal can reconcile treasury-interest receipts".into());
    }
    let configured_ledger = crate::state::read_state(|s| s.icusd_ledger_principal);
    if operation.tuple.ledger != configured_ledger {
        return Err("configured icUSD ledger differs from the pinned operation ledger".into());
    }
    if operation.treasury != operation.tuple.destination {
        return Err("treasury-interest journal destination differs from the pinned mint tuple".into());
    }
    Ok(())
}

async fn process_treasury_interest_mint_with_now<M, MFut, N, NFut>(
    op_nonce: u128,
    now: u64,
    mint: M,
    notify: N,
)
where
    M: FnOnce(crate::state::TreasuryInterestMintJournal) -> MFut,
    MFut: std::future::Future<Output = Result<u64, TreasuryInterestMintFailure>>,
    N: FnOnce(crate::state::TreasuryInterestMintJournal, u64) -> NFut,
    NFut: std::future::Future<Output = Result<(), String>>,
{
    let Some(_guard) = TreasuryInterestMintGuard::try_new(op_nonce) else { return; };
    let Some(mut operation) = crate::state::read_state(|s| {
        s.pending_treasury_interest_mints.get(&op_nonce).cloned()
    }) else { return; };
    if operation.phase == crate::state::TreasuryInterestMintPhase::ReceiptRecoveryRequired {
        return;
    }

    if operation.phase == crate::state::TreasuryInterestMintPhase::MintPending {
        if now.saturating_sub(operation.tuple.created_at_time_ns)
            >= TREASURY_INTEREST_AUTO_RETRY_MAX_AGE_NS
        {
            hold_treasury_interest_mint(op_nonce);
            return;
        }
        match mint(operation.clone()).await {
            Ok(block_index) => {
                let advanced = crate::state::mutate_state(|s| {
                    let Some(current) = s.pending_treasury_interest_mints.get_mut(&op_nonce) else {
                        return false;
                    };
                    if current.phase != crate::state::TreasuryInterestMintPhase::MintPending {
                        return false;
                    }
                    crate::storage::record_pending_payout_event(
                        &crate::event::PendingPayoutEvent::TreasuryInterestMintAccepted {
                            op_nonce, block_index,
                        },
                    );
                    current.phase = crate::state::TreasuryInterestMintPhase::NotifyPending {
                        mint_block_index: block_index,
                    };
                    true
                });
                if !advanced { return; }
                operation.phase = crate::state::TreasuryInterestMintPhase::NotifyPending {
                    mint_block_index: block_index,
                };
            }
            Err(TreasuryInterestMintFailure::Hold) => {
                hold_treasury_interest_mint(op_nonce);
                return;
            }
            Err(TreasuryInterestMintFailure::Retry(error)) => {
                log!(INFO, "[treasury] Exact interest mint retained for retry (nonce {op_nonce}): {error:?}");
                return;
            }
        }
    }

    let crate::state::TreasuryInterestMintPhase::NotifyPending { mint_block_index } = operation.phase else {
        return;
    };
    match notify(operation, mint_block_index).await {
        Ok(()) => crate::state::mutate_state(|s| {
            if s.pending_treasury_interest_mints.get(&op_nonce).is_some_and(|row| {
                row.phase == crate::state::TreasuryInterestMintPhase::NotifyPending {
                    mint_block_index,
                }
            }) {
                crate::storage::record_pending_payout_event(
                    &crate::event::PendingPayoutEvent::TreasuryInterestMintCompleted { op_nonce },
                );
                s.pending_treasury_interest_mints.remove(&op_nonce);
            }
        }),
        Err(error) => log!(INFO, "[treasury] Interest receipt held for exact retry (nonce {op_nonce}, mint block {mint_block_index}): {error}"),
    }
}

fn hold_treasury_interest_mint(op_nonce: u128) {
    crate::state::mutate_state(|s| {
        if let Some(row) = s.pending_treasury_interest_mints.get_mut(&op_nonce) {
            if row.phase == crate::state::TreasuryInterestMintPhase::MintPending {
                crate::storage::record_pending_payout_event(
                    &crate::event::PendingPayoutEvent::TreasuryInterestMintHeld { op_nonce },
                );
                row.phase = crate::state::TreasuryInterestMintPhase::ReceiptRecoveryRequired;
            }
        }
    });
}

/// Mint icUSD interest revenue to the stability pool canister.
/// The stability pool distributes this pro-rata to depositors.
///
/// `collateral_type` identifies which collateral's vault generated this interest.
/// The pool uses it to exclude depositors who opted out of that collateral.
///
/// Returns `Ok(())` when an exact mint journal owns the share. The journal
/// retains it across ambiguous mint outcomes and retries the same tuple;
/// only missing configuration or journal-admission failure returns
/// `Err(interest_share)` for the caller's restore path.
pub async fn mint_interest_to_stability_pool(
    interest_share: ICUSD,
    collateral_type: Principal,
) -> Result<(), ICUSD> {
    if interest_share.0 == 0 {
        return Ok(());
    }
    let Some(pool_principal) = read_state(|s| s.stability_pool_canister) else {
        log!(
            INFO,
            "[treasury] WARNING: no stability pool configured; {} icUSD interest unminted",
            interest_share.to_u64()
        );
        return Err(interest_share);
    };
    let op_nonce = start_routed_interest_mint_at(
        interest_share,
        pool_principal,
        crate::state::TreasuryInterestMintRoute::StabilityPool { collateral_type },
        ic_cdk::api::time(),
    )
    .map_err(|_| interest_share)?;
    process_treasury_interest_mint(op_nonce).await;
    // Once admitted, the exact journal exclusively owns this share, including
    // ambiguous mint outcomes and notification retries.
    Ok(())
}

async fn deliver_stability_pool_interest_notification(
    notification: &crate::state::PendingStabilityPoolInterestNotification,
) -> bool {
    if !has_receipt_safe_delivery_protocol(notification) {
        log!(
            INFO,
            "[treasury] holding pre-receipt Stability Pool interest notification {} for operator reconciliation",
            notification.source_mint_block,
        );
        return false;
    }
    let key = (
        notification.pool_principal,
        notification.token_ledger,
        notification.source_mint_block,
    );
    let Some(_guard) = InterestNotificationGuard::try_new(key) else {
        // Leave the durable row untouched. The active delivery or a later
        // timer tick will resolve this receipt.
        return false;
    };
    let result: Result<(StabilityPoolInterestNotificationResult,), _> = ic_cdk::call(
        notification.pool_principal,
        "receive_interest_revenue_v2",
        (
            notification.token_ledger,
            notification.amount_e8s,
            Some(notification.collateral_type),
            notification.source_mint_block,
        ),
    )
    .await;
    match result {
        Ok((StabilityPoolInterestNotificationResult::Ok,)) => true,
        Ok((StabilityPoolInterestNotificationResult::Err(error),)) => {
            log!(
                INFO,
                "[treasury] pool interest notification {} rejected: {:?}; retained for retry",
                notification.source_mint_block,
                error
            );
            false
        }
        Err(error) => {
            log!(
                INFO,
                "[treasury] pool interest notification {} failed: {:?}; retained for retry",
                notification.source_mint_block,
                error
            );
            false
        }
    }
}

#[cfg(test)]
mod interest_notification_guard_tests {
    use super::{
        has_receipt_safe_delivery_protocol, notification_delivery_acknowledged,
        InterestNotificationGuard, StabilityPoolInterestNotificationError as PoolError,
        StabilityPoolInterestNotificationResult as PoolResult,
    };
    use candid::Principal;

    #[test]
    fn inline_and_timer_delivery_share_one_in_flight_receipt() {
        let key = (Principal::from_slice(&[1]), Principal::from_slice(&[2]), 77);
        let inline = InterestNotificationGuard::try_new(key).expect("first delivery acquires");
        assert!(InterestNotificationGuard::try_new(key).is_none());
        drop(inline);
        assert!(InterestNotificationGuard::try_new(key).is_some());
    }

    #[test]
    fn legacy_outbox_rows_remain_held_until_operator_reconciliation() {
        use std::collections::BTreeMap;

        let legacy = crate::state::PendingStabilityPoolInterestNotification {
            pool_principal: Principal::from_slice(&[1]),
            token_ledger: Principal::from_slice(&[2]),
            amount_e8s: 99,
            collateral_type: Principal::from_slice(&[3]),
            source_mint_block: 44,
            receipt_protocol_version: None,
        };
        let current = crate::state::PendingStabilityPoolInterestNotification {
            receipt_protocol_version: Some(1),
            ..legacy.clone()
        };
        assert!(!has_receipt_safe_delivery_protocol(&legacy));
        assert!(has_receipt_safe_delivery_protocol(&current));
        assert!(
            !notification_delivery_acknowledged(&legacy, true),
            "a legacy acknowledgement can never remove a potentially already-applied row",
        );
        let mut pending = BTreeMap::from([(legacy.source_mint_block, legacy.clone())]);
        if notification_delivery_acknowledged(&legacy, false) {
            pending.remove(&legacy.source_mint_block);
        }
        assert!(pending.contains_key(&legacy.source_mint_block), "upgrade leaves the legacy outbox row held");
        assert_eq!(legacy.source_mint_block, 44, "held row retains its source receipt for inspection");
    }

    #[test]
    fn notification_decoder_accepts_real_typed_pool_error_arms() {
        for error in [
            PoolError::EmergencyPaused,
            PoolError::LedgerTransferFailed {
                reason: "ledger unavailable".to_string(),
            },
        ] {
            let bytes = candid::encode_args((PoolResult::Err(error.clone()),)).unwrap();
            let decoded: (PoolResult,) = candid::decode_args(&bytes)
                .expect("mirrored result accepts the Stability Pool's typed Err payload");
            assert!(matches!(decoded.0, PoolResult::Err(decoded_error) if decoded_error == error));
        }
    }
}

/// Retry post-mint notifications. The SP's v2 receipt key makes this safe if
/// a prior call committed but its response was lost.
pub async fn flush_pending_stability_pool_interest_notifications() {
    let pending: Vec<crate::state::PendingStabilityPoolInterestNotification> = read_state(|s| {
        s.pending_stability_pool_interest_notifications
            .values()
            .cloned()
            .collect()
    });
    for notification in pending {
        if notification_delivery_acknowledged(
            &notification,
            deliver_stability_pool_interest_notification(&notification).await,
        ) {
            crate::state::mutate_state(|s| {
                s.pending_stability_pool_interest_notifications
                    .remove(&notification.source_mint_block);
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Interest distribution — N-way split
// ---------------------------------------------------------------------------

/// Distribute interest revenue according to the configured interest_split.
/// Mints icUSD to each destination: stability pool, treasury, and/or 3pool.
///
/// For the 3pool destination, mints icUSD to self, approves the 3pool canister,
/// then calls `donate(0, amount)` to inject yield into the pool.
///
/// `collateral_type` is needed for stability pool interest routing.
///
/// Returns the total icUSD that failed to mint across all recipients.
/// The caller (e.g. `flush_pending_interest`) re-queues this via the
/// snapshot-then-decrement restore path so the failed share is replayed
/// on the next tick rather than silently lost.
pub async fn distribute_interest(interest: ICUSD, collateral_type: Principal) -> ICUSD {
    distribute_interest_at(interest, collateral_type, ic_cdk::api::time()).await
}

async fn distribute_interest_at(interest: ICUSD, collateral_type: Principal, now: u64) -> ICUSD {
    if interest.0 == 0 {
        return ICUSD::new(0);
    }

    let (split, three_pool) = read_state(|s| (s.interest_split.clone(), s.three_pool_canister));

    let total_e8s = interest.to_u64();

    // Reserve every asynchronous side effect before the first await. Each
    // journal owns its exact tuple even if an earlier recipient's call traps
    // this callback or the canister upgrades while a call is outstanding.
    let (interest_jobs, amm1_jobs, amm1_missing) = reserve_interest_distribution_jobs(
        total_e8s,
        collateral_type,
        &split,
        three_pool,
        now,
    );
    for (index, recipient) in split.iter().enumerate() {
        let share_e8s = ((total_e8s as u128) * (recipient.bps as u128) / 10_000) as u64;
        if share_e8s == 0 {
            continue;
        }
        if matches!(
            recipient.destination,
            crate::state::InterestDestination::Amm1
        ) && !amm1_missing {
            match amm1_jobs.get(&index).copied().flatten() {
                Some(notify_nonce) => process_amm1_donation(notify_nonce).await,
                None => {
                    // Reservation failure is held or restored synchronously.
                    // Never allocate a replacement identity after an await.
                }
            }
            continue;
        }
        if let Some(Some(op_nonce)) = interest_jobs.get(&index) {
            process_treasury_interest_mint(*op_nonce).await;
        }
    }
    ICUSD::new(0)
}

/// Reserve every mint and AMM1 donation for a distribution synchronously.
/// `None` in the AMM1 map means the share was held or restored without an
/// external call; it must not be assigned a new nonce later in the flow.
fn reserve_interest_distribution_jobs(
    total_e8s: u64,
    collateral_type: Principal,
    split: &[crate::state::InterestRecipient],
    three_pool: Option<Principal>,
    now: u64,
) -> (
    BTreeMap<usize, Option<u128>>,
    BTreeMap<usize, Option<u64>>,
    bool,
) {
    // Each row owns its exact ledger tuple even if a call reply is lost or
    // this canister upgrades while the call is outstanding.
    let mut interest_jobs: BTreeMap<usize, Option<u128>> = BTreeMap::new();
    let mut amm1_jobs: BTreeMap<usize, Option<u64>> = BTreeMap::new();
    let amm1_missing = read_state(|s| s.amm1_canister.is_none());
    for (index, recipient) in split.iter().enumerate() {
        let (journaled, target) = match &recipient.destination {
            crate::state::InterestDestination::Treasury => read_state(|s| {
                (true, s.treasury_principal.map(|principal| (
                    principal, crate::state::TreasuryInterestMintRoute::Treasury,
                )))
            }),
            crate::state::InterestDestination::StabilityPool => read_state(|s| {
                (true, s.stability_pool_canister.map(|principal| (
                    principal, crate::state::TreasuryInterestMintRoute::StabilityPool { collateral_type },
                )))
            }),
            crate::state::InterestDestination::ThreePool => (
                true,
                three_pool.map(|principal| (
                    principal, crate::state::TreasuryInterestMintRoute::ThreePool,
                )).or_else(|| read_state(|s| s.treasury_principal.map(|principal| (
                    principal, crate::state::TreasuryInterestMintRoute::Treasury,
                )))),
            ),
            crate::state::InterestDestination::Amm1 if amm1_missing => read_state(|s| {
                (true, s.treasury_principal.map(|principal| (
                    principal, crate::state::TreasuryInterestMintRoute::Treasury,
                )))
            }),
            crate::state::InterestDestination::Amm1 => (true, None),
        };
        if !journaled {
            continue;
        }
        let share_e8s = ((total_e8s as u128) * (recipient.bps as u128) / 10_000) as u64;
        if share_e8s == 0 {
            continue;
        }
        if matches!(
            recipient.destination,
            crate::state::InterestDestination::Amm1
        ) && !amm1_missing {
            let notify_nonce = match start_amm1_donation_at(share_e8s, now) {
                Ok(Some(notify_nonce)) => Some(notify_nonce),
                Ok(None) => {
                    // Configuration changed since the route snapshot. There
                    // is no durable AMM1 row, so preserve only this share.
                    crate::state::mutate_state(|state| {
                        state.restore_pending_interest_for_pool(collateral_type, share_e8s)
                    });
                    None
                }
                Err(()) => None, // The existing held-donation row owns it.
            };
            amm1_jobs.insert(index, notify_nonce);
            continue;
        }
        let nonce = target.and_then(|(destination, route)| {
            start_routed_interest_mint_at(
                ICUSD::from(share_e8s),
                destination,
                route,
                now,
            )
            .ok()
        });
        if nonce.is_none() {
            // No exact row means no ledger call can be made. Return the share
            // to the accumulator before any await can trap this callback.
            crate::state::mutate_state(|state| {
                state.restore_pending_interest_for_pool(collateral_type, share_e8s)
            });
        }
        interest_jobs.insert(index, nonce);
    }
    (interest_jobs, amm1_jobs, amm1_missing)
}

/// Distribute stablecoin-denominated interest revenue.
/// Similar to `distribute_interest` but handles the stablecoin-specific case:
/// - StabilityPool: mint icUSD (backed by stablecoins in reserves)
/// - Treasury: transfer actual stablecoins
/// - ThreePool: mint icUSD + donate to pool
///
/// `interest_e8s` is in icUSD-equivalent e8s (8 decimals).
/// `token_type` and the corresponding ledger are used for treasury stablecoin transfers.
pub async fn distribute_stablecoin_interest(
    interest_e8s: u64,
    collateral_type: Principal,
    token_type: crate::StableTokenType,
    source_block_index: u64,
    source_ledger: Principal,
) {
    distribute_stablecoin_interest_at(
        interest_e8s,
        collateral_type,
        token_type,
        source_block_index,
        source_ledger,
        ic_cdk::api::time(),
    )
    .await;
}

async fn distribute_stablecoin_interest_at(
    interest_e8s: u64,
    collateral_type: Principal,
    token_type: crate::StableTokenType,
    source_block_index: u64,
    source_ledger: Principal,
    now: u64,
) {
    if interest_e8s == 0 {
        return;
    }

    let (split, three_pool, stability_pool) = read_state(|s| {
        (
            s.interest_split.clone(),
            s.three_pool_canister,
            s.stability_pool_canister,
        )
    });

    // Admit every Treasury-directed stablecoin share before the first pool,
    // AMM, or other recipient await. The e8->e6 conversion intentionally
    // floors as before; sub-e6 dust remains in the existing reserve economics.
    {
        for (slot, recipient) in split.iter().enumerate() {
            if recipient.destination == crate::state::InterestDestination::Treasury {
                let share_e8s = ((interest_e8s as u128) * (recipient.bps as u128) / 10_000) as u64;
                let amount_e6s = stable_interest_share_e6s(share_e8s);
                let asset_type = match token_type {
                    crate::StableTokenType::CKUSDT => AssetType::CKUSDT,
                    crate::StableTokenType::CKUSDC => AssetType::CKUSDC,
                };
                admit_treasury_stable_transfer(
                    crate::state::TreasuryStableTransferSource {
                        ledger: source_ledger, block_index: source_block_index, lane: "stable_interest".into(), slot: slot as u32,
                    }, source_ledger, amount_e6s, asset_type, DepositType::InterestRevenue,
                );
            }
        }
    }

    // A missing donation destination converts its share into an icUSD mint
    // for Treasury. Put that share in the durable aggregate before any await:
    // admission may be full, and a preceding pool callback can trap. Timer B
    // later admits it into the exact mint journal without losing the source.
    let amm1_missing = read_state(|s| s.amm1_canister.is_none());
    let stability_pool_jobs = reserve_stablecoin_sp_interest_shares(
        interest_e8s,
        collateral_type,
        &split,
        stability_pool,
        now,
    );
    // Like icUSD-denominated interest, a configured 3pool donation mints
    // icUSD and therefore needs a stable exact tuple before the first await.
    // Admission failure moves the share to the already-supported Treasury
    // fallback aggregate; this source has no stablecoin accumulator to restore.
    let three_pool_jobs = reserve_stablecoin_three_pool_shares(
        interest_e8s,
        &split,
        three_pool,
        now,
    );
    let amm1_jobs = reserve_stablecoin_amm1_donations(
        interest_e8s,
        &split,
        amm1_missing,
        now,
    );
    for recipient in &split {
        let treasury_fallback = match &recipient.destination {
            crate::state::InterestDestination::ThreePool => three_pool.is_none(),
            crate::state::InterestDestination::Amm1 => amm1_missing,
            _ => false,
        };
        if treasury_fallback {
            let share_e8s = ((interest_e8s as u128) * (recipient.bps as u128) / 10_000) as u64;
            if share_e8s > 0 {
                queue_treasury_interest_fallback(share_e8s);
            }
        }
    }

    for (index, recipient) in split.iter().enumerate() {
        let share_e8s = ((interest_e8s as u128) * (recipient.bps as u128) / 10_000) as u64;
        if share_e8s == 0 {
            continue;
        }

        match &recipient.destination {
            crate::state::InterestDestination::StabilityPool => {
                if let Some(op_nonce) = stability_pool_jobs.get(&index).copied() {
                    process_treasury_interest_mint(op_nonce).await;
                } else {
                    log!(INFO, "[treasury] stablecoin Stability Pool interest retained for admission retry ({share_e8s} icUSD)");
                }
            }
            crate::state::InterestDestination::Treasury => {
                // Durable outbox admission happened before any recipient await.
                let _ = share_e8s;
            }
            crate::state::InterestDestination::ThreePool => {
                if let Some(op_nonce) = three_pool_jobs.get(&index).copied().flatten() {
                    process_treasury_interest_mint(op_nonce).await;
                } else {
                    log!(INFO, "[treasury] 3pool interest share ({} icUSD) queued for Treasury or retained for exact-journal retry", share_e8s);
                }
            }
            crate::state::InterestDestination::Amm1 => {
                if amm1_missing {
                    log!(INFO, "[treasury] AMM1 interest fallback ({} icUSD) queued for Treasury", share_e8s);
                    continue;
                }
                if let Some(notify_nonce) = amm1_jobs.get(&index).copied().flatten() {
                    process_amm1_donation(notify_nonce).await;
                } else {
                    // A missing pool ID is already represented by a durable
                    // held-donation row; never allocate a replacement here.
                    log!(INFO, "[treasury] AMM1 stablecoin interest share ({}) held or restored during reservation", share_e8s);
                }
            }
        }
    }
}

fn reserve_stablecoin_sp_interest_shares(
    interest_e8s: u64,
    collateral_type: Principal,
    split: &[crate::state::InterestRecipient],
    stability_pool: Option<Principal>,
    now: u64,
) -> BTreeMap<usize, u128> {
    let mut jobs = BTreeMap::new();
    for (index, recipient) in split.iter().enumerate() {
        if recipient.destination != crate::state::InterestDestination::StabilityPool {
            continue;
        }
        let share_e8s = ((interest_e8s as u128) * u128::from(recipient.bps) / 10_000) as u64;
        if share_e8s == 0 {
            continue;
        }
        let admitted = stability_pool.and_then(|pool| {
            start_stability_pool_interest_mint_at(
                ICUSD::from(share_e8s),
                collateral_type,
                pool,
                now,
            )
            .ok()
        });
        if let Some(nonce) = admitted {
            jobs.insert(index, nonce);
        } else {
            // Missing pool configuration or a full bounded journal leaves a
            // typed, event-backed liability for a later timer tick.
            queue_stability_pool_interest_fallback(collateral_type, share_e8s);
        }
    }
    jobs
}

/// Convert an e8 stable-interest share to the CK ledger's e6 units. Floor as
/// the legacy path did; the remainder stays in the reserve accounting.
fn stable_interest_share_e6s(share_e8s: u64) -> u64 {
    share_e8s / 100
}

fn reserve_stablecoin_three_pool_shares(
    interest_e8s: u64,
    split: &[crate::state::InterestRecipient],
    three_pool: Option<Principal>,
    now: u64,
) -> BTreeMap<usize, Option<u128>> {
    let mut jobs = BTreeMap::new();
    for (index, recipient) in split.iter().enumerate() {
        if recipient.destination != crate::state::InterestDestination::ThreePool {
            continue;
        }
        let share_e8s = ((interest_e8s as u128) * (recipient.bps as u128) / 10_000) as u64;
        if share_e8s == 0 {
            continue;
        }
        let op_nonce = three_pool.and_then(|pool_canister| {
            start_routed_interest_mint_at(
                ICUSD::from(share_e8s),
                pool_canister,
                crate::state::TreasuryInterestMintRoute::ThreePool,
                now,
            )
            .ok()
        });
        if op_nonce.is_none() && three_pool.is_some() {
            // This source has no stablecoin accumulator to restore. Credit the
            // same icUSD Treasury fallback used when ThreePool is unconfigured.
            queue_treasury_interest_fallback(share_e8s);
        }
        jobs.insert(index, op_nonce);
    }
    jobs
}

fn reserve_stablecoin_amm1_donations(
    interest_e8s: u64,
    split: &[crate::state::InterestRecipient],
    amm1_missing: bool,
    now: u64,
) -> BTreeMap<usize, Option<u64>> {
    let mut jobs = BTreeMap::new();
    for (index, recipient) in split.iter().enumerate() {
        if recipient.destination != crate::state::InterestDestination::Amm1 {
            continue;
        }
        let share_e8s = ((interest_e8s as u128) * u128::from(recipient.bps) / 10_000) as u64;
        if share_e8s == 0 || amm1_missing {
            continue;
        }
        let notify_nonce = match start_amm1_donation_at(share_e8s, now) {
            Ok(Some(notify_nonce)) => Some(notify_nonce),
            Ok(None) => {
                // No stablecoin source bucket exists for this callback. Keep
                // the already-earned share in the existing Treasury fallback.
                queue_treasury_interest_fallback(share_e8s);
                None
            }
            Err(()) => None, // The exact share is owned by the held row.
        };
        jobs.insert(index, notify_nonce);
    }
    jobs
}

/// Candid-compatible mirror of the additive ThreePool donation receipt error.
#[derive(CandidType, Deserialize, Clone, Debug)]
enum ThreePoolDonationReceiptError {
    Unauthorized,
    PayloadConflict,
    CapacityExceeded,
    InvalidCoinIndex,
    ZeroAmount,
    PoolEmpty,
    PoolPaused,
    PoolLocked,
    MathOverflow,
    TransferFailed {
        token: String,
        reason: String,
    },
}

/// Record the exact transfer and notification tuple before any await. None
/// means AMM1 is not configured; Err means AMM1 exists but the pool ID is
/// missing, so the share is held.
fn start_amm1_donation_at(amount_e8s: u64, now: u64) -> Result<Option<u64>, ()> {
    crate::state::mutate_state(|s| {
        let Some(amm_canister) = s.amm1_canister else {
            return Ok(None);
        };
        let notify_nonce = s.amm1_donation_nonce.checked_add(1)
            .expect("AMM1 donation nonce exhausted");
        s.amm1_donation_nonce = notify_nonce;
        let Some(pool_id) = s.amm1_pool_id.clone() else {
            let donation = crate::state::HeldAmm1Donation {
                amount_e8s,
                notify_nonce,
                reason: "AMM1 is configured but pool ID is missing; no mint was attempted".into(),
            };
            crate::storage::record_pending_payout_event(
                &crate::event::PendingPayoutEvent::Amm1DonationHeld { donation: donation.clone() },
            );
            s.held_amm1_donations.push(donation);
            return Err(());
        };
        let operation = crate::state::PendingAmm1Donation {
            ledger: s.icusd_ledger_principal,
            amm_canister,
            reward_subaccount: compute_amm_reward_subaccount(&pool_id),
            pool_id,
            amount_e8s,
            mint_op_nonce: s.next_op_nonce_at(now),
            notify_nonce,
            mint_block_index: None,
            phase: crate::state::Amm1DonationPhase::MintPending,
            reconciliation_reason: None,
        };
        crate::storage::record_pending_payout_event(
            &crate::event::PendingPayoutEvent::Amm1DonationStarted { operation: operation.clone() },
        );
        s.pending_amm1_donation_operations.insert(notify_nonce, operation);
        Ok(Some(notify_nonce))
    })
}

/// Advance one durable donation. Ledger Duplicate is normalized to Ok by
/// `transfer_idempotent`; that accepted outcome is journaled before notify.
async fn process_amm1_donation(notify_nonce: u64) {
    process_amm1_donation_with_now(
        notify_nonce,
        ic_cdk::api::time(),
        |operation| async move {
            use icrc_ledger_types::icrc1::account::Account;
            management::transfer_idempotent(
                operation.ledger,
                None,
                Account { owner: operation.amm_canister, subaccount: Some(operation.reward_subaccount) },
                u128::from(operation.amount_e8s),
                operation.mint_op_nonce,
                None,
            ).await.map_err(Amm1MintFailure::from)
        },
        |operation| async move {
            let result: Result<(Result<(), AmmDonateError>,), _> = ic_cdk::call(
                operation.amm_canister,
                "notify_reward_received",
                (operation.pool_id, u128::from(operation.amount_e8s), operation.notify_nonce),
            ).await;
            match result {
                Ok((Ok(()),)) => Ok(()),
                Ok((Err(error),)) => Err(format!("{error:?}")),
                Err((code, message)) => Err(format!("{code:?} {message}")),
            }
        },
    ).await;
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Amm1MintFailure {
    Retry(icrc_ledger_types::icrc1::transfer::TransferError),
    ReconciliationRequired(crate::state::Amm1DonationReconciliationReason),
}

impl From<icrc_ledger_types::icrc1::transfer::TransferError> for Amm1MintFailure {
    fn from(error: icrc_ledger_types::icrc1::transfer::TransferError) -> Self {
        match error {
            icrc_ledger_types::icrc1::transfer::TransferError::TooOld => {
                Self::ReconciliationRequired(
                    crate::state::Amm1DonationReconciliationReason::LedgerTooOld,
                )
            }
            other => Self::Retry(other),
        }
    }
}

/// Stop automatic retries before the pinned ICRC tuple may age out of ledger
/// deduplication; hold unknown outcomes for exact receipt reconciliation.
const AMM1_AUTO_MINT_RETRY_MAX_AGE_NS: u64 = 24 * 60 * 60 * 1_000_000_000;

async fn process_amm1_donation_with_now<M, MFut, N, NFut>(
    notify_nonce: u64,
    now: u64,
    mint: M,
    notify: N,
)
where
    M: FnOnce(crate::state::PendingAmm1Donation) -> MFut,
    MFut: std::future::Future<Output = Result<u64, Amm1MintFailure>>,
    N: FnOnce(crate::state::PendingAmm1Donation) -> NFut,
    NFut: std::future::Future<Output = Result<(), String>>,
{
    let Some(mut operation) = crate::state::read_state(|s| {
        s.pending_amm1_donation_operations.get(&notify_nonce).cloned()
    }) else { return; };

    if operation.phase == crate::state::Amm1DonationPhase::ReconciliationRequired {
        return;
    }

    if operation.phase == crate::state::Amm1DonationPhase::MintPending {
        let created_at = management::nonce_to_created_at_time(operation.mint_op_nonce);
        if now.saturating_sub(created_at) >= AMM1_AUTO_MINT_RETRY_MAX_AGE_NS {
            mark_amm1_donation_reconciliation_required(
                notify_nonce,
                crate::state::Amm1DonationReconciliationReason::DedupWindowElapsedUnknown,
            );
            return;
        }
        match mint(operation.clone()).await {
            Ok(block_index) => {
                let advanced = crate::state::mutate_state(|s| {
                    let Some(current) = s.pending_amm1_donation_operations.get_mut(&notify_nonce) else { return false; };
                    if current.phase != crate::state::Amm1DonationPhase::MintPending
                        || current.mint_op_nonce != operation.mint_op_nonce
                    {
                        return current.phase == crate::state::Amm1DonationPhase::NotifyPending;
                    }
                    crate::storage::record_pending_payout_event(
                        &crate::event::PendingPayoutEvent::Amm1DonationMintAccepted { notify_nonce, block_index },
                    );
                    current.mint_block_index = Some(block_index);
                    current.phase = crate::state::Amm1DonationPhase::NotifyPending;
                    current.reconciliation_reason = None;
                    true
                });
                if !advanced { return; }
                operation.phase = crate::state::Amm1DonationPhase::NotifyPending;
                operation.mint_block_index = Some(block_index);
                log!(INFO, "[treasury] AMM1 mint accepted for ({}, nonce {}) at block {}", operation.amount_e8s, notify_nonce, block_index);
            }
            Err(Amm1MintFailure::Retry(error)) => {
                log!(INFO, "[treasury] AMM1 mint attempt retained for exact retry ({}, nonce {}): {:?}", operation.amount_e8s, notify_nonce, error);
                return;
            }
            Err(Amm1MintFailure::ReconciliationRequired(reason)) => {
                mark_amm1_donation_reconciliation_required(notify_nonce, reason);
                return;
            }
        }
    }

    match notify(operation.clone()).await {
        Ok(()) => {
            crate::state::mutate_state(|s| {
                if s.pending_amm1_donation_operations.get(&notify_nonce)
                    .is_some_and(|row| row.phase == crate::state::Amm1DonationPhase::NotifyPending)
                {
                    crate::storage::record_pending_payout_event(
                        &crate::event::PendingPayoutEvent::Amm1DonationCompleted { notify_nonce },
                    );
                    s.pending_amm1_donation_operations.remove(&notify_nonce);
                }
            });
            log!(INFO, "[treasury] AMM1 acknowledged donation of {} icUSD (nonce {})", operation.amount_e8s, notify_nonce);
        }
        Err(error) => log!(INFO, "[treasury] AMM1 notification remains pending for retry ({}, nonce {}): {}", operation.amount_e8s, notify_nonce, error),
    }
}

fn mark_amm1_donation_reconciliation_required(
    notify_nonce: u64,
    reason: crate::state::Amm1DonationReconciliationReason,
) {
    crate::state::mutate_state(|s| {
        let Some(operation) = s.pending_amm1_donation_operations.get_mut(&notify_nonce) else {
            return;
        };
        if operation.phase != crate::state::Amm1DonationPhase::MintPending {
            return;
        }
        crate::storage::record_pending_payout_event(
            &crate::event::PendingPayoutEvent::Amm1DonationReconciliationRequired {
                notify_nonce,
                reason: reason.clone(),
            },
        );
        operation.phase = crate::state::Amm1DonationPhase::ReconciliationRequired;
        operation.reconciliation_reason = Some(reason);
    });
}

/// Compute the per-pool reward subaccount on AMM1.
/// Must match rumi_amm::reward_subaccount_for.
fn compute_amm_reward_subaccount(pool_id: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"rumi_amm:rewards:");
    h.update(pool_id.as_bytes());
    let digest = h.finalize();
    let mut sub = [0u8; 32];
    sub.copy_from_slice(&digest);
    sub
}

/// Mirror of the AMM's AmmError for the notify response.
/// Only used for deserialization; we treat any Err the same way (re-queue).
#[derive(CandidType, Deserialize, Clone, Debug)]
enum AmmDonateError {
    PoolBusy,
    PoolNotFound,
    Unauthorized,
    DuplicateNonce,
    NoLiquidity,
    BelowMinClaim {
        claimable: candid::Nat,
        min: candid::Nat,
    },
    RewardLedgerTransferFailed {
        reason: String,
    },
    InsufficientOnChainBalance {
        expected: candid::Nat,
        actual: candid::Nat,
    },
    PoolPaused,
    InsufficientLiquidity,
    InvalidCoinIndex,
    ZeroAmount,
    PoolEmpty,
    SlippageExceeded,
    TransferFailed {
        token: String,
        reason: String,
    },
    MathOverflow,
    InvariantNotConverged,
    InsufficientOutput {
        expected_min: candid::Nat,
        actual: candid::Nat,
    },
    InsufficientLpShares {
        required: candid::Nat,
        available: candid::Nat,
    },
    MaintenanceMode,
}

// ---------------------------------------------------------------------------
// Public helpers — mint/transfer + notify
// ---------------------------------------------------------------------------

/// Mint icUSD borrowing fee to treasury and record the deposit.
///
/// Wave-8e LIQ-005: routes a configurable fraction of the fee to deficit
/// repayment first via `plan_fee_routing`. The "repayment" is supply-
/// conserving — we mint `to_remainder` instead of the full `fee`, so the
/// skipped `to_repay` mint is the foregone-revenue that pays down the
/// deficit. No separate ledger op is required.
pub async fn mint_borrowing_fee_to_treasury(fee: ICUSD) {
    if fee.0 == 0 {
        return;
    }
    let outcome = crate::state::mutate_state(|s| {
        plan_fee_routing(s, fee, crate::event::FeeSource::BorrowingFee)
    });
    if outcome.to_remainder.0 == 0 {
        log!(
            INFO,
            "[treasury] Borrowing fee {} fully routed to deficit repayment (no treasury mint)",
            fee.to_u64()
        );
        return;
    }
    let treasury = read_state(|s| s.treasury_principal);
    if let Some(tp) = treasury {
        match management::mint_icusd(outcome.to_remainder, tp).await {
            Ok(block_index) => {
                if outcome.to_repay.0 > 0 {
                    log!(
                        INFO,
                        "[treasury] Minted {} icUSD borrowing fee (deficit repay {}, block {})",
                        outcome.to_remainder.to_u64(),
                        outcome.to_repay.to_u64(),
                        block_index
                    );
                } else {
                    log!(
                        INFO,
                        "[treasury] Minted {} icUSD borrowing fee (block {})",
                        outcome.to_remainder.to_u64(),
                        block_index
                    );
                }
                let _ = notify_treasury_deposit(
                    tp,
                    DepositType::BorrowingFee,
                    AssetType::ICUSD,
                    outcome.to_remainder.to_u64(),
                    block_index,
                )
                .await;
            }
            Err(e) => log!(
                INFO,
                "[treasury] WARNING: borrowing fee mint failed: {:?}",
                e
            ),
        }
    }
}

/// Transfer collateral (liquidation fee) to treasury and record the deposit.
pub async fn send_liquidation_fee_to_treasury(
    amount: u64,
    collateral_ledger: Principal,
    asset_type: AssetType,
) {
    if amount == 0 {
        return;
    }
    let treasury = read_state(|s| s.treasury_principal);
    if let Some(tp) = treasury {
        match management::transfer_collateral(amount, tp, collateral_ledger).await {
            Ok(block_index) => {
                log!(
                    INFO,
                    "[treasury] Sent {} collateral liquidation fee (block {})",
                    amount,
                    block_index
                );
                let _ = notify_treasury_deposit(
                    tp,
                    DepositType::LiquidationFee,
                    asset_type,
                    amount,
                    block_index,
                )
                .await;
            }
            Err(e) => log!(
                INFO,
                "[treasury] WARNING: liquidation fee transfer failed: {:?}",
                e
            ),
        }
    }
}

/// Flush accumulated interest from periodic harvesting to pools/treasury.
/// For each collateral bucket that has reached the threshold, calls
/// `distribute_interest` which handles the N-way split (3pool, stability pool, treasury).
///
/// Audit INT-002: uses the snapshot-then-decrement pattern so a mint
/// failure inside `distribute_interest` re-queues the unminted portion
/// via `restore_pending_interest_for_pool` (saturating_add). Concurrent
/// harvest credits that land during the await accumulate against zero
/// rather than against the old snapshot, so neither side is silently
/// overwritten.
pub async fn flush_pending_interest() {
    let (pending, threshold) = read_state(|s| {
        (
            s.pending_interest_for_pools.clone(),
            s.interest_flush_threshold_e8s,
        )
    });

    for (collateral_type, amount) in pending {
        if amount < threshold {
            continue;
        }
        log!(
            INFO,
            "[treasury] Flushing {} icUSD interest for collateral {}",
            amount,
            collateral_type
        );
        // Atomic snapshot+take. The actual amount taken may be larger
        // than the cloned `amount` if a concurrent harvest landed
        // between the clone and this mutate; we mint what is currently
        // in the bucket regardless.
        let snapshot_e8s =
            crate::state::mutate_state(|s| s.take_pending_interest_for_pool(collateral_type));
        if snapshot_e8s == 0 {
            continue;
        }
        let unminted = distribute_interest(ICUSD::from(snapshot_e8s), collateral_type).await;
        if unminted.0 > 0 {
            crate::state::mutate_state(|s| {
                s.restore_pending_interest_for_pool(collateral_type, unminted.to_u64());
            });
            log!(
                INFO,
                "[treasury] CRITICAL: re-queued {} icUSD for collateral {} after partial mint failure (snapshot {})",
                unminted.to_u64(),
                collateral_type,
                snapshot_e8s,
            );
        }
    }
}

/// Retry durable AMM1 operations. Rows remain stored across awaits, traps,
/// and upgrades; NotifyPending rows never execute the ledger mint again.
pub async fn flush_pending_amm1_donations() {
    let pending: Vec<u64> = crate::state::read_state(|s| {
        s.pending_amm1_donation_operations.keys().copied().collect()
    });
    if pending.is_empty() {
        return;
    }
    log!(INFO, "[treasury] AMM1 retry queue: advancing {} durable operations", pending.len());
    for notify_nonce in pending {
        process_amm1_donation(notify_nonce).await;
    }
}

/// Mint pending treasury interest accumulated from sync liquidations.
/// Called from the XRC timer tick to drain `pending_treasury_interest`.
///
/// Audit INT-006: uses the snapshot-then-decrement pattern so a
/// concurrent credit landing during the await is preserved on both
/// arms. The pre-await `take` zeroes the field so any concurrent
/// increment accumulates against zero; on mint failure the snapshot is
/// restored via `saturating_add`, merging with whatever landed.
pub async fn drain_pending_treasury_interest() {
    if read_state(|s| s.treasury_principal.is_none()) {
        return;
    }
    // Atomic snapshot+zero before the await.
    let snapshot = crate::state::mutate_state(|s| s.take_pending_treasury_interest());
    if snapshot.0 == 0 {
        return;
    }
    match start_treasury_interest_mint_at(snapshot, ic_cdk::api::time()) {
        Ok(op_nonce) => process_treasury_interest_mint(op_nonce).await,
        Err(_) => {
            crate::state::mutate_state(|s| s.restore_pending_treasury_interest(snapshot));
            log!(
                INFO,
                "[treasury] Pending interest admission unavailable; retained {} icUSD",
                snapshot.to_u64(),
            );
        }
    }
}

/// Admit event-backed stablecoin-interest fallbacks without erasing their
/// source obligation when the mint journal is full or Treasury is unavailable.
pub async fn drain_pending_treasury_interest_fallback() {
    let amount_e8s = crate::state::read_state(|s| {
        s.pending_treasury_interest_fallback_e8s.min(u128::from(u64::MAX)) as u64
    });
    if amount_e8s == 0 { return; }
    let Ok(op_nonce) = start_treasury_interest_mint_at(ICUSD::from(amount_e8s), ic_cdk::api::time()) else {
        return;
    };
    // No await separates the journal start from this source debit. If this
    // callback traps, both writes roll back and replay keeps the old credit.
    crate::state::mutate_state(|s| {
        let next = s.pending_treasury_interest_fallback_e8s
            .checked_sub(u128::from(amount_e8s))
            .expect("Treasury fallback source changed during admission");
        crate::storage::record_pending_payout_event(
            &crate::event::PendingPayoutEvent::TreasuryInterestFallbackAdmitted { amount_e8s },
        );
        s.pending_treasury_interest_fallback_e8s = next;
    });
    process_treasury_interest_mint(op_nonce).await;
}

/// Admit a bounded batch of stablecoin-backed Stability Pool shares that
/// could not get an exact journal slot (or pool target) at source time.
pub async fn drain_pending_stability_pool_interest_fallback() {
    let pending: Vec<(Principal, u64, Option<Principal>)> = crate::state::read_state(|s| {
        s.pending_stability_pool_interest_fallback_e8s
            .iter()
            .take(TREASURY_INTEREST_RETRY_BATCH)
            .map(|(&collateral_type, &amount)| {
                (
                    collateral_type,
                    amount.min(u128::from(u64::MAX)) as u64,
                    s.stability_pool_canister,
                )
            })
            .collect()
    });
    for (collateral_type, amount_e8s, destination) in pending {
        let Some(destination) = destination else {
            continue;
        };
        if amount_e8s == 0 {
            continue;
        }
        let Ok(op_nonce) = admit_stability_pool_interest_fallback_at(
            collateral_type,
            ICUSD::from(amount_e8s),
            destination,
            ic_cdk::api::time(),
        ) else {
            return;
        };
        // The atomic event is durable before this first await.
        process_treasury_interest_mint(op_nonce).await;
    }
}

/// Retry a bounded, fair batch of exact Treasury-interest operations. Held
/// unknown mint outcomes stay visible in state and are never reminted here.
pub async fn flush_pending_treasury_interest_mints() {
    let pending = crate::state::mutate_state(|s| {
        let cursor = s.treasury_interest_mint_retry_cursor;
        let eligible: Vec<u128> = s.pending_treasury_interest_mints.iter()
            .filter_map(|(&nonce, row)|
                (row.phase != crate::state::TreasuryInterestMintPhase::ReceiptRecoveryRequired)
                    .then_some(nonce))
            .collect();
        let mut selected: Vec<u128> = eligible.iter().copied()
            .filter(|nonce| *nonce > cursor)
            .take(TREASURY_INTEREST_RETRY_BATCH)
            .collect();
        if selected.len() < TREASURY_INTEREST_RETRY_BATCH {
            selected.extend(eligible.into_iter()
                .filter(|nonce| *nonce <= cursor)
                .take(TREASURY_INTEREST_RETRY_BATCH - selected.len()));
        }
        if let Some(last) = selected.last() {
            s.treasury_interest_mint_retry_cursor = *last;
        }
        selected
    });
    for op_nonce in pending {
        process_treasury_interest_mint(op_nonce).await;
    }
}

/// Transfer pending collateral fees to treasury.
/// Called from the XRC timer tick to drain `pending_treasury_collateral`.
pub async fn drain_pending_treasury_collateral() {
    let pending: Vec<(u64, Principal)> = read_state(|s| s.pending_treasury_collateral.clone());
    if pending.is_empty() {
        return;
    }
    let treasury = read_state(|s| s.treasury_principal);
    if let Some(tp) = treasury {
        let mut drained = Vec::new();
        for (amount, ledger) in &pending {
            let asset_type = collateral_to_asset_type(ledger);
            match management::transfer_collateral(*amount, tp, *ledger).await {
                Ok(block_index) => {
                    log!(
                        INFO,
                        "[treasury] Drained {} collateral fee for ledger {} (block {})",
                        amount,
                        ledger,
                        block_index
                    );
                    let _ = notify_treasury_deposit(
                        tp,
                        DepositType::LiquidationFee,
                        asset_type,
                        *amount,
                        block_index,
                    )
                    .await;
                    drained.push((*amount, *ledger));
                }
                Err(e) => log!(
                    INFO,
                    "[treasury] WARNING: collateral drain failed for {}: {:?}",
                    ledger,
                    e
                ),
            }
        }
        // Remove successfully drained entries
        if !drained.is_empty() {
            crate::state::mutate_state(|s| {
                s.pending_treasury_collateral
                    .retain(|entry| !drained.contains(entry));
            });
        }
    }
}


#[cfg(test)]
mod amm1_donation_retry_tests {
    use super::{process_amm1_donation_with_now, start_amm1_donation_at, Amm1MintFailure};
    use crate::state::{self, Amm1DonationPhase, State};
    use candid::Principal;
    use std::cell::RefCell;
    use std::collections::BTreeSet;

    fn configured_state() {
        let mut state = State::default();
        state.icusd_ledger_principal = Principal::from_slice(&[1]);
        state.amm1_canister = Some(Principal::from_slice(&[2]));
        state.amm1_pool_id = Some("pool".to_string());
        state::replace_state(state);
    }

    #[test]
    fn notify_rejection_retries_notification_without_reminting() {
        configured_state();
        let notify_nonce = start_amm1_donation_at(321, 1_000).expect("AMM1 pool configured").expect("AMM1 configured");
        let mint_calls = RefCell::new(0);
        futures::executor::block_on(process_amm1_donation_with_now(
            notify_nonce,
            1_001,
            |_| {
                *mint_calls.borrow_mut() += 1;
                async { Ok(77) }
            },
            |_| async { Err("temporary notification rejection".to_string()) },
        ));
        assert_eq!(*mint_calls.borrow(), 1);
        let pending =
            state::read_state(|s| s.pending_amm1_donation_operations[&notify_nonce].clone());
        assert_eq!(pending.phase, Amm1DonationPhase::NotifyPending);
        assert_eq!(pending.mint_block_index, Some(77));

        futures::executor::block_on(process_amm1_donation_with_now(
            notify_nonce,
            1_001,
            |_| async { panic!("notify retry must not call mint") },
            |_| async { Ok(()) },
        ));
        assert!(state::read_state(|s| !s
            .pending_amm1_donation_operations
            .contains_key(&notify_nonce)));
        assert_eq!(*mint_calls.borrow(), 1);
    }

    #[test]
    fn lost_mint_reply_retries_same_ledger_nonce_and_preserves_obligation() {
        configured_state();
        let notify_nonce = start_amm1_donation_at(654, 1_000).expect("AMM1 pool configured").expect("AMM1 configured");
        let expected_nonce =
            state::read_state(|s| s.pending_amm1_donation_operations[&notify_nonce].mint_op_nonce);
        let committed = RefCell::new(BTreeSet::new());
        let observed_nonces = RefCell::new(Vec::new());
        let first = RefCell::new(true);
        futures::executor::block_on(process_amm1_donation_with_now(
            notify_nonce,
            1_001,
            |operation| {
                observed_nonces.borrow_mut().push(operation.mint_op_nonce);
                let nonce = operation.mint_op_nonce;
                let committed = &committed;
                let first = &first;
                async move {
                    committed.borrow_mut().insert(nonce); // ledger commits, reply is lost
                    if std::mem::replace(&mut *first.borrow_mut(), false) {
                        Err(Amm1MintFailure::from(
                            icrc_ledger_types::icrc1::transfer::TransferError::TemporarilyUnavailable,
                        ))
                    } else {
                        Ok(88) // retry sees ledger Duplicate and helper normalizes it to Ok
                    }
                }
            },
            |_| async { panic!("first mint reply was lost, so notify is not reached") },
        ));
        assert!(state::read_state(|s| s
            .pending_amm1_donation_operations
            .contains_key(&notify_nonce)));
        assert_eq!(
            state::read_state(|s| s.pending_amm1_donation_operations[&notify_nonce].phase.clone()),
            Amm1DonationPhase::MintPending
        );

        futures::executor::block_on(process_amm1_donation_with_now(
            notify_nonce,
            1_001,
            |operation| {
                observed_nonces.borrow_mut().push(operation.mint_op_nonce);
                assert!(committed.borrow().contains(&operation.mint_op_nonce));
                async { Ok(88) }
            },
            |_| async { Ok(()) },
        ));
        assert_eq!(
            observed_nonces.borrow().as_slice(),
            &[expected_nonce, expected_nonce]
        );
        assert!(state::read_state(|s| !s
            .pending_amm1_donation_operations
            .contains_key(&notify_nonce)));
    }

    #[test]
    fn trap_during_remote_mint_keeps_the_preawait_row() {
        configured_state();
        let notify_nonce = start_amm1_donation_at(987, 1_000).expect("AMM1 pool configured").expect("AMM1 configured");
        let trap = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            futures::executor::block_on(process_amm1_donation_with_now(
                notify_nonce,
                1_001,
                |_| async { panic!("simulated callback trap") },
                |_| async { Ok(()) },
            ));
        }));
        assert!(trap.is_err());
        let pending = state::read_state(|s| {
            s.pending_amm1_donation_operations
                .get(&notify_nonce)
                .cloned()
        });
        assert!(
            pending.is_some(),
            "the obligation is stored before the remote await"
        );
        assert_eq!(pending.unwrap().phase, Amm1DonationPhase::MintPending);
    }

    #[test]
    fn too_old_mint_is_held_and_never_automatically_retried() {
        configured_state();
        let notify_nonce = start_amm1_donation_at(222, 1_000)
            .expect("AMM1 pool configured")
            .expect("AMM1 configured");
        let mint_calls = RefCell::new(0);
        let notify_calls = RefCell::new(0);
        futures::executor::block_on(process_amm1_donation_with_now(
            notify_nonce,
            1_001,
            |_| {
                *mint_calls.borrow_mut() += 1;
                async {
                    Err(Amm1MintFailure::from(
                        icrc_ledger_types::icrc1::transfer::TransferError::TooOld,
                    ))
                }
            },
            |_| {
                *notify_calls.borrow_mut() += 1;
                async { Ok(()) }
            },
        ));
        let pending = state::read_state(|s| {
            s.pending_amm1_donation_operations[&notify_nonce].clone()
        });
        assert_eq!(pending.phase, Amm1DonationPhase::ReconciliationRequired);
        assert_eq!(pending.reconciliation_reason, Some(crate::state::Amm1DonationReconciliationReason::LedgerTooOld));
        assert_eq!(*mint_calls.borrow(), 1);
        assert_eq!(*notify_calls.borrow(), 0);

        futures::executor::block_on(process_amm1_donation_with_now(
            notify_nonce,
            1_002,
            |_| async { panic!("held TooOld mint must not be retried") },
            |_| async { panic!("held TooOld mint must not be notified") },
        ));
        assert_eq!(*mint_calls.borrow(), 1);
        assert_eq!(*notify_calls.borrow(), 0);
    }

    #[test]
    fn aged_unknown_mint_is_held_before_another_ledger_call() {
        configured_state();
        let notify_nonce = start_amm1_donation_at(333, 1_000)
            .expect("AMM1 pool configured")
            .expect("AMM1 configured");
        let mint_calls = RefCell::new(0);
        futures::executor::block_on(process_amm1_donation_with_now(
            notify_nonce,
            1_000 + super::AMM1_AUTO_MINT_RETRY_MAX_AGE_NS,
            |_| {
                *mint_calls.borrow_mut() += 1;
                async { Ok(90) }
            },
            |_| async { Ok(()) },
        ));
        let pending = state::read_state(|s| {
            s.pending_amm1_donation_operations[&notify_nonce].clone()
        });
        assert_eq!(pending.phase, Amm1DonationPhase::ReconciliationRequired);
        assert_eq!(pending.reconciliation_reason, Some(crate::state::Amm1DonationReconciliationReason::DedupWindowElapsedUnknown));
        assert_eq!(*mint_calls.borrow(), 0);
    }

    #[test]
    fn legacy_rows_are_moved_to_held_reconciliation_without_mint_data_invention() {
        let mut state = State::default();
        state.pending_amm1_donations.push_back((42, 7));
        assert_eq!(state.migrate_legacy_amm1_donations(), 1);
        assert!(state.pending_amm1_donations.is_empty());
        assert!(state.pending_amm1_donation_operations.is_empty());
        assert_eq!(state.held_amm1_donations.len(), 1);
        assert_eq!(state.held_amm1_donations[0].amount_e8s, 42);
        assert_eq!(state.held_amm1_donations[0].notify_nonce, 7);
        assert!(state.held_amm1_donations[0]
            .reason
            .contains("lacks pinned ledger"));
    }

    #[test]
    fn configured_amm_without_pool_id_holds_share_instead_of_redirecting_it() {
        configured_state();
        state::mutate_state(|s| s.amm1_pool_id = None);
        assert!(start_amm1_donation_at(123, 1_000).is_err());
        assert_eq!(state::read_state(|s| s.held_amm1_donations.len()), 1);
        assert_eq!(state::read_state(|s| s.held_amm1_donations[0].amount_e8s), 123);
        assert!(state::read_state(|s| s.pending_amm1_donation_operations.is_empty()));
    }
}

#[cfg(test)]
mod treasury_interest_mint_tests {
    use super::{
        advance_pending_treasury_interest_mint_recovery_with, distribute_interest_at,
        distribute_stablecoin_interest_at, process_treasury_interest_mint_with_now,
        admit_stability_pool_interest_fallback_at, reserve_stablecoin_sp_interest_shares,
        reserve_stablecoin_amm1_donations, reserve_stablecoin_three_pool_shares,
        reserve_interest_distribution_jobs,
        start_routed_interest_mint_at, start_treasury_interest_mint_at,
        TreasuryInterestMintFailure, TREASURY_INTEREST_AUTO_RETRY_MAX_AGE_NS,
    };
    use crate::numeric::ICUSD;
    use crate::state::{
        self, InterestDestination, InterestRecipient, State, TreasuryInterestMintPhase,
        TreasuryInterestMintRoute,
    };
    use candid::Principal;
    use std::cell::RefCell;

    fn configured_state() {
        let mut state = State::default();
        state.icusd_ledger_principal = Principal::from_slice(&[1]);
        state.treasury_principal = Some(Principal::from_slice(&[2]));
        state.developer_principal = Principal::from_slice(&[9]);
        state::replace_state(state);
    }

    #[test]
    fn amm1_share_is_journaled_before_first_recipient_await() {
        configured_state();
        let collateral = Principal::from_slice(&[3]);
        state::mutate_state(|s| {
            s.stability_pool_canister = Some(Principal::from_slice(&[4]));
            s.amm1_canister = Some(Principal::from_slice(&[5]));
            s.amm1_pool_id = Some("pool".to_string());
            s.interest_split = vec![
                InterestRecipient { destination: InterestDestination::StabilityPool, bps: 7_500 },
                InterestRecipient { destination: InterestDestination::Amm1, bps: 2_500 },
            ];
            s.pending_interest_for_pools.insert(collateral, 10_000);
            assert_eq!(s.take_pending_interest_for_pool(collateral), 10_000);
        });
        let split = state::read_state(|s| s.interest_split.clone());

        // This is the synchronous pre-await reservation pass used by
        // distribute_interest_at. Simulate interruption at the first await:
        // no processor runs, but every share already has a durable owner.
        let reservations = std::cell::RefCell::new(None);
        let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            futures::executor::block_on(async {
                reservations.replace(Some(reserve_interest_distribution_jobs(
                    10_000, collateral, &split, None, 1_000,
                )));
                async { panic!("simulated trap at first recipient await") }.await;
            });
        }));
        assert!(interrupted.is_err());

        let (mint_jobs, amm1_jobs, amm1_missing) = reservations.into_inner().unwrap();
        assert!(!amm1_missing);
        let mint_nonce = mint_jobs[&0].expect("SP mint reservation");
        let donation_nonce = amm1_jobs[&1].expect("AMM1 donation reservation");
        let sp_amount = state::read_state(|s| {
            s.pending_treasury_interest_mints[&mint_nonce].tuple.amount_e8s
        });
        let donation_amount = state::read_state(|s| {
            s.pending_amm1_donation_operations[&donation_nonce].amount_e8s
        });
        assert_eq!(sp_amount, 7_500);
        assert_eq!(donation_amount, 2_500);
        assert_eq!(sp_amount + donation_amount, 10_000);
        assert_eq!(
            state::read_state(|s| s
                .pending_interest_for_pools
                .get(&collateral)
                .copied()
                .unwrap_or(0)),
            0
        );
        assert_eq!(state::read_state(|s| s.pending_amm1_donation_operations.len()), 1);
    }

    #[test]
    fn stablecoin_amm1_share_is_journaled_before_first_recipient_await() {
        configured_state();
        let collateral = Principal::from_slice(&[3]);
        let stability_pool = Principal::from_slice(&[4]);
        state::mutate_state(|s| {
            s.stability_pool_canister = Some(stability_pool);
            s.amm1_canister = Some(Principal::from_slice(&[5]));
            s.amm1_pool_id = Some("pool".to_string());
            s.interest_split = vec![
                InterestRecipient { destination: InterestDestination::StabilityPool, bps: 7_500 },
                InterestRecipient { destination: InterestDestination::Amm1, bps: 2_500 },
            ];
        });
        let split = state::read_state(|s| s.interest_split.clone());

        // Both reservation helpers are called before stablecoin distribution's
        // first await. Simulate interruption there and verify that both source
        // shares already have exact durable owners.
        let reservations = std::cell::RefCell::new(None);
        let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            futures::executor::block_on(async {
                let sp_jobs = reserve_stablecoin_sp_interest_shares(
                    10_000,
                    collateral,
                    &split,
                    Some(stability_pool),
                    1_000,
                );
                let amm1_jobs = reserve_stablecoin_amm1_donations(10_000, &split, false, 1_000);
                reservations.replace(Some((sp_jobs, amm1_jobs)));
                async { panic!("simulated trap at first recipient await") }.await;
            });
        }));
        assert!(interrupted.is_err());

        let (sp_jobs, amm1_jobs) = reservations.into_inner().unwrap();
        let sp_nonce = sp_jobs[&0];
        let donation_nonce = amm1_jobs[&1].expect("AMM1 donation reservation");
        let (sp_amount, sp_destination) = state::read_state(|s| {
            let row = &s.pending_treasury_interest_mints[&sp_nonce];
            (row.tuple.amount_e8s, row.tuple.destination)
        });
        let donation_amount = state::read_state(|s| {
            s.pending_amm1_donation_operations[&donation_nonce].amount_e8s
        });
        assert_eq!(sp_destination, stability_pool);
        assert_eq!(sp_amount, 7_500);
        assert_eq!(donation_amount, 2_500);
        assert_eq!(sp_amount + donation_amount, 10_000);
        assert_eq!(state::read_state(|s| s.pending_amm1_donation_operations.len()), 1);
    }

    fn arm_mint(nonce: u128) {
        state::mutate_state(|s| {
            let row = s.pending_treasury_interest_mints.get_mut(&nonce).unwrap();
            row.history_floor = Some(0);
            row.phase = TreasuryInterestMintPhase::MintPending;
        });
    }

    fn held_interest_mint(history_floor: Option<u64>) -> u128 {
        let nonce = start_treasury_interest_mint_at(ICUSD::from(42u64), 1_000).unwrap();
        state::mutate_state(|s| {
            let row = s.pending_treasury_interest_mints.get_mut(&nonce).unwrap();
            row.phase = TreasuryInterestMintPhase::ReceiptRecoveryRequired;
            row.history_floor = history_floor;
        });
        nonce
    }

    fn recovery_page(
        start: u64,
        end: u64,
        tip: u64,
        tuple: &crate::state::BorrowMintTuple,
        exact_index: Option<u64>,
    ) -> icrc_ledger_types::icrc3::blocks::GetBlocksResult {
        use icrc_ledger_types::icrc::generic_value::{ICRC3Map, ICRC3Value};
        use icrc_ledger_types::icrc3::blocks::{BlockWithId, GetBlocksResult};
        let blocks = (start..end).map(|index| {
            let mut tx = ICRC3Map::new();
            let op = if exact_index == Some(index) { "mint" } else { "burn" };
            tx.insert("op".into(), ICRC3Value::Text(op.into()));
            if op == "mint" {
                tx.insert("to".into(), ICRC3Value::Array(vec![ICRC3Value::Blob(
                    serde_bytes::ByteBuf::from(tuple.destination.as_slice().to_vec()),
                )]));
                tx.insert("amt".into(), ICRC3Value::Nat(candid::Nat::from(tuple.amount_e8s)));
                tx.insert("memo".into(), ICRC3Value::Blob(
                    serde_bytes::ByteBuf::from(tuple.memo.to_vec()),
                ));
                tx.insert("ts".into(), ICRC3Value::Nat(candid::Nat::from(tuple.created_at_time_ns)));
            } else {
                tx.insert("amt".into(), ICRC3Value::Nat(candid::Nat::from(1u8)));
            }
            let mut block = ICRC3Map::new();
            block.insert("btype".into(), ICRC3Value::Text(format!("1{op}")));
            block.insert("tx".into(), ICRC3Value::Map(tx));
            BlockWithId { id: candid::Nat::from(index), block: ICRC3Value::Map(block) }
        }).collect();
        GetBlocksResult { log_length: candid::Nat::from(tip), blocks, archived_blocks: vec![] }
    }

    #[test]
    fn uncertain_mint_retries_same_tuple_then_retries_only_notification() {
        configured_state();
        let nonce = start_treasury_interest_mint_at(ICUSD::from(900u64), 1_000).unwrap();
        arm_mint(nonce);
        let tuple = state::read_state(|s| s.pending_treasury_interest_mints[&nonce].tuple.clone());
        let observed = RefCell::new(Vec::new());
        futures::executor::block_on(process_treasury_interest_mint_with_now(
            nonce,
            1_001,
            |operation| {
                observed.borrow_mut().push(operation.tuple);
                async {
                    Err(TreasuryInterestMintFailure::Retry(
                        icrc_ledger_types::icrc1::transfer::TransferError::TemporarilyUnavailable,
                    ))
                }
            },
            |_, _| async { panic!("uncertain mint must not notify") },
        ));
        assert_eq!(state::read_state(|s| s.pending_treasury_interest_mints[&nonce].phase.clone()),
            TreasuryInterestMintPhase::MintPending);
        futures::executor::block_on(process_treasury_interest_mint_with_now(
            nonce,
            1_002,
            |operation| {
                observed.borrow_mut().push(operation.tuple);
                async { Ok(77) } // exact ledger Duplicate of first committed transfer
            },
            |operation, block| {
                assert_eq!(operation.treasury, tuple.destination);
                assert_eq!(block, 77);
                async { Err("Treasury backfill incomplete".into()) }
            },
        ));
        assert_eq!(observed.borrow().as_slice(), &[tuple.clone(), tuple]);
        assert_eq!(state::read_state(|s| s.pending_treasury_interest_mints[&nonce].phase.clone()),
            TreasuryInterestMintPhase::NotifyPending { mint_block_index: 77 });
        futures::executor::block_on(process_treasury_interest_mint_with_now(
            nonce,
            1_003,
            |_| async { panic!("notification retry must never remint") },
            |_, block| async move { assert_eq!(block, 77); Ok(()) },
        ));
        assert!(state::read_state(|s| !s.pending_treasury_interest_mints.contains_key(&nonce)));
    }

    #[test]
    fn aged_unknown_and_typed_too_old_are_held_without_remint() {
        configured_state();
        let aged = start_treasury_interest_mint_at(ICUSD::from(1u64), 1_000).unwrap();
        arm_mint(aged);
        futures::executor::block_on(process_treasury_interest_mint_with_now(
            aged,
            1_000 + TREASURY_INTEREST_AUTO_RETRY_MAX_AGE_NS,
            |_| async { panic!("aged unknown may not be retried") },
            |_, _| async { panic!("unknown mint may not be notified") },
        ));
        assert_eq!(state::read_state(|s| s.pending_treasury_interest_mints[&aged].phase.clone()),
            TreasuryInterestMintPhase::ReceiptRecoveryRequired);

        let too_old = start_treasury_interest_mint_at(ICUSD::from(2u64), 2_000).unwrap();
        arm_mint(too_old);
        futures::executor::block_on(process_treasury_interest_mint_with_now(
            too_old,
            2_001,
            |_| async { Err(TreasuryInterestMintFailure::Hold) },
            |_, _| async { panic!("TooOld mint may not be notified") },
        ));
        assert_eq!(state::read_state(|s| s.pending_treasury_interest_mints[&too_old].phase.clone()),
            TreasuryInterestMintPhase::ReceiptRecoveryRequired);
    }

    #[test]
    fn admission_failure_does_not_claim_the_share() {
        let mut state = State::default();
        state.treasury_principal = None;
        state::replace_state(state);
        assert_eq!(start_treasury_interest_mint_at(ICUSD::from(42u64), 1_000), Err(ICUSD::from(42u64)));
        assert!(state::read_state(|s| s.pending_treasury_interest_mints.is_empty()));
    }

    #[test]
    fn exact_positive_receipt_advances_held_row_and_replay_is_idempotent() {
        configured_state();
        let nonce = start_treasury_interest_mint_at(ICUSD::from(42u64), 1_000).unwrap();
        let developer = Principal::from_slice(&[9]);
        state::mutate_state(|s| {
            s.pending_treasury_interest_mints.get_mut(&nonce).unwrap().phase =
                TreasuryInterestMintPhase::ReceiptRecoveryRequired;
        });
        let expected = state::read_state(|s| s.pending_treasury_interest_mints[&nonce].clone());
        futures::executor::block_on(super::reconcile_treasury_interest_mint_from_block_with(
            nonce,
            77,
            developer,
            move |ledger, block, tuple| async move {
                assert_eq!(ledger, expected.tuple.ledger);
                assert_eq!(block, 77);
                assert_eq!(tuple, expected.tuple);
                Ok(())
            },
        )).unwrap();
        assert_eq!(state::read_state(|s| s.pending_treasury_interest_mints[&nonce].phase.clone()),
            TreasuryInterestMintPhase::NotifyPending { mint_block_index: 77 });

        futures::executor::block_on(super::reconcile_treasury_interest_mint_from_block_with(
            nonce,
            77,
            developer,
            |_, _, _| async { panic!("same-block replay must not fetch or remint") },
        )).unwrap();
        assert_eq!(state::read_state(|s| s.pending_treasury_interest_mints[&nonce].phase.clone()),
            TreasuryInterestMintPhase::NotifyPending { mint_block_index: 77 });
    }

    #[test]
    fn mismatched_receipt_leaves_held_row_unchanged() {
        configured_state();
        let nonce = start_treasury_interest_mint_at(ICUSD::from(42u64), 1_000).unwrap();
        let developer = Principal::from_slice(&[9]);
        state::mutate_state(|s| {
            s.pending_treasury_interest_mints.get_mut(&nonce).unwrap().phase =
                TreasuryInterestMintPhase::ReceiptRecoveryRequired;
        });
        let before = state::read_state(|s| s.pending_treasury_interest_mints[&nonce].clone());
        let result = futures::executor::block_on(
            super::reconcile_treasury_interest_mint_from_block_with(
                nonce,
                77,
                developer,
                |_, _, _| async { Err("candidate block does not match the exact mint".into()) },
            ),
        );
        assert!(result.is_err());
        assert_eq!(state::read_state(|s| s.pending_treasury_interest_mints[&nonce].clone()), before);
    }

    #[test]
    fn expiry_probe_accepts_only_typed_too_old() {
        use icrc_ledger_types::icrc1::transfer::TransferError;
        assert!(super::classify_treasury_interest_expiry_probe_outcome(
            Ok(Err(TransferError::TooOld)),
        ).is_ok());
        assert!(super::classify_treasury_interest_expiry_probe_outcome(
            Ok(Err(TransferError::Duplicate { duplicate_of: 1u64.into() })),
        ).is_err());
        assert!(super::classify_treasury_interest_expiry_probe_outcome(
            Ok(Ok(0u64.into())),
        ).is_err());
        assert!(super::classify_treasury_interest_expiry_probe_outcome(
            Err((ic_cdk::api::call::RejectionCode::CanisterError, "rejected".into())),
        ).is_err());
    }

    #[test]
    fn full_absence_scan_replaces_tuple_only_after_fixed_tip_is_reached() {
        configured_state();
        let nonce = held_interest_mint(Some(0));
        let developer = Principal::from_slice(&[9]);
        let original = state::read_state(|s| s.pending_treasury_interest_mints[&nonce].clone());
        let probe_tuple = original.tuple.clone();
        let probe_treasury = original.treasury;
        let first_page_tuple = original.tuple.clone();
        futures::executor::block_on(advance_pending_treasury_interest_mint_recovery_with(
            nonce,
            developer,
            2_000,
            move |tuple, sink, memo| async move {
                assert_eq!(tuple, probe_tuple);
                assert_ne!(sink, probe_treasury);
                assert_ne!(memo.as_slice(), tuple.memo.as_slice());
                Ok(()) // injected typed TooOld result
            },
            |ledger| async move {
                assert_eq!(ledger, Principal::from_slice(&[1]));
                Ok(65)
            },
            |ledger, start, end| async move {
                assert_eq!(ledger, Principal::from_slice(&[1]));
                assert_eq!((start, end), (0, 64));
                Ok(recovery_page(start, end, 65, &first_page_tuple, None))
            },
        )).unwrap();
        let after_first = state::read_state(|s| s.pending_treasury_interest_mints[&nonce].clone());
        assert!(after_first.expiry_proven);
        assert_eq!(after_first.absence_scan.as_ref().unwrap().next_block_index, 64);
        assert!(after_first.absence_scan.as_ref().unwrap().candidate_block_index.is_none());
        let continuation_tuple = after_first.tuple.clone();
        let continuation_treasury = after_first.treasury;
        let continuation_page_tuple = continuation_tuple.clone();

        futures::executor::block_on(advance_pending_treasury_interest_mint_recovery_with(
            nonce,
            developer,
            2_001,
            |_, _, _| async { Err("probe must not be repeated".into()) },
            |_| async { Err("fixed tip must not be recaptured".into()) },
            |ledger, start, end| async move {
                assert_eq!(ledger, Principal::from_slice(&[1]));
                assert_eq!((start, end), (64, 65));
                Ok(recovery_page(start, end, 65, &continuation_page_tuple, None))
            },
        )).unwrap();
        let rows = state::read_state(|s| s.pending_treasury_interest_mints.values().cloned().collect::<Vec<_>>());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].phase, TreasuryInterestMintPhase::AwaitingHistoryFloor);
        assert_eq!(rows[0].tuple.amount_e8s, 42);
        assert_eq!(rows[0].treasury, continuation_treasury);
        assert_ne!(rows[0].tuple, continuation_tuple);
        assert!(rows[0].history_floor.is_none());
        assert!(rows[0].absence_scan.is_none());
    }

    #[test]
    fn scan_with_candidate_keeps_scanning_to_tip_before_receipt_commit() {
        configured_state();
        let nonce = held_interest_mint(Some(0));
        let developer = Principal::from_slice(&[9]);
        let original = state::read_state(|s| s.pending_treasury_interest_mints[&nonce].clone());
        let first_page_tuple = original.tuple.clone();
        let second_page_tuple = original.tuple.clone();
        futures::executor::block_on(advance_pending_treasury_interest_mint_recovery_with(
            nonce,
            developer,
            2_000,
            |_, _, _| async { Ok(()) },
            |_| async { Ok(65) },
            move |_, start, end| async move {
                assert_eq!((start, end), (0, 64));
                Ok(recovery_page(start, end, 65, &first_page_tuple, Some(0)))
            },
        )).unwrap();
        let scan = state::read_state(|s| s.pending_treasury_interest_mints[&nonce].absence_scan.clone().unwrap());
        assert_eq!(scan.next_block_index, 64);
        assert_eq!(scan.candidate_block_index, Some(0));
        assert_eq!(state::read_state(|s| s.pending_treasury_interest_mints[&nonce].phase.clone()),
            TreasuryInterestMintPhase::ReceiptRecoveryRequired);

        futures::executor::block_on(advance_pending_treasury_interest_mint_recovery_with(
            nonce,
            developer,
            2_001,
            |_, _, _| async { Err("probe must not be repeated".into()) },
            |_| async { Err("fixed tip must not be recaptured".into()) },
            move |_, start, end| async move {
                assert_eq!((start, end), (64, 65));
                Ok(recovery_page(start, end, 65, &second_page_tuple, None))
            },
        )).unwrap();
        assert_eq!(state::read_state(|s| s.pending_treasury_interest_mints[&nonce].phase.clone()),
            TreasuryInterestMintPhase::NotifyPending { mint_block_index: 0 });
    }

    #[test]
    fn stablecoin_fallback_preserves_interest_when_mint_journal_is_full() {
        for destination in [InterestDestination::ThreePool, InterestDestination::Amm1] {
            configured_state();
            let row = state::read_state(|s| {
                let nonce = 1u128;
                crate::state::TreasuryInterestMintJournal {
                    tuple: crate::state::BorrowMintTuple {
                        ledger: s.icusd_ledger_principal,
                        destination: s.treasury_principal.unwrap(),
                        amount_e8s: 1,
                        memo: nonce.to_be_bytes(),
                        created_at_time_ns: 1,
                        op_nonce: nonce,
                    },
                    treasury: s.treasury_principal.unwrap(),
                    route: crate::state::TreasuryInterestMintRoute::Treasury,
                    phase: TreasuryInterestMintPhase::ReceiptRecoveryRequired,
                    history_floor: None,
                    expiry_proven: false,
                    absence_scan: None,
                }
            });
            state::mutate_state(|s| {
                s.interest_split = vec![InterestRecipient { destination, bps: 10_000 }];
                for nonce in 1..=super::MAX_PENDING_TREASURY_INTEREST_MINTS {
                    s.pending_treasury_interest_mints.insert(nonce as u128, row.clone());
                }
            });
            futures::executor::block_on(distribute_stablecoin_interest_at(
                500,
                Principal::from_slice(&[3]),
                crate::StableTokenType::CKUSDC,
                7,
                Principal::from_slice(&[2]),
                1_000,
            ));
            assert_eq!(state::read_state(|s| s.pending_treasury_interest_fallback_e8s), 500);
            assert_eq!(state::read_state(|s| s.pending_treasury_interest_mints.len()),
                super::MAX_PENDING_TREASURY_INTEREST_MINTS);
        }
    }

    #[test]
    fn icusd_interest_admission_failure_restores_share_before_any_await() {
        configured_state();
        state::mutate_state(|s| {
            s.interest_split = vec![InterestRecipient {
                destination: InterestDestination::Treasury,
                bps: 10_000,
            }];
            let treasury = s.treasury_principal.unwrap();
            for nonce in 1..=super::MAX_PENDING_TREASURY_INTEREST_MINTS {
                s.pending_treasury_interest_mints.insert(nonce as u128,
                    crate::state::TreasuryInterestMintJournal {
                        tuple: crate::state::BorrowMintTuple {
                            ledger: s.icusd_ledger_principal,
                            destination: treasury,
                            amount_e8s: 1,
                            memo: (nonce as u128).to_be_bytes(),
                            created_at_time_ns: 1,
                            op_nonce: nonce as u128,
                        },
                        treasury,
                        route: crate::state::TreasuryInterestMintRoute::Treasury,
                        phase: TreasuryInterestMintPhase::ReceiptRecoveryRequired,
                        history_floor: None,
                        expiry_proven: false,
                        absence_scan: None,
                    });
            }
        });
        let collateral = Principal::from_slice(&[3]);
        let unminted = futures::executor::block_on(distribute_interest_at(ICUSD::from(500u64), collateral, 1_000));
        assert_eq!(unminted.to_u64(), 0, "the share was already put in the pending bucket");
        assert_eq!(state::read_state(|s| s.pending_interest_for_pools[&collateral]), 500);
    }

    #[test]
    fn stablecoin_interest_journals_exact_floored_sp_share() {
        configured_state();
        let collateral = Principal::from_slice(&[3]);
        let pool = Principal::from_slice(&[4]);
        state::mutate_state(|s| s.stability_pool_canister = Some(pool));
        let split = vec![
            InterestRecipient { destination: InterestDestination::StabilityPool, bps: 7_500 },
            InterestRecipient { destination: InterestDestination::Treasury, bps: 2_500 },
        ];

        let jobs = reserve_stablecoin_sp_interest_shares(
            500_000_001, collateral, &split, Some(pool), 1_000,
        );
        let nonce = jobs[&0];
        let row = state::read_state(|s| s.pending_treasury_interest_mints[&nonce].clone());
        assert_eq!(row.tuple.amount_e8s, 375_000_000);
        assert_eq!(row.tuple.destination, pool);
        assert_eq!(row.route, TreasuryInterestMintRoute::StabilityPool { collateral_type: collateral });
        assert_eq!(row.phase, TreasuryInterestMintPhase::AwaitingHistoryFloor);
        assert!(state::read_state(|s| s.pending_stability_pool_interest_fallback_e8s.is_empty()));
    }

    #[test]
    fn stablecoin_sp_lost_mint_reply_and_notification_retry_preserve_one_mint() {
        configured_state();
        let collateral = Principal::from_slice(&[3]);
        let pool = Principal::from_slice(&[4]);
        state::mutate_state(|s| s.stability_pool_canister = Some(pool));
        let split = vec![InterestRecipient { destination: InterestDestination::StabilityPool, bps: 7_500 }];
        let nonce = reserve_stablecoin_sp_interest_shares(
            500_000_000, collateral, &split, Some(pool), 1_000,
        )[&0];
        state::mutate_state(|s| {
            let row = s.pending_treasury_interest_mints.get_mut(&nonce).unwrap();
            row.history_floor = Some(0);
            row.phase = TreasuryInterestMintPhase::MintPending;
        });
        let tuple = state::read_state(|s| s.pending_treasury_interest_mints[&nonce].tuple.clone());
        let sent = RefCell::new(Vec::new());
        let first_mint = RefCell::new(true);

        futures::executor::block_on(process_treasury_interest_mint_with_now(
            nonce,
            1_001,
            |operation| {
                assert_eq!(operation.tuple, tuple);
                sent.borrow_mut().push(operation.tuple.clone());
                let first_mint = &first_mint;
                async move {
                    if std::mem::replace(&mut *first_mint.borrow_mut(), false) {
                        Err(TreasuryInterestMintFailure::Retry(
                            icrc_ledger_types::icrc1::transfer::TransferError::TemporarilyUnavailable,
                        ))
                    } else {
                        Ok(88)
                    }
                }
            },
            |_, _| async { panic!("lost mint reply must not trigger notification") },
        ));
        assert_eq!(sent.borrow().as_slice(), &[tuple.clone()]);
        assert_eq!(state::read_state(|s| s.pending_treasury_interest_mints[&nonce].phase.clone()),
            TreasuryInterestMintPhase::MintPending);

        futures::executor::block_on(process_treasury_interest_mint_with_now(
            nonce,
            1_002,
            |operation| {
                assert_eq!(operation.tuple, tuple);
                sent.borrow_mut().push(operation.tuple.clone());
                async { Ok(88) } // the same tuple's Duplicate resolves to its original block
            },
            |operation, block| async move {
                assert_eq!(operation.route, TreasuryInterestMintRoute::StabilityPool { collateral_type: collateral });
                assert_eq!(block, 88);
                Err("notification response unavailable".into())
            },
        ));
        assert_eq!(sent.borrow().as_slice(), &[tuple.clone(), tuple]);
        assert_eq!(state::read_state(|s| s.pending_treasury_interest_mints[&nonce].phase.clone()),
            TreasuryInterestMintPhase::NotifyPending { mint_block_index: 88 });

        futures::executor::block_on(process_treasury_interest_mint_with_now(
            nonce,
            1_003,
            |_| async { panic!("notification retry must not remint") },
            |_, block| async move {
                assert_eq!(block, 88);
                Ok(())
            },
        ));
        assert!(!state::read_state(|s| s.pending_treasury_interest_mints.contains_key(&nonce)));
    }

    #[test]
    fn icusd_sp_commit_then_lost_reply_retries_the_same_mint_tuple() {
        use icrc_ledger_types::icrc1::transfer::TransferError;

        configured_state();
        let collateral = Principal::from_slice(&[3]);
        let pool = Principal::from_slice(&[4]);
        let nonce = start_routed_interest_mint_at(
            ICUSD::from(765u64),
            pool,
            TreasuryInterestMintRoute::StabilityPool { collateral_type: collateral },
            1_000,
        ).unwrap();
        arm_mint(nonce);
        let tuple = state::read_state(|s| s.pending_treasury_interest_mints[&nonce].tuple.clone());
        let committed = RefCell::new(None::<(crate::state::BorrowMintTuple, u64)>);
        let sent = RefCell::new(Vec::new());
        let notify_calls = RefCell::new(0usize);

        // Model a ledger that commits block 88 but whose response is lost.
        // Replaying the exact tuple resolves as Duplicate at the same block.
        futures::executor::block_on(process_treasury_interest_mint_with_now(
            nonce,
            1_001,
            |operation| {
                assert_eq!(operation.tuple, tuple);
                sent.borrow_mut().push(operation.tuple.clone());
                let committed = &committed;
                async move {
                    let existing = committed.borrow().clone();
                    match existing {
                        Some((prior, block)) => {
                            assert_eq!(prior, operation.tuple);
                            Ok(block)
                        }
                        None => {
                            *committed.borrow_mut() = Some((operation.tuple, 88));
                            Err(TreasuryInterestMintFailure::Retry(
                                TransferError::TemporarilyUnavailable,
                            ))
                        }
                    }
                }
            },
            |_, _| async { panic!("lost mint response must not notify") },
        ));
        assert_eq!(state::read_state(|s| s.pending_treasury_interest_mints[&nonce].phase.clone()),
            TreasuryInterestMintPhase::MintPending);

        futures::executor::block_on(process_treasury_interest_mint_with_now(
            nonce,
            1_002,
            |operation| {
                assert_eq!(operation.tuple, tuple);
                sent.borrow_mut().push(operation.tuple.clone());
                let committed = &committed;
                async move {
                    let receipt = committed.borrow();
                    let (prior, block) = receipt.as_ref().unwrap();
                    assert_eq!(prior, &operation.tuple);
                    Ok(*block)
                }
            },
            |operation, block| {
                assert_eq!(operation.route,
                    TreasuryInterestMintRoute::StabilityPool { collateral_type: collateral });
                assert_eq!(block, 88);
                *notify_calls.borrow_mut() += 1;
                async { Err("notification response lost".into()) }
            },
        ));
        assert_eq!(sent.borrow().as_slice(), &[tuple.clone(), tuple.clone()]);
        assert_eq!(committed.borrow().as_ref().unwrap().1, 88);
        assert_eq!(*notify_calls.borrow(), 1);
        assert_eq!(state::read_state(|s| s.pending_treasury_interest_mints[&nonce].phase.clone()),
            TreasuryInterestMintPhase::NotifyPending { mint_block_index: 88 });

        futures::executor::block_on(process_treasury_interest_mint_with_now(
            nonce,
            1_003,
            |_| async { panic!("notification retry must not mint") },
            |_, block| {
                assert_eq!(block, 88);
                *notify_calls.borrow_mut() += 1;
                async { Ok(()) }
            },
        ));
        assert_eq!(*notify_calls.borrow(), 2);
        assert!(!state::read_state(|s| s.pending_treasury_interest_mints.contains_key(&nonce)));
    }

    #[test]
    fn three_pool_reply_loss_retries_same_key_until_accounting_acknowledges() {
        configured_state();
        let pool = Principal::from_slice(&[4]);
        let nonce = start_routed_interest_mint_at(
            ICUSD::from(321u64),
            pool,
            TreasuryInterestMintRoute::ThreePool,
            1_000,
        ).unwrap();
        arm_mint(nonce);
        let row = state::read_state(|s| s.pending_treasury_interest_mints[&nonce].clone());
        assert_eq!(row.tuple.destination, pool);
        assert_eq!(row.tuple.amount_e8s, 321);
        assert_eq!(row.tuple.memo, nonce.to_be_bytes());
        assert_eq!(row.route, TreasuryInterestMintRoute::ThreePool);

        let receipt = RefCell::new(None::<(u128, u64, u64)>);
        let reserve_added = RefCell::new(0u64);
        futures::executor::block_on(process_treasury_interest_mint_with_now(
            nonce,
            1_001,
            |_| async { Ok(55) },
            |operation, block| {
                assert_eq!(operation.tuple.op_nonce, nonce);
                assert_eq!(block, 55);
                assert!(state::read_state(|s| s.pending_treasury_interest_mints.contains_key(&nonce)),
                    "backend must retain NotifyPending until the destination acknowledges");
                let key = (operation.tuple.op_nonce, block, operation.tuple.amount_e8s);
                let existing = receipt.borrow().clone();
                let result = match existing {
                    None => {
                        *receipt.borrow_mut() = Some(key);
                        *reserve_added.borrow_mut() += operation.tuple.amount_e8s;
                        Err("reply lost after pool accounting".into())
                    }
                    Some(prior) => {
                        assert_eq!(prior, key, "retry must use the identical receipt key and payload");
                        Ok(())
                    }
                };
                async move { result }
            },
        ));
        assert_eq!(*reserve_added.borrow(), 321);
        assert_eq!(state::read_state(|s| s.pending_treasury_interest_mints[&nonce].phase.clone()),
            TreasuryInterestMintPhase::NotifyPending { mint_block_index: 55 });

        futures::executor::block_on(process_treasury_interest_mint_with_now(
            nonce,
            1_002,
            |_| async { panic!("ack retry must not remint") },
            |operation, block| {
                assert_eq!(operation.tuple.op_nonce, nonce);
                assert_eq!(block, 55);
                assert_eq!(receipt.borrow().as_ref(), Some(&(nonce, 55, 321)));
                *reserve_added.borrow_mut() += 0;
                async { Ok(()) }
            },
        ));
        assert_eq!(*reserve_added.borrow(), 321, "same-key duplicate cannot credit reserves twice");
        assert!(!state::read_state(|s| s.pending_treasury_interest_mints.contains_key(&nonce)));
    }

    #[test]
    fn stablecoin_three_pool_shares_reserve_distinct_exact_mint_tuples() {
        configured_state();
        let pool = Principal::from_slice(&[4]);
        let split = vec![
            InterestRecipient { destination: InterestDestination::ThreePool, bps: 5_000 },
            InterestRecipient { destination: InterestDestination::ThreePool, bps: 5_000 },
        ];
        let jobs = reserve_stablecoin_three_pool_shares(1_000, &split, Some(pool), 1_000);
        assert_eq!(jobs.len(), 2);
        let nonces = jobs.values().map(|nonce| nonce.unwrap()).collect::<Vec<_>>();
        assert_ne!(nonces[0], nonces[1]);

        for nonce in nonces {
            let row = state::read_state(|s| s.pending_treasury_interest_mints[&nonce].clone());
            assert_eq!(row.tuple.destination, pool);
            assert_eq!(row.tuple.amount_e8s, 500);
            assert_eq!(row.tuple.op_nonce, nonce);
            assert_eq!(row.tuple.memo, nonce.to_be_bytes());
            assert_eq!(row.route, TreasuryInterestMintRoute::ThreePool);
        }
    }

    #[test]
    fn definite_sp_mint_rejection_retains_the_same_exact_journal_row() {
        configured_state();
        let collateral = Principal::from_slice(&[3]);
        let pool = Principal::from_slice(&[4]);
        state::mutate_state(|s| s.stability_pool_canister = Some(pool));
        let split = vec![InterestRecipient { destination: InterestDestination::StabilityPool, bps: 10_000 }];
        let nonce = reserve_stablecoin_sp_interest_shares(
            765, collateral, &split, Some(pool), 1_000,
        )[&0];
        state::mutate_state(|s| {
            let row = s.pending_treasury_interest_mints.get_mut(&nonce).unwrap();
            row.history_floor = Some(0);
            row.phase = TreasuryInterestMintPhase::MintPending;
        });
        let tuple = state::read_state(|s| s.pending_treasury_interest_mints[&nonce].tuple.clone());

        futures::executor::block_on(process_treasury_interest_mint_with_now(
            nonce,
            1_001,
            |operation| {
                assert_eq!(operation.tuple, tuple);
                async {
                    Err(TreasuryInterestMintFailure::Retry(
                        icrc_ledger_types::icrc1::transfer::TransferError::BadFee {
                            expected_fee: candid::Nat::from(10u64),
                        },
                    ))
                }
            },
            |_, _| async { panic!("a rejected mint must not notify the Stability Pool") },
        ));

        let retained = state::read_state(|s| s.pending_treasury_interest_mints[&nonce].clone());
        assert_eq!(retained.tuple, tuple);
        assert_eq!(retained.phase, TreasuryInterestMintPhase::MintPending);
    }

    #[test]
    fn full_sp_journal_retains_exact_share_in_stable_aggregate() {
        configured_state();
        let collateral = Principal::from_slice(&[3]);
        let pool = Principal::from_slice(&[4]);
        state::mutate_state(|s| {
            s.stability_pool_canister = Some(pool);
            let treasury = s.treasury_principal.unwrap();
            let row = crate::state::TreasuryInterestMintJournal {
                tuple: crate::state::BorrowMintTuple {
                    ledger: s.icusd_ledger_principal,
                    destination: treasury,
                    amount_e8s: 1,
                    memo: 1u128.to_be_bytes(),
                    created_at_time_ns: 1,
                    op_nonce: 1,
                },
                treasury,
                route: TreasuryInterestMintRoute::Treasury,
                phase: TreasuryInterestMintPhase::ReceiptRecoveryRequired,
                history_floor: None,
                expiry_proven: false,
                absence_scan: None,
            };
            for nonce in 1..=super::MAX_PENDING_TREASURY_INTEREST_MINTS {
                s.pending_treasury_interest_mints.insert(nonce as u128, row.clone());
            }
        });
        let split = vec![InterestRecipient { destination: InterestDestination::StabilityPool, bps: 10_000 }];
        let jobs = reserve_stablecoin_sp_interest_shares(765, collateral, &split, Some(pool), 1_000);
        assert!(jobs.is_empty());
        assert_eq!(state::read_state(|s| s.pending_stability_pool_interest_fallback_e8s[&collateral]), 765);
    }

    #[test]
    fn fallback_admission_atomically_pins_row_and_only_debits_admitted_amount() {
        configured_state();
        let collateral = Principal::from_slice(&[3]);
        let pool = Principal::from_slice(&[4]);
        state::mutate_state(|s| {
            s.pending_stability_pool_interest_fallback_e8s.insert(collateral, 765);
        });

        let nonce = admit_stability_pool_interest_fallback_at(
            collateral, ICUSD::from(500), pool, 1_000,
        ).expect("available journal capacity admits the exact share");
        let (row, remaining) = state::read_state(|s| (
            s.pending_treasury_interest_mints[&nonce].clone(),
            s.pending_stability_pool_interest_fallback_e8s[&collateral],
        ));
        assert_eq!(row.tuple.amount_e8s, 500);
        assert_eq!(row.tuple.destination, pool);
        assert_eq!(row.route, TreasuryInterestMintRoute::StabilityPool { collateral_type: collateral });
        assert_eq!(remaining, 265);
    }
}
