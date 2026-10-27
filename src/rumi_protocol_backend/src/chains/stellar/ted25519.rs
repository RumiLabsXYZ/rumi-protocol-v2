//! Threshold Ed25519 key derivation and signing for Stellar.
//!
//! Stellar transaction signing uses the 32-byte network-bound transaction
//! signature-payload hash produced by `stellar::codec`. The management
//! canister's Ed25519 Schnorr method signs the bytes it receives, so this
//! module validates and forwards exactly that 32-byte hash.
//!
//! ic-cdk 0.12 does not expose typed Schnorr management-canister bindings.
//! These minimal Candid records mirror the current XRP/Solana call shape and
//! intentionally keep the key name explicit at each call site.

use crate::chains::config::ChainId;
use candid::{CandidType, Deserialize, Principal};

use super::address::classic_address_from_ed25519_pubkey;

/// Stable chain-key namespace for Stellar. Verified distinct from the repo's
/// configured XRP (144) and Solana (501) derivation namespaces.
pub const STELLAR_CHAIN_ID: ChainId = ChainId(148);

/// The only Stellar key roles this signer can address. Callers cannot supply an
/// arbitrary management-canister derivation path or select another chain domain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StellarKeyRole {
    /// Per-vault user custody key, isolated by owner and nonce.
    Custody { owner: Principal, nonce: u64 },
    /// Protocol-controlled settlement key.
    Settlement,
}

/// Build the private, fixed-domain derivation path for one supported role.
fn derivation_path(role: &StellarKeyRole) -> Vec<Vec<u8>> {
    match role {
        StellarKeyRole::Custody { owner, nonce } => vec![
            STELLAR_CHAIN_ID.0.to_le_bytes().to_vec(),
            b"custody".to_vec(),
            owner.as_slice().to_vec(),
            nonce.to_le_bytes().to_vec(),
        ],
        StellarKeyRole::Settlement => vec![
            STELLAR_CHAIN_ID.0.to_le_bytes().to_vec(),
            b"settlement".to_vec(),
        ],
    }
}

// ─── Management-canister Schnorr Candid records ─────────────────────────────

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

fn key_id(key_name: &str) -> SchnorrKeyId {
    SchnorrKeyId {
        algorithm: SchnorrAlgorithm::Ed25519,
        name: key_name.to_string(),
    }
}

fn require_pubkey32(bytes: &[u8]) -> Result<[u8; 32], String> {
    bytes
        .try_into()
        .map_err(|_| format!("expected 32-byte Ed25519 pubkey, got {}", bytes.len()))
}

fn require_payload_hash32(bytes: &[u8]) -> Result<[u8; 32], String> {
    bytes
        .try_into()
        .map_err(|_| format!("expected 32-byte Stellar payload hash, got {}", bytes.len()))
}

fn require_signature64(bytes: &[u8]) -> Result<[u8; 64], String> {
    bytes
        .try_into()
        .map_err(|_| format!("expected 64-byte Ed25519 signature, got {}", bytes.len()))
}

