//! Cluster-aware SOL RPC wrappers for native-SOL-collateral custody (mirrors
//! `chains::solana::sol_rpc`, but the cluster comes from `config::sol_cluster()`
//! — derived from the configured Schnorr key name — instead of a hardcoded
//! `SolanaCluster::Devnet`). See `chains::sol::config`'s module doc comment
//! for why key name and cluster are coupled through one predicate.
//!
//! Every candid request/response type, and every parser except
//! `parse_rent_exempt_minimum` (new here — the dormant M1 rail never needed a
//! rent-exempt read), is REUSED verbatim from `chains::solana::sol_rpc`: they
//! are already `pub`, cluster-agnostic, and pure, so redeclaring them would be
//! pure duplication. Only the cluster selection inside `json_request` differs.
//!
//! Consensus: reads demand `Equality` agreement — `Consistent(Ok)` only;
//! `Consistent(Err)` and `Inconsistent` both surface as `Err`. Reads use
//! commitment `finalized` (custody accepts the extra latency for
//! non-reversibility, same choice XRP makes reading at `validated`).
//! `send_transaction` is SINGLE-ATTEMPT and never blindly retried by this
//! module: a durable-nonce transaction that already landed must not be
//! re-signed and re-broadcast under a fresh nonce (see the settlement
//! idempotency table in the design doc §5.3).

use candid::Principal;
use solana_message::Hash;

use crate::chains::solana::sol_rpc::{
    build_send_transaction_payload, is_valid_tx_signature, parse_account_data_base64,
    parse_balance_lamports, parse_get_transaction, parse_latest_blockhash,
    parse_nonce_account_blockhash, parse_send_transaction_signature, parse_slot,
    text_from_request_result, ConsensusStrategy, MultiRequestResult, RpcConfig, RpcSources,
    TxStatus,
};

/// Production SOL RPC canister principal. Duplicated (not made `pub` in
/// `chains::solana::sol_rpc`) deliberately: it is a plain canister-id literal,
/// not cryptographic material, and one fiduciary canister serves every
/// cluster via the `RpcSources` request param (not a per-cluster canister
/// id), so this rail does not need to reach into the dormant rail's private
/// state to reuse it. VERIFY against the live repo before mainnet.
const SOL_RPC_PRINCIPAL: &str = "tghme-zyaaa-aaaar-qarca-cai";

/// Cycles attached per SOL RPC call. Mirrors `chains::solana::sol_rpc`'s
/// generous headroom (unused cycles are refunded).
pub const SOL_RPC_CALL_CYCLES: u128 = 10_000_000_000;

fn sol_rpc_principal() -> Principal {
    Principal::from_text(SOL_RPC_PRINCIPAL).expect("valid SOL RPC principal")
}

/// Send a JSON-RPC payload via the SOL RPC canister's `jsonRequest` escape
/// hatch, at the cluster implied by the CONFIGURED Schnorr key name
/// (`config::sol_cluster()`), with `Equality` consensus. Returns the
/// provider's response text.
async fn json_request(payload: &str) -> Result<String, String> {
    let sources = RpcSources::Default(super::config::sol_cluster());
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

/// Read a SOL balance (lamports) at `finalized`, demanding provider
/// agreement. `pubkey` MUST be a validated base58 address (callers validate
/// at the boundary), so interpolating it into the JSON payload cannot inject.
pub async fn get_balance(pubkey: &str) -> Result<u64, String> {
    let payload = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"getBalance","params":["{}",{{"commitment":"finalized"}}]}}"#,
        pubkey
    );
    let text = json_request(&payload).await?;
    parse_balance_lamports(&text)
}

/// Extract `result` (a bare u64, lamports) from a
/// `getMinimumBalanceForRentExemption` JSON-RPC response — the SAME nesting
/// shape as `getSlot` (not nested under `result.value` the way `getBalance`
/// is). New here: absent from `chains::solana::sol_rpc` (the dormant M1 rail
/// never needed a rent-exempt read).
pub fn parse_rent_exempt_minimum(json: &str) -> Result<u64, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("bad json: {e}"))?;
    if let Some(err) = v.get("error") {
        return Err(format!("json-rpc error: {err}"));
    }
    v.get("result").and_then(|r| r.as_u64()).ok_or_else(|| {
        format!("missing result (lamports) in getMinimumBalanceForRentExemption response: {json}")
    })
}

