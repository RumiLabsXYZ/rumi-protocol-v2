//! Issuer-side XRPL access for the token lane: explicit-network `account_info`
//! (sequence / owner count / flags / transfer rate), the base fee, an
//! owner-directory probe, transaction outcomes for ANY transaction type
//! (AccountSet setup steps as well as Payments), signed-blob submission, and the
//! complete-history evidence used to prove that an expired transaction was never
//! included.
//!
//! Same rules as `issued_rpc`: every function takes an explicit [`XrplNetwork`]
//! (a fixture build cannot reach mainnet), every `transform_*` is a strict reducer,
//! reads need a `READ_QUORUM` of identical reduced facts, and nothing here decides
//! anything: it only produces evidence for the pure logic in
//! `xrpl_icusd::settlement`.

use ic_cdk::api::management_canister::http_request::{
    http_request, CanisterHttpRequestArgument, HttpResponse, TransformArgs,
};
use serde_json::{json, Value};

use super::issued_amount::IssuedAsset;
use super::issued_rpc::{
    check_account, history_providers_covering, is_hex64, parse_reduced_tx, read_with_quorum, s,
    u32_of, verdict, IssuedRpcError, ReceiptLookup, ServerInfo, MAX_ISSUED_TX_BYTES, TX_CYCLES,
};
use super::network::{read_provider_urls_for, XrplNetwork};
use super::xrp_rpc::{
    build_request_to_url, reduced_response, MAX_READ_BYTES, MAX_SUBMIT_BYTES, READ_CYCLES,
    READ_QUORUM, SUBMIT_CYCLES,
};

pub const TRANSFORM_ISSUER_ACCOUNT: &str = "xrp_transform_issuer_account";
pub const TRANSFORM_FEE: &str = "xrp_transform_fee";
pub const TRANSFORM_OBJECTS: &str = "xrp_transform_issuer_objects";
/// The issued-tx reducer is reused verbatim for any transaction type.
pub const TRANSFORM_TX_ANY: &str = super::issued_rpc::TRANSFORM_ISSUED_TX;
pub const TRANSFORM_SUBMIT: &str = "xrp_transform_submit";

/// `lsfDefaultRipple`, `lsfRequireDestTag`, `lsfNoFreeze`, `lsfAllowTrustLineClawback`.
pub const LSF_DEFAULT_RIPPLE: u32 = 0x0080_0000;
pub const LSF_REQUIRE_DEST_TAG: u32 = 0x0002_0000;
pub const LSF_NO_FREEZE: u32 = 0x0020_0000;
pub const LSF_ALLOW_TRUSTLINE_CLAWBACK: u32 = 0x8000_0000;

// ---- account_info -----------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuerAccountInfo {
    pub exists: bool,
    pub sequence: u32,
    pub owner_count: u32,
    pub flags: u32,
    /// `None` when the field is absent (default, i.e. no surcharge).
    pub transfer_rate: Option<u32>,
    pub balance_drops: u128,
    /// Validated ledger the answer was read at.
    pub ledger_index: u32,
}

pub fn account_info_plan(url: &str, account: &str) -> CanisterHttpRequestArgument {
    build_request_to_url(
        url,
        "account_info",
        json!({ "account": account, "ledger_index": "validated", "strict": true }),
        MAX_READ_BYTES,
        TRANSFORM_ISSUER_ACCOUNT,
    )
}

pub fn reduce_issuer_account_body(body: &[u8]) -> Vec<u8> {
    let reduced = match serde_json::from_slice::<Value>(body) {
        Ok(v) => {
            let r = v.get("result");
            json!({
                "sequence": r.and_then(|r| r.pointer("/account_data/Sequence")),
                "owner_count": r.and_then(|r| r.pointer("/account_data/OwnerCount")),
                "flags": r.and_then(|r| r.pointer("/account_data/Flags")),
                "transfer_rate": r.and_then(|r| r.pointer("/account_data/TransferRate")),
                "balance": r.and_then(|r| r.pointer("/account_data/Balance")),
                "ledger_index": r.and_then(|r| r.get("ledger_index")),
                "validated": r.and_then(|r| r.get("validated")),
                "error": r.and_then(|r| r.get("error")),
            })
        }
        Err(_) => json!({ "parse_error": true }),
    };
    reduced.to_string().into_bytes()
}

