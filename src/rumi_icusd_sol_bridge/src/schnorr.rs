//! The real signer (49a): threshold Ed25519 through the management canister, behind the
//! [`TxSigner`] seam the nonce lane already runs against. Unit tests keep using the mock; nothing
//! here is exercised against a network.
//!
//! * The key name follows the profile rule (note section 5): mainnet-beta genesis `key_1`, every
//!   other cluster `test_key_1` (`state::key_name_for_genesis`). The signer refuses a stored
//!   configuration whose key name breaks the rule.
//! * Derivation paths are `ids::derivation_path(role, genesis_hash)`, i.e.
//!   `[rumi-icusd-sol-bridge, v1, role, genesis_hash_32]`.
//! * The message signed is the full serialized Solana message bytes (not a hash); the reply is the
//!   64-byte signature. ic-cdk 0.12 has no typed `schnorr` module, so the argument records are
//!   hand-mirrored (same shapes the backend rail uses in `chains/solana/ted25519.rs`).
//! * A reply the lane cannot trust is classified for the lane's own rule, which is "re-sign the SAME
//!   bytes": only a definite rejection is `Failed`; a lost or ambiguous reply (`SYS_UNKNOWN`,
//!   `CANISTER_ERROR`, a transient system error) is `Unknown`, because a signature may exist that
//!   we never saw.
//! * Cost: the published price of one `sign_with_schnorr` is 26,153,846,153 cycles on `key_1` and
//!   10,000,000,000 on `test_key_1` (source S10 of the note; unused cycles are refunded). The
//!   signer attaches a headroom amount per key; the cost has not been measured here.

use crate::ids::{derivation_path, KeyRole};
use crate::lane::{SignOutcome, TxSigner};
use crate::state::{BridgeConfig, MAINNET_KEY_NAME, TEST_KEY_NAME};
use crate::tx::Pubkey;
use candid::{CandidType, Deserialize, Principal};
use ic_cdk::api::call::RejectionCode;

/// Cycles attached to one `sign_with_schnorr` on `key_1` (published cost 26.15B, plus headroom).
pub const SIGN_CYCLES_KEY_1: u128 = 30_000_000_000;
/// Cycles attached on `test_key_1` (published cost 10B, plus headroom).
pub const SIGN_CYCLES_TEST_KEY: u128 = 13_000_000_000;

/// Management-canister Schnorr records. `aux` (BIP341 tweak, secp256k1 only) is omitted.
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

/// Which signing path a role uses, bound to one cluster and one key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchnorrSigner {
    pub key_name: String,
    pub genesis_hash: [u8; 32],
}

impl SchnorrSigner {
    /// From the stored configuration. Refuses a key name that breaks the profile rule.
    pub fn from_config(config: &BridgeConfig) -> Result<Self, String> {
        let expected = crate::state::key_name_for_genesis(&config.genesis_hash);
        if config.schnorr_key_name != expected {
            return Err(format!("the stored key name {} breaks the key rule ({expected})", config.schnorr_key_name));
        }
        Ok(SchnorrSigner { key_name: config.schnorr_key_name.clone(), genesis_hash: config.genesis_hash })
    }

    fn key_id(&self) -> SchnorrKeyId {
        SchnorrKeyId { algorithm: SchnorrAlgorithm::Ed25519, name: self.key_name.clone() }
    }

    pub fn path(&self, role: KeyRole) -> Vec<Vec<u8>> {
        derivation_path(role, &self.genesis_hash)
    }

    pub fn public_key_argument(&self, role: KeyRole) -> SchnorrPublicKeyArgument {
        SchnorrPublicKeyArgument { canister_id: None, derivation_path: self.path(role), key_id: self.key_id() }
    }

    pub fn sign_argument(&self, role: KeyRole, message: &[u8]) -> SignWithSchnorrArgument {
        SignWithSchnorrArgument { message: message.to_vec(), derivation_path: self.path(role), key_id: self.key_id() }
    }

    pub fn cycles(&self) -> u128 {
        if self.key_name == MAINNET_KEY_NAME {
            SIGN_CYCLES_KEY_1
        } else {
            debug_assert_eq!(self.key_name, TEST_KEY_NAME);
            SIGN_CYCLES_TEST_KEY
        }
    }
}

/// Classify a rejected `sign_with_schnorr` call for the lane. Only a definite rejection is
/// `Failed`; anything that may have executed is `Unknown`.
pub fn classify_rejection(code: RejectionCode, message: &str) -> SignOutcome {
    let text = format!("{code:?}: {message}");
    match code {
        // The callee refused, or does not exist, or the system says it will not work: nothing signed.
        RejectionCode::CanisterReject | RejectionCode::DestinationInvalid | RejectionCode::SysFatal => SignOutcome::Failed(text),
        // A transient system error, a canister error and "unknown" may have executed.
        RejectionCode::SysTransient | RejectionCode::CanisterError | RejectionCode::Unknown | RejectionCode::NoError => SignOutcome::Unknown(text),
    }
}

