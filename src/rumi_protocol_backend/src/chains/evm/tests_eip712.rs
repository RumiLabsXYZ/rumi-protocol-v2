//! Unit tests for the EIP-712 self-serve auth primitives.
//!
//! The golden values below were produced by INDEPENDENT tools, so these tests
//! prove the Rust implementation matches the EIP-712 standard a real EVM wallet
//! (ethers/viem/MetaMask) uses — not merely self-consistency:
//!   - typehashes:   `cast keccak "<type string>"`
//!   - digest + sig: python `eth_account.Account` over the typed-data with the
//!     scalar=1 private key (`0x00..01`, address 0x7e5f…95bdf).

use super::eip712::*;
use crate::chains::config::ChainId;

const GOLDEN_OWNER: &str = "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf";
const GOLDEN_CONTRACT: &str = "0x00000000000000000000000000000000cf1c0de5";
const GOLDEN_DIGEST: &str = "0x76fe467010b364bc9ed7caf7153a42bdc924e1cb7bf223d8182d9537717b9adc";
const GOLDEN_SIG65: &str = "0x06f8ac987a3a020f6e25dbfc7634ebfd95f10ab9d657a2dcd91323506381f8b65e4a53a93ee2d35887b616a4c0efccdd6406b9c93fdfcdcf0b7cbe20c3b2127a1c";

fn hexb(s: &str) -> Vec<u8> {
    hex::decode(s.strip_prefix("0x").unwrap_or(s)).unwrap()
}

/// The exact intent the golden vector was generated for.
fn golden_intent() -> VaultIntent {
    VaultIntent {
        action: IntentAction::Open.as_u8(),
        chain_id: 71,
        owner: GOLDEN_OWNER.to_string(),
        vault_id: 0,
        collateral_wei: 1_400_000_000_000_000_000_000, // 1400 CFX
        debt_e8s: 10_000_000_000,                      // 100 icUSD
        recipient: GOLDEN_OWNER.to_string(),
        nonce: 0,
        deadline_secs: 9_999_999_999,
    }
}

#[test]
fn domain_typehash_matches_cast_keccak() {
    // cast keccak "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"
    assert_eq!(
        format!("0x{}", hex::encode(domain_typehash())),
        "0x8b73c3c69bb8fe3d512ecc4cf759cc79239f7b179b0ffacaa9a75d522b39400f"
    );
}

#[test]
fn intent_typehash_matches_cast_keccak() {
    // cast keccak "VaultIntent(uint8 action,uint64 chainId,address owner,uint64 vaultId,uint256 collateralWei,uint256 debtE8s,address recipient,uint256 nonce,uint256 deadline)"
    assert_eq!(
        format!("0x{}", hex::encode(intent_typehash())),
        "0x07f73e19bcdb683434be07c61e7b96781a58480279d7eb147175fb3bc38182f5"
    );
}

#[test]
fn digest_matches_eth_account_golden() {
    let dsep = domain_separator(71, GOLDEN_CONTRACT).unwrap();
    let sh = intent_struct_hash(&golden_intent()).unwrap();
    let digest = intent_digest(&dsep, &sh);
    assert_eq!(format!("0x{}", hex::encode(digest)), GOLDEN_DIGEST);
}

#[test]
fn recovers_eth_account_golden_signature() {
    let digest: [u8; 32] = hexb(GOLDEN_DIGEST).try_into().unwrap();
    let recovered = recover_evm_address(&digest, &hexb(GOLDEN_SIG65)).unwrap();
    assert_eq!(recovered, GOLDEN_OWNER);
}

