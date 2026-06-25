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

use super::collateral_config::ChainDebtConfigV1;
use super::config::{ChainConfigV1, ChainConfigV2, ChainConfigV3, ChainId};
use super::liquidation_config::ChainLiquidationConfigV1;
use super::monad::chain_vault::ChainVaultV1;
use super::settlement_queue::SettlementQueueV1;
use candid::{CandidType, Deserialize, Principal};
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
    /// where the key is `"{tx_hash}:{log_index}"`, the canonical on-chain
    /// identity of an EVM log, so two identical `Burn`s emitted in one tx (same
    /// vault and amount, different log indices) are credited separately rather
    /// than collapsed into one entry. A burn whose key
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

/// Phase 1c snapshot. Identical to `MultiChainStateV2` in every field EXCEPT
/// `chain_configs`, whose value type bumps from `ChainConfigV1` to
/// `ChainConfigV2` (the latter adds the `burn_watch_poll_enabled` poll-scan
/// flag). This is a NON-BREAKING reshape under ciborium:
///
///  - The eight outer fields are byte-for-byte identical to V2 and map across
///    by name (NO `#[serde(default)]` needed — V2 always wrote them).
///  - `chain_configs` is a CBOR map `{ChainId -> {field-map}}`. On decode each
///    inner field-map decodes as a `ChainConfigV2`; the new
///    `burn_watch_poll_enabled` key is absent in any V2-written sub-map and is
///    supplied by its field-level `#[serde(default)]` (=> `false`). So a live
///    `MultiChainStateV2` CBOR snapshot decodes into `MultiChainStateV3`
///    without error and without wiping any chain/vault/supply state.
///
/// Because the decode is in-place (the four-then-eight fields carry across by
/// name, the nested config field gains a defaulted bool), NO explicit migration
/// call is needed in `post_upgrade`; `migrate_multi_chain_state` in `supply.rs`
/// remains the dormant template for the next BREAKING bump.
///
/// Add the NEXT field by bumping to `MultiChainStateV4` (keep V3 verbatim),
/// `#[serde(default)]` on the new field, and rebinding the alias below.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, Default)]
pub struct MultiChainStateV3 {
    /// Bumped value type vs V2 (`ChainConfigV1` -> `ChainConfigV2`). Always
    /// present in any valid snapshot; the nested `ChainConfigV2` add-a-field is
    /// what carries the `#[serde(default)]` (see `ChainConfigV2`).
    pub chain_configs: BTreeMap<ChainId, ChainConfigV2>,
    pub chain_supplies: BTreeMap<ChainId, u128>,
    pub settlement_queues: BTreeMap<ChainId, SettlementQueueV1>,
    pub invariant_halted: bool,
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
    #[serde(default)]
    pub reorg_halted: BTreeMap<ChainId, bool>,
    #[serde(default)]
    pub reorg_suspect_streak: BTreeMap<ChainId, u32>,
    #[serde(default)]
    pub processed_burn_keys: BTreeMap<u64, BTreeSet<String>>,
}

impl MultiChainStateV3 {
    pub fn total_supply_all_chains_e8s(&self) -> u128 {
        self.chain_supplies.values().copied().sum()
    }

    /// Sum of confirmed debt across all foreign-chain vaults (e8s). See the
    /// V2 doc — same foreign-chain-only invariant.
    pub fn total_chain_vault_debt_e8s(&self) -> u128 {
        self.chain_vaults.values().map(|v| v.debt_e8s).sum()
    }
}

