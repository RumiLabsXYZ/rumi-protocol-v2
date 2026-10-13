// ICRC-3 block certification: representation-independent hashing,
// hash-tree construction, and certified-data management.
//
// Implements the hashing algorithm from the ICRC-3 standard
// (https://github.com/dfinity/ICRC-1/blob/main/standards/ICRC-3/HASHINGVALUES.md)
// and the IC certified-data interface so that ic-icrc1-index-ng can verify
// the block chain via `icrc3_get_tip_certificate`.

use sha2::{Digest, Sha256};
use crate::icrc3::Icrc3Value;
use num_traits::Signed;

// ─── Representation-independent hash of ICRC-3 Values ───

/// Compute the representation-independent hash of an ICRC-3 Value.
/// This follows the ICRC-3 hashing spec exactly:
///   Blob  → SHA-256(bytes)
///   Text  → SHA-256(utf8_bytes)
///   Nat   → SHA-256(leb128(value))
///   Int   → SHA-256(sleb128(value))
///   Array → SHA-256(hash(e1) || hash(e2) || ...)
///   Map   → sort (hash(k), hash(v)) pairs lexicographically, then SHA-256(concat)
pub fn hash_value(value: &Icrc3Value) -> [u8; 32] {
    match value {
        Icrc3Value::Blob(bytes) => {
            Sha256::digest(bytes).into()
        }
        Icrc3Value::Text(text) => {
            Sha256::digest(text.as_bytes()).into()
        }
        Icrc3Value::Nat(nat) => {
            let bytes = leb128_encode_nat(nat);
            Sha256::digest(&bytes).into()
        }
        Icrc3Value::Int(int) => {
            let bytes = sleb128_encode_int(int);
            Sha256::digest(&bytes).into()
        }
        Icrc3Value::Array(values) => {
            let mut hasher = Sha256::new();
            for v in values {
                hasher.update(hash_value(v));
            }
            hasher.finalize().into()
        }
        Icrc3Value::Map(entries) => {
            // Hash each (key, value) pair
            let mut hpairs: Vec<([u8; 32], [u8; 32])> = entries
                .iter()
                .map(|(k, v)| {
                    let kh = Sha256::digest(k.as_bytes()).into();
                    let vh = hash_value(v);
                    (kh, vh)
                })
                .collect();
            // Sort lexicographically on (key_hash, val_hash)
            hpairs.sort_unstable();
            let mut hasher = Sha256::new();
            for (kh, vh) in &hpairs {
                hasher.update(kh);
                hasher.update(vh);
            }
            hasher.finalize().into()
        }
    }
}

// ─── LEB128 encoding ───

/// Extract canonical unsigned base-128 digits from a little-endian byte
/// magnitude in one pass. Runtime and allocation are linear in input size.
fn unsigned_leb128_digits(bytes: &[u8]) -> Vec<u8> {
    let mut digits = Vec::with_capacity(bytes.len().saturating_mul(8) / 7 + 1);
    let mut accumulator = 0u16;
    let mut bit_count = 0u8;

    for byte in bytes {
        accumulator |= u16::from(*byte) << bit_count;
        bit_count += 8;
        while bit_count >= 7 {
            digits.push((accumulator & 0x7f) as u8);
            accumulator >>= 7;
            bit_count -= 7;
        }
    }
    if bit_count > 0 {
        digits.push((accumulator & 0x7f) as u8);
    }
    while digits.len() > 1 && digits.last() == Some(&0) {
        digits.pop();
    }
    if digits.is_empty() {
        digits.push(0);
    }
    digits
}

/// Encode a Candid Nat as canonical unsigned LEB128 without narrowing it.
fn leb128_encode_nat(nat: &candid::Nat) -> Vec<u8> {
    let mut digits = unsigned_leb128_digits(&nat.0.to_bytes_le());
    let last = digits.len() - 1;
    for digit in &mut digits[..last] {
        *digit |= 0x80;
    }
    digits
}

/// Encode a Candid Int as canonical arbitrary-width signed LEB128.
fn sleb128_encode_int(int: &candid::Int) -> Vec<u8> {
    let negative = int.0.is_negative();
    let (_, magnitude) = int.0.to_bytes_le();
    let mut digits = unsigned_leb128_digits(&magnitude);

    if negative {
        // Negate the base-128 digits in place (radix complement plus one).
        let mut carry = 1u16;
        for digit in &mut digits {
            let value = (0x7f - u16::from(*digit)) + carry;
            *digit = (value & 0x7f) as u8;
            carry = value >> 7;
        }
    }

    let last = digits.len() - 1;
    let sign_bit_set = digits[last] & 0x40 != 0;
    let needs_sign_extension = if negative {
        !sign_bit_set
    } else {
        sign_bit_set
    };
    if needs_sign_extension {
        digits[last] |= 0x80;
        digits.push(if negative { 0x7f } else { 0x00 });
    }
    let continuation_len = digits.len() - 1;
    for digit in &mut digits[..continuation_len] {
        *digit |= 0x80;
    }
    digits
}

