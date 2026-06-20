//! Supply-invariant enforcement.
//!
//! Every mutation to `multi_chain.chain_supplies` flows through
//! `apply_supply_delta`. The function maintains the invariant
//! `sum(chain_supplies) == total_debt` at call time, refuses underflows
//! and unknown chain ids, and short-circuits whenever a prior Timer B
//! self-check left `multi_chain.invariant_halted = true`.
//!
//! Phase 1a never invokes `apply_supply_delta` from a state-mutating
//! endpoint (no flow mints icUSD on a foreign chain yet). The function
//! exists so Phase 1b's first cross-chain mint, burn, and bridge ops
//! can call it without inventing the invariant under deadline pressure.

use super::config::ChainId;
use super::multi_chain_state::{MultiChainStateV1, MultiChainStateV2, MultiChainState};
use candid::{CandidType, Deserialize};
use serde::Serialize;

/// DORMANT TEMPLATE — not called on the live upgrade path.
///
/// The V1->V2 upgrade happens automatically via the ciborium in-place decode:
/// the four V1 fields map across by name; the new-in-V2 fields carry
/// `#[serde(default)]` and come up empty. No explicit migration call is
/// needed in `post_upgrade`.
///
/// This function is kept as the unit-tested template for the NEXT version bump
/// (V2->V3). When V3 lands, rename this to `migrate_v2_to_v3`, add it to the
/// `post_upgrade` hook, and write a parallel ciborium round-trip test (see
/// `tests_multi_chain_state_v2::v1_cbor_snapshot_decodes_into_v2_without_wiping_state`
/// as the model).
pub fn migrate_multi_chain_state(v1: MultiChainStateV1) -> MultiChainStateV2 {
    MultiChainStateV2 {
        chain_configs: v1.chain_configs,
        chain_supplies: v1.chain_supplies,
        settlement_queues: v1.settlement_queues,
        invariant_halted: v1.invariant_halted,
        chain_vaults: Default::default(),
        chain_contracts: Default::default(),
        manual_prices: Default::default(),
        last_observed_block: Default::default(),
        hot_wallet_balance_e18: Default::default(),
        reorg_halted: Default::default(),
        reorg_suspect_streak: Default::default(),
        processed_burn_keys: Default::default(),
    }
}

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug)]
pub enum SupplyDelta {
    Increase(u128),
    Decrease(u128),
}

#[derive(Debug, PartialEq, Eq)]
pub enum SupplyInvariantError {
    UnknownChain(ChainId),
    Underflow { chain: ChainId, current: u128, attempted_decrease: u128 },
    /// `sum(chain_supplies)` did not equal the unified-invariant RHS. `total_debt`
    /// carries that full RHS (debt + reserve_backing + pending_chain_burn — spec
    /// 5.2), NOT the bare debt; the field name is kept for wire stability. With
    /// all-zero reserve/pending (Increment 1) the RHS equals bare debt, so the
    /// reported pair is byte-identical to the pre-Increment-1 behavior.
    Divergence { sum_after: u128, total_debt: u128 },
    HaltedAfterSelfCheckFailure,
}

/// The unified supply-invariant right-hand side (spec 5.2): every circulating
/// foreign icUSD is backed by EITHER an open vault's collateral (the
/// `total_debt_e8s` term the caller passes), OR protocol-held USDC reserve
/// (`total_reserve_backing_e8s`), OR an IC-side SP burn awaiting its eSpace burn
/// (`total_pending_chain_burn_e8s`).
///
/// The caller passes the debt total it already computes (it owns the debt
/// mutation); the reserve + pending-burn terms are read from `state` HERE, so a
/// caller can never forget a term and FALSE-HALT the chain (finding #24). This is
/// the SINGLE source of truth for the RHS — `apply_supply_delta` and
/// `check_invariant` (hence the Timer-B self-check AND `clear_invariant_halt`) all
/// route through it, so the consumers can never disagree (findings #2, #7).
///
/// Deliberately NOT terms: `reserve_usdc_native` tracks the physical USDC asset,
/// not icUSD-denominated backing (spec 3.2, 5.6); `pending_interest_mint_e8s`
/// mints new supply only on confirm and is excluded from
/// `total_chain_vault_debt_e8s` (finding #1). With all-zero reserve/pending
/// (Increment 1) this reduces to the old `supply == debt`, so it is
/// behavior-preserving.
pub fn chain_backing_rhs_e8s(state: &MultiChainState, total_debt_e8s: u128) -> u128 {
    total_debt_e8s
        .saturating_add(state.total_reserve_backing_e8s())
        .saturating_add(state.total_pending_chain_burn_e8s())
}

