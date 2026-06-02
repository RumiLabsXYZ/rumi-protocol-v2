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
use solana_message::Hash;
use crate::state::read_state;

/// Serialized byte length of a System nonce account's data (see
/// `parse_nonce_account_blockhash`). Mirrors `solana_system_interface`'s
/// `NONCE_STATE_SIZE` (verified against the crate source).
pub const NONCE_STATE_SIZE: usize = 80;

/// The `Initialized` value of a nonce account's `state` field (`buf[4..8]` as a
/// u32 LE). 0 is `Uninitialized` (created but not yet initialized).
const NONCE_STATE_INITIALIZED: u32 = 1;

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

/// Build the `sendTransaction` JSON-RPC payload for a base64-encoded wire tx.
/// `skipPreflight=false` keeps the provider's pre-flight simulation (catches a
/// bad blockhash / insufficient funds before the tx is forwarded). `b64` is the
/// canister's own deterministic base64 of bytes it produced, so interpolating it
/// into the JSON cannot inject (base64's alphabet has no JSON-breaking chars).
pub fn build_send_transaction_payload(b64: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"sendTransaction","params":["{}",{{"encoding":"base64","skipPreflight":false}}]}}"#,
        b64
    )
}

/// Extract the transaction signature (a base58 string) from a `sendTransaction`
/// JSON-RPC response, where the signature is `result` itself (not nested).
pub fn parse_send_transaction_signature(json: &str) -> Result<String, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("bad json: {e}"))?;
    if let Some(err) = v.get("error") {
        return Err(format!("json-rpc error: {err}"));
    }
    v.get("result")
        .and_then(|r| r.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("missing result (signature) in sendTransaction response: {json}"))
}

// ─── durable nonce helpers (pure, unit-tested) ───────────────────────────────

/// Extract and base64-decode a `getAccountInfo` (`encoding: base64`) response's
/// account data buffer: `result.value.data[0]` is the base64 string and
/// `result.value.data[1]` is the literal `"base64"`. Errs if the account is not
/// found (`result.value` is null), which for a nonce account means it has not
/// been bootstrapped yet.
pub fn parse_account_data_base64(json: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("bad json: {e}"))?;
    if let Some(err) = v.get("error") {
        return Err(format!("json-rpc error: {err}"));
    }
    if v.pointer("/result/value").map(|x| x.is_null()).unwrap_or(true) {
        return Err(format!("account not found (value is null): {json}"));
    }
    let b64 = v
        .pointer("/result/value/data/0")
        .and_then(|s| s.as_str())
        .ok_or_else(|| format!("missing result.value.data[0] (base64): {json}"))?;
    base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| format!("bad base64 account data: {e}"))
}

/// Parse a System nonce account's data buffer and return its durable-nonce
/// blockhash (the 32 bytes at offset 40). Layout (80 bytes total):
///   version: u32 LE          [0..4]
///   state:   u32 LE          [4..8]   (0 = Uninitialized, 1 = Initialized)
///   authority: Pubkey        [8..40]
///   durable_nonce/blockhash: [40..72] <- returned here
///   fee_calculator.lamports_per_signature: u64 LE [72..80]
///
/// Errs unless the buffer is exactly 80 bytes AND the state is `Initialized`
/// (1); an uninitialized account holds no usable nonce.
pub fn parse_nonce_account_blockhash(buf: &[u8]) -> Result<[u8; 32], String> {
    if buf.len() != NONCE_STATE_SIZE {
        return Err(format!(
            "nonce account data must be {NONCE_STATE_SIZE} bytes, got {}",
            buf.len()
        ));
    }
    let state = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
    if state != NONCE_STATE_INITIALIZED {
        return Err(format!(
            "nonce account is not Initialized (state = {state}, expected {NONCE_STATE_INITIALIZED})"
        ));
    }
    let mut blockhash = [0u8; 32];
    blockhash.copy_from_slice(&buf[40..72]);
    Ok(blockhash)
}

/// Extract `result.value.blockhash` (a base58 string) from a `getLatestBlockhash`
/// response and decode it to a 32-byte blockhash. Errs on a JSON-RPC error, a
/// missing field, or a decode that is not exactly 32 bytes.
pub fn parse_latest_blockhash(json: &str) -> Result<[u8; 32], String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("bad json: {e}"))?;
    if let Some(err) = v.get("error") {
        return Err(format!("json-rpc error: {err}"));
    }
    let b58 = v
        .pointer("/result/value/blockhash")
        .and_then(|s| s.as_str())
        .ok_or_else(|| format!("missing result.value.blockhash: {json}"))?;
    let bytes = bs58::decode(b58)
        .into_vec()
        .map_err(|e| format!("bad base58 blockhash '{b58}': {e}"))?;
    let arr: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| format!("blockhash must decode to 32 bytes, got {}", bytes.len()))?;
    Ok(arr)
}

