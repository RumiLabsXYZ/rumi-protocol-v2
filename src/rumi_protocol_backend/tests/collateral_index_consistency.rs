//! Regression fence: `collateral_to_vault_ids` must shrink when vaults close.
//!
//! Measured on mainnet 2026-06-11: summing `ids.len()` across
//! `get_collateral_totals()` gave ~185 indexed ids while `get_vault_count()`
//! returned 82 open vaults. Root cause: `unindex_vault_by_collateral` existed
//! (state.rs) but had zero callers — every vault-removal path leaked its id:
//!
//!   * `State::close_vault` (runtime withdraw_and_close / repay_and_close +
//!     replay of CloseVault / WithdrawAndCloseVault / VaultWithdrawnAndClosed)
//!   * `State::liquidate_vault` full-liquidation branch (runtime + replay)
//!   * `State::redistribute_vault` (replay)
//!   * vault.rs partial-liquidation drain cleanups (runtime only)
//!   * main.rs post_upgrade empty-vault cleanup
//!
//! Impact was display-only (consumers `filter_map` over the primary map, so
//! stale ids were skipped in sums) but `get_collateral_totals().vault_count`,
//! the dashboard, and `get_protocol_status` reported inflated counts, and
//! every index consumer paid O(stale) wasted lookups.
//!
//! Layers in this file:
//!  1. State-level: each close path un-indexes, both for the collateral index
//!     and the principal index (empty per-owner sets must be pruned so replay
//!     and runtime converge on identical maps).
//!  2. Replay-level: replaying a log that opens and closes vaults yields an
//!     index identical to the one live operation produces, including the
//!     drain-to-zero partial-liquidation removal that previously only
//!     happened at runtime.
//!  3. Healing: `rebuild_collateral_index` converges a corrupted index back
//!     to the primary map (run by post_upgrade so mainnet heals on deploy).

use candid::Principal;

use rumi_protocol_backend::event::{replay, Event};
use rumi_protocol_backend::numeric::{ICUSD, ICP, UsdIcp};
use rumi_protocol_backend::state::{Mode, State};
use rumi_protocol_backend::vault::Vault;
use rumi_protocol_backend::InitArg;
use rust_decimal_macros::dec;

fn icp_ledger() -> Principal {
    Principal::from_slice(&[10])
}

fn init_arg() -> InitArg {
    InitArg {
        xrc_principal: Principal::anonymous(),
        icusd_ledger_principal: Principal::anonymous(),
        icp_ledger_principal: icp_ledger(),
        fee_e8s: 0,
        developer_principal: Principal::anonymous(),
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    }
}

fn fresh_state_with_price(price: f64) -> State {
    let mut state = State::from(init_arg());
    let icp = state.icp_collateral_type();
    if let Some(config) = state.collateral_configs.get_mut(&icp) {
        config.last_price = Some(price);
    }
    state
}

fn owner() -> Principal {
    Principal::from_slice(&[42])
}

fn make_vault(vault_id: u64, collateral_e8s: u64, borrowed_icusd_e8s: u64) -> Vault {
    Vault {
        owner: owner(),
        vault_id,
        collateral_amount: collateral_e8s,
        borrowed_icusd_amount: ICUSD::new(borrowed_icusd_e8s),
        collateral_type: icp_ledger(),
        last_accrual_time: 0,
        accrued_interest: ICUSD::new(0),
        bot_processing: false,
    }
}

/// Ids indexed under a collateral type (empty if the entry was pruned).
fn indexed_ids(state: &State, ct: &Principal) -> Vec<u64> {
    state
        .collateral_to_vault_ids
        .get(ct)
        .map(|ids| ids.iter().copied().collect())
        .unwrap_or_default()
}

/// The exact two-direction consistency contract between the collateral index
/// and the primary vault map.
fn assert_index_matches_vaults(state: &State, context: &str) {
    let indexed_total: usize = state
        .collateral_to_vault_ids
        .values()
        .map(|ids| ids.len())
        .sum();
    assert_eq!(
        indexed_total,
        state.vault_id_to_vaults.len(),
        "{context}: indexed id count must equal open vault count",
    );
    for (ct, ids) in &state.collateral_to_vault_ids {
        for id in ids {
            let vault = state
                .vault_id_to_vaults
                .get(id)
                .unwrap_or_else(|| panic!("{context}: indexed id {id} has no vault"));
            assert_eq!(
                &vault.collateral_type, ct,
                "{context}: vault {id} indexed under wrong collateral type",
            );
        }
    }
}

// ============================================================================
// Layer 1 — state-level close paths
// ============================================================================

