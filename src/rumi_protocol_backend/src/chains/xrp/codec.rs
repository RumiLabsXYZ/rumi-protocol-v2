//! XRPL binary codec for a native-XRP `Payment`. Fields are emitted in canonical
//! order (sorted by `(type_code, field_code)`); each is prefixed by its field-id
//! header. Native XRP amounts set bit 62 (positive) with bit 63 (is-IOU) clear.
//!
//! Ported from the delegated-vault native-XRP rail and locked byte-for-byte
//! against xrpl.js (`testdata/xrp_kat.json`).

/// A native-XRP Payment, ready to serialize. `signing_pub_key` is the 33-byte
/// `0xED ‖ ed25519` key (see `sign::ed25519_signing_pubkey`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Payment {
    pub account: [u8; 20],
    pub destination: [u8; 20],
    pub amount_drops: u64,
    pub fee_drops: u64,
    pub sequence: u32,
    pub last_ledger_sequence: u32,
    pub destination_tag: Option<u32>,
    pub signing_pub_key: [u8; 33],
}

const TX_TYPE_PAYMENT: u16 = 0;

/// Field-id header: `(type<<4)|field` when both < 16; otherwise `type<<4` then the
/// field-code byte. All of our fields have `type_code < 16`; `LastLedgerSequence`
/// (type 2, field 27) is the only one whose field code is ≥ 16 (the two-byte
/// `0x20 0x1B`).
fn push_field_id(out: &mut Vec<u8>, type_code: u8, field_code: u8) {
    if field_code < 16 {
        out.push((type_code << 4) | field_code);
    } else {
        out.push(type_code << 4);
        out.push(field_code);
    }
}

/// Native-XRP Amount: 8 bytes BE, `0x4000_0000_0000_0000 | drops`. Drops max out
/// at 1e17, well below the flag bit, so the OR never collides (the caller still
/// validates the range before encoding).
fn push_amount(out: &mut Vec<u8>, drops: u64) {
    out.extend_from_slice(&(0x4000_0000_0000_0000u64 | drops).to_be_bytes());
}

/// Variable-length blob: single length byte (valid for len ≤ 192) then bytes.
fn push_vl(out: &mut Vec<u8>, bytes: &[u8]) {
    out.push(bytes.len() as u8);
    out.extend_from_slice(bytes);
}

fn serialize_inner(p: &Payment, txn_signature: Option<&[u8]>) -> Vec<u8> {
    let mut out = Vec::new();
    push_field_id(&mut out, 1, 2); // TransactionType UInt16
    out.extend_from_slice(&TX_TYPE_PAYMENT.to_be_bytes());
    push_field_id(&mut out, 2, 4); // Sequence UInt32
    out.extend_from_slice(&p.sequence.to_be_bytes());
    if let Some(tag) = p.destination_tag {
        push_field_id(&mut out, 2, 14); // DestinationTag UInt32 (field 14 < 27 -> before LLS)
        out.extend_from_slice(&tag.to_be_bytes());
    }
    push_field_id(&mut out, 2, 27); // LastLedgerSequence UInt32 (two-byte id 0x20 0x1B)
    out.extend_from_slice(&p.last_ledger_sequence.to_be_bytes());
    push_field_id(&mut out, 6, 1); // Amount
    push_amount(&mut out, p.amount_drops);
    push_field_id(&mut out, 6, 8); // Fee
    push_amount(&mut out, p.fee_drops);
    push_field_id(&mut out, 7, 3); // SigningPubKey Blob
    push_vl(&mut out, &p.signing_pub_key);
    if let Some(sig) = txn_signature {
        push_field_id(&mut out, 7, 4); // TxnSignature Blob (between SigningPubKey and Account)
        push_vl(&mut out, sig);
    }
    push_field_id(&mut out, 8, 1); // Account AccountID
    push_vl(&mut out, &p.account);
    push_field_id(&mut out, 8, 3); // Destination AccountID
    push_vl(&mut out, &p.destination);
    out
}

/// Serialized-for-signing form: includes SigningPubKey, excludes TxnSignature.
pub fn serialize_unsigned(p: &Payment) -> Vec<u8> {
    serialize_inner(p, None)
}

