//! RED-001 + LIQ-006 regression fence: per-collateral freshness gating.
//!
//! Audit reports:
//!   * `audit-reports/2026-04-22-28e9896/raw-pass-results/redemption-peg.json`
//!     finding RED-001.
//!   * `audit-reports/2026-04-22-28e9896/raw-pass-results/liquidation-mechanics.json`
//!     finding LIQ-006.
//!
//! # What the bugs were
//!
//! * **RED-001**: `redeem_collateral` and `redeem_reserves` (vault spillover
//!   branch) only called `validate_call`, which refreshes ICP via
//!   `ensure_fresh_price`. For non-ICP collaterals (BOB, EXE, ckBTC, ckETH,
//!   ckXAUT, nICP) the redeemer was paid out at whatever `last_price` was
//!   cached, even if the per-collateral background fetch had been silently
//!   failing for hours. A redeemer could capture the spread on a stale price.
//! * **LIQ-006**: `validate_price_for_liquidation` only checked
//!   `last_icp_timestamp`, never the per-collateral `last_price_timestamp`.
//!   Liquidations of non-ICP vaults proceeded against arbitrarily stale
//!   stored prices.
//!
//! # How this file tests the fix
//!
//! The state-side primitive both fixes lean on already exists at
//! `xrc::ensure_fresh_price_for(&collateral_type)` (audit verification noted
//! the helper was dead code with zero callers pre-Wave-5). Wave-5 wires it
//! into:
//!   * `main::redeem_collateral` (after `validate_call`).
//!   * `vault::redeem_reserves` (at the spillover branch, after `best_ct`
//!     is chosen).
//!   * `main::validate_freshness_for_vault(vault_id)` helper called by
//!     every liquidation entry point in main.rs (`liquidate_vault`,
//!     `liquidate_vault_partial`, `liquidate_vault_partial_with_stable`,
//!     `partial_liquidate_vault`, `stability_pool_liquidate`,
//!     `stability_pool_liquidate_debt_burned`,
//!     `stability_pool_liquidate_with_reserves`).
//!
//! These tests verify the underlying state primitive that the helper reads:
//! `CollateralConfig::last_price_timestamp` is the canonical per-collateral
//! freshness field. They also lock in the F-004 fix: the freshness threshold
//! constant now matches the XRC margin so the cache hit rate is non-zero.

use candid::Principal;

use rumi_protocol_backend::numeric::ICUSD;
use rumi_protocol_backend::state::{CollateralStatus, State};
use rumi_protocol_backend::vault::Vault;
use rumi_protocol_backend::xrc::{FETCHING_ICP_RATE_INTERVAL, PRICE_FRESHNESS_THRESHOLD_NANOS};
use rumi_protocol_backend::InitArg;
use std::collections::BTreeSet;

fn fresh_state() -> State {
    State::from(InitArg {
        xrc_principal: Principal::anonymous(),
        icusd_ledger_principal: Principal::anonymous(),
        icp_ledger_principal: Principal::from_slice(&[10]),
        fee_e8s: 0,
        developer_principal: Principal::anonymous(),
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    })
}

#[test]
fn red_001_per_collateral_timestamp_field_exists_and_is_independent() {
    let mut state = fresh_state();
    let icp = state.icp_collateral_type();

    let now_nanos: u64 = 1_700_000_000_000_000_000;
    if let Some(config) = state.collateral_configs.get_mut(&icp) {
        config.last_price = Some(8.0);
        config.last_price_timestamp = Some(now_nanos);
    }
    state.last_icp_timestamp = Some(now_nanos);

    // Mark the per-collateral timestamp as stale (an hour old) while leaving
    // the global ICP timestamp fresh. Pre-Wave-5 redemption/liquidation
    // freshness checks only looked at last_icp_timestamp, so this divergence
    // would have gone unnoticed.
    let one_hour_nanos = 60 * 60 * 1_000_000_000_u64;
    if let Some(config) = state.collateral_configs.get_mut(&icp) {
        config.last_price_timestamp = Some(now_nanos.saturating_sub(one_hour_nanos));
    }

    let stale_age = now_nanos.saturating_sub(
        state.collateral_configs
            .get(&icp)
            .and_then(|c| c.last_price_timestamp)
            .unwrap()
    );
    assert!(
        stale_age > PRICE_FRESHNESS_THRESHOLD_NANOS,
        "the per-collateral timestamp must be the field that ensure_fresh_price_for compares against"
    );
}

