//! Sui primitives: 32-byte addresses and object ids, object references, the Ed25519 address
//! derivation and the hex and base58 forms the GraphQL surface uses.
//!
//! Sources (pinned official): an address is the BLAKE2b-256 hash of the signature scheme flag
//! byte followed by the public key, flag `0x00` for Ed25519 (`sui-types` `crypto.rs`,
//! `SuiAddress::from(&PublicKey)`; docs.sui.io/concepts/cryptography/transaction-auth/keys-addresses).
//! An `ObjectRef` is `(ObjectID, SequenceNumber, ObjectDigest)`; the digest is BCS-serialized as a
//! length-prefixed byte string, 33 bytes in all (`sui-sdk-types` `digest.rs`).

use crate::hash::blake2b256;

pub type Bytes32 = [u8; 32];
/// A 32-byte Sui address.
pub type SuiAddress = Bytes32;
/// A 32-byte object id (the same shape as an address).
pub type ObjectId = Bytes32;

/// Signature scheme flag for Ed25519 (`sui-types` `SignatureScheme::ED25519 => 0x00`).
pub const ED25519_FLAG: u8 = 0x00;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SuiParseError {
    /// Missing `0x`, empty, too long, or not hexadecimal.
    BadAddress(String),
    /// Not a 32-byte base58 digest.
    BadDigest(String),
    /// Not 8 lowercase hexadecimal digits.
    BadShortChainId(String),
}

impl core::fmt::Display for SuiParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// An object reference: the id, the version and the digest of that exact version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectRef {
    pub id: ObjectId,
    pub version: u64,
    pub digest: Bytes32,
}

/// `BLAKE2b-256(0x00 || ed25519_public_key)`.
pub fn address_from_ed25519(public_key: &Bytes32) -> SuiAddress {
    let mut preimage = [0u8; 33];
    preimage[0] = ED25519_FLAG;
    preimage[1..].copy_from_slice(public_key);
    blake2b256(&preimage)
}

/// `0x` followed by exactly 64 lowercase hex digits.
pub fn format_address(a: &Bytes32) -> String {
    format!("0x{}", hex::encode(a))
}

/// Strict form used for configuration: `0x` and exactly 64 lowercase hex digits.
pub fn parse_address_strict(s: &str) -> Result<Bytes32, SuiParseError> {
    let rest = s.strip_prefix("0x").ok_or_else(|| SuiParseError::BadAddress(s.to_string()))?;
    if rest.len() != 64 || !rest.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Err(SuiParseError::BadAddress(s.to_string()));
    }
    let mut out = [0u8; 32];
    hex::decode_to_slice(rest, &mut out).map_err(|_| SuiParseError::BadAddress(s.to_string()))?;
    Ok(out)
}

/// Normalizing form used for provider responses: `0x` and 1 to 64 hex digits (either case),
/// left-padded with zeros, so `0x2` and the 64-digit form compare equal.
pub fn parse_address_padded(s: &str) -> Result<Bytes32, SuiParseError> {
    let rest = s.strip_prefix("0x").ok_or_else(|| SuiParseError::BadAddress(s.to_string()))?;
    if rest.is_empty() || rest.len() > 64 || !rest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(SuiParseError::BadAddress(s.to_string()));
    }
    let padded = format!("{rest:0>64}");
    let mut out = [0u8; 32];
    hex::decode_to_slice(&padded, &mut out).map_err(|_| SuiParseError::BadAddress(s.to_string()))?;
    Ok(out)
}

/// A base58 string that decodes to exactly 32 bytes (object, checkpoint and transaction digests).
pub fn parse_digest_base58(s: &str) -> Result<Bytes32, SuiParseError> {
    let bytes = bs58::decode(s).into_vec().map_err(|_| SuiParseError::BadDigest(s.to_string()))?;
    <Bytes32>::try_from(bytes.as_slice()).map_err(|_| SuiParseError::BadDigest(s.to_string()))
}

pub fn format_digest_base58(d: &Bytes32) -> String {
    bs58::encode(d).into_string()
}

/// The RPC "chain identifier": the first four bytes of the genesis checkpoint digest, as
/// lowercase hex (`sui-types` `digests.rs`, `ChainIdentifier` Display).
pub fn short_chain_id(full: &Bytes32) -> String {
    hex::encode(&full[..4])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_is_the_hash_of_flag_and_key() {
        // Independent value from `reference/sui_golden_reference.py` (treasury test seed).
        let pk = hex::decode("d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737").unwrap();
        let pk: Bytes32 = pk.try_into().unwrap();
        assert_eq!(
            format_address(&address_from_ed25519(&pk)),
            "0x0881c07520943bbf13989b92892093c1b50672156fa5f873c22892701cb2e207"
        );
    }

    #[test]
    fn strict_and_padded_address_parsing() {
        let full = format!("0x{}", "ab".repeat(32));
        assert_eq!(parse_address_strict(&full).unwrap(), [0xab; 32]);
        assert!(parse_address_strict("0x2").is_err(), "short form is not a configuration value");
        assert!(parse_address_strict(&full.to_uppercase()).is_err());
        assert!(parse_address_strict(&full[2..]).is_err(), "prefix required");
        let mut two = [0u8; 32];
        two[31] = 2;
        assert_eq!(parse_address_padded("0x2").unwrap(), two);
        assert_eq!(parse_address_padded(&format!("0x{}{}", "0".repeat(63), "2")).unwrap(), two);
        assert_eq!(parse_address_padded("0xAB").unwrap()[31], 0xab);
        assert!(parse_address_padded("0x").is_err());
        assert!(parse_address_padded("2").is_err());
        assert!(parse_address_padded(&format!("0x{}", "1".repeat(65))).is_err());
        assert!(parse_address_padded("0xzz").is_err());
    }

    #[test]
    fn digest_base58_round_trips_and_checks_length() {
        let d = [7u8; 32];
        assert_eq!(parse_digest_base58(&format_digest_base58(&d)).unwrap(), d);
        assert!(parse_digest_base58("abc").is_err());
        assert!(parse_digest_base58("0OIl").is_err(), "characters outside the base58 alphabet");
    }

    #[test]
    fn short_chain_id_is_the_first_four_bytes() {
        assert_eq!(short_chain_id(&[0x35, 0x83, 0x4a, 0x8a, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9]), "35834a8a");
    }
}