/// Validate a `sign_with_schnorr` reply. A reply of the wrong length cannot be used, but a
/// signature may exist behind it, so it is `Unknown`, not `Failed`.
pub fn signature_from_reply(response: SignWithSchnorrResponse) -> SignOutcome {
    match <[u8; 64]>::try_from(response.signature.as_slice()) {
        Ok(sig) => SignOutcome::Signed(sig),
        Err(_) => SignOutcome::Unknown(format!("expected a 64-byte Ed25519 signature, got {} bytes", response.signature.len())),
    }
}

pub fn pubkey_from_reply(response: SchnorrPublicKeyResponse) -> Result<Pubkey, String> {
    <[u8; 32]>::try_from(response.public_key.as_slice())
        .map_err(|_| format!("expected a 32-byte Ed25519 public key, got {} bytes", response.public_key.len()))
}

/// Result of a management-canister call. Off-chain (unit tests) every call is `OffChain`.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
enum CallError {
    Rejected(RejectionCode, String),
    #[cfg(not(target_arch = "wasm32"))]
    OffChain,
}

#[cfg(target_arch = "wasm32")]
async fn call_management<A, R>(method: &str, args: A, cycles: u128) -> Result<R, CallError>
where
    A: candid::utils::ArgumentEncoder,
    R: for<'a> candid::utils::ArgumentDecoder<'a>,
{
    ic_cdk::api::call::call_with_payment128(Principal::management_canister(), method, args, cycles)
        .await
        .map_err(|(code, message)| CallError::Rejected(code, message))
}

#[cfg(not(target_arch = "wasm32"))]
async fn call_management<A, R>(_method: &str, _args: A, _cycles: u128) -> Result<R, CallError>
where
    A: candid::utils::ArgumentEncoder,
    R: for<'a> candid::utils::ArgumentDecoder<'a>,
{
    Err(CallError::OffChain)
}

impl TxSigner for SchnorrSigner {
    async fn public_key(&self, role: KeyRole) -> Result<Pubkey, String> {
        // `schnorr_public_key` is free (no cycles attached).
        let reply: Result<(SchnorrPublicKeyResponse,), CallError> = call_management("schnorr_public_key", (self.public_key_argument(role),), 0).await;
        match reply {
            Ok((r,)) => pubkey_from_reply(r),
            Err(CallError::Rejected(code, message)) => Err(format!("schnorr_public_key {role:?}: {code:?}: {message}")),
            #[cfg(not(target_arch = "wasm32"))]
            Err(CallError::OffChain) => Err("schnorr_public_key: management canister calls are unavailable off-chain".into()),
        }
    }