/// Phase 1d snapshot (audit M-05, QUORUM-2). Identical to `MultiChainStateV3`
/// in every field EXCEPT `chain_configs`, whose value type bumps from
/// `ChainConfigV2` to `ChainConfigV3` (the latter adds the
/// `min_quorum_providers` per-chain quorum floor). This is a NON-BREAKING
/// reshape under ciborium, exactly like the V2->V3 `ChainConfigV1->V2` bump:
///
///  - The ten outer fields are byte-for-byte identical to V3 and map across by
///    name (the four originals are always present; the V2-added maps keep their
///    `#[serde(default)]`).
///  - `chain_configs` is a CBOR map `{ChainId -> {field-map}}`. On decode each
///    inner field-map decodes as a `ChainConfigV3`; the new
///    `min_quorum_providers` key is absent in any V3-written sub-map and is
///    supplied by its field-level `#[serde(default)]` (=> `None`). So a live
///    `MultiChainStateV3` CBOR snapshot decodes into `MultiChainStateV4`
///    without error and without wiping any chain/vault/supply state.
///
/// Because the decode is in-place, NO explicit migration call is needed in
/// `post_upgrade`; `migrate_multi_chain_state` in `supply.rs` remains the
/// dormant template for the next BREAKING bump.
///
/// Add the NEXT field by bumping to `MultiChainStateV5` (keep V4 verbatim),
/// `#[serde(default)]` on the new field, and rebinding the alias below.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, Default)]
pub struct MultiChainStateV4 {
    /// Bumped value type vs V3 (`ChainConfigV2` -> `ChainConfigV3`). Always
    /// present in any valid snapshot; the nested `ChainConfigV3` add-a-field is
    /// what carries the `#[serde(default)]` (see `ChainConfigV3`).
    pub chain_configs: BTreeMap<ChainId, ChainConfigV3>,
    pub chain_supplies: BTreeMap<ChainId, u128>,
    pub settlement_queues: BTreeMap<ChainId, SettlementQueueV1>,
    pub invariant_halted: bool,
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
    #[serde(default)]
    pub reorg_halted: BTreeMap<ChainId, bool>,
    #[serde(default)]
    pub reorg_suspect_streak: BTreeMap<ChainId, u32>,
    #[serde(default)]
    pub processed_burn_keys: BTreeMap<u64, BTreeSet<String>>,
    /// M2: per-synthetic-owner monotonic nonce for EIP-712 intent replay
    /// protection. Keyed by `eip712::synthetic_owner(chain, evm_addr)` (which
    /// embeds the chain → nonces are per-(owner,chain)). `#[serde(default)]`:
    /// additive, ciborium-safe (no version-struct bump — coordinated with the
    /// concurrent interest-accrual branch so both changes stay purely additive).
    #[serde(default)]
    pub evm_owner_nonces: BTreeMap<Principal, u64>,
}

impl MultiChainStateV4 {
    pub fn total_supply_all_chains_e8s(&self) -> u128 {
        self.chain_supplies.values().copied().sum()
    }

    /// Sum of confirmed debt across all foreign-chain vaults (e8s). See the
    /// V2 doc — same foreign-chain-only invariant.
    pub fn total_chain_vault_debt_e8s(&self) -> u128 {
        self.chain_vaults.values().map(|v| v.debt_e8s).sum()
    }

    /// M2: the expected next EIP-712 nonce for a synthetic owner (0 if unseen).
    pub fn expected_evm_nonce(&self, owner: &Principal) -> u64 {
        self.evm_owner_nonces.get(owner).copied().unwrap_or(0)
    }

    /// M2: consume `nonce` for `owner`. Succeeds and bumps the counter iff
    /// `nonce == expected`; else returns the expected value as `Err` (no mutation).
    pub fn consume_evm_nonce(&mut self, owner: &Principal, nonce: u64) -> Result<(), u64> {
        let expected = self.expected_evm_nonce(owner);
        if nonce != expected {
            return Err(expected);
        }
        self.evm_owner_nonces
            .insert(*owner, expected.saturating_add(1));
        Ok(())
    }

