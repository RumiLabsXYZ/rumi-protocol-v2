//! Treasury inter-canister helpers.
//!
//! Mint/transfer protocol revenue to the treasury canister and call
//! `treasury.deposit()` for categorized bookkeeping.
//!
//! All treasury operations are **non-critical**: failures are logged but
//! never block user-facing operations (borrow, repay, liquidation).

use candid::{CandidType, Deserialize, Principal};
use ic_canister_log::log;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc3::archive::QueryArchiveFn;
use serde::Serialize;
use std::cell::RefCell;
use std::collections::BTreeSet;

use crate::logs::INFO;
use crate::management;
use crate::numeric::ICUSD;
use crate::state::read_state;

// Native ICP's legacy `query_blocks` interface encodes accounts as 32-byte
// AccountIdentifiers. These wire types intentionally match that ledger ABI.
#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeIcpTokens { e8s: u64 }
#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeIcpTimestamp { timestamp_nanos: u64 }
#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeIcpGetBlocksArgs { start: u64, length: u64 }
#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeIcpTransaction {
    memo: u64,
    icrc1_memo: Option<Vec<u8>>,
    operation: Option<NativeIcpOperation>,
    created_at_time: NativeIcpTimestamp,
}
#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeIcpBlock {
    parent_hash: Option<Vec<u8>>,
    transaction: NativeIcpTransaction,
    timestamp: NativeIcpTimestamp,
}
#[derive(CandidType, Deserialize, Clone, Debug)]
enum NativeIcpOperation {
    Burn { from: Vec<u8>, spender: Option<Vec<u8>>, amount: NativeIcpTokens },
    Mint { to: Vec<u8>, amount: NativeIcpTokens },
    Transfer { from: Vec<u8>, to: Vec<u8>, spender: Option<Vec<u8>>, amount: NativeIcpTokens, fee: NativeIcpTokens },
    Approve { from: Vec<u8>, spender: Vec<u8>, allowance_e8s: i128, allowance: NativeIcpTokens, expected_allowance: Option<NativeIcpTokens>, fee: NativeIcpTokens, expires_at: Option<NativeIcpTimestamp> },
}
#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeIcpBlockRange { blocks: Vec<NativeIcpBlock> }
#[derive(CandidType, Deserialize, Clone, Debug)]
enum NativeIcpQueryArchiveError {
    BadFirstBlockIndex { requested_index: u64, first_valid_index: u64 },
    Other { error_code: u64, error_message: String },
}
type NativeIcpQueryArchiveResult = Result<NativeIcpBlockRange, NativeIcpQueryArchiveError>;
#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeIcpArchivedBlocksRange {
    start: u64,
    length: u64,
    callback: QueryArchiveFn<NativeIcpGetBlocksArgs, NativeIcpQueryArchiveResult>,
}
#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeIcpQueryBlocksResponse {
    chain_length: u64,
    certificate: Option<Vec<u8>>,
    blocks: Vec<NativeIcpBlock>,
    first_block_index: u64,
    archived_blocks: Vec<NativeIcpArchivedBlocksRange>,
}


/// Verify a native ICP transfer against its exact persisted tuple. The caller
/// supplies the pinned ledger, accounts, amount, fee, memo, timestamp, and block.
pub async fn verify_native_icp_transfer_receipt(
    ledger: Principal,
    source: Principal,
    destination: Principal,
    amount: u64,
    fee: u64,
    memo: &[u8],
    created_at_time: u64,
    block_index: u64,
) -> Result<(), String> {
    let block = fetch_native_icp_block(ledger, block_index).await?;
    validate_native_icp_transfer_block(
        &block, ledger, source, destination, amount, fee, memo, created_at_time,
    ).await
}