// ─── IC Hash Tree ───
//
// Implements the certified-data hash tree format from the IC interface spec.
// Only the subset needed for ICRC-3 tip certification.

enum HashTree {
    Empty,
    Fork(Box<HashTree>, Box<HashTree>),
    Labeled(Vec<u8>, Box<HashTree>),
    Leaf(Vec<u8>),
}

impl HashTree {
    /// Compute the tree digest (root hash) per IC spec.
    fn digest(&self) -> [u8; 32] {
        match self {
            HashTree::Empty => {
                let mut h = Sha256::new();
                h.update(domain_sep("ic-hashtree-empty"));
                h.finalize().into()
            }
            HashTree::Fork(left, right) => {
                let mut h = Sha256::new();
                h.update(domain_sep("ic-hashtree-fork"));
                h.update(left.digest());
                h.update(right.digest());
                h.finalize().into()
            }
            HashTree::Labeled(label, subtree) => {
                let mut h = Sha256::new();
                h.update(domain_sep("ic-hashtree-labeled"));
                h.update(label);
                h.update(subtree.digest());
                h.finalize().into()
            }
            HashTree::Leaf(data) => {
                let mut h = Sha256::new();
                h.update(domain_sep("ic-hashtree-leaf"));
                h.update(data);
                h.finalize().into()
            }
        }
    }

    /// CBOR-encode the tree using ciborium.
    /// Format per IC spec:
    ///   Empty      → [0]
    ///   Fork(l,r)  → [1, encode(l), encode(r)]
    ///   Labeled    → [2, label_bytes, encode(tree)]
    ///   Leaf       → [3, data_bytes]
    ///   Pruned     → [4, hash_bytes]  (not used here)
    fn to_cbor_value(&self) -> ciborium::value::Value {
        use ciborium::value::Value;
        match self {
            HashTree::Empty => {
                Value::Array(vec![Value::Integer(0.into())])
            }
            HashTree::Fork(left, right) => {
                Value::Array(vec![
                    Value::Integer(1.into()),
                    left.to_cbor_value(),
                    right.to_cbor_value(),
                ])
            }
            HashTree::Labeled(label, subtree) => {
                Value::Array(vec![
                    Value::Integer(2.into()),
                    Value::Bytes(label.clone()),
                    subtree.to_cbor_value(),
                ])
            }
            HashTree::Leaf(data) => {
                Value::Array(vec![
                    Value::Integer(3.into()),
                    Value::Bytes(data.clone()),
                ])
            }
        }
    }
}

fn domain_sep(s: &str) -> Vec<u8> {
    let mut buf = Vec::with_capacity(1 + s.len());
    buf.push(s.len() as u8);
    buf.extend_from_slice(s.as_bytes());
    buf
}

// ─── Public API ───

/// Build the hash tree for ICRC-3 tip certification and set certified data.
/// Call this after every block is added.
pub fn set_certified_tip(last_block_index: u64, last_block_hash: &[u8; 32]) {
    let tree = HashTree::Fork(
        Box::new(HashTree::Labeled(
            b"last_block_hash".to_vec(),
            Box::new(HashTree::Leaf(last_block_hash.to_vec())),
        )),
        Box::new(HashTree::Labeled(
            b"last_block_index".to_vec(),
            Box::new(HashTree::Leaf(leb128_encode_u64(last_block_index))),
        )),
    );
    let root = tree.digest();
    ic_cdk::api::set_certified_data(&root);
}

/// Return the ICRC-3 data certificate for the current tip.
/// Must be called from a query context (ic_cdk::api::data_certificate() is Some).
pub fn get_tip_certificate(last_block_index: u64, last_block_hash: &[u8; 32]) -> Option<crate::icrc3::Icrc3DataCertificate> {
    let certificate = ic_cdk::api::data_certificate()?;

    let tree = HashTree::Fork(
        Box::new(HashTree::Labeled(
            b"last_block_hash".to_vec(),
            Box::new(HashTree::Leaf(last_block_hash.to_vec())),
        )),
        Box::new(HashTree::Labeled(
            b"last_block_index".to_vec(),
            Box::new(HashTree::Leaf(leb128_encode_u64(last_block_index))),
        )),
    );

    let cbor_value = tree.to_cbor_value();
    let mut tree_buf = Vec::new();
    ciborium::ser::into_writer(&cbor_value, &mut tree_buf)
        .expect("Failed to CBOR-encode hash tree");

    Some(crate::icrc3::Icrc3DataCertificate {
        certificate,
        hash_tree: tree_buf,
    })
}

fn leb128_encode_u64(mut val: u64) -> Vec<u8> {
    if val == 0 {
        return vec![0];
    }
    let mut buf = Vec::new();
    while val > 0 {
        let byte = (val & 0x7f) as u8;
        val >>= 7;
        if val > 0 {
            buf.push(byte | 0x80);
        } else {
            buf.push(byte);
        }
    }
    buf
}

