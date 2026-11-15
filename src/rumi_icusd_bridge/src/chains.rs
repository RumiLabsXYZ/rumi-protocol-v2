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
//! - the gas and log-scan values a fresh instance starts with (`defaults`);
//! - the user-facing finality wording (`consent_wait`, `finality_label`).
//!
//! Base, Ethereum, Arbitrum and Monad rows are installable at source level. Robinhood
//! Chain and HyperEVM stay `Disabled` with the reason that blocks them, so the table can
//! describe a chain without the canister being able to serve it. An enabled row is not an
//! activation: a fresh instance still starts with deposits paused and no contract bound,
//! and what each chain needs before real use (a testnet round trip, measured gas, an
//! approved launch) is tracked outside this file. Install (`state::validate_init`) is
//! where enablement is checked. Upgrade (`post_upgrade`) only requires that the stored
//! chain id still has a row, so a module that later disables a row never strands an
//! instance that was installed while the row was enabled.

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

/// The operator settings a fresh instance of a chain starts with. Install copies them into
/// the stored config (`state::validate_init`); after that they are ordinary settings that
/// `admin_set_gas` and `admin_set_observer_config` change, inside the same global bounds on
/// every chain. A row never reads them again, and upgrade never rewrites them.
///
/// Fields mirror `state::GasSettings` and the one observer field that depends on the chain.
/// Every other observer setting and every bridge limit starts at the shared default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainDefaults {
    /// Cap on `maxFeePerGas`, in wei.
    pub max_fee_per_gas_wei: u128,
    /// Cap on `maxPriorityFeePerGas`, in wei.
    pub max_priority_fee_per_gas_wei: u128,
    pub mint_gas_limit: u64,
    pub admin_gas_limit: u64,
    /// Wei the lane address keeps above `gas_limit * max_fee` (see `evm::lane::gas_required`).
    pub min_gas_reserve_wei: u128,
    pub replace_after_secs: u64,
    /// Widest `eth_getLogs` block range the observer starts with.
    pub logs_chunk_blocks: u64,
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
    /// Gas and log-scan values a fresh instance starts with.
    pub defaults: ChainDefaults,
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

const ETH_WAIT: &str = "after Ethereum finalization, about 13 minutes";
const ETH_LABEL: &str = "Ethereum `finalized` head (about 2 epochs)";

const ARB_WAIT: &str = "after the Arbitrum batch is finalized on Ethereum, about 13 minutes";
const ARB_LABEL: &str = "Arbitrum `finalized` head (parent-chain batch finality)";

const RHOOD_BLOCKER: &str = "robinhood-finalized-tag-probe";
const RHOOD_NOT_ACTIVE: &str = "robinhood-finalized-tag-probe: install waits for the recorded `finalized` tag probe (BB-46) and at least two independent keyless providers";
const RHOOD_WAIT: &str = "after Robinhood Chain finality (timing not yet confirmed)";
const RHOOD_LABEL: &str = "Robinhood Chain `finalized` head";

const HYPER_BLOCKER: &str = "hyperevm-historical-state";
const HYPER_NOT_ACTIVE: &str = "hyperevm-historical-state: install waits for archive-capable independent providers that answer numbered finalized eth_call, receipts and logs, or a reviewed observer redesign that needs no historical state";
const HYPER_WAIT: &str = "after HyperEVM finality (timing not yet confirmed)";
const HYPER_LABEL: &str = "HyperEVM head";

/// Blocks between Monad Finalized and Verified (state root finality), from Monad's
/// developer summary (see the reconciliation in `docs/coordination/multichain/sources-evm.md`).
pub const MONAD_VERIFIED_DELAY_BLOCKS: u64 = 3;
const MONAD_WAIT: &str = "after Monad Verified state (Finalized plus 3 blocks), within seconds";
const MONAD_LABEL: &str = "Monad Verified head";

// ---------------------------------------------------------------------------
// Per-chain defaults
//
// Each value is labelled `sourced` (with the facts file row, `S` = sources-evm.md) or
// `unsourced` (a conservative choice with no measurement behind it). Unsourced values are
// starting points for `admin_set_gas` and `admin_set_observer_config`, never findings; the
// bounds those calls enforce (`admin::validate_gas`, `admin::validate_observer`) are the
// same on every chain, and a test checks that every default sits inside them.
// ---------------------------------------------------------------------------

const GWEI: u128 = 1_000_000_000;
const ETHER: u128 = 1_000_000_000_000_000_000;

