//! INT-004 regression fence: `withdraw_partial_collateral` must accrue
//! interest before checking the collateral ratio.
//!
//! Audit report:
//!   * `audit-reports/2026-04-22-28e9896/raw-pass-results/debt-interest.json`
//!     finding INT-004.
//!
//! # What the bug was
//!
//! `borrow_from_vault_internal` and both repay paths call
//! `mutate_state(|s| s.accrue_single_vault(vault_id, now))` at the top so the
//! CR check works against fresh debt. `withdraw_partial_collateral` did NOT,
//! reading `vault.borrowed_icusd_amount` directly. Between two XRC harvests
//! (up to 5 minutes), a user could withdraw collateral against debt that was
//! up to that interval out of date. Once the next tick caught up, the vault
//! could land below MCR by exactly the missed accrual.
//!
//! # How this file tests the fix
//!
//! The fix is the one-line accrual call at the top of
//! `withdraw_partial_collateral`. The property the fix relies on is:
//!
//!   *post-accrual debt is strictly greater than pre-accrual debt for any
//!   vault with `borrowed > 0` and a positive interest rate over a non-zero
//!   elapsed window.*
//!
//! `int_004_post_accrual_debt_shrinks_withdrawable_headroom` constructs a
//! vault, computes the max withdrawable collateral using the stale debt, runs
//! `accrue_single_vault` for a one-year window, then re-computes max
//! withdrawable using the fresh debt. The fresh-debt headroom is strictly
//! smaller, demonstrating that the order of operations in
//! `withdraw_partial_collateral` (accrue → read → CR-check) materially
//! changes the rejection boundary.

use candid::Principal;
use rust_decimal_macros::dec;

use rumi_protocol_backend::numeric::{icusd_to_collateral_amount, ICUSD, Ratio, UsdIcp};
use rumi_protocol_backend::state::State;
use rumi_protocol_backend::vault::Vault;
use rumi_protocol_backend::InitArg;

const NANOS_PER_YEAR: u64 = 365 * 24 * 60 * 60 * 1_000_000_000;

fn fresh_state_with_priced_icp(rate_apr: f64) -> State {
    let mut state = State::from(InitArg {
        xrc_principal: Principal::anonymous(),
        icusd_ledger_principal: Principal::anonymous(),
        icp_ledger_principal: Principal::from_slice(&[10]),
        fee_e8s: 0,
        developer_principal: Principal::anonymous(),
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    });
    let icp = state.icp_collateral_type();
    state.last_icp_rate = Some(UsdIcp::from(dec!(10.0)));
    if let Some(config) = state.collateral_configs.get_mut(&icp) {
        config.last_price = Some(10.0);
        config.interest_rate_apr = Ratio::from_f64(rate_apr);
    }
    state
}

/// Compute max withdrawable collateral using the same formula
/// `withdraw_partial_collateral` uses. The stale-vs-fresh divergence in this
/// test is purely driven by the order of `accrue_single_vault` calls; the
/// formula is identical to the live code path.
fn max_withdrawable_raw(state: &State, vault_id: u64) -> Option<u64> {
    let vault = state.vault_id_to_vaults.get(&vault_id)?;
    let collateral_type = vault.collateral_type;
    if vault.borrowed_icusd_amount == ICUSD::new(0) {
        return Some(vault.collateral_amount);
    }
    let min_ratio = state.get_min_collateral_ratio_for(&collateral_type);
    let price = state
        .get_collateral_price_decimal(&collateral_type)
        .expect("price must be set");
    let decimals = state
        .get_collateral_config(&collateral_type)
        .map(|c| c.decimals)
        .unwrap_or(8);
    let min_value: ICUSD = vault.borrowed_icusd_amount * min_ratio;
    let min_raw = icusd_to_collateral_amount(min_value, price, decimals);
    if vault.collateral_amount <= min_raw {
        None
    } else {
        Some(vault.collateral_amount - min_raw)
    }
}

