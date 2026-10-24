//! Identifiers, ledger memos, subaccounts and threshold-ECDSA derivation paths.
//!
//! Every identifier that crosses a trust boundary carries an explicit domain tag so
//! a value produced for one purpose can never be accepted for another:
//!
//! - Base mint ids for deposits and for held-claim re-mints use different domains.
//! - Bridge-out transfer ids are computed by the Base contract under its own domain
//!   (`rumi.icusd.base.bridge-out.v1`), so they cannot collide with mint ids.
//! - Ledger memos carry a purpose prefix.
//! - Derivation paths start with a bridge-specific root and a purpose label. They are
//!   also bound to this canister's principal by the IC, so they can never equal any
//!   address derived by `rumi_protocol_backend`.

use sha2::{Digest as _, Sha256};
use sha3::Keccak256;

/// Contract version tag the canister requires before binding (`VERSION()`).
pub const CONTRACT_VERSION: &str = "rumi-base-icusd-bridge-v1";

/// Domain string hashed into deposit mint ids.
pub const MINT_DOMAIN: &str = "rumi.icusd.base.mint.v1";
/// Domain string hashed into held-claim re-mint ids.
pub const REMINT_DOMAIN: &str = "rumi.icusd.base.remint.v1";
/// Domain string the contract hashes into bridge-out transfer ids.
pub const BRIDGE_OUT_DOMAIN: &str = "rumi.icusd.base.bridge-out.v1";

/// Label hashed (SHA-256) into the escrow subaccount.
pub const ESCROW_SUBACCOUNT_LABEL: &[u8] = b"rumi-icusd-base-bridge:escrow:v1";

/// Root segment of every threshold-ECDSA derivation path used by this canister.
pub const KEY_PATH_ROOT: &[u8] = b"rumi-icusd-base-bridge";
/// Version segment of every derivation path.
pub const KEY_PATH_VERSION: &[u8] = b"v1";

/// Memo prefixes (8 bytes each) for ledger operations.
pub const PULL_MEMO_PREFIX: &[u8; 8] = b"RBRG-DEP";
pub const REFUND_MEMO_PREFIX: &[u8; 8] = b"RBRG-RFD";
pub const SWEEP_MEMO_PREFIX: &[u8; 8] = b"RBRG-FEE";

/// The 32-byte escrow subaccount of the bridge canister.
pub fn escrow_subaccount() -> [u8; 32] {
    Sha256::digest(ESCROW_SUBACCOUNT_LABEL).into()
}

pub fn keccak256(data: &[u8]) -> [u8; 32] {
    Keccak256::digest(data).into()
}

/// ABI word for a `uint256` holding a `u64`.
pub fn word_u64(v: u64) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[24..].copy_from_slice(&v.to_be_bytes());
    w
}

/// ABI word for a `uint256` holding a `u128`.
pub fn word_u128(v: u128) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[16..].copy_from_slice(&v.to_be_bytes());
    w
}

/// ABI word for an `address`.
pub fn word_address(a: &[u8; 20]) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[12..].copy_from_slice(a);
    w
}

fn abi_hash(words: &[[u8; 32]]) -> [u8; 32] {
    let mut buf = Vec::with_capacity(words.len() * 32);
    for w in words {
        buf.extend_from_slice(w);
    }
    keccak256(&buf)
}

/// `keccak256(abi.encode(keccak256(MINT_DOMAIN), chain_id, contract, instance_salt, deposit_id))`.
pub fn mint_id(chain_id: u64, contract: &[u8; 20], instance_salt: &[u8; 32], deposit_id: u64) -> [u8; 32] {
    abi_hash(&[
        keccak256(MINT_DOMAIN.as_bytes()),
        word_u64(chain_id),
        word_address(contract),
        *instance_salt,
        word_u64(deposit_id),
    ])
}

