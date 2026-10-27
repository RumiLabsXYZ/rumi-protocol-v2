//! Minimal XDR encoder for a single classic native-XLM Stellar payment.
//!
//! This intentionally supports no muxed accounts, memos, assets other than
//! native XLM, extra operations, fee bumps, Soroban fields, or key handling.

use sha2::{Digest, Sha256};

/// Stellar network domains used when creating transaction signing payloads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Network {
    /// `Public Global Stellar Network ; September 2015`.
    Public,
    /// `Test SDF Network ; September 2015`.
    Test,
}

impl Network {
    pub(crate) fn passphrase(self) -> &'static str {
        match self {
            Self::Public => "Public Global Stellar Network ; September 2015",
            Self::Test => "Test SDF Network ; September 2015",
        }
    }

    /// Return Stellar's network ID, SHA-256 of the canonical passphrase.
    pub fn network_id(self) -> [u8; 32] {
        Sha256::digest(self.passphrase().as_bytes()).into()
    }
}

/// Single-payment transaction fields. Amounts and fees are in stroops.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Payment {
    pub source: [u8; 32],
    pub destination: [u8; 32],
    pub amount_stroops: u64,
    pub sequence: i64,
    pub fee: u32,
    pub min_time: u64,
    pub max_time: u64,
}

/// Encoded transaction material for an externally authorized signer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SigningMaterial {
    /// SHA-256(TransactionSignaturePayload), the digest to sign.
    pub payload_hash: [u8; 32],
    /// Canonical XDR TransactionEnvelope containing one decorated signature.
    pub envelope_xdr: Vec<u8>,
}

/// Errors caused by inputs that cannot form the requested payment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodecError {
    ZeroAmount,
    AmountOutOfRange,
    InvalidSequence,
    ZeroFee,
    InvalidTimeBounds,
    SigningKeyDoesNotMatchSource,
}

impl core::fmt::Display for CodecError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::ZeroAmount => "payment amount must be positive",
            Self::AmountOutOfRange => "payment amount exceeds Stellar int64 range",
            Self::InvalidSequence => "sequence must be positive",
            Self::ZeroFee => "fee must be positive",
            Self::InvalidTimeBounds => "time bounds must satisfy min_time < max_time",
            Self::SigningKeyDoesNotMatchSource => "signing public key does not match source",
        })
    }
}

impl std::error::Error for CodecError {}

/// Hash the canonical Stellar transaction-signature payload for `payment`.
pub fn signing_payload_hash(network: Network, payment: Payment) -> Result<[u8; 32], CodecError> {
    validate(payment)?;
    let mut payload = Vec::with_capacity(32 + 4 + 4 + 32 + 4 * 14);
    payload.extend_from_slice(&network.network_id());
    put_i32(&mut payload, 2); // ENVELOPE_TYPE_TX
    encode_transaction(&mut payload, payment);
    Ok(Sha256::digest(payload).into())
}

/// Encode a signed envelope from a 64-byte externally produced Ed25519
/// signature and its 32-byte public key. No signature validation is performed.
pub fn encode_signed_envelope(
    network: Network,
    payment: Payment,
    signature: &[u8; 64],
    signing_public_key: &[u8; 32],
) -> Result<SigningMaterial, CodecError> {
    validate(payment)?;
    if signing_public_key != &payment.source {
        return Err(CodecError::SigningKeyDoesNotMatchSource);
    }
    let payload_hash = signing_payload_hash(network, payment)?;
    let mut out = Vec::with_capacity(200);
    put_i32(&mut out, 2); // ENVELOPE_TYPE_TX
    encode_transaction(&mut out, payment);
    put_u32(&mut out, 1); // signatures vector length
    out.extend_from_slice(&signing_public_key[28..32]); // SignatureHint
    put_opaque(&mut out, signature); // Signature
    Ok(SigningMaterial {
        payload_hash,
        envelope_xdr: out,
    })
}

fn validate(payment: Payment) -> Result<(), CodecError> {
    if payment.amount_stroops == 0 {
        return Err(CodecError::ZeroAmount);
    }
    if payment.amount_stroops > i64::MAX as u64 {
        return Err(CodecError::AmountOutOfRange);
    }
    if payment.sequence <= 0 {
        return Err(CodecError::InvalidSequence);
    }
    if payment.fee == 0 {
        return Err(CodecError::ZeroFee);
    }
    if payment.min_time >= payment.max_time {
        return Err(CodecError::InvalidTimeBounds);
    }
    Ok(())
}

fn encode_transaction(out: &mut Vec<u8>, payment: Payment) {
    // Transaction.sourceAccount: MuxedAccount(KEY_TYPE_ED25519, key).
    put_i32(out, 0);
    out.extend_from_slice(&payment.source);
    put_u32(out, payment.fee);
    put_i64(out, payment.sequence);
    // Preconditions PRECOND_TIME, followed by TimeBounds.
    put_i32(out, 1);
    put_u64(out, payment.min_time);
    put_u64(out, payment.max_time);
    put_i32(out, 0); // Memo MEMO_NONE
    put_u32(out, 1); // exactly one operation
    put_bool(out, false); // Operation.sourceAccount is absent
    put_i32(out, 1); // OperationType PAYMENT
    put_i32(out, 0); // PaymentOp.destination: classic Ed25519 account
    out.extend_from_slice(&payment.destination);
    put_i32(out, 0); // AssetType ASSET_TYPE_NATIVE
    put_i64(out, payment.amount_stroops as i64);
    put_i32(out, 0); // Transaction.ext v0
}

