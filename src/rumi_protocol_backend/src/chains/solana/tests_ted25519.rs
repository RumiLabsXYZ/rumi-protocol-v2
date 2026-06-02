use super::ted25519::*;
use candid::{Decode, Encode, Principal};

#[test]
fn sign_with_schnorr_arg_candid_round_trips() {
    // Proves the hand-mirrored sign_with_schnorr arg (+ key id + Ed25519 algo)
    // encode/decode cleanly, so the management-canister call is well-formed.
    let arg = SignWithSchnorrArgument {
        message: vec![1, 2, 3, 4],
        derivation_path: vec![vec![0u8], b"settlement".to_vec()],
        key_id: SchnorrKeyId {
            algorithm: SchnorrAlgorithm::Ed25519,
            name: "test_key_1".to_string(),
        },
    };
    let bytes = Encode!(&arg).expect("encode");
    let back = Decode!(&bytes, SignWithSchnorrArgument).expect("decode");
    assert_eq!(back.message, vec![1, 2, 3, 4]);
    assert_eq!(back.key_id.name, "test_key_1");
}

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
fn decode_solana_address_roundtrips_and_rejects_bad_input() {
    // Valid 32-byte base58 decodes back to the original bytes.
    let pk = [42u8; 32];
    let addr = solana_address_from_pubkey(&pk).unwrap();
    assert_eq!(decode_solana_address(&addr).unwrap(), pk);
    // System Program address (32 zero bytes).
    assert_eq!(
        decode_solana_address("11111111111111111111111111111111").unwrap(),
        [0u8; 32]
    );
    // Non-base58 junk is rejected.
    assert!(decode_solana_address("not base58 !!!").is_err());
    // Wrong length (31 bytes of base58) is rejected.
    let short = bs58::encode([1u8; 31]).into_string();
    assert!(decode_solana_address(&short).is_err());
    // is_valid_solana_address agrees with the decoder on each case.
    assert_eq!(is_valid_solana_address(&addr), decode_solana_address(&addr).is_ok());
    assert_eq!(
        is_valid_solana_address("not base58 !!!"),
        decode_solana_address("not base58 !!!").is_ok()
    );
}

#[test]
fn derivation_paths_are_distinct_and_structured() {
    let chain = crate::chains::config::ChainId(501);
    let settle = settlement_derivation_path(chain);
    let custody = custody_derivation_path(chain, Principal::anonymous(), 0);
    assert_ne!(settle, custody);
    assert_eq!(settle[0], 501u32.to_le_bytes().to_vec());
}

#[test]
fn nonce_path_is_distinct_from_settlement_and_custody() {
    // The nonce account is a second threshold-Ed25519 key under the same chain,
    // so the canister controls it AND is its authority (no PDA). It must derive a
    // DIFFERENT key than the settlement and custody paths, else they would collide
    // on a single key (the settlement key would also be the nonce account).
    let chain = crate::chains::config::ChainId(501);
    let nonce = nonce_derivation_path(chain);
    let settle = settlement_derivation_path(chain);
    let custody = custody_derivation_path(chain, Principal::anonymous(), 0);

    assert_ne!(nonce, settle, "nonce path must differ from settlement");
    assert_ne!(nonce, custody, "nonce path must differ from custody");

    // Structure: [chain_id LE, b"nonce"].
    assert_eq!(nonce.len(), 2);
    assert_eq!(nonce[0], 501u32.to_le_bytes().to_vec(), "first component is the chain id LE");
    assert_eq!(nonce[1], b"nonce".to_vec(), "second component is the b\"nonce\" tag");

    // Same chain-id prefix as settlement, but a different second component, which
    // is exactly what makes the derived key distinct.
    assert_eq!(nonce[0], settle[0], "both share the chain-id prefix");
    assert_ne!(nonce[1], settle[1], "the tag distinguishes nonce from settlement");
}
