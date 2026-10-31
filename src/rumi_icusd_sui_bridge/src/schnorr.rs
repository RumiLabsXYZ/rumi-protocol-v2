//! The threshold-Ed25519 signer seam (pattern: frozen Solana `schnorr.rs` at 23dfe841).
//!
//! * The key name follows the profile rule: Sui mainnet `key_1`, everything else `test_key_1`
//!   ([`crate::ids::SuiNetwork::key_name`]).
//! * Derivation paths are `ids::derivation_path(role, chain_identifier)`.
//! * What is signed for Sui is the 32-byte BLAKE2b-256 SIGNING digest of the intent message
//!   (never the typed transaction digest, never the raw BCS): Ed25519 signs that digest. The IC
//!   mapping `sign_with_schnorr(ed25519, message = signing digest)` is an inference from two
//!   documents (chain-sui.md section 3) and has NOT been run here; no live call is made.
//! * A reply the lane cannot trust is classified for the lane's own "re-sign the SAME bytes"
//!   rule: only a definite rejection is `Failed`; a lost or ambiguous reply is `Unknown`,
//!   because a signature may exist that we never saw.
//! * ic-cdk 0.12 has no typed `schnorr` module, so the records are hand-mirrored (same shapes
//!   the Solana instance and the backend use).
//! * Cost: the published price of one `sign_with_schnorr` is 26,153,846,153 cycles on `key_1` and
//!   10,000,000,000 on `test_key_1` (chain-sui.md section 3, sources-native.md); unused cycles are
//!   refunded. The attached headroom here is not a measurement.

use crate::ids::{derivation_path, SuiNetwork, SuiRole};
use crate::sui::Bytes32;
use candid::{CandidType, Deserialize, Principal};
use ic_cdk::api::call::RejectionCode;

pub const SIGN_CYCLES_KEY_1: u128 = 30_000_000_000;
pub const SIGN_CYCLES_TEST_KEY: u128 = 13_000_000_000;

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

/// The outcome of one signing attempt, classified for the same-attempt rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignOutcome {
    Signed([u8; 64]),
    /// Definitely rejected: nothing was signed.
    Failed(String),
    /// Lost or ambiguous: a signature may exist.
    Unknown(String),
}

/// Which signing path a role uses, bound to one network and one key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SuiSigner {
    pub key_name: String,
    pub network: SuiNetwork,
    pub chain_identifier: Bytes32,
}

impl SuiSigner {
    /// From the stored identity. Refuses a chain identifier that is not the network's.
    pub fn new(network: SuiNetwork, chain_identifier: Bytes32) -> Result<Self, String> {
        if chain_identifier != network.chain_identifier() {
            return Err("the chain identifier is not the declared network's".into());
        }
        Ok(SuiSigner { key_name: network.key_name().to_string(), network, chain_identifier })
    }

    fn key_id(&self) -> SchnorrKeyId {
        SchnorrKeyId { algorithm: SchnorrAlgorithm::Ed25519, name: self.key_name.clone() }
    }

    pub fn path(&self, role: SuiRole) -> Vec<Vec<u8>> {
        derivation_path(role, &self.chain_identifier)
    }

    pub fn public_key_argument(&self, role: SuiRole) -> SchnorrPublicKeyArgument {
        SchnorrPublicKeyArgument { canister_id: None, derivation_path: self.path(role), key_id: self.key_id() }
    }

    /// The signing argument. Takes the 32-byte signing digest, so a raw BCS blob or the typed
    /// transaction digest cannot be passed by mistake.
    pub fn sign_argument(&self, role: SuiRole, signing_digest: &Bytes32) -> SignWithSchnorrArgument {
        SignWithSchnorrArgument { message: signing_digest.to_vec(), derivation_path: self.path(role), key_id: self.key_id() }
    }

    pub fn cycles(&self) -> u128 {
        match self.network {
            SuiNetwork::Mainnet => SIGN_CYCLES_KEY_1,
            SuiNetwork::Testnet => SIGN_CYCLES_TEST_KEY,
        }
    }
}

pub fn classify_rejection(code: RejectionCode, message: &str) -> SignOutcome {
    let text = format!("{code:?}: {message}");
    match code {
        RejectionCode::CanisterReject | RejectionCode::DestinationInvalid | RejectionCode::SysFatal => SignOutcome::Failed(text),
        RejectionCode::SysTransient | RejectionCode::CanisterError | RejectionCode::Unknown | RejectionCode::NoError => SignOutcome::Unknown(text),
    }
}

/// A reply of the wrong length cannot be used, but a signature may exist behind it: `Unknown`.
pub fn signature_from_reply(response: SignWithSchnorrResponse) -> SignOutcome {
    match <[u8; 64]>::try_from(response.signature.as_slice()) {
        Ok(sig) => SignOutcome::Signed(sig),
        Err(_) => SignOutcome::Unknown(format!("expected a 64-byte Ed25519 signature, got {} bytes", response.signature.len())),
    }
}

pub fn pubkey_from_reply(response: SchnorrPublicKeyResponse) -> Result<Bytes32, String> {
    <Bytes32>::try_from(response.public_key.as_slice()).map_err(|_| format!("expected a 32-byte Ed25519 public key, got {} bytes", response.public_key.len()))
}

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

