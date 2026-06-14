//! XRPL address derivation: `AccountID = RIPEMD160(SHA256(0xED ‖ pubkey))`,
//! classic address = `Base58Check(version 0x00, AccountID, RIPPLE alphabet)`.
//!
//! Ported from the delegated-vault native-XRP rail (a real, shipped integration)
//! and locked against xrpl.js by `testdata/xrp_kat.json`. Rumi addresses are
//! plain `String`s (matching `DepositRecord`/`WithdrawalRequest`), not a newtype.
//!
//! The `0xED` prefix is what tells XRPL the key is Ed25519 (not secp256k1) and is
//! hashed into the AccountID — it is NOT optional. The Base58 alphabet is XRPL's
//! own (`rpshnaf3…`), NOT the Bitcoin alphabet; `bs58::Alphabet::RIPPLE` plus the
//! `check` feature (`with_check_version`/`with_check`) gives the exact codec.

use ripemd::Ripemd160;
use sha2::{Digest, Sha256};

/// XRPL version byte for a classic account address (`r…`).
const CLASSIC_ADDRESS_VERSION: u8 = 0x00;

/// `AccountID = RIPEMD160(SHA256(0xED ‖ pubkey))` — the 20-byte account hash.
pub fn account_id_from_ed25519_pubkey(pubkey: &[u8; 32]) -> [u8; 20] {
    let mut pk = Vec::with_capacity(33);
    pk.push(0xED);
    pk.extend_from_slice(pubkey);
    let sha = Sha256::digest(pk);
    let rip = Ripemd160::digest(sha);
    let mut out = [0u8; 20];
    out.copy_from_slice(&rip);
    out
}

/// Encode a 20-byte AccountID as a classic `r…` address. `with_check_version`
/// appends the 4-byte `SHA256(SHA256(version ‖ payload))[..4]` checksum.
pub fn classic_address_from_account_id(account_id: &[u8; 20]) -> String {
    bs58::encode(account_id)
        .with_alphabet(bs58::Alphabet::RIPPLE)
        .with_check_version(CLASSIC_ADDRESS_VERSION)
        .into_string()
}

/// Convenience: pubkey → classic address.
pub fn classic_address_from_ed25519_pubkey(pubkey: &[u8; 32]) -> String {
    classic_address_from_account_id(&account_id_from_ed25519_pubkey(pubkey))
}

/// Decode a classic `r…` address to its 20-byte AccountID, rejecting anything
/// that is not a valid version-0x00 classic address with a good checksum. This is
/// the single trust-boundary validator for a user-supplied destination: it
/// rejects X-addresses (the `X…` format), wrong-version strings, empty input, and
/// corrupted checksums BEFORE any bytes are signed.
pub fn account_id_from_classic_address(addr: &str) -> Result<[u8; 20], String> {
    // `with_check(Some(ver))` verifies both the 4-byte SHA256d checksum and the
    // leading version byte; the returned vec is `version ‖ 20-byte payload`.
    let decoded = bs58::decode(addr)
        .with_alphabet(bs58::Alphabet::RIPPLE)
        .with_check(Some(CLASSIC_ADDRESS_VERSION))
        .into_vec()
        .map_err(|e| format!("invalid XRPL classic address: {e}"))?;
    if decoded.len() != 21 {
        return Err(format!(
            "XRPL address payload is {} bytes, expected 21",
            decoded.len()
        ));
    }
    let mut out = [0u8; 20];
    out.copy_from_slice(&decoded[1..21]);
    Ok(out)
}

/// Boolean view of `account_id_from_classic_address` for cheap validation.
pub fn is_valid_classic_address(addr: &str) -> bool {
    account_id_from_classic_address(addr).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const KAT: &str = include_str!("testdata/xrp_kat.json");

    fn pubkey32() -> [u8; 32] {
        let v: serde_json::Value = serde_json::from_str(KAT).unwrap();
        let hex = v["keypair"]["ed25519_pubkey_hex"].as_str().unwrap();
        let b = hex::decode(hex).unwrap();
        let mut a = [0u8; 32];
        a.copy_from_slice(&b);
        a
    }

    #[test]
    fn classic_address_matches_xrpl() {
        let v: serde_json::Value = serde_json::from_str(KAT).unwrap();
        let expected = v["keypair"]["classic_address"].as_str().unwrap();
        assert_eq!(classic_address_from_ed25519_pubkey(&pubkey32()), expected);
    }

    #[test]
    fn decode_round_trips_account_id() {
        let id = account_id_from_ed25519_pubkey(&pubkey32());
        let addr = classic_address_from_account_id(&id);
        assert_eq!(account_id_from_classic_address(&addr).unwrap(), id);
    }

    #[test]
    fn decode_accepts_kat_destination() {
        let v: serde_json::Value = serde_json::from_str(KAT).unwrap();
        let dest = v["payment"]["destination"].as_str().unwrap();
        assert!(is_valid_classic_address(dest));
    }

    #[test]
    fn decode_rejects_corrupt_checksum() {
        let mut a = classic_address_from_ed25519_pubkey(&pubkey32());
        a.pop();
        a.push('Z');
        assert!(account_id_from_classic_address(&a).is_err());
    }

    #[test]
    fn decode_rejects_empty() {
        assert!(account_id_from_classic_address("").is_err());
    }

    #[test]
    fn decode_rejects_x_address() {
        // X-addresses (CryptoCondition tag-bearing format) start with `X` and use
        // a different version; the classic decoder must reject them so a tagged
        // destination never silently loses its tag.
        let x = "X7AcgcsBL6XDcUb289X4mJ8ZEEgnXc1FLfDV3oNpL2nNT7sJ";
        assert!(account_id_from_classic_address(x).is_err());
    }

    #[test]
    fn decode_rejects_bitcoin_alphabet_string() {
        // A valid-looking base58 string in the Bitcoin alphabet (contains `0`,
        // which is absent from the RIPPLE alphabet) must not decode.
        assert!(account_id_from_classic_address("100000000000000000000000000").is_err());
    }
}
