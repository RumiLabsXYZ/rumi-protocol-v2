//! ICRC-1 escrow account identity (card BB-58).
//!
//! An escrow binding names the account on the production icUSD ledger that
//! holds a destination's locked icUSD: the owner principal as text and an
//! optional 32-byte subaccount. The aggregate refuses to count one escrow
//! twice, so two spellings of the same account must compare equal:
//!
//! * a missing subaccount and the all-zero subaccount are the same ICRC-1
//!   account;
//! * the principal text is decoded (base32 plus CRC-32) and compared as bytes,
//!   and only the canonical lowercase text is accepted, so upper case, missing
//!   dashes or a bad checksum cannot smuggle a second spelling past the
//!   duplicate check.
//!
//! No dependency is added: the codec is the documented principal text
//! encoding (CRC-32 big endian, then the principal bytes, base32 RFC 4648 in
//! lower case without padding, a dash every five characters).

use serde::Deserialize;

/// Longest principal, in bytes.
pub const MAX_PRINCIPAL_BYTES: usize = 29;

const BASE32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
/// The anonymous principal is the single byte 4 (text `2vxsx-fae`).
const ANONYMOUS: [u8; 1] = [4];

/// The escrow account of one destination (or of the XRPL lane).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EscrowBinding {
    /// Owner principal, canonical lowercase text.
    pub owner: String,
    /// 32-byte subaccount as 64 hex characters in JSON, or null. `None` and
    /// all zeroes are the same account.
    #[serde(default, with = "opt_hex32")]
    pub subaccount: Option<[u8; 32]>,
}

/// The ledger-level identity of an account: owner bytes and the effective
/// (zero-filled) subaccount.
pub type AccountIdentity = (Vec<u8>, [u8; 32]);

/// Principal bytes of a canister or account owner given as canonical text.
/// Rejects the empty (management canister) and the anonymous principal, which
/// can never be a bridge instance or an escrow owner.
pub fn usable_principal(text: &str) -> Result<Vec<u8>, String> {
    let bytes = principal_text_to_bytes(text)?;
    if bytes.is_empty() {
        return Err("the empty (management canister) principal is not usable here".into());
    }
    if bytes == ANONYMOUS {
        return Err("the anonymous principal is not usable here".into());
    }
    Ok(bytes)
}

/// A readable label for an account identity: the owner text, plus `.` and the
/// subaccount in hex when it is not the default.
pub fn account_label(identity: &AccountIdentity) -> String {
    let owner = principal_to_text(&identity.0);
    if identity.1 == [0u8; 32] {
        owner
    } else {
        let hex: String = identity.1.iter().map(|b| format!("{b:02x}")).collect();
        format!("{owner}.{hex}")
    }
}

impl EscrowBinding {
    /// A binding from raw principal bytes (used by tests and fixtures).
    pub fn from_principal_bytes(owner: &[u8], subaccount: Option<[u8; 32]>) -> Self {
        EscrowBinding {
            owner: principal_to_text(owner),
            subaccount,
        }
    }

