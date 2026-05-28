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
pub mod config;
pub mod settlement_queue;
pub mod supply;

pub use adapter::ChainAdapter;
pub use config::{ChainConfig, ChainId, ChainStatus};
pub use settlement_queue::{SettlementOp, SettlementQueueV1};
pub use supply::{apply_supply_delta, SupplyDelta, SupplyInvariantError};

#[cfg(test)]
mod tests_adapter;