async fn fetch_native_icp_block(ledger: Principal, block_index: u64) -> Result<NativeIcpBlock, String> {
    let request = NativeIcpGetBlocksArgs { start: block_index, length: 1 };
    let (response,): (NativeIcpQueryBlocksResponse,) = ic_cdk::call(
        ledger, "query_blocks", (request.clone(),),
    ).await.map_err(|(code, message)| format!("native ICP query_blocks failed: {code:?} {message}"))?;
    if response.blocks.len() > 1 {
        return Err(format!("native ICP ledger returned multiple direct blocks for {block_index}"));
    }

    if let Some(offset) = block_index.checked_sub(response.first_block_index)
        .and_then(|offset| usize::try_from(offset).ok())
        .filter(|offset| *offset < response.blocks.len())
    {
        return Ok(response.blocks[offset].clone());
    }

    let mut covering = response.archived_blocks.iter().filter(|archive| {
        archive.length > 0 && archive.start <= block_index
            && archive.start.checked_add(archive.length).is_some_and(|end| block_index < end)
    });
    let archive = covering.next()
        .ok_or_else(|| format!("native ICP ledger returned no block/archive descriptor for {block_index}"))?;
    if covering.next().is_some() {
        return Err(format!("native ICP ledger returned overlapping archive descriptors for {block_index}"));
    }
    let (result,): (NativeIcpQueryArchiveResult,) = ic_cdk::call(
        archive.callback.canister_id,
        &archive.callback.method,
        (request,),
    ).await.map_err(|(code, message)| format!("native ICP archive call failed: {code:?} {message}"))?;
    match result {
        Ok(range) if range.blocks.len() == 1 => Ok(range.blocks.into_iter().next().expect("one native ICP block")),
        Ok(_) => Err("native ICP archive did not return exactly one requested block".into()),
        Err(error) => Err(format!("native ICP archive rejected block lookup: {error:?}")),
    }
}

async fn validate_native_icp_transfer_block(
    block: &NativeIcpBlock,
    ledger: Principal,
    source: Principal,
    destination: Principal,
    amount: u64,
    fee: u64,
    memo: &[u8],
    created_at_time: u64,
) -> Result<(), String> {
    let operation = block.transaction.operation.as_ref().ok_or("native ICP block operation is missing")?;
    let NativeIcpOperation::Transfer { from, to, spender, amount: block_amount, fee: block_fee } = operation else {
        return Err("native ICP block operation is not Transfer".into());
    };
    if spender.is_some() {
        return Err("native ICP transfer block unexpectedly names a spender".into());
    }
    if from.len() != 32 || to.len() != 32 {
        return Err("native ICP block account identifier is not 32 bytes".into());
    }
    let (sender_id,): (Vec<u8>,) = ic_cdk::call(
        ledger,
        "account_identifier",
        (Account { owner: source, subaccount: None },),
    ).await.map_err(|(code, message)| format!("native ICP sender account_identifier failed: {code:?} {message}"))?;
    let (destination_id,): (Vec<u8>,) = ic_cdk::call(
        ledger,
        "account_identifier",
        (Account { owner: destination, subaccount: None },),
    ).await.map_err(|(code, message)| format!("native ICP destination account_identifier failed: {code:?} {message}"))?;
    if sender_id.len() != 32 || destination_id.len() != 32 || *from != sender_id || *to != destination_id {
        return Err("native ICP block source or destination account identifier differs from pinned tuple".into());
    }
    if block_amount.e8s != amount || block_fee.e8s != fee
        || block.transaction.icrc1_memo.as_deref() != Some(memo)
        || block.transaction.created_at_time.timestamp_nanos != created_at_time
    {
        return Err("native ICP block amount, fee, ICRC-1 memo, or created_at_time differs from pinned tuple".into());
    }
    Ok(())
}


/// Minimal native-ICP history projection for backend bot-cancel recovery.
/// Kept beside the wire decoder so cancellation uses the same legacy
/// `query_blocks` and archive ABI as Treasury's existing exact-receipt code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativeIcpCancelHistoryBlock {
    Transfer {
        from: Vec<u8>,
        to: Vec<u8>,
        spender: Option<Vec<u8>>,
        amount_e8s: u64,
        fee_e8s: u64,
        icrc1_memo: Option<Vec<u8>>,
        created_at_time_nanos: u64,
    },
    Other,
}

pub async fn native_icp_cancel_history_length(ledger: Principal) -> Result<u64, String> {
    let request = NativeIcpGetBlocksArgs { start: 0, length: 0 };
    let (response,): (NativeIcpQueryBlocksResponse,) = ic_cdk::call(
        ledger, "query_blocks", (request,),
    ).await.map_err(|(code, message)| {
        format!("native ICP query_blocks head request failed: {code:?} {message}")
    })?;
    Ok(response.chain_length)
}

