//! Owned by card BB-07. ICRC ledger client, outcome taxonomy and ICRC-3 search.
//!
//! Three layers, kept apart so the safety-critical parts are pure and unit tested:
//!
//! 1. **Calls** (`execute`, `fee`, `balance_of`, `log_length`): thin wrappers over
//!    `ic_cdk::call` that turn every reply into a [`CallOutcome`]. Only a typed ledger
//!    reply is "no effect"; any reject, trap or undecodable reply is `Ambiguous`.
//! 2. **Classification** (`classify`): the pure table that turns an outcome plus the
//!    operation's history into a [`Resolution`]. After any ambiguous attempt even a typed
//!    no-effect reply is not proof, because the earlier attempt may have executed.
//! 3. **ICRC-3 search** (`search`): bounded, resumable scan of ledger history for the exact
//!    block an operation would have created. Absence is proven only by a complete scan to
//!    the tip after the ledger's deduplication window (plus margins) has closed.
//!
//! Inter-canister calls exist only on wasm32. On the host every call returns an error or
//! `Ambiguous`, so `cargo test --lib` builds and the pure logic (plus a mock transport for
//! the search) is exercised without PocketIC.

use crate::state::{LedgerOp, LedgerOpKind};
use candid::utils::{ArgumentDecoder, ArgumentEncoder};
use candid::{Nat, Principal};
use icrc_ledger_types::icrc::generic_value::ICRC3Value;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{Memo, TransferArg, TransferError};
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
use icrc_ledger_types::icrc3::archive::{GetArchivesArgs, GetArchivesResult};
use icrc_ledger_types::icrc3::blocks::{BlockWithId, GetBlocksRequest, GetBlocksResult};
use num_traits::ToPrimitive;

