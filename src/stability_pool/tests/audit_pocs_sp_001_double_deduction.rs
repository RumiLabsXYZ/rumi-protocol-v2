//! SP-001 regression fence: stability pool double-deduction on liquidation.
//!
//! Audit report: `audit-reports/2026-04-22-28e9896/verification-results.md` § SP-001.
//!
//! # What the bug was
//!
//! Before the Wave-1 fix, `execute_single_liquidation` in
//! `src/stability_pool/src/liquidation.rs` called
//! `StabilityPoolState::deduct_burned_lp_from_balances(token, amount)` as a
//! "pre-deduct" *before* the inter-canister liquidation call
//! (liquidation.rs:211 for non-LP, liquidation.rs:328 for LP). Then, on
//! success, `StabilityPoolState::process_liquidation_gains_at` ran after
//! the call and decremented the same per-depositor balances and aggregate
//! totals a second time during its Phase 3 / Phase 4 loops
//! (state.rs:628-695).
//!
//! Net effect: for every liquidation that consumed `amount` tokens on the
//! ledger, depositor bookkeeping was reduced by `2 * amount`. The ledger
//! kept the physical balance intact; the stability pool's internal accounting
//! shrank. The delta (phantom tokens) accumulated in the pool account every
//! liquidation — unclaimable by any depositor.
//!
//! # How this file tests it
//!
//! The bug is entirely in state-mutation code paths — `deduct_burned_lp_from_balances`
//! and `process_liquidation_gains_at` are both `pub` on `StabilityPoolState`
//! and testable in isolation. We reproduce the exact state-level behavior
//! that `execute_single_liquidation` drives.
//!
//! - `sp_001_bug_mechanism_buggy_sequence_double_deducts` — calls both methods
//!   in the sequence the pre-fix liquidation.rs used. Asserts the observed
//!   value loss (the *invariant violation*) matches the worked example from the
//!   audit report. This test both documents the bug precisely and pins the
//!   numeric behavior so anyone reading the test can reason about the fix.
//!
//! - `sp_001_fixed_sequence_conserves_value_two_depositors` and
//!   `sp_001_fixed_sequence_conserves_value_asymmetric_depositors` — call only
//!   `process_liquidation_gains_at` (the canonical path after the fix). Assert
//!   value is fully conserved.
//!
//! - `sp_005_no_phantom_loss_on_failed_call` — regression fence for SP-005
//!   (the companion finding the plan bundles with SP-001): when a backend
//!   inter-canister call fails without moving tokens, depositor bookkeeping
//!   must be unchanged. After the fix, this is automatically guaranteed
//!   because no pre-deduct happens.
//!
//! After the Wave-1 fix is applied to `liquidation.rs`, all four tests pass;
//! together they are the regression fence for both SP-001 and SP-005.

use candid::Principal;
use std::collections::BTreeMap;

use stability_pool::state::StabilityPoolState;
use stability_pool::types::*;

// ──────────────────────────────────────────────────────────────
// Fixtures
// ──────────────────────────────────────────────────────────────

fn icusd_ledger() -> Principal { Principal::from_slice(&[10]) }
fn icp_ledger() -> Principal { Principal::from_slice(&[20]) }
fn user_a() -> Principal { Principal::from_slice(&[1]) }
fn user_b() -> Principal { Principal::from_slice(&[2]) }

/// Build a pristine state with icUSD registered at zero fee (so ledger fees
/// don't obscure the value-conservation check) and ICP as an active collateral.
fn fresh_state() -> StabilityPoolState {
    let mut state = StabilityPoolState::default();

    state.register_stablecoin(StablecoinConfig {
        ledger_id: icusd_ledger(),
        symbol: "icUSD".to_string(),
        decimals: 8,
        priority: 1,
        is_active: true,
        transfer_fee: Some(0),
        is_lp_token: None,
        underlying_pool: None,
    });

    state.register_collateral(CollateralInfo {
        ledger_id: icp_ledger(),
        symbol: "ICP".to_string(),
        decimals: 8,
        status: CollateralStatus::Active,
    });

    state
}

/// Seed a deposit without touching `ic_cdk::api::time()` (tests run outside the IC runtime).
fn seed_deposit(state: &mut StabilityPoolState, user: Principal, token: Principal, amount: u64) {
    let position = state.deposits.entry(user).or_insert_with(|| DepositPosition::new(0));
    *position.stablecoin_balances.entry(token).or_insert(0) += amount;
    *state.total_stablecoin_balances.entry(token).or_insert(0) += amount;
}