/// `keccak256(abi.encode(keccak256(REMINT_DOMAIN), chain_id, contract, instance_salt, transfer_id))`.
pub fn remint_id(chain_id: u64, contract: &[u8; 20], instance_salt: &[u8; 32], transfer_id: &[u8; 32]) -> [u8; 32] {
    abi_hash(&[
        keccak256(REMINT_DOMAIN.as_bytes()),
        word_u64(chain_id),
        word_address(contract),
        *instance_salt,
        *transfer_id,
    ])
}

/// The transfer id the contract assigns: `keccak256(abi.encode(keccak256(BRIDGE_OUT_DOMAIN),
/// chainid, contract, from, clientNonce))`. Used by tests and by the frontend vector check.
pub fn bridge_out_transfer_id(chain_id: u64, contract: &[u8; 20], from: &[u8; 20], client_nonce: &[u8; 32]) -> [u8; 32] {
    abi_hash(&[
        keccak256(BRIDGE_OUT_DOMAIN.as_bytes()),
        word_u64(chain_id),
        word_address(contract),
        word_address(from),
        *client_nonce,
    ])
}

fn prefixed_memo(prefix: &[u8; 8], id: u64) -> Vec<u8> {
    let mut m = Vec::with_capacity(16);
    m.extend_from_slice(prefix);
    m.extend_from_slice(&id.to_be_bytes());
    m
}

pub fn pull_memo(deposit_id: u64) -> Vec<u8> {
    prefixed_memo(PULL_MEMO_PREFIX, deposit_id)
}

pub fn refund_memo(deposit_id: u64) -> Vec<u8> {
    prefixed_memo(REFUND_MEMO_PREFIX, deposit_id)
}

pub fn sweep_memo(sweep_id: u64) -> Vec<u8> {
    prefixed_memo(SWEEP_MEMO_PREFIX, sweep_id)
}

/// Release memo is the 32-byte transfer id itself.
pub fn release_memo(transfer_id: &[u8; 32]) -> Vec<u8> {
    transfer_id.to_vec()
}

/// Which signing identity a Base transaction uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize, candid::CandidType)]
pub enum LaneKind {
    /// Holds `MINTER_ROLE` on the Base contract.
    Minter,
    /// Holds `DEFAULT_ADMIN_ROLE` on the Base contract.
    Admin,
}

impl LaneKind {
    pub fn label(&self) -> &'static [u8] {
        match self {
            LaneKind::Minter => b"minter",
            LaneKind::Admin => b"admin",
        }
    }
}

/// `[KEY_PATH_ROOT, "v1", purpose, chain_id (8 bytes BE)]`.
pub fn derivation_path(lane: LaneKind, chain_id: u64) -> Vec<Vec<u8>> {
    vec![
        KEY_PATH_ROOT.to_vec(),
        KEY_PATH_VERSION.to_vec(),
        lane.label().to_vec(),
        chain_id.to_be_bytes().to_vec(),
    ]
}

/// Lowercase `0x`-prefixed hex.
pub fn hex0x(bytes: &[u8]) -> String {
    format!("0x{}", hex::encode(bytes))
}

/// Parse a `0x`-prefixed 20-byte address (case-insensitive, no checksum enforcement).
pub fn parse_address(s: &str) -> Result<[u8; 20], String> {
    let raw = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).ok_or("address must start with 0x")?;
    if raw.len() != 40 {
        return Err(format!("address must be 20 bytes, got {} hex chars", raw.len()));
    }
    let bytes = hex::decode(raw).map_err(|e| format!("invalid address hex: {e}"))?;
    let mut out = [0u8; 20];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// Parse a `0x`-prefixed 32-byte value.
