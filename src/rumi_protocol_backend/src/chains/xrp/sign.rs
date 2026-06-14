//! XRPL signing helpers (pure). Ed25519 signs `STX\0 ‖ serialized_unsigned_tx`
//! DIRECTLY — no SHA-512Half (that is the secp256k1 path; mixing them up is a
//! classic "invalid signature" bug). Ed25519 (PureEdDSA) hashes internally with
//! SHA-512, so the threshold signer receives the prefixed blob verbatim.
//!
//! The transaction id explorers show is `SHA512Half(TXN\0 ‖ signed_blob)`, which
//! the canister computes LOCALLY rather than trusting a single RPC node's report.
//! Locked against xrpl.js (`testdata/xrp_kat.json`).

use sha2::{Digest, Sha512};

const HASH_PREFIX_SIGN: [u8; 4] = [0x53, 0x54, 0x58, 0x00]; // "STX\0"
const HASH_PREFIX_TXN: [u8; 4] = [0x54, 0x58, 0x4E, 0x00]; // "TXN\0"

/// The 33-byte XRPL SigningPubKey: `0xED ‖ raw ed25519 pubkey`. The `0xED` flag is
/// what marks the key as Ed25519 on the wire; dropping it is a classic bug.
pub fn ed25519_signing_pubkey(pubkey: &[u8; 32]) -> [u8; 33] {
    let mut out = [0u8; 33];
    out[0] = 0xED;
    out[1..].copy_from_slice(pubkey);
    out
}

/// Message handed to the threshold Ed25519 signer: `STX\0 ‖ unsigned_blob`.
pub fn signing_message(unsigned_blob: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(4 + unsigned_blob.len());
    m.extend_from_slice(&HASH_PREFIX_SIGN);
    m.extend_from_slice(unsigned_blob);
    m
}

/// Transaction id (uppercase hex of this is what XRPL explorers show).
pub fn tx_hash(signed_blob: &[u8]) -> [u8; 32] {
    let mut h = Sha512::new();
    h.update(HASH_PREFIX_TXN);
    h.update(signed_blob);
    let full = h.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&full[..32]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const KAT: &str = include_str!("testdata/xrp_kat.json");

    #[test]
    fn signing_message_matches_xrpl() {
        let v: serde_json::Value = serde_json::from_str(KAT).unwrap();
        let unsigned = hex::decode(v["unsigned_blob_hex"].as_str().unwrap()).unwrap();
        let got = hex::encode_upper(signing_message(&unsigned));
        assert_eq!(
            got,
            v["signing_message_hex"].as_str().unwrap().to_uppercase()
        );
    }

    #[test]
    fn tx_hash_matches_xrpl() {
        let v: serde_json::Value = serde_json::from_str(KAT).unwrap();
        let signed = hex::decode(v["signed_blob_hex"].as_str().unwrap()).unwrap();
        let got = hex::encode_upper(tx_hash(&signed));
        assert_eq!(got, v["tx_hash_hex"].as_str().unwrap().to_uppercase());
    }

    #[test]
    fn signing_pubkey_has_ed_flag() {
        let v: serde_json::Value = serde_json::from_str(KAT).unwrap();
        let raw = hex::decode(v["keypair"]["ed25519_pubkey_hex"].as_str().unwrap()).unwrap();
        let mut pk = [0u8; 32];
        pk.copy_from_slice(&raw);
        assert_eq!(
            hex::encode_upper(ed25519_signing_pubkey(&pk)),
            v["keypair"]["signing_pubkey_hex"]
                .as_str()
                .unwrap()
                .to_uppercase()
        );
    }
}