/// Derive the raw Ed25519 public key and its Stellar classic `G…` address.
/// The key name is explicit so callers must select a configured key themselves.
pub async fn derive_stellar_address(
    role: StellarKeyRole,
    key_name: &str,
) -> Result<([u8; 32], String), String> {
    let arg = SchnorrPublicKeyArgument {
        canister_id: None,
        derivation_path: derivation_path(&role),
        key_id: key_id(key_name),
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

/// Cycles for one threshold Ed25519 signature. Current production reference
/// cost is 26,153,846,153 cycles on a 34-node fiduciary subnet; 30B adds margin
/// and unused cycles are refunded. The pinned ic-cdk is 0.12.2, so this retains
/// the repo's direct management-canister Candid call instead of newer bindings.
const SIGN_WITH_SCHNORR_CYCLES: u128 = 30_000_000_000;

/// Sign exactly one 32-byte Stellar transaction signature-payload hash.
/// Returns a fixed-size 64-byte Ed25519 signature. The key name is explicit and
/// has no module or config default.
pub async fn sign_payload_hash(
    payload_hash: &[u8],
    role: StellarKeyRole,
    key_name: &str,
) -> Result<[u8; 64], String> {
    let payload_hash = require_payload_hash32(payload_hash)?;
    let arg = SignWithSchnorrArgument {
        message: payload_hash.to_vec(),
        derivation_path: derivation_path(&role),
        key_id: key_id(key_name),
    };
    let (res,): (SignWithSchnorrResponse,) = ic_cdk::api::call::call_with_payment128(
        Principal::management_canister(),
        "sign_with_schnorr",
        (arg,),
        SIGN_WITH_SCHNORR_CYCLES,
    )
    .await
    .map_err(|(code, msg)| format!("{code:?}: {msg}"))?;

    require_signature64(&res.signature)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stellar_paths_are_separate_by_chain_role_and_nonce() {
        let user = Principal::from_slice(&[0xabu8; 16]);
        let custody = derivation_path(&StellarKeyRole::Custody {
            owner: user.clone(),
            nonce: 7,
        });
        let settlement = derivation_path(&StellarKeyRole::Settlement);
        assert_eq!(STELLAR_CHAIN_ID.0, 148);
        assert_ne!(custody, settlement);
        assert_ne!(
            custody,
            derivation_path(&StellarKeyRole::Custody {
                owner: user.clone(),
                nonce: 8,
            })
        );

        let xrp = crate::chains::xrp::ted25519::custody_derivation_path(
            crate::chains::xrp::config::XRP_CHAIN_ID,
            user.clone(),
            7,
        );
        let solana = crate::chains::solana::ted25519::custody_derivation_path(
            crate::chains::solana::config::SOLANA_CHAIN_ID,
            user,
            7,
        );
        assert_ne!(custody, xrp);
        assert_ne!(custody, solana);
        assert_ne!(STELLAR_CHAIN_ID, crate::chains::xrp::config::XRP_CHAIN_ID);
        assert_ne!(
            STELLAR_CHAIN_ID,
            crate::chains::solana::config::SOLANA_CHAIN_ID
        );
    }

    #[test]
    fn role_type_exposes_only_stellar_custody_and_settlement_paths() {
        let owner = Principal::from_slice(&[0x42; 16]);
        let role = StellarKeyRole::Custody {
            owner: owner.clone(),
            nonce: 9,
        };
        let path = derivation_path(&role);
        assert_eq!(path.len(), 4);
        assert_eq!(path[0], STELLAR_CHAIN_ID.0.to_le_bytes().to_vec());
        assert_eq!(path[1], b"custody".to_vec());
        assert_eq!(path[2], owner.as_slice().to_vec());
        assert_eq!(path[3], 9u64.to_le_bytes().to_vec());
        // Both public canister-call APIs accept only `StellarKeyRole`; the
        // derivation-path function remains private to this module.
    }

    #[test]
    fn validates_public_key_payload_hash_and_signature_lengths() {
        assert!(require_pubkey32(&[0u8; 31]).is_err());
        assert!(require_pubkey32(&[0u8; 33]).is_err());
        assert!(require_pubkey32(&[0u8; 32]).is_ok());

        assert!(require_payload_hash32(&[0u8; 31]).is_err());
        assert!(require_payload_hash32(&[0u8; 33]).is_err());
        assert!(require_payload_hash32(&[0u8; 32]).is_ok());

        assert!(require_signature64(&[0u8; 63]).is_err());
        assert!(require_signature64(&[0u8; 65]).is_err());
        assert!(require_signature64(&[0u8; 64]).is_ok());
    }

    #[test]
    fn key_name_is_passed_through_without_a_default() {
        assert_eq!(key_id("explicit-key").name, "explicit-key");
    }
}