pub fn transform_issuer_account(args: TransformArgs) -> HttpResponse {
    reduced_response(reduce_issuer_account_body(&args.response.body))
}

pub fn parse_issuer_account(body: &[u8]) -> Result<IssuerAccountInfo, IssuedRpcError> {
    let v: Value =
        serde_json::from_slice(body).map_err(|e| IssuedRpcError::Unparseable(e.to_string()))?;
    if v.get("parse_error").and_then(Value::as_bool) == Some(true) {
        return Err(IssuedRpcError::Unparseable("account_info".into()));
    }
    let ledger_index = u32_of(v.get("ledger_index"));
    match v.get("error").and_then(Value::as_str) {
        Some("actNotFound") => {
            return Ok(IssuerAccountInfo {
                exists: false,
                sequence: 0,
                owner_count: 0,
                flags: 0,
                transfer_rate: None,
                balance_drops: 0,
                ledger_index: ledger_index.unwrap_or(0),
            })
        }
        Some(e) => return Err(IssuedRpcError::Unparseable(format!("account_info {e}"))),
        None => {}
    }
    // Only a VALIDATED ledger counts as account state.
    if v.get("validated").and_then(Value::as_bool) != Some(true) {
        return Err(IssuedRpcError::Unparseable("account_info not validated".into()));
    }
    let need = |k: &str| {
        u32_of(v.get(k)).ok_or_else(|| IssuedRpcError::Unparseable(format!("account_info {k}")))
    };
    let balance_drops = v
        .get("balance")
        .and_then(|b| b.as_str().and_then(|s| s.parse::<u128>().ok()))
        .ok_or_else(|| IssuedRpcError::Unparseable("account_info balance".into()))?;
    Ok(IssuerAccountInfo {
        exists: true,
        sequence: need("sequence")?,
        owner_count: need("owner_count")?,
        flags: need("flags")?,
        transfer_rate: u32_of(v.get("transfer_rate")),
        balance_drops,
        ledger_index: ledger_index
            .ok_or_else(|| IssuedRpcError::Unparseable("account_info ledger_index".into()))?,
    })
}

pub fn quorum_issuer_account(
    provider_bodies: &[(String, Vec<u8>)],
) -> Result<IssuerAccountInfo, IssuedRpcError> {
    if provider_bodies.len() < READ_QUORUM {
        return Err(IssuedRpcError::NoQuorum("account_info: too few providers".into()));
    }
    let parsed: Vec<IssuerAccountInfo> = provider_bodies
        .iter()
        .filter_map(|(_, b)| parse_issuer_account(b).ok())
        .collect();
    for cand in &parsed {
        let same = |o: &&IssuerAccountInfo| {
            o.exists == cand.exists
                && o.sequence == cand.sequence
                && o.owner_count == cand.owner_count
                && o.flags == cand.flags
                && o.transfer_rate == cand.transfer_rate
                && o.balance_drops == cand.balance_drops
        };
        let votes: Vec<&IssuerAccountInfo> = parsed.iter().filter(same).collect();
        if votes.len() >= READ_QUORUM {
            let mut out = cand.clone();
            // The conservative (lowest) agreeing ledger: every provider has it.
            out.ledger_index = votes.iter().map(|o| o.ledger_index).min().unwrap_or(0);
            return Ok(out);
        }
    }
    Err(IssuedRpcError::NoQuorum("account_info: no identical majority".into()))
}

/// Issuer account state at the newest validated ledger (semantic quorum).
pub async fn fetch_issuer_account(
    network: XrplNetwork,
    account: &str,
) -> Result<IssuerAccountInfo, IssuedRpcError> {
    check_account(account)?;
    let urls = read_provider_urls_for(network)?;
    read_with_quorum(
        urls,
        |url| account_info_plan(url, account),
        READ_CYCLES,
        |bodies| quorum_issuer_account(bodies).map_err(verdict),
    )
    .await
}