#[test]
fn round_trip_sign_and_recover_with_k256() {
    use k256::ecdsa::{signature::hazmat::PrehashSigner, RecoveryId, Signature, SigningKey, VerifyingKey};
    use k256::elliptic_curve::sec1::ToEncodedPoint;
    let mut b = [0u8; 32];
    b[31] = 1;
    let sk = SigningKey::from_bytes(&b.into()).unwrap();
    let addr = crate::chains::evm::tecdsa::evm_address_from_pubkey(
        &VerifyingKey::from(&sk).to_encoded_point(false).as_bytes().to_vec(),
    )
    .unwrap();
    let dsep = domain_separator(71, GOLDEN_CONTRACT).unwrap();
    let intent = golden_intent();
    let digest = intent_digest(&dsep, &intent_struct_hash(&intent).unwrap());
    let (sig, rid): (Signature, RecoveryId) = sk.sign_prehash_recoverable(&digest).unwrap();
    let mut sig65 = sig.to_bytes().to_vec();
    sig65.push(27 + u8::from(rid));
    assert_eq!(recover_evm_address(&digest, &sig65).unwrap(), addr);
}

#[test]
fn wrong_contract_changes_digest() {
    let intent = golden_intent();
    let sh = intent_struct_hash(&intent).unwrap();
    let a = intent_digest(&domain_separator(71, GOLDEN_CONTRACT).unwrap(), &sh);
    let b = intent_digest(
        &domain_separator(71, "0x00000000000000000000000000000000deadbeef").unwrap(),
        &sh,
    );
    assert_ne!(a, b);
}

#[test]
fn wrong_chain_changes_digest() {
    let sh = intent_struct_hash(&golden_intent()).unwrap();
    let a = intent_digest(&domain_separator(71, GOLDEN_CONTRACT).unwrap(), &sh);
    let b = intent_digest(&domain_separator(10143, GOLDEN_CONTRACT).unwrap(), &sh);
    assert_ne!(a, b);
}

#[test]
fn tampered_field_breaks_owner_match() {
    use k256::ecdsa::{signature::hazmat::PrehashSigner, RecoveryId, Signature, SigningKey};
    let mut b = [0u8; 32];
    b[31] = 1;
    let sk = SigningKey::from_bytes(&b.into()).unwrap();
    let intent = golden_intent();
    let digest = intent_digest(
        &domain_separator(71, GOLDEN_CONTRACT).unwrap(),
        &intent_struct_hash(&intent).unwrap(),
    );
    let (sig, rid): (Signature, RecoveryId) = sk.sign_prehash_recoverable(&digest).unwrap();
    let mut sig65 = sig.to_bytes().to_vec();
    sig65.push(27 + u8::from(rid));
    // Recover against a DIFFERENT digest (tampered debt) → not the owner.
    let mut tampered = intent.clone();
    tampered.debt_e8s += 1;
    let d2 = intent_digest(
        &domain_separator(71, GOLDEN_CONTRACT).unwrap(),
        &intent_struct_hash(&tampered).unwrap(),
    );
    assert_ne!(recover_evm_address(&d2, &sig65).unwrap(), GOLDEN_OWNER);
}

#[test]
fn bad_recovery_byte_rejected() {
    let digest: [u8; 32] = hexb(GOLDEN_DIGEST).try_into().unwrap();
    let mut sig = hexb(GOLDEN_SIG65);
    sig[64] = 42; // invalid v
    assert!(recover_evm_address(&digest, &sig).is_err());
}

#[test]
fn synthetic_owner_is_opaque_deterministic_and_distinct() {
    let p1 = synthetic_owner(ChainId(71), GOLDEN_OWNER).unwrap();
    // Case-insensitive: a checksummed address yields the same principal.
    let p2 = synthetic_owner(ChainId(71), "0x7E5F4552091a69125d5DFcb7b8C2659029395Bdf").unwrap();
    assert_eq!(p1, p2);
    let bytes = p1.as_slice();
    assert_eq!(bytes.len(), 29);
    assert_eq!(bytes[28], 0x01, "opaque class tag");
    assert_ne!(bytes[28], 0x02, "must never be a self-authenticating principal");
    // Distinct per chain (same address on Monad differs).
    assert_ne!(p1, synthetic_owner(ChainId(10143), GOLDEN_OWNER).unwrap());
}