/// Raw result of one ledger call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallOutcome {
    /// `Ok(block)` or `Duplicate { duplicate_of }`.
    Accepted { block_index: u64 },
    /// A typed ledger error: nothing was applied by this call.
    Rejected(LedgerRejection),
    /// The call was rejected by the system, trapped, or returned something we cannot
    /// interpret. The transfer may or may not have executed.
    Ambiguous(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LedgerRejection {
    BadFee { expected_fee: u64 },
    InsufficientFunds { balance: u128 },
    InsufficientAllowance { allowance: u128 },
    TooOld,
    CreatedInFuture,
    TemporarilyUnavailable,
    GenericError { code: u64, message: String },
    Other(String),
}

/// What the flow should do with an operation after a call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    Succeeded { block_index: u64 },
    /// Nothing moved and the same tuple may be sent again later.
    RetrySameTuple(String),
    /// Nothing moved, and the tuple must be rebuilt (new fee or timestamp).
    RebuildTuple(LedgerRejection),
    /// Nothing moved and the operation cannot succeed (e.g. depositor lacks funds).
    Failed(String),
    /// Outcome unknown; resolve only by `Accepted` or a complete search.
    Unknown(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchOutcome {
    Found { block_index: u64 },
    /// Searched every block up to the current tip; `next_block` resumes the search.
    NotFoundYet { next_block: u64, tip: u64 },
    /// Searched to the tip after the dedup window closed: the op never executed.
    ProvenAbsent,
    Error(String),
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// The ICRC-1 ledger transaction window: a `created_at_time` older than this (plus drift)
/// is rejected with `TooOld`.
pub const TX_WINDOW_NS: u64 = 24 * 60 * 60 * 1_000_000_000;
/// Permitted clock drift between caller and ledger.
pub const PERMITTED_DRIFT_NS: u64 = 2 * 60 * 1_000_000_000;
/// Extra slack before absence is declared (subnet clock skew, query lag).
pub const SAFETY_MARGIN_NS: u64 = 10 * 60 * 1_000_000_000;
/// At most this many archive ranges are fetched by one `search` call. Ranges beyond the
/// cap are not scanned (the call reports progress up to them and resumes next time).
pub const MAX_ARCHIVE_RANGES_PER_CALL: usize = 8;

/// True once the ledger can no longer accept a transaction stamped `created_at_time`
/// (window, drift and safety margin all elapsed). Only then can a complete scan to the
/// tip prove that the transaction never executed.
pub fn dedup_window_closed(created_at_time: u64, now_ns: u64) -> bool {
    now_ns > created_at_time.saturating_add(TX_WINDOW_NS + PERMITTED_DRIFT_NS + SAFETY_MARGIN_NS)
}

// ---------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------

/// Pure decision table. See the module docs for the rule behind each row.
///
/// | outcome                                   | first attempt          | after an ambiguous attempt |
/// | ----------------------------------------- | ---------------------- | -------------------------- |
/// | `Accepted`                                | `Succeeded`            | `Succeeded`                |
/// | `Ambiguous`                               | `Unknown`              | `Unknown`                  |
/// | `Rejected(InsufficientFunds)`             | `Failed`               | `Unknown`                  |
/// | `Rejected(InsufficientAllowance)`         | `Failed`               | `Unknown`                  |
/// | `Rejected(TemporarilyUnavailable)`        | `RetrySameTuple`       | `Unknown`                  |
/// | `Rejected(BadFee / TooOld / CreatedInFuture)` | `RebuildTuple`     | `Unknown`                  |
/// | `Rejected(GenericError / Other)`          | `Unknown`              | `Unknown`                  |
pub fn classify(op: &LedgerOp, outcome: &CallOutcome) -> Resolution {
    match outcome {
        CallOutcome::Accepted { block_index } => Resolution::Succeeded { block_index: *block_index },
        CallOutcome::Ambiguous(why) => Resolution::Unknown(why.clone()),
        CallOutcome::Rejected(rejection) => {
            if op.ambiguous_seen {
                return Resolution::Unknown(format!(
                    "typed ledger reply {rejection:?} after an earlier ambiguous attempt; the earlier attempt may have executed"
                ));
            }
            match rejection {
                LedgerRejection::InsufficientFunds { balance } => {
                    Resolution::Failed(format!("ledger reports insufficient funds (balance {balance})"))
                }
                LedgerRejection::InsufficientAllowance { allowance } => {
                    Resolution::Failed(format!("ledger reports insufficient allowance (allowance {allowance})"))
                }
                LedgerRejection::TemporarilyUnavailable => {
                    Resolution::RetrySameTuple("ledger temporarily unavailable".into())
                }
                LedgerRejection::BadFee { .. } | LedgerRejection::TooOld | LedgerRejection::CreatedInFuture => {
                    Resolution::RebuildTuple(rejection.clone())
                }
                LedgerRejection::GenericError { code, message } => {
                    Resolution::Unknown(format!("ledger generic error {code}: {message}"))
                }
                LedgerRejection::Other(message) => Resolution::Unknown(format!("unclassified ledger reply: {message}")),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Transfer calls
// ---------------------------------------------------------------------------

/// Candid call wrapper. Any system reject or decode failure is reported as an error
/// string; callers treat it as ambiguous (the callee may have executed).
#[cfg(target_arch = "wasm32")]
async fn call_candid<A, R>(canister: Principal, method: &str, args: A) -> Result<R, String>
where
    A: ArgumentEncoder,
    R: for<'a> ArgumentDecoder<'a>,
{
    ic_cdk::call(canister, method, args)
        .await
        .map_err(|(code, message)| format!("{method} on {canister} failed ({code:?}): {message}"))
}

#[cfg(not(target_arch = "wasm32"))]
async fn call_candid<A, R>(canister: Principal, method: &str, _args: A) -> Result<R, String>
where
    A: ArgumentEncoder,
    R: for<'a> ArgumentDecoder<'a>,
{
    Err(format!("{method} on {canister}: inter-canister calls are unavailable off-chain"))
}

fn nat_to_u64(n: &Nat) -> Option<u64> {
    n.0.to_u64()
}

fn nat_to_u128(n: &Nat) -> Option<u128> {
    n.0.to_u128()
}

/// Typed ledger reply, common to `TransferError` and `TransferFromError`.
enum TypedReply {
    /// `Duplicate { duplicate_of }`: the same tuple already executed at this block.
    Duplicate(Nat),
    Rejection(LedgerRejection),
}

fn bad_fee(expected_fee: &Nat) -> LedgerRejection {
    match nat_to_u64(expected_fee) {
        Some(expected_fee) => LedgerRejection::BadFee { expected_fee },
        None => LedgerRejection::Other(format!("BadFee with unrepresentable expected_fee {expected_fee}")),
    }
}

fn insufficient_funds(balance: &Nat) -> LedgerRejection {
    match nat_to_u128(balance) {
        Some(balance) => LedgerRejection::InsufficientFunds { balance },
        None => LedgerRejection::Other(format!("InsufficientFunds with unrepresentable balance {balance}")),
    }
}

fn generic_error(error_code: &Nat, message: String) -> LedgerRejection {
    LedgerRejection::GenericError { code: nat_to_u64(error_code).unwrap_or(u64::MAX), message }
}

fn typed_from_transfer_error(error: TransferError) -> TypedReply {
    TypedReply::Rejection(match error {
        TransferError::Duplicate { duplicate_of } => return TypedReply::Duplicate(duplicate_of),
        TransferError::BadFee { expected_fee } => bad_fee(&expected_fee),
        TransferError::BadBurn { min_burn_amount } => {
            LedgerRejection::Other(format!("BadBurn (min_burn_amount {min_burn_amount})"))
        }
        TransferError::InsufficientFunds { balance } => insufficient_funds(&balance),
        TransferError::TooOld => LedgerRejection::TooOld,
        TransferError::CreatedInFuture { .. } => LedgerRejection::CreatedInFuture,
        TransferError::TemporarilyUnavailable => LedgerRejection::TemporarilyUnavailable,
        TransferError::GenericError { error_code, message } => generic_error(&error_code, message),
    })
}

fn typed_from_transfer_from_error(error: TransferFromError) -> TypedReply {
    TypedReply::Rejection(match error {
        TransferFromError::Duplicate { duplicate_of } => return TypedReply::Duplicate(duplicate_of),
        TransferFromError::BadFee { expected_fee } => bad_fee(&expected_fee),
        TransferFromError::BadBurn { min_burn_amount } => {
            LedgerRejection::Other(format!("BadBurn (min_burn_amount {min_burn_amount})"))
        }
        TransferFromError::InsufficientFunds { balance } => insufficient_funds(&balance),
        TransferFromError::InsufficientAllowance { allowance } => match nat_to_u128(&allowance) {
            Some(allowance) => LedgerRejection::InsufficientAllowance { allowance },
            None => LedgerRejection::Other(format!("InsufficientAllowance with unrepresentable allowance {allowance}")),
        },
        TransferFromError::TooOld => LedgerRejection::TooOld,
        TransferFromError::CreatedInFuture { .. } => LedgerRejection::CreatedInFuture,
        TransferFromError::TemporarilyUnavailable => LedgerRejection::TemporarilyUnavailable,
        TransferFromError::GenericError { error_code, message } => generic_error(&error_code, message),
    })
}

fn accepted(block: &Nat, what: &str) -> CallOutcome {
    match nat_to_u64(block) {
        Some(block_index) => CallOutcome::Accepted { block_index },
        // The ledger executed (or deduplicated) the call but the index is unusable: never
        // report block 0, and let the search find the real block.
        None => CallOutcome::Ambiguous(format!("{what} block index {block} does not fit in u64")),
    }
}

fn outcome_of(reply: Result<Result<Nat, TypedReply>, String>) -> CallOutcome {
    match reply {
        Err(why) => CallOutcome::Ambiguous(why),
        Ok(Ok(block)) => accepted(&block, "transfer"),
        Ok(Err(TypedReply::Duplicate(block))) => accepted(&block, "duplicate_of"),
        Ok(Err(TypedReply::Rejection(rejection))) => CallOutcome::Rejected(rejection),
    }
}

/// Map the decoded reply of `icrc1_transfer` (or a call failure) to an outcome.
pub fn outcome_from_transfer(reply: Result<Result<Nat, TransferError>, String>) -> CallOutcome {
    outcome_of(reply.map(|inner| inner.map_err(typed_from_transfer_error)))
}

/// Map the decoded reply of `icrc2_transfer_from` (or a call failure) to an outcome.
pub fn outcome_from_transfer_from(reply: Result<Result<Nat, TransferFromError>, String>) -> CallOutcome {
    outcome_of(reply.map(|inner| inner.map_err(typed_from_transfer_from_error)))
}

/// The exact `icrc2_transfer_from` arguments for a pull operation.
pub fn transfer_from_args(op: &LedgerOp, spender: &Account) -> TransferFromArgs {
    TransferFromArgs {
        spender_subaccount: spender.subaccount,
        from: op.from,
        to: op.to,
        amount: Nat::from(op.amount_e8s),
        fee: op.fee_e8s.map(Nat::from),
        memo: Some(Memo::from(op.memo.clone())),
        created_at_time: Some(op.created_at_time),
    }
}

/// The exact `icrc1_transfer` arguments for a release, refund or sweep operation.
pub fn transfer_args(op: &LedgerOp) -> TransferArg {
    TransferArg {
        from_subaccount: op.from.subaccount,
        to: op.to,
        fee: op.fee_e8s.map(Nat::from),
        created_at_time: Some(op.created_at_time),
        memo: Some(Memo::from(op.memo.clone())),
        amount: Nat::from(op.amount_e8s),
    }
}

/// Send one attempt of the persisted tuple. The tuple is taken from `op` unchanged so the
/// ledger's deduplication applies to every retry.
pub async fn execute(ledger: Principal, op: &LedgerOp) -> CallOutcome {
    match op.kind {
        LedgerOpKind::Pull => {
            let Some(spender) = op.spender.as_ref() else {
                // No call is made, so nothing could have executed; classification treats
                // an unclassified rejection conservatively.
                return CallOutcome::Rejected(LedgerRejection::Other("pull operation has no spender".into()));
            };
            let args = transfer_from_args(op, spender);
            let reply: Result<(Result<Nat, TransferFromError>,), String> =
                call_candid(ledger, "icrc2_transfer_from", (args,)).await;
            outcome_from_transfer_from(reply.map(|(inner,)| inner))
        }
        LedgerOpKind::Release | LedgerOpKind::Refund | LedgerOpKind::Sweep => {
            let args = transfer_args(op);
            let reply: Result<(Result<Nat, TransferError>,), String> =
                call_candid(ledger, "icrc1_transfer", (args,)).await;
            outcome_from_transfer(reply.map(|(inner,)| inner))
        }
    }
}

// ---------------------------------------------------------------------------
// Read-only queries
// ---------------------------------------------------------------------------

pub async fn fee(ledger: Principal) -> Result<u64, String> {
    let (fee,): (Nat,) = call_candid(ledger, "icrc1_fee", ()).await?;
    nat_to_u64(&fee).ok_or_else(|| format!("ledger fee {fee} does not fit in u64"))
}

pub async fn balance_of(ledger: Principal, account: Account) -> Result<u128, String> {
    let (balance,): (Nat,) = call_candid(ledger, "icrc1_balance_of", (account,)).await?;
    nat_to_u128(&balance).ok_or_else(|| format!("ledger balance {balance} does not fit in u128"))
}

pub async fn log_length(ledger: Principal) -> Result<u64, String> {
    let request = vec![GetBlocksRequest { start: Nat::from(0u64), length: Nat::from(0u64) }];
    let (response,): (GetBlocksResult,) = call_candid(ledger, "icrc3_get_blocks", (request,)).await?;
    nat_to_u64(&response.log_length).ok_or_else(|| format!("ledger log length {} does not fit in u64", response.log_length))
}

// ---------------------------------------------------------------------------
// ICRC-3 block decoding and matching (pure)
// ---------------------------------------------------------------------------

/// The fields of a transfer block that an operation is matched on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedBlock {
    /// Raw top-level `btype` (for example `1xfer`, `2xfer`) when present.
    pub btype: Option<String>,
    /// Normalized operation (`xfer` for both `1xfer` and `2xfer`).
    pub op: String,
    pub from: Option<Account>,
    pub to: Option<Account>,
    pub spender: Option<Account>,
    pub amount: u128,
    pub fee: Option<u64>,
    /// The caller-supplied `created_at_time` (`tx.ts`), not the ledger's block time.
    pub created_at_time: Option<u64>,
    pub memo: Option<Vec<u8>>,
}

fn text_of(v: &ICRC3Value) -> Option<&str> {
    match v {
        ICRC3Value::Text(t) => Some(t.as_str()),
        _ => None,
    }
}

/// Strip the ICRC-3 schema prefix (`1xfer` -> `xfer`) and lowercase.
fn normalize_op(raw: &str) -> String {
    raw.trim_start_matches(|c: char| c.is_ascii_digit()).to_ascii_lowercase()
}

fn normalize_semantic_op(raw: &str) -> String {
    let normalized = normalize_op(raw);
    if normalized == "transfer" {
        "xfer".into()
    } else {
        normalized
    }
}

fn account_from_value(v: &ICRC3Value) -> Result<Account, String> {
    let ICRC3Value::Array(parts) = v else {
        return Err("account is not an Array".into());
    };
    if parts.is_empty() || parts.len() > 2 {
        return Err(format!("account array must have 1 or 2 elements, got {}", parts.len()));
    }
    let ICRC3Value::Blob(owner) = &parts[0] else {
        return Err("account owner is not a Blob".into());
    };
    let owner = Principal::try_from_slice(owner.as_ref()).map_err(|e| format!("account owner is not a principal: {e}"))?;
    let subaccount = match parts.get(1) {
        None => None,
        Some(ICRC3Value::Blob(sub)) => {
            let bytes: [u8; 32] = sub.as_ref().try_into().map_err(|_| "subaccount is not 32 bytes".to_string())?;
            Some(bytes)
        }
        Some(_) => return Err("account subaccount is not a Blob".into()),
    };
    Ok(Account { owner, subaccount })
}

fn nat_value_u128(v: &ICRC3Value) -> Result<u128, String> {
    match v {
        ICRC3Value::Nat(n) => nat_to_u128(n).ok_or_else(|| format!("Nat {n} does not fit in u128")),
        _ => Err("expected a Nat value".into()),
    }
}

fn nat_value_u64(v: &ICRC3Value, what: &str) -> Result<u64, String> {
    u64::try_from(nat_value_u128(v)?).map_err(|_| format!("{what} does not fit in u64"))
}

/// Decode one ICRC-3 block.
///
/// * `Ok(None)`: the block is positively identified as something other than a transfer
///   (mint, burn, approve, fee collector, ...) and cannot be the block we are looking for.
/// * `Ok(Some(_))`: a fully decoded transfer block.
/// * `Err(_)`: the block cannot be classified, or it is a transfer whose fields cannot be
///   decoded. The caller must not treat such a block as "not ours": the search reports an
///   error instead of advancing past it.
///
/// The operation comes from the top-level `btype` (digits stripped) and/or `tx.op`; when
/// both exist they must agree.
pub fn decode_transfer_block(value: &ICRC3Value) -> Result<Option<DecodedBlock>, String> {
    let ICRC3Value::Map(block_map) = value else {
        return Err("block is not a Map".into());
    };
    let btype = match block_map.get("btype") {
        Some(ICRC3Value::Text(t)) => Some(t.clone()),
        Some(_) => return Err("block 'btype' is not Text".into()),
        None => None,
    };
    let tx_map = match block_map.get("tx") {
        Some(ICRC3Value::Map(m)) => Some(m),
        Some(_) => return Err("block 'tx' is not a Map".into()),
        None => None,
    };
    let tx_op = match tx_map.and_then(|m| m.get("op")) {
        Some(v) => Some(text_of(v).ok_or_else(|| "tx 'op' is not Text".to_string())?),
        None => None,
    };
    let op = match (btype.as_deref(), tx_op) {
        (Some(b), Some(o)) => {
            let (b, o) = (normalize_semantic_op(b), normalize_semantic_op(o));
            if b != o {
                return Err("block type and transaction operation disagree".into());
            }
            b
        }
        (Some(b), None) => normalize_semantic_op(b),
        (None, Some(o)) => normalize_semantic_op(o),
        (None, None) => return Err("block has neither top-level 'btype' nor tx.'op'".into()),
    };
    if op != "xfer" {
        return Ok(None);
    }

    let tx = tx_map.ok_or_else(|| "transfer block has no 'tx' map".to_string())?;
    let account = |key: &str| tx.get(key).map(account_from_value).transpose();
    let from = account("from")?;
    let to = account("to")?;
    let spender = account("spender")?;
    let amount = nat_value_u128(tx.get("amt").ok_or_else(|| "transfer tx has no 'amt'".to_string())?)?;
    // Standard ICRC-3 records the fee in `tx` only when the caller supplied one; otherwise
    // the ledger-charged fee is a block-level field.
    let fee = tx
        .get("fee")
        .or_else(|| block_map.get("fee"))
        .map(|v| nat_value_u64(v, "fee"))
        .transpose()?;
    let created_at_time = tx
        .get("ts")
        .or_else(|| tx.get("created_at_time"))
        .map(|v| nat_value_u64(v, "created_at_time"))
        .transpose()?;
    let memo = match tx.get("memo") {
        Some(ICRC3Value::Blob(b)) => Some(b.to_vec()),
        Some(_) => return Err("tx 'memo' is not a Blob".into()),
        None => None,
    };
    Ok(Some(DecodedBlock { btype, op, from, to, spender, amount, fee, created_at_time, memo }))
}

/// Account equality with default-subaccount equivalence (`None == Some([0; 32])`).
fn same_account(a: &Account, b: &Account) -> bool {
    a.owner == b.owner && a.effective_subaccount() == b.effective_subaccount()
}

fn same_optional_account(block: &Option<Account>, expected: &Account) -> bool {
    block.as_ref().is_some_and(|a| same_account(a, expected))
}

/// True iff `block` is exactly the transfer that `op` would have created.
///
/// Pull: a `2xfer` with `spender`, `from`, `to`, amount, memo and `created_at_time` all
/// equal to the operation. Release, refund and sweep: a `1xfer` (no spender) with the same
/// fields. When the operation pins a fee and the block records one, they must be equal.
pub fn block_matches_op(op: &LedgerOp, block: &DecodedBlock) -> bool {
    if block.op != "xfer" {
        return false;
    }
    let expected_btype = if op.kind == LedgerOpKind::Pull { "2xfer" } else { "1xfer" };
    if block.btype.as_deref().is_some_and(|b| !b.eq_ignore_ascii_case(expected_btype)) {
        return false;
    }
    let spender_ok = match (op.kind, op.spender.as_ref()) {
        (LedgerOpKind::Pull, Some(expected)) => same_optional_account(&block.spender, expected),
        (LedgerOpKind::Pull, None) => false,
        _ => block.spender.is_none(),
    };
    let fee_ok = match (op.fee_e8s, block.fee) {
        (Some(expected), Some(recorded)) => expected == recorded,
        _ => true,
    };
    spender_ok
        && same_optional_account(&block.from, &op.from)
        && same_optional_account(&block.to, &op.to)
        && block.amount == u128::from(op.amount_e8s)
        && block.memo.as_deref().unwrap_or(&[]) == op.memo.as_slice()
        && block.created_at_time == Some(op.created_at_time)
        && fee_ok
}

/// Scan `blocks` (ids `first_id`, `first_id + 1`, ...) for the operation's block. Decode
/// failures are remembered in `decode_error` (first one wins) and scanning continues: a
/// later exact match is still valid evidence, but the caller must not advance past an
/// undecodable block without a match.
fn scan_blocks<'a>(
    op: &LedgerOp,
    first_id: u64,
    blocks: impl IntoIterator<Item = &'a ICRC3Value>,
    decode_error: &mut Option<String>,
) -> Option<u64> {
    for (offset, value) in blocks.into_iter().enumerate() {
        let id = first_id.saturating_add(offset as u64);
        match decode_transfer_block(value) {
            Ok(Some(decoded)) if block_matches_op(op, &decoded) => return Some(id),
            Ok(_) => {}
            Err(e) => {
                decode_error.get_or_insert_with(|| format!("block {id} cannot be decoded: {e}"));
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Scan planning and archive validation (pure)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SegmentSource {
    /// The blocks returned inline by the ledger (all of `response.blocks`).
    Inline,
    /// An archived range the ledger directed us to, fetched from `canister`.
    Archive { canister: Principal, method: String },
}

/// A contiguous run `[start, end)` of block ids and where to read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub start: u64,
    pub end: u64,
    pub source: SegmentSource,
}

/// The portion of a `icrc3_get_blocks` response that a search may use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanPlan {
    /// The ledger's current log length.
    pub tip: u64,
    /// Exclusive end of the requested window, clamped to the tip.
    pub window_end: u64,
    /// Segments in id order, contiguous from the requested start, at most
    /// [`MAX_ARCHIVE_RANGES_PER_CALL`] of them archived. Anything after the first gap (or
    /// beyond the archive cap) is left for a later call.
    pub segments: Vec<Segment>,
}

/// Validate a ledger response to a single-range request `[from_block, from_block +
/// max_blocks)` and extract the usable contiguous prefix.
///
/// Rejects (with an error, never silently): a cursor beyond the tip, inline ids that are
/// not consecutive, ranges outside the requested window, empty ranges, empty callback
/// method names, and overlapping ranges.
pub fn plan_scan(from_block: u64, max_blocks: u64, response: &GetBlocksResult) -> Result<ScanPlan, String> {
    let tip = nat_to_u64(&response.log_length).ok_or_else(|| "ledger log length does not fit in u64".to_string())?;
    if from_block > tip {
        return Err(format!("search cursor {from_block} is beyond the ledger tip {tip}"));
    }
    let window_end = from_block.saturating_add(max_blocks).min(tip);

    let mut segments: Vec<Segment> = Vec::new();
    if let Some(first) = response.blocks.first() {
        let first_id = nat_to_u64(&first.id).ok_or_else(|| "inline block id does not fit in u64".to_string())?;
        for (offset, block) in response.blocks.iter().enumerate() {
            let id = nat_to_u64(&block.id).ok_or_else(|| "inline block id does not fit in u64".to_string())?;
            if first_id.checked_add(offset as u64) != Some(id) {
                return Err(format!("inline block ids are not consecutive (found {id} at position {offset} after {first_id})"));
            }
        }
        let end = first_id
            .checked_add(response.blocks.len() as u64)
            .ok_or_else(|| "inline block range overflows u64".to_string())?;
        segments.push(Segment { start: first_id, end, source: SegmentSource::Inline });
    }
    for archived in &response.archived_blocks {
        if archived.callback.method.trim().is_empty() {
            return Err("archive callback method is empty".into());
        }
        for range in &archived.args {
            let (start, length) = range.as_start_and_length()?;
            if length == 0 {
                return Err("archive range is empty".into());
            }
            let end = start.checked_add(length).ok_or_else(|| "archive range overflows u64".to_string())?;
            segments.push(Segment {
                start,
                end,
                source: SegmentSource::Archive {
                    canister: archived.callback.canister_id,
                    method: archived.callback.method.clone(),
                },
            });
        }
    }
    for segment in &segments {
        if segment.start < from_block || segment.end > window_end {
            return Err(format!(
                "ledger returned blocks [{}, {}) outside the requested window [{from_block}, {window_end})",
                segment.start, segment.end
            ));
        }
    }
    segments.sort_by_key(|s| s.start);
    for pair in segments.windows(2) {
        if pair[1].start < pair[0].end {
            return Err(format!(
                "ledger returned overlapping ranges [{}, {}) and [{}, {})",
                pair[0].start, pair[0].end, pair[1].start, pair[1].end
            ));
        }
    }

    let mut cursor = from_block;
    let mut archive_ranges = 0usize;
    let mut usable = Vec::new();
    for segment in segments {
        if segment.start != cursor {
            break;
        }
        if matches!(segment.source, SegmentSource::Archive { .. }) {
            if archive_ranges == MAX_ARCHIVE_RANGES_PER_CALL {
                break;
            }
            archive_ranges += 1;
        }
        cursor = segment.end;
        usable.push(segment);
    }
    Ok(ScanPlan { tip, window_end, segments: usable })
}

/// An archive callback is only a locator. Follow it only if the ledger itself lists that
/// canister in `icrc3_get_archives`.
pub fn check_archive_listed(canister: Principal, listed: &[Principal]) -> Result<(), String> {
    if listed.contains(&canister) {
        Ok(())
    } else {
        Err(format!("archive callback canister {canister} is not listed by icrc3_get_archives"))
    }
}

/// Validate an archive's reply to a request for `[start, start + length)`. The reply must
/// contain only requested ids, in order from `start` with no gaps, and no nested archive
/// references. It may be a strict prefix of the range (archives cap response size); the
/// caller then resumes after the prefix. An empty reply for a non-empty range is an error
/// (it would make no progress).
pub fn validate_archive_response(start: u64, length: u64, response: &GetBlocksResult) -> Result<&[BlockWithId], String> {
    if !response.archived_blocks.is_empty() {
        return Err("archive response contains nested archived_blocks".into());
    }
    if response.blocks.is_empty() {
        return Err("archive returned no blocks for a non-empty range".into());
    }
    if response.blocks.len() as u64 > length {
        return Err("archive returned more blocks than requested".into());
    }
    for (offset, block) in response.blocks.iter().enumerate() {
        let expected = start.checked_add(offset as u64);
        let id = nat_to_u64(&block.id);
        if id.is_none() || id != expected {
            return Err(format!(
                "archive returned block id {} where {:?} was expected (ids must be the requested ids in order)",
                block.id, expected
            ));
        }
    }
    Ok(&response.blocks)
}

/// Turn the scan position into a result. `scanned_to` is the exclusive end of the
/// contiguous range that was fully examined without a match.
pub fn conclude_scan(created_at_time: u64, from_block: u64, scanned_to: u64, tip: u64, now_ns: u64) -> SearchOutcome {
    if scanned_to >= tip {
        if dedup_window_closed(created_at_time, now_ns) {
            SearchOutcome::ProvenAbsent
        } else {
            SearchOutcome::NotFoundYet { next_block: tip, tip }
        }
    } else if scanned_to <= from_block {
        SearchOutcome::Error(format!("ledger search made no progress from block {from_block} (tip {tip})"))
    } else {
        SearchOutcome::NotFoundYet { next_block: scanned_to, tip }
    }
}

// ---------------------------------------------------------------------------
// ICRC-3 search
// ---------------------------------------------------------------------------

/// The two ledger interactions the search needs, so the algorithm can run against a mock
/// on the host.
trait Transport {
    async fn get_blocks(
        &self,
        canister: Principal,
        method: &str,
        request: Vec<GetBlocksRequest>,
    ) -> Result<GetBlocksResult, String>;

    async fn list_archives(&self, ledger: Principal) -> Result<Vec<Principal>, String>;
}

struct ChainTransport;

impl Transport for ChainTransport {
    async fn get_blocks(
        &self,
        canister: Principal,
        method: &str,
        request: Vec<GetBlocksRequest>,
    ) -> Result<GetBlocksResult, String> {
        let (response,): (GetBlocksResult,) = call_candid(canister, method, (request,)).await?;
        Ok(response)
    }

    async fn list_archives(&self, ledger: Principal) -> Result<Vec<Principal>, String> {
        let (archives,): (GetArchivesResult,) =
            call_candid(ledger, "icrc3_get_archives", (GetArchivesArgs { from: None },)).await?;
        Ok(archives.into_iter().map(|a| a.canister_id).collect())
    }
}

/// Search ledger history for the block that `op` created.
///
/// Scans at most `max_blocks` blocks from `from_block` (callers resume with the returned
/// `next_block`; the first call should start at the op's `search_from_block`). One call
/// makes one `icrc3_get_blocks` request to the ledger, at most one `icrc3_get_archives`
/// request, and at most [`MAX_ARCHIVE_RANGES_PER_CALL`] archive requests.
///
/// * `Found`: an exact tuple match (see [`block_matches_op`]).
/// * `ProvenAbsent`: the scan reached the tip and the dedup window has closed
///   ([`dedup_window_closed`]), so the ledger can no longer accept the tuple.
/// * `NotFoundYet`: no match in the part scanned; resume at `next_block`.
/// * `Error`: any call, decode or validation failure. Never treated as absence.
///
/// Archive callbacks are trusted only if the ledger lists the callback canister in
/// `icrc3_get_archives`, and each archive reply is validated against the requested ids.
pub async fn search(ledger: Principal, op: &LedgerOp, from_block: u64, max_blocks: u64, now_ns: u64) -> SearchOutcome {
    search_with(&ChainTransport, ledger, op, from_block, max_blocks, now_ns).await
}

async fn search_with<T: Transport>(
    transport: &T,
    ledger: Principal,
    op: &LedgerOp,
    from_block: u64,
    max_blocks: u64,
    now_ns: u64,
) -> SearchOutcome {
    if max_blocks == 0 {
        return SearchOutcome::Error("max_blocks must be positive".into());
    }
    let request = vec![GetBlocksRequest { start: Nat::from(from_block), length: Nat::from(max_blocks) }];
    let response = match transport.get_blocks(ledger, "icrc3_get_blocks", request).await {
        Ok(response) => response,
        Err(e) => return SearchOutcome::Error(e),
    };
    let plan = match plan_scan(from_block, max_blocks, &response) {
        Ok(plan) => plan,
        Err(e) => return SearchOutcome::Error(e),
    };

    let mut listed_archives: Option<Vec<Principal>> = None;
    let mut decode_error: Option<String> = None;
    let mut scanned_to = from_block;
    for segment in &plan.segments {
        match &segment.source {
            SegmentSource::Inline => {
                let blocks = response.blocks.iter().map(|b| &b.block);
                if let Some(block_index) = scan_blocks(op, segment.start, blocks, &mut decode_error) {
                    return SearchOutcome::Found { block_index };
                }
                scanned_to = segment.end;
            }
            SegmentSource::Archive { canister, method } => {
                if listed_archives.is_none() {
                    match transport.list_archives(ledger).await {
                        Ok(listed) => listed_archives = Some(listed),
                        Err(e) => return SearchOutcome::Error(e),
                    }
                }
                let listed: &[Principal] = listed_archives.as_deref().unwrap_or(&[]);
                if let Err(e) = check_archive_listed(*canister, listed) {
                    return SearchOutcome::Error(e);
                }
                let length = segment.end - segment.start;
                let request = vec![GetBlocksRequest { start: Nat::from(segment.start), length: Nat::from(length) }];
                let archive_response = match transport.get_blocks(*canister, method, request).await {
                    Ok(response) => response,
                    Err(e) => return SearchOutcome::Error(e),
                };
                let blocks = match validate_archive_response(segment.start, length, &archive_response) {
                    Ok(blocks) => blocks,
                    Err(e) => return SearchOutcome::Error(e),
                };
                if let Some(block_index) =
                    scan_blocks(op, segment.start, blocks.iter().map(|b| &b.block), &mut decode_error)
                {
                    return SearchOutcome::Found { block_index };
                }
                let received = blocks.len() as u64;
                scanned_to = segment.start + received;
                if received < length {
                    break;
                }
            }
        }
    }
    if let Some(e) = decode_error {
        return SearchOutcome::Error(e);
    }
    conclude_scan(op.created_at_time, from_block, scanned_to, plan.tip, now_ns)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::LedgerOpStatus;
    use icrc_ledger_types::icrc::generic_value::ICRC3Map;
    use icrc_ledger_types::icrc3::archive::QueryArchiveFn;
    use icrc_ledger_types::icrc3::blocks::ArchivedBlocks;
    use serde_bytes::ByteBuf;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    // -----------------------------------------------------------------------
    // Fixtures
    // -----------------------------------------------------------------------

    type Perturb = Box<dyn Fn(&mut BlockSpec)>;
    type Tamper = Box<dyn Fn(&mut GetBlocksResult)>;
    type CallLog = Vec<(Principal, String, Vec<(u64, u64)>)>;

    const T0: u64 = 1_700_000_000_000_000_000;
    const HOUR: u64 = 3_600 * 1_000_000_000;
    /// First instant at which absence may be declared for an op created at `T0`.
    const CLOSED_AT: u64 = T0 + TX_WINDOW_NS + PERMITTED_DRIFT_NS + SAFETY_MARGIN_NS + 1;

    fn principal(b: u8) -> Principal {
        Principal::from_slice(&[b; 10])
    }

    fn account(owner: u8, sub: Option<u8>) -> Account {
        Account { owner: principal(owner), subaccount: sub.map(|s| [s; 32]) }
    }

    fn blank_op(kind: LedgerOpKind) -> LedgerOp {
        LedgerOp {
            kind,
            from: account(1, None),
            to: account(2, Some(0xee)),
            spender: None,
            amount_e8s: 5_000,
            fee_e8s: None,
            memo: vec![7u8; 32],
            created_at_time: T0,
            attempts: 1,
            ambiguous_seen: false,
            status: LedgerOpStatus::Pending,
            search_from_block: 0,
            search_next_block: None,
            last_attempt_ns: None,
            outflow_reserved: false,
        }
    }

    fn pull_op() -> LedgerOp {
        LedgerOp { spender: Some(account(3, None)), ..blank_op(LedgerOpKind::Pull) }
    }

    fn release_op() -> LedgerOp {
        LedgerOp {
            from: account(2, Some(0xee)),
            to: account(9, None),
            amount_e8s: 4_000,
            fee_e8s: Some(10),
            memo: vec![8u8; 32],
            ..blank_op(LedgerOpKind::Release)
        }
    }

    fn account_value(a: &Account) -> ICRC3Value {
        let mut parts = vec![ICRC3Value::Blob(ByteBuf::from(a.owner.as_slice().to_vec()))];
        if let Some(sub) = a.subaccount {
            parts.push(ICRC3Value::Blob(ByteBuf::from(sub.to_vec())));
        }
        ICRC3Value::Array(parts)
    }

    fn nat_value(n: u128) -> ICRC3Value {
        ICRC3Value::Nat(Nat::from(n))
    }

    /// A configurable ICRC-3 transfer block. `for_op` starts as the exact block the op
    /// would create; tests then perturb one field.
    #[derive(Clone)]
    struct BlockSpec {
        btype: Option<String>,
        tx_op: Option<String>,
        from: Option<Account>,
        to: Option<Account>,
        spender: Option<Account>,
        amt: Option<u128>,
        tx_fee: Option<u64>,
        block_fee: Option<u64>,
        ts: Option<u64>,
        memo: Option<Vec<u8>>,
    }

    impl BlockSpec {
        fn for_op(op: &LedgerOp) -> Self {
            let pull = op.kind == LedgerOpKind::Pull;
            BlockSpec {
                btype: Some(if pull { "2xfer" } else { "1xfer" }.into()),
                tx_op: None,
                from: Some(op.from),
                to: Some(op.to),
                spender: if pull { op.spender } else { None },
                amt: Some(u128::from(op.amount_e8s)),
                tx_fee: op.fee_e8s,
                block_fee: if op.fee_e8s.is_none() { Some(10) } else { None },
                ts: Some(op.created_at_time),
                memo: Some(op.memo.clone()),
            }
        }

        fn build(&self) -> ICRC3Value {
            let mut tx: ICRC3Map = BTreeMap::new();
            if let Some(op) = &self.tx_op {
                tx.insert("op".into(), ICRC3Value::Text(op.clone()));
            }
            if let Some(a) = &self.from {
                tx.insert("from".into(), account_value(a));
            }
            if let Some(a) = &self.to {
                tx.insert("to".into(), account_value(a));
            }
            if let Some(a) = &self.spender {
                tx.insert("spender".into(), account_value(a));
            }
            if let Some(a) = self.amt {
                tx.insert("amt".into(), nat_value(a));
            }
            if let Some(f) = self.tx_fee {
                tx.insert("fee".into(), nat_value(u128::from(f)));
            }
            if let Some(t) = self.ts {
                tx.insert("ts".into(), nat_value(u128::from(t)));
            }
            if let Some(m) = &self.memo {
                tx.insert("memo".into(), ICRC3Value::Blob(ByteBuf::from(m.clone())));
            }
            let mut block: ICRC3Map = BTreeMap::new();
            if let Some(b) = &self.btype {
                block.insert("btype".into(), ICRC3Value::Text(b.clone()));
            }
            if let Some(f) = self.block_fee {
                block.insert("fee".into(), nat_value(u128::from(f)));
            }
            // The ledger's own block time. It must never be mistaken for created_at_time.
            block.insert("ts".into(), nat_value(u128::from(T0 + 12_345)));
            block.insert("phash".into(), ICRC3Value::Blob(ByteBuf::from(vec![0u8; 32])));
            block.insert("tx".into(), ICRC3Value::Map(tx));
            ICRC3Value::Map(block)
        }
    }

    fn exact_block(op: &LedgerOp) -> ICRC3Value {
        BlockSpec::for_op(op).build()
    }

    fn decoded_for(op: &LedgerOp) -> DecodedBlock {
        decode_transfer_block(&exact_block(op)).unwrap().unwrap()
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        use std::sync::Arc;
        use std::task::{Context, Poll, Wake, Waker};
        struct Noop;
        impl Wake for Noop {
            fn wake(self: Arc<Self>) {}
        }
        let waker = Waker::from(Arc::new(Noop));
        let mut cx = Context::from_waker(&waker);
        let mut future = Box::pin(future);
        loop {
            if let Poll::Ready(v) = future.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

    // -----------------------------------------------------------------------
    // classify
    // -----------------------------------------------------------------------

    fn kind(r: &Resolution) -> &'static str {
        match r {
            Resolution::Succeeded { .. } => "Succeeded",
            Resolution::RetrySameTuple(_) => "RetrySameTuple",
            Resolution::RebuildTuple(_) => "RebuildTuple",
            Resolution::Failed(_) => "Failed",
            Resolution::Unknown(_) => "Unknown",
        }
    }

    fn all_rejections() -> Vec<LedgerRejection> {
        vec![
            LedgerRejection::BadFee { expected_fee: 10 },
            LedgerRejection::InsufficientFunds { balance: 1 },
            LedgerRejection::InsufficientAllowance { allowance: 2 },
            LedgerRejection::TooOld,
            LedgerRejection::CreatedInFuture,
            LedgerRejection::TemporarilyUnavailable,
            LedgerRejection::GenericError { code: 5, message: "boom".into() },
            LedgerRejection::Other("odd".into()),
        ]
    }

    #[test]
    fn classify_accepted_is_succeeded_regardless_of_history() {
        for ambiguous_seen in [false, true] {
            let op = LedgerOp { ambiguous_seen, ..release_op() };
            assert_eq!(
                classify(&op, &CallOutcome::Accepted { block_index: 41 }),
                Resolution::Succeeded { block_index: 41 }
            );
        }
    }

    #[test]
    fn classify_ambiguous_is_unknown_regardless_of_history() {
        for ambiguous_seen in [false, true] {
            let op = LedgerOp { ambiguous_seen, ..release_op() };
            let r = classify(&op, &CallOutcome::Ambiguous("reject".into()));
            assert_eq!(r, Resolution::Unknown("reject".into()));
        }
    }

    #[test]
    fn classify_first_attempt_table() {
        let op = LedgerOp { ambiguous_seen: false, ..release_op() };
        let expected = [
            ("InsufficientFunds", "Failed"),
            ("InsufficientAllowance", "Failed"),
            ("TemporarilyUnavailable", "RetrySameTuple"),
            ("BadFee", "RebuildTuple"),
            ("TooOld", "RebuildTuple"),
            ("CreatedInFuture", "RebuildTuple"),
            ("GenericError", "Unknown"),
            ("Other", "Unknown"),
        ];
        let mut seen = 0;
        for rejection in all_rejections() {
            let name = match &rejection {
                LedgerRejection::BadFee { .. } => "BadFee",
                LedgerRejection::InsufficientFunds { .. } => "InsufficientFunds",
                LedgerRejection::InsufficientAllowance { .. } => "InsufficientAllowance",
                LedgerRejection::TooOld => "TooOld",
                LedgerRejection::CreatedInFuture => "CreatedInFuture",
                LedgerRejection::TemporarilyUnavailable => "TemporarilyUnavailable",
                LedgerRejection::GenericError { .. } => "GenericError",
                LedgerRejection::Other(_) => "Other",
            };
            let want = expected.iter().find(|(n, _)| *n == name).unwrap().1;
            let got = classify(&op, &CallOutcome::Rejected(rejection));
            assert_eq!(kind(&got), want, "first-attempt row {name}");
            seen += 1;
        }
        assert_eq!(seen, expected.len(), "every rejection variant has a row");
    }

    #[test]
    fn classify_rebuild_carries_the_rejection() {
        let op = release_op();
        for rejection in [LedgerRejection::BadFee { expected_fee: 77 }, LedgerRejection::TooOld, LedgerRejection::CreatedInFuture] {
            assert_eq!(
                classify(&op, &CallOutcome::Rejected(rejection.clone())),
                Resolution::RebuildTuple(rejection)
            );
        }
    }

    #[test]
    fn classify_typed_reply_after_ambiguous_attempt_is_always_unknown() {
        let op = LedgerOp { ambiguous_seen: true, ..release_op() };
        for rejection in all_rejections() {
            let got = classify(&op, &CallOutcome::Rejected(rejection.clone()));
            assert_eq!(kind(&got), "Unknown", "after an ambiguous attempt {rejection:?} is not proof");
        }
    }

    #[test]
    fn classify_pull_uses_the_same_table() {
        let fresh = pull_op();
        let after = LedgerOp { ambiguous_seen: true, ..pull_op() };
        let funds = CallOutcome::Rejected(LedgerRejection::InsufficientFunds { balance: 0 });
        assert_eq!(kind(&classify(&fresh, &funds)), "Failed");
        assert_eq!(kind(&classify(&after, &funds)), "Unknown");
    }

    // -----------------------------------------------------------------------
    // Reply mapping
    // -----------------------------------------------------------------------

    fn huge() -> Nat {
        Nat::from(u128::MAX) + Nat::from(1u8)
    }

    #[test]
    fn transfer_ok_and_duplicate_are_accepted() {
        assert_eq!(outcome_from_transfer(Ok(Ok(Nat::from(5u8)))), CallOutcome::Accepted { block_index: 5 });
        assert_eq!(
            outcome_from_transfer(Ok(Err(TransferError::Duplicate { duplicate_of: Nat::from(7u8) }))),
            CallOutcome::Accepted { block_index: 7 }
        );
        assert_eq!(
            outcome_from_transfer_from(Ok(Err(TransferFromError::Duplicate { duplicate_of: Nat::from(9u8) }))),
            CallOutcome::Accepted { block_index: 9 }
        );
        assert_eq!(outcome_from_transfer_from(Ok(Ok(Nat::from(0u8)))), CallOutcome::Accepted { block_index: 0 });
    }

    #[test]
    fn unrepresentable_block_index_is_ambiguous_never_zero() {
        let big = Nat::from(u64::MAX) + Nat::from(1u8);
        assert!(matches!(outcome_from_transfer(Ok(Ok(big.clone()))), CallOutcome::Ambiguous(_)));
        assert!(matches!(
            outcome_from_transfer(Ok(Err(TransferError::Duplicate { duplicate_of: big.clone() }))),
            CallOutcome::Ambiguous(_)
        ));
        assert!(matches!(
            outcome_from_transfer_from(Ok(Err(TransferFromError::Duplicate { duplicate_of: big }))),
            CallOutcome::Ambiguous(_)
        ));
    }

    #[test]
    fn call_failure_is_ambiguous() {
        assert_eq!(outcome_from_transfer(Err("reject".into())), CallOutcome::Ambiguous("reject".into()));
        assert_eq!(outcome_from_transfer_from(Err("trap".into())), CallOutcome::Ambiguous("trap".into()));
    }

    #[test]
    fn transfer_errors_map_to_typed_rejections() {
        let rejected = |e: TransferError| match outcome_from_transfer(Ok(Err(e))) {
            CallOutcome::Rejected(r) => r,
            other => panic!("expected Rejected, got {other:?}"),
        };
        assert_eq!(rejected(TransferError::BadFee { expected_fee: Nat::from(10u8) }), LedgerRejection::BadFee { expected_fee: 10 });
        assert!(matches!(rejected(TransferError::BadFee { expected_fee: huge() }), LedgerRejection::Other(_)));
        assert!(matches!(rejected(TransferError::BadBurn { min_burn_amount: Nat::from(1u8) }), LedgerRejection::Other(_)));
        assert_eq!(
            rejected(TransferError::InsufficientFunds { balance: Nat::from(3u8) }),
            LedgerRejection::InsufficientFunds { balance: 3 }
        );
        assert!(matches!(rejected(TransferError::InsufficientFunds { balance: huge() }), LedgerRejection::Other(_)));
        assert_eq!(rejected(TransferError::TooOld), LedgerRejection::TooOld);
        assert_eq!(rejected(TransferError::CreatedInFuture { ledger_time: 1 }), LedgerRejection::CreatedInFuture);
        assert_eq!(rejected(TransferError::TemporarilyUnavailable), LedgerRejection::TemporarilyUnavailable);
        assert_eq!(
            rejected(TransferError::GenericError { error_code: Nat::from(3u8), message: "m".into() }),
            LedgerRejection::GenericError { code: 3, message: "m".into() }
        );
        assert_eq!(
            rejected(TransferError::GenericError { error_code: huge(), message: "m".into() }),
            LedgerRejection::GenericError { code: u64::MAX, message: "m".into() }
        );
    }

    #[test]
    fn transfer_from_errors_map_to_typed_rejections() {
        let rejected = |e: TransferFromError| match outcome_from_transfer_from(Ok(Err(e))) {
            CallOutcome::Rejected(r) => r,
            other => panic!("expected Rejected, got {other:?}"),
        };
        assert_eq!(
            rejected(TransferFromError::InsufficientAllowance { allowance: Nat::from(4u8) }),
            LedgerRejection::InsufficientAllowance { allowance: 4 }
        );
        assert!(matches!(
            rejected(TransferFromError::InsufficientAllowance { allowance: huge() }),
            LedgerRejection::Other(_)
        ));
        assert_eq!(
            rejected(TransferFromError::InsufficientFunds { balance: Nat::from(3u8) }),
            LedgerRejection::InsufficientFunds { balance: 3 }
        );
        assert_eq!(rejected(TransferFromError::BadFee { expected_fee: Nat::from(10u8) }), LedgerRejection::BadFee { expected_fee: 10 });
        assert!(matches!(rejected(TransferFromError::BadBurn { min_burn_amount: Nat::from(1u8) }), LedgerRejection::Other(_)));
        assert_eq!(rejected(TransferFromError::TooOld), LedgerRejection::TooOld);
        assert_eq!(rejected(TransferFromError::CreatedInFuture { ledger_time: 1 }), LedgerRejection::CreatedInFuture);
        assert_eq!(rejected(TransferFromError::TemporarilyUnavailable), LedgerRejection::TemporarilyUnavailable);
        assert_eq!(
            rejected(TransferFromError::GenericError { error_code: Nat::from(8u8), message: "x".into() }),
            LedgerRejection::GenericError { code: 8, message: "x".into() }
        );
    }

    #[test]
    fn pull_args_carry_the_persisted_tuple() {
        let op = pull_op();
        let spender = Account { owner: principal(3), subaccount: Some([5u8; 32]) };
        let args = transfer_from_args(&op, &spender);
        assert_eq!(args.spender_subaccount, Some([5u8; 32]));
        assert_eq!(args.from, op.from);
        assert_eq!(args.to, op.to);
        assert_eq!(args.amount, Nat::from(5_000u64));
        assert_eq!(args.fee, None);
        assert_eq!(args.memo, Some(Memo::from(op.memo.clone())));
        assert_eq!(args.created_at_time, Some(T0));
        let pinned = LedgerOp { fee_e8s: Some(10), ..pull_op() };
        assert_eq!(transfer_from_args(&pinned, &spender).fee, Some(Nat::from(10u8)));
    }

    #[test]
    fn escrow_transfer_args_carry_the_persisted_tuple() {
        for kind in [LedgerOpKind::Release, LedgerOpKind::Refund, LedgerOpKind::Sweep] {
            let op = LedgerOp { kind, ..release_op() };
            let args = transfer_args(&op);
            assert_eq!(args.from_subaccount, Some([0xee; 32]));
            assert_eq!(args.to, op.to);
            assert_eq!(args.fee, Some(Nat::from(10u8)));
            assert_eq!(args.created_at_time, Some(T0));
            assert_eq!(args.memo, Some(Memo::from(vec![8u8; 32])));
            assert_eq!(args.amount, Nat::from(4_000u64));
        }
    }

    #[test]
    fn off_chain_execute_is_ambiguous_and_queries_error() {
        let ledger = principal(0x10);
        assert!(matches!(block_on(execute(ledger, &release_op())), CallOutcome::Ambiguous(_)));
        assert!(matches!(block_on(execute(ledger, &pull_op())), CallOutcome::Ambiguous(_)));
        assert!(block_on(fee(ledger)).is_err());
        assert!(block_on(balance_of(ledger, account(1, None))).is_err());
        assert!(block_on(log_length(ledger)).is_err());
        assert!(matches!(block_on(search(ledger, &pull_op(), 0, 10, CLOSED_AT)), SearchOutcome::Error(_)));
    }

    #[test]
    fn pull_without_spender_makes_no_call_and_is_not_proof_of_anything() {
        let op = LedgerOp { spender: None, ..blank_op(LedgerOpKind::Pull) };
        let outcome = block_on(execute(principal(0x10), &op));
        assert!(matches!(outcome, CallOutcome::Rejected(LedgerRejection::Other(_))));
        assert_eq!(kind(&classify(&op, &outcome)), "Unknown");
    }

    // -----------------------------------------------------------------------
    // Dedup window
    // -----------------------------------------------------------------------

    #[test]
    fn dedup_window_closes_only_after_window_drift_and_margin() {
        assert_eq!(TX_WINDOW_NS, 24 * HOUR);
        assert_eq!(PERMITTED_DRIFT_NS, 120 * 1_000_000_000);
        assert_eq!(SAFETY_MARGIN_NS, 600 * 1_000_000_000);
        let boundary = T0 + TX_WINDOW_NS + PERMITTED_DRIFT_NS + SAFETY_MARGIN_NS;
        assert!(!dedup_window_closed(T0, T0));
        assert!(!dedup_window_closed(T0, T0 + TX_WINDOW_NS));
        assert!(!dedup_window_closed(T0, T0 + TX_WINDOW_NS + PERMITTED_DRIFT_NS + 1));
        assert!(!dedup_window_closed(T0, boundary), "exactly at the boundary is still open");
        assert!(dedup_window_closed(T0, boundary + 1));
        assert!(!dedup_window_closed(u64::MAX, u64::MAX), "saturation never reports closed");
    }

    // -----------------------------------------------------------------------
    // Decoding
    // -----------------------------------------------------------------------

    #[test]
    fn decodes_standard_1xfer_and_2xfer_blocks() {
        let release = decoded_for(&release_op());
        assert_eq!(release.btype.as_deref(), Some("1xfer"));
        assert_eq!(release.op, "xfer");
        assert_eq!(release.from, Some(account(2, Some(0xee))));
        assert_eq!(release.to, Some(account(9, None)));
        assert_eq!(release.spender, None);
        assert_eq!(release.amount, 4_000);
        assert_eq!(release.fee, Some(10));
        assert_eq!(release.created_at_time, Some(T0), "tx.ts, not the block time");
        assert_eq!(release.memo, Some(vec![8u8; 32]));

        let pull = decoded_for(&pull_op());
        assert_eq!(pull.btype.as_deref(), Some("2xfer"));
        assert_eq!(pull.spender, Some(account(3, None)));
        assert_eq!(pull.fee, Some(10), "block-level fee when tx.fee is absent");
    }

    #[test]
    fn tx_fee_wins_over_block_level_fee() {
        let mut spec = BlockSpec::for_op(&release_op());
        spec.block_fee = Some(99);
        let decoded = decode_transfer_block(&spec.build()).unwrap().unwrap();
        assert_eq!(decoded.fee, Some(10));
    }

    #[test]
    fn decodes_legacy_blocks_with_only_tx_op() {
        let mut spec = BlockSpec::for_op(&release_op());
        spec.btype = None;
        spec.tx_op = Some("xfer".into());
        let decoded = decode_transfer_block(&spec.build()).unwrap().unwrap();
        assert_eq!(decoded.btype, None);
        assert_eq!(decoded.op, "xfer");
        spec.tx_op = Some("transfer".into());
        assert_eq!(decode_transfer_block(&spec.build()).unwrap().unwrap().op, "xfer");
    }

    #[test]
    fn btype_and_tx_op_may_both_be_present_if_they_agree() {
        let mut spec = BlockSpec::for_op(&pull_op());
        spec.tx_op = Some("xfer".into());
        assert!(decode_transfer_block(&spec.build()).unwrap().is_some());
    }

    #[test]
    fn btype_and_tx_op_disagreement_is_an_error() {
        let mut spec = BlockSpec::for_op(&release_op());
        spec.tx_op = Some("burn".into());
        assert!(decode_transfer_block(&spec.build()).unwrap_err().contains("disagree"));
        let mut spec = BlockSpec::for_op(&release_op());
        spec.btype = Some("1burn".into());
        spec.tx_op = Some("xfer".into());
        assert!(decode_transfer_block(&spec.build()).is_err());
    }

    #[test]
    fn non_transfer_blocks_are_skipped_without_decoding_their_fields() {
        for btype in ["1mint", "1burn", "2approve", "107feecol"] {
            // No amt, no accounts: only the type is inspected.
            let mut block: ICRC3Map = BTreeMap::new();
            block.insert("btype".into(), ICRC3Value::Text(btype.into()));
            block.insert("tx".into(), ICRC3Value::Map(BTreeMap::new()));
            assert_eq!(decode_transfer_block(&ICRC3Value::Map(block)), Ok(None), "{btype}");
        }
        // A fee-collector style block with no tx at all.
        let mut block: ICRC3Map = BTreeMap::new();
        block.insert("btype".into(), ICRC3Value::Text("107feecol".into()));
        assert_eq!(decode_transfer_block(&ICRC3Value::Map(block)), Ok(None));
    }

    #[test]
    fn unclassifiable_and_malformed_blocks_are_errors_not_skips() {
        assert!(decode_transfer_block(&ICRC3Value::Nat(Nat::from(1u8))).is_err(), "not a map");
        let mut spec = BlockSpec::for_op(&release_op());
        spec.btype = None;
        assert!(decode_transfer_block(&spec.build()).unwrap_err().contains("neither"), "no btype and no tx.op");

        let mut spec = BlockSpec::for_op(&release_op());
        spec.amt = None;
        assert!(decode_transfer_block(&spec.build()).unwrap_err().contains("amt"), "transfer without amt");

        let bad_account = |v: ICRC3Value| {
            let mut block = match BlockSpec::for_op(&release_op()).build() {
                ICRC3Value::Map(m) => m,
                _ => unreachable!(),
            };
            let ICRC3Value::Map(tx) = block.get_mut("tx").unwrap() else { unreachable!() };
            tx.insert("from".into(), v);
            ICRC3Value::Map(block)
        };
        assert!(decode_transfer_block(&bad_account(ICRC3Value::Text("x".into()))).is_err());
        assert!(decode_transfer_block(&bad_account(ICRC3Value::Array(vec![]))).is_err());
        assert!(decode_transfer_block(&bad_account(ICRC3Value::Array(vec![ICRC3Value::Text("x".into())]))).is_err());
        assert!(decode_transfer_block(&bad_account(ICRC3Value::Array(vec![
            ICRC3Value::Blob(ByteBuf::from(vec![1u8])),
            ICRC3Value::Blob(ByteBuf::from(vec![0u8; 5])),
        ])))
        .is_err());
        assert!(decode_transfer_block(&bad_account(ICRC3Value::Array(vec![
            ICRC3Value::Blob(ByteBuf::from(vec![1u8; 30])),
        ])))
        .is_err());

        let mut block = match BlockSpec::for_op(&release_op()).build() {
            ICRC3Value::Map(m) => m,
            _ => unreachable!(),
        };
        let ICRC3Value::Map(tx) = block.get_mut("tx").unwrap() else { unreachable!() };
        tx.insert("memo".into(), ICRC3Value::Text("not a blob".into()));
        assert!(decode_transfer_block(&ICRC3Value::Map(block)).unwrap_err().contains("memo"));
    }

    #[test]
    fn account_without_subaccount_and_with_zero_subaccount_decode_distinctly_but_compare_equal() {
        let a = account_from_value(&ICRC3Value::Array(vec![ICRC3Value::Blob(ByteBuf::from(principal(1).as_slice().to_vec()))])).unwrap();
        let b = account_from_value(&ICRC3Value::Array(vec![
            ICRC3Value::Blob(ByteBuf::from(principal(1).as_slice().to_vec())),
            ICRC3Value::Blob(ByteBuf::from(vec![0u8; 32])),
        ]))
        .unwrap();
        assert_eq!(a.subaccount, None);
        assert_eq!(b.subaccount, Some([0u8; 32]));
        assert!(same_account(&a, &b));
        assert!(same_account(&b, &a));
        assert!(!same_account(&a, &account(1, Some(1))));
        assert!(!same_account(&a, &account(2, None)));
    }

    // -----------------------------------------------------------------------
    // Matching
    // -----------------------------------------------------------------------

    fn matches(op: &LedgerOp, spec: &BlockSpec) -> bool {
        match decode_transfer_block(&spec.build()) {
            Ok(Some(decoded)) => block_matches_op(op, &decoded),
            Ok(None) => false,
            Err(e) => panic!("fixture must decode: {e}"),
        }
    }

    #[test]
    fn exact_blocks_match_for_every_kind() {
        assert!(matches(&pull_op(), &BlockSpec::for_op(&pull_op())));
        for kind in [LedgerOpKind::Release, LedgerOpKind::Refund, LedgerOpKind::Sweep] {
            let op = LedgerOp { kind, ..release_op() };
            assert!(matches(&op, &BlockSpec::for_op(&op)), "{kind:?}");
        }
    }

    #[test]
    fn pull_rejects_each_single_field_perturbation() {
        let op = pull_op();
        let base = BlockSpec::for_op(&op);
        let cases: Vec<(&str, Perturb)> = vec![
            ("wrong spender owner", Box::new(|s| s.spender = Some(account(4, None)))),
            ("wrong spender subaccount", Box::new(|s| s.spender = Some(account(3, Some(1))))),
            ("missing spender", Box::new(|s| s.spender = None)),
            ("wrong from", Box::new(|s| s.from = Some(account(5, None)))),
            ("missing from", Box::new(|s| s.from = None)),
            ("wrong to", Box::new(|s| s.to = Some(account(2, Some(0xef))))),
            ("missing to", Box::new(|s| s.to = None)),
            ("amount plus one", Box::new(|s| s.amt = Some(5_001))),
            ("amount minus one", Box::new(|s| s.amt = Some(4_999))),
            ("wrong memo", Box::new(|s| s.memo = Some(vec![7u8; 31]))),
            ("memo differs in last byte", Box::new(|s| s.memo = Some([[7u8; 31].to_vec(), vec![6u8]].concat()))),
            ("missing memo", Box::new(|s| s.memo = None)),
            ("created_at_time plus one", Box::new(|s| s.ts = Some(T0 + 1))),
            ("created_at_time minus one", Box::new(|s| s.ts = Some(T0 - 1))),
            ("missing created_at_time", Box::new(|s| s.ts = None)),
            ("1xfer type", Box::new(|s| s.btype = Some("1xfer".into()))),
            ("approve type", Box::new(|s| s.btype = Some("2approve".into()))),
            ("burn type", Box::new(|s| s.btype = Some("1burn".into()))),
        ];
        for (name, perturb) in cases {
            let mut spec = base.clone();
            perturb(&mut spec);
            let outcome = decode_transfer_block(&spec.build());
            let matched = matches!(&outcome, Ok(Some(d)) if block_matches_op(&op, d));
            assert!(!matched, "pull must not match: {name}");
        }
    }

    #[test]
    fn release_rejects_each_single_field_perturbation() {
        let op = release_op();
        let base = BlockSpec::for_op(&op);
        let cases: Vec<(&str, Perturb)> = vec![
            ("spender present (a pull)", Box::new(|s| s.spender = Some(account(3, None)))),
            ("2xfer type", Box::new(|s| s.btype = Some("2xfer".into()))),
            ("wrong from subaccount", Box::new(|s| s.from = Some(account(2, None)))),
            ("wrong to", Box::new(|s| s.to = Some(account(9, Some(1))))),
            ("amount plus one", Box::new(|s| s.amt = Some(4_001))),
            ("amount minus one", Box::new(|s| s.amt = Some(3_999))),
            ("wrong memo", Box::new(|s| s.memo = Some(vec![9u8; 32]))),
            ("created_at_time plus one", Box::new(|s| s.ts = Some(T0 + 1))),
            ("fee in tx differs", Box::new(|s| s.tx_fee = Some(11))),
            ("fee at block level differs", Box::new(|s| {
                s.tx_fee = None;
                s.block_fee = Some(11);
            })),
            ("mint type", Box::new(|s| s.btype = Some("1mint".into()))),
        ];
        for (name, perturb) in cases {
            let mut spec = base.clone();
            perturb(&mut spec);
            let outcome = decode_transfer_block(&spec.build());
            let matched = matches!(&outcome, Ok(Some(d)) if block_matches_op(&op, d));
            assert!(!matched, "release must not match: {name}");
        }
    }

    #[test]
    fn fee_is_checked_only_when_the_block_records_one_and_the_op_pins_one() {
        let op = release_op();
        let mut spec = BlockSpec::for_op(&op);
        spec.tx_fee = None;
        spec.block_fee = None;
        assert!(matches(&op, &spec), "block records no fee: not a mismatch");
        // A pull pins no fee, so whatever the ledger charged is fine.
        let pull = pull_op();
        let mut spec = BlockSpec::for_op(&pull);
        spec.block_fee = Some(12_345);
        assert!(matches(&pull, &spec));
        // If a pull does pin a fee, a different recorded fee is a mismatch.
        let pinned = LedgerOp { fee_e8s: Some(10), ..pull_op() };
        let mut spec = BlockSpec::for_op(&pinned);
        spec.tx_fee = Some(11);
        assert!(!matches(&pinned, &spec));
    }

    #[test]
    fn default_subaccount_equivalence_holds_in_both_directions() {
        // Op uses None, block records the explicit zero subaccount.
        let op = LedgerOp { to: account(9, None), ..release_op() };
        let mut spec = BlockSpec::for_op(&op);
        spec.to = Some(Account { owner: principal(9), subaccount: Some([0u8; 32]) });
        assert!(matches(&op, &spec));
        // Op uses the explicit zero subaccount, block records None.
        let op = LedgerOp { to: Account { owner: principal(9), subaccount: Some([0u8; 32]) }, ..release_op() };
        let mut spec = BlockSpec::for_op(&op);
        spec.to = Some(account(9, None));
        assert!(matches(&op, &spec));
        // Same for the pull spender and source.
        let op = LedgerOp {
            from: Account { owner: principal(1), subaccount: Some([0u8; 32]) },
            spender: Some(Account { owner: principal(3), subaccount: Some([0u8; 32]) }),
            ..pull_op()
        };
        let mut spec = BlockSpec::for_op(&op);
        spec.from = Some(account(1, None));
        spec.spender = Some(account(3, None));
        assert!(matches(&op, &spec));
    }

    #[test]
    fn memo_absent_equals_empty_only_when_the_op_memo_is_empty() {
        let op = LedgerOp { memo: vec![], ..release_op() };
        let mut spec = BlockSpec::for_op(&op);
        spec.memo = None;
        assert!(matches(&op, &spec));
        spec.memo = Some(vec![]);
        assert!(matches(&op, &spec));
        let op = release_op();
        let mut spec = BlockSpec::for_op(&op);
        spec.memo = None;
        assert!(!matches(&op, &spec));
    }

    #[test]
    fn pull_op_without_spender_never_matches() {
        let op = LedgerOp { spender: None, ..blank_op(LedgerOpKind::Pull) };
        let mut spec = BlockSpec::for_op(&pull_op());
        spec.spender = Some(account(3, None));
        assert!(!matches(&op, &spec));
    }

    #[test]
    fn block_time_is_not_confused_with_created_at_time() {
        // The fixture's block-level ts is T0 + 12345; an op stamped with that value must
        // not match a block whose tx.ts is T0.
        let op = LedgerOp { created_at_time: T0 + 12_345, ..release_op() };
        let spec = BlockSpec::for_op(&release_op());
        assert!(!matches(&op, &spec));
    }

    // -----------------------------------------------------------------------
    // plan_scan / archive validation
    // -----------------------------------------------------------------------

    fn block_with_id(id: u64) -> BlockWithId {
        BlockWithId { id: Nat::from(id), block: ICRC3Value::Nat(Nat::from(id)) }
    }

    fn archived(canister: Principal, ranges: &[(u64, u64)]) -> ArchivedBlocks {
        ArchivedBlocks {
            args: ranges
                .iter()
                .map(|(s, l)| GetBlocksRequest { start: Nat::from(*s), length: Nat::from(*l) })
                .collect(),
            callback: QueryArchiveFn::new(canister, "icrc3_get_blocks"),
        }
    }

    fn response(log_length: u64, inline: &[u64], archived_blocks: Vec<ArchivedBlocks>) -> GetBlocksResult {
        GetBlocksResult {
            log_length: Nat::from(log_length),
            blocks: inline.iter().map(|id| block_with_id(*id)).collect(),
            archived_blocks,
        }
    }

    #[test]
    fn plan_clamps_window_to_tip_and_accepts_inline_only() {
        let plan = plan_scan(5, 100, &response(8, &[5, 6, 7], vec![])).unwrap();
        assert_eq!(plan.tip, 8);
        assert_eq!(plan.window_end, 8);
        assert_eq!(plan.segments, vec![Segment { start: 5, end: 8, source: SegmentSource::Inline }]);
    }

    #[test]
    fn plan_orders_archive_before_inline_and_requires_contiguity() {
        let a = principal(0xa1);
        let plan = plan_scan(0, 100, &response(10, &[6, 7, 8, 9], vec![archived(a, &[(0, 6)])])).unwrap();
        assert_eq!(plan.segments.len(), 2);
        assert_eq!(plan.segments[0].start, 0);
        assert_eq!(plan.segments[0].end, 6);
        assert!(matches!(plan.segments[0].source, SegmentSource::Archive { .. }));
        assert_eq!(plan.segments[1], Segment { start: 6, end: 10, source: SegmentSource::Inline });
    }

    #[test]
    fn plan_stops_at_the_first_gap() {
        let a = principal(0xa1);
        // Archive covers [0, 4), inline starts at 6: blocks 4 and 5 are missing.
        let plan = plan_scan(0, 100, &response(10, &[6, 7, 8, 9], vec![archived(a, &[(0, 4)])])).unwrap();
        assert_eq!(plan.segments.len(), 1);
        assert_eq!(plan.segments[0].end, 4);
        // Nothing at the cursor at all.
        let plan = plan_scan(0, 100, &response(10, &[6, 7], vec![])).unwrap();
        assert!(plan.segments.is_empty());
    }

    #[test]
    fn plan_rejects_out_of_order_or_gapped_inline_ids() {
        assert!(plan_scan(0, 10, &response(10, &[1, 0, 2], vec![])).is_err());
        assert!(plan_scan(0, 10, &response(10, &[0, 2], vec![])).is_err());
        assert!(plan_scan(0, 10, &response(10, &[0, 0], vec![])).is_err());
    }

    #[test]
    fn plan_rejects_ranges_outside_the_requested_window() {
        let a = principal(0xa1);
        assert!(plan_scan(5, 3, &response(20, &[5, 6, 7, 8], vec![])).is_err(), "inline beyond window_end");
        assert!(plan_scan(5, 10, &response(20, &[4, 5], vec![])).is_err(), "inline before from_block");
        assert!(plan_scan(5, 10, &response(20, &[], vec![archived(a, &[(3, 4)])])).is_err(), "archive before from_block");
        assert!(plan_scan(0, 10, &response(8, &[], vec![archived(a, &[(0, 9)])])).is_err(), "archive beyond tip");
    }

    #[test]
    fn plan_rejects_overlap_empty_ranges_and_empty_method() {
        let a = principal(0xa1);
        let b = principal(0xa2);
        assert!(plan_scan(0, 10, &response(10, &[], vec![archived(a, &[(0, 6), (4, 4)])])).is_err());
        assert!(plan_scan(0, 10, &response(10, &[4, 5], vec![archived(a, &[(0, 6)])])).is_err(), "inline overlaps archive");
        assert!(plan_scan(0, 10, &response(10, &[], vec![archived(a, &[(0, 5)]), archived(b, &[(0, 5)])])).is_err());
        assert!(plan_scan(0, 10, &response(10, &[], vec![archived(a, &[(0, 0)])])).is_err(), "zero-length range");
        let mut r = response(10, &[], vec![archived(a, &[(0, 5)])]);
        r.archived_blocks[0].callback = QueryArchiveFn::new(a, "  ");
        assert!(plan_scan(0, 10, &r).is_err(), "blank callback method");
    }

    #[test]
    fn plan_rejects_a_cursor_beyond_the_tip_but_accepts_the_tip_itself() {
        assert!(plan_scan(11, 10, &response(10, &[], vec![])).is_err());
        let plan = plan_scan(10, 10, &response(10, &[], vec![])).unwrap();
        assert!(plan.segments.is_empty());
        assert_eq!(plan.window_end, 10);
    }

    #[test]
    fn plan_caps_archive_ranges_per_call() {
        let a = principal(0xa1);
        let ranges: Vec<(u64, u64)> = (0..12).map(|i| (i * 10, 10)).collect();
        let plan = plan_scan(0, 1_000, &response(120, &[], vec![archived(a, &ranges)])).unwrap();
        assert_eq!(plan.segments.len(), MAX_ARCHIVE_RANGES_PER_CALL);
        assert_eq!(plan.segments.last().unwrap().end, 80);
    }

    #[test]
    fn archive_must_be_listed_by_the_ledger() {
        let a = principal(0xa1);
        assert!(check_archive_listed(a, &[principal(0xa2), a]).is_ok());
        assert!(check_archive_listed(a, &[principal(0xa2)]).unwrap_err().contains("not listed"));
        assert!(check_archive_listed(a, &[]).is_err());
    }

    #[test]
    fn archive_response_validation() {
        // Exact.
        let ok = response(0, &[10, 11, 12], vec![]);
        assert_eq!(validate_archive_response(10, 3, &ok).unwrap().len(), 3);
        // A strict prefix is allowed.
        assert_eq!(validate_archive_response(10, 5, &ok).unwrap().len(), 3);
        // Empty for a non-empty range makes no progress.
        assert!(validate_archive_response(10, 3, &response(0, &[], vec![])).is_err());
        // More than requested.
        assert!(validate_archive_response(10, 2, &ok).is_err());
        // Wrong first id (shifted either way).
        assert!(validate_archive_response(10, 3, &response(0, &[11, 12, 13], vec![])).is_err());
        assert!(validate_archive_response(10, 3, &response(0, &[9, 10, 11], vec![])).is_err());
        // Out of order, duplicated or gapped ids.
        assert!(validate_archive_response(10, 3, &response(0, &[10, 12, 11], vec![])).is_err());
        assert!(validate_archive_response(10, 3, &response(0, &[10, 10, 11], vec![])).is_err());
        assert!(validate_archive_response(10, 3, &response(0, &[10, 12], vec![])).is_err());
        // Nested archives are never followed.
        let nested = response(0, &[10, 11, 12], vec![archived(principal(0xa3), &[(0, 1)])]);
        assert!(validate_archive_response(10, 3, &nested).unwrap_err().contains("nested"));
    }

    // -----------------------------------------------------------------------
    // conclude_scan
    // -----------------------------------------------------------------------

    #[test]
    fn conclude_scan_decision_table() {
        // Reached the tip, window still open.
        assert_eq!(
            conclude_scan(T0, 0, 10, 10, T0 + HOUR),
            SearchOutcome::NotFoundYet { next_block: 10, tip: 10 }
        );
        // Reached the tip, window closed: absence is proven.
        assert_eq!(conclude_scan(T0, 0, 10, 10, CLOSED_AT), SearchOutcome::ProvenAbsent);
        // Scan stopped before the tip, even with the window closed: not proof.
        assert_eq!(
            conclude_scan(T0, 0, 6, 10, CLOSED_AT),
            SearchOutcome::NotFoundYet { next_block: 6, tip: 10 }
        );
        // Resumed at the tip with nothing new.
        assert_eq!(conclude_scan(T0, 10, 10, 10, CLOSED_AT), SearchOutcome::ProvenAbsent);
        assert_eq!(
            conclude_scan(T0, 10, 10, 10, T0 + 2 * HOUR),
            SearchOutcome::NotFoundYet { next_block: 10, tip: 10 }
        );
        // No progress before the tip is an error, not absence.
        assert!(matches!(conclude_scan(T0, 3, 3, 10, CLOSED_AT), SearchOutcome::Error(_)));
    }

    // -----------------------------------------------------------------------
    // End-to-end search against a mock ledger and archive
    // -----------------------------------------------------------------------

    /// A ledger whose blocks `< local_start` live in one archive, the rest inline.
    /// Response caps imitate the real ledger/archive truncating large replies.
    struct MockChain {
        ledger: Principal,
        archive: Principal,
        listed: Vec<Principal>,
        blocks: BTreeMap<u64, ICRC3Value>,
        log_length: u64,
        local_start: u64,
        ledger_cap: u64,
        archive_cap: u64,
        fail_ledger: bool,
        fail_archive: bool,
        fail_list: bool,
        tamper_ledger: Option<Tamper>,
        tamper_archive: Option<Tamper>,
        calls: RefCell<CallLog>,
        list_calls: RefCell<u32>,
    }

    impl MockChain {
        /// `n` filler blocks that never match `release_op()` or `pull_op()`.
        fn new(n: u64, local_start: u64) -> Self {
            let blocks = (0..n).map(|i| (i, filler_block(i))).collect();
            MockChain {
                ledger: principal(0x10),
                archive: principal(0xa1),
                listed: vec![principal(0xa1)],
                blocks,
                log_length: n,
                local_start,
                ledger_cap: 10_000,
                archive_cap: 10_000,
                fail_ledger: false,
                fail_archive: false,
                fail_list: false,
                tamper_ledger: None,
                tamper_archive: None,
                calls: RefCell::new(vec![]),
                list_calls: RefCell::new(0),
            }
        }

        fn put(&mut self, id: u64, block: ICRC3Value) {
            self.blocks.insert(id, block);
        }

        fn with(&self, id: u64) -> BlockWithId {
            BlockWithId { id: Nat::from(id), block: self.blocks.get(&id).cloned().expect("mock block") }
        }

        fn ledger_response(&self, start: u64, length: u64) -> GetBlocksResult {
            let end = start.saturating_add(length).min(self.log_length).min(start.saturating_add(self.ledger_cap));
            let mut archived_blocks = vec![];
            let archived_end = end.min(self.local_start);
            if start < archived_end {
                archived_blocks.push(ArchivedBlocks {
                    args: vec![GetBlocksRequest { start: Nat::from(start), length: Nat::from(archived_end - start) }],
                    callback: QueryArchiveFn::new(self.archive, "icrc3_get_blocks"),
                });
            }
            let inline_start = start.max(self.local_start);
            let blocks = if inline_start < end { (inline_start..end).map(|i| self.with(i)).collect() } else { vec![] };
            GetBlocksResult { log_length: Nat::from(self.log_length), blocks, archived_blocks }
        }

        fn archive_response(&self, start: u64, length: u64) -> GetBlocksResult {
            let end = start.saturating_add(length).min(self.local_start).min(start.saturating_add(self.archive_cap));
            let blocks = if start < end { (start..end).map(|i| self.with(i)).collect() } else { vec![] };
            GetBlocksResult { log_length: Nat::from(self.local_start), blocks, archived_blocks: vec![] }
        }
    }

    fn filler_block(i: u64) -> ICRC3Value {
        BlockSpec {
            btype: Some("1xfer".into()),
            tx_op: None,
            from: Some(account(0x50, None)),
            to: Some(account(0x51, None)),
            spender: None,
            amt: Some(u128::from(1_000 + i)),
            tx_fee: Some(10),
            block_fee: None,
            ts: Some(T0 - HOUR + i),
            memo: Some(i.to_be_bytes().to_vec()),
        }
        .build()
    }

    impl Transport for MockChain {
        async fn get_blocks(
            &self,
            canister: Principal,
            method: &str,
            request: Vec<GetBlocksRequest>,
        ) -> Result<GetBlocksResult, String> {
            assert_eq!(request.len(), 1, "the search always sends exactly one range");
            let (start, length) = request[0].as_start_and_length().unwrap();
            self.calls.borrow_mut().push((canister, method.to_string(), vec![(start, length)]));
            if canister == self.ledger {
                if self.fail_ledger {
                    return Err("ledger call rejected".into());
                }
                let mut r = self.ledger_response(start, length);
                if let Some(t) = &self.tamper_ledger {
                    t(&mut r);
                }
                Ok(r)
            } else if canister == self.archive || self.listed.contains(&canister) {
                if self.fail_archive {
                    return Err("archive call rejected".into());
                }
                let mut r = self.archive_response(start, length);
                if let Some(t) = &self.tamper_archive {
                    t(&mut r);
                }
                Ok(r)
            } else {
                panic!("search called an unexpected canister {canister}: archive callbacks must be validated first");
            }
        }

        async fn list_archives(&self, _ledger: Principal) -> Result<Vec<Principal>, String> {
            *self.list_calls.borrow_mut() += 1;
            if self.fail_list {
                Err("icrc3_get_archives rejected".into())
            } else {
                Ok(self.listed.clone())
            }
        }
    }

    fn run(chain: &MockChain, op: &LedgerOp, from: u64, max: u64, now: u64) -> SearchOutcome {
        block_on(search_with(chain, chain.ledger, op, from, max, now))
    }

    fn archive_calls(chain: &MockChain) -> usize {
        chain.calls.borrow().iter().filter(|(c, _, _)| *c == chain.archive).count()
    }

    #[test]
    fn search_finds_an_inline_block() {
        let op = release_op();
        let mut chain = MockChain::new(20, 0);
        chain.put(13, exact_block(&op));
        assert_eq!(run(&chain, &op, 0, 100, T0), SearchOutcome::Found { block_index: 13 });
        assert_eq!(archive_calls(&chain), 0);
        assert_eq!(*chain.list_calls.borrow(), 0, "archives are only listed when a callback is followed");
    }

    #[test]
    fn search_finds_a_pull_block_in_an_archive() {
        let op = pull_op();
        let mut chain = MockChain::new(30, 20);
        chain.put(7, exact_block(&op));
        assert_eq!(run(&chain, &op, 0, 100, T0), SearchOutcome::Found { block_index: 7 });
        assert_eq!(archive_calls(&chain), 1);
        assert_eq!(*chain.list_calls.borrow(), 1);
    }

    #[test]
    fn search_finds_a_block_in_the_inline_part_after_scanning_an_archive() {
        let op = release_op();
        let mut chain = MockChain::new(30, 20);
        chain.put(25, exact_block(&op));
        assert_eq!(run(&chain, &op, 0, 100, T0), SearchOutcome::Found { block_index: 25 });
    }

    #[test]
    fn search_returns_the_first_of_two_matching_blocks() {
        let op = release_op();
        let mut chain = MockChain::new(20, 0);
        chain.put(4, exact_block(&op));
        chain.put(9, exact_block(&op));
        assert_eq!(run(&chain, &op, 0, 100, T0), SearchOutcome::Found { block_index: 4 });
    }

    #[test]
    fn search_resumes_from_a_nonzero_cursor() {
        let op = release_op();
        let mut chain = MockChain::new(20, 10);
        chain.put(15, exact_block(&op));
        assert_eq!(run(&chain, &op, 12, 100, T0), SearchOutcome::Found { block_index: 15 });
        // A match below the cursor is not seen: the caller already scanned there.
        let mut chain = MockChain::new(20, 10);
        chain.put(3, exact_block(&op));
        assert_eq!(run(&chain, &op, 12, 100, T0), SearchOutcome::NotFoundYet { next_block: 20, tip: 20 });
    }

    #[test]
    fn search_reaching_the_tip_with_an_open_window_is_not_found_yet() {
        let op = release_op();
        let chain = MockChain::new(20, 5);
        assert_eq!(run(&chain, &op, 0, 100, T0 + HOUR), SearchOutcome::NotFoundYet { next_block: 20, tip: 20 });
    }

    #[test]
    fn search_reaching_the_tip_after_the_window_proves_absence() {
        let op = release_op();
        let chain = MockChain::new(20, 5);
        assert_eq!(run(&chain, &op, 0, 100, CLOSED_AT), SearchOutcome::ProvenAbsent);
        // Exactly at the margin boundary it is still open.
        assert_eq!(
            run(&chain, &op, 0, 100, CLOSED_AT - 1),
            SearchOutcome::NotFoundYet { next_block: 20, tip: 20 }
        );
    }

    #[test]
    fn search_bounded_by_max_blocks_never_proves_absence() {
        let op = release_op();
        let chain = MockChain::new(50, 10);
        assert_eq!(run(&chain, &op, 0, 20, CLOSED_AT), SearchOutcome::NotFoundYet { next_block: 20, tip: 50 });
        assert_eq!(run(&chain, &op, 20, 20, CLOSED_AT), SearchOutcome::NotFoundYet { next_block: 40, tip: 50 });
        assert_eq!(run(&chain, &op, 40, 20, CLOSED_AT), SearchOutcome::ProvenAbsent);
    }

    #[test]
    fn search_truncated_by_the_ledger_cap_resumes_and_never_proves_absence() {
        let op = release_op();
        let mut chain = MockChain::new(50, 0);
        chain.ledger_cap = 8;
        assert_eq!(run(&chain, &op, 0, 100, CLOSED_AT), SearchOutcome::NotFoundYet { next_block: 8, tip: 50 });
        let mut cursor = 0;
        let mut rounds = 0;
        loop {
            match run(&chain, &op, cursor, 100, CLOSED_AT) {
                SearchOutcome::NotFoundYet { next_block, .. } => cursor = next_block,
                SearchOutcome::ProvenAbsent => break,
                other => panic!("unexpected {other:?}"),
            }
            rounds += 1;
            assert!(rounds < 20);
        }
        assert_eq!(cursor, 48, "the final round started at the last cursor and reached the tip");
    }

    #[test]
    fn search_with_a_truncating_archive_resumes_after_the_valid_prefix() {
        let op = release_op();
        let mut chain = MockChain::new(40, 30);
        chain.archive_cap = 12;
        chain.put(20, exact_block(&op));
        assert_eq!(run(&chain, &op, 0, 100, CLOSED_AT), SearchOutcome::NotFoundYet { next_block: 12, tip: 40 });
        assert_eq!(run(&chain, &op, 12, 100, CLOSED_AT), SearchOutcome::Found { block_index: 20 });
    }

    #[test]
    fn search_at_the_tip_with_no_new_blocks() {
        let op = release_op();
        let chain = MockChain::new(20, 5);
        assert_eq!(run(&chain, &op, 20, 100, T0 + HOUR), SearchOutcome::NotFoundYet { next_block: 20, tip: 20 });
        assert_eq!(run(&chain, &op, 20, 100, CLOSED_AT), SearchOutcome::ProvenAbsent);
    }

    #[test]
    fn search_cursor_beyond_the_tip_is_an_error() {
        let op = release_op();
        let chain = MockChain::new(20, 5);
        assert!(matches!(run(&chain, &op, 21, 100, CLOSED_AT), SearchOutcome::Error(_)));
    }

    #[test]
    fn search_with_zero_max_blocks_is_an_error() {
        let op = release_op();
        let chain = MockChain::new(20, 5);
        assert!(matches!(run(&chain, &op, 0, 0, CLOSED_AT), SearchOutcome::Error(_)));
    }

    #[test]
    fn call_failures_are_errors_even_when_the_window_is_closed() {
        let op = release_op();
        let mut chain = MockChain::new(20, 10);
        chain.fail_ledger = true;
        assert!(matches!(run(&chain, &op, 0, 100, CLOSED_AT), SearchOutcome::Error(_)));
        let mut chain = MockChain::new(20, 10);
        chain.fail_list = true;
        assert!(matches!(run(&chain, &op, 0, 100, CLOSED_AT), SearchOutcome::Error(_)));
        let mut chain = MockChain::new(20, 10);
        chain.fail_archive = true;
        assert!(matches!(run(&chain, &op, 0, 100, CLOSED_AT), SearchOutcome::Error(_)));
    }

    #[test]
    fn an_archive_the_ledger_does_not_list_is_never_called() {
        let op = release_op();
        let mut chain = MockChain::new(30, 20);
        chain.put(7, exact_block(&op));
        chain.listed = vec![principal(0xa2)];
        let outcome = run(&chain, &op, 0, 100, CLOSED_AT);
        assert!(matches!(&outcome, SearchOutcome::Error(e) if e.contains("not listed")), "{outcome:?}");
        assert_eq!(archive_calls(&chain), 0, "no call may reach an unlisted archive");
    }

    #[test]
    fn a_callback_pointing_at_a_foreign_canister_is_rejected_before_any_call() {
        let op = release_op();
        let mut chain = MockChain::new(30, 20);
        chain.put(7, exact_block(&op));
        chain.tamper_ledger = Some(Box::new(|r| {
            for a in r.archived_blocks.iter_mut() {
                a.callback = QueryArchiveFn::new(Principal::from_slice(&[0xbd; 10]), "icrc3_get_blocks");
            }
        }));
        let outcome = run(&chain, &op, 0, 100, CLOSED_AT);
        assert!(matches!(&outcome, SearchOutcome::Error(e) if e.contains("not listed")), "{outcome:?}");
        // MockChain panics if it is asked to call a canister that is neither the ledger,
        // the archive nor a listed archive, so reaching here proves no call was made.
        assert_eq!(archive_calls(&chain), 0);
    }

    #[test]
    fn nested_archive_references_are_not_followed() {
        let op = release_op();
        let mut chain = MockChain::new(30, 20);
        chain.put(7, exact_block(&op));
        chain.tamper_archive = Some(Box::new(|r| {
            r.archived_blocks.push(ArchivedBlocks {
                args: vec![GetBlocksRequest { start: Nat::from(0u8), length: Nat::from(1u8) }],
                callback: QueryArchiveFn::new(Principal::from_slice(&[0xbd; 10]), "icrc3_get_blocks"),
            });
        }));
        let outcome = run(&chain, &op, 0, 100, CLOSED_AT);
        assert!(matches!(&outcome, SearchOutcome::Error(e) if e.contains("nested")), "{outcome:?}");
    }

    #[test]
    fn archive_replies_with_wrong_ids_or_order_are_errors() {
        let op = release_op();
        let tampers: Vec<(&str, Tamper)> = vec![
            ("reversed", Box::new(|r| r.blocks.reverse())),
            ("shifted ids", Box::new(|r| {
                for b in r.blocks.iter_mut() {
                    b.id = Nat::from(nat_to_u64(&b.id).unwrap() + 1);
                }
            })),
            ("empty", Box::new(|r| r.blocks.clear())),
            ("gap", Box::new(|r| {
                r.blocks.remove(2);
            })),
            ("extra block", Box::new(|r| {
                let next = nat_to_u64(&r.blocks.last().unwrap().id).unwrap() + 1;
                r.blocks.push(BlockWithId { id: Nat::from(next), block: ICRC3Value::Nat(Nat::from(0u8)) });
            })),
        ];
        for (name, tamper) in tampers {
            let mut chain = MockChain::new(30, 20);
            chain.put(7, exact_block(&op));
            chain.tamper_archive = Some(tamper);
            let outcome = run(&chain, &op, 0, 100, CLOSED_AT);
            assert!(matches!(outcome, SearchOutcome::Error(_)), "{name}: {outcome:?}");
        }
    }

    #[test]
    fn a_forged_archive_block_with_the_wrong_id_cannot_produce_found() {
        // The archive returns the matching block, but labelled with the wrong id.
        let op = release_op();
        let mut chain = MockChain::new(30, 20);
        chain.tamper_archive = Some(Box::new({
            let block = exact_block(&op);
            move |r| r.blocks[3] = BlockWithId { id: Nat::from(99u8), block: block.clone() }
        }));
        assert!(matches!(run(&chain, &op, 0, 100, CLOSED_AT), SearchOutcome::Error(_)));
    }

    #[test]
    fn ledger_replies_with_bad_structure_are_errors() {
        let op = release_op();
        let tampers: Vec<(&str, Tamper)> = vec![
            ("inline out of order", Box::new(|r| r.blocks.reverse())),
            ("inline gap", Box::new(|r| {
                r.blocks.remove(1);
            })),
            ("block beyond window", Box::new(|r| {
                r.blocks.push(BlockWithId { id: Nat::from(500u16), block: ICRC3Value::Nat(Nat::from(0u8)) });
            })),
            ("log_length overflow", Box::new(|r| r.log_length = Nat::from(u64::MAX) + Nat::from(1u8))),
        ];
        for (name, tamper) in tampers {
            let mut chain = MockChain::new(20, 0);
            chain.tamper_ledger = Some(tamper);
            let outcome = run(&chain, &op, 0, 100, CLOSED_AT);
            assert!(matches!(outcome, SearchOutcome::Error(_)), "{name}: {outcome:?}");
        }
    }

    #[test]
    fn at_most_eight_archive_ranges_are_fetched_per_call() {
        let op = release_op();
        let mut chain = MockChain::new(130, 120);
        chain.tamper_ledger = Some(Box::new(|r| {
            // Present the archived part as separate ranges of ten blocks each.
            let callback = r.archived_blocks[0].callback.clone();
            let (start, length) = r.archived_blocks[0].args[0].as_start_and_length().unwrap();
            r.archived_blocks = (0..length / 10)
                .map(|i| ArchivedBlocks {
                    args: vec![GetBlocksRequest { start: Nat::from(start + i * 10), length: Nat::from(10u8) }],
                    callback: callback.clone(),
                })
                .collect();
        }));
        let outcome = run(&chain, &op, 0, 1_000, CLOSED_AT);
        assert_eq!(outcome, SearchOutcome::NotFoundYet { next_block: 80, tip: 130 });
        assert_eq!(archive_calls(&chain), MAX_ARCHIVE_RANGES_PER_CALL);
        // The next call resumes where this one stopped: four more archive ranges, then the
        // inline tail, reaching the tip.
        assert_eq!(run(&chain, &op, 80, 1_000, CLOSED_AT), SearchOutcome::ProvenAbsent);
        assert_eq!(archive_calls(&chain), MAX_ARCHIVE_RANGES_PER_CALL + 4);
    }

    #[test]
    fn non_transfer_blocks_do_not_stall_the_search() {
        let op = release_op();
        let mut chain = MockChain::new(20, 0);
        for (id, btype) in [(2, "1mint"), (3, "1burn"), (4, "2approve"), (5, "107feecol")] {
            let mut block: ICRC3Map = BTreeMap::new();
            block.insert("btype".into(), ICRC3Value::Text(btype.into()));
            chain.put(id, ICRC3Value::Map(block));
        }
        chain.put(15, exact_block(&op));
        assert_eq!(run(&chain, &op, 0, 100, CLOSED_AT), SearchOutcome::Found { block_index: 15 });
        let chain2 = {
            let mut c = MockChain::new(20, 0);
            let mut block: ICRC3Map = BTreeMap::new();
            block.insert("btype".into(), ICRC3Value::Text("1mint".into()));
            c.put(2, ICRC3Value::Map(block));
            c
        };
        assert_eq!(run(&chain2, &op, 0, 100, CLOSED_AT), SearchOutcome::ProvenAbsent);
    }

    #[test]
    fn an_undecodable_transfer_block_blocks_absence_but_not_a_later_match() {
        let op = release_op();
        // A transfer-typed block with no amount cannot be shown to be "not ours".
        let broken = {
            let mut spec = BlockSpec::for_op(&op);
            spec.amt = None;
            spec.build()
        };
        let mut chain = MockChain::new(20, 0);
        chain.put(6, broken.clone());
        let outcome = run(&chain, &op, 0, 100, CLOSED_AT);
        assert!(matches!(&outcome, SearchOutcome::Error(e) if e.contains("block 6")), "{outcome:?}");
        // Also not NotFoundYet: the cursor must not advance past it.
        let outcome = run(&chain, &op, 0, 100, T0);
        assert!(matches!(outcome, SearchOutcome::Error(_)), "{outcome:?}");
        // A genuine match later in the same window is still valid evidence.
        chain.put(12, exact_block(&op));
        assert_eq!(run(&chain, &op, 0, 100, CLOSED_AT), SearchOutcome::Found { block_index: 12 });
    }

    #[test]
    fn near_miss_blocks_never_produce_found() {
        // Every near-miss in the ledger, the true block absent: the window closing must
        // yield ProvenAbsent, not Found, and never an Error from the near misses.
        let op = pull_op();
        let mut chain = MockChain::new(20, 10);
        let base = BlockSpec::for_op(&op);
        let near: Vec<Perturb> = vec![
            Box::new(|s| s.spender = Some(account(4, None))),
            Box::new(|s| s.memo = Some(vec![0u8; 32])),
            Box::new(|s| s.ts = Some(T0 + 1)),
            Box::new(|s| s.amt = Some(5_001)),
            Box::new(|s| s.btype = Some("1xfer".into())),
        ];
        for (i, perturb) in near.iter().enumerate() {
            let mut spec = base.clone();
            perturb(&mut spec);
            chain.put(2 + i as u64 * 3, spec.build());
        }
        assert_eq!(run(&chain, &op, 0, 100, CLOSED_AT), SearchOutcome::ProvenAbsent);
    }
}
