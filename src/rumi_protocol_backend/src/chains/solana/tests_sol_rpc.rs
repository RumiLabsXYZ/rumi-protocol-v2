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

// ─── durable nonce: parse_nonce_account_blockhash (pure, 80-byte buffer) ─────
//
// Nonce account on-chain data layout (80 bytes):
//   version: u32 LE        [0..4]
//   state:   u32 LE        [4..8]   (0 = Uninitialized, 1 = Initialized)
//   authority: Pubkey      [8..40]
//   durable_nonce/blockhash: [u8;32] [40..72]   <- the value we extract
//   fee_calculator.lamports_per_signature: u64 LE [72..80]

/// Build an 80-byte nonce account buffer with the given state byte and an
/// embedded blockhash at offset 40, for the pure-helper tests.
fn nonce_buf(state: u32, blockhash: [u8; 32]) -> Vec<u8> {
    let mut buf = vec![0u8; 80];
    buf[0..4].copy_from_slice(&1u32.to_le_bytes()); // version = 1
    buf[4..8].copy_from_slice(&state.to_le_bytes());
    buf[8..40].copy_from_slice(&[0xAB; 32]); // authority (irrelevant to the parse)
    buf[40..72].copy_from_slice(&blockhash);
    buf[72..80].copy_from_slice(&5000u64.to_le_bytes()); // lamports_per_signature
    buf
}

#[test]
fn parse_nonce_blockhash_extracts_initialized_value() {
    let bh = [0x42u8; 32];
    let buf = nonce_buf(1, bh); // state = 1 (Initialized)
    assert_eq!(parse_nonce_account_blockhash(&buf).unwrap(), bh);
}

#[test]
fn parse_nonce_blockhash_rejects_uninitialized() {
    // state = 0 (Uninitialized): the account exists but holds no usable nonce.
    let buf = nonce_buf(0, [0x42u8; 32]);
    assert!(parse_nonce_account_blockhash(&buf).is_err());
}

#[test]
fn parse_nonce_blockhash_rejects_short_buffer() {
    // Anything other than exactly 80 bytes is not a System nonce account.
    assert!(parse_nonce_account_blockhash(&[1u8; 79]).is_err());
    assert!(parse_nonce_account_blockhash(&[1u8; 81]).is_err());
    assert!(parse_nonce_account_blockhash(&[]).is_err());
}

#[test]
fn parse_nonce_blockhash_rejects_unknown_state() {
    // Only state == 1 is accepted; a corrupt / future state value errs.
    let buf = nonce_buf(2, [0x42u8; 32]);
    assert!(parse_nonce_account_blockhash(&buf).is_err());
}

// ─── durable nonce: getAccountInfo base64 data extraction (pure) ─────────────

#[test]
fn parse_account_data_base64_extracts_buffer() {
    // result.value.data is [base64_string, "base64"]; we decode element 0.
    use base64::Engine;
    let raw = vec![7u8; 80];
    let b64 = base64::engine::general_purpose::STANDARD.encode(&raw);
    let json = format!(
        r#"{{"jsonrpc":"2.0","result":{{"context":{{"slot":1}},"value":{{"data":["{b64}","base64"],"executable":false,"lamports":1,"owner":"11111111111111111111111111111111","rentEpoch":0,"space":80}}}},"id":1}}"#
    );
    assert_eq!(parse_account_data_base64(&json).unwrap(), raw);
}

#[test]
fn parse_account_data_base64_null_value_errs() {
    // getAccountInfo returns value: null when the account is not bootstrapped.
    let json = r#"{"jsonrpc":"2.0","result":{"context":{"slot":1},"value":null},"id":1}"#;
    assert!(parse_account_data_base64(json).is_err());
}

#[test]
fn parse_account_data_base64_reports_rpc_error() {
    let json = r#"{"jsonrpc":"2.0","error":{"code":-32000,"message":"x"},"id":1}"#;
    assert!(parse_account_data_base64(json).is_err());
}

// ─── getLatestBlockhash extraction (pure) ────────────────────────────────────

#[test]
fn parse_latest_blockhash_extracts_32_bytes() {
    // result.value.blockhash is a base58 string; we decode it to 32 bytes.
    let bh = [9u8; 32];
    let b58 = bs58::encode(bh).into_string();
    let json = format!(
        r#"{{"jsonrpc":"2.0","result":{{"context":{{"slot":1}},"value":{{"blockhash":"{b58}","lastValidBlockHeight":100}}}},"id":1}}"#
    );
    assert_eq!(parse_latest_blockhash(&json).unwrap(), bh);
}

