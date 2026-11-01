//! BLAKE2b-256, the hash Sui uses for addresses, the signing digest and the typed transaction
//! digest. This is the vetted RustCrypto `blake2` crate (0.10, on the `digest` 0.10 that
//! `sha2` already uses); the official Rust SDK declares the same type
//! (`sui-sdk-types` `hash.rs`: `type Blake2b256 = blake2::Blake2b<blake2::digest::consts::U32>`).
//!
//! The vectors in the tests come from Python `hashlib.blake2b(data, digest_size=32)`, computed
//! independently: empty, "abc" and inputs around the 128-byte block boundary.

use blake2::digest::consts::U32;
use blake2::{Blake2b, Digest};

type Blake2b256 = Blake2b<U32>;

/// BLAKE2b with a 32-byte output and no key.
pub fn blake2b256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Blake2b256::new();
    hasher.update(data);
    hasher.finalize().into()
}

/// BLAKE2b-256 over several parts, hashed in order without joining them first.
pub fn blake2b256_parts(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Blake2b256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i % 251) as u8).collect()
    }

    /// Vectors from Python `hashlib.blake2b(data, digest_size=32)`, computed independently.
    #[test]
    fn matches_the_independent_reference_around_the_block_boundary() {
        let cases: [(&[u8], &str); 2] = [
            (b"", "0e5751c026e543b2e8ab2eb06099daa1d1e5df47778f7787faab45cdf12fe3a8"),
            (b"abc", "bddd813c634239723171ef3fee98579b94964e3bb1cb3e427262c8c068d52319"),
        ];
        for (data, want) in cases {
            assert_eq!(hex::encode(blake2b256(data)), want);
        }
        for (n, want) in [
            (127, "f2fe67ff342e21b8f45e8f2e0bcd1d9243245d50ee6c78042e9c491388791c72"),
            (128, "c3582f71ebb2be66fa5dd750f80baae97554f3b015663c8be377cfcb2488c1d1"),
            (129, "f7f3c46ba2564ff4c4c162da1f5b605f9f1c4aa6a20652a9f9a337c1a2f5b9c9"),
            (256, "582f782226018ec33076bd8d1c42413530ac7e1126260ffc0f306ba3befc3f24"),
            (1000, "b372d0608f720c8c3dd41e9c8eecb10143b41abe520b616607e754bf79c08331"),
        ] {
            assert_eq!(hex::encode(blake2b256(&pattern(n))), want, "length {n}");
        }
    }

    #[test]
    fn parts_equal_the_concatenation() {
        let all = pattern(300);
        assert_eq!(blake2b256_parts(&[&all[..7], &all[7..200], &all[200..]]), blake2b256(&all));
    }
}