pub fn parse_bytes32(s: &str) -> Result<[u8; 32], String> {
    let raw = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).ok_or("value must start with 0x")?;
    if raw.len() != 64 {
        return Err(format!("value must be 32 bytes, got {} hex chars", raw.len()));
    }
    let bytes = hex::decode(raw).map_err(|e| format!("invalid hex: {e}"))?;
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> [u8; 32] {
        parse_bytes32(s).unwrap()
    }

    #[test]
    fn bridge_out_domain_matches_contract_constant() {
        // Pinned by foundry/test/BridgedIcUSD.t.sol (BRIDGE_OUT_DOMAIN).
        assert_eq!(
            keccak256(BRIDGE_OUT_DOMAIN.as_bytes()),
            h("0xbf6555f205691a2cd4c2403b214c0d516fdb66c929882e857755b63e90f1d4f1")
        );
    }

    #[test]
    fn transfer_id_matches_contract_vector() {
        // foundry/test/BridgedIcUSD.t.sol test_transfer_id_pinned_vector.
        let contract = parse_address("0x00000000000000000000000000000000000B45E1").unwrap();
        let from = parse_address("0x00000000000000000000000000000000000A11CE").unwrap();
        let mut nonce = [0u8; 32];
        nonce[31] = 1;
        assert_eq!(
            bridge_out_transfer_id(8453, &contract, &from, &nonce),
            h("0xf2f74fbfe0e8d729020d775d796ee6ff86aa29273d4873986667fde1c136336c")
        );
    }

    #[test]
    fn mint_and_remint_ids_are_domain_separated() {
        let contract = [7u8; 20];
        let salt = [9u8; 32];
        let a = mint_id(8453, &contract, &salt, 1);
        let b = mint_id(8453, &contract, &salt, 2);
        let c = mint_id(84532, &contract, &salt, 1);
        let d = mint_id(8453, &[8u8; 20], &salt, 1);
        let e = mint_id(8453, &contract, &[1u8; 32], 1);
        let mut tid = [0u8; 32];
        tid[24..].copy_from_slice(&1u64.to_be_bytes());
        let r = remint_id(8453, &contract, &salt, &tid);
        let all = [a, b, c, d, e, r];
        for i in 0..all.len() {
            for j in (i + 1)..all.len() {
                assert_ne!(all[i], all[j], "ids {i} and {j} collide");
            }
        }
        // A re-mint id for a transfer id equal to the deposit-id word still differs.
        assert_ne!(r, a);
    }

    #[test]
    fn memos_are_prefixed_and_distinct() {
        assert_eq!(pull_memo(5).len(), 16);
        assert_ne!(pull_memo(5), refund_memo(5));
        assert_ne!(pull_memo(5), sweep_memo(5));
        assert_eq!(&pull_memo(0x0102)[..8], b"RBRG-DEP");
        assert_eq!(&pull_memo(0x0102)[8..], &0x0102u64.to_be_bytes());
        assert_eq!(release_memo(&[3u8; 32]).len(), 32);
    }

    #[test]
    fn derivation_paths_are_purpose_and_chain_separated() {
        assert_ne!(derivation_path(LaneKind::Minter, 8453), derivation_path(LaneKind::Admin, 8453));
        assert_ne!(derivation_path(LaneKind::Minter, 8453), derivation_path(LaneKind::Minter, 84532));
        let p = derivation_path(LaneKind::Minter, 8453);
        assert_eq!(p[0], KEY_PATH_ROOT.to_vec());
        assert_eq!(p[3], 8453u64.to_be_bytes().to_vec());
    }

    #[test]
    fn escrow_subaccount_is_stable() {
        // Changing the label would orphan escrow. Pin it.
        assert_eq!(escrow_subaccount(), <[u8; 32]>::from(Sha256::digest(b"rumi-icusd-base-bridge:escrow:v1")));
        assert_ne!(escrow_subaccount(), [0u8; 32]);
    }

    #[test]
    fn address_parsing_is_strict() {
        assert!(parse_address("0x0000000000000000000000000000000000000001").is_ok());
        assert!(parse_address("0000000000000000000000000000000000000001").is_err());
        assert!(parse_address("0x00000000000000000000000000000000000001").is_err());
        assert!(parse_address("0xzz00000000000000000000000000000000000001").is_err());
    }
}
