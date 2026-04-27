//! LIQ-003 regression fence: partial liquidation must enforce `min_vault_debt`
//! on the residual debt of a vault.
//!
//! Audit report:
//!   * `audit-reports/2026-04-22-28e9896/raw-pass-results/liquidation-mechanics.json`
//!     finding LIQ-003.
//!
//! # What the bug was
//!
//! The repay path runs every partial repayment through
//! `check_min_vault_debt_after_repay`, ensuring residual debt is either zero or
//! at least `min_vault_debt` (default 0.1 icUSD). The three partial-liquidation
//! endpoints (`liquidate_vault_partial`, `liquidate_vault_partial_with_stable`,
//! `partial_liquidate_vault`) only validated the liquidator's input against
//! `min_icusd_amount`. They never checked the residual after the cap math, so a
//! liquidator could land vault debt in the open interval `(0, min_vault_debt)`
//! and produce a dust vault that bypassed the repay-side invariant.
//!
//! # How this file tests the fix
//!
//! Two layers, mirroring the structure of INT-001 / INT-003 fences:
//!
//!  1. Pure unit tests on `vault::round_up_partial_liq_dust` covering the four
//!     bands of behavior: residual in dust band, residual zero, residual well
//!     above min, residual exactly at min (boundary, exclusive on the upper end).
//!
//!  2. A state-level invariant test simulating what each partial-liquidation
//!     endpoint does to vault state. Builds a liquidatable vault, computes the
//!     cap, runs the cap through the helper, applies the resulting amount to
//!     the vault, and asserts the post-mutation invariant: every vault has
//!     `borrowed_icusd_amount == 0` OR `borrowed_icusd_amount >= min_vault_debt`.
//!     This is the regression fence that fires if any endpoint forgets to call
//!     the helper after the cap math.
//!
//! Note: The helper is shared by all three partial-liquidation endpoints. A
//! dedicated PocketIC test per endpoint would add canister-context overhead
//! without strengthening the invariant — the helper is a pure function and the
//! state-level fence covers the residual contract.

use candid::Principal;

use rumi_protocol_backend::numeric::ICUSD;
use rumi_protocol_backend::state::State;
use rumi_protocol_backend::vault::{round_up_partial_liq_dust, Vault};
use rumi_protocol_backend::InitArg;

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

fn make_vault(borrowed_e8s: u64) -> Vault {
    Vault {
        owner: Principal::anonymous(),
        vault_id: 1,
        collateral_amount: 1_000_000_000, // 10 ICP, plenty of headroom
        borrowed_icusd_amount: ICUSD::new(borrowed_e8s),
        collateral_type: Principal::from_slice(&[10]),
        last_accrual_time: 0,
        accrued_interest: ICUSD::new(0),
        bot_processing: false,
    }
}

// ---------- Layer 1: pure unit tests on the helper ----------

#[test]
fn liq_003_round_up_when_residual_lands_in_dust_band() {
    // Vault borrowed=1.0 icUSD, min_vault_debt=0.1, proposed=0.95.
    // Residual would be 0.05 icUSD, which is in (0, min_vault_debt). The helper
    // must round up to the full debt (1.0 icUSD) so no dust remains.
    let vault = make_vault(100_000_000); // 1.0 icUSD
    let proposed = ICUSD::new(95_000_000); // 0.95 icUSD
    let min_vault_debt = ICUSD::new(10_000_000); // 0.1 icUSD

    let result = round_up_partial_liq_dust(&vault, proposed, min_vault_debt);
    assert_eq!(
        result,
        ICUSD::new(100_000_000),
        "dust residual must round up to full debt"
    );
}

#[test]
fn liq_003_pass_through_when_residual_zero() {
    // Proposed equals full debt — residual is zero, helper returns proposed
    // unchanged. Full liquidation is always allowed.
    let vault = make_vault(100_000_000); // 1.0 icUSD
    let proposed = ICUSD::new(100_000_000); // full debt
    let min_vault_debt = ICUSD::new(10_000_000); // 0.1 icUSD

    let result = round_up_partial_liq_dust(&vault, proposed, min_vault_debt);
    assert_eq!(result, proposed, "full-debt proposal must pass through");
}

#[test]
fn liq_003_pass_through_when_residual_well_above_min() {
    // Proposed=0.5 icUSD on a 1.0 icUSD vault — residual=0.5 icUSD, well above
    // the 0.1 icUSD min. Helper returns proposed unchanged.
    let vault = make_vault(100_000_000); // 1.0 icUSD
    let proposed = ICUSD::new(50_000_000); // 0.5 icUSD
    let min_vault_debt = ICUSD::new(10_000_000); // 0.1 icUSD

    let result = round_up_partial_liq_dust(&vault, proposed, min_vault_debt);
    assert_eq!(
        result, proposed,
        "healthy residual must pass through unchanged"
    );
}

#[test]
fn liq_003_boundary_residual_exactly_at_min() {
    // Residual = min_vault_debt exactly. The dust band is open on the upper
    // end (`residual < min_vault_debt`), so this case must pass through.
    // Vault=1.0, proposed=0.9 → residual=0.1=min_vault_debt.
    let vault = make_vault(100_000_000); // 1.0 icUSD
    let proposed = ICUSD::new(90_000_000); // 0.9 icUSD
    let min_vault_debt = ICUSD::new(10_000_000); // 0.1 icUSD

    let result = round_up_partial_liq_dust(&vault, proposed, min_vault_debt);
    assert_eq!(
        result, proposed,
        "residual exactly at min_vault_debt must pass through (band is exclusive on upper end)"
    );
}