/// Recompute the full block hash chain from existing blocks.
/// Call in post_upgrade to rebuild `last_block_hash` from persisted blocks.
pub fn recompute_hash_chain(blocks: &[crate::types::Icrc3Block]) -> Option<[u8; 32]> {
    if blocks.is_empty() {
        return None;
    }
    let mut last_hash: Option<[u8; 32]> = None;
    for block in blocks {
        let encoded = crate::icrc3::encode_block_with_phash(block, last_hash.as_ref());
        last_hash = Some(hash_value(&encoded));
    }
    last_hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expected_hash(hex: &str) -> [u8; 32] {
        let mut bytes = [0; 32];
        for (index, slot) in bytes.iter_mut().enumerate() {
            *slot = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).unwrap();
        }
        bytes
    }

    #[test]
    fn leb_hashes_match_published_icrc3_vectors() {
        // DFINITY ICRC-3 HASHINGVALUES.md test vectors.
        assert_eq!(leb128_encode_nat(&candid::Nat::from(42u64)), [0x2a]);
        assert_eq!(sleb128_encode_int(&candid::Int::from(-42i64)), [0x56]);
        assert_eq!(
            hash_value(&Icrc3Value::Int(candid::Int::from(-42i64))),
            expected_hash("de5a6f78116eca62d7fc5ce159d23ae6b889b365a1739ad2cf36f925a140d0cc"),
        );
    }

    #[test]
    fn sleb_encoder_handles_signed_seven_bit_boundaries() {
        let vectors = [
            (-1, vec![0x7f]),
            (-64, vec![0x40]),
            (-65, vec![0xbf, 0x7f]),
            (-128, vec![0x80, 0x7f]),
            (-129, vec![0xff, 0x7e]),
            (63, vec![0x3f]),
            (64, vec![0xc0, 0x00]),
            (127, vec![0xff, 0x00]),
            (128, vec![0x80, 0x01]),
        ];
        for (value, expected) in vectors {
            assert_eq!(sleb128_encode_int(&candid::Int::from(value)), expected);
        }
    }

    #[test]
    fn leb_encoders_preserve_arbitrary_width_nat_and_int_values() {
        let one_nat = candid::Nat::from(1u64).0;
        let nat_2_200 = candid::Nat(one_nat << 200usize);
        let one_int = candid::Int::from(1u64).0;
        let int_2_200 = candid::Int(one_int << 200usize);
        let int_minus_2_200 = candid::Int(-int_2_200.0.clone());

        let mut positive_bytes = vec![0x80; 28];
        positive_bytes.push(0x10);
        let mut negative_bytes = vec![0x80; 28];
        negative_bytes.push(0x70);

        assert_eq!(leb128_encode_nat(&nat_2_200), positive_bytes);
        assert_eq!(sleb128_encode_int(&int_2_200), positive_bytes);
        assert_eq!(sleb128_encode_int(&int_minus_2_200), negative_bytes);
        assert_eq!(
            hash_value(&Icrc3Value::Nat(nat_2_200)),
            expected_hash("1cbb68968241b14574304bfc0517a80258a09a34fec3972716ac9ed542f4abed"),
        );
        assert_eq!(
            hash_value(&Icrc3Value::Int(int_minus_2_200)),
            expected_hash("b5c7df05c9af015289f46d15cc7df00d2932c90fd6b2a8f9143af16d2e988a40"),
        );
    }

    #[test]
    fn leb_encoders_handle_64kib_integer_magnitudes_with_linear_output_size() {
        const MAGNITUDE_BYTES: usize = 64 * 1024;
        let bit_index = MAGNITUDE_BYTES * 8 - 1;
        let nat = candid::Nat(candid::Nat::from(1u64).0 << bit_index);
        let positive = candid::Int(candid::Int::from(1u64).0 << bit_index);
        let negative = candid::Int(-positive.0.clone());
        let expected_digits = (bit_index + 1 + 6) / 7;

        assert_eq!(leb128_encode_nat(&nat).len(), expected_digits);
        assert_eq!(sleb128_encode_int(&positive).len(), expected_digits);
        assert_eq!(sleb128_encode_int(&negative).len(), expected_digits);
    }

    #[test]
    fn nested_int_hash_matches_independent_composition_vector() {
        let one_nat = candid::Nat::from(1u64).0;
        let nat_2_200 = candid::Nat(one_nat << 200usize);
        let value = Icrc3Value::Array(vec![
            Icrc3Value::Int(candid::Int::from(-42i64)),
            Icrc3Value::Nat(nat_2_200),
        ]);
        assert_eq!(
            hash_value(&value),
            expected_hash("079ec84d9858582abd980b87ae3fc5349b918ee6fd6bd32697abbaa618bce713"),
        );
    }
}