/// Read the network-wide rent-exempt minimum (lamports) for a 0-byte system
/// account via `getMinimumBalanceForRentExemption(0)`. This is a network
/// constant (not a per-slot value), so `Equality` consensus across providers
/// reaches agreement exactly like XRP's `server_state` reserve read — unlike
/// `getLatestBlockhash`, which changes every slot and is chronically
/// `Inconsistent` (see `chains::solana::sol_rpc::get_latest_blockhash`'s doc
/// comment; the durable-nonce design in this rail exists precisely to avoid
/// that class of read).
pub async fn get_rent_exempt_minimum() -> Result<u64, String> {
    let payload = r#"{"jsonrpc":"2.0","id":1,"method":"getMinimumBalanceForRentExemption","params":[0]}"#;
    let text = json_request(payload).await?;
    parse_rent_exempt_minimum(&text)
}

/// Read a fresh recent blockhash via `getLatestBlockhash` at `finalized`, at
/// the CLUSTER implied by the CONFIGURED Schnorr key (`config::sol_cluster()`).
/// Used only by `vault::sol_bootstrap_nonce_account`'s `None` (auto-fetch)
/// fallback: the create+initialize nonce-account transaction needs a REAL
/// recent blockhash, since the durable nonce does not exist yet to
/// self-reference. Same consensus caveat as
/// `chains::solana::sol_rpc::get_latest_blockhash`: this value changes every
/// slot, so multi-provider `Equality` consensus chronically returns
/// `#Inconsistent` (surfaced here as an error) on a real cluster. Retained
/// for PocketIC / other consensus-capable environments and as the documented
/// fallback; the production bootstrap path is an operator-supplied
/// `blockhash_override`, not this auto-fetch.
pub async fn get_latest_blockhash() -> Result<Hash, String> {
    let payload = r#"{"jsonrpc":"2.0","id":1,"method":"getLatestBlockhash","params":[{"commitment":"finalized"}]}"#;
    let text = json_request(payload).await?;
    let blockhash = parse_latest_blockhash(&text)?;
    Ok(Hash::new_from_array(blockhash))
}

/// Read a System nonce account's current durable nonce (a `Hash`) via
/// `getAccountInfo` with `base64` encoding at `finalized`. Errs if the
/// account is not found, is not exactly 80 bytes, or is not yet Initialized.
/// `nonce_pubkey` MUST be a validated/derived base58 address.
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

/// Broadcast a legacy wire transaction via `sendTransaction` and return the
/// transaction signature (a base58 string). Single attempt — never retried
/// blindly by this function (a durable-nonce tx that landed once must not be
/// re-broadcast after re-signing under a fresh nonce; the caller owns
/// idempotency, see the design doc §5.3 table).
pub async fn send_transaction(wire_tx: &[u8]) -> Result<String, String> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(wire_tx);
    let payload = build_send_transaction_payload(&b64);
    let text = json_request(&payload).await?;
    parse_send_transaction_signature(&text)
}

/// Look up a transaction by signature at `finalized` via `getTransaction` and
/// distill the result to a `TxStatus` (not-found / confirmed-with-slot /
/// failed). `signature` is validated as a 64-byte base58 Ed25519 signature
/// before interpolation.
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

/// Read the current slot at the given commitment via `getSlot`. `commitment`
/// is a fixed string supplied by this crate's callers (never user input), so
/// interpolating it is safe.
pub async fn get_slot(commitment: &str) -> Result<u64, String> {
    let payload = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"getSlot","params":[{{"commitment":"{}"}}]}}"#,
        commitment
    );
    let text = json_request(&payload).await?;
    parse_slot(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rent_exempt_minimum_reads_bare_result() {
        let json = r#"{"jsonrpc":"2.0","result":890880,"id":1}"#;
        assert_eq!(parse_rent_exempt_minimum(json).unwrap(), 890_880);
    }

    #[test]
    fn parse_rent_exempt_minimum_rejects_missing_result() {
        assert!(parse_rent_exempt_minimum(r#"{"jsonrpc":"2.0","id":1}"#).is_err());
    }

    #[test]
    fn parse_rent_exempt_minimum_surfaces_json_rpc_error() {
        let json = r#"{"jsonrpc":"2.0","error":{"code":-32600,"message":"bad"},"id":1}"#;
        assert!(parse_rent_exempt_minimum(json).is_err());
    }

    #[test]
    fn sol_rpc_principal_is_well_formed() {
        // Just guards against a typo in the literal breaking `Principal::from_text`.
        let _ = sol_rpc_principal();
    }
}
