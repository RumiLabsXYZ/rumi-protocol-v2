//! XRPL binary codec for the issued-token (icUSD) lane: issued-currency
//! `Payment`, `AccountSet` and `TrustSet`.
//!
//! The native-XRP codec (`codec.rs`) is deliberately untouched; this module
//! mirrors its framing (field-id headers, single-byte VL lengths, signing over
//! `STX\0 ‖ unsigned_blob` via `sign::signing_message`). Fields are collected as
//! `(type_code, field_code, bytes)` triples and sorted by `(type_code,
//! field_code)` before emission, exactly the canonical rippled ordering, so adding
//! a field can never silently land out of order. Golden vectors from the `xrpl`
//! npm package are in `testdata/xrp_issued_kat.json`.

use super::issued_amount::{ExactIssuedAmount, IssuedAsset};

const TT_PAYMENT: u16 = 0;
const TT_ACCOUNT_SET: u16 = 3;
const TT_TRUST_SET: u16 = 20;

// (type_code, field_code) pairs from rippled's definitions.
const F_TRANSACTION_TYPE: (u8, u8) = (1, 2);
const F_FLAGS: (u8, u8) = (2, 2);
const F_SEQUENCE: (u8, u8) = (2, 4);
const F_TRANSFER_RATE: (u8, u8) = (2, 11);
const F_DESTINATION_TAG: (u8, u8) = (2, 14);
const F_LAST_LEDGER_SEQUENCE: (u8, u8) = (2, 27);
const F_SET_FLAG: (u8, u8) = (2, 33);
const F_CLEAR_FLAG: (u8, u8) = (2, 34);
const F_AMOUNT: (u8, u8) = (6, 1);
const F_LIMIT_AMOUNT: (u8, u8) = (6, 3);
const F_FEE: (u8, u8) = (6, 8);
const F_SIGNING_PUB_KEY: (u8, u8) = (7, 3);
const F_TXN_SIGNATURE: (u8, u8) = (7, 4);
const F_ACCOUNT: (u8, u8) = (8, 1);
const F_DESTINATION: (u8, u8) = (8, 3);

/// Largest valid XRP drops value (1e17); also keeps the native flag bit clear.
pub const MAX_DROPS: u64 = 100_000_000_000_000_000;
/// Standard TransferRate for "no surcharge" (1.0 in rippled's 1e9 fixed point).
pub const TRANSFER_RATE_PAR: u32 = 1_000_000_000;

/// AccountSet flag numbers (`asf*`).
pub const ASF_REQUIRE_DEST: u32 = 1;
pub const ASF_DEFAULT_RIPPLE: u32 = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodecError {
    FeeOutOfRange,
    NonPositivePaymentAmount,
    NegativeLimit,
    BadSignatureLength(usize),
    AccountSetWithoutEffect,
}

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for CodecError {}

