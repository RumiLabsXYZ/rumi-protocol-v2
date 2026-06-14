//! Threshold Ed25519 (Schnorr) key derivation + signing for the XRP Ledger.
//!
//! XRPL Ed25519 reuses the SAME threshold Schnorr Ed25519 signer as Solana
//! verbatim: the signer signs the full message with NO prehash, which is exactly
//! what XRPL Ed25519 (PureEdDSA) wants (the SHA-512 happens inside Ed25519). The
//! signing message is `STX\0 ‖ unsigned_blob` (see `sign::signing_message`).
//!
//! ic-cdk 0.12 has no `management_canister::schnorr` module, so the management
//! canister is called directly with the candid structs hand-mirrored below
//! (verified against the management canister .did). This mirrors
//! `chains::solana::ted25519`; the structs are duplicated here on purpose, the
//! same way Solana and Monad each carry their own signing module.
//!
//! XRP gets its OWN derivation path (the chain-id-144 prefix) so its keypair is
//! distinct from Solana's (501) and every other chain's, even though they share
//! the same Ed25519 algorithm and key name.

use crate::chains::config::ChainId;
use candid::{CandidType, Deserialize, Principal};

use super::address::classic_address_from_ed25519_pubkey;
use super::config::xrp_schnorr_key_name;

// ─── Derivation paths (mirror solana::ted25519) ──────────────────────────────

/// Per-user collateral custody address path: `[chain_id LE, principal, nonce LE]`.
/// Each (user, nonce) pair yields a distinct XRPL deposit address.
pub fn custody_derivation_path(chain: ChainId, user: Principal, nonce: u64) -> Vec<Vec<u8>> {
    vec![
        chain.0.to_le_bytes().to_vec(),
        user.as_slice().to_vec(),
        nonce.to_le_bytes().to_vec(),
    ]
}

/// Per-chain settlement (protocol-controlled) address path. The settlement
/// address is the canister-owned XRPL account used for protocol-lane operations.
pub fn settlement_derivation_path(chain: ChainId) -> Vec<Vec<u8>> {
    vec![chain.0.to_le_bytes().to_vec(), b"settlement".to_vec()]
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

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct SignWithSchnorrArgument {
    pub message: Vec<u8>,
    pub derivation_path: Vec<Vec<u8>>,
    pub key_id: SchnorrKeyId,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct SignWithSchnorrResponse {
    pub signature: Vec<u8>,
}

fn key_id() -> SchnorrKeyId {
    SchnorrKeyId {
        algorithm: SchnorrAlgorithm::Ed25519,
        name: xrp_schnorr_key_name(),
    }
}

/// Validate a management-canister Ed25519 pubkey is exactly 32 bytes.
fn require_pubkey32(bytes: &[u8]) -> Result<[u8; 32], String> {
    bytes
        .try_into()
        .map_err(|_| format!("expected 32-byte Ed25519 pubkey, got {}", bytes.len()))
}

/// Async: derive the raw 32-byte Ed25519 pubkey AND its XRPL classic address from
/// the management canister. `schnorr_public_key` is FREE (no cycles attached),
/// exactly like `ecdsa_public_key`.
pub async fn derive_xrp_address(
    derivation_path: Vec<Vec<u8>>,
) -> Result<([u8; 32], String), String> {
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
    let pubkey = require_pubkey32(&res.public_key)?;
    let address = classic_address_from_ed25519_pubkey(&pubkey);
    Ok((pubkey, address))
}

/// Cycles for one threshold-Ed25519 signature. Published cost is ~26B; attach 30B
/// of headroom (unused cycles are refunded). Mirrors Solana's reservation. We
/// sign only on real ops, never on a timer.
const SIGN_WITH_SCHNORR_CYCLES: u128 = 30_000_000_000;

/// Async: sign `message` with threshold Ed25519 at `derivation_path`, returning
/// the 64-byte Ed25519 signature. For XRPL, `message` is `STX\0 ‖ unsigned_blob`
/// (the signer must NOT prehash). The arg's `aux` field (BIP341 taproot tweak
/// only) is optional and omitted for Ed25519.
pub async fn sign_message(
    message: Vec<u8>,
    derivation_path: Vec<Vec<u8>>,
) -> Result<Vec<u8>, String> {
    let arg = SignWithSchnorrArgument {
        message,
        derivation_path,
        key_id: key_id(),
    };
    let (res,): (SignWithSchnorrResponse,) = ic_cdk::api::call::call_with_payment128(
        Principal::management_canister(),
        "sign_with_schnorr",
        (arg,),
        SIGN_WITH_SCHNORR_CYCLES,
    )
    .await
    .map_err(|(code, msg)| format!("{code:?}: {msg}"))?;
    if res.signature.len() != 64 {
        return Err(format!(
            "expected 64-byte Ed25519 signature, got {}",
            res.signature.len()
        ));
    }
    Ok(res.signature)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_distinct_per_role_and_chain() {
        let user = Principal::from_slice(&[0xab; 16]);
        let custody = custody_derivation_path(super::super::config::XRP_CHAIN_ID, user, 0);
        let settlement = settlement_derivation_path(super::super::config::XRP_CHAIN_ID);
        assert_ne!(custody, settlement);
        // Nonce changes the path (a fresh deposit address per vault).
        assert_ne!(
            custody,
            custody_derivation_path(super::super::config::XRP_CHAIN_ID, user, 1)
        );
        // The chain-id prefix keeps XRP paths distinct from Solana's.
        let sol_settlement =
            settlement_derivation_path(crate::chains::solana::config::SOLANA_CHAIN_ID);
        assert_ne!(settlement, sol_settlement);
    }

    #[test]
    fn require_pubkey32_rejects_wrong_length() {
        assert!(require_pubkey32(&[0u8; 31]).is_err());
        assert!(require_pubkey32(&[0u8; 33]).is_err());
        assert!(require_pubkey32(&[0u8; 32]).is_ok());
    }
}
