//! rippled JSON-RPC over RAW HTTPS outcalls (there is no XRP RPC canister on the
//! IC the way there is for Bitcoin / EVM / Solana). Every replica issues the same
//! request and must agree on the response byte-for-byte, so:
//!
//!  (a) Each `transform_*` reduces the body to ONLY the stable fields consumed,
//!      collapsing volatile rippled fields (server time, load factors, per-edge
//!      error tokens) so replicas converge. A raw rippled body would void
//!      consensus.
//!  (b) Reads are wrapped in a consensus-retry and provider quorum
//!      (`outcall_read_provider_bodies` + typed quorum): a "validated" ledger
//!      advances every ~3-4s, so providers can report the same account or reserve
//!      state at different ledger indices. Reads require a semantic quorum over
//!      the consumed fields instead of byte-for-byte ledger identity. `submit`
//!      uses a NARROWER path (`outcall_submit`, single attempt) — a submit that
//!      reached a node may already have broadcast, so it must never be blindly
//!      re-issued on a consensus miss.
//!
//! ## Wiring note (deferred)
//! For the outcalls to run, the four `transform_*` functions below must be exposed
//! as `#[query]` shims on the canister (the IC resolves `TransformContext::
//! from_name("xrp_transform_account", …)` against the canister's query methods),
//! e.g. in `main.rs`:
//! ```ignore
//! #[query] fn xrp_transform_account(a: TransformArgs) -> HttpResponse {
//!     rumi_protocol_backend::chains::xrp::xrp_rpc::transform_account(a)
//! }
//! ```
//! That registration is part of "wiring up" and is intentionally NOT done here;
//! the transforms/parsers are unit-tested directly (pure functions). Ported from
//! the delegated-vault native-XRP rail.

use ic_cdk::api::call::RejectionCode;
use ic_cdk::api::management_canister::http_request::{
    http_request, CanisterHttpRequestArgument, HttpHeader, HttpMethod, HttpResponse, TransformArgs,
    TransformContext,
};
use serde_json::{json, Value};

use super::config::{is_xrp_production_key_name, xrp_schnorr_key_name};

const MAINNET_URL: &str = "https://xrplcluster.com/";
const TESTNET_URL: &str = "https://s.altnet.rippletest.net:51234/";
const MAINNET_READ_URLS: &[&str] = &[MAINNET_URL, "https://s1.ripple.com:51234/"];
const TESTNET_READ_URLS: &[&str] = &[TESTNET_URL, "https://testnet.xrpl-labs.com/"];

pub const MAX_READ_BYTES: u64 = 8_192;
pub const MAX_TX_BYTES: u64 = 65_536;
pub const MAX_SUBMIT_BYTES: u64 = 8_192;
pub const READ_CYCLES: u128 = 2_000_000_000;
pub const SUBMIT_CYCLES: u128 = 4_000_000_000;

/// How many times a consensus-relative READ is re-issued on the transient
/// consensus-miss class before giving up. Reads are idempotent, so retrying is
/// safe; a validated ledger usually stabilises within a couple of rounds.
const READ_RETRIES: u8 = 3;

/// Parsed `account_info` result. `exists=false` for `actNotFound` (an unfunded
/// account); `ledger_index` is still returned so a withdrawal can set
/// `LastLedgerSequence`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XrpAccountInfo {
    pub exists: bool,
    pub sequence: u32,
    pub balance_drops: u128,
    pub ledger_index: u32,
}

/// Validation status of a transaction (used to confirm a deposit landed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum XrpTxStatus {
    /// Validated into a closed ledger with a `tes*` engine result.
    /// `delivered_drops` is the partial-payment-safe `meta.delivered_amount` in
    /// drops (0 if the delivery was a non-native issued currency, or rippled
    /// reports it `"unavailable"`). Deposit crediting MUST use this, never `Amount`.
    Validated {
        ledger_index: u32,
        delivered_drops: u128,
    },
    /// Not (yet) validated, or `txnNotFound`.
    NotFound,
    /// Validated but the transaction failed (non-`tes*` result).
    Failed,
}

/// Select the rippled cluster by key name: mainnet for the production key,
/// the public altnet testnet otherwise (matches the `test_key_1` default).
pub fn rpc_url(chain_key_name: &str) -> &'static str {
    read_provider_urls(chain_key_name)[0]
}

fn read_provider_urls(chain_key_name: &str) -> &'static [&'static str] {
    if is_xrp_production_key_name(chain_key_name) {
        MAINNET_READ_URLS
    } else {
        TESTNET_READ_URLS
    }
}

fn rpc_body(method: &str, params: Value) -> Vec<u8> {
    json!({ "method": method, "params": [params] })
        .to_string()
        .into_bytes()
}

fn build_request(
    chain_key_name: &str,
    method: &str,
    params: Value,
    max_response_bytes: u64,
    transform_name: &str,
) -> CanisterHttpRequestArgument {
    build_request_to_url(
        rpc_url(chain_key_name),
        method,
        params,
        max_response_bytes,
        transform_name,
    )
}

