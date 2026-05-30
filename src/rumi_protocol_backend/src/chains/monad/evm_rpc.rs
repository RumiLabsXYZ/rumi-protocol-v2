//! EVM RPC wrapper for Monad chain state reads and signed-tx broadcasting.
//!
//! Design: calls the EVM RPC canister (`7hfb6-caaaa-aaaar-qadga-cai`) via the
//! generic `request` escape-hatch method using `call_with_payment128`.  This
//! avoids the `evm_rpc_client` crate (which requires ic-cdk 0.19, incompatible
//! with this project's ic-cdk 0.12 pin).  JSON-RPC responses are parsed with
//! `serde_json` (already a workspace dep).
//!
//! Cycle cost: `EVM_RPC_CALL_CYCLES` (2_000_000_000) per request.  This is
//! intentionally generous — the actual HTTPS-outcall cost depends on response
//! size and subnet node count but is typically in the hundreds-of-millions
//! range.  The constant is marked tunable; a developer-gated setter can be
//! added once we have production measurements.
//!
//! Provider fallback: `call_evm_rpc` iterates over the chain's configured
//! `rpc_endpoints` in order and returns the first `Ok` response; on all-fail
//! it returns the last error.
//!
//! Pure parsers (`parse_hex_quantity`, `BurnLog::from_raw`,
//! `MintLog::from_raw`) are unit-tested in `tests_evm_rpc`.  The async
//! network functions are covered by the Task 17 PocketIC mock.
//!
//! Candid types (`RpcApi`, `RpcService`, `RequestResult`, `RpcError`) are
//! defined inline here from the live .did (verified 2026-05-28 by fetching
//! `7hfb6-caaaa-aaaar-qadga-cai candid:service`).

use candid::{CandidType, Deserialize, Principal};
use ic_canister_log::log;
use crate::chains::config::ChainId;
use crate::state::read_state;
use crate::logs::DEBUG;

// ─── Cycle cost ─────────────────────────────────────────────────────────────

/// Cycles attached per `request` call to the EVM RPC canister.
///
/// Tunable constant.  The canister's HTTPS-outcall forwarding cost is roughly:
///   base(49_140_000) + 5_200*request_bytes + 10_400*response_bytes,
/// scaled by the number of nodes in the subnet (13 for the NNS app subnet).
/// For a typical 200-byte request and 2 KB response on a 13-node subnet that
/// is approximately 400M cycles.  2B gives ~5x headroom and matches the
/// approach used for the XRC `get_exchange_rate` call.
pub const EVM_RPC_CALL_CYCLES: u128 = 2_000_000_000;

/// Maximum response bytes requested from the EVM RPC canister per call.
/// Sized to cover typical JSON-RPC responses; `eth_getLogs` responses can be
/// large — callers using a narrow block range stay within this limit.
const EVM_RPC_MAX_RESPONSE_BYTES: u64 = 8192;

// ─── Topic0 constants ────────────────────────────────────────────────────────

/// keccak256("Burn(uint256,address,uint256)")
/// Computed via pycryptodome Keccak-256 (verified against the Transfer
/// event well-known hash to confirm the implementation is correct).
pub const BURN_EVENT_TOPIC0: &str =
    "0xe1b6e34006e9871307436c226f232f9c5e7690c1d2c4f4adda4f607a75a9beca";

/// keccak256("Mint(uint256,address,uint256)")
pub const MINT_EVENT_TOPIC0: &str =
    "0x4e3883c75cc9c752bb1db2e406a822e4a75067ae77ad9a0a4d179f2709b9e1f6";

// ─── Candid types for the EVM RPC canister `request` method ─────────────────
//
// Source: live .did of `7hfb6-caaaa-aaaar-qadga-cai` (fetched 2026-05-28).
// We define only the minimal surface needed for `request` + `RequestResult`.
// The full .did has many more variants (EthMainnet, EthSepolia, etc.) which
// we do not need since Monad is always addressed via the `Custom` variant.

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

/// Minimal `RpcService` — only the `Custom` variant is needed for Monad
/// (all other variants address built-in chains by name).
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum RpcService {
    Custom(RpcApi),
    // Other built-in variants exist in the .did but are unused here.
    // Candid decoding is flexible: we only need to encode Custom.
}

/// Mirrors the live .did `JsonRpcError` record.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

/// Mirrors the live .did `ProviderError` variant.
/// `TooFewCycles` carries an anonymous record `{ expected: nat; received: nat }`.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TooFewCyclesRecord {
    pub expected: candid::Nat,
    pub received: candid::Nat,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ProviderError {
    TooFewCycles(TooFewCyclesRecord),
    MissingRequiredProvider,
    ProviderNotFound,
    NoPermission,
    InvalidRpcConfig(String),
}

