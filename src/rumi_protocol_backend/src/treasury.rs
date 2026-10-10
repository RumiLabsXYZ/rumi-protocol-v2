//! Treasury inter-canister helpers.
//!
//! Mint/transfer protocol revenue to the treasury canister and call
//! `treasury.deposit()` for categorized bookkeeping.
//!
//! All treasury operations are **non-critical**: failures are logged but
//! never block user-facing operations (borrow, repay, liquidation).

use candid::{CandidType, Deserialize, Principal};
use ic_canister_log::log;
use serde::Serialize;
use std::cell::RefCell;
use std::collections::BTreeSet;

use crate::logs::INFO;
use crate::management;
use crate::numeric::ICUSD;
use crate::state::read_state;

const BORROWING_FEE_ONCE_METHOD: &str = "deposit_borrowing_fee_once";

// ---------------------------------------------------------------------------
// Mirror types matching rumi_treasury::types (can't depend on cdylib crate)
// ---------------------------------------------------------------------------

/// Mirrors `rumi_treasury::types::DepositType`.
#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub enum DepositType {
    BorrowingFee,
    RedemptionFee,
    LiquidationFee,
    InterestRevenue,
}

/// Mirrors `rumi_treasury::types::AssetType`.
#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub enum AssetType {
    ICUSD,
    ICP,
    CKBTC,
    CKUSDT,
    CKUSDC,
}

/// The stable pool's Candid result is intentionally mirrored without taking a
/// dependency on its cdylib crate. `IDLValue` accepts every typed error arm;
/// only `Ok` acknowledges a post-mint receipt.
#[derive(CandidType, Deserialize)]
enum StabilityPoolInterestNotificationResult {
    Ok,
    Err(candid::IDLValue),
}

type InterestNotificationKey = (Principal, Principal, u64);