#[test]
fn close_vault_removes_id_from_collateral_index() {
    let mut state = fresh_state_with_price(5.0);
    let icp = state.icp_collateral_type();
    state.open_vault(make_vault(1, 0, 0));
    state.open_vault(make_vault(2, 0, 0));
    assert_eq!(indexed_ids(&state, &icp), vec![1, 2]);

    state.close_vault(1);
    assert_eq!(
        indexed_ids(&state, &icp),
        vec![2],
        "closing vault 1 must drop it from collateral_to_vault_ids",
    );

    state.close_vault(2);
    assert!(
        indexed_ids(&state, &icp).is_empty(),
        "closing the last vault must leave no indexed ids",
    );
    assert_index_matches_vaults(&state, "after closing all vaults");
}

#[test]
fn close_vault_prunes_empty_principal_entry() {
    let mut state = fresh_state_with_price(5.0);
    state.open_vault(make_vault(1, 0, 0));
    state.close_vault(1);
    assert!(
        state.principal_to_vault_ids.get(&owner()).is_none(),
        "closing the owner's last vault must prune the empty principal entry \
         (the runtime close path prunes it; replay must match)",
    );
}

#[test]
fn full_liquidation_removes_id_from_collateral_index() {
    // GeneralAvailability mode always takes the full-liquidation branch,
    // which removes the vault from the primary map entirely.
    let mut state = fresh_state_with_price(5.0);
    let icp = state.icp_collateral_type();
    // 1 ICP collateral @ $5 vs 10 icUSD debt → deeply underwater.
    state.open_vault(make_vault(1, 100_000_000, 1_000_000_000));
    state.open_vault(make_vault(2, 10_000_000_000, 1_000_000_000));

    let price = UsdIcp::from(dec!(5.0));
    state.liquidate_vault(1, Mode::GeneralAvailability, price);

    assert!(
        !state.vault_id_to_vaults.contains_key(&1),
        "sanity: full liquidation removes the vault",
    );
    assert_eq!(
        indexed_ids(&state, &icp),
        vec![2],
        "full liquidation must drop the vault from collateral_to_vault_ids",
    );
    assert_index_matches_vaults(&state, "after full liquidation");
}

#[test]
fn full_liquidation_prunes_empty_principal_entry() {
    let mut state = fresh_state_with_price(5.0);
    state.open_vault(make_vault(1, 100_000_000, 1_000_000_000));

    let price = UsdIcp::from(dec!(5.0));
    state.liquidate_vault(1, Mode::GeneralAvailability, price);

    assert!(
        state.principal_to_vault_ids.get(&owner()).is_none(),
        "full liquidation of the owner's last vault must prune the empty principal entry",
    );
}

#[test]
fn redistribute_vault_removes_id_from_collateral_index() {
    let mut state = fresh_state_with_price(5.0);
    let icp = state.icp_collateral_type();
    state.open_vault(make_vault(1, 100_000_000, 1_000_000_000));
    state.open_vault(make_vault(2, 10_000_000_000, 1_000_000_000));

    state.redistribute_vault(1);

    assert!(
        !state.vault_id_to_vaults.contains_key(&1),
        "sanity: redistribution removes the source vault",
    );
    assert_eq!(
        indexed_ids(&state, &icp),
        vec![2],
        "redistribution must drop the source vault from collateral_to_vault_ids",
    );
    assert_index_matches_vaults(&state, "after redistribution");
}

#[test]
fn open_vault_duplicate_id_with_new_collateral_type_reindexes() {
    // Defensive: a duplicate OpenVault event for an existing id must not leave
    // the id indexed under the old collateral type.
    let mut state = fresh_state_with_price(5.0);
    let icp = state.icp_collateral_type();
    let other_ct = Principal::from_slice(&[77]);

    state.open_vault(make_vault(1, 0, 0));
    assert_eq!(indexed_ids(&state, &icp), vec![1]);

    let mut moved = make_vault(1, 0, 0);
    moved.collateral_type = other_ct;
    state.open_vault(moved);

    assert!(
        indexed_ids(&state, &icp).is_empty(),
        "re-opening vault 1 under a new collateral type must unindex the old type",
    );
    assert_eq!(indexed_ids(&state, &other_ct), vec![1]);
    assert_index_matches_vaults(&state, "after duplicate open with new type");
}

// ============================================================================
// Layer 2 — replay-exactness
// ============================================================================

#[test]
fn replay_close_vault_keeps_collateral_index_exact() {
    let events = vec![
        Event::Init(init_arg()),
        Event::OpenVault {
            vault: make_vault(1, 0, 0),
            block_index: 1,
            timestamp: None,
        },
        Event::OpenVault {
            vault: make_vault(2, 0, 0),
            block_index: 2,
            timestamp: None,
        },
        Event::CloseVault {
            vault_id: 1,
            block_index: None,
            timestamp: None,
        },
    ];

    let state = replay(events.into_iter()).expect("replay must succeed");

    assert_eq!(
        indexed_ids(&state, &state.icp_collateral_type()),
        vec![2],
        "replayed CloseVault must unindex exactly like the live path",
    );
    assert_index_matches_vaults(&state, "after replayed close");
}

