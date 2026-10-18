//! Issued-token (icUSD) XRPL reads: trust lines, verified payment receipts,
//! paged issuer history, and the issuer-obligation snapshot.
//!
//! Everything here takes an explicit [`XrplNetwork`] (never derived from a signing
//! key name) and goes through [`read_provider_urls_for`], so a fixture build can
//! never reach mainnet. There is NO money-moving endpoint in this module; it only
//! produces independently checkable evidence.
//!
//! Design rules:
//!  * Each `transform_*` is a strict reducer (HTTP status pinned, headers dropped,
//!    only consumed fields kept) so IC replicas converge byte-for-byte.
//!  * Receipt and history facts are accepted only when >= `READ_QUORUM` providers
//!    return the identical reduced fact (a validated transaction is bound to one
//!    ledger hash, so honest providers agree exactly).
//!  * Credited value comes from `meta.delivered_amount` ONLY; the nominal `Amount`
//!    is kept as evidence and never credited.
//!  * History is read with FIXED ledger bounds and opaque markers; the cursor only
//!    moves past an interval when all of its pages were consumed, and the providers'
//!    `complete_ledgers` must cover the interval (typed recoverable `HistoryGap`).
//!  * All response sizes are bounded; the largest read is `MAX_HISTORY_BYTES`.

use ic_cdk::api::management_canister::http_request::{
    http_request, CanisterHttpRequestArgument, HttpResponse, TransformArgs,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::address::account_id_from_classic_address;
use super::issued_amount::{AmountError, AssetError, ExactIssuedAmount, IssuedAsset};
use super::network::{read_provider_urls_for, NetworkError, XrplNetwork};
use super::xrp_rpc::{
    build_request_to_url, is_transient, reduced_response, READ_QUORUM, READ_RETRIES,
};

// ---- bounds -----------------------------------------------------------------

pub const LINES_PAGE_LIMIT: u32 = 100;
pub const MAX_LINES_BYTES: u64 = 98_304;
pub const MAX_ISSUED_TX_BYTES: u64 = 65_536;
pub const MAX_SERVER_INFO_BYTES: u64 = 8_192;
pub const MAX_HISTORY_PAGE_LIMIT: u32 = 50;
pub const MAX_HISTORY_BYTES: u64 = 16_384 + MAX_HISTORY_PAGE_LIMIT as u64 * 8_192;
pub const MAX_OBLIGATION_PAGES: u32 = 64;
pub const LINES_CYCLES: u128 = 2_500_000_000;
pub const TX_CYCLES: u128 = 2_000_000_000;
pub const SERVER_INFO_CYCLES: u128 = 2_000_000_000;
pub const HISTORY_CYCLES: u128 = 6_000_000_000;
/// `tfPartialPayment`.
pub const TF_PARTIAL_PAYMENT: u32 = 0x0002_0000;

pub const TRANSFORM_LINES: &str = "xrp_transform_lines";
pub const TRANSFORM_ISSUED_TX: &str = "xrp_transform_issued_tx";
pub const TRANSFORM_ACCOUNT_TX: &str = "xrp_transform_account_tx";
pub const TRANSFORM_SERVER_INFO: &str = "xrp_transform_server_info";

// ---- errors -----------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReceiptError {
    Unparseable(String),
    HashMismatch {
        expected: String,
        got: String,
    },
    MissingField(&'static str),
    NotPayment(String),
    WrongAsset(String),
    /// `meta.delivered_amount` was absent, `"unavailable"`, or native.
    DeliveredAmountUnavailable,
    InvalidAmount(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssuedRpcError {
    Network(NetworkError),
    InvalidInput(String),
    Transport(String),
    NoQuorum(String),
    Unparseable(String),
    Receipt(ReceiptError),
    /// Providers' `complete_ledgers` (or validated tip) do not cover the requested
    /// interval. Recoverable: retry later or from other providers; never advance.
    HistoryGap {
        requested_min: u32,
        requested_max: u32,
        detail: String,
    },
    /// A paged reply made no forward progress (same marker echoed).
    NoProgress,
    SnapshotTooLarge {
        pages: u32,
    },
    SnapshotLedgerMismatch,
    TrustLinePagingUnsupported,
    Amount(AmountError),
    Asset(AssetError),
}

impl IssuedRpcError {
    /// True when retrying the same operation later is the right response.
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            IssuedRpcError::Transport(_)
                | IssuedRpcError::NoQuorum(_)
                | IssuedRpcError::HistoryGap { .. }
                | IssuedRpcError::SnapshotLedgerMismatch
        )
    }
}

impl std::fmt::Display for IssuedRpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl From<NetworkError> for IssuedRpcError {
    fn from(e: NetworkError) -> Self {
        IssuedRpcError::Network(e)
    }
}

// ---- small helpers ------------------------------------------------------------

pub(super) fn s(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(str::to_owned)
}
pub(super) fn u32_of(v: Option<&Value>) -> Option<u32> {
    v.and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
    })
    .and_then(|n| u32::try_from(n).ok())
}

pub(super) fn is_hex64(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Validate and uppercase a 32-byte transaction hash before it is ever sent out.
pub fn normalize_tx_hash(h: &str) -> Result<String, IssuedRpcError> {
    if is_hex64(h) {
        Ok(h.to_ascii_uppercase())
    } else {
        Err(IssuedRpcError::InvalidInput(
            "tx hash must be 64 hex chars".into(),
        ))
    }
}

pub(super) fn check_account(a: &str) -> Result<(), IssuedRpcError> {
    account_id_from_classic_address(a)
        .map(|_| ())
        .map_err(IssuedRpcError::InvalidInput)
}

fn marker_to_string(m: &Value) -> String {
    m.to_string()
}
fn marker_from_string(m: &str) -> Result<Value, IssuedRpcError> {
    serde_json::from_str(m).map_err(|_| IssuedRpcError::InvalidInput("bad marker".into()))
}

// ---- request builders -----------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgerSpec {
    Validated,
    Index(u32),
}

impl LedgerSpec {
    fn to_json(self) -> Value {
        match self {
            LedgerSpec::Validated => json!("validated"),
            LedgerSpec::Index(i) => json!(i),
        }
    }
}

/// A request described as plain data. `into_http` is the only step that touches the
/// IC (`TransformContext::from_name` needs a canister context), so everything up to
/// it is unit-testable on the host.
#[derive(Clone, Debug, PartialEq)]
pub struct RpcPlan {
    pub url: String,
    pub method: &'static str,
    pub params: Value,
    pub max_response_bytes: u64,
    pub transform: &'static str,
}

impl RpcPlan {
    pub fn into_http(self) -> CanisterHttpRequestArgument {
        build_request_to_url(
            &self.url,
            self.method,
            self.params,
            self.max_response_bytes,
            self.transform,
        )
    }
}

pub fn account_lines_request_to_url(
    url: &str,
    account: &str,
    peer: Option<&str>,
    ledger: LedgerSpec,
    marker: Option<&Value>,
    limit: u32,
) -> RpcPlan {
    let mut params = json!({
        "account": account,
        "ledger_index": ledger.to_json(),
        "limit": limit,
    });
    if let Some(peer) = peer {
        params["peer"] = json!(peer);
    }
    if let Some(m) = marker {
        params["marker"] = m.clone();
    }
    RpcPlan {
        url: url.to_string(),
        method: "account_lines",
        params,
        max_response_bytes: MAX_LINES_BYTES,
        transform: TRANSFORM_LINES,
    }
}

pub fn issued_tx_request_to_url(url: &str, tx_hash: &str) -> RpcPlan {
    RpcPlan {
        url: url.to_string(),
        method: "tx",
        params: json!({ "transaction": tx_hash, "binary": false }),
        max_response_bytes: MAX_ISSUED_TX_BYTES,
        transform: TRANSFORM_ISSUED_TX,
    }
}

pub fn account_tx_request_to_url(
    url: &str,
    account: &str,
    ledger_min: u32,
    ledger_max: u32,
    marker: Option<&Value>,
    limit: u32,
) -> RpcPlan {
    let mut params = json!({
        "account": account,
        "ledger_index_min": ledger_min,
        "ledger_index_max": ledger_max,
        "binary": false,
        "forward": true,
        "limit": limit,
    });
    if let Some(m) = marker {
        params["marker"] = m.clone();
    }
    RpcPlan {
        url: url.to_string(),
        method: "account_tx",
        params,
        max_response_bytes: history_max_response_bytes(limit),
        transform: TRANSFORM_ACCOUNT_TX,
    }
}

pub fn server_info_request_to_url(url: &str) -> RpcPlan {
    RpcPlan {
        url: url.to_string(),
        method: "server_info",
        params: json!({}),
        max_response_bytes: MAX_SERVER_INFO_BYTES,
        transform: TRANSFORM_SERVER_INFO,
    }
}

pub fn history_max_response_bytes(limit: u32) -> u64 {
    16_384 + u64::from(limit.min(MAX_HISTORY_PAGE_LIMIT)) * 8_192
}

// ---- transforms (pure reducers; exported as #[query] shims in main.rs) -----------

fn reduce_amount(v: Option<&Value>) -> Value {
    match v {
        Some(Value::String(x)) => json!(x),
        Some(Value::Object(o)) => json!({
            "currency": o.get("currency"),
            "issuer": o.get("issuer"),
            "value": o.get("value"),
        }),
        _ => Value::Null,
    }
}

/// Prefer `tx_json` (API v2) then fall back to the flat v1 layout.
fn pick<'a>(primary: &'a Value, fallback: &'a Value, key: &str) -> Option<&'a Value> {
    primary.get(key).or_else(|| fallback.get(key))
}

fn reduce_line_changes(meta: &Value) -> Value {
    let nodes = meta.get("AffectedNodes").and_then(Value::as_array);
    let mut out = Vec::new();
    for n in nodes.into_iter().flatten() {
        let (kind, body) = if let Some(b) = n.get("ModifiedNode") {
            ("Modified", b)
        } else if let Some(b) = n.get("CreatedNode") {
            ("Created", b)
        } else if let Some(b) = n.get("DeletedNode") {
            ("Deleted", b)
        } else {
            continue;
        };
        if body.get("LedgerEntryType").and_then(Value::as_str) != Some("RippleState") {
            continue;
        }
        let fin = if kind == "Created" {
            body.get("NewFields")
        } else {
            body.get("FinalFields")
        };
        let Some(fin) = fin else { continue };
        out.push(json!({
            "kind": kind,
            "low": fin.pointer("/LowLimit/issuer"),
            "high": fin.pointer("/HighLimit/issuer"),
            "currency": fin.pointer("/Balance/currency"),
            "final": fin.pointer("/Balance/value"),
            "prev": body.pointer("/PreviousFields/Balance/value"),
        }));
    }
    Value::Array(out)
}

/// Reduce one transaction (+ metadata) to the stable consumed fields.
fn reduce_tx(
    txj: &Value,
    flat: &Value,
    meta: &Value,
    validated: Option<&Value>,
    ledger_hash: Option<&Value>,
) -> Value {
    json!({
        "hash": pick(txj, flat, "hash"),
        "ledger_index": pick(txj, flat, "ledger_index"),
        "ledger_hash": ledger_hash,
        "validated": validated,
        "type": pick(txj, flat, "TransactionType"),
        "account": pick(txj, flat, "Account"),
        "destination": pick(txj, flat, "Destination"),
        "tag": pick(txj, flat, "DestinationTag"),
        "flags": pick(txj, flat, "Flags"),
        "amount": reduce_amount(pick(txj, flat, "Amount")),
        "has_paths": pick(txj, flat, "Paths").and_then(Value::as_array).map(|a| !a.is_empty()).unwrap_or(false),
        "has_send_max": pick(txj, flat, "SendMax").is_some(),
        "result": meta.get("TransactionResult"),
        "delivered": reduce_amount(meta.get("delivered_amount")),
        "lines": reduce_line_changes(meta),
    })
}

