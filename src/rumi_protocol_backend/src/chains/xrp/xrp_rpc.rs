//! rippled JSON-RPC over RAW HTTPS outcalls (there is no XRP RPC canister on the
//! IC the way there is for Bitcoin / EVM / Solana). Every replica issues the same
//! request and must agree on the response byte-for-byte, so:
//!
//!  (a) Each `transform_*` reduces the body to ONLY the stable fields consumed,
//!      collapsing volatile rippled fields (server time, load factors, per-edge
//!      error tokens) so replicas converge. A raw rippled body would void
//!      consensus.
//!  (b) Reads are wrapped in a consensus-retry (`outcall_read`): a "validated"
//!      ledger advances every ~3-4s, so replicas can disagree when a ledger
//!      validates mid-fetch ("No consensus could be reached"). Re-issuing lands
//!      them on the same ledger. `submit` uses a NARROWER path (`outcall_submit`,
//!      single attempt) — a submit that reached a node may already have broadcast,
//!      so it must never be blindly re-issued on a consensus miss.
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

use super::config::xrp_schnorr_key_name;

const MAINNET_URL: &str = "https://xrplcluster.com/";
const TESTNET_URL: &str = "https://s.altnet.rippletest.net:51234/";

pub const MAX_READ_BYTES: u64 = 8_192;
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
    Validated { ledger_index: u32, delivered_drops: u128 },
    /// Not (yet) validated, or `txnNotFound`.
    NotFound,
    /// Validated but the transaction failed (non-`tes*` result).
    Failed,
}

/// Select the rippled cluster by key name: mainnet for the production key,
/// the public altnet testnet otherwise (matches the `test_key_1` default).
pub fn rpc_url(chain_key_name: &str) -> &'static str {
    if chain_key_name == "key_1" {
        MAINNET_URL
    } else {
        TESTNET_URL
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
    CanisterHttpRequestArgument {
        url: rpc_url(chain_key_name).to_string(),
        method: HttpMethod::POST,
        body: Some(rpc_body(method, params)),
        max_response_bytes: Some(max_response_bytes),
        headers: vec![HttpHeader {
            name: "content-type".to_string(),
            value: "application/json".to_string(),
        }],
        transform: Some(TransformContext::from_name(transform_name.to_string(), vec![])),
    }
}

pub fn account_info_request(chain_key_name: &str, account: &str) -> CanisterHttpRequestArgument {
    build_request(
        chain_key_name,
        "account_info",
        json!({ "account": account, "ledger_index": "validated", "strict": true }),
        MAX_READ_BYTES,
        "xrp_transform_account",
    )
}