/// Single-entry mutation path for `chain_supplies`. The caller passes the
/// authoritative `total_debt_e8s` snapshot taken at the same logical moment; we
/// reject any apply that would leave `sum(chain_supplies)` != the unified RHS
/// (`chain_backing_rhs_e8s` = debt + reserve + pending-burn). No mutation on
/// rejection.
pub fn apply_supply_delta(
    state: &mut MultiChainState,
    chain: ChainId,
    delta: SupplyDelta,
    total_debt_e8s: u128,
) -> Result<(), SupplyInvariantError> {
    if state.invariant_halted {
        return Err(SupplyInvariantError::HaltedAfterSelfCheckFailure);
    }
    let current = match state.chain_supplies.get(&chain) {
        Some(v) => *v,
        None => return Err(SupplyInvariantError::UnknownChain(chain)),
    };
    let new = match delta {
        SupplyDelta::Increase(n) => current.saturating_add(n),
        SupplyDelta::Decrease(n) => {
            if n > current {
                return Err(SupplyInvariantError::Underflow {
                    chain,
                    current,
                    attempted_decrease: n,
                });
            }
            current - n
        }
    };

    // Compute the post-delta sum WITHOUT mutating state yet, so a divergence
    // rejection leaves the state untouched.
    let sum_after: u128 = state
        .chain_supplies
        .iter()
        .map(|(&id, &v)| if id == chain { new } else { v })
        .sum();
    // Compare against the unified RHS (debt + reserve + pending-burn), read from
    // the SAME `state` so the check can never use a stale reserve/pending snapshot
    // (finding #2). With all-zero reserve/pending this is `sum_after != total_debt`.
    let rhs = chain_backing_rhs_e8s(state, total_debt_e8s);
    if sum_after != rhs {
        return Err(SupplyInvariantError::Divergence { sum_after, total_debt: rhs });
    }

    state.chain_supplies.insert(chain, new);
    Ok(())
}

/// Periodic self-check (called from the Timer-B self-check AND from
/// `clear_invariant_halt`). Returns `Ok(())` when `sum(chain_supplies)` equals the
/// unified RHS (`chain_backing_rhs_e8s` = debt + reserve + pending-burn) and
/// `Err(...)` otherwise. On `Err`, the Timer-B caller flips
/// `state.invariant_halted = true` and flips to ReadOnly.
///
/// Both callers pass only `total_chain_vault_debt_e8s()`; the reserve + pending
/// terms are added here, so the Timer-B self-check and `clear_invariant_halt` both
/// pick up the generalized RHS WITHOUT any caller change — a bot liquidation that
/// shifts debt->reserve no longer FALSE-HALTs the chain, and the un-halt path can
/// succeed against the unified RHS (findings #2, #7).
pub fn check_invariant(
    state: &MultiChainState,
    total_debt_e8s: u128,
) -> Result<(), SupplyInvariantError> {
    let sum: u128 = state.chain_supplies.values().copied().sum();
    let rhs = chain_backing_rhs_e8s(state, total_debt_e8s);
    if sum != rhs {
        return Err(SupplyInvariantError::Divergence { sum_after: sum, total_debt: rhs });
    }
    Ok(())
}

/// Phase 1b Task 12 migration: stamp `last_interest_accrual_ns = now_ns` for any
/// chain vault that decoded with 0 (an existing vault from a snapshot written
/// before the interest fields existed), so the first harvest does not bill
/// interest from the unix epoch. New vaults are stamped to `now` at mint-confirm
/// (`confirm_mint_in_state`) and never decode as 0, so this only ever touches
/// pre-feature vaults. Idempotent (re-running is a no-op once stamped). Called
/// from `post_upgrade` after `restore_state`.
pub fn stamp_chain_interest_accrual_start(state: &mut MultiChainState, now_ns: u64) {
    for v in state.chain_vaults.values_mut() {
        if v.last_interest_accrual_ns == 0 {
            v.last_interest_accrual_ns = now_ns;
        }
    }
}
