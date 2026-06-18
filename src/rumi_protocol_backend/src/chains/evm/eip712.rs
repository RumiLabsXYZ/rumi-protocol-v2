//! EIP-712 typed-data intents for EVM-native self-serve vault auth (M2).
//!
//! The canister is the verifier: a user signs a `VaultIntent` in their EVM
//! wallet, and the canister recomputes the digest, recovers the signer, and
//! acts. There is NO on-chain verifying contract; the IcUSD contract address
//! merely binds the EIP-712 domain to a specific chain + deployment so a
//! signature cannot be replayed across chains (71 vs 10143) or deployments
//! (staging kvg63 vs mainnet IcUSD addresses differ). The signer principal is
//! NEVER the IC caller — authenticity is the EVM signature alone.

use crate::chains::config::ChainId;
use candid::{CandidType, Deserialize, Principal};
use serde::Serialize;
use sha3::{Digest, Keccak256};

/// Domain name + version, frozen into the EIP-712 domain separator.
pub const DOMAIN_NAME: &str = "Rumi icUSD CDP";
pub const DOMAIN_VERSION: &str = "1";

/// The four vault operations a `VaultIntent` can authorize. The numeric value is
/// the on-wire `uint8 action` field hashed into the struct (do NOT renumber).
#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntentAction {
    Open,               // 0
    Borrow,             // 1
    WithdrawCollateral, // 2
    Close,              // 3
}

impl IntentAction {
    pub fn as_u8(self) -> u8 {
        match self {
            IntentAction::Open => 0,
            IntentAction::Borrow => 1,
            IntentAction::WithdrawCollateral => 2,
            IntentAction::Close => 3,
        }
    }
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(IntentAction::Open),
            1 => Some(IntentAction::Borrow),
            2 => Some(IntentAction::WithdrawCollateral),
            3 => Some(IntentAction::Close),
            _ => None,
        }
    }
}

/// The signed intent. Candid mirror: `action` is a `nat8`, addresses are
/// lowercase `0x` `text`, amounts `nat`, nonce/deadline `nat64`.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct VaultIntent {
    pub action: u8,
    pub chain_id: u64,
    /// Claimed EVM owner; MUST equal the recovered signer.
    pub owner: String,
    pub vault_id: u64,
    /// Open: declared collateral (wei). Withdraw: amount to release (wei). Else 0.
    pub collateral_wei: u128,
    /// Open: initial debt (e8s). Borrow: additional debt (e8s). Else 0.
    pub debt_e8s: u128,
    /// Mint recipient (open/borrow) or collateral destination (withdraw/close).
    /// Enforced `== owner` in M2.
    pub recipient: String,
    pub nonce: u64,
    pub deadline_secs: u64,
}

/// `keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)")`
pub fn domain_typehash() -> [u8; 32] {
    Keccak256::digest(
        b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
    )
    .into()
}

/// `keccak256("VaultIntent(uint8 action,uint64 chainId,address owner,uint64 vaultId,uint256 collateralWei,uint256 debtE8s,address recipient,uint256 nonce,uint256 deadline)")`
pub fn intent_typehash() -> [u8; 32] {
    Keccak256::digest(
        b"VaultIntent(uint8 action,uint64 chainId,address owner,uint64 vaultId,uint256 collateralWei,uint256 debtE8s,address recipient,uint256 nonce,uint256 deadline)",
    )
    .into()
}

/// 32-byte big-endian word for a u128 (left-padded). Identical to the uint256
/// ABI encoding of the same value.
fn word_u128(n: u128) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[16..].copy_from_slice(&n.to_be_bytes());
    w
}

/// 32-byte big-endian word for a u64. Identical to the uint8/uint64/uint256 ABI
/// encoding of the same value (all widen to one 32-byte word).
fn word_u64(n: u64) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[24..].copy_from_slice(&n.to_be_bytes());
    w
}

/// 32-byte word for a 20-byte address (right-aligned). Err on bad hex/length.
fn word_address(addr: &str) -> Result<[u8; 32], String> {
    let bytes = parse_addr_20(addr)?;
    let mut w = [0u8; 32];
    w[12..].copy_from_slice(&bytes);
    Ok(w)
}

/// Parse a `0x`-prefixed 20-byte EVM address into raw bytes (canonical key input).
pub fn parse_addr_20(addr: &str) -> Result<[u8; 20], String> {
    let h = addr.strip_prefix("0x").or_else(|| addr.strip_prefix("0X")).unwrap_or(addr);
    let v = hex::decode(h).map_err(|e| format!("bad address hex: {e}"))?;
    v.try_into().map_err(|v: Vec<u8>| format!("address is {} bytes, expected 20", v.len()))
}