    /// M2 anti-spam: count NON-terminal vaults (everything but `Closed`) owned by
    /// `owner` — the per-owner open cap.
    pub fn count_owner_active_vaults(&self, owner: &Principal) -> usize {
        use crate::chains::vault::ChainVaultStatus;
        self.chain_vaults
            .values()
            .filter(|v| &v.owner == owner && v.status != ChainVaultStatus::Closed)
            .count()
    }
}

/// Phase 1e snapshot (audit F-01 price-freshness mitigation). Identical to
/// `MultiChainStateV4` in every field, plus ONE new map: `manual_price_set_at_ns`,
/// the wall-clock nanosecond timestamp of the LAST `set_manual_collateral_price`
/// write for each `(chain, symbol)`. This is a NON-BREAKING reshape under ciborium:
///
///  - The eleven V4 fields are byte-for-byte identical and map across by name
///    (the four originals are always present; the V2-added maps keep their
///    `#[serde(default)]`).
///  - `manual_price_set_at_ns` is new and carries `#[serde(default)]`, so a live
///    `MultiChainStateV4` CBOR snapshot (which lacks the key entirely) decodes
///    into V5 with the map defaulting to empty. The existing `manual_prices` map
///    is UNTOUCHED, so every collateral-ratio read of it is byte-identical — the
///    timestamp is a pure side-channel for the off-chain monitor's getter.
///
/// A price set before this upgrade has an entry in `manual_prices` but NOT in
/// `manual_price_set_at_ns`; the getter reports `set_at_ns = 0` for it until the
/// next refresh writes both. Because the decode is in-place, NO explicit
/// `post_upgrade` migration call is needed.
///
/// Add the NEXT field by bumping to `MultiChainStateV6` (keep V5 verbatim),
/// `#[serde(default)]` on the new field, and rebinding the alias below.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, Default)]
pub struct MultiChainStateV5 {
    pub chain_configs: BTreeMap<ChainId, ChainConfigV3>,
    pub chain_supplies: BTreeMap<ChainId, u128>,
    pub settlement_queues: BTreeMap<ChainId, SettlementQueueV1>,
    pub invariant_halted: bool,
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
    #[serde(default)]
    pub reorg_halted: BTreeMap<ChainId, bool>,
    #[serde(default)]
    pub reorg_suspect_streak: BTreeMap<ChainId, u32>,
    #[serde(default)]
    pub processed_burn_keys: BTreeMap<u64, BTreeSet<String>>,
    /// M2: per-synthetic-owner monotonic nonce for EIP-712 intent replay
    /// protection (carried verbatim from V4). Keyed by
    /// `eip712::synthetic_owner(chain, evm_addr)`. `#[serde(default)]` additive.
    #[serde(default)]
    pub evm_owner_nonces: BTreeMap<Principal, u64>,
    /// New in V5 (audit F-01): wall-clock ns of the last manual-price write per
    /// `(chain, symbol)`. The off-chain monitor owns freshness; this lets the
    /// getter expose how stale the canister's own manual price is. `#[serde(default)]`
    /// is mandatory state-wipe defense (a V4 snapshot lacks this key).
    #[serde(default)]
    pub manual_price_set_at_ns: BTreeMap<(ChainId, String), u64>,
}

impl MultiChainStateV5 {
    pub fn total_supply_all_chains_e8s(&self) -> u128 {
        self.chain_supplies.values().copied().sum()
    }

    /// Sum of confirmed debt across all foreign-chain vaults (e8s). See the
    /// V2 doc — same foreign-chain-only invariant.
    pub fn total_chain_vault_debt_e8s(&self) -> u128 {
        self.chain_vaults.values().map(|v| v.debt_e8s).sum()
    }

    /// M2: the expected next EIP-712 nonce for a synthetic owner (0 if unseen).
    pub fn expected_evm_nonce(&self, owner: &Principal) -> u64 {
        self.evm_owner_nonces.get(owner).copied().unwrap_or(0)
    }

