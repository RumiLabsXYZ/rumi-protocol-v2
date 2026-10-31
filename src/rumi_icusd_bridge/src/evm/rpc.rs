//! Owned by card BB-05. EVM RPC canister transport with provider quorum.
//!
//! Every read goes to each configured provider in turn (sequentially, through the EVM
//! RPC canister's raw `request` method). Each provider's answer is parsed, strictly,
//! into the typed value the caller needs, and a value wins only when at least
//! `min_quorum` providers returned an IDENTICAL parsed value. Voting is on parsed
//! values, never on raw bodies: harmless extra fields cannot break agreement, and a
//! difference in any field we use cannot be masked. A provider whose answer does not
//! parse is simply a provider that did not answer.
//!
//! Settlement finality is chosen by the head rule of the configured chain's row in the
//! chain table (`chains.rs`). For Base that is `finalized` (L1 batch finality):
//! `tag_finalized_quorum` takes the lowest height at least `min_quorum` providers have
//! reached and then requires a quorum on the block hash at exactly that height.
//! `finalized_head` is the dispatcher over the rules. No rule ever reads `latest`.
//!
//! Nothing here decides what a value means. Callers (the observer, the lanes, the
//! flows) decide; this module only makes sure a decision is never taken on a value that
//! fewer than `min_quorum` independent providers agree on.

use crate::chains::{self, HeadRule};
use crate::evm::abi::RawLog;
use crate::ids::hex0x;
use crate::state;
use crate::types::{EthMainnetService, EthSepoliaService, L2MainnetService, RpcServiceConfig};
use candid::{CandidType, Deserialize, Nat, Principal};
use serde_json::{json, Map, Value};

// ---------------------------------------------------------------------------
// Public result types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RpcError {
    /// Fewer than `min_quorum` providers agreed.
    NoQuorum(String),
    /// Every provider failed.
    AllFailed(String),
    Decode(String),
    NotConfigured(String),
}