fn put_bool(out: &mut Vec<u8>, value: bool) {
    put_i32(out, i32::from(value));
}
fn put_i32(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_be_bytes());
}
fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}
fn put_i64(out: &mut Vec<u8>, value: i64) {
    out.extend_from_slice(&value.to_be_bytes());
}
fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_be_bytes());
}
fn put_opaque(out: &mut Vec<u8>, value: &[u8]) {
    put_u32(out, value.len() as u32);
    out.extend_from_slice(value);
    let padding = (4 - value.len() % 4) % 4;
    out.resize(out.len() + padding, 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payment() -> Payment {
        Payment {
            source: [0x11; 32],
            destination: [0x22; 32],
            amount_stroops: 123_456_789,
            sequence: 7,
            fee: 100,
            min_time: 1_700_000_000,
            max_time: 1_700_000_600,
        }
    }

    #[test]
    fn encodes_classic_native_payment_and_decorated_signature() {
        let p = payment();
        let signature = [0x5a; 64];
        let material = encode_signed_envelope(Network::Public, p, &signature, &p.source).unwrap();
        assert_eq!(material.envelope_xdr.len(), 216);
        assert_eq!(&material.envelope_xdr[..4], &2_i32.to_be_bytes());
        assert_eq!(&material.envelope_xdr[4..8], &0_i32.to_be_bytes());
        assert_eq!(&material.envelope_xdr[8..40], &[0x11; 32]);
        assert_eq!(
            &material.envelope_xdr
                [material.envelope_xdr.len() - 72..material.envelope_xdr.len() - 68],
            &[0x11; 4]
        );
        assert_eq!(
            &material.envelope_xdr
                [material.envelope_xdr.len() - 68..material.envelope_xdr.len() - 64],
            &64_u32.to_be_bytes()
        );
        assert_eq!(
            &material.envelope_xdr[material.envelope_xdr.len() - 64..],
            &signature
        );
        assert_ne!(
            material.payload_hash,
            signing_payload_hash(Network::Test, p).unwrap()
        );
    }

    #[test]
    fn validates_amount_fee_sequence_time_and_signing_identity() {
        let mut p = payment();
        p.amount_stroops = 0;
        assert_eq!(
            signing_payload_hash(Network::Public, p),
            Err(CodecError::ZeroAmount)
        );
        p = payment();
        p.amount_stroops = i64::MAX as u64 + 1;
        assert_eq!(
            signing_payload_hash(Network::Public, p),
            Err(CodecError::AmountOutOfRange)
        );
        p = payment();
        p.sequence = 0;
        assert_eq!(
            signing_payload_hash(Network::Public, p),
            Err(CodecError::InvalidSequence)
        );
        p = payment();
        p.fee = 0;
        assert_eq!(
            signing_payload_hash(Network::Public, p),
            Err(CodecError::ZeroFee)
        );
        p = payment();
        p.max_time = p.min_time;
        assert_eq!(
            signing_payload_hash(Network::Public, p),
            Err(CodecError::InvalidTimeBounds)
        );
        p = payment();
        assert_eq!(
            encode_signed_envelope(Network::Public, p, &[0; 64], &[0; 32]),
            Err(CodecError::SigningKeyDoesNotMatchSource)
        );
    }

    #[test]
    fn xdr_words_are_big_endian_and_signature_is_fixed_width() {
        let p = payment();
        let encoded = encode_signed_envelope(Network::Test, p, &[0; 64], &p.source)
            .unwrap()
            .envelope_xdr;
        assert_eq!(encoded.len() % 4, 0);
        assert_eq!(&encoded[40..44], &100_u32.to_be_bytes());
        assert_eq!(&encoded[44..52], &7_i64.to_be_bytes());
    }

    // Independently generated with @stellar/stellar-sdk 14.2.0 XDR classes:
    // Transaction + TransactionSignaturePayload + TransactionEnvelope. Keep
    // the SDK-generated bytes here so changes to the hand-written codec are
    // checked against a separate implementation, not just our own layout.
    #[test]
    fn matches_stellar_sdk_14_2_0_vector() {
        let p = payment();
        let material = encode_signed_envelope(Network::Public, p, &[0x5a; 64], &p.source).unwrap();
        assert_eq!(
            material.payload_hash,
            [
                0xc0, 0x73, 0xe2, 0xf0, 0xef, 0x52, 0xd5, 0xa9, 0x03, 0x45, 0xb8, 0xf6, 0x58, 0xbe,
                0xa3, 0x62, 0x12, 0x99, 0xff, 0x30, 0x85, 0xed, 0x99, 0x8b, 0xf9, 0x67, 0x5d, 0x3f,
                0x64, 0xae, 0xe5, 0x8d,
            ]
        );
        assert_eq!(
            material.envelope_xdr,
            decode_hex(concat!(
                "00000002000000001111111111111111111111111111111111111111111111111111111111111111",
                "00000064000000000000000700000001000000006553f100000000006553f3580000000000000001",
                "00000000000000010000000022222222222222222222222222222222222222222222222222222222",
                "222222220000000000000000075bcd15000000000000000111111111000000405a5a5a5a5a5a5a5a",
                "5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a",
                "5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a"
            ))
        );
    }

    fn decode_hex(value: &str) -> Vec<u8> {
        assert_eq!(value.len() % 2, 0);
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let digit = |b: u8| match b {
                    b'0'..=b'9' => b - b'0',
                    b'a'..=b'f' => b - b'a' + 10,
                    _ => panic!("invalid fixture hex"),
                };
                digit(pair[0]) * 16 + digit(pair[1])
            })
            .collect()
    }
}
