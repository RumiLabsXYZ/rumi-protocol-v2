//! Stellar classic account StrKey encoding and decoding.
//!
//! Stellar SEP-0023 encodes an Ed25519 public key as version byte `0x30`, the
//! 32-byte key, and a CRC16-XModem checksum in little-endian byte order, then
//! RFC 4648 base32 without padding. Only the classic `G…` form is accepted.

const ED25519_PUBLIC_KEY_VERSION: u8 = 6 << 3;
const CLASSIC_ADDRESS_ENCODED_LEN: usize = 56;
const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Encode a 32-byte Ed25519 public key as a classic Stellar `G…` address.
pub fn classic_address_from_ed25519_pubkey(public_key: &[u8; 32]) -> String {
    let mut payload = [0u8; 35];
    payload[0] = ED25519_PUBLIC_KEY_VERSION;
    payload[1..33].copy_from_slice(public_key);
    let checksum = crc16_xmodem(&payload[..33]).to_le_bytes();
    payload[33..35].copy_from_slice(&checksum);
    base32_encode(&payload)
}

/// Decode a classic Stellar `G…` address to its 32-byte Ed25519 public key.
///
/// Rejects non-canonical base32, padding, other StrKey types, wrong lengths,
/// and invalid CRCs. This is a format check only; it does not prove that the
/// account exists on any Stellar network.
pub fn ed25519_pubkey_from_classic_address(address: &str) -> Result<[u8; 32], String> {
    if address.len() != CLASSIC_ADDRESS_ENCODED_LEN {
        return Err(format!(
            "invalid Stellar classic address length: expected {CLASSIC_ADDRESS_ENCODED_LEN}, got {}",
            address.len()
        ));
    }

    let decoded = base32_decode(address)?;
    if decoded.len() != 35 {
        return Err(format!(
            "invalid Stellar classic address payload length: expected 35, got {}",
            decoded.len()
        ));
    }
    if decoded[0] != ED25519_PUBLIC_KEY_VERSION {
        return Err("invalid Stellar classic address version".to_string());
    }

    let expected_crc = crc16_xmodem(&decoded[..33]).to_le_bytes();
    if decoded[33..35] != expected_crc {
        return Err("invalid Stellar classic address checksum".to_string());
    }

    let mut public_key = [0u8; 32];
    public_key.copy_from_slice(&decoded[1..33]);

    // Enforce the unique RFC 4648 unpadded representation.
    if classic_address_from_ed25519_pubkey(&public_key) != address {
        return Err("non-canonical Stellar classic address".to_string());
    }
    Ok(public_key)
}

/// Return whether `address` is a valid classic Stellar Ed25519 `G…` StrKey.
pub fn is_valid_classic_address(address: &str) -> bool {
    ed25519_pubkey_from_classic_address(address).is_ok()
}

fn crc16_xmodem(bytes: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &byte in bytes {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn base32_encode(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity((bytes.len() * 8 + 4) / 5);
    let mut accumulator = 0u32;
    let mut bits = 0u8;
    for &byte in bytes {
        accumulator = (accumulator << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            encoded.push(BASE32_ALPHABET[((accumulator >> bits) & 0x1f) as usize] as char);
        }
    }
    if bits != 0 {
        encoded.push(BASE32_ALPHABET[((accumulator << (5 - bits)) & 0x1f) as usize] as char);
    }
    encoded
}

fn base32_decode(encoded: &str) -> Result<Vec<u8>, String> {
    if encoded.len() % 8 == 1 || encoded.len() % 8 == 3 || encoded.len() % 8 == 6 {
        return Err("invalid unpadded base32 length".to_string());
    }

    let mut decoded = Vec::with_capacity(encoded.len() * 5 / 8);
    let mut accumulator = 0u32;
    let mut bits = 0u8;
    for byte in encoded.bytes() {
        let value = BASE32_ALPHABET
            .iter()
            .position(|&candidate| candidate == byte)
            .ok_or_else(|| "invalid unpadded base32 character".to_string())?
            as u32;
        accumulator = (accumulator << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            decoded.push((accumulator >> bits) as u8);
        }
    }
    if bits != 0 && accumulator & ((1u32 << bits) - 1) != 0 {
        return Err("non-zero unused bits in base32 encoding".to_string());
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    // SEP-0023 valid non-multiplexed account vector.
    const OFFICIAL_ADDRESS: &str = "GA7QYNF7SOWQ3GLR2BGMZEHXAVIRZA4KVWLTJJFC7MGXUA74P7UJVSGZ";
    const OFFICIAL_PUBLIC_KEY: [u8; 32] = [
        0x3f, 0x0c, 0x34, 0xbf, 0x93, 0xad, 0x0d, 0x99, 0x71, 0xd0, 0x4c, 0xcc, 0x90, 0xf7, 0x05,
        0x51, 0x1c, 0x83, 0x8a, 0xad, 0x97, 0x34, 0xa4, 0xa2, 0xfb, 0x0d, 0x7a, 0x03, 0xfc, 0x7f,
        0xe8, 0x9a,
    ];

    #[test]
    fn matches_official_sep_0023_vector() {
        assert_eq!(
            classic_address_from_ed25519_pubkey(&OFFICIAL_PUBLIC_KEY),
            OFFICIAL_ADDRESS
        );
        assert_eq!(
            ed25519_pubkey_from_classic_address(OFFICIAL_ADDRESS).unwrap(),
            OFFICIAL_PUBLIC_KEY
        );
    }

    #[test]
    fn round_trips_public_key() {
        let public_key = [0xa5; 32];
        let address = classic_address_from_ed25519_pubkey(&public_key);
        assert_eq!(address.len(), CLASSIC_ADDRESS_ENCODED_LEN);
        assert!(address.starts_with('G'));
        assert_eq!(
            ed25519_pubkey_from_classic_address(&address).unwrap(),
            public_key
        );
        assert!(is_valid_classic_address(&address));
    }

    #[test]
    fn rejects_bad_lengths_and_padding() {
        assert!(ed25519_pubkey_from_classic_address("").is_err());
        assert!(ed25519_pubkey_from_classic_address(&format!("{OFFICIAL_ADDRESS}=")).is_err());
    }

    #[test]
    fn rejects_noncanonical_alphabet_and_other_strkey_types() {
        let lower = OFFICIAL_ADDRESS.to_ascii_lowercase();
        assert!(ed25519_pubkey_from_classic_address(&lower).is_err());

        let mut wrong_type = OFFICIAL_ADDRESS.to_string();
        wrong_type.replace_range(..1, "S");
        assert!(ed25519_pubkey_from_classic_address(&wrong_type).is_err());
    }

    #[test]
    fn rejects_invalid_checksum() {
        let mut corrupted = OFFICIAL_ADDRESS.to_string();
        corrupted.replace_range(55..56, "A");
        assert!(ed25519_pubkey_from_classic_address(&corrupted).is_err());
    }

    #[test]
    fn rejects_invalid_base32_lengths_characters_and_unused_bits() {
        assert!(base32_decode("ABC").is_err());
        assert!(base32_decode("A=").is_err());
        assert!(base32_decode("0").is_err());
        assert!(base32_decode("AB").is_err()); // one decoded byte leaves two unused bits
    }
}