pub async fn native_icp_cancel_history_block(
    ledger: Principal,
    block_index: u64,
) -> Result<NativeIcpCancelHistoryBlock, String> {
    let block = fetch_native_icp_block(ledger, block_index).await?;
    let operation = block.transaction.operation.ok_or("native ICP block operation is missing")?;
    match operation {
        NativeIcpOperation::Transfer { from, to, spender, amount, fee } => {
            if from.len() != 32 || to.len() != 32
                || spender.as_ref().is_some_and(|id| id.len() != 32)
            {
                return Err("native ICP transfer contains malformed account identifiers".into());
            }
            Ok(NativeIcpCancelHistoryBlock::Transfer {
                from,
                to,
                spender,
                amount_e8s: amount.e8s,
                fee_e8s: fee.e8s,
                icrc1_memo: block.transaction.icrc1_memo,
                created_at_time_nanos: block.transaction.created_at_time.timestamp_nanos,
            })
        }
        NativeIcpOperation::Burn { .. }
        | NativeIcpOperation::Mint { .. }
        | NativeIcpOperation::Approve { .. } => Ok(NativeIcpCancelHistoryBlock::Other),
    }
}


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
                    // The durable journal owns this share from before its
                    // first await; do not restore/re-split it on delivery error.
                    donate_to_three_pool(pool_canister, share_e8s).await;
                } else {
                    log!(INFO, "[treasury] WARNING: 3pool interest share ({} icUSD) has no target canister configured, sending to treasury instead", share_e8s);
                    if let Err(unsent) = mint_interest_to_treasury(share).await {
                        unminted_e8s = unminted_e8s.saturating_add(unsent.to_u64());
                    }
                }
            }
            crate::state::InterestDestination::Amm1 => {
                let (amm_opt, nonce) = crate::state::mutate_state(|s| {
                    s.amm1_donation_nonce += 1;
                    (s.amm1_canister, s.amm1_donation_nonce)
                });
                if let Some(amm_canister) = amm_opt {
                    donate_to_amm1(amm_canister, share_e8s, nonce).await;
                } else {
                    log!(INFO, "[treasury] WARNING: AMM1 interest share ({} icUSD) has no target canister configured, sending to treasury instead", share_e8s);
                    if let Err(unsent) = mint_interest_to_treasury(share).await {
                        unminted_e8s = unminted_e8s.saturating_add(unsent.to_u64());
                    }
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
                    donate_to_three_pool(pool_canister, share_e8s).await;
                } else {
                    // Fallback: mint icUSD to treasury (same as distribute_interest)
                    log!(INFO, "[treasury] WARNING: 3pool interest share ({} icUSD) has no target canister, sending to treasury instead", share_e8s);
                    let _ = mint_interest_to_treasury(ICUSD::from(share_e8s)).await;
                }
            }
            crate::state::InterestDestination::Amm1 => {
                let (amm_opt, nonce) = crate::state::mutate_state(|s| {
                    s.amm1_donation_nonce += 1;
                    (s.amm1_canister, s.amm1_donation_nonce)
                });
                if let Some(amm_canister) = amm_opt {
                    donate_to_amm1(amm_canister, share_e8s, nonce).await;
                } else {
                    log!(INFO, "[treasury] WARNING: AMM1 stablecoin interest share ({} icUSD) has no target canister; routing to treasury", share_e8s);
                    let pool_icusd = ICUSD::from(share_e8s);
                    let _ = mint_interest_to_treasury(pool_icusd).await;
                }
            }
        }
    }
}

/// Persist before mint, then advance a two-phase donation with one nonce for
/// both the icUSD ledger tuple and the 3pool's permanent receipt.
async fn donate_to_three_pool(pool_canister: Principal, amount_e8s: u64) {
    if amount_e8s == 0 {
        return;
    }
    let op_nonce = crate::state::mutate_state(|s| {
        let nonce = s.next_op_nonce();
        let ledger = s.icusd_ledger_principal;
        s.pending_three_pool_donations.insert(
            nonce,
            crate::state::PendingThreePoolDonation {
                pool: pool_canister,
                ledger,
                amount_e8s,
                phase: crate::state::ThreePoolDonationPhase::MintPending,
            },
        );
        nonce
    });
    process_pending_three_pool_donation(op_nonce).await;
}