    /// M2: consume `nonce` for `owner`. Succeeds and bumps the counter iff
    /// `nonce == expected`; else returns the expected value as `Err` (no mutation).
    pub fn consume_evm_nonce(&mut self, owner: &Principal, nonce: u64) -> Result<(), u64> {
        let expected = self.expected_evm_nonce(owner);
        if nonce != expected {
            return Err(expected);
        }
        self.evm_owner_nonces
            .insert(*owner, expected.saturating_add(1));
        Ok(())
    }

    /// M2 anti-spam: count NON-terminal vaults (everything but `Closed`) owned by
    /// `owner` — the per-owner open cap.
    pub fn count_owner_active_vaults(&self, owner: &Principal) -> usize {
        use crate::chains::vault::ChainVaultStatus;
        self.chain_vaults
            .values()
            .filter(|v| &v.owner == owner && v.status != ChainVaultStatus::Closed)
            .count()
    }

    /// Write a manual collateral price for `(chain, symbol)` and stamp the
    /// wall-clock time of the write. Both maps are updated together so the
    /// getter can never see a price without its freshness timestamp.
    pub fn set_manual_price(&mut self, chain: ChainId, symbol: String, price_e8: u64, now_ns: u64) {
        self.manual_prices.insert((chain, symbol.clone()), price_e8);
        self.manual_price_set_at_ns.insert((chain, symbol), now_ns);
    }

    /// Read the manual collateral price for `(chain, symbol)` as
    /// `(price_e8, set_at_ns)`. Returns `None` if no price is set. `set_at_ns` is
    /// `0` for a price set before the V5 upgrade (timestamp not yet recorded).
    pub fn get_manual_price(&self, chain: ChainId, symbol: &str) -> Option<(u64, u64)> {
        let key = (chain, symbol.to_string());
        let price = *self.manual_prices.get(&key)?;
        let set_at = self.manual_price_set_at_ns.get(&key).copied().unwrap_or(0);
        Some((price, set_at))
    }
}

/// Chains-liquidation working snapshot. Carries every `MultiChainStateV5` field
/// verbatim, plus additive liquidation-engine accounting/routing fields:
/// per-chain reserve backing (bot/PSM path), the physical USDC custody mirror,
/// per-chain pending SP burn, the one-shot SP-attempted set, liquidation config,
/// bot-pending timestamps, bad-debt counters, and SP claim records. This is a
/// NON-BREAKING reshape under ciborium, exactly like every prior bump:
///
///  - The fifteen V5 fields are byte-for-byte identical and map across by name
///    (the four originals are always present; every V2+-added map keeps its
///    `#[serde(default)]`).
///  - The V6-added fields each carry `#[serde(default)]`, so a live
///    `MultiChainStateV5` CBOR snapshot (which lacks these keys entirely) decodes
///    into V6 with them defaulting to empty — NOT a state wipe. Proven by
///    `tests_multi_chain_state_v2::v5_cbor_snapshot_decodes_into_v6_without_wiping_state`.
///
/// All additive fields default to 0/empty on snapshots that predate them, so the
/// unified supply invariant (debt + reserve + pending-burn) reduces to the old
/// `supply == debt` until the corresponding increment writes them — the upgrade
/// is behavior-preserving.
///
/// Because the decode is in-place, NO explicit `post_upgrade` migration call is
/// needed; `migrate_multi_chain_state` in `supply.rs` remains the dormant
/// template for the next BREAKING bump.
///
/// Any further additive field on this CBOR root MUST use `#[serde(default)]` and
/// extend the V5->V6 and V6->V6 decode tests. A genuinely breaking reshape
/// should bump to `MultiChainStateV7` (keep V6 verbatim) and rebind the alias
/// below.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ChainLiqClaimV1 {
    pub vault_id: u64,
    pub chain: ChainId,
    pub custody_address: String,
    pub seized_native_total: u128,
    pub paid_native: u128,
}

