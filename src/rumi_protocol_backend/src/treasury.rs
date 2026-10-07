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
use crate::state::{read_state, PendingTreasuryPayment, TreasuryPaymentKind, TreasuryPaymentPhase};

// The supported ICP/ck* ICRC ledgers use a 24-hour transaction deduplication
// window. Stop retrying one hour early so clock skew and timer delay cannot
// push an exact-tuple retry across that boundary.
const TREASURY_TRANSFER_RETRY_WINDOW_NS: u64 = 23 * 60 * 60 * 1_000_000_000;
const TREASURY_TRANSFER_MAX_ATTEMPTS: u8 = 6;
const TREASURY_TRANSFER_RETRY_DELAY_NS: u64 = 30 * 1_000_000_000;

// Native ICP's legacy `query_blocks` interface encodes accounts as 32-byte
// AccountIdentifiers. These wire types intentionally match that ledger ABI.
#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeIcpTokens {
    e8s: u64,
}
#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeIcpTimestamp {
    timestamp_nanos: u64,
}
#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeIcpGetBlocksArgs {
    start: u64,
    length: u64,
}
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
    Burn {
        from: Vec<u8>,
        spender: Option<Vec<u8>>,
        amount: NativeIcpTokens,
    },
    Mint {
        to: Vec<u8>,
        amount: NativeIcpTokens,
    },
    Transfer {
        from: Vec<u8>,
        to: Vec<u8>,
        spender: Option<Vec<u8>>,
        amount: NativeIcpTokens,
        fee: NativeIcpTokens,
    },
    Approve {
        from: Vec<u8>,
        spender: Vec<u8>,
        allowance_e8s: i128,
        allowance: NativeIcpTokens,
        expected_allowance: Option<NativeIcpTokens>,
        fee: NativeIcpTokens,
        expires_at: Option<NativeIcpTimestamp>,
    },
}
#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeIcpBlockRange {
    blocks: Vec<NativeIcpBlock>,
}
#[derive(CandidType, Deserialize, Clone, Debug)]
enum NativeIcpQueryArchiveError {
    BadFirstBlockIndex {
        requested_index: u64,
        first_valid_index: u64,
    },
    Other {
        error_code: u64,
        error_message: String,
    },
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
    if validate_native_icp_transfer_block(
        &block,
        ledger,
        source,
        destination,
        amount,
        fee,
        memo,
        created_at_time,
    )
    .await?
    {
        Ok(())
    } else {
        Err("native ICP block does not match the exact transfer tuple".into())
    }
}

/// Verify a direct native-ICP ICRC-1 transfer, including source subaccounts.
/// `query_blocks` stores account identifiers, so resolve both exact accounts
/// through the ledger before matching the persisted sweep tuple.
pub async fn verify_native_icp_direct_account_transfer_receipt(
    ledger: Principal,
    source: &Account,
    destination: &Account,
    amount: u64,
    fee: u64,
    memo: &[u8],
    created_at_time: u64,
    block_index: u64,
) -> Result<(), String> {
    let block = fetch_native_icp_block(ledger, block_index).await?;
    let (source_id,): (Vec<u8>,) = ic_cdk::call(ledger, "account_identifier", (source.clone(),))
        .await
        .map_err(|(code, message)| format!("native ICP source account_identifier failed: {code:?} {message}"))?;
    let (destination_id,): (Vec<u8>,) = ic_cdk::call(ledger, "account_identifier", (destination.clone(),))
        .await
        .map_err(|(code, message)| format!("native ICP destination account_identifier failed: {code:?} {message}"))?;
    if source_id.len() != 32 || destination_id.len() != 32 {
        return Err("native ICP account_identifier returned malformed account bytes".into());
    }
    let Some(NativeIcpOperation::Transfer { from, to, spender, amount: actual_amount, fee: actual_fee }) = block.transaction.operation.as_ref() else {
        return Err("native ICP block is not a transfer".into());
    };
    if spender.is_some()
        || from != &source_id
        || to != &destination_id
        || actual_amount.e8s != amount
        || actual_fee.e8s != fee
        || block.transaction.icrc1_memo.as_deref() != Some(memo)
        || block.transaction.created_at_time.timestamp_nanos != created_at_time
    {
        return Err("native ICP block does not match the exact direct transfer tuple".into());
    }
    Ok(())
}

/// Scan helper that distinguishes a valid nonmatching block from an
/// unavailable or malformed history response. `true` is exact receipt proof.
pub async fn native_icp_transfer_history_matches(
    ledger: Principal,
    source: Principal,
    destination: Principal,
    amount: u64,
    fee: u64,
    memo: &[u8],
    created_at_time: u64,
    block_index: u64,
) -> Result<bool, String> {
    let block = fetch_native_icp_block(ledger, block_index).await?;
    validate_native_icp_transfer_block(
        &block,
        ledger,
        source,
        destination,
        amount,
        fee,
        memo,
        created_at_time,
    )
    .await
}

/// Verify an exact ICRC-2 transfer_from tuple against native ICP's legacy
/// `query_blocks` history. The native ledger includes ICRC-2 `spender`, ICRC-1
/// memo bytes, and `created_at_time`; all are required alongside the account
/// identifiers, amount, and fee before inbound collateral can be credited.
pub async fn verify_native_icp_transfer_from_receipt(
    tuple: &crate::SpLiquidationStablePullTuple,
    block_index: u64,
) -> Result<(), String> {
    let block = fetch_native_icp_block(tuple.ledger, block_index).await?;
    let source_id = native_icp_account_identifier(tuple.ledger, &tuple.from).await?;
    let spender_id = native_icp_account_identifier(tuple.ledger, &tuple.spender).await?;
    let destination_id = native_icp_account_identifier(tuple.ledger, &tuple.to).await?;
    validate_native_icp_transfer_from_block(
        &block,
        &source_id,
        &spender_id,
        &destination_id,
        tuple.amount_raw,
        tuple.fee_raw,
        &tuple.memo,
        tuple.created_at_time_ns,
    )
}

async fn native_icp_account_identifier(
    ledger: Principal,
    account: &Account,
) -> Result<Vec<u8>, String> {
    let (identifier,): (Vec<u8>,) =
        ic_cdk::call(ledger, "account_identifier", (account.clone(),))
        .await
        .map_err(|(code, message)| {
            format!("native ICP account_identifier failed: {code:?} {message}")
        })?;
    if identifier.len() != 32 {
        return Err("native ICP account_identifier returned malformed bytes".to_string());
    }
    Ok(identifier)
}