/// Bounded round-robin retry of pending 3pool donations. Existing persisted
/// rows own their amount and are never split into a second operation.
pub async fn flush_pending_three_pool_donations() {
    const LIMIT: usize = 8;
    let rows = crate::state::mutate_state(|s| {
        use std::ops::Bound::{Excluded, Unbounded};
        let cursor = s.three_pool_donation_retry_cursor;
        let mut selected: Vec<_> = if let Some(cursor) = cursor {
            s.pending_three_pool_donations
                .range((Excluded(cursor), Unbounded))
                .take(LIMIT)
                .map(|(id, _)| *id)
                .collect()
        } else {
            Vec::new()
        };
        if selected.len() < LIMIT {
            selected.extend(
                s.pending_three_pool_donations
                    .range(..=cursor.unwrap_or(u128::MAX))
                    .take(LIMIT - selected.len())
                    .map(|(id, _)| *id),
            );
        }
        if let Some(last) = selected.last() {
            s.three_pool_donation_retry_cursor = Some(*last);
        }
        selected
    });
    for id in rows {
        process_pending_three_pool_donation(id).await;
    }
}

async fn process_pending_three_pool_donation(op_nonce: u128) {
    let Some(mut donation) = crate::state::read_state(|s| {
        s.pending_three_pool_donations.get(&op_nonce).cloned()
    }) else {
        return;
    };
    let mint_block = match donation.phase {
        crate::state::ThreePoolDonationPhase::MintPending => {
            let to = icrc_ledger_types::icrc1::account::Account {
                owner: donation.pool,
                subaccount: None,
            };
            match crate::management::mint_icusd_with_nonce(
                donation.ledger,
                ICUSD::from(donation.amount_e8s),
                to,
                op_nonce,
            )
            .await
            {
                Ok(block) => {
                    let phase = crate::state::ThreePoolDonationPhase::MintedAwaitingAck {
                        mint_block: block,
                    };
                    donation.phase = phase.clone();
                    crate::state::mutate_state(|s| {
                        if let Some(row) = s.pending_three_pool_donations.get_mut(&op_nonce) {
                            row.phase = phase;
                        }
                    });
                    block
                }
                Err(error) => {
                    log!(INFO, "[treasury] 3pool donation mint remains pending (nonce {}): {:?}", op_nonce, error);
                    return;
                }
            }
        }
        crate::state::ThreePoolDonationPhase::MintedAwaitingAck { mint_block } => mint_block,
    };
    let result: Result<(Result<(), ThreePoolDonateError>,), _> = ic_cdk::call(
        donation.pool,
        "receive_donation_with_id",
        (candid::Nat::from(op_nonce), 0u8, candid::Nat::from(donation.amount_e8s)),
    )
    .await;
    match result {
        Ok((Ok(()),)) => {
            crate::state::mutate_state(|s| {
                s.pending_three_pool_donations.remove(&op_nonce);
            });
            log!(INFO, "[treasury] 3pool acknowledged donation nonce {} (mint block {})", op_nonce, mint_block);
        }
        Ok((Err(error),)) => log!(INFO, "[treasury] 3pool donation nonce {} remains pending acknowledgment: {:?}", op_nonce, error),
        Err((code, message)) => log!(INFO, "[treasury] 3pool donation nonce {} acknowledgment failed: {:?} {}", op_nonce, code, message),
    }
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
    TransferFailed {
        token: String,
        reason: String,
    },
    Unauthorized,
    MathOverflow,
    InvariantNotConverged,
    PoolPaused,
    DonationIntentConflict,
}