thread_local! {
    /// Prevents the inline post-mint delivery and timer flush from issuing the
    /// same inter-canister notification concurrently in one backend instance.
    static IN_FLIGHT_INTEREST_NOTIFICATIONS: RefCell<BTreeSet<InterestNotificationKey>> =
        RefCell::new(BTreeSet::new());
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

// ---------------------------------------------------------------------------
// Public helpers — mint/transfer + notify
// ---------------------------------------------------------------------------

/// Mint icUSD interest revenue to treasury and record the deposit.
///
/// Returns `Ok(())` when the icUSD mint itself succeeded — the
/// downstream `notify_treasury_deposit` call is bookkeeping and its
/// failure does not roll back the mint. Returns `Err(interest_share)`
/// when the mint did not happen (no treasury configured, or the ledger
/// call failed); the caller is expected to re-queue this amount via
/// the snapshot-then-decrement restore path so revenue is not lost.
pub async fn mint_interest_to_treasury(interest_share: ICUSD) -> Result<(), ICUSD> {
    if interest_share.0 == 0 {
        return Ok(());
    }
    let treasury = read_state(|s| s.treasury_principal);
    let Some(tp) = treasury else {
        log!(
            INFO,
            "[treasury] WARNING: no treasury principal configured; {} icUSD interest unminted",
            interest_share.to_u64()
        );
        return Err(interest_share);
    };
    match management::mint_icusd(interest_share, tp).await {
        Ok(block_index) => {
            log!(
                INFO,
                "[treasury] Minted {} icUSD interest revenue (block {})",
                interest_share.to_u64(),
                block_index
            );
            let _ = notify_treasury_deposit(
                tp,
                DepositType::InterestRevenue,
                AssetType::ICUSD,
                interest_share.to_u64(),
                block_index,
            )
            .await;
            Ok(())
        }
        Err(e) => {
            log!(INFO, "[treasury] WARNING: interest mint failed: {:?}", e);
            Err(interest_share)
        }
    }
}

/// Mint icUSD interest revenue to the stability pool canister.
/// The stability pool distributes this pro-rata to depositors.
///
/// `collateral_type` identifies which collateral's vault generated this interest.
/// The pool uses it to exclude depositors who opted out of that collateral.
///
/// Returns `Ok(())` when the icUSD mint succeeded — the post-mint
/// `receive_interest_revenue` notification is bookkeeping and its
/// failure does not roll back the mint. Returns `Err(interest_share)`
/// when the mint did not happen (no pool configured, or the ledger
/// call failed); the caller re-queues this via the snapshot-then-
/// decrement restore path.
pub async fn mint_interest_to_stability_pool(
    interest_share: ICUSD,
    collateral_type: Principal,
) -> Result<(), ICUSD> {
    if interest_share.0 == 0 {
        return Ok(());
    }
    let (stability_pool, icusd_ledger) =
        read_state(|s| (s.stability_pool_canister, s.icusd_ledger_principal));
    let Some(pool_principal) = stability_pool else {
        log!(
            INFO,
            "[treasury] WARNING: no stability pool configured; {} icUSD interest unminted",
            interest_share.to_u64()
        );
        return Err(interest_share);
    };
    match management::mint_icusd(interest_share, pool_principal).await {
        Ok(block_index) => {
            log!(
                INFO,
                "[treasury] Minted {} icUSD interest to stability pool (block {})",
                interest_share.to_u64(),
                block_index
            );

            let notification = crate::state::PendingStabilityPoolInterestNotification {
                pool_principal,
                token_ledger: icusd_ledger,
                amount_e8s: interest_share.to_u64(),
                collateral_type,
                source_mint_block: block_index,
                receipt_protocol_version: Some(1),
            };
            // Persist BEFORE the await. A failed call must be retried as a
            // notification, never by minting a second copy of the interest.
            crate::state::mutate_state(|s| {
                s.pending_stability_pool_interest_notifications
                    .insert(block_index, notification.clone());
            });
            if notification_delivery_acknowledged(
                &notification,
                deliver_stability_pool_interest_notification(&notification).await,
            ) {
                crate::state::mutate_state(|s| {
                    s.pending_stability_pool_interest_notifications
                        .remove(&block_index);
                });
            }
            Ok(())
        }
        Err(e) => {
            log!(
                INFO,
                "[treasury] WARNING: stability pool interest mint failed ({} icUSD): {:?}",
                interest_share.to_u64(),
                e
            );
            Err(interest_share)
        }
    }
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
        InterestNotificationGuard,
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
    if interest.0 == 0 {
        return ICUSD::new(0);
    }

    let (split, three_pool) = read_state(|s| (s.interest_split.clone(), s.three_pool_canister));

    let total_e8s = interest.to_u64();
    let mut unminted_e8s: u64 = 0;

    for recipient in &split {
        let share_e8s = ((total_e8s as u128) * (recipient.bps as u128) / 10_000) as u64;
        if share_e8s == 0 {
            continue;
        }
        let share = ICUSD::from(share_e8s);

        match &recipient.destination {
            crate::state::InterestDestination::StabilityPool => {
                if let Err(unsent) = mint_interest_to_stability_pool(share, collateral_type).await {
                    unminted_e8s = unminted_e8s.saturating_add(unsent.to_u64());
                }
            }
            crate::state::InterestDestination::Treasury => {
                if let Err(unsent) = mint_interest_to_treasury(share).await {
                    unminted_e8s = unminted_e8s.saturating_add(unsent.to_u64());
                }
            }
            crate::state::InterestDestination::ThreePool => {
                if let Some(pool_canister) = three_pool {
                    if let Err(unsent_e8s) = donate_to_three_pool(pool_canister, share_e8s).await {
                        unminted_e8s = unminted_e8s.saturating_add(unsent_e8s);
                    }
                } else {
                    log!(INFO, "[treasury] WARNING: 3pool interest share ({} icUSD) has no target canister configured, sending to treasury instead", share_e8s);
                    if let Err(unsent) = mint_interest_to_treasury(share).await {
                        unminted_e8s = unminted_e8s.saturating_add(unsent.to_u64());
                    }
                }
            }
            crate::state::InterestDestination::Amm1 => {
                match start_amm1_donation(share_e8s) {
                    Ok(Some(notify_nonce)) => {
                        process_amm1_donation(notify_nonce).await;
                        // Keep this outside pending_interest_for_pools: that
                        // bucket would re-split the obligation on retry.
                    }
                    Ok(None) => {
                        log!(INFO, "[treasury] WARNING: AMM1 interest share ({} icUSD) has no target canister configured, sending to treasury instead", share_e8s);
                        if let Err(unsent) = mint_interest_to_treasury(share).await {
                            unminted_e8s = unminted_e8s.saturating_add(unsent.to_u64());
                        }
                    }
                    Err(()) => log!(INFO, "[treasury] AMM1 interest share ({}) held: AMM1 pool ID is missing", share_e8s),
                }
            }
        }
    }
    ICUSD::from(unminted_e8s)
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
) {
    if interest_e8s == 0 {
        return;
    }

    let (split, three_pool) = read_state(|s| (s.interest_split.clone(), s.three_pool_canister));

    for recipient in &split {
        let share_e8s = ((interest_e8s as u128) * (recipient.bps as u128) / 10_000) as u64;
        if share_e8s == 0 {
            continue;
        }

        match &recipient.destination {
            crate::state::InterestDestination::StabilityPool => {
                let pool_icusd = ICUSD::from(share_e8s);
                // Stablecoin path keeps legacy "log + drop" on mint failure.
                // No bucket to re-queue against (the funds live in
                // collateral reserves, not a pending field). Audit
                // INT-002 covered only the icUSD-denominated path.
                let _ = mint_interest_to_stability_pool(pool_icusd, collateral_type).await;
            }
            crate::state::InterestDestination::Treasury => {
                // Transfer stablecoins (not icUSD) to treasury
                let treasury_e6s = share_e8s / 100; // e8s → e6s
                if treasury_e6s > 0 {
                    let (treasury_principal, stable_ledger) = read_state(|s| {
                        let ledger = match token_type {
                            crate::StableTokenType::CKUSDT => s.ckusdt_ledger_principal,
                            crate::StableTokenType::CKUSDC => s.ckusdc_ledger_principal,
                        };
                        (s.treasury_principal, ledger)
                    });
                    if let (Some(tp), Some(ledger)) = (treasury_principal, stable_ledger) {
                        match crate::management::transfer_collateral(treasury_e6s, tp, ledger).await
                        {
                            Ok(block_index) => {
                                log!(INFO, "[treasury] Transferred {} {:?} interest to treasury (block {})", treasury_e6s, token_type, block_index);
                                let asset_type = match token_type {
                                    crate::StableTokenType::CKUSDT => AssetType::CKUSDT,
                                    crate::StableTokenType::CKUSDC => AssetType::CKUSDC,
                                };
                                let _ = notify_treasury_deposit(
                                    tp,
                                    DepositType::InterestRevenue,
                                    asset_type,
                                    treasury_e6s,
                                    block_index,
                                )
                                .await;
                            }
                            Err(e) => log!(
                                INFO,
                                "[treasury] WARNING: stablecoin interest transfer failed: {:?}",
                                e
                            ),
                        }
                    }
                }
            }
            crate::state::InterestDestination::ThreePool => {
                if let Some(pool_canister) = three_pool {
                    let _ = donate_to_three_pool(pool_canister, share_e8s).await;
                } else {
                    // Fallback: mint icUSD to treasury (same as distribute_interest)
                    log!(INFO, "[treasury] WARNING: 3pool interest share ({} icUSD) has no target canister, sending to treasury instead", share_e8s);
                    let _ = mint_interest_to_treasury(ICUSD::from(share_e8s)).await;
                }
            }
            crate::state::InterestDestination::Amm1 => {
                match start_amm1_donation(share_e8s) {
                    Ok(Some(notify_nonce)) => process_amm1_donation(notify_nonce).await,
                    Ok(None) => {
                        log!(INFO, "[treasury] WARNING: AMM1 stablecoin interest share ({} icUSD) has no target canister; routing to treasury", share_e8s);
                        let pool_icusd = ICUSD::from(share_e8s);
                        let _ = mint_interest_to_treasury(pool_icusd).await;
                    }
                    Err(()) => log!(INFO, "[treasury] AMM1 stablecoin interest share ({}) held: AMM1 pool ID is missing", share_e8s),
                }
            }
        }
    }
}

/// Mint icUSD directly to the 3pool canister, then call `receive_donation`
/// so the pool updates its internal balances.
/// Non-critical: failures are logged but don't block protocol operations.
///
/// Returns `Ok(())` when the icUSD mint succeeded — the subsequent
/// `receive_donation` notification is bookkeeping and its failure does
/// not roll back the mint. Returns `Err(amount_e8s)` when the mint
/// itself failed; the caller re-queues this via the snapshot-then-
/// decrement restore path.
async fn donate_to_three_pool(pool_canister: Principal, amount_e8s: u64) -> Result<(), u64> {
    // 1. Mint icUSD directly to the 3pool canister
    let icusd = ICUSD::from(amount_e8s);
    match crate::management::mint_icusd(icusd, pool_canister).await {
        Ok(block_index) => {
            log!(
                INFO,
                "[treasury] Minted {} icUSD to 3pool for donation (block {})",
                amount_e8s,
                block_index
            );
        }
        Err(e) => {
            log!(
                INFO,
                "[treasury] WARNING: 3pool donation mint failed: {:?}",
                e
            );
            return Err(amount_e8s);
        }
    }

    // 2. Call receive_donation(0, amount) so 3pool updates internal balances
    let donate_amount = candid::Nat::from(amount_e8s);
    let result: Result<(Result<(), ThreePoolDonateError>,), _> =
        ic_cdk::call(pool_canister, "receive_donation", (0u8, donate_amount)).await;
    match result {
        Ok((Ok(()),)) => {
            log!(
                INFO,
                "[treasury] 3pool acknowledged donation of {} icUSD",
                amount_e8s
            );
        }
        Ok((Err(e),)) => {
            log!(
                INFO,
                "[treasury] WARNING: 3pool receive_donation returned error: {:?}",
                e
            );
        }
        Err((code, msg)) => {
            log!(
                INFO,
                "[treasury] WARNING: 3pool receive_donation call failed: {:?} {}",
                code,
                msg
            );
        }
    }
    Ok(())
}

/// Mirror of the 3pool ThreePoolError for the donate response.
/// We only need this for deserialization of the Result.
#[derive(CandidType, Deserialize, Clone, Debug)]
enum ThreePoolDonateError {
    InsufficientOutput {
        expected_min: candid::Nat,
        actual: candid::Nat,
    },
    InsufficientLiquidity,
    InvalidCoinIndex,
    ZeroAmount,
    PoolEmpty,
    SlippageExceeded,
    DepositConcentrationLimitExceeded,
    TransferFailed {
        token: String,
        reason: String,
    },
    Unauthorized,
    MathOverflow,
    InvariantNotConverged,
    PoolPaused,
}

/// Record the exact transfer and notification tuple before any await. None
/// means AMM1 is not configured and the existing treasury fallback applies;
/// Err means AMM1 exists but the pool ID is missing, so the share is held.
fn start_amm1_donation(amount_e8s: u64) -> Result<Option<u64>, ()> {
    start_amm1_donation_at(amount_e8s, ic_cdk::api::time())
}

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
    let treasury = read_state(|s| s.treasury_principal);
    let Some(tp) = treasury else {
        return;
    };
    // Atomic snapshot+zero before the await.
    let snapshot = crate::state::mutate_state(|s| s.take_pending_treasury_interest());
    if snapshot.0 == 0 {
        return;
    }
    match management::mint_icusd(snapshot, tp).await {
        Ok(block_index) => {
            log!(
                INFO,
                "[treasury] Drained {} pending interest (block {})",
                snapshot.to_u64(),
                block_index
            );
            let _ = notify_treasury_deposit(
                tp,
                DepositType::InterestRevenue,
                AssetType::ICUSD,
                snapshot.to_u64(),
                block_index,
            )
            .await;
        }
        Err(e) => {
            crate::state::mutate_state(|s| s.restore_pending_treasury_interest(snapshot));
            log!(
                INFO,
                "[treasury] CRITICAL: pending interest drain failed, re-queued {} icUSD: {:?}",
                snapshot.to_u64(),
                e
            );
        }
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