/// Base, and the local chain that stands in for it. These are the values `GasSettings` and
/// `ObserverSettings` have always defaulted to, so a Base instance starts exactly as before
/// (pinned by `chains::tests::base_defaults_are_byte_identical_to_the_pre_table_config`).
/// Measured on anvil: the most expensive mint uses 152,390 gas and the most expensive admin
/// call 46,905, so 300,000 and 100,000 keep about 2x headroom.
const BASE_DEFAULTS: ChainDefaults = ChainDefaults {
    max_fee_per_gas_wei: 2_000_000_000,
    max_priority_fee_per_gas_wei: 100_000_000,
    mint_gas_limit: 300_000,
    admin_gas_limit: 100_000,
    min_gas_reserve_wei: 500_000_000_000_000,
    replace_after_secs: 90,
    logs_chunk_blocks: 2_000,
};

/// Ethereum and Ethereum Sepolia.
/// - Gas limits 300,000 and 100,000: the Base anvil measurement above (152,390 and 46,905).
///   Ethereum charges the same opcode costs and has no L1 data component, so the figures
///   carry over; they were not re-run on Sepolia. Measured elsewhere, not on this chain.
/// - Fee caps 50 gwei and 2 gwei: unsourced. `chain-evm.md` 2.2 calls the Base cap of 2 gwei
///   Base-tuned and asks for higher caps on Ethereum. 50 gwei lets a mint land when the base
///   fee is anywhere up to 50 gwei and bounds one attempt at `300,000 x 50 gwei` = 0.015 ETH.
/// - Reserve 0.001 ETH: unsourced margin. There is no L1 data fee on Ethereum.
/// - Replace after 90 s (about 7 slots) and 2,000-block log ranges: unsourced, the Base values.
const ETH_DEFAULTS: ChainDefaults = ChainDefaults {
    max_fee_per_gas_wei: 50 * GWEI,
    max_priority_fee_per_gas_wei: 2 * GWEI,
    mint_gas_limit: 300_000,
    admin_gas_limit: 100_000,
    min_gas_reserve_wei: ETHER / 1_000,
    replace_after_secs: 90,
    logs_chunk_blocks: 2_000,
};

/// Arbitrum One, Arbitrum Sepolia and Robinhood Chain (an Arbitrum chain, `S` 4 row 5).
/// - Fee caps 2 gwei and 0.1 gwei: the Base values, kept. Sourced floor: the gas price floor
///   is 0.02 gwei on One and 0.2 gwei on Sepolia and tips are ignored (`S` 3 row 5), so a
///   2 gwei cap is 10x to 100x the floor.
/// - Gas limits 600,000 and 200,000: unsourced, 2x the Base figures. The fee includes a
///   poster (L1 data) charge expressed in gas, so `eth_estimateGas` and gas used exceed the
///   execution-only anvil measurement (`S` 3 row 5, `chain-evm.md` 2.3). The mint
///   simulation runs at the configured limit and holds on out-of-gas, which lowers the risk
///   that a low limit costs a paid revert; it cannot rule one out, because state or fees can
///   change between simulation and inclusion. Re-measure on Arbitrum Sepolia before real use.
/// - Reserve 0.0005 ETH, replace after 90 s, 2,000-block ranges: unsourced, the Base values.
const ARB_DEFAULTS: ChainDefaults = ChainDefaults {
    max_fee_per_gas_wei: 2_000_000_000,
    max_priority_fee_per_gas_wei: 100_000_000,
    mint_gas_limit: 600_000,
    admin_gas_limit: 200_000,
    min_gas_reserve_wei: 500_000_000_000_000,
    replace_after_secs: 90,
    logs_chunk_blocks: 2_000,
};

/// HyperEVM (the row stays disabled).
/// - Log ranges 50 blocks: sourced, `eth_getLogs` is limited to 50 blocks (`S` 5 row 4).
/// - Everything else: the Base values, unsourced. `eth_maxPriorityFeePerGas` is always 0
///   (`S` 5 row 4), which the fee code handles; HyperEVM's fee scale and mint gas are
///   unmeasured and small blocks hold 3M gas (`S` 5 row 2), well above these limits.
const HYPER_DEFAULTS: ChainDefaults = ChainDefaults {
    max_fee_per_gas_wei: 2_000_000_000,
    max_priority_fee_per_gas_wei: 100_000_000,
    mint_gas_limit: 300_000,
    admin_gas_limit: 100_000,
    min_gas_reserve_wei: 500_000_000_000_000,
    replace_after_secs: 90,
    logs_chunk_blocks: 50,
};