#[test]
fn parse_latest_blockhash_reports_rpc_error() {
    let json = r#"{"jsonrpc":"2.0","error":{"code":-32000,"message":"x"},"id":1}"#;
    assert!(parse_latest_blockhash(json).is_err());
}

#[test]
fn parse_latest_blockhash_rejects_non_32_byte_decode() {
    // A base58 string that decodes to the wrong length is rejected.
    let short = bs58::encode([1u8; 31]).into_string();
    let json = format!(
        r#"{{"jsonrpc":"2.0","result":{{"value":{{"blockhash":"{short}"}}}},"id":1}}"#
    );
    assert!(parse_latest_blockhash(&json).is_err());
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

// ─── getTransaction (verify_deposit) ─────────────────────────────────────────

#[test]
fn parse_get_transaction_null_result_is_not_found() {
    // A confirmed-at-finalized lookup of an unknown/unconfirmed signature.
    let json = r#"{"jsonrpc":"2.0","result":null,"id":1}"#;
    assert_eq!(parse_get_transaction(json).unwrap(), TxStatus::NotFound);
}

#[test]
fn parse_get_transaction_missing_result_is_not_found() {
    // Some providers omit the member entirely instead of returning null.
    let json = r#"{"jsonrpc":"2.0","id":1}"#;
    assert_eq!(parse_get_transaction(json).unwrap(), TxStatus::NotFound);
}

#[test]
fn parse_get_transaction_success_returns_slot() {
    // Non-null result, meta.err == null => confirmed at result.slot.
    let json = r#"{"jsonrpc":"2.0","result":{"slot":123456789,"meta":{"err":null,"fee":5000},"transaction":{}},"id":1}"#;
    assert_eq!(
        parse_get_transaction(json).unwrap(),
        TxStatus::Confirmed { slot: 123_456_789 }
    );
}

#[test]
fn parse_get_transaction_failed_meta_is_failed() {
    // Non-null result with a non-null meta.err => the tx reverted on-chain.
    let json = r#"{"jsonrpc":"2.0","result":{"slot":555,"meta":{"err":{"InstructionError":[0,{"Custom":1}]},"fee":5000},"transaction":{}},"id":1}"#;
    assert_eq!(parse_get_transaction(json).unwrap(), TxStatus::Failed);
}

#[test]
fn parse_get_transaction_reports_rpc_error() {
    let json = r#"{"jsonrpc":"2.0","error":{"code":-32004,"message":"Block not available"},"id":1}"#;
    assert!(parse_get_transaction(json).is_err());
}

#[test]
fn parse_get_transaction_missing_slot_errs() {
    // A non-null, non-failed result that omits slot is malformed.
    let json = r#"{"jsonrpc":"2.0","result":{"meta":{"err":null}},"id":1}"#;
    assert!(parse_get_transaction(json).is_err());
}

// ─── getSlot (fetch_finality) ────────────────────────────────────────────────

#[test]
fn parse_slot_extracts_bare_u64() {
    // getSlot's result is a bare number, NOT nested under result.value.
    let json = r#"{"jsonrpc":"2.0","result":341078412,"id":1}"#;
    assert_eq!(parse_slot(json).unwrap(), 341_078_412);
}

#[test]
fn parse_slot_reports_rpc_error() {
    let json = r#"{"jsonrpc":"2.0","error":{"code":-32000,"message":"x"},"id":1}"#;
    assert!(parse_slot(json).is_err());
}

#[test]
fn parse_slot_missing_result_errs() {
    assert!(parse_slot(r#"{"jsonrpc":"2.0","id":1}"#).is_err());
}

#[test]
fn parse_slot_rejects_nested_value_shape() {
    // Guard against accidentally parsing a getBalance-shaped response.
    let json = r#"{"jsonrpc":"2.0","result":{"value":123},"id":1}"#;
    assert!(parse_slot(json).is_err());
}

// ─── tx-signature validation (getTransaction injection guard) ────────────────

#[test]
fn valid_tx_signature_accepts_64_byte_base58() {
    let sig = bs58::encode([7u8; 64]).into_string();
    assert!(is_valid_tx_signature(&sig));
}

#[test]
fn valid_tx_signature_rejects_32_byte_pubkey_length() {
    // A 32-byte value is a pubkey, not a signature.
    let pk = bs58::encode([7u8; 32]).into_string();
    assert!(!is_valid_tx_signature(&pk));
}

#[test]
fn valid_tx_signature_rejects_non_base58() {
    // '0', 'O', 'I', 'l' are outside the base58 alphabet.
    assert!(!is_valid_tx_signature("not valid base58 0OIl"));
    assert!(!is_valid_tx_signature(""));
}