pub fn reduce_issued_tx_body(body: &[u8]) -> Vec<u8> {
    let reduced = match serde_json::from_slice::<Value>(body) {
        Ok(v) => {
            let r = v.get("result").unwrap_or(&Value::Null);
            let txj = r.get("tx_json").unwrap_or(r);
            let meta = r
                .get("meta")
                .or_else(|| r.get("metadata"))
                .unwrap_or(&Value::Null);
            let error = r.get("error");
            let tx = if error.is_some() {
                Value::Null
            } else {
                reduce_tx(txj, r, meta, r.get("validated"), r.get("ledger_hash"))
            };
            json!({ "error": error, "tx": tx })
        }
        Err(_) => json!({ "parse_error": true }),
    };
    reduced.to_string().into_bytes()
}

pub fn reduce_account_tx_body(body: &[u8]) -> Vec<u8> {
    let reduced = match serde_json::from_slice::<Value>(body) {
        Ok(v) => {
            let r = v.get("result").unwrap_or(&Value::Null);
            let txs: Vec<Value> = r
                .get("transactions")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|e| {
                            let flat = &Value::Null;
                            let txj = e.get("tx_json").or_else(|| e.get("tx")).unwrap_or(flat);
                            let meta = e.get("meta").or_else(|| e.get("metadata")).unwrap_or(flat);
                            // v2 puts hash/ledger_index at the entry level, v1 inside `tx`.
                            reduce_tx(txj, e, meta, e.get("validated"), e.get("ledger_hash"))
                        })
                        .collect()
                })
                .unwrap_or_default();
            json!({
                "error": r.get("error"),
                "ledger_index_min": r.get("ledger_index_min"),
                "ledger_index_max": r.get("ledger_index_max"),
                "marker": r.get("marker"),
                "txs": txs,
            })
        }
        Err(_) => json!({ "parse_error": true }),
    };
    reduced.to_string().into_bytes()
}

pub fn reduce_lines_body(body: &[u8]) -> Vec<u8> {
    let reduced = match serde_json::from_slice::<Value>(body) {
        Ok(v) => {
            let r = v.get("result").unwrap_or(&Value::Null);
            let lines: Vec<Value> = r
                .get("lines")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|l| {
                            json!({
                                "account": l.get("account"),
                                "balance": l.get("balance"),
                                "currency": l.get("currency"),
                                "limit": l.get("limit"),
                                "limit_peer": l.get("limit_peer"),
                                "no_ripple": l.get("no_ripple"),
                                "no_ripple_peer": l.get("no_ripple_peer"),
                                "freeze": l.get("freeze"),
                                "freeze_peer": l.get("freeze_peer"),
                                "authorized": l.get("authorized"),
                                "peer_authorized": l.get("peer_authorized"),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            json!({
                "error": r.get("error"),
                "account": r.get("account"),
                "ledger_index": r.get("ledger_index"),
                "ledger_hash": r.get("ledger_hash"),
                "validated": r.get("validated"),
                "marker": r.get("marker"),
                "lines": lines,
            })
        }
        Err(_) => json!({ "parse_error": true }),
    };
    reduced.to_string().into_bytes()
}

pub fn reduce_server_info_body(body: &[u8]) -> Vec<u8> {
    let reduced = match serde_json::from_slice::<Value>(body) {
        Ok(v) => json!({
            "error": v.pointer("/result/error"),
            "complete_ledgers": v.pointer("/result/info/complete_ledgers"),
            "validated_seq": v.pointer("/result/info/validated_ledger/seq"),
            "validated_hash": v.pointer("/result/info/validated_ledger/hash"),
        }),
        Err(_) => json!({ "parse_error": true }),
    };
    reduced.to_string().into_bytes()
}

pub fn transform_issued_tx(args: TransformArgs) -> HttpResponse {
    reduced_response(reduce_issued_tx_body(&args.response.body))
}
pub fn transform_account_tx(args: TransformArgs) -> HttpResponse {
    reduced_response(reduce_account_tx_body(&args.response.body))
}
pub fn transform_lines(args: TransformArgs) -> HttpResponse {
    reduced_response(reduce_lines_body(&args.response.body))
}
pub fn transform_server_info(args: TransformArgs) -> HttpResponse {
    reduced_response(reduce_server_info_body(&args.response.body))
}

// ---- reduced-tx parsing ------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RawAmount {
    Absent,
    Unavailable,
    Native(String),
    Issued {
        currency: String,
        issuer: String,
        value: String,
    },
}

impl RawAmount {
    fn from_value(v: Option<&Value>) -> RawAmount {
        match v {
            Some(Value::String(x)) if x == "unavailable" => RawAmount::Unavailable,
            Some(Value::String(x)) => RawAmount::Native(x.clone()),
            Some(Value::Object(o)) => RawAmount::Issued {
                currency: o
                    .get("currency")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                issuer: o
                    .get("issuer")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                value: o
                    .get("value")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            },
            _ => RawAmount::Absent,
        }
    }

    /// Exact value iff this is an issued amount of exactly `asset`.
    pub fn exact_issued(&self, asset: &IssuedAsset) -> Result<ExactIssuedAmount, AssetError> {
        match self {
            RawAmount::Issued {
                currency,
                issuer,
                value,
            } => {
                asset.check_identity(currency, issuer)?;
                ExactIssuedAmount::parse(value)
                    .map_err(|e| AssetError::BadAmountJson(e.to_string()))
            }
            _ => Err(AssetError::BadAmountJson(
                "not an issued-currency amount".into(),
            )),
        }
    }
}

/// A RippleState change as reported by metadata (raw strings; validated lazily).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawLineChange {
    pub kind: String,
    pub low_account: String,
    pub high_account: String,
    pub currency: String,
    pub final_low_balance: String,
    pub previous_low_balance: Option<String>,
}

/// Net effect of one metadata RippleState node on the expected asset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustLineBalanceChange {
    pub low_account: String,
    pub high_account: String,
    /// Balance from the LOW account's perspective (positive: low holds the token).
    pub previous_low_balance: ExactIssuedAmount,
    pub final_low_balance: ExactIssuedAmount,
}