impl ChainLiqClaimV1 {
    pub fn paid_within_seized(&self) -> bool {
        self.paid_native <= self.seized_native_total
    }

    pub fn remaining_native(&self) -> u128 {
        self.seized_native_total.saturating_sub(self.paid_native)
    }
}

/// Compact durable marker for an operator-settled foreign-chain burn proof.
/// Full receipts/logs are intentionally not stored in stable state.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct SettlementProofRecord {
    pub proof_id: String,
    pub chain_id: ChainId,
    pub tx_hash: String,
    pub log_index: u64,
    pub amount_e8s: u128,
    pub block_number: u64,
    pub recorded_at_ns: u64,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct PendingChainBurnAging {
    pub chain_id: ChainId,
    pub pending_chain_burn_e8s: u128,
    pub oldest_reference_ns: Option<u64>,
    pub age_ns: Option<u64>,
    pub proof_count: u64,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, Default)]
pub struct MultiChainStateV6 {
    pub chain_configs: BTreeMap<ChainId, ChainConfigV3>,
    pub chain_supplies: BTreeMap<ChainId, u128>,
    pub settlement_queues: BTreeMap<ChainId, SettlementQueueV1>,
    pub invariant_halted: bool,
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
    #[serde(default)]
    pub reorg_halted: BTreeMap<ChainId, bool>,
    #[serde(default)]
    pub reorg_suspect_streak: BTreeMap<ChainId, u32>,
    #[serde(default)]
    pub processed_burn_keys: BTreeMap<u64, BTreeSet<String>>,
    /// M2: per-synthetic-owner monotonic nonce for EIP-712 intent replay
    /// protection (carried verbatim from V5).
    #[serde(default)]
    pub evm_owner_nonces: BTreeMap<Principal, u64>,
    /// Audit F-01: wall-clock ns of the last manual-price write per
    /// `(chain, symbol)` (carried verbatim from V5).
    #[serde(default)]
    pub manual_price_set_at_ns: BTreeMap<(ChainId, String), u64>,
    // --- New in V6: chains-liquidation accounting scaffolding (Increment 1) ---
    /// Bot-path (PSM) reserve backing per chain (e8s): icUSD value whose backing
    /// shifted CFX->USDC, no longer an open vault's debt but not yet burned.
    /// RHS term-2 of the unified supply invariant. Grows on bot-liquidation
    /// confirm; shrinks ONLY when a human bridges + the foreign icUSD is burned.
    /// Empty until Increment 2 wires the bot liquidation path. `#[serde(default)]`
    /// is mandatory state-wipe defense (a V5 snapshot lacks this key).
    #[serde(default)]
    pub reserve_backing_e8s: BTreeMap<ChainId, u128>,
    /// Physical settle-stable (USDC) the reserve address holds, per chain, in
    /// native base units (18-dec on eSpace), pending the manual bridge to cUSDT
    /// reserves. Bookkeeping of the ASSET, distinct from the icUSD-denominated
    /// backing; the gap reveals realized slippage / penalty surplus. NOT on the
    /// invariant RHS. Empty until Increment 2.
    #[serde(default)]
    pub reserve_usdc_native: BTreeMap<ChainId, u128>,
    /// SP-path IC-side burn backing per chain (e8s): the SP has absorbed debt
    /// by burning IC-native icUSD, while the foreign-chain icUSD representation
    /// remains outstanding. RHS term-3 of the unified supply invariant. Later
    /// manual reconciliation can consume this term together with a
    /// `chain_supplies` decrement after the protocol acquires and retires the
    /// foreign representation. Empty until Increment 4.
    #[serde(default)]
    pub pending_chain_burn_e8s: BTreeMap<ChainId, u128>,
    /// Chains analog of the ICP `sp_attempted_vaults`: vaults whose bot
    /// liquidation failed and were escalated to the SP exactly once (the
    /// no-retry guard; once present, never re-attempted by the SP). Empty until
    /// Increment 4. Rides V6 because it is in the persisted root, but it is
    /// transient routing state (reset on resolution); surviving an upgrade is
    /// harmless (worst case a vault waits one extra tick for manual).
    #[serde(default)]
    pub sp_attempted_chain_vaults: BTreeSet<u64>,
    /// Vault_id -> SP liquidation claim record. Created when the Stability Pool
    /// absorbs a foreign-chain vault; tracks how much seized native collateral is
    /// reserved in that vault's custody address and how much has already been
    /// paid out to CFX-claiming SP depositors. `#[serde(default)]` is mandatory:
    /// live V6 snapshots written before Inc 4 lack this key.
    #[serde(default)]
    pub chain_liquidation_claims: BTreeMap<u64, ChainLiqClaimV1>,
    /// Per-chain, operator-settable liquidation config (spec 8): the DEX wiring +
    /// risk knobs the bot path reads, keyed by chain. Added directly to V6 (not a
    /// V7) because V6 has not yet been persisted to any live canister — same
    /// pre-deploy rationale as the reorg fields were added directly to V2.
    /// `#[serde(default)]` is mandatory state-wipe defense regardless. Inert until
    /// Increment 2+ reads it; Increment 1 ships only the getter/setter.
    #[serde(default)]
    pub chain_liquidation_configs: BTreeMap<ChainId, ChainLiquidationConfigV1>,
    /// Optional Tier-B per-chain debt overrides. Missing row means the
    /// compile-time `chain_collateral_config` values remain authoritative.
    #[serde(default)]
    pub chain_debt_configs: BTreeMap<ChainId, ChainDebtConfigV1>,
    /// Vault_id -> first-bot-routed wall-clock ns. Set when
    /// `begin_liquidation_in_state` routes a vault to the bot tier; the durable
    /// timestamp the bot->SP escalation predicate reads (spec §10, finding #10).
    /// Pruned when a vault recovers/resolves. The SP consumer lands in Increment
    /// 4; the timestamp history must start now. Added directly to V6 (pre-deploy,
    /// same rationale as the maps above); `#[serde(default)]` is mandatory.
    #[serde(default)]
    pub bot_pending_chain_vaults: BTreeMap<u64, u64>,
    /// Per-chain realized bad debt (e8s): when a bot swap's realized USDC valued
    /// in USD is LESS than the debt it was sized to clear, the shortfall
    /// (debt_to_clear - actual_cleared) is recorded here — reserve_backing is
    /// NEVER credited more than the realized value (findings #16/#1). Strict
    /// min-out makes this unreachable on the happy path; the counter exists so a
    /// structural under-cover is visible, not silent. Added directly to V6
    /// (pre-deploy); `#[serde(default)]` mandatory.
    #[serde(default)]
    pub chain_bad_debt_e8s: BTreeMap<ChainId, u128>,
    /// Proof ids already consumed to settle SP pending-chain-burn backing. Kept
    /// separate from user burn ids and reserve-burn ids so domains cannot collide.
    #[serde(default)]
    pub settled_pending_burn_proofs: BTreeMap<String, SettlementProofRecord>,
    /// Proof ids already consumed to settle Tier-1 reserve-backed burns. Kept
    /// separate from pending-chain-burn proofs because a reserve proof id encodes
    /// both burn and stable-transfer receipt identities.
    #[serde(default)]
    pub settled_reserve_burn_proofs: BTreeMap<String, SettlementProofRecord>,
    /// Burn log identities already consumed by either pending-chain-burn or
    /// reserve-burn settlement. This closes cross-domain replay where the same
    /// foreign icUSD burn could otherwise debit both backing buckets.
    #[serde(default)]
    pub settled_settlement_burn_logs: BTreeSet<String>,
    /// Reserve transfer log identity -> e8s capacity already consumed by
    /// reserve-burn settlements. One reserve transfer can fund several burns,
    /// but cumulative consumption must never exceed the verified transfer size.
    #[serde(default)]
    pub settled_reserve_transfer_e8s: BTreeMap<String, u128>,
}