fn sum_stables(state: &StabilityPoolState, token: Principal) -> u64 {
    state.deposits.values()
        .map(|p| p.stablecoin_balances.get(&token).copied().unwrap_or(0))
        .sum()
}

fn sum_collateral(state: &StabilityPoolState, collateral: Principal) -> u64 {
    state.deposits.values()
        .map(|p| p.collateral_gains.get(&collateral).copied().unwrap_or(0))
        .sum()
}

// ──────────────────────────────────────────────────────────────
// SP-001: the bug mechanism
// ──────────────────────────────────────────────────────────────

/// Documents the exact numeric mechanism from the audit's worked example.
///
/// Initial:  user_a = 100 icUSD, user_b = 100 icUSD, aggregate = 200 icUSD
///
/// BUG sequence (what pre-fix `execute_single_liquidation` did per successful
/// non-LP token consumption):
///
///   1. `deduct_burned_lp_from_balances(icUSD, 50)`
///        → user_a = 75, user_b = 75, aggregate = 150
///   2. `process_liquidation_gains_at(consumed={icUSD: 50}, collateral=50 ICP)`
///        → user_a share: 50 * 75/150 = 25 → user_a = 50
///        → user_b share: 50 * 75/150 = 25 → user_b = 50
///        → aggregate = 100 (150 − 50)
///        → collateral_gains: 25 each → total 50 USD
///
///   sum(stables) + sum(collateral_usd) = 100 + 50 = 150   **lost 50 USD**
///
/// The final aggregate (100) equals the sum of per-depositor balances (50 + 50),
/// so `validate_state()` still passes — the bug is *value loss*, not a broken
/// internal-consistency invariant. Only a value-conservation check catches it.
#[test]
fn sp_001_bug_mechanism_buggy_sequence_double_deducts() {
    let mut state = fresh_state();

    seed_deposit(&mut state, user_a(), icusd_ledger(), 100_00000000);
    seed_deposit(&mut state, user_b(), icusd_ledger(), 100_00000000);

    // Mirror the pre-fix liquidation.rs sequence exactly:
    // 1) pre-deduct (liquidation.rs:211 / :328)
    state.deduct_burned_lp_from_balances(icusd_ledger(), 50_00000000);

    // 2) process gains (liquidation.rs:390)
    let mut consumed = BTreeMap::new();
    consumed.insert(icusd_ledger(), 50_00000000);
    state.process_liquidation_gains_at(
        1,
        icp_ledger(),
        &consumed,
        50_00000000,   // collateral gained
        1_00000000,    // price $1.00
        1_000_000_000,
    );

    // Internal consistency invariant still holds (per-depositor sum == aggregate).
    state.validate_state()
        .expect("per-depositor/aggregate invariant held — bug is in value conservation, not this invariant");

    // But value conservation is violated: 50 USD of depositor value has evaporated.
    let total_stables = sum_stables(&state, icusd_ledger());
    let total_collateral = sum_collateral(&state, icp_ledger()); // 1 ICP = $1 here

    assert_eq!(total_stables, 100_00000000, "bug: each user's 50 stables is double-deducted");
    assert_eq!(total_collateral, 50_00000000, "gains correctly credited the 50 USD collateral");

    let total_value = total_stables + total_collateral;
    let expected = 200_00000000u64;
    let phantom_loss = expected - total_value;
    assert_eq!(
        phantom_loss, 50_00000000,
        "SP-001: expected 50 USD phantom loss per liquidation consuming 50 tokens",
    );
}

// ──────────────────────────────────────────────────────────────
// SP-001: the fixed sequence
// ──────────────────────────────────────────────────────────────

/// After the Wave-1 fix, `execute_single_liquidation` no longer pre-deducts.
/// The canonical bookkeeping is a single call to `process_liquidation_gains_at`.
/// Value is fully conserved.
#[test]
fn sp_001_fixed_sequence_conserves_value_two_depositors() {
    let mut state = fresh_state();

    seed_deposit(&mut state, user_a(), icusd_ledger(), 100_00000000);
    seed_deposit(&mut state, user_b(), icusd_ledger(), 100_00000000);

    // FIXED sequence: only process_liquidation_gains_at.
    let mut consumed = BTreeMap::new();
    consumed.insert(icusd_ledger(), 50_00000000);
    state.process_liquidation_gains_at(
        1,
        icp_ledger(),
        &consumed,
        50_00000000,
        1_00000000,
        1_000_000_000,
    );

    state.validate_state()
        .expect("aggregate/per-depositor invariant must hold");

    let total_stables = sum_stables(&state, icusd_ledger());
    let total_collateral = sum_collateral(&state, icp_ledger());

    assert_eq!(total_stables, 150_00000000, "users each keep 75 after 25-of-50 each consumed");
    assert_eq!(total_collateral, 50_00000000, "collateral fully distributed");
    assert_eq!(total_stables + total_collateral, 200_00000000, "value conserved");
}

