//! Network identity, role key paths and the id domains for the Sui bridge instance
//! (`docs/coordination/multichain/chain-sui.md` sections 2 and 3; profile
//! `rumi_bridge_destinations/profiles/sui.json`; the pattern is the frozen Solana instance's
//! `ids.rs` at 23dfe841).
//!
//! Every identifier that crosses a trust boundary carries an explicit domain tag, so a value
//! produced for one purpose can never be accepted for another. The Sui hash for canister-side
//! ids is `sha256(domain_bytes || (u32_be(len(field)) || field)*)`, integers 8-byte big-endian,
//! exactly the Solana shape. The one exception is the return `transfer_id`, which the Move gate
//! computes itself (sha2-256 over BCS, see [`transfer_id`]); the canister recomputes it so both
//! share golden vectors with the accepted Move package at 23f75ef1.
//!
//! # Network identity
//!
//! A network is identified by its 32-byte genesis checkpoint digest (what a transaction's
//! `chain` field and the Gate's `chain_identifier` use). Only two are accepted, taken from the
//! base58 constants in `sui-types` `digests.rs` at mainnet-v1.82.3 and decoded by hand
//! (`MAINNET_CHAIN_IDENTIFIER_BASE58`, `TESTNET_CHAIN_IDENTIFIER_BASE58`); the decoded
//! four-byte prefixes `35834a8a` and `4c78adac` match JSON-RPC's chain identifiers (GraphQL
//! reports the full Base58 digest). Anything else
//! is refused rather than guessed.

use crate::sui::{short_chain_id, Bytes32};
use candid::{CandidType, Deserialize};
use serde::Serialize;
use sha2::{Digest as _, Sha256};

/// Mainnet genesis checkpoint digest (`4btiuiMPvEENsttpZC7CZ53DruC3MAgfznDbASZ7DR6S`).
pub const MAINNET_CHAIN_IDENTIFIER: Bytes32 = [
    0x35, 0x83, 0x4a, 0x8a, 0xc1, 0x7c, 0xa4, 0x8f, 0xb1, 0x4a, 0xc8, 0xf9, 0x9c, 0x17, 0xc9, 0x87, 0x47, 0xe9, 0x5d, 0xd0, 0x72, 0x94,
    0xae, 0x41, 0xa4, 0x6b, 0x38, 0x22, 0x46, 0xa4, 0x49, 0x9b,
];
/// Testnet genesis checkpoint digest (`69WiPg3DAQiwdxfncX6wYQ2siKwAe6L9BZthQea3JNMD`).
pub const TESTNET_CHAIN_IDENTIFIER: Bytes32 = [
    0x4c, 0x78, 0xad, 0xac, 0xf2, 0xa2, 0xf5, 0xad, 0x80, 0xf2, 0x7e, 0xd7, 0xd5, 0x4a, 0xa6, 0x9d, 0x3a, 0x78, 0xf1, 0xca, 0x67, 0xfd,
    0xef, 0x9e, 0xcf, 0x57, 0x54, 0xf5, 0xb8, 0xbb, 0x77, 0xb0,
];

/// Threshold-Schnorr key names (profile `signing.key_name`): `key_1` on mainnet, `test_key_1`
/// on everything else.
pub const MAINNET_KEY_NAME: &str = "key_1";
pub const TEST_KEY_NAME: &str = "test_key_1";

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuiNetwork {
    Mainnet,
    Testnet,
}

impl SuiNetwork {
    /// The one genesis checkpoint digest this network may use.
    pub fn chain_identifier(&self) -> Bytes32 {
        match self {
            SuiNetwork::Mainnet => MAINNET_CHAIN_IDENTIFIER,
            SuiNetwork::Testnet => TESTNET_CHAIN_IDENTIFIER,
        }
    }

    /// The RPC form of the chain identifier (first four bytes, hex).
    pub fn short_chain_id(&self) -> String {
        short_chain_id(&self.chain_identifier())
    }