impl SuiSigner {
    /// `schnorr_public_key` is free (no cycles attached).
    pub async fn fetch_public_key(&self, role: SuiRole) -> Result<Bytes32, String> {
        let reply: Result<(SchnorrPublicKeyResponse,), CallError> = call_management("schnorr_public_key", (self.public_key_argument(role),), 0).await;
        match reply {
            Ok((r,)) => pubkey_from_reply(r),
            Err(CallError::Rejected(code, message)) => Err(format!("schnorr_public_key {role:?}: {code:?}: {message}")),
            #[cfg(not(target_arch = "wasm32"))]
            Err(CallError::OffChain) => Err("schnorr_public_key: management canister calls are unavailable off-chain".into()),
        }
    }

    /// Sign one signing digest with one role. Not called by any endpoint in the foundation; the
    /// mint lane that uses it is BB-53.
    pub async fn sign_digest(&self, role: SuiRole, signing_digest: &Bytes32) -> SignOutcome {
        let reply: Result<(SignWithSchnorrResponse,), CallError> =
            call_management("sign_with_schnorr", (self.sign_argument(role, signing_digest),), self.cycles()).await;
        match reply {
            Ok((r,)) => signature_from_reply(r),
            Err(CallError::Rejected(code, message)) => classify_rejection(code, &message),
            #[cfg(not(target_arch = "wasm32"))]
            Err(CallError::OffChain) => SignOutcome::Failed("sign_with_schnorr: management canister calls are unavailable off-chain".into()),
        }
    }
}

#[cfg(test)]
pub fn block_on<F: std::future::Future>(f: F) -> F::Output {
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};
    struct Noop;
    impl Wake for Noop {
        fn wake(self: Arc<Self>) {}
    }
    let waker = Waker::from(Arc::new(Noop));
    let mut cx = Context::from_waker(&waker);
    let mut f = Box::pin(f);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{MAINNET_CHAIN_IDENTIFIER, TESTNET_CHAIN_IDENTIFIER};

    #[test]
    fn the_key_name_follows_the_network_rule_and_refuses_a_mismatched_identity() {
        assert_eq!(SuiSigner::new(SuiNetwork::Mainnet, MAINNET_CHAIN_IDENTIFIER).unwrap().key_name, "key_1");
        assert_eq!(SuiSigner::new(SuiNetwork::Testnet, TESTNET_CHAIN_IDENTIFIER).unwrap().key_name, "test_key_1");
        assert!(SuiSigner::new(SuiNetwork::Mainnet, TESTNET_CHAIN_IDENTIFIER).is_err(), "mainnet must not sign for the testnet identity");
        assert!(SuiSigner::new(SuiNetwork::Testnet, MAINNET_CHAIN_IDENTIFIER).is_err(), "a test network must not use the production key");
        assert!(SuiSigner::new(SuiNetwork::Testnet, [7; 32]).is_err());
    }

    #[test]
    fn arguments_use_ed25519_the_role_path_and_the_signing_digest() {
        let s = SuiSigner::new(SuiNetwork::Testnet, TESTNET_CHAIN_IDENTIFIER).unwrap();
        let digest = [0x5a; 32];
        for role in SuiRole::ALL {
            let a = s.sign_argument(role, &digest);
            assert_eq!(a.message, digest.to_vec(), "the 32-byte signing digest");
            assert_eq!(a.derivation_path, derivation_path(role, &TESTNET_CHAIN_IDENTIFIER));
            assert!(matches!(a.key_id.algorithm, SchnorrAlgorithm::Ed25519));
            assert_eq!(a.key_id.name, "test_key_1");
            let pk = s.public_key_argument(role);
            assert_eq!(pk.derivation_path, a.derivation_path);
            assert_eq!(pk.canister_id, None);
        }
        let paths: std::collections::BTreeSet<_> = SuiRole::ALL.iter().map(|r| s.path(*r)).collect();
        assert_eq!(paths.len(), 3, "the three roles never share a path");
        let main = SuiSigner::new(SuiNetwork::Mainnet, MAINNET_CHAIN_IDENTIFIER).unwrap();
        assert_ne!(s.path(SuiRole::Treasury), main.path(SuiRole::Treasury), "the network is part of the path");
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
        for code in [RejectionCode::CanisterReject, RejectionCode::DestinationInvalid, RejectionCode::SysFatal] {
            assert!(matches!(classify_rejection(code, "m"), SignOutcome::Failed(_)), "{code:?}");
        }
        for code in [RejectionCode::SysTransient, RejectionCode::CanisterError, RejectionCode::Unknown, RejectionCode::NoError] {
            assert!(matches!(classify_rejection(code, "SYS_UNKNOWN"), SignOutcome::Unknown(_)), "{code:?}");
        }
    }

    #[test]
    fn cycles_are_per_key_and_the_signer_is_off_chain_in_tests() {
        let main = SuiSigner::new(SuiNetwork::Mainnet, MAINNET_CHAIN_IDENTIFIER).unwrap();
        let test = SuiSigner::new(SuiNetwork::Testnet, TESTNET_CHAIN_IDENTIFIER).unwrap();
        assert!(main.cycles() >= 26_153_846_153);
        assert!(test.cycles() >= 10_000_000_000);
        assert!(test.cycles() < main.cycles());
        assert!(block_on(test.fetch_public_key(SuiRole::Treasury)).unwrap_err().contains("unavailable off-chain"));
        assert!(matches!(block_on(test.sign_digest(SuiRole::Treasury, &[1; 32])), SignOutcome::Failed(m) if m.contains("unavailable off-chain")));
    }
}
