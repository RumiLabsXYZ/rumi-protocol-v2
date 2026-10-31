//! Static table of the chains this bridge knows how to serve.
//!
//! A row is derived data: it is looked up from the chain id and never stored in state,
//! never crosses Candid and is not serialized. Adding a row changes no stored value and
//! no public interface. The identifier templates in [`crate::ids`] take the row's `slug`,
//! so a Base row reproduces the strings the live Base instance already signs under, byte
//! for byte.
//!
//! A row carries everything that differs between chains and is not an operator setting:
//!
//! - the identifier `slug` and the `display_name` shown to users;
//! - which ECDSA key names install accepts (`allowed_keys`);
//! - which built-in EVM RPC provider family is valid for the chain (`native`);
//! - how the settlement head is chosen (`head_rule`, used by `evm::rpc`);
//! - whether install may serve the chain at all (`install`);
//! - the user-facing finality wording (`consent_wait`, `finality_label`).
//!
//! Only the Base rows are installable. Every other row is `Disabled` with a reason, so
//! the table can describe a chain without the canister being able to serve it. Install
//! (`state::validate_init`) is where enablement is checked. Upgrade (`post_upgrade`) only
//! requires that the stored chain id still has a row, so a module that later disables a
//! row never strands an instance that was installed while the row was enabled.

use crate::types::RpcServiceConfig;

/// What a chain id is used for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChainRole {
    Mainnet,
    Testnet,
    Local,
}

/// A family of built-in providers of the EVM RPC canister that `RpcServiceConfig` can name.
/// A family is valid only for the row that lists it in `native`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeProvider {
    /// `RpcServiceConfig::BaseMainnet`: the five built-in Base mainnet providers.
    BaseMainnet,
    /// `RpcServiceConfig::EthMainnet`: the six built-in Ethereum mainnet providers.
    EthMainnet,
    /// `RpcServiceConfig::EthSepolia`: the five built-in Ethereum Sepolia providers.
    EthSepolia,
    /// `RpcServiceConfig::ArbitrumOne`: the five built-in Arbitrum One providers.
    ArbitrumOne,
}

impl NativeProvider {
    /// The `RpcServiceConfig` variant name, for error text.
    pub fn variant_name(self) -> &'static str {
        match self {
            NativeProvider::BaseMainnet => "BaseMainnet",
            NativeProvider::EthMainnet => "EthMainnet",
            NativeProvider::EthSepolia => "EthSepolia",
            NativeProvider::ArbitrumOne => "ArbitrumOne",
        }
    }
}

/// How the settlement head is chosen. Read by `evm::rpc`; no rule ever reads `latest`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeadRule {
    /// `n` is the `min_quorum`-th highest `finalized` height reported by the providers,
    /// and the head is block `n`, its hash confirmed by the same quorum.
    TagFinalizedQuorum,
    /// The head is `n - delay_blocks` for the quorum-agreed finalized height `n`, with the
    /// block hash at that height confirmed by the same quorum. Monad's Verified state (the
    /// execution outputs are agreed) comes `delay_blocks` after Finalized.
    MonadVerified { delay_blocks: u64 },
    /// No head is read. Every call fails before any provider is asked. The reason is a
    /// short activation-blocker id.
    Blocked(&'static str),
}

/// Whether install may serve a chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallGate {
    Enabled,
    /// Install refuses the chain. The reason names the activation blocker.
    Disabled(&'static str),
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
    /// Chain name used in consent messages and error text. For Base this is `Base` under
    /// every network, as the consent text has always been.
    pub display_name: &'static str,
    /// ECDSA key names install accepts for this chain.
    pub allowed_keys: &'static [&'static str],
    /// The built-in provider family valid for this chain, if any.
    pub native: Option<NativeProvider>,
    pub head_rule: HeadRule,
    pub install: InstallGate,
    /// Completes "Delivery is expected ..." in the ICRC-21 consent message.
    pub consent_wait: &'static str,
    /// Names the settlement head in the `finality_policy` view.
    pub finality_label: &'static str,
}