    /// Decode to the identity the duplicate check compares. Rejects a text
    /// that is not a canonical principal, the anonymous principal and the
    /// empty (management) principal.
    pub fn identity(&self) -> Result<AccountIdentity, String> {
        let bytes = usable_principal(&self.owner)
            .map_err(|e| e.replace("is not usable here", "cannot own an escrow"))?;
        Ok((bytes, self.subaccount.unwrap_or([0u8; 32])))
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Canonical text of a principal given as bytes.
pub fn principal_to_text(bytes: &[u8]) -> String {
    let mut data = Vec::with_capacity(bytes.len() + 4);
    data.extend_from_slice(&crc32(bytes).to_be_bytes());
    data.extend_from_slice(bytes);

    let mut chars = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits = 0u32;
    for &byte in &data {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            chars.push(BASE32[((buffer >> bits) & 31) as usize] as char);
        }
        buffer &= (1 << bits) - 1;
    }
    if bits > 0 {
        chars.push(BASE32[((buffer << (5 - bits)) & 31) as usize] as char);
    }

    let mut out = String::with_capacity(chars.len() + chars.len() / 5);
    for (i, c) in chars.into_iter().enumerate() {
        if i > 0 && i % 5 == 0 {
            out.push('-');
        }
        out.push(c);
    }
    out
}

/// Principal bytes of a canonical principal text. Anything else is an error:
/// bad characters, bad checksum, too long, or text that is not exactly the
/// canonical spelling of the bytes it decodes to.
pub fn principal_text_to_bytes(text: &str) -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits = 0u32;
    let mut any = false;
    for c in text.chars().filter(|c| *c != '-') {
        any = true;
        let value = BASE32
            .iter()
            .position(|b| *b as char == c)
            .ok_or_else(|| format!("principal text {text:?} has an invalid character {c:?}"))?;
        buffer = (buffer << 5) | value as u32;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            data.push(((buffer >> bits) & 0xFF) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    if !any {
        return Err("empty principal text".into());
    }
    if data.len() < 4 {
        return Err(format!("principal text {text:?} is too short"));
    }
    let (checksum, bytes) = data.split_at(4);
    if checksum != crc32(bytes).to_be_bytes() {
        return Err(format!("principal text {text:?} has a bad checksum"));
    }
    if bytes.len() > MAX_PRINCIPAL_BYTES {
        return Err(format!(
            "principal text {text:?} decodes to {} bytes (max {MAX_PRINCIPAL_BYTES})",
            bytes.len()
        ));
    }
    if principal_to_text(bytes) != text {
        return Err(format!(
            "principal text {text:?} is not canonical (expected {})",
            principal_to_text(bytes)
        ));
    }
    Ok(bytes.to_vec())
}

/// Serde adapter: `Option<[u8; 32]>` as 64 hex characters or null.
mod opt_hex32 {
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<[u8; 32]>, D::Error> {
        let Some(text) = Option::<String>::deserialize(d)? else {
            return Ok(None);
        };
        if text.len() != 64 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(serde::de::Error::custom(format!(
                "subaccount must be 64 hex characters, got {text:?}"
            )));
        }
        let mut out = [0u8; 32];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = u8::from_str_radix(&text[2 * i..2 * i + 2], 16)
                .map_err(serde::de::Error::custom)?;
        }
        Ok(Some(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_matches_known_principal_texts() {
        // The three published reference values: management canister,
        // anonymous principal, and a 10-byte canister id.
        assert_eq!(principal_to_text(&[]), "aaaaa-aa");
        assert_eq!(principal_to_text(&[4]), "2vxsx-fae");
        let ledger = [0, 0, 0, 0, 0, 0, 0, 2, 1, 1];
        assert_eq!(principal_to_text(&ledger), "ryjl3-tyaaa-aaaaa-aaaba-cai");
        assert_eq!(
            principal_text_to_bytes("ryjl3-tyaaa-aaaaa-aaaba-cai").unwrap(),
            ledger.to_vec()
        );
    }

    #[test]
    fn codec_round_trips_every_length() {
        for len in 0..=MAX_PRINCIPAL_BYTES {
            let bytes: Vec<u8> = (0..len as u8).map(|i| i.wrapping_mul(37) ^ 0xA5).collect();
            let text = principal_to_text(&bytes);
            assert_eq!(principal_text_to_bytes(&text).unwrap(), bytes, "len {len}");
        }
    }

    #[test]
    fn non_canonical_and_corrupt_text_is_rejected() {
        let text = principal_to_text(b"synthetic-escrow");
        assert!(principal_text_to_bytes(&text).is_ok());
        // Upper case, missing dashes and a flipped character are all refused.
        assert!(principal_text_to_bytes(&text.to_uppercase()).is_err());
        assert!(principal_text_to_bytes(&text.replace('-', "")).is_err());
        let mut flipped = text.clone().into_bytes();
        let last = flipped.len() - 1;
        flipped[last] = if flipped[last] == b'a' { b'b' } else { b'a' };
        assert!(principal_text_to_bytes(&String::from_utf8(flipped).unwrap()).is_err());
        assert!(principal_text_to_bytes("").is_err());
        assert!(principal_text_to_bytes("---").is_err());
        assert!(principal_text_to_bytes("not a principal").is_err());
        // 30 bytes is one too many.
        assert!(principal_text_to_bytes(&principal_to_text(&[7u8; 30])).is_err());
    }

    #[test]
    fn anonymous_and_empty_owners_are_rejected() {
        let anonymous = EscrowBinding {
            owner: "2vxsx-fae".into(),
            subaccount: None,
        };
        assert!(anonymous.identity().unwrap_err().contains("anonymous"));
        let management = EscrowBinding {
            owner: "aaaaa-aa".into(),
            subaccount: None,
        };
        assert!(management.identity().unwrap_err().contains("empty"));
    }

    #[test]
    fn usable_principal_and_account_label() {
        let text = principal_to_text(b"synthetic-instance");
        assert_eq!(
            usable_principal(&text).unwrap(),
            b"synthetic-instance".to_vec()
        );
        assert!(usable_principal("2vxsx-fae").is_err());
        assert!(usable_principal("aaaaa-aa").is_err());
        assert!(usable_principal("garbage").is_err());

        let owner = b"synthetic-a".to_vec();
        assert_eq!(
            account_label(&(owner.clone(), [0; 32])),
            principal_to_text(&owner)
        );
        let mut sub = [0u8; 32];
        sub[31] = 0xab;
        let label = account_label(&(owner.clone(), sub));
        assert!(label.starts_with(&principal_to_text(&owner)));
        assert!(label.ends_with("ab"), "{label}");
    }

    #[test]
    fn missing_and_all_zero_subaccount_are_the_same_account() {
        let none = EscrowBinding::from_principal_bytes(b"synthetic-a", None);
        let zero = EscrowBinding::from_principal_bytes(b"synthetic-a", Some([0; 32]));
        let mut sub = [0u8; 32];
        sub[31] = 1;
        let other = EscrowBinding::from_principal_bytes(b"synthetic-a", Some(sub));
        assert_eq!(none.identity().unwrap(), zero.identity().unwrap());
        assert_ne!(none.identity().unwrap(), other.identity().unwrap());
    }

    #[test]
    fn subaccount_json_is_strict_hex() {
        let ok: EscrowBinding = serde_json::from_str(&format!(
            r#"{{"owner":"{}","subaccount":"{}"}}"#,
            principal_to_text(b"synthetic-a"),
            "00".repeat(31) + "01"
        ))
        .unwrap();
        assert_eq!(ok.subaccount.unwrap()[31], 1);
        let absent: EscrowBinding = serde_json::from_str(&format!(
            r#"{{"owner":"{}"}}"#,
            principal_to_text(b"synthetic-a")
        ))
        .unwrap();
        assert_eq!(absent.subaccount, None);
        for bad in ["\"00\"", "\"zz\"", &format!("\"{}\"", "0g".repeat(32))] {
            let json = format!(
                r#"{{"owner":"{}","subaccount":{bad}}}"#,
                principal_to_text(b"synthetic-a")
            );
            assert!(serde_json::from_str::<EscrowBinding>(&json).is_err());
        }
    }
}