/// `domainSeparator = keccak256(abi.encode(DOMAIN_TYPEHASH, keccak(name), keccak(version), chainId, verifyingContract))`
pub fn domain_separator(chain_id: u64, verifying_contract: &str) -> Result<[u8; 32], String> {
    let mut buf = Vec::with_capacity(32 * 5);
    buf.extend_from_slice(&domain_typehash());
    buf.extend_from_slice(&<[u8; 32]>::from(Keccak256::digest(DOMAIN_NAME.as_bytes())));
    buf.extend_from_slice(&<[u8; 32]>::from(Keccak256::digest(DOMAIN_VERSION.as_bytes())));
    buf.extend_from_slice(&word_u64(chain_id));
    buf.extend_from_slice(&word_address(verifying_contract)?);
    Ok(Keccak256::digest(&buf).into())
}

/// `hashStruct(intent) = keccak256(abi.encode(INTENT_TYPEHASH, ...fields...))`.
pub fn intent_struct_hash(intent: &VaultIntent) -> Result<[u8; 32], String> {
    let mut buf = Vec::with_capacity(32 * 10);
    buf.extend_from_slice(&intent_typehash());
    buf.extend_from_slice(&word_u64(intent.action as u64));
    buf.extend_from_slice(&word_u64(intent.chain_id));
    buf.extend_from_slice(&word_address(&intent.owner)?);
    buf.extend_from_slice(&word_u64(intent.vault_id));
    buf.extend_from_slice(&word_u128(intent.collateral_wei));
    buf.extend_from_slice(&word_u128(intent.debt_e8s));
    buf.extend_from_slice(&word_address(&intent.recipient)?);
    buf.extend_from_slice(&word_u64(intent.nonce));
    buf.extend_from_slice(&word_u64(intent.deadline_secs));
    Ok(Keccak256::digest(&buf).into())
}

/// `digest = keccak256(0x19 ‖ 0x01 ‖ domainSeparator ‖ hashStruct)`.
pub fn intent_digest(domain_sep: &[u8; 32], struct_hash: &[u8; 32]) -> [u8; 32] {
    let mut buf = Vec::with_capacity(2 + 64);
    buf.push(0x19);
    buf.push(0x01);
    buf.extend_from_slice(domain_sep);
    buf.extend_from_slice(struct_hash);
    Keccak256::digest(&buf).into()
}

/// Recover the lowercase `0x` EVM signer address from a 65-byte signature over a
/// 32-byte prehash digest. Accepts `v ∈ {0,1,27,28}`.
pub fn recover_evm_address(digest: &[u8; 32], sig65: &[u8]) -> Result<String, String> {
    use super::tecdsa::evm_address_from_pubkey;
    use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};

    if sig65.len() != 65 {
        return Err(format!("signature must be 65 bytes, got {}", sig65.len()));
    }
    let r: [u8; 32] = sig65[0..32].try_into().unwrap();
    let s: [u8; 32] = sig65[32..64].try_into().unwrap();
    let parity = match sig65[64] {
        0 | 27 => 0u8,
        1 | 28 => 1u8,
        v => return Err(format!("bad recovery byte v={v}")),
    };
    let sig = Signature::from_scalars(r, s).map_err(|e| format!("bad (r,s): {e}"))?;
    let rid = RecoveryId::new(parity == 1, false);
    let vk = VerifyingKey::recover_from_prehash(digest, &sig, rid)
        .map_err(|e| format!("ecrecover failed: {e}"))?;
    let pk = vk.to_encoded_point(false).as_bytes().to_vec();
    evm_address_from_pubkey(&pk)
}

/// Deterministic opaque-class synthetic owner principal for an EVM address on a
/// chain. `keccak256("rumi.evm.owner.v1:" ‖ chain_le ‖ addr20)[0..28] ‖ 0x01`.
/// The trailing `0x01` is the opaque type tag, so this can never equal a
/// self-authenticating (trailing `0x02`) user principal. Internal owner key only
/// — it is never used as a caller identity nor authenticated against.
pub fn synthetic_owner(chain: ChainId, evm_addr: &str) -> Result<Principal, String> {
    let addr20 = parse_addr_20(evm_addr)?;
    let mut hasher = Keccak256::new();
    hasher.update(b"rumi.evm.owner.v1:");
    hasher.update(chain.0.to_le_bytes());
    hasher.update(addr20);
    let h: [u8; 32] = hasher.finalize().into();
    let mut bytes = Vec::with_capacity(29);
    bytes.extend_from_slice(&h[0..28]);
    bytes.push(0x01);
    Ok(Principal::from_slice(&bytes))
}