/// Mirrors the live .did `RejectionCode` variant (fieldless).
///
/// The EVM RPC canister returns `IcError { code: RejectionCode; ... }` where
/// `RejectionCode` is a Candid variant.  Previously this was declared as `u32`
/// which caused a Candid decode trap whenever the canister returned an
/// `HttpOutcallError::IcError` (e.g. during no-consensus scenarios).
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

/// Mirrors the live .did `HttpOutcallError` variant.
/// `IcError` carries `{ code: RejectionCode; message: text }`.
/// `InvalidHttpJsonRpcResponse` carries `{ status: nat16; body: text; parsingError: opt text }`.
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
pub enum ValidationError {
    Custom(String),
    InvalidHex(String),
}

/// Full `RpcError` as in the live .did.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum RpcError {
    JsonRpcError(JsonRpcError),
    ProviderError(ProviderError),
    ValidationError(ValidationError),
    HttpOutcallError(HttpOutcallError),
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// `RequestResult` from the EVM RPC canister `request` method.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum RequestResult {
    Ok(String),
    Err(RpcError),
}

// ─── Pure parsers ─────────────────────────────────────────────────────────────

/// Parse a 0x-prefixed hex string into a `u128`.
///
/// Strips the leading `"0x"` prefix (case-insensitive) and then interprets the
/// remainder as a big-endian hex integer.  Returns `Err` on malformed input.
pub fn parse_hex_quantity(s: &str) -> Result<u128, String> {
    let hex = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .ok_or_else(|| format!("missing 0x prefix: {:?}", s))?;
    u128::from_str_radix(hex, 16)
        .map_err(|e| format!("invalid hex quantity {:?}: {}", s, e))
}

// ─── BurnLog ─────────────────────────────────────────────────────────────────

/// Parsed representation of a `Burn(uint256 vault_id, address burner,
/// uint256 amount)` log emitted by the icUSD EVM contract on Monad.
///
/// ABI layout:
///   topics[0] = BURN_EVENT_TOPIC0
///   topics[1] = vault_id  (indexed, padded to 32 bytes)
///   topics[2] = burner    (indexed, EVM address padded to 32 bytes)
///   data      = amount    (uint256, 32 bytes)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BurnLog {
    pub vault_id: u64,
    pub amount_e8s: u128,
    pub tx_hash: String,
    pub block_number: u64,
}

impl BurnLog {
    /// Decode a `BurnLog` from raw log fields.
    ///
    /// `topics` must have at least 3 entries; `data` is the ABI-encoded amount.
    /// topic0 comparison is case-insensitive (RPC responses vary in
    /// EIP-55 mixed-case vs lowercase; our constant is lowercase).
    pub fn from_raw(
        topics: &[String],
        data: &str,
        tx_hash: &str,
        block_number: u64,
    ) -> Result<Self, String> {
        if topics.len() < 3 {
            return Err(format!(
                "BurnLog: expected >=3 topics, got {}",
                topics.len()
            ));
        }
        if !topics[0].eq_ignore_ascii_case(BURN_EVENT_TOPIC0) {
            return Err(format!(
                "BurnLog: wrong topic0: expected {} got {}",
                BURN_EVENT_TOPIC0, topics[0]
            ));
        }
        let vault_id_raw = parse_hex_quantity(&topics[1])?;
        if vault_id_raw > u64::MAX as u128 {
            return Err(format!("BurnLog: vault_id overflow: {}", vault_id_raw));
        }
        let amount_e8s = parse_hex_quantity(data)?;
        Ok(BurnLog {
            vault_id: vault_id_raw as u64,
            amount_e8s,
            tx_hash: tx_hash.to_string(),
            block_number,
        })
    }
}

/// Convenience free fn delegating to `BurnLog::from_raw`.
pub fn decode_burn_log(
    topics: &[String],
    data: &str,
    tx_hash: &str,
    block_number: u64,
) -> Result<BurnLog, String> {
    BurnLog::from_raw(topics, data, tx_hash, block_number)
}

// ─── MintLog ─────────────────────────────────────────────────────────────────

/// Parsed representation of a `Mint(uint256 vault_id, address recipient,
/// uint256 amount)` log.
///
/// ABI layout:
///   topics[0] = MINT_EVENT_TOPIC0
///   topics[1] = vault_id   (indexed)
///   topics[2] = recipient  (indexed, EVM address padded to 32 bytes)
///   data      = amount     (uint256)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MintLog {
    pub vault_id: u64,
    /// EVM address of the mint recipient (0x-prefixed, lowercase).
    pub recipient: String,
    pub amount_e8s: u128,
    pub tx_hash: String,
    pub block_number: u64,
}