/// Persist the donation and both operation identities before invoking either
/// the ledger or AMM. A missing pool configuration is held without minting.
async fn donate_to_amm1(amm_canister: Principal, amount_e8s: u64, notify_nonce: u64) {
    if amount_e8s == 0 {
        return;
    }
    let configured = read_state(|s| {
        (s.amm1_canister == Some(amm_canister))
            .then(|| s.amm1_pool_id.clone().map(|pool| (pool, s.icusd_ledger_principal)))
            .flatten()
    });
    let (ledger, pool_id, reward_subaccount, mint_op_nonce, phase) =
        if let Some((pool_id, ledger)) = configured {
            let nonce = crate::state::mutate_state(|s| s.next_op_nonce());
            (
                Some(ledger),
                Some(pool_id.clone()),
                Some(compute_amm_reward_subaccount(&pool_id)),
                Some(nonce),
                crate::state::AmmDonationPhase::MintPending,
            )
        } else {
            (None, None, None, None, crate::state::AmmDonationPhase::AwaitingPoolConfig)
        };
    crate::state::mutate_state(|s| {
        s.pending_amm_donations.entry(notify_nonce).or_insert(
            crate::state::PendingAmmDonation {
                amm_canister,
                ledger,
                amount_e8s,
                notify_nonce,
                mint_op_nonce,
                pool_id,
                reward_subaccount,
                phase,
            },
        );
    });
    process_pending_amm_donation(notify_nonce).await;
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
/// Only used for deserialization; any Err leaves the durable row pending.
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

async fn process_pending_amm_donation(notify_nonce: u64) {
    let Some(mut donation) =
        crate::state::read_state(|s| s.pending_amm_donations.get(&notify_nonce).cloned())
    else {
        return;
    };
    if matches!(&donation.phase, crate::state::AmmDonationPhase::AwaitingPoolConfig) {
        let Some((ledger, pool_id, op_nonce)) = crate::state::mutate_state(|s| {
            if s.amm1_canister != Some(donation.amm_canister) {
                return None;
            }
            let pool_id = s.amm1_pool_id.clone()?;
            let ledger = s.icusd_ledger_principal;
            let op_nonce = s.next_op_nonce();
            let row = s.pending_amm_donations.get_mut(&notify_nonce)?;
            row.ledger = Some(ledger);
            row.pool_id = Some(pool_id.clone());
            row.reward_subaccount = Some(compute_amm_reward_subaccount(&pool_id));
            row.mint_op_nonce = Some(op_nonce);
            row.phase = crate::state::AmmDonationPhase::MintPending;
            Some((ledger, pool_id, op_nonce))
        }) else {
            return;
        };
        donation.ledger = Some(ledger);
        donation.pool_id = Some(pool_id.clone());
        donation.reward_subaccount = Some(compute_amm_reward_subaccount(&pool_id));
        donation.mint_op_nonce = Some(op_nonce);
        donation.phase = crate::state::AmmDonationPhase::MintPending;
    }

    if matches!(&donation.phase, crate::state::AmmDonationPhase::MintPending) {
        let (Some(ledger), Some(op_nonce), Some(subaccount), Some(pool_id)) = (
            donation.ledger,
            donation.mint_op_nonce,
            donation.reward_subaccount,
            donation.pool_id.clone(),
        ) else {
            return;
        };
        let destination = Account {
            owner: donation.amm_canister,
            subaccount: Some(subaccount),
        };
        match crate::management::transfer_idempotent(
            ledger,
            None,
            destination,
            donation.amount_e8s as u128,
            op_nonce,
            None,
        )
        .await
        {
            Ok(block_index) => {
                let phase = crate::state::AmmDonationPhase::NotifyPending { mint_block: block_index };
                crate::state::mutate_state(|s| {
                    if let Some(row) = s.pending_amm_donations.get_mut(&notify_nonce) {
                        row.phase = phase;
                    }
                });
                donation.phase = crate::state::AmmDonationPhase::NotifyPending { mint_block: block_index };
            }
            Err(icrc_ledger_types::icrc1::transfer::TransferError::TooOld) => {
                crate::state::mutate_state(|s| {
                    if let Some(row) = s.pending_amm_donations.get_mut(&notify_nonce) {
                        row.phase = crate::state::AmmDonationPhase::MintHeldAfterTooOld;
                    }
                });
                log!(INFO, "[treasury] AMM donation {} held: exact mint tuple is TooOld; refusing a fresh mint identity", notify_nonce);
                return;
            }
            Err(error) => {
                log!(INFO, "[treasury] AMM donation {} mint remains pending: {:?}", notify_nonce, error);
                return;
            }
        }
        donation.pool_id = Some(pool_id);
    }

    let crate::state::AmmDonationPhase::NotifyPending { .. } = donation.phase else {
        return;
    };
    let (Some(pool_id), Some(_ledger), Some(_nonce)) =
        (donation.pool_id.clone(), donation.ledger, donation.mint_op_nonce)
    else {
        return;
    };
    let response: Result<(Result<(), AmmDonateError>,), _> = ic_cdk::call(
        donation.amm_canister,
        "notify_reward_received",
        (pool_id, donation.amount_e8s as u128, donation.notify_nonce),
    )
    .await;
    match response {
        Ok((Ok(()),)) => {
            crate::state::mutate_state(|s| {
                s.pending_amm_donations.remove(&notify_nonce);
            });
        }
        Ok((Err(error),)) => log!(INFO, "[treasury] AMM donation {} acknowledgment remains pending: {:?}", notify_nonce, error),
        Err((code, message)) => log!(INFO, "[treasury] AMM donation {} acknowledgment failed: {:?} {}", notify_nonce, code, message),
    }
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

/// Retry a bounded round-robin prefix of durable AMM donation journals.
/// Legacy tuple rows are intentionally not replayed because their mint
/// outcome and destination cannot be reconstructed safely.
pub async fn flush_pending_amm1_donations() {
    const LIMIT: usize = 8;
    let rows = crate::state::mutate_state(|s| {
        use std::ops::Bound::{Excluded, Unbounded};
        let cursor = s.amm_donation_retry_cursor;
        let mut selected: Vec<_> = if let Some(cursor) = cursor {
            s.pending_amm_donations
                .range((Excluded(cursor), Unbounded))
                .take(LIMIT)
                .map(|(id, _)| *id)
                .collect()
        } else {
            Vec::new()
        };
        if selected.len() < LIMIT {
            selected.extend(
                s.pending_amm_donations
                    .range(..=cursor.unwrap_or(u64::MAX))
                    .take(LIMIT - selected.len())
                    .map(|(id, _)| *id),
            );
        }
        if let Some(last) = selected.last() {
            s.amm_donation_retry_cursor = Some(*last);
        }
        selected
    });
    for id in rows {
        process_pending_amm_donation(id).await;
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


// Native ICP's legacy `query_blocks` interface encodes accounts as 32-byte
// AccountIdentifiers. These wire types intentionally match that ledger ABI.
#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeIcpTokens { e8s: u64 }
#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeIcpTimestamp { timestamp_nanos: u64 }
#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeIcpGetBlocksArgs { start: u64, length: u64 }
#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeIcpTransaction {
    memo: u64,
    icrc1_memo: Option<Vec<u8>>,
    operation: Option<NativeIcpOperation>,
    created_at_time: NativeIcpTimestamp,
}
#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeIcpBlock {
    parent_hash: Option<Vec<u8>>,
    transaction: NativeIcpTransaction,
    timestamp: NativeIcpTimestamp,
}
#[derive(CandidType, Deserialize, Clone, Debug)]
enum NativeIcpOperation {
    Burn { from: Vec<u8>, spender: Option<Vec<u8>>, amount: NativeIcpTokens },
    Mint { to: Vec<u8>, amount: NativeIcpTokens },
    Transfer { from: Vec<u8>, to: Vec<u8>, spender: Option<Vec<u8>>, amount: NativeIcpTokens, fee: NativeIcpTokens },
    Approve { from: Vec<u8>, spender: Vec<u8>, allowance_e8s: i128, allowance: NativeIcpTokens, expected_allowance: Option<NativeIcpTokens>, fee: NativeIcpTokens, expires_at: Option<NativeIcpTimestamp> },
}
#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeIcpBlockRange { blocks: Vec<NativeIcpBlock> }
#[derive(CandidType, Deserialize, Clone, Debug)]
enum NativeIcpQueryArchiveError {
    BadFirstBlockIndex { requested_index: u64, first_valid_index: u64 },
    Other { error_code: u64, error_message: String },
}
type NativeIcpQueryArchiveResult = Result<NativeIcpBlockRange, NativeIcpQueryArchiveError>;
#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeIcpArchivedBlocksRange {
    start: u64,
    length: u64,
    callback: QueryArchiveFn<NativeIcpGetBlocksArgs, NativeIcpQueryArchiveResult>,
}
#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeIcpQueryBlocksResponse {
    chain_length: u64,
    certificate: Option<Vec<u8>>,
    blocks: Vec<NativeIcpBlock>,
    first_block_index: u64,
    archived_blocks: Vec<NativeIcpArchivedBlocksRange>,
}


