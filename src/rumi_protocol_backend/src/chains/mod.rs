//! STATUS (2026-08-20): the Conflux (CFX/eSpace) chains-liquidation rail is
//! code-complete and dormant, not experimental scaffolding. Increments 0-13
//! (PRs #261-#286, June 2026) built the full engine: EIP-712 vault auth,
//! observer/settlement workers, the liquidation bot path, and the SP
//! escalation. It is dormant purely because prod carries no per-chain config
//! row; registering a chain and setting its liquidation config IS the public
//! launch (the `_evm` vault endpoints are EIP-712-signature-authed, not
//! dev-gated, so there is no separate "flip a switch" step).
//!
//! Go-live checklist for the first prod chain (Conflux, chain id 1030):
//!  1. `set_chains_ecdsa_key_name("key_1")` BEFORE the first prod chain vault
//!     opens (the key locks in at first vault use). Prod currently carries
//!     "test_key_1" (verified 2026-08-20).
//!  2. Register chain 1030 with MAINNET-shaped args
//!     (`conflux_mainnet_register_arg`): finality_depth 400,
//!     min_quorum_providers 2, operator-vetted RPC URLs.
//!  3. Deploy IcUSD.sol on eSpace mainnet, then `set_chain_contract`.
//!  4. Set a liquidation config row (real Swappi router, fee/divergence/
//!     deadline). This ALSO activates the XRC-sourced CFX price timer for
//!     the chain (see `xrc::chains_needing_price_feed`); an unconfigured
//!     chain makes zero XRC calls.
//!  5. Verify `get_evm_rpc_principal` reports the official EVM-RPC canister
//!     (no override left pointing at a mock).
//!  6. Fund nothing: the reserve address is a sink; the custody address pays
//!     gas from deposited collateral.
//!
//! Monad and Solana remain genuinely experimental: testnet/devnet only,
//! observer/settlement timers off by default (Solana also gated behind
//! `solana_workers_enabled`), and their write paths are still developer-
//! gated pending the same kind of review pass Conflux already had.

pub mod adapter;
pub mod admin;
pub mod collateral_config;
pub mod config;
pub mod interest;
pub mod liquidation;
pub mod liquidation_config;
pub mod multi_chain_state;
pub mod recovery;
pub mod settlement_queue;
pub mod supply;
pub mod vault;
pub mod evm;
pub mod monad;
pub mod solana;
pub mod xrp;

pub use adapter::ChainAdapter;
pub use config::{ChainConfig, ChainId, ChainStatus};
pub use liquidation_config::{ChainLiquidationConfigV1, DexKind, LiquidationConfigError};
pub use multi_chain_state::{
    MultiChainState, MultiChainStateV1, MultiChainStateV2, MultiChainStateV3, MultiChainStateV4,
    MultiChainStateV5, MultiChainStateV6,
};
pub use settlement_queue::{SettlementOp, SettlementQueueV1};
pub use supply::{apply_supply_delta, SupplyDelta, SupplyInvariantError};

#[cfg(test)]
mod tests_adapter;

#[cfg(test)]
mod tests_config;

#[cfg(test)]
mod tests_settlement_queue;

#[cfg(test)]
mod tests_multi_chain_state;

#[cfg(test)]
mod tests_multi_chain_state_v2;

#[cfg(test)]
mod tests_supply;

#[cfg(test)]
mod tests_admin;

#[cfg(test)]
mod tests_recovery;

#[cfg(test)]
mod tests_self_check;

#[cfg(test)]
mod tests_vault;
