//! Threshold Ed25519 (Schnorr) address derivation for Solana.
//!
//! ic-cdk 0.12 has no `management_canister::schnorr` module, so the management
//! canister is called directly with the candid structs hand-mirrored below
//! (verified against the management canister .did). Mirrors
//! `chains::monad::tecdsa` (which uses the built-in `ecdsa` module).
//!
//! `schnorr_public_key` is a FREE call (no cycles attached), exactly like
//! `ecdsa_public_key`. Signing (`sign_with_schnorr`, which DOES cost cycles)
//! lands in M2.

use crate::chains::config::ChainId;
use candid::{CandidType, Deserialize, Principal};

use super::config::solana_schnorr_key_name;

// ─── Derivation paths (mirror tecdsa) ───────────────────────────────────────

/// Per-user collateral custody address path: `[chain_id LE, principal, nonce LE]`.
pub fn custody_derivation_path(chain: ChainId, user: Principal, nonce: u64) -> Vec<Vec<u8>> {
    vec![
        chain.0.to_le_bytes().to_vec(),
        user.as_slice().to_vec(),
        nonce.to_le_bytes().to_vec(),
    ]
}

/// Per-chain settlement (mint-authority) address path.
pub fn settlement_derivation_path(chain: ChainId) -> Vec<Vec<u8>> {
    vec![chain.0.to_le_bytes().to_vec(), b"settlement".to_vec()]
}

// ─── Pure encoding helpers ──────────────────────────────────────────────────

/// A Solana address is the base58 of the 32-byte Ed25519 public key (no hashing).
pub fn solana_address_from_pubkey(pubkey: &[u8]) -> Result<String, String> {
    if pubkey.len() != 32 {
        return Err(format!("expected 32-byte Ed25519 pubkey, got {}", pubkey.len()));
    }
    Ok(bs58::encode(pubkey).into_string())
}

/// True iff `s` base58-decodes to exactly 32 bytes.
pub fn is_valid_solana_address(s: &str) -> bool {
    match bs58::decode(s).into_vec() {
        Ok(bytes) => bytes.len() == 32,
        Err(_) => false,
    }
}

// ─── Management-canister Schnorr candid structs (hand-mirrored) ──────────────
// Source: management canister .did. ic-cdk 0.12 lacks the typed `schnorr`
// module, so we define the minimal surface and call by name.

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum SchnorrAlgorithm {
    #[serde(rename = "ed25519")]
    Ed25519,
    #[serde(rename = "bip340secp256k1")]
    Bip340Secp256k1,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct SchnorrKeyId {
    pub algorithm: SchnorrAlgorithm,
    pub name: String,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct SchnorrPublicKeyArgument {
    pub canister_id: Option<Principal>,
    pub derivation_path: Vec<Vec<u8>>,
    pub key_id: SchnorrKeyId,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct SchnorrPublicKeyResponse {
    pub public_key: Vec<u8>,
    pub chain_code: Vec<u8>,
}

fn key_id() -> SchnorrKeyId {
    SchnorrKeyId { algorithm: SchnorrAlgorithm::Ed25519, name: solana_schnorr_key_name() }
}

/// Async: derive the Ed25519 public key from the management canister and return
/// both the raw 32-byte pubkey and its base58 Solana address. `schnorr_public_key`
/// is free (no cycles attached), like `ecdsa_public_key`.
pub async fn derive_solana_address(
    derivation_path: Vec<Vec<u8>>,
) -> Result<(Vec<u8>, String), String> {
    let arg = SchnorrPublicKeyArgument {
        canister_id: None,
        derivation_path,
        key_id: key_id(),
    };
    let (res,): (SchnorrPublicKeyResponse,) = ic_cdk::api::call::call(
        Principal::management_canister(),
        "schnorr_public_key",
        (arg,),
    )
    .await
    .map_err(|(code, msg)| format!("{code:?}: {msg}"))?;
    let addr = solana_address_from_pubkey(&res.public_key)?;
    Ok((res.public_key, addr))
}
