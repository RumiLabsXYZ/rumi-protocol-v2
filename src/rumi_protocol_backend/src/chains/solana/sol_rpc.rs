//! Hand-rolled SOL RPC canister wrapper (reads only in M1). Mirrors
//! `chains::monad::evm_rpc`: raw `call_with_payment128` with candid types
//! hand-mirrored from the live .did (`canister/sol_rpc_canister.did`). We avoid
//! `sol_rpc_client` / `sol_rpc_types` (they require ic-cdk 0.20; this project is
//! pinned to 0.12, the same wall Monad hit with `evm_rpc_client`).
//!
//! All reads go through the generic `jsonRequest` escape hatch (returns the
//! provider's JSON-RPC text), parsed with `serde_json` (exactly like the Monad
//! `request` path). The candid surface is therefore just the shared
//! `RequestResult` / `MultiRequestResult` / `RpcError` types, reused by every
//! method, rather than a typed result per method.
//!
//! Consensus: reads demand agreement. `Consistent(Ok)` only; `Consistent(Err)`
//! and `Inconsistent` both => Err (playbook #4). Reads use commitment
//! `finalized`.
//!
//! Decode safety (playbook #3): the bulky variant arms we never inspect on the
//! success path (`Inconsistent`'s per-provider vector, and the `ProviderError` /
//! `HttpOutcallError` payloads) are typed as `candid::Reserved` (the candid top
//! type), so the whole-type-table subtype check passes without enumerating the
//! large `SupportedProvider` variant. We still surface `JsonRpcError`
//! (code+message) and `ValidationError` text for diagnostics.

use candid::{CandidType, Deserialize, Principal, Reserved};
use crate::state::read_state;

/// Production SOL RPC canister principal (fiduciary subnet). VERIFY against the
/// live repo before mainnet; the developer-gated state override
/// (`sol_rpc_principal_override`) points at a mock in PocketIC / staging.
const SOL_RPC_PRINCIPAL: &str = "tghme-zyaaa-aaaar-qarca-cai";

/// Cycles attached per SOL RPC call. The docs suggest ~1-5B per request; 10B is
/// generous headroom and unused cycles are refunded.
pub const SOL_RPC_CALL_CYCLES: u128 = 10_000_000_000;

// ─── Request-side candid types (mirror canister/sol_rpc_canister.did) ────────

#[derive(CandidType, Clone, Debug)]
pub enum SolanaCluster {
    Mainnet,
    Devnet,
    Testnet,
}

#[derive(CandidType, Clone, Debug)]
pub enum RpcSources {
    Default(SolanaCluster),
    // `Custom(vec RpcSource)` exists in the .did but is unused in M1.
}

#[derive(CandidType, Clone, Debug)]
pub enum ConsensusStrategy {
    Equality,
    Threshold { total: Option<u8>, min: u8 },
}

/// Field names match the .did exactly (camelCase) so candid encodes them
/// correctly without relying on a rename attribute.
#[derive(CandidType, Clone, Debug)]
#[allow(non_snake_case)]
pub struct RpcConfig {
    pub responseSizeEstimate: Option<u64>,
    pub responseConsensus: Option<ConsensusStrategy>,
}

// ─── Response-side candid types ──────────────────────────────────────────────

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

/// Mirrors the live `RpcError` variant. `ProviderError` / `HttpOutcallError`
/// payloads are decoded as `Reserved` (we never read their details here, and
/// enumerating `SupportedProvider` / `RejectionCode` buys nothing for M1 reads).
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum RpcError {
    JsonRpcError(JsonRpcError),
    ProviderError(Reserved),
    ValidationError(String),
    HttpOutcallError(Reserved),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum RequestResult {
    Ok(String),
    Err(RpcError),
}

/// The `Inconsistent` arm's per-provider vector is typed `Reserved`: reads
/// reject any inconsistent response without inspecting it.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum MultiRequestResult {
    Consistent(RequestResult),
    Inconsistent(Reserved),
}

// ─── Pure helpers (unit-tested) ──────────────────────────────────────────────