pub async fn verify_native_icp_transfer_receipt(
    ledger: Principal,
    source: Principal,
    destination: Principal,
    amount: u64,
    fee: u64,
    memo: &[u8],
    created_at_time: u64,
    block_index: u64,
) -> Result<(), String> {
    let block = fetch_native_icp_block(ledger, block_index).await?;
    validate_native_icp_transfer_block(
        &block, ledger, source, destination, amount, fee, memo, created_at_time,
    ).await
}

async fn fetch_native_icp_block(ledger: Principal, block_index: u64) -> Result<NativeIcpBlock, String> {
    let request = NativeIcpGetBlocksArgs { start: block_index, length: 1 };
    let (response,): (NativeIcpQueryBlocksResponse,) = ic_cdk::call(
        ledger, "query_blocks", (request.clone(),),
    ).await.map_err(|(code, message)| format!("native ICP query_blocks failed: {code:?} {message}"))?;
    if response.blocks.len() > 1 {
        return Err(format!("native ICP ledger returned multiple direct blocks for {block_index}"));
    }

    if let Some(offset) = block_index.checked_sub(response.first_block_index)
        .and_then(|offset| usize::try_from(offset).ok())
        .filter(|offset| *offset < response.blocks.len())
    {
        return Ok(response.blocks[offset].clone());
    }

    let mut covering = response.archived_blocks.iter().filter(|archive| {
        archive.length > 0 && archive.start <= block_index
            && archive.start.checked_add(archive.length).is_some_and(|end| block_index < end)
    });
    let archive = covering.next()
        .ok_or_else(|| format!("native ICP ledger returned no block/archive descriptor for {block_index}"))?;
    if covering.next().is_some() {
        return Err(format!("native ICP ledger returned overlapping archive descriptors for {block_index}"));
    }
    let (result,): (NativeIcpQueryArchiveResult,) = ic_cdk::call(
        archive.callback.canister_id,
        &archive.callback.method,
        (request,),
    ).await.map_err(|(code, message)| format!("native ICP archive call failed: {code:?} {message}"))?;
    match result {
        Ok(range) if range.blocks.len() == 1 => Ok(range.blocks.into_iter().next().expect("one native ICP block")),
        Ok(_) => Err("native ICP archive did not return exactly one requested block".into()),
        Err(error) => Err(format!("native ICP archive rejected block lookup: {error:?}")),
    }
}