// ---- fee -----------------------------------------------------------------------------

pub fn fee_plan(url: &str) -> CanisterHttpRequestArgument {
    build_request_to_url(url, "fee", json!({}), MAX_READ_BYTES, TRANSFORM_FEE)
}

/// Only the (stable) reference fee is kept: the escalating open-ledger fee changes
/// per ledger and would break replica consensus.
pub fn reduce_fee_body(body: &[u8]) -> Vec<u8> {
    let reduced = match serde_json::from_slice::<Value>(body) {
        Ok(v) => json!({
            "base_fee": v.pointer("/result/drops/base_fee"),
            "error": v.pointer("/result/error"),
        }),
        Err(_) => json!({ "parse_error": true }),
    };
    reduced.to_string().into_bytes()
}

pub fn transform_fee(args: TransformArgs) -> HttpResponse {
    reduced_response(reduce_fee_body(&args.response.body))
}

pub fn parse_base_fee(body: &[u8]) -> Result<u64, IssuedRpcError> {
    let v: Value =
        serde_json::from_slice(body).map_err(|e| IssuedRpcError::Unparseable(e.to_string()))?;
    if v.get("parse_error").and_then(Value::as_bool) == Some(true)
        || v.get("error").is_some_and(|e| !e.is_null())
    {
        return Err(IssuedRpcError::Unparseable("fee".into()));
    }
    v.get("base_fee")
        .and_then(|f| f.as_str().and_then(|s| s.parse::<u64>().ok()))
        .filter(|f| *f > 0)
        .ok_or_else(|| IssuedRpcError::Unparseable("fee base_fee".into()))
}

pub fn quorum_base_fee(provider_bodies: &[(String, Vec<u8>)]) -> Result<u64, IssuedRpcError> {
    if provider_bodies.len() < READ_QUORUM {
        return Err(IssuedRpcError::NoQuorum("fee: too few providers".into()));
    }
    let parsed: Vec<u64> = provider_bodies
        .iter()
        .filter_map(|(_, b)| parse_base_fee(b).ok())
        .collect();
    for c in &parsed {
        if parsed.iter().filter(|o| *o == c).count() >= READ_QUORUM {
            return Ok(*c);
        }
    }
    Err(IssuedRpcError::NoQuorum("fee: no identical majority".into()))
}

pub async fn fetch_base_fee(network: XrplNetwork) -> Result<u64, IssuedRpcError> {
    let urls = read_provider_urls_for(network)?;
    read_with_quorum(urls, fee_plan, READ_CYCLES, |bodies| {
        quorum_base_fee(bodies).map_err(verdict)
    })
    .await
}

// ---- account reserve ------------------------------------------------------------------

pub fn reserve_plan(url: &str) -> CanisterHttpRequestArgument {
    build_request_to_url(
        url,
        "server_state",
        json!({}),
        MAX_READ_BYTES,
        // The existing reducer keeps only `reserve_base` (stable across replicas).
        "xrp_transform_server",
    )
}

pub fn quorum_reserve_base(provider_bodies: &[(String, Vec<u8>)]) -> Result<u128, IssuedRpcError> {
    if provider_bodies.len() < READ_QUORUM {
        return Err(IssuedRpcError::NoQuorum("reserve: too few providers".into()));
    }
    let parsed: Vec<u128> = provider_bodies
        .iter()
        .filter_map(|(_, b)| super::xrp_rpc::parse_reserve_base(b).ok())
        .collect();
    for c in &parsed {
        if parsed.iter().filter(|o| *o == c).count() >= READ_QUORUM {
            return Ok(*c);
        }
    }
    Err(IssuedRpcError::NoQuorum("reserve: no identical majority".into()))
}

/// Base account reserve (drops) for an explicit network.
pub async fn fetch_reserve_base(network: XrplNetwork) -> Result<u128, IssuedRpcError> {
    let urls = read_provider_urls_for(network)?;
    read_with_quorum(urls, reserve_plan, READ_CYCLES, |bodies| {
        quorum_reserve_base(bodies).map_err(verdict)
    })
    .await
}