/// Full signed form, with the 64-byte Ed25519 `TxnSignature` inserted in its
/// canonical position (between SigningPubKey and Account).
pub fn serialize_signed(p: &Payment, txn_signature: &[u8]) -> Vec<u8> {
    serialize_inner(p, Some(txn_signature))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chains::xrp::address::account_id_from_classic_address;

    const KAT: &str = include_str!("testdata/xrp_kat.json");

    fn kat_payment() -> (Payment, serde_json::Value) {
        let v: serde_json::Value = serde_json::from_str(KAT).unwrap();
        let acct = account_id_from_classic_address(v["payment"]["account"].as_str().unwrap()).unwrap();
        let dest =
            account_id_from_classic_address(v["payment"]["destination"].as_str().unwrap()).unwrap();
        let spk = hex::decode(v["keypair"]["signing_pubkey_hex"].as_str().unwrap()).unwrap();
        let mut signing_pub_key = [0u8; 33];
        signing_pub_key.copy_from_slice(&spk);
        let p = Payment {
            account: acct,
            destination: dest,
            amount_drops: v["payment"]["amount_drops"].as_u64().unwrap(),
            fee_drops: v["payment"]["fee_drops"].as_u64().unwrap(),
            sequence: v["payment"]["sequence"].as_u64().unwrap() as u32,
            last_ledger_sequence: v["payment"]["last_ledger_sequence"].as_u64().unwrap() as u32,
            destination_tag: v["payment"]["destination_tag"].as_u64().map(|t| t as u32),
            signing_pub_key,
        };
        (p, v)
    }

    #[test]
    fn unsigned_blob_matches_xrpl() {
        let (p, v) = kat_payment();
        let got = hex::encode_upper(serialize_unsigned(&p));
        assert_eq!(got, v["unsigned_blob_hex"].as_str().unwrap().to_uppercase());
    }

    #[test]
    fn signed_blob_matches_xrpl() {
        let (p, v) = kat_payment();
        let sig = hex::decode(v["txn_signature_hex"].as_str().unwrap()).unwrap();
        let got = hex::encode_upper(serialize_signed(&p, &sig));
        assert_eq!(got, v["signed_blob_hex"].as_str().unwrap().to_uppercase());
    }

    // Guide §5: the happy-path vector uses tag 12345. Cover the cases it does not:
    // absent tag, and a `Some(0)` tag (which must still emit the field, distinct
    // from absent). A dropped or mis-positioned tag yields a different blob and an
    // invalid signature — these lock the boundary.

    #[test]
    fn destination_tag_absent_omits_field() {
        let (with_tag, _) = kat_payment(); // KAT carries Some(12345)
        let mut p = with_tag.clone();
        p.destination_tag = None;
        let absent = serialize_unsigned(&p);
        // The absent-tag blob is shorter by exactly the 1-byte field id + 4-byte
        // value, and at offset 8 (after TransactionType + Sequence) the next field
        // is LastLedgerSequence (0x20 0x1B), NOT DestinationTag (0x2E).
        assert_eq!(
            serialize_unsigned(&with_tag).len(),
            absent.len() + 5,
            "absent tag must drop 5 bytes (0x2E + u32)"
        );
        assert_eq!(&absent[8..10], &[0x20, 0x1B], "no 0x2E when tag is absent");
    }

    #[test]
    fn destination_tag_zero_is_distinct_from_absent() {
        let (mut p, _) = kat_payment();
        p.destination_tag = Some(0);
        let with_zero = serialize_unsigned(&p);
        p.destination_tag = None;
        let absent = serialize_unsigned(&p);
        assert_ne!(with_zero, absent, "Some(0) must NOT serialize like None");
        assert_eq!(
            with_zero.len(),
            absent.len() + 5,
            "Some(0) emits 0x2E + 00000000"
        );
        // The DestinationTag sits right after Sequence: TransactionType(3) +
        // Sequence(5) = offset 8, then 0x2E and four zero bytes.
        assert_eq!(&with_zero[8..13], &[0x2E, 0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn last_ledger_sequence_uses_two_byte_field_id() {
        let (mut p, _) = kat_payment();
        p.destination_tag = None;
        let blob = serialize_unsigned(&p);
        // After TransactionType(3) + Sequence(5) = offset 8, the next field with no
        // DestinationTag is LastLedgerSequence with the two-byte id 0x20 0x1B.
        assert_eq!(&blob[8..10], &[0x20, 0x1B]);
    }
}
