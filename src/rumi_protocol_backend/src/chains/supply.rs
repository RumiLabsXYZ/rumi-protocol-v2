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
use super::multi_chain_state::{MultiChainStateV1, MultiChainStateV2, MultiChainStateV4};
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
    Divergence { sum_after: u128, total_debt: u128 },
    HaltedAfterSelfCheckFailure,
}

/// Single-entry mutation path for `chain_supplies`. Caller passes the
/// authoritative `total_debt_e8s` snapshot taken at the same logical
/// moment; we reject any apply that would leave sum != total_debt.
pub fn apply_supply_delta(
    state: &mut MultiChainStateV4,
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
    if sum_after != total_debt_e8s {
        return Err(SupplyInvariantError::Divergence { sum_after, total_debt: total_debt_e8s });
    }

    state.chain_supplies.insert(chain, new);
    Ok(())
}

/// Phase 1a periodic self-check (called from Timer B in Task 11).
/// Returns `Ok(())` when sum == total_debt and `Err(...)` otherwise.
/// On `Err`, the caller flips `state.invariant_halted = true` and emits
/// an event.
pub fn check_invariant(
    state: &MultiChainStateV4,
    total_debt_e8s: u128,
) -> Result<(), SupplyInvariantError> {
    let sum: u128 = state.chain_supplies.values().copied().sum();
    if sum != total_debt_e8s {
        return Err(SupplyInvariantError::Divergence { sum_after: sum, total_debt: total_debt_e8s });
    }
    Ok(())
}