/// Monad and Monad testnet.
/// - Max fee 500 gwei: the base fee minimum is 100 gwei (`S` 6 row 5; sourced for mainnet,
///   assumed for the testnet), so any cap at or below it can never land a transaction and
///   the Base cap of 2 gwei is useless here. 500 gwei is 5x the minimum, unsourced headroom,
///   and under the 1,000 gwei bound `admin::validate_gas` applies on every chain.
/// - Priority cap 2 gwei: unsourced.
/// - Reserve 15 MON: sourced floor of 10 MON per account (`S` 6 row 5: a transaction that dips
///   below it can be included and still revert while paying gas), plus 5 MON of margin
///   (unsourced). `gas_required` adds it on top of `gas_limit x max_fee` (0.25 MON here).
/// - Gas limits 500,000 and 150,000: unsourced. The sender pays `gas_limit x price`, not gas
///   used (`S` 6 row 5), but Monad reprices some opcodes (`S` 6 row 2), so the Base anvil
///   figure is not transferable. Both lie inside the shared bounds (250,000..=1,000,000
///   and 80,000..=1,000,000). The mint simulation runs at the configured limit and holds
///   on out-of-gas. Re-measure on Monad testnet before real use.
/// - Log ranges 100 blocks: sourced, provider caps run 100 to 1,000 blocks (`S` 6 row 4).
/// - Replace after 90 s: unsourced (there is no global mempool, so a dropped transaction is
///   replaced by the lane after this wait).
const MONAD_DEFAULTS: ChainDefaults = ChainDefaults {
    max_fee_per_gas_wei: 500 * GWEI,
    max_priority_fee_per_gas_wei: 2 * GWEI,
    mint_gas_limit: 500_000,
    admin_gas_limit: 150_000,
    min_gas_reserve_wei: 15 * ETHER,
    replace_after_secs: 90,
    logs_chunk_blocks: 100,
};

