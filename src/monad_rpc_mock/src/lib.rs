//! Mock EVM RPC canister for the EVM-chain PocketIC happy-path tests (Monad
//! chain 10143 and Conflux eSpace testnet chain 71).
//!
//! The mock is chain-AGNOSTIC: it serves whatever chain the backend talks to
//! through the shared EVM-RPC `request` escape-hatch (the JSON-RPC payload
//! carries no chain id). Per-chain differences the tests need are scripted via
//! the `set_*` test-control endpoints, e.g. `set_getlogs_max_range(1000)` for
//! Conflux (whose backend `getlogs_max_range` is 1000, vs Monad's 100) and
//! `set_espace_receipt_fields(true)` to attach the eSpace-only receipt fields.
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

/// A scripted log entry the mock returns from `eth_getLogs` and (when stored on
/// a `Receipt`) from `eth_getTransactionReceipt`.
#[derive(Clone, Debug)]
struct ScriptedLog {
    /// Emitting contract address (lowercased). Empty for logs pushed via
    /// `push_log`/`push_log_at` (the `eth_getLogs` path filters by the request's
    /// `address` field, not the log's own, so it does not need this). Set by
    /// `push_receipt_log` so the rendered `eth_getTransactionReceipt` log JSON
    /// includes an `"address"` field — burn-proof verification rejects logs not
    /// emitted by the configured icUSD contract.
    address: String,
    topics: Vec<String>,
    data: String,
    tx_hash: String,
    block: u64,
    /// Stable EVM log index within the transaction. Assigned explicitly via
    /// `push_log_at`, or auto-assigned as the total log count at push time by
    /// `push_log`. The same log always gets the same `logIndex` across
    /// re-scans, which is required for the dedup key `(tx_hash, log_index)` to
    /// be stable across observer re-scans of the same block range.
    log_index: u64,
}

#[derive(Clone, Debug)]
struct Receipt {
    /// status 0x1 (true) vs 0x0 (false, reverted).
    ok: bool,
    block: u64,
    /// Logs emitted by this transaction, rendered into the
    /// `eth_getTransactionReceipt` `"logs"` array. Empty for receipts created by
    /// `set_receipt` / the `eth_sendRawTransaction` auto-mine path.
    logs: Vec<ScriptedLog>,
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
    /// Scripted ERC-20 totalSupply (e8s) returned by `eth_call` of selector
    /// 0x18160ddd. Defaults to 0; a test sets it via `set_total_supply`.
    total_supply: u128,
    /// One-shot failure injection: JSON-RPC `method` -> message. When the NEXT
    /// `request` for that method arrives, the mock returns a real-wire-shaped
    /// `RpcError::HttpOutcallError(IcError{ code: SysTransient, message })`
    /// instead of a canned success, then clears the entry. Lets a test
    /// exercise the IcError decode path end-to-end (the Layer-1 trap).
    fail_next: HashMap<String, String>,
    /// Persistent failure injection: JSON-RPC `method` -> message. Like
    /// `fail_next` but NEVER cleared on hit — EVERY subsequent `request` for
    /// the given method returns the IcError. Overwritten (not merged) by a
    /// new `fail_always` call for the same method. Cleared by `clear_failures`.
    /// Used by the consensus-safe regression test to arm a permanent barrier so
    /// the test reliably catches a revert to the volatile `eth_blockNumber` read
    /// across ALL observer ticks, not just the first.
    fail_always: HashMap<String, String>,
    /// Max `eth_getLogs` block range (`toBlock - fromBlock`) the mocked provider
    /// accepts in a single query. Defaults to `MONAD_GETLOGS_MAX_RANGE` (100,
    /// Monad fidelity); a Conflux test raises it to 1000 via
    /// `set_getlogs_max_range` so the backend's chain-71 `getlogs_max_range`
    /// (1000) chunking is exercised without the Monad cap wrongly rejecting it.
    getlogs_max_range: u64,
    /// When true, `eth_getTransactionReceipt` results carry the extra Conflux
    /// eSpace receipt fields `gasFee`/`burntGasFee` so the decoder's tolerance of
    /// those (non-Monad) fields is exercised. Toggled by `set_espace_receipt_fields`.
    espace_receipt_fields: bool,
    /// Per-selector canned `eth_call` results (selector hex "0x........" lowercased
    /// -> the full 0x result blob). Lets a swap test script getReserves / token0 /
    /// balanceOf returns. Set via `set_eth_call_response`.
    eth_call_responses: HashMap<String, String>,
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
        // Monad fidelity by default; a Conflux test raises this to 1000.
        s.getlogs_max_range = MONAD_GETLOGS_MAX_RANGE;
    });
}