const MAINNET_KEYS: &[&str] = &["key_1"];
const TESTNET_KEYS: &[&str] = &["test_key_1"];
const BASE_SEPOLIA_KEYS: &[&str] = &["key_1", "test_key_1"];
const LOCAL_KEYS: &[&str] = &["test_key_1", "dfx_test_key"];

const BASE_WAIT: &str = "after Base L1 batch finality, about 20 minutes";
const BASE_LABEL: &str = "Base `finalized` head (L1 batch finality)";

const ETH_NOT_ACTIVE: &str = "not activated: needs built-in Ethereum RPC providers (D1b) and activation approval";
const ETH_WAIT: &str = "after Ethereum finalization, about 13 minutes";
const ETH_LABEL: &str = "Ethereum `finalized` head (about 2 epochs)";

const ARB_NOT_ACTIVE: &str = "not activated: needs built-in Arbitrum RPC providers (D1b), mint gas re-measurement and activation approval";
const ARB_WAIT: &str = "after the Arbitrum batch is finalized on Ethereum, about 13 minutes";
const ARB_LABEL: &str = "Arbitrum `finalized` head (parent-chain batch finality)";

const RHOOD_BLOCKER: &str = "robinhood-finalized-tag-probe";
const RHOOD_NOT_ACTIVE: &str = "robinhood-finalized-tag-probe, single-keyless-endpoint";
const RHOOD_WAIT: &str = "after Robinhood Chain finality (timing not yet confirmed)";
const RHOOD_LABEL: &str = "Robinhood Chain `finalized` head";

const HYPER_BLOCKER: &str = "hyperevm-historical-state";
const HYPER_WAIT: &str = "after HyperEVM finality (timing not yet confirmed)";
const HYPER_LABEL: &str = "HyperEVM head";

/// Blocks between Monad Finalized and Verified (state root finality), from Monad's
/// developer summary (see the reconciliation in `docs/coordination/multichain/sources-evm.md`).
pub const MONAD_VERIFIED_DELAY_BLOCKS: u64 = 3;
const MONAD_NOT_ACTIVE: &str = "monad-verified-head: Verified head not yet exercised on a testnet, fast-tag consensus unmeasured, D8";
const MONAD_WAIT: &str = "after Monad Verified state (Finalized plus 3 blocks), within seconds";
const MONAD_LABEL: &str = "Monad Verified head";

