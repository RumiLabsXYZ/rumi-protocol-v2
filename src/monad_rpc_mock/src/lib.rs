//! Mock EVM RPC canister for the Phase 1b (Monad) PocketIC happy-path test.
//!
//! Speaks the REAL EVM RPC canister `request` escape-hatch interface so the
//! production backend wrapper (`chains/monad/evm_rpc.rs`) can talk to it
//! unchanged via `set_evm_rpc_principal`. The Candid arg/return types are
//! copied (minimally) from `evm_rpc.rs` so encode/decode line up byte-for-byte:
//!
//!   request : (RpcService, text /*json payload*/, nat64 /*max_bytes*/)
//!           -> (RequestResult)
//!
//! It parses the JSON-RPC `method` out of the `json_payload` string and returns
//! `RequestResult::Ok("<canned json-rpc response>")` shaped exactly as the
//! wrapper's parsers expect (verified against `evm_rpc.rs` lines ~343-639).
//!
//! Behavior is fully scripted by the test via the `set_*` / `push_log` /
//! `clear_logs` test-control endpoints, backed by a `thread_local!
//! RefCell<Script>` (same pattern as `src/xrc_demo/xrc_mock/src/lib.rs`).
//!
//! Supported JSON-RPC methods (and the response shape each wrapper fn parses):
//!   - eth_blockNumber              -> {"result":"0x<latest>"}
//!   - eth_getBlockByNumber         -> {"result":{"number":"0x<finalized>"}}
//!   - eth_getBalance               -> {"result":"0x<balance_wei>"}
//!   - eth_getTransactionCount      -> {"result":"0x<nonce>"}
//!   - eth_gasPrice                 -> {"result":"0x<gas_price_wei>"}  (fetch_fees)
//!   - eth_sendRawTransaction       -> {"result":"0x<tx_hash>"} + AUTO-MINE a
//!                                     successful receipt at the finalized block
//!   - eth_getTransactionReceipt    -> {"result":{"status":"0x1","blockNumber":
//!                                     "0x<block>"}} when mined, else null
//!   - eth_getLogs                  -> {"result":[<log objects>]}, filtered by
//!                                     the requested fromBlock..toBlock range
//!                                     AND topics[0] (case-insensitive), matching
//!                                     the real RPC's topic filter (M-3 fidelity)
//!
//! Build:
//!   cargo build --target wasm32-unknown-unknown --release --package monad_rpc_mock

use candid::{CandidType, Deserialize};
use std::cell::RefCell;
use std::collections::HashMap;