/// Outcome of a `getTransaction` lookup, distilled to what `verify_deposit`
/// needs: is the tx on-chain, and did it succeed.
///
/// - `NotFound`: a null `result` (the signature is unknown at the requested
///   commitment, i.e. not yet finalized or never landed).
/// - `Confirmed { slot }`: a non-null result whose `meta.err` is null (the tx
///   landed and executed successfully); `slot` is the confirmation slot.
/// - `Failed`: a non-null result whose `meta.err` is non-null (the tx landed
///   but its execution reverted on-chain). We surface this as a DISTINCT signal
///   (rather than folding it into `NotFound`) so `verify_deposit` can return a
///   "deposit failed" error rather than a "not finalized" one, mirroring the
///   Monad adapter's reverted-vs-not-mined distinction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TxStatus {
    NotFound,
    Confirmed { slot: u64 },
    Failed,
}

/// Parse a `getTransaction` JSON-RPC response into a `TxStatus`.
///
/// A null `result` => `NotFound`. A non-null result reads `result.slot` (a bare
/// u64) and `result.meta.err`: a null `err` => `Confirmed { slot }`, a non-null
/// `err` => `Failed`. A JSON-RPC `error` member is a transport/provider failure,
/// surfaced as `Err`. A non-null result missing `slot` is malformed => `Err`.
pub fn parse_get_transaction(json: &str) -> Result<TxStatus, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("bad json: {e}"))?;
    if let Some(err) = v.get("error") {
        return Err(format!("json-rpc error: {err}"));
    }
    let result = match v.get("result") {
        // A null result, or no result member at all, means the signature is not
        // known at the requested commitment.
        None => return Ok(TxStatus::NotFound),
        Some(r) if r.is_null() => return Ok(TxStatus::NotFound),
        Some(r) => r,
    };
    // meta.err == null => success; a non-null err means the tx reverted.
    let failed = result
        .pointer("/meta/err")
        .map(|e| !e.is_null())
        .unwrap_or(false);
    if failed {
        return Ok(TxStatus::Failed);
    }
    let slot = result
        .get("slot")
        .and_then(|s| s.as_u64())
        .ok_or_else(|| format!("missing result.slot in getTransaction response: {json}"))?;
    Ok(TxStatus::Confirmed { slot })
}

/// Extract the slot number from a `getSlot` JSON-RPC response, where `result` is
/// a bare u64 (NOT nested under `result.value`).
pub fn parse_slot(json: &str) -> Result<u64, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("bad json: {e}"))?;
    if let Some(err) = v.get("error") {
        return Err(format!("json-rpc error: {err}"));
    }
    v.get("result")
        .and_then(|r| r.as_u64())
        .ok_or_else(|| format!("missing result (slot) in getSlot response: {json}"))
}

/// True iff `sig` base58-decodes to a plausible Ed25519 signature length (64
/// bytes). A Solana transaction signature is a 64-byte Ed25519 signature encoded
/// in base58, NOT a 32-byte pubkey, so this is intentionally distinct from
/// `ted25519::is_valid_solana_address` (which checks for 32 bytes). Used to
/// reject a caller-supplied signature before it is interpolated into the
/// `getTransaction` JSON payload, so a malformed value cannot inject.
pub fn is_valid_tx_signature(sig: &str) -> bool {
    bs58::decode(sig)
        .into_vec()
        .map(|b| b.len() == 64)
        .unwrap_or(false)
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

/// Broadcast a legacy wire transaction via `sendTransaction` and return the
/// transaction signature (a base58 string).
///
/// Consensus: this reuses the shared `json_request` path (devnet, `Equality`).
/// A `sendTransaction` response is the transaction's first signature, which is
/// deterministic from the signed bytes, so honest providers return the SAME
/// string and `Equality` agrees (the "first Ok wins" outcome the M2 plan calls
/// for, with a built-in cross-provider sanity check). If providers disagree
/// (one accepted, another rejected with a different body), `json_request`
/// surfaces `Inconsistent` as an error rather than silently picking one.
pub async fn send_transaction(wire_tx: &[u8]) -> Result<String, String> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(wire_tx);
    let payload = build_send_transaction_payload(&b64);
    let text = json_request(&payload).await?;
    parse_send_transaction_signature(&text)
}