#[test]
fn replay_partial_liquidation_drain_removes_vault_and_index() {
    // Runtime behavior (vault.rs liquidate_vault_partial / _debt_burned):
    // a partial liquidation that drains the vault to zero debt AND zero
    // collateral removes the vault and prunes the owner's entry. Replay must
    // mirror that rule or replayed state diverges from live state (shell
    // vaults + stale index ids — exactly what post_upgrade's one-time
    // empty-vault cleanup had to patch over on mainnet).
    let events = vec![
        Event::Init(init_arg()),
        Event::OpenVault {
            vault: make_vault(1, 100_000_000, 50_000_000),
            block_index: 1,
            timestamp: None,
        },
        Event::PartialLiquidateVault {
            vault_id: 1,
            liquidator_payment: ICUSD::new(50_000_000),
            icp_to_liquidator: ICP::new(100_000_000),
            liquidator: None,
            icp_rate: Some(UsdIcp::from(dec!(5.0))),
            protocol_fee_collateral: None,
            timestamp: None,
            three_usd_reserves_e8s: None,
        },
    ];

    let state = replay(events.into_iter()).expect("replay must succeed");

    assert!(
        !state.vault_id_to_vaults.contains_key(&1),
        "replay must remove a vault drained to zero debt and zero collateral, \
         mirroring the runtime partial-liquidation cleanup",
    );
    assert!(
        indexed_ids(&state, &state.icp_collateral_type()).is_empty(),
        "drained vault must not linger in collateral_to_vault_ids after replay",
    );
    assert!(
        state.principal_to_vault_ids.get(&owner()).is_none(),
        "drained vault's owner entry must be pruned after replay",
    );
}

#[test]
fn replay_partial_liquidation_keeps_surviving_vault_indexed() {
    // The drain-removal rule must NOT fire when anything remains in the vault.
    let events = vec![
        Event::Init(init_arg()),
        Event::OpenVault {
            vault: make_vault(1, 100_000_000, 50_000_000),
            block_index: 1,
            timestamp: None,
        },
        Event::PartialLiquidateVault {
            vault_id: 1,
            liquidator_payment: ICUSD::new(20_000_000),
            icp_to_liquidator: ICP::new(40_000_000),
            liquidator: None,
            icp_rate: Some(UsdIcp::from(dec!(5.0))),
            protocol_fee_collateral: None,
            timestamp: None,
            three_usd_reserves_e8s: None,
        },
    ];

    let state = replay(events.into_iter()).expect("replay must succeed");

    assert!(
        state.vault_id_to_vaults.contains_key(&1),
        "partially liquidated vault with remaining balances must survive replay",
    );
    assert_eq!(
        indexed_ids(&state, &state.icp_collateral_type()),
        vec![1],
        "surviving vault must stay indexed",
    );
}

// ============================================================================
// Layer 3 — healing sweep (run by post_upgrade) and invariants
// ============================================================================

#[test]
fn rebuild_collateral_index_heals_corrupted_index() {
    let mut state = fresh_state_with_price(5.0);
    let icp = state.icp_collateral_type();
    let bogus_ct = Principal::from_slice(&[88]);
    state.open_vault(make_vault(1, 0, 0));
    state.open_vault(make_vault(2, 0, 0));
    state.open_vault(make_vault(3, 0, 0));

    // Corrupt the index the way mainnet drifted: a stale id whose vault is
    // gone, a duplicate entry under a wrong collateral type, and a missing
    // entry for a live vault.
    state
        .collateral_to_vault_ids
        .get_mut(&icp)
        .unwrap()
        .insert(99);
    state
        .collateral_to_vault_ids
        .entry(bogus_ct)
        .or_default()
        .insert(2);
    state
        .collateral_to_vault_ids
        .get_mut(&icp)
        .unwrap()
        .remove(&3);

    let (stale_removed, missing_added) = state.rebuild_collateral_index();

    assert_eq!(stale_removed, 2, "stale id 99 + wrong-type duplicate of 2");
    assert_eq!(missing_added, 1, "vault 3 was missing from the index");
    assert_eq!(indexed_ids(&state, &icp), vec![1, 2, 3]);
    assert!(state.collateral_to_vault_ids.get(&bogus_ct).is_none());
    assert_index_matches_vaults(&state, "after rebuild");
}

#[test]
fn rebuild_collateral_index_is_idempotent_on_consistent_state() {
    let mut state = fresh_state_with_price(5.0);
    state.open_vault(make_vault(1, 0, 0));
    state.open_vault(make_vault(2, 0, 0));

    let (stale_removed, missing_added) = state.rebuild_collateral_index();

    assert_eq!((stale_removed, missing_added), (0, 0));
    assert_index_matches_vaults(&state, "after no-op rebuild");
}