// ─── pure verify_intent (the rejection paths) ─────────────────────────────────

/// Sign `intent` for `contract` with the fixed scalar=1 key → 65-byte sig.
fn sign_for(intent: &VaultIntent, contract: &str) -> Vec<u8> {
    use k256::ecdsa::{signature::hazmat::PrehashSigner, RecoveryId, Signature, SigningKey};
    let mut b = [0u8; 32];
    b[31] = 1;
    let sk = SigningKey::from_bytes(&b.into()).unwrap();
    let digest = intent_digest(
        &domain_separator(intent.chain_id, contract).unwrap(),
        &intent_struct_hash(intent).unwrap(),
    );
    let (sig, rid): (Signature, RecoveryId) = sk.sign_prehash_recoverable(&digest).unwrap();
    let mut out = sig.to_bytes().to_vec();
    out.push(27 + u8::from(rid));
    out
}

#[test]
fn verify_intent_happy_path_returns_owner_and_synthetic() {
    let intent = golden_intent();
    let sig = sign_for(&intent, GOLDEN_CONTRACT);
    let (owner, synthetic) =
        verify_intent(&intent, &sig, IntentAction::Open, GOLDEN_CONTRACT, 1000).unwrap();
    assert_eq!(owner, GOLDEN_OWNER);
    assert_eq!(synthetic, synthetic_owner(ChainId(71), GOLDEN_OWNER).unwrap());
}

#[test]
fn verify_intent_rejects_action_mismatch() {
    let intent = golden_intent(); // action = Open
    let sig = sign_for(&intent, GOLDEN_CONTRACT);
    // Endpoint expects Borrow, intent is Open.
    assert_eq!(
        verify_intent(&intent, &sig, IntentAction::Borrow, GOLDEN_CONTRACT, 1000),
        Err(VerifyError::ActionMismatch)
    );
}

#[test]
fn verify_intent_rejects_expired_deadline() {
    let mut intent = golden_intent();
    intent.deadline_secs = 500;
    let sig = sign_for(&intent, GOLDEN_CONTRACT);
    assert_eq!(
        verify_intent(&intent, &sig, IntentAction::Open, GOLDEN_CONTRACT, 501),
        Err(VerifyError::Expired)
    );
}

#[test]
fn verify_intent_rejects_wrong_signer() {
    // A signature over a DIFFERENT contract recovers a signer that won't match
    // the intent's owner once we verify against the real contract domain.
    let intent = golden_intent();
    let sig = sign_for(&intent, "0x00000000000000000000000000000000deadbeef");
    assert_eq!(
        verify_intent(&intent, &sig, IntentAction::Open, GOLDEN_CONTRACT, 1000),
        Err(VerifyError::SignerMismatch)
    );
}

#[test]
fn verify_intent_rejects_recipient_not_owner() {
    let mut intent = golden_intent();
    intent.recipient = "0x00000000000000000000000000000000deadbeef".into();
    let sig = sign_for(&intent, GOLDEN_CONTRACT);
    assert_eq!(
        verify_intent(&intent, &sig, IntentAction::Open, GOLDEN_CONTRACT, 1000),
        Err(VerifyError::RecipientNotOwner)
    );
}

#[test]
fn verify_intent_rejects_tampered_intent() {
    // Sign the original, then tamper the debt — the recovered signer no longer
    // matches owner (the signature is over the un-tampered digest).
    let intent = golden_intent();
    let sig = sign_for(&intent, GOLDEN_CONTRACT);
    let mut tampered = intent.clone();
    tampered.debt_e8s += 1;
    assert_eq!(
        verify_intent(&tampered, &sig, IntentAction::Open, GOLDEN_CONTRACT, 1000),
        Err(VerifyError::SignerMismatch)
    );
}