impl MultiChainStateV6 {
    pub fn total_supply_all_chains_e8s(&self) -> u128 {
        self.chain_supplies.values().copied().sum()
    }

    /// Sum of confirmed debt across all foreign-chain vaults (e8s). RHS term-1 of
    /// the unified supply invariant (spec 5.2). NOTE: counts only REALIZED
    /// `debt_e8s`, so `pending_interest_mint_e8s` (accrued-but-unconfirmed
    /// interest) is deliberately excluded — it mints new supply only on confirm
    /// and is not yet in `chain_supplies` (finding #1).
    pub fn total_chain_vault_debt_e8s(&self) -> u128 {
        self.chain_vaults.values().map(|v| v.debt_e8s).sum()
    }

    /// RHS term-2 of the unified supply invariant (spec 3.3, 5.2): total icUSD
    /// whose backing shifted CFX->USDC reserve (bot/PSM path), summed across
    /// chains. 0 until Increment 2 wires the bot liquidation path.
    pub fn total_reserve_backing_e8s(&self) -> u128 {
        self.reserve_backing_e8s.values().copied().sum()
    }

    /// RHS term-3 of the unified supply invariant (spec 3.3, 5.2): total icUSD the
    /// SP burned IC-side but whose matching eSpace burn is not yet confirmed,
    /// summed across chains. 0 until Increment 4 wires the SP path.
    pub fn total_pending_chain_burn_e8s(&self) -> u128 {
        self.pending_chain_burn_e8s.values().copied().sum()
    }

