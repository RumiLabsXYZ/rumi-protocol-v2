use super::tecdsa::{
    custody_derivation_path, evm_address_from_pubkey, interest_treasury_derivation_path,
    is_valid_evm_address, settlement_derivation_path,
};
use crate::chains::config::ChainId;
use candid::Principal;

// Task 12: the interest-treasury path is deterministic, distinct from the
// settlement path, and uses the `b"interest-treasury"` label, so revenue lands
// at a separate canister-controlled address per chain.
#[test]
fn interest_treasury_path_is_distinct_and_labeled() {
    let chain = ChainId(71);
    assert_eq!(
        interest_treasury_derivation_path(chain),
        vec![chain.0.to_le_bytes().to_vec(), b"interest-treasury".to_vec()]
    );
    assert_ne!(
        interest_treasury_derivation_path(chain),
        settlement_derivation_path(chain),
        "interest treasury must not collide with the minter address"
    );
}

#[test]
fn evm_address_from_known_uncompressed_pubkey() {
    // Canonical vector: private key = 1 -> address 0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf.
    // Uncompressed pubkey for k=1 (65 bytes, 0x04 || X || Y):
    let pubkey_hex = "0479be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8";
    let pubkey = hex_to_bytes(pubkey_hex);
    let addr = evm_address_from_pubkey(&pubkey).expect("address");
    assert_eq!(addr.to_lowercase(), "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf");
}

#[test]
fn evm_address_accepts_compressed_pubkey() {
    // 33-byte compressed pubkey for k=1: 0x02 || X.
    let compressed = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
    let addr = evm_address_from_pubkey(&hex_to_bytes(compressed)).expect("address");
    assert_eq!(addr.to_lowercase(), "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf");
}

#[test]
fn custody_path_is_deterministic_and_distinct_per_user() {
    let p1 = Principal::from_slice(&[1, 2, 3]);
    let p2 = Principal::from_slice(&[4, 5, 6]);
    let a = custody_derivation_path(ChainId(10143), p1, 0);
    let b = custody_derivation_path(ChainId(10143), p1, 0);
    let c = custody_derivation_path(ChainId(10143), p2, 0);
    let d = custody_derivation_path(ChainId(10143), p1, 1);
    assert_eq!(a, b);
    assert_ne!(a, c);
    assert_ne!(a, d);
}

#[test]
fn settlement_path_differs_from_any_custody_path() {
    let s = settlement_derivation_path(ChainId(10143));
    let cust = custody_derivation_path(ChainId(10143), Principal::anonymous(), 0);
    assert_ne!(s, cust);
}

#[test]
fn is_valid_evm_address_accepts_well_formed_and_rejects_malformed() {
    // Valid: 0x + exactly 40 hex digits, case-insensitive (mixed case ok —
    // format-only, no EIP-55 checksum enforcement).
    assert!(is_valid_evm_address("0x7e5f4552091a69125d5dfcb7b8c2659029395bdf"));
    assert!(is_valid_evm_address("0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf"));
    assert!(is_valid_evm_address("0X7e5f4552091a69125d5dfcb7b8c2659029395bdf")); // 0X prefix
    assert!(is_valid_evm_address("0x0000000000000000000000000000000000000000")); // all-zero, 40 hex
    assert!(is_valid_evm_address("0x000000000000000000000000000000000000c0de"));

    // Missing 0x prefix.
    assert!(!is_valid_evm_address("7e5f4552091a69125d5dfcb7b8c2659029395bdf"));
    // Wrong length: 39 hex (one short).
    assert!(!is_valid_evm_address("0x7e5f4552091a69125d5dfcb7b8c2659029395bd"));
    // Wrong length: 41 hex (one long).
    assert!(!is_valid_evm_address("0x7e5f4552091a69125d5dfcb7b8c2659029395bdff"));
    // Zero-length hex body.
    assert!(!is_valid_evm_address("0x"));
    // Empty string.
    assert!(!is_valid_evm_address(""));
    // Non-hex characters (40 chars but contains 'g', 'r', etc.).
    assert!(!is_valid_evm_address("0xgggggggggggggggggggggggggggggggggggggggg"));
    assert!(!is_valid_evm_address("0xrecipient")); // realistic placeholder typo
    // 0x followed by a too-short numeric body.
    assert!(!is_valid_evm_address("0x123"));
}

fn hex_to_bytes(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

// ─── Increment 3 / Task 5: liquidation-reserve derivation path ───
#[test]
fn reserve_path_distinct_from_settlement_and_treasury() {
    use super::tecdsa::{interest_treasury_derivation_path, reserve_derivation_path, settlement_derivation_path};
    use crate::chains::config::ChainId;
    let c = ChainId(71);
    let reserve = reserve_derivation_path(c);
    assert_ne!(reserve, settlement_derivation_path(c), "reserve path must differ from settlement");
    assert_ne!(reserve, interest_treasury_derivation_path(c), "reserve path must differ from treasury");
    // The label component is the distinguishing element.
    assert_eq!(reserve[1], b"liquidation-reserve".to_vec());
    // Per-chain: a different chain yields a different path.
    assert_ne!(reserve_derivation_path(ChainId(1030)), reserve);
}
