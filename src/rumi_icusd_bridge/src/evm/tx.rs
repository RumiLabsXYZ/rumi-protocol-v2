//! Owned by card BB-04. EIP-1559 (type 0x02) encoding, signing hash, signature
//! finalization and tx hash.
//!
//! ```text
//! signing hash = keccak256(0x02 || rlp([chain_id, nonce, max_priority_fee_per_gas,
//!                                       max_fee_per_gas, gas_limit, to, value, data,
//!                                       access_list]))
//! signed tx    = 0x02 || rlp([... same nine fields ..., y_parity, r, s])
//! tx hash      = keccak256(signed tx)
//! ```
//!
//! Integers are minimal big-endian (zero is the empty string, `0x80`). `to` is always
//! exactly 20 bytes (this canister never creates contracts). `r` and `s` are minimal
//! integers too, so leading zero bytes are stripped: encoding them as fixed 32-byte
//! strings is the classic RLP mistake that makes a node reject roughly 1 in 128
//! signatures. The access list is always empty.

use alloy_rlp::{Encodable, Header};
use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
use sha3::{Digest, Keccak256};

const TX_TYPE_EIP1559: u8 = 0x02;
/// RLP encoding of an empty list (the empty access list).
const EMPTY_LIST: u8 = 0xc0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Eip1559Tx {
    pub chain_id: u64,
    pub nonce: u64,
    pub max_priority_fee_per_gas: u128,
    pub max_fee_per_gas: u128,
    pub gas_limit: u64,
    pub to: [u8; 20],
    pub value: u128,
    pub data: Vec<u8>,
}

fn keccak(data: &[u8]) -> [u8; 32] {
    Keccak256::digest(data).into()
}

/// RLP payload (without list header) of the nine unsigned fields.
fn unsigned_payload(tx: &Eip1559Tx) -> Vec<u8> {
    let mut p = Vec::with_capacity(64 + tx.data.len());
    tx.chain_id.encode(&mut p);
    tx.nonce.encode(&mut p);
    tx.max_priority_fee_per_gas.encode(&mut p);
    tx.max_fee_per_gas.encode(&mut p);
    tx.gas_limit.encode(&mut p);
    // `to` is a 20-byte string (not an integer, so no minimization).
    tx.to.encode(&mut p);
    tx.value.encode(&mut p);
    tx.data.as_slice().encode(&mut p);
    p.push(EMPTY_LIST);
    p
}

/// `0x02 || rlp_list(payload)`.
fn wrap_typed(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + 9 + payload.len());
    out.push(TX_TYPE_EIP1559);
    Header { list: true, payload_length: payload.len() }.encode(&mut out);
    out.extend_from_slice(payload);
    out
}

/// A 32-byte big-endian scalar as a minimal RLP integer (leading zeros stripped).
fn encode_scalar(word: &[u8; 32], out: &mut Vec<u8>) {
    let first = word.iter().position(|&b| b != 0).unwrap_or(32);
    word[first..].encode(out);
}

/// The 32-byte digest that is signed: `keccak256(0x02 || rlp(unsigned fields))`.
pub fn signing_hash(tx: &Eip1559Tx) -> [u8; 32] {
    keccak(&wrap_typed(&unsigned_payload(tx)))
}

/// The raw signed transaction `0x02 || rlp([..., y_parity, r, s])`.
pub fn encode_signed(tx: &Eip1559Tx, y_parity: bool, r: &[u8; 32], s: &[u8; 32]) -> Vec<u8> {
    let mut p = unsigned_payload(tx);
    u8::from(y_parity).encode(&mut p);
    encode_scalar(r, &mut p);
    encode_scalar(s, &mut p);
    wrap_typed(&p)
}

/// The transaction hash a node assigns to `raw`: `keccak256(raw)`.
pub fn tx_hash(raw: &[u8]) -> [u8; 32] {
    keccak(raw)
}

