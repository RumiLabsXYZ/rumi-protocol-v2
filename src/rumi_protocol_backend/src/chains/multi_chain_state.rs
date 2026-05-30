//! Persisted multi-chain root.
//!
//! Lives at `state::State::multi_chain` and carries every chain-aware
//! piece of state in one struct so the AMM-style state-wipe pattern
//! (missing field at decode time -> fallback wipes state) cannot happen for
//! any sub-component.
//!
//! ## Adding a new field (non-breaking reshape)
//!
//! 1. Keep `MultiChainStateVN` exactly as shipped.
//! 2. Add `MultiChainStateV(N+1)` with the new field(s), each annotated with
//!    `#[serde(default)]`. The four original V1 fields must NOT be decorated
//!    because they are always present in any live snapshot.
//! 3. Rebind `pub type MultiChainState = MultiChainStateV(N+1);`.
//! 4. That is it. The V1->V2 (or V2->V3, etc.) decode happens in-place via
//!    ciborium: the old fields map across by name; the new fields hit
//!    `serde_default` and come up empty. No explicit migration call in
//!    `post_upgrade` is needed. `migrate_multi_chain_state` in `supply.rs`
//!    is a unit-tested TEMPLATE for the next version bump, NOT the live path.
//!
//! ## Adding a field that requires a BREAKING reshape
//!
//! (e.g. a field type change the in-place decode cannot handle)
//!
//! 1-3 same as above, then:
//! 4. Add `migrate_vN_to_v(N+1)` in `chains/supply.rs`.
//! 5. Call it from `post_upgrade` in `main.rs` after `restore_state`.
//!
//! See spec Section 3 ("State wipe on upgrade") and the 2026-05-18 AMM
//! incident (MEMORY.md: `project_amm_state_wipe_2026_05_18.md`).

