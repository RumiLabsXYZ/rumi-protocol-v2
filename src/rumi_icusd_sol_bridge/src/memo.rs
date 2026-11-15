//! Memo grammar (`chain-solana.md` sections 2 and 2.2).
//!
//! Two families, both ASCII, case-sensitive, tagged `rumi1`:
//!
//! * the **user return memo** `rumi1|r|<principal>|<subaccount>|<nonce>`, at most 160
//!   bytes, five `|`-separated fields and no other bytes:
//!   * `<principal>`: an ICP textual principal that parses and re-encodes identically, and
//!     is not the anonymous principal, not the management canister and not the bridge itself;
//!   * `<subaccount>`: `-` for the default subaccount, else 64 lowercase hex characters
//!     that are not all zero (an all-zero subaccount is spelled `-`);
//!   * `<nonce>`: a `u64` in decimal with no leading zeros (`0` alone is allowed). It
//!     correlates the user's own intent; uniqueness comes from the transaction signature.
//! * the **canister memos** `rumi1|m|<id>` (mint), `rumi1|b|<id>` (burn) and
//!   `rumi1|f|<id>` (refund), where `<id>` is 64 lowercase hex.
//!
//! A memo is **discovery, never proof**: a memo can be copied into another transaction by
//! anyone, so adopting a transaction needs its exact stored message bytes (note section 12).
//! A later grammar version needs a new `rumiN` tag and a reviewed profile change.

use candid::types::principal::PrincipalError;
use candid::Principal;

/// The grammar tag. A later grammar is `rumi2`, not a silent extension.
pub const MEMO_TAG: &str = "rumi1";
/// Longest accepted return memo, in bytes.
pub const MAX_RETURN_MEMO_BYTES: usize = 160;

/// A parsed, validated user return memo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReturnMemo {
    /// The ICP principal credited with the returned icUSD.
    pub destination: Principal,
    /// `None` is the default subaccount (`-` on the wire).
    pub subaccount: Option<[u8; 32]>,
    /// The client nonce.
    pub client_nonce: u64,
}

/// Why a memo was rejected. Every variant is a distinct, testable reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemoError {
    TooLong { len: usize },
    NotAscii,
    /// Not exactly five `|`-separated fields (return memo) or three (canister memo).
    FieldCount { found: usize },
    WrongTag,
    /// The kind field is not the one this parser accepts.
    WrongKind,
    /// The principal text does not parse.
    PrincipalSyntax,
    /// The principal parses but its canonical text differs from the field.
    PrincipalNotCanonical,
    PrincipalAnonymous,
    PrincipalManagement,
    PrincipalIsBridge,
    SubaccountSyntax,
    SubaccountAllZero,
    NonceSyntax,
    NonceLeadingZero,
    NonceOverflow,
    IdSyntax,
}

fn lower_hex_32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return None;
    }
    let mut out = [0u8; 32];
    hex::decode_to_slice(s, &mut out).ok()?;
    Some(out)
}

/// Parse and fully validate a user return memo. `bridge` is this canister's own principal
/// (a return credited to the bridge itself would be a self-credit).
pub fn parse_return_memo(data: &[u8], bridge: &Principal) -> Result<ReturnMemo, MemoError> {
    if data.len() > MAX_RETURN_MEMO_BYTES {
        return Err(MemoError::TooLong { len: data.len() });
    }
    if !data.is_ascii() {
        return Err(MemoError::NotAscii);
    }
    // ASCII is valid UTF-8, so this cannot fail; the error arm keeps the path explicit.
    let text = std::str::from_utf8(data).map_err(|_| MemoError::NotAscii)?;
    let fields: Vec<&str> = text.split('|').collect();
    let [tag, kind, principal_text, subaccount_text, nonce_text] = fields.as_slice() else {
        return Err(MemoError::FieldCount { found: fields.len() });
    };
    if *tag != MEMO_TAG {
        return Err(MemoError::WrongTag);
    }
    if *kind != "r" {
        return Err(MemoError::WrongKind);
    }

    // The library parses case-insensitively and reports a wrong grouping as an error that
    // still carries the principal; both are "parses, but not the canonical text".
    let destination = match Principal::from_text(principal_text) {
        Ok(p) => p,
        Err(PrincipalError::AbnormalGrouped(_)) => return Err(MemoError::PrincipalNotCanonical),
        Err(_) => return Err(MemoError::PrincipalSyntax),
    };
    if destination.to_text() != *principal_text {
        return Err(MemoError::PrincipalNotCanonical);
    }
    if destination == Principal::anonymous() {
        return Err(MemoError::PrincipalAnonymous);
    }
    if destination == Principal::management_canister() {
        return Err(MemoError::PrincipalManagement);
    }
    if destination == *bridge {
        return Err(MemoError::PrincipalIsBridge);
    }

    let subaccount = if *subaccount_text == "-" {
        None
    } else {
        let bytes = lower_hex_32(subaccount_text).ok_or(MemoError::SubaccountSyntax)?;
        if bytes == [0u8; 32] {
            return Err(MemoError::SubaccountAllZero);
        }
        Some(bytes)
    };

    if nonce_text.is_empty() || !nonce_text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(MemoError::NonceSyntax);
    }
    if nonce_text.len() > 1 && nonce_text.starts_with('0') {
        return Err(MemoError::NonceLeadingZero);
    }
    let client_nonce = nonce_text.parse::<u64>().map_err(|_| MemoError::NonceOverflow)?;

    Ok(ReturnMemo { destination, subaccount, client_nonce })
}