fn build_request_to_url(
    url: &str,
    method: &str,
    params: Value,
    max_response_bytes: u64,
    transform_name: &str,
) -> CanisterHttpRequestArgument {
    CanisterHttpRequestArgument {
        url: url.to_string(),
        method: HttpMethod::POST,
        body: Some(rpc_body(method, params)),
        max_response_bytes: Some(max_response_bytes),
        headers: vec![HttpHeader {
            name: "content-type".to_string(),
            value: "application/json".to_string(),
        }],
        transform: Some(TransformContext::from_name(
            transform_name.to_string(),
            vec![],
        )),
    }
}

fn account_info_request_to_url(url: &str, account: &str) -> CanisterHttpRequestArgument {
    build_request_to_url(
        url,
        "account_info",
        json!({ "account": account, "ledger_index": "validated", "strict": true }),
        MAX_READ_BYTES,
        "xrp_transform_account",
    )
}

pub fn account_info_request(chain_key_name: &str, account: &str) -> CanisterHttpRequestArgument {
    account_info_request_to_url(rpc_url(chain_key_name), account)
}

fn server_state_request_to_url(url: &str) -> CanisterHttpRequestArgument {
    build_request_to_url(
        url,
        "server_state",
        json!({}),
        MAX_READ_BYTES,
        "xrp_transform_server",
    )
}

pub fn server_state_request(chain_key_name: &str) -> CanisterHttpRequestArgument {
    server_state_request_to_url(rpc_url(chain_key_name))
}

pub fn submit_request(chain_key_name: &str, tx_blob_hex: &str) -> CanisterHttpRequestArgument {
    build_request(
        chain_key_name,
        "submit",
        json!({ "tx_blob": tx_blob_hex }),
        MAX_SUBMIT_BYTES,
        "xrp_transform_submit",
    )
}

pub fn tx_request(chain_key_name: &str, tx_hash: &str) -> CanisterHttpRequestArgument {
    tx_request_to_url(rpc_url(chain_key_name), tx_hash)
}

pub fn tx_max_response_bytes() -> u64 {
    MAX_TX_BYTES
}

fn tx_request_to_url(url: &str, tx_hash: &str) -> CanisterHttpRequestArgument {
    build_request_to_url(
        url,
        "tx",
        json!({ "transaction": tx_hash, "binary": false }),
        tx_max_response_bytes(),
        "xrp_transform_tx",
    )
}

// ---- transforms (pure; must be exported as #[query] to activate outcalls) ----

/// Build the canonical reduced response. IC HTTPS-outcall consensus runs over the
/// FULL transformed response (status + body; headers are excluded), so the status
/// MUST be a fixed constant: `xrplcluster.com` is a round-robin cluster of
/// independent rippled nodes, so one replica can see 200 while another sees
/// 429/503, which would void consensus even when the reduced bodies are identical
/// ("a transform that lets a volatile field through → consensus failures"). All
/// error/edge signal is carried in the body (`parse_error`/`error`), and the
/// parsers key entirely off the body, so pinning the status to 200 is loss-free.
fn reduced_response(body: Vec<u8>) -> HttpResponse {
    HttpResponse {
        status: candid::Nat::from(200u16),
        headers: vec![],
        body,
    }
}

pub fn transform_account(args: TransformArgs) -> HttpResponse {
    let reduced = match serde_json::from_slice::<Value>(&args.response.body) {
        Ok(v) => {
            let r = v.get("result");
            json!({
                "sequence": r.and_then(|r| r.pointer("/account_data/Sequence")),
                "balance": r.and_then(|r| r.pointer("/account_data/Balance")),
                "ledger_index": r.and_then(|r| r.get("ledger_index")),
                "ledger_hash": r.and_then(|r| r.get("ledger_hash")),
                "ledger_current_index": r.and_then(|r| r.get("ledger_current_index")),
                "validated": r.and_then(|r| r.get("validated")),
                "error": r.and_then(|r| r.get("error")),
            })
        }
        Err(_) => json!({ "parse_error": true }),
    };
    reduced_response(reduced.to_string().into_bytes())
}

pub fn transform_server(args: TransformArgs) -> HttpResponse {
    let reduced = match serde_json::from_slice::<Value>(&args.response.body) {
        Ok(v) => json!({
            "reserve_base": v.pointer("/result/state/validated_ledger/reserve_base"),
            "ledger_index": v.pointer("/result/state/validated_ledger/seq"),
            "ledger_hash": v.pointer("/result/state/validated_ledger/hash"),
            "error": v.pointer("/result/error"),
        }),
        Err(_) => json!({ "parse_error": true }),
    };
    reduced_response(reduced.to_string().into_bytes())
}

pub fn transform_submit(args: TransformArgs) -> HttpResponse {
    let reduced = match serde_json::from_slice::<Value>(&args.response.body) {
        Ok(v) => json!({
            "engine_result": v.pointer("/result/engine_result"),
            "hash": v.pointer("/result/tx_json/hash"),
            "error": v.pointer("/result/error"),
        }),
        Err(_) => json!({ "parse_error": true }),
    };
    reduced_response(reduced.to_string().into_bytes())
}

