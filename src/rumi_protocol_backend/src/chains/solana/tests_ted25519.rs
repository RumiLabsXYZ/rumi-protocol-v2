use super::ted25519::*;
use candid::Principal;

#[test]
fn all_zero_pubkey_is_system_program_address() {
    // 32 zero bytes base58-encode to the 32-char System Program address.
    let addr = solana_address_from_pubkey(&[0u8; 32]).unwrap();
    assert_eq!(addr, "11111111111111111111111111111111");
}

#[test]
fn address_roundtrips_through_base58() {
    let pk = [7u8; 32];
    let addr = solana_address_from_pubkey(&pk).unwrap();
    let decoded = bs58::decode(&addr).into_vec().unwrap();
    assert_eq!(decoded, pk.to_vec());
}

#[test]
fn wrong_length_pubkey_rejected() {
    assert!(solana_address_from_pubkey(&[0u8; 31]).is_err());
    assert!(solana_address_from_pubkey(&[0u8; 33]).is_err());
}

#[test]
fn is_valid_solana_address_accepts_32_byte_base58() {
    assert!(is_valid_solana_address("11111111111111111111111111111111"));
    let good = solana_address_from_pubkey(&[42u8; 32]).unwrap();
    assert!(is_valid_solana_address(&good));
}

#[test]
fn is_valid_solana_address_rejects_evm_and_junk() {
    assert!(!is_valid_solana_address("0x0000000000000000000000000000000000000000"));
    assert!(!is_valid_solana_address("not base58 !!!"));
    // base58 of 31 bytes -> wrong length
    let short = bs58::encode([1u8; 31]).into_string();
    assert!(!is_valid_solana_address(&short));
}

#[test]
fn derivation_paths_are_distinct_and_structured() {
    let chain = crate::chains::config::ChainId(501);
    let settle = settlement_derivation_path(chain);
    let custody = custody_derivation_path(chain, Principal::anonymous(), 0);
    assert_ne!(settle, custody);
    assert_eq!(settle[0], 501u32.to_le_bytes().to_vec());
}
