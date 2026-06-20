//! ⚠️ EXPERIMENTAL — NOT WIRED UP FOR PRODUCTION. LEAVE ALONE / REVISIT LATER. ⚠️
//!
//! The entire `chains/` tree (Monad + Solana cross-chain CDP) is experimental
//! and intentionally dormant: the observer/settlement timers are OFF by default,
//! every cross-chain write endpoint is developer-gated, and the chains run on
//! testnet/devnet only. It is NOT part of the production ICP-native protocol.
//!
//! The 2026-06-05 security audit (`audit-reports/2026-06-05-d67e100/`) added the
//! M-04..M-09 quorum / finality / recovery remediations here (endpoint dedup,
//! `min_quorum_providers` fail-closed floor, quorumed finality probe, finality-
//! aware deposit cursor, on-chain re-verify in resolve/recover). That work
//! compiles and the backend lib tests pass, BUT it has NOT been exhaustively
//! reviewed, the candid `.did` sync for the new surface is unverified, and the
//! sub-agent that wrote it was stopped before finishing additional tests.
//!
//! DO NOT enable any cross-chain feature (timers, real providers, mainnet) or
//! treat this code as production-ready without a deliberate, careful human
//! review pass first. It is preserved here on purpose — parked, not abandoned.
//!
//! Multi-chain scaffolding (Phase 1a).
//!
//! This module tree carries the chain-agnostic abstractions used by every
//! foreign-chain integration: the `ChainAdapter` trait (adapter.rs), the
//! per-chain configuration record (config.rs), the per-chain settlement
//! queue (settlement_queue.rs), and the supply-invariant accounting helpers
//! (supply.rs).
//!
//! Phase 1a registers no real chain. The trait has no production impls,
//! the settlement queues are never drained, and `chain_supplies` stays
//! empty after install. Phase 1b (Monad) will add the first real adapter
//! and the first non-zero entries.

pub mod adapter;
pub mod admin;
pub mod collateral_config;
pub mod config;
pub mod interest;
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
