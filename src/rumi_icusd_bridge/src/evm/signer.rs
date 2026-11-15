//! Owned by card BB-04. Threshold-ECDSA public keys, addresses and signing.
//!
//! Each lane (minter, admin) has its own derived key: path
//! `ids::derivation_path(lane, chain_id)` under the configured key name. The derived
//! public key and address are cached in core state the first time they are needed and
//! never overwritten: if a later derivation yields a different address something is
//! badly wrong (the subnet key changed, or the config did) and we refuse rather than
//! silently start signing as a different identity.
//!
//! Signatures are never cached or reused: every `sign_tx` call asks the management
//! canister for a fresh signature over the exact digest, then normalizes it (low-s) and
//! recovers the y-parity locally against the cached public key.
//!
//! The management-canister calls only exist on wasm32. On the host (unit tests) they
//! return `Err("tECDSA unavailable on host")`, except inside this crate's own tests
//! where `host_tecdsa` can emulate a subnet key.

use crate::evm::tx::{self, Eip1559Tx};
use crate::ids::{self, hex0x, LaneKind};
use crate::state::{self, AuditCtx};
use crate::types::{BaseAddresses, BridgeError, BridgeResult};
use k256::elliptic_curve::sec1::ToEncodedPoint;
use k256::PublicKey;

/// Message returned by the management-canister shims when not running on the IC.
pub const HOST_UNAVAILABLE: &str = "tECDSA unavailable on host";

/// EVM address of a secp256k1 public key (SEC1, compressed or uncompressed):
/// `keccak256(uncompressed_xy)[12..]`.
pub fn address_from_pubkey(sec1: &[u8]) -> Result<[u8; 20], String> {
    let pk = PublicKey::from_sec1_bytes(sec1).map_err(|e| format!("invalid secp256k1 public key: {e}"))?;
    let point = pk.to_encoded_point(false);
    let bytes = point.as_bytes();
    if bytes.len() != 65 || bytes[0] != 0x04 {
        return Err(format!("unexpected uncompressed point encoding ({} bytes)", bytes.len()));
    }
    let hash = ids::keccak256(&bytes[1..]);
    let mut address = [0u8; 20];
    address.copy_from_slice(&hash[12..]);
    Ok(address)
}

fn lane_name(lane: LaneKind) -> &'static str {
    match lane {
        LaneKind::Minter => "minter",
        LaneKind::Admin => "admin",
    }
}

/// `(ecdsa_key_name, chain_id)` from the immutable-in-practice config.
fn key_config() -> (String, u64) {
    state::read_core(|c| (c.config.ecdsa_key_name.clone(), c.config.chain_id))
}

/// A lane's cached signing identity.
#[derive(Clone, Debug)]
struct LaneIdentity {
    public_key: Vec<u8>,
    address: [u8; 20],
}

/// The lane's identity, deriving and caching it on first use.
async fn lane_identity(lane: LaneKind) -> Result<LaneIdentity, String> {
    let (key_name, chain_id, cached_address, cached_pubkey) = state::read_core(|c| {
        let l = c.lane(lane);
        (c.config.ecdsa_key_name.clone(), c.config.chain_id, l.address, l.public_key.clone())
    });
    if let (Some(address), Some(public_key)) = (cached_address, cached_pubkey.clone()) {
        return Ok(LaneIdentity { public_key, address });
    }
    if key_name.is_empty() || chain_id == 0 {
        return Err("bridge is not configured (missing ECDSA key name or chain id)".into());
    }

    // Nothing is held across this await: the cache is re-checked when it returns.
    let path = ids::derivation_path(lane, chain_id);
    let public_key = platform::public_key(&key_name, path).await?;
    let address = address_from_pubkey(&public_key)?;

    let newly_cached = state::mutate_core(|c| {
        let l = c.lane_mut(lane);
        if let Some(existing) = l.address {
            if existing != address {
                return Err(format!(
                    "{} lane address changed: cached {} but derivation now yields {}",
                    lane_name(lane),
                    hex0x(&existing),
                    hex0x(&address)
                ));
            }
        }
        if let Some(existing_key) = &l.public_key {
            if address_from_pubkey(existing_key)? != address {
                return Err(format!("{} lane public key changed since it was cached", lane_name(lane)));
            }
        }
        let newly = l.address.is_none() || l.public_key.is_none();
        l.address = Some(address);
        l.public_key = Some(public_key.clone());
        Ok(newly)
    })?;
    if newly_cached {
        state::append_audit(
            None,
            AuditCtx::note(format!("{} lane address derived: {}", lane_name(lane), hex0x(&address))),
        );
    }
    Ok(LaneIdentity { public_key, address })
}