// ─── Provider limits ─────────────────────────────────────────────────────────

/// Max `eth_getLogs` block range the Monad testnet RPC accepts: `toBlock -
/// fromBlock` must be <= this. A difference of 101 returns HTTP 413 / JSON-RPC
/// -32614. Must match `MONAD_GETLOGS_MAX_RANGE` in the backend's
/// `chains/monad/evm_rpc.rs` so the mock's cap and the wrapper's chunk size
/// agree (a chunk produced at exactly the wrapper's max must pass here).
const MONAD_GETLOGS_MAX_RANGE: u64 = 100;

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

    // Persistent failure injection: if a test armed `fail_always` for this
    // method, return a real-wire-shaped IcError WITHOUT clearing the arming —
    // every subsequent call for this method also fails. Checked BEFORE
    // `fail_next` so a persistent barrier takes precedence.
    if let Some(msg) = SCRIPT.with(|s| s.borrow().fail_always.get(method).cloned()) {
        return RequestResult::Err(RpcError::HttpOutcallError(HttpOutcallError::IcError(
            IcErrorRecord {
                code: RejectionCode::SysTransient,
                message: msg,
            },
        )));
    }

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

    // Provider fidelity (Gate-4 getLogs bug): the Monad testnet RPC caps
    // `eth_getLogs` at a 100-block RANGE — `toBlock - fromBlock` must be <= 100;
    // a difference of 101 returns HTTP 413 with JSON-RPC code -32614
    // ("eth_getLogs is limited to a 100 range"). Empirically confirmed against
    // https://testnet-rpc.monad.xyz on 2026-05-31. The EVM RPC canister surfaces
    // that 413 to the backend as `HttpOutcallError::InvalidHttpJsonRpcResponse`.
    // The mock enforces the SAME cap so the wrapper's `get_logs` chunking is
    // exercised: an un-chunked wide-range scan fails here exactly as it does on
    // staging, and only succeeds once `get_logs` pages the range into sub-queries
    // within the cap. (Mint-confirm scans a single block `[b, b]`, diff 0, and
    // is unaffected.)
    //
    // The cap is per-instance-configurable (`set_getlogs_max_range`), defaulting
    // to `MONAD_GETLOGS_MAX_RANGE` (100). Conflux's backend `getlogs_max_range`
    // is 1000, so the Conflux test raises this to 1000; a chunk produced at
    // exactly the chain's max must pass here (the mock and the chain's cap agree).
    if method == "eth_getLogs" {
        let filter = params.get(0).cloned().unwrap_or(serde_json::Value::Null);
        let from = filter
            .get("fromBlock")
            .and_then(|v| v.as_str())
            .and_then(parse_hex_u64);
        let to = filter
            .get("toBlock")
            .and_then(|v| v.as_str())
            .and_then(parse_hex_u64);
        if let (Some(f), Some(t)) = (from, to) {
            let cap = SCRIPT.with(|s| s.borrow().getlogs_max_range);
            if t.saturating_sub(f) > cap {
                return RequestResult::Err(RpcError::HttpOutcallError(
                    HttpOutcallError::InvalidHttpJsonRpcResponse(InvalidHttpJsonRpcRecord {
                        status: 413,
                        body: format!(
                            r#"{{"jsonrpc":"2.0","id":0,"error":{{"code":-32614,"message":"eth_getLogs is limited to a {} range"}}}}"#,
                            cap
                        ),
                        parsing_error: None,
                    }),
                ));
            }
        }
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
                    Receipt { ok: true, block: fin, logs: vec![] },
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
                    Some(r) => {
                        let logs_json = r
                            .logs
                            .iter()
                            .map(|log| {
                                let topics_json = log
                                    .topics
                                    .iter()
                                    .map(|t| format!("{:?}", t))
                                    .collect::<Vec<_>>()
                                    .join(",");
                                format!(
                                    r#"{{"address":{:?},"topics":[{}],"data":{:?},"transactionHash":{:?},"blockNumber":{:?},"logIndex":{:?}}}"#,
                                    log.address,
                                    topics_json,
                                    log.data,
                                    log.tx_hash,
                                    hex_u64(log.block),
                                    hex_u64(log.log_index)
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(",");
                        // Conflux eSpace receipts carry extra fields `gasFee` and
                        // `burntGasFee` that vanilla Ethereum/Monad receipts do
                        // not. When enabled, include them so the backend's receipt
                        // parser (which reads only `status`/`blockNumber`/`logs`)
                        // is exercised against the richer eSpace wire shape and
                        // must tolerate the unknown fields.
                        let espace_extra = if script.espace_receipt_fields {
                            r#","gasFee":"0x5208","burntGasFee":"0x4e20""#
                        } else {
                            ""
                        };
                        format!(
                            r#"{{"jsonrpc":"2.0","id":{},"result":{{"status":{:?},"blockNumber":{:?}{},"logs":[{}]}}}}"#,
                            id,
                            if r.ok { "0x1" } else { "0x0" },
                            hex_u64(r.block),
                            espace_extra,
                            logs_json
                        )
                    }
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
                        r#"{{"topics":[{}],"data":{:?},"transactionHash":{:?},"blockNumber":{:?},"logIndex":{:?}}}"#,
                        topics_json,
                        log.data,
                        log.tx_hash,
                        hex_u64(log.block),
                        hex_u64(log.log_index)
                    ));
                }
                format!(
                    r#"{{"jsonrpc":"2.0","id":{},"result":[{}]}}"#,
                    id,
                    items.join(",")
                )
            }
            "eth_call" => {
                // params = [{to, data}, "0x<block>"]. Dispatch by the 4-byte
                // selector in `data`: a test can script per-selector returns (e.g.
                // getReserves 0x0902f1ac, token0 0x0dfe1681, balanceOf 0x70a08231)
                // via `set_eth_call_response`. Fall back to the scripted
                // total_supply (selector 0x18160ddd) for the observer supply gate.
                let selector = params
                    .get(0)
                    .and_then(|v| v.get("data"))
                    .and_then(|v| v.as_str())
                    .map(|d| d.chars().take(10).collect::<String>().to_lowercase())
                    .unwrap_or_default();
                if let Some(ret) = script.eth_call_responses.get(&selector) {
                    format!(r#"{{"jsonrpc":"2.0","id":{},"result":{:?}}}"#, id, ret)
                } else {
                    // Default: the ABI-encoded uint256 totalSupply.
                    format!(
                        r#"{{"jsonrpc":"2.0","id":{},"result":"0x{:064x}"}}"#,
                        id, script.total_supply
                    )
                }
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
            .insert(txhash.to_lowercase(), Receipt { ok, block, logs: vec![] });
    });
}

