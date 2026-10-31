//! Static table of the chains this bridge knows how to serve.
//!
//! A row is derived data: it is looked up from the chain id and never stored in state,
//! never crosses Candid and is not serialized. Adding a row changes no stored value and
//! no public interface. For now the table holds the Base rows only; the identifier
//! templates in [`crate::ids`] take the row's `slug`, so a Base row reproduces the strings
//! the live Base instance already signs under, byte for byte.
//!
//! Keep a row minimal. Display names, key rules, head rules and providers are added by
//! later cards, one field at a time.

/// What a chain id is used for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChainRole {
    Mainnet,
    Testnet,
    Local,
}

/// One row of the chain table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainProfile {
    pub chain_id: u64,
    /// Lowercase ASCII label that is hashed into the off-chain identifier strings
    /// (`rumi.icusd.{slug}.mint.v1` and friends). Changing a live row's slug would move a
    /// signing domain, an escrow subaccount and the threshold-ECDSA key path.
    pub slug: &'static str,
    pub role: ChainRole,
}

/// Base mainnet, Base Sepolia and the local Anvil chain all share the `base` slug, so a
/// Base instance keeps the identifiers it had before the table existed.
static CHAINS: [ChainProfile; 3] = [
    ChainProfile { chain_id: 8453, slug: "base", role: ChainRole::Mainnet },
    ChainProfile { chain_id: 84532, slug: "base", role: ChainRole::Testnet },
    ChainProfile { chain_id: 31337, slug: "base", role: ChainRole::Local },
];

/// The row for `chain_id`, or `None` when this bridge has no profile for it.
pub fn profile(chain_id: u64) -> Option<&'static ChainProfile> {
    CHAINS.iter().find(|p| p.chain_id == chain_id)
}

/// Check that the chain id stored by a previous module still has a profile. Pure, so it
/// can be unit tested; `post_upgrade` traps on an error (which rolls the upgrade back to
/// the old module).
///
/// This is half of the invariant that lets [`crate::ids`] panic on an unknown chain id
/// instead of falling back to a default: install admits only ids with a profile
/// (`state::validate_init`), and upgrade refuses to continue with a stored id that has none.
pub fn validate_stored_chain(chain_id: u64) -> Result<(), String> {
    match profile(chain_id) {
        Some(_) => Ok(()),
        None => Err(format!("stored chain id {chain_id} has no bridge chain profile")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{validate_init, CHAIN_BASE_MAINNET, CHAIN_BASE_SEPOLIA, CHAIN_LOCAL_ANVIL};
    use crate::types::{InitArgs, RpcServiceConfig};
    use candid::Principal;

    fn principal(n: u8) -> Principal {
        Principal::from_slice(&[n; 4])
    }

    fn init_args(chain: u64, key: &str) -> InitArgs {
        InitArgs {
            icusd_ledger: principal(1),
            evm_rpc: principal(2),
            ecdsa_key_name: key.into(),
            base_chain_id: chain,
            admins: vec![principal(3)],
            fee_recipient: None,
            rpc_services: vec![
                RpcServiceConfig::Custom { url: "https://a.example".into() },
                RpcServiceConfig::Custom { url: "https://b.example".into() },
                RpcServiceConfig::Custom { url: "https://c.example".into() },
            ],
            min_quorum: 2,
        }
    }

    #[test]
    fn the_table_holds_exactly_the_three_base_rows() {
        assert_eq!(CHAINS.len(), 3);
        let p = profile(8453).unwrap();
        assert_eq!((p.chain_id, p.slug, p.role), (8453, "base", ChainRole::Mainnet));
        let p = profile(84532).unwrap();
        assert_eq!((p.chain_id, p.slug, p.role), (84532, "base", ChainRole::Testnet));
        let p = profile(31337).unwrap();
        assert_eq!((p.chain_id, p.slug, p.role), (31337, "base", ChainRole::Local));
    }

    #[test]
    fn chain_ids_are_unique_and_slugs_are_safe_label_text() {
        for (i, a) in CHAINS.iter().enumerate() {
            for b in &CHAINS[i + 1..] {
                assert_ne!(a.chain_id, b.chain_id, "duplicate chain id");
            }
            assert!(!a.slug.is_empty());
            assert!(a.slug.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()), "slug {:?} must be lowercase alphanumeric", a.slug);
        }
    }

    #[test]
    fn every_chain_install_admits_has_a_profile() {
        // Every (chain, key) pair `validate_init` accepts today. If install ever admits a
        // chain without a profile, `ids` would panic on it, so this must stay true.
        let accepted: [(u64, &str); 5] = [
            (CHAIN_BASE_MAINNET, "key_1"),
            (CHAIN_BASE_SEPOLIA, "key_1"),
            (CHAIN_BASE_SEPOLIA, "test_key_1"),
            (CHAIN_LOCAL_ANVIL, "test_key_1"),
            (CHAIN_LOCAL_ANVIL, "dfx_test_key"),
        ];
        for (chain, key) in accepted {
            let config = validate_init(&init_args(chain, key)).unwrap_or_else(|e| panic!("{chain}/{key} must be accepted: {e}"));
            assert_eq!(config.chain_id, chain);
            assert!(profile(config.chain_id).is_some(), "chain {chain} is admitted by install but has no profile");
        }
        assert_eq!(profile(CHAIN_BASE_MAINNET).map(|p| p.chain_id), Some(8453));
        assert_eq!(profile(CHAIN_BASE_SEPOLIA).map(|p| p.chain_id), Some(84532));
        assert_eq!(profile(CHAIN_LOCAL_ANVIL).map(|p| p.chain_id), Some(31337));
    }

    #[test]
    fn install_admits_nothing_beyond_the_table() {
        // The reverse direction, probed over the ids the other chains in the groundwork
        // plan would use plus the unset value: install refuses each, and the table has none.
        for chain in [0u64, 1, 10, 143, 998, 999, 1030, 4663, 10143, 42161, 421614, 46630, 11155111] {
            assert!(profile(chain).is_none(), "{chain} must have no profile yet");
            for key in ["key_1", "test_key_1", "dfx_test_key"] {
                assert!(validate_init(&init_args(chain, key)).is_err(), "install must refuse chain {chain} with {key}");
            }
        }
    }

    #[test]
    fn stored_chain_check_accepts_profiled_ids_and_refuses_others() {
        for chain in [8453u64, 84532, 31337] {
            assert!(validate_stored_chain(chain).is_ok(), "{chain}");
        }
        for chain in [0u64, 1, 8454, 84531, 31338, u64::MAX] {
            let e = validate_stored_chain(chain).unwrap_err();
            assert_eq!(e, format!("stored chain id {chain} has no bridge chain profile"));
        }
    }
}