/// Conservative spendability: after paying `fee_drops` the issuer must still hold the
/// base reserve plus one base reserve per owned object (the real owner increment is
/// smaller, so this over-reserves rather than risking a `tecINSUFFICIENT_RESERVE`
/// that would burn the fee).
pub fn issuer_can_pay(
    info: &IssuerAccountInfo,
    fee_drops: u64,
    reserve_base_drops: u128,
) -> Result<(), String> {
    if !info.exists {
        return Err("the issuer account is not funded".into());
    }
    let need = reserve_base_drops
        .saturating_mul(1 + u128::from(info.owner_count))
        .saturating_add(u128::from(fee_drops));
    if info.balance_drops < need {
        return Err(format!(
            "issuer balance {} drops is below the required {} drops (reserve + fee)",
            info.balance_drops, need
        ));
    }
    Ok(())
}

// ---- owner directory probe ---------------------------------------------------------------

pub fn objects_plan(url: &str, account: &str) -> CanisterHttpRequestArgument {
    build_request_to_url(
        url,
        "account_objects",
        json!({ "account": account, "ledger_index": "validated", "limit": 1 }),
        MAX_READ_BYTES,
        TRANSFORM_OBJECTS,
    )
}

pub fn reduce_objects_body(body: &[u8]) -> Vec<u8> {
    let reduced = match serde_json::from_slice::<Value>(body) {
        Ok(v) => {
            let objs = v
                .pointer("/result/account_objects")
                .and_then(Value::as_array);
            json!({
                "has_objects": objs.map(|a| !a.is_empty()),
                "error": v.pointer("/result/error"),
            })
        }
        Err(_) => json!({ "parse_error": true }),
    };
    reduced.to_string().into_bytes()
}

pub fn transform_objects(args: TransformArgs) -> HttpResponse {
    reduced_response(reduce_objects_body(&args.response.body))
}

/// `Ok(true)` when the issuer owns at least one ledger object. An unfunded
/// account (`actNotFound`) owns none.
pub fn parse_has_objects(body: &[u8]) -> Result<bool, IssuedRpcError> {
    let v: Value =
        serde_json::from_slice(body).map_err(|e| IssuedRpcError::Unparseable(e.to_string()))?;
    if v.get("parse_error").and_then(Value::as_bool) == Some(true) {
        return Err(IssuedRpcError::Unparseable("account_objects".into()));
    }
    match v.get("error").and_then(Value::as_str) {
        Some("actNotFound") => return Ok(false),
        Some(e) => return Err(IssuedRpcError::Unparseable(format!("account_objects {e}"))),
        None => {}
    }
    v.get("has_objects")
        .and_then(Value::as_bool)
        .ok_or_else(|| IssuedRpcError::Unparseable("account_objects has_objects".into()))
}

pub fn quorum_has_objects(provider_bodies: &[(String, Vec<u8>)]) -> Result<bool, IssuedRpcError> {
    if provider_bodies.len() < READ_QUORUM {
        return Err(IssuedRpcError::NoQuorum("objects: too few providers".into()));
    }
    let parsed: Vec<bool> = provider_bodies
        .iter()
        .filter_map(|(_, b)| parse_has_objects(b).ok())
        .collect();
    for c in &parsed {
        if parsed.iter().filter(|o| *o == c).count() >= READ_QUORUM {
            return Ok(*c);
        }
    }
    Err(IssuedRpcError::NoQuorum("objects: no identical majority".into()))
}

pub async fn fetch_has_owner_objects(
    network: XrplNetwork,
    account: &str,
) -> Result<bool, IssuedRpcError> {
    check_account(account)?;
    let urls = read_provider_urls_for(network)?;
    read_with_quorum(
        urls,
        |url| objects_plan(url, account),
        READ_CYCLES,
        |bodies| quorum_has_objects(bodies).map_err(verdict),
    )
    .await
}