    async fn sign(&self, role: KeyRole, message: &[u8]) -> SignOutcome {
        let reply: Result<(SignWithSchnorrResponse,), CallError> =
            call_management("sign_with_schnorr", (self.sign_argument(role, message),), self.cycles()).await;
        match reply {
            Ok((r,)) => signature_from_reply(r),
            Err(CallError::Rejected(code, message)) => classify_rejection(code, &message),
            #[cfg(not(target_arch = "wasm32"))]
            Err(CallError::OffChain) => SignOutcome::Failed("sign_with_schnorr: management canister calls are unavailable off-chain".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::block_on;
    use crate::tx::pubkey_to_base58;

    fn config(genesis: [u8; 32], key: &str) -> BridgeConfig {
        BridgeConfig { genesis_hash: genesis, schnorr_key_name: key.into(), ..BridgeConfig::default() }
    }

    fn mainnet() -> [u8; 32] {
        crate::tx::pubkey_from_base58(crate::state::MAINNET_GENESIS_HASH_B58).unwrap()
    }

    #[test]
    fn the_key_name_follows_the_profile_rule() {
        assert_eq!(SchnorrSigner::from_config(&config(mainnet(), "key_1")).unwrap().key_name, "key_1");
        assert_eq!(SchnorrSigner::from_config(&config([0x42; 32], "test_key_1")).unwrap().key_name, "test_key_1");
        assert!(SchnorrSigner::from_config(&config(mainnet(), "test_key_1")).is_err(), "mainnet must not sign with the test key");
        assert!(SchnorrSigner::from_config(&config([0x42; 32], "key_1")).is_err(), "a test cluster must not sign with the production key");
        assert!(SchnorrSigner::from_config(&config([0x42; 32], "dfx_test_key")).is_err());
    }

    #[test]
    fn arguments_use_ed25519_the_role_path_and_the_full_message() {
        let genesis = [0xAB; 32];
        let s = SchnorrSigner::from_config(&config(genesis, "test_key_1")).unwrap();
        let msg = vec![1u8, 2, 3, 4];
        for role in KeyRole::ALL {
            let a = s.sign_argument(role, &msg);
            assert_eq!(a.message, msg, "the full serialized message, not a hash");
            assert_eq!(a.derivation_path, crate::ids::derivation_path(role, &genesis));
            assert!(matches!(a.key_id.algorithm, SchnorrAlgorithm::Ed25519));
            assert_eq!(a.key_id.name, "test_key_1");
            let pk = s.public_key_argument(role);
            assert_eq!(pk.derivation_path, a.derivation_path);
            assert_eq!(pk.canister_id, None);
        }
        // The four roles never share a path, and the cluster is part of it.
        let paths: std::collections::BTreeSet<_> = KeyRole::ALL.iter().map(|r| s.path(*r)).collect();
        assert_eq!(paths.len(), 4);
        let other = SchnorrSigner::from_config(&config([0xAC; 32], "test_key_1")).unwrap();
        assert_ne!(s.path(KeyRole::FeePayer), other.path(KeyRole::FeePayer));
    }

    #[test]
    fn the_candid_records_have_the_management_canister_field_names() {
        use candid::types::{CandidType, TypeInner};
        fn fields<T: CandidType>() -> Vec<String> {
            match T::ty().as_ref() {
                TypeInner::Record(fs) => fs.iter().map(|f| f.id.to_string()).collect(),
                other => panic!("not a record: {other:?}"),
            }
        }
        let mut sign = fields::<SignWithSchnorrArgument>();
        sign.sort();
        assert_eq!(sign, ["derivation_path", "key_id", "message"]);
        let mut public = fields::<SchnorrPublicKeyArgument>();
        public.sort();
        assert_eq!(public, ["canister_id", "derivation_path", "key_id"]);
        let mut key_id = fields::<SchnorrKeyId>();
        key_id.sort();
        assert_eq!(key_id, ["algorithm", "name"]);
        assert_eq!(fields::<SignWithSchnorrResponse>(), ["signature"]);
        let mut pk = fields::<SchnorrPublicKeyResponse>();
        pk.sort();
        assert_eq!(pk, ["chain_code", "public_key"]);
        match SchnorrAlgorithm::ty().as_ref() {
            TypeInner::Variant(vs) => {
                let mut names: Vec<_> = vs.iter().map(|f| f.id.to_string()).collect();
                names.sort();
                assert_eq!(names, ["bip340secp256k1", "ed25519"]);
            }
            other => panic!("not a variant: {other:?}"),
        }
    }

    #[test]
    fn replies_are_validated_and_classified_for_the_resign_rule() {
        assert_eq!(signature_from_reply(SignWithSchnorrResponse { signature: vec![7; 64] }), SignOutcome::Signed([7; 64]));
        for len in [0usize, 63, 65] {
            assert!(matches!(signature_from_reply(SignWithSchnorrResponse { signature: vec![7; len] }), SignOutcome::Unknown(_)), "len {len}");
        }
        assert_eq!(pubkey_from_reply(SchnorrPublicKeyResponse { public_key: vec![5; 32], chain_code: vec![] }).unwrap(), [5; 32]);
        assert!(pubkey_from_reply(SchnorrPublicKeyResponse { public_key: vec![5; 33], chain_code: vec![] }).is_err());
        // Definite rejections are Failed; lost or ambiguous replies are Unknown.
        for code in [RejectionCode::CanisterReject, RejectionCode::DestinationInvalid, RejectionCode::SysFatal] {
            assert!(matches!(classify_rejection(code, "m"), SignOutcome::Failed(_)), "{code:?}");
        }
        for code in [RejectionCode::SysTransient, RejectionCode::CanisterError, RejectionCode::Unknown, RejectionCode::NoError] {
            assert!(matches!(classify_rejection(code, "SYS_UNKNOWN"), SignOutcome::Unknown(_)), "{code:?}");
        }
    }

    #[test]
    fn cycles_are_per_key_and_the_signer_is_off_chain_in_tests() {
        let main = SchnorrSigner::from_config(&config(mainnet(), "key_1")).unwrap();
        let test = SchnorrSigner::from_config(&config([1; 32], "test_key_1")).unwrap();
        assert!(main.cycles() >= 26_153_846_153, "covers the published key_1 price");
        assert!(test.cycles() >= 10_000_000_000, "covers the published test_key_1 price");
        assert!(test.cycles() < main.cycles());
        assert!(block_on(test.public_key(KeyRole::FeePayer)).unwrap_err().contains("unavailable off-chain"));
        assert!(matches!(block_on(test.sign(KeyRole::FeePayer, &[1])), SignOutcome::Failed(m) if m.contains("unavailable off-chain")));
        let _ = pubkey_to_base58(&[0; 32]);
    }
}
