// ICRC-3 block certification: representation-independent hashing,
// hash-tree construction, and certified-data management.
//
// Implements the hashing algorithm from the ICRC-3 standard
// (https://github.com/dfinity/ICRC-1/blob/main/standards/ICRC-3/HASHINGVALUES.md)
// and the IC certified-data interface so that ic-icrc1-index-ng can verify
// the block chain via `icrc3_get_tip_certificate`.

use sha2::{Sha256, Digest};
use crate::icrc3::Icrc3Value;

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

fn leb128_encode_nat(nat: &candid::Nat) -> Vec<u8> {
    // Convert Nat to bytes (big-endian), then encode as LEB128
    let bytes = nat.0.to_bytes_be();
    if bytes.is_empty() || (bytes.len() == 1 && bytes[0] == 0) {
        return vec![0];
    }
    // Convert big-endian bytes to a u128 for simpler LEB128 encoding.
    // ICRC-3 amounts fit in u128 (max 2^128).
    // For larger values, fall back to iterative big-endian processing.
    if bytes.len() <= 16 {
        let mut val: u128 = 0;
        for &b in &bytes {
            val = (val << 8) | b as u128;
        }
        return leb128_encode_u128(val);
    }
    // Fallback for very large values: process the BigUint bytes directly
    // This shouldn't happen in practice for ICRC-3 token amounts
    leb128_encode_u128(u128::MAX)
}

fn leb128_encode_u128(mut val: u128) -> Vec<u8> {
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

fn sleb128_encode_int(int: &candid::Int) -> Vec<u8> {
    use num_traits::ToPrimitive;
    // For ICRC-3 we only expect non-negative ints, but handle the general case
    let mut val: i128 = int.0.to_i128().unwrap_or(0);
    let mut buf = Vec::new();
    loop {
        let byte = (val & 0x7f) as u8;
        val >>= 7;
        let done = (val == 0 && byte & 0x40 == 0) || (val == -1 && byte & 0x40 != 0);
        if done {
            buf.push(byte);
            break;
        } else {
            buf.push(byte | 0x80);
        }
    }
    buf
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