/// Base mainnet, Base Sepolia and the local Anvil chain all share the `base` slug, so a
/// Base instance keeps the identifiers it had before the table existed. The other rows
/// are described but not installable.
static CHAINS: [ChainProfile; 13] = [
    ChainProfile {
        chain_id: 8453,
        slug: "base",
        role: ChainRole::Mainnet,
        display_name: "Base",
        allowed_keys: MAINNET_KEYS,
        native: Some(NativeProvider::BaseMainnet),
        head_rule: HeadRule::TagFinalizedQuorum,
        install: InstallGate::Enabled,
        consent_wait: BASE_WAIT,
        finality_label: BASE_LABEL,
    },
    ChainProfile {
        chain_id: 84532,
        slug: "base",
        role: ChainRole::Testnet,
        display_name: "Base",
        allowed_keys: BASE_SEPOLIA_KEYS,
        native: None,
        head_rule: HeadRule::TagFinalizedQuorum,
        install: InstallGate::Enabled,
        consent_wait: BASE_WAIT,
        finality_label: BASE_LABEL,
    },
    ChainProfile {
        chain_id: 31337,
        slug: "base",
        role: ChainRole::Local,
        display_name: "Base",
        allowed_keys: LOCAL_KEYS,
        native: None,
        head_rule: HeadRule::TagFinalizedQuorum,
        install: InstallGate::Enabled,
        consent_wait: BASE_WAIT,
        finality_label: BASE_LABEL,
    },
    ChainProfile {
        chain_id: 1,
        slug: "eth",
        role: ChainRole::Mainnet,
        display_name: "Ethereum",
        allowed_keys: MAINNET_KEYS,
        native: Some(NativeProvider::EthMainnet),
        head_rule: HeadRule::TagFinalizedQuorum,
        install: InstallGate::Disabled(ETH_NOT_ACTIVE),
        consent_wait: ETH_WAIT,
        finality_label: ETH_LABEL,
    },
    ChainProfile {
        chain_id: 11155111,
        slug: "eth",
        role: ChainRole::Testnet,
        display_name: "Ethereum Sepolia",
        allowed_keys: TESTNET_KEYS,
        native: Some(NativeProvider::EthSepolia),
        head_rule: HeadRule::TagFinalizedQuorum,
        install: InstallGate::Disabled(ETH_NOT_ACTIVE),
        consent_wait: ETH_WAIT,
        finality_label: ETH_LABEL,
    },
    ChainProfile {
        chain_id: 42161,
        slug: "arb",
        role: ChainRole::Mainnet,
        display_name: "Arbitrum One",
        allowed_keys: MAINNET_KEYS,
        native: Some(NativeProvider::ArbitrumOne),
        head_rule: HeadRule::TagFinalizedQuorum,
        install: InstallGate::Disabled(ARB_NOT_ACTIVE),
        consent_wait: ARB_WAIT,
        finality_label: ARB_LABEL,
    },
    ChainProfile {
        chain_id: 421614,
        slug: "arb",
        role: ChainRole::Testnet,
        display_name: "Arbitrum Sepolia",
        allowed_keys: TESTNET_KEYS,
        native: None,
        head_rule: HeadRule::TagFinalizedQuorum,
        install: InstallGate::Disabled(ARB_NOT_ACTIVE),
        consent_wait: ARB_WAIT,
        finality_label: ARB_LABEL,
    },
    ChainProfile {
        chain_id: 4663,
        slug: "rhood",
        role: ChainRole::Mainnet,
        display_name: "Robinhood Chain",
        allowed_keys: MAINNET_KEYS,
        native: None,
        head_rule: HeadRule::Blocked(RHOOD_BLOCKER),
        install: InstallGate::Disabled(RHOOD_NOT_ACTIVE),
        consent_wait: RHOOD_WAIT,
        finality_label: RHOOD_LABEL,
    },
    ChainProfile {
        chain_id: 46630,
        slug: "rhood",
        role: ChainRole::Testnet,
        display_name: "Robinhood Chain testnet",
        allowed_keys: TESTNET_KEYS,
        native: None,
        head_rule: HeadRule::Blocked(RHOOD_BLOCKER),
        install: InstallGate::Disabled(RHOOD_NOT_ACTIVE),
        consent_wait: RHOOD_WAIT,
        finality_label: RHOOD_LABEL,
    },
    ChainProfile {
        chain_id: 999,
        slug: "hyper",
        role: ChainRole::Mainnet,
        display_name: "HyperEVM",
        allowed_keys: MAINNET_KEYS,
        native: None,
        head_rule: HeadRule::Blocked(HYPER_BLOCKER),
        install: InstallGate::Disabled(HYPER_BLOCKER),
        consent_wait: HYPER_WAIT,
        finality_label: HYPER_LABEL,
    },
    ChainProfile {
        chain_id: 998,
        slug: "hyper",
        role: ChainRole::Testnet,
        display_name: "HyperEVM testnet",
        allowed_keys: TESTNET_KEYS,
        native: None,
        head_rule: HeadRule::Blocked(HYPER_BLOCKER),
        install: InstallGate::Disabled(HYPER_BLOCKER),
        consent_wait: HYPER_WAIT,
        finality_label: HYPER_LABEL,
    },
    ChainProfile {
        chain_id: 143,
        slug: "monad",
        role: ChainRole::Mainnet,
        display_name: "Monad",
        allowed_keys: MAINNET_KEYS,
        native: None,
        head_rule: HeadRule::MonadVerified { delay_blocks: MONAD_VERIFIED_DELAY_BLOCKS },
        install: InstallGate::Disabled(MONAD_NOT_ACTIVE),
        consent_wait: MONAD_WAIT,
        finality_label: MONAD_LABEL,
    },
    ChainProfile {
        chain_id: 10143,
        slug: "monad",
        role: ChainRole::Testnet,
        display_name: "Monad testnet",
        allowed_keys: TESTNET_KEYS,
        native: None,
        head_rule: HeadRule::MonadVerified { delay_blocks: MONAD_VERIFIED_DELAY_BLOCKS },
        install: InstallGate::Disabled(MONAD_NOT_ACTIVE),
        consent_wait: MONAD_WAIT,
        finality_label: MONAD_LABEL,
    },
];

