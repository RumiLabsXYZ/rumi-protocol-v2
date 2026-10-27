//! Typed read-only Stellar Horizon HTTPS outcalls.
//!
//! These functions use one fixed Horizon provider selected by the closed
//! [`Network`] enum. They do not establish independent-provider quorum or a
//! common ledger snapshot across calls. Callers must treat the returned
//! account, reserve, transaction, and operation reads as separate observations.
//! There is intentionally no transaction submission or deposit-credit path here.

use ic_cdk::api::management_canister::http_request::{http_request, HttpResponse};
use serde_json::Value;

use super::{
    codec::Network,
    horizon_parse::{self, AccountSnapshot, FinalizedDeposit, HorizonParseError, LedgerReserve},
    horizon_rpc,
};

/// Match the established XRP read outcall budget. Horizon requests are GETs
/// with explicit response byte limits in `horizon_rpc`.
pub const READ_CYCLES: u128 = 2_000_000_000;
/// The operations request declares a 256 KiB raw response cap. Under legacy
/// HTTPS pricing the declared maximum is charged even when fewer bytes arrive;
/// 2B cycles would not cover that cap even on a 13-node subnet. This 10B
/// attachment covers the current 34-node reference pricing plus base/request
/// cost, with the unused attachment refunded by the management canister.
pub const OPERATIONS_READ_CYCLES: u128 = 10_000_000_000;

fn require_success(response: HttpResponse) -> Result<Vec<u8>, String> {
    if response.status != candid::Nat::from(200u16) {
        return Err(format!(
            "Horizon HTTP request returned status {}",
            response.status
        ));
    }
    Ok(response.body)
}

async fn read(
    request: ic_cdk::api::management_canister::http_request::CanisterHttpRequestArgument,
    cycles: u128,
) -> Result<Vec<u8>, String> {
    let (response,) = http_request(request, cycles)
        .await
        .map_err(|(code, message)| {
            format!("Horizon HTTPS outcall rejected ({code:?}): {message}")
        })?;
    require_success(response)
}

fn parse_error(error: HorizonParseError) -> String {
    format!("Horizon response failed validation: {error:?}")
}

/// Fetch and validate the account's native XLM balance and sequence snapshot.
pub async fn fetch_account_snapshot(
    network: Network,
    account_id: &str,
) -> Result<AccountSnapshot, String> {
    let request = horizon_rpc::account_request(network, account_id)
        .ok_or_else(|| "invalid Stellar classic account address".to_string())?;
    let body = read(request, READ_CYCLES).await?;
    horizon_parse::parse_account(&body, account_id).map_err(parse_error)
}

/// Fetch the newest ledger returned by Horizon and validate its reserve value.
/// The sequence is extracted only to bind the strict reserve parser to the
/// sequence returned in the same response.
pub async fn fetch_latest_ledger_reserve(network: Network) -> Result<LedgerReserve, String> {
    let body = read(
        horizon_rpc::latest_ledger_reserve_request(network),
        READ_CYCLES,
    )
    .await?;
    parse_latest_ledger_reserve(&body)
}

fn parse_latest_ledger_reserve(body: &[u8]) -> Result<LedgerReserve, String> {
    let reduced: Value = serde_json::from_slice(&body)
        .map_err(|error| format!("Horizon latest ledger JSON is invalid: {error}"))?;
    let sequence = reduced
        .get("sequence")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| "Horizon latest ledger has no valid sequence".to_string())?;
    horizon_parse::parse_ledger_reserve(&body, sequence).map_err(parse_error)
}

/// Fetch a transaction by hash and return its ledger only when it is
/// successful according to Horizon and the returned hash matches the request.
pub async fn fetch_successful_transaction_ledger(
    network: Network,
    transaction_hash: &str,
) -> Result<u32, String> {
    let request = horizon_rpc::transaction_request(network, transaction_hash)
        .ok_or_else(|| "invalid Stellar transaction hash".to_string())?;
    let body = read(request, READ_CYCLES).await?;
    horizon_parse::parse_transaction(&body, transaction_hash).map_err(parse_error)
}

/// Fetch and validate a bounded operation page for a successful transaction.
/// A full page is rejected by the strict parser as potentially truncated.
#[allow(clippy::too_many_arguments)]
pub async fn fetch_deposit_operations(
    network: Network,
    transaction_hash: &str,
    expected_ledger: u32,
    expected_destination: &str,
    expected_amount_stroops: u64,
    page_limit: usize,
) -> Result<FinalizedDeposit, String> {
    let request =
        horizon_rpc::transaction_operations_request(network, transaction_hash, page_limit)
            .ok_or_else(|| {
                "invalid Stellar transaction hash or operations page limit".to_string()
            })?;
    let body = read(request, OPERATIONS_READ_CYCLES).await?;
    horizon_parse::parse_deposit_operations(
        &body,
        transaction_hash,
        expected_ledger,
        expected_destination,
        expected_amount_stroops,
        page_limit,
    )
    .map_err(parse_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_status_gate_accepts_only_exact_success() {
        let response = |status: u16| HttpResponse {
            status: candid::Nat::from(status),
            headers: vec![],
            body: b"{}".to_vec(),
        };
        assert_eq!(require_success(response(200)).unwrap(), b"{}");
        assert!(require_success(response(204)).is_err());
        assert!(require_success(response(404)).is_err());
    }

    #[test]
    fn latest_ledger_sequence_is_required_and_bounded() {
        let valid = br#"{"sequence":42,"base_reserve_in_stroops":5000000}"#;
        assert_eq!(parse_latest_ledger_reserve(valid).unwrap().sequence, 42);
        assert!(parse_latest_ledger_reserve(
            br#"{"sequence":4294967296,"base_reserve_in_stroops":5000000}"#
        )
        .is_err());
        assert!(parse_latest_ledger_reserve(
            br#"{"sequence":"42","base_reserve_in_stroops":5000000}"#
        )
        .is_err());
    }
}
