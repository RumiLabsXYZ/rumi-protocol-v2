//! Hand-typed ICP Ledger/CMC contracts and the pure parts of the ICP
//! fallback state machine.
//!
//! The wire types in this module intentionally do not use generated bindings.
//! They are copied from the repository's pinned ICP Ledger declaration and
//! the official NNS CMC declaration at commit
//! `98a0f4b60fd7dd340ad49848368e3b63a9490b42`.  Keeping the transfer and
//! notification arguments here makes the immutable operation snapshot the
//! source of truth for every retry.

use candid::{CandidType, Int, Nat, Principal};
use serde::Deserialize;

use crate::types::{FixedBytes32, IcpCmcSnapshot};

/// Mainnet NNS CMC.  This is a protocol constant, not a configurable
/// funding destination.
pub const CMC_PRINCIPAL_TEXT: &str = "rkp4c-7iaaa-aaaaa-aaaca-cai";

/// Mainnet ICP Ledger used by the NNS CMC funding protocol.
pub const ICP_LEDGER_PRINCIPAL_TEXT: &str = "ryjl3-tyaaa-aaaaa-aaaba-cai";

/// CMC's top-up memo, encoded as the eight-byte little-endian ICRC memo
/// required by the ICP Ledger transfer.  The numeric value is the ASCII
/// bytes `TPUP` in little-endian order.
pub const TPUP_MEMO: u64 = 1_347_768_404;

/// A rate is not safe to use after this age.  The bound is deliberately
/// independent of the public sample interval.
pub const RATE_MAX_AGE_SECS: u64 = 600;

/// ICP sizing includes ten percent headroom over the requested cycle refill.
pub const HEADROOM_NUMERATOR: u128 = 11;
pub const HEADROOM_DENOMINATOR: u128 = 10;

/// The CMC's top-up refund path charges the original ledger fee and its
/// top-up refund action fee (two more default fees).  The refund transaction
/// itself also carries the default ledger fee.  These constants mirror the
/// official CMC `TOP_UP_CANISTER_REFUND_FEE` contract at the pinned source
/// commit and are used only to verify a refund block, never to infer one.
pub const TOP_UP_REFUND_ACTION_FEE_MULTIPLIER: u128 = 2;

pub fn cmc_principal() -> Principal {
    Principal::from_text(CMC_PRINCIPAL_TEXT).expect("pinned CMC principal must decode")
}

pub fn icp_ledger_principal() -> Principal {
    Principal::from_text(ICP_LEDGER_PRINCIPAL_TEXT)
        .expect("pinned ICP ledger principal must decode")
}

