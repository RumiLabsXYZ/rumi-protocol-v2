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
