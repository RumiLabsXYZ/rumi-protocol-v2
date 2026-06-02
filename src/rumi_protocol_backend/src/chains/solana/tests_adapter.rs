//! `SolanaAdapter` pure-part tests (mirror `monad/tests_adapter.rs`).
//!
//! Only the SYNCHRONOUS surface is unit-tested here: `chain_id()`, trait-object
//! safety, and the two synchronous validation helpers (`decode_pubkey`,
//! `checked_amount_u64`) that gate `sign_mint` / `sign_withdrawal` before any
//! await. The async signing/broadcast path (derive -> read nonce -> sign ->
//! assemble) is covered end-to-end by the Task 9 PocketIC test, since it needs
//! the management canister and the SOL RPC mock that a `#[test]` cannot provide.

use super::adapter::{checked_amount_u64, decode_pubkey, SolanaAdapter};
use crate::chains::adapter::{ChainAdapter, ChainAdapterError};
use crate::chains::config::ChainId;
use crate::chains::solana::config::SOLANA_CHAIN_ID;

#[test]
fn adapter_reports_solana_chain_id() {
    let a = SolanaAdapter::new(SOLANA_CHAIN_ID);
    assert_eq!(a.chain_id(), SOLANA_CHAIN_ID);
}

#[test]
fn adapter_reports_arbitrary_chain_id() {
    // `new` keeps whatever id it is handed (tests may isolate with a non-501 id).
    let a = SolanaAdapter::new(ChainId(777));
    assert_eq!(a.chain_id(), ChainId(777));
}

#[test]
fn adapter_is_trait_object_safe() {
    let a: Box<dyn ChainAdapter> = Box::new(SolanaAdapter::new(SOLANA_CHAIN_ID));
    assert_eq!(a.chain_id(), SOLANA_CHAIN_ID);
}

// ─── synchronous payload validation (the part of sign_* before any await) ────

#[test]
fn decode_pubkey_accepts_valid_base58_address() {
    // 32 zero bytes is the System Program id, a canonical valid Solana address.
    let b58 = bs58::encode([0u8; 32]).into_string();
    assert!(decode_pubkey(&b58, "recipient").is_ok());
}

#[test]
fn decode_pubkey_rejects_non_base58_as_invalid_payload() {
    // '0', 'O', 'I', 'l' are outside the base58 alphabet => InvalidPayload, never a panic.
    let err = decode_pubkey("not base58 0OIl", "recipient").unwrap_err();
    assert!(matches!(err, ChainAdapterError::InvalidPayload(_)));
}

#[test]
fn decode_pubkey_rejects_wrong_length_as_invalid_payload() {
    // Valid base58 but only 31 bytes => not a 32-byte pubkey => InvalidPayload.
    let short = bs58::encode([1u8; 31]).into_string();
    let err = decode_pubkey(&short, "mint").unwrap_err();
    assert!(matches!(err, ChainAdapterError::InvalidPayload(_)));
}

#[test]
fn decode_pubkey_rejects_empty_as_invalid_payload() {
    let err = decode_pubkey("", "recipient").unwrap_err();
    assert!(matches!(err, ChainAdapterError::InvalidPayload(_)));
}

#[test]
fn checked_amount_accepts_u64_max() {
    // Exactly u64::MAX is representable; the checked conversion must accept it.
    let amt = u64::MAX as u128;
    assert_eq!(checked_amount_u64(amt).unwrap(), u64::MAX);
}

#[test]
fn checked_amount_accepts_zero() {
    assert_eq!(checked_amount_u64(0).unwrap(), 0);
}

#[test]
fn checked_amount_rejects_overflow_as_invalid_payload() {
    // One past u64::MAX must error (never silently truncate via `as u64`).
    let over = (u64::MAX as u128) + 1;
    let err = checked_amount_u64(over).unwrap_err();
    assert!(matches!(err, ChainAdapterError::InvalidPayload(_)));
}
