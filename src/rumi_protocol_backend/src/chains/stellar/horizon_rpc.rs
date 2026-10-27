//! Read-only Stellar Horizon HTTPS transport.
//!
//! Requests are bounded and use deterministic response transforms so IC HTTPS
//! outcall consensus is computed over only stable fields consumed by
//! `horizon_parse`. This module deliberately has no transaction submission API
//! and does not credit deposits.

use ic_cdk::api::management_canister::http_request::{
    CanisterHttpRequestArgument, HttpMethod, HttpResponse, TransformArgs, TransformContext,
};
use serde_json::{json, Value};

use super::{address::is_valid_classic_address, codec::Network};

const MAINNET_URL: &str = "https://horizon.stellar.org";
const TESTNET_URL: &str = "https://horizon-testnet.stellar.org";

pub const MAX_ACCOUNT_RESPONSE_BYTES: u64 = 16_384;
pub const MAX_LEDGER_RESPONSE_BYTES: u64 = 4_096;
pub const MAX_LATEST_LEDGER_RESPONSE_BYTES: u64 = 8_192;
pub const MAX_TRANSACTION_RESPONSE_BYTES: u64 = 8_192;
pub const MAX_OPERATIONS_RESPONSE_BYTES: u64 = 262_144;
pub const OPERATIONS_PAGE_LIMIT: usize = 200;

impl Network {
    fn base_url(self) -> &'static str {
        match self {
            Self::Public => MAINNET_URL,
            Self::Test => TESTNET_URL,
        }
    }
}

pub(super) fn request_at(
    url: String,
    max_response_bytes: u64,
    transform_name: &str,
) -> CanisterHttpRequestArgument {
    CanisterHttpRequestArgument {
        url,
        method: HttpMethod::GET,
        body: None,
        max_response_bytes: Some(max_response_bytes),
        headers: vec![],
        transform: Some(TransformContext::from_name(
            transform_name.to_string(),
            vec![],
        )),
    }
}

fn account_url(network: Network, account_id: &str) -> Option<String> {
    if !is_valid_classic_address(account_id) {
        return None;
    }
    Some(format!("{}/accounts/{}", network.base_url(), account_id))
}

pub fn account_request(network: Network, account_id: &str) -> Option<CanisterHttpRequestArgument> {
    Some(request_at(
        account_url(network, account_id)?,
        MAX_ACCOUNT_RESPONSE_BYTES,
        "stellar_transform_account",
    ))
}

/// Build a latest-ledger list request against a caller-configured Horizon
/// origin. The origin is validated by the dormant provider fanout layer.
pub fn latest_ledger_reserve_request(base_url: &str) -> Option<CanisterHttpRequestArgument> {
    if !is_https_origin(base_url) {
        return None;
    }
    Some(request_at(
        format!(
            "{}/ledgers?order=desc&limit=1",
            base_url.trim_end_matches('/')
        ),
        MAX_LATEST_LEDGER_RESPONSE_BYTES,
        "stellar_transform_ledger",
    ))
}

fn is_https_origin(value: &str) -> bool {
    let Some(authority) = value.strip_prefix("https://") else {
        return false;
    };
    !authority.is_empty()
        && !authority.contains('/')
        && !authority.contains('?')
        && !authority.contains('#')
        && !authority.contains('@')
}

/// Request the ledger at the account body's last-modified ledger. This is a
/// historical reference only; it does not prove the current reserve.
fn ledger_reserve_url(network: Network, account_last_modified_ledger: u32) -> Option<String> {
    if account_last_modified_ledger == 0 {
        return None;
    }
    Some(format!(
        "{}/ledgers/{account_last_modified_ledger}",
        network.base_url()
    ))
}

pub fn ledger_reserve_request(
    network: Network,
    account_last_modified_ledger: u32,
) -> Option<CanisterHttpRequestArgument> {
    Some(request_at(
        ledger_reserve_url(network, account_last_modified_ledger)?,
        MAX_LEDGER_RESPONSE_BYTES,
        "stellar_transform_ledger",
    ))
}

fn transaction_url(network: Network, hash: &str) -> Option<String> {
    if !is_valid_transaction_hash(hash) {
        return None;
    }
    Some(format!("{}/transactions/{}", network.base_url(), hash))
}

pub fn transaction_request(network: Network, hash: &str) -> Option<CanisterHttpRequestArgument> {
    Some(request_at(
        transaction_url(network, hash)?,
        MAX_TRANSACTION_RESPONSE_BYTES,
        "stellar_transform_transaction",
    ))
}