impl std::fmt::Display for RpcError {
    /// Plain text for status fields and error strings; the messages never contain
    /// provider URLs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RpcError::NoQuorum(m) | RpcError::AllFailed(m) => write!(f, "{m}"),
            RpcError::Decode(m) => write!(f, "RPC decode error: {m}"),
            RpcError::NotConfigured(m) => write!(f, "RPC not configured: {m}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockTag {
    Latest,
    Pending,
    Safe,
    Finalized,
    Number(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockRef {
    pub number: u64,
    pub hash: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub tx_hash: [u8; 32],
    pub success: bool,
    pub block_number: u64,
    pub block_hash: [u8; 32],
    pub from: [u8; 20],
    pub to: Option<[u8; 20]>,
    pub logs: Vec<RawLog>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeeEstimate {
    pub base_fee_per_gas: u128,
    pub priority_fee_per_gas: u128,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SimulationResult {
    Ok(Vec<u8>),
    Reverted(Vec<u8>),
    /// The call ran out of gas at the gas limit it was simulated with (or the limit is
    /// below the intrinsic cost). A transaction sent with that limit would fail on chain.
    OutOfGas,
}

// ---------------------------------------------------------------------------
// Candid mirror of the EVM RPC canister (`request` method only)
// ---------------------------------------------------------------------------

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct HttpHeader {
    pub value: String,
    pub name: String,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct RpcApi {
    pub url: String,
    pub headers: Option<Vec<HttpHeader>>,
}

/// `EthSepoliaService`, `EthMainnetService` and `L2MainnetService` are defined in
/// `types.rs`, where `RpcServiceConfig` names them, and used here as the mirror's service
/// enums.
///
/// `BaseMainnet`, `EthMainnet`, `EthSepolia`, `ArbitrumOne` and `Custom` are the arms
/// `to_rpc_service` sends; the rest mirror the live interface so the type stays a faithful
/// copy of it.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum RpcService {
    EthSepolia(EthSepoliaService),
    EthMainnet(EthMainnetService),
    ArbitrumOne(L2MainnetService),
    BaseMainnet(L2MainnetService),
    OptimismMainnet(L2MainnetService),
    Provider(u64),
    Custom(RpcApi),
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TooFewCyclesRecord {
    pub expected: Nat,
    pub received: Nat,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ProviderError {
    TooFewCycles(TooFewCyclesRecord),
    MissingRequiredProvider,
    ProviderNotFound,
    NoPermission,
    InvalidRpcConfig(String),
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum RejectionCode {
    NoError,
    CanisterError,
    SysTransient,
    DestinationInvalid,
    Unknown,
    SysFatal,
    CanisterReject,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct IcErrorRecord {
    pub code: RejectionCode,
    pub message: String,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct InvalidHttpJsonRpcRecord {
    pub status: u16,
    pub body: String,
    #[serde(rename = "parsingError")]
    pub parsing_error: Option<String>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum HttpOutcallError {
    IcError(IcErrorRecord),
    InvalidHttpJsonRpcResponse(InvalidHttpJsonRpcRecord),
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ValidationError {
    Custom(String),
    InvalidHex(String),
}

/// The EVM RPC canister's own error type. Named apart from this module's [`RpcError`],
/// which is what callers see. Error details are only ever rendered into text.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum EvmRpcError {
    ProviderError(ProviderError),
    HttpOutcallError(HttpOutcallError),
    JsonRpcError(JsonRpcError),
    ValidationError(ValidationError),
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum RequestResult {
    Ok(String),
    Err(EvmRpcError),
}

/// Map a configured provider to the EVM RPC canister's `RpcService`. `Custom` carries no
/// headers: secrets would be visible in canister state.
pub fn to_rpc_service(cfg: &RpcServiceConfig) -> RpcService {
    match cfg {
        RpcServiceConfig::BaseMainnet(p) => RpcService::BaseMainnet(*p),
        RpcServiceConfig::EthMainnet(p) => RpcService::EthMainnet(*p),
        RpcServiceConfig::EthSepolia(p) => RpcService::EthSepolia(*p),
        RpcServiceConfig::ArbitrumOne(p) => RpcService::ArbitrumOne(*p),
        RpcServiceConfig::Custom { url } => RpcService::Custom(RpcApi { url: url.clone(), headers: None }),
    }
}

// ---------------------------------------------------------------------------
// Configuration snapshot
// ---------------------------------------------------------------------------

/// Largest response cap the EVM RPC canister accepts for one `request`.
pub const MAX_RESPONSE_BYTES: u64 = 2_000_000;
/// Largest multiple of `cycles_per_rpc_call` attached to one call. The EVM RPC canister
/// charges in proportion to the response cap and refunds the unused part of the
/// attached cycles. The multiple grows with the cap (see [`Settings::cycles_for`]) and
/// stops here; at the largest cap (2 MB) the HTTPS outcall alone costs roughly 55 billion
/// cycles on a 34-node subnet (about 27 200 cycles per cap byte), which 48 x the default
/// 2 billion covers with margin. Raising a cap therefore never starves the call.
const MAX_CYCLE_FACTOR: u64 = 48;
/// Maximum characters of any provider-supplied text kept in an error message.
const MAX_ERR_CHARS: usize = 120;

#[derive(Clone, Debug)]
struct Settings {
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    evm_rpc: Principal,
    services: Vec<RpcServiceConfig>,
    quorum: usize,
    cycles: u128,
    default_max: u64,
    logs_max: u64,
}

impl Settings {
    /// Cap for block reads. A block response lists every transaction hash, which on
    /// Base is far larger than a view call, so it never uses less than the logs cap.
    fn block_cap(&self) -> u64 {
        self.default_max.max(self.logs_max)
    }

    /// Cycles to attach for a call with this response cap: the base amount covers
    /// `default_max_response_bytes`; larger caps scale it up (excess is refunded).
    fn cycles_for(&self, max_bytes: u64) -> u128 {
        let unit = self.default_max.max(1);
        let factor = max_bytes.div_ceil(unit).clamp(1, MAX_CYCLE_FACTOR);
        self.cycles.saturating_mul(factor as u128)
    }

    /// The providers and quorum must describe a usable set.
    fn validated(self) -> Result<Settings, RpcError> {
        if self.services.is_empty() {
            return Err(RpcError::NotConfigured("no RPC services configured".into()));
        }
        if self.quorum == 0 || self.quorum > self.services.len() {
            return Err(RpcError::NotConfigured(format!(
                "min_quorum {} is not in 1..={}",
                self.quorum,
                self.services.len()
            )));
        }
        Ok(self)
    }
}

/// The configured settings, unchecked.
fn base_settings() -> Settings {
    state::read_core(|c| Settings {
        evm_rpc: c.config.evm_rpc,
        services: c.config.rpc_services.clone(),
        quorum: c.config.min_quorum as usize,
        cycles: c.config.observer.cycles_per_rpc_call,
        default_max: c.config.observer.default_max_response_bytes,
        logs_max: c.config.observer.logs_max_response_bytes,
    })
}

fn settings() -> Result<Settings, RpcError> {
    base_settings().validated()
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
async fn transport(s: &Settings, idx: usize, body: &str, max_bytes: u64) -> Result<String, String> {
    let service = to_rpc_service(&s.services[idx]);
    let cycles = s.cycles_for(max_bytes);
    let res: Result<(RequestResult,), (ic_cdk::api::call::RejectionCode, String)> =
        ic_cdk::api::call::call_with_payment128(s.evm_rpc, "request", (service, body.to_string(), max_bytes), cycles).await;
    match res {
        Ok((RequestResult::Ok(text),)) => Ok(text),
        // A JSON-RPC level error may surface as a structured error. Re-wrap it as the
        // JSON-RPC body it came from so revert detection works either way.
        Ok((RequestResult::Err(EvmRpcError::JsonRpcError(e)),)) => {
            Ok(json!({"jsonrpc": "2.0", "id": 1, "error": {"code": e.code, "message": e.message}}).to_string())
        }
        Ok((RequestResult::Err(e),)) => Err(redact_urls(&trunc(&format!("EVM RPC canister error: {e:?}"), MAX_ERR_CHARS))),
        Err((code, msg)) => Err(redact_urls(&trunc(&format!("EVM RPC canister call failed ({code:?}): {msg}"), MAX_ERR_CHARS))),
    }
}

#[cfg(not(target_arch = "wasm32"))]
type MockFn = Box<dyn Fn(&RpcServiceConfig, &str) -> Result<String, String>>;

/// One transport call as seen by the host mock.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MockCall {
    pub service: RpcServiceConfig,
    pub method: String,
    pub max_response_bytes: u64,
    pub cycles: u128,
}

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    static MOCK: std::cell::RefCell<Option<MockFn>> = const { std::cell::RefCell::new(None) };
    static MOCK_CALLS: std::cell::RefCell<Vec<MockCall>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Host only: install the transport used instead of the EVM RPC canister. The function
/// receives the provider and the JSON-RPC request text and returns the response text
/// (or a transport error). Also clears the recorded call log.
#[cfg(not(target_arch = "wasm32"))]
pub fn set_mock_transport<F>(f: F)
where
    F: Fn(&RpcServiceConfig, &str) -> Result<String, String> + 'static,
{
    MOCK.with(|m| *m.borrow_mut() = Some(Box::new(f)));
    clear_mock_calls();
}

/// Host only: remove the mock transport (calls then fail as "not configured").
#[cfg(not(target_arch = "wasm32"))]
pub fn clear_mock_transport() {
    MOCK.with(|m| *m.borrow_mut() = None);
    clear_mock_calls();
}

/// Host only: every call made through the mock transport since the last clear.
#[cfg(not(target_arch = "wasm32"))]
pub fn mock_calls() -> Vec<MockCall> {
    MOCK_CALLS.with(|c| c.borrow().clone())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn clear_mock_calls() {
    MOCK_CALLS.with(|c| c.borrow_mut().clear());
}

#[cfg(not(target_arch = "wasm32"))]
async fn transport(s: &Settings, idx: usize, body: &str, max_bytes: u64) -> Result<String, String> {
    let service = s.services[idx].clone();
    let method = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("method").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default();
    MOCK_CALLS.with(|c| {
        c.borrow_mut().push(MockCall { service: service.clone(), method, max_response_bytes: max_bytes, cycles: s.cycles_for(max_bytes) })
    });
    MOCK.with(|m| match m.borrow().as_ref() {
        Some(f) => f(&service, body),
        None => Err("no mock transport installed".to_string()),
    })
}

// ---------------------------------------------------------------------------
// Text helpers
// ---------------------------------------------------------------------------

/// At most `max` characters, never splitting a character.
fn trunc(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}...", &s[..i]),
        None => s.to_string(),
    }
}

/// Replace anything that looks like a URL. A custom provider URL may embed a key, and
/// error text can echo it.
pub fn redact_urls(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        let next = ["https://", "http://"].iter().filter_map(|p| rest.find(p)).min();
        match next {
            None => {
                out.push_str(rest);
                return out;
            }
            Some(i) => {
                out.push_str(&rest[..i]);
                let tail = &rest[i..];
                let end = tail.find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ')' | '>' | ',' | '}')).unwrap_or(tail.len());
                out.push_str("<url>");
                rest = &tail[end..];
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Strict JSON-RPC parsing
// ---------------------------------------------------------------------------

/// One provider's answer to one request, after the transport and the JSON-RPC envelope.
#[derive(Clone, Debug)]
enum Reply {
    Ok(Value),
    JsonRpc { code: i64, message: String, data: Option<Value> },
    Failed(String),
}

fn parse_envelope(body: &str) -> Reply {
    let v: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return Reply::Failed(format!("response is not JSON: {}", trunc(&e.to_string(), 60))),
    };
    let Some(obj) = v.as_object() else {
        return Reply::Failed("response is not a JSON object".into());
    };
    // A well-formed response carries exactly one of `result` (possibly null) and a
    // non-null `error`.
    let error = obj.get("error").filter(|e| !e.is_null());
    match (obj.get("result"), error) {
        (Some(r), None) => Reply::Ok(r.clone()),
        (None, Some(e)) => {
            let code = e.get("code").and_then(Value::as_i64).unwrap_or(i64::MIN);
            let message = match e.get("message").and_then(Value::as_str) {
                Some(m) => m.to_string(),
                None => trunc(&e.to_string(), 80),
            };
            Reply::JsonRpc { code, message, data: e.get("data").cloned() }
        }
        _ => Reply::Failed("malformed JSON-RPC envelope".into()),
    }
}

/// The `result` of a reply, or why there is none.
fn ok_value(r: &Reply) -> Result<&Value, String> {
    match r {
        Reply::Ok(v) => Ok(v),
        Reply::JsonRpc { code, message, .. } => Err(format!("JSON-RPC error {code}: {}", trunc(&redact_urls(message), 80))),
        Reply::Failed(e) => Err(e.clone()),
    }
}

fn as_str<'a>(v: &'a Value, what: &str) -> Result<&'a str, String> {
    v.as_str().ok_or_else(|| format!("{what}: expected a hex string"))
}

/// A `0x` QUANTITY: at least one digit, no leading zeros except the single value `0x0`.
fn quantity_digits<'a>(v: &'a Value, what: &str) -> Result<&'a str, String> {
    let s = as_str(v, what)?;
    let digits = s.strip_prefix("0x").ok_or_else(|| format!("{what}: QUANTITY must start with 0x"))?;
    if digits.is_empty() {
        return Err(format!("{what}: empty QUANTITY"));
    }
    if digits.len() > 1 && digits.starts_with('0') {
        return Err(format!("{what}: QUANTITY has leading zeros"));
    }
    if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{what}: QUANTITY has a non-hex digit"));
    }
    Ok(digits)
}

pub(crate) fn parse_quantity_u64(v: &Value, what: &str) -> Result<u64, String> {
    let d = quantity_digits(v, what)?;
    if d.len() > 16 {
        return Err(format!("{what}: QUANTITY does not fit u64"));
    }
    u64::from_str_radix(d, 16).map_err(|e| format!("{what}: {e}"))
}

pub(crate) fn parse_quantity_u128(v: &Value, what: &str) -> Result<u128, String> {
    let d = quantity_digits(v, what)?;
    if d.len() > 32 {
        return Err(format!("{what}: QUANTITY does not fit u128"));
    }
    u128::from_str_radix(d, 16).map_err(|e| format!("{what}: {e}"))
}

/// `0x` DATA: even number of hex digits (possibly none).
pub(crate) fn parse_data(v: &Value, what: &str) -> Result<Vec<u8>, String> {
    let s = as_str(v, what)?;
    let digits = s.strip_prefix("0x").ok_or_else(|| format!("{what}: DATA must start with 0x"))?;
    if digits.len() & 1 == 1 {
        return Err(format!("{what}: DATA has an odd number of digits"));
    }
    hex::decode(digits).map_err(|e| format!("{what}: {e}"))
}

fn parse_fixed<const N: usize>(v: &Value, what: &str) -> Result<[u8; N], String> {
    let bytes = parse_data(v, what)?;
    <[u8; N]>::try_from(bytes.as_slice()).map_err(|_| format!("{what}: expected {N} bytes, got {}", bytes.len()))
}

fn field<'a>(o: &'a Map<String, Value>, name: &str) -> Result<&'a Value, String> {
    o.get(name).ok_or_else(|| format!("missing field {name}"))
}

/// Block header fields we use. `expect` pins the height the caller asked for.
fn parse_block(v: &Value, expect: Option<u64>) -> Result<BlockRef, String> {
    if v.is_null() {
        return Err("block not found".into());
    }
    let o = v.as_object().ok_or("block is not an object")?;
    let number = parse_quantity_u64(field(o, "number")?, "block number")?;
    let hash = parse_fixed::<32>(field(o, "hash")?, "block hash")?;
    if let Some(want) = expect {
        if number != want {
            return Err(format!("asked for block {want}, provider returned block {number}"));
        }
    }
    Ok(BlockRef { number, hash })
}

fn parse_log(v: &Value) -> Result<RawLog, String> {
    let o = v.as_object().ok_or("log is not an object")?;
    let topics = field(o, "topics")?
        .as_array()
        .ok_or("log topics is not an array")?
        .iter()
        .map(|t| parse_fixed::<32>(t, "log topic"))
        .collect::<Result<Vec<_>, _>>()?;
    if topics.len() > 4 {
        return Err(format!("log has {} topics", topics.len()));
    }
    let removed = match o.get("removed") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err("log removed flag is not a boolean".into()),
    };
    Ok(RawLog {
        address: parse_fixed::<20>(field(o, "address")?, "log address")?,
        topics,
        data: parse_data(field(o, "data")?, "log data")?,
        block_number: parse_quantity_u64(field(o, "blockNumber")?, "log blockNumber")?,
        block_hash: parse_fixed::<32>(field(o, "blockHash")?, "log blockHash")?,
        tx_hash: parse_fixed::<32>(field(o, "transactionHash")?, "log transactionHash")?,
        log_index: parse_quantity_u64(field(o, "logIndex")?, "log logIndex")?,
        removed,
    })
}

/// Parse an `eth_getLogs` result: sorted by `(block_number, log_index)`, no two logs at
/// the same position. Every log must satisfy the filter that was sent.
fn parse_logs(v: &Value, address: &[u8; 20], topic0: &[u8; 32], from: u64, to: u64) -> Result<Vec<RawLog>, String> {
    let arr = v.as_array().ok_or("eth_getLogs result is not an array")?;
    let mut logs = arr.iter().map(parse_log).collect::<Result<Vec<_>, _>>()?;
    for l in &logs {
        if &l.address != address {
            return Err("provider returned a log from a different address than filtered".into());
        }
        if l.topics.first() != Some(topic0) {
            return Err("provider returned a log with a different topic0 than filtered".into());
        }
        if l.block_number < from || l.block_number > to {
            return Err(format!("provider returned a log at block {} outside [{from}, {to}]", l.block_number));
        }
    }
    logs.sort_by_key(|l| (l.block_number, l.log_index));
    if logs.windows(2).any(|w| w[0].block_number == w[1].block_number && w[0].log_index == w[1].log_index) {
        return Err("provider returned two logs at the same position".into());
    }
    Ok(logs)
}

fn parse_receipt(v: &Value, want: &[u8; 32]) -> Result<Option<Receipt>, String> {
    if v.is_null() {
        return Ok(None);
    }
    let o = v.as_object().ok_or("receipt is not an object")?;
    let tx_hash = parse_fixed::<32>(field(o, "transactionHash")?, "receipt transactionHash")?;
    if &tx_hash != want {
        return Err("provider returned the receipt of a different transaction".into());
    }
    let success = match parse_quantity_u64(field(o, "status")?, "receipt status")? {
        1 => true,
        0 => false,
        other => return Err(format!("receipt status {other} is neither 0 nor 1")),
    };
    let block_number = parse_quantity_u64(field(o, "blockNumber")?, "receipt blockNumber")?;
    let block_hash = parse_fixed::<32>(field(o, "blockHash")?, "receipt blockHash")?;
    let from = parse_fixed::<20>(field(o, "from")?, "receipt from")?;
    let to = match o.get("to") {
        None | Some(Value::Null) => None,
        Some(t) => Some(parse_fixed::<20>(t, "receipt to")?),
    };
    let logs = field(o, "logs")?
        .as_array()
        .ok_or("receipt logs is not an array")?
        .iter()
        .map(parse_log)
        .collect::<Result<Vec<_>, _>>()?;
    for l in &logs {
        if l.block_number != block_number || l.block_hash != block_hash || l.tx_hash != tx_hash {
            return Err("receipt log does not belong to the receipt's block and transaction".into());
        }
    }
    Ok(Some(Receipt { tx_hash, success, block_number, block_hash, from, to, logs }))
}

fn quantity_param(n: u64) -> String {
    format!("0x{n:x}")
}

fn tag_param(tag: &BlockTag) -> String {
    match tag {
        BlockTag::Latest => "latest".into(),
        BlockTag::Pending => "pending".into(),
        BlockTag::Safe => "safe".into(),
        BlockTag::Finalized => "finalized".into(),
        BlockTag::Number(n) => quantity_param(*n),
    }
}

// ---------------------------------------------------------------------------
// Fan-out and quorum
// ---------------------------------------------------------------------------

async fn call_one(s: &Settings, idx: usize, method: &str, params: &Value, max_bytes: u64) -> Reply {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
    match transport(s, idx, &body, max_bytes).await {
        Ok(text) => parse_envelope(&text),
        Err(e) => Reply::Failed(redact_urls(&e)),
    }
}

/// Ask every provider in turn.
async fn call_all(s: &Settings, method: &str, params: &Value, max_bytes: u64) -> Vec<Reply> {
    let mut out = Vec::with_capacity(s.services.len());
    for idx in 0..s.services.len() {
        out.push(call_one(s, idx, method, params, max_bytes).await);
    }
    out
}

fn providers_label(idx: &[usize]) -> String {
    idx.iter().map(|i| format!("p{}", i + 1)).collect::<Vec<_>>().join(",")
}

fn errors_label(errors: &[(usize, String)]) -> String {
    errors.iter().map(|(i, e)| format!("p{} failed: {}", i + 1, trunc(e, 80))).collect::<Vec<_>>().join("; ")
}

/// Decide a vote over one parsed answer per provider (in provider order).
///
/// A value wins when at least `quorum` providers returned it. If two different values
/// both reach `quorum` (possible when `quorum <= total / 2`) nothing wins: the providers
/// are split. Providers are named by position only, never by URL.
fn tally<T: PartialEq + Clone + std::fmt::Debug>(parsed: Vec<Result<T, String>>, quorum: usize, what: &str) -> Result<T, RpcError> {
    let total = parsed.len();
    let mut groups: Vec<(T, Vec<usize>)> = Vec::new();
    let mut errors: Vec<(usize, String)> = Vec::new();
    for (i, p) in parsed.into_iter().enumerate() {
        match p {
            Ok(v) => match groups.iter_mut().find(|(g, _)| *g == v) {
                Some((_, idx)) => idx.push(i),
                None => groups.push((v, vec![i])),
            },
            Err(e) => errors.push((i, e)),
        }
    }
    if groups.is_empty() {
        return Err(RpcError::AllFailed(format!("{what}: all {total} providers failed: {}", errors_label(&errors))));
    }
    let winners: Vec<usize> = groups.iter().enumerate().filter(|(_, (_, idx))| idx.len() >= quorum).map(|(g, _)| g).collect();
    if winners.len() == 1 {
        return Ok(groups.swap_remove(winners[0]).0);
    }
    let mut parts: Vec<String> = groups
        .iter()
        .map(|(v, idx)| format!("[{}] answered {}", providers_label(idx), trunc(&format!("{v:?}"), 64)))
        .collect();
    if !errors.is_empty() {
        parts.push(errors_label(&errors));
    }
    let why = if winners.is_empty() { "no quorum" } else { "conflicting answers each reached quorum" };
    Err(RpcError::NoQuorum(format!("{what}: {why} (need {quorum} identical of {total}): {}", parts.join("; "))))
}

fn vote<T, F>(replies: &[Reply], quorum: usize, what: &str, parse: F) -> Result<T, RpcError>
where
    T: PartialEq + Clone + std::fmt::Debug,
    F: Fn(&Reply) -> Result<T, String>,
{
    tally(replies.iter().map(parse).collect(), quorum, what)
}

/// The height `finalized_head` settles on: the largest `n` such that at least `quorum`
/// providers report a finalized height `>= n` (the `quorum`-th highest report).
fn agreed_height(heads: &[Result<BlockRef, String>], quorum: usize) -> Result<u64, RpcError> {
    let mut numbers: Vec<u64> = heads.iter().filter_map(|h| h.as_ref().ok()).map(|b| b.number).collect();
    if numbers.is_empty() {
        let errors: Vec<(usize, String)> = heads.iter().enumerate().filter_map(|(i, h)| h.as_ref().err().map(|e| (i, e.clone()))).collect();
        return Err(RpcError::AllFailed(format!("finalized head: all {} providers failed: {}", heads.len(), errors_label(&errors))));
    }
    if numbers.len() < quorum {
        return Err(RpcError::NoQuorum(format!(
            "finalized head: only {} of {} providers answered, need {quorum}",
            numbers.len(),
            heads.len()
        )));
    }
    numbers.sort_unstable_by(|a, b| b.cmp(a));
    Ok(numbers[quorum - 1])
}

// ---------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------

/// The head rule of the configured chain, from its chain table row.
///
/// A chain id with no row has no rule. That cannot happen in a running canister (install
/// admits only enabled rows and `post_upgrade` refuses a stored id with no row); it fails
/// closed here rather than falling back to another chain's rule.
pub fn head_rule() -> Result<HeadRule, RpcError> {
    let chain = state::read_core(|c| c.config.chain_id);
    match chains::profile(chain) {
        Some(p) => Ok(p.head_rule),
        None => Err(RpcError::NotConfigured(format!("no bridge chain profile for chain id {chain}"))),
    }
}

/// The settlement head of the configured chain, as a quorum value. The rule comes from
/// the chain table; no rule ever sends `latest`.
///
/// - `TagFinalizedQuorum`: [`tag_finalized_quorum`].
/// - `MonadVerified { delay_blocks }`: the quorum-agreed finalized height `n` less
///   `delay_blocks`, with the hash of that block confirmed by the same quorum.
/// - `Blocked(reason)`: an error, before any provider is asked.
pub async fn finalized_head() -> Result<BlockRef, RpcError> {
    match head_rule()? {
        HeadRule::TagFinalizedQuorum => tag_finalized_quorum().await,
        HeadRule::MonadVerified { delay_blocks } => monad_verified_head(delay_blocks).await,
        HeadRule::Blocked(reason) => Err(blocked_error(reason)),
    }
}

fn blocked_error(reason: &str) -> RpcError {
    RpcError::NotConfigured(format!("the settlement head is blocked for this chain ({reason})"))
}

/// The block tag a view call must use to read at the settlement head without first
/// reading the head, when the rule allows it.
///
/// `TagFinalizedQuorum` reads at the `finalized` tag itself, which asks no provider here.
/// The other rules have no tag that means "the settlement head", so the head is read through
/// [`finalized_head`] and the call is pinned to its number. A blocked rule is an error.
pub async fn settlement_block_tag() -> Result<BlockTag, RpcError> {
    match head_rule()? {
        HeadRule::TagFinalizedQuorum => Ok(BlockTag::Finalized),
        HeadRule::MonadVerified { delay_blocks } => Ok(BlockTag::Number(monad_verified_head(delay_blocks).await?.number)),
        HeadRule::Blocked(reason) => Err(blocked_error(reason)),
    }
}

/// Monad's Verified head: the execution outputs of this block are agreed, which is what
/// receipts, logs and contract state depend on.
///
/// 1. `n` = the quorum-agreed `finalized` height, with its hash agreed by the same quorum
///    ([`tag_finalized_quorum`]). `finalized` fixes ordering only.
/// 2. The head is `n - delay_blocks`. A chain younger than the delay has no Verified block.
/// 3. The hash of that block is agreed by a quorum at exactly that number.
async fn monad_verified_head(delay_blocks: u64) -> Result<BlockRef, RpcError> {
    let finalized = tag_finalized_quorum().await?;
    let head = finalized.number.checked_sub(delay_blocks).ok_or_else(|| {
        RpcError::NoQuorum(format!("verified head: finalized height {} is below the execution delay of {delay_blocks} blocks", finalized.number))
    })?;
    block_by_number(head).await
}

/// The settlement head for a tag-based chain: `finalized`, as a quorum value.
///
/// 1. Ask every provider for its `finalized` block `(number, hash)`.
/// 2. `n` = the `min_quorum`-th highest reported number (the largest height at least
///    `min_quorum` providers have reached). One far-ahead outlier cannot raise it.
/// 3. Require a quorum on `(n, hash)`. A provider that reported exactly `n` has already
///    said what the block at `n` is, so that report is reused. Only providers that
///    reported a higher number are asked again for block `n` itself. Providers below
///    `n` do not vote.
async fn tag_finalized_quorum() -> Result<BlockRef, RpcError> {
    let s = settings()?;
    let cap = s.block_cap();
    let replies = call_all(&s, "eth_getBlockByNumber", &json!(["finalized", false]), cap).await;
    let heads: Vec<Result<BlockRef, String>> = replies.iter().map(|r| ok_value(r).and_then(|v| parse_block(v, None))).collect();
    let n = agreed_height(&heads, s.quorum)?;
    let mut votes: Vec<Result<BlockRef, String>> = Vec::with_capacity(heads.len());
    let mut ahead: Vec<usize> = Vec::new();
    for (i, h) in heads.iter().enumerate() {
        votes.push(match h {
            Ok(b) if b.number == n => Ok(*b),
            Ok(b) if b.number > n => {
                ahead.push(i);
                Err("pending refetch".to_string())
            }
            Ok(b) => Err(format!("finalized height {} is below the agreed height {n}", b.number)),
            Err(e) => Err(e.clone()),
        });
    }
    for i in ahead {
        let reply = call_one(&s, i, "eth_getBlockByNumber", &json!([quantity_param(n), false]), cap).await;
        votes[i] = ok_value(&reply).and_then(|v| parse_block(v, Some(n)));
    }
    tally(votes, s.quorum, &format!("finalized block {n}"))
}

/// Block `n` as `(number, hash)`, by quorum.
pub async fn block_by_number(n: u64) -> Result<BlockRef, RpcError> {
    let s = settings()?;
    let replies = call_all(&s, "eth_getBlockByNumber", &json!([quantity_param(n), false]), s.block_cap()).await;
    vote(&replies, s.quorum, &format!("block {n}"), |r| ok_value(r).and_then(|v| parse_block(v, Some(n))))
}

/// `eth_call`, by quorum on the returned bytes. A revert is an error here; use
/// [`simulate_call`] when a revert is an answer.
pub async fn eth_call(to: &[u8; 20], data: &[u8], block: BlockTag) -> Result<Vec<u8>, RpcError> {
    let s = settings()?;
    let params = json!([{"to": hex0x(to), "data": hex0x(data)}, tag_param(&block)]);
    let replies = call_all(&s, "eth_call", &params, s.default_max).await;
    vote(&replies, s.quorum, "eth_call", |r| ok_value(r).and_then(|v| parse_data(v, "eth_call result")))
}

/// `eth_call` pinned to a block hash (EIP-1898), by quorum on the returned bytes.
pub async fn eth_call_at_block_hash(to: &[u8; 20], data: &[u8], block_hash: &[u8; 32]) -> Result<Vec<u8>, RpcError> {
    let s = settings()?;
    let params = json!([{"to": hex0x(to), "data": hex0x(data)}, {"blockHash": hex0x(block_hash), "requireCanonical": true}]);
    let replies = call_all(&s, "eth_call", &params, s.default_max).await;
    vote(&replies, s.quorum, "eth_call at block hash", |r| ok_value(r).and_then(|v| parse_data(v, "eth_call result")))
}

/// Whether a JSON-RPC error is an EVM execution revert.
fn is_revert(code: i64, message: &str) -> bool {
    code == 3 || message.to_ascii_lowercase().contains("revert")
}

/// Out-of-gas wording across clients: geth "out of gas" and "gas required exceeds
/// allowance", anvil "EVM error OutOfGas", and "intrinsic gas too low/high" when the
/// limit is below the intrinsic cost.
fn is_out_of_gas(message: &str) -> bool {
    let m: String = message.to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    m.contains("outofgas") || m.contains("gasrequiredexceeds") || m.contains("intrinsicgas")
}

/// Simulate a call from `from` with exactly `gas` gas, the limit the real transaction
/// will carry. An execution revert is an answer ([`SimulationResult::Reverted`], with the
/// revert bytes when the provider supplies them) and so is running out of gas
/// ([`SimulationResult::OutOfGas`]); both are voted on like any other value.
pub async fn simulate_call(
    from: &[u8; 20],
    to: &[u8; 20],
    data: &[u8],
    gas: u64,
    block: BlockTag,
) -> Result<SimulationResult, RpcError> {
    let s = settings()?;
    let params = json!([
        {"from": hex0x(from), "to": hex0x(to), "data": hex0x(data), "gas": quantity_param(gas)},
        tag_param(&block)
    ]);
    let replies = call_all(&s, "eth_call", &params, s.default_max).await;
    vote(&replies, s.quorum, "simulate_call", |r| match r {
        Reply::Ok(v) => parse_data(v, "eth_call result").map(SimulationResult::Ok),
        Reply::JsonRpc { message, .. } if is_out_of_gas(message) => Ok(SimulationResult::OutOfGas),
        Reply::JsonRpc { code, message, data } if is_revert(*code, message) => {
            let bytes = data.as_ref().and_then(|d| parse_data(d, "revert data").ok()).unwrap_or_default();
            Ok(SimulationResult::Reverted(bytes))
        }
        // Any other JSON-RPC error (rate limit, unknown block) is a failed provider.
        Reply::JsonRpc { code, message, .. } => Err(format!("JSON-RPC error {code}: {}", trunc(&redact_urls(message), 80))),
        Reply::Failed(e) => Err(e.clone()),
    })
}

/// `eth_getLogs` for one address and topic0 over `[from, to]`, by quorum on the whole
/// sorted vector, with the configured logs response cap. A provider whose answer breaks
/// the filter or has duplicate positions does not vote.
pub async fn get_logs(address: &[u8; 20], topic0: &[u8; 32], from: u64, to: u64) -> Result<Vec<RawLog>, RpcError> {
    let cap = settings()?.logs_max;
    get_logs_with_cap(address, topic0, from, to, cap).await
}

/// [`get_logs`] with an explicit response cap. The cap is bounded to what the EVM RPC
/// canister accepts, and the attached cycles scale with it.
pub async fn get_logs_with_cap(
    address: &[u8; 20],
    topic0: &[u8; 32],
    from: u64,
    to: u64,
    max_response_bytes: u64,
) -> Result<Vec<RawLog>, RpcError> {
    if from > to {
        return Err(RpcError::Decode(format!("invalid log range [{from}, {to}]")));
    }
    let s = settings()?;
    let cap = max_response_bytes.clamp(1, MAX_RESPONSE_BYTES);
    let params = json!([{
        "address": hex0x(address),
        "topics": [hex0x(topic0)],
        "fromBlock": quantity_param(from),
        "toBlock": quantity_param(to),
    }]);
    let replies = call_all(&s, "eth_getLogs", &params, cap).await;
    vote(&replies, s.quorum, &format!("eth_getLogs [{from}, {to}]"), |r| {
        ok_value(r).and_then(|v| parse_logs(v, address, topic0, from, to))
    })
}

async fn receipt_round(s: &Settings, tx_hash: &[u8; 32], cap: u64) -> Result<Option<Receipt>, RpcError> {
    let replies = call_all(s, "eth_getTransactionReceipt", &json!([hex0x(tx_hash)]), cap).await;
    vote(&replies, s.quorum, "receipt", |r| ok_value(r).and_then(|v| parse_receipt(v, tx_hash)))
}

/// The receipt of `tx_hash`, by quorum. `None` (no receipt) is a value like any other.
///
/// The bridge's own transactions (a mint, a few admin calls) have small receipts, so the
/// small default cap is used. Only when every provider fails (an oversized response is
/// reported as a failure by the EVM RPC canister) is the call repeated once with the
/// larger logs cap.
pub async fn get_receipt(tx_hash: &[u8; 32]) -> Result<Option<Receipt>, RpcError> {
    let s = settings()?;
    match receipt_round(&s, tx_hash, s.default_max).await {
        Err(RpcError::AllFailed(_)) if s.logs_max > s.default_max => receipt_round(&s, tx_hash, s.logs_max).await,
        other => other,
    }
}

async fn chain_id_with(s: &Settings) -> Result<u64, RpcError> {
    let replies = call_all(s, "eth_chainId", &json!([]), s.default_max).await;
    vote(&replies, s.quorum, "chain id", |r| ok_value(r).and_then(|v| parse_quantity_u64(v, "chain id")))
}

/// `eth_chainId`, by quorum.
pub async fn chain_id() -> Result<u64, RpcError> {
    let s = settings()?;
    chain_id_with(&s).await
}

/// Check that a quorum of an explicit provider set reports chain `expected`. Used on a
/// proposed provider list BEFORE it is stored, so a list that points at the wrong
/// network is never adopted. The quorum rules are those of every other read.
pub async fn verify_chain_id_with(services: &[RpcServiceConfig], min_quorum: u8, expected: u64) -> Result<(), RpcError> {
    let mut s = base_settings();
    s.services = services.to_vec();
    s.quorum = min_quorum as usize;
    let s = s.validated()?;
    let got = chain_id_with(&s).await?;
    if got != expected {
        return Err(RpcError::NotConfigured(format!("the RPC providers serve chain id {got}, expected {expected}")));
    }
    Ok(())
}

/// `eth_getTransactionCount`, by quorum.
pub async fn transaction_count(address: &[u8; 20], tag: BlockTag) -> Result<u64, RpcError> {
    let s = settings()?;
    let replies = call_all(&s, "eth_getTransactionCount", &json!([hex0x(address), tag_param(&tag)]), s.default_max).await;
    vote(&replies, s.quorum, "transaction count", |r| ok_value(r).and_then(|v| parse_quantity_u64(v, "transaction count")))
}

/// `eth_getBalance` in wei, by quorum.
pub async fn balance(address: &[u8; 20], tag: BlockTag) -> Result<u128, RpcError> {
    let s = settings()?;
    let replies = call_all(&s, "eth_getBalance", &json!([hex0x(address), tag_param(&tag)]), s.default_max).await;
    vote(&replies, s.quorum, "balance", |r| ok_value(r).and_then(|v| parse_quantity_u128(v, "balance")))
}

/// Fee inputs: the maximum base fee of the `latest` block and the maximum suggested
/// priority fee across the providers that answered. No quorum: a high answer can only
/// raise a bid that the caller's fee caps bound anyway. Errors only if no provider
/// answered one of the two questions.
pub async fn fee_estimate() -> Result<FeeEstimate, RpcError> {
    let s = settings()?;
    let block_replies = call_all(&s, "eth_getBlockByNumber", &json!(["latest", false]), s.block_cap()).await;
    let bases: Vec<Result<u128, String>> = block_replies
        .iter()
        .map(|r| {
            ok_value(r).and_then(|v| {
                let o = v.as_object().ok_or("latest block is not an object")?;
                parse_quantity_u128(field(o, "baseFeePerGas")?, "baseFeePerGas")
            })
        })
        .collect();
    let prio_replies = call_all(&s, "eth_maxPriorityFeePerGas", &json!([]), s.default_max).await;
    let prios: Vec<Result<u128, String>> =
        prio_replies.iter().map(|r| ok_value(r).and_then(|v| parse_quantity_u128(v, "maxPriorityFeePerGas"))).collect();
    let max_of = |xs: &[Result<u128, String>], what: &str| -> Result<u128, RpcError> {
        match xs.iter().filter_map(|x| x.as_ref().ok()).max() {
            Some(m) => Ok(*m),
            None => {
                let errors: Vec<(usize, String)> = xs.iter().enumerate().filter_map(|(i, x)| x.as_ref().err().map(|e| (i, e.clone()))).collect();
                Err(RpcError::AllFailed(format!("{what}: all {} providers failed: {}", xs.len(), errors_label(&errors))))
            }
        }
    };
    Ok(FeeEstimate { base_fee_per_gas: max_of(&bases, "base fee")?, priority_fee_per_gas: max_of(&prios, "priority fee")? })
}

/// Broadcast to every provider. Results are hints only: the caller learns whether a
/// transaction landed from receipts, never from these.
pub async fn send_raw(raw: &[u8]) -> Vec<Result<String, String>> {
    let s = match settings() {
        Ok(s) => s,
        Err(e) => return vec![Err(e.to_string())],
    };
    let replies = call_all(&s, "eth_sendRawTransaction", &json!([hex0x(raw)]), s.default_max).await;
    replies
        .iter()
        .map(|r| match r {
            Reply::Ok(Value::String(h)) => Ok(trunc(h, MAX_ERR_CHARS)),
            Reply::Ok(other) => Ok(trunc(&other.to_string(), MAX_ERR_CHARS)),
            Reply::JsonRpc { code, message, .. } => Err(format!("{code}: {}", trunc(&redact_urls(message), MAX_ERR_CHARS))),
            Reply::Failed(e) => Err(e.clone()),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Test support shared with the observer tests
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;

    pub fn ok_body(result: Value) -> String {
        json!({"jsonrpc": "2.0", "id": 1, "result": result}).to_string()
    }

    pub fn err_body(code: i64, message: &str, data: Option<Value>) -> String {
        let mut e = json!({"code": code, "message": message});
        if let Some(d) = data {
            e["data"] = d;
        }
        json!({"jsonrpc": "2.0", "id": 1, "error": e}).to_string()
    }

    pub fn h32(n: u64, salt: u8) -> [u8; 32] {
        let mut h = [0u8; 32];
        h[..8].copy_from_slice(&n.to_be_bytes());
        h[31] = salt;
        h
    }

    pub fn block_json(number: u64, hash: [u8; 32]) -> Value {
        json!({"number": quantity_param(number), "hash": hex0x(&hash), "baseFeePerGas": "0x3b9aca00", "transactions": []})
    }

    pub fn log_json(l: &RawLog) -> Value {
        json!({
            "address": hex0x(&l.address),
            "topics": l.topics.iter().map(|t| hex0x(t)).collect::<Vec<_>>(),
            "data": hex0x(&l.data),
            "blockNumber": quantity_param(l.block_number),
            "blockHash": hex0x(&l.block_hash),
            "transactionHash": hex0x(&l.tx_hash),
            "transactionIndex": "0x0",
            "logIndex": quantity_param(l.log_index),
            "removed": l.removed,
        })
    }

    pub fn url(i: usize) -> String {
        format!("https://p{}.example", i + 1)
    }

    pub fn services(n: usize) -> Vec<RpcServiceConfig> {
        (0..n).map(|i| RpcServiceConfig::Custom { url: url(i) }).collect()
    }

    /// Zero-based provider position of a configured service.
    pub fn provider_index(svc: &RpcServiceConfig) -> usize {
        match svc {
            RpcServiceConfig::Custom { url } => {
                url.trim_start_matches("https://p").trim_end_matches(".example").parse::<usize>().expect("test url") - 1
            }
            other => panic!("unexpected service {other:?}"),
        }
    }

    /// Providers `p1..pn` on Base Sepolia (the head rule comes from the chain table, so a
    /// test chain needs a profiled id; callers that need another chain set it afterwards).
    pub fn configure(n: usize, quorum: u8) {
        state::reset_for_tests();
        state::mutate_core(|c| {
            c.config.chain_id = state::CHAIN_BASE_SEPOLIA;
            c.config.rpc_services = services(n);
            c.config.min_quorum = quorum;
            c.config.evm_rpc = Principal::from_slice(&[7; 10]);
        });
    }

    /// `(method, params)` of a JSON-RPC request text.
    pub fn request_of(body: &str) -> (String, Value) {
        let v: Value = serde_json::from_str(body).expect("request is JSON");
        (v["method"].as_str().expect("method").to_string(), v["params"].clone())
    }

    /// Poll a future that never suspends (the host transport completes immediately).
    pub fn block_on<F: std::future::Future>(f: F) -> F::Output {
        use std::task::{Context, Poll, Waker};
        let mut f = std::pin::pin!(f);
        let mut cx = Context::from_waker(Waker::noop());
        match f.as_mut().poll(&mut cx) {
            Poll::Ready(v) => v,
            Poll::Pending => panic!("future suspended on the host"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;
    use crate::types::L2MainnetService;

    const ADDR: [u8; 20] = [0xcc; 20];
    const TOPIC: [u8; 32] = [0x57; 32];

    fn quantity(n: u64) -> Value {
        json!(quantity_param(n))
    }

    /// All providers answer from one function of `(provider, method, params)`.
    fn script<F>(f: F)
    where
        F: Fn(usize, &str, &Value) -> Result<String, String> + 'static,
    {
        set_mock_transport(move |svc, body| {
            let (m, p) = request_of(body);
            f(provider_index(svc), &m, &p)
        });
    }

    fn methods() -> Vec<(usize, String)> {
        mock_calls().iter().map(|c| (provider_index(&c.service), c.method.clone())).collect()
    }

    fn sample_log(block: u64, index: u64) -> RawLog {
        RawLog {
            address: ADDR,
            topics: vec![TOPIC, [1; 32], [2; 32]],
            data: vec![0xab; 32],
            block_number: block,
            block_hash: h32(block, 0),
            tx_hash: h32(block * 1000 + index, 9),
            log_index: index,
            removed: false,
        }
    }

    // ----- strict parsing --------------------------------------------------------

    #[test]
    fn quantities_are_strict() {
        let ok = |s: &str| parse_quantity_u64(&json!(s), "q");
        assert_eq!(ok("0x0"), Ok(0));
        assert_eq!(ok("0x1"), Ok(1));
        assert_eq!(ok("0xff"), Ok(255));
        assert_eq!(ok("0xFF"), Ok(255));
        assert_eq!(ok("0xffffffffffffffff"), Ok(u64::MAX));
        for bad in ["0x", "0x00", "0x01", "0x0f", "ff", "0Xff", "0x1g", "0x10000000000000000", " 0x1", "0x1 ", ""] {
            assert!(ok(bad).is_err(), "{bad:?} must be rejected");
        }
        assert!(parse_quantity_u64(&json!(5), "q").is_err(), "a JSON number is not a QUANTITY");
        assert!(parse_quantity_u64(&Value::Null, "q").is_err());
        assert_eq!(parse_quantity_u128(&json!("0xffffffffffffffffffffffffffffffff"), "q"), Ok(u128::MAX));
        assert!(parse_quantity_u128(&json!("0x100000000000000000000000000000000"), "q").is_err());
        assert!(parse_quantity_u128(&json!("0x00"), "q").is_err());
    }

    #[test]
    fn data_addresses_and_hashes_are_strict() {
        assert_eq!(parse_data(&json!("0x"), "d"), Ok(vec![]));
        assert_eq!(parse_data(&json!("0x00ff"), "d"), Ok(vec![0, 255]));
        assert!(parse_data(&json!("0x0"), "d").is_err(), "odd length");
        assert!(parse_data(&json!("00ff"), "d").is_err(), "no prefix");
        assert!(parse_data(&json!("0xzz"), "d").is_err());
        assert!(parse_fixed::<20>(&json!(format!("0x{}", "11".repeat(20))), "a").is_ok());
        assert!(parse_fixed::<20>(&json!(format!("0x{}", "11".repeat(19))), "a").is_err());
        assert!(parse_fixed::<20>(&json!(format!("0x{}", "11".repeat(21))), "a").is_err());
        assert!(parse_fixed::<32>(&json!(format!("0x{}", "11".repeat(32))), "h").is_ok());
        assert!(parse_fixed::<32>(&json!(format!("0x{}", "11".repeat(31))), "h").is_err());
    }

    #[test]
    fn envelope_requires_exactly_one_of_result_and_error() {
        assert!(matches!(parse_envelope(r#"{"jsonrpc":"2.0","id":1,"result":"0x1"}"#), Reply::Ok(_)));
        assert!(matches!(parse_envelope(r#"{"result":null}"#), Reply::Ok(Value::Null)));
        assert!(matches!(parse_envelope(r#"{"result":"0x1","error":null}"#), Reply::Ok(_)));
        assert!(matches!(parse_envelope(r#"{"error":{"code":3,"message":"execution reverted","data":"0x08c379a0"}}"#), Reply::JsonRpc { code: 3, .. }));
        for bad in [r#"{"id":1}"#, r#"{"result":"0x1","error":{"code":1,"message":"x"}}"#, "[]", "not json", "", r#""0x1""#] {
            assert!(matches!(parse_envelope(bad), Reply::Failed(_)), "{bad:?}");
        }
    }

    #[test]
    fn urls_are_redacted_from_error_text() {
        assert_eq!(redact_urls("call to https://k.example/v2/SECRET failed"), "call to <url> failed");
        assert_eq!(redact_urls("a http://x/y, b https://z"), "a <url>, b <url>");
        assert_eq!(redact_urls("no urls here"), "no urls here");
        assert_eq!(trunc("héllo wörld", 5), "héllo...");
        assert_eq!(trunc("short", 50), "short");
    }

    // ----- Candid mirror ---------------------------------------------------------

    #[test]
    fn service_mapping() {
        assert_eq!(to_rpc_service(&RpcServiceConfig::BaseMainnet(L2MainnetService::Ankr)), RpcService::BaseMainnet(L2MainnetService::Ankr));
        assert_eq!(
            to_rpc_service(&RpcServiceConfig::Custom { url: "https://x".into() }),
            RpcService::Custom(RpcApi { url: "https://x".into(), headers: None })
        );
    }

    #[test]
    fn every_native_provider_maps_to_its_own_mirror_service() {
        use crate::types::{EthMainnetService as M, EthSepoliaService as S, L2MainnetService as L};
        let mut checked = 0;
        for p in [M::Alchemy, M::Ankr, M::BlockPi, M::Cloudflare, M::PublicNode, M::Llama] {
            assert_eq!(to_rpc_service(&RpcServiceConfig::EthMainnet(p)), RpcService::EthMainnet(p));
            checked += 1;
        }
        for p in [S::Alchemy, S::Ankr, S::BlockPi, S::PublicNode, S::Sepolia] {
            assert_eq!(to_rpc_service(&RpcServiceConfig::EthSepolia(p)), RpcService::EthSepolia(p));
            checked += 1;
        }
        for p in [L::Alchemy, L::Ankr, L::BlockPi, L::PublicNode, L::Llama] {
            assert_eq!(to_rpc_service(&RpcServiceConfig::ArbitrumOne(p)), RpcService::ArbitrumOne(p));
            // The same inner provider names a different service on each chain.
            assert_ne!(to_rpc_service(&RpcServiceConfig::ArbitrumOne(p)), to_rpc_service(&RpcServiceConfig::BaseMainnet(p)));
            checked += 1;
        }
        assert_eq!(checked, 16);
    }

    #[test]
    fn native_services_are_spelled_as_the_live_interface_spells_them() {
        use crate::types::{EthMainnetService as M, EthSepoliaService as S, L2MainnetService as L};
        // `request : (RpcService, text, nat64)`, the arm names and provider names exactly as
        // the EVM RPC canister's interface (and the test relay's copy of it) writes them.
        let cases = [
            ("variant { EthMainnet = variant { Cloudflare } }", RpcServiceConfig::EthMainnet(M::Cloudflare)),
            ("variant { EthMainnet = variant { Llama } }", RpcServiceConfig::EthMainnet(M::Llama)),
            ("variant { EthSepolia = variant { Sepolia } }", RpcServiceConfig::EthSepolia(S::Sepolia)),
            ("variant { EthSepolia = variant { PublicNode } }", RpcServiceConfig::EthSepolia(S::PublicNode)),
            ("variant { ArbitrumOne = variant { BlockPi } }", RpcServiceConfig::ArbitrumOne(L::BlockPi)),
            ("variant { BaseMainnet = variant { BlockPi } }", RpcServiceConfig::BaseMainnet(L::BlockPi)),
        ];
        for (text, cfg) in cases {
            let args = format!(r#"({text}, "{{}}", 8192 : nat64)"#);
            let bytes = candid_parser::parse_idl_args(&args).unwrap().to_bytes().unwrap();
            let (service, body, cap): (RpcService, String, u64) = candid::decode_args(&bytes).unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(service, to_rpc_service(&cfg), "{text}");
            assert_eq!((body.as_str(), cap), ("{}", 8192));
            // And our own encoding decodes back to the same service.
            let ours = candid::encode_args((to_rpc_service(&cfg), "{}".to_string(), 8_192u64)).unwrap();
            let (back, _, _): (RpcService, String, u64) = candid::decode_args(&ours).unwrap();
            assert_eq!(back, service, "{text}");
        }
    }

    #[test]
    fn request_arguments_and_every_error_shape_roundtrip_through_candid() {
        // Arguments of `request : (RpcService, text, nat64)`.
        let args = (to_rpc_service(&RpcServiceConfig::BaseMainnet(L2MainnetService::Llama)), "{}".to_string(), 8_192u64);
        let bytes = candid::encode_args(args.clone()).unwrap();
        let back: (RpcService, String, u64) = candid::decode_args(&bytes).unwrap();
        assert_eq!(back, args);
        // Replies, written in Candid text exactly as the canister's interface spells them.
        let cases = [
            r#"(variant { Ok = "{}" })"#,
            r#"(variant { Err = variant { ProviderError = variant { TooFewCycles = record { expected = 3_714_459_200 : nat; received = 2_000_000_000 : nat } } } })"#,
            r#"(variant { Err = variant { ProviderError = variant { MissingRequiredProvider } } })"#,
            r#"(variant { Err = variant { ProviderError = variant { ProviderNotFound } } })"#,
            r#"(variant { Err = variant { ProviderError = variant { NoPermission } } })"#,
            r#"(variant { Err = variant { ProviderError = variant { InvalidRpcConfig = "x" } } })"#,
            r#"(variant { Err = variant { HttpOutcallError = variant { IcError = record { code = variant { SysTransient }; message = "m" } } } })"#,
            r#"(variant { Err = variant { HttpOutcallError = variant { InvalidHttpJsonRpcResponse = record { status = 502 : nat16; body = "b"; parsingError = opt "p" } } } })"#,
            r#"(variant { Err = variant { JsonRpcError = record { code = -32000 : int64; message = "m" } } })"#,
            r#"(variant { Err = variant { ValidationError = variant { InvalidHex = "x" } } })"#,
            r#"(variant { Err = variant { ValidationError = variant { Custom = "x" } } })"#,
        ];
        for c in cases {
            let bytes = candid_parser::parse_idl_args(c).unwrap().to_bytes().unwrap();
            let (decoded,): (RequestResult,) = candid::decode_args(&bytes).unwrap_or_else(|e| panic!("{c}: {e}"));
            let again = candid::encode_args((decoded.clone(),)).unwrap();
            let (decoded2,): (RequestResult,) = candid::decode_args(&again).unwrap();
            assert_eq!(decoded, decoded2, "{c}");
        }
    }

    // ----- quorum ---------------------------------------------------------------

    #[test]
    fn two_of_three_agreeing_wins_and_the_odd_one_out_is_ignored() {
        configure(3, 2);
        script(|p, _, _| Ok(ok_body(json!(if p == 1 { "0x2a" } else { "0x7" }))));
        assert_eq!(block_on(transaction_count(&ADDR, BlockTag::Latest)), Ok(7));
    }

    #[test]
    fn one_of_three_answering_is_no_quorum_when_two_are_required() {
        configure(3, 2);
        script(|p, _, _| if p == 0 { Ok(ok_body(json!("0x7"))) } else { Err("timeout".into()) });
        match block_on(transaction_count(&ADDR, BlockTag::Latest)) {
            Err(RpcError::NoQuorum(m)) => {
                assert!(m.contains("p2 failed") && m.contains("p3 failed"), "{m}");
                assert!(!m.contains("example"), "provider URLs must not appear: {m}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn one_of_three_is_enough_when_one_is_required() {
        configure(3, 1);
        script(|p, _, _| if p == 2 { Ok(ok_body(json!("0x7"))) } else { Err("down".into()) });
        assert_eq!(block_on(transaction_count(&ADDR, BlockTag::Latest)), Ok(7));
    }

    #[test]
    fn all_three_disagreeing_is_no_quorum() {
        configure(3, 2);
        script(|p, _, _| Ok(ok_body(quantity(p as u64 + 1))));
        assert!(matches!(block_on(transaction_count(&ADDR, BlockTag::Latest)), Err(RpcError::NoQuorum(_))));
    }

    #[test]
    fn every_provider_failing_is_all_failed() {
        configure(3, 2);
        script(|_, _, _| Err("down".into()));
        assert!(matches!(block_on(balance(&ADDR, BlockTag::Latest)), Err(RpcError::AllFailed(_))));
    }

    #[test]
    fn provider_errors_and_malformed_answers_do_not_vote() {
        configure(4, 2);
        // p1 transport error, p2 JSON-RPC error, p3 malformed hex, p4 fine.
        script(|p, _, _| match p {
            0 => Err("boom".into()),
            1 => Ok(err_body(-32005, "rate limited", None)),
            2 => Ok(ok_body(json!("0x007"))),
            _ => Ok(ok_body(json!("0x7"))),
        });
        assert!(matches!(block_on(transaction_count(&ADDR, BlockTag::Latest)), Err(RpcError::NoQuorum(_))), "one valid answer cannot reach quorum 2");
        script(|p, _, _| match p {
            0 => Err("boom".into()),
            1 => Ok(err_body(-32005, "rate limited", None)),
            _ => Ok(ok_body(json!("0x7"))),
        });
        assert_eq!(block_on(transaction_count(&ADDR, BlockTag::Latest)), Ok(7));
    }

    #[test]
    fn a_split_vote_where_two_values_reach_quorum_decides_nothing() {
        configure(4, 2);
        script(|p, _, _| Ok(ok_body(json!(if p < 2 { "0x1" } else { "0x2" }))));
        match block_on(transaction_count(&ADDR, BlockTag::Latest)) {
            Err(RpcError::NoQuorum(m)) => assert!(m.contains("conflicting"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn voting_is_on_parsed_values_so_extra_fields_do_not_break_agreement() {
        configure(3, 3);
        let h = h32(100, 0);
        script(move |p, _, _| {
            let mut b = block_json(100, h);
            b["extraProviderField"] = json!(format!("only-{p}"));
            b["miner"] = json!(format!("0x{:040x}", p));
            Ok(ok_body(b))
        });
        assert_eq!(block_on(block_by_number(100)), Ok(BlockRef { number: 100, hash: h }));
    }

    #[test]
    fn unconfigured_transport_is_reported() {
        state::reset_for_tests();
        assert!(matches!(block_on(balance(&ADDR, BlockTag::Latest)), Err(RpcError::NotConfigured(_))));
        configure(3, 2);
        state::mutate_core(|c| c.config.min_quorum = 4);
        assert!(matches!(block_on(balance(&ADDR, BlockTag::Latest)), Err(RpcError::NotConfigured(_))));
        state::mutate_core(|c| c.config.min_quorum = 0);
        assert!(matches!(block_on(balance(&ADDR, BlockTag::Latest)), Err(RpcError::NotConfigured(_))));
        configure(3, 2);
        clear_mock_transport();
        assert!(matches!(block_on(balance(&ADDR, BlockTag::Latest)), Err(RpcError::AllFailed(_))));
    }

    // ----- finalized head ---------------------------------------------------------

    /// Providers at the given finalized heights, all agreeing on hashes, serving any
    /// block up to their own head.
    fn chain(heads: Vec<u64>) {
        script(move |p, m, params| {
            assert_eq!(m, "eth_getBlockByNumber");
            let n = match params[0].as_str().unwrap() {
                "finalized" => heads[p],
                hex => u64::from_str_radix(hex.trim_start_matches("0x"), 16).unwrap(),
            };
            if n > heads[p] {
                return Ok(ok_body(Value::Null));
            }
            Ok(ok_body(block_json(n, h32(n, 0))))
        });
    }

    #[test]
    fn finalized_head_is_the_quorum_th_highest_height_with_a_hash_quorum() {
        configure(3, 2);
        chain(vec![100, 103, 98]);
        assert_eq!(block_on(finalized_head()), Ok(BlockRef { number: 100, hash: h32(100, 0) }));
        // Round 1: finalized from all three. Round 2: block 100 only from the provider
        // that was ahead (p2). The one at 100 is reused; the one at 98 does not vote.
        let m = methods();
        assert_eq!(m.len(), 4, "{m:?}");
        assert_eq!(m[3].0, 1);
    }

    #[test]
    fn finalized_head_is_one_round_when_providers_are_level() {
        configure(3, 2);
        chain(vec![500, 500, 500]);
        assert_eq!(block_on(finalized_head()).unwrap().number, 500);
        assert_eq!(mock_calls().len(), 3);
    }

    #[test]
    fn a_far_ahead_outlier_cannot_raise_the_head() {
        configure(3, 2);
        chain(vec![100, 100, u64::MAX / 2]);
        assert_eq!(block_on(finalized_head()).unwrap().number, 100);
        configure(3, 3);
        chain(vec![100, 100, 5_000]);
        assert_eq!(block_on(finalized_head()).unwrap().number, 100, "quorum 3 takes the lowest");
    }

    #[test]
    fn finalized_head_hash_disagreement_is_no_quorum() {
        configure(3, 2);
        script(|p, _, params| {
            let n = match params[0].as_str().unwrap() {
                "finalized" => 100,
                hex => u64::from_str_radix(hex.trim_start_matches("0x"), 16).unwrap(),
            };
            Ok(ok_body(block_json(n, h32(n, p as u8))))
        });
        assert!(matches!(block_on(finalized_head()), Err(RpcError::NoQuorum(_))));
    }

    #[test]
    fn finalized_head_with_one_dissenting_hash_still_wins() {
        configure(3, 2);
        script(|p, _, params| {
            let n = match params[0].as_str().unwrap() {
                "finalized" => 100,
                hex => u64::from_str_radix(hex.trim_start_matches("0x"), 16).unwrap(),
            };
            Ok(ok_body(block_json(n, h32(n, if p == 2 { 99 } else { 0 }))))
        });
        assert_eq!(block_on(finalized_head()), Ok(BlockRef { number: 100, hash: h32(100, 0) }));
    }

    #[test]
    fn finalized_head_needs_quorum_many_answers() {
        configure(3, 2);
        script(|p, _, _| if p == 0 { Ok(ok_body(block_json(100, h32(100, 0)))) } else { Err("down".into()) });
        assert!(matches!(block_on(finalized_head()), Err(RpcError::NoQuorum(_))));
        script(|_, _, _| Err("down".into()));
        assert!(matches!(block_on(finalized_head()), Err(RpcError::AllFailed(_))));
    }

    #[test]
    fn finalized_head_refetch_must_return_the_block_asked_for() {
        configure(3, 2);
        // p2 is ahead but answers the refetch for block 100 with a different block.
        script(|p, _, params| {
            let tag = params[0].as_str().unwrap();
            if tag == "finalized" {
                let n = [100u64, 103, 100][p];
                return Ok(ok_body(block_json(n, h32(n, 0))));
            }
            Ok(ok_body(block_json(103, h32(103, 0))))
        });
        // p1 and p3 are at 100 and agree, so quorum holds without p2.
        assert_eq!(block_on(finalized_head()).unwrap().number, 100);
        // With quorum 3 the wrong refetch costs the vote.
        configure(3, 3);
        script(|p, _, params| {
            let tag = params[0].as_str().unwrap();
            if tag == "finalized" {
                let n = [100u64, 103, 100][p];
                return Ok(ok_body(block_json(n, h32(n, 0))));
            }
            Ok(ok_body(block_json(103, h32(103, 0))))
        });
        assert!(matches!(block_on(finalized_head()), Err(RpcError::NoQuorum(_))));
    }

    // ----- head rules -------------------------------------------------------------

    /// Point the configured chain at a table row (the head rule is read from it).
    fn on_chain(chain_id: u64) {
        state::mutate_core(|c| c.config.chain_id = chain_id);
    }

    type Bodies = std::rc::Rc<std::cell::RefCell<Vec<String>>>;

    /// Like `chain`, and every request body is kept.
    fn recording_chain(heads: Vec<u64>) -> Bodies {
        let bodies: Bodies = Default::default();
        let sink = bodies.clone();
        set_mock_transport(move |svc, body| {
            sink.borrow_mut().push(body.to_string());
            let (m, params) = request_of(body);
            assert_eq!(m, "eth_getBlockByNumber");
            let p = provider_index(svc);
            let n = match params[0].as_str().unwrap() {
                "finalized" => heads[p],
                hex => u64::from_str_radix(hex.trim_start_matches("0x"), 16).unwrap(),
            };
            if n > heads[p] {
                return Ok(ok_body(Value::Null));
            }
            Ok(ok_body(block_json(n, h32(n, 0))))
        });
        bodies
    }

    /// The first parameter of every kept request, in order.
    fn first_params(bodies: &Bodies) -> Vec<String> {
        bodies.borrow().iter().map(|b| request_of(b).1[0].as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn monad_head_is_finalized_minus_delay() {
        configure(3, 2);
        on_chain(10_143);
        let bodies = recording_chain(vec![1_000, 1_000, 1_000]);
        assert_eq!(block_on(finalized_head()), Ok(BlockRef { number: 997, hash: h32(997, 0) }));
        // Round 1: the finalized tag from each provider. Round 2: block 997 from each.
        let want_finalized = "finalized".to_string();
        let want_head = quantity_param(997);
        assert_eq!(
            first_params(&bodies),
            vec![want_finalized.clone(), want_finalized.clone(), want_finalized, want_head.clone(), want_head.clone(), want_head]
        );
        // The same on mainnet, and with providers at different heights: n is the quorum-th
        // highest (here 1_000), so the head is 997 whatever the far-ahead provider says.
        configure(3, 2);
        on_chain(143);
        let bodies = recording_chain(vec![1_000, 1_003, 998]);
        assert_eq!(block_on(finalized_head()), Ok(BlockRef { number: 997, hash: h32(997, 0) }));
        let last = first_params(&bodies).pop().unwrap();
        assert_eq!(last, quantity_param(997));
        // Quorum 3 takes the lowest height, 998, and the head is 995.
        configure(3, 3);
        on_chain(143);
        recording_chain(vec![1_000, 1_003, 998]);
        assert_eq!(block_on(finalized_head()).unwrap().number, 995);
    }

    #[test]
    fn monad_head_hash_needs_a_quorum() {
        configure(3, 2);
        on_chain(10_143);
        // Providers agree on the finalized block but p3 disagrees on the hash at the head.
        script(|p, _, params| {
            let n = match params[0].as_str().unwrap() {
                "finalized" => 1_000,
                hex => u64::from_str_radix(hex.trim_start_matches("0x"), 16).unwrap(),
            };
            let salt = if n == 997 && p == 2 { 9 } else { 0 };
            Ok(ok_body(block_json(n, h32(n, salt))))
        });
        assert_eq!(block_on(finalized_head()), Ok(BlockRef { number: 997, hash: h32(997, 0) }), "one dissenting hash is outvoted");
        script(|p, _, params| {
            let n = match params[0].as_str().unwrap() {
                "finalized" => 1_000,
                hex => u64::from_str_radix(hex.trim_start_matches("0x"), 16).unwrap(),
            };
            Ok(ok_body(block_json(n, h32(n, if n == 997 { p as u8 } else { 0 }))))
        });
        assert!(matches!(block_on(finalized_head()), Err(RpcError::NoQuorum(_))), "three different hashes at the head");
        // Too few providers serve the head block.
        script(|p, _, params| {
            let n = match params[0].as_str().unwrap() {
                "finalized" => 1_000,
                hex => u64::from_str_radix(hex.trim_start_matches("0x"), 16).unwrap(),
            };
            if n == 997 && p != 0 {
                return Err("down".into());
            }
            Ok(ok_body(block_json(n, h32(n, 0))))
        });
        assert!(matches!(block_on(finalized_head()), Err(RpcError::NoQuorum(_))));
    }

    #[test]
    fn monad_head_refuses_when_finalized_is_below_delay() {
        configure(3, 2);
        on_chain(10_143);
        // finalized = 2 < 3: there is no Verified block yet. Nothing is asked after the
        // finalized round.
        let bodies = recording_chain(vec![2, 2, 2]);
        match block_on(finalized_head()) {
            Err(RpcError::NoQuorum(m)) => assert!(m.contains("below the execution delay of 3"), "{m}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(bodies.borrow().len(), 3, "only the finalized round was sent");
        // finalized = 3 is exactly the delay: the head is block 0.
        configure(3, 2);
        on_chain(10_143);
        recording_chain(vec![3, 3, 3]);
        assert_eq!(block_on(finalized_head()), Ok(BlockRef { number: 0, hash: h32(0, 0) }));
        // And the numbered tag for a view call refuses the same way.
        configure(3, 2);
        on_chain(10_143);
        recording_chain(vec![2, 2, 2]);
        assert!(matches!(block_on(settlement_block_tag()), Err(RpcError::NoQuorum(_))));
    }

    #[test]
    fn head_rules_never_send_latest() {
        // Every row of the chain table, through the dispatcher and the view-call tag: no
        // request of any rule carries `latest` (or any other moving tag but `finalized`).
        let ids = [8_453u64, 84_532, 31_337, 1, 11_155_111, 42_161, 421_614, 4_663, 46_630, 999, 998, 143, 10_143];
        for chain_id in ids {
            let row = chains::profile(chain_id).unwrap();
            configure(3, 2);
            on_chain(chain_id);
            let bodies = recording_chain(vec![1_000, 1_000, 1_000]);
            let head = block_on(finalized_head());
            let tag = block_on(settlement_block_tag());
            match row.head_rule {
                HeadRule::Blocked(_) => {
                    assert!(head.is_err() && tag.is_err(), "chain {chain_id}");
                    assert!(bodies.borrow().is_empty(), "chain {chain_id}: a blocked rule sent a request");
                }
                _ => {
                    assert!(head.is_ok() && tag.is_ok(), "chain {chain_id}: {head:?} {tag:?}");
                    assert!(!bodies.borrow().is_empty() || tag == Ok(BlockTag::Finalized));
                }
            }
            for b in bodies.borrow().iter() {
                for moving in ["latest", "pending", "safe", "earliest"] {
                    assert!(!b.contains(&format!("\"{moving}\"")), "chain {chain_id}: request carries {moving}: {b}");
                }
                let (m, params) = request_of(b);
                assert_eq!(m, "eth_getBlockByNumber", "chain {chain_id}");
                let first = params[0].as_str().unwrap();
                assert!(first == "finalized" || first.starts_with("0x"), "chain {chain_id}: unexpected block parameter {first}");
            }
        }
    }

    #[test]
    fn blocked_rule_returns_error_without_provider_calls() {
        for (chain_id, reason) in [
            (4_663u64, "robinhood-finalized-tag-probe"),
            (46_630, "robinhood-finalized-tag-probe"),
            (999, "hyperevm-historical-state"),
            (998, "hyperevm-historical-state"),
        ] {
            configure(3, 2);
            on_chain(chain_id);
            // A transport that must never be reached.
            set_mock_transport(|_, _| panic!("a blocked head rule asked a provider"));
            match block_on(finalized_head()) {
                Err(RpcError::NotConfigured(m)) => assert!(m.contains(reason), "{m}"),
                other => panic!("chain {chain_id}: {other:?}"),
            }
            match block_on(settlement_block_tag()) {
                Err(RpcError::NotConfigured(m)) => assert!(m.contains(reason), "{m}"),
                other => panic!("chain {chain_id}: {other:?}"),
            }
            assert!(mock_calls().is_empty(), "chain {chain_id}");
        }
        // Even with no providers configured at all the answer is the block, not a transport error.
        state::reset_for_tests();
        on_chain(999);
        assert!(matches!(block_on(finalized_head()), Err(RpcError::NotConfigured(m)) if m.contains("hyperevm-historical-state")));
    }

    #[test]
    fn a_chain_without_a_row_has_no_head_rule_and_asks_no_provider() {
        configure(3, 2);
        on_chain(0);
        set_mock_transport(|_, _| panic!("an unprofiled chain asked a provider"));
        assert!(matches!(block_on(finalized_head()), Err(RpcError::NotConfigured(m)) if m.contains("no bridge chain profile for chain id 0")));
        assert!(matches!(block_on(settlement_block_tag()), Err(RpcError::NotConfigured(_))));
        assert!(matches!(head_rule(), Err(RpcError::NotConfigured(_))));
        assert!(mock_calls().is_empty());
    }

    #[test]
    fn tag_rules_dispatch_to_the_unchanged_body() {
        // Every TagFinalizedQuorum row sends exactly the requests the body alone sends.
        let heads = vec![100u64, 103, 98];
        configure(3, 2);
        on_chain(84_532);
        let direct = recording_chain(heads.clone());
        let want = block_on(tag_finalized_quorum());
        let direct_params = first_params(&direct);
        assert_eq!(want, Ok(BlockRef { number: 100, hash: h32(100, 0) }));
        for chain_id in [8_453u64, 84_532, 31_337, 1, 11_155_111, 42_161, 421_614] {
            configure(3, 2);
            on_chain(chain_id);
            let bodies = recording_chain(heads.clone());
            assert_eq!(block_on(finalized_head()), want, "chain {chain_id}");
            assert_eq!(first_params(&bodies), direct_params, "chain {chain_id}");
            assert_eq!(head_rule(), Ok(HeadRule::TagFinalizedQuorum));
            // The view-call tag for a tag rule is the tag itself and asks no provider.
            clear_mock_calls();
            assert_eq!(block_on(settlement_block_tag()), Ok(BlockTag::Finalized));
            assert!(mock_calls().is_empty());
        }
    }

    #[test]
    fn monad_view_calls_are_pinned_to_the_numbered_head() {
        configure(3, 2);
        on_chain(10_143);
        recording_chain(vec![500, 500, 500]);
        assert_eq!(block_on(settlement_block_tag()), Ok(BlockTag::Number(497)));
    }

    #[test]
    fn block_by_number_votes_on_number_and_hash() {
        configure(3, 2);
        script(|p, _, params| {
            assert_eq!(params[0], json!("0x64"));
            Ok(ok_body(block_json(100, h32(100, if p == 0 { 7 } else { 0 }))))
        });
        assert_eq!(block_on(block_by_number(100)), Ok(BlockRef { number: 100, hash: h32(100, 0) }));
        // A provider that returns a different block than asked does not vote.
        script(|p, _, _| Ok(ok_body(block_json(if p == 0 { 101 } else { 100 }, h32(100, 0)))));
        assert!(block_on(block_by_number(100)).is_ok());
        script(|p, _, _| Ok(ok_body(block_json(if p == 0 { 100 } else { 101 }, h32(100, 0)))));
        assert!(matches!(block_on(block_by_number(100)), Err(RpcError::NoQuorum(_))));
        // Null (block not found) and malformed blocks do not vote.
        script(|_, _, _| Ok(ok_body(Value::Null)));
        assert!(matches!(block_on(block_by_number(100)), Err(RpcError::AllFailed(_))));
        script(|_, _, _| Ok(ok_body(json!({"number": "0x064", "hash": hex0x(&h32(100, 0))}))));
        assert!(matches!(block_on(block_by_number(100)), Err(RpcError::AllFailed(_))));
        script(|_, _, _| Ok(ok_body(json!({"number": "0x64", "hash": "0x1234"}))));
        assert!(matches!(block_on(block_by_number(100)), Err(RpcError::AllFailed(_))));
    }

    // ----- eth_call and simulation -------------------------------------------------

    #[test]
    fn eth_call_returns_quorum_bytes_and_sends_the_right_request() {
        configure(3, 2);
        script(|p, m, params| {
            assert_eq!(m, "eth_call");
            assert_eq!(params[0]["to"], json!(hex0x(&ADDR)));
            assert_eq!(params[0]["data"], json!("0xdeadbeef"));
            assert_eq!(params[1], json!("0x2a"));
            Ok(ok_body(json!(if p == 2 { "0x01" } else { "0x00000000000000000000000000000000000000000000000000000000000000ff" })))
        });
        let got = block_on(eth_call(&ADDR, &[0xde, 0xad, 0xbe, 0xef], BlockTag::Number(42))).unwrap();
        assert_eq!(got.len(), 32);
        assert_eq!(got[31], 0xff);
    }

    #[test]
    fn block_tags_render_as_the_protocol_names() {
        configure(1, 1);
        for (tag, want) in [
            (BlockTag::Latest, "latest"),
            (BlockTag::Pending, "pending"),
            (BlockTag::Safe, "safe"),
            (BlockTag::Finalized, "finalized"),
            (BlockTag::Number(0), "0x0"),
            (BlockTag::Number(255), "0xff"),
        ] {
            script(move |_, _, params| {
                assert_eq!(params[1], json!(want));
                Ok(ok_body(json!("0x")))
            });
            assert_eq!(block_on(eth_call(&ADDR, &[], tag)), Ok(vec![]));
        }
    }

    #[test]
    fn eth_call_at_block_hash_uses_eip1898() {
        configure(3, 2);
        let h = h32(5, 0);
        script(move |_, _, params| {
            assert_eq!(params[1]["blockHash"], json!(hex0x(&h)));
            assert_eq!(params[1]["requireCanonical"], json!(true));
            Ok(ok_body(json!("0x01")))
        });
        assert_eq!(block_on(eth_call_at_block_hash(&ADDR, &[1], &h)), Ok(vec![1]));
    }

    #[test]
    fn a_reverting_eth_call_is_an_error_for_eth_call_but_an_answer_for_simulation() {
        configure(3, 2);
        let revert_data = "0x08c379a000";
        script(move |_, _, _| Ok(err_body(3, "execution reverted: nope", Some(json!(revert_data)))));
        assert!(matches!(block_on(eth_call(&ADDR, &[1], BlockTag::Latest)), Err(RpcError::AllFailed(_))));
        assert_eq!(
            block_on(simulate_call(&[1; 20], &ADDR, &[1], 300_000, BlockTag::Latest)),
            Ok(SimulationResult::Reverted(vec![0x08, 0xc3, 0x79, 0xa0, 0x00]))
        );
    }

    #[test]
    fn simulation_maps_each_revert_style_and_votes_on_the_result() {
        configure(3, 2);
        // Code 3 without data.
        script(|_, _, _| Ok(err_body(3, "x", None)));
        assert_eq!(block_on(simulate_call(&[1; 20], &ADDR, &[], 300_000, BlockTag::Latest)), Ok(SimulationResult::Reverted(vec![])));
        // Message-only revert, different code, empty data.
        script(|_, _, _| Ok(err_body(-32000, "Execution Reverted", Some(json!("0x")))));
        assert_eq!(block_on(simulate_call(&[1; 20], &ADDR, &[], 300_000, BlockTag::Latest)), Ok(SimulationResult::Reverted(vec![])));
        // Success.
        script(|_, _, params| {
            assert_eq!(params[0]["from"], json!(hex0x(&[1; 20])));
            Ok(ok_body(json!("0x0102")))
        });
        assert_eq!(block_on(simulate_call(&[1; 20], &ADDR, &[], 300_000, BlockTag::Latest)), Ok(SimulationResult::Ok(vec![1, 2])));
        // Two providers say revert, one says success: revert wins by quorum.
        script(|p, _, _| if p == 0 { Ok(ok_body(json!("0x01"))) } else { Ok(err_body(3, "reverted", Some(json!("0xaa")))) });
        assert_eq!(block_on(simulate_call(&[1; 20], &ADDR, &[], 300_000, BlockTag::Latest)), Ok(SimulationResult::Reverted(vec![0xaa])));
        // Revert and success split 1/1/1 with different revert payloads: no quorum.
        script(|p, _, _| match p {
            0 => Ok(ok_body(json!("0x01"))),
            1 => Ok(err_body(3, "reverted", Some(json!("0xaa")))),
            _ => Ok(err_body(3, "reverted", Some(json!("0xbb")))),
        });
        assert!(matches!(block_on(simulate_call(&[1; 20], &ADDR, &[], 300_000, BlockTag::Latest)), Err(RpcError::NoQuorum(_))));
        // A non-revert JSON-RPC error is a failed provider, not an answer.
        script(|_, _, _| Ok(err_body(-32005, "rate limit exceeded", None)));
        assert!(matches!(block_on(simulate_call(&[1; 20], &ADDR, &[], 300_000, BlockTag::Latest)), Err(RpcError::AllFailed(_))));
    }

    #[test]
    fn simulation_sends_the_gas_limit_and_maps_out_of_gas_wording() {
        configure(3, 2);
        // The exact limit the transaction will carry is sent as a QUANTITY.
        script(|_, m, params| {
            assert_eq!(m, "eth_call");
            assert_eq!(params[0]["gas"], json!("0x493e0"));
            Ok(ok_body(json!("0x")))
        });
        assert_eq!(block_on(simulate_call(&[1; 20], &ADDR, &[], 300_000, BlockTag::Latest)), Ok(SimulationResult::Ok(vec![])));
        // Wording observed or documented per client (anvil reply captured locally).
        for (code, msg) in [
            (-32603, "EVM error OutOfGas"),
            (-32000, "out of gas"),
            (-32000, "gas required exceeds allowance (150000)"),
            (-32000, "intrinsic gas too low"),
            (-32000, "intrinsic gas too high -- CallGasCostMoreThanGasLimit"),
        ] {
            script(move |_, _, _| Ok(err_body(code, msg, None)));
            assert_eq!(
                block_on(simulate_call(&[1; 20], &ADDR, &[], 150_000, BlockTag::Latest)),
                Ok(SimulationResult::OutOfGas),
                "{msg}"
            );
        }
        // Out of gas from a quorum beats a single success.
        script(|p, _, _| if p == 0 { Ok(ok_body(json!("0x"))) } else { Ok(err_body(-32000, "out of gas", None)) });
        assert_eq!(block_on(simulate_call(&[1; 20], &ADDR, &[], 150_000, BlockTag::Latest)), Ok(SimulationResult::OutOfGas));
        // An unrelated JSON-RPC error is still a failed provider, not an answer.
        script(|_, _, _| Ok(err_body(-32005, "rate limited", None)));
        assert!(matches!(block_on(simulate_call(&[1; 20], &ADDR, &[], 300_000, BlockTag::Latest)), Err(RpcError::AllFailed(_))));
    }

    // ----- logs -------------------------------------------------------------------

    #[test]
    fn get_logs_sends_the_filter_parses_sorts_and_preserves_removed() {
        configure(3, 2);
        let mut removed = sample_log(12, 1);
        removed.removed = true;
        let logs = [sample_log(12, 5), sample_log(10, 2), removed.clone(), sample_log(10, 0)];
        let wire: Vec<Value> = logs.iter().map(log_json).collect();
        script(move |_, m, params| {
            assert_eq!(m, "eth_getLogs");
            assert_eq!(params[0]["address"], json!(hex0x(&ADDR)));
            assert_eq!(params[0]["topics"], json!([hex0x(&TOPIC)]));
            assert_eq!(params[0]["fromBlock"], json!("0xa"));
            assert_eq!(params[0]["toBlock"], json!("0x10"));
            Ok(ok_body(Value::Array(wire.clone())))
        });
        let got = block_on(get_logs(&ADDR, &TOPIC, 10, 16)).unwrap();
        let order: Vec<(u64, u64)> = got.iter().map(|l| (l.block_number, l.log_index)).collect();
        assert_eq!(order, vec![(10, 0), (10, 2), (12, 1), (12, 5)]);
        assert!(got[2].removed, "removed:true is preserved for the decoder to reject");
        assert_eq!(got[2], removed);
        assert!(!got[0].removed);
        // The logs response cap is used.
        assert!(mock_calls().iter().all(|c| c.max_response_bytes == 65_536));
    }

    #[test]
    fn get_logs_omitted_removed_flag_means_false() {
        configure(1, 1);
        let mut j = log_json(&sample_log(10, 0));
        j.as_object_mut().unwrap().remove("removed");
        script(move |_, _, _| Ok(ok_body(json!([j.clone()]))));
        assert!(!block_on(get_logs(&ADDR, &TOPIC, 10, 10)).unwrap()[0].removed);
    }

    #[test]
    fn get_logs_empty_agreement_and_disagreement() {
        configure(3, 2);
        script(|_, _, _| Ok(ok_body(json!([]))));
        assert_eq!(block_on(get_logs(&ADDR, &TOPIC, 1, 5)), Ok(vec![]));
        // One provider hides the log, one differs in a field we use, one is right.
        let good = sample_log(3, 0);
        let mut other_hash = good.clone();
        other_hash.block_hash = h32(3, 1);
        let (g, o) = (log_json(&good), log_json(&other_hash));
        script(move |p, _, _| match p {
            0 => Ok(ok_body(json!([]))),
            1 => Ok(ok_body(json!([o.clone()]))),
            _ => Ok(ok_body(json!([g.clone()]))),
        });
        assert!(matches!(block_on(get_logs(&ADDR, &TOPIC, 1, 5)), Err(RpcError::NoQuorum(_))));
        // Two agree on the real log; the third omits it.
        let g = log_json(&good);
        script(move |p, _, _| if p == 0 { Ok(ok_body(json!([]))) } else { Ok(ok_body(json!([g.clone()]))) });
        assert_eq!(block_on(get_logs(&ADDR, &TOPIC, 1, 5)), Ok(vec![good]));
    }

    #[test]
    fn get_logs_provider_answers_that_break_the_filter_do_not_vote() {
        let good = sample_log(3, 0);
        let mut wrong_addr = good.clone();
        wrong_addr.address = [0xdd; 20];
        let mut wrong_topic = good.clone();
        wrong_topic.topics[0] = [0x99; 32];
        let mut outside = good.clone();
        outside.block_number = 99;
        let dup = [good.clone(), good.clone()];
        for (name, bad) in [
            ("address", json!([log_json(&wrong_addr)])),
            ("topic", json!([log_json(&wrong_topic)])),
            ("range", json!([log_json(&outside)])),
            ("duplicate position", Value::Array(dup.iter().map(log_json).collect())),
            ("not an array", json!({})),
            ("malformed log", json!([{"address": "0x01"}])),
        ] {
            configure(3, 2);
            let g = log_json(&good);
            let b = bad.clone();
            script(move |p, _, _| if p == 0 { Ok(ok_body(b.clone())) } else { Ok(ok_body(json!([g.clone()]))) });
            assert_eq!(block_on(get_logs(&ADDR, &TOPIC, 1, 5)), Ok(vec![good.clone()]), "{name}: the bad provider must be excluded, not outvote");
            let b = bad.clone();
            script(move |_, _, _| Ok(ok_body(b.clone())));
            assert!(matches!(block_on(get_logs(&ADDR, &TOPIC, 1, 5)), Err(RpcError::AllFailed(_))), "{name}");
        }
    }

    #[test]
    fn get_logs_rejects_an_inverted_range_before_any_call() {
        configure(3, 2);
        script(|_, _, _| panic!("no call expected"));
        assert!(matches!(block_on(get_logs(&ADDR, &TOPIC, 6, 5)), Err(RpcError::Decode(_))));
    }

    // ----- receipts ---------------------------------------------------------------

    fn receipt_json(tx: [u8; 32], status: &str, logs: Vec<RawLog>) -> Value {
        json!({
            "transactionHash": hex0x(&tx),
            "status": status,
            "blockNumber": "0x64",
            "blockHash": hex0x(&h32(100, 0)),
            "from": hex0x(&[0x11; 20]),
            "to": hex0x(&ADDR),
            "gasUsed": "0x5208",
            "logs": logs.iter().map(log_json).collect::<Vec<_>>(),
        })
    }

    fn receipt_log(tx: [u8; 32]) -> RawLog {
        RawLog { block_number: 100, block_hash: h32(100, 0), tx_hash: tx, ..sample_log(100, 3) }
    }

    #[test]
    fn receipt_found_null_and_status() {
        configure(3, 2);
        let tx = [0x77; 32];
        let l = receipt_log(tx);
        let ok = receipt_json(tx, "0x1", vec![l.clone()]);
        script(move |_, m, params| {
            assert_eq!(m, "eth_getTransactionReceipt");
            assert_eq!(params[0], json!(hex0x(&[0x77; 32])));
            Ok(ok_body(ok.clone()))
        });
        let r = block_on(get_receipt(&tx)).unwrap().unwrap();
        assert!(r.success);
        assert_eq!((r.block_number, r.block_hash, r.from, r.to), (100, h32(100, 0), [0x11; 20], Some(ADDR)));
        assert_eq!(r.logs, vec![l]);
        assert_eq!(mock_calls()[0].max_response_bytes, 8_192, "our own receipts are small: the default cap");
        // Reverted.
        let rv = receipt_json(tx, "0x0", vec![]);
        script(move |_, _, _| Ok(ok_body(rv.clone())));
        assert!(!block_on(get_receipt(&tx)).unwrap().unwrap().success);
        // Contract creation: no `to`.
        let mut c = receipt_json(tx, "0x1", vec![]);
        c["to"] = Value::Null;
        script(move |_, _, _| Ok(ok_body(c.clone())));
        assert_eq!(block_on(get_receipt(&tx)).unwrap().unwrap().to, None);
        // All null.
        script(|_, _, _| Ok(ok_body(Value::Null)));
        assert_eq!(block_on(get_receipt(&tx)), Ok(None));
    }

    #[test]
    fn receipt_null_versus_found_disagreement_is_no_quorum_and_majority_wins() {
        configure(3, 2);
        let tx = [0x77; 32];
        let found = receipt_json(tx, "0x1", vec![]);
        // 1 found, 2 null: null wins by quorum (a lone provider cannot invent inclusion).
        let f = found.clone();
        script(move |p, _, _| Ok(ok_body(if p == 0 { f.clone() } else { Value::Null })));
        assert_eq!(block_on(get_receipt(&tx)), Ok(None));
        // 1 found, 1 null, 1 down: split.
        let f = found.clone();
        script(move |p, _, _| match p {
            0 => Ok(ok_body(f.clone())),
            1 => Ok(ok_body(Value::Null)),
            _ => Err("down".into()),
        });
        assert!(matches!(block_on(get_receipt(&tx)), Err(RpcError::NoQuorum(_))));
        // Two found with different blocks: split.
        let mut other = found.clone();
        other["blockHash"] = json!(hex0x(&h32(100, 5)));
        let (f, o) = (found, other);
        script(move |p, _, _| match p {
            0 => Ok(ok_body(f.clone())),
            1 => Ok(ok_body(o.clone())),
            _ => Ok(ok_body(Value::Null)),
        });
        assert!(matches!(block_on(get_receipt(&tx)), Err(RpcError::NoQuorum(_))));
    }

    #[test]
    fn malformed_receipts_do_not_vote() {
        let tx = [0x77; 32];
        let good = receipt_json(tx, "0x1", vec![]);
        let mut wrong_tx = good.clone();
        wrong_tx["transactionHash"] = json!(hex0x(&[0x01; 32]));
        let mut bad_status = good.clone();
        bad_status["status"] = json!("0x2");
        let mut no_status = good.clone();
        no_status.as_object_mut().unwrap().remove("status");
        let foreign_log = receipt_json(tx, "0x1", vec![RawLog { block_number: 101, ..receipt_log(tx) }]);
        let other_tx_log = receipt_json(tx, "0x1", vec![receipt_log([0x01; 32])]);
        for (name, bad) in [
            ("tx hash", wrong_tx),
            ("status 2", bad_status),
            ("no status", no_status),
            ("log in another block", foreign_log),
            ("log of another tx", other_tx_log),
            ("not an object", json!("0x1")),
        ] {
            configure(3, 2);
            script(move |_, _, _| Ok(ok_body(bad.clone())));
            assert!(matches!(block_on(get_receipt(&tx)), Err(RpcError::AllFailed(_))), "{name}");
        }
    }

    // ----- explicit log caps, receipt fallback, chain id -----------------------------------

    #[test]
    fn get_logs_with_cap_uses_the_given_cap_bounded_to_the_canister_maximum() {
        configure(1, 1);
        script(|_, _, _| Ok(ok_body(json!([]))));
        let base = state::read_core(|c| c.config.observer.cycles_per_rpc_call);
        let mut last_cycles = 0u128;
        for cap in [1_024u64, 8_192, 65_536, 131_072, 524_288, MAX_RESPONSE_BYTES] {
            clear_mock_calls();
            block_on(get_logs_with_cap(&ADDR, &TOPIC, 1, 2, cap)).unwrap();
            let c = &mock_calls()[0];
            assert_eq!(c.max_response_bytes, cap);
            assert!(c.cycles >= last_cycles, "the attached cycles never shrink as the cap grows");
            assert!(c.cycles >= base, "at least the base amount");
            last_cycles = c.cycles;
        }
        // The largest cap attaches the largest multiple, and that covers the outcall cost
        // of a 2 MB response (about 27 200 cycles per cap byte on a 34-node subnet).
        assert_eq!(last_cycles, base * MAX_CYCLE_FACTOR as u128);
        let outcall_estimate = 27_200 * MAX_RESPONSE_BYTES as u128;
        assert!(last_cycles >= outcall_estimate * 3 / 2, "{last_cycles} must cover {outcall_estimate} with margin");
        // Above the maximum the cap is clamped, never sent as is.
        clear_mock_calls();
        block_on(get_logs_with_cap(&ADDR, &TOPIC, 1, 2, 50_000_000)).unwrap();
        assert_eq!(mock_calls()[0].max_response_bytes, MAX_RESPONSE_BYTES);
        // `get_logs` is the configured-cap wrapper.
        clear_mock_calls();
        block_on(get_logs(&ADDR, &TOPIC, 1, 2)).unwrap();
        assert_eq!(mock_calls()[0].max_response_bytes, 65_536);
        // An inverted range is refused before any call, whatever the cap.
        clear_mock_calls();
        assert!(matches!(block_on(get_logs_with_cap(&ADDR, &TOPIC, 6, 5, 8_192)), Err(RpcError::Decode(_))));
        assert!(mock_calls().is_empty());
    }

    #[test]
    fn a_receipt_is_retried_once_with_the_logs_cap_only_when_every_provider_fails() {
        configure(3, 2);
        let tx = [0x77; 32];
        let big = receipt_json(tx, "0x1", vec![]);
        // Providers fail any response above the cap they were given (simulating an
        // oversized receipt on the small cap).
        set_mock_transport({
            let b = big.clone();
            move |_, _| {
                let cap = mock_calls().last().unwrap().max_response_bytes;
                if cap < 10_000 {
                    Err("Http body exceeds size limit".into())
                } else {
                    Ok(ok_body(b.clone()))
                }
            }
        });
        let r = block_on(get_receipt(&tx)).unwrap().unwrap();
        assert!(r.success);
        let caps: Vec<u64> = mock_calls().iter().map(|c| c.max_response_bytes).collect();
        assert_eq!(caps, vec![8_192, 8_192, 8_192, 65_536, 65_536, 65_536], "small cap first, then one retry with the logs cap");

        // A split vote is NOT retried: only a total failure is.
        let f = big.clone();
        script(move |p, _, _| match p {
            0 => Ok(ok_body(f.clone())),
            1 => Ok(ok_body(Value::Null)),
            _ => Err("down".into()),
        });
        assert!(matches!(block_on(get_receipt(&tx)), Err(RpcError::NoQuorum(_))));
        assert_eq!(mock_calls().len(), 3);

        // With a logs cap that is not larger there is nothing to retry with.
        state::mutate_core(|c| c.config.observer.logs_max_response_bytes = 8_192);
        script(|_, _, _| Err("down".into()));
        assert!(matches!(block_on(get_receipt(&tx)), Err(RpcError::AllFailed(_))));
        assert_eq!(mock_calls().len(), 3);
    }

    #[test]
    fn chain_id_is_a_strict_quorum_quantity() {
        configure(3, 2);
        script(|p, m, params| {
            assert_eq!(m, "eth_chainId");
            assert_eq!(*params, json!([]));
            Ok(ok_body(json!(if p == 2 { "0x1" } else { "0x14a34" })))
        });
        assert_eq!(block_on(chain_id()), Ok(84_532));
        assert!(mock_calls().iter().all(|c| c.max_response_bytes == 8_192));
        script(|p, _, _| Ok(ok_body(json!(["0x14a34", "0x2105", "0x1"][p]))));
        assert!(matches!(block_on(chain_id()), Err(RpcError::NoQuorum(_))));
        for bad in [json!("0x014a34"), json!(84532), json!("84532"), json!("0x")] {
            script(move |_, _, _| Ok(ok_body(bad.clone())));
            assert!(matches!(block_on(chain_id()), Err(RpcError::AllFailed(_))));
        }
    }

    #[test]
    fn verify_chain_id_with_checks_a_proposed_provider_set_not_the_stored_one() {
        configure(3, 2);
        // The stored set is on Base Sepolia; the proposed one (p4, p5, p6) is not.
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::<usize>::new()));
        let s2 = seen.clone();
        set_mock_transport(move |svc, body| {
            let (m, _) = request_of(body);
            assert_eq!(m, "eth_chainId");
            let i = provider_index(svc);
            s2.borrow_mut().push(i);
            Ok(ok_body(json!(if i >= 3 { "0x2105" } else { "0x14a34" })))
        });
        let proposed: Vec<RpcServiceConfig> = (3..6).map(|i| RpcServiceConfig::Custom { url: url(i) }).collect();
        match block_on(verify_chain_id_with(&proposed, 2, 84_532)) {
            Err(RpcError::NotConfigured(m)) => assert!(m.contains("8453") && m.contains("84532"), "{m}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(*seen.borrow(), vec![3, 4, 5], "only the proposed providers are asked");
        assert_eq!(block_on(verify_chain_id_with(&proposed, 2, 8_453)), Ok(()));
        // Quorum rules: one wrong provider is outvoted, two are not.
        let s3 = seen.clone();
        set_mock_transport(move |svc, _| {
            s3.borrow_mut().push(provider_index(svc));
            Ok(ok_body(json!(if provider_index(svc) == 5 { "0x1" } else { "0x14a34" })))
        });
        assert_eq!(block_on(verify_chain_id_with(&proposed, 2, 84_532)), Ok(()));
        assert!(block_on(verify_chain_id_with(&proposed, 3, 84_532)).is_err(), "quorum 3 needs all three");
        // A malformed set is refused like the stored one.
        assert!(matches!(block_on(verify_chain_id_with(&[], 1, 84_532)), Err(RpcError::NotConfigured(_))));
        assert!(matches!(block_on(verify_chain_id_with(&proposed, 4, 84_532)), Err(RpcError::NotConfigured(_))));
        assert!(matches!(block_on(verify_chain_id_with(&proposed, 0, 84_532)), Err(RpcError::NotConfigured(_))));
        // Unreachable providers are an error, not a pass.
        script(|_, _, _| Err("down".into()));
        assert!(matches!(block_on(verify_chain_id_with(&proposed, 2, 84_532)), Err(RpcError::AllFailed(_))));
    }

    // ----- counts, balances, fees, broadcast ------------------------------------------

    #[test]
    fn transaction_count_and_balance_are_strict_quorum_quantities() {
        configure(3, 2);
        script(|_, m, params| {
            assert_eq!(params[0], json!(hex0x(&ADDR)));
            Ok(ok_body(json!(if m == "eth_getBalance" { "0xde0b6b3a7640000" } else { "0x5" })))
        });
        assert_eq!(block_on(transaction_count(&ADDR, BlockTag::Pending)), Ok(5));
        assert_eq!(block_on(balance(&ADDR, BlockTag::Latest)), Ok(1_000_000_000_000_000_000));
        // Leading zeros, empty and non-quantity answers are not answers.
        for bad in [json!("0x05"), json!("0x"), json!(5), json!("5")] {
            let b = bad.clone();
            script(move |_, _, _| Ok(ok_body(b.clone())));
            assert!(matches!(block_on(transaction_count(&ADDR, BlockTag::Latest)), Err(RpcError::AllFailed(_))), "{bad}");
        }
        // A balance too large for u128 is rejected, not truncated.
        script(|_, _, _| Ok(ok_body(json!(format!("0x1{}", "0".repeat(32))))));
        assert!(matches!(block_on(balance(&ADDR, BlockTag::Latest)), Err(RpcError::AllFailed(_))));
    }

    #[test]
    fn fee_estimate_takes_the_maximum_of_what_answered() {
        configure(3, 2);
        script(|p, m, _| {
            if m == "eth_getBlockByNumber" {
                let mut b = block_json(10, h32(10, 0));
                b["baseFeePerGas"] = json!(quantity_param([100u64, 300, 200][p]));
                if p == 2 {
                    return Err("down".into());
                }
                Ok(ok_body(b))
            } else {
                assert_eq!(m, "eth_maxPriorityFeePerGas");
                Ok(ok_body(json!(quantity_param([5u64, 9, 7][p]))))
            }
        });
        assert_eq!(block_on(fee_estimate()), Ok(FeeEstimate { base_fee_per_gas: 300, priority_fee_per_gas: 9 }));
    }

    #[test]
    fn fee_estimate_fails_only_when_a_component_has_no_answer() {
        configure(2, 2);
        script(|p, m, _| {
            if m == "eth_getBlockByNumber" {
                Ok(ok_body(block_json(10, h32(10, 0))))
            } else if p == 0 {
                Err("down".into())
            } else {
                Ok(ok_body(json!("0x3")))
            }
        });
        assert_eq!(block_on(fee_estimate()), Ok(FeeEstimate { base_fee_per_gas: 1_000_000_000, priority_fee_per_gas: 3 }));
        script(|_, m, _| if m == "eth_getBlockByNumber" { Ok(ok_body(json!({"number": "0x1", "hash": hex0x(&h32(1, 0))}))) } else { Ok(ok_body(json!("0x3"))) });
        assert!(matches!(block_on(fee_estimate()), Err(RpcError::AllFailed(_))), "a block without baseFeePerGas is not an answer");
        script(|_, _, _| Err("down".into()));
        assert!(matches!(block_on(fee_estimate()), Err(RpcError::AllFailed(_))));
    }

    #[test]
    fn send_raw_reports_each_provider_and_never_fails_the_call() {
        configure(3, 2);
        script(|p, m, params| {
            assert_eq!(m, "eth_sendRawTransaction");
            assert_eq!(params[0], json!("0x02abcd"));
            match p {
                0 => Ok(ok_body(json!(hex0x(&[0x42; 32])))),
                1 => Ok(err_body(-32000, "nonce too low via https://secret.example/key123", None)),
                _ => Err("timeout".into()),
            }
        });
        let out = block_on(send_raw(&[0x02, 0xab, 0xcd]));
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], Ok(hex0x(&[0x42; 32])));
        let e1 = out[1].clone().unwrap_err();
        assert!(e1.contains("nonce too low") && !e1.contains("secret"), "{e1}");
        assert_eq!(out[2], Err("timeout".to_string()));
        state::reset_for_tests();
        assert_eq!(block_on(send_raw(&[1])).len(), 1);
    }

    // ----- response caps and cycles -----------------------------------------------------

    #[test]
    fn response_caps_and_attached_cycles_follow_the_call_kind() {
        configure(1, 1);
        script(|_, m, _| match m {
            "eth_getBlockByNumber" => Ok(ok_body(block_json(10, h32(10, 0)))),
            "eth_getLogs" => Ok(ok_body(json!([]))),
            "eth_getTransactionReceipt" => Ok(ok_body(Value::Null)),
            "eth_call" => Ok(ok_body(json!("0x00"))),
            _ => Ok(ok_body(json!("0x1"))),
        });
        block_on(eth_call(&ADDR, &[], BlockTag::Latest)).unwrap();
        block_on(block_by_number(10)).unwrap();
        block_on(get_logs(&ADDR, &TOPIC, 1, 2)).unwrap();
        block_on(get_receipt(&[1; 32])).unwrap();
        block_on(transaction_count(&ADDR, BlockTag::Latest)).unwrap();
        let by_method = |m: &str| mock_calls().into_iter().find(|c| c.method == m).unwrap();
        let base = state::read_core(|c| c.config.observer.cycles_per_rpc_call);
        // Views and counts use the default cap and the base cycles.
        assert_eq!((by_method("eth_call").max_response_bytes, by_method("eth_call").cycles), (8_192, base));
        assert_eq!(by_method("eth_getTransactionCount").max_response_bytes, 8_192);
        // Block responses list every transaction hash, so they use the larger cap, and
        // the attached cycles scale with it (the unused part is refunded).
        assert_eq!((by_method("eth_getBlockByNumber").max_response_bytes, by_method("eth_getBlockByNumber").cycles), (65_536, base * 8));
        assert_eq!((by_method("eth_getLogs").max_response_bytes, by_method("eth_getLogs").cycles), (65_536, base * 8));
        assert_eq!(by_method("eth_getTransactionReceipt").max_response_bytes, 8_192);
        // The scaling is bounded.
        state::mutate_core(|c| c.config.observer.logs_max_response_bytes = 2_000_000);
        clear_mock_calls();
        block_on(get_logs(&ADDR, &TOPIC, 1, 2)).unwrap();
        assert_eq!(mock_calls()[0].cycles, base * MAX_CYCLE_FACTOR as u128);
        // A block cap is never below the default cap.
        state::mutate_core(|c| {
            c.config.observer.logs_max_response_bytes = 1_024;
            c.config.observer.default_max_response_bytes = 4_096;
        });
        clear_mock_calls();
        block_on(block_by_number(10)).unwrap();
        assert_eq!(mock_calls()[0].max_response_bytes, 4_096);
    }

    #[test]
    fn calls_are_sequential_in_provider_order() {
        configure(3, 2);
        script(|_, _, _| Ok(ok_body(json!("0x1"))));
        block_on(balance(&ADDR, BlockTag::Latest)).unwrap();
        assert_eq!(methods().iter().map(|(p, _)| *p).collect::<Vec<_>>(), vec![0, 1, 2]);
    }
}