/// The canonical wire form of a return memo: what a wallet must write. The inverse of
/// [`parse_return_memo`] for every memo it accepts.
pub fn encode_return_memo(memo: &ReturnMemo) -> String {
    let sub = match &memo.subaccount {
        None => "-".to_string(),
        Some(s) => hex::encode(s),
    };
    format!("{MEMO_TAG}|r|{}|{sub}|{}", memo.destination.to_text(), memo.client_nonce)
}

/// The three canister-written memo kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanisterMemoKind {
    /// `rumi1|m|<mint_id>`
    Mint,
    /// `rumi1|b|<burn_id>`
    Burn,
    /// `rumi1|f|<refund_id>`
    Refund,
}

impl CanisterMemoKind {
    fn tag(&self) -> &'static str {
        match self {
            CanisterMemoKind::Mint => "m",
            CanisterMemoKind::Burn => "b",
            CanisterMemoKind::Refund => "f",
        }
    }
}

/// `rumi1|<m|b|f>|<64 lowercase hex>`.
pub fn canister_memo(kind: CanisterMemoKind, id: &[u8; 32]) -> String {
    format!("{MEMO_TAG}|{}|{}", kind.tag(), hex::encode(id))
}

/// Parse a canister memo. For **discovery only**: the result says what a transaction claims
/// to be, never that it is ours.
pub fn parse_canister_memo(data: &[u8]) -> Result<(CanisterMemoKind, [u8; 32]), MemoError> {
    if !data.is_ascii() {
        return Err(MemoError::NotAscii);
    }
    let text = std::str::from_utf8(data).map_err(|_| MemoError::NotAscii)?;
    let fields: Vec<&str> = text.split('|').collect();
    let [tag, kind_text, id_text] = fields.as_slice() else {
        return Err(MemoError::FieldCount { found: fields.len() });
    };
    if *tag != MEMO_TAG {
        return Err(MemoError::WrongTag);
    }
    let kind = match *kind_text {
        "m" => CanisterMemoKind::Mint,
        "b" => CanisterMemoKind::Burn,
        "f" => CanisterMemoKind::Refund,
        _ => return Err(MemoError::WrongKind),
    };
    let id = lower_hex_32(id_text).ok_or(MemoError::IdSyntax)?;
    Ok((kind, id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bridge() -> Principal {
        Principal::from_slice(&[0xbb; 10])
    }

    fn user() -> Principal {
        Principal::from_slice(&[0x07; 10])
    }

    fn user_text() -> String {
        user().to_text()
    }

    fn ok_memo(sub: &str, nonce: &str) -> String {
        format!("rumi1|r|{}|{sub}|{nonce}", user_text())
    }

    #[test]
    fn well_known_principal_texts_are_what_the_rules_assume() {
        assert_eq!(Principal::anonymous().to_text(), "2vxsx-fae");
        assert_eq!(Principal::management_canister().to_text(), "aaaaa-aa");
    }

    #[test]
    fn valid_default_subaccount_memo_parses() {
        let m = parse_return_memo(ok_memo("-", "0").as_bytes(), &bridge()).unwrap();
        assert_eq!(m, ReturnMemo { destination: user(), subaccount: None, client_nonce: 0 });
    }

    #[test]
    fn valid_explicit_subaccount_memo_parses_and_round_trips() {
        let sub_hex = "00".repeat(31) + "01";
        let text = ok_memo(&sub_hex, "18446744073709551615");
        let m = parse_return_memo(text.as_bytes(), &bridge()).unwrap();
        let mut sub = [0u8; 32];
        sub[31] = 1;
        assert_eq!(m.subaccount, Some(sub));
        assert_eq!(m.client_nonce, u64::MAX);
        assert_eq!(encode_return_memo(&m), text);
        let d = parse_return_memo(ok_memo("-", "42").as_bytes(), &bridge()).unwrap();
        assert_eq!(encode_return_memo(&d), ok_memo("-", "42"));
    }

    #[test]
    fn the_longest_legal_memo_fits_in_160_bytes() {
        // A 29-byte principal (63 characters), a subaccount and a 20-digit nonce.
        let p = Principal::from_slice(&[0xee; 29]);
        let text = format!("rumi1|r|{}|{}|{}", p.to_text(), "ab".repeat(32), u64::MAX);
        assert!(text.len() <= MAX_RETURN_MEMO_BYTES, "{}", text.len());
        assert!(parse_return_memo(text.as_bytes(), &bridge()).is_ok());
    }

    /// The table the note asks for: valid, trailing bytes, leading-zero nonce,
    /// non-canonical principal, 161 bytes, wrong tag, plus every other rejection class.
    #[test]
    fn return_memo_table() {
        let bridge = bridge();
        let user_text = user_text();
        let zero_sub = "0".repeat(64);
        let upper_sub = "AB".repeat(32);
        let short_sub = "ab".repeat(31);
        let long_sub = "ab".repeat(33);
        let nonhex_sub = "zz".repeat(32);
        let upper_principal = user_text.to_uppercase();
        let undashed_principal = user_text.replace('-', "");
        let bridge_text = bridge.to_text();

        // 161 bytes: a valid memo padded by one byte inside the nonce is not parseable as
        // a valid nonce either, so pad in a way that only the length rule can explain.
        let mut too_long = ok_memo("-", "1").into_bytes();
        too_long.resize(MAX_RETURN_MEMO_BYTES + 1, b'0');

        type Case = (&'static str, Vec<u8>, Result<(), MemoError>);
        let cases: Vec<Case> = vec![
            ("valid, default subaccount", ok_memo("-", "7").into_bytes(), Ok(())),
            ("valid, explicit subaccount", ok_memo(&"ab".repeat(32), "7").into_bytes(), Ok(())),
            ("trailing byte", format!("{} ", ok_memo("-", "7")).into_bytes(), Err(MemoError::NonceSyntax)),
            ("trailing field", format!("{}|x", ok_memo("-", "7")).into_bytes(), Err(MemoError::FieldCount { found: 6 })),
            ("trailing newline", format!("{}\n", ok_memo("-", "7")).into_bytes(), Err(MemoError::NonceSyntax)),
            ("leading-zero nonce", ok_memo("-", "07").into_bytes(), Err(MemoError::NonceLeadingZero)),
            ("double-zero nonce", ok_memo("-", "00").into_bytes(), Err(MemoError::NonceLeadingZero)),
            ("empty nonce", ok_memo("-", "").into_bytes(), Err(MemoError::NonceSyntax)),
            ("signed nonce", ok_memo("-", "+1").into_bytes(), Err(MemoError::NonceSyntax)),
            ("negative nonce", ok_memo("-", "-1").into_bytes(), Err(MemoError::NonceSyntax)),
            ("hex nonce", ok_memo("-", "0x1").into_bytes(), Err(MemoError::NonceSyntax)),
            ("nonce above u64", ok_memo("-", "18446744073709551616").into_bytes(), Err(MemoError::NonceOverflow)),
            ("non-canonical principal (upper case)", ok_memo_with(&upper_principal, "-", "1"), Err(MemoError::PrincipalNotCanonical)),
            ("non-canonical principal (no dashes)", ok_memo_with(&undashed_principal, "-", "1"), Err(MemoError::PrincipalNotCanonical)),
            ("principal syntax", ok_memo_with("not-a-principal", "-", "1"), Err(MemoError::PrincipalSyntax)),
            ("empty principal", ok_memo_with("", "-", "1"), Err(MemoError::PrincipalSyntax)),
            ("anonymous principal", ok_memo_with("2vxsx-fae", "-", "1"), Err(MemoError::PrincipalAnonymous)),
            ("management canister", ok_memo_with("aaaaa-aa", "-", "1"), Err(MemoError::PrincipalManagement)),
            ("the bridge itself", ok_memo_with(&bridge_text, "-", "1"), Err(MemoError::PrincipalIsBridge)),
            ("all-zero subaccount", ok_memo(&zero_sub, "1").into_bytes(), Err(MemoError::SubaccountAllZero)),
            ("upper-case subaccount", ok_memo(&upper_sub, "1").into_bytes(), Err(MemoError::SubaccountSyntax)),
            ("short subaccount", ok_memo(&short_sub, "1").into_bytes(), Err(MemoError::SubaccountSyntax)),
            ("long subaccount", ok_memo(&long_sub, "1").into_bytes(), Err(MemoError::SubaccountSyntax)),
            ("non-hex subaccount", ok_memo(&nonhex_sub, "1").into_bytes(), Err(MemoError::SubaccountSyntax)),
            ("empty subaccount", ok_memo("", "1").into_bytes(), Err(MemoError::SubaccountSyntax)),
            ("161 bytes", too_long.clone(), Err(MemoError::TooLong { len: 161 })),
            ("wrong tag (rumi2)", format!("rumi2|r|{user_text}|-|1").into_bytes(), Err(MemoError::WrongTag)),
            ("wrong tag (case)", format!("Rumi1|r|{user_text}|-|1").into_bytes(), Err(MemoError::WrongTag)),
            ("wrong kind (m)", format!("rumi1|m|{user_text}|-|1").into_bytes(), Err(MemoError::WrongKind)),
            ("wrong kind (R)", format!("rumi1|R|{user_text}|-|1").into_bytes(), Err(MemoError::WrongKind)),
            ("four fields", format!("rumi1|r|{user_text}|-").into_bytes(), Err(MemoError::FieldCount { found: 4 })),
            ("canister memo is not a return memo", canister_memo(CanisterMemoKind::Mint, &[1; 32]).into_bytes(), Err(MemoError::FieldCount { found: 3 })),
            ("non-ASCII byte", {
                let mut v = ok_memo("-", "1").into_bytes();
                v[0] = 0xc3;
                v
            }, Err(MemoError::NotAscii)),
            ("empty", vec![], Err(MemoError::FieldCount { found: 1 })),
        ];

        for (name, bytes, expected) in cases {
            let got = parse_return_memo(&bytes, &bridge).map(|_| ());
            assert_eq!(got, expected, "case `{name}`: {:?}", String::from_utf8_lossy(&bytes));
        }
    }

    fn ok_memo_with(principal: &str, sub: &str, nonce: &str) -> Vec<u8> {
        format!("rumi1|r|{principal}|{sub}|{nonce}").into_bytes()
    }

    #[test]
    fn exactly_160_bytes_is_accepted_by_the_length_rule() {
        // 160 is the limit; a memo of exactly 160 bytes must not trip `TooLong`.
        let mut v = ok_memo("-", "1").into_bytes();
        v.resize(MAX_RETURN_MEMO_BYTES, b'0');
        assert_ne!(parse_return_memo(&v, &bridge()), Err(MemoError::TooLong { len: 160 }));
    }

    #[test]
    fn canister_memos_round_trip_and_reject_malformed_ids() {
        let id = [0xa5; 32];
        for kind in [CanisterMemoKind::Mint, CanisterMemoKind::Burn, CanisterMemoKind::Refund] {
            let text = canister_memo(kind, &id);
            assert_eq!(parse_canister_memo(text.as_bytes()), Ok((kind, id)));
        }
        assert_eq!(canister_memo(CanisterMemoKind::Mint, &id), format!("rumi1|m|{}", "a5".repeat(32)));
        assert_eq!(canister_memo(CanisterMemoKind::Burn, &id), format!("rumi1|b|{}", "a5".repeat(32)));
        assert_eq!(canister_memo(CanisterMemoKind::Refund, &id), format!("rumi1|f|{}", "a5".repeat(32)));

        let h = "a5".repeat(32);
        assert_eq!(parse_canister_memo(format!("rumi1|r|{h}").as_bytes()), Err(MemoError::WrongKind));
        assert_eq!(parse_canister_memo(format!("rumi2|m|{h}").as_bytes()), Err(MemoError::WrongTag));
        assert_eq!(parse_canister_memo(format!("rumi1|m|{}", "A5".repeat(32)).as_bytes()), Err(MemoError::IdSyntax));
        assert_eq!(parse_canister_memo(format!("rumi1|m|{}", "a5".repeat(31)).as_bytes()), Err(MemoError::IdSyntax));
        assert_eq!(parse_canister_memo(format!("rumi1|m|{h}|x").as_bytes()), Err(MemoError::FieldCount { found: 4 }));
        assert_eq!(parse_canister_memo(format!("rumi1|m|{h} ").as_bytes()), Err(MemoError::IdSyntax));
        assert_eq!(parse_canister_memo(b"rumi1|m|\xc3\xa9"), Err(MemoError::NotAscii));
    }
}