pub fn transform_tx(args: TransformArgs) -> HttpResponse {
    let reduced = match serde_json::from_slice::<Value>(&args.response.body) {
        Ok(v) => {
            let r = v.get("result");
            json!({
                "validated": r.and_then(|r| r.get("validated")),
                "engine_result": r.and_then(|r| r.pointer("/meta/TransactionResult")),
                "ledger_index": r.and_then(|r| r.get("ledger_index")),
                "ledger_hash": r.and_then(|r| r.get("ledger_hash")),
                "tx_hash": r.and_then(|r| r.get("hash")),
                // Partial-payment-safe delivered amount: credit THIS, never the
                // Payment's `Amount` (tfPartialPayment can make Amount > delivered —
                // the classic exchange-drainer). rippled synthesizes the top-level
                // `delivered_amount` for every validated Payment: a drops string when
                // known, or the literal "unavailable" for pre-2014 txs (which
                // `parse_delivered_drops` maps to 0 — fail-closed). Deterministic for
                // a validated tx, so consensus-safe.
                "delivered_amount": r.and_then(|r| r.pointer("/meta/delivered_amount")),
                "error": r.and_then(|r| r.get("error")),
            })
        }
        Err(_) => json!({ "parse_error": true }),
    };
    reduced_response(reduced.to_string().into_bytes())
}

// ---- parsers (operate on the reduced transform output) ----

fn as_u64(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
}
fn as_u128(v: &Value) -> Option<u128> {
    v.as_u64()
        .map(u128::from)
        .or_else(|| v.as_str().and_then(|s| s.parse::<u128>().ok()))
}

/// Parse the authoritative delivered amount of a native-XRP Payment into drops.
/// `meta.delivered_amount` is the partial-payment-safe field — it can be LESS than
/// the Payment's `Amount`, so deposit crediting MUST use this, never `Amount`.
/// Returns 0 for: a non-native delivery (an issued-currency object), the
/// `"unavailable"` sentinel (a pre-2014 tx rippled cannot reconstruct), or a
/// missing/garbage value. The caller treats 0 as "no creditable native XRP".
fn parse_delivered_drops(v: &Value) -> u128 {
    match v {
        // rippled renders XRP amounts as a drops string; "unavailable" -> 0.
        Value::String(s) => s.parse::<u128>().unwrap_or(0),
        Value::Number(n) => n.as_u64().map(u128::from).unwrap_or(0),
        // object (issued currency) or null -> not native XRP.
        _ => 0,
    }
}

pub fn parse_account_info(body: &[u8]) -> Result<XrpAccountInfo, String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| format!("json: {e}"))?;
    if v.get("parse_error").and_then(Value::as_bool) == Some(true) {
        return Err("account_info: unparseable rippled response".to_string());
    }
    let err = v.get("error").and_then(Value::as_str);
    if err == Some("actNotFound") {
        let ledger_index = as_u64(v.get("ledger_current_index").unwrap_or(&Value::Null))
            .or_else(|| as_u64(v.get("ledger_index").unwrap_or(&Value::Null)))
            .ok_or_else(|| "actNotFound without a ledger index".to_string())?
            as u32;
        return Ok(XrpAccountInfo {
            exists: false,
            sequence: 0,
            balance_drops: 0,
            ledger_index,
        });
    }
    if let Some(e) = err {
        return Err(format!("account_info error: {e}"));
    }
    let sequence = as_u64(v.get("sequence").unwrap_or(&Value::Null))
        .ok_or_else(|| "missing Sequence".to_string())? as u32;
    let balance_drops = as_u128(v.get("balance").unwrap_or(&Value::Null))
        .ok_or_else(|| "missing Balance".to_string())?;
    let ledger_index = as_u64(v.get("ledger_index").unwrap_or(&Value::Null))
        .ok_or_else(|| "missing ledger_index".to_string())? as u32;
    Ok(XrpAccountInfo {
        exists: true,
        sequence,
        balance_drops,
        ledger_index,
    })
}

pub fn parse_reserve_base(body: &[u8]) -> Result<u128, String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| format!("json: {e}"))?;
    if v.get("parse_error").and_then(Value::as_bool) == Some(true) {
        return Err("server_state: unparseable rippled response".to_string());
    }
    if let Some(e) = v.get("error").and_then(Value::as_str) {
        return Err(format!("server_state error: {e}"));
    }
    as_u128(v.get("reserve_base").unwrap_or(&Value::Null))
        .ok_or_else(|| "missing reserve_base".to_string())
}

/// Returns the tx hash for an accepted submission. `tes*`/`ter*` engine results
/// are accepted (applied / queued); anything else is surfaced as an error.
pub fn parse_submit(body: &[u8]) -> Result<String, String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| format!("json: {e}"))?;
    if v.get("parse_error").and_then(Value::as_bool) == Some(true) {
        return Err("submit: unparseable rippled response".to_string());
    }
    if let Some(e) = v.get("error").and_then(Value::as_str) {
        return Err(format!("submit rpc error: {e}"));
    }
    let engine = v.get("engine_result").and_then(Value::as_str).unwrap_or("");
    let hash = v.get("hash").and_then(Value::as_str).map(String::from);
    if engine.starts_with("tes") || engine.starts_with("ter") {
        hash.ok_or_else(|| format!("submit accepted ({engine}) but no hash"))
    } else {
        Err(format!("submit rejected: {engine}"))
    }
}