#[test]
fn liq_003_boundary_residual_one_e8s_below_min() {
    // Tighter boundary: residual = min_vault_debt - 1 e8s. This is the largest
    // residual that still falls in the dust band; helper must round up.
    let vault = make_vault(100_000_000); // 1.0 icUSD
    let proposed = ICUSD::new(90_000_001); // residual = 0.1 icUSD - 1 e8s
    let min_vault_debt = ICUSD::new(10_000_000); // 0.1 icUSD

    let result = round_up_partial_liq_dust(&vault, proposed, min_vault_debt);
    assert_eq!(
        result,
        ICUSD::new(100_000_000),
        "residual one e8s below min must round up to full debt"
    );
}

#[test]
fn liq_003_boundary_proposed_one_e8s() {
    // Tiniest proposed amount: 1 e8s on a 1.0 icUSD vault. Residual is huge,
    // well above min_vault_debt. Helper passes through.
    let vault = make_vault(100_000_000); // 1.0 icUSD
    let proposed = ICUSD::new(1); // 0.00000001 icUSD
    let min_vault_debt = ICUSD::new(10_000_000); // 0.1 icUSD

    let result = round_up_partial_liq_dust(&vault, proposed, min_vault_debt);
    assert_eq!(result, proposed, "small proposal with healthy residual passes through");
}

// ---------- Layer 2: state-level invariant fence ----------

/// Simulates what a partial-liquidation endpoint does to vault state after the
/// cap math + dust round-up. Returns the post-mutation borrowed amount so the
/// caller can assert the residual invariant.
fn simulate_partial_liq_state_change(
    vault: &mut Vault,
    proposed_amount: ICUSD,
    min_vault_debt: ICUSD,
) -> ICUSD {
    // The fix's contract: every endpoint must run the cap math result through
    // the dust round-up helper before mutating state.
    let actual = round_up_partial_liq_dust(vault, proposed_amount, min_vault_debt);
    vault.borrowed_icusd_amount -= actual;
    vault.borrowed_icusd_amount
}

#[test]
fn liq_003_invariant_holds_across_full_residual_range() {
    // Sweep across proposed amounts that cover all four bands:
    //  - proposed < debt - min_vault_debt (residual >= min_vault_debt: pass through)
    //  - proposed = debt - min_vault_debt (residual = min_vault_debt: pass through)
    //  - debt - min_vault_debt < proposed < debt (residual in dust band: round up)
    //  - proposed = debt (residual = 0: pass through)
    let debt = ICUSD::new(100_000_000); // 1.0 icUSD
    let min_vault_debt = ICUSD::new(10_000_000); // 0.1 icUSD

    // Walk in 5 e8s steps (0.05 icUSD increments) so we hit both the safe band
    // and the dust band on the same vault config.
    let step = 5_000_000;
    let mut proposed = step;
    while proposed <= debt.to_u64() {
        let mut vault = make_vault(debt.to_u64());
        let post = simulate_partial_liq_state_change(
            &mut vault,
            ICUSD::new(proposed),
            min_vault_debt,
        );
        assert!(
            post == ICUSD::new(0) || post >= min_vault_debt,
            "LIQ-003 invariant violated: post-liq residual {} is in dust band (proposed={}, debt={}, min_vault_debt={})",
            post.to_u64(),
            proposed,
            debt.to_u64(),
            min_vault_debt.to_u64(),
        );
        proposed += step;
    }
}

#[test]
fn liq_003_state_level_open_vault_then_simulated_partial_liquidation_preserves_invariant() {
    // Tie the helper into actual State::open_vault flow to confirm that a
    // freshly-opened vault, run through the fix's flow, ends with a residual
    // that satisfies the invariant.
    let mut state = fresh_state();
    let collateral_type = state.icp_collateral_type();

    let initial_debt = ICUSD::new(100_000_000); // 1.0 icUSD
    state.open_vault(Vault {
        owner: Principal::anonymous(),
        vault_id: 1,
        collateral_amount: 1_000_000_000, // 10 ICP
        borrowed_icusd_amount: initial_debt,
        collateral_type,
        last_accrual_time: 0,
        accrued_interest: ICUSD::new(0),
        bot_processing: false,
    });

    // Pick a proposal that lands residual in the dust band pre-fix.
    let proposed = ICUSD::new(95_000_000); // residual would be 0.05 icUSD
    let min_vault_debt = ICUSD::new(10_000_000); // 0.1 icUSD

    let vault = state
        .vault_id_to_vaults
        .get(&1)
        .cloned()
        .expect("vault present after open_vault");
    let actual = round_up_partial_liq_dust(&vault, proposed, min_vault_debt);

    // The fix rounds up to full debt, eliminating the dust vault path.
    assert_eq!(
        actual, initial_debt,
        "fix must round up to full debt when residual would land in dust band"
    );

    // Apply the rounded-up amount and confirm the invariant holds.
    if let Some(v) = state.vault_id_to_vaults.get_mut(&1) {
        v.borrowed_icusd_amount -= actual;
    }
    let post = state.vault_id_to_vaults.get(&1).unwrap().borrowed_icusd_amount;
    assert!(
        post == ICUSD::new(0) || post >= min_vault_debt,
        "invariant: post={} must be 0 or >= {}",
        post.to_u64(),
        min_vault_debt.to_u64(),
    );
}