impl MintLog {
    /// Decode a `MintLog` from raw log fields.
    pub fn from_raw(
        topics: &[String],
        data: &str,
        tx_hash: &str,
        block_number: u64,
    ) -> Result<Self, String> {
        if topics.len() < 3 {
            return Err(format!(
                "MintLog: expected >=3 topics, got {}",
                topics.len()
            ));
        }
        if !topics[0].eq_ignore_ascii_case(MINT_EVENT_TOPIC0) {
            return Err(format!(
                "MintLog: wrong topic0: expected {} got {}",
                MINT_EVENT_TOPIC0, topics[0]
            ));
        }
        let vault_id_raw = parse_hex_quantity(&topics[1])?;
        if vault_id_raw > u64::MAX as u128 {
            return Err(format!("MintLog: vault_id overflow: {}", vault_id_raw));
        }
        // Recipient address: last 20 bytes of the 32-byte padded topic2.
        // We store the full 32-byte topic as a 0x-prefixed hex string,
        // but callers can extract the address from the last 40 hex chars.
        let recipient = {
            let raw = topics[2]
                .strip_prefix("0x")
                .or_else(|| topics[2].strip_prefix("0X"))
                .unwrap_or(&topics[2]);
            // Ethereum addresses are 20 bytes (40 hex chars); topic is padded to 32 bytes (64 hex chars).
            let addr_hex = if raw.len() >= 40 {
                &raw[raw.len() - 40..]
            } else {
                raw
            };
            format!("0x{}", addr_hex.to_lowercase())
        };
        let amount_e8s = parse_hex_quantity(data)?;
        Ok(MintLog {
            vault_id: vault_id_raw as u64,
            recipient,
            amount_e8s,
            tx_hash: tx_hash.to_string(),
            block_number,
        })
    }
}

/// Convenience free fn delegating to `MintLog::from_raw`.
pub fn decode_mint_log(
    topics: &[String],
    data: &str,
    tx_hash: &str,
    block_number: u64,
) -> Result<MintLog, String> {
    MintLog::from_raw(topics, data, tx_hash, block_number)
}

// ─── Internal helpers ────────────────────────────────────────────────────────

/// Returns the EVM RPC canister principal.
///
/// Uses the developer-gated `evm_rpc_principal_override` from State when set
/// (enables PocketIC and staging to point at a mock canister).  Falls back to
/// the production canister on the IC app subnet.
fn evm_rpc_principal() -> Principal {
    read_state(|s| s.evm_rpc_override())
        .unwrap_or_else(|| {
            Principal::from_text("7hfb6-caaaa-aaaar-qadga-cai")
                .expect("static EVM RPC principal is valid")
        })
}

/// Send a raw JSON-RPC payload to the EVM RPC canister using the `request`
/// escape-hatch method.  Tries each of the chain's configured RPC endpoints in
/// order and returns the first successful `Ok` inner text.  On all-fail returns
/// the last error string.
async fn call_evm_rpc(chain: ChainId, json_payload: &str) -> Result<String, String> {
    let endpoints: Vec<String> = read_state(|s| {
        s.multi_chain
            .chain_configs
            .get(&chain)
            .map(|c| c.rpc_endpoints.clone())
            .unwrap_or_default()
    });

    if endpoints.is_empty() {
        return Err(format!("no RPC endpoints configured for chain {:?}", chain));
    }

    let canister = evm_rpc_principal();
    let mut last_err = String::new();

    for url in &endpoints {
        let rpc_service = RpcService::Custom(RpcApi {
            url: url.clone(),
            headers: None,
        });

        let result: Result<(RequestResult,), _> =
            ic_cdk::api::call::call_with_payment128(
                canister,
                "request",
                (rpc_service, json_payload.to_string(), EVM_RPC_MAX_RESPONSE_BYTES),
                EVM_RPC_CALL_CYCLES,
            )
            .await;

        match result {
            Ok((RequestResult::Ok(text),)) => {
                log!(DEBUG, "[evm_rpc] call ok via {}", url);
                return Ok(text);
            }
            Ok((RequestResult::Err(rpc_err),)) => {
                last_err = format!("RPC error from {}: {:?}", url, rpc_err);
                log!(DEBUG, "[evm_rpc] provider error via {}: {:?}", url, rpc_err);
            }
            Err((code, msg)) => {
                last_err = format!("call error to {} ({:?}): {}", url, code, msg);
                log!(DEBUG, "[evm_rpc] call error via {}: {:?} {}", url, code, msg);
            }
        }
    }

    Err(last_err)
}