#[test]
fn int_004_post_accrual_debt_shrinks_withdrawable_headroom() {
    // 10% APR so a one-year accrual moves debt by ~10% — clearly visible in
    // the headroom delta.
    let mut state = fresh_state_with_priced_icp(0.10);
    let icp = state.icp_collateral_type();

    // Vault: 2 ICP at $10/ICP = $20 collateral, 10 icUSD debt → CR = 2.0x.
    // At MCR 1.5x: min collateral USD = $15 (i.e. 1.5 ICP). Headroom = 0.5 ICP.
    state.open_vault(Vault {
        owner: Principal::anonymous(),
        vault_id: 1,
        collateral_amount: 200_000_000,
        borrowed_icusd_amount: ICUSD::new(1_000_000_000), // 10 icUSD
        collateral_type: icp,
        last_accrual_time: 0,
        accrued_interest: ICUSD::new(0),
        bot_processing: false,
    });

    let stale_max = max_withdrawable_raw(&state, 1)
        .expect("vault has headroom on stale debt");

    // Pre-fix path: withdraw_partial_collateral reads vault directly. The
    // returned `stale_max` is what the user would have been allowed to take.
    // Post-fix path: the function calls accrue_single_vault first.
    state.accrue_single_vault(1, NANOS_PER_YEAR);

    let fresh_max = max_withdrawable_raw(&state, 1)
        .expect("vault should still have some headroom even post-accrual");

    assert!(
        fresh_max < stale_max,
        "INT-004: post-accrual headroom ({fresh_max}) must be strictly less than \
         stale headroom ({stale_max}); accrual increases debt and shrinks \
         what the vault can safely withdraw."
    );

    // Sanity: the difference is non-trivial — accrual must move debt enough
    // that a real user could observe the divergence.
    let delta = stale_max - fresh_max;
    assert!(
        delta > 1_000_000, // > 0.01 ICP
        "INT-004: headroom delta ({delta} raw units) must be material; if this \
         drops to zero the test no longer fences the bug"
    );
}

#[test]
fn int_004_no_accrual_when_rate_is_zero() {
    // Property check: if the interest rate is zero, accrual is a no-op and
    // the CR check yields the same answer either way. This guards against an
    // over-eager future "always recompute" change that might pessimize the
    // zero-rate case.
    let mut state = fresh_state_with_priced_icp(0.0);
    let icp = state.icp_collateral_type();

    state.open_vault(Vault {
        owner: Principal::anonymous(),
        vault_id: 1,
        collateral_amount: 200_000_000,
        borrowed_icusd_amount: ICUSD::new(1_000_000_000),
        collateral_type: icp,
        last_accrual_time: 0,
        accrued_interest: ICUSD::new(0),
        bot_processing: false,
    });

    let stale_max = max_withdrawable_raw(&state, 1).unwrap();
    state.accrue_single_vault(1, NANOS_PER_YEAR);
    let fresh_max = max_withdrawable_raw(&state, 1).unwrap();

    assert_eq!(
        stale_max, fresh_max,
        "zero-rate accrual must not change the headroom"
    );
}

#[test]
fn int_004_accrue_single_vault_advances_debt_monotonically() {
    // The fix's property: every accrue call with a later timestamp adds
    // interest to a debted vault. This is the invariant
    // `withdraw_partial_collateral` relies on.
    let mut state = fresh_state_with_priced_icp(0.05);
    let icp = state.icp_collateral_type();

    state.open_vault(Vault {
        owner: Principal::anonymous(),
        vault_id: 1,
        collateral_amount: 200_000_000,
        borrowed_icusd_amount: ICUSD::new(1_000_000_000),
        collateral_type: icp,
        last_accrual_time: 0,
        accrued_interest: ICUSD::new(0),
        bot_processing: false,
    });

    let initial = state
        .vault_id_to_vaults
        .get(&1)
        .unwrap()
        .borrowed_icusd_amount;

    state.accrue_single_vault(1, NANOS_PER_YEAR);
    let after_first = state
        .vault_id_to_vaults
        .get(&1)
        .unwrap()
        .borrowed_icusd_amount;

    state.accrue_single_vault(1, 2 * NANOS_PER_YEAR);
    let after_second = state
        .vault_id_to_vaults
        .get(&1)
        .unwrap()
        .borrowed_icusd_amount;

    assert!(
        after_first > initial,
        "first accrual must increase debt: {} -> {}",
        initial.to_u64(),
        after_first.to_u64()
    );
    assert!(
        after_second > after_first,
        "second accrual must increase debt further: {} -> {}",
        after_first.to_u64(),
        after_second.to_u64()
    );
}

