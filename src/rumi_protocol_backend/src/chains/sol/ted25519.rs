//! Threshold Ed25519 (Schnorr) derivation + signing for native-SOL-collateral
//! custody (mirrors `chains::xrp::ted25519`; deliberately distinct from the
//! dormant `chains::solana::ted25519`, which serves the SPL-mint chain-vault
//! rail — see `chains::sol::mod` and the collision-avoidance note below).
//!
//! ic-cdk 0.12 has no `management_canister::schnorr` module, so the management
//! canister is called directly. The candid request/response structs are
//! REUSED from `chains::solana::ted25519` (they are already `pub` and are a
//! byte-for-byte mirror of the management-canister `.did`, so redeclaring them
//! here would be pure duplication) rather than re-declared, unlike
//! `chains::xrp::ted25519` (which duplicates them on principle, matching how
//! Solana and Monad each carry their own copy). Only `key_id()` differs
//! (reads `sol_schnorr_key_name()`), so only that is written locally.
//!
//! ## Derivation-path collision avoidance (design doc §1.1)
//! `chains::solana::ted25519::custody_derivation_path` already produces
//! `[SOL_CHAIN_ID_le, user_bytes, vault_id_le]` (THREE elements) for the
//! dormant chain-vault rail, whose `vault_id` comes from a DIFFERENT counter
//! (`multi_chain`) than the CDP vault counter this rail uses. Reusing that
//! path shape here would let a native-SOL-collateral vault and an unrelated
//! dormant-rail chain-vault derive the SAME custody address whenever their
//! (user, id) pair happened to coincide.
//!
//! This module inserts an explicit role label — the same technique XRP uses
//! for its settlement path — making every path here FOUR elements:
//! `[SOL_CHAIN_ID_le, b"collateral", user_bytes, nonce_le]`. A four-element
//! vector can never equal a three-element vector, regardless of the chain id
//! (which is intentionally the same numeric 501 on both rails — see
//! `config::SOL_CHAIN_ID`). `tests_ted25519` asserts this non-collision
//! directly against `chains::solana::ted25519::custody_derivation_path`.

use candid::Principal;

use crate::chains::solana::ted25519::{
    solana_address_from_pubkey, SchnorrAlgorithm, SchnorrKeyId, SchnorrPublicKeyArgument,
    SchnorrPublicKeyResponse, SignWithSchnorrArgument, SignWithSchnorrResponse,
};

use super::config::{sol_schnorr_key_name, SOL_CHAIN_ID};

/// Role-label tag inserted into every path this module derives, so a
/// native-SOL-collateral address can never collide with the dormant
/// `chains::solana` chain-vault rail's addresses even though they share the
/// same chain id (501). See the module doc comment.
const ROLE_TAG: &[u8] = b"collateral";

// ─── Derivation paths ─────────────────────────────────────────────────────

/// Per-vault custody address path: `[SOL_CHAIN_ID le, b"collateral", user,
/// vault_id le]`. Each (user, vault_id) pair yields a distinct Solana deposit
/// address; `vault_id` is the CDP vault counter's reserved id (`derivation_nonce`
/// on `SolPendingDeposit`), not the dormant rail's `multi_chain` counter.
pub fn sol_custody_derivation_path(user: Principal, vault_id: u64) -> Vec<Vec<u8>> {
    vec![
        SOL_CHAIN_ID.0.to_le_bytes().to_vec(),
        ROLE_TAG.to_vec(),
        user.as_slice().to_vec(),
        vault_id.to_le_bytes().to_vec(),
    ]
}

/// Protocol-controlled settlement (fee-payer / nonce-authority) address path:
/// `[SOL_CHAIN_ID le, b"collateral", b"settlement"]`. Three elements, so it
/// can never equal a custody path (always four) or the dormant rail's own
/// settlement path (`[chain le, b"settlement"]`, two elements, no role tag).
pub fn sol_settlement_derivation_path() -> Vec<Vec<u8>> {
    vec![
        SOL_CHAIN_ID.0.to_le_bytes().to_vec(),
        ROLE_TAG.to_vec(),
        b"settlement".to_vec(),
    ]
}