// ─── JSON-RPC request ID counter ─────────────────────────────────────────────

use std::sync::atomic::{AtomicU64, Ordering};
static RPC_ID: AtomicU64 = AtomicU64::new(1);

fn next_rpc_id() -> u64 {
    RPC_ID.fetch_add(1, Ordering::Relaxed)
}

// ─── Public async interface ──────────────────────────────────────────────────

/// Returns `(latest_block, finalized_block)` for the given chain.
///
/// Tries `eth_getBlockByNumber("finalized", ...)` first.  If the provider
/// does not support the `finalized` tag (returns an error), falls back to
/// `latest.saturating_sub(finality_depth)` from the chain config.
pub async fn fetch_block_numbers(chain: ChainId) -> Result<(u64, u64), String> {
    // Fetch latest block number
    let latest_payload = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":{}}}"#,
        next_rpc_id()
    );
    let latest_text = call_evm_rpc(chain, &latest_payload).await?;
    let latest_val: serde_json::Value = serde_json::from_str(&latest_text)
        .map_err(|e| format!("eth_blockNumber parse error: {}", e))?;
    let latest_hex = latest_val["result"]
        .as_str()
        .ok_or_else(|| format!("eth_blockNumber: missing result in {:?}", latest_text))?;
    let latest = parse_hex_quantity(latest_hex)? as u64;

    // Try "finalized" tag; fall back to latest - finality_depth on failure.
    let finalized_payload = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_getBlockByNumber","params":["finalized",false],"id":{}}}"#,
        next_rpc_id()
    );
    let finalized = match call_evm_rpc(chain, &finalized_payload).await {
        Ok(text) => {
            let val: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| format!("eth_getBlockByNumber(finalized) parse: {}", e))?;
            if val["result"].is_null() || val["error"].is_object() {
                // Provider doesn't support finalized tag; use depth fallback.
                let depth = read_state(|s| {
                    s.multi_chain
                        .chain_configs
                        .get(&chain)
                        .map(|c| c.finality_depth as u64)
                        .unwrap_or(64)
                });
                latest.saturating_sub(depth)
            } else {
                let number_hex = val["result"]["number"]
                    .as_str()
                    .ok_or_else(|| format!("finalized block missing number field"))?;
                parse_hex_quantity(number_hex)? as u64
            }
        }
        Err(_) => {
            let depth = read_state(|s| {
                s.multi_chain
                    .chain_configs
                    .get(&chain)
                    .map(|c| c.finality_depth as u64)
                    .unwrap_or(64)
            });
            latest.saturating_sub(depth)
        }
    };

    Ok((latest, finalized))
}

/// Returns the ETH/native balance (in wei) of `address` at the latest block.
pub async fn get_balance(chain: ChainId, address: &str) -> Result<u128, String> {
    let payload = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_getBalance","params":[{:?},"latest"],"id":{}}}"#,
        address,
        next_rpc_id()
    );
    let text = call_evm_rpc(chain, &payload).await?;
    let val: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("eth_getBalance parse: {}", e))?;
    let hex = val["result"]
        .as_str()
        .ok_or_else(|| format!("eth_getBalance: missing result in {:?}", text))?;
    parse_hex_quantity(hex)
}

/// Returns the transaction count (nonce) of `address` at the latest block.
pub async fn get_transaction_count(chain: ChainId, address: &str) -> Result<u64, String> {
    let payload = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_getTransactionCount","params":[{:?},"latest"],"id":{}}}"#,
        address,
        next_rpc_id()
    );
    let text = call_evm_rpc(chain, &payload).await?;
    let val: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("eth_getTransactionCount parse: {}", e))?;
    let hex = val["result"]
        .as_str()
        .ok_or_else(|| format!("eth_getTransactionCount: missing result in {:?}", text))?;
    Ok(parse_hex_quantity(hex)? as u64)
}