fn is_valid_transaction_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Fetch a bounded page of transaction operations. The limit is explicit in
/// the URL and capped at the parser's page bound to preserve truncation checks.
fn transaction_operations_url(network: Network, hash: &str, limit: usize) -> Option<String> {
    if !is_valid_transaction_hash(hash) || limit == 0 || limit > OPERATIONS_PAGE_LIMIT {
        return None;
    }
    Some(format!(
        "{}/transactions/{}/operations?limit={}&order=asc",
        network.base_url(),
        hash,
        limit
    ))
}

pub fn transaction_operations_request(
    network: Network,
    hash: &str,
    limit: usize,
) -> Option<CanisterHttpRequestArgument> {
    Some(request_at(
        transaction_operations_url(network, hash, limit)?,
        MAX_OPERATIONS_RESPONSE_BYTES,
        "stellar_transform_operations",
    ))
}

fn reduced_response(status: candid::Nat, body: Vec<u8>) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![],
        body,
    }
}

fn parse_body(args: &TransformArgs) -> Result<Value, HttpResponse> {
    if args.response.status < candid::Nat::from(200u16)
        || args.response.status >= candid::Nat::from(300u16)
    {
        return Err(reduced_response(
            args.response.status.clone(),
            b"{\"http_error\":true}".to_vec(),
        ));
    }
    serde_json::from_slice(&args.response.body).map_err(|_| {
        reduced_response(
            candid::Nat::from(200u16),
            b"{\"parse_error\":true}".to_vec(),
        )
    })
}

fn field<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.get(key)
}

pub fn transform_account(args: TransformArgs) -> HttpResponse {
    let v = match parse_body(&args) {
        Ok(v) => v,
        Err(response) => return response,
    };
    let account_last_modified_ledger = v
        .get("last_modified_ledger")
        .and_then(Value::as_u64)
        .and_then(|ledger| u32::try_from(ledger).ok())
        .filter(|ledger| *ledger > 0);
    let Some(account_last_modified_ledger) = account_last_modified_ledger else {
        return reduced_response(
            candid::Nat::from(200u16),
            b"{\"parse_error\":true}".to_vec(),
        );
    };
    let balances = v
        .get("balances")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter(|row| row.get("asset_type").and_then(Value::as_str) == Some("native"))
                .map(|row| json!({ "asset_type": field(row, "asset_type"), "balance": field(row, "balance"), "selling_liabilities": field(row, "selling_liabilities") }))
                .collect::<Vec<_>>()
        });
    let reduced = json!({
        "account_id": field(&v, "account_id"),
        "sequence": field(&v, "sequence"),
        "account_last_modified_ledger": account_last_modified_ledger,
        "balances": balances,
        "subentry_count": field(&v, "subentry_count"),
        "num_sponsoring": field(&v, "num_sponsoring"),
        "num_sponsored": field(&v, "num_sponsored"),
    });
    reduced_response(candid::Nat::from(200u16), reduced.to_string().into_bytes())
}

pub fn transform_ledger(args: TransformArgs) -> HttpResponse {
    let v = match parse_body(&args) {
        Ok(v) => v,
        Err(response) => return response,
    };
    let reduced = if let Some(records) = v.pointer("/_embedded/records").and_then(Value::as_array) {
        if records.len() != 1 {
            return reduced_response(
                candid::Nat::from(200u16),
                b"{\"parse_error\":true}".to_vec(),
            );
        }
        let record = &records[0];
        json!({
            "sequence": field(record, "sequence"),
            "base_reserve_in_stroops": field(record, "base_reserve_in_stroops"),
            "closed_at": field(record, "closed_at"),
        })
    } else {
        json!({
            "sequence": field(&v, "sequence"),
            "base_reserve_in_stroops": field(&v, "base_reserve_in_stroops"),
        })
    };
    reduced_response(candid::Nat::from(200u16), reduced.to_string().into_bytes())
}

pub fn transform_transaction(args: TransformArgs) -> HttpResponse {
    let v = match parse_body(&args) {
        Ok(v) => v,
        Err(response) => return response,
    };
    let reduced = json!({
        "hash": field(&v, "hash"),
        "successful": field(&v, "successful"),
        "ledger": field(&v, "ledger"),
    });
    reduced_response(candid::Nat::from(200u16), reduced.to_string().into_bytes())
}