/// Derive (once) and cache the lane's public key and address in core state.
pub async fn lane_address(lane: LaneKind) -> Result<[u8; 20], String> {
    let (address, has_public_key) = state::read_core(|c| (c.lane(lane).address, c.lane(lane).public_key.is_some()));
    match address {
        Some(address) if has_public_key => Ok(address),
        // Nothing cached, or a cached address whose public key is missing: derive, and
        // (when an address is cached) verify the derivation matches it.
        _ => Ok(lane_identity(lane).await?.address),
    }
}

/// Both lane addresses, as `0x` hex. Derives and caches any that are missing.
pub async fn ensure_lane_addresses() -> BridgeResult<BaseAddresses> {
    let minter = lane_address(LaneKind::Minter).await.map_err(BridgeError::Internal)?;
    let admin = lane_address(LaneKind::Admin).await.map_err(BridgeError::Internal)?;
    if minter == admin {
        // The contract constructor also rejects minter == admin; never present such a pair.
        return Err(BridgeError::Internal("minter and admin lane addresses are identical".into()));
    }
    Ok(BaseAddresses { minter: hex0x(&minter), admin: hex0x(&admin) })
}

/// Sign `tx` with the lane key. Returns `(raw signed bytes, tx hash)`.
///
/// The signature is requested fresh for this exact transaction and is not stored here.
/// Callers persist the returned bytes before broadcasting.
pub async fn sign_tx(lane: LaneKind, tx: &Eip1559Tx) -> Result<(Vec<u8>, [u8; 32]), String> {
    let (key_name, chain_id) = key_config();
    if tx.chain_id != chain_id {
        // Fail closed: a lane key is bound to one chain, a mismatched tx is a bug.
        return Err(format!("refusing to sign: tx chain id {} does not match configured chain id {chain_id}", tx.chain_id));
    }
    let identity = lane_identity(lane).await?;
    let digest = tx::signing_hash(tx);
    let signature = platform::sign(&key_name, ids::derivation_path(lane, chain_id), digest).await?;
    let sig64: [u8; 64] = signature
        .as_slice()
        .try_into()
        .map_err(|_| format!("expected a 64-byte signature, got {} bytes", signature.len()))?;
    let (y_parity, r, s) = tx::finalize_signature(&digest, &sig64, &identity.public_key)?;
    let raw = tx::encode_signed(tx, y_parity, &r, &s);
    let hash = tx::tx_hash(&raw);
    Ok((raw, hash))
}

// ---------------------------------------------------------------------------
// Management-canister shims
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
mod platform {
    use ic_cdk::api::management_canister::ecdsa::{
        ecdsa_public_key, sign_with_ecdsa, EcdsaCurve, EcdsaKeyId, EcdsaPublicKeyArgument, SignWithEcdsaArgument,
    };

    fn key_id(name: &str) -> EcdsaKeyId {
        EcdsaKeyId { curve: EcdsaCurve::Secp256k1, name: name.to_string() }
    }

    pub async fn public_key(key_name: &str, derivation_path: Vec<Vec<u8>>) -> Result<Vec<u8>, String> {
        let arg = EcdsaPublicKeyArgument { canister_id: None, derivation_path, key_id: key_id(key_name) };
        let (res,) = ecdsa_public_key(arg).await.map_err(|(code, msg)| format!("ecdsa_public_key failed: {code:?}: {msg}"))?;
        Ok(res.public_key)
    }