    pub fn pending_chain_burn_aging(&self, now_ns: u64) -> Vec<PendingChainBurnAging> {
        self.pending_chain_burn_e8s
            .iter()
            .filter_map(|(&chain_id, &pending_chain_burn_e8s)| {
                if pending_chain_burn_e8s == 0 {
                    return None;
                }
                let mut proof_count = 0u64;
                let mut oldest_reference_ns: Option<u64> = None;
                for record in self
                    .settled_pending_burn_proofs
                    .values()
                    .filter(|record| record.chain_id == chain_id)
                {
                    proof_count = proof_count.saturating_add(1);
                    oldest_reference_ns = Some(match oldest_reference_ns {
                        Some(oldest) => oldest.min(record.recorded_at_ns),
                        None => record.recorded_at_ns,
                    });
                }
                let age_ns = oldest_reference_ns.map(|oldest| now_ns.saturating_sub(oldest));
                Some(PendingChainBurnAging {
                    chain_id,
                    pending_chain_burn_e8s,
                    oldest_reference_ns,
                    age_ns,
                    proof_count,
                })
            })
            .collect()
    }

    /// M2: the expected next EIP-712 nonce for a synthetic owner (0 if unseen).
    pub fn expected_evm_nonce(&self, owner: &Principal) -> u64 {
        self.evm_owner_nonces.get(owner).copied().unwrap_or(0)
    }

    /// M2: consume `nonce` for `owner`. Succeeds and bumps the counter iff
    /// `nonce == expected`; else returns the expected value as `Err` (no mutation).
    pub fn consume_evm_nonce(&mut self, owner: &Principal, nonce: u64) -> Result<(), u64> {
        let expected = self.expected_evm_nonce(owner);
        if nonce != expected {
            return Err(expected);
        }
        self.evm_owner_nonces
            .insert(*owner, expected.saturating_add(1));
        Ok(())
    }

    /// M2 anti-spam: count NON-terminal vaults (everything but `Closed`) owned by
    /// `owner` — the per-owner open cap.
    pub fn count_owner_active_vaults(&self, owner: &Principal) -> usize {
        use crate::chains::vault::ChainVaultStatus;
        self.chain_vaults
            .values()
            .filter(|v| &v.owner == owner && v.status != ChainVaultStatus::Closed)
            .count()
    }