pub fn transform_operations(args: TransformArgs) -> HttpResponse {
    let v = match parse_body(&args) {
        Ok(v) => v,
        Err(response) => return response,
    };
    let records = v.pointer("/_embedded/records").and_then(Value::as_array);
    let reduced_records = records.map(|rows| {
        rows.iter()
            .map(|op| {
                json!({
                    "paging_token": field(op, "paging_token"),
                    "id": field(op, "id"),
                    "transaction_hash": field(op, "transaction_hash"),
                    "transaction_successful": field(op, "transaction_successful"),
                    "type": field(op, "type"),
                    "account": field(op, "account"),
                    "starting_balance": field(op, "starting_balance"),
                    "asset_type": field(op, "asset_type"),
                    "to": field(op, "to"),
                    "amount": field(op, "amount"),
                })
            })
            .collect::<Vec<_>>()
    });
    let reduced = json!({ "_embedded": { "records": reduced_records } });
    reduced_response(candid::Nat::from(200u16), reduced.to_string().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(status: u16, body: &[u8]) -> TransformArgs {
        TransformArgs {
            response: HttpResponse {
                status: candid::Nat::from(status),
                headers: vec![],
                body: body.to_vec(),
            },
            context: vec![],
        }
    }

    #[test]
    fn requests_are_gets_bounded_and_operations_limit_is_validated() {
        let account_id = "GAYOLLLUIZE4DZMBB2ZBKGBUBZLIOYU6XFLW37GBP2VZD3ABNXCW4BVA";
        let account = account_url(Network::Test, account_id).unwrap();
        assert_eq!(
            account,
            format!("https://horizon-testnet.stellar.org/accounts/{account_id}")
        );
        assert_eq!(
            ledger_reserve_url(Network::Public, 42).as_deref(),
            Some("https://horizon.stellar.org/ledgers/42")
        );
        assert_eq!(
            ledger_reserve_url(Network::Test, 42).as_deref(),
            Some("https://horizon-testnet.stellar.org/ledgers/42")
        );
        assert!(ledger_reserve_url(Network::Public, 0).is_none());
        let hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let ops = transaction_operations_url(Network::Public, hash, OPERATIONS_PAGE_LIMIT).unwrap();
        assert!(ops.ends_with(&format!(
            "/transactions/{hash}/operations?limit=200&order=asc"
        )));
        assert!(transaction_operations_url(Network::Public, hash, 0).is_none());
        assert!(
            transaction_operations_url(Network::Public, hash, OPERATIONS_PAGE_LIMIT + 1).is_none()
        );
    }

    #[test]
    fn request_paths_reject_untrusted_account_and_hash_values() {
        assert!(account_url(Network::Public, "G/../../admin").is_none());
        assert!(account_url(Network::Public, "GABC?limit=200").is_none());
        assert!(transaction_url(Network::Public, "../transactions/other").is_none());
        assert!(transaction_url(Network::Public, &format!("{}?x=y", "a".repeat(64))).is_none());
        assert!(transaction_operations_url(Network::Public, "../hash", 10).is_none());

        let hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert!(transaction_url(Network::Public, hash)
            .unwrap()
            .ends_with(&format!("/transactions/{hash}")));
    }

    #[test]
    fn exact_ledger_transform_keeps_only_reserve_inputs() {
        let transformed = transform_ledger(args(200, br#"{"sequence":42,"base_reserve_in_stroops":5000000,"base_fee_in_stroops":100,"closed_at":"volatile"}"#));
        let body = String::from_utf8(transformed.body).unwrap();
        assert!(body.contains("5000000"));
        assert!(body.contains("42"));
        assert!(!body.contains("volatile"));
    }

    #[test]
    fn latest_ledger_collection_transform_requires_exactly_one_record() {
        let record = r#"{"sequence":42,"base_reserve_in_stroops":5000000,"closed_at":"2026-10-27T12:00:00Z"}"#;
        let one = format!(r#"{{"_embedded":{{"records":[{record}]}}}}"#);
        let transformed = transform_ledger(args(200, one.as_bytes()));
        let parsed =
            crate::chains::stellar::horizon_parse::parse_latest_ledger_reserve(&transformed.body)
                .unwrap();
        assert_eq!(parsed.reserve.sequence, 42);
        assert_eq!(parsed.reserve.base_reserve_in_stroops, 5_000_000);
        assert_eq!(parsed.closed_at, "2026-10-27T12:00:00Z");

        for records in ["[]".to_string(), format!("[{record},{record}]")] {
            let body = format!(r#"{{"_embedded":{{"records":{records}}}}}"#);
            let transformed = transform_ledger(args(200, body.as_bytes()));
            assert_eq!(transformed.body, br#"{"parse_error":true}"#);
            assert!(
                crate::chains::stellar::horizon_parse::parse_latest_ledger_reserve(
                    &transformed.body
                )
                .is_err()
            );
        }

        let malformed = transform_ledger(args(200, br#"{"_embedded":{"records":{}}}"#));
        assert!(
            crate::chains::stellar::horizon_parse::parse_latest_ledger_reserve(&malformed.body)
                .is_err()
        );

        let missing_time = transform_ledger(args(
            200,
            br#"{"_embedded":{"records":[{"sequence":42,"base_reserve_in_stroops":5000000}]}}"#,
        ));
        assert!(
            crate::chains::stellar::horizon_parse::parse_latest_ledger_reserve(&missing_time.body)
                .is_err()
        );
    }

    #[test]
    fn account_transform_ignores_moving_headers_and_uses_body_ledger() {
        let body = br#"{"account_id":"G","sequence":"4","last_modified_ledger":42,"subentry_count":1,"num_sponsoring":0,"num_sponsored":0,"balances":[{"asset_type":"native","balance":"1.0000000","selling_liabilities":"0.1000000"}]}"#;
        let baseline = transform_account(args(200, body));
        let mut changed_header = args(200, body);
        changed_header.response.headers.extend([
            ic_cdk::api::management_canister::http_request::HttpHeader {
                name: "Latest-Ledger".to_string(),
                value: "42".to_string(),
            },
            ic_cdk::api::management_canister::http_request::HttpHeader {
                name: "latest-ledger".to_string(),
                value: "43".to_string(),
            },
        ]);
        let transformed = transform_account(changed_header);
        assert_eq!(transformed.body, baseline.body);
        let value: Value = serde_json::from_slice(&transformed.body).unwrap();
        assert_eq!(value.get("account_last_modified_ledger"), Some(&json!(42)));
    }

    #[test]
    fn account_transform_fails_closed_for_missing_or_malformed_body_ledger() {
        for account in [
            br#"{"account_id":"G","sequence":"4","subentry_count":1,"num_sponsoring":0,"num_sponsored":0,"balances":[]}"#.as_slice(),
            br#"{"account_id":"G","sequence":"4","last_modified_ledger":"42","subentry_count":1,"num_sponsoring":0,"num_sponsored":0,"balances":[]}"#,
            br#"{"account_id":"G","sequence":"4","last_modified_ledger":0,"subentry_count":1,"num_sponsoring":0,"num_sponsored":0,"balances":[]}"#,
            br#"{"account_id":"G","sequence":"4","last_modified_ledger":4294967296,"subentry_count":1,"num_sponsoring":0,"num_sponsored":0,"balances":[]}"#,
        ] {
            let transformed = transform_account(args(200, account));
            let value: Value = serde_json::from_slice(&transformed.body).unwrap();
            assert_eq!(value, json!({"parse_error": true}));
        }
    }

    #[test]
    fn account_transform_drops_volatile_and_non_native_fields() {
        let transformed = transform_account(args(200, br#"{"account_id":"G","sequence":"4","last_modified_ledger":42,"subentry_count":1,"num_sponsoring":0,"num_sponsored":0,"balances":[{"asset_type":"native","balance":"1.0000000","selling_liabilities":"0.1000000"},{"asset_type":"credit_alphanum4","balance":"99"}],"last_modified_time":"volatile"}"#));
        let body = String::from_utf8(transformed.body).unwrap();
        assert!(body.contains("1.0000000"));
        assert!(body.contains("0.1000000"));
        assert!(!body.contains("credit_alphanum4"));
        assert!(!body.contains("volatile"));
    }

    #[test]
    fn operation_transform_preserves_identity_fields_and_normalizes_status() {
        let transformed = transform_operations(args(200, br#"{"_embedded":{"records":[{"paging_token":"9","transaction_hash":"H","transaction_successful":true,"type":"payment","asset_type":"native","to":"G","amount":"2.0000000","created_at":"volatile"}]},"links":{"next":{"href":"volatile"}}}"#));
        assert_eq!(transformed.status, candid::Nat::from(200u16));
        let body = String::from_utf8(transformed.body).unwrap();
        assert!(body.contains("transaction_hash"));
        assert!(body.contains("2.0000000"));
        assert!(!body.contains("volatile"));
    }

    #[test]
    fn non_success_http_status_is_preserved_and_body_fails_closed() {
        let transformed = transform_transaction(args(
            404,
            br#"{"title":"Resource Missing","detail":"volatile detail"}"#,
        ));
        assert_eq!(transformed.status, candid::Nat::from(404u16));
        let body = String::from_utf8(transformed.body).unwrap();
        assert!(body.contains("http_error"));
        assert!(!body.contains("volatile detail"));
    }
}
