use super::sol_rpc::*;
use candid::Reserved;

#[test]
fn consistent_ok_yields_text() {
    let r = MultiRequestResult::Consistent(RequestResult::Ok("hello".to_string()));
    assert_eq!(text_from_request_result(r).unwrap(), "hello");
}

#[test]
fn consistent_err_is_error() {
    let r = MultiRequestResult::Consistent(RequestResult::Err(RpcError::ValidationError(
        "boom".to_string(),
    )));
    assert!(text_from_request_result(r).is_err());
}

#[test]
fn inconsistent_is_rejected_for_reads() {
    // Reads demand agreement (playbook #4): Inconsistent => Err.
    let r = MultiRequestResult::Inconsistent(Reserved);
    assert!(text_from_request_result(r).is_err());
}

#[test]
fn parse_balance_extracts_value() {
    let json = r#"{"jsonrpc":"2.0","result":{"context":{"slot":1},"value":1000000000},"id":1}"#;
    assert_eq!(parse_balance_lamports(json).unwrap(), 1_000_000_000);
}

#[test]
fn parse_balance_reports_rpc_error() {
    let json = r#"{"jsonrpc":"2.0","error":{"code":-32000,"message":"x"},"id":1}"#;
    assert!(parse_balance_lamports(json).is_err());
}

#[test]
fn parse_balance_missing_value_errs() {
    assert!(parse_balance_lamports(r#"{"result":{}}"#).is_err());
}

#[test]
fn parse_mint_supply_extracts_jsonparsed_supply() {
    let json = r#"{
      "jsonrpc":"2.0",
      "result":{
        "context":{"slot":1},
        "value":{
          "data":{"parsed":{"info":{"decimals":8,"supply":"123456789","isInitialized":true},"type":"mint"},"program":"spl-token","space":82},
          "executable":false,"lamports":1461600,"owner":"TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA","rentEpoch":0,"space":82
        }
      },
      "id":1
    }"#;
    assert_eq!(parse_mint_supply_jsonparsed(json).unwrap(), 123_456_789);
}

#[test]
fn parse_mint_supply_missing_account_errs() {
    // getAccountInfo returns value: null when the account does not exist.
    let json = r#"{"jsonrpc":"2.0","result":{"context":{"slot":1},"value":null},"id":1}"#;
    assert!(parse_mint_supply_jsonparsed(json).is_err());
}

// ─── sendTransaction helpers ─────────────────────────────────────────────────

#[test]
fn send_payload_embeds_b64_and_base64_encoding() {
    let payload = build_send_transaction_payload("AQID");
    assert!(payload.contains(r#""method":"sendTransaction""#));
    assert!(payload.contains(r#""AQID""#), "base64 tx is the first param");
    assert!(payload.contains(r#""encoding":"base64""#));
    assert!(payload.contains(r#""skipPreflight":false"#));
}

#[test]
fn parse_send_signature_extracts_result_string() {
    // sendTransaction returns the signature as `result` (a top-level base58 string).
    let sig = "5VERv8NMvzbJMEkV8xnrLkEaWRtSz9CosKDYjCJjBRnbJLgp8uirBgmQpjKhoR4tjF3ZpRzrFmBV6UjKdiSZkQUW";
    let json = format!(r#"{{"jsonrpc":"2.0","result":"{sig}","id":1}}"#);
    assert_eq!(parse_send_transaction_signature(&json).unwrap(), sig);
}

#[test]
fn parse_send_signature_reports_rpc_error() {
    let json = r#"{"jsonrpc":"2.0","error":{"code":-32002,"message":"Transaction simulation failed"},"id":1}"#;
    assert!(parse_send_transaction_signature(json).is_err());
}

#[test]
fn parse_send_signature_missing_result_errs() {
    assert!(parse_send_transaction_signature(r#"{"jsonrpc":"2.0","id":1}"#).is_err());
}