#[ic_cdk_macros::update]
fn set_next_send_hash(h: String) {
    SCRIPT.with(|s| {
        s.borrow_mut().next_send_hash = Some(h);
    });
}

/// Push a scripted log, auto-assigning `log_index` as the current total log
/// count. Existing tests use this; the assigned index is stable (logs are never
/// removed except by `clear_logs`), so re-scans always see the same logIndex
/// for the same log.
#[ic_cdk_macros::update]
fn push_log(topics: Vec<String>, data: String, tx_hash: String, block: u64) {
    SCRIPT.with(|s| {
        let mut s = s.borrow_mut();
        let log_index = s.logs.len() as u64;
        s.logs.push(ScriptedLog {
            address: String::new(),
            topics,
            data,
            tx_hash,
            block,
            log_index,
        });
    });
}

/// Push a scripted log with an explicit `log_index`. Used by the same-tx
/// different-log-index test to push two Burn logs with the same tx_hash but
/// distinct log indices. The `log_index` must be provided explicitly so that
/// re-scans of the same block range always return the same logIndex for the
/// same log (dedup stability requirement).
#[ic_cdk_macros::update]
fn push_log_at(topics: Vec<String>, data: String, tx_hash: String, block: u64, log_index: u64) {
    SCRIPT.with(|s| {
        s.borrow_mut().logs.push(ScriptedLog {
            address: String::new(),
            topics,
            data,
            tx_hash,
            block,
            log_index,
        });
    });
}