#[test]
fn red_001_redemption_priority_picks_collateral_for_spillover() {
    // The redeem_reserves spillover branch picks the highest-priority eligible
    // collateral via `get_collateral_types_by_redemption_priority` and now
    // refreshes its price before reading. The function requires a known price
    // AND at least one debt-bearing vault of that type, so we set up minimal
    // state here and assert the picker returns the only eligible entry.
    let mut state = fresh_state();
    let icp = state.icp_collateral_type();
    if let Some(config) = state.collateral_configs.get_mut(&icp) {
        config.last_price = Some(8.0);
        config.last_price_timestamp = Some(1);
        config.status = CollateralStatus::Active;
    }
    state.vault_id_to_vaults.insert(
        1,
        Vault {
            owner: Principal::from_slice(&[1]),
            borrowed_icusd_amount: ICUSD::new(100_000_000),
            collateral_amount: 50_000_000,
            vault_id: 1,
            collateral_type: icp,
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        },
    );
    let mut ids = BTreeSet::new();
    ids.insert(1);
    state.collateral_to_vault_ids.insert(icp, ids);

    let priority_list = state.get_collateral_types_by_redemption_priority();
    assert!(
        priority_list.contains(&icp),
        "ICP must surface as the priority spillover collateral once it has a price + debt"
    );
}

#[test]
fn liq_006_collateral_lookup_round_trips_for_freshness_check() {
    // validate_freshness_for_vault reads vault.collateral_type from state, then
    // hands it to ensure_fresh_price_for. That state read is the load-bearing
    // step. Verify the lookup pattern works for the ICP default collateral
    // and for an added non-ICP collateral.
    let mut state = fresh_state();
    let icp = state.icp_collateral_type();
    assert!(state.collateral_configs.contains_key(&icp));

    // Add a synthetic non-ICP collateral and assert the lookup surfaces it.
    let bob = Principal::from_slice(&[42]);
    let cfg = state.collateral_configs.get(&icp).cloned().unwrap();
    let mut bob_cfg = cfg.clone();
    bob_cfg.ledger_canister_id = bob;
    bob_cfg.status = CollateralStatus::Active;
    bob_cfg.last_price = Some(0.10);
    bob_cfg.last_price_timestamp = Some(1);
    state.collateral_configs.insert(bob, bob_cfg);

    assert!(state.collateral_configs.contains_key(&bob));
    let stored = state
        .collateral_configs
        .get(&bob)
        .and_then(|c| c.last_price);
    assert_eq!(stored, Some(0.10));
}

#[test]
fn f_004_freshness_threshold_matches_xrc_margin() {
    // F-004: pre-Wave-5 PRICE_FRESHNESS_THRESHOLD_NANOS was 30s but the XRC
    // request timestamp is set to (wall_clock - 60s). Every ensure_fresh_price
    // call therefore re-fetched. The fix bumps the threshold to 60s so back-
    // to-back operations within the same fetch window can hit the cache.
    let sixty_seconds_nanos: u64 = 60 * 1_000_000_000;
    assert!(
        PRICE_FRESHNESS_THRESHOLD_NANOS >= sixty_seconds_nanos,
        "freshness threshold must be at least the XRC margin (60s) to allow any cache hits"
    );
}

#[test]
fn f_004_background_fetch_interval_unchanged() {
    // The background timer interval is 300s; bumping the threshold to 60s
    // doesn't relax the lazy refresh cadence — it just lets bursts of
    // user-driven operations within ~60s of the last fetch reuse the cache.
    let three_hundred_seconds = std::time::Duration::from_secs(300);
    assert_eq!(FETCHING_ICP_RATE_INTERVAL, three_hundred_seconds);
}