// ─────────────────────────── ICP Ledger wire types ───────────────────────────

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub owner: Principal,
    pub subaccount: Option<Vec<u8>>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TransferArg {
    pub from_subaccount: Option<Vec<u8>>,
    pub to: Account,
    pub amount: Nat,
    pub fee: Option<Nat>,
    pub memo: Option<Vec<u8>>,
    pub created_at_time: Option<u64>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum TransferError {
    BadFee { expected_fee: Nat },
    BadBurn { min_burn_amount: Nat },
    InsufficientFunds { balance: Nat },
    TooOld,
    CreatedInFuture { ledger_time: u64 },
    TemporarilyUnavailable,
    Duplicate { duplicate_of: Nat },
    GenericError { error_code: Nat, message: String },
}

pub type TransferReply = Result<Nat, TransferError>;

/// The typed transaction shape exposed by the ICP Ledger's `get_transactions`
/// endpoint.  It is retained as an alternate authoritative proof seam because
/// it is easier to inspect than the generic `Value` block representation.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct LedgerTransaction {
    pub burn: Option<LedgerBurn>,
    pub kind: String,
    pub mint: Option<LedgerMint>,
    pub approve: Option<LedgerApprove>,
    pub timestamp: u64,
    pub transfer: Option<LedgerTransfer>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct LedgerBurn {
    pub from: Account,
    pub memo: Option<Vec<u8>>,
    pub created_at_time: Option<u64>,
    pub amount: Nat,
    pub spender: Option<Account>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct LedgerMint {
    pub to: Account,
    pub memo: Option<Vec<u8>>,
    pub created_at_time: Option<u64>,
    pub amount: Nat,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct LedgerApprove {
    pub fee: Option<Nat>,
    pub from: Account,
    pub memo: Option<Vec<u8>>,
    pub created_at_time: Option<u64>,
    pub amount: Nat,
    pub expected_allowance: Option<Nat>,
    pub expires_at: Option<u64>,
    pub spender: Account,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct LedgerTransfer {
    pub to: Account,
    pub fee: Option<Nat>,
    pub from: Account,
    pub memo: Option<Vec<u8>>,
    pub created_at_time: Option<u64>,
    pub amount: Nat,
    pub spender: Option<Account>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct GetTransactionsRequest {
    pub start: Nat,
    pub length: Nat,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TransactionRange {
    pub transactions: Vec<LedgerTransaction>,
}

candid::define_function!(
    pub QueryArchiveFn : (GetTransactionsRequest) -> (TransactionRange) query
);

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct GetTransactionsResponse {
    pub first_index: Nat,
    pub log_length: Nat,
    pub transactions: Vec<LedgerTransaction>,
    pub archived_transactions: Vec<ArchivedTransactionRange>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ArchivedTransactionRange {
    pub start: Nat,
    pub length: Nat,
    pub callback: QueryArchiveFn,
}

/// Generic block values used by the ICP Ledger `get_blocks` interface.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum IcpLedgerValue {
    Blob(Vec<u8>),
    Text(String),
    Nat(Nat),
    Nat64(u64),
    Int(Int),
    Array(Vec<IcpLedgerValue>),
    Map(Vec<(String, IcpLedgerValue)>),
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct GetBlocksArgs {
    pub start: Nat,
    pub length: Nat,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BlockRange {
    pub blocks: Vec<IcpLedgerValue>,
}

candid::define_function!(
    pub QueryBlockArchiveFn : (GetBlocksArgs) -> (BlockRange) query
);

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct GetBlocksResponse {
    pub first_index: Nat,
    pub chain_length: u64,
    pub certificate: Option<Vec<u8>>,
    pub blocks: Vec<IcpLedgerValue>,
    pub archived_blocks: Vec<ArchivedBlockRange>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ArchivedBlockRange {
    pub start: Nat,
    pub length: Nat,
    pub callback: QueryBlockArchiveFn,
}

// ─────────────────────────── CMC wire types ───────────────────────────

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct NotifyTopUpArg {
    pub block_index: u64,
    pub canister_id: Principal,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum NotifyError {
    Refunded {
        reason: String,
        block_index: Option<u64>,
    },
    Processing,
    TransactionTooOld(u64),
    InvalidTransaction(String),
    Other {
        error_code: u64,
        error_message: String,
    },
}

pub type NotifyTopUpReply = Result<Nat, NotifyError>;

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct IcpXdrConversionRate {
    pub timestamp_seconds: u64,
    pub xdr_permyriad_per_icp: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct IcpXdrConversionRateResponse {
    pub data: IcpXdrConversionRate,
    pub hash_tree: Vec<u8>,
    pub certificate: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotValidationError {
    AnonymousSource,
    WrongSource,
    /// Version 1 always queries and debits the Sentinel's default ICP
    /// account. A non-default source subaccount would make the cached
    /// balance/reserve and the outbound transfer refer to different
    /// accounts, so it is rejected everywhere rather than silently
    /// selecting a second account.
    SourceSubaccountUnsupported,
    WrongLedgerPrincipal,
    WrongCmcPrincipal,
    WrongCmcAccount,
    WrongMemo,
    ZeroAmount,
    ZeroFee,
    ZeroCreatedAtTime,
    RateZero,
    RateFuture,
    RateStale,
    RateMathMismatch,
    RateOverflow,
}

/// Validates every protocol fact that can be checked from the immutable ICP
/// snapshot alone.  `opened_at_secs` is supplied by operation open/decode so
/// the six-hundred-second freshness promise is retained as a storable
/// relation, not merely checked at the instant of a network call.  The
/// optional `expected_source` is supplied by whole-state validation to bind
/// the snapshot to the actual Sentinel canister identity.
pub fn validate_snapshot(
    snapshot: &IcpCmcSnapshot,
    opened_at_secs: Option<u64>,
    expected_source: Option<Principal>,
) -> Result<(), SnapshotValidationError> {
    if snapshot.source_principal == Principal::anonymous() {
        return Err(SnapshotValidationError::AnonymousSource);
    }
    if snapshot.source_subaccount.is_some() {
        return Err(SnapshotValidationError::SourceSubaccountUnsupported);
    }
    if let Some(expected_source) = expected_source {
        if snapshot.source_principal != expected_source {
            return Err(SnapshotValidationError::WrongSource);
        }
    }
    if snapshot.ledger_principal != icp_ledger_principal() {
        return Err(SnapshotValidationError::WrongLedgerPrincipal);
    }
    if snapshot.cmc_principal != cmc_principal() {
        return Err(SnapshotValidationError::WrongCmcPrincipal);
    }
    if snapshot.cmc_account_identifier != cmc_subaccount(snapshot.target_canister) {
        return Err(SnapshotValidationError::WrongCmcAccount);
    }
    if snapshot.memo != TPUP_MEMO {
        return Err(SnapshotValidationError::WrongMemo);
    }
    if snapshot.amount_e8s == 0 {
        return Err(SnapshotValidationError::ZeroAmount);
    }
    if snapshot.fee_e8s == 0 {
        return Err(SnapshotValidationError::ZeroFee);
    }
    if snapshot.created_at_time_ns == 0 {
        return Err(SnapshotValidationError::ZeroCreatedAtTime);
    }
    if snapshot.rate_xdr_permyriad_per_icp == 0 {
        return Err(SnapshotValidationError::RateZero);
    }
    let created_at_secs = snapshot.created_at_time_ns / 1_000_000_000;
    if snapshot.rate_timestamp_secs > created_at_secs {
        return Err(SnapshotValidationError::RateFuture);
    }
    if created_at_secs.saturating_sub(snapshot.rate_timestamp_secs) > RATE_MAX_AGE_SECS {
        return Err(SnapshotValidationError::RateStale);
    }
    if let Some(opened_at_secs) = opened_at_secs {
        if snapshot.created_at_time_ns / 1_000_000_000 > opened_at_secs {
            return Err(SnapshotValidationError::RateFuture);
        }
        validate_rate(
            IcpXdrConversionRate {
                timestamp_seconds: snapshot.rate_timestamp_secs,
                xdr_permyriad_per_icp: snapshot.rate_xdr_permyriad_per_icp,
            },
            opened_at_secs,
        )
        .map_err(|error| match error {
            RateError::Zero => SnapshotValidationError::RateZero,
            RateError::Future => SnapshotValidationError::RateFuture,
            RateError::Stale => SnapshotValidationError::RateStale,
            RateError::Overflow => SnapshotValidationError::RateOverflow,
        })?;
    }
    let expected = expected_cycles(
        snapshot.amount_e8s,
        IcpXdrConversionRate {
            timestamp_seconds: snapshot.rate_timestamp_secs,
            xdr_permyriad_per_icp: snapshot.rate_xdr_permyriad_per_icp,
        },
    )
    .map_err(|_| SnapshotValidationError::RateOverflow)?;
    if snapshot.expected_cycles != expected {
        return Err(SnapshotValidationError::RateMathMismatch);
    }
    Ok(())
}

// ─────────────────────────── Account/memo/rate helpers ───────────────────────────

/// CMC derives the ICRC-1 subaccount from the canister principal: one byte
/// length, then the principal bytes, then zero padding to 32 bytes.  The
/// length byte is part of the protocol and is not optional padding.
pub fn cmc_subaccount(target: Principal) -> FixedBytes32 {
    let raw = target.as_slice();
    assert!(raw.len() <= 29, "Principal exceeds the IC maximum length");
    let mut bytes = [0u8; 32];
    bytes[0] = raw.len() as u8;
    bytes[1..1 + raw.len()].copy_from_slice(raw);
    FixedBytes32::new(bytes.to_vec()).expect("32-byte CMC subaccount")
}

pub fn cmc_account(target: Principal) -> Account {
    Account {
        owner: cmc_principal(),
        subaccount: Some(cmc_subaccount(target).to_vec()),
    }
}

pub fn tpup_memo_bytes() -> Vec<u8> {
    TPUP_MEMO.to_le_bytes().to_vec()
}

/// Builds the exact transfer wire value from the immutable operation
/// snapshot.  This function intentionally has no caller-provided amount,
/// fee, destination, memo, or timestamp arguments.
pub fn transfer_args(
    snapshot: &IcpCmcSnapshot,
    source: Principal,
) -> Result<TransferArg, SnapshotValidationError> {
    validate_snapshot(snapshot, None, Some(source))?;
    Ok(TransferArg {
        from_subaccount: snapshot.source_subaccount.map(|s| s.to_vec()),
        to: Account {
            owner: snapshot.cmc_principal,
            subaccount: Some(snapshot.cmc_account_identifier.to_vec()),
        },
        amount: Nat::from(snapshot.amount_e8s),
        fee: Some(Nat::from(snapshot.fee_e8s)),
        memo: Some(snapshot.memo.to_le_bytes().to_vec()),
        created_at_time: Some(snapshot.created_at_time_ns),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateError {
    Zero,
    Future,
    Stale,
    Overflow,
}

pub fn validate_rate(
    rate: IcpXdrConversionRate,
    now_secs: u64,
) -> Result<IcpXdrConversionRate, RateError> {
    if rate.xdr_permyriad_per_icp == 0 {
        return Err(RateError::Zero);
    }
    if rate.timestamp_seconds > now_secs {
        return Err(RateError::Future);
    }
    if now_secs.saturating_sub(rate.timestamp_seconds) > RATE_MAX_AGE_SECS {
        return Err(RateError::Stale);
    }
    Ok(rate)
}

fn checked_ceil_div(numerator: u128, denominator: u128) -> Result<u128, RateError> {
    if denominator == 0 {
        return Err(RateError::Zero);
    }
    let adjusted = numerator
        .checked_add(denominator - 1)
        .ok_or(RateError::Overflow)?;
    Ok(adjusted / denominator)
}

pub fn cycles_with_headroom(refill_cycles: u128) -> Result<u128, RateError> {
    let numerator = refill_cycles
        .checked_mul(HEADROOM_NUMERATOR)
        .ok_or(RateError::Overflow)?;
    checked_ceil_div(numerator, HEADROOM_DENOMINATOR)
}

/// Converts a requested refill to e8s using the CMC rate.  The formula is
/// exact for the CMC units: `e8s * xdr_permyriad_per_icp` yields cycles after
/// the 1e8 ICP and 1e12 cycles/XDR scale factors cancel.  The returned amount
/// is checked against the ICRC-1 `nat` value used by our persisted `u64`
/// snapshot.
pub fn icp_amount_e8s_for_cycles(
    refill_cycles: u128,
    rate: IcpXdrConversionRate,
) -> Result<u64, RateError> {
    if rate.xdr_permyriad_per_icp == 0 {
        return Err(RateError::Zero);
    }
    let required_cycles = cycles_with_headroom(refill_cycles)?;
    let amount = checked_ceil_div(required_cycles, rate.xdr_permyriad_per_icp as u128)?;
    u64::try_from(amount).map_err(|_| RateError::Overflow)
}

pub fn expected_cycles(amount_e8s: u64, rate: IcpXdrConversionRate) -> Result<u128, RateError> {
    (amount_e8s as u128)
        .checked_mul(rate.xdr_permyriad_per_icp as u128)
        .ok_or(RateError::Overflow)
}

// ─────────────────────────── Reply classification ───────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferOutcome {
    Confirmed(u64),
    Unknown,
    Quarantined,
    TerminalNoSpend,
}

fn checked_u64(n: &Nat) -> Option<u64> {
    u64::try_from(n.0.clone()).ok()
}

pub fn classify_transfer_reply(reply: TransferReply) -> TransferOutcome {
    match reply {
        Ok(block) => checked_u64(&block)
            .map(TransferOutcome::Confirmed)
            .unwrap_or(TransferOutcome::Unknown),
        Err(TransferError::Duplicate { duplicate_of }) => checked_u64(&duplicate_of)
            .map(TransferOutcome::Confirmed)
            .unwrap_or(TransferOutcome::Unknown),
        Err(TransferError::TemporarilyUnavailable) | Err(TransferError::GenericError { .. }) => {
            TransferOutcome::Unknown
        }
        Err(TransferError::TooOld) => TransferOutcome::Quarantined,
        Err(TransferError::BadFee { .. })
        | Err(TransferError::BadBurn { .. })
        | Err(TransferError::InsufficientFunds { .. })
        | Err(TransferError::CreatedInFuture { .. }) => TransferOutcome::TerminalNoSpend,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotifyOutcome {
    Delivered { cycles: u128 },
    Pending,
    RefundedWithBlock(u64),
    RefundedWithoutBlock,
    Quarantined,
    Unknown,
}

pub fn classify_notify_reply(reply: NotifyTopUpReply) -> NotifyOutcome {
    match reply {
        Ok(cycles) => u128::try_from(cycles.0)
            .map(|cycles| NotifyOutcome::Delivered { cycles })
            .unwrap_or(NotifyOutcome::Unknown),
        Err(NotifyError::Processing) => NotifyOutcome::Pending,
        Err(NotifyError::Refunded {
            block_index: Some(block),
            ..
        }) => NotifyOutcome::RefundedWithBlock(block),
        Err(NotifyError::Refunded {
            block_index: None, ..
        }) => NotifyOutcome::RefundedWithoutBlock,
        Err(NotifyError::TransactionTooOld(_)) | Err(NotifyError::InvalidTransaction(_)) => {
            NotifyOutcome::Quarantined
        }
        Err(NotifyError::Other { .. }) => NotifyOutcome::Unknown,
    }
}

// ─────────────────────────── Authoritative block proof ───────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockProofError {
    NotTransfer,
    MissingField,
    WrongSource,
    WrongDestination,
    WrongAmount,
    WrongFee,
    WrongMemo,
    WrongCreatedAtTime,
    WrongRefundSource,
    WrongRefundDestination,
    WrongRefundAmount,
    WrongRefundFee,
    WrongRefundMemo,
    UnexpectedRefundTimestamp,
    NoRefundAmount,
    UnsupportedValue,
}

fn map_field<'a>(map: &'a [(String, IcpLedgerValue)], name: &str) -> Option<&'a IcpLedgerValue> {
    map.iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

fn map_account(value: &IcpLedgerValue) -> Option<Account> {
    match value {
        // ICRC-3's canonical Account encoding is an Array containing the
        // owner bytes and, when present, the 32-byte subaccount bytes.
        IcpLedgerValue::Array(values) => {
            if !(1..=2).contains(&values.len()) {
                return None;
            }
            let IcpLedgerValue::Blob(owner_bytes) = &values[0] else {
                return None;
            };
            let owner = Principal::from_slice(owner_bytes);
            let subaccount = if values.len() == 2 {
                let IcpLedgerValue::Blob(bytes) = &values[1] else {
                    return None;
                };
                if bytes.len() != 32 {
                    return None;
                }
                Some(bytes.clone())
            } else {
                None
            };
            Some(Account { owner, subaccount })
        }
        // Retain support for the typed record-shaped fixture used by older
        // ledger APIs.  It is stricter than treating an arbitrary map as an
        // account and is not used for the canonical ICRC-3 path above.
        IcpLedgerValue::Map(map) => {
            let owner = match map_field(map, "owner")? {
                IcpLedgerValue::Blob(bytes) => Principal::from_slice(bytes),
                _ => return None,
            };
            let subaccount = match map_field(map, "subaccount")? {
                IcpLedgerValue::Blob(bytes) => Some(bytes.clone()),
                IcpLedgerValue::Array(values) if values.is_empty() => None,
                IcpLedgerValue::Array(values) if values.len() == 1 => {
                    let IcpLedgerValue::Blob(bytes) = &values[0] else {
                        return None;
                    };
                    Some(bytes.clone())
                }
                _ => return None,
            };
            if let Some(bytes) = &subaccount {
                if bytes.len() != 32 {
                    return None;
                }
            }
            Some(Account { owner, subaccount })
        }
        _ => None,
    }
}

fn map_nat(value: Option<&IcpLedgerValue>) -> Option<Nat> {
    match value? {
        IcpLedgerValue::Nat(nat) => Some(nat.clone()),
        IcpLedgerValue::Nat64(value) => Some(Nat::from(*value)),
        IcpLedgerValue::Array(values) if values.len() == 1 => map_nat(values.first()),
        _ => None,
    }
}

fn find_transfer_map(value: &IcpLedgerValue) -> Option<&[(String, IcpLedgerValue)]> {
    match value {
        IcpLedgerValue::Map(map)
            if map_field(map, "from").is_some()
                && map_field(map, "to").is_some()
                && (map_field(map, "amt").is_some() || map_field(map, "amount").is_some()) =>
        {
            Some(map)
        }
        IcpLedgerValue::Map(map) => map.iter().find_map(|(_, value)| find_transfer_map(value)),
        IcpLedgerValue::Array(values) => values.iter().find_map(find_transfer_map),
        _ => None,
    }
}

/// Verifies that a generic ICP Ledger block contains exactly the immutable
/// transfer represented by `snapshot`.  It rejects incomplete values rather
/// than treating a partial match as proof.  A caller must obtain `block` from
/// the ledger's `get_blocks` query (including an archive callback) before
/// invoking this function; this function itself is deliberately pure.
pub fn verify_block_matches_snapshot(
    block: &IcpLedgerValue,
    snapshot: &IcpCmcSnapshot,
    source: Principal,
) -> Result<(), BlockProofError> {
    // Keep a caller/source mismatch distinguishable from malformed snapshot
    // data. The source argument is the runtime identity at the proof seam;
    // it must agree with the immutable snapshot before any block fields are
    // accepted as delivery evidence.
    validate_snapshot(snapshot, None, None).map_err(|_| BlockProofError::UnsupportedValue)?;
    if source != snapshot.source_principal {
        return Err(BlockProofError::WrongSource);
    }
    let map = find_transfer_map(block).ok_or(BlockProofError::NotTransfer)?;
    let from = map_account(map_field(map, "from").ok_or(BlockProofError::MissingField)?)
        .ok_or(BlockProofError::UnsupportedValue)?;
    let to = map_account(map_field(map, "to").ok_or(BlockProofError::MissingField)?)
        .ok_or(BlockProofError::UnsupportedValue)?;
    if from.owner != snapshot.source_principal
        || from.subaccount != snapshot.source_subaccount.map(|value| value.to_vec())
    {
        return Err(BlockProofError::WrongSource);
    }
    let expected_to = Account {
        owner: snapshot.cmc_principal,
        subaccount: Some(snapshot.cmc_account_identifier.to_vec()),
    };
    if to != expected_to {
        return Err(BlockProofError::WrongDestination);
    }
    // ICRC-3 calls the user-supplied transfer amount `amt` and places it in
    // the nested `tx` map.  `amount` is accepted only for the older typed
    // record-shaped block representation used by the repository fixture.
    let amount = map_nat(map_field(map, "amt").or_else(|| map_field(map, "amount")))
        .ok_or(BlockProofError::MissingField)?;
    if amount != Nat::from(snapshot.amount_e8s) {
        return Err(BlockProofError::WrongAmount);
    }
    let fee = map_nat(map_field(map, "fee")).ok_or(BlockProofError::MissingField)?;
    if fee != Nat::from(snapshot.fee_e8s) {
        return Err(BlockProofError::WrongFee);
    }
    let memo = match map_field(map, "memo").ok_or(BlockProofError::MissingField)? {
        IcpLedgerValue::Blob(bytes) => bytes,
        IcpLedgerValue::Array(values) if values.len() == 1 => {
            let IcpLedgerValue::Blob(bytes) = &values[0] else {
                return Err(BlockProofError::UnsupportedValue);
            };
            bytes
        }
        _ => return Err(BlockProofError::UnsupportedValue),
    };
    if memo != &snapshot.memo.to_le_bytes().to_vec() {
        return Err(BlockProofError::WrongMemo);
    }
    // In the canonical ICRC-3 block, `created_at_time` is represented by the
    // `tx.ts` field.  The typed legacy representation uses the original field
    // name, so both are checked against the same immutable nanosecond value.
    let created_at = map_nat(map_field(map, "ts").or_else(|| map_field(map, "created_at_time")))
        .and_then(|value| checked_u64(&value))
        .ok_or(BlockProofError::UnsupportedValue)?;
    if created_at != snapshot.created_at_time_ns {
        return Err(BlockProofError::WrongCreatedAtTime);
    }
    Ok(())
}

pub fn verify_transaction_matches_snapshot(
    transaction: &LedgerTransaction,
    snapshot: &IcpCmcSnapshot,
    source: Principal,
) -> Result<(), BlockProofError> {
    validate_snapshot(snapshot, None, Some(source))
        .map_err(|_| BlockProofError::UnsupportedValue)?;
    let transfer = transaction
        .transfer
        .as_ref()
        .ok_or(BlockProofError::NotTransfer)?;
    let from = &transfer.from;
    if from.owner != snapshot.source_principal
        || from.subaccount != snapshot.source_subaccount.map(|value| value.to_vec())
    {
        return Err(BlockProofError::WrongSource);
    }
    if transfer.to
        != (Account {
            owner: snapshot.cmc_principal,
            subaccount: Some(snapshot.cmc_account_identifier.to_vec()),
        })
    {
        return Err(BlockProofError::WrongDestination);
    }
    if transfer.amount != Nat::from(snapshot.amount_e8s) {
        return Err(BlockProofError::WrongAmount);
    }
    if transfer.fee != Some(Nat::from(snapshot.fee_e8s)) {
        return Err(BlockProofError::WrongFee);
    }
    if transfer.memo.as_deref() != Some(snapshot.memo.to_le_bytes().as_slice()) {
        return Err(BlockProofError::WrongMemo);
    }
    if transfer.created_at_time != Some(snapshot.created_at_time_ns) {
        return Err(BlockProofError::WrongCreatedAtTime);
    }
    Ok(())
}

/// Returns the exact ICP amount the official CMC top-up refund sends back to
/// the original source. The CMC first deducts the original ledger fee and its
/// two-fee top-up refund action charge. `None` means the official CMC cannot
/// emit a refund transfer because the payment is too small.
pub fn expected_refund_amount_e8s(snapshot: &IcpCmcSnapshot) -> Option<u64> {
    let deduction =
        (1u128 + TOP_UP_REFUND_ACTION_FEE_MULTIPLIER).checked_mul(snapshot.fee_e8s as u128)?;
    let amount = (snapshot.amount_e8s as u128).checked_sub(deduction)?;
    u64::try_from(amount).ok().filter(|amount| *amount > 0)
}

/// The source-side net debit after the original transfer and an authoritative
/// CMC refund. This is derived only after `verify_refund_block_matches_snapshot`
/// has proved the refund transaction; it is never accepted as caller input.
pub fn refund_net_debit_e8s(snapshot: &IcpCmcSnapshot) -> Result<u128, BlockProofError> {
    let refund = expected_refund_amount_e8s(snapshot).ok_or(BlockProofError::NoRefundAmount)?;
    (snapshot.amount_e8s as u128)
        .checked_add(snapshot.fee_e8s as u128)
        .and_then(|gross| gross.checked_sub(refund as u128))
        .ok_or(BlockProofError::UnsupportedValue)
}

/// Verifies the CMC's authoritative refund block. The block must be an ICP
/// transfer from the exact CMC top-up account back to the exact source
/// account, for the exact refund amount and ledger fee. CMC's legacy refund
/// transfer has no created-at-time and an empty/default memo; if those fields
/// are present in a generic representation they must be the default value.
pub fn verify_refund_block_matches_snapshot(
    block: &IcpLedgerValue,
    snapshot: &IcpCmcSnapshot,
) -> Result<(), BlockProofError> {
    validate_snapshot(snapshot, None, None).map_err(|_| BlockProofError::UnsupportedValue)?;
    let map = find_transfer_map(block).ok_or(BlockProofError::NotTransfer)?;
    let from = map_account(map_field(map, "from").ok_or(BlockProofError::MissingField)?)
        .ok_or(BlockProofError::UnsupportedValue)?;
    let expected_from = Account {
        owner: snapshot.cmc_principal,
        subaccount: Some(snapshot.cmc_account_identifier.to_vec()),
    };
    if from != expected_from {
        return Err(BlockProofError::WrongRefundSource);
    }
    let to = map_account(map_field(map, "to").ok_or(BlockProofError::MissingField)?)
        .ok_or(BlockProofError::UnsupportedValue)?;
    let expected_to = Account {
        owner: snapshot.source_principal,
        subaccount: snapshot.source_subaccount.map(|value| value.to_vec()),
    };
    if to != expected_to {
        return Err(BlockProofError::WrongRefundDestination);
    }
    let expected_amount =
        expected_refund_amount_e8s(snapshot).ok_or(BlockProofError::NoRefundAmount)?;
    let amount = map_nat(map_field(map, "amt").or_else(|| map_field(map, "amount")))
        .ok_or(BlockProofError::MissingField)?;
    if amount != Nat::from(expected_amount) {
        return Err(BlockProofError::WrongRefundAmount);
    }
    let fee = map_nat(map_field(map, "fee")).ok_or(BlockProofError::MissingField)?;
    if fee != Nat::from(snapshot.fee_e8s) {
        return Err(BlockProofError::WrongRefundFee);
    }
    if let Some(memo) = map_field(map, "memo") {
        let bytes = match memo {
            IcpLedgerValue::Blob(bytes) => bytes,
            IcpLedgerValue::Array(values) if values.is_empty() => &[] as &[u8],
            IcpLedgerValue::Array(values) if values.len() == 1 => {
                let IcpLedgerValue::Blob(bytes) = &values[0] else {
                    return Err(BlockProofError::UnsupportedValue);
                };
                bytes.as_slice()
            }
            _ => return Err(BlockProofError::UnsupportedValue),
        };
        if !(bytes.is_empty() || bytes == 0u64.to_le_bytes()) {
            return Err(BlockProofError::WrongRefundMemo);
        }
    }
    if let Some(created_at) = map_field(map, "ts").or_else(|| map_field(map, "created_at_time")) {
        let created_at = map_nat(Some(created_at))
            .and_then(|value| checked_u64(&value))
            .ok_or(BlockProofError::UnsupportedValue)?;
        if created_at != 0 {
            return Err(BlockProofError::UnexpectedRefundTimestamp);
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockLookupError {
    CallFailed,
    NotFound,
    ResponseOverflow,
    ArchiveCallFailed,
}

/// Queries a block and, when the live range does not contain it, follows the
/// exact archive callback returned by the ledger.  The callback descriptor is
/// part of the official Candid response and is never guessed from a local
/// archive list.
pub async fn query_block(
    ledger: Principal,
    block_index: u64,
) -> Result<IcpLedgerValue, BlockLookupError> {
    let (response,) = ic_cdk::call::<(GetBlocksArgs,), (GetBlocksResponse,)>(
        ledger,
        "get_blocks",
        (GetBlocksArgs {
            start: Nat::from(block_index),
            length: Nat::from(1u8),
        },),
    )
    .await
    .map_err(|_| BlockLookupError::CallFailed)?;
    let first = u64::try_from(response.first_index.0.clone())
        .map_err(|_| BlockLookupError::ResponseOverflow)?;
    if block_index >= first
        && block_index
            < first
                .checked_add(response.blocks.len() as u64)
                .ok_or(BlockLookupError::ResponseOverflow)?
    {
        return response
            .blocks
            .into_iter()
            .nth((block_index - first) as usize)
            .ok_or(BlockLookupError::NotFound);
    }
    for archive in response.archived_blocks {
        let start = u64::try_from(archive.start.0.clone())
            .map_err(|_| BlockLookupError::ResponseOverflow)?;
        let length = u64::try_from(archive.length.0.clone())
            .map_err(|_| BlockLookupError::ResponseOverflow)?;
        let end = start
            .checked_add(length)
            .ok_or(BlockLookupError::ResponseOverflow)?;
        if block_index < start || block_index >= end {
            continue;
        }
        let (range,) = ic_cdk::call::<(GetBlocksArgs,), (BlockRange,)>(
            archive.callback.0.principal,
            &archive.callback.0.method,
            (GetBlocksArgs {
                start: Nat::from(block_index),
                length: Nat::from(1u8),
            },),
        )
        .await
        .map_err(|_| BlockLookupError::ArchiveCallFailed)?;
        return range
            .blocks
            .into_iter()
            .next()
            .ok_or(BlockLookupError::NotFound);
    }
    Err(BlockLookupError::NotFound)
}

pub async fn transfer(
    ledger: Principal,
    snapshot: &IcpCmcSnapshot,
    source: Principal,
) -> Result<TransferOutcome, SnapshotValidationError> {
    if ledger != snapshot.ledger_principal {
        return Err(SnapshotValidationError::WrongLedgerPrincipal);
    }
    let args = transfer_args(snapshot, source)?;
    match ic_cdk::call::<(TransferArg,), (TransferReply,)>(ledger, "icrc1_transfer", (args,)).await
    {
        Ok((reply,)) => Ok(classify_transfer_reply(reply)),
        Err(_) => Ok(TransferOutcome::Unknown),
    }
}

pub async fn notify_top_up(
    snapshot: &IcpCmcSnapshot,
    block_index: u64,
) -> Result<NotifyOutcome, SnapshotValidationError> {
    validate_snapshot(snapshot, None, None)?;
    let args = NotifyTopUpArg {
        block_index,
        canister_id: snapshot.target_canister,
    };
    match ic_cdk::call::<(NotifyTopUpArg,), (NotifyTopUpReply,)>(
        snapshot.cmc_principal,
        "notify_top_up",
        (args,),
    )
    .await
    {
        Ok((reply,)) => Ok(classify_notify_reply(reply)),
        Err(_) => Ok(NotifyOutcome::Unknown),
    }
}

pub async fn query_rate(cmc: Principal) -> Result<IcpXdrConversionRate, RateError> {
    let (response,) =
        ic_cdk::call::<(), (IcpXdrConversionRateResponse,)>(cmc, "get_icp_xdr_conversion_rate", ())
            .await
            .map_err(|_| RateError::Overflow)?;
    Ok(response.data)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheQueryError {
    CallFailed,
    Overflow,
}

pub async fn query_balance_and_fee(
    ledger: Principal,
    source: Principal,
) -> Result<(u128, u128), CacheQueryError> {
    let account = Account {
        owner: source,
        subaccount: None,
    };
    let (balance,) = ic_cdk::call::<(Account,), (Nat,)>(ledger, "icrc1_balance_of", (account,))
        .await
        .map_err(|_| CacheQueryError::CallFailed)?;
    let (fee,) = ic_cdk::call::<(), (Nat,)>(ledger, "icrc1_fee", ())
        .await
        .map_err(|_| CacheQueryError::CallFailed)?;
    Ok((
        u128::try_from(balance.0).map_err(|_| CacheQueryError::Overflow)?,
        u128::try_from(fee.0).map_err(|_| CacheQueryError::Overflow)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    const PINNED_CMC_DID: &str = include_str!("../tests/vendor/cmc_master_98a0f4b6.did");
    const PINNED_CMC_DID_SHA256: [u8; 32] = [
        0xff, 0xd8, 0x43, 0xf0, 0xc5, 0x66, 0x46, 0xdd, 0xe6, 0x35, 0x97, 0xe1, 0x74, 0x49, 0x6f,
        0xf7, 0xc9, 0xa4, 0xc4, 0x7e, 0xdf, 0xe9, 0xab, 0xd2, 0xec, 0xbb, 0x43, 0x18, 0xdd, 0x36,
        0x02, 0x6d,
    ];

    fn sha256(bytes: &[u8]) -> [u8; 32] {
        Sha256::digest(bytes).into()
    }

    fn target() -> Principal {
        Principal::from_slice(&[1, 2, 3, 4])
    }

    #[test]
    fn vendor_fixture_pins_the_official_cmc_contract() {
        assert_eq!(
            sha256(PINNED_CMC_DID.as_bytes()),
            PINNED_CMC_DID_SHA256,
            "the vendored CMC Candid fixture changed without a source-pin update"
        );
        assert!(PINNED_CMC_DID.contains("Commit: 98a0f4b60fd7dd340ad49848368e3b63a9490b42"));
        for field in [
            "notify_top_up: (NotifyTopUpArg) -> (NotifyTopUpResult);",
            "get_icp_xdr_conversion_rate: () -> (IcpXdrConversionRateResponse) query;",
            "block_index: BlockIndex;",
            "canister_id: principal;",
            "Refunded: record { reason: text; block_index: opt BlockIndex };",
            "Processing;",
        ] {
            assert!(
                PINNED_CMC_DID.contains(field),
                "pinned CMC fixture is missing `{field}`"
            );
        }
    }

    fn snapshot() -> IcpCmcSnapshot {
        IcpCmcSnapshot {
            source_principal: Principal::from_slice(&[9]),
            ledger_principal: icp_ledger_principal(),
            cmc_principal: cmc_principal(),
            source_subaccount: None,
            cmc_account_identifier: cmc_subaccount(target()),
            target_canister: target(),
            amount_e8s: 123,
            fee_e8s: 10,
            memo: TPUP_MEMO,
            created_at_time_ns: 999_000_000_000,
            rate_xdr_permyriad_per_icp: 2,
            rate_timestamp_secs: 500,
            expected_cycles: 246,
        }
    }

    #[test]
    fn cmc_account_vector_has_length_prefix_and_zero_padding() {
        let subaccount = cmc_subaccount(target()).to_vec();
        assert_eq!(subaccount[0], 4);
        assert_eq!(&subaccount[1..5], &[1, 2, 3, 4]);
        assert!(subaccount[5..].iter().all(|byte| *byte == 0));
        assert_eq!(cmc_account(target()).owner, cmc_principal());
        assert_eq!(cmc_account(target()).subaccount, Some(subaccount));
    }

    #[test]
    fn tpup_is_exact_little_endian_eight_bytes() {
        assert_eq!(tpup_memo_bytes(), vec![b'T', b'P', b'U', b'P', 0, 0, 0, 0]);
    }

    #[test]
    fn default_source_account_is_enforced_at_every_wire_boundary() {
        let mut snap = snapshot();
        snap.source_subaccount = Some(FixedBytes32::new(vec![7; 32]).unwrap());
        assert_eq!(
            validate_snapshot(&snap, None, None),
            Err(SnapshotValidationError::SourceSubaccountUnsupported)
        );
        assert_eq!(
            transfer_args(&snap, snap.source_principal),
            Err(SnapshotValidationError::SourceSubaccountUnsupported)
        );
        assert_eq!(
            verify_transaction_matches_snapshot(
                &LedgerTransaction {
                    burn: None,
                    kind: "1xfer".into(),
                    mint: None,
                    approve: None,
                    timestamp: 0,
                    transfer: None,
                },
                &snap,
                snap.source_principal,
            ),
            Err(BlockProofError::UnsupportedValue)
        );
    }

    #[test]
    fn rate_freshness_and_checked_headroom() {
        let rate = IcpXdrConversionRate {
            timestamp_seconds: 400,
            xdr_permyriad_per_icp: 3,
        };
        assert_eq!(validate_rate(rate, 1000), Ok(rate));
        assert_eq!(validate_rate(rate, 1001), Err(RateError::Stale));
        assert_eq!(
            validate_rate(
                IcpXdrConversionRate {
                    xdr_permyriad_per_icp: 0,
                    ..rate
                },
                500
            ),
            Err(RateError::Zero)
        );
        assert_eq!(
            validate_rate(
                IcpXdrConversionRate {
                    timestamp_seconds: 501,
                    ..rate
                },
                500
            ),
            Err(RateError::Future)
        );
        assert_eq!(cycles_with_headroom(1), Ok(2));
        assert_eq!(cycles_with_headroom(10), Ok(11));
        assert_eq!(icp_amount_e8s_for_cycles(30, rate), Ok(11));
    }

    #[test]
    fn duplicate_is_original_block_and_transfer_args_are_immutable() {
        let snap = snapshot();
        let args = transfer_args(&snap, Principal::from_slice(&[9])).unwrap();
        assert_eq!(args.to, cmc_account(target()));
        assert_eq!(args.memo, Some(tpup_memo_bytes()));
        assert_eq!(args.amount, Nat::from(123u64));
        assert_eq!(
            classify_transfer_reply(Err(TransferError::Duplicate {
                duplicate_of: Nat::from(77u64),
            })),
            TransferOutcome::Confirmed(77)
        );
        assert_eq!(
            classify_transfer_reply(Err(TransferError::TooOld)),
            TransferOutcome::Quarantined
        );
    }

    #[test]
    fn notify_reply_matrix_keeps_processing_and_uncertainty_distinct() {
        assert_eq!(
            classify_notify_reply(Err(NotifyError::Processing)),
            NotifyOutcome::Pending
        );
        assert_eq!(
            classify_notify_reply(Err(NotifyError::Other {
                error_code: 7,
                error_message: "transport-like error".into(),
            })),
            NotifyOutcome::Unknown
        );
        assert_eq!(
            classify_notify_reply(Err(NotifyError::TransactionTooOld(9))),
            NotifyOutcome::Quarantined
        );
        assert_eq!(
            classify_notify_reply(Err(NotifyError::Refunded {
                reason: "not enough cycles".into(),
                block_index: Some(88),
            })),
            NotifyOutcome::RefundedWithBlock(88)
        );
        assert_eq!(
            classify_notify_reply(Err(NotifyError::Refunded {
                reason: "refund block unavailable".into(),
                block_index: None,
            })),
            NotifyOutcome::RefundedWithoutBlock
        );
    }

    #[test]
    fn generic_block_proof_requires_every_immutable_transfer_field() {
        let snap = snapshot();
        let map = IcpLedgerValue::Map(vec![
            (
                "from".into(),
                IcpLedgerValue::Map(vec![
                    (
                        "owner".into(),
                        IcpLedgerValue::Blob(Principal::from_slice(&[9]).as_slice().to_vec()),
                    ),
                    ("subaccount".into(), IcpLedgerValue::Array(vec![])),
                ]),
            ),
            (
                "to".into(),
                IcpLedgerValue::Map(vec![
                    (
                        "owner".into(),
                        IcpLedgerValue::Blob(cmc_principal().as_slice().to_vec()),
                    ),
                    (
                        "subaccount".into(),
                        IcpLedgerValue::Blob(cmc_subaccount(target()).to_vec()),
                    ),
                ]),
            ),
            ("amount".into(), IcpLedgerValue::Nat(Nat::from(123u64))),
            ("fee".into(), IcpLedgerValue::Nat(Nat::from(10u64))),
            ("memo".into(), IcpLedgerValue::Blob(tpup_memo_bytes())),
            (
                "created_at_time".into(),
                IcpLedgerValue::Nat64(999_000_000_000),
            ),
        ]);
        assert_eq!(
            verify_block_matches_snapshot(&map, &snap, Principal::from_slice(&[9])),
            Ok(())
        );
        assert_eq!(
            verify_block_matches_snapshot(&map, &snap, Principal::from_slice(&[8])),
            Err(BlockProofError::WrongSource)
        );
    }

    #[test]
    fn icrc3_block_proof_uses_canonical_account_and_tx_fields() {
        let snap = snapshot();
        let owner = Principal::from_slice(&[9]);
        let canonical_account = |principal: Principal, subaccount: Option<Vec<u8>>| {
            let mut values = vec![IcpLedgerValue::Blob(principal.as_slice().to_vec())];
            if let Some(subaccount) = subaccount {
                values.push(IcpLedgerValue::Blob(subaccount));
            }
            IcpLedgerValue::Array(values)
        };
        let tx = IcpLedgerValue::Map(vec![
            ("amt".into(), IcpLedgerValue::Nat(Nat::from(123u64))),
            ("from".into(), canonical_account(owner, None)),
            (
                "to".into(),
                canonical_account(cmc_principal(), Some(cmc_subaccount(target()).to_vec())),
            ),
            ("fee".into(), IcpLedgerValue::Nat(Nat::from(10u64))),
            ("memo".into(), IcpLedgerValue::Blob(tpup_memo_bytes())),
            (
                "ts".into(),
                IcpLedgerValue::Nat(Nat::from(999_000_000_000u64)),
            ),
        ]);
        let block = IcpLedgerValue::Map(vec![
            ("btype".into(), IcpLedgerValue::Text("1xfer".into())),
            ("ts".into(), IcpLedgerValue::Nat(Nat::from(1_000u64))),
            ("tx".into(), tx),
        ]);
        assert_eq!(verify_block_matches_snapshot(&block, &snap, owner), Ok(()));
    }
}