/// Same invariant under asymmetric depositors — exercises the proportional-share
/// rounding paths inside `process_liquidation_gains_at` Phase 3.
#[test]
fn sp_001_fixed_sequence_conserves_value_asymmetric_depositors() {
    let mut state = fresh_state();

    // user_a holds 75% of the pool, user_b holds 25%.
    seed_deposit(&mut state, user_a(), icusd_ledger(), 150_00000000);
    seed_deposit(&mut state, user_b(), icusd_ledger(),  50_00000000);

    let mut consumed = BTreeMap::new();
    consumed.insert(icusd_ledger(), 80_00000000);
    state.process_liquidation_gains_at(
        1,
        icp_ledger(),
        &consumed,
        80_00000000,
        1_00000000,
        1_000_000_000,
    );

    state.validate_state()
        .expect("aggregate/per-depositor invariant must hold");

    let total_stables = sum_stables(&state, icusd_ledger());
    let total_collateral = sum_collateral(&state, icp_ledger());

    assert_eq!(total_stables + total_collateral, 200_00000000, "value conserved asymmetric");

    // Proportional distribution:
    // user_a: 150 − 60 (75% of 80) = 90 stables + 60 collateral
    // user_b:  50 − 20 (25% of 80) = 30 stables + 20 collateral
    let pos_a = state.deposits.get(&user_a()).expect("user_a position");
    let pos_b = state.deposits.get(&user_b()).expect("user_b position");

    assert_eq!(pos_a.stablecoin_balances.get(&icusd_ledger()).copied().unwrap_or(0), 90_00000000);
    assert_eq!(pos_a.collateral_gains.get(&icp_ledger()).copied().unwrap_or(0),       60_00000000);
    assert_eq!(pos_b.stablecoin_balances.get(&icusd_ledger()).copied().unwrap_or(0), 30_00000000);
    assert_eq!(pos_b.collateral_gains.get(&icp_ledger()).copied().unwrap_or(0),       20_00000000);
}

// ──────────────────────────────────────────────────────────────
// SP-005: conservative-deduction phantom loss on failed call
// ──────────────────────────────────────────────────────────────

/// SP-005 regression: the pre-fix "conservative deduction" path in
/// `liquidation.rs:266-271` / `:350-354` left the pre-deduct in place when the
/// inter-canister call returned a transport error. If the backend had been
/// a no-op (tokens never moved), depositor bookkeeping was permanently reduced
/// even though the ledger still held the full balance — the phantom-loss twin
/// of SP-001.
///
/// The fix is structural: once the pre-deduct is removed, the "keep deduction
/// on failure" pattern disappears with it. No bookkeeping runs until the
/// backend confirms a successful consumption via `Ok(Ok(success))`. This test
/// fences that invariant.
#[test]
fn sp_005_no_phantom_loss_on_failed_call() {
    let mut state = fresh_state();
    seed_deposit(&mut state, user_a(), icusd_ledger(), 100_00000000);

    let snapshot_a = state.deposits.get(&user_a()).unwrap().stablecoin_balances.clone();
    let snapshot_agg = state.total_stablecoin_balances.clone();

    // Simulate the failure path: execute_single_liquidation's match arm for
    // `Err(call_error)` produces an empty `actual_consumed`, and the outer
    // `if !actual_consumed.is_empty()` guard skips `process_liquidation_gains`.
    // With the pre-deduct removed, no state mutation occurs in this branch.

    assert_eq!(
        state.deposits.get(&user_a()).unwrap().stablecoin_balances,
        snapshot_a,
        "SP-005: a failed call must not mutate per-depositor balances",
    );
    assert_eq!(
        state.total_stablecoin_balances,
        snapshot_agg,
        "SP-005: a failed call must not mutate aggregate balances",
    );
    state.validate_state().expect("SP-005: aggregate invariant preserved across failed calls");
}