use super::config::{ChainConfigV1, ChainId};
use super::monad::chain_vault::ChainVaultV1;
use super::settlement_queue::SettlementQueueV1;
use candid::{CandidType, Deserialize};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, Default)]
pub struct MultiChainStateV1 {
    pub chain_configs: BTreeMap<ChainId, ChainConfigV1>,
    /// Canonical per-chain icUSD supply (e8s). Phase 1b invariant:
    /// `sum(chain_supplies.values()) == sum(chain_vault.debt_e8s)`
    /// after every state mutation. Enforced by `apply_supply_delta`.
    /// ICP-native debt (`total_borrowed_icusd_amount`) is a separate pool
    /// and is NOT part of this invariant (unification is a Phase 2 task).
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

/// Phase 1b snapshot. Carries the four V1 fields verbatim (so the
/// `#[serde(default)]` in-place decode of `State.multi_chain` maps each by
/// name straight across) and adds the Monad/foreign-chain working set:
/// per-vault records, deployed-contract addresses, manual price overrides,
/// last-observed block cursors, hot-wallet gas balances, per-chain reorg
/// halt flags, and the burn-watch idempotency set (C-1). The new-in-V2 fields
/// carry field-level `#[serde(default)]` so a
/// V1 CBOR snapshot (which lacks these keys entirely) decodes into V2 without
/// error, defaulting the new fields to empty. The four V1-carried fields are
/// NOT decorated because V1 always wrote them and they must be present in any
/// valid snapshot. (`reorg_halted` was added to V2 in Task 11, before V2 had
/// ever been persisted, so it is an in-V2 field rather than a V3 bump.)
///
/// Add the NEXT field by bumping to `MultiChainStateV3` (keep V2 verbatim),
/// adding `#[serde(default)]` on the new field, and rebinding the alias below.
/// For a BREAKING reshape (field type change that the in-place decode cannot
/// handle), add a `migrate_v2_to_v3` in `chains/supply.rs` and call it from
/// `post_upgrade` after `restore_state`.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, Default)]
pub struct MultiChainStateV2 {
    // carried verbatim from V1 — always present in any valid snapshot
    pub chain_configs: BTreeMap<ChainId, ChainConfigV1>,
    pub chain_supplies: BTreeMap<ChainId, u128>,
    pub settlement_queues: BTreeMap<ChainId, SettlementQueueV1>,
    pub invariant_halted: bool,
    // new in V2 — field-level serde(default) lets a V1 snapshot decode cleanly
    #[serde(default)]
    pub chain_vaults: BTreeMap<u64, ChainVaultV1>,
    #[serde(default)]
    pub chain_contracts: BTreeMap<ChainId, String>,
    #[serde(default)]
    pub manual_prices: BTreeMap<(ChainId, String), u64>,
    #[serde(default)]
    pub last_observed_block: BTreeMap<ChainId, u64>,
    #[serde(default)]
    pub hot_wallet_balance_e18: BTreeMap<ChainId, u128>,
    /// Per-chain reorg circuit breaker. Set `true` by the observer when a
    /// finalized-block regression deeper than `finality_depth` is detected
    /// (`hardening::is_reorg`) and CONFIRMED across `REORG_CONFIRM_TICKS`
    /// consecutive observer ticks; halts BOTH the observer and the settlement
    /// worker for that chain until cleared by `clear_reorg_halt` (Task 14).
    /// Added directly to V2 (not a new V3) because V2 is brand-new this phase
    /// and has never been persisted, so no live snapshot lacks this key; the
    /// `#[serde(default)]` is still mandatory state-wipe defense.
    #[serde(default)]
    pub reorg_halted: BTreeMap<ChainId, bool>,
    /// Per-chain count of CONSECUTIVE observer ticks that suspected a reorg
    /// (a finalized-block regression deeper than `finality_depth`). The observer
    /// only flips `reorg_halted` once this streak reaches
    /// `hardening::REORG_CONFIRM_TICKS`; a single non-suspect tick resets it to 0
    /// (`hardening::on_reorg_tick`). This debounces a transient single-provider
    /// RPC lag (fetch_block_numbers is un-quorumed) so one stale read cannot
    /// permanently halt the chain. NOTE: Task 14's `clear_reorg_halt` MUST reset
    /// BOTH `reorg_halted` AND this streak for the cleared chain. Same V2
    /// rationale as `reorg_halted`; `#[serde(default)]` is mandatory state-wipe
    /// defense.
    #[serde(default)]
    pub reorg_suspect_streak: BTreeMap<ChainId, u32>,
    /// Persisted idempotency set for the burn-watch observer (C-1
    /// supply-divergence fix). Maps `block_number -> { burn-identity key }`,
    /// where the key is `"{tx_hash}:{vault_id}:{amount_e8s}"`. A burn whose key
    /// is already present at its block has ALREADY been applied to
    /// `chain_supplies`/`debt_e8s` and MUST be skipped on any re-scan — this is
    /// what kills the silent double-apply (the pre-fix loop re-applied the
    /// already-applied prefix of a range whenever a later poison burn stalled
    /// the cursor). The map is BOUNDED: after the cursor advances to `N`, the
    /// observer prunes every entry with `block <= N` (those blocks can never be
    /// re-scanned, since the next scan starts at `N+1`). Both InvalidBurn-skips
    /// and successful applies are recorded so a permanently-poison burn is never
    /// reprocessed either. Added directly to V2 (same brand-new-this-phase
    /// rationale as `reorg_halted`); `#[serde(default)]` is mandatory
    /// state-wipe defense so a pre-existing V1/V2 CBOR snapshot lacking this key
    /// decodes cleanly to an empty map.
    #[serde(default)]
    pub processed_burn_keys: BTreeMap<u64, BTreeSet<String>>,
}

impl MultiChainStateV2 {
    pub fn total_supply_all_chains_e8s(&self) -> u128 {
        self.chain_supplies.values().copied().sum()
    }

    /// Sum of confirmed debt across all foreign-chain vaults (e8s). Under the
    /// Phase 1b foreign-chain-only supply invariant, this MUST equal
    /// `total_supply_all_chains_e8s()` at all times; the Timer-B self-check
    /// compares the two to catch drift. ICP-native debt
    /// (`State::total_borrowed_icusd_amount`) is a SEPARATE pool, deliberately
    /// excluded (unification to a single global total is a Phase 2 task).
    pub fn total_chain_vault_debt_e8s(&self) -> u128 {
        self.chain_vaults.values().map(|v| v.debt_e8s).sum()
    }
}

pub type MultiChainState = MultiChainStateV2;