/// Issued-currency Payment (issuer->holder, or a direct holder->issuer return).
/// `Amount` only: no `SendMax`, no `Paths`, no partial-payment flag is ever set
/// by this codec (callers pass `flags` explicitly).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuedPayment {
    pub account: [u8; 20],
    pub destination: [u8; 20],
    pub amount: ExactIssuedAmount,
    pub asset: IssuedAsset,
    pub fee_drops: u64,
    pub sequence: u32,
    pub last_ledger_sequence: u32,
    pub destination_tag: Option<u32>,
    pub flags: Option<u32>,
    pub signing_pub_key: [u8; 33],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountSet {
    pub account: [u8; 20],
    pub fee_drops: u64,
    pub sequence: u32,
    pub last_ledger_sequence: u32,
    pub flags: Option<u32>,
    pub set_flag: Option<u32>,
    pub clear_flag: Option<u32>,
    pub transfer_rate: Option<u32>,
    pub signing_pub_key: [u8; 33],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustSet {
    pub account: [u8; 20],
    pub limit: ExactIssuedAmount,
    pub asset: IssuedAsset,
    pub fee_drops: u64,
    pub sequence: u32,
    pub last_ledger_sequence: u32,
    pub flags: Option<u32>,
    pub signing_pub_key: [u8; 33],
}

struct Fields(Vec<((u8, u8), Vec<u8>)>);

impl Fields {
    fn new(tx_type: u16) -> Self {
        let mut f = Fields(Vec::with_capacity(16));
        f.0.push((F_TRANSACTION_TYPE, tx_type.to_be_bytes().to_vec()));
        f
    }
    fn u32(&mut self, id: (u8, u8), v: u32) {
        self.0.push((id, v.to_be_bytes().to_vec()));
    }
    fn opt_u32(&mut self, id: (u8, u8), v: Option<u32>) {
        if let Some(v) = v {
            self.u32(id, v);
        }
    }
    fn native_amount(&mut self, id: (u8, u8), drops: u64) -> Result<(), CodecError> {
        if drops > MAX_DROPS {
            return Err(CodecError::FeeOutOfRange);
        }
        self.0.push((
            id,
            (0x4000_0000_0000_0000u64 | drops).to_be_bytes().to_vec(),
        ));
        Ok(())
    }
    fn issued_amount(&mut self, id: (u8, u8), amount: &ExactIssuedAmount, asset: &IssuedAsset) {
        self.0
            .push((id, issued_amount_bytes(amount, asset).to_vec()));
    }
    fn vl(&mut self, id: (u8, u8), bytes: &[u8]) {
        debug_assert!(bytes.len() <= 192);
        let mut v = Vec::with_capacity(1 + bytes.len());
        v.push(bytes.len() as u8);
        v.extend_from_slice(bytes);
        self.0.push((id, v));
    }
    fn account(&mut self, id: (u8, u8), a: &[u8; 20]) {
        self.vl(id, a);
    }

    fn encode(mut self) -> Vec<u8> {
        self.0.sort_by_key(|(id, _)| *id);
        let mut out = Vec::new();
        for ((type_code, field_code), bytes) in self.0 {
            if field_code < 16 {
                out.push((type_code << 4) | field_code);
            } else {
                out.push(type_code << 4);
                out.push(field_code);
            }
            out.extend_from_slice(&bytes);
        }
        out
    }
}

/// 48-byte Amount field body: 8-byte value ‖ 20-byte currency ‖ 20-byte issuer.
pub fn issued_amount_bytes(amount: &ExactIssuedAmount, asset: &IssuedAsset) -> [u8; 48] {
    let mut out = [0u8; 48];
    out[..8].copy_from_slice(&amount.to_wire_u64().to_be_bytes());
    out[8..28].copy_from_slice(&asset.currency.0);
    out[28..].copy_from_slice(&asset.issuer);
    out
}

fn check_sig(sig: Option<&[u8]>) -> Result<(), CodecError> {
    match sig {
        Some(s) if s.len() != 64 => Err(CodecError::BadSignatureLength(s.len())),
        _ => Ok(()),
    }
}

fn payment_fields(p: &IssuedPayment, sig: Option<&[u8]>) -> Result<Vec<u8>, CodecError> {
    check_sig(sig)?;
    if p.amount.is_zero() || p.amount.is_negative() {
        return Err(CodecError::NonPositivePaymentAmount);
    }
    let mut f = Fields::new(TT_PAYMENT);
    f.opt_u32(F_FLAGS, p.flags);
    f.u32(F_SEQUENCE, p.sequence);
    f.opt_u32(F_DESTINATION_TAG, p.destination_tag);
    f.u32(F_LAST_LEDGER_SEQUENCE, p.last_ledger_sequence);
    f.issued_amount(F_AMOUNT, &p.amount, &p.asset);
    f.native_amount(F_FEE, p.fee_drops)?;
    f.vl(F_SIGNING_PUB_KEY, &p.signing_pub_key);
    if let Some(s) = sig {
        f.vl(F_TXN_SIGNATURE, s);
    }
    f.account(F_ACCOUNT, &p.account);
    f.account(F_DESTINATION, &p.destination);
    Ok(f.encode())
}

fn account_set_fields(a: &AccountSet, sig: Option<&[u8]>) -> Result<Vec<u8>, CodecError> {
    check_sig(sig)?;
    if a.set_flag.is_none() && a.clear_flag.is_none() && a.transfer_rate.is_none() {
        return Err(CodecError::AccountSetWithoutEffect);
    }
    let mut f = Fields::new(TT_ACCOUNT_SET);
    f.opt_u32(F_FLAGS, a.flags);
    f.u32(F_SEQUENCE, a.sequence);
    f.opt_u32(F_TRANSFER_RATE, a.transfer_rate);
    f.u32(F_LAST_LEDGER_SEQUENCE, a.last_ledger_sequence);
    f.opt_u32(F_SET_FLAG, a.set_flag);
    f.opt_u32(F_CLEAR_FLAG, a.clear_flag);
    f.native_amount(F_FEE, a.fee_drops)?;
    f.vl(F_SIGNING_PUB_KEY, &a.signing_pub_key);
    if let Some(s) = sig {
        f.vl(F_TXN_SIGNATURE, s);
    }
    f.account(F_ACCOUNT, &a.account);
    Ok(f.encode())
}

fn trust_set_fields(t: &TrustSet, sig: Option<&[u8]>) -> Result<Vec<u8>, CodecError> {
    check_sig(sig)?;
    if t.limit.is_negative() {
        return Err(CodecError::NegativeLimit);
    }
    let mut f = Fields::new(TT_TRUST_SET);
    f.opt_u32(F_FLAGS, t.flags);
    f.u32(F_SEQUENCE, t.sequence);
    f.u32(F_LAST_LEDGER_SEQUENCE, t.last_ledger_sequence);
    f.issued_amount(F_LIMIT_AMOUNT, &t.limit, &t.asset);
    f.native_amount(F_FEE, t.fee_drops)?;
    f.vl(F_SIGNING_PUB_KEY, &t.signing_pub_key);
    if let Some(s) = sig {
        f.vl(F_TXN_SIGNATURE, s);
    }
    f.account(F_ACCOUNT, &t.account);
    Ok(f.encode())
}

/// Serialized-for-signing form (includes SigningPubKey, excludes TxnSignature).
pub fn serialize_issued_payment_unsigned(p: &IssuedPayment) -> Result<Vec<u8>, CodecError> {
    payment_fields(p, None)
}
pub fn serialize_issued_payment_signed(
    p: &IssuedPayment,
    txn_signature: &[u8],
) -> Result<Vec<u8>, CodecError> {
    payment_fields(p, Some(txn_signature))
}
pub fn serialize_account_set_unsigned(a: &AccountSet) -> Result<Vec<u8>, CodecError> {
    account_set_fields(a, None)
}
pub fn serialize_account_set_signed(a: &AccountSet, sig: &[u8]) -> Result<Vec<u8>, CodecError> {
    account_set_fields(a, Some(sig))
}
pub fn serialize_trust_set_unsigned(t: &TrustSet) -> Result<Vec<u8>, CodecError> {
    trust_set_fields(t, None)
}
pub fn serialize_trust_set_signed(t: &TrustSet, sig: &[u8]) -> Result<Vec<u8>, CodecError> {
    trust_set_fields(t, Some(sig))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chains::xrp::address::{
        account_id_from_classic_address, classic_address_from_ed25519_pubkey,
    };
    use crate::chains::xrp::codec;
    use crate::chains::xrp::issued_amount::{CurrencyCode, ICUSD_CURRENCY_HEX};
    use crate::chains::xrp::sign::{signing_message, tx_hash};
    use serde_json::Value;

    const KAT: &str = include_str!("testdata/xrp_issued_kat.json");
    const NATIVE_KAT: &str = include_str!("testdata/xrp_kat.json");

    fn kat() -> Value {
        serde_json::from_str(KAT).unwrap()
    }

    fn pubkey(v: &Value, who: &str) -> [u8; 33] {
        let h = hex::decode(v[who]["signing_pubkey_hex"].as_str().unwrap()).unwrap();
        h.try_into().unwrap()
    }

    fn u32_of(tx: &Value, k: &str) -> Option<u32> {
        tx.get(k).and_then(Value::as_u64).map(|x| x as u32)
    }

    fn acct(s: &str) -> [u8; 20] {
        account_id_from_classic_address(s).unwrap()
    }

    fn signer_pubkey(v: &Value, account: &str) -> [u8; 33] {
        if account == v["issuer"]["classic_address"].as_str().unwrap() {
            pubkey(v, "issuer")
        } else {
            assert_eq!(account, v["holder"]["classic_address"].as_str().unwrap());
            pubkey(v, "holder")
        }
    }

    fn asset_of(v: &Value) -> IssuedAsset {
        IssuedAsset::from_classic(
            CurrencyCode::from_hex(v["currency_hex"].as_str().unwrap()).unwrap(),
            v["issuer"]["classic_address"].as_str().unwrap(),
        )
        .unwrap()
    }

    fn build(v: &Value, vec: &Value) -> (Vec<u8>, Vec<u8>) {
        try_build(v, vec).unwrap()
    }

    /// (unsigned blob, signed blob) for one vector.
    fn try_build(v: &Value, vec: &Value) -> Result<(Vec<u8>, Vec<u8>), CodecError> {
        let tx = &vec["tx"];
        let sig = hex::decode(vec["txn_signature_hex"].as_str().unwrap()).unwrap();
        let account = tx["Account"].as_str().unwrap();
        let spk = signer_pubkey(v, account);
        let asset = asset_of(v);
        match vec["kind"].as_str().unwrap() {
            "payment" => {
                let p = IssuedPayment {
                    account: acct(account),
                    destination: acct(tx["Destination"].as_str().unwrap()),
                    amount: ExactIssuedAmount::parse(tx["Amount"]["value"].as_str().unwrap())
                        .unwrap(),
                    asset,
                    fee_drops: tx["Fee"].as_str().unwrap().parse().unwrap(),
                    sequence: u32_of(tx, "Sequence").unwrap(),
                    last_ledger_sequence: u32_of(tx, "LastLedgerSequence").unwrap(),
                    destination_tag: u32_of(tx, "DestinationTag"),
                    flags: u32_of(tx, "Flags"),
                    signing_pub_key: spk,
                };
                Ok((
                    serialize_issued_payment_unsigned(&p)?,
                    serialize_issued_payment_signed(&p, &sig)?,
                ))
            }
            "account_set" => {
                let a = AccountSet {
                    account: acct(account),
                    fee_drops: tx["Fee"].as_str().unwrap().parse().unwrap(),
                    sequence: u32_of(tx, "Sequence").unwrap(),
                    last_ledger_sequence: u32_of(tx, "LastLedgerSequence").unwrap(),
                    flags: u32_of(tx, "Flags"),
                    set_flag: u32_of(tx, "SetFlag"),
                    clear_flag: u32_of(tx, "ClearFlag"),
                    transfer_rate: u32_of(tx, "TransferRate"),
                    signing_pub_key: spk,
                };
                Ok((
                    serialize_account_set_unsigned(&a)?,
                    serialize_account_set_signed(&a, &sig)?,
                ))
            }
            "trust_set" => {
                let t = TrustSet {
                    account: acct(account),
                    limit: ExactIssuedAmount::parse(tx["LimitAmount"]["value"].as_str().unwrap())
                        .unwrap(),
                    asset,
                    fee_drops: tx["Fee"].as_str().unwrap().parse().unwrap(),
                    sequence: u32_of(tx, "Sequence").unwrap(),
                    last_ledger_sequence: u32_of(tx, "LastLedgerSequence").unwrap(),
                    flags: u32_of(tx, "Flags"),
                    signing_pub_key: spk,
                };
                Ok((
                    serialize_trust_set_unsigned(&t)?,
                    serialize_trust_set_signed(&t, &sig)?,
                ))
            }
            other => panic!("unknown kind {other}"),
        }
    }

    #[test]
    fn issued_bytes_match_sdk() {
        let v = kat();
        let vectors = v["vectors"].as_array().unwrap();
        assert!(vectors.len() >= 14, "fixture shrank: {}", vectors.len());
        assert_eq!(v["currency_hex"].as_str().unwrap(), ICUSD_CURRENCY_HEX);
        let mut kinds = std::collections::BTreeSet::new();
        for vec in vectors {
            let name = vec["name"].as_str().unwrap();
            kinds.insert(vec["kind"].as_str().unwrap().to_string());
            if name == "zero_amount" {
                // The SDK will encode a zero IOU, but a zero-value Payment is never
                // valid on the ledger: we refuse to build it. The zero Amount field
                // itself (canonical 0x8000...) must still match the SDK bytes.
                assert!(try_build(&v, vec).is_err());
                let field = issued_amount_bytes(&ExactIssuedAmount::ZERO, &asset_of(&v));
                assert!(vec["signed_blob_hex"]
                    .as_str()
                    .unwrap()
                    .contains(&hex::encode_upper(field)));
                continue;
            }
            let (unsigned, signed) = build(&v, vec);
            assert_eq!(
                hex::encode_upper(signing_message(&unsigned)),
                vec["signing_message_hex"].as_str().unwrap(),
                "{name}: signing message"
            );
            assert_eq!(
                hex::encode_upper(&signed),
                vec["signed_blob_hex"].as_str().unwrap(),
                "{name}: signed blob"
            );
            assert_eq!(
                hex::encode_upper(tx_hash(&signed)),
                vec["tx_hash"].as_str().unwrap(),
                "{name}: tx hash"
            );
            if let Some(dec) = vec["sdk_decoded_value"].as_str() {
                let want = ExactIssuedAmount::parse(dec).unwrap();
                let tx = &vec["tx"];
                let given = tx.get("Amount").or_else(|| tx.get("LimitAmount")).unwrap()["value"]
                    .as_str()
                    .unwrap();
                assert_eq!(
                    ExactIssuedAmount::parse(given).unwrap(),
                    want,
                    "{name}: value"
                );
            }
        }
        assert_eq!(
            kinds.into_iter().collect::<Vec<_>>(),
            vec!["account_set", "payment", "trust_set"]
        );
    }

    #[test]
    fn kat_keys_and_addresses_are_consistent() {
        let v = kat();
        for who in ["issuer", "holder"] {
            let pk = pubkey(&v, who);
            assert_eq!(pk[0], 0xED);
            let raw: [u8; 32] = pk[1..].try_into().unwrap();
            assert_eq!(
                classic_address_from_ed25519_pubkey(&raw),
                v[who]["classic_address"].as_str().unwrap()
            );
        }
        assert_eq!(v["sdk"]["package"], "xrpl");
        assert!(v["sdk"]["version"].as_str().unwrap().split('.').count() == 3);
        assert!(v["sdk"]["integrity"]
            .as_str()
            .unwrap()
            .starts_with("sha512-"));
    }

    #[test]
    fn field_order_is_canonical_for_every_vector() {
        // Walk each unsigned blob's field headers and require strictly increasing
        // (type, field) ids; this catches ordering bugs independent of the SDK.
        let v = kat();
        for vec in v["vectors"].as_array().unwrap() {
            if vec["name"] == "zero_amount" {
                continue;
            }
            let (unsigned, _) = build(&v, vec);
            let mut i = 0usize;
            let mut last = (0u8, 0u8);
            while i < unsigned.len() {
                let hi = unsigned[i] >> 4;
                let lo = unsigned[i] & 0x0F;
                i += 1;
                let (t, f) = if hi != 0 && lo != 0 {
                    (hi, lo)
                } else if hi != 0 {
                    let f = unsigned[i];
                    i += 1;
                    (hi, f)
                } else {
                    panic!("unexpected header form");
                };
                assert!((t, f) > last, "{}: out of order", vec["name"]);
                last = (t, f);
                i += match (t, f) {
                    (1, _) => 2,
                    (2, _) => 4,
                    (6, _) => 48.min(if f == 8 { 8 } else { 48 }),
                    (7, _) => 1 + unsigned[i] as usize,
                    (8, _) => 1 + unsigned[i] as usize,
                    other => panic!("unhandled field {other:?}"),
                };
            }
            assert_eq!(i, unsigned.len());
        }
    }

    #[test]
    fn native_vectors_unchanged() {
        // The native codec still reproduces the original xrpl.js vector exactly,
        // and the sorted-field builder used here agrees with it byte for byte.
        let v: Value = serde_json::from_str(NATIVE_KAT).unwrap();
        let spk: [u8; 33] = hex::decode(v["keypair"]["signing_pubkey_hex"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        let p = codec::Payment {
            account: acct(v["payment"]["account"].as_str().unwrap()),
            destination: acct(v["payment"]["destination"].as_str().unwrap()),
            amount_drops: v["payment"]["amount_drops"].as_u64().unwrap(),
            fee_drops: v["payment"]["fee_drops"].as_u64().unwrap(),
            sequence: v["payment"]["sequence"].as_u64().unwrap() as u32,
            last_ledger_sequence: v["payment"]["last_ledger_sequence"].as_u64().unwrap() as u32,
            destination_tag: v["payment"]["destination_tag"].as_u64().map(|t| t as u32),
            signing_pub_key: spk,
        };
        let native = codec::serialize_unsigned(&p);
        assert_eq!(
            hex::encode_upper(&native),
            v["unsigned_blob_hex"].as_str().unwrap().to_uppercase()
        );
        let mut f = Fields::new(TT_PAYMENT);
        f.u32(F_SEQUENCE, p.sequence);
        f.opt_u32(F_DESTINATION_TAG, p.destination_tag);
        f.u32(F_LAST_LEDGER_SEQUENCE, p.last_ledger_sequence);
        f.native_amount(F_AMOUNT, p.amount_drops).unwrap();
        f.native_amount(F_FEE, p.fee_drops).unwrap();
        f.vl(F_SIGNING_PUB_KEY, &p.signing_pub_key);
        f.account(F_ACCOUNT, &p.account);
        f.account(F_DESTINATION, &p.destination);
        assert_eq!(f.encode(), native);
    }

    fn sample_payment() -> IssuedPayment {
        let v = kat();
        IssuedPayment {
            account: acct(v["issuer"]["classic_address"].as_str().unwrap()),
            destination: acct(v["holder"]["classic_address"].as_str().unwrap()),
            amount: ExactIssuedAmount::parse("1").unwrap(),
            asset: asset_of(&v),
            fee_drops: 12,
            sequence: 1,
            last_ledger_sequence: 2,
            destination_tag: None,
            flags: None,
            signing_pub_key: pubkey(&v, "issuer"),
        }
    }

    #[test]
    fn codec_rejects_invalid_inputs() {
        let mut p = sample_payment();
        p.amount = ExactIssuedAmount::ZERO;
        assert_eq!(
            serialize_issued_payment_unsigned(&p),
            Err(CodecError::NonPositivePaymentAmount)
        );
        p.amount = ExactIssuedAmount::parse("-1").unwrap();
        assert_eq!(
            serialize_issued_payment_unsigned(&p),
            Err(CodecError::NonPositivePaymentAmount)
        );
        let mut p = sample_payment();
        p.fee_drops = MAX_DROPS + 1;
        assert_eq!(
            serialize_issued_payment_unsigned(&p),
            Err(CodecError::FeeOutOfRange)
        );
        let p = sample_payment();
        assert_eq!(
            serialize_issued_payment_signed(&p, &[0u8; 63]),
            Err(CodecError::BadSignatureLength(63))
        );
        let a = AccountSet {
            account: p.account,
            fee_drops: 12,
            sequence: 1,
            last_ledger_sequence: 2,
            flags: None,
            set_flag: None,
            clear_flag: None,
            transfer_rate: None,
            signing_pub_key: p.signing_pub_key,
        };
        assert_eq!(
            serialize_account_set_unsigned(&a),
            Err(CodecError::AccountSetWithoutEffect)
        );
        let t = TrustSet {
            account: p.account,
            limit: ExactIssuedAmount::parse("-5").unwrap(),
            asset: p.asset,
            fee_drops: 12,
            sequence: 1,
            last_ledger_sequence: 2,
            flags: None,
            signing_pub_key: p.signing_pub_key,
        };
        assert_eq!(
            serialize_trust_set_unsigned(&t),
            Err(CodecError::NegativeLimit)
        );
    }

    #[test]
    fn destination_tag_zero_is_emitted_and_distinct() {
        let mut p = sample_payment();
        let absent = serialize_issued_payment_unsigned(&p).unwrap();
        p.destination_tag = Some(0);
        let zero = serialize_issued_payment_unsigned(&p).unwrap();
        assert_eq!(zero.len(), absent.len() + 5);
        assert_ne!(zero, absent);
    }
}