/// Push a scripted log onto a transaction's RECEIPT (returned by
/// `eth_getTransactionReceipt`, NOT `eth_getLogs`). Creates the receipt if it
/// does not yet exist (mined+ok at block 0; raise its block with `set_receipt`
/// or `set_blocks` + a fresh push as needed). The log's `address` is rendered
/// into the receipt JSON so burn-proof verification can reject logs from a
/// non-icUSD contract. The `log_index` is stored verbatim so the dedup key
/// `(tx_hash, log_index)` is stable across re-submits.
#[ic_cdk_macros::update]
fn push_receipt_log(
    tx_hash: String,
    address: String,
    topics: Vec<String>,
    data: String,
    log_index: u64,
) {
    SCRIPT.with(|s| {
        let mut s = s.borrow_mut();
        let key = tx_hash.to_lowercase();
        let r = s.receipts.entry(key).or_insert(Receipt {
            ok: true,
            block: 0,
            logs: vec![],
        });
        let block = r.block;
        r.logs.push(ScriptedLog {
            address: address.to_lowercase(),
            topics,
            data,
            tx_hash,
            block,
            log_index,
        });
    });
}

#[ic_cdk_macros::update]
fn clear_logs() {
    SCRIPT.with(|s| {
        s.borrow_mut().logs.clear();
    });
}

#[ic_cdk_macros::update]
fn set_total_supply(value: u128) {
    SCRIPT.with(|s| s.borrow_mut().total_supply = value);
}

/// Script a per-selector `eth_call` result. `selector` is the 4-byte selector hex
/// ("0x0902f1ac" etc.); `return_data` is the full 0x ABI-encoded result blob the
/// mock returns for any `eth_call` whose `data` starts with that selector. Lets a
/// swap test canned-respond getReserves / token0 / balanceOf.
#[ic_cdk_macros::update]
fn set_eth_call_response(selector: String, return_data: String) {
    SCRIPT.with(|s| {
        s.borrow_mut()
            .eth_call_responses
            .insert(selector.to_lowercase(), return_data);
    });
}

/// Set the max `eth_getLogs` block range (`toBlock - fromBlock`) the mock
/// accepts before returning the 413 / -32614 range error. Default is
/// `MONAD_GETLOGS_MAX_RANGE` (100). A Conflux test sets this to 1000 to match
/// chain-71's backend `getlogs_max_range`.
#[ic_cdk_macros::update]
fn set_getlogs_max_range(range: u64) {
    SCRIPT.with(|s| s.borrow_mut().getlogs_max_range = range);
}

/// Enable/disable the Conflux eSpace extra receipt fields (`gasFee`,
/// `burntGasFee`) on `eth_getTransactionReceipt` results, so a test can confirm
/// the backend receipt parser tolerates the eSpace wire shape.
#[ic_cdk_macros::update]
fn set_espace_receipt_fields(enabled: bool) {
    SCRIPT.with(|s| s.borrow_mut().espace_receipt_fields = enabled);
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

/// Arm a PERSISTENT real-wire IcError for ALL future `request` calls whose
/// JSON-RPC `method` equals `method`. Unlike `fail_next`, the arming is NEVER
/// cleared on hit — every call for this method continues to return the IcError.
/// Calling `fail_always` again for the same method overwrites the message.
/// Use `clear_failures` to lift the barrier.
///
/// Used by the consensus-safe regression test (`phase1b_observer_consensus_safe_pic`)
/// to ensure every observer tick would fail if the backend reverted to the
/// volatile `eth_blockNumber` via `request`. The fixed backend never calls
/// `eth_blockNumber` via `request` (it uses the TYPED `eth_getBlockByNumber`
/// instead), so the persistent barrier is inert for the correct implementation.
#[ic_cdk_macros::update]
fn fail_always(method: String, message: String) {
    SCRIPT.with(|s| {
        s.borrow_mut().fail_always.insert(method, message);
    });
}

/// Remove all persistent (`fail_always`) and one-shot (`fail_next`) failure
/// injections. After this call the mock resumes normal scripted responses.
#[ic_cdk_macros::update]
fn clear_failures() {
    SCRIPT.with(|s| {
        let mut s = s.borrow_mut();
        s.fail_always.clear();
        s.fail_next.clear();
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