/// Base mainnet, Base Sepolia and the local Anvil chain all share the `base` slug, so a
/// Base instance keeps the identifiers it had before the table existed. Ethereum, Arbitrum
/// and Monad rows are installable at source level; Robinhood Chain and HyperEVM are
/// described but not installable.
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
        defaults: BASE_DEFAULTS,
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
        defaults: BASE_DEFAULTS,
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
        defaults: BASE_DEFAULTS,
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
        install: InstallGate::Enabled,
        defaults: ETH_DEFAULTS,
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
        install: InstallGate::Enabled,
        defaults: ETH_DEFAULTS,
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
        install: InstallGate::Enabled,
        defaults: ARB_DEFAULTS,
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
        install: InstallGate::Enabled,
        defaults: ARB_DEFAULTS,
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
        defaults: ARB_DEFAULTS,
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
        defaults: ARB_DEFAULTS,
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
        install: InstallGate::Disabled(HYPER_NOT_ACTIVE),
        defaults: HYPER_DEFAULTS,
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
        install: InstallGate::Disabled(HYPER_NOT_ACTIVE),
        defaults: HYPER_DEFAULTS,
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
        install: InstallGate::Enabled,
        defaults: MONAD_DEFAULTS,
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
        install: InstallGate::Enabled,
        defaults: MONAD_DEFAULTS,
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
    use crate::evm::rpc::{self, testutil, BlockRef, BlockTag, RpcError};
    use crate::state::{self, validate_init, BridgeConfig, GasSettings, ObserverSettings, CHAIN_BASE_MAINNET, CHAIN_BASE_SEPOLIA, CHAIN_LOCAL_ANVIL};
    use crate::types::{BridgeToBaseArgs, EthMainnetService, EthSepoliaService, InitArgs, L2MainnetService, RpcServiceConfig};
    use candid::{Encode, Principal};
    use std::cell::RefCell;
    use std::rc::Rc;

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
    /// The rows BB-45 opens for install, with the one key each accepts.
    const OPEN_PAIRS: [(u64, &str); 6] =
        [(1, "key_1"), (11155111, "test_key_1"), (42161, "key_1"), (421614, "test_key_1"), (143, "key_1"), (10143, "test_key_1")];
    /// The rows that stay held, each with the blocker its head rule names.
    const HELD: [(u64, &str); 4] = [
        (4663, "robinhood-finalized-tag-probe"),
        (46630, "robinhood-finalized-tag-probe"),
        (999, "hyperevm-historical-state"),
        (998, "hyperevm-historical-state"),
    ];
    /// Every (chain, key) pair install accepts.
    const ACCEPTED: [(u64, &str); 11] = [
        (8453, "key_1"),
        (84532, "key_1"),
        (84532, "test_key_1"),
        (31337, "test_key_1"),
        (31337, "dfx_test_key"),
        (1, "key_1"),
        (11155111, "test_key_1"),
        (42161, "key_1"),
        (421614, "test_key_1"),
        (143, "key_1"),
        (10143, "test_key_1"),
    ];

    /// Install `chain` with the mock-addressable providers `p1..p3` and quorum 2.
    fn installed(chain: u64, key: &str) -> BridgeConfig {
        let mut a = init_args(chain, key);
        a.rpc_services = testutil::services(3);
        validate_init(&a).unwrap_or_else(|e| panic!("chain {chain} / {key} must install: {e}"))
    }

    /// Start a fresh canister state from an installed config.
    fn boot(config: BridgeConfig) {
        state::reset_for_tests();
        state::mutate_core(|c| {
            c.initialized = true;
            c.config = config;
        });
    }

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
    fn exactly_the_planned_rows_are_enabled_and_every_held_row_names_its_blocker() {
        let open: Vec<u64> = OPEN_PAIRS.iter().map(|(id, _)| *id).collect();
        for p in &CHAINS {
            let held = HELD.iter().find(|(id, _)| *id == p.chain_id);
            match (BASE_IDS.contains(&p.chain_id) || open.contains(&p.chain_id), held, p.install) {
                (true, None, InstallGate::Enabled) => {}
                (false, Some((_, blocker)), InstallGate::Disabled(reason)) => {
                    // The install reason leads with the same blocker id the head rule carries.
                    assert!(reason.starts_with(blocker), "{}: {reason}", p.chain_id);
                    assert_eq!(p.head_rule, HeadRule::Blocked(blocker), "{}", p.chain_id);
                }
                (is_open, held, gate) => panic!("chain {} (open: {is_open}, held: {held:?}) has gate {gate:?}", p.chain_id),
            }
        }
        // The held reasons say what lifts them.
        let reason = |id: u64| match profile(id).unwrap().install {
            InstallGate::Disabled(r) => r,
            InstallGate::Enabled => panic!("{id} is enabled"),
        };
        for id in [4663u64, 46630] {
            assert!(reason(id).contains("BB-46") && reason(id).contains("two independent keyless providers"), "{}", reason(id));
        }
        for id in [999u64, 998] {
            assert!(reason(id).contains("archive-capable independent providers") && reason(id).contains("reviewed observer redesign"), "{}", reason(id));
        }
        // A Blocked head rule is never paired with an enabled row, and an enabled row never
        // reads a head without a rule that can answer.
        for p in &CHAINS {
            assert_eq!(matches!(p.head_rule, HeadRule::Blocked(_)), matches!(p.install, InstallGate::Disabled(_)), "{}", p.chain_id);
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
    fn the_opened_rows_keep_their_key_rule_head_rule_and_slug() {
        // BB-45 changes the gate and adds defaults; the key rule, head rule and slug of
        // every row are the ones BB-42 and BB-43 gave it.
        for (id, rule, slug) in [
            (1u64, HeadRule::TagFinalizedQuorum, "eth"),
            (11155111, HeadRule::TagFinalizedQuorum, "eth"),
            (42161, HeadRule::TagFinalizedQuorum, "arb"),
            (421614, HeadRule::TagFinalizedQuorum, "arb"),
            (143, HeadRule::MonadVerified { delay_blocks: 3 }, "monad"),
            (10143, HeadRule::MonadVerified { delay_blocks: 3 }, "monad"),
        ] {
            let p = profile(id).unwrap();
            assert_eq!(p.install, InstallGate::Enabled, "{id}");
            assert_eq!(p.head_rule, rule, "{id}");
            assert_eq!(p.slug, slug, "{id}");
            let keys: &[&str] = if p.role == ChainRole::Mainnet { &["key_1"] } else { &["test_key_1"] };
            assert_eq!(p.allowed_keys, keys, "{id}");
        }
    }

    #[test]
    fn every_chain_install_admits_has_a_profile() {
        // Every (chain, key) pair `validate_init` accepts. If install ever admits a chain
        // without a profile, `ids` would panic on it, so this must stay true.
        for (chain, key) in ACCEPTED {
            let config = validate_init(&init_args(chain, key)).unwrap_or_else(|e| panic!("{chain}/{key} must be accepted: {e}"));
            assert_eq!(config.chain_id, chain);
            assert!(profile(config.chain_id).is_some(), "chain {chain} is admitted by install but has no profile");
            assert_eq!(profile(chain).unwrap().install, InstallGate::Enabled, "{chain}");
        }
        assert_eq!(profile(CHAIN_BASE_MAINNET).map(|p| p.chain_id), Some(8453));
        assert_eq!(profile(CHAIN_BASE_SEPOLIA).map(|p| p.chain_id), Some(84532));
        assert_eq!(profile(CHAIN_LOCAL_ANVIL).map(|p| p.chain_id), Some(31337));
    }

    #[test]
    fn install_accepts_exactly_the_open_pairs_and_refuses_the_held_rows() {
        // The full (row x key) truth table over a key list that includes every key name the
        // table uses plus junk. Accepted: the five Base pairs and one key per opened row.
        let keys = ["key_1", "test_key_1", "dfx_test_key", "", "KEY_1", "other"];
        for p in &CHAINS {
            for key in keys {
                let result = validate_init(&init_args(p.chain_id, key));
                let expect_ok = ACCEPTED.contains(&(p.chain_id, key));
                assert_eq!(result.is_ok(), expect_ok, "chain {} key {key:?}: {result:?}", p.chain_id);
            }
        }
        // The held rows refuse every key (the gate comes first), the opened rows refuse the
        // wrong key with the row's own wording.
        for (id, _) in HELD {
            for key in keys {
                assert!(validate_init(&init_args(id, key)).unwrap_err().contains("cannot be installed"), "{id} {key:?}");
            }
        }
        for (id, key) in OPEN_PAIRS {
            let p = profile(id).unwrap();
            for wrong in keys.into_iter().filter(|k| *k != key) {
                let e = validate_init(&init_args(id, wrong)).unwrap_err();
                match p.role {
                    ChainRole::Mainnet => assert_eq!(e, format!("{} mainnet requires key_1, got {wrong}", p.display_name)),
                    _ => assert_eq!(e, format!("unsupported chain id {id} / key {wrong}")),
                }
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

    // ----- per-chain defaults ------------------------------------------------------

    /// The config the Base-only install built before the table carried defaults.
    fn pre_defaults_config(chain: u64, key: &str) -> BridgeConfig {
        let a = init_args(chain, key);
        BridgeConfig {
            icusd_ledger: a.icusd_ledger,
            evm_rpc: a.evm_rpc,
            ecdsa_key_name: a.ecdsa_key_name.clone(),
            chain_id: a.base_chain_id,
            admins: a.admins.clone(),
            fee_recipient: a.fee_recipient,
            rpc_services: a.rpc_services.clone(),
            min_quorum: a.min_quorum,
            ..BridgeConfig::default()
        }
    }

    #[test]
    fn base_defaults_are_byte_identical_to_the_pre_table_config() {
        // The numbers a Base instance has always started with, spelled out.
        let gas = GasSettings {
            max_fee_per_gas_wei: 2_000_000_000,
            max_priority_fee_per_gas_wei: 100_000_000,
            mint_gas_limit: 300_000,
            admin_gas_limit: 100_000,
            min_gas_reserve_wei: 500_000_000_000_000,
            replace_after_secs: 90,
        };
        assert_eq!(GasSettings::default(), gas);
        assert_eq!(ObserverSettings::default().logs_chunk_blocks, 2_000);
        for id in BASE_IDS {
            let row = profile(id).unwrap();
            assert_eq!(row.defaults, BASE_DEFAULTS, "{id}");
            assert_eq!(state::gas_defaults(&row.defaults), gas, "{id}");
            assert_eq!(row.defaults.logs_chunk_blocks, ObserverSettings::default().logs_chunk_blocks, "{id}");
        }
        // The whole installed config, encoded as it is stored, equals what the install built
        // before defaults existed: not one stored byte of a Base install moved.
        for (chain, key) in ACCEPTED.iter().filter(|(c, _)| BASE_IDS.contains(c)) {
            let got = validate_init(&init_args(*chain, key)).unwrap();
            let before = pre_defaults_config(*chain, key);
            assert_eq!(got, before, "{chain}/{key}");
            assert_eq!(state::cbor_encode(&got), state::cbor_encode(&before), "{chain}/{key}");
        }
    }

    #[test]
    fn install_applies_the_row_defaults_and_changes_nothing_else() {
        for (chain, key) in ACCEPTED {
            let row = profile(chain).unwrap();
            let got = validate_init(&init_args(chain, key)).unwrap();
            assert_eq!(got.gas, state::gas_defaults(&row.defaults), "{chain}");
            assert_eq!(got.observer.logs_chunk_blocks, row.defaults.logs_chunk_blocks, "{chain}");
            // Every other field is what the install built before defaults existed.
            let mut rest = got.clone();
            rest.gas = GasSettings::default();
            rest.observer = ObserverSettings::default();
            assert_eq!(rest, pre_defaults_config(chain, key), "{chain}");
            // And the observer's other settings keep the shared defaults.
            assert_eq!(
                ObserverSettings { logs_chunk_blocks: ObserverSettings::default().logs_chunk_blocks, ..got.observer.clone() },
                ObserverSettings::default(),
                "{chain}"
            );
            // A fresh instance still starts with deposits paused and no contract bound.
            assert!(got.pauses.deposits && got.contract.is_none(), "{chain}");
        }
    }

    #[test]
    fn every_row_default_sits_inside_the_bounds_admin_enforces() {
        // A default the admin calls would refuse to set would be a trap: `admin_set_gas`
        // re-validates the whole record, so an out-of-bounds default could never be edited.
        for p in &CHAINS {
            let gas = state::gas_defaults(&p.defaults);
            crate::admin::validate_gas(&gas).unwrap_or_else(|e| panic!("chain {}: {e}", p.chain_id));
            let observer = ObserverSettings { logs_chunk_blocks: p.defaults.logs_chunk_blocks, ..ObserverSettings::default() };
            crate::admin::validate_observer(&observer).unwrap_or_else(|e| panic!("chain {}: {e}", p.chain_id));
        }
    }

    #[test]
    fn monad_defaults_follow_the_sourced_figures() {
        for id in [143u64, 10143] {
            let d = profile(id).unwrap().defaults;
            // Base fee minimum 100 gwei (S 6 row 5): the cap must exceed it or nothing lands.
            assert!(d.max_fee_per_gas_wei > 100 * GWEI, "{id}");
            assert!(d.max_priority_fee_per_gas_wei <= d.max_fee_per_gas_wei, "{id}");
            // Reserve balance 10 MON per account (S 6 row 5), plus a margin.
            assert!(d.min_gas_reserve_wei > 10 * ETHER, "{id}");
            // Provider log caps run 100 to 1,000 blocks (S 6 row 4): start at the narrowest.
            assert!(d.logs_chunk_blocks <= 100, "{id}");
            // One attempt at the cap costs well under the reserve margin, so a lane funded to
            // the reserve can send several before the balance gets near 10 MON.
            let attempt = d.mint_gas_limit as u128 * d.max_fee_per_gas_wei;
            assert!(attempt < ETHER, "{id}: {attempt}");
            assert!(d.min_gas_reserve_wei - 10 * ETHER > 10 * attempt, "{id}");
        }
        // And the literal figures, so a change here is a visible decision.
        let d = profile(143).unwrap().defaults;
        assert_eq!(
            (d.max_fee_per_gas_wei, d.max_priority_fee_per_gas_wei, d.mint_gas_limit, d.admin_gas_limit, d.min_gas_reserve_wei, d.logs_chunk_blocks),
            (500_000_000_000, 2_000_000_000, 500_000, 150_000, 15_000_000_000_000_000_000, 100)
        );
        assert_eq!(profile(10143).unwrap().defaults, d);
    }

    #[test]
    fn other_chains_start_from_their_own_documented_defaults() {
        // HyperEVM: eth_getLogs is limited to 50 blocks (S 5 row 4); the row stays held.
        for id in [999u64, 998] {
            assert!(profile(id).unwrap().defaults.logs_chunk_blocks <= 50, "{id}");
        }
        // Ethereum: the Base cap of 2 gwei is Base-tuned (chain-evm.md 2.2), so it is higher.
        for id in [1u64, 11155111] {
            let d = profile(id).unwrap().defaults;
            assert!(d.max_fee_per_gas_wei > BASE_DEFAULTS.max_fee_per_gas_wei, "{id}");
            assert_eq!((d.max_fee_per_gas_wei, d.max_priority_fee_per_gas_wei), (50_000_000_000, 2_000_000_000), "{id}");
            assert_eq!((d.mint_gas_limit, d.admin_gas_limit), (300_000, 100_000), "{id}");
        }
        // Arbitrum and Robinhood Chain: the fee is two-part, so the gas limits start higher
        // than Base's execution-only figures; fee caps stay far above the 0.02 gwei floor.
        for id in [42161u64, 421614, 4663, 46630] {
            let d = profile(id).unwrap().defaults;
            assert!(d.mint_gas_limit > BASE_DEFAULTS.mint_gas_limit && d.admin_gas_limit > BASE_DEFAULTS.admin_gas_limit, "{id}");
            assert_eq!((d.mint_gas_limit, d.admin_gas_limit), (600_000, 200_000), "{id}");
            assert!(d.max_fee_per_gas_wei >= 100 * 20_000_000, "{id}: at least 100x the 0.02 gwei floor");
        }
        // Rows of one chain family share their defaults.
        assert_eq!(profile(1).unwrap().defaults, profile(11155111).unwrap().defaults);
        assert_eq!(profile(42161).unwrap().defaults, profile(421614).unwrap().defaults);
    }

    // ----- opened chains, end to end through the local seams -------------------------

    #[test]
    fn rpc_validation_per_role_on_every_opened_chain() {
        let https = testutil::services(3);
        let http: Vec<RpcServiceConfig> = (1..=3).map(|i| RpcServiceConfig::Custom { url: format!("http://p{i}.example") }).collect();
        let install = |chain: u64, key: &str, services: &[RpcServiceConfig], quorum: u8| {
            let mut a = init_args(chain, key);
            a.rpc_services = services.to_vec();
            a.min_quorum = quorum;
            validate_init(&a)
        };
        for (chain, key) in OPEN_PAIRS {
            let row = profile(chain).unwrap();
            // Custom https providers are valid on every opened row.
            assert!(install(chain, key, &https, 2).is_ok(), "{chain}");
            assert!(install(chain, key, &https, 3).is_ok(), "{chain}");
            // Plain http is refused: only the local chain accepts it.
            assert_eq!(install(chain, key, &http, 2).unwrap_err(), "RPC URL must be https: http://p1.example", "{chain}");
            // A mainnet row needs quorum 2; a testnet row accepts quorum 1.
            let q1 = install(chain, key, &https[..1], 1);
            match row.role {
                ChainRole::Mainnet => assert_eq!(q1.unwrap_err(), format!("{} mainnet requires min_quorum >= 2", row.display_name), "{chain}"),
                _ => assert!(q1.is_ok(), "{chain}"),
            }
            // The Base built-in family is valid for no opened row.
            let base_native = [RpcServiceConfig::BaseMainnet(L2MainnetService::Alchemy), RpcServiceConfig::BaseMainnet(L2MainnetService::Ankr)];
            assert_eq!(
                install(chain, key, &base_native, 2).unwrap_err(),
                "BaseMainnet providers are only valid for chain 8453",
                "{chain}"
            );
        }

        // Ethereum's built-in providers (BB-44) install on Ethereum and Ethereum Sepolia, and
        // only on their own network.
        let eth: Vec<RpcServiceConfig> =
            [EthMainnetService::Alchemy, EthMainnetService::Ankr, EthMainnetService::Cloudflare].into_iter().map(RpcServiceConfig::EthMainnet).collect();
        let sepolia: Vec<RpcServiceConfig> =
            [EthSepoliaService::Alchemy, EthSepoliaService::PublicNode, EthSepoliaService::Sepolia].into_iter().map(RpcServiceConfig::EthSepolia).collect();
        let arb: Vec<RpcServiceConfig> =
            [L2MainnetService::Alchemy, L2MainnetService::Ankr, L2MainnetService::Llama].into_iter().map(RpcServiceConfig::ArbitrumOne).collect();
        assert!(install(1, "key_1", &eth, 2).is_ok());
        assert!(install(11155111, "test_key_1", &sepolia, 1).is_ok());
        assert!(install(42161, "key_1", &arb, 2).is_ok());
        assert_eq!(install(11155111, "test_key_1", &eth, 2).unwrap_err(), "EthMainnet providers are only valid for chain 1");
        assert_eq!(install(1, "key_1", &sepolia, 2).unwrap_err(), "EthSepolia providers are only valid for chain 11155111");
        assert_eq!(install(421614, "test_key_1", &arb, 2).unwrap_err(), "ArbitrumOne providers are only valid for chain 42161");
        // Monad and Arbitrum Sepolia have no built-in family: custom URLs only.
        for (chain, key) in [(143u64, "key_1"), (10143, "test_key_1"), (421614, "test_key_1")] {
            assert!(profile(chain).unwrap().native.is_none(), "{chain}");
            for set in [&eth, &sepolia, &arb] {
                assert!(install(chain, key, set, 2).is_err(), "{chain}");
            }
        }
    }

    #[test]
    fn an_installed_chain_reads_its_head_by_its_rule() {
        // Install each opened chain, then read the settlement head through the existing mock
        // transport: the tag-quorum rows send `finalized` and take that height, Monad takes
        // `finalized` minus 3 and pins view calls to that number.
        for (chain, key) in OPEN_PAIRS {
            let row = profile(chain).unwrap();
            boot(installed(chain, key));
            assert_eq!(rpc::head_rule(), Ok(row.head_rule), "{chain}");

            let seen: Rc<RefCell<Vec<String>>> = Rc::default();
            let sink = seen.clone();
            rpc::set_mock_transport(move |_, body| {
                let (method, params) = testutil::request_of(body);
                assert_eq!(method, "eth_getBlockByNumber");
                let tag = params[0].as_str().unwrap().to_string();
                let n = if tag == "finalized" { 1_000 } else { u64::from_str_radix(tag.trim_start_matches("0x"), 16).unwrap() };
                sink.borrow_mut().push(tag);
                Ok(testutil::ok_body(testutil::block_json(n, testutil::h32(n, 0))))
            });
            let head = testutil::block_on(rpc::finalized_head());
            let view_tag = testutil::block_on(rpc::settlement_block_tag());
            let want = match row.head_rule {
                HeadRule::TagFinalizedQuorum => 1_000,
                HeadRule::MonadVerified { delay_blocks } => 1_000 - delay_blocks,
                HeadRule::Blocked(r) => panic!("{chain} is blocked: {r}"),
            };
            assert_eq!(head, Ok(BlockRef { number: want, hash: testutil::h32(want, 0) }), "{chain}");
            match row.head_rule {
                HeadRule::TagFinalizedQuorum => {
                    assert_eq!(view_tag, Ok(BlockTag::Finalized), "{chain}");
                    // Three providers, level: one round of `finalized`, no refetch.
                    assert_eq!(*seen.borrow(), vec!["finalized"; 3], "{chain}");
                }
                _ => {
                    assert_eq!(view_tag, Ok(BlockTag::Number(997)), "{chain}");
                    // `finalized` from each provider, then block 997 from each, twice (head and view tag).
                    let seen = seen.borrow();
                    assert_eq!(seen.iter().filter(|t| *t == "finalized").count(), 6, "{chain}");
                    assert_eq!(seen.iter().filter(|t| t.as_str() == "0x3e5").count(), 6, "{chain}");
                }
            }
            // Nothing the chain sent is a moving tag.
            for t in seen.borrow().iter() {
                assert!(t == "finalized" || t.starts_with("0x"), "{chain}: {t}");
            }
        }
    }

    #[test]
    fn a_held_chain_never_reads_a_head_even_with_providers_configured() {
        for (chain, blocker) in HELD {
            // Held rows cannot be installed, so boot the state a stored config would give.
            let mut config = pre_defaults_config(CHAIN_BASE_SEPOLIA, "test_key_1");
            config.chain_id = chain;
            config.rpc_services = testutil::services(3);
            boot(config);
            rpc::set_mock_transport(|_, _| panic!("a held chain asked a provider"));
            match testutil::block_on(rpc::finalized_head()) {
                Err(RpcError::NotConfigured(m)) => assert!(m.contains(blocker), "{chain}: {m}"),
                other => panic!("{chain}: {other:?}"),
            }
        }
    }

    #[test]
    fn consent_and_finality_text_of_an_installed_chain_read_its_row() {
        let tx_args = |fee: u64| {
            Encode!(&BridgeToBaseArgs {
                amount_e8s: 250_000_000,
                base_recipient: "0x00000000000000000000000000000000000000aa".into(),
                from_subaccount: None,
                client_request_id: serde_bytes::ByteBuf::from(vec![1u8; 16]),
                expected_deposit_fee_e8s: fee,
            })
            .unwrap()
        };
        for (chain, key) in OPEN_PAIRS {
            let row = profile(chain).unwrap();
            boot(installed(chain, key));
            let text = crate::icrc21::consent_text("bridge_to_base", &tx_args(0)).unwrap();
            assert!(text.starts_with(&format!("## Bridge icUSD to {}\n", row.display_name)), "{text}");
            assert!(text.contains(&format!("icUSD on {} (chain {chain}) to", row.display_name)), "{text}");
            assert!(text.contains(&format!("* Delivery is expected {}.", row.consent_wait)), "{text}");
            assert!(!text.contains("Base"), "chain {chain} must not name Base: {text}");

            let info = crate::views::bridge_info();
            assert_eq!(info.chain_id, chain);
            let policy = info.finality_policy;
            assert!(policy.starts_with(row.finality_label), "{policy}");
            assert!(policy.contains("agreed by 2 of 3 providers") && policy.ends_with("confirmed by the same quorum"), "{policy}");
            assert_eq!(policy.contains("minus 3 blocks"), matches!(row.head_rule, HeadRule::MonadVerified { .. }), "{policy}");
            assert!(!policy.contains("Base"), "{policy}");

            // The views show the row's defaults, and a fresh instance is paused with no contract.
            assert_eq!(info.gas.max_fee_per_gas_wei, candid::Nat::from(row.defaults.max_fee_per_gas_wei), "{chain}");
            assert_eq!(info.gas.mint_gas_limit, row.defaults.mint_gas_limit, "{chain}");
            assert_eq!(info.gas.min_gas_reserve_wei, candid::Nat::from(row.defaults.min_gas_reserve_wei), "{chain}");
            assert_eq!(info.observer_config.logs_chunk_blocks, row.defaults.logs_chunk_blocks, "{chain}");
            assert!(info.pauses.deposits && info.contract.is_none(), "{chain}");
        }
    }

    #[test]
    fn the_admin_gas_path_accepts_every_opened_chains_defaults_and_nothing_wider_than_before() {
        // `admin_set_gas` and `admin_set_observer_config` keep one set of global bounds: a
        // record equal to each row's defaults is accepted, and the Base-era ceilings are the
        // ones that still refuse anything beyond them.
        for p in &CHAINS {
            assert!(crate::admin::validate_gas(&state::gas_defaults(&p.defaults)).is_ok(), "{}", p.chain_id);
        }
        let mut over = state::gas_defaults(&profile(143).unwrap().defaults);
        over.max_fee_per_gas_wei = 1_000_000_000_001;
        assert!(crate::admin::validate_gas(&over).is_err());
        let mut over = state::gas_defaults(&profile(143).unwrap().defaults);
        over.mint_gas_limit = state::MAX_GAS_LIMIT + 1;
        assert!(crate::admin::validate_gas(&over).is_err());
        let mut under = state::gas_defaults(&profile(143).unwrap().defaults);
        under.mint_gas_limit = state::MIN_MINT_GAS_LIMIT - 1;
        assert!(crate::admin::validate_gas(&under).is_err());
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
