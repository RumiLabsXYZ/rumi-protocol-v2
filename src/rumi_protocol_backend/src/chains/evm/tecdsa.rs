//! tECDSA address derivation for Monad (secp256k1).
//!
//! Pure helpers (pubkey -> address, derivation paths) are unit-tested against
//! the canonical k=1 vector. The async `ecdsa_public_key` call hits the
//! management canister and is covered by the PocketIC integration test (Task 17)
//! and manual staging (Task 23).

use crate::chains::config::ChainId;
use candid::Principal;
use ic_cdk::api::management_canister::ecdsa::{
    ecdsa_public_key, EcdsaCurve, EcdsaKeyId, EcdsaPublicKeyArgument,
};
use k256::elliptic_curve::sec1::ToEncodedPoint;
use k256::PublicKey;
use sha3::{Digest, Keccak256};

use crate::chains::monad::config::monad_ecdsa_key_name;

/// Derivation path for a per-user collateral custody address.
/// `[chain_id (LE u32), principal bytes, nonce (LE u64)]`.
pub fn custody_derivation_path(chain: ChainId, user: Principal, nonce: u64) -> Vec<Vec<u8>> {
    vec![
        chain.0.to_le_bytes().to_vec(),
        user.as_slice().to_vec(),
        nonce.to_le_bytes().to_vec(),
    ]
}

/// Derivation path for the per-chain settlement (minter) address.
pub fn settlement_derivation_path(chain: ChainId) -> Vec<Vec<u8>> {
    vec![chain.0.to_le_bytes().to_vec(), b"settlement".to_vec()]
}

fn key_id() -> EcdsaKeyId {
    EcdsaKeyId { curve: EcdsaCurve::Secp256k1, name: monad_ecdsa_key_name() }
}

/// Convert a secp256k1 public key (33-byte compressed or 65-byte uncompressed)
/// to a lowercase 0x EVM address: keccak256(uncompressed[1..])[12..].
pub fn evm_address_from_pubkey(pubkey: &[u8]) -> Result<String, String> {
    let pk = PublicKey::from_sec1_bytes(pubkey).map_err(|e| format!("bad pubkey: {e}"))?;
    let uncompressed = pk.to_encoded_point(false); // 0x04 || X(32) || Y(32)
    let bytes = uncompressed.as_bytes();
    if bytes.len() != 65 {
        return Err(format!("expected 65-byte uncompressed pubkey, got {}", bytes.len()));
    }
    let hash = Keccak256::digest(&bytes[1..]); // drop the 0x04 prefix
    let addr = &hash[12..]; // last 20 bytes
    Ok(format!("0x{}", hex::encode(addr)))
}

/// True iff `s` is a well-formed EVM address: a `0x`/`0X` prefix followed by
/// EXACTLY 40 hex digits (20 bytes), case-insensitive. This is the format the
/// tx-building helpers (`tx::abi_word_address`/`parse_address`) require; an
/// address that passes this can never panic those helpers. (Format only — no
/// EIP-55 checksum; derived addresses are lowercase and RPC responses vary.)
pub fn is_valid_evm_address(s: &str) -> bool {
    let hex = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => h,
        None => return false,
    };
    hex.len() == 40 && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Async: fetch the derived public key from the management canister and return
/// both the raw pubkey and the EVM address. Used by deposit-address queries and
/// by the settlement worker to learn its minter address.
pub async fn derive_evm_address(derivation_path: Vec<Vec<u8>>) -> Result<(Vec<u8>, String), String> {
    let arg = EcdsaPublicKeyArgument {
        canister_id: None,
        derivation_path,
        key_id: key_id(),
    };
    let (res,) = ecdsa_public_key(arg).await.map_err(|(code, msg)| format!("{code:?}: {msg}"))?;
    let addr = evm_address_from_pubkey(&res.public_key)?;
    Ok((res.public_key, addr))
}

// ─── Settlement-address cache (Task 11 review M1; wired Task 15) ────────────────

thread_local! {
    static SETTLEMENT_ADDR_CACHE: std::cell::RefCell<std::collections::BTreeMap<ChainId, String>> =
        const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
}

/// Cached settlement (minter) address for `chain`. Derives + caches on first
/// use; returns the cached value thereafter. The address is deterministic
/// (`settlement_derivation_path` has no nonce), so caching is always correct;
/// the cache is a thread_local (not persisted) and simply re-derives once per
/// chain after an upgrade. Returns (path, address) so callers that also need the
/// derivation path for signing get both.
///
/// SETTLEMENT-ONLY: this caches the SETTLEMENT address exclusively (the key is
/// `chain`, and the path is always `settlement_derivation_path(chain)`). It must
/// NEVER be used for a per-vault custody address — those derive from
/// `custody_derivation_path(chain, user, nonce)` and are per-vault, not per-chain.
pub async fn cached_settlement_address(chain: ChainId) -> Result<(Vec<Vec<u8>>, String), String> {
    let path = settlement_derivation_path(chain);
    // Synchronous cache read — the borrow is dropped before any `.await`.
    if let Some(addr) = SETTLEMENT_ADDR_CACHE.with(|c| c.borrow().get(&chain).cloned()) {
        return Ok((path, addr));
    }
    let (_pubkey, addr) = derive_evm_address(path.clone()).await?;
    // Synchronous cache write — borrow dropped before returning, never held
    // across an `.await`.
    SETTLEMENT_ADDR_CACHE.with(|c| {
        c.borrow_mut().insert(chain, addr.clone());
    });
    Ok((path, addr))
}