/// The row for `chain_id`, or `None` when this bridge has no profile for it.
pub fn profile(chain_id: u64) -> Option<&'static ChainProfile> {
    CHAINS.iter().find(|p| p.chain_id == chain_id)
}

/// The built-in provider family a configured provider belongs to, or `None` for a custom
/// URL. The match is exhaustive on purpose: a new `RpcServiceConfig` variant must say here
/// which family it is.
pub fn native_provider_of(cfg: &RpcServiceConfig) -> Option<NativeProvider> {
    match cfg {
        RpcServiceConfig::BaseMainnet(_) => Some(NativeProvider::BaseMainnet),
        RpcServiceConfig::EthMainnet(_) => Some(NativeProvider::EthMainnet),
        RpcServiceConfig::EthSepolia(_) => Some(NativeProvider::EthSepolia),
        RpcServiceConfig::ArbitrumOne(_) => Some(NativeProvider::ArbitrumOne),
        RpcServiceConfig::Custom { .. } => None,
    }
}

/// The row a built-in provider family is valid for.
pub fn native_owner(family: NativeProvider) -> Option<&'static ChainProfile> {
    CHAINS.iter().find(|p| p.native == Some(family))
}

/// Check that the chain id stored by a previous module still has a profile. Pure, so it
/// can be unit tested; `post_upgrade` traps on an error (which rolls the upgrade back to
/// the old module).
///
/// This is half of the invariant that lets [`crate::ids`] panic on an unknown chain id
/// instead of falling back to a default: install admits only ids with an enabled profile
/// (`state::validate_init`), and upgrade refuses to continue with a stored id that has no
/// profile. Upgrade deliberately checks the profile only, not the install gate: enablement
/// is an install-time decision.
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
    use crate::types::{InitArgs, L2MainnetService, RpcServiceConfig};
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

    const BASE_IDS: [u64; 3] = [8453, 84532, 31337];

    /// (chain id, slug, role) for every row, as the groundwork plan names them.
    const EXPECTED: [(u64, &str, ChainRole); 13] = [
        (8453, "base", ChainRole::Mainnet),
        (84532, "base", ChainRole::Testnet),
        (31337, "base", ChainRole::Local),
        (1, "eth", ChainRole::Mainnet),
        (11155111, "eth", ChainRole::Testnet),
        (42161, "arb", ChainRole::Mainnet),
        (421614, "arb", ChainRole::Testnet),
        (4663, "rhood", ChainRole::Mainnet),
        (46630, "rhood", ChainRole::Testnet),
        (999, "hyper", ChainRole::Mainnet),
        (998, "hyper", ChainRole::Testnet),
        (143, "monad", ChainRole::Mainnet),
        (10143, "monad", ChainRole::Testnet),
    ];

    #[test]
    fn the_table_holds_exactly_the_planned_rows() {
        assert_eq!(CHAINS.len(), EXPECTED.len());
        for (id, slug, role) in EXPECTED {
            let p = profile(id).unwrap_or_else(|| panic!("row {id} missing"));
            assert_eq!((p.chain_id, p.slug, p.role), (id, slug, role), "row {id}");
        }
    }

    #[test]
    fn the_base_rows_keep_todays_behavior() {
        for id in BASE_IDS {
            let p = profile(id).unwrap();
            assert_eq!(p.slug, "base");
            assert_eq!(p.display_name, "Base");
            assert_eq!(p.head_rule, HeadRule::TagFinalizedQuorum);
            assert_eq!(p.install, InstallGate::Enabled);
            assert_eq!(p.consent_wait, "after Base L1 batch finality, about 20 minutes");
            assert_eq!(p.finality_label, "Base `finalized` head (L1 batch finality)");
        }
        // The key rule is the one `validate_init` always had.
        assert_eq!(profile(8453).unwrap().allowed_keys, &["key_1"]);
        assert_eq!(profile(84532).unwrap().allowed_keys, &["key_1", "test_key_1"]);
        assert_eq!(profile(31337).unwrap().allowed_keys, &["test_key_1", "dfx_test_key"]);
        // Only Base mainnet has a built-in provider family.
        assert_eq!(profile(8453).unwrap().native, Some(NativeProvider::BaseMainnet));
        assert_eq!(profile(84532).unwrap().native, None);
        assert_eq!(profile(31337).unwrap().native, None);
    }

    #[test]
    fn chain_ids_are_unique_and_slugs_are_safe_label_text() {
        for (i, a) in CHAINS.iter().enumerate() {
            for b in &CHAINS[i + 1..] {
                assert_ne!(a.chain_id, b.chain_id, "duplicate chain id");
            }
            assert!(!a.slug.is_empty());
            assert!(a.slug.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()), "slug {:?} must be lowercase alphanumeric", a.slug);
            assert!(!a.display_name.is_empty());
            assert!(!a.consent_wait.is_empty());
            assert!(!a.finality_label.is_empty());
        }
    }

    #[test]
    fn a_slug_belongs_to_one_chain_family() {
        // Two rows may share a slug only when they are networks of one chain (mainnet and its
        // test networks), never two unrelated chains: a shared slug shares every identifier.
        for a in &CHAINS {
            for b in &CHAINS {
                if a.slug == b.slug {
                    assert_eq!(a.head_rule, b.head_rule, "{} and {} share slug {} but not a head rule", a.chain_id, b.chain_id, a.slug);
                    assert_eq!(a.consent_wait, b.consent_wait, "{} and {}", a.chain_id, b.chain_id);
                }
            }
        }
        let mainnet_slugs: Vec<&str> = CHAINS.iter().filter(|p| p.role == ChainRole::Mainnet).map(|p| p.slug).collect();
        let mut sorted = mainnet_slugs.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), mainnet_slugs.len(), "two mainnet rows share a slug");
    }

    #[test]
    fn key_rules_follow_the_role() {
        for p in &CHAINS {
            let is_base = BASE_IDS.contains(&p.chain_id);
            match (is_base, p.role) {
                // New rows: mainnet key_1 only, testnet test_key_1 only.
                (false, ChainRole::Mainnet) => assert_eq!(p.allowed_keys, &["key_1"], "{}", p.chain_id),
                (false, ChainRole::Testnet) => assert_eq!(p.allowed_keys, &["test_key_1"], "{}", p.chain_id),
                (false, ChainRole::Local) => panic!("only the Base local row may be Local"),
                // Base rows keep their legacy rules (asserted in `the_base_rows_keep_todays_behavior`).
                (true, _) => {}
            }
            // A mainnet row never accepts a test key, whatever the family.
            if p.role == ChainRole::Mainnet {
                assert!(!p.allowed_keys.iter().any(|k| k.contains("test")), "{}", p.chain_id);
            }
            assert!(!p.allowed_keys.is_empty());
        }
        assert_eq!(CHAINS.iter().filter(|p| p.role == ChainRole::Local).count(), 1);
    }

    #[test]
    fn head_rules_follow_the_plan() {
        for id in [8453u64, 84532, 31337, 1, 11155111, 42161, 421614] {
            assert_eq!(profile(id).unwrap().head_rule, HeadRule::TagFinalizedQuorum, "{id}");
        }
        for id in [4663u64, 46630] {
            assert_eq!(profile(id).unwrap().head_rule, HeadRule::Blocked("robinhood-finalized-tag-probe"), "{id}");
        }
        for id in [999u64, 998] {
            assert_eq!(profile(id).unwrap().head_rule, HeadRule::Blocked("hyperevm-historical-state"), "{id}");
        }
        for id in [143u64, 10143] {
            assert_eq!(profile(id).unwrap().head_rule, HeadRule::MonadVerified { delay_blocks: 3 }, "{id}");
        }
    }

    #[test]
    fn only_the_base_rows_are_enabled_and_every_other_row_has_a_reason() {
        for p in &CHAINS {
            match (BASE_IDS.contains(&p.chain_id), p.install) {
                (true, InstallGate::Enabled) => {}
                (false, InstallGate::Disabled(reason)) => assert!(!reason.is_empty(), "{} needs a reason", p.chain_id),
                (is_base, gate) => panic!("chain {} (base: {is_base}) has gate {gate:?}", p.chain_id),
            }
        }
        // A Blocked head rule is never paired with an enabled row.
        for p in &CHAINS {
            if matches!(p.head_rule, HeadRule::Blocked(_)) {
                assert!(matches!(p.install, InstallGate::Disabled(_)), "{}", p.chain_id);
            }
        }
    }

    #[test]
    fn native_provider_families_have_exactly_one_owner() {
        assert_eq!(native_owner(NativeProvider::BaseMainnet).map(|p| p.chain_id), Some(8453));
        assert_eq!(CHAINS.iter().filter(|p| p.native == Some(NativeProvider::BaseMainnet)).count(), 1);
        assert_eq!(native_provider_of(&RpcServiceConfig::BaseMainnet(L2MainnetService::Alchemy)), Some(NativeProvider::BaseMainnet));
        assert_eq!(native_provider_of(&RpcServiceConfig::Custom { url: "https://a.example".into() }), None);
        assert_eq!(NativeProvider::BaseMainnet.variant_name(), "BaseMainnet");
    }

    #[test]
    fn each_native_family_is_bound_to_exactly_one_row() {
        use crate::types::{EthMainnetService, EthSepoliaService};
        // The rows that carry a built-in family, and nothing else. Arbitrum Sepolia has none:
        // the EVM RPC canister offers no built-in Arbitrum Sepolia provider.
        let expected: [(u64, NativeProvider); 4] = [
            (8453, NativeProvider::BaseMainnet),
            (1, NativeProvider::EthMainnet),
            (11155111, NativeProvider::EthSepolia),
            (42161, NativeProvider::ArbitrumOne),
        ];
        for p in &CHAINS {
            let want = expected.iter().find(|(id, _)| *id == p.chain_id).map(|(_, f)| *f);
            assert_eq!(p.native, want, "row {}", p.chain_id);
        }
        assert_eq!(profile(421614).unwrap().native, None);
        for (id, family) in expected {
            assert_eq!(native_owner(family).map(|p| p.chain_id), Some(id), "{family:?}");
            assert_eq!(CHAINS.iter().filter(|p| p.native == Some(family)).count(), 1, "{family:?}");
        }
        // A mainnet family sits on a Mainnet row, the Sepolia family on a Testnet row.
        assert_eq!(profile(1).unwrap().role, ChainRole::Mainnet);
        assert_eq!(profile(42161).unwrap().role, ChainRole::Mainnet);
        assert_eq!(profile(11155111).unwrap().role, ChainRole::Testnet);

        // Every configured variant names its family.
        assert_eq!(native_provider_of(&RpcServiceConfig::EthMainnet(EthMainnetService::Cloudflare)), Some(NativeProvider::EthMainnet));
        assert_eq!(native_provider_of(&RpcServiceConfig::EthSepolia(EthSepoliaService::Sepolia)), Some(NativeProvider::EthSepolia));
        assert_eq!(native_provider_of(&RpcServiceConfig::ArbitrumOne(L2MainnetService::Llama)), Some(NativeProvider::ArbitrumOne));
        // A family's error text names the variant exactly as the type spells it.
        assert_eq!(NativeProvider::EthMainnet.variant_name(), "EthMainnet");
        assert_eq!(NativeProvider::EthSepolia.variant_name(), "EthSepolia");
        assert_eq!(NativeProvider::ArbitrumOne.variant_name(), "ArbitrumOne");
    }

    #[test]
    fn native_providers_change_no_gate_key_or_head_rule() {
        // The three rows that gained a family are still described but not installable, with
        // the head rule and key rule they had before.
        for id in [1u64, 11155111, 42161] {
            let p = profile(id).unwrap();
            assert!(matches!(p.install, InstallGate::Disabled(r) if !r.is_empty()), "{id}");
            assert_eq!(p.head_rule, HeadRule::TagFinalizedQuorum, "{id}");
            let keys: &[&str] = if p.role == ChainRole::Mainnet { &["key_1"] } else { &["test_key_1"] };
            assert_eq!(p.allowed_keys, keys, "{id}");
        }
        assert_eq!(profile(1).unwrap().slug, "eth");
        assert_eq!(profile(11155111).unwrap().slug, "eth");
        assert_eq!(profile(42161).unwrap().slug, "arb");
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
    fn install_accepts_exactly_the_base_pairs_and_refuses_the_rest_of_the_table() {
        // The full (row x key) truth table over a key list that includes every key name the
        // table uses plus junk. Accepted: the five Base pairs, nothing else.
        let keys = ["key_1", "test_key_1", "dfx_test_key", "", "KEY_1", "other"];
        let accepted: [(u64, &str); 5] = [
            (8453, "key_1"),
            (84532, "key_1"),
            (84532, "test_key_1"),
            (31337, "test_key_1"),
            (31337, "dfx_test_key"),
        ];
        for p in &CHAINS {
            for key in keys {
                let result = validate_init(&init_args(p.chain_id, key));
                let expect_ok = accepted.contains(&(p.chain_id, key));
                assert_eq!(result.is_ok(), expect_ok, "chain {} key {key:?}: {result:?}", p.chain_id);
            }
        }
    }

    #[test]
    fn every_disabled_row_refuses_every_key_naming_the_chain_and_the_reason() {
        for p in &CHAINS {
            let InstallGate::Disabled(reason) = p.install else { continue };
            // The row's own keys, the other role's keys, and junk: all refused.
            for key in ["key_1", "test_key_1", "dfx_test_key", "", "other"] {
                let e = validate_init(&init_args(p.chain_id, key)).expect_err(&format!("chain {} must refuse {key:?}", p.chain_id));
                assert!(e.contains(p.display_name), "{e}");
                assert!(e.contains(&p.chain_id.to_string()), "{e}");
                assert!(e.contains(reason), "{e}");
                assert!(e.contains("cannot be installed"), "{e}");
            }
        }
    }

    #[test]
    fn unprofiled_ids_keep_the_unsupported_chain_error() {
        for chain in [0u64, 10, 1030, 8454, 84531, 31338, u64::MAX] {
            assert!(profile(chain).is_none(), "{chain} must have no profile");
            for key in ["key_1", "test_key_1", "dfx_test_key"] {
                let e = validate_init(&init_args(chain, key)).unwrap_err();
                assert_eq!(e, format!("unsupported chain id {chain} / key {key}"));
            }
        }
    }

    #[test]
    fn stored_chain_check_accepts_profiled_ids_and_refuses_others() {
        // Upgrade requires a profile and nothing more: it does not look at the install gate.
        for (chain, _, _) in EXPECTED {
            assert!(validate_stored_chain(chain).is_ok(), "{chain}");
        }
        for chain in [0u64, 10, 1030, 8454, 84531, 31338, 11155112, u64::MAX] {
            let e = validate_stored_chain(chain).unwrap_err();
            assert_eq!(e, format!("stored chain id {chain} has no bridge chain profile"));
        }
    }
}