fn parse_tx_status_value(v: &Value, expected_hash: Option<&str>) -> Result<XrpTxStatus, String> {
    if let Some(expected) = expected_hash {
        if let Some(actual) = v.get("tx_hash").and_then(Value::as_str) {
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(format!(
                    "tx hash mismatch: expected {expected}, got {actual}"
                ));
            }
        }
    }
    if v.get("parse_error").and_then(Value::as_bool) == Some(true) {
        return Err("tx: unparseable rippled response".to_string());
    }
    if v.get("error").and_then(Value::as_str) == Some("txnNotFound") {
        return Ok(XrpTxStatus::NotFound);
    }
    if let Some(e) = v.get("error").and_then(Value::as_str) {
        return Err(format!("tx rpc error: {e}"));
    }
    // Not yet in a validated ledger => treat as not-found (caller retries later).
    if v.get("validated").and_then(Value::as_bool) != Some(true) {
        return Ok(XrpTxStatus::NotFound);
    }
    if expected_hash.is_some() && v.get("tx_hash").and_then(Value::as_str).is_none() {
        return Err("validated tx without tx_hash".to_string());
    }
    let engine = v.get("engine_result").and_then(Value::as_str).unwrap_or("");
    if engine.starts_with("tes") {
        let ledger_index = as_u64(v.get("ledger_index").unwrap_or(&Value::Null))
            .ok_or_else(|| "validated tx without ledger_index".to_string())?
            as u32;
        let delivered_drops =
            parse_delivered_drops(v.get("delivered_amount").unwrap_or(&Value::Null));
        Ok(XrpTxStatus::Validated {
            ledger_index,
            delivered_drops,
        })
    } else {
        Ok(XrpTxStatus::Failed)
    }
}

pub fn parse_tx_status(body: &[u8]) -> Result<XrpTxStatus, String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| format!("json: {e}"))?;
    parse_tx_status_value(&v, None)
}

fn parse_tx_status_for_hash(body: &[u8], expected_hash: &str) -> Result<XrpTxStatus, String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| format!("json: {e}"))?;
    parse_tx_status_value(&v, Some(expected_hash))
}

// ---- async outcalls (consensus-retry + provider-quorum wrapped) ----

/// True for the transient consensus-miss class that is safe to re-issue on a READ.
fn is_transient(code: RejectionCode, msg: &str) -> bool {
    matches!(code, RejectionCode::SysTransient)
        || msg.contains("No consensus could be reached")
        || msg.contains("Timeout expired")
}

const READ_QUORUM: usize = 2;

fn require_minimum_provider_set(provider_bodies: &[Vec<u8>]) -> Result<(), String> {
    if provider_bodies.len() < READ_QUORUM {
        return Err(format!(
            "read quorum requires at least {READ_QUORUM} provider responses"
        ));
    }
    Ok(())
}

fn quorum_account_info(provider_bodies: &[Vec<u8>]) -> Result<XrpAccountInfo, String> {
    require_minimum_provider_set(provider_bodies)?;
    let mut parsed = Vec::with_capacity(provider_bodies.len());
    let mut parse_errors = Vec::new();
    for (idx, body) in provider_bodies.iter().enumerate() {
        match parse_account_info(body) {
            Ok(info) => parsed.push((idx, info)),
            Err(e) => parse_errors.push(format!("provider {idx}: {e}")),
        }
    }

    for (_, candidate) in &parsed {
        let mut votes = 0usize;
        let mut latest = candidate.clone();
        for (_, info) in &parsed {
            if info.exists == candidate.exists
                && info.sequence == candidate.sequence
                && info.balance_drops == candidate.balance_drops
            {
                votes += 1;
                if info.ledger_index > latest.ledger_index {
                    latest.ledger_index = info.ledger_index;
                }
            }
        }
        if votes >= READ_QUORUM {
            return Ok(latest);
        }
    }

    Err(format!(
        "account_info semantic quorum failed: {} parsed responses, errors: {}",
        parsed.len(),
        parse_errors.join("; ")
    ))
}

fn quorum_reserve_base(provider_bodies: &[Vec<u8>]) -> Result<u128, String> {
    require_minimum_provider_set(provider_bodies)?;
    let mut parsed = Vec::with_capacity(provider_bodies.len());
    let mut parse_errors = Vec::new();
    for (idx, body) in provider_bodies.iter().enumerate() {
        match parse_reserve_base(body) {
            Ok(reserve) => parsed.push((idx, reserve)),
            Err(e) => parse_errors.push(format!("provider {idx}: {e}")),
        }
    }

    for (_, candidate) in &parsed {
        let votes = parsed
            .iter()
            .filter(|(_, reserve)| reserve == candidate)
            .count();
        if votes >= READ_QUORUM {
            return Ok(*candidate);
        }
    }

    Err(format!(
        "server_state semantic quorum failed: {} parsed responses, errors: {}",
        parsed.len(),
        parse_errors.join("; ")
    ))
}