// ---- transaction outcome (any type) -------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TxOutcome {
    NotFound,
    /// Found but not (yet) in a validated ledger.
    NotValidated,
    /// Validated with a non-`tesSUCCESS` result (a `tec*` still consumed the
    /// sequence).
    Failed(String),
    Succeeded {
        ledger_index: u32,
        tx_type: String,
        account: String,
    },
}

/// `tx` params for the any-type outcome lookup (API v2, see `RIPPLED_API_VERSION`).
pub fn tx_outcome_params(hash: &str) -> Value {
    json!({ "transaction": hash, "binary": false, "api_version": super::issued_rpc::RIPPLED_API_VERSION })
}

pub fn tx_outcome_plan(url: &str, hash: &str) -> CanisterHttpRequestArgument {
    build_request_to_url(
        url,
        "tx",
        tx_outcome_params(hash),
        MAX_ISSUED_TX_BYTES,
        TRANSFORM_TX_ANY,
    )
}

pub fn parse_tx_outcome(body: &[u8], expected_hash: &str) -> Result<TxOutcome, IssuedRpcError> {
    let v: Value =
        serde_json::from_slice(body).map_err(|e| IssuedRpcError::Unparseable(e.to_string()))?;
    if v.get("parse_error").and_then(Value::as_bool) == Some(true) {
        return Err(IssuedRpcError::Unparseable("tx".into()));
    }
    match v.get("error").and_then(Value::as_str) {
        Some("txnNotFound") => return Ok(TxOutcome::NotFound),
        Some(e) => return Err(IssuedRpcError::Unparseable(format!("tx rpc error {e}"))),
        None => {}
    }
    let tx = v
        .get("tx")
        .ok_or_else(|| IssuedRpcError::Unparseable("tx missing".into()))?;
    let p = parse_reduced_tx(tx).map_err(IssuedRpcError::Unparseable)?;
    if !p.hash.eq_ignore_ascii_case(expected_hash) {
        return Err(IssuedRpcError::Unparseable(format!(
            "tx hash mismatch: expected {expected_hash}, got {}",
            p.hash
        )));
    }
    if !p.validated {
        return Ok(TxOutcome::NotValidated);
    }
    if p.engine_result != "tesSUCCESS" {
        return Ok(TxOutcome::Failed(p.engine_result));
    }
    Ok(TxOutcome::Succeeded {
        ledger_index: p.ledger_index,
        tx_type: p.tx_type,
        account: p.account,
    })
}

pub fn quorum_tx_outcome(
    provider_bodies: &[(String, Vec<u8>)],
    expected_hash: &str,
) -> Result<TxOutcome, IssuedRpcError> {
    if provider_bodies.len() < READ_QUORUM {
        return Err(IssuedRpcError::NoQuorum("tx: too few providers".into()));
    }
    let parsed: Vec<TxOutcome> = provider_bodies
        .iter()
        .filter_map(|(_, b)| parse_tx_outcome(b, expected_hash).ok())
        .collect();
    for c in &parsed {
        if parsed.iter().filter(|o| *o == c).count() >= READ_QUORUM {
            return Ok(c.clone());
        }
    }
    Err(IssuedRpcError::NoQuorum("tx: no identical majority".into()))
}

/// Outcome of `hash`, optionally restricted to the given provider URLs (used so that
/// a "not found" answer only counts when it comes from history-complete providers).
pub async fn fetch_tx_outcome(
    network: XrplNetwork,
    hash: &str,
    only_urls: Option<&[String]>,
) -> Result<TxOutcome, IssuedRpcError> {
    if !is_hex64(hash) {
        return Err(IssuedRpcError::InvalidInput("tx hash must be 64 hex".into()));
    }
    let hash = hash.to_ascii_uppercase();
    let all = read_provider_urls_for(network)?;
    let urls = restrict_urls(all, only_urls);
    read_with_quorum(
        &urls,
        |url| tx_outcome_plan(url, &hash),
        TX_CYCLES,
        |bodies| quorum_tx_outcome(bodies, &hash).map_err(verdict),
    )
    .await
}