pub fn server_state_request(chain_key_name: &str) -> CanisterHttpRequestArgument {
    build_request(
        chain_key_name,
        "server_state",
        json!({}),
        MAX_READ_BYTES,
        "xrp_transform_server",
    )
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
    build_request(
        chain_key_name,
        "tx",
        json!({ "transaction": tx_hash, "binary": false }),
        MAX_READ_BYTES,
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
                "ledger_current_index": r.and_then(|r| r.get("ledger_current_index")),
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
    v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
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
            .ok_or_else(|| "actNotFound without a ledger index".to_string())? as u32;
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
    let sequence =
        as_u64(v.get("sequence").unwrap_or(&Value::Null)).ok_or_else(|| "missing Sequence".to_string())?
            as u32;
    let balance_drops =
        as_u128(v.get("balance").unwrap_or(&Value::Null)).ok_or_else(|| "missing Balance".to_string())?;
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

pub fn parse_tx_status(body: &[u8]) -> Result<XrpTxStatus, String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| format!("json: {e}"))?;
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
    let engine = v.get("engine_result").and_then(Value::as_str).unwrap_or("");
    if engine.starts_with("tes") {
        let ledger_index = as_u64(v.get("ledger_index").unwrap_or(&Value::Null))
            .ok_or_else(|| "validated tx without ledger_index".to_string())? as u32;
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

// ---- async outcalls (consensus-retry wrapped) ----

/// True for the transient consensus-miss class that is safe to re-issue on a READ.
fn is_transient(code: RejectionCode, msg: &str) -> bool {
    matches!(code, RejectionCode::SysTransient)
        || msg.contains("No consensus could be reached")
        || msg.contains("Timeout expired")
}

/// Issue a consensus-relative READ, re-issuing on the transient consensus-miss
/// class up to `READ_RETRIES` times. Returns the (already-transformed) body.
async fn outcall_read(req: CanisterHttpRequestArgument) -> Result<Vec<u8>, String> {
    let mut last = String::new();
    for attempt in 0..READ_RETRIES {
        match http_request(req.clone(), READ_CYCLES).await {
            Ok((resp,)) => return Ok(resp.body),
            Err((code, msg)) if is_transient(code, &msg) => {
                last = format!("attempt {}: {code:?}: {msg}", attempt + 1);
            }
            Err((code, msg)) => return Err(format!("{code:?}: {msg}")),
        }
    }
    Err(format!("read failed after {READ_RETRIES} attempts ({last})"))
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
    let body = outcall_read(account_info_request(&xrp_schnorr_key_name(), account)).await?;
    parse_account_info(&body)
}

/// Read the base reserve (drops) from `server_state`.
pub async fn fetch_reserve_base() -> Result<u128, String> {
    let body = outcall_read(server_state_request(&xrp_schnorr_key_name())).await?;
    parse_reserve_base(&body)
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
    let body = outcall_read(tx_request(&xrp_schnorr_key_name(), tx_hash)).await?;
    parse_tx_status(&body)
}

#[cfg(test)]
mod tests {
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
        assert!(
            parse_submit(br#"{"engine_result":"tecUNFUNDED_PAYMENT","hash":"X","error":null}"#)
                .is_err()
        );
        assert!(parse_submit(br#"{"engine_result":"tefMAX_LEDGER","hash":null,"error":null}"#).is_err());
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
        assert_eq!(parse_delivered_drops(&serde_json::json!("1000000")), 1_000_000);
        assert_eq!(parse_delivered_drops(&serde_json::json!(1_000_000u64)), 1_000_000);
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
            XrpTxStatus::Validated { delivered_drops, .. } => {
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
        let mk = |t: &str| TransformArgs {
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
        };
        let a = transform_server(mk("A"));
        let b = transform_server(mk("B"));
        assert_eq!(a.body, b.body, "replicas converge");
        assert_eq!(parse_reserve_base(&a.body).unwrap(), 1_000_000);
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
        assert!(parse_account_info(&a.body).unwrap_err().contains("unparseable"));
    }

    #[test]
    fn transform_normalizes_status_across_replicas() {
        // A round-robin cluster can return DIFFERENT statuses on different replicas
        // for the same logical body (e.g. 200 vs 503 during a rate-limit blip).
        // Consensus runs over the FULL response (status + body), so the transform
        // must pin the status; assert convergence on the whole response, not body.
        let mk = |status: u32| TransformArgs {
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
        };
        let ok = transform_account(mk(200));
        let throttled = transform_account(mk(503));
        assert_eq!(ok.status, throttled.status, "status pinned for consensus");
        assert_eq!(ok.body, throttled.body, "bodies converge");
        assert_eq!(ok.status, candid::Nat::from(200u32), "canonical status is 200");
        // The reduced body still parses the consumed fields.
        assert_eq!(parse_account_info(&ok.body).unwrap().sequence, 42);
    }

    #[test]
    fn transform_tx_reduces_and_converges() {
        let mk = |edge: &str| TransformArgs {
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
    fn rpc_url_selects_network_by_key() {
        assert_eq!(rpc_url("key_1"), MAINNET_URL);
        assert_eq!(rpc_url("test_key_1"), TESTNET_URL);
    }
}