fn quorum_tx_status(
    provider_bodies: &[Vec<u8>],
    expected_hash: &str,
) -> Result<XrpTxStatus, String> {
    require_minimum_provider_set(provider_bodies)?;
    let mut parsed = Vec::with_capacity(provider_bodies.len());
    let mut parse_errors = Vec::new();
    for (idx, body) in provider_bodies.iter().enumerate() {
        match parse_tx_status_for_hash(body, expected_hash) {
            Ok(status) => parsed.push((idx, status)),
            Err(e) => parse_errors.push(format!("provider {idx}: {e}")),
        }
    }

    for (_, candidate) in &parsed {
        let votes = parsed
            .iter()
            .filter(|(_, status)| status == candidate)
            .count();
        if votes >= READ_QUORUM {
            return Ok(candidate.clone());
        }
    }

    Err(format!(
        "tx semantic quorum failed: {} parsed responses, errors: {}",
        parsed.len(),
        parse_errors.join("; ")
    ))
}

async fn outcall_read_provider_bodies<F>(
    provider_urls: &[&str],
    mut build: F,
) -> Result<Vec<Vec<u8>>, String>
where
    F: FnMut(&str) -> CanisterHttpRequestArgument,
{
    if provider_urls.len() < READ_QUORUM {
        return Err(format!(
            "read quorum requires at least {READ_QUORUM} configured providers"
        ));
    }

    let mut last = String::new();
    for attempt in 0..READ_RETRIES {
        let mut provider_bodies = Vec::with_capacity(provider_urls.len());
        let mut errors = Vec::new();
        let mut has_transient = false;

        for provider_url in provider_urls {
            match http_request(build(provider_url), READ_CYCLES).await {
                Ok((resp,)) => provider_bodies.push(resp.body),
                Err((code, msg)) => {
                    has_transient |= is_transient(code, &msg);
                    errors.push(format!(
                        "attempt {} provider {provider_url}: {code:?}: {msg}",
                        attempt + 1
                    ));
                }
            }
        }

        if provider_bodies.len() >= READ_QUORUM {
            return Ok(provider_bodies);
        }

        last = errors.join("; ");
        if !has_transient {
            break;
        }
    }
    Err(format!(
        "read quorum failed after {READ_RETRIES} attempts ({last})"
    ))
}

async fn outcall_read_quorum<F, T, Q>(
    provider_urls: &[&str],
    mut build: F,
    quorum: Q,
) -> Result<T, String>
where
    F: FnMut(&str) -> CanisterHttpRequestArgument,
    Q: Fn(&[Vec<u8>]) -> Result<T, String>,
{
    let mut last = String::new();
    for attempt in 0..READ_RETRIES {
        let bodies = outcall_read_provider_bodies(provider_urls, |url| build(url)).await?;
        match quorum(&bodies) {
            Ok(value) => return Ok(value),
            Err(e) => last = format!("attempt {}: {e}", attempt + 1),
        }
    }
    Err(format!(
        "read semantic quorum failed after {READ_RETRIES} attempts ({last})"
    ))
}

/// Issue `submit` exactly ONCE. A submit that reached a node may already have
/// broadcast, so a consensus miss is NOT re-issued (that would risk a double
/// send). Phase 2: on a consensus miss record the expected hash and
/// confirm-before-resubmit instead.
async fn outcall_submit(req: CanisterHttpRequestArgument) -> Result<Vec<u8>, String> {
    match http_request(req, SUBMIT_CYCLES).await {
        Ok((resp,)) => Ok(resp.body),
        Err((code, msg)) => Err(format!("{code:?}: {msg}")),
    }
}

/// Read `account_info` (validated): sequence, balance, ledger index.
pub async fn fetch_account_info(account: &str) -> Result<XrpAccountInfo, String> {
    let key_name = xrp_schnorr_key_name();
    outcall_read_quorum(
        read_provider_urls(&key_name),
        |url| account_info_request_to_url(url, account),
        quorum_account_info,
    )
    .await
}

/// Read the base reserve (drops) from `server_state`.
pub async fn fetch_reserve_base() -> Result<u128, String> {
    let key_name = xrp_schnorr_key_name();
    outcall_read_quorum(
        read_provider_urls(&key_name),
        server_state_request_to_url,
        quorum_reserve_base,
    )
    .await
}

/// Submit a hex-encoded signed tx blob; returns the rippled-reported hash on a
/// `tes*`/`ter*` engine result (the caller should prefer the LOCALLY computed
/// hash from `sign::tx_hash`).
pub async fn submit_blob(tx_blob_hex: &str) -> Result<String, String> {
    let body = outcall_submit(submit_request(&xrp_schnorr_key_name(), tx_blob_hex)).await?;
    parse_submit(&body)
}