async fn fetch_native_icp_block(
    ledger: Principal,
    block_index: u64,
) -> Result<NativeIcpBlock, String> {
    let request = NativeIcpGetBlocksArgs {
        start: block_index,
        length: 1,
    };
    let (response,): (NativeIcpQueryBlocksResponse,) =
        ic_cdk::call(ledger, "query_blocks", (request.clone(),))
            .await
            .map_err(|(code, message)| {
                format!("native ICP query_blocks failed: {code:?} {message}")
            })?;
    if response.blocks.len() > 1 {
        return Err(format!(
            "native ICP ledger returned multiple direct blocks for {block_index}"
        ));
    }

    if let Some(offset) = block_index
        .checked_sub(response.first_block_index)
        .and_then(|offset| usize::try_from(offset).ok())
        .filter(|offset| *offset < response.blocks.len())
    {
        return Ok(response.blocks[offset].clone());
    }

    let mut covering = response.archived_blocks.iter().filter(|archive| {
        archive.length > 0
            && archive.start <= block_index
            && archive
                .start
                .checked_add(archive.length)
                .is_some_and(|end| block_index < end)
    });
    let archive = covering.next().ok_or_else(|| {
        format!("native ICP ledger returned no block/archive descriptor for {block_index}")
    })?;
    if covering.next().is_some() {
        return Err(format!(
            "native ICP ledger returned overlapping archive descriptors for {block_index}"
        ));
    }
    let (result,): (NativeIcpQueryArchiveResult,) = ic_cdk::call(
        archive.callback.canister_id,
        &archive.callback.method,
        (request,),
    )
    .await
    .map_err(|(code, message)| format!("native ICP archive call failed: {code:?} {message}"))?;
    match result {
        Ok(range) if range.blocks.len() == 1 => Ok(range
            .blocks
            .into_iter()
            .next()
            .expect("one native ICP block")),
        Ok(_) => Err("native ICP archive did not return exactly one requested block".into()),
        Err(error) => Err(format!(
            "native ICP archive rejected block lookup: {error:?}"
        )),
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
) -> Result<bool, String> {
    let operation = block
        .transaction
        .operation
        .as_ref()
        .ok_or("native ICP block operation is missing")?;
    let NativeIcpOperation::Transfer {
        from,
        to,
        spender,
        amount: block_amount,
        fee: block_fee,
    } = operation
    else {
        return Ok(false);
    };
    if spender.is_some() {
        return Ok(false);
    }
    if from.len() != 32 || to.len() != 32 {
        return Err("native ICP block account identifier is not 32 bytes".into());
    }
    if block_amount.e8s != amount
        || block_fee.e8s != fee
        || block.transaction.icrc1_memo.as_deref() != Some(memo)
        || block.transaction.created_at_time.timestamp_nanos != created_at_time
    {
        return Ok(false);
    }
    let (sender_id,): (Vec<u8>,) = ic_cdk::call(
        ledger,
        "account_identifier",
        (Account {
            owner: source,
            subaccount: None,
        },),
    )
    .await
    .map_err(|(code, message)| {
        format!("native ICP sender account_identifier failed: {code:?} {message}")
    })?;
    let (destination_id,): (Vec<u8>,) = ic_cdk::call(
        ledger,
        "account_identifier",
        (Account {
            owner: destination,
            subaccount: None,
        },),
    )
    .await
    .map_err(|(code, message)| {
        format!("native ICP destination account_identifier failed: {code:?} {message}")
    })?;
    if sender_id.len() != 32 || destination_id.len() != 32 {
        return Err("native ICP account_identifier returned malformed account bytes".into());
    }
    Ok(*from == sender_id && *to == destination_id)
}

fn validate_native_icp_transfer_from_block(
    block: &NativeIcpBlock,
    source_id: &[u8],
    spender_id: &[u8],
    destination_id: &[u8],
    amount: u64,
    fee: u64,
    memo: &[u8],
    created_at_time: u64,
) -> Result<(), String> {
    let Some(NativeIcpOperation::Transfer {
        from,
        to,
        spender: Some(spender),
        amount: actual_amount,
        fee: actual_fee,
    }) = block.transaction.operation.as_ref()
    else {
        return Err("native ICP block is not an ICRC-2 transfer_from".into());
    };
    if from.len() != 32 || to.len() != 32 || spender.len() != 32 {
        return Err("native ICP transfer_from contains malformed account identifiers".into());
    }
    if from != source_id
        || spender != spender_id
        || to != destination_id
        || actual_amount.e8s != amount
        || actual_fee.e8s != fee
        || block.transaction.icrc1_memo.as_deref() != Some(memo)
        || block.transaction.created_at_time.timestamp_nanos != created_at_time
    {
        return Err("native ICP block does not match the exact ICRC-2 transfer_from tuple".into());
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
    let request = NativeIcpGetBlocksArgs {
        start: 0,
        length: 0,
    };
    let (response,): (NativeIcpQueryBlocksResponse,) =
        ic_cdk::call(ledger, "query_blocks", (request,))
            .await
            .map_err(|(code, message)| {
                format!("native ICP query_blocks head request failed: {code:?} {message}")
            })?;
    Ok(response.chain_length)
}

pub async fn native_icp_cancel_history_block(
    ledger: Principal,
    block_index: u64,
) -> Result<NativeIcpCancelHistoryBlock, String> {
    let block = fetch_native_icp_block(ledger, block_index).await?;
    let operation = block
        .transaction
        .operation
        .ok_or("native ICP block operation is missing")?;
    match operation {
        NativeIcpOperation::Transfer {
            from,
            to,
            spender,
            amount,
            fee,
        } => {
            if from.len() != 32
                || to.len() != 32
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
    static IN_FLIGHT_TREASURY_PAYMENTS: RefCell<BTreeSet<u128>> =
        RefCell::new(BTreeSet::new());
    static IN_FLIGHT_SP_INTEREST_MINTS: RefCell<BTreeSet<u128>> = RefCell::new(BTreeSet::new());
}

struct SpInterestMintGuard(u128);
impl SpInterestMintGuard {
    fn try_new(nonce: u128) -> Option<Self> {
        IN_FLIGHT_SP_INTEREST_MINTS
            .with(|set| set.borrow_mut().insert(nonce).then_some(Self(nonce)))
    }
}
impl Drop for SpInterestMintGuard {
    fn drop(&mut self) {
        IN_FLIGHT_SP_INTEREST_MINTS.with(|set| {
            set.borrow_mut().remove(&self.0);
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

struct TreasuryPaymentGuard(u128);

impl TreasuryPaymentGuard {
    fn try_new(operation_id: u128) -> Option<Self> {
        IN_FLIGHT_TREASURY_PAYMENTS.with(|in_flight| {
            in_flight
                .borrow_mut()
                .insert(operation_id)
                .then_some(Self(operation_id))
        })
    }
}

impl Drop for TreasuryPaymentGuard {
    fn drop(&mut self) {
        IN_FLIGHT_TREASURY_PAYMENTS.with(|in_flight| {
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
pub fn collateral_to_asset_type(ct: &Principal) -> Option<AssetType> {
    read_state(|s| {
        if *ct == s.icp_ledger_principal {
            return Some(AssetType::ICP);
        }
        if let Some(ckusdt) = s.ckusdt_ledger_principal {
            if *ct == ckusdt {
                return Some(AssetType::CKUSDT);
            }
        }
        if let Some(ckusdc) = s.ckusdc_ledger_principal {
            if *ct == ckusdc {
                return Some(AssetType::CKUSDC);
            }
        }
        // A different collateral must not be reported as ICP. Keep its
        // pending fee held until a correctly typed treasury route exists.
        None
    })
}

/// Persist a V2 liquidation fee obligation inside the same State transition as
/// the debt/collateral commit. The returned payment is dispatched only after
/// the caller has saved the complete liquidation outbox.
pub(crate) fn queue_liquidation_fee_obligation_in_state(
    state: &mut crate::state::State,
    collateral_ledger: Principal,
    amount_raw: u64,
) -> Option<u128> {
    if amount_raw == 0 {
        return None;
    }
    let asset_type = if collateral_ledger == state.icp_ledger_principal {
        Some(AssetType::ICP)
    } else if state.ckusdt_ledger_principal == Some(collateral_ledger) {
        Some(AssetType::CKUSDT)
    } else if state.ckusdc_ledger_principal == Some(collateral_ledger) {
        Some(AssetType::CKUSDC)
    } else {
        None
    };
    let Some(asset_type) = asset_type else {
        state
            .pending_treasury_collateral
            .push((amount_raw, collateral_ledger));
        return None;
    };
    let treasury = state.treasury_principal;
    Some(queue_configurable_treasury_obligation_in_state(
        state,
        TreasuryPaymentKind::LiquidationCollateralTransfer,
        Some(collateral_ledger),
        treasury,
        amount_raw,
        DepositType::LiquidationFee,
        asset_type,
    ))
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
    notify_treasury_deposit_with_memo(
        treasury,
        deposit_type,
        asset_type,
        amount,
        block_index,
        None,
    )
    .await
}

async fn notify_treasury_deposit_with_memo(
    treasury: Principal,
    deposit_type: DepositType,
    asset_type: AssetType,
    amount: u64,
    block_index: u64,
    memo: Option<String>,
) -> Result<u64, String> {
    let args = DepositArgs {
        deposit_type,
        asset_type,
        amount,
        block_index,
        memo,
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

fn queue_pinned_treasury_payment(
    kind: TreasuryPaymentKind,
    ledger: Principal,
    recipient: Principal,
    amount_e8s: u64,
    deposit_type: DepositType,
    asset_type: AssetType,
) -> u128 {
    crate::state::mutate_state(|state| {
        let operation_id = state.next_op_nonce();
        let payment = PendingTreasuryPayment {
            operation_id,
            kind,
            ledger,
            from_owner: ic_cdk::id(),
            from_subaccount: None,
            recipient: Account {
                owner: recipient,
                subaccount: None,
            },
            amount_e8s,
            fee_e8s: None,
            memo: management::nonce_to_memo(operation_id).0.to_vec(),
            created_at_time_ns: management::nonce_to_created_at_time(operation_id),
            deposit_type,
            asset_type,
            deposit_memo: None,
            phase: TreasuryPaymentPhase::TransferPending,
            dispatch_attempts: 0,
            last_dispatch_at_ns: 0,
            transfer_nonce: operation_id,
        };
        crate::event::record_treasury_payment_queued(state, payment);
        operation_id
    })
}

fn queue_configurable_treasury_obligation(
    kind: TreasuryPaymentKind,
    ledger: Option<Principal>,
    treasury: Option<Principal>,
    amount_raw: u64,
    deposit_type: DepositType,
    asset_type: AssetType,
) -> u128 {
    crate::state::mutate_state(|state| {
        queue_configurable_treasury_obligation_in_state(
            state,
            kind,
            ledger,
            treasury,
            amount_raw,
            deposit_type,
            asset_type,
        )
    })
}

fn queue_configurable_treasury_obligation_in_state(
    state: &mut crate::state::State,
    kind: TreasuryPaymentKind,
    ledger: Option<Principal>,
    treasury: Option<Principal>,
    amount_raw: u64,
    deposit_type: DepositType,
    asset_type: AssetType,
) -> u128 {
    queue_configurable_treasury_obligation_in_state_at(
        state,
        kind,
        ledger,
        treasury,
        amount_raw,
        deposit_type,
        asset_type,
        ic_cdk::api::time(),
        ic_cdk::id(),
    )
}

fn queue_configurable_treasury_obligation_in_state_at(
    state: &mut crate::state::State,
    kind: TreasuryPaymentKind,
    ledger: Option<Principal>,
    treasury: Option<Principal>,
    amount_raw: u64,
    deposit_type: DepositType,
    asset_type: AssetType,
    now_ns: u64,
    backend_owner: Principal,
) -> u128 {
    let operation_id = state.next_op_nonce_at(now_ns);
    let ledger_is_configured = ledger.is_some_and(|principal| principal != Principal::anonymous());
    let treasury_is_configured =
        treasury.is_some_and(|principal| principal != Principal::anonymous());
    let initially_pinned = ledger_is_configured && treasury_is_configured;
    let payment = PendingTreasuryPayment {
        operation_id,
        kind,
        ledger: ledger.unwrap_or_else(Principal::anonymous),
        from_owner: backend_owner,
        from_subaccount: None,
        recipient: Account {
            owner: treasury.unwrap_or_else(Principal::anonymous),
            subaccount: None,
        },
        amount_e8s: amount_raw,
        fee_e8s: None,
        memo: management::nonce_to_memo(operation_id).0.to_vec(),
        created_at_time_ns: management::nonce_to_created_at_time(operation_id),
        deposit_type,
        asset_type,
        deposit_memo: None,
        phase: if initially_pinned {
            TreasuryPaymentPhase::TransferPending
        } else {
            TreasuryPaymentPhase::AwaitingDestination
        },
        dispatch_attempts: 0,
        last_dispatch_at_ns: 0,
        transfer_nonce: if initially_pinned { operation_id } else { 0 },
    };
    crate::event::record_treasury_payment_queued_at(state, payment, now_ns);
    operation_id
}

fn retry_is_open(payment: &PendingTreasuryPayment, now_ns: u64) -> bool {
    now_ns >= payment.created_at_time_ns
        && now_ns.saturating_sub(payment.created_at_time_ns) < TREASURY_TRANSFER_RETRY_WINDOW_NS
}

fn retry_delay_ns(attempts: u8) -> u64 {
    TREASURY_TRANSFER_RETRY_DELAY_NS.saturating_mul(1u64 << attempts.saturating_sub(1).min(5))
}

fn interest_split_amounts(
    total: u64,
    split: &[crate::state::InterestRecipient],
) -> Option<Vec<u64>> {
    let bps_total = split
        .iter()
        .try_fold(0u64, |sum, recipient| sum.checked_add(recipient.bps))?;
    if bps_total != 10_000 {
        return None;
    }
    let mut shares: Vec<u64> = split
        .iter()
        .map(|recipient| ((total as u128) * (recipient.bps as u128) / 10_000) as u64)
        .collect();
    let allocated = shares
        .iter()
        .fold(0u64, |sum, value| sum.saturating_add(*value));
    let remainder = total.saturating_sub(allocated);
    if remainder > 0 {
        if let Some(index) = split.iter().rposition(|recipient| recipient.bps > 0) {
            shares[index] = shares[index].saturating_add(remainder);
        }
    }
    (shares.iter().sum::<u64>() == total).then_some(shares)
}

/// Verify a treasury transfer against the exact durable request identity.
/// The request omitted `fee`, so the recorded block fee may be any value, but
/// it must be present; absent ICRC-3 identity fields never prove settlement.
pub async fn verify_treasury_payment_receipt(
    payment: &PendingTreasuryPayment,
    block_index: u64,
) -> Result<(), String> {
    let mut dispatchable = payment.clone();
    dispatchable.phase = TreasuryPaymentPhase::TransferPending;
    if !crate::event::treasury_payment_is_well_formed(&dispatchable, payment.operation_id)
        || payment.recipient.owner == Principal::anonymous()
    {
        return Err("treasury payment does not contain a complete exact transfer identity".into());
    }
    let block = crate::icrc3_proof::fetch_icrc3_block(payment.ledger, block_index).await?;
    if matches!(
        payment.kind,
        TreasuryPaymentKind::BorrowingFeeMint | TreasuryPaymentKind::InterestIcusdMint
    ) {
        validate_treasury_payment_mint_receipt(&block, payment)
    } else {
        if block.transaction_fee.is_none() {
            return Err("ICRC-3 transfer block omitted its transaction fee".into());
        }
        crate::icrc3_proof::validate_icrc3_direct_transfer_block(
            &block,
            Account {
                owner: payment.from_owner,
                subaccount: payment.from_subaccount,
            },
            payment.recipient.clone(),
            payment.amount_e8s,
            Some(&payment.memo),
            Some(payment.created_at_time_ns),
        )
    }
}

fn validate_treasury_payment_mint_receipt(
    block: &crate::icrc3_proof::DecodedBlock,
    payment: &PendingTreasuryPayment,
) -> Result<(), String> {
    if !matches!(block.btype.as_deref(), None | Some("1mint")) || block.op != "mint" {
        return Err("ICRC-3 block is not an ICRC-3 mint".into());
    }
    let expected_to = &payment.recipient;
    let actual_to = block
        .to
        .as_ref()
        .ok_or_else(|| "ICRC-3 mint block omitted destination account".to_string())?;
    if actual_to != expected_to
        || block.from.is_some()
        || block.spender.is_some()
        || block.amount != payment.amount_e8s as u128
        || block.transaction_fee.unwrap_or(0) != 0
        || block.fee.unwrap_or(0) != 0
        || block.memo.as_deref() != Some(payment.memo.as_slice())
        || block.created_at_time != Some(payment.created_at_time_ns)
        || block.expected_allowance.is_some()
        || block.expires_at.is_some()
    {
        return Err(
            "ICRC-3 1mint does not match the exact persisted treasury mint identity".into(),
        );
    }
    Ok(())
}

/// Persist a stablecoin fee surcharge before the caller performs any
/// later await. Configuration is pinned only when both ledger and treasury
/// destination exist; a missing route remains a visible no-dispatch row.
pub fn queue_stablecoin_surcharge_obligation(
    amount_raw: u64,
    token_type: crate::StableTokenType,
    kind: TreasuryPaymentKind,
) -> u128 {
    let (treasury, ledger, asset_type) = read_state(|state| match token_type {
        crate::StableTokenType::CKUSDT => (
            state.treasury_principal,
            state.ckusdt_ledger_principal,
            AssetType::CKUSDT,
        ),
        crate::StableTokenType::CKUSDC => (
            state.treasury_principal,
            state.ckusdc_ledger_principal,
            AssetType::CKUSDC,
        ),
    });
    queue_configurable_treasury_obligation(
        kind,
        ledger,
        treasury,
        amount_raw,
        DepositType::LiquidationFee,
        asset_type,
    )
}

pub async fn dispatch_pending_treasury_payment(operation_id: u128) {
    process_pending_treasury_payment(operation_id).await;
}

pub fn has_retryable_pinned_treasury_payments() -> bool {
    read_state(|state| {
        state
            .pending_treasury_payments
            .values()
            .any(|payment| !matches!(&payment.phase, TreasuryPaymentPhase::Held { .. }))
    })
}

/// Resume durable treasury transfers and post-transfer deposit receipts.
/// Transfer-pending rows replay the exact stored ICRC tuple; notification-
/// pending rows never mint or transfer again.
pub async fn process_pending_treasury_payments() {
    const MAX_PER_PASS: usize = 16;
    let operation_ids = crate::state::mutate_state(|state| {
        use std::ops::Bound::{Excluded, Unbounded};
        let cursor = state.treasury_payment_retry_cursor;
        let mut rows: Vec<u128> = if let Some(cursor) = cursor {
            state
                .pending_treasury_payments
                .range((Excluded(cursor), Unbounded))
                .take(MAX_PER_PASS)
                .map(|(id, _)| *id)
                .collect()
        } else {
            Vec::new()
        };
        if rows.len() < MAX_PER_PASS {
            rows.extend(
                state
                    .pending_treasury_payments
                    .range(..=cursor.unwrap_or(u128::MAX))
                    .take(MAX_PER_PASS - rows.len())
                    .map(|(id, _)| *id),
            );
        }
        if let Some(last) = rows.last() {
            state.treasury_payment_retry_cursor = Some(*last);
        }
        rows
    });
    for operation_id in operation_ids {
        process_pending_treasury_payment(operation_id).await;
    }
}

async fn process_pending_treasury_payment(operation_id: u128) {
    let Some(_guard) = TreasuryPaymentGuard::try_new(operation_id) else {
        return;
    };
    let Some(mut payment) = crate::state::read_state(|state| {
        state.pending_treasury_payments.get(&operation_id).cloned()
    }) else {
        return;
    };

    if payment.phase == TreasuryPaymentPhase::AwaitingDestination {
        let route = read_state(|state| {
            let ledger = if payment.ledger != Principal::anonymous() {
                Some(payment.ledger)
            } else {
                match &payment.asset_type {
                    AssetType::ICUSD => (state.icusd_ledger_principal != Principal::anonymous())
                        .then_some(state.icusd_ledger_principal),
                    AssetType::ICP => (state.icp_ledger_principal != Principal::anonymous())
                        .then_some(state.icp_ledger_principal),
                    AssetType::CKBTC => None,
                    AssetType::CKUSDT => state.ckusdt_ledger_principal,
                    AssetType::CKUSDC => state.ckusdc_ledger_principal,
                }
            };
            (state.treasury_principal, ledger)
        });
        let (Some(recipient), Some(ledger)) = route else {
            return;
        };
        if let Err(error) = crate::state::mutate_state(|state| {
            crate::event::record_treasury_payment_destination_pinned(
                state,
                operation_id,
                recipient,
                ledger,
            )
        }) {
            log!(
                INFO,
                "[treasury] payment {} destination pin failed: {}",
                operation_id,
                error
            );
            return;
        }
        payment.recipient.owner = recipient;
        payment.ledger = ledger;
        // Destination pinning journals a fresh dedup identity as well as the
        // route. Reload the row so the first dispatch uses that exact tuple.
        let Some(pinned) = crate::state::read_state(|state| {
            state.pending_treasury_payments.get(&operation_id).cloned()
        }) else {
            return;
        };
        payment = pinned;
    }

    if matches!(&payment.phase, TreasuryPaymentPhase::TransferPending) {
        if payment.from_owner != ic_cdk::id()
            || !crate::event::treasury_payment_is_well_formed(&payment, operation_id)
        {
            let reason = "pinned treasury payment identity failed local validation".to_string();
            let _ = crate::state::mutate_state(|state| {
                crate::event::record_treasury_payment_held(state, operation_id, reason.clone())
            });
            log!(INFO, "[treasury] payment {} held: {}", operation_id, reason);
            return;
        }
        let now_ns = ic_cdk::api::time();
        if !retry_is_open(&payment, now_ns) {
            let reason = "exact transfer retry window expired; receipt reconciliation required";
            let _ = crate::state::mutate_state(|state| {
                crate::event::record_treasury_payment_held(state, operation_id, reason.into())
            });
            return;
        }
        if payment.dispatch_attempts >= TREASURY_TRANSFER_MAX_ATTEMPTS {
            let reason =
                "exact transfer retry attempt limit reached; receipt reconciliation required";
            let _ = crate::state::mutate_state(|state| {
                crate::event::record_treasury_payment_held(state, operation_id, reason.into())
            });
            return;
        }
        if payment.last_dispatch_at_ns != 0
            && now_ns.saturating_sub(payment.last_dispatch_at_ns)
                < retry_delay_ns(payment.dispatch_attempts)
        {
            return;
        }
        let dispatch_attempts = match crate::state::mutate_state(|state| {
            crate::event::record_treasury_payment_attempted(state, operation_id, now_ns)
        }) {
            Ok(attempts) => attempts,
            Err(error) => {
                log!(
                    INFO,
                    "[treasury] payment {} attempt journal failed: {}",
                    operation_id,
                    error
                );
                return;
            }
        };
        payment.dispatch_attempts = dispatch_attempts;
        payment.last_dispatch_at_ns = now_ns;
        match management::transfer_idempotent_exact_tuple(
            payment.ledger,
            payment.from_subaccount,
            payment.recipient.clone(),
            payment.amount_e8s as u128,
            payment.fee_e8s,
            icrc_ledger_types::icrc1::transfer::Memo::from(payment.memo.clone()),
            payment.created_at_time_ns,
        )
        .await
        {
            Ok(block_index) => {
                if let Err(reason) = verify_treasury_payment_receipt(&payment, block_index).await {
                    let reason = format!("source returned block {block_index}, but exact receipt is not yet proven: {reason}");
                    let _ = crate::state::mutate_state(|state| {
                        crate::event::record_treasury_payment_held(
                            state,
                            operation_id,
                            reason.clone(),
                        )
                    });
                    return;
                }
                let result = crate::state::mutate_state(|state| {
                    crate::event::record_treasury_payment_transfer_confirmed(
                        state,
                        operation_id,
                        block_index,
                    )
                });
                if let Err(error) = result {
                    log!(
                        INFO,
                        "[treasury] payment {} confirmation journal failed: {}",
                        operation_id,
                        error
                    );
                    return;
                }
                payment.phase = TreasuryPaymentPhase::NotificationPending { block_index };
            }
            Err(error) => {
                let too_old = matches!(
                    &error,
                    icrc_ledger_types::icrc1::transfer::TransferError::TooOld
                );
                let retry_open = retry_is_open(&payment, ic_cdk::api::time());
                let reason = format!("exact source transfer attempt failed: {error:?}");
                let should_hold =
                    too_old || !retry_open || dispatch_attempts >= TREASURY_TRANSFER_MAX_ATTEMPTS;
                let _ = crate::state::mutate_state(|state| {
                    if should_hold {
                        crate::event::record_treasury_payment_held(
                            state,
                            operation_id,
                            reason.clone(),
                        )
                    } else {
                        crate::event::record_treasury_payment_transfer_failed(
                            state,
                            operation_id,
                            reason.clone(),
                        )
                    }
                });
                log!(
                    INFO,
                    "[treasury] payment {} {}: {}",
                    operation_id,
                    if should_hold {
                        "held"
                    } else {
                        "remains retryable"
                    },
                    reason
                );
                return;
            }
        }
    }

    if let TreasuryPaymentPhase::NotificationPending { block_index } = &payment.phase {
        let block_index = *block_index;
        match notify_treasury_deposit_with_memo(
            payment.recipient.owner,
            payment.deposit_type,
            payment.asset_type,
            payment.amount_e8s,
            block_index,
            payment.deposit_memo,
        )
        .await
        {
            Ok(deposit_id) => {
                if let Err(error) = crate::state::mutate_state(|state| {
                    crate::event::record_treasury_payment_notification_acknowledged(
                        state,
                        operation_id,
                        deposit_id,
                    )
                }) {
                    log!(
                        INFO,
                        "[treasury] payment {} receipt acknowledgment journal failed: {}",
                        operation_id,
                        error
                    );
                }
            }
            Err(error) => log!(
                INFO,
                "[treasury] payment {} receipt notification remains pending for block {}: {}",
                operation_id,
                block_index,
                error
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Public helpers — mint/transfer + notify
// ---------------------------------------------------------------------------

/// Pin an icUSD interest mint to treasury before dispatch. Once queued, the
/// outbox owns this share even if the ledger reply is lost or reconciliation
/// holds the row; callers must not put it back into an aggregate bucket.
/// Returns `Err` only when no treasury destination exists and no row was made.
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
    let ledger = read_state(|s| s.icusd_ledger_principal);
    let operation_id = queue_pinned_treasury_payment(
        TreasuryPaymentKind::InterestIcusdMint,
        ledger,
        tp,
        interest_share.to_u64(),
        DepositType::InterestRevenue,
        AssetType::ICUSD,
    );
    process_pending_treasury_payment(operation_id).await;
    Ok(())
}

/// Mint icUSD interest revenue to the stability pool canister.
/// The stability pool distributes this pro-rata to depositors.
///
/// `collateral_type` identifies which collateral's vault generated this interest.
/// The pool uses it to exclude depositors who opted out of that collateral.
///
/// Pin the mint identity before dispatch. Once queued, the durable outbox owns
/// this share through retries and notification recovery.
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
    if icusd_ledger == Principal::anonymous() {
        return Err(interest_share);
    }
    let payment = crate::state::mutate_state(|s| {
        let operation_nonce = s.next_op_nonce();
        let payment = crate::state::PendingStabilityPoolInterestMint {
            operation_nonce,
            ledger: icusd_ledger,
            pool: pool_principal,
            amount_e8s: interest_share.to_u64(),
            collateral_type,
            memo: management::nonce_to_memo(operation_nonce).0.to_vec(),
            created_at_time_ns: management::nonce_to_created_at_time(operation_nonce),
            phase: crate::state::StabilityPoolInterestMintPhase::MintPending,
            attempts: 0,
            last_attempt_at_ns: 0,
        };
        crate::event::record_sp_interest_mint_queued(s, payment.clone());
        payment
    });
    process_pending_stability_pool_interest_mint(payment.operation_nonce).await;
    // The durable outbox now owns this share regardless of the current reply.
    Ok(())
}

const SP_INTEREST_MINT_MAX_ATTEMPTS: u8 = 60;

pub fn has_retryable_sp_interest_mints() -> bool {
    read_state(|s| {
        s.pending_stability_pool_interest_mints.values().any(|row| {
            !matches!(
                &row.phase,
                crate::state::StabilityPoolInterestMintPhase::Held { .. }
            )
        })
    })
}

/// Reconcile a held or retryable mint only from a positive exact ICRC-3 proof.
/// The CAS after the await prevents applying stale proof to a changed row.
pub async fn reconcile_sp_interest_mint_receipt(
    operation_nonce: u128,
    block_index: u64,
) -> Result<bool, String> {
    let Some(_guard) = SpInterestMintGuard::try_new(operation_nonce) else {
        return Err("SP interest mint is currently being processed".into());
    };
    let payment = read_state(|s| {
        s.pending_stability_pool_interest_mints
            .get(&operation_nonce)
            .cloned()
    })
    .ok_or_else(|| "No pending SP interest mint found".to_string())?;
    if let crate::state::StabilityPoolInterestMintPhase::NotificationPending { mint_block } =
        &payment.phase
    {
        return if *mint_block == block_index {
            Ok(false)
        } else {
            Err("SP interest mint is already confirmed at another block".into())
        };
    }
    verify_sp_interest_mint_receipt(&payment, block_index).await?;
    crate::state::mutate_state(|s| {
        if s.pending_stability_pool_interest_mints
            .get(&operation_nonce)
            != Some(&payment)
        {
            return Err(
                "SP interest mint changed during receipt verification; retry reconciliation".into(),
            );
        }
        crate::event::record_sp_interest_mint_confirmed(s, operation_nonce, block_index)
    })?;
    drop(_guard);
    process_pending_stability_pool_interest_mint(operation_nonce).await;
    Ok(true)
}

/// Pin a previously held Stability Pool share once its endpoint and ledger are
/// configured. The held ID links to one fresh ledger operation nonce because
/// the hold may be older than the ledger dedup window. Amount and collateral
/// remain unchanged and never pass through the mutable interest split again.
/// The bool reports whether this call created the link; it is not payout
/// confirmation. Only a verified mint receipt and downstream pool receipt are
/// settlement evidence.
pub async fn release_held_interest_distribution_share(
    operation_nonce: u128,
) -> Result<bool, String> {
    let held = read_state(|s| {
        s.held_interest_distribution_shares
            .get(&operation_nonce)
            .cloned()
    });
    let Some(held) = held else {
        let linked_nonce = read_state(|s| {
            s.released_interest_distribution_shares
                .get(&operation_nonce)
                .copied()
        });
        if let Some(linked_nonce) = linked_nonce {
            // An earlier release may have committed and lost its reply. Resume
            // the same durable row; its exact ledger tuple prevents replay.
            process_pending_stability_pool_interest_mint(linked_nonce).await;
            return Ok(false);
        }
        return Err(format!("held interest share {operation_nonce} not found"));
    };
    if held.destination != Some(crate::state::InterestDestination::StabilityPool) {
        return Err(
            "held share has no valid recipient allocation; explicit reviewed allocation required"
                .into(),
        );
    }
    let (pool, ledger) = read_state(|s| (s.stability_pool_canister, s.icusd_ledger_principal));
    let pool = pool.ok_or_else(|| "Stability Pool is not configured".to_string())?;
    if pool == Principal::anonymous() || ledger == Principal::anonymous() {
        return Err("Stability Pool and icUSD ledger must be configured".into());
    }
    let linked_nonce = crate::state::mutate_state(|s| {
        if s.held_interest_distribution_shares.get(&operation_nonce) != Some(&held) {
            return Err("held interest share changed; retry release".to_string());
        }
        if s.stability_pool_canister != Some(pool) || s.icusd_ledger_principal != ledger {
            return Err("Stability Pool configuration changed; retry release".to_string());
        }
        crate::event::record_interest_distribution_share_released_to_sp(
            s,
            operation_nonce,
            pool,
            ledger,
            ic_cdk::api::time(),
        )
    })?;
    process_pending_stability_pool_interest_mint(linked_nonce).await;
    Ok(true)
}

pub async fn process_pending_stability_pool_interest_mints() {
    const LIMIT: usize = 16;
    let nonces: Vec<u128> = crate::state::mutate_state(|s| {
        use std::ops::Bound::{Excluded, Unbounded};
        let cursor = s.stability_pool_interest_mint_retry_cursor;
        let mut rows: Vec<u128> = cursor.map_or_else(Vec::new, |cursor| {
            s.pending_stability_pool_interest_mints
                .range((Excluded(cursor), Unbounded))
                .filter(|(_, row)| {
                    !matches!(
                        &row.phase,
                        crate::state::StabilityPoolInterestMintPhase::Held { .. }
                    )
                })
                .take(LIMIT)
                .map(|(nonce, _)| *nonce)
                .collect()
        });
        if rows.len() < LIMIT {
            let remaining = LIMIT - rows.len();
            rows.extend(
                s.pending_stability_pool_interest_mints
                    .range(..=cursor.unwrap_or(u128::MAX))
                    .filter(|(_, row)| {
                        !matches!(
                            &row.phase,
                            crate::state::StabilityPoolInterestMintPhase::Held { .. }
                        )
                    })
                    .take(remaining)
                    .map(|(nonce, _)| *nonce),
            );
        }
        if let Some(last) = rows.last() {
            s.stability_pool_interest_mint_retry_cursor = Some(*last);
        }
        rows
    });
    for nonce in nonces {
        process_pending_stability_pool_interest_mint(nonce).await;
    }
}

async fn process_pending_stability_pool_interest_mint(operation_nonce: u128) {
    let Some(_guard) = SpInterestMintGuard::try_new(operation_nonce) else {
        return;
    };
    let Some(mut payment) = read_state(|s| {
        s.pending_stability_pool_interest_mints
            .get(&operation_nonce)
            .cloned()
    }) else {
        return;
    };
    match payment.phase.clone() {
        crate::state::StabilityPoolInterestMintPhase::MintPending => {
            let now = ic_cdk::api::time();
            if now.saturating_sub(payment.created_at_time_ns) >= TREASURY_TRANSFER_RETRY_WINDOW_NS {
                let reason =
                    "exact mint tuple reached the safe retry horizon; reconciliation required"
                        .to_string();
                let _ = crate::state::mutate_state(|s| {
                    crate::event::record_sp_interest_mint_held(s, operation_nonce, reason)
                });
                return;
            }
            if payment.attempts > 0
                && now.saturating_sub(payment.last_attempt_at_ns) < retry_delay_ns(payment.attempts)
            {
                return;
            }
            let attempts = match crate::state::mutate_state(|s| {
                crate::event::record_sp_interest_mint_attempted(s, operation_nonce)
            }) {
                Ok(attempts) => attempts,
                Err(error) => {
                    log!(
                        INFO,
                        "[treasury] SP interest mint {} attempt journal failed: {}",
                        operation_nonce,
                        error
                    );
                    return;
                }
            };
            payment.attempts = attempts;
            payment.last_attempt_at_ns = now;
            let to = Account {
                owner: payment.pool,
                subaccount: None,
            };
            match management::mint_icusd_with_nonce(
                payment.ledger,
                ICUSD::from(payment.amount_e8s),
                to,
                payment.operation_nonce,
            )
            .await
            {
                Ok(block_index) => {
                    if let Err(reason) =
                        verify_sp_interest_mint_receipt(&payment, block_index).await
                    {
                        let _ = crate::state::mutate_state(|s| {
                            crate::event::record_sp_interest_mint_held(s, operation_nonce, reason)
                        });
                        return;
                    }
                    if let Err(error) = crate::state::mutate_state(|s| {
                        crate::event::record_sp_interest_mint_confirmed(
                            s,
                            operation_nonce,
                            block_index,
                        )
                    }) {
                        log!(
                            INFO,
                            "[treasury] SP interest mint {} confirmation journal failed: {}",
                            operation_nonce,
                            error
                        );
                        return;
                    }
                    log!(
                        INFO,
                        "[treasury] confirmed SP interest mint {} at ledger block {}",
                        operation_nonce,
                        block_index
                    );
                }
                Err(error) => {
                    let held = matches!(
                        &error,
                        icrc_ledger_types::icrc1::transfer::TransferError::TooOld
                    ) || attempts >= SP_INTEREST_MINT_MAX_ATTEMPTS;
                    let reason = format!("exact SP interest mint attempt failed: {error:?}");
                    if held {
                        let _ = crate::state::mutate_state(|s| {
                            crate::event::record_sp_interest_mint_held(
                                s,
                                operation_nonce,
                                reason.clone(),
                            )
                        });
                    }
                    log!(
                        INFO,
                        "[treasury] SP interest mint {} {}: {}",
                        operation_nonce,
                        if held {
                            "held"
                        } else {
                            "will retry exact tuple"
                        },
                        reason
                    );
                    return;
                }
            }
        }
        crate::state::StabilityPoolInterestMintPhase::NotificationPending { .. } => {}
        crate::state::StabilityPoolInterestMintPhase::Held { .. } => return,
    }
    let Some(payment) = read_state(|s| {
        s.pending_stability_pool_interest_mints
            .get(&operation_nonce)
            .cloned()
    }) else {
        return;
    };
    if let crate::state::StabilityPoolInterestMintPhase::NotificationPending { mint_block } =
        payment.phase
    {
        let notification = crate::state::PendingStabilityPoolInterestNotification {
            pool_principal: payment.pool,
            token_ledger: payment.ledger,
            amount_e8s: payment.amount_e8s,
            collateral_type: payment.collateral_type,
            source_mint_block: mint_block,
            receipt_protocol_version: Some(1),
        };
        crate::state::mutate_state(|s| {
            s.pending_stability_pool_interest_notifications
                .insert(mint_block, notification.clone());
        });
        if notification_delivery_acknowledged(
            &notification,
            deliver_stability_pool_interest_notification(&notification).await,
        ) {
            if let Err(error) = crate::state::mutate_state(|s| {
                crate::event::record_sp_interest_mint_acknowledged(s, operation_nonce)
            }) {
                log!(
                    INFO,
                    "[treasury] SP interest mint {} acknowledgment journal failed: {}",
                    operation_nonce,
                    error
                );
            }
        }
    }
}

async fn verify_sp_interest_mint_receipt(
    payment: &crate::state::PendingStabilityPoolInterestMint,
    block_index: u64,
) -> Result<(), String> {
    let block = crate::icrc3_proof::fetch_icrc3_block(payment.ledger, block_index).await?;
    validate_sp_interest_mint_block(&block, payment)
}

fn validate_sp_interest_mint_block(
    block: &crate::icrc3_proof::DecodedBlock,
    payment: &crate::state::PendingStabilityPoolInterestMint,
) -> Result<(), String> {
    let to = block
        .to
        .as_ref()
        .ok_or_else(|| "ICRC-3 1mint omitted destination".to_string())?;
    if !matches!(block.btype.as_deref(), None | Some("1mint"))
        || block.op != "mint"
        || to != &(Account { owner: payment.pool, subaccount: None })
        || block.from.is_some()
        || block.spender.is_some()
        || block.amount != payment.amount_e8s as u128
        || block.transaction_fee.unwrap_or(0) != 0
        || block.fee.unwrap_or(0) != 0
        || block.memo.as_deref() != Some(payment.memo.as_slice())
        || block.created_at_time != Some(payment.created_at_time_ns)
        || block.expected_allowance.is_some()
        || block.expires_at.is_some()
    {
        return Err("ICRC-3 1mint does not match exact SP interest identity".into());
    }
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
        validate_sp_interest_mint_block, InterestNotificationGuard,
    };
    use candid::Principal;
    use icrc_ledger_types::icrc1::account::Account;

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
        assert!(
            pending.contains_key(&legacy.source_mint_block),
            "upgrade leaves the legacy outbox row held"
        );
        assert_eq!(
            legacy.source_mint_block, 44,
            "held row retains its source receipt for inspection"
        );
    }

    #[test]
    fn sp_interest_receipt_requires_exact_typed_mint_identity() {
        let nonce = 77;
        let pool = Principal::from_slice(&[1]);
        let payment = crate::state::PendingStabilityPoolInterestMint {
            operation_nonce: nonce,
            ledger: Principal::from_slice(&[2]),
            pool,
            amount_e8s: 1234,
            collateral_type: Principal::from_slice(&[3]),
            memo: crate::management::nonce_to_memo(nonce).0.to_vec(),
            created_at_time_ns: crate::management::nonce_to_created_at_time(nonce),
            phase: crate::state::StabilityPoolInterestMintPhase::MintPending,
            attempts: 1,
            last_attempt_at_ns: 1,
        };
        let valid = crate::icrc3_proof::DecodedBlock {
            btype: Some("1mint".into()),
            op: "mint".into(),
            from: None,
            to: Some(Account {
                owner: pool,
                subaccount: None,
            }),
            spender: None,
            amount: 1234,
            transaction_fee: None,
            fee: None,
            memo: Some(payment.memo.clone()),
            created_at_time: Some(payment.created_at_time_ns),
            expected_allowance: None,
            expires_at: None,
        };
        assert!(validate_sp_interest_mint_block(&valid, &payment).is_ok());
        let mut untyped = valid.clone();
        untyped.btype = None;
        assert!(validate_sp_interest_mint_block(&untyped, &payment).is_ok());
        let mut zero_fee = valid.clone();
        zero_fee.transaction_fee = Some(0);
        zero_fee.fee = Some(0);
        assert!(validate_sp_interest_mint_block(&zero_fee, &payment).is_ok());
        let mut wrong_type = valid.clone();
        wrong_type.btype = Some("1xfer".into());
        assert!(validate_sp_interest_mint_block(&wrong_type, &payment).is_err());
        let mut wrong_memo = valid.clone();
        wrong_memo.memo = Some(vec![9]);
        assert!(validate_sp_interest_mint_block(&wrong_memo, &payment).is_err());
        let mut wrong_destination = valid.clone();
        wrong_destination.to = Some(Account {
            owner: Principal::from_slice(&[4]),
            subaccount: None,
        });
        assert!(validate_sp_interest_mint_block(&wrong_destination, &payment).is_err());
        let mut nonzero_fee = valid.clone();
        nonzero_fee.fee = Some(1);
        assert!(validate_sp_interest_mint_block(&nonzero_fee, &payment).is_err());
        let mut nonzero_transaction_fee = valid.clone();
        nonzero_transaction_fee.transaction_fee = Some(1);
        assert!(validate_sp_interest_mint_block(&nonzero_transaction_fee, &payment).is_err());
        let mut wrong_op = valid.clone();
        wrong_op.op = "transfer".into();
        assert!(validate_sp_interest_mint_block(&wrong_op, &payment).is_err());
        let mut tuple_drift = valid.clone();
        tuple_drift.created_at_time = Some(payment.created_at_time_ns + 1);
        assert!(validate_sp_interest_mint_block(&tuple_drift, &payment).is_err());
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
/// Pins the complete split to recipient outboxes in one synchronous state
/// transition before the first inter-canister await. From that point the
/// outboxes own every share, including shares whose destination is currently
/// unavailable. Returns zero: retryable and held shares must never be restored
/// to the aggregate bucket and split a second time.
pub async fn distribute_interest(interest: ICUSD, collateral_type: Principal) -> ICUSD {
    if interest.0 == 0 {
        return ICUSD::new(0);
    }

    #[derive(Clone, Copy)]
    enum Delivery {
        Treasury(u128),
        StabilityPool(u128),
        ThreePool(u128),
        Amm1(u64),
        Held(u128),
    }

    // Freeze recipient rows, amounts, destinations, and operation identities
    // in a single state mutation. No await is allowed before this completes.
    let deliveries = crate::state::mutate_state(|s| {
        use crate::state::{
            AmmDonationPhase, InterestDestination, PendingAmmDonation,
            PendingStabilityPoolInterestMint, PendingThreePoolDonation,
            StabilityPoolInterestMintPhase, ThreePoolDonationPhase, TreasuryPaymentKind,
        };
        let split = s.interest_split.clone();
        let total = interest.to_u64();
        // The admin setter requires exactly 10,000 bps. Assign division dust
        // to the last nonzero recipient so the configured split preserves the
        // full accrued amount instead of burning a few e8s per distribution.
        let Some(shares) = interest_split_amounts(total, &split) else {
            let nonce = s.next_op_nonce();
            let held = crate::state::HeldInterestDistributionShare {
                operation_nonce: nonce,
                amount_e8s: total,
                collateral_type,
                destination: None,
                reason: "invalid persisted interest split; held without dispatch".into(),
            };
            crate::event::record_interest_distribution_share_held(s, held);
            return vec![Delivery::Held(nonce)];
        };

        let mut deliveries = Vec::new();
        for (recipient, amount_e8s) in split.iter().zip(shares) {
            if amount_e8s == 0 {
                continue;
            }
            match recipient.destination {
                InterestDestination::StabilityPool => {
                    let (pool, ledger) = (s.stability_pool_canister, s.icusd_ledger_principal);
                    if let Some(pool) = pool.filter(|_| ledger != Principal::anonymous()) {
                        let nonce = s.next_op_nonce();
                        let payment = PendingStabilityPoolInterestMint {
                            operation_nonce: nonce,
                            ledger,
                            pool,
                            amount_e8s,
                            collateral_type,
                            memo: management::nonce_to_memo(nonce).0.to_vec(),
                            created_at_time_ns: management::nonce_to_created_at_time(nonce),
                            phase: StabilityPoolInterestMintPhase::MintPending,
                            attempts: 0,
                            last_attempt_at_ns: 0,
                        };
                        crate::event::record_sp_interest_mint_queued(s, payment);
                        deliveries.push(Delivery::StabilityPool(nonce));
                    } else {
                        let nonce = s.next_op_nonce();
                        let held = crate::state::HeldInterestDistributionShare {
                            operation_nonce: nonce,
                            amount_e8s,
                            collateral_type,
                            destination: Some(InterestDestination::StabilityPool),
                            reason: "Stability Pool destination unavailable when interest split was pinned".into(),
                        };
                        crate::event::record_interest_distribution_share_held(s, held);
                        deliveries.push(Delivery::Held(nonce));
                    }
                }
                InterestDestination::Treasury => {
                    let ledger = s.icusd_ledger_principal;
                    let treasury = s.treasury_principal;
                    let id = queue_configurable_treasury_obligation_in_state(
                        s,
                        TreasuryPaymentKind::InterestIcusdMint,
                        Some(ledger),
                        treasury,
                        amount_e8s,
                        DepositType::InterestRevenue,
                        AssetType::ICUSD,
                    );
                    deliveries.push(Delivery::Treasury(id));
                }
                InterestDestination::ThreePool => {
                    if let Some(pool) = s.three_pool_canister {
                        let nonce = s.next_op_nonce();
                        let ledger = s.icusd_ledger_principal;
                        s.pending_three_pool_donations.insert(
                            nonce,
                            PendingThreePoolDonation {
                                pool,
                                ledger,
                                amount_e8s,
                                phase: ThreePoolDonationPhase::MintPending,
                            },
                        );
                        deliveries.push(Delivery::ThreePool(nonce));
                    } else {
                        let ledger = s.icusd_ledger_principal;
                        let treasury = s.treasury_principal;
                        let id = queue_configurable_treasury_obligation_in_state(
                            s,
                            TreasuryPaymentKind::InterestIcusdMint,
                            Some(ledger),
                            treasury,
                            amount_e8s,
                            DepositType::InterestRevenue,
                            AssetType::ICUSD,
                        );
                        deliveries.push(Delivery::Treasury(id));
                    }
                }
                InterestDestination::Amm1 => {
                    if let Some(amm) = s.amm1_canister {
                        s.amm1_donation_nonce = s
                            .amm1_donation_nonce
                            .checked_add(1)
                            .expect("AMM1 donation nonce exhausted");
                        let notify_nonce = s.amm1_donation_nonce;
                        let configured = s.amm1_pool_id.clone();
                        let (ledger, pool_id, reward_subaccount, mint_op_nonce, phase) =
                            if let Some(pool_id) = configured {
                                let nonce = s.next_op_nonce();
                                let ledger = s.icusd_ledger_principal;
                                (
                                    Some(ledger),
                                    Some(pool_id.clone()),
                                    Some(compute_amm_reward_subaccount(&pool_id)),
                                    Some(nonce),
                                    AmmDonationPhase::MintPending,
                                )
                            } else {
                                (None, None, None, None, AmmDonationPhase::AwaitingPoolConfig)
                            };
                        s.pending_amm_donations.insert(
                            notify_nonce,
                            PendingAmmDonation {
                                amm_canister: amm,
                                ledger,
                                amount_e8s,
                                notify_nonce,
                                mint_op_nonce,
                                pool_id,
                                reward_subaccount,
                                phase,
                            },
                        );
                        deliveries.push(Delivery::Amm1(notify_nonce));
                    } else {
                        let ledger = s.icusd_ledger_principal;
                        let treasury = s.treasury_principal;
                        let id = queue_configurable_treasury_obligation_in_state(
                            s,
                            TreasuryPaymentKind::InterestIcusdMint,
                            Some(ledger),
                            treasury,
                            amount_e8s,
                            DepositType::InterestRevenue,
                            AssetType::ICUSD,
                        );
                        deliveries.push(Delivery::Treasury(id));
                    }
                }
            }
        }
        deliveries
    });

    // Dispatch only rows created above. Their exact tuple/receipt identities
    // remain stable if a callback reply is lost or an upgrade aborts this future.
    for delivery in deliveries {
        match delivery {
            Delivery::Treasury(id) => process_pending_treasury_payment(id).await,
            Delivery::StabilityPool(nonce) => {
                process_pending_stability_pool_interest_mint(nonce).await
            }
            Delivery::ThreePool(nonce) => process_pending_three_pool_donation(nonce).await,
            Delivery::Amm1(nonce) => process_pending_amm_donation(nonce).await,
            Delivery::Held(nonce) => log!(
                INFO,
                "[treasury] interest share {} held pending Stability Pool configuration",
                nonce
            ),
        }
    }
    ICUSD::new(0)
}

/// Pin the complete icUSD interest split into receipt-safe recipient outboxes
/// without dispatching. Manual liquidation calls this from the same State
/// transition that applies the receipt-backed debt and collateral effects, so
/// an upgrade or callback trap cannot lose a share between accounting and the
/// first await.
pub(crate) fn pin_icusd_interest_distribution_in_state(
    state: &mut crate::state::State,
    interest: ICUSD,
    collateral_type: Principal,
    plan: &crate::state::StableRepaymentV2InterestRoutingPlan,
) -> Result<(), String> {
    pin_icusd_interest_distribution_in_state_at(
        state,
        interest,
        collateral_type,
        plan,
        ic_cdk::api::time(),
        ic_cdk::id(),
    )
}

pub(crate) fn pin_icusd_interest_distribution_in_state_at(
    state: &mut crate::state::State,
    interest: ICUSD,
    collateral_type: Principal,
    plan: &crate::state::StableRepaymentV2InterestRoutingPlan,
    now_ns: u64,
    backend_owner: Principal,
) -> Result<(), String> {
    use crate::state::{
        AmmDonationPhase, HeldInterestDistributionShare, InterestDestination, PendingAmmDonation,
        PendingStabilityPoolInterestMint, PendingThreePoolDonation, StabilityPoolInterestMintPhase,
        ThreePoolDonationPhase, TreasuryPaymentKind,
    };
    if interest.0 == 0 {
        return Ok(());
    }
    let Some(shares) = interest_split_amounts(interest.to_u64(), &plan.split) else {
        let nonce = state.next_op_nonce_at(now_ns);
        crate::event::record_interest_distribution_share_held_at(
            state,
            HeldInterestDistributionShare {
                operation_nonce: nonce,
                amount_e8s: interest.to_u64(),
                collateral_type,
                destination: None,
                reason: "invalid pinned interest split; retained without dispatch".into(),
            },
            now_ns,
        );
        return Ok(());
    };
    let amm_rows = plan
        .split
        .iter()
        .zip(&shares)
        .filter(|(recipient, share)| {
            **share > 0 && recipient.destination == InterestDestination::Amm1 && plan.amm1.is_some()
        })
        .count() as u64;
    if state.amm1_donation_nonce.checked_add(amm_rows).is_none() {
        return Err("AMM donation nonce exhausted before interest outbox creation".into());
    }

    for (recipient, amount_e8s) in plan.split.iter().zip(shares) {
        if amount_e8s == 0 {
            continue;
        }
        match recipient.destination {
            InterestDestination::StabilityPool => {
                if let Some(pool) = plan
                    .stability_pool
                    .filter(|_| plan.icusd_ledger != Principal::anonymous())
                {
                    let nonce = state.next_op_nonce_at(now_ns);
                    crate::event::record_sp_interest_mint_queued_at(
                        state,
                        PendingStabilityPoolInterestMint {
                            operation_nonce: nonce,
                            ledger: plan.icusd_ledger,
                            pool,
                            amount_e8s,
                            collateral_type,
                            memo: management::nonce_to_memo(nonce).0.to_vec(),
                            created_at_time_ns: management::nonce_to_created_at_time(nonce),
                            phase: StabilityPoolInterestMintPhase::MintPending,
                            attempts: 0,
                            last_attempt_at_ns: 0,
                        },
                        now_ns,
                    );
                } else {
                    let nonce = state.next_op_nonce_at(now_ns);
                    crate::event::record_interest_distribution_share_held_at(
                        state,
                        HeldInterestDistributionShare {
                            operation_nonce: nonce,
                            amount_e8s,
                            collateral_type,
                            destination: Some(InterestDestination::StabilityPool),
                            reason: "Stability Pool route unavailable when interest was pinned"
                                .into(),
                        },
                        now_ns,
                    );
                }
            }
            InterestDestination::Treasury => {
                queue_configurable_treasury_obligation_in_state_at(
                    state,
                    TreasuryPaymentKind::InterestIcusdMint,
                    Some(plan.icusd_ledger),
                    plan.stable_treasury,
                    amount_e8s,
                    DepositType::InterestRevenue,
                    AssetType::ICUSD,
                    now_ns,
                    backend_owner,
                );
            }
            InterestDestination::ThreePool => {
                if let Some(pool) = plan.three_pool {
                    let nonce = state.next_op_nonce_at(now_ns);
                    state.pending_three_pool_donations.insert(
                        nonce,
                        PendingThreePoolDonation {
                            pool,
                            ledger: plan.icusd_ledger,
                            amount_e8s,
                            phase: ThreePoolDonationPhase::MintPending,
                        },
                    );
                } else {
                    queue_configurable_treasury_obligation_in_state_at(
                        state,
                        TreasuryPaymentKind::InterestIcusdMint,
                        Some(plan.icusd_ledger),
                        plan.stable_treasury,
                        amount_e8s,
                        DepositType::InterestRevenue,
                        AssetType::ICUSD,
                        now_ns,
                        backend_owner,
                    );
                }
            }
            InterestDestination::Amm1 => {
                if let Some(amm) = plan.amm1 {
                    state.amm1_donation_nonce = state
                        .amm1_donation_nonce
                        .checked_add(1)
                        .expect("AMM nonce capacity was checked before outbox writes");
                    let notify_nonce = state.amm1_donation_nonce;
                    let (ledger, pool_id, reward_subaccount, mint_op_nonce, phase) =
                        if let Some(pool_id) = plan.amm1_pool_id.clone() {
                            let nonce = state.next_op_nonce_at(now_ns);
                            (
                                Some(plan.icusd_ledger),
                                Some(pool_id.clone()),
                                Some(compute_amm_reward_subaccount(&pool_id)),
                                Some(nonce),
                                AmmDonationPhase::MintPending,
                            )
                        } else {
                            (None, None, None, None, AmmDonationPhase::AwaitingPoolConfig)
                        };
                    state.pending_amm_donations.insert(
                        notify_nonce,
                        PendingAmmDonation {
                            amm_canister: amm,
                            ledger,
                            amount_e8s,
                            notify_nonce,
                            mint_op_nonce,
                            pool_id,
                            reward_subaccount,
                            phase,
                        },
                    );
                } else {
                    queue_configurable_treasury_obligation_in_state_at(
                        state,
                        TreasuryPaymentKind::InterestIcusdMint,
                        Some(plan.icusd_ledger),
                        plan.stable_treasury,
                        amount_e8s,
                        DepositType::InterestRevenue,
                        AssetType::ICUSD,
                        now_ns,
                        backend_owner,
                    );
                }
            }
        }
    }
    Ok(())
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

    // Capture the complete routing plan and persist all recipient rows in one
    // state transition before the first inter-canister await. A callback trap
    // or upgrade can then resume the same ledger tuple instead of splitting
    // this share again against mutable configuration.
    let deliveries = crate::state::mutate_state(|state| {
        let stable_ledger = match token_type {
            crate::StableTokenType::CKUSDT => state.ckusdt_ledger_principal,
            crate::StableTokenType::CKUSDC => state.ckusdc_ledger_principal,
        }
        .unwrap_or_else(Principal::anonymous);
        let plan = crate::state::StableRepaymentV2InterestRoutingPlan {
            split: state.interest_split.clone(),
            stable_treasury: state.treasury_principal,
            icusd_ledger: state.icusd_ledger_principal,
            stability_pool: state.stability_pool_canister,
            three_pool: state.three_pool_canister,
            amm1: state.amm1_canister,
            amm1_pool_id: state.amm1_pool_id.clone(),
        };
        match pin_stablecoin_interest_distribution_in_state_at(
            state,
            interest_e8s,
            collateral_type,
            token_type.clone(),
            stable_ledger,
            &plan,
            ic_cdk::api::time(),
            ic_cdk::id(),
        ) {
            Ok(deliveries) => deliveries,
            Err(error) => {
                // Preflight failures happen before any recipient row is added.
                // Retain the complete allocation for reviewed reconciliation.
                log!(INFO, "[treasury] stablecoin interest remains held: {}", error);
                let operation_nonce = state.next_op_nonce();
                crate::event::record_interest_distribution_share_held(
                    state,
                    crate::state::HeldInterestDistributionShare {
                        operation_nonce,
                        amount_e8s: interest_e8s,
                        collateral_type,
                        destination: None,
                        reason: format!("stable-interest routing preflight failed: {error}"),
                    },
                );
                Vec::new()
            }
        }
    });

    for delivery in deliveries {
        match delivery {
            StableInterestDelivery::Treasury(operation_id) => {
                process_pending_treasury_payment(operation_id).await
            }
            StableInterestDelivery::StabilityPool(operation_nonce) => {
                process_pending_stability_pool_interest_mint(operation_nonce).await
            }
            StableInterestDelivery::ThreePool(operation_nonce) => {
                process_pending_three_pool_donation(operation_nonce).await
            }
            StableInterestDelivery::Amm1(notify_nonce) => {
                process_pending_amm_donation(notify_nonce).await
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StableInterestDelivery {
    Treasury(u128),
    StabilityPool(u128),
    ThreePool(u128),
    Amm1(u64),
}

/// Pin every stablecoin-repayment interest share before an external await.
/// Stable repayments call this from the same state mutation as debt reduction;
/// later timer workers own delivery. Treasury sub-e6 residue is retained per
/// ledger rather than discarded by the e8-to-e6 conversion.
pub(crate) fn pin_stablecoin_interest_distribution_in_state(
    state: &mut crate::state::State,
    interest_e8s: u64,
    collateral_type: Principal,
    token_type: crate::StableTokenType,
    stable_ledger: Principal,
    plan: &crate::state::StableRepaymentV2InterestRoutingPlan,
) -> Result<(), String> {
    pin_stablecoin_interest_distribution_in_state_at(
        state,
        interest_e8s,
        collateral_type,
        token_type,
        stable_ledger,
        plan,
        ic_cdk::api::time(),
        ic_cdk::id(),
    )
    .map(|_| ())
}

pub(crate) fn pin_stablecoin_interest_distribution_in_state_at(
    state: &mut crate::state::State,
    interest_e8s: u64,
    collateral_type: Principal,
    token_type: crate::StableTokenType,
    stable_ledger: Principal,
    plan: &crate::state::StableRepaymentV2InterestRoutingPlan,
    now_ns: u64,
    backend_owner: Principal,
) -> Result<Vec<StableInterestDelivery>, String> {
    use crate::state::{
        AmmDonationPhase, InterestDestination, PendingAmmDonation,
        PendingStabilityPoolInterestMint, PendingThreePoolDonation, StabilityPoolInterestMintPhase,
        ThreePoolDonationPhase, TreasuryPaymentKind,
    };
    if interest_e8s == 0 {
        return Ok(Vec::new());
    }
    let Some(shares) = interest_split_amounts(interest_e8s, &plan.split) else {
        let nonce = state.next_op_nonce_at(now_ns);
        crate::event::record_interest_distribution_share_held_at(
            state,
            crate::state::HeldInterestDistributionShare {
                operation_nonce: nonce,
                amount_e8s: interest_e8s,
                collateral_type,
                destination: None,
                reason: "invalid persisted stable-interest split; retained without dispatch".into(),
            },
            now_ns,
        );
        return Ok(Vec::new());
    };
    let has_stability_pool_share = plan.split.iter().zip(&shares).any(|(recipient, share)| {
        *share > 0 && recipient.destination == InterestDestination::StabilityPool
    });
    let stability_pool = plan.stability_pool.filter(|pool| {
        has_stability_pool_share
            && *pool != Principal::anonymous()
            && plan.icusd_ledger != Principal::anonymous()
    });
    if plan.amm1.is_some() {
        let amm_rows = plan
            .split
            .iter()
            .zip(&shares)
            .filter(|(recipient, share)| {
                **share > 0 && recipient.destination == InterestDestination::Amm1
            })
            .count() as u64;
        if state.amm1_donation_nonce.checked_add(amm_rows).is_none() {
            return Err("AMM donation nonce exhausted; receipt remains held".into());
        }
    }

    let mut deliveries = Vec::new();
    for (recipient, share_e8s) in plan.split.clone().into_iter().zip(shares) {
        if share_e8s == 0 {
            continue;
        }
        match recipient.destination {
            InterestDestination::StabilityPool => {
                if let Some(pool) = stability_pool {
                    let ledger = plan.icusd_ledger;
                    let nonce = state.next_op_nonce_at(now_ns);
                    crate::event::record_sp_interest_mint_queued_at(
                        state,
                        PendingStabilityPoolInterestMint {
                            operation_nonce: nonce,
                            ledger,
                            pool,
                            amount_e8s: share_e8s,
                            collateral_type,
                            memo: management::nonce_to_memo(nonce).0.to_vec(),
                            created_at_time_ns: management::nonce_to_created_at_time(nonce),
                            phase: StabilityPoolInterestMintPhase::MintPending,
                            attempts: 0,
                            last_attempt_at_ns: 0,
                        },
                        now_ns,
                    );
                    deliveries.push(StableInterestDelivery::StabilityPool(nonce));
                } else {
                    let nonce = state.next_op_nonce_at(now_ns);
                    crate::event::record_interest_distribution_share_held_at(
                        state,
                        crate::state::HeldInterestDistributionShare {
                            operation_nonce: nonce,
                            amount_e8s: share_e8s,
                            collateral_type,
                            destination: Some(InterestDestination::StabilityPool),
                            reason: "Stability Pool stable-interest route unavailable when pinned"
                                .into(),
                        },
                        now_ns,
                    );
                }
            }
            InterestDestination::Treasury => {
                if stable_ledger == Principal::anonymous() {
                    let nonce = state.next_op_nonce_at(now_ns);
                    crate::event::record_interest_distribution_share_held_at(
                        state,
                        crate::state::HeldInterestDistributionShare {
                            operation_nonce: nonce,
                            amount_e8s: share_e8s,
                            collateral_type,
                            destination: Some(InterestDestination::Treasury),
                            reason: "stablecoin ledger unavailable when interest was pinned".into(),
                        },
                        now_ns,
                    );
                    continue;
                }
                let (amount_e6, residue) = stable_interest_e8_to_e6(
                    share_e8s,
                    *state
                        .stable_interest_conversion_remainders
                        .get(&stable_ledger)
                        .unwrap_or(&0),
                );
                state.stable_interest_conversion_remainders.insert(stable_ledger, residue);
                if amount_e6 > 0 {
                    let asset = match token_type {
                        crate::StableTokenType::CKUSDT => AssetType::CKUSDT,
                        crate::StableTokenType::CKUSDC => AssetType::CKUSDC,
                    };
                    let operation_id = queue_configurable_treasury_obligation_in_state_at(
                        state,
                        TreasuryPaymentKind::InterestStablecoinTransfer,
                        Some(stable_ledger),
                        plan.stable_treasury,
                        amount_e6,
                        DepositType::InterestRevenue,
                        asset,
                        now_ns,
                        backend_owner,
                    );
                    deliveries.push(StableInterestDelivery::Treasury(operation_id));
                }
            }
            InterestDestination::ThreePool => {
                if let Some(pool) = plan.three_pool {
                    let nonce = state.next_op_nonce_at(now_ns);
                    state.pending_three_pool_donations.insert(
                        nonce,
                        PendingThreePoolDonation {
                            pool,
                            ledger: plan.icusd_ledger,
                            amount_e8s: share_e8s,
                            phase: ThreePoolDonationPhase::MintPending,
                        },
                    );
                    deliveries.push(StableInterestDelivery::ThreePool(nonce));
                } else {
                    let operation_id = queue_configurable_treasury_obligation_in_state_at(
                        state,
                        TreasuryPaymentKind::InterestIcusdMint,
                        Some(plan.icusd_ledger),
                        plan.stable_treasury,
                        share_e8s,
                        DepositType::InterestRevenue,
                        AssetType::ICUSD,
                        now_ns,
                        backend_owner,
                    );
                    deliveries.push(StableInterestDelivery::Treasury(operation_id));
                }
            }
            InterestDestination::Amm1 => {
                if let Some(amm) = plan.amm1 {
                    state.amm1_donation_nonce += 1; // overflow was checked before any outbox write
                    let notify_nonce = state.amm1_donation_nonce;
                    let configured = plan.amm1_pool_id.clone();
                    let (ledger, pool_id, reward_subaccount, mint_op_nonce, phase) =
                        if let Some(pool_id) = configured {
                            let nonce = state.next_op_nonce_at(now_ns);
                            (
                                Some(plan.icusd_ledger),
                                Some(pool_id.clone()),
                                Some(compute_amm_reward_subaccount(&pool_id)),
                                Some(nonce),
                                AmmDonationPhase::MintPending,
                            )
                        } else {
                            (None, None, None, None, AmmDonationPhase::AwaitingPoolConfig)
                        };
                    state.pending_amm_donations.insert(
                        notify_nonce,
                        PendingAmmDonation {
                            amm_canister: amm,
                            ledger,
                            amount_e8s: share_e8s,
                            notify_nonce,
                            mint_op_nonce,
                            pool_id,
                            reward_subaccount,
                            phase,
                        },
                    );
                    deliveries.push(StableInterestDelivery::Amm1(notify_nonce));
                } else {
                    let operation_id = queue_configurable_treasury_obligation_in_state_at(
                        state,
                        TreasuryPaymentKind::InterestIcusdMint,
                        Some(plan.icusd_ledger),
                        plan.stable_treasury,
                        share_e8s,
                        DepositType::InterestRevenue,
                        AssetType::ICUSD,
                        now_ns,
                        backend_owner,
                    );
                    deliveries.push(StableInterestDelivery::Treasury(operation_id));
                }
            }
        }
    }
    Ok(deliveries)
}

fn stable_interest_e8_to_e6(share_e8: u64, prior_remainder: u8) -> (u64, u8) {
    let total = u128::from(share_e8) + u128::from(prior_remainder);
    ((total / 100) as u64, (total % 100) as u8)
}

pub(crate) fn queue_stablecoin_surcharge_in_state(
    state: &mut crate::state::State,
    ledger: Principal,
    treasury: Option<Principal>,
    amount_e6: u64,
    asset_type: AssetType,
) -> u128 {
    queue_configurable_treasury_obligation_in_state(
        state,
        TreasuryPaymentKind::StablecoinRepaySurcharge,
        Some(ledger),
        treasury,
        amount_e6,
        DepositType::LiquidationFee,
        asset_type,
    )
}

/// Queue a receipt-backed manual-liquidation stable surcharge using the exact
/// ledger, treasury recipient, and asset pinned with the liquidation quote.
pub(crate) fn queue_manual_liquidation_stable_surcharge_in_state(
    state: &mut crate::state::State,
    ledger: Principal,
    treasury: Option<Principal>,
    amount_e6: u64,
    token_type: crate::StableTokenType,
) -> Option<u128> {
    queue_manual_liquidation_stable_surcharge_in_state_at(
        state,
        ledger,
        treasury,
        amount_e6,
        token_type,
        ic_cdk::api::time(),
        ic_cdk::id(),
    )
}

pub(crate) fn queue_manual_liquidation_stable_surcharge_in_state_at(
    state: &mut crate::state::State,
    ledger: Principal,
    treasury: Option<Principal>,
    amount_e6: u64,
    token_type: crate::StableTokenType,
    now_ns: u64,
    backend_owner: Principal,
) -> Option<u128> {
    if amount_e6 == 0 {
        return None;
    }
    let asset_type = match token_type {
        crate::StableTokenType::CKUSDT => AssetType::CKUSDT,
        crate::StableTokenType::CKUSDC => AssetType::CKUSDC,
    };
    Some(queue_configurable_treasury_obligation_in_state_at(
        state,
        TreasuryPaymentKind::LiquidationStablecoinSurcharge,
        Some(ledger),
        treasury,
        amount_e6,
        DepositType::LiquidationFee,
        asset_type,
        now_ns,
        backend_owner,
    ))
}

fn queue_pinned_treasury_payment_in_state(
    state: &mut crate::state::State,
    kind: crate::state::TreasuryPaymentKind,
    ledger: Principal,
    recipient: Principal,
    amount_raw: u64,
    deposit_type: DepositType,
    asset_type: AssetType,
) -> u128 {
    let operation_id = state.next_op_nonce();
    crate::event::record_treasury_payment_queued(
        state,
        PendingTreasuryPayment {
            operation_id,
            kind,
            ledger,
            from_owner: ic_cdk::id(),
            from_subaccount: None,
            recipient: Account {
                owner: recipient,
                subaccount: None,
            },
            amount_e8s: amount_raw,
            fee_e8s: None,
            memo: management::nonce_to_memo(operation_id).0.to_vec(),
            created_at_time_ns: management::nonce_to_created_at_time(operation_id),
            deposit_type,
            asset_type,
            deposit_memo: None,
            phase: TreasuryPaymentPhase::TransferPending,
            dispatch_attempts: 0,
            last_dispatch_at_ns: 0,
            transfer_nonce: operation_id,
        },
    );
    operation_id
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
    let Some(mut donation) =
        crate::state::read_state(|s| s.pending_three_pool_donations.get(&op_nonce).cloned())
    else {
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
                    log!(
                        INFO,
                        "[treasury] 3pool donation mint remains pending (nonce {}): {:?}",
                        op_nonce,
                        error
                    );
                    return;
                }
            }
        }
        crate::state::ThreePoolDonationPhase::MintedAwaitingAck { mint_block } => mint_block,
    };
    let result: Result<(Result<(), ThreePoolDonateError>,), _> = ic_cdk::call(
        donation.pool,
        "receive_donation_with_id",
        (
            candid::Nat::from(op_nonce),
            0u8,
            candid::Nat::from(donation.amount_e8s),
        ),
    )
    .await;
    match result {
        Ok((Ok(()),)) => {
            crate::state::mutate_state(|s| {
                s.pending_three_pool_donations.remove(&op_nonce);
            });
            log!(
                INFO,
                "[treasury] 3pool acknowledged donation nonce {} (mint block {})",
                op_nonce,
                mint_block
            );
        }
        Ok((Err(error),)) => log!(
            INFO,
            "[treasury] 3pool donation nonce {} remains pending acknowledgment: {:?}",
            op_nonce,
            error
        ),
        Err((code, message)) => log!(
            INFO,
            "[treasury] 3pool donation nonce {} acknowledgment failed: {:?} {}",
            op_nonce,
            code,
            message
        ),
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
    PoolLocked,
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
            .then(|| {
                s.amm1_pool_id
                    .clone()
                    .map(|pool| (pool, s.icusd_ledger_principal))
            })
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
            (
                None,
                None,
                None,
                None,
                crate::state::AmmDonationPhase::AwaitingPoolConfig,
            )
        };
    crate::state::mutate_state(|s| {
        s.pending_amm_donations
            .entry(notify_nonce)
            .or_insert(crate::state::PendingAmmDonation {
                amm_canister,
                ledger,
                amount_e8s,
                notify_nonce,
                mint_op_nonce,
                pool_id,
                reward_subaccount,
                phase,
            });
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
    if matches!(
        &donation.phase,
        crate::state::AmmDonationPhase::AwaitingPoolConfig
    ) {
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
                let phase = crate::state::AmmDonationPhase::NotifyPending {
                    mint_block: block_index,
                };
                crate::state::mutate_state(|s| {
                    if let Some(row) = s.pending_amm_donations.get_mut(&notify_nonce) {
                        row.phase = phase;
                    }
                });
                donation.phase = crate::state::AmmDonationPhase::NotifyPending {
                    mint_block: block_index,
                };
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
                log!(
                    INFO,
                    "[treasury] AMM donation {} mint remains pending: {:?}",
                    notify_nonce,
                    error
                );
                return;
            }
        }
        donation.pool_id = Some(pool_id);
    }

    let crate::state::AmmDonationPhase::NotifyPending { .. } = donation.phase else {
        return;
    };
    let (Some(pool_id), Some(_ledger), Some(_nonce)) = (
        donation.pool_id.clone(),
        donation.ledger,
        donation.mint_op_nonce,
    ) else {
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
        Ok((Err(error),)) => log!(
            INFO,
            "[treasury] AMM donation {} acknowledgment remains pending: {:?}",
            notify_nonce,
            error
        ),
        Err((code, message)) => log!(
            INFO,
            "[treasury] AMM donation {} acknowledgment failed: {:?} {}",
            notify_nonce,
            code,
            message
        ),
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
    if let Some(operation_id) =
        crate::state::mutate_state(|state| queue_borrowing_fee_in_state(state, fee))
    {
        process_queued_treasury_payment(operation_id).await;
    }
}

/// Route and durably queue the borrowing fee in the same state transition as
/// the vault debt and borrow-journal settlement. The caller must dispatch the
/// returned operation only after that transition has been recorded.
pub fn queue_borrowing_fee_in_state(state: &mut crate::state::State, fee: ICUSD) -> Option<u128> {
    if fee.0 == 0 {
        return None;
    }
    let outcome = plan_fee_routing(state, fee, crate::event::FeeSource::BorrowingFee);
    if outcome.to_remainder.0 == 0 {
        return None;
    }
    let ledger = state.icusd_ledger_principal;
    let treasury = state.treasury_principal;
    Some(queue_configurable_treasury_obligation_in_state(
        state,
        TreasuryPaymentKind::BorrowingFeeMint,
        Some(ledger),
        treasury,
        outcome.to_remainder.to_u64(),
        DepositType::BorrowingFee,
        AssetType::ICUSD,
    ))
}

pub async fn process_queued_treasury_payment(operation_id: u128) {
    process_pending_treasury_payment(operation_id).await;
}

/// Transfer collateral (liquidation fee) to treasury and record the deposit.
pub async fn send_liquidation_fee_to_treasury(
    amount: u64,
    collateral_ledger: Principal,
    asset_type: Option<AssetType>,
) {
    if amount == 0 {
        return;
    }
    let Some(asset_type) = asset_type else {
        crate::state::mutate_state(|s| {
            s.pending_treasury_collateral
                .push((amount, collateral_ledger));
        });
        log!(
            INFO,
            "[treasury] Held {} fee for unsupported asset ledger {}",
            amount,
            collateral_ledger
        );
        return;
    };
    let treasury = read_state(|s| s.treasury_principal);
    if treasury.is_none() {
        let operation_id = queue_configurable_treasury_obligation(
            TreasuryPaymentKind::LiquidationCollateralTransfer,
            Some(collateral_ledger),
            None,
            amount,
            DepositType::LiquidationFee,
            asset_type,
        );
        log!(
            INFO,
            "[treasury] liquidation fee {} retained as no-dispatch treasury obligation {}",
            amount,
            operation_id
        );
        return;
    }
    let treasury = treasury.expect("treasury was checked above");
    let operation_id = queue_pinned_treasury_payment(
        TreasuryPaymentKind::LiquidationCollateralTransfer,
        collateral_ledger,
        treasury,
        amount,
        DepositType::LiquidationFee,
        asset_type,
    );
    process_pending_treasury_payment(operation_id).await;
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

#[cfg(test)]
mod interest_distribution_batch_tests {
    use super::{
        interest_split_amounts, pin_stablecoin_interest_distribution_in_state_at,
        stable_interest_e8_to_e6,
    };
    use crate::state::{
        InterestDestination, InterestRecipient, StableRepaymentV2InterestRoutingPlan, State,
    };

    #[test]
    fn split_rounding_remainder_is_assigned_once_to_last_configured_recipient() {
        let split = vec![
            InterestRecipient {
                destination: InterestDestination::ThreePool,
                bps: 5000,
            },
            InterestRecipient {
                destination: InterestDestination::StabilityPool,
                bps: 4000,
            },
            InterestRecipient {
                destination: InterestDestination::Treasury,
                bps: 1000,
            },
        ];
        // Floor shares are 0, 1, 0; the remaining unit goes to the last
        // nonzero configured recipient. The complete batch therefore owns
        // exactly the source amount.
        let shares = interest_split_amounts(1, &split).unwrap();
        assert_eq!(shares, vec![0, 0, 1]);
        assert_eq!(shares.iter().sum::<u64>(), 1);
    }

    #[test]
    fn empty_split_does_not_invent_a_destination() {
        assert!(interest_split_amounts(99, &[]).is_none());
    }

    #[test]
    fn stable_treasury_conversion_carries_e8_remainder_across_batches() {
        let (first_e6, first_remainder) = stable_interest_e8_to_e6(65, 0);
        assert_eq!((first_e6, first_remainder), (0, 65));
        let (second_e6, second_remainder) = stable_interest_e8_to_e6(40, first_remainder);
        assert_eq!((second_e6, second_remainder), (1, 5));
        let (max_e6, max_remainder) = stable_interest_e8_to_e6(u64::MAX, 99);
        assert!(max_e6 > 0);
        assert!(max_remainder < 100);
    }

    #[test]
    fn invalid_historical_split_is_held_instead_of_dropping_or_overminting() {
        for split in [
            vec![InterestRecipient {
                destination: InterestDestination::Treasury,
                bps: 9_999,
            }],
            vec![InterestRecipient {
                destination: InterestDestination::Treasury,
                bps: 10_001,
            }],
            vec![
                InterestRecipient {
                    destination: InterestDestination::Treasury,
                    bps: u64::MAX,
                },
                InterestRecipient {
                    destination: InterestDestination::ThreePool,
                    bps: 1,
                },
            ],
        ] {
            assert!(interest_split_amounts(100, &split).is_none());
        }
    }

    #[test]
    fn invalid_split_and_missing_stability_pool_are_held_without_partial_dispatch() {
        let stable_ledger = candid::Principal::from_slice(&[8]);
        let token_type = crate::StableTokenType::CKUSDT;
        for plan in [
            StableRepaymentV2InterestRoutingPlan {
                split: vec![InterestRecipient {
                    destination: InterestDestination::Treasury,
                    bps: 9_999,
                }],
                stable_treasury: None,
                icusd_ledger: candid::Principal::from_slice(&[9]),
                stability_pool: None,
                three_pool: None,
                amm1: None,
                amm1_pool_id: None,
            },
            StableRepaymentV2InterestRoutingPlan {
                split: vec![InterestRecipient {
                    destination: InterestDestination::StabilityPool,
                    bps: 10_000,
                }],
                stable_treasury: None,
                icusd_ledger: candid::Principal::from_slice(&[9]),
                stability_pool: None,
                three_pool: None,
                amm1: None,
                amm1_pool_id: None,
            },
        ] {
            let mut state = State::default();
            let nonce_before = state.op_nonce_counter;
            assert!(pin_stablecoin_interest_distribution_in_state_at(
                &mut state,
                100,
                candid::Principal::from_slice(&[7]),
                token_type.clone(),
                stable_ledger,
                &plan,
                10,
                candid::Principal::from_slice(&[10]),
            )
            .is_ok());
            assert!(state.pending_stability_pool_interest_mints.is_empty());
            assert!(state.pending_treasury_payments.is_empty());
            assert!(state.pending_three_pool_donations.is_empty());
            assert!(state.pending_amm_donations.is_empty());
            assert!(state.op_nonce_counter > nonce_before);
            let held = state.held_interest_distribution_shares.values().next().unwrap();
            assert_eq!(held.amount_e8s, 100);
            assert_eq!(held.collateral_type, candid::Principal::from_slice(&[7]));
            assert_eq!(
                held.destination,
                if plan.split[0].destination == InterestDestination::StabilityPool {
                    Some(InterestDestination::StabilityPool)
                } else {
                    None
                }
            );
        }
    }

    #[test]
    fn missing_stable_ledger_holds_treasury_share_without_delivery() {
        let plan = StableRepaymentV2InterestRoutingPlan {
            split: vec![InterestRecipient {
                destination: InterestDestination::Treasury,
                bps: 10_000,
            }],
            stable_treasury: Some(candid::Principal::from_slice(&[6])),
            icusd_ledger: candid::Principal::from_slice(&[9]),
            stability_pool: None,
            three_pool: None,
            amm1: None,
            amm1_pool_id: None,
        };
        let mut state = State::default();
        let deliveries = pin_stablecoin_interest_distribution_in_state_at(
            &mut state,
            1_234,
            candid::Principal::from_slice(&[7]),
            crate::StableTokenType::CKUSDT,
            candid::Principal::anonymous(),
            &plan,
            10,
            candid::Principal::from_slice(&[8]),
        )
        .unwrap();

        assert!(deliveries.is_empty());
        assert!(state.pending_treasury_payments.is_empty());
        let held = state.held_interest_distribution_shares.values().next().unwrap();
        assert_eq!(held.amount_e8s, 1_234);
        assert_eq!(
            held.destination,
            Some(InterestDestination::Treasury)
        );
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

/// Legacy aggregate amounts do not identify whether an earlier mint was
/// accepted. Preserve them for operator reconciliation rather than minting
/// them under a fresh ledger identity after an upgrade.
pub async fn drain_pending_treasury_interest() {
    let amount = read_state(|s| s.pending_treasury_interest.to_u64());
    if amount > 0 {
        log!(
            INFO,
            "[treasury] Holding legacy aggregate interest {} icUSD without a recoverable ledger identity",
            amount
        );
    }
}

/// Transfer pending collateral fees to treasury.
/// Called from the XRC timer tick to drain `pending_treasury_collateral`.
pub async fn drain_pending_treasury_collateral() {
    let pending: Vec<(u64, Principal)> = read_state(|s| s.pending_treasury_collateral.clone());
    for (amount, ledger) in pending {
        log!(
            INFO,
            "[treasury] Holding legacy collateral tuple fee {} for ledger {} without a recoverable ledger identity",
            amount,
            ledger
        );
    }
}

#[cfg(test)]
mod treasury_mint_receipt_tests {
    use super::*;

    fn payment() -> PendingTreasuryPayment {
        let operation_id = 99;
        PendingTreasuryPayment {
            operation_id,
            transfer_nonce: operation_id,
            kind: TreasuryPaymentKind::BorrowingFeeMint,
            ledger: Principal::self_authenticating([1; 32]),
            from_owner: Principal::self_authenticating([9; 32]),
            from_subaccount: None,
            recipient: Account {
                owner: Principal::self_authenticating([2; 32]),
                subaccount: None,
            },
            amount_e8s: 500,
            fee_e8s: None,
            memo: management::nonce_to_memo(operation_id).0.to_vec(),
            created_at_time_ns: management::nonce_to_created_at_time(operation_id),
            deposit_type: DepositType::BorrowingFee,
            asset_type: AssetType::ICUSD,
            deposit_memo: None,
            phase: TreasuryPaymentPhase::TransferPending,
            dispatch_attempts: 1,
            last_dispatch_at_ns: 10,
        }
    }

    fn mint_block(payment: &PendingTreasuryPayment) -> crate::icrc3_proof::DecodedBlock {
        crate::icrc3_proof::DecodedBlock {
            btype: Some("1mint".into()),
            op: "mint".into(),
            from: None,
            to: Some(payment.recipient.clone()),
            spender: None,
            amount: payment.amount_e8s as u128,
            transaction_fee: None,
            fee: None,
            memo: Some(payment.memo.clone()),
            created_at_time: Some(payment.created_at_time_ns),
            expected_allowance: None,
            expires_at: None,
        }
    }

    #[test]
    fn icusd_mint_receipt_requires_exact_1mint_without_transfer_fields() {
        let payment = payment();
        let mut block = mint_block(&payment);
        assert!(validate_treasury_payment_mint_receipt(&block, &payment).is_ok());
        let mut untyped = block.clone();
        untyped.btype = None;
        assert!(validate_treasury_payment_mint_receipt(&untyped, &payment).is_ok());
        let mut zero_fee = block.clone();
        zero_fee.transaction_fee = Some(0);
        zero_fee.fee = Some(0);
        assert!(validate_treasury_payment_mint_receipt(&zero_fee, &payment).is_ok());

        block.btype = Some("1xfer".into());
        assert!(validate_treasury_payment_mint_receipt(&block, &payment).is_err());
        block = mint_block(&payment);
        block.from = Some(Account {
            owner: Principal::self_authenticating([3; 32]),
            subaccount: None,
        });
        assert!(validate_treasury_payment_mint_receipt(&block, &payment).is_err());
        block = mint_block(&payment);
        block.memo = Some(vec![0]);
        assert!(validate_treasury_payment_mint_receipt(&block, &payment).is_err());
        block = mint_block(&payment);
        block.transaction_fee = Some(1);
        assert!(validate_treasury_payment_mint_receipt(&block, &payment).is_err());
        block = mint_block(&payment);
        block.created_at_time = Some(payment.created_at_time_ns + 1);
        assert!(validate_treasury_payment_mint_receipt(&block, &payment).is_err());
        block = mint_block(&payment);
        block.amount += 1;
        assert!(validate_treasury_payment_mint_receipt(&block, &payment).is_err());
    }
}

#[cfg(test)]
mod manual_liquidation_interest_outbox_tests {
    use super::{
        pin_icusd_interest_distribution_in_state_at,
        pin_stablecoin_interest_distribution_in_state_at,
        queue_manual_liquidation_stable_surcharge_in_state_at,
    };
    use crate::state::{
        InterestDestination, InterestRecipient, StableRepaymentV2InterestRoutingPlan, State,
        TreasuryPaymentKind,
    };
    use crate::StableTokenType;
    use candid::Principal;

    fn principal(byte: u8) -> Principal {
        Principal::from_slice(&[byte])
    }

    fn routing(ledger: Principal, treasury: Principal) -> StableRepaymentV2InterestRoutingPlan {
        StableRepaymentV2InterestRoutingPlan {
            split: vec![InterestRecipient {
                destination: InterestDestination::Treasury,
                bps: 10_000,
            }],
            stable_treasury: Some(treasury),
            icusd_ledger: ledger,
            stability_pool: None,
            three_pool: None,
            amm1: None,
            amm1_pool_id: None,
        }
    }

    #[test]
    fn icusd_interest_outbox_survives_interruption_before_dispatch() {
        let ledger = principal(1);
        let treasury = principal(2);
        let collateral = principal(3);
        let plan = routing(ledger, treasury);
        let mut state = State::default();
        pin_icusd_interest_distribution_in_state_at(
            &mut state,
            crate::numeric::ICUSD::new(1_234),
            collateral,
            &plan,
            100,
            principal(8),
        )
        .unwrap();

        // This is the upgrade/interruption boundary: nothing has dispatched,
        // but the exact recipient row is already part of stable State.
        assert_eq!(state.pending_treasury_payments.len(), 1);
        let payment = state.pending_treasury_payments.values().next().unwrap();
        assert_eq!(payment.kind, TreasuryPaymentKind::InterestIcusdMint);
        assert_eq!(payment.ledger, ledger);
        assert_eq!(payment.recipient.owner, treasury);
        assert_eq!(payment.amount_e8s, 1_234);
    }

    #[test]
    fn stable_interest_and_surcharge_are_both_pinned_before_dispatch() {
        let icusd_ledger = principal(4);
        let stable_ledger = principal(5);
        let treasury = principal(6);
        let collateral = principal(7);
        let plan = routing(icusd_ledger, treasury);
        let mut state = State::default();
        pin_stablecoin_interest_distribution_in_state_at(
            &mut state,
            12_345,
            collateral,
            StableTokenType::CKUSDT,
            stable_ledger,
            &plan,
            100,
            principal(8),
        )
        .unwrap();
        queue_manual_liquidation_stable_surcharge_in_state_at(
            &mut state,
            stable_ledger,
            Some(treasury),
            17,
            StableTokenType::CKUSDT,
            100,
            principal(8),
        );

        assert_eq!(state.pending_treasury_payments.len(), 2);
        assert!(state.pending_treasury_payments.values().any(|payment| {
            payment.kind == TreasuryPaymentKind::InterestStablecoinTransfer
                && payment.ledger == stable_ledger
                && payment.recipient.owner == treasury
                && payment.amount_e8s == 123
        }));
        assert!(state.pending_treasury_payments.values().any(|payment| {
            payment.kind == TreasuryPaymentKind::LiquidationStablecoinSurcharge
                && payment.ledger == stable_ledger
                && payment.recipient.owner == treasury
                && payment.amount_e8s == 17
        }));
    }
}

#[cfg(test)]
mod native_icp_transfer_from_proof_tests {
    use super::{
        validate_native_icp_transfer_from_block, NativeIcpBlock, NativeIcpOperation,
        NativeIcpTimestamp, NativeIcpTokens, NativeIcpTransaction,
    };

    fn valid_block() -> NativeIcpBlock {
        NativeIcpBlock {
            parent_hash: None,
            transaction: NativeIcpTransaction {
                memo: 0,
                icrc1_memo: Some(b"inbound-request-7".to_vec()),
                operation: Some(NativeIcpOperation::Transfer {
                    from: vec![1; 32],
                    to: vec![3; 32],
                    spender: Some(vec![2; 32]),
                    amount: NativeIcpTokens { e8s: 5_000_000_000 },
                    fee: NativeIcpTokens { e8s: 10_000 },
                }),
                created_at_time: NativeIcpTimestamp {
                    timestamp_nanos: 1_711_324_800_000_000_123,
                },
            },
            timestamp: NativeIcpTimestamp {
                timestamp_nanos: 1_711_324_800_000_000_456,
            },
        }
    }

    fn verify(block: &NativeIcpBlock) -> Result<(), String> {
        validate_native_icp_transfer_from_block(
            block,
            &[1; 32],
            &[2; 32],
            &[3; 32],
            5_000_000_000,
            10_000,
            b"inbound-request-7",
            1_711_324_800_000_000_123,
        )
    }

    #[test]
    fn native_icp_transfer_from_requires_exact_spender_and_tuple() {
        let block = valid_block();
        assert!(verify(&block).is_ok());

        let mut wrong_source = valid_block();
        if let Some(NativeIcpOperation::Transfer { from, .. }) =
            &mut wrong_source.transaction.operation
        {
            from[0] ^= 1;
        }
        assert!(verify(&wrong_source).is_err());

        let mut wrong_spender = valid_block();
        if let Some(NativeIcpOperation::Transfer {
            spender: Some(spender),
            ..
        }) = &mut wrong_spender.transaction.operation
        {
            spender[0] ^= 1;
        }
        assert!(verify(&wrong_spender).is_err());

        let mut wrong_destination = valid_block();
        if let Some(NativeIcpOperation::Transfer { to, .. }) =
            &mut wrong_destination.transaction.operation
        {
            to[0] ^= 1;
        }
        assert!(verify(&wrong_destination).is_err());

        let mut wrong_amount = valid_block();
        if let Some(NativeIcpOperation::Transfer { amount, .. }) =
            &mut wrong_amount.transaction.operation
        {
            amount.e8s += 1;
        }
        assert!(verify(&wrong_amount).is_err());

        let mut wrong_fee = valid_block();
        if let Some(NativeIcpOperation::Transfer { fee, .. }) = &mut wrong_fee.transaction.operation
        {
            fee.e8s += 1;
        }
        assert!(verify(&wrong_fee).is_err());

        let mut missing_spender = valid_block();
        if let Some(NativeIcpOperation::Transfer { spender, .. }) =
            &mut missing_spender.transaction.operation
        {
            *spender = None;
        }
        assert!(verify(&missing_spender).is_err());

        let mut wrong_memo = valid_block();
        wrong_memo.transaction.icrc1_memo = Some(b"other-request".to_vec());
        assert!(verify(&wrong_memo).is_err());

        let mut wrong_time = valid_block();
        wrong_time.transaction.created_at_time.timestamp_nanos += 1;
        assert!(verify(&wrong_time).is_err());
    }
}