#[test]
fn verify_intent_rejects_chain_id_above_u32() {
    // chain_id is hashed as u64 but resolved as u32; a value > u32::MAX must be
    // rejected so the digest-bound chain cannot diverge from the resolved chain.
    let mut intent = golden_intent();
    intent.chain_id = (u32::MAX as u64) + 1;
    let sig = sign_for(&intent, GOLDEN_CONTRACT);
    assert!(matches!(
        verify_intent(&intent, &sig, IntentAction::Open, GOLDEN_CONTRACT, 1000),
        Err(VerifyError::Recover(_))
    ));
}

// ─── secp256k1 high-s malleability ─────────────────────────────────────────────

/// secp256k1 group order `n`, big-endian. Used to construct the classic ECDSA
/// signature-malleability companion `(r, n - s, flipped v)` of a valid
/// signature `(r, s, v)`.
const SECP256K1_ORDER: [u8; 32] = [
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xfe, 0xba, 0xae, 0xdc, 0xe6, 0xaf, 0x48, 0xa0, 0x3b, 0xbf, 0xd2, 0x5e, 0x8c, 0xd0, 0x36,
    0x41, 0x41,
];

/// `n - s`, exact for any `0 < s < n` (true of any `s` a real ECDSA signature
/// carries), via plain big-endian subtraction with borrow (no modular
/// wraparound needed since `s < n` always holds here).
fn negate_s_mod_n(s: [u8; 32]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let mut borrow: i16 = 0;
    for i in (0..32).rev() {
        let mut diff = SECP256K1_ORDER[i] as i16 - s[i] as i16 - borrow;
        if diff < 0 {
            diff += 256;
            borrow = 1;
        } else {
            borrow = 0;
        }
        out[i] = diff as u8;
    }
    out
}

