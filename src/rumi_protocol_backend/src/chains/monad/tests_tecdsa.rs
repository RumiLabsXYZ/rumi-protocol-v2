use super::tecdsa::{custody_derivation_path, evm_address_from_pubkey, settlement_derivation_path};
use crate::chains::config::ChainId;
use candid::Principal;

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

fn hex_to_bytes(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}