impl TrustLineBalanceChange {
    /// Signed change in what `account` holds (negative: issuer-side obligation grew).
    pub fn delta_for(&self, account: &str) -> Option<ExactIssuedAmount> {
        let d = self
            .final_low_balance
            .checked_sub(&self.previous_low_balance)
            .ok()?;
        if account == self.low_account {
            Some(d)
        } else if account == self.high_account {
            Some(d.negated())
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedTx {
    pub hash: String,
    pub ledger_index: u32,
    pub ledger_hash: Option<String>,
    pub validated: bool,
    pub tx_type: String,
    pub account: String,
    pub destination: Option<String>,
    pub destination_tag: Option<u32>,
    pub flags: u32,
    pub engine_result: String,
    pub amount: RawAmount,
    pub delivered: RawAmount,
    pub has_paths: bool,
    pub has_send_max: bool,
    pub line_changes: Vec<RawLineChange>,
}

pub(super) fn parse_reduced_tx(v: &Value) -> Result<ParsedTx, String> {
    let line_changes = v
        .get("lines")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|l| {
                    Some(RawLineChange {
                        kind: s(l.get("kind"))?,
                        low_account: s(l.get("low"))?,
                        high_account: s(l.get("high"))?,
                        currency: s(l.get("currency"))?,
                        final_low_balance: s(l.get("final"))?,
                        previous_low_balance: s(l.get("prev")),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(ParsedTx {
        hash: s(v.get("hash"))
            .ok_or("tx without hash")?
            .to_ascii_uppercase(),
        ledger_index: u32_of(v.get("ledger_index")).ok_or("tx without ledger_index")?,
        ledger_hash: s(v.get("ledger_hash")).map(|h| h.to_ascii_uppercase()),
        validated: v.get("validated").and_then(Value::as_bool).unwrap_or(false),
        tx_type: s(v.get("type")).unwrap_or_default(),
        account: s(v.get("account")).unwrap_or_default(),
        destination: s(v.get("destination")),
        destination_tag: u32_of(v.get("tag")),
        flags: u32_of(v.get("flags")).unwrap_or(0),
        engine_result: s(v.get("result")).unwrap_or_default(),
        amount: RawAmount::from_value(v.get("amount")),
        delivered: RawAmount::from_value(v.get("delivered")),
        has_paths: v.get("has_paths").and_then(Value::as_bool).unwrap_or(false),
        has_send_max: v
            .get("has_send_max")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        line_changes,
    })
}

// ---- verified receipt ---------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderEvidence {
    pub provider: String,
    pub ledger_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeldReason {
    PartialPaymentFlag,
    DeliveredDiffersFromAmount,
    PathsOrSendMax,
    ZeroDelivery,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreditEligibility {
    /// A plain, non-partial, direct exact-asset payment whose delivered amount
    /// equals its nominal amount. (The caller still checks destination/tag/payer.)
    Eligible,
    /// Anomalous (partial/path/zero): held for recovery, never credited from `Amount`.
    Held(HeldReason),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifiedIssuedReceipt {
    pub hash: String,
    pub ledger_index: u32,
    pub ledger_hash: String,
    pub validated: bool,
    pub engine_result: String,
    pub account: String,
    pub destination: String,
    pub destination_tag: Option<u32>,
    pub flags: u32,
    /// Evidence only. NEVER the credited value.
    pub nominal_amount: Option<ExactIssuedAmount>,
    /// `meta.delivered_amount`, exact. The only creditable value.
    pub delivered_amount: ExactIssuedAmount,
    pub has_paths: bool,
    pub has_send_max: bool,
    /// Metadata trust-line balance changes for the expected asset.
    pub balance_changes: Vec<TrustLineBalanceChange>,
    pub provider_evidence: Vec<ProviderEvidence>,
}

impl VerifiedIssuedReceipt {
    pub fn credit_eligibility(&self) -> CreditEligibility {
        if self.flags & TF_PARTIAL_PAYMENT != 0 {
            return CreditEligibility::Held(HeldReason::PartialPaymentFlag);
        }
        if self.has_paths || self.has_send_max {
            return CreditEligibility::Held(HeldReason::PathsOrSendMax);
        }
        if self.delivered_amount.is_zero() || self.delivered_amount.is_negative() {
            return CreditEligibility::Held(HeldReason::ZeroDelivery);
        }
        if self.nominal_amount != Some(self.delivered_amount) {
            return CreditEligibility::Held(HeldReason::DeliveredDiffersFromAmount);
        }
        CreditEligibility::Eligible
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReceiptLookup {
    /// `txnNotFound` (quorum-agreed).
    NotFound,
    /// Found but not in a validated ledger: not credited, retry later.
    NotValidated,
    /// Validated but not `tesSUCCESS`: not credited.
    Failed(String),
    Verified(Box<VerifiedIssuedReceipt>),
}

/// Verify one provider body for `expected_hash`/`asset`. Pure.
pub fn verify_issued_receipt_body(
    body: &[u8],
    expected_hash: &str,
    asset: &IssuedAsset,
) -> Result<ReceiptLookup, ReceiptError> {
    let v: Value =
        serde_json::from_slice(body).map_err(|e| ReceiptError::Unparseable(e.to_string()))?;
    if v.get("parse_error").and_then(Value::as_bool) == Some(true) {
        return Err(ReceiptError::Unparseable("rippled response".into()));
    }
    match v.get("error").and_then(Value::as_str) {
        Some("txnNotFound") => return Ok(ReceiptLookup::NotFound),
        Some(e) => return Err(ReceiptError::Unparseable(format!("rpc error {e}"))),
        None => {}
    }
    let tx = v.get("tx").ok_or(ReceiptError::MissingField("tx"))?;
    let p = parse_reduced_tx(tx).map_err(ReceiptError::Unparseable)?;
    if !p.hash.eq_ignore_ascii_case(expected_hash) {
        return Err(ReceiptError::HashMismatch {
            expected: expected_hash.to_ascii_uppercase(),
            got: p.hash,
        });
    }
    if !p.validated {
        return Ok(ReceiptLookup::NotValidated);
    }
    if p.engine_result != "tesSUCCESS" {
        return Ok(ReceiptLookup::Failed(p.engine_result));
    }
    if p.tx_type != "Payment" {
        return Err(ReceiptError::NotPayment(p.tx_type));
    }
    let ledger_hash = p
        .ledger_hash
        .clone()
        .filter(|h| is_hex64(h))
        .ok_or(ReceiptError::MissingField("ledger_hash"))?;
    let destination = p
        .destination
        .clone()
        .ok_or(ReceiptError::MissingField("Destination"))?;
    // delivered_amount ONLY: absent / "unavailable" / native is not creditable.
    let delivered_amount = match &p.delivered {
        RawAmount::Issued { .. } => p
            .delivered
            .exact_issued(asset)
            .map_err(|e| ReceiptError::WrongAsset(e.to_string()))?,
        _ => return Err(ReceiptError::DeliveredAmountUnavailable),
    };
    // The nominal amount is evidence only; an unparseable/other-asset nominal does
    // not block recording (it will simply fail the equality check in eligibility).
    let nominal_amount = p.amount.exact_issued(asset).ok();
    let mut balance_changes = Vec::new();
    for c in &p.line_changes {
        let issuer_side =
            c.low_account == asset.issuer_address() || c.high_account == asset.issuer_address();
        if !issuer_side || !asset.currency.matches_rpc(&c.currency) {
            continue;
        }
        let fin = ExactIssuedAmount::parse(&c.final_low_balance)
            .map_err(|e| ReceiptError::InvalidAmount(e.to_string()))?;
        let prev = match (&c.previous_low_balance, c.kind.as_str()) {
            (Some(p), _) => ExactIssuedAmount::parse(p)
                .map_err(|e| ReceiptError::InvalidAmount(e.to_string()))?,
            (None, "Created") => ExactIssuedAmount::ZERO,
            (None, _) => fin,
        };
        balance_changes.push(TrustLineBalanceChange {
            low_account: c.low_account.clone(),
            high_account: c.high_account.clone(),
            previous_low_balance: prev,
            final_low_balance: fin,
        });
    }
    Ok(ReceiptLookup::Verified(Box::new(VerifiedIssuedReceipt {
        hash: p.hash,
        ledger_index: p.ledger_index,
        ledger_hash,
        validated: true,
        engine_result: p.engine_result,
        account: p.account,
        destination,
        destination_tag: p.destination_tag,
        flags: p.flags,
        nominal_amount,
        delivered_amount,
        has_paths: p.has_paths,
        has_send_max: p.has_send_max,
        balance_changes,
        provider_evidence: vec![],
    })))
}

/// Provider quorum over receipt lookups: >= `READ_QUORUM` providers must return the
/// identical outcome (for a verified receipt: identical fields INCLUDING the
/// validated ledger hash). A single provider's hostile or stale answer cannot win.
pub fn quorum_issued_receipt(
    provider_bodies: &[(String, Vec<u8>)],
    expected_hash: &str,
    asset: &IssuedAsset,
) -> Result<ReceiptLookup, IssuedRpcError> {
    if provider_bodies.len() < READ_QUORUM {
        return Err(IssuedRpcError::NoQuorum(format!(
            "need {READ_QUORUM} provider responses, got {}",
            provider_bodies.len()
        )));
    }
    let mut parsed: Vec<(&str, ReceiptLookup)> = Vec::new();
    let mut errors: Vec<ReceiptError> = Vec::new();
    for (url, body) in provider_bodies {
        match verify_issued_receipt_body(body, expected_hash, asset) {
            Ok(l) => parsed.push((url, l)),
            Err(e) => errors.push(e),
        }
    }
    // A hash mismatch from any provider is a hard failure (hostile or buggy node).
    if let Some(e) = errors
        .iter()
        .find(|e| matches!(e, ReceiptError::HashMismatch { .. }))
    {
        return Err(IssuedRpcError::Receipt(e.clone()));
    }
    for (_, cand) in &parsed {
        let agreeing: Vec<&str> = parsed
            .iter()
            .filter(|(_, l)| l == cand)
            .map(|(u, _)| *u)
            .collect();
        if agreeing.len() >= READ_QUORUM {
            let mut out = cand.clone();
            if let ReceiptLookup::Verified(r) = &mut out {
                r.provider_evidence = agreeing
                    .iter()
                    .map(|u| ProviderEvidence {
                        provider: (*u).to_string(),
                        ledger_hash: r.ledger_hash.clone(),
                    })
                    .collect();
            }
            return Ok(out);
        }
    }
    if let Some(e) = errors.into_iter().next() {
        if parsed.is_empty() {
            return Err(IssuedRpcError::Receipt(e));
        }
    }
    Err(IssuedRpcError::NoQuorum(format!(
        "receipt quorum failed: {} parsed responses disagree",
        parsed.len()
    )))
}

// ---- lines pages / trust lines --------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawLine {
    /// The counterparty ("account" in rippled's line object).
    pub peer: String,
    pub balance: ExactIssuedAmount,
    pub currency: String,
    pub limit: ExactIssuedAmount,
    pub limit_peer: ExactIssuedAmount,
    pub no_ripple: bool,
    pub no_ripple_peer: bool,
    pub freeze: bool,
    pub freeze_peer: bool,
    pub authorized: bool,
    pub peer_authorized: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinesPage {
    pub account_found: bool,
    pub ledger_index: u32,
    pub ledger_hash: String,
    pub marker: Option<String>,
    pub lines: Vec<RawLine>,
}

fn parse_amount_field(l: &Value, key: &str) -> Result<ExactIssuedAmount, IssuedRpcError> {
    let t = l
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| IssuedRpcError::Unparseable(format!("line missing {key}")))?;
    ExactIssuedAmount::parse(t).map_err(IssuedRpcError::Amount)
}

pub fn parse_lines_page(body: &[u8]) -> Result<LinesPage, IssuedRpcError> {
    let v: Value =
        serde_json::from_slice(body).map_err(|e| IssuedRpcError::Unparseable(e.to_string()))?;
    if v.get("parse_error").and_then(Value::as_bool) == Some(true) {
        return Err(IssuedRpcError::Unparseable("account_lines response".into()));
    }
    let ledger_index = u32_of(v.get("ledger_index"));
    let ledger_hash = s(v.get("ledger_hash")).map(|h| h.to_ascii_uppercase());
    match v.get("error").and_then(Value::as_str) {
        Some("actNotFound") => {
            return Ok(LinesPage {
                account_found: false,
                ledger_index: ledger_index.unwrap_or(0),
                ledger_hash: ledger_hash.unwrap_or_default(),
                marker: None,
                lines: vec![],
            })
        }
        Some(e) => {
            return Err(IssuedRpcError::Unparseable(format!(
                "account_lines error {e}"
            )))
        }
        None => {}
    }
    if v.get("validated").and_then(Value::as_bool) != Some(true) {
        return Err(IssuedRpcError::Unparseable(
            "account_lines ledger not validated".into(),
        ));
    }
    let ledger_index =
        ledger_index.ok_or_else(|| IssuedRpcError::Unparseable("missing ledger_index".into()))?;
    let ledger_hash = ledger_hash
        .filter(|h| is_hex64(h))
        .ok_or_else(|| IssuedRpcError::Unparseable("missing ledger_hash".into()))?;
    let mut lines = Vec::new();
    for l in v
        .get("lines")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        lines.push(RawLine {
            peer: s(l.get("account"))
                .ok_or_else(|| IssuedRpcError::Unparseable("line without account".into()))?,
            balance: parse_amount_field(l, "balance")?,
            currency: s(l.get("currency")).unwrap_or_default(),
            limit: parse_amount_field(l, "limit")?,
            limit_peer: parse_amount_field(l, "limit_peer")?,
            no_ripple: l.get("no_ripple").and_then(Value::as_bool).unwrap_or(false),
            no_ripple_peer: l
                .get("no_ripple_peer")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            freeze: l.get("freeze").and_then(Value::as_bool).unwrap_or(false),
            freeze_peer: l
                .get("freeze_peer")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            authorized: l
                .get("authorized")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            peer_authorized: l
                .get("peer_authorized")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        });
    }
    Ok(LinesPage {
        account_found: true,
        ledger_index,
        ledger_hash,
        marker: v
            .get("marker")
            .filter(|m| !m.is_null())
            .map(marker_to_string),
        lines,
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustLine {
    pub holder: String,
    pub issuer: String,
    /// Holder-perspective balance.
    pub balance: ExactIssuedAmount,
    pub limit: ExactIssuedAmount,
    pub limit_peer: ExactIssuedAmount,
    pub no_ripple: bool,
    pub no_ripple_peer: bool,
    pub freeze: bool,
    pub freeze_peer: bool,
    pub authorized: bool,
    pub peer_authorized: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustLineProof {
    /// `None`: the validated ledger has no such trust line (or no holder account).
    pub line: Option<TrustLine>,
    pub ledger_index: u32,
    pub ledger_hash: String,
    pub provider_evidence: Vec<ProviderEvidence>,
}

fn line_matches(l: &RawLine, asset: &IssuedAsset) -> bool {
    l.peer == asset.issuer_address() && asset.currency.matches_rpc(&l.currency)
}

/// Reduce one holder-side `account_lines` page to the trust line for `asset`.
pub fn trust_line_from_page(
    page: &LinesPage,
    holder: &str,
    asset: &IssuedAsset,
) -> Result<Option<TrustLine>, IssuedRpcError> {
    if page.marker.is_some() {
        // `peer` filtering leaves at most a handful of lines; a marker means a
        // hostile or unexpected shape. Fail closed rather than paging.
        return Err(IssuedRpcError::TrustLinePagingUnsupported);
    }
    let mut it = page.lines.iter().filter(|l| line_matches(l, asset));
    let first = it.next();
    if it.next().is_some() {
        return Err(IssuedRpcError::Unparseable("duplicate trust lines".into()));
    }
    Ok(first.map(|l| TrustLine {
        holder: holder.to_string(),
        issuer: asset.issuer_address(),
        balance: l.balance,
        limit: l.limit,
        limit_peer: l.limit_peer,
        no_ripple: l.no_ripple,
        no_ripple_peer: l.no_ripple_peer,
        freeze: l.freeze,
        freeze_peer: l.freeze_peer,
        authorized: l.authorized,
        peer_authorized: l.peer_authorized,
    }))
}

/// Semantic quorum for an unpinned (`validated`) holder trust-line read: providers
/// may be a ledger or two apart, so agreement is on the line itself; the reported
/// ledger is the newest among agreeing providers.
pub fn quorum_trust_line(
    provider_bodies: &[(String, Vec<u8>)],
    holder: &str,
    asset: &IssuedAsset,
) -> Result<TrustLineProof, IssuedRpcError> {
    if provider_bodies.len() < READ_QUORUM {
        return Err(IssuedRpcError::NoQuorum(
            "trust line: too few providers".into(),
        ));
    }
    let mut parsed: Vec<(&str, Option<TrustLine>, u32, String)> = Vec::new();
    let mut hard_err: Option<IssuedRpcError> = None;
    for (url, body) in provider_bodies {
        match parse_lines_page(body).and_then(|p| {
            trust_line_from_page(&p, holder, asset).map(|l| (l, p.ledger_index, p.ledger_hash))
        }) {
            Ok((l, idx, h)) => parsed.push((url, l, idx, h)),
            Err(e) => hard_err = Some(e),
        }
    }
    for (_, cand, _, _) in &parsed {
        let agreeing: Vec<_> = parsed.iter().filter(|(_, l, _, _)| l == cand).collect();
        if agreeing.len() >= READ_QUORUM {
            let newest = agreeing.iter().max_by_key(|(_, _, idx, _)| *idx).unwrap();
            return Ok(TrustLineProof {
                line: cand.clone(),
                ledger_index: newest.2,
                ledger_hash: newest.3.clone(),
                provider_evidence: agreeing
                    .iter()
                    .map(|(u, _, _, h)| ProviderEvidence {
                        provider: (*u).to_string(),
                        ledger_hash: h.clone(),
                    })
                    .collect(),
            });
        }
    }
    match hard_err {
        Some(e) if parsed.is_empty() => Err(e),
        _ => Err(IssuedRpcError::NoQuorum("trust line quorum failed".into())),
    }
}

// ---- issuer obligation snapshot --------------------------------------------------------

/// Exact running total of issuer obligations (holders' positive balances), kept as
/// whole e8s plus a 1e-40 fixed-point sub-e8s tally so no holder dust can make the
/// sum inexact or fail it (a dust line is a cheap grief vector otherwise).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObligationTotal {
    pub whole_e8s: u128,
    /// Sub-e8s dust in units of 1e-40 icUSD (1 e8s == 1e32 units).
    pub sub_e8s_units_1e40: u128,
    /// Lines whose dust was finer than 1e-40 (rounded up at the end).
    pub finer_dust_lines: u64,
    pub holder_lines: u64,
    /// Lines where the issuer is owed (should be zero).
    pub anomalous_positive_lines: u64,
}

const UNITS_PER_E8S: u128 = 100_000_000_000_000_000_000_000_000_000_000; // 1e32

impl ObligationTotal {
    pub fn add_holder_balance(
        &mut self,
        issuer_balance: &ExactIssuedAmount,
    ) -> Result<(), IssuedRpcError> {
        if issuer_balance.is_zero() {
            return Ok(());
        }
        if !issuer_balance.is_negative() {
            self.anomalous_positive_lines += 1;
            return Ok(());
        }
        let owed = issuer_balance.negated();
        let (floor, rem) = owed.split_e8s().map_err(IssuedRpcError::Amount)?;
        self.whole_e8s = self
            .whole_e8s
            .checked_add(u128::from(floor))
            .ok_or(IssuedRpcError::Amount(AmountError::Overflow))?;
        if !rem.is_zero() {
            // rem < 1e-8, mantissa in [1e15, 1e16), exponent < -23.
            let shift = rem.exponent() as i64 + 40;
            let units = if shift >= 0 {
                u128::from(rem.mantissa())
                    .checked_mul(10u128.pow(shift as u32))
                    .ok_or(IssuedRpcError::Amount(AmountError::Overflow))?
            } else {
                let k = (-shift) as u32;
                if k > 16 {
                    self.finer_dust_lines += 1;
                    0
                } else {
                    let div = 10u128.pow(k);
                    if u128::from(rem.mantissa()) % div != 0 {
                        self.finer_dust_lines += 1;
                    }
                    u128::from(rem.mantissa()) / div
                }
            };
            self.sub_e8s_units_1e40 = self
                .sub_e8s_units_1e40
                .checked_add(units)
                .ok_or(IssuedRpcError::Amount(AmountError::Overflow))?;
        }
        self.holder_lines += 1;
        Ok(())
    }

    /// Conservative (rounded UP) total in e8s for coverage checks.
    pub fn ceil_e8s(&self) -> u128 {
        let dust = self.sub_e8s_units_1e40 + u128::from(self.finer_dust_lines > 0);
        self.whole_e8s + dust.div_ceil(UNITS_PER_E8S)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuerObligationSnapshot {
    pub ledger_index: u32,
    pub ledger_hash: String,
    pub total: ObligationTotal,
    pub pages: u32,
    pub provider_evidence: Vec<ProviderEvidence>,
}

/// Add one issuer-side page (already pinned/verified) into the running total.
pub fn accumulate_obligation_page(
    total: &mut ObligationTotal,
    page: &LinesPage,
    asset: &IssuedAsset,
) -> Result<(), IssuedRpcError> {
    for l in &page.lines {
        if asset.currency.matches_rpc(&l.currency) {
            total.add_holder_balance(&l.balance)?;
        }
    }
    Ok(())
}

// ---- server info / history coverage ------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerInfo {
    pub complete_ledgers: String,
    pub validated_seq: u32,
    pub validated_hash: String,
}

pub fn parse_server_info(body: &[u8]) -> Result<ServerInfo, IssuedRpcError> {
    let v: Value =
        serde_json::from_slice(body).map_err(|e| IssuedRpcError::Unparseable(e.to_string()))?;
    if v.get("parse_error").and_then(Value::as_bool) == Some(true)
        || v.get("error").is_some_and(|e| !e.is_null())
    {
        return Err(IssuedRpcError::Unparseable("server_info".into()));
    }
    Ok(ServerInfo {
        complete_ledgers: s(v.get("complete_ledgers"))
            .ok_or_else(|| IssuedRpcError::Unparseable("no complete_ledgers".into()))?,
        validated_seq: u32_of(v.get("validated_seq"))
            .ok_or_else(|| IssuedRpcError::Unparseable("no validated_seq".into()))?,
        validated_hash: s(v.get("validated_hash"))
            .map(|h| h.to_ascii_uppercase())
            .filter(|h| is_hex64(h))
            .ok_or_else(|| IssuedRpcError::Unparseable("no validated_hash".into()))?,
    })
}

/// True iff a single contiguous range in rippled's `complete_ledgers` covers
/// `[min, max]`. ("empty" and malformed strings cover nothing.)
pub fn ledger_range_covers(complete_ledgers: &str, min: u32, max: u32) -> bool {
    complete_ledgers.split(',').any(|part| {
        let part = part.trim();
        let (lo, hi) = match part.split_once('-') {
            Some((a, b)) => (a.trim().parse::<u32>().ok(), b.trim().parse::<u32>().ok()),
            None => {
                let n = part.parse::<u32>().ok();
                (n, n)
            }
        };
        matches!((lo, hi), (Some(lo), Some(hi)) if lo <= min && max <= hi)
    })
}

/// Choose providers able to serve `[min, max]`; typed recoverable `HistoryGap` unless
/// `READ_QUORUM` of them cover it AND have it validated.
pub fn history_providers_covering(
    infos: &[(String, Result<ServerInfo, IssuedRpcError>)],
    min: u32,
    max: u32,
) -> Result<Vec<String>, IssuedRpcError> {
    let covering: Vec<String> = infos
        .iter()
        .filter_map(|(url, info)| match info {
            Ok(i)
                if i.validated_seq >= max && ledger_range_covers(&i.complete_ledgers, min, max) =>
            {
                Some(url.clone())
            }
            _ => None,
        })
        .collect();
    if covering.len() < READ_QUORUM {
        let detail = infos
            .iter()
            .map(|(u, i)| match i {
                Ok(i) => format!(
                    "{u}: complete={} tip={}",
                    i.complete_ledgers, i.validated_seq
                ),
                Err(e) => format!("{u}: {e}"),
            })
            .collect::<Vec<_>>()
            .join("; ");
        return Err(IssuedRpcError::HistoryGap {
            requested_min: min,
            requested_max: max,
            detail,
        });
    }
    Ok(covering)
}

/// Pin a validated ledger for a snapshot: the LOWEST tip among responding
/// providers (every provider has it), requiring `READ_QUORUM` responses.
pub fn pin_snapshot_ledger(
    infos: &[(String, Result<ServerInfo, IssuedRpcError>)],
) -> Result<(u32, String), IssuedRpcError> {
    let ok: Vec<&ServerInfo> = infos.iter().filter_map(|(_, i)| i.as_ref().ok()).collect();
    if ok.len() < READ_QUORUM {
        return Err(IssuedRpcError::NoQuorum(
            "server_info: too few providers".into(),
        ));
    }
    let low = ok.iter().min_by_key(|i| i.validated_seq).unwrap();
    Ok((low.validated_seq, low.validated_hash.clone()))
}

// ---- issuer history --------------------------------------------------------------------

/// Fixed-bounds history cursor. `ledger_min..=ledger_max` never changes while an
/// interval is in progress; `marker` is rippled's opaque continuation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuerHistoryCursor {
    pub ledger_min: u32,
    pub ledger_max: u32,
    pub marker: Option<String>,
}

impl IssuerHistoryCursor {
    pub fn new(ledger_min: u32, ledger_max: u32) -> Result<Self, IssuedRpcError> {
        if ledger_min == 0 || ledger_min > ledger_max {
            return Err(IssuedRpcError::InvalidInput("bad ledger interval".into()));
        }
        Ok(Self {
            ledger_min,
            ledger_max,
            marker: None,
        })
    }

    /// Progress after a page whose transactions have ALL been durably processed.
    /// Mid-interval pages keep the interval bounds and only swap in the new marker;
    /// the interval is reported complete only when no continuation marker remains.
    pub fn advance(&self, page: &IssuerHistoryPage) -> HistoryProgress {
        if page.interval_complete {
            HistoryProgress::IntervalComplete {
                next_ledger_min: self.ledger_max.saturating_add(1),
            }
        } else {
            HistoryProgress::Continue(IssuerHistoryCursor {
                ledger_min: self.ledger_min,
                ledger_max: self.ledger_max,
                marker: page.next_marker.clone(),
            })
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryProgress {
    Continue(IssuerHistoryCursor),
    IntervalComplete { next_ledger_min: u32 },
}

/// One issuer-history page. These are discovery facts; any payment must still be
/// confirmed with `fetch_issued_receipt` before value is credited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuerHistoryPage {
    pub txs: Vec<ParsedTx>,
    pub next_marker: Option<String>,
    pub interval_complete: bool,
    /// Highest ledger known to be FULLY included (None when unknown): a page that
    /// ends mid-ledger does not complete that ledger.
    pub fully_consumed_through: Option<u32>,
    pub ledger_min: u32,
    pub ledger_max: u32,
    pub provider_evidence: Vec<ProviderEvidence>,
}

pub fn parse_account_tx_page(
    body: &[u8],
    ledger_min: u32,
    ledger_max: u32,
) -> Result<IssuerHistoryPage, IssuedRpcError> {
    let v: Value =
        serde_json::from_slice(body).map_err(|e| IssuedRpcError::Unparseable(e.to_string()))?;
    if v.get("parse_error").and_then(Value::as_bool) == Some(true) {
        return Err(IssuedRpcError::Unparseable("account_tx response".into()));
    }
    if let Some(e) = v.get("error").and_then(Value::as_str) {
        return Err(IssuedRpcError::Unparseable(format!("account_tx error {e}")));
    }
    // The server must honour OUR bounds; anything else is a different query.
    if u32_of(v.get("ledger_index_min")) != Some(ledger_min)
        || u32_of(v.get("ledger_index_max")) != Some(ledger_max)
    {
        return Err(IssuedRpcError::Unparseable(
            "account_tx bounds not honoured".into(),
        ));
    }
    let mut txs = Vec::new();
    for t in v.get("txs").and_then(Value::as_array).into_iter().flatten() {
        let p = parse_reduced_tx(t).map_err(IssuedRpcError::Unparseable)?;
        if !p.validated {
            return Err(IssuedRpcError::Unparseable(
                "unvalidated history entry".into(),
            ));
        }
        if p.ledger_index < ledger_min || p.ledger_index > ledger_max {
            return Err(IssuedRpcError::Unparseable(
                "history entry outside bounds".into(),
            ));
        }
        txs.push(p);
    }
    let marker_val = v.get("marker").filter(|m| !m.is_null());
    let next_marker = marker_val.map(marker_to_string);
    let interval_complete = next_marker.is_none();
    let fully_consumed_through = if interval_complete {
        Some(ledger_max)
    } else {
        // rippled markers are {"ledger": N, "seq": M}: ledger N is only partly read.
        marker_val
            .and_then(|m| u32_of(m.get("ledger")))
            .and_then(|n| n.checked_sub(1))
            .filter(|n| *n >= ledger_min)
    };
    Ok(IssuerHistoryPage {
        txs,
        next_marker,
        interval_complete,
        fully_consumed_through,
        ledger_min,
        ledger_max,
        provider_evidence: vec![],
    })
}

pub fn quorum_history_page(
    provider_bodies: &[(String, Vec<u8>)],
    ledger_min: u32,
    ledger_max: u32,
) -> Result<IssuerHistoryPage, IssuedRpcError> {
    if provider_bodies.len() < READ_QUORUM {
        return Err(IssuedRpcError::NoQuorum(
            "history: too few providers".into(),
        ));
    }
    let mut parsed: Vec<(&str, IssuerHistoryPage)> = Vec::new();
    let mut last_err = None;
    for (url, body) in provider_bodies {
        match parse_account_tx_page(body, ledger_min, ledger_max) {
            Ok(p) => parsed.push((url, p)),
            Err(e) => last_err = Some(e),
        }
    }
    for (_, cand) in &parsed {
        let agreeing: Vec<&str> = parsed
            .iter()
            .filter(|(_, p)| p == cand)
            .map(|(u, _)| *u)
            .collect();
        if agreeing.len() >= READ_QUORUM {
            let mut out = cand.clone();
            out.provider_evidence = agreeing
                .iter()
                .map(|u| ProviderEvidence {
                    provider: (*u).to_string(),
                    ledger_hash: String::new(),
                })
                .collect();
            return Ok(out);
        }
    }
    match last_err {
        Some(e) if parsed.is_empty() => Err(e),
        _ => Err(IssuedRpcError::NoQuorum(
            "history page quorum failed".into(),
        )),
    }
}

// ---- async fetchers ------------------------------------------------------------------------

pub(super) enum Verdict {
    Retry(String),
    Fatal(IssuedRpcError),
}

async fn call_providers<F>(
    urls: &[&str],
    build: &F,
    cycles: u128,
) -> (Vec<(String, Vec<u8>)>, Vec<String>, bool)
where
    F: Fn(&str) -> CanisterHttpRequestArgument,
{
    let mut bodies = Vec::with_capacity(urls.len());
    let mut errors = Vec::new();
    let mut transient = false;
    for url in urls {
        match http_request(build(url), cycles).await {
            Ok((resp,)) => bodies.push((url.to_string(), resp.body)),
            Err((code, msg)) => {
                transient |= is_transient(code, &msg);
                errors.push(format!("{url}: {code:?}: {msg}"));
            }
        }
    }
    (bodies, errors, transient)
}

/// Fan a read out to `urls`, then hand the tagged bodies to `quorum`; consensus-miss
/// class transport errors and `Retry` verdicts are retried (reads are idempotent).
pub(super) async fn read_with_quorum<F, T, Q>(
    urls: &[&str],
    build: F,
    cycles: u128,
    quorum: Q,
) -> Result<T, IssuedRpcError>
where
    F: Fn(&str) -> CanisterHttpRequestArgument,
    Q: Fn(&[(String, Vec<u8>)]) -> Result<T, Verdict>,
{
    let mut last = String::new();
    for attempt in 0..READ_RETRIES {
        let (bodies, errors, transient) = call_providers(urls, &build, cycles).await;
        if bodies.len() < READ_QUORUM.min(urls.len()) {
            last = format!("attempt {}: {}", attempt + 1, errors.join("; "));
            if !transient {
                break;
            }
            continue;
        }
        match quorum(&bodies) {
            Ok(v) => return Ok(v),
            Err(Verdict::Fatal(e)) => return Err(e),
            Err(Verdict::Retry(m)) => last = format!("attempt {}: {m}", attempt + 1),
        }
    }
    Err(IssuedRpcError::Transport(last))
}

pub(super) fn verdict(e: IssuedRpcError) -> Verdict {
    if e.is_recoverable() {
        Verdict::Retry(e.to_string())
    } else {
        Verdict::Fatal(e)
    }
}

/// Holder trust line for `asset` from the newest validated ledger (semantic quorum).
pub async fn fetch_trust_line(
    network: XrplNetwork,
    holder: &str,
    asset: &IssuedAsset,
) -> Result<TrustLineProof, IssuedRpcError> {
    check_account(holder)?;
    let urls = read_provider_urls_for(network)?;
    let peer = asset.issuer_address();
    read_with_quorum(
        urls,
        |url| {
            account_lines_request_to_url(
                url,
                holder,
                Some(&peer),
                LedgerSpec::Validated,
                None,
                LINES_PAGE_LIMIT,
            )
            .into_http()
        },
        LINES_CYCLES,
        |bodies| quorum_trust_line(bodies, holder, asset).map_err(verdict),
    )
    .await
}

/// Validated receipt for `tx_hash` (hash is checked against the reply; wallet-reported
/// hashes are only ever lookup keys, never evidence).
pub async fn fetch_issued_receipt(
    network: XrplNetwork,
    tx_hash: &str,
    asset: &IssuedAsset,
) -> Result<ReceiptLookup, IssuedRpcError> {
    let hash = normalize_tx_hash(tx_hash)?;
    let urls = read_provider_urls_for(network)?;
    read_with_quorum(
        urls,
        |url| issued_tx_request_to_url(url, &hash).into_http(),
        TX_CYCLES,
        |bodies| quorum_issued_receipt(bodies, &hash, asset).map_err(verdict),
    )
    .await
}

pub(super) async fn fetch_server_infos(urls: &[&str]) -> Vec<(String, Result<ServerInfo, IssuedRpcError>)> {
    let (bodies, errors, _) = call_providers(
        urls,
        &|u: &str| server_info_request_to_url(u).into_http(),
        SERVER_INFO_CYCLES,
    )
    .await;
    let mut out: Vec<_> = bodies
        .into_iter()
        .map(|(u, b)| {
            let i = parse_server_info(&b);
            (u, i)
        })
        .collect();
    for e in errors {
        out.push((e.clone(), Err(IssuedRpcError::Transport(e))));
    }
    out
}

/// One bounded page of issuer history for the cursor's fixed interval.
pub async fn fetch_issuer_history(
    network: XrplNetwork,
    account: &str,
    cursor: &IssuerHistoryCursor,
    limit: u32,
) -> Result<IssuerHistoryPage, IssuedRpcError> {
    check_account(account)?;
    if limit == 0 || limit > MAX_HISTORY_PAGE_LIMIT {
        return Err(IssuedRpcError::InvalidInput(format!(
            "limit must be 1..={MAX_HISTORY_PAGE_LIMIT}"
        )));
    }
    if cursor.ledger_min == 0 || cursor.ledger_min > cursor.ledger_max {
        return Err(IssuedRpcError::InvalidInput("bad ledger interval".into()));
    }
    let marker = cursor
        .marker
        .as_deref()
        .map(marker_from_string)
        .transpose()?;
    let all_urls = read_provider_urls_for(network)?;
    let infos = fetch_server_infos(all_urls).await;
    let covering = history_providers_covering(&infos, cursor.ledger_min, cursor.ledger_max)?;
    let covering_refs: Vec<&str> = covering.iter().map(String::as_str).collect();
    let page = read_with_quorum(
        &covering_refs,
        |url| {
            account_tx_request_to_url(
                url,
                account,
                cursor.ledger_min,
                cursor.ledger_max,
                marker.as_ref(),
                limit,
            )
            .into_http()
        },
        HISTORY_CYCLES,
        |bodies| quorum_history_page(bodies, cursor.ledger_min, cursor.ledger_max).map_err(verdict),
    )
    .await?;
    if cursor.marker.is_some() && page.next_marker == cursor.marker {
        return Err(IssuedRpcError::NoProgress);
    }
    Ok(page)
}

/// Issuer obligations for `asset` at one fixed validated ledger, summed exactly over
/// all pages (<= `MAX_OBLIGATION_PAGES`; otherwise a typed error, never a partial
/// total).
pub async fn fetch_issuer_obligation_snapshot(
    network: XrplNetwork,
    asset: &IssuedAsset,
) -> Result<IssuerObligationSnapshot, IssuedRpcError> {
    let urls = read_provider_urls_for(network)?;
    let issuer = asset.issuer_address();
    let infos = fetch_server_infos(urls).await;
    let (index, hash) = pin_snapshot_ledger(&infos)?;
    let mut total = ObligationTotal::default();
    let mut marker: Option<Value> = None;
    let mut pages = 0u32;
    let mut evidence = Vec::new();
    loop {
        if pages >= MAX_OBLIGATION_PAGES {
            return Err(IssuedRpcError::SnapshotTooLarge { pages });
        }
        let m = marker.clone();
        let (page, ev) = read_with_quorum(
            urls,
            |url| {
                account_lines_request_to_url(
                    url,
                    &issuer,
                    None,
                    LedgerSpec::Index(index),
                    m.as_ref(),
                    LINES_PAGE_LIMIT,
                )
                .into_http()
            },
            LINES_CYCLES,
            |bodies| quorum_pinned_lines_page(bodies, &hash).map_err(verdict),
        )
        .await?;
        pages += 1;
        accumulate_obligation_page(&mut total, &page, asset)?;
        evidence = ev;
        match page.marker {
            None => break,
            Some(next) => {
                if marker.as_ref().map(marker_to_string).as_deref() == Some(next.as_str()) {
                    return Err(IssuedRpcError::NoProgress);
                }
                marker = Some(marker_from_string(&next)?);
            }
        }
    }
    Ok(IssuerObligationSnapshot {
        ledger_index: index,
        ledger_hash: hash,
        total,
        pages,
        provider_evidence: evidence,
    })
}

/// Pinned-ledger page quorum: >= `READ_QUORUM` providers must return the IDENTICAL
/// page and it must be at the pinned ledger hash.
pub fn quorum_pinned_lines_page(
    provider_bodies: &[(String, Vec<u8>)],
    pinned_hash: &str,
) -> Result<(LinesPage, Vec<ProviderEvidence>), IssuedRpcError> {
    if provider_bodies.len() < READ_QUORUM {
        return Err(IssuedRpcError::NoQuorum("lines: too few providers".into()));
    }
    let mut parsed: Vec<(&str, LinesPage)> = Vec::new();
    for (url, body) in provider_bodies {
        if let Ok(p) = parse_lines_page(body) {
            if p.account_found {
                parsed.push((url, p));
            }
        }
    }
    if parsed
        .iter()
        .any(|(_, p)| !p.ledger_hash.eq_ignore_ascii_case(pinned_hash))
    {
        return Err(IssuedRpcError::SnapshotLedgerMismatch);
    }
    for (_, cand) in &parsed {
        let agreeing: Vec<&str> = parsed
            .iter()
            .filter(|(_, p)| p == cand)
            .map(|(u, _)| *u)
            .collect();
        if agreeing.len() >= READ_QUORUM {
            let ev = agreeing
                .iter()
                .map(|u| ProviderEvidence {
                    provider: (*u).to_string(),
                    ledger_hash: cand.ledger_hash.clone(),
                })
                .collect();
            return Ok((cand.clone(), ev));
        }
    }
    Err(IssuedRpcError::NoQuorum(
        "pinned lines page quorum failed".into(),
    ))
}

// ---------------------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    const ISSUER: &str = "rM2a5NiwBDRxoWCTnisrGaGGfmXC2w8FaW";
    const HOLDER: &str = "r3r5SV7NAnKKRbgBFcLvSYH786Ac8Dv4ik";
    const OTHER_ISSUER: &str = "rLUEXYuLiQptky37CqLcm9USQpPiz5rkpD";
    const CUR: &str = "6963555344000000000000000000000000000000";
    const TXH: &str = "A1B2C3D4E5F60718293A4B5C6D7E8F90A1B2C3D4E5F60718293A4B5C6D7E8F90";
    const LH: &str = "11112222333344445555666677778888999900001111222233334444AAAABBBB";

    fn asset() -> IssuedAsset {
        IssuedAsset::icusd(ISSUER).unwrap()
    }

    fn iou(value: &str) -> Value {
        json!({"currency": CUR, "issuer": ISSUER, "value": value})
    }

    /// Raw rippled-shaped `tx` response (API v1 layout).
    fn raw_tx(f: impl FnOnce(&mut Value)) -> Vec<u8> {
        let mut r = json!({
            "result": {
                "Account": HOLDER,
                "Destination": ISSUER,
                "DestinationTag": 4242,
                "TransactionType": "Payment",
                "Flags": 0,
                "Amount": iou("25.5"),
                "Fee": "12",
                "Sequence": 5,
                "hash": TXH,
                "ledger_index": 9000050,
                "ledger_hash": LH,
                "validated": true,
                "date": 123456,
                "meta": {
                    "TransactionResult": "tesSUCCESS",
                    "delivered_amount": iou("25.5"),
                    "AffectedNodes": [
                        {"ModifiedNode": {
                            "LedgerEntryType": "RippleState",
                            "FinalFields": {
                                "Balance": {"currency": CUR, "issuer": "rrrrrrrrrrrrrrrrrrrrBZbvji", "value": "-100"},
                                "HighLimit": {"currency": CUR, "issuer": ISSUER, "value": "0"},
                                "LowLimit": {"currency": CUR, "issuer": HOLDER, "value": "1000000"}
                            },
                            "PreviousFields": {"Balance": {"currency": CUR, "issuer": "rrrrrrrrrrrrrrrrrrrrBZbvji", "value": "-74.5"}}
                        }},
                        {"ModifiedNode": {"LedgerEntryType": "AccountRoot", "FinalFields": {"Balance": "1"}}},
                        {"ModifiedNode": {
                            "LedgerEntryType": "RippleState",
                            "FinalFields": {
                                "Balance": {"currency": "USD", "issuer": "rrrrrrrrrrrrrrrrrrrrBZbvji", "value": "5"},
                                "HighLimit": {"currency": "USD", "issuer": ISSUER, "value": "0"},
                                "LowLimit": {"currency": "USD", "issuer": HOLDER, "value": "10"}
                            }
                        }}
                    ]
                }
            }
        });
        f(&mut r["result"]);
        serde_json::to_vec(&r).unwrap()
    }

    fn reduce(raw: &[u8]) -> Vec<u8> {
        reduce_issued_tx_body(raw)
    }

    fn lookup(raw: &[u8]) -> Result<ReceiptLookup, ReceiptError> {
        verify_issued_receipt_body(&reduce(raw), TXH, &asset())
    }

    fn verified(raw: &[u8]) -> VerifiedIssuedReceipt {
        match lookup(raw).unwrap() {
            ReceiptLookup::Verified(r) => *r,
            other => panic!("expected verified, got {other:?}"),
        }
    }

    #[test]
    fn receipt_verifies_and_extracts_exact_evidence() {
        let r = verified(&raw_tx(|_| {}));
        assert_eq!(r.hash, TXH);
        assert_eq!(r.ledger_index, 9_000_050);
        assert_eq!(r.ledger_hash, LH);
        assert!(r.validated);
        assert_eq!(r.engine_result, "tesSUCCESS");
        assert_eq!(r.account, HOLDER);
        assert_eq!(r.destination, ISSUER);
        assert_eq!(r.destination_tag, Some(4242));
        assert_eq!(
            r.delivered_amount,
            ExactIssuedAmount::parse("25.5").unwrap()
        );
        assert_eq!(r.credit_eligibility(), CreditEligibility::Eligible);
        // Only the icUSD RippleState node is kept (USD and AccountRoot are not).
        assert_eq!(r.balance_changes.len(), 1);
        let c = &r.balance_changes[0];
        // Holder is the LOW account here: its balance rose by 25.5, so the issuer's
        // side (HIGH account) moved by -25.5 (obligation retired).
        assert_eq!(
            c.delta_for(HOLDER),
            Some(ExactIssuedAmount::parse("-25.5").unwrap())
        );
        assert_eq!(
            c.delta_for(ISSUER),
            Some(ExactIssuedAmount::parse("25.5").unwrap())
        );
        assert_eq!(c.delta_for(OTHER_ISSUER), None);
    }

    #[test]
    fn v2_layout_with_tx_json_is_reduced_identically() {
        let v1 = reduce(&raw_tx(|_| {}));
        let raw_v2 = raw_tx(|r| {
            let flat = r.clone();
            let mut txj = json!({});
            for k in [
                "Account",
                "Destination",
                "DestinationTag",
                "TransactionType",
                "Flags",
                "Amount",
                "Fee",
                "Sequence",
            ] {
                txj[k] = flat[k].clone();
            }
            *r = json!({
                "tx_json": txj,
                "meta": flat["meta"],
                "hash": TXH,
                "ledger_index": 9000050,
                "ledger_hash": LH,
                "validated": true,
            });
        });
        assert_eq!(reduce(&raw_v2), v1);
    }

    #[test]
    fn non_success_unvalidated_or_partial_not_credited() {
        // Not yet validated.
        let raw = raw_tx(|r| r["validated"] = json!(false));
        assert_eq!(lookup(&raw).unwrap(), ReceiptLookup::NotValidated);
        // Validated but failed (tec).
        let raw = raw_tx(|r| r["meta"]["TransactionResult"] = json!("tecPATH_DRY"));
        assert_eq!(
            lookup(&raw).unwrap(),
            ReceiptLookup::Failed("tecPATH_DRY".into())
        );
        // Unknown tx.
        let raw = serde_json::to_vec(&json!({"result": {"error": "txnNotFound"}})).unwrap();
        assert_eq!(lookup(&raw).unwrap(), ReceiptLookup::NotFound);
        // Partial payment flag, nominal > delivered: held, and the credited value is
        // the delivered amount, never Amount.
        let raw = raw_tx(|r| {
            r["Flags"] = json!(TF_PARTIAL_PAYMENT);
            r["Amount"] = iou("100");
            r["meta"]["delivered_amount"] = iou("1");
        });
        let rc = verified(&raw);
        assert_eq!(
            rc.credit_eligibility(),
            CreditEligibility::Held(HeldReason::PartialPaymentFlag)
        );
        assert_eq!(rc.delivered_amount, ExactIssuedAmount::parse("1").unwrap());
        assert_eq!(
            rc.nominal_amount,
            Some(ExactIssuedAmount::parse("100").unwrap())
        );
        // No partial flag but delivered != Amount (anomalous): held.
        let raw = raw_tx(|r| r["meta"]["delivered_amount"] = iou("24"));
        assert_eq!(
            verified(&raw).credit_eligibility(),
            CreditEligibility::Held(HeldReason::DeliveredDiffersFromAmount)
        );
        // Paths or SendMax present: held.
        let raw = raw_tx(|r| r["SendMax"] = iou("30"));
        assert_eq!(
            verified(&raw).credit_eligibility(),
            CreditEligibility::Held(HeldReason::PathsOrSendMax)
        );
        let raw = raw_tx(|r| r["Paths"] = json!([[{"currency": "USD"}]]));
        assert_eq!(
            verified(&raw).credit_eligibility(),
            CreditEligibility::Held(HeldReason::PathsOrSendMax)
        );
        // delivered_amount "unavailable" / absent / native: never creditable.
        for da in [json!("unavailable"), json!("1000000"), Value::Null] {
            let raw = raw_tx(|r| r["meta"]["delivered_amount"] = da.clone());
            assert_eq!(
                lookup(&raw),
                Err(ReceiptError::DeliveredAmountUnavailable),
                "{da}"
            );
        }
        // Wrong issuer in the delivered amount (same symbol, different issuer).
        let raw = raw_tx(|r| {
            r["meta"]["delivered_amount"] =
                json!({"currency": CUR, "issuer": OTHER_ISSUER, "value": "25.5"})
        });
        assert!(matches!(lookup(&raw), Err(ReceiptError::WrongAsset(_))));
        // Not a Payment.
        let raw = raw_tx(|r| r["TransactionType"] = json!("TrustSet"));
        assert_eq!(
            lookup(&raw),
            Err(ReceiptError::NotPayment("TrustSet".into()))
        );
        // Missing ledger hash cannot be bound to a validated ledger.
        let raw = raw_tx(|r| {
            r.as_object_mut().unwrap().remove("ledger_hash");
        });
        assert_eq!(lookup(&raw), Err(ReceiptError::MissingField("ledger_hash")));
    }

    #[test]
    fn receipt_hash_mismatch_rejected() {
        let other = "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF";
        let raw = raw_tx(|r| r["hash"] = json!(other));
        assert!(matches!(
            lookup(&raw),
            Err(ReceiptError::HashMismatch { .. })
        ));
        // Case-insensitive on the SAME hash is fine.
        let raw = raw_tx(|r| r["hash"] = json!(TXH.to_lowercase()));
        assert!(matches!(lookup(&raw), Ok(ReceiptLookup::Verified(_))));
        // A single provider returning a different hash poisons the whole quorum.
        let good = reduce(&raw_tx(|_| {}));
        let bad = reduce(&raw_tx(|r| r["hash"] = json!(other)));
        let bodies = vec![
            ("p1".into(), good.clone()),
            ("p2".into(), bad),
            ("p3".into(), good),
        ];
        assert!(matches!(
            quorum_issued_receipt(&bodies, TXH, &asset()),
            Err(IssuedRpcError::Receipt(ReceiptError::HashMismatch { .. }))
        ));
        assert!(normalize_tx_hash("not-a-hash").is_err());
        assert!(normalize_tx_hash(&"g".repeat(64)).is_err());
        assert_eq!(normalize_tx_hash(&TXH.to_lowercase()).unwrap(), TXH);
    }

    #[test]
    fn receipt_quorum_requires_same_validated_ledger_hash() {
        let a = reduce(&raw_tx(|_| {}));
        let b = reduce(&raw_tx(|r| {
            r["ledger_hash"] =
                json!("00000000000000000000000000000000000000000000000000000000000000AA")
        }));
        // Two providers, two different ledger hashes for the same tx: no quorum.
        let split = vec![("p1".to_string(), a.clone()), ("p2".to_string(), b.clone())];
        assert!(matches!(
            quorum_issued_receipt(&split, TXH, &asset()),
            Err(IssuedRpcError::NoQuorum(_))
        ));
        // Two agree, one dissents: quorum with evidence from the agreeing pair only.
        let three = vec![
            ("p1".to_string(), a.clone()),
            ("p2".to_string(), b),
            ("p3".to_string(), a.clone()),
        ];
        match quorum_issued_receipt(&three, TXH, &asset()).unwrap() {
            ReceiptLookup::Verified(r) => {
                let names: Vec<_> = r
                    .provider_evidence
                    .iter()
                    .map(|e| e.provider.as_str())
                    .collect();
                assert_eq!(names, vec!["p1", "p3"]);
                assert!(r.provider_evidence.iter().all(|e| e.ledger_hash == LH));
            }
            other => panic!("{other:?}"),
        }
        // Too few responses.
        assert!(quorum_issued_receipt(&[("p1".to_string(), a)], TXH, &asset()).is_err());
        // Agreement that a tx is unknown is a quorum fact too.
        let nf = serde_json::to_vec(&json!({"result": {"error": "txnNotFound"}})).unwrap();
        let nf = reduce(&nf);
        let r = quorum_issued_receipt(
            &[("p1".into(), nf.clone()), ("p2".into(), nf)],
            TXH,
            &asset(),
        )
        .unwrap();
        assert_eq!(r, ReceiptLookup::NotFound);
    }

    #[test]
    fn transforms_pin_status_and_drop_volatile_fields() {
        let raw = raw_tx(|r| {
            r["date"] = json!(999);
            r["inLedger"] = json!(1);
        });
        let reduced = reduce(&raw);
        let v: Value = serde_json::from_slice(&reduced).unwrap();
        assert!(v.pointer("/tx/date").is_none());
        // Same fact, different volatile noise -> identical reduction.
        let raw2 = raw_tx(|r| {
            r["date"] = json!(111);
            r["close_time_iso"] = json!("x");
        });
        assert_eq!(reduce(&raw2), reduced);
        let resp = reduced_response(reduced);
        assert_eq!(resp.status, candid::Nat::from(200u16));
        assert!(resp.headers.is_empty());
        // Garbage in -> a fixed parse_error marker out.
        assert_eq!(
            reduce_issued_tx_body(b"<html>"),
            br#"{"parse_error":true}"#.to_vec()
        );
        assert_eq!(
            reduce_account_tx_body(b"x"),
            br#"{"parse_error":true}"#.to_vec()
        );
        assert_eq!(reduce_lines_body(b"x"), br#"{"parse_error":true}"#.to_vec());
        assert_eq!(
            reduce_server_info_body(b"x"),
            br#"{"parse_error":true}"#.to_vec()
        );
    }

    // ---- lines / trust lines ----

    fn raw_lines(
        account: &str,
        lines: Value,
        marker: Option<Value>,
        ledger_index: u32,
        hash: &str,
    ) -> Vec<u8> {
        let mut r = json!({"result": {
            "account": account, "lines": lines, "ledger_index": ledger_index,
            "ledger_hash": hash, "validated": true, "status": "success"
        }});
        if let Some(m) = marker {
            r["result"]["marker"] = m;
        }
        serde_json::to_vec(&r).unwrap()
    }

    fn line(peer: &str, currency: &str, balance: &str) -> Value {
        json!({"account": peer, "balance": balance, "currency": currency, "limit": "1000000",
               "limit_peer": "0", "no_ripple": true, "no_ripple_peer": false, "quality_in": 0, "quality_out": 0})
    }

    #[test]
    fn trust_line_found_absent_and_wrong_issuer() {
        let ok = reduce_lines_body(&raw_lines(
            HOLDER,
            json!([line(ISSUER, CUR, "12.5")]),
            None,
            100,
            LH,
        ));
        let page = parse_lines_page(&ok).unwrap();
        let tl = trust_line_from_page(&page, HOLDER, &asset())
            .unwrap()
            .unwrap();
        assert_eq!(tl.balance, ExactIssuedAmount::parse("12.5").unwrap());
        assert_eq!(tl.limit, ExactIssuedAmount::parse("1000000").unwrap());
        assert!(tl.no_ripple && !tl.no_ripple_peer && !tl.freeze);
        // A same-symbol line to a DIFFERENT issuer does not satisfy the proof.
        let evil = reduce_lines_body(&raw_lines(
            HOLDER,
            json!([line(OTHER_ISSUER, CUR, "12.5")]),
            None,
            100,
            LH,
        ));
        let page = parse_lines_page(&evil).unwrap();
        assert_eq!(trust_line_from_page(&page, HOLDER, &asset()).unwrap(), None);
        // Different currency, right issuer: no.
        let other_cur = reduce_lines_body(&raw_lines(
            HOLDER,
            json!([line(ISSUER, "USD", "5")]),
            None,
            100,
            LH,
        ));
        assert_eq!(
            trust_line_from_page(&parse_lines_page(&other_cur).unwrap(), HOLDER, &asset()).unwrap(),
            None
        );
        // Unfunded holder account.
        let nf = reduce_lines_body(br#"{"result":{"error":"actNotFound"}}"#);
        let page = parse_lines_page(&nf).unwrap();
        assert!(!page.account_found);
        assert_eq!(trust_line_from_page(&page, HOLDER, &asset()).unwrap(), None);
        // A marker on a peer-filtered query fails closed.
        let m = reduce_lines_body(&raw_lines(HOLDER, json!([]), Some(json!("MARK")), 100, LH));
        assert_eq!(
            trust_line_from_page(&parse_lines_page(&m).unwrap(), HOLDER, &asset()),
            Err(IssuedRpcError::TrustLinePagingUnsupported)
        );
        // Unvalidated ledger is refused outright.
        let mut v: Value =
            serde_json::from_slice(&raw_lines(HOLDER, json!([]), None, 100, LH)).unwrap();
        v["result"]["validated"] = json!(false);
        assert!(parse_lines_page(&reduce_lines_body(&serde_json::to_vec(&v).unwrap())).is_err());
    }

    #[test]
    fn trust_line_quorum_tolerates_ledger_skew_but_not_content_skew() {
        let at = |bal: &str, idx: u32, hash: &str| {
            reduce_lines_body(&raw_lines(
                HOLDER,
                json!([line(ISSUER, CUR, bal)]),
                None,
                idx,
                hash,
            ))
        };
        let h2 = "22223333444455556666777788889999000011112222333344445555AAAABBBB";
        let skew = vec![
            ("p1".to_string(), at("1", 100, LH)),
            ("p2".to_string(), at("1", 101, h2)),
        ];
        let proof = quorum_trust_line(&skew, HOLDER, &asset()).unwrap();
        assert_eq!(proof.ledger_index, 101);
        assert_eq!(proof.ledger_hash, h2);
        assert_eq!(proof.provider_evidence.len(), 2);
        let bad = vec![
            ("p1".to_string(), at("1", 100, LH)),
            ("p2".to_string(), at("2", 100, LH)),
        ];
        assert!(matches!(
            quorum_trust_line(&bad, HOLDER, &asset()),
            Err(IssuedRpcError::NoQuorum(_))
        ));
    }

    #[test]
    fn issuer_obligation_snapshot_sums_exactly_and_resists_dust() {
        let mut total = ObligationTotal::default();
        let page = |lines: Value| {
            parse_lines_page(&reduce_lines_body(&raw_lines(ISSUER, lines, None, 200, LH))).unwrap()
        };
        let p1 = page(json!([
            line(HOLDER, CUR, "-100.5"),
            line("rUn84CUYbNjRoTQ6mSW7BVJPSVJNLb1QLo", CUR, "-0.00000001"),
            line("rUn84CUYbNjRoTQ6mSW7BVJPSVJNLb1QLo", "USD", "-999"),
        ]));
        accumulate_obligation_page(&mut total, &p1, &asset()).unwrap();
        assert_eq!(total.whole_e8s, 10_050_000_000 + 1);
        assert_eq!(total.ceil_e8s(), 10_050_000_001);
        // Dust finer than e8s and a huge balance in the same sum must not fail.
        let p2 = page(json!([
            line(HOLDER, CUR, "-0.0000000000000001"),
            line(HOLDER, CUR, "-1e-50"),
            line(HOLDER, CUR, "-1000000"),
            line(HOLDER, CUR, "7"), // anomalous: issuer is owed
        ]));
        accumulate_obligation_page(&mut total, &p2, &asset()).unwrap();
        assert_eq!(total.anomalous_positive_lines, 1);
        assert_eq!(total.finer_dust_lines, 1);
        assert_eq!(total.whole_e8s, 10_050_000_001 + 100_000_000_000_000);
        // Dust is accounted and the conservative total rounds UP.
        assert_eq!(total.ceil_e8s(), 10_050_000_001 + 100_000_000_000_000 + 1);
        // Exactly-e8s totals do not gain a phantom unit.
        let mut t2 = ObligationTotal::default();
        accumulate_obligation_page(&mut t2, &page(json!([line(HOLDER, CUR, "-3")])), &asset())
            .unwrap();
        assert_eq!(t2.ceil_e8s(), 300_000_000);
        // Many sub-e8s dust lines that sum past one e8s are carried exactly.
        let mut t3 = ObligationTotal::default();
        let half = ExactIssuedAmount::parse("-0.000000005").unwrap();
        t3.add_holder_balance(&half).unwrap();
        t3.add_holder_balance(&half).unwrap();
        assert_eq!(t3.whole_e8s, 0);
        assert_eq!(t3.ceil_e8s(), 1);
    }

    #[test]
    fn pinned_page_quorum_requires_identical_page_at_pinned_hash() {
        let good = reduce_lines_body(&raw_lines(
            ISSUER,
            json!([line(HOLDER, CUR, "-5")]),
            None,
            200,
            LH,
        ));
        let bodies = vec![
            ("p1".to_string(), good.clone()),
            ("p2".to_string(), good.clone()),
        ];
        let (page, ev) = quorum_pinned_lines_page(&bodies, LH).unwrap();
        assert_eq!(page.lines.len(), 1);
        assert_eq!(ev.len(), 2);
        // A provider on a different ledger hash than the pin is a mismatch.
        let h2 = "22223333444455556666777788889999000011112222333344445555AAAABBBB";
        let other = reduce_lines_body(&raw_lines(
            ISSUER,
            json!([line(HOLDER, CUR, "-5")]),
            None,
            200,
            h2,
        ));
        let mixed = vec![("p1".to_string(), good.clone()), ("p2".to_string(), other)];
        assert_eq!(
            quorum_pinned_lines_page(&mixed, LH),
            Err(IssuedRpcError::SnapshotLedgerMismatch)
        );
        // Same hash but diverging content: no quorum.
        let diff = reduce_lines_body(&raw_lines(
            ISSUER,
            json!([line(HOLDER, CUR, "-6")]),
            None,
            200,
            LH,
        ));
        let split = vec![("p1".to_string(), good), ("p2".to_string(), diff)];
        assert!(matches!(
            quorum_pinned_lines_page(&split, LH),
            Err(IssuedRpcError::NoQuorum(_))
        ));
    }

    // ---- history ----

    fn raw_account_tx(
        min: u32,
        max: u32,
        marker: Option<Value>,
        entries: Vec<(u32, &str)>,
    ) -> Vec<u8> {
        let txs: Vec<Value> = entries
            .into_iter()
            .map(|(ledger, hash)| {
                json!({
                    "meta": {"TransactionResult": "tesSUCCESS", "delivered_amount": iou("1")},
                    "tx": {"Account": HOLDER, "Destination": ISSUER, "TransactionType": "Payment",
                           "Amount": iou("1"), "Flags": 0, "hash": hash, "ledger_index": ledger},
                    "validated": true
                })
            })
            .collect();
        let mut r = json!({"result": {
            "account": ISSUER, "ledger_index_min": min, "ledger_index_max": max,
            "transactions": txs, "validated": true, "status": "success"
        }});
        if let Some(m) = marker {
            r["result"]["marker"] = m;
        }
        serde_json::to_vec(&r).unwrap()
    }

    #[test]
    fn history_page_mid_ledger_preserves_cursor() {
        let h = |n: u8| format!("{:064X}", n);
        let cursor = IssuerHistoryCursor::new(100, 200).unwrap();
        // Page ends mid-ledger 150: rippled returns a marker for the rest of it.
        let body = reduce_account_tx_body(&raw_account_tx(
            100,
            200,
            Some(json!({"ledger": 150, "seq": 3})),
            vec![(120, &h(1)), (150, &h(2))],
        ));
        let page = parse_account_tx_page(&body, 100, 200).unwrap();
        assert_eq!(page.txs.len(), 2);
        assert!(!page.interval_complete);
        assert_eq!(
            page.next_marker.as_deref(),
            Some(r#"{"ledger":150,"seq":3}"#)
        );
        // Ledger 150 is only partly read: fully consumed only through 149.
        assert_eq!(page.fully_consumed_through, Some(149));
        match cursor.advance(&page) {
            HistoryProgress::Continue(c) => {
                assert_eq!(
                    (c.ledger_min, c.ledger_max),
                    (100, 200),
                    "interval bounds never move mid-interval"
                );
                assert_eq!(c.marker.as_deref(), Some(r#"{"ledger":150,"seq":3}"#));
            }
            other => panic!("cursor advanced past an unfinished interval: {other:?}"),
        }
        // The final page (no marker) is the only thing that completes the interval.
        let last = reduce_account_tx_body(&raw_account_tx(
            100,
            200,
            None,
            vec![(150, &h(3)), (199, &h(4))],
        ));
        let last = parse_account_tx_page(&last, 100, 200).unwrap();
        assert!(last.interval_complete);
        assert_eq!(last.fully_consumed_through, Some(200));
        let resumed = IssuerHistoryCursor {
            marker: page.next_marker.clone(),
            ..cursor.clone()
        };
        assert_eq!(
            resumed.advance(&last),
            HistoryProgress::IntervalComplete {
                next_ledger_min: 201
            }
        );
        // An empty interval with no marker is complete too (no phantom transactions).
        let empty = parse_account_tx_page(
            &reduce_account_tx_body(&raw_account_tx(100, 200, None, vec![])),
            100,
            200,
        )
        .unwrap();
        assert!(empty.interval_complete && empty.txs.is_empty());
        // The server must honour our fixed bounds, and entries must lie inside them.
        let shifted = reduce_account_tx_body(&raw_account_tx(100, 250, None, vec![]));
        assert!(parse_account_tx_page(&shifted, 100, 200).is_err());
        let outside = reduce_account_tx_body(&raw_account_tx(100, 200, None, vec![(300, &h(5))]));
        assert!(parse_account_tx_page(&outside, 100, 200).is_err());
        // Unvalidated entries are refused.
        let mut v: Value =
            serde_json::from_slice(&raw_account_tx(100, 200, None, vec![(150, &h(6))])).unwrap();
        v["result"]["transactions"][0]["validated"] = json!(false);
        let unval = reduce_account_tx_body(&serde_json::to_vec(&v).unwrap());
        assert!(parse_account_tx_page(&unval, 100, 200).is_err());
        assert!(IssuerHistoryCursor::new(0, 5).is_err());
        assert!(IssuerHistoryCursor::new(9, 5).is_err());
    }

    #[test]
    fn history_quorum_needs_identical_pages() {
        let h = format!("{:064X}", 9);
        let a = reduce_account_tx_body(&raw_account_tx(10, 20, None, vec![(12, &h)]));
        let b = reduce_account_tx_body(&raw_account_tx(10, 20, None, vec![]));
        let ok = quorum_history_page(
            &[("p1".to_string(), a.clone()), ("p2".to_string(), a.clone())],
            10,
            20,
        )
        .unwrap();
        assert_eq!(ok.txs.len(), 1);
        assert_eq!(ok.provider_evidence.len(), 2);
        assert!(matches!(
            quorum_history_page(&[("p1".to_string(), a), ("p2".to_string(), b)], 10, 20),
            Err(IssuedRpcError::NoQuorum(_))
        ));
    }

    fn info(complete: &str, seq: u32) -> Result<ServerInfo, IssuedRpcError> {
        parse_server_info(&reduce_server_info_body(
            &serde_json::to_vec(&json!({"result": {"info": {
                "complete_ledgers": complete, "validated_ledger": {"seq": seq, "hash": LH}
            }}}))
            .unwrap(),
        ))
    }

    #[test]
    fn complete_ledgers_must_cover_interval_else_typed_history_gap() {
        assert!(ledger_range_covers("32570-95000", 40_000, 50_000));
        assert!(!ledger_range_covers("32570-95000", 100, 200));
        assert!(ledger_range_covers("1-50,100-300", 100, 200));
        assert!(
            !ledger_range_covers("1-150,160-300", 100, 200),
            "a hole inside the interval is a gap"
        );
        assert!(!ledger_range_covers("empty", 100, 200));
        assert!(!ledger_range_covers("", 100, 200));
        assert!(!ledger_range_covers("abc-def", 100, 200));
        assert!(ledger_range_covers("100", 100, 100));

        let infos = vec![
            ("p1".to_string(), info("1-1000", 1000)),
            ("p2".to_string(), info("1-1000", 1000)),
        ];
        assert_eq!(
            history_providers_covering(&infos, 100, 200).unwrap(),
            vec!["p1", "p2"]
        );
        // One provider lacks the range: only one covering -> recoverable gap.
        let partial = vec![
            ("p1".to_string(), info("1-1000", 1000)),
            ("p2".to_string(), info("500-1000", 1000)),
        ];
        let e = history_providers_covering(&partial, 100, 200).unwrap_err();
        assert!(matches!(
            e,
            IssuedRpcError::HistoryGap {
                requested_min: 100,
                requested_max: 200,
                ..
            }
        ));
        assert!(e.is_recoverable());
        // Tip below the interval end (not yet validated there) is a gap too.
        let behind = vec![
            ("p1".to_string(), info("1-150", 150)),
            ("p2".to_string(), info("1-150", 150)),
        ];
        assert!(matches!(
            history_providers_covering(&behind, 100, 200),
            Err(IssuedRpcError::HistoryGap { .. })
        ));
        // A failed provider counts as not covering.
        let down = vec![
            ("p1".to_string(), info("1-1000", 1000)),
            (
                "p2".to_string(),
                Err(IssuedRpcError::Transport("down".into())),
            ),
        ];
        assert!(history_providers_covering(&down, 100, 200).is_err());
        // Snapshot pin: lowest tip among responders.
        let skew = vec![
            ("p1".to_string(), info("1-1000", 1000)),
            ("p2".to_string(), info("1-998", 998)),
        ];
        assert_eq!(pin_snapshot_ledger(&skew).unwrap().0, 998);
        assert!(pin_snapshot_ledger(&down).is_err());
    }

    #[test]
    fn requests_are_bounded_and_use_fixed_bounds() {
        let r = account_tx_request_to_url(
            "https://x/",
            ISSUER,
            10,
            20,
            Some(&json!({"ledger": 12, "seq": 1})),
            50,
        );
        let p = &r.params;
        assert_eq!(r.method, "account_tx");
        assert_eq!(r.transform, TRANSFORM_ACCOUNT_TX);
        assert_eq!(p["ledger_index_min"], 10);
        assert_eq!(p["ledger_index_max"], 20);
        assert_eq!(p["forward"], true);
        assert_eq!(p["limit"], 50);
        assert_eq!(p["marker"], json!({"ledger": 12, "seq": 1}));
        assert_eq!(r.max_response_bytes, MAX_HISTORY_BYTES);
        assert!(
            history_max_response_bytes(1000) <= MAX_HISTORY_BYTES,
            "limit is clamped"
        );
        let l = account_lines_request_to_url(
            "https://x/",
            HOLDER,
            Some(ISSUER),
            LedgerSpec::Index(77),
            None,
            LINES_PAGE_LIMIT,
        );
        assert_eq!(l.params["ledger_index"], 77);
        assert_eq!(l.params["peer"], ISSUER);
        assert_eq!(l.max_response_bytes, MAX_LINES_BYTES);
        let t = issued_tx_request_to_url("https://x/", TXH);
        assert_eq!(t.max_response_bytes, MAX_ISSUED_TX_BYTES);
        assert_eq!(t.params["transaction"], TXH);
        assert_eq!(t.params["binary"], false);
        assert_eq!(
            server_info_request_to_url("https://x/").method,
            "server_info"
        );
        // Cycles attached cover the largest reply (IC: ~800 cycles/byte/replica).
        assert!(HISTORY_CYCLES > MAX_HISTORY_BYTES as u128 * 800 * 13);
        assert!(LINES_CYCLES > MAX_LINES_BYTES as u128 * 800 * 13);
        assert!(TX_CYCLES > MAX_ISSUED_TX_BYTES as u128 * 800 * 13);
        assert!(check_account("rNotAnAddress").is_err());
    }

    #[test]
    fn network_is_explicit_and_never_inferred_from_key() {
        // The token-lane URL set is selected only by the explicit network argument.
        let t = read_provider_urls_for(XrplNetwork::Testnet).unwrap();
        assert!(t.len() >= READ_QUORUM && t.iter().all(|u| !u.contains("xrplcluster")));
        #[cfg(not(feature = "xrpl-testnet-fixture"))]
        {
            let m = read_provider_urls_for(XrplNetwork::Mainnet).unwrap();
            assert!(m.len() >= READ_QUORUM);
        }
        #[cfg(feature = "xrpl-testnet-fixture")]
        assert_eq!(
            read_provider_urls_for(XrplNetwork::Mainnet),
            Err(NetworkError::FixtureCannotSelectMainnet)
        );
    }
}
