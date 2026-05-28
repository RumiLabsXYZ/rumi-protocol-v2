//! Persisted multi-chain root.
//!
//! Lives at `state::State::multi_chain` and carries every chain-aware
//! piece of state in one struct so the AMM-style state-wipe pattern
//! (missing field at decode time -> Default applied silently) cannot
//! happen for any sub-component. Add fields ONLY by:
//!
//! 1. Renaming `MultiChainStateV1` -> keep the V1 fields exactly.
//! 2. Adding `MultiChainStateV2` with the new field plus a `From<V1>` impl.
//! 3. Updating the `pub type MultiChainState = MultiChainStateV2;` alias.
//! 4. Adding a one-line entry to `migrate_multi_chain_state` (see `supply.rs`).
//!
//! See spec Section 3 ("State wipe on upgrade") and the 2026-05-18 AMM
//! incident.

use super::config::{ChainConfigV1, ChainId};
use super::settlement_queue::SettlementQueueV1;
use candid::{CandidType, Deserialize};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, Default)]
pub struct MultiChainStateV1 {
    pub chain_configs: BTreeMap<ChainId, ChainConfigV1>,
    /// Canonical per-chain icUSD supply (e8s). Invariant:
    /// `sum(chain_supplies.values()) == state.total_borrowed_icusd_amount()`
    /// after every state mutation. Enforced by `apply_supply_delta`.
    pub chain_supplies: BTreeMap<ChainId, u128>,
    pub settlement_queues: BTreeMap<ChainId, SettlementQueueV1>,
    /// `true` iff the periodic invariant self-check on Timer B failed the
    /// last time it ran. When set, every entry point into `apply_supply_delta`
    /// returns `SupplyInvariantError::HaltedAfterSelfCheckFailure`.
    /// Cleared only by `clear_invariant_halt` (developer-gated, lands in
    /// Phase 1b along with operational tooling). For Phase 1a the field
    /// exists, defaults to false, and is only set by the self-check.
    pub invariant_halted: bool,
}

impl MultiChainStateV1 {
    pub fn total_supply_all_chains_e8s(&self) -> u128 {
        self.chain_supplies.values().copied().sum()
    }
}

pub type MultiChainState = MultiChainStateV1;