    /// ic-cdk 0.12 attaches the signing fee itself.
    pub async fn sign(key_name: &str, derivation_path: Vec<Vec<u8>>, digest: [u8; 32]) -> Result<Vec<u8>, String> {
        let arg = SignWithEcdsaArgument { message_hash: digest.to_vec(), derivation_path, key_id: key_id(key_name) };
        let (res,) = sign_with_ecdsa(arg).await.map_err(|(code, msg)| format!("sign_with_ecdsa failed: {code:?}: {msg}"))?;
        Ok(res.signature)
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod platform {
    pub async fn public_key(key_name: &str, derivation_path: Vec<Vec<u8>>) -> Result<Vec<u8>, String> {
        #[cfg(test)]
        if let Some(pk) = super::host_tecdsa::public_key(key_name, &derivation_path) {
            return Ok(pk);
        }
        let _ = (key_name, derivation_path);
        Err(super::HOST_UNAVAILABLE.into())
    }

    pub async fn sign(key_name: &str, derivation_path: Vec<Vec<u8>>, digest: [u8; 32]) -> Result<Vec<u8>, String> {
        #[cfg(test)]
        if let Some(sig) = super::host_tecdsa::sign(key_name, &derivation_path, &digest) {
            return Ok(sig);
        }
        let _ = (key_name, derivation_path, digest);
        Err(super::HOST_UNAVAILABLE.into())
    }
}

/// Host-only tECDSA emulation for this crate's unit tests. Disabled by default, so the
/// host fallback (`HOST_UNAVAILABLE`) is what ordinary tests see. When enabled, every
/// `(key name, derivation path)` maps to its own deterministic secp256k1 key, mirroring
/// the subnet property that each path has an independent key.
#[cfg(test)]
pub mod host_tecdsa {
    use k256::ecdsa::signature::hazmat::PrehashSigner;
    use k256::ecdsa::{Signature, SigningKey};
    use sha2::{Digest, Sha256};
    use std::cell::{Cell, RefCell};

    thread_local! {
        static ENABLED: Cell<bool> = const { Cell::new(false) };
        static RETURN_HIGH_S: Cell<bool> = const { Cell::new(false) };
        static CALLS: RefCell<Vec<(String, Vec<Vec<u8>>)>> = const { RefCell::new(Vec::new()) };
    }

    pub fn enable() {
        ENABLED.with(|e| e.set(true));
    }

    pub fn disable() {
        ENABLED.with(|e| e.set(false));
        RETURN_HIGH_S.with(|e| e.set(false));
        CALLS.with(|c| c.borrow_mut().clear());
    }

    /// Make the emulated subnet return the high-s twin of every signature (real
    /// threshold signatures are not guaranteed to be low-s).
    pub fn set_return_high_s(on: bool) {
        RETURN_HIGH_S.with(|e| e.set(on));
    }

    /// Every `(key name, derivation path)` the signer asked the emulated subnet about.
    pub fn calls() -> Vec<(String, Vec<Vec<u8>>)> {
        CALLS.with(|c| c.borrow().clone())
    }

    /// The deterministic key for `(key_name, path)`.
    pub fn signing_key(key_name: &str, path: &[Vec<u8>]) -> SigningKey {
        let mut counter = 0u32;
        loop {
            let mut h = Sha256::new();
            h.update(b"host-tecdsa");
            h.update(counter.to_be_bytes());
            h.update(key_name.as_bytes());
            for seg in path {
                h.update((seg.len() as u64).to_be_bytes());
                h.update(seg);
            }
            if let Ok(k) = SigningKey::from_slice(&h.finalize()) {
                return k;
            }
            counter += 1;
        }
    }

    pub fn public_key(key_name: &str, path: &[Vec<u8>]) -> Option<Vec<u8>> {
        if !ENABLED.with(|e| e.get()) {
            return None;
        }
        CALLS.with(|c| c.borrow_mut().push((key_name.to_string(), path.to_vec())));
        // The management canister returns SEC1 compressed keys.
        Some(signing_key(key_name, path).verifying_key().to_encoded_point(true).as_bytes().to_vec())
    }

    pub fn sign(key_name: &str, path: &[Vec<u8>], digest: &[u8; 32]) -> Option<Vec<u8>> {
        if !ENABLED.with(|e| e.get()) {
            return None;
        }
        CALLS.with(|c| c.borrow_mut().push((key_name.to_string(), path.to_vec())));
        let sig: Signature = signing_key(key_name, path).sign_prehash(digest).expect("sign");
        let sig = if RETURN_HIGH_S.with(|e| e.get()) {
            let high = -*sig.s();
            Signature::from_scalars(sig.r().to_bytes(), high.to_bytes()).expect("high-s twin")
        } else {
            sig
        };
        Some(sig.to_bytes().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evm::tx::Eip1559Tx;
    use crate::state::{audit_len, audit_page, mutate_core, read_core, reset_for_tests};
    use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
    use std::future::Future;

    const PRIV: &str = "4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318";
    const PRIV_ADDRESS: &str = "2c7536e3605d9c16a7a3d7b1898e529396a65c23";
    const CHAIN: u64 = 84532;

    /// Poll a future that never suspends (host shims complete immediately).
    fn block_on<F: Future>(f: F) -> F::Output {
        use std::task::{Context, Poll, Waker};
        let mut f = std::pin::pin!(f);
        let mut cx = Context::from_waker(Waker::noop());
        match f.as_mut().poll(&mut cx) {
            Poll::Ready(v) => v,
            Poll::Pending => panic!("future suspended on the host"),
        }
    }

    fn setup() {
        reset_for_tests();
        host_tecdsa::disable();
        mutate_core(|c| {
            c.config.ecdsa_key_name = "test_key_1".into();
            c.config.chain_id = CHAIN;
        });
    }

    fn sample_tx() -> Eip1559Tx {
        Eip1559Tx {
            chain_id: CHAIN,
            nonce: 3,
            max_priority_fee_per_gas: 1_000_000,
            max_fee_per_gas: 2_000_000_000,
            gas_limit: 150_000,
            to: [0xc0; 20],
            value: 0,
            data: vec![0x1e, 0x45, 0x8b, 0xee, 1, 2, 3],
        }
    }

    // ----- address derivation -------------------------------------------------

    fn priv_pubkey(compressed: bool) -> Vec<u8> {
        let k = k256::ecdsa::SigningKey::from_slice(&hex::decode(PRIV).unwrap()).unwrap();
        k.verifying_key().to_encoded_point(compressed).as_bytes().to_vec()
    }

    #[test]
    fn address_from_pubkey_matches_known_vector_compressed_and_uncompressed() {
        for compressed in [true, false] {
            let pk = priv_pubkey(compressed);
            assert_eq!(pk.len(), if compressed { 33 } else { 65 });
            assert_eq!(hex::encode(address_from_pubkey(&pk).unwrap()), PRIV_ADDRESS);
        }
    }

    #[test]
    fn address_from_pubkey_rejects_garbage() {
        assert!(address_from_pubkey(&[]).is_err());
        assert!(address_from_pubkey(&[0u8; 33]).is_err());
        // x = 5 has no y on secp256k1 (5^3 + 7 is a non-residue mod p).
        let mut off_curve = [0u8; 33];
        off_curve[0] = 0x02;
        off_curve[32] = 0x05;
        assert!(address_from_pubkey(&off_curve).is_err(), "x = 5 is not on the curve");
        assert!(address_from_pubkey(&priv_pubkey(true)[..32]).is_err());
        let mut bad = priv_pubkey(false);
        bad[64] ^= 1; // off-curve point
        assert!(address_from_pubkey(&bad).is_err());
        // A raw 64-byte x||y without the 0x04 tag is not SEC1.
        assert!(address_from_pubkey(&priv_pubkey(false)[1..]).is_err());
    }

    // ----- host fallback ------------------------------------------------------

    #[test]
    fn host_fallback_reports_unavailable_and_caches_nothing() {
        setup();
        assert_eq!(block_on(lane_address(LaneKind::Minter)), Err(HOST_UNAVAILABLE.to_string()));
        assert_eq!(block_on(sign_tx(LaneKind::Minter, &sample_tx())), Err(HOST_UNAVAILABLE.to_string()));
        assert!(read_core(|c| c.minter_lane.address.is_none() && c.minter_lane.public_key.is_none()));
        match block_on(ensure_lane_addresses()) {
            Err(BridgeError::Internal(m)) => assert_eq!(m, HOST_UNAVAILABLE),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn unconfigured_bridge_does_not_derive() {
        reset_for_tests();
        host_tecdsa::enable();
        assert!(block_on(lane_address(LaneKind::Minter)).unwrap_err().contains("not configured"));
        assert!(host_tecdsa::calls().is_empty());
        host_tecdsa::disable();
    }

    // ----- lane addresses -----------------------------------------------------

    #[test]
    fn lane_address_derives_once_caches_and_audits() {
        setup();
        host_tecdsa::enable();
        let a1 = block_on(lane_address(LaneKind::Minter)).unwrap();
        // Derived from the configured key name and the lane's path.
        let path = ids::derivation_path(LaneKind::Minter, CHAIN);
        assert_eq!(host_tecdsa::calls(), vec![("test_key_1".to_string(), path.clone())]);
        // Cached in state: address and compressed public key.
        let (cached_addr, cached_pk) = read_core(|c| (c.minter_lane.address, c.minter_lane.public_key.clone()));
        assert_eq!(cached_addr, Some(a1));
        assert_eq!(address_from_pubkey(&cached_pk.unwrap()).unwrap(), a1);
        // Second call: no new subnet request, same address, no second audit note.
        let a2 = block_on(lane_address(LaneKind::Minter)).unwrap();
        assert_eq!(a1, a2);
        assert_eq!(host_tecdsa::calls().len(), 1, "cache hit must not call the subnet");
        let notes: Vec<_> = audit_page(0, 100).into_iter().filter(|r| r.note.contains("lane address derived")).collect();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].note.contains(&hex0x(&a1)));
        assert!(notes[0].note.starts_with("minter"));
        host_tecdsa::disable();
    }

    #[test]
    fn lanes_have_distinct_addresses_and_ensure_returns_both() {
        setup();
        host_tecdsa::enable();
        let both = block_on(ensure_lane_addresses()).unwrap();
        assert_ne!(both.minter, both.admin);
        assert!(both.minter.starts_with("0x") && both.minter.len() == 42);
        assert_eq!(both.minter, both.minter.to_lowercase());
        assert_eq!(both.minter, hex0x(&block_on(lane_address(LaneKind::Minter)).unwrap()));
        assert_eq!(both.admin, hex0x(&block_on(lane_address(LaneKind::Admin)).unwrap()));
        assert_eq!(host_tecdsa::calls().len(), 2);
        // Idempotent.
        assert_eq!(block_on(ensure_lane_addresses()).unwrap(), both);
        assert_eq!(host_tecdsa::calls().len(), 2);
        host_tecdsa::disable();
    }

    #[test]
    fn address_depends_on_chain_id_and_key_name() {
        let mut addrs = vec![];
        for (key, chain) in [("test_key_1", 84532u64), ("test_key_1", 8453), ("key_1", 84532)] {
            reset_for_tests();
            host_tecdsa::disable();
            host_tecdsa::enable();
            mutate_core(|c| {
                c.config.ecdsa_key_name = key.into();
                c.config.chain_id = chain;
            });
            addrs.push(block_on(lane_address(LaneKind::Minter)).unwrap());
        }
        assert_ne!(addrs[0], addrs[1]);
        assert_ne!(addrs[0], addrs[2]);
        host_tecdsa::disable();
    }

    #[test]
    fn a_changed_key_is_an_error_and_never_overwrites_the_cached_address() {
        setup();
        host_tecdsa::enable();
        // A cached address with the public key missing (e.g. a state written by an older
        // build) is verified against the fresh derivation.
        let genuine = block_on(lane_address(LaneKind::Minter)).unwrap();
        mutate_core(|c| {
            c.minter_lane.public_key = None;
            c.minter_lane.address = Some([0xde; 20]); // not what the subnet will derive
        });
        let err = block_on(lane_address(LaneKind::Minter)).unwrap_err();
        assert!(err.contains("changed"), "{err}");
        assert_eq!(read_core(|c| c.minter_lane.address), Some([0xde; 20]), "cached address untouched");
        assert!(read_core(|c| c.minter_lane.public_key.is_none()), "no key stored on mismatch");
        assert!(block_on(sign_tx(LaneKind::Minter, &sample_tx())).is_err(), "no signing under a mismatched identity");

        // When the cached address does match, the missing public key is filled in.
        mutate_core(|c| c.minter_lane.address = Some(genuine));
        assert_eq!(block_on(lane_address(LaneKind::Minter)).unwrap(), genuine);
        assert!(read_core(|c| c.minter_lane.public_key.is_some()));
        host_tecdsa::disable();
    }

    #[test]
    fn a_mismatched_cached_public_key_is_an_error() {
        setup();
        host_tecdsa::enable();
        mutate_core(|c| {
            c.minter_lane.address = None;
            // A valid key that is not the lane key.
            c.minter_lane.public_key = Some(priv_pubkey(true));
        });
        let before = read_core(|c| c.minter_lane.clone());
        assert!(block_on(lane_address(LaneKind::Minter)).unwrap_err().contains("public key changed"));
        assert_eq!(read_core(|c| c.minter_lane.clone()), before);
        host_tecdsa::disable();
    }

    // ----- signing ------------------------------------------------------------

    fn recover_address(digest: &[u8; 32], raw: &[u8]) -> [u8; 20] {
        // Recover from the raw bytes' trailing (y, r, s) using an independent RLP decode.
        use alloy_rlp::{Decodable, Header};
        let mut buf = &raw[1..];
        let h = Header::decode(&mut buf).unwrap();
        assert_eq!(h.payload_length, buf.len());
        let mut cur = buf;
        for _ in 0..8 {
            let hd = Header::decode(&mut cur).unwrap();
            cur = &cur[hd.payload_length..];
        }
        let al = Header::decode(&mut cur).unwrap();
        assert!(al.list);
        let y = u8::decode(&mut cur).unwrap();
        let r = Header::decode_bytes(&mut cur, false).unwrap().to_vec();
        let s = Header::decode_bytes(&mut cur, false).unwrap().to_vec();
        let mut r32 = [0u8; 32];
        r32[32 - r.len()..].copy_from_slice(&r);
        let mut s32 = [0u8; 32];
        s32[32 - s.len()..].copy_from_slice(&s);
        let sig = Signature::from_scalars(r32, s32).unwrap();
        assert!(sig.normalize_s().is_none(), "signature must be low-s");
        let vk = VerifyingKey::recover_from_prehash(digest, &sig, RecoveryId::new(y == 1, false)).unwrap();
        address_from_pubkey(vk.to_encoded_point(false).as_bytes()).unwrap()
    }

    #[test]
    fn sign_tx_produces_a_valid_low_s_signature_from_the_lane_address() {
        setup();
        host_tecdsa::enable();
        let tx = sample_tx();
        let (raw, hash) = block_on(sign_tx(LaneKind::Minter, &tx)).unwrap();
        assert_eq!(raw[0], 0x02);
        assert_eq!(hash, tx::tx_hash(&raw));
        let digest = tx::signing_hash(&tx);
        let lane = block_on(lane_address(LaneKind::Minter)).unwrap();
        assert_eq!(recover_address(&digest, &raw), lane);
        // The admin lane signs as a different address.
        let (raw_admin, _) = block_on(sign_tx(LaneKind::Admin, &tx)).unwrap();
        let admin = block_on(lane_address(LaneKind::Admin)).unwrap();
        assert_ne!(admin, lane);
        assert_eq!(recover_address(&digest, &raw_admin), admin);
        host_tecdsa::disable();
    }

    #[test]
    fn sign_tx_signs_the_exact_digest_with_the_lane_path() {
        setup();
        host_tecdsa::enable();
        let tx = sample_tx();
        block_on(sign_tx(LaneKind::Admin, &tx)).unwrap();
        let path = ids::derivation_path(LaneKind::Admin, CHAIN);
        // One derivation call plus one signing call, both with the admin path and key.
        assert_eq!(host_tecdsa::calls(), vec![("test_key_1".to_string(), path.clone()), ("test_key_1".to_string(), path)]);
        host_tecdsa::disable();
    }

    #[test]
    fn sign_tx_normalizes_a_high_s_signature_from_the_subnet() {
        setup();
        host_tecdsa::enable();
        host_tecdsa::set_return_high_s(true);
        let tx = sample_tx();
        let (raw_high, _) = block_on(sign_tx(LaneKind::Minter, &tx)).unwrap();
        host_tecdsa::set_return_high_s(false);
        let (raw_low, _) = block_on(sign_tx(LaneKind::Minter, &tx)).unwrap();
        // Same final transaction bytes either way, signed by the lane address.
        assert_eq!(raw_high, raw_low);
        let digest = tx::signing_hash(&tx);
        assert_eq!(recover_address(&digest, &raw_high), block_on(lane_address(LaneKind::Minter)).unwrap());
        host_tecdsa::disable();
    }

    #[test]
    fn sign_tx_refuses_a_wrong_chain_id() {
        setup();
        host_tecdsa::enable();
        let mut tx = sample_tx();
        tx.chain_id = 8453;
        let err = block_on(sign_tx(LaneKind::Minter, &tx)).unwrap_err();
        assert!(err.contains("chain id"), "{err}");
        assert!(host_tecdsa::calls().is_empty(), "no subnet call for a refused tx");
        host_tecdsa::disable();
    }

    #[test]
    fn signing_is_deterministic_per_input_but_never_cached() {
        setup();
        host_tecdsa::enable();
        let tx = sample_tx();
        let (a, _) = block_on(sign_tx(LaneKind::Minter, &tx)).unwrap();
        let calls_after_first = host_tecdsa::calls().len();
        let (b, _) = block_on(sign_tx(LaneKind::Minter, &tx)).unwrap();
        // The emulated subnet is RFC 6979 deterministic, so equal bytes are expected,
        // but the signer must have asked it again (no signature cache).
        assert_eq!(a, b);
        assert_eq!(host_tecdsa::calls().len(), calls_after_first + 1);
        // A different nonce is a different digest and a different transaction.
        let mut tx2 = tx.clone();
        tx2.nonce += 1;
        let (c, _) = block_on(sign_tx(LaneKind::Minter, &tx2)).unwrap();
        assert_ne!(a, c);
        host_tecdsa::disable();
    }

    #[test]
    fn derivation_notes_do_not_grow_the_audit_log_on_repeat() {
        setup();
        host_tecdsa::enable();
        block_on(ensure_lane_addresses()).unwrap();
        let n = audit_len();
        for _ in 0..3 {
            block_on(ensure_lane_addresses()).unwrap();
            block_on(sign_tx(LaneKind::Minter, &sample_tx())).unwrap();
        }
        assert_eq!(audit_len(), n);
        host_tecdsa::disable();
    }
}