/// Look up a transaction's validation status (deposit confirmation).
pub async fn fetch_tx_status(tx_hash: &str) -> Result<XrpTxStatus, String> {
    let key_name = xrp_schnorr_key_name();
    outcall_read_quorum(
        read_provider_urls(&key_name),
        |url| tx_request_to_url(url, tx_hash),
        |bodies| quorum_tx_status(bodies, tx_hash),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::super::config::{XRP_PRODUCTION_SCHNORR_KEY_NAME, XRP_TEST_SCHNORR_KEY_NAME};
    use super::*;

    #[test]
    fn parse_account_info_reads_funded() {
        let body = br#"{"sequence":42,"balance":"25000000","ledger_index":9000000,"ledger_current_index":null,"error":null}"#;
        let a = parse_account_info(body).unwrap();
        assert_eq!(
            a,
            XrpAccountInfo {
                exists: true,
                sequence: 42,
                balance_drops: 25_000_000,
                ledger_index: 9_000_000
            }
        );
    }

    #[test]
    fn parse_account_info_handles_unfunded() {
        let body = br#"{"sequence":null,"balance":null,"ledger_index":null,"ledger_current_index":9000001,"error":"actNotFound"}"#;
        let a = parse_account_info(body).unwrap();
        assert!(!a.exists);
        assert_eq!(a.balance_drops, 0);
        assert_eq!(a.ledger_index, 9_000_001);
    }

    #[test]
    fn parse_reserve_base_reads_drops() {
        let body = br#"{"reserve_base":1000000,"error":null}"#;
        assert_eq!(parse_reserve_base(body).unwrap(), 1_000_000);
    }

    #[test]
    fn parse_submit_accepts_tes_and_ter() {
        assert_eq!(
            parse_submit(br#"{"engine_result":"tesSUCCESS","hash":"ABC","error":null}"#).unwrap(),
            "ABC"
        );
        assert_eq!(
            parse_submit(br#"{"engine_result":"terQUEUED","hash":"DEF","error":null}"#).unwrap(),
            "DEF"
        );
    }

    #[test]
    fn parse_submit_rejects_tec_and_tef() {
        assert!(parse_submit(
            br#"{"engine_result":"tecUNFUNDED_PAYMENT","hash":"X","error":null}"#
        )
        .is_err());
        assert!(
            parse_submit(br#"{"engine_result":"tefMAX_LEDGER","hash":null,"error":null}"#).is_err()
        );
    }

    #[test]
    fn parse_tx_status_validated_success() {
        let body = br#"{"validated":true,"engine_result":"tesSUCCESS","ledger_index":9000000,"delivered_amount":"1000000","error":null}"#;
        assert_eq!(
            parse_tx_status(body).unwrap(),
            XrpTxStatus::Validated {
                ledger_index: 9_000_000,
                delivered_drops: 1_000_000
            }
        );
    }

    #[test]
    fn parse_delivered_drops_variants() {
        assert_eq!(
            parse_delivered_drops(&serde_json::json!("1000000")),
            1_000_000
        );
        assert_eq!(
            parse_delivered_drops(&serde_json::json!(1_000_000u64)),
            1_000_000
        );
        // issued-currency object -> not native XRP -> 0
        assert_eq!(
            parse_delivered_drops(
                &serde_json::json!({"currency":"USD","issuer":"rIssuer","value":"5"})
            ),
            0
        );
        // rippled "unavailable" sentinel and null -> 0
        assert_eq!(parse_delivered_drops(&serde_json::json!("unavailable")), 0);
        assert_eq!(parse_delivered_drops(&Value::Null), 0);
        // negative / overflow / non-numeric strings -> 0 (fail-closed)
        assert_eq!(parse_delivered_drops(&serde_json::json!("-500000")), 0);
        assert_eq!(
            parse_delivered_drops(&serde_json::json!(
                "999999999999999999999999999999999999999999"
            )),
            0
        );
    }

    #[test]
    fn tx_credits_delivered_amount_not_payment_amount() {
        // Partial-payment drainer guard: the Payment claims a huge `Amount`, but
        // only `delivered_amount` actually arrived. Crediting `Amount` would mint
        // against XRP never received. The transform keeps delivered_amount and the
        // parser credits ONLY it.
        let args = TransformArgs {
            response: HttpResponse {
                status: candid::Nat::from(200u32),
                headers: vec![],
                body: br#"{"result":{"validated":true,"ledger_index":9000000,"Amount":"9999999999","meta":{"TransactionResult":"tesSUCCESS","delivered_amount":"500000"}}}"#.to_vec(),
            },
            context: vec![],
        };
        let reduced = transform_tx(args);
        match parse_tx_status(&reduced.body).unwrap() {
            XrpTxStatus::Validated {
                delivered_drops, ..
            } => {
                assert_eq!(delivered_drops, 500_000, "must credit delivered_amount");
                assert_ne!(delivered_drops, 9_999_999_999, "must NOT credit Amount");
            }
            other => panic!("expected Validated, got {other:?}"),
        }
    }

    #[test]
    fn parse_tx_status_not_found_and_unvalidated() {
        assert_eq!(
            parse_tx_status(br#"{"error":"txnNotFound"}"#).unwrap(),
            XrpTxStatus::NotFound
        );
        assert_eq!(
            parse_tx_status(br#"{"validated":false,"engine_result":"tesSUCCESS"}"#).unwrap(),
            XrpTxStatus::NotFound
        );
    }

    #[test]
    fn parse_tx_status_failed_when_validated_non_tes() {
        let body =
            br#"{"validated":true,"engine_result":"tecPATH_DRY","ledger_index":9000000,"error":null}"#;
        assert_eq!(parse_tx_status(body).unwrap(), XrpTxStatus::Failed);
    }

    #[test]
    fn transform_server_strips_volatile_fields() {
        // Two replicas: identical reserve_base, different server time/load.
        let mk = |t: &str| -> TransformArgs {
            TransformArgs {
                response: HttpResponse {
                    status: candid::Nat::from(200u32),
                    headers: vec![HttpHeader {
                        name: "date".into(),
                        value: "now".into(),
                    }],
                    body: format!(
                        r#"{{"result":{{"state":{{"server_state":"full","time":"{t}","load_factor":256,"validated_ledger":{{"reserve_base":1000000,"seq":9000000}}}}}}}}"#
                    )
                    .into_bytes(),
                },
                context: vec![],
            }
        };
        let a = transform_server(mk("A"));
        let b = transform_server(mk("B"));
        assert_eq!(a.body, b.body, "replicas converge");
        assert_eq!(parse_reserve_base(&a.body).unwrap(), 1_000_000);
    }

    #[test]
    fn transform_server_keeps_validated_ledger_identity_for_quorum() {
        let reduced = transform_server(TransformArgs {
            response: HttpResponse {
                status: candid::Nat::from(200u32),
                headers: vec![],
                body: br#"{"result":{"state":{"validated_ledger":{"reserve_base":1000000,"seq":9000000,"hash":"ABCDEF"}}}}"#
                    .to_vec(),
            },
            context: vec![],
        });
        let v: Value = serde_json::from_slice(&reduced.body).unwrap();
        assert_eq!(v.get("reserve_base"), Some(&json!(1_000_000)));
        assert_eq!(v.get("ledger_index"), Some(&json!(9_000_000)));
        assert_eq!(v.get("ledger_hash"), Some(&json!("ABCDEF")));
    }

    #[test]
    fn transform_account_collapses_non_json_edge_error() {
        let mk = |html: &[u8]| TransformArgs {
            response: HttpResponse {
                status: candid::Nat::from(429u32),
                headers: vec![],
                body: html.to_vec(),
            },
            context: vec![],
        };
        let a = transform_account(mk(b"<html>ray=A</html>"));
        let b = transform_account(mk(b"<html>ray=B</html>"));
        assert_eq!(a.body, b.body);
        assert!(parse_account_info(&a.body)
            .unwrap_err()
            .contains("unparseable"));
    }

    #[test]
    fn transform_normalizes_status_across_replicas() {
        // A round-robin cluster can return DIFFERENT statuses on different replicas
        // for the same logical body (e.g. 200 vs 503 during a rate-limit blip).
        // Consensus runs over the FULL response (status + body), so the transform
        // must pin the status; assert convergence on the whole response, not body.
        let mk = |status: u32| -> TransformArgs {
            TransformArgs {
                response: HttpResponse {
                    status: candid::Nat::from(status),
                    headers: vec![HttpHeader {
                        name: "x-edge".into(),
                        value: format!("node-{status}"),
                    }],
                    body: br#"{"result":{"account_data":{"Sequence":42,"Balance":"25000000"},"ledger_index":9000000}}"#
                        .to_vec(),
                },
                context: vec![],
            }
        };
        let ok = transform_account(mk(200));
        let throttled = transform_account(mk(503));
        assert_eq!(ok.status, throttled.status, "status pinned for consensus");
        assert_eq!(ok.body, throttled.body, "bodies converge");
        assert_eq!(
            ok.status,
            candid::Nat::from(200u32),
            "canonical status is 200"
        );
        // The reduced body still parses the consumed fields.
        assert_eq!(parse_account_info(&ok.body).unwrap().sequence, 42);
    }

    #[test]
    fn transform_tx_reduces_and_converges() {
        let mk = |edge: &str| -> TransformArgs {
            TransformArgs {
                response: HttpResponse {
                    status: candid::Nat::from(200u32),
                    headers: vec![HttpHeader {
                        name: "x-ripple-edge".into(),
                        value: edge.into(),
                    }],
                    body: format!(
                        r#"{{"result":{{"validated":true,"ledger_index":9000000,"close_time_human":"{edge}","meta":{{"TransactionResult":"tesSUCCESS","delivered_amount":"1000000"}}}}}}"#
                    )
                    .into_bytes(),
                },
                context: vec![],
            }
        };
        let a = transform_tx(mk("A"));
        let b = transform_tx(mk("B"));
        assert_eq!(a.body, b.body, "volatile close_time stripped");
        assert_eq!(
            parse_tx_status(&a.body).unwrap(),
            XrpTxStatus::Validated {
                ledger_index: 9_000_000,
                delivered_drops: 1_000_000
            }
        );
    }

    #[test]
    fn transform_tx_keeps_ledger_hash_for_quorum() {
        let reduced = transform_tx(TransformArgs {
            response: HttpResponse {
                status: candid::Nat::from(200u32),
                headers: vec![],
                body: br#"{"result":{"validated":true,"hash":"TXHASH","ledger_index":9000000,"ledger_hash":"LEDGERHASH","meta":{"TransactionResult":"tesSUCCESS","delivered_amount":"1000000"}}}"#
                    .to_vec(),
            },
            context: vec![],
        });
        let v: Value = serde_json::from_slice(&reduced.body).unwrap();
        assert_eq!(v.get("ledger_index"), Some(&json!(9_000_000)));
        assert_eq!(v.get("ledger_hash"), Some(&json!("LEDGERHASH")));
        assert_eq!(v.get("tx_hash"), Some(&json!("TXHASH")));
    }

    #[test]
    fn read_provider_urls_are_quorum_sets_by_key_network() {
        let mainnet = read_provider_urls(XRP_PRODUCTION_SCHNORR_KEY_NAME);
        let testnet = read_provider_urls(XRP_TEST_SCHNORR_KEY_NAME);

        assert!(
            mainnet.len() >= 2,
            "mainnet reads must not trust one provider"
        );
        assert!(
            testnet.len() >= 2,
            "testnet reads must not trust one provider"
        );
        assert_eq!(rpc_url(XRP_PRODUCTION_SCHNORR_KEY_NAME), mainnet[0]);
        assert_eq!(rpc_url(XRP_TEST_SCHNORR_KEY_NAME), testnet[0]);
        assert_ne!(
            mainnet, testnet,
            "production key must select mainnet providers"
        );
        assert!(
            !mainnet.iter().any(|url| url.contains("xrpl.ws")),
            "xrpl.ws is an alias for xrplcluster.com and must not be counted as an independent provider"
        );
        assert!(
            !mainnet.iter().any(|url| url.contains("s2.ripple.com")),
            "do not let one operator form the read quorum by listing both Ripple public clusters"
        );
    }

    #[test]
    fn account_quorum_accepts_same_state_across_ledger_progress() {
        let provider_a =
            br#"{"sequence":42,"balance":"25000000","ledger_index":9000000,"ledger_hash":"A","error":null}"#
                .to_vec();
        let provider_b =
            br#"{"sequence":42,"balance":"25000000","ledger_index":9000001,"ledger_hash":"B","error":null}"#
                .to_vec();
        let info = quorum_account_info(&[provider_a, provider_b]).unwrap();
        assert_eq!(info.sequence, 42);
        assert_eq!(info.balance_drops, 25_000_000);
        assert_eq!(
            info.ledger_index, 9_000_001,
            "return the freshest ledger index among agreeing providers"
        );
    }

    #[test]
    fn account_quorum_rejects_state_disagreement_without_majority() {
        let provider_a =
            br#"{"sequence":42,"balance":"25000000","ledger_index":9000000,"ledger_hash":"A","error":null}"#
                .to_vec();
        let provider_b =
            br#"{"sequence":43,"balance":"24000000","ledger_index":9000001,"ledger_hash":"B","error":null}"#
                .to_vec();
        let err = quorum_account_info(&[provider_a, provider_b]).unwrap_err();
        assert!(err.contains("semantic quorum failed"), "{err}");
    }

    #[test]
    fn reserve_quorum_accepts_matching_reserve_across_ledger_progress() {
        let provider_a =
            br#"{"reserve_base":1000000,"ledger_index":9000000,"ledger_hash":"A","error":null}"#
                .to_vec();
        let provider_b =
            br#"{"reserve_base":1000000,"ledger_index":9000001,"ledger_hash":"B","error":null}"#
                .to_vec();
        assert_eq!(
            quorum_reserve_base(&[provider_a, provider_b]).unwrap(),
            1_000_000
        );
    }

    #[test]
    fn tx_status_for_hash_rejects_mismatched_hash() {
        let body = br#"{"validated":true,"tx_hash":"WRONG","engine_result":"tesSUCCESS","ledger_index":9000000,"delivered_amount":"1000000","error":null}"#;
        let err = parse_tx_status_for_hash(body, "EXPECTED").unwrap_err();
        assert!(err.contains("tx hash mismatch"), "{err}");
    }

    #[test]
    fn tx_quorum_accepts_matching_validated_status() {
        let provider_a =
            br#"{"validated":true,"tx_hash":"ABC","engine_result":"tesSUCCESS","ledger_index":9000000,"delivered_amount":"1000000","error":null}"#
                .to_vec();
        let provider_b =
            br#"{"validated":true,"tx_hash":"abc","engine_result":"tesSUCCESS","ledger_index":9000000,"delivered_amount":"1000000","error":null}"#
                .to_vec();
        assert_eq!(
            quorum_tx_status(&[provider_a, provider_b], "ABC").unwrap(),
            XrpTxStatus::Validated {
                ledger_index: 9_000_000,
                delivered_drops: 1_000_000
            }
        );
    }

    #[test]
    fn tx_lookup_uses_confirmation_response_limit_not_generic_read_limit() {
        assert!(
            tx_max_response_bytes() > MAX_READ_BYTES,
            "tx confirmation can include metadata and must not be capped at generic read size"
        );
    }

    #[test]
    fn rpc_url_selects_network_by_key() {
        assert_eq!(rpc_url("key_1"), MAINNET_URL);
        assert_eq!(rpc_url("test_key_1"), TESTNET_URL);
    }
}