/// Durable-nonce account address path: `[SOL_CHAIN_ID le, b"collateral",
/// b"nonce"]`. A SECOND threshold-Ed25519 path (not a PDA), distinct from the
/// settlement path only by its trailing tag, so the nonce account and the
/// settlement (fee-payer) account are two different on-curve keys — the
/// canister is its own nonce authority via the settlement key, and separately
/// controls the nonce account's own signing key for the one-time bootstrap
/// (`create_account` requires the new account itself to sign).
pub fn sol_nonce_derivation_path() -> Vec<Vec<u8>> {
    vec![
        SOL_CHAIN_ID.0.to_le_bytes().to_vec(),
        ROLE_TAG.to_vec(),
        b"nonce".to_vec(),
    ]
}

fn key_id() -> SchnorrKeyId {
    SchnorrKeyId {
        algorithm: SchnorrAlgorithm::Ed25519,
        name: sol_schnorr_key_name(),
    }
}

/// Validate a management-canister Ed25519 pubkey is exactly 32 bytes.
fn require_pubkey32(bytes: &[u8]) -> Result<[u8; 32], String> {
    bytes
        .try_into()
        .map_err(|_| format!("expected 32-byte Ed25519 pubkey, got {}", bytes.len()))
}

/// Async: derive the raw 32-byte Ed25519 pubkey AND its base58 Solana address
/// from the management canister at `derivation_path`. `schnorr_public_key` is
/// FREE (no cycles attached), exactly like `ecdsa_public_key`.
pub async fn derive_sol_address(
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
    let address = solana_address_from_pubkey(&res.public_key)?;
    Ok((pubkey, address))
}

/// Cycles for one threshold-Ed25519 signature (mirrors XRP/Solana's 30B
/// headroom over the published ~26B cost; unused cycles are refunded).
const SIGN_WITH_SCHNORR_CYCLES: u128 = 30_000_000_000;

/// Async: sign `message` with threshold Ed25519 at `derivation_path`,
/// returning the 64-byte Ed25519 signature. Used both to sign the serialized
/// legacy-message bytes of a claim settlement transfer AND (with the
/// settlement path) to co-sign that same message as fee payer / nonce
/// authority. The signer signs the exact bytes given with NO prehash.
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
    fn paths_are_distinct_per_role() {
        let user = Principal::from_slice(&[0xab; 16]);
        let custody = sol_custody_derivation_path(user, 0);
        let settlement = sol_settlement_derivation_path();
        let nonce = sol_nonce_derivation_path();
        assert_ne!(custody, settlement);
        assert_ne!(custody, nonce);
        assert_ne!(settlement, nonce);
        // Nonce (vault_id) changes the custody path (a fresh deposit address
        // per vault).
        assert_ne!(custody, sol_custody_derivation_path(user, 1));
    }

    /// The single most important test in this module (design doc §1.1): a
    /// native-SOL-collateral custody path must NEVER equal the dormant
    /// `chains::solana` chain-vault rail's custody path for the SAME
    /// (user, id) pair, even though both rails use chain id 501.
    #[test]
    fn custody_path_never_collides_with_dormant_solana_rail() {
        let user = Principal::from_slice(&[0x11; 29]);
        for id in [0u64, 1, 2, 42, u64::MAX] {
            let collateral_path = sol_custody_derivation_path(user, id);
            let dormant_rail_path = crate::chains::solana::ted25519::custody_derivation_path(
                crate::chains::solana::config::SOLANA_CHAIN_ID,
                user,
                id,
            );
            assert_ne!(
                collateral_path, dormant_rail_path,
                "collateral custody path collided with dormant-rail path for id {id}"
            );
            // The reason is structural (4 elements vs 3), not incidental —
            // assert that directly too, so a future refactor that shortens
            // either path shape trips this test immediately.
            assert_eq!(collateral_path.len(), 4);
            assert_eq!(dormant_rail_path.len(), 3);
        }
    }

    #[test]
    fn require_pubkey32_rejects_wrong_length() {
        assert!(require_pubkey32(&[0u8; 31]).is_err());
        assert!(require_pubkey32(&[0u8; 33]).is_err());
        assert!(require_pubkey32(&[0u8; 32]).is_ok());
    }
}