    /// Write a manual collateral price for `(chain, symbol)` and stamp the
    /// wall-clock time of the write. Both maps are updated together so the
    /// getter can never see a price without its freshness timestamp.
    pub fn set_manual_price(&mut self, chain: ChainId, symbol: String, price_e8: u64, now_ns: u64) {
        self.manual_prices.insert((chain, symbol.clone()), price_e8);
        self.manual_price_set_at_ns.insert((chain, symbol), now_ns);
    }

    /// Read the manual collateral price for `(chain, symbol)` as
    /// `(price_e8, set_at_ns)`. Returns `None` if no price is set. `set_at_ns` is
    /// `0` for a price set before the V5 upgrade (timestamp not yet recorded).
    pub fn get_manual_price(&self, chain: ChainId, symbol: &str) -> Option<(u64, u64)> {
        let key = (chain, symbol.to_string());
        let price = *self.manual_prices.get(&key)?;
        let set_at = self.manual_price_set_at_ns.get(&key).copied().unwrap_or(0);
        Some((price, set_at))
    }
}

pub type MultiChainState = MultiChainStateV6;

#[cfg(test)]
mod manual_price_tests {
    use super::*;

    const CFX: ChainId = ChainId(1030);

    #[test]
    fn set_manual_price_stores_price_and_timestamp() {
        let mut mc = MultiChainState::default();
        mc.set_manual_price(CFX, "CFX".to_string(), 15_000_000, 1234);
        assert_eq!(mc.get_manual_price(CFX, "CFX"), Some((15_000_000, 1234)));
    }

    #[test]
    fn get_manual_price_none_when_unset() {
        let mc = MultiChainState::default();
        assert_eq!(mc.get_manual_price(CFX, "CFX"), None);
    }

    #[test]
    fn set_manual_price_overwrites_price_and_timestamp() {
        let mut mc = MultiChainState::default();
        mc.set_manual_price(CFX, "CFX".to_string(), 15_000_000, 1000);
        mc.set_manual_price(CFX, "CFX".to_string(), 16_000_000, 2000);
        assert_eq!(mc.get_manual_price(CFX, "CFX"), Some((16_000_000, 2000)));
    }

    #[test]
    fn get_manual_price_reports_zero_timestamp_for_pre_v5_price() {
        // A price written before V5 lives in `manual_prices` with NO entry in
        // `manual_price_set_at_ns`. The getter must report set_at_ns = 0, not None.
        let mut mc = MultiChainState::default();
        mc.manual_prices
            .insert((CFX, "CFX".to_string()), 15_000_000);
        assert_eq!(mc.get_manual_price(CFX, "CFX"), Some((15_000_000, 0)));
    }

    #[test]
    fn v4_snapshot_decodes_into_v5_without_wipe() {
        // State-wipe defense: a live V4 CBOR snapshot (no manual_price_set_at_ns
        // key) must decode into V5 with prices intact and the timestamp map empty.
        let mut v4 = MultiChainStateV4::default();
        v4.manual_prices
            .insert((CFX, "CFX".to_string()), 15_000_000);
        v4.chain_supplies.insert(CFX, 42);
        let mut buf = Vec::new();
        ciborium::ser::into_writer(&v4, &mut buf).unwrap();

        let v5: MultiChainStateV5 = ciborium::de::from_reader(buf.as_slice()).unwrap();
        assert_eq!(
            v5.manual_prices.get(&(CFX, "CFX".to_string())),
            Some(&15_000_000)
        );
        assert_eq!(v5.chain_supplies.get(&CFX), Some(&42));
        assert!(v5.manual_price_set_at_ns.is_empty());
        assert_eq!(v5.get_manual_price(CFX, "CFX"), Some((15_000_000, 0)));
    }
}