/// Read a System nonce account's current durable nonce (a `Hash`) via
/// `getAccountInfo` with `base64` encoding at `finalized`. Errs if the account is
/// not found (not bootstrapped), is not exactly 80 bytes, or is not yet
/// Initialized. `nonce_pubkey` MUST be a validated/derived base58 address (so
/// interpolation cannot inject). Public so Tasks 4/8 can read the nonce before
/// building advance-nonce-led transactions.
pub async fn get_durable_nonce(nonce_pubkey: &str) -> Result<Hash, String> {
    let payload = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"getAccountInfo","params":["{}",{{"encoding":"base64","commitment":"finalized"}}]}}"#,
        nonce_pubkey
    );
    let text = json_request(&payload).await?;
    let buf = parse_account_data_base64(&text)?;
    let blockhash = parse_nonce_account_blockhash(&buf)?;
    Ok(Hash::new_from_array(blockhash))
}

/// Read a fresh recent blockhash (a `Hash`) via `getLatestBlockhash` at
/// `finalized`. Used by the nonce-account bootstrap (the create+initialize tx
/// uses a REAL recent blockhash, not the durable nonce, since the nonce does not
/// exist yet). Public so Task 4/8 can reuse it.
///
/// Consensus caveat (playbook #4): this goes through `json_request`, which demands
/// `Equality` consensus and rejects `#Inconsistent`. But `getLatestBlockhash`
/// returns a value that CHANGES EVERY SLOT, so the sol-rpc canister's
/// multi-provider consensus almost never agrees -> on real devnet/mainnet this
/// call WILL chronically return `#Inconsistent` (surfaced here as an error). It is
/// retained for PocketIC / consensus-capable environments (where the mock returns a
/// single `Consistent(Ok)` response) and as the `None` fallback of
/// `bootstrap_nonce_account`; the production bootstrap path is the operator-supplied
/// blockhash override, NOT this fetch. Do not add any per-slot value to a
/// consensus-dependent read for the same reason.
pub async fn get_latest_blockhash() -> Result<Hash, String> {
    let payload = r#"{"jsonrpc":"2.0","id":1,"method":"getLatestBlockhash","params":[{"commitment":"finalized"}]}"#;
    let text = json_request(payload).await?;
    let blockhash = parse_latest_blockhash(&text)?;
    Ok(Hash::new_from_array(blockhash))
}

/// Look up a transaction by signature at `finalized` via `getTransaction` and
/// distill the result to a `TxStatus` (not-found / confirmed-with-slot /
/// failed). Used by the adapter's `verify_deposit` to confirm a deposit landed
/// and succeeded.
///
/// `signature` is validated as a 64-byte base58 Ed25519 signature before
/// interpolation (the value reaches `verify_deposit` from a caller, so we never
/// trust it to be injection-safe), erroring early on a malformed value rather
/// than building a payload that could break the JSON.
///
/// `maxSupportedTransactionVersion: 0` tells the provider to return versioned
/// (v0) transactions rather than erroring on them; `encoding: json` is the
/// default object form (we only read `slot` and `meta.err`, not the inner
/// instructions, so we do not need `jsonParsed`).
pub async fn get_transaction(signature: &str) -> Result<TxStatus, String> {
    if !is_valid_tx_signature(signature) {
        return Err(format!(
            "invalid transaction signature (must be 64-byte base58): {signature}"
        ));
    }
    let payload = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"getTransaction","params":["{}",{{"encoding":"json","commitment":"finalized","maxSupportedTransactionVersion":0}}]}}"#,
        signature
    );
    let text = json_request(&payload).await?;
    parse_get_transaction(&text)
}

/// Read the current slot at the given commitment via `getSlot`. `verify_deposit`
/// uses the confirmation slot from `getTransaction`; `fetch_finality` uses this
/// to report the chain's latest/finalized slot. On Solana the commitment level
/// (`finalized` / `confirmed`) replaces EVM block depth as the finality knob.
///
/// `commitment` is a fixed string supplied by this crate's callers (never user
/// input), so interpolating it is safe.
pub async fn get_slot(commitment: &str) -> Result<u64, String> {
    let payload = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"getSlot","params":[{{"commitment":"{}"}}]}}"#,
        commitment
    );
    let text = json_request(&payload).await?;
    parse_slot(&text)
}