fn restrict_urls<'a>(all: &[&'a str], only: Option<&[String]>) -> Vec<&'a str> {
    match only {
        None => all.to_vec(),
        Some(o) => all
            .iter()
            .copied()
            .filter(|u| o.iter().any(|x| x == u))
            .collect(),
    }
}

/// Payment receipt (exact delivered amount, destination, tag, ...) restricted to a
/// provider subset. Same quorum rules as `issued_rpc::fetch_issued_receipt`.
pub async fn fetch_receipt_via(
    network: XrplNetwork,
    hash: &str,
    asset: &IssuedAsset,
    only_urls: Option<&[String]>,
) -> Result<ReceiptLookup, IssuedRpcError> {
    let hash = super::issued_rpc::normalize_tx_hash(hash)?;
    let all = read_provider_urls_for(network)?;
    let urls = restrict_urls(all, only_urls);
    read_with_quorum(
        &urls,
        |url| super::issued_rpc::issued_tx_request_to_url(url, &hash).into_http(),
        TX_CYCLES,
        |bodies| {
            super::issued_rpc::quorum_issued_receipt(bodies, &hash, asset).map_err(verdict)
        },
    )
    .await
}

// ---- complete-history evidence -------------------------------------------------------------------

/// Evidence that providers hold COMPLETE history for `[history_from, history_to]`
/// and that the validated ledger has passed `last_ledger_sequence`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpiryEvidence {
    pub validated_ledger: u32,
    pub history_from: u32,
    pub history_to: u32,
    /// Provider URLs that attest the coverage (the lookup must come from these).
    pub providers: Vec<String>,
}

/// Pure: pick providers that are validated past `last_ledger_sequence` AND whose
/// `complete_ledgers` cover `[from_ledger, last_ledger_sequence + 1]`. Fewer than
/// `READ_QUORUM` such providers is a typed, recoverable `HistoryGap`.
pub fn expiry_evidence_from_infos(
    infos: &[(String, Result<ServerInfo, IssuedRpcError>)],
    from_ledger: u32,
    last_ledger_sequence: u32,
) -> Result<ExpiryEvidence, IssuedRpcError> {
    let to = last_ledger_sequence.saturating_add(1);
    let covering = history_providers_covering(infos, from_ledger, to)?;
    let validated_ledger = infos
        .iter()
        .filter(|(u, _)| covering.contains(u))
        .filter_map(|(_, i)| i.as_ref().ok().map(|i| i.validated_seq))
        .min()
        .unwrap_or(0);
    Ok(ExpiryEvidence {
        validated_ledger,
        history_from: from_ledger,
        history_to: to,
        providers: covering,
    })
}

pub async fn fetch_expiry_evidence(
    network: XrplNetwork,
    from_ledger: u32,
    last_ledger_sequence: u32,
) -> Result<ExpiryEvidence, IssuedRpcError> {
    let urls = read_provider_urls_for(network)?;
    let infos = super::issued_rpc::fetch_server_infos(urls).await;
    expiry_evidence_from_infos(&infos, from_ledger, last_ledger_sequence)
}

// ---- submission -------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubmitOutcome {
    /// `tes*` / `ter*` / `tec*`-queued: accepted for relay or applied.
    Accepted {
        engine_result: String,
        hash: Option<String>,
    },
    /// A definite engine rejection BY THAT SERVER. Never proof that the transaction
    /// cannot execute (another node may relay it): reconciliation decides.
    Rejected { engine_result: String },
    RpcError(String),
    Transport(String),
}

pub fn submit_plan(url: &str, blob_hex: &str) -> CanisterHttpRequestArgument {
    build_request_to_url(
        url,
        "submit",
        json!({ "tx_blob": blob_hex }),
        MAX_SUBMIT_BYTES,
        TRANSFORM_SUBMIT,
    )
}