/// Normalize to low-s (EIP-2) and recover the y-parity matching `pubkey_sec1`.
///
/// `sig64` is `r || s` as returned by `sign_with_ecdsa`. `pubkey_sec1` may be compressed
/// or uncompressed. Returns `(y_parity, r, s)` of the NORMALIZED signature. The parity
/// is found by trying both recovery ids and keeping the one whose recovered key equals
/// `pubkey_sec1`; if neither does (wrong key, wrong digest or corrupt signature) this is
/// an error and nothing is returned.
pub fn finalize_signature(digest: &[u8; 32], sig64: &[u8; 64], pubkey_sec1: &[u8]) -> Result<(bool, [u8; 32], [u8; 32]), String> {
    let expected = VerifyingKey::from_sec1_bytes(pubkey_sec1).map_err(|e| format!("invalid public key: {e}"))?;
    let sig = Signature::from_slice(sig64).map_err(|e| format!("invalid signature scalars: {e}"))?;
    // `normalize_s` returns `Some` only when `s` was in the upper half.
    let sig = sig.normalize_s().unwrap_or(sig);
    for y_odd in [false, true] {
        let recid = RecoveryId::new(y_odd, false);
        let Ok(recovered) = VerifyingKey::recover_from_prehash(digest, &sig, recid) else {
            continue;
        };
        if recovered == expected {
            let bytes = sig.to_bytes();
            let mut r = [0u8; 32];
            let mut s = [0u8; 32];
            r.copy_from_slice(&bytes[..32]);
            s.copy_from_slice(&bytes[32..]);
            return Ok((y_odd, r, s));
        }
    }
    Err("signature does not recover to the expected public key".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use k256::ecdsa::SigningKey;

    const KEY_HEX: &str = "4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318";
    /// Address of `KEY_HEX` (cast wallet address --private-key ...).
    const KEY_ADDRESS: &str = "2c7536e3605d9c16a7a3d7b1898e529396a65c23";

    fn key() -> SigningKey {
        SigningKey::from_slice(&hex::decode(KEY_HEX).unwrap()).unwrap()
    }

    fn pubkey_uncompressed(k: &SigningKey) -> Vec<u8> {
        k.verifying_key().to_encoded_point(false).as_bytes().to_vec()
    }

    fn pubkey_compressed(k: &SigningKey) -> Vec<u8> {
        k.verifying_key().to_encoded_point(true).as_bytes().to_vec()
    }

    fn address_of(vk: &VerifyingKey) -> String {
        let pt = vk.to_encoded_point(false);
        hex::encode(&keccak(&pt.as_bytes()[1..])[12..])
    }

    /// Sign `digest` with `k` and return `r || s` (low-s, as k256 produces).
    fn sign64(k: &SigningKey, digest: &[u8; 32]) -> [u8; 64] {
        use k256::ecdsa::signature::hazmat::PrehashSigner;
        let sig: Signature = k.sign_prehash(digest).unwrap();
        let mut out = [0u8; 64];
        out.copy_from_slice(&sig.to_bytes());
        out
    }

    /// Full pipeline used by the golden vectors.
    fn sign_tx_raw(tx: &Eip1559Tx) -> Vec<u8> {
        let k = key();
        let digest = signing_hash(tx);
        let (y, r, s) = finalize_signature(&digest, &sign64(&k, &digest), &pubkey_compressed(&k)).unwrap();
        encode_signed(tx, y, &r, &s)
    }

    /// A representative Base mint: nonce 7, 0.1 gwei priority, 1 gwei max fee.
    fn mint_tx() -> Eip1559Tx {
        Eip1559Tx {
            chain_id: 8453,
            nonce: 7,
            max_priority_fee_per_gas: 100_000_000,
            max_fee_per_gas: 1_000_000_000,
            gas_limit: 150_000,
            to: [0xc0; 20],
            value: 0,
            data: hex::decode("1e458bee000000000000000000000000000000000000000000000000000000000000cafe000000000000000000000000000000000000000000000000000000009502f9001111111111111111111111111111111111111111111111111111111111111111").unwrap(),
        }
    }

    /// Edge case: zero nonce, zero value, empty data, minimal fees.
    fn zero_tx() -> Eip1559Tx {
        Eip1559Tx {
            chain_id: 84532,
            nonce: 0,
            max_priority_fee_per_gas: 0,
            max_fee_per_gas: 1,
            gas_limit: 21_000,
            to: [0x11; 20],
            value: 0,
            data: vec![],
        }
    }

    /// A mint-shaped tx whose real signature has `r[0] == 0` (found by scanning nonces
    /// from 0), the case that breaks encoders which pad `r` to 32 bytes.
    fn leading_zero_r_tx() -> Eip1559Tx {
        let k = key();
        let pk = pubkey_compressed(&k);
        for nonce in 0..20_000u64 {
            let mut tx = mint_tx();
            tx.nonce = nonce;
            let digest = signing_hash(&tx);
            let (_, r, _) = finalize_signature(&digest, &sign64(&k, &digest), &pk).unwrap();
            if r[0] == 0 {
                return tx;
            }
        }
        panic!("no leading-zero r within 20k nonces");
    }

    // Golden vectors. Each raw transaction is deterministic (RFC 6979) and was decoded
    // with `cast decode-transaction <raw>`; the decoded fields, the recovered signer
    // (KEY_ADDRESS) and the `hash` field are asserted below.
    const GOLDEN_MINT_RAW: &str = "02f8d2822105078405f5e100843b9aca00830249f094c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c080b8641e458bee000000000000000000000000000000000000000000000000000000000000cafe000000000000000000000000000000000000000000000000000000009502f9001111111111111111111111111111111111111111111111111111111111111111c080a0f914cd4b7c215ca0ddd15a9abe9455b469f68ae0d897311a38a2fac2a83886fea020f49a8ebf4c794f1ce8a0899cb46140478ccd88011e01b20d4ab5b2a7001422";
    const GOLDEN_MINT_HASH: &str = "6832f1dae68e2d08487e9bea89a02ba156785da05b1f323464d438ea3d9ee56d";
    const GOLDEN_ZERO_RAW: &str = "02f86583014a348080018252089411111111111111111111111111111111111111118080c001a02c686fd8cb6710b355ed7bafdc0d5fa5e261f5060703add28a5becce64497824a0262d7d8d1b2326cf4da530736ef2e271c217aea2a1e844af55bf16e5b48d0763";
    const GOLDEN_ZERO_HASH: &str = "6bb0f17c6ed2565a535d784e6fea31322c8d63573d85018618e1503e8e5366f0";
    const GOLDEN_LEADING_ZERO_R_RAW: &str = "02f8d282210581c18405f5e100843b9aca00830249f094c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c080b8641e458bee000000000000000000000000000000000000000000000000000000000000cafe000000000000000000000000000000000000000000000000000000009502f9001111111111111111111111111111111111111111111111111111111111111111c0809f2a2354e6b83958cbd582fcdc2d25def7346ab696c028dc3ae4279c46e3c9d1a03e9dcb034ecd4f5addafb3d06bc0faadf4529d9bd12ca8e1528b06e9301279ae";
    const GOLDEN_LEADING_ZERO_R_HASH: &str = "47b8d5faaa63137b07e7e91426a92795874e038b1e2b59fb7e1399d12541a1b3";

    #[test]
    fn print_vectors_for_cast() {
        if std::env::var("PRINT_TX_VECTORS").is_ok() {
            eprintln!("MINT_RAW={}", hex::encode(sign_tx_raw(&mint_tx())));
            eprintln!("ZERO_RAW={}", hex::encode(sign_tx_raw(&zero_tx())));
            let lz = leading_zero_r_tx();
            eprintln!("LZ_NONCE={}", lz.nonce);
            eprintln!("LZ_RAW={}", hex::encode(sign_tx_raw(&lz)));
        }
    }

    #[test]
    fn key_address_is_the_documented_one() {
        assert_eq!(address_of(key().verifying_key()), KEY_ADDRESS);
    }

    #[test]
    fn tx_hash_is_keccak_of_raw() {
        let raw = sign_tx_raw(&mint_tx());
        assert_eq!(tx_hash(&raw), keccak(&raw));
        assert_ne!(tx_hash(&raw), signing_hash(&mint_tx()));
        // Known keccak of the empty string.
        assert_eq!(
            hex::encode(tx_hash(&[])),
            "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
        );
    }

    #[test]
    fn signing_hash_binds_every_field() {
        let base = mint_tx();
        let h0 = signing_hash(&base);
        let variants: Vec<Eip1559Tx> = vec![
            Eip1559Tx { chain_id: 84532, ..base.clone() },
            Eip1559Tx { nonce: 8, ..base.clone() },
            Eip1559Tx { max_priority_fee_per_gas: base.max_priority_fee_per_gas + 1, ..base.clone() },
            Eip1559Tx { max_fee_per_gas: base.max_fee_per_gas + 1, ..base.clone() },
            Eip1559Tx { gas_limit: base.gas_limit + 1, ..base.clone() },
            Eip1559Tx { to: [0xc1; 20], ..base.clone() },
            Eip1559Tx { value: 1, ..base.clone() },
            Eip1559Tx { data: vec![], ..base.clone() },
        ];
        for (i, v) in variants.iter().enumerate() {
            assert_ne!(signing_hash(v), h0, "field {i} must change the signing hash");
        }
    }

    #[test]
    fn signed_encoding_is_type2_prefixed_and_extends_unsigned_list() {
        let tx = mint_tx();
        let raw = sign_tx_raw(&tx);
        assert_eq!(raw[0], 0x02);
        let mut buf = &raw[1..];
        let h = Header::decode(&mut buf).unwrap();
        assert!(h.list);
        assert_eq!(h.payload_length, buf.len(), "list header covers the whole body");
        // The signed list body is the unsigned payload followed by (y_parity, r, s).
        assert!(buf.starts_with(&unsigned_payload(&tx)));
        assert!(buf.len() > unsigned_payload(&tx).len());
    }

    #[test]
    fn signed_fields_round_trip_through_an_independent_rlp_decoder() {
        use alloy_rlp::Decodable;
        for tx in [mint_tx(), zero_tx()] {
            let raw = sign_tx_raw(&tx);
            let mut buf = &raw[1..];
            let h = Header::decode(&mut buf).unwrap();
            assert!(h.list);
            assert_eq!(h.payload_length, buf.len());
            assert_eq!(u64::decode(&mut buf).unwrap(), tx.chain_id);
            assert_eq!(u64::decode(&mut buf).unwrap(), tx.nonce);
            assert_eq!(u128::decode(&mut buf).unwrap(), tx.max_priority_fee_per_gas);
            assert_eq!(u128::decode(&mut buf).unwrap(), tx.max_fee_per_gas);
            assert_eq!(u64::decode(&mut buf).unwrap(), tx.gas_limit);
            assert_eq!(<[u8; 20]>::decode(&mut buf).unwrap(), tx.to);
            assert_eq!(u128::decode(&mut buf).unwrap(), tx.value);
            assert_eq!(Header::decode_bytes(&mut buf, false).unwrap(), tx.data.as_slice());
            // Empty access list.
            let al = Header::decode(&mut buf).unwrap();
            assert!(al.list);
            assert_eq!(al.payload_length, 0);
            let y = u8::decode(&mut buf).unwrap();
            assert!(y <= 1);
            let r = Header::decode_bytes(&mut buf, false).unwrap();
            let s = Header::decode_bytes(&mut buf, false).unwrap();
            assert!(buf.is_empty(), "no trailing bytes");
            assert!(!r.is_empty() && r[0] != 0, "r is a minimal integer");
            assert!(!s.is_empty() && s[0] != 0, "s is a minimal integer");
        }
    }

    #[test]
    fn zero_value_zero_nonce_and_empty_data_use_the_empty_string() {
        let tx = zero_tx();
        let payload = unsigned_payload(&tx);
        // chain_id 84532 = 0x014a34 -> 83 01 4a 34; nonce 0 -> 80; priority 0 -> 80;
        // max fee 1 -> 01; gas 21000 = 0x5208 -> 82 52 08; to -> 94 + 20 bytes;
        // value 0 -> 80; data [] -> 80; access list -> c0.
        let mut expected = vec![0x83, 0x01, 0x4a, 0x34, 0x80, 0x80, 0x01, 0x82, 0x52, 0x08, 0x94];
        expected.extend_from_slice(&[0x11; 20]);
        expected.extend_from_slice(&[0x80, 0x80, 0xc0]);
        assert_eq!(payload, expected);
    }

    #[test]
    fn single_low_byte_data_and_boundary_integers_encode_canonically() {
        // 0x7f encodes as itself, 0x80 gets a length prefix (RLP single-byte rule).
        let mut tx = zero_tx();
        tx.data = vec![0x7f];
        assert!(unsigned_payload(&tx).ends_with(&[0x80, 0x7f, 0xc0]));
        tx.data = vec![0x80];
        assert!(unsigned_payload(&tx).ends_with(&[0x80, 0x81, 0x80, 0xc0]));
        tx.data = vec![0u8; 56];
        // 56-byte string needs the long form: b8 38 ...
        let p = unsigned_payload(&tx);
        let tail = &p[p.len() - 1 - 56 - 2..];
        assert_eq!(&tail[..2], &[0xb8, 0x38]);
        // u128 maximum fee.
        let mut tx = zero_tx();
        tx.max_fee_per_gas = u128::MAX;
        let p = unsigned_payload(&tx);
        assert!(p.windows(17).any(|w| w[0] == 0x90 && w[1..] == [0xff; 16]));
    }

    #[test]
    fn encode_signed_strips_leading_zeros_of_r_and_s() {
        let tx = zero_tx();
        let mut r = [0u8; 32];
        let mut s = [0u8; 32];
        r[31] = 0x05; // single byte < 0x80 encodes as itself
        s[30] = 0x01;
        s[31] = 0x00; // 0x0100 -> 82 01 00
        let raw = encode_signed(&tx, false, &r, &s);
        // ... 80 (y_parity 0) 05 (r) 82 01 00 (s)
        assert!(raw.ends_with(&[0x80, 0x05, 0x82, 0x01, 0x00]));
        // r with 31 leading zeros and value 0x80 needs a length prefix.
        let mut r = [0u8; 32];
        r[31] = 0x80;
        let raw = encode_signed(&tx, true, &r, &s);
        assert!(raw.ends_with(&[0x01, 0x81, 0x80, 0x82, 0x01, 0x00]));
        // Full-width scalars use a 0xa0 prefix.
        let r = [0xff; 32];
        let s = [0x7f; 32];
        let raw = encode_signed(&tx, true, &r, &s);
        let mut tail = vec![0x01, 0xa0];
        tail.extend_from_slice(&r);
        tail.push(0xa0);
        tail.extend_from_slice(&s);
        assert!(raw.ends_with(&tail));
    }

    #[test]
    fn real_signature_with_a_leading_zero_r_is_encoded_minimally() {
        let k = key();
        let tx = leading_zero_r_tx();
        let digest = signing_hash(&tx);
        let (y, r, s) = finalize_signature(&digest, &sign64(&k, &digest), &pubkey_compressed(&k)).unwrap();
        assert_eq!(r[0], 0);
        let raw = encode_signed(&tx, y, &r, &s);
        let first = r.iter().position(|&b| b != 0).unwrap();
        let mut r_enc = vec![0x80 + (32 - first) as u8];
        r_enc.extend_from_slice(&r[first..]);
        assert!(raw.windows(r_enc.len()).any(|w| w == r_enc.as_slice()), "minimal r present");
        let padded: Vec<u8> = [&[0xa0u8][..], &r[..]].concat();
        assert!(!raw.windows(padded.len()).any(|w| w == padded.as_slice()), "r must not be padded to 32 bytes");
    }

    #[test]
    fn finalize_recovers_the_signer_and_verifies_against_the_digest() {
        let k = key();
        let digest = signing_hash(&mint_tx());
        let sig64 = sign64(&k, &digest);
        for pk in [pubkey_compressed(&k), pubkey_uncompressed(&k)] {
            let (y, r, s) = finalize_signature(&digest, &sig64, &pk).unwrap();
            let sig = Signature::from_scalars(r, s).unwrap();
            let recovered = VerifyingKey::recover_from_prehash(&digest, &sig, RecoveryId::new(y, false)).unwrap();
            assert_eq!(&recovered, k.verifying_key());
            assert_eq!(address_of(&recovered), KEY_ADDRESS);
            // Output is low-s.
            assert!(sig.normalize_s().is_none(), "s must already be low");
        }
    }

    #[test]
    fn high_s_input_is_normalized_and_still_verifies() {
        let k = key();
        let digest = signing_hash(&mint_tx());
        let low = Signature::from_slice(&sign64(&k, &digest)).unwrap();
        assert!(low.normalize_s().is_none(), "k256 emits low-s");
        // Build the high-s twin (r, n - s).
        let s_scalar = *low.s();
        let high_s = -s_scalar;
        let high = Signature::from_scalars(low.r().to_bytes(), high_s.to_bytes()).unwrap();
        assert!(high.normalize_s().is_some(), "twin is high-s");
        let mut high64 = [0u8; 64];
        high64.copy_from_slice(&high.to_bytes());

        let pk = pubkey_compressed(&k);
        let (y_low, r_low, s_low) = finalize_signature(&digest, &sign64(&k, &digest), &pk).unwrap();
        let (y_high, r_high, s_high) = finalize_signature(&digest, &high64, &pk).unwrap();
        // Both inputs finalize to the identical, low-s signature.
        assert_eq!((y_low, r_low, s_low), (y_high, r_high, s_high));
        let sig = Signature::from_scalars(r_high, s_high).unwrap();
        assert!(sig.normalize_s().is_none());
        let rec = VerifyingKey::recover_from_prehash(&digest, &sig, RecoveryId::new(y_high, false)).unwrap();
        assert_eq!(&rec, k.verifying_key());
    }

    #[test]
    fn wrong_key_digest_or_garbage_fails() {
        let k = key();
        let digest = signing_hash(&mint_tx());
        let sig64 = sign64(&k, &digest);
        // Wrong public key.
        let other = SigningKey::from_slice(&[0x42; 32]).unwrap();
        assert!(finalize_signature(&digest, &sig64, &pubkey_compressed(&other)).is_err());
        assert!(finalize_signature(&digest, &sig64, &pubkey_uncompressed(&other)).is_err());
        // Wrong digest.
        let mut bad = digest;
        bad[0] ^= 1;
        assert!(finalize_signature(&bad, &sig64, &pubkey_compressed(&k)).is_err());
        // Zero r/s and an all-ones (>= n) signature are rejected.
        assert!(finalize_signature(&digest, &[0u8; 64], &pubkey_compressed(&k)).is_err());
        assert!(finalize_signature(&digest, &[0xff; 64], &pubkey_compressed(&k)).is_err());
        // Malformed public keys.
        assert!(finalize_signature(&digest, &sig64, &[]).is_err());
        assert!(finalize_signature(&digest, &sig64, &[0x02; 33]).is_err());
        assert!(finalize_signature(&digest, &sig64, &pubkey_compressed(&k)[..32]).is_err());
    }

    #[test]
    fn golden_raw_transactions_match_cast_decoded_vectors() {
        let mint = sign_tx_raw(&mint_tx());
        assert_eq!(hex::encode(&mint), GOLDEN_MINT_RAW);
        assert_eq!(hex::encode(tx_hash(&mint)), GOLDEN_MINT_HASH);

        let zero = sign_tx_raw(&zero_tx());
        assert_eq!(hex::encode(&zero), GOLDEN_ZERO_RAW);
        assert_eq!(hex::encode(tx_hash(&zero)), GOLDEN_ZERO_HASH);

        let lz = sign_tx_raw(&leading_zero_r_tx());
        assert_eq!(hex::encode(&lz), GOLDEN_LEADING_ZERO_R_RAW);
        assert_eq!(hex::encode(tx_hash(&lz)), GOLDEN_LEADING_ZERO_R_HASH);
    }

    #[test]
    fn golden_decoded_fields_match_cast_output() {
        // `cast decode-transaction` output for GOLDEN_MINT_RAW, field by field.
        let raw = hex::decode(GOLDEN_MINT_RAW).unwrap();
        let tx = mint_tx();
        assert_eq!(tx.chain_id, 0x2105);
        assert_eq!(tx.nonce, 0x7);
        assert_eq!(tx.gas_limit, 0x249f0);
        assert_eq!(tx.max_fee_per_gas, 0x3b9aca00);
        assert_eq!(tx.max_priority_fee_per_gas, 0x5f5e100);
        assert_eq!(tx.to, [0xc0; 20]);
        assert_eq!(tx.value, 0);
        // r, s, yParity as printed by cast.
        let (y, r, s) = {
            let k = key();
            let digest = signing_hash(&tx);
            finalize_signature(&digest, &sign64(&k, &digest), &pubkey_compressed(&k)).unwrap()
        };
        assert!(!y, "cast: yParity 0x0");
        assert_eq!(hex::encode(r), "f914cd4b7c215ca0ddd15a9abe9455b469f68ae0d897311a38a2fac2a83886fe");
        assert_eq!(hex::encode(s), "20f49a8ebf4c794f1ce8a0899cb46140478ccd88011e01b20d4ab5b2a7001422");
        assert_eq!(encode_signed(&tx, y, &r, &s), raw);
        // Zero-edge tx: cast chainId 0x14a34, nonce 0, gas 0x5208, maxFee 1, priority 0.
        let z = zero_tx();
        assert_eq!((z.chain_id, z.nonce, z.gas_limit, z.max_fee_per_gas, z.max_priority_fee_per_gas), (0x14a34, 0, 0x5208, 1, 0));
    }

    #[test]
    fn golden_signers_recover_to_the_key_address() {
        for raw_hex in [GOLDEN_MINT_RAW, GOLDEN_ZERO_RAW, GOLDEN_LEADING_ZERO_R_RAW] {
            let raw = hex::decode(raw_hex).unwrap();
            // Independent recovery: rebuild the signing hash from the RLP body (everything
            // but y_parity, r, s) and recover with k256.
            use alloy_rlp::Decodable;
            let mut buf = &raw[1..];
            let h = Header::decode(&mut buf).unwrap();
            assert_eq!(h.payload_length, buf.len());
            let body_start = buf;
            // Skip nine fields.
            let mut cur = buf;
            for _ in 0..8 {
                let hd = Header::decode(&mut cur).unwrap();
                cur = &cur[hd.payload_length..];
            }
            let al = Header::decode(&mut cur).unwrap();
            assert!(al.list);
            let unsigned_len = body_start.len() - cur.len();
            let unsigned = &body_start[..unsigned_len];
            let y = u8::decode(&mut cur).unwrap();
            let r = Header::decode_bytes(&mut cur, false).unwrap().to_vec();
            let s = Header::decode_bytes(&mut cur, false).unwrap().to_vec();
            assert!(cur.is_empty());
            let digest = keccak(&wrap_typed(unsigned));
            let mut r32 = [0u8; 32];
            r32[32 - r.len()..].copy_from_slice(&r);
            let mut s32 = [0u8; 32];
            s32[32 - s.len()..].copy_from_slice(&s);
            let sig = Signature::from_scalars(r32, s32).unwrap();
            assert!(sig.normalize_s().is_none(), "low-s");
            let vk = VerifyingKey::recover_from_prehash(&digest, &sig, RecoveryId::new(y == 1, false)).unwrap();
            assert_eq!(address_of(&vk), KEY_ADDRESS);
        }
    }
}