#[test]
fn prune_empty_principal_entries_removes_only_empty_sets() {
    let mut state = fresh_state_with_price(5.0);
    state.open_vault(make_vault(1, 0, 0));
    // Historical close paths left empty sets behind; simulate one.
    let ghost = Principal::from_slice(&[9, 9]);
    state
        .principal_to_vault_ids
        .insert(ghost, std::collections::BTreeSet::new());

    let pruned = state.prune_empty_principal_entries();

    assert_eq!(pruned, 1);
    assert!(state.principal_to_vault_ids.get(&ghost).is_none());
    assert_eq!(
        state
            .principal_to_vault_ids
            .get(&owner())
            .map(|ids| ids.len()),
        Some(1),
        "live owner entry must survive the prune",
    );
}

#[test]
fn cleanup_if_drained_removes_zero_zero_vault() {
    let mut state = fresh_state_with_price(5.0);
    let icp = state.icp_collateral_type();
    state.open_vault(make_vault(1, 0, 0));
    state.open_vault(make_vault(2, 100_000_000, 50_000_000));

    assert!(
        state.cleanup_if_drained(1),
        "a zero-debt zero-collateral vault must be reported drained",
    );
    assert!(!state.vault_id_to_vaults.contains_key(&1));
    assert_eq!(indexed_ids(&state, &icp), vec![2]);
    assert_index_matches_vaults(&state, "after drain cleanup");
}

#[test]
fn cleanup_if_drained_keeps_vault_with_remaining_balances() {
    let mut state = fresh_state_with_price(5.0);
    let icp = state.icp_collateral_type();
    // Zero debt but collateral remains (post-repay, pre-withdraw) — NOT drained.
    state.open_vault(make_vault(1, 100_000_000, 0));
    // Zero collateral but debt remains (underwater write-down) — NOT drained.
    state.open_vault(make_vault(2, 0, 50_000_000));

    assert!(!state.cleanup_if_drained(1));
    assert!(!state.cleanup_if_drained(2));
    assert!(!state.cleanup_if_drained(99), "missing vault is not drained");
    assert_eq!(indexed_ids(&state, &icp), vec![1, 2]);
}

#[test]
fn check_invariants_accepts_consistent_state() {
    let mut state = fresh_state_with_price(5.0);
    state.open_vault(make_vault(1, 0, 0));
    state.open_vault(make_vault(2, 0, 0));
    assert!(state.check_invariants().is_ok());
}

#[test]
fn check_invariants_rejects_stale_collateral_index_id() {
    let mut state = fresh_state_with_price(5.0);
    let icp = state.icp_collateral_type();
    state.open_vault(make_vault(1, 0, 0));
    state
        .collateral_to_vault_ids
        .get_mut(&icp)
        .unwrap()
        .insert(99);

    assert!(
        state.check_invariants().is_err(),
        "an indexed id with no live vault must fail the invariant check",
    );
}

#[test]
fn check_invariants_rejects_unindexed_vault() {
    let mut state = fresh_state_with_price(5.0);
    let icp = state.icp_collateral_type();
    state.open_vault(make_vault(1, 0, 0));
    state
        .collateral_to_vault_ids
        .get_mut(&icp)
        .unwrap()
        .remove(&1);

    assert!(
        state.check_invariants().is_err(),
        "a live vault missing from the collateral index must fail the invariant check",
    );
}

#[test]
fn check_invariants_rejects_wrong_collateral_type_entry() {
    let mut state = fresh_state_with_price(5.0);
    let icp = state.icp_collateral_type();
    let bogus_ct = Principal::from_slice(&[88]);
    state.open_vault(make_vault(1, 0, 0));
    state
        .collateral_to_vault_ids
        .get_mut(&icp)
        .unwrap()
        .remove(&1);
    state
        .collateral_to_vault_ids
        .entry(bogus_ct)
        .or_default()
        .insert(1);

    assert!(
        state.check_invariants().is_err(),
        "a vault indexed under the wrong collateral type must fail the invariant check",
    );
}

#[test]
fn semantic_eq_catches_collateral_index_divergence() {
    let mut a = fresh_state_with_price(5.0);
    let mut b = fresh_state_with_price(5.0);
    a.open_vault(make_vault(1, 0, 0));
    b.open_vault(make_vault(1, 0, 0));
    assert!(a.check_semantically_eq(&b).is_ok());

    let icp = b.icp_collateral_type();
    b.collateral_to_vault_ids.get_mut(&icp).unwrap().insert(99);

    assert!(
        a.check_semantically_eq(&b).is_err(),
        "self_check must flag a replayed/live collateral-index mismatch",
    );
}