    pub fn key_name(&self) -> &'static str {
        match self {
            SuiNetwork::Mainnet => MAINNET_KEY_NAME,
            SuiNetwork::Testnet => TEST_KEY_NAME,
        }
    }
}

/// Which network a 32-byte chain identifier names, or `None` for any unknown one.
pub fn network_of(chain_identifier: &Bytes32) -> Option<SuiNetwork> {
    if *chain_identifier == MAINNET_CHAIN_IDENTIFIER {
        Some(SuiNetwork::Mainnet)
    } else if *chain_identifier == TESTNET_CHAIN_IDENTIFIER {
        Some(SuiNetwork::Testnet)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Domains
// ---------------------------------------------------------------------------

/// Mint id domain (profile `signing.domains.mint_id`).
pub const MINT_DOMAIN: &str = "rumi.icusd.sui.mint.v1";
/// Re-mint of a held claim (profile `signing.domains.remint_id`).
pub const REMINT_DOMAIN: &str = "rumi.icusd.sui.remint.v1";
/// Return id domain (profile `signing.domains.return_id`); the Move gate's `DOMAIN_RETURN`.
pub const RETURN_DOMAIN: &str = "rumi.icusd.sui.return.v1";
/// Every domain, for the prefix-freeness test.
pub const ALL_DOMAINS: [&str; 3] = [MINT_DOMAIN, REMINT_DOMAIN, RETURN_DOMAIN];

/// Root segment of every derivation path.
pub const KEY_ROOT: &[u8] = b"rumi-icusd-sui-bridge";
/// Version segment of every derivation path.
pub const KEY_PATH_VERSION: &[u8] = b"v1";
/// Label hashed (SHA-256) into the escrow subaccount (profile `signing.domains.escrow_label`).
pub const ESCROW_LABEL: &str = "rumi-icusd-sui-bridge:escrow:v1";
/// Label hashed into the per-instance salt at install.
pub const INSTANCE_SALT_LABEL: &str = "rumi-icusd-sui-bridge:instance-salt:v1";
/// Version string `bridge_version` reports.
pub const BRIDGE_VERSION: &str = "rumi-sui-icusd-bridge-v1";

/// ICP ledger memo prefixes (8 bytes each) for escrow operations (`RSUI-*`, design 5.1).
pub const PULL_MEMO_PREFIX: &[u8; 8] = b"RSUI-DEP";
pub const REFUND_MEMO_PREFIX: &[u8; 8] = b"RSUI-RFD";
pub const SWEEP_MEMO_PREFIX: &[u8; 8] = b"RSUI-FEE";

/// 32-byte hash (ids, transfer ids, instance salt).
pub type Hash32 = Bytes32;

/// The three signing roles. Distinct keys by construction (least privilege): a mint is a
/// sponsored transaction signed by `Treasury` (sender, holds the `MintCap`) and `Gas` (gas owner),
/// and `UpgradeCustody` holds the `UpgradeCap`, `AdminCap` and `MetadataCap`. The canister never
/// signs with `UpgradeCustody` in this foundation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, CandidType)]
pub enum SuiRole {
    Treasury,
    Gas,
    UpgradeCustody,
}

impl SuiRole {
    pub const ALL: [SuiRole; 3] = [SuiRole::Treasury, SuiRole::Gas, SuiRole::UpgradeCustody];

    /// The path element for this role (profile `signing.roles`).
    pub fn label(&self) -> &'static [u8] {
        match self {
            SuiRole::Treasury => b"treasury",
            SuiRole::Gas => b"gas",
            SuiRole::UpgradeCustody => b"upgrade-custody",
        }
    }
}

/// `[rumi-icusd-sui-bridge, v1, role, chain_identifier_32]`. The network is a path element, so
/// a testnet key can never equal a mainnet key even if canister separation failed.
pub fn derivation_path(role: SuiRole, chain_identifier: &Bytes32) -> Vec<Vec<u8>> {
    vec![KEY_ROOT.to_vec(), KEY_PATH_VERSION.to_vec(), role.label().to_vec(), chain_identifier.to_vec()]
}

/// The 32-byte escrow subaccount: `sha256("rumi-icusd-sui-bridge:escrow:v1")`.
pub fn escrow_subaccount() -> Bytes32 {
    Sha256::digest(ESCROW_LABEL.as_bytes()).into()
}

/// `sha256(INSTANCE_SALT_LABEL || canister id || install time (u64 big-endian))`.
pub fn instance_salt(canister_id: &[u8], install_time_ns: u64) -> Bytes32 {
    let mut h = Sha256::new();
    h.update(INSTANCE_SALT_LABEL.as_bytes());
    h.update(canister_id);
    h.update(install_time_ns.to_be_bytes());
    h.finalize().into()
}

fn domain_hash(domain: &str, fields: &[&[u8]]) -> Bytes32 {
    let mut h = Sha256::new();
    h.update(domain.as_bytes());
    for f in fields {
        h.update((f.len() as u32).to_be_bytes());
        h.update(f);
    }
    h.finalize().into()
}

/// The 32-byte mint id the Gate consumes. Bound to the full network and gate identity: the
/// chain identifier, the package, the gate object, the instance salt, the ICP escrow-pull block
/// index, the recipient and the amount.
pub fn mint_id(
    chain_identifier: &Bytes32,
    package: &Bytes32,
    gate: &Bytes32,
    instance_salt: &Bytes32,
    escrow_pull_block: u64,
    recipient: &Bytes32,
    amount: u64,
) -> Bytes32 {
    domain_hash(
        MINT_DOMAIN,
        &[
            chain_identifier,
            package,
            gate,
            instance_salt,
            &escrow_pull_block.to_be_bytes(),
            recipient,
            &amount.to_be_bytes(),
        ],
    )
}

/// The 32-byte mint id of the re-mint of a held release claim: bound to the network and gate
/// identity, the instance, the return it re-mints, the Sui address that sent it and the amount, in
/// its own domain so it can never equal a user deposit's mint id.
pub fn remint_id(
    chain_identifier: &Bytes32,
    package: &Bytes32,
    gate: &Bytes32,
    instance_salt: &Bytes32,
    return_id: &Bytes32,
    recipient: &Bytes32,
    amount: u64,
) -> Bytes32 {
    domain_hash(REMINT_DOMAIN, &[chain_identifier, package, gate, instance_salt, return_id, recipient, &amount.to_be_bytes()])
}

/// The return `transfer_id` exactly as the Move gate computes it
/// (`rumi_icusd_gate::compute_transfer_id` at 23f75ef1): sha2-256 over
/// `bcs(DOMAIN_RETURN) ++ bcs(chain_identifier) ++ gate ++ sender ++ bcs(client_nonce)`.
pub fn transfer_id(chain_identifier: &Bytes32, gate: &Bytes32, sender: &Bytes32, client_nonce: u64) -> Bytes32 {
    let mut w = crate::bcs::Writer::new();
    w.string(RETURN_DOMAIN).bytes(chain_identifier).raw(gate).raw(sender).u64(client_nonce);
    Sha256::digest(w.into_bytes()).into()
}

fn prefixed_memo(prefix: &[u8; 8], id: u64) -> Vec<u8> {
    let mut m = Vec::with_capacity(16);
    m.extend_from_slice(prefix);
    m.extend_from_slice(&id.to_be_bytes());
    m
}

/// ICP ledger memo of a deposit pull: `RSUI-DEP` and the deposit id (8 bytes big-endian).
pub fn pull_memo(deposit_id: u64) -> Vec<u8> {
    prefixed_memo(PULL_MEMO_PREFIX, deposit_id)
}

/// ICP ledger memo of a deposit refund.
pub fn refund_ledger_memo(deposit_id: u64) -> Vec<u8> {
    prefixed_memo(REFUND_MEMO_PREFIX, deposit_id)
}

/// ICP ledger memo of a fee sweep.
pub fn sweep_memo(sweep_id: u64) -> Vec<u8> {
    prefixed_memo(SWEEP_MEMO_PREFIX, sweep_id)
}

/// Release memo is the 32-byte return `transfer_id` itself.
pub fn release_memo(transfer_id: &Hash32) -> Vec<u8> {
    transfer_id.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHAIN_A: Bytes32 = [
        1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32,
    ];

    #[test]
    fn the_two_accepted_networks_decode_from_the_sourced_base58() {
        let main = bs58::decode("4btiuiMPvEENsttpZC7CZ53DruC3MAgfznDbASZ7DR6S").into_vec().unwrap();
        let test = bs58::decode("69WiPg3DAQiwdxfncX6wYQ2siKwAe6L9BZthQea3JNMD").into_vec().unwrap();
        assert_eq!(main, MAINNET_CHAIN_IDENTIFIER);
        assert_eq!(test, TESTNET_CHAIN_IDENTIFIER);
        assert_eq!(SuiNetwork::Mainnet.short_chain_id(), "35834a8a");
        assert_eq!(SuiNetwork::Testnet.short_chain_id(), "4c78adac");
        assert_eq!(network_of(&MAINNET_CHAIN_IDENTIFIER), Some(SuiNetwork::Mainnet));
        assert_eq!(network_of(&TESTNET_CHAIN_IDENTIFIER), Some(SuiNetwork::Testnet));
        assert_eq!(network_of(&CHAIN_A), None, "an unknown chain identifier is refused, not guessed");
        assert_eq!(network_of(&[0; 32]), None);
    }

    #[test]
    fn key_names_follow_the_profile_rule() {
        assert_eq!(SuiNetwork::Mainnet.key_name(), "key_1");
        assert_eq!(SuiNetwork::Testnet.key_name(), "test_key_1");
    }

    #[test]
    fn role_paths_are_distinct_and_carry_the_network() {
        let paths: std::collections::BTreeSet<_> = SuiRole::ALL.iter().map(|r| derivation_path(*r, &CHAIN_A)).collect();
        assert_eq!(paths.len(), 3, "the three roles never share a path");
        for role in SuiRole::ALL {
            let p = derivation_path(role, &CHAIN_A);
            assert_eq!(p[0], b"rumi-icusd-sui-bridge");
            assert_eq!(p[1], b"v1");
            assert_eq!(p[2], role.label());
            assert_eq!(p[3], CHAIN_A.to_vec());
            assert_ne!(p, derivation_path(role, &TESTNET_CHAIN_IDENTIFIER), "the network is part of the path");
        }
        assert_eq!(derivation_path(SuiRole::UpgradeCustody, &CHAIN_A)[2], b"upgrade-custody");
    }

    #[test]
    fn domains_are_prefix_free() {
        for a in ALL_DOMAINS {
            for b in ALL_DOMAINS {
                if a != b {
                    assert!(!b.starts_with(a), "{a} is a prefix of {b}");
                }
            }
        }
        assert_eq!(RETURN_DOMAIN.as_bytes(), b"rumi.icusd.sui.return.v1");
    }

    #[test]
    fn instance_salt_and_mint_id_match_the_independent_reference() {
        let salt = instance_salt(&[0xbb; 10], 1_700_000_000_000_000_000);
        assert_eq!(hex::encode(salt), "acf91ae9876b3796d988636e807a01170d021e9f0f6f3439c561fb8a1ddad0db");
        let id = mint_id(&CHAIN_A, &[0x70; 32], &[0x6a; 32], &salt, 12345, &[0x8b; 32], 250 * 100_000_000);
        assert_eq!(hex::encode(id), "b6094c28f6e0520293ccc1abac2fd84070a1d1896aa0058983cdd12d787cd1b6");
    }

    #[test]
    fn every_input_of_the_mint_id_matters() {
        let base = || (CHAIN_A, [0x70u8; 32], [0x6au8; 32], [1u8; 32], 7u64, [0x8bu8; 32], 5u64);
        let id = |t: (Bytes32, Bytes32, Bytes32, Bytes32, u64, Bytes32, u64)| mint_id(&t.0, &t.1, &t.2, &t.3, t.4, &t.5, t.6);
        let b = id(base());
        let mut ids = vec![b];
        let mut v = base();
        v.0 = TESTNET_CHAIN_IDENTIFIER;
        ids.push(id(v));
        let mut v = base();
        v.1[0] ^= 1;
        ids.push(id(v));
        let mut v = base();
        v.2[0] ^= 1;
        ids.push(id(v));
        let mut v = base();
        v.3[0] ^= 1;
        ids.push(id(v));
        let mut v = base();
        v.4 += 1;
        ids.push(id(v));
        let mut v = base();
        v.5[0] ^= 1;
        ids.push(id(v));
        let mut v = base();
        v.6 += 1;
        ids.push(id(v));
        let unique: std::collections::BTreeSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "changing any single input changes the id");
    }

    /// The same vectors the accepted Move tests pin (`tests/rumi_icusd_gate_tests.move`,
    /// `transfer_ids_match_the_independent_reference_vectors`), from the Python reference.
    #[test]
    fn transfer_ids_equal_the_move_gate_vectors() {
        let gate = [0xaa; 32];
        let sender = [0xbb; 32];
        assert_eq!(hex::encode(transfer_id(&CHAIN_A, &gate, &sender, 1)), "d9f230d9c727a3d311b72b92db4a127bf94b9af42643a4ccb4cd7f98b474cea2");
        assert_eq!(hex::encode(transfer_id(&CHAIN_A, &gate, &sender, 0)), "85f1a74776c47a2abaa38a52a4db09b97c9b206f178ff89172c6e17ba2efda71");
        assert_eq!(
            hex::encode(transfer_id(&CHAIN_A, &gate, &sender, u64::MAX)),
            "2f94a6e9243dfa2926c128f2116aa532c15a82864a73b800a2944bdec05b2641"
        );
        assert_eq!(hex::encode(transfer_id(&CHAIN_A, &gate, &[0xcc; 32], 1)), "c394fd448284bbaa385a723bdb5152b4a887370c706f15357d03bdbb122ec325");
        assert_eq!(hex::encode(transfer_id(&[0x11; 32], &gate, &sender, 1)), "b5a581713bfc17dda3a799b9620a4d648ebcff60d4adf974b27e6c985d2176e6");
        assert_eq!(hex::encode(transfer_id(&CHAIN_A, &[0xab; 32], &sender, 1)), "d8b7c2b90ebf5c379e16543f373045e5db63ca4afcf020a1aa483a45657c6c49");
    }

    #[test]
    fn ledger_memos_use_rsui_prefixes() {
        assert_eq!(pull_memo(5), [b"RSUI-DEP".as_slice(), &5u64.to_be_bytes()].concat());
        assert_eq!(refund_ledger_memo(5), [b"RSUI-RFD".as_slice(), &5u64.to_be_bytes()].concat());
        assert_eq!(sweep_memo(5), [b"RSUI-FEE".as_slice(), &5u64.to_be_bytes()].concat());
        assert_eq!(release_memo(&[9; 32]), vec![9u8; 32]);
        // Distinct from the Solana instance's prefixes: a memo never crosses instances.
        for m in [pull_memo(1), refund_ledger_memo(1), sweep_memo(1)] {
            assert!(!m.starts_with(b"RSOL"));
        }
    }

    #[test]
    fn the_escrow_subaccount_is_the_label_hash() {
        assert_eq!(escrow_subaccount(), <[u8; 32]>::from(Sha256::digest(b"rumi-icusd-sui-bridge:escrow:v1")));
    }
}