// ─── Candid types mirrored from chains/monad/evm_rpc.rs (must encode/decode
//     identically to the backend's local defs) ─────────────────────────────

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct EvmRpcHttpHeader {
    pub name: String,
    pub value: String,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct RpcApi {
    pub url: String,
    pub headers: Option<Vec<EvmRpcHttpHeader>>,
}

/// Only the `Custom` variant is needed — the backend wrapper always addresses
/// Monad via `RpcService::Custom(RpcApi { url, headers })`.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum RpcService {
    Custom(RpcApi),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

// ─── Full `RpcError` tree (parity with docs/evm_rpc_reference.did) ───────────
//
// The mock now mirrors the REAL `RpcError` shape (4 variants) so it can return
// a real-wire-shaped `HttpOutcallError::IcError { code: RejectionCode; .. }` —
// exactly the value the live EVM RPC canister returns when an HTTPS-outcall
// fails consensus. This lets a future test exercise that decode path
// end-to-end (it was the Layer-1 trap).

/// Mirrors the live .did `RejectionCode` variant (fieldless, 7 arms).
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum RejectionCode {
    NoError,
    CanisterError,
    SysTransient,
    DestinationInvalid,
    Unknown,
    SysFatal,
    CanisterReject,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct IcErrorRecord {
    pub code: RejectionCode,
    pub message: String,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct InvalidHttpJsonRpcRecord {
    pub status: u16,
    pub body: String,
    #[serde(rename = "parsingError")]
    pub parsing_error: Option<String>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum HttpOutcallError {
    IcError(IcErrorRecord),
    InvalidHttpJsonRpcResponse(InvalidHttpJsonRpcRecord),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct TooFewCyclesRecord {
    pub expected: candid::Nat,
    pub received: candid::Nat,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum ProviderError {
    TooFewCycles(TooFewCyclesRecord),
    MissingRequiredProvider,
    ProviderNotFound,
    NoPermission,
    InvalidRpcConfig(String),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum ValidationError {
    Custom(String),
    InvalidHex(String),
}

/// Full `RpcError` (4 variants) as in the live .did. Variant ORDER matches the
/// real `.did` (`JsonRpcError`, `ProviderError`, `ValidationError`,
/// `HttpOutcallError`) so candid tags line up with the backend's mirror.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum RpcError {
    JsonRpcError(JsonRpcError),
    ProviderError(ProviderError),
    ValidationError(ValidationError),
    HttpOutcallError(HttpOutcallError),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum RequestResult {
    Ok(String),
    Err(RpcError),
}

// ─── Typed `eth_getBlockByNumber` method types (parity with the .did) ────────
//
// The backend's consensus-safe finalized-height probe calls the TYPED
// `eth_getBlockByNumber : (RpcServices, opt RpcConfig, BlockTag) ->
// (MultiGetBlockByNumberResult)` method (a specific `Number(n)` is
// byte-identical across IC replicas). The mock implements it below.

/// PLURAL `RpcServices` (only the `Custom` arm — the backend always sends a
/// single Monad custom provider).
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum RpcServices {
    Custom {
        #[serde(rename = "chainId")]
        chain_id: u64,
        services: Vec<RpcApi>,
    },
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum BlockTag {
    Earliest,
    Safe,
    Finalized,
    Latest,
    Number(candid::Nat),
    Pending,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum ConsensusStrategy {
    Equality,
    Threshold { total: Option<u8>, min: u8 },
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct RpcConfig {
    #[serde(rename = "responseSizeEstimate")]
    pub response_size_estimate: Option<u64>,
    #[serde(rename = "responseConsensus")]
    pub response_consensus: Option<ConsensusStrategy>,
}

/// Minimal `Block` (only `number`) — matches the backend's reader. Record
/// subtyping means this still decodes a richer wire `Block`; the mock only
/// needs to ENCODE `number` here since the backend only reads `number`.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct Block {
    pub number: candid::Nat,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum GetBlockByNumberResult {
    Ok(Block),
    Err(RpcError),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum MultiGetBlockByNumberResult {
    Consistent(GetBlockByNumberResult),
    Inconsistent(Vec<(RpcService, GetBlockByNumberResult)>),
}

// ─── Scripted state ───────────────────────────────────────────────────────

/// A scripted log entry the mock returns from `eth_getLogs`.
#[derive(Clone, Debug)]
struct ScriptedLog {
    topics: Vec<String>,
    data: String,
    tx_hash: String,
    block: u64,
}

#[derive(Clone, Debug)]
struct Receipt {
    /// status 0x1 (true) vs 0x0 (false, reverted).
    ok: bool,
    block: u64,
}

#[derive(Default)]
struct Script {
    latest_block: u64,
    finalized_block: u64,
    /// address (lowercased) -> balance in wei.
    balances: HashMap<String, u128>,
    /// address (lowercased) -> nonce.
    nonces: HashMap<String, u64>,
    /// tx hash (lowercased) -> receipt.
    receipts: HashMap<String, Receipt>,
    /// gas price in wei returned by eth_gasPrice (fetch_fees). A sane default
    /// is seeded in `init` so the settlement submit path never divides by zero.
    gas_price_wei: u128,
    /// The hash eth_sendRawTransaction returns for the NEXT broadcast.
    next_send_hash: Option<String>,
    /// Scripted logs returned (range-filtered) by eth_getLogs.
    logs: Vec<ScriptedLog>,
    /// One-shot failure injection: JSON-RPC `method` -> message. When the NEXT
    /// `request` for that method arrives, the mock returns a real-wire-shaped
    /// `RpcError::HttpOutcallError(IcError{ code: SysTransient, message })`
    /// instead of a canned success, then clears the entry. Lets a test
    /// exercise the IcError decode path end-to-end (the Layer-1 trap).
    fail_next: HashMap<String, String>,
}

thread_local! {
    static SCRIPT: RefCell<Script> = RefCell::new(Script::default());
}

// ─── Init ─────────────────────────────────────────────────────────────────

#[ic_cdk_macros::init]
fn init() {
    SCRIPT.with(|s| {
        let mut s = s.borrow_mut();
        // 1 gwei default so fetch_fees has a non-zero gas price even if the
        // test never sets one.
        s.gas_price_wei = 1_000_000_000;
    });
}

// ─── JSON helpers ───────────────────────────────────────────────────────────

fn hex_u128(v: u128) -> String {
    format!("0x{:x}", v)
}

fn hex_u64(v: u64) -> String {
    format!("0x{:x}", v)
}

/// Parse a 0x-prefixed (or bare) hex quantity into a u64. Used for fromBlock /
/// toBlock parsing of eth_getLogs filters. Block tags like "latest"/"finalized"
/// are mapped to the scripted finalized block by the caller; this only handles
/// the 0x-hex numeric form.
fn parse_hex_u64(s: &str) -> Option<u64> {
    let hex = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X"))?;
    u64::from_str_radix(hex, 16).ok()
}

// ─── The real EVM RPC `request` escape-hatch ─────────────────────────────────

/// Mirrors the EVM RPC canister's generic `request` method. The backend wrapper
/// calls this via `call_with_payment128(canister, "request", (RpcService, String,
/// u64), cycles)`. We ignore the `RpcService` url (the test configures exactly
/// one endpoint; the mock serves all methods) and the `max_bytes` cap, parse the
/// JSON-RPC `method`, and return a canned response.
#[ic_cdk_macros::update]
fn request(_service: RpcService, json_payload: String, _max_response_bytes: u64) -> RequestResult {
    let payload: serde_json::Value = match serde_json::from_str(&json_payload) {
        Ok(v) => v,
        Err(e) => {
            return RequestResult::Err(RpcError::JsonRpcError(JsonRpcError {
                code: -32700,
                message: format!("mock: invalid json payload: {e}"),
            }))
        }
    };
    let method = payload["method"].as_str().unwrap_or("");
    let id = payload["id"].clone();
    let params = &payload["params"];

    // One-shot failure injection: if a test armed `fail_next` for this method,
    // return a REAL-WIRE-SHAPED IcError (the exact value the live EVM RPC
    // canister returns on a no-consensus HTTPS outcall) and clear the arming.
    if let Some(msg) = SCRIPT.with(|s| s.borrow_mut().fail_next.remove(method)) {
        return RequestResult::Err(RpcError::HttpOutcallError(HttpOutcallError::IcError(
            IcErrorRecord {
                code: RejectionCode::SysTransient,
                message: msg,
            },
        )));
    }

    let response_json: String = SCRIPT.with(|s| {
        let mut script = s.borrow_mut();
        match method {
            "eth_blockNumber" => {
                format!(
                    r#"{{"jsonrpc":"2.0","id":{},"result":{:?}}}"#,
                    id,
                    hex_u64(script.latest_block)
                )
            }
            "eth_getBlockByNumber" => {
                // params = ["finalized", false]. Return a real finalized block
                // (never null) so the wrapper's finality path uses the number.
                format!(
                    r#"{{"jsonrpc":"2.0","id":{},"result":{{"number":{:?}}}}}"#,
                    id,
                    hex_u64(script.finalized_block)
                )
            }
            "eth_getBalance" => {
                // params = [addr, "latest"].
                let addr = params
                    .get(0)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_lowercase();
                let bal = script.balances.get(&addr).copied().unwrap_or(0);
                format!(
                    r#"{{"jsonrpc":"2.0","id":{},"result":{:?}}}"#,
                    id,
                    hex_u128(bal)
                )
            }
            "eth_getTransactionCount" => {
                // params = [addr, "latest"].
                let addr = params
                    .get(0)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_lowercase();
                let nonce = script.nonces.get(&addr).copied().unwrap_or(0);
                format!(
                    r#"{{"jsonrpc":"2.0","id":{},"result":{:?}}}"#,
                    id,
                    hex_u64(nonce)
                )
            }
            "eth_gasPrice" => {
                // fetch_fees parses {"result":"0x<wei>"}.
                format!(
                    r#"{{"jsonrpc":"2.0","id":{},"result":{:?}}}"#,
                    id,
                    hex_u128(script.gas_price_wei)
                )
            }
            "eth_sendRawTransaction" => {
                // params = [rawhex]. Return the scripted next hash (default a
                // deterministic placeholder) and AUTO-MINE a successful receipt
                // for it at the current finalized block, so the confirm path
                // immediately sees it mined + final.
                let tx_hash = script
                    .next_send_hash
                    .clone()
                    .unwrap_or_else(|| "0xmocktx".to_string());
                let fin = script.finalized_block;
                script.receipts.insert(
                    tx_hash.to_lowercase(),
                    Receipt { ok: true, block: fin },
                );
                format!(
                    r#"{{"jsonrpc":"2.0","id":{},"result":{:?}}}"#,
                    id, tx_hash
                )
            }
            "eth_getTransactionReceipt" => {
                // params = [txhash]. Return a receipt when mined, else null.
                let txhash = params
                    .get(0)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_lowercase();
                match script.receipts.get(&txhash) {
                    Some(r) => format!(
                        r#"{{"jsonrpc":"2.0","id":{},"result":{{"status":{:?},"blockNumber":{:?}}}}}"#,
                        id,
                        if r.ok { "0x1" } else { "0x0" },
                        hex_u64(r.block)
                    ),
                    None => format!(r#"{{"jsonrpc":"2.0","id":{},"result":null}}"#, id),
                }
            }
            "eth_getLogs" => {
                // params = [{address, topics, fromBlock, toBlock}]. Filter the
                // scripted logs by BOTH the requested block range AND the
                // requested topics[0] — the REAL EVM RPC `eth_getLogs` is
                // topic-filtered, so the mock must be too (M-3 fidelity). Without
                // the topic filter the observer's BURN scan would also return the
                // Mint log, which the old happy-path test had to work around with
                // a `clear_logs` call. The match is case-insensitive (RPC
                // responses vary in EIP-55 mixed-case vs lowercase). A request
                // with no/empty topics filter returns all logs in range (the
                // permissive JSON-RPC default).
                let filter = params.get(0).cloned().unwrap_or(serde_json::Value::Null);
                let from = filter
                    .get("fromBlock")
                    .and_then(|v| v.as_str())
                    .and_then(parse_hex_u64);
                let to = filter
                    .get("toBlock")
                    .and_then(|v| v.as_str())
                    .and_then(parse_hex_u64);
                // The requested topic0 (first entry of the `topics` array), if any.
                let want_topic0: Option<String> = filter
                    .get("topics")
                    .and_then(|t| t.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_lowercase());

                let mut items: Vec<String> = Vec::new();
                for log in script.logs.iter() {
                    if let Some(f) = from {
                        if log.block < f {
                            continue;
                        }
                    }
                    if let Some(t) = to {
                        if log.block > t {
                            continue;
                        }
                    }
                    // Topic-filter: if the request specified topics[0], only
                    // return logs whose own topics[0] matches (case-insensitive).
                    if let Some(ref want) = want_topic0 {
                        match log.topics.first() {
                            Some(got) if got.to_lowercase() == *want => {}
                            _ => continue,
                        }
                    }
                    let topics_json = log
                        .topics
                        .iter()
                        .map(|t| format!("{:?}", t))
                        .collect::<Vec<_>>()
                        .join(",");
                    items.push(format!(
                        r#"{{"topics":[{}],"data":{:?},"transactionHash":{:?},"blockNumber":{:?}}}"#,
                        topics_json,
                        log.data,
                        log.tx_hash,
                        hex_u64(log.block)
                    ));
                }
                format!(
                    r#"{{"jsonrpc":"2.0","id":{},"result":[{}]}}"#,
                    id,
                    items.join(",")
                )
            }
            other => {
                // Unknown method: a JSON-RPC error so an unexpected wrapper call
                // is loud rather than silently mis-parsed.
                return format!(
                    r#"{{"jsonrpc":"2.0","id":{},"error":{{"code":-32601,"message":"mock: unsupported method {}"}}}}"#,
                    id, other
                );
            }
        }
    });

    RequestResult::Ok(response_json)
}

// ─── Test-control endpoints (called by the PocketIC test) ────────────────────

#[ic_cdk_macros::update]
fn set_blocks(latest: u64, finalized: u64) {
    SCRIPT.with(|s| {
        let mut s = s.borrow_mut();
        s.latest_block = latest;
        s.finalized_block = finalized;
    });
}

#[ic_cdk_macros::update]
fn set_balance(addr: String, wei: candid::Nat) {
    let wei_u128 = nat_to_u128(&wei);
    SCRIPT.with(|s| {
        s.borrow_mut().balances.insert(addr.to_lowercase(), wei_u128);
    });
}

#[ic_cdk_macros::update]
fn set_nonce(addr: String, n: u64) {
    SCRIPT.with(|s| {
        s.borrow_mut().nonces.insert(addr.to_lowercase(), n);
    });
}

#[ic_cdk_macros::update]
fn set_gas_price(wei: candid::Nat) {
    let wei_u128 = nat_to_u128(&wei);
    SCRIPT.with(|s| {
        s.borrow_mut().gas_price_wei = wei_u128;
    });
}

#[ic_cdk_macros::update]
fn set_receipt(txhash: String, ok: bool, block: u64) {
    SCRIPT.with(|s| {
        s.borrow_mut()
            .receipts
            .insert(txhash.to_lowercase(), Receipt { ok, block });
    });
}

#[ic_cdk_macros::update]
fn set_next_send_hash(h: String) {
    SCRIPT.with(|s| {
        s.borrow_mut().next_send_hash = Some(h);
    });
}

#[ic_cdk_macros::update]
fn push_log(topics: Vec<String>, data: String, tx_hash: String, block: u64) {
    SCRIPT.with(|s| {
        s.borrow_mut().logs.push(ScriptedLog {
            topics,
            data,
            tx_hash,
            block,
        });
    });
}

#[ic_cdk_macros::update]
fn clear_logs() {
    SCRIPT.with(|s| {
        s.borrow_mut().logs.clear();
    });
}

/// Arm a one-shot real-wire IcError for the NEXT `request` whose JSON-RPC
/// `method` equals `method`. The mock returns
/// `RpcError::HttpOutcallError(IcError{ code: SysTransient, message })` and
/// clears the arming. Lets a test exercise the real IcError decode path
/// end-to-end (the Layer-1 candid trap).
#[ic_cdk_macros::update]
fn fail_next(method: String, message: String) {
    SCRIPT.with(|s| {
        s.borrow_mut().fail_next.insert(method, message);
    });
}

// ─── Typed `eth_getBlockByNumber` method (consensus-safe probe target) ───────

/// Mirrors the EVM RPC canister's TYPED `eth_getBlockByNumber` method. The
/// backend's `eth_get_block_number_at` probe calls this with a specific
/// `BlockTag::Number(n)`. For `Number(n)`: if `n <= finalized_block` the block
/// exists (return `Consistent(Ok(Block{ number: n }))`); otherwise it models a
/// future block (return `Consistent(Err(JsonRpcError{ -32000, "block not
/// found" }))`). The `_services`/`_config` args are ignored (single provider).
#[ic_cdk_macros::update]
#[allow(non_snake_case)]
fn eth_getBlockByNumber(
    _services: RpcServices,
    _config: Option<RpcConfig>,
    tag: BlockTag,
) -> MultiGetBlockByNumberResult {
    match tag {
        BlockTag::Number(n) => {
            let n_u64 = u64::try_from(n.0.clone()).unwrap_or(u64::MAX);
            let finalized = SCRIPT.with(|s| s.borrow().finalized_block);
            if n_u64 <= finalized {
                MultiGetBlockByNumberResult::Consistent(GetBlockByNumberResult::Ok(Block {
                    number: candid::Nat::from(n_u64),
                }))
            } else {
                MultiGetBlockByNumberResult::Consistent(GetBlockByNumberResult::Err(
                    RpcError::JsonRpcError(JsonRpcError {
                        code: -32000,
                        message: "block not found".to_string(),
                    }),
                ))
            }
        }
        // Volatile tags are exactly what the backend stopped sending; if a test
        // ever sends one, surface it as an error rather than silently faking.
        _ => MultiGetBlockByNumberResult::Consistent(GetBlockByNumberResult::Err(
            RpcError::JsonRpcError(JsonRpcError {
                code: -32000,
                message: "mock: only Number(n) tag supported".to_string(),
            }),
        )),
    }
}

// ─── helpers ─────────────────────────────────────────────────────────────────

/// Convert a `candid::Nat` to u128 (saturating). The wei amounts in this test
/// (100 * 1e18 = 1e20) exceed u64 but fit comfortably in u128.
fn nat_to_u128(n: &candid::Nat) -> u128 {
    use std::convert::TryFrom;
    u128::try_from(n.0.clone()).unwrap_or(u128::MAX)
}