/// Extract the response text, demanding provider agreement.
pub fn text_from_request_result(r: MultiRequestResult) -> Result<String, String> {
    match r {
        MultiRequestResult::Consistent(RequestResult::Ok(text)) => Ok(text),
        MultiRequestResult::Consistent(RequestResult::Err(e)) => {
            Err(format!("SOL RPC error: {e:?}"))
        }
        MultiRequestResult::Inconsistent(_) => {
            Err("SOL RPC providers disagree (Inconsistent)".to_string())
        }
    }
}

/// Extract `result.value` (lamports) from a `getBalance` JSON-RPC response.
pub fn parse_balance_lamports(json: &str) -> Result<u64, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("bad json: {e}"))?;
    if let Some(err) = v.get("error") {
        return Err(format!("json-rpc error: {err}"));
    }
    v.get("result")
        .and_then(|r| r.get("value"))
        .and_then(|val| val.as_u64())
        .ok_or_else(|| format!("missing result.value in response: {json}"))
}

/// Extract the SPL mint `supply` (e8s, a decimal string) from a `getAccountInfo`
/// `jsonParsed` response: `result.value.data.parsed.info.supply`.
pub fn parse_mint_supply_jsonparsed(json: &str) -> Result<u64, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("bad json: {e}"))?;
    if let Some(err) = v.get("error") {
        return Err(format!("json-rpc error: {err}"));
    }
    if v.pointer("/result/value").map(|x| x.is_null()).unwrap_or(true) {
        return Err(format!("mint account not found: {json}"));
    }
    let supply_str = v
        .pointer("/result/value/data/parsed/info/supply")
        .and_then(|s| s.as_str())
        .ok_or_else(|| format!("missing result.value.data.parsed.info.supply: {json}"))?;
    supply_str
        .parse::<u64>()
        .map_err(|e| format!("bad supply '{supply_str}': {e}"))
}

// ─── Async network calls ─────────────────────────────────────────────────────

fn sol_rpc_principal() -> Principal {
    read_state(|s| s.sol_rpc_override())
        .unwrap_or_else(|| Principal::from_text(SOL_RPC_PRINCIPAL).expect("valid SOL RPC principal"))
}

/// Send a JSON-RPC payload via the SOL RPC canister's `jsonRequest` escape hatch
/// (devnet cluster, Equality consensus). Returns the provider's response text.
async fn json_request(payload: &str) -> Result<String, String> {
    let sources = RpcSources::Default(SolanaCluster::Devnet);
    let config: Option<RpcConfig> = Some(RpcConfig {
        responseSizeEstimate: None,
        responseConsensus: Some(ConsensusStrategy::Equality),
    });
    let result: Result<(MultiRequestResult,), _> = ic_cdk::api::call::call_with_payment128(
        sol_rpc_principal(),
        "jsonRequest",
        (sources, config, payload.to_string()),
        SOL_RPC_CALL_CYCLES,
    )
    .await;
    match result {
        Ok((multi,)) => text_from_request_result(multi),
        Err((code, msg)) => Err(format!("jsonRequest call error {code:?}: {msg}")),
    }
}

/// Read a SOL balance (lamports) at `finalized`, demanding provider agreement.
/// `pubkey` MUST be a validated base58 address (callers validate at the
/// boundary), so interpolating it into the JSON payload cannot inject.
pub async fn get_balance(pubkey: &str) -> Result<u64, String> {
    let payload = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"getBalance","params":["{}",{{"commitment":"finalized"}}]}}"#,
        pubkey
    );
    let text = json_request(&payload).await?;
    parse_balance_lamports(&text)
}

/// Read the icUSD SPL mint's on-chain `supply` (e8s) at `finalized` via
/// `getAccountInfo` with `jsonParsed` encoding.
pub async fn get_mint_supply(mint_pubkey: &str) -> Result<u64, String> {
    let payload = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"getAccountInfo","params":["{}",{{"encoding":"jsonParsed","commitment":"finalized"}}]}}"#,
        mint_pubkey
    );
    let text = json_request(&payload).await?;
    parse_mint_supply_jsonparsed(&text)
}