/// Returns logs matching `(contract, topic0, fromBlock..toBlock)`.
///
/// Each entry in the returned `Vec` is `(topics, data, txHash, blockNumber)`.
/// The caller is responsible for decoding via `decode_burn_log` /
/// `decode_mint_log` etc.
pub async fn get_logs(
    chain: ChainId,
    contract: &str,
    topic0: &str,
    from_block: u64,
    to_block: u64,
) -> Result<Vec<(Vec<String>, String, String, u64)>, String> {
    let payload = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_getLogs","params":[{{"address":{:?},"topics":[{:?}],"fromBlock":"0x{:x}","toBlock":"0x{:x}"}}],"id":{}}}"#,
        contract,
        topic0,
        from_block,
        to_block,
        next_rpc_id()
    );
    let text = call_evm_rpc(chain, &payload).await?;
    let val: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("eth_getLogs parse: {}", e))?;

    if let Some(err) = val.get("error") {
        return Err(format!("eth_getLogs RPC error: {}", err));
    }

    let logs = val["result"]
        .as_array()
        .ok_or_else(|| format!("eth_getLogs: result is not an array in {:?}", text))?;

    let mut out = Vec::with_capacity(logs.len());
    for entry in logs {
        let topics: Vec<String> = entry["topics"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let data = entry["data"].as_str().unwrap_or("0x").to_string();
        let tx_hash = entry["transactionHash"].as_str().unwrap_or("0x").to_string();
        let block_number_hex = entry["blockNumber"].as_str().unwrap_or("0x0");
        let block_number = parse_hex_quantity(block_number_hex)? as u64;
        out.push((topics, data, tx_hash, block_number));
    }
    Ok(out)
}

/// Returns `None` if the transaction is still pending, or `Some((success,
/// block_number))` once mined, where `success` is `true` iff status == 0x1.
pub async fn get_transaction_receipt(
    chain: ChainId,
    tx_hash: &str,
) -> Result<Option<(bool, u64)>, String> {
    let payload = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_getTransactionReceipt","params":[{:?}],"id":{}}}"#,
        tx_hash,
        next_rpc_id()
    );
    let text = call_evm_rpc(chain, &payload).await?;
    let val: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("eth_getTransactionReceipt parse: {}", e))?;

    if val["result"].is_null() {
        return Ok(None); // transaction still pending
    }

    let status_hex = val["result"]["status"].as_str().unwrap_or("0x0");
    let success = parse_hex_quantity(status_hex)? == 1;
    let block_number_hex = val["result"]["blockNumber"]
        .as_str()
        .ok_or_else(|| format!("receipt missing blockNumber in {:?}", text))?;
    let block_number = parse_hex_quantity(block_number_hex)? as u64;
    Ok(Some((success, block_number)))
}

/// Broadcasts a signed raw transaction.  Returns the transaction hash on
/// success.
pub async fn send_raw_transaction(chain: ChainId, raw_tx_hex: &str) -> Result<String, String> {
    let payload = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_sendRawTransaction","params":[{:?}],"id":{}}}"#,
        raw_tx_hex,
        next_rpc_id()
    );
    let text = call_evm_rpc(chain, &payload).await?;
    let val: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("eth_sendRawTransaction parse: {}", e))?;

    if let Some(err) = val.get("error") {
        return Err(format!("eth_sendRawTransaction RPC error: {}", err));
    }

    val["result"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| format!("eth_sendRawTransaction: missing result in {:?}", text))
}

/// Returns `(base_fee_wei, priority_fee_wei)` for gas estimation.
///
/// Unit: **wei** (not gwei).  Uses `eth_gasPrice` as a simple heuristic;
/// splits the result 90/10 into base_fee/priority_fee.  If `eth_gasPrice`
/// fails, returns a conservative default of (1_000_000_000 wei = 1 gwei,
/// 100_000_000 wei = 0.1 gwei) and logs the failure.
///
/// A more accurate implementation can switch to `eth_feeHistory` once
/// production gas data is available from Monad testnet.
pub async fn fetch_fees(chain: ChainId) -> Result<(u128, u128), String> {
    let payload = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_gasPrice","params":[],"id":{}}}"#,
        next_rpc_id()
    );
    match call_evm_rpc(chain, &payload).await {
        Ok(text) => {
            let val: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| format!("eth_gasPrice parse: {}", e))?;
            let hex = val["result"]
                .as_str()
                .ok_or_else(|| format!("eth_gasPrice: missing result in {:?}", text))?;
            let gas_price = parse_hex_quantity(hex)?;
            // Simple split: 90% base, 10% priority tip.
            let base_fee = gas_price * 9 / 10;
            let priority_fee = gas_price - base_fee;
            Ok((base_fee, priority_fee))
        }
        Err(e) => {
            log!(
                DEBUG,
                "[evm_rpc] fetch_fees failed for chain {:?}: {}; using default",
                chain,
                e
            );
            // Conservative default: 1 gwei base + 0.1 gwei priority.
            Ok((1_000_000_000, 100_000_000))
        }
    }
}