/// Finding 3 (secp256k1 high-s malleability) regression, from the M2 security
/// close-out.
///
/// The audit question was whether `recover_evm_address` (`eip712.rs`) accepts
/// the classic ECDSA malleability companion `(r, n - s, flipped v)` of a valid
/// signature `(r, s, v)`, and if so whether `(synthetic_owner, nonce)`
/// consumption alone is what neutralizes a malleated resubmit. EMPIRICALLY,
/// it is narrower than that: `recover_evm_address` calls `k256`'s
/// `VerifyingKey::recover_from_prehash`, whose LAST step re-verifies the
/// recovered key against the input signature via `AffinePoint::verify_prehashed`
/// (k256 0.13.4, `src/ecdsa.rs`, the `VerifyPrimitive` impl), and THAT
/// function unconditionally rejects any signature whose `s` is high
/// (`if sig.s().is_high() { return Err(..) }`) before doing any curve math.
/// So a genuinely malleated `(r, n-s, flipped v)` signature is rejected by
/// `recover_evm_address` OUTRIGHT (asserted in part 1); it never reaches
/// `verify_intent`'s signer check, let alone nonce consumption. This is a
/// STRONGER guarantee than the nonce-binding fallback the audit question
/// assumed. It also means the earlier (accurate) half of the finding, that
/// `Signature::from_scalars` itself performs no high-S check, is true but
/// not exploitable end-to-end, because the check lives one call further in,
/// in the recovery path's internal consistency re-verify.
///
/// Part 2 independently pins the OTHER layer the finding asked about: exact
/// signature replay (not malleation) is separately blocked by
/// `MultiChainState::consume_evm_nonce`, so even if some future dependency
/// bump ever relaxed k256's high-S rejection, a resubmitted intent (malleated
/// or byte-identical) still cannot authorize a second state transition once
/// its nonce is spent.
///
/// Both properties hold with `eip712.rs` and `multi_chain_state.rs`
/// UNCHANGED; this test is the deliverable.
#[test]
fn high_s_malleated_signature_is_rejected_and_nonce_replay_is_independently_blocked() {
    use k256::ecdsa::{RecoveryId, Signature, SigningKey};

    let mut b = [0u8; 32];
    b[31] = 1;
    let sk = SigningKey::from_bytes(&b.into()).unwrap();

    let intent = golden_intent(); // nonce = 0
    let digest = intent_digest(
        &domain_separator(intent.chain_id, GOLDEN_CONTRACT).unwrap(),
        &intent_struct_hash(&intent).unwrap(),
    );
    let (sig, rid): (Signature, RecoveryId) = sk.sign_prehash_recoverable(&digest).unwrap();
    let sig_bytes = sig.to_bytes();
    let r: [u8; 32] = sig_bytes[0..32].try_into().unwrap();
    let s: [u8; 32] = sig_bytes[32..64].try_into().unwrap();
    let v = 27 + u8::from(rid);

    let mut original_sig65 = Vec::with_capacity(65);
    original_sig65.extend_from_slice(&r);
    original_sig65.extend_from_slice(&s);
    original_sig65.push(v);

    // The malleated companion: same r, s -> n - s, v flipped between 27/28.
    // `sk.sign_prehash_recoverable` always yields a LOW-S signature (k256
    // normalizes on signing), so this negation genuinely produces a HIGH-S
    // signature, the case under test.
    let malleated_s = negate_s_mod_n(s);
    assert_ne!(malleated_s, s, "sanity: negation actually changed s");
    let malleated_v = if v == 27 { 28 } else { 27 };
    let mut malleated_sig65 = Vec::with_capacity(65);
    malleated_sig65.extend_from_slice(&r);
    malleated_sig65.extend_from_slice(&malleated_s);
    malleated_sig65.push(malleated_v);

    // 1. The original (low-S) signature recovers the real owner; the
    //    malleated (high-S) companion is REJECTED outright by
    //    `recover_evm_address`, for EITHER choice of recovery byte (rules out
    //    "wrong parity" as the cause and confirms it is the high-S rejection).
    let owner_original = recover_evm_address(&digest, &original_sig65).unwrap();
    assert_eq!(owner_original, GOLDEN_OWNER);
    assert!(
        recover_evm_address(&digest, &malleated_sig65).is_err(),
        "a high-S malleated signature must be rejected outright, not merely neutralized downstream"
    );
    for try_v in [27u8, 28u8] {
        let mut trial = Vec::with_capacity(65);
        trial.extend_from_slice(&r);
        trial.extend_from_slice(&malleated_s);
        trial.push(try_v);
        assert!(
            recover_evm_address(&digest, &trial).is_err(),
            "high-S rejection holds regardless of the recovery byte (v={try_v})"
        );
    }
    // `verify_intent` (which calls `recover_evm_address` internally) rejects
    // the malleated form too, for the same reason: it never gets past the
    // ecrecover step to reach the signer-match check.
    assert!(matches!(
        verify_intent(&intent, &malleated_sig65, IntentAction::Open, GOLDEN_CONTRACT, 1000),
        Err(VerifyError::Recover(_))
    ));

    // 2. Independent layer: exact-signature nonce replay is ALSO blocked, so
    //    the resubmit is neutralized twice over even if a future dependency
    //    change ever relaxed part 1's high-S rejection. Consuming a nonce
    //    twice (same owner, same nonce) is rejected regardless of which
    //    signature carried the first submission.
    let (_owner, synthetic) =
        verify_intent(&intent, &original_sig65, IntentAction::Open, GOLDEN_CONTRACT, 1000)
            .expect("original signature verifies");
    let mut state = crate::chains::multi_chain_state::MultiChainState::default();
    state
        .consume_evm_nonce(&synthetic, intent.nonce)
        .expect("first submission (nonce 0) consumes cleanly");
    let resubmit = state.consume_evm_nonce(&synthetic, intent.nonce);
    assert_eq!(
        resubmit,
        Err(1),
        "a nonce-0 resubmit after nonce 0 is already spent must be rejected (expected next = 1), \
         producing no second authorized state transition"
    );
}