pub fn parse_submit_body(body: &[u8]) -> SubmitOutcome {
    let Ok(v) = serde_json::from_slice::<Value>(body) else {
        return SubmitOutcome::RpcError("unparseable submit response".into());
    };
    if v.get("parse_error").and_then(Value::as_bool) == Some(true) {
        return SubmitOutcome::RpcError("unparseable submit response".into());
    }
    if let Some(e) = v.get("error").and_then(Value::as_str) {
        return SubmitOutcome::RpcError(e.to_string());
    }
    let engine = s(v.get("engine_result")).unwrap_or_default();
    let hash = s(v.get("hash"));
    if engine.starts_with("tes") || engine.starts_with("ter") {
        SubmitOutcome::Accepted {
            engine_result: engine,
            hash,
        }
    } else {
        SubmitOutcome::Rejected {
            engine_result: engine,
        }
    }
}

/// Submit the SAME signed blob to the providers in order until one answers. Duplicate
/// relay of identical bytes is harmless (reliable-submission rules). A transport
/// failure on every provider is `Transport` (ambiguous: the blob may have been
/// relayed).
pub async fn submit_signed_blob(network: XrplNetwork, blob_hex: &str) -> SubmitOutcome {
    let urls = match read_provider_urls_for(network) {
        Ok(u) => u,
        Err(e) => return SubmitOutcome::RpcError(e.to_string()),
    };
    let mut last = String::new();
    for url in urls {
        match http_request(submit_plan(url, blob_hex), SUBMIT_CYCLES).await {
            Ok((resp,)) => return parse_submit_body(&resp.body),
            Err((code, msg)) => last = format!("{url}: {code:?}: {msg}"),
        }
    }
    SubmitOutcome::Transport(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(_seq: u32, complete: &str, tip: u32) -> Result<ServerInfo, IssuedRpcError> {
        Ok(ServerInfo {
            complete_ledgers: complete.into(),
            validated_seq: tip,
            validated_hash: "A".repeat(64),
        })
    }

    #[test]
    fn issuer_account_parses_flags_and_quorum() {
        let body = reduce_issuer_account_body(
            br#"{"result":{"account_data":{"Sequence":7,"OwnerCount":0,"Flags":8388608,"TransferRate":1000000000,"Balance":"50000000"},"ledger_index":100,"validated":true}}"#,
        );
        let a = parse_issuer_account(&body).unwrap();
        assert_eq!(a.sequence, 7);
        assert_eq!(a.owner_count, 0);
        assert_eq!(a.flags & LSF_DEFAULT_RIPPLE, LSF_DEFAULT_RIPPLE);
        assert_eq!(a.transfer_rate, Some(1_000_000_000));
        let q = quorum_issuer_account(&[
            ("a".into(), body.clone()),
            ("b".into(), body.clone()),
        ])
        .unwrap();
        assert_eq!(q, a);
        // a lone provider is not a quorum
        assert!(quorum_issuer_account(&[("a".into(), body)]).is_err());
    }

    #[test]
    fn unvalidated_account_info_is_refused() {
        let body = reduce_issuer_account_body(
            br#"{"result":{"account_data":{"Sequence":7,"OwnerCount":0,"Flags":0,"Balance":"1"},"ledger_index":100,"validated":false}}"#,
        );
        assert!(parse_issuer_account(&body).is_err());
    }

    #[test]
    fn act_not_found_is_nonexistent_not_error() {
        let body = reduce_issuer_account_body(
            br#"{"result":{"error":"actNotFound","ledger_index":55}}"#,
        );
        let a = parse_issuer_account(&body).unwrap();
        assert!(!a.exists);
    }

    #[test]
    fn fee_and_objects_reducers_keep_only_stable_fields() {
        let f = reduce_fee_body(br#"{"result":{"drops":{"base_fee":"10","open_ledger_fee":"999"}}}"#);
        assert_eq!(parse_base_fee(&f).unwrap(), 10);
        assert!(!String::from_utf8(f).unwrap().contains("999"));
        let o = reduce_objects_body(br#"{"result":{"account_objects":[{"x":1}]}}"#);
        assert!(parse_has_objects(&o).unwrap());
        let o = reduce_objects_body(br#"{"result":{"account_objects":[]}}"#);
        assert!(!parse_has_objects(&o).unwrap());
        let o = reduce_objects_body(br#"{"result":{"error":"actNotFound"}}"#);
        assert!(!parse_has_objects(&o).unwrap());
    }

    #[test]
    fn issuer_must_be_able_to_pay_fee_above_reserve() {
        let mut i = IssuerAccountInfo {
            exists: true,
            sequence: 1,
            owner_count: 0,
            flags: 0,
            transfer_rate: None,
            balance_drops: 10_000_012,
            ledger_index: 5,
        };
        assert!(issuer_can_pay(&i, 12, 10_000_000).is_ok());
        assert!(issuer_can_pay(&i, 13, 10_000_000).is_err());
        i.owner_count = 1;
        assert!(issuer_can_pay(&i, 12, 10_000_000).is_err());
        i.exists = false;
        assert!(issuer_can_pay(&i, 1, 1).is_err());
        let body = br#"{"reserve_base":10000000}"#.to_vec();
        assert_eq!(
            quorum_reserve_base(&[("a".into(), body.clone()), ("b".into(), body.clone())]).unwrap(),
            10_000_000
        );
        assert!(quorum_reserve_base(&[("a".into(), body)]).is_err());
    }

    #[test]
    fn expiry_evidence_requires_complete_history_and_validated_past_lls() {
        let infos = vec![
            ("p1".to_string(), info(0, "100-900", 700)),
            ("p2".to_string(), info(0, "100-900", 710)),
            ("p3".to_string(), info(0, "650-900", 710)),
        ];
        // lls = 600: p1/p2 cover [500, 601] and are validated past it; p3 does not
        // cover 500.
        let ev = expiry_evidence_from_infos(&infos, 500, 600).unwrap();
        assert_eq!(ev.providers, vec!["p1".to_string(), "p2".to_string()]);
        assert_eq!(ev.validated_ledger, 700);
        // History starting before the providers' window: a typed gap, not evidence.
        assert!(matches!(
            expiry_evidence_from_infos(&infos, 50, 600),
            Err(IssuedRpcError::HistoryGap { .. })
        ));
        // Validated ledger not yet past lls.
        assert!(matches!(
            expiry_evidence_from_infos(&infos, 500, 705),
            Err(IssuedRpcError::HistoryGap { .. })
        ));
    }

    #[test]
    fn submit_parse_classifies_without_deciding() {
        assert!(matches!(
            parse_submit_body(br#"{"engine_result":"tesSUCCESS","hash":"AB"}"#),
            SubmitOutcome::Accepted { .. }
        ));
        assert!(matches!(
            parse_submit_body(br#"{"engine_result":"terQUEUED"}"#),
            SubmitOutcome::Accepted { .. }
        ));
        assert!(matches!(
            parse_submit_body(br#"{"engine_result":"tefPAST_SEQ"}"#),
            SubmitOutcome::Rejected { .. }
        ));
        assert!(matches!(
            parse_submit_body(b"not json"),
            SubmitOutcome::RpcError(_)
        ));
    }

    #[test]
    fn tx_outcome_distinguishes_all_states() {
        let h = "A".repeat(64);
        let mk = |validated: bool, result: &str| {
            format!(
                r#"{{"error":null,"tx":{{"hash":"{h}","ledger_index":9,"validated":{validated},"type":"AccountSet","account":"rX","result":"{result}","flags":0}}}}"#
            )
            .into_bytes()
        };
        assert_eq!(
            parse_tx_outcome(br#"{"error":"txnNotFound","tx":null}"#, &h).unwrap(),
            TxOutcome::NotFound
        );
        assert_eq!(
            parse_tx_outcome(&mk(false, "tesSUCCESS"), &h).unwrap(),
            TxOutcome::NotValidated
        );
        assert_eq!(
            parse_tx_outcome(&mk(true, "tecNO_PERMISSION"), &h).unwrap(),
            TxOutcome::Failed("tecNO_PERMISSION".into())
        );
        assert!(matches!(
            parse_tx_outcome(&mk(true, "tesSUCCESS"), &h).unwrap(),
            TxOutcome::Succeeded { .. }
        ));
        assert!(parse_tx_outcome(&mk(true, "tesSUCCESS"), &"B".repeat(64)).is_err());
    }
}