async fn validate_native_icp_transfer_block(
    block: &NativeIcpBlock,
    ledger: Principal,
    source: Principal,
    destination: Principal,
    amount: u64,
    fee: u64,
    memo: &[u8],
    created_at_time: u64,
) -> Result<(), String> {
    let operation = block.transaction.operation.as_ref().ok_or("native ICP block operation is missing")?;
    let NativeIcpOperation::Transfer { from, to, spender, amount: block_amount, fee: block_fee } = operation else {
        return Err("native ICP block operation is not Transfer".into());
    };
    if spender.is_some() {
        return Err("native ICP transfer block unexpectedly names a spender".into());
    }
    if from.len() != 32 || to.len() != 32 {
        return Err("native ICP block account identifier is not 32 bytes".into());
    }
    let (sender_id,): (Vec<u8>,) = ic_cdk::call(
        ledger,
        "account_identifier",
        (Account { owner: source, subaccount: None },),
    ).await.map_err(|(code, message)| format!("native ICP sender account_identifier failed: {code:?} {message}"))?;
    let (destination_id,): (Vec<u8>,) = ic_cdk::call(
        ledger,
        "account_identifier",
        (Account { owner: destination, subaccount: None },),
    ).await.map_err(|(code, message)| format!("native ICP destination account_identifier failed: {code:?} {message}"))?;
    if sender_id.len() != 32 || destination_id.len() != 32 || *from != sender_id || *to != destination_id {
        return Err("native ICP block source or destination account identifier differs from pinned tuple".into());
    }
    if block_amount.e8s != amount || block_fee.e8s != fee
        || block.transaction.icrc1_memo.as_deref() != Some(memo)
        || block.transaction.created_at_time.timestamp_nanos != created_at_time
    {
        return Err("native ICP block amount, fee, ICRC-1 memo, or created_at_time differs from pinned tuple".into());
    }
    Ok(())
}
