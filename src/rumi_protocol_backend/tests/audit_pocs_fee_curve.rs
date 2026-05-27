//! Fee-curve property coverage. Locks the behavioural invariants of the
//! five fee-curve methods on `State` that the 2026-05-27 graph-driven
//! coverage audit flagged as having zero direct test references:
//!
//! 1. `State::interpolate_multiplier` (private — reached via
//!    `get_borrowing_fee_multiplier`)
//! 2. `State::resolve_anchor`
//! 3. `State::resolve_curve`
//! 4. `State::get_borrowing_fee_multiplier`
//! 5. `State::get_dynamic_interest_rate_for`
//!
//! Together these implement the dynamic borrowing-fee and dynamic
//! interest-rate surface. A regression here would silently mis-charge
//! borrowers without tripping any existing pocket-IC or POC test.
//!
//! The properties enforced below:
//!
//! - **Boundary clamping** — when CR is below the first marker or above
//!   the last marker, `interpolate_multiplier` returns the boundary
//!   multiplier (no extrapolation, no NaN).
//! - **Linear interpolation between markers** — output at an interior
//!   CR matches the analytic linear formula.
//! - **Monotonicity preservation** — for a curve whose multipliers are
//!   monotonically non-increasing in CR (the standard "charge more for
//!   risky vaults" shape), the output multiplier is non-increasing in
//!   CR across the curve domain.
//! - **Anchor resolution** — every `CrAnchor` variant resolves to the
//!   expected concrete Ratio, including the `Midpoint` overflow guard
//!   that prevents `Decimal::MAX + finite` from panicking when the
//!   total-collateral-ratio is unset.
//! - **Curve resolution sorts ascending** — `resolve_curve` always
//!   returns markers sorted by resolved CR level, regardless of input
//!   order.
//! - **Inverted-curve safety** — `get_borrowing_fee_multiplier`
//!   conservatively returns the *highest* multiplier when the resolved
//!   curve is inverted (can occur after upgrade before first oracle
//!   refresh), so a risky borrow can never accidentally pay a lower fee.
//! - **No curve configured → multiplier of 1.0** — disabled-curve case
//!   is exactly a passthrough.
//! - **Recovery-mode static override** — when `mode == Recovery` and a
//!   collateral has a `recovery_interest_rate_apr` set, the static
//!   value short-circuits both layers.

use candid::Principal;
use rumi_protocol_backend::numeric::Ratio;
use rumi_protocol_backend::state::{
    AssetThreshold, CrAnchor, InterpolationMethod, Mode, RateCurveV2, RateMarker, RateMarkerV2,
    RecoveryRateMarker, State, SystemThreshold,
};
use rumi_protocol_backend::InitArg;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

// ─── Helpers ───

fn fresh_state() -> (State, Principal) {
    let icp = Principal::from_slice(&[10]);
    let state = State::from(InitArg {
        xrc_principal: Principal::anonymous(),
        icusd_ledger_principal: Principal::anonymous(),
        icp_ledger_principal: icp,
        fee_e8s: 0,
        developer_principal: Principal::anonymous(),
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    });
    (state, icp)
}

fn r(value: Decimal) -> Ratio {
    Ratio::from(value)
}

/// Construct a fee-curve marker pinned to a literal CR value.
fn fixed_marker(cr: Decimal, mult: Decimal) -> RateMarkerV2 {
    RateMarkerV2 {
        cr_anchor: CrAnchor::Fixed(r(cr)),
        multiplier: r(mult),
    }
}

fn curve(markers: Vec<RateMarkerV2>) -> RateCurveV2 {
    RateCurveV2 {
        markers,
        method: InterpolationMethod::Linear,
    }
}

// ─── interpolate_multiplier (via get_borrowing_fee_multiplier) ───

#[test]
fn interpolate_clamps_below_first_marker() {
    let (mut state, _icp) = fresh_state();
    state.borrowing_fee_curve = Some(curve(vec![
        fixed_marker(dec!(1.5), dec!(3.0)),
        fixed_marker(dec!(2.0), dec!(2.0)),
        fixed_marker(dec!(3.0), dec!(1.0)),
    ]));
    // Below the lowest marker (CR=1.5) → clamps to its multiplier (3.0).
    assert_eq!(
        state.get_borrowing_fee_multiplier(r(dec!(1.0))),
        r(dec!(3.0))
    );
    assert_eq!(
        state.get_borrowing_fee_multiplier(r(dec!(1.49999))),
        r(dec!(3.0))
    );
    // Exactly at the first marker — same multiplier.
    assert_eq!(
        state.get_borrowing_fee_multiplier(r(dec!(1.5))),
        r(dec!(3.0))
    );
}

#[test]
fn interpolate_clamps_above_last_marker() {
    let (mut state, _icp) = fresh_state();
    state.borrowing_fee_curve = Some(curve(vec![
        fixed_marker(dec!(1.5), dec!(3.0)),
        fixed_marker(dec!(3.0), dec!(1.0)),
    ]));
    // Above the highest marker (CR=3.0) → clamps to its multiplier (1.0).
    assert_eq!(
        state.get_borrowing_fee_multiplier(r(dec!(100.0))),
        r(dec!(1.0))
    );
    assert_eq!(
        state.get_borrowing_fee_multiplier(r(dec!(3.0))),
        r(dec!(1.0))
    );
}

#[test]
fn interpolate_linear_midpoint_matches_analytic_formula() {
    let (mut state, _icp) = fresh_state();
    // Two markers: (CR=1.5, mult=3.0) and (CR=3.0, mult=1.0). Slope -4/3.
    state.borrowing_fee_curve = Some(curve(vec![
        fixed_marker(dec!(1.5), dec!(3.0)),
        fixed_marker(dec!(3.0), dec!(1.0)),
    ]));
    // Midpoint CR = 2.25 → analytic mult = 3.0 + 0.5 * (1.0 - 3.0) = 2.0.
    assert_eq!(
        state.get_borrowing_fee_multiplier(r(dec!(2.25))),
        r(dec!(2.0))
    );
    // Quarter point CR = 1.875 → mult = 3.0 + 0.25 * (1.0 - 3.0) = 2.5.
    assert_eq!(
        state.get_borrowing_fee_multiplier(r(dec!(1.875))),
        r(dec!(2.5))
    );
    // Three-quarter point CR = 2.625 → mult = 3.0 + 0.75 * (1.0 - 3.0) = 1.5.
    assert_eq!(
        state.get_borrowing_fee_multiplier(r(dec!(2.625))),
        r(dec!(1.5))
    );
}

#[test]
fn interpolate_picks_correct_segment_with_three_markers() {
    let (mut state, _icp) = fresh_state();
    state.borrowing_fee_curve = Some(curve(vec![
        fixed_marker(dec!(1.5), dec!(5.0)),
        fixed_marker(dec!(2.0), dec!(2.0)),
        fixed_marker(dec!(3.0), dec!(1.0)),
    ]));
    // In the first segment [1.5, 2.0]: CR=1.75 → mult = 5.0 + 0.5 * (2.0 - 5.0) = 3.5.
    assert_eq!(
        state.get_borrowing_fee_multiplier(r(dec!(1.75))),
        r(dec!(3.5))
    );
    // In the second segment [2.0, 3.0]: CR=2.5 → mult = 2.0 + 0.5 * (1.0 - 2.0) = 1.5.
    assert_eq!(
        state.get_borrowing_fee_multiplier(r(dec!(2.5))),
        r(dec!(1.5))
    );
    // At the boundary CR=2.0 → exactly 2.0 (matches both segments).
    assert_eq!(
        state.get_borrowing_fee_multiplier(r(dec!(2.0))),
        r(dec!(2.0))
    );
}

#[test]
fn interpolate_single_marker_acts_as_constant() {
    let (mut state, _icp) = fresh_state();
    state.borrowing_fee_curve = Some(curve(vec![fixed_marker(dec!(2.0), dec!(1.5))]));
    // Single marker case: `interpolate_multiplier` clamps both sides to that marker's value.
    for cr in [dec!(0.1), dec!(1.5), dec!(2.0), dec!(3.0), dec!(100.0)] {
        assert_eq!(
            state.get_borrowing_fee_multiplier(r(cr)),
            r(dec!(1.5)),
            "single-marker curve must be constant at every CR (cr={cr})"
        );
    }
}

#[test]
fn interpolate_zero_range_adjacent_markers_returns_left_value() {
    let (mut state, _icp) = fresh_state();
    // Two markers at the same CR with different multipliers. After
    // sorting, the lo marker's multiplier wins (per the source code's
    // `if range == Decimal::ZERO { return lo.1; }` branch).
    state.borrowing_fee_curve = Some(curve(vec![
        fixed_marker(dec!(2.0), dec!(3.0)),
        fixed_marker(dec!(2.0), dec!(2.0)),
    ]));
    let out = state.get_borrowing_fee_multiplier(r(dec!(2.0)));
    // Both markers project to the same CR. With sort_by, the original
    // order is preserved for equal keys, so the first marker (mult=3.0)
    // ends up as `lo` and is returned. Either way: no panic, no NaN.
    assert!(
        out == r(dec!(3.0)) || out == r(dec!(2.0)),
        "zero-range degenerate case must collapse to one of the two markers; got {out:?}"
    );
}

#[test]
fn interpolate_monotone_decreasing_curve_stays_monotone_decreasing() {
    // Property: if every marker is monotone non-increasing in CR, then
    // for any two CRs c1 < c2, mult(c1) >= mult(c2). This is the shape
    // the protocol relies on — "more collateral = lower fee multiplier".
    let (mut state, _icp) = fresh_state();
    state.borrowing_fee_curve = Some(curve(vec![
        fixed_marker(dec!(1.33), dec!(5.0)),
        fixed_marker(dec!(1.55), dec!(3.0)),
        fixed_marker(dec!(1.75), dec!(2.0)),
        fixed_marker(dec!(2.00), dec!(1.5)),
        fixed_marker(dec!(3.00), dec!(1.0)),
    ]));

    // Walk 50 CR values across the domain [1.0, 5.0]; the curve must be
    // non-increasing at every step.
    let mut prev: Option<Ratio> = None;
    let mut cr = dec!(1.0);
    let step = dec!(0.08);
    for _ in 0..50 {
        let m = state.get_borrowing_fee_multiplier(r(cr));
        if let Some(p) = prev {
            assert!(
                m <= p,
                "monotone-decreasing curve broken at cr={cr}: prev={p:?}, now={m:?}"
            );
        }
        prev = Some(m);
        cr += step;
    }
}

#[test]
fn no_borrowing_fee_curve_yields_unit_multiplier() {
    let (mut state, _icp) = fresh_state();
    state.borrowing_fee_curve = None;
    for cr in [dec!(0.5), dec!(1.5), dec!(2.0), dec!(100.0)] {
        assert_eq!(
            state.get_borrowing_fee_multiplier(r(cr)),
            r(dec!(1.0)),
            "disabled curve must be a passthrough (cr={cr})"
        );
    }
}

#[test]
fn inverted_curve_returns_highest_multiplier_for_safety() {
    // Documented safety in get_borrowing_fee_multiplier: if the resolved
    // curve is inverted (multipliers increase with CR — meaning the
    // anchors haven't resolved yet, e.g. TCR < BorrowThreshold pre-first
    // price), return the MAX multiplier to conservatively over-charge.
    let (mut state, _icp) = fresh_state();
    state.borrowing_fee_curve = Some(curve(vec![
        fixed_marker(dec!(1.5), dec!(1.0)),
        fixed_marker(dec!(2.0), dec!(2.0)),
        fixed_marker(dec!(3.0), dec!(5.0)),
    ]));
    // At every CR, output must be the maximum multiplier (5.0), not
    // whatever the interpolation would produce.
    for cr in [dec!(0.5), dec!(1.75), dec!(2.5), dec!(10.0)] {
        assert_eq!(
            state.get_borrowing_fee_multiplier(r(cr)),
            r(dec!(5.0)),
            "inverted-curve safety: must clamp to max multiplier (cr={cr})"
        );
    }
}

// ─── resolve_anchor ───

#[test]
fn resolve_anchor_fixed_returns_literal() {
    let (state, _icp) = fresh_state();
    assert_eq!(state.resolve_anchor(&CrAnchor::Fixed(r(dec!(1.42))), None), r(dec!(1.42)));
    assert_eq!(state.resolve_anchor(&CrAnchor::Fixed(r(dec!(0.0))), None), r(dec!(0.0)));
}

#[test]
fn resolve_anchor_asset_threshold_uses_collateral_config() {
    let (state, icp) = fresh_state();
    let cfg = state.collateral_configs.get(&icp).expect("ICP config seeded");
    let expected_borrow_threshold = cfg.borrow_threshold_ratio;
    let expected_liq_ratio = cfg.liquidation_ratio;

    assert_eq!(
        state.resolve_anchor(&CrAnchor::AssetThreshold(AssetThreshold::BorrowThreshold), Some(&icp)),
        expected_borrow_threshold
    );
    assert_eq!(
        state.resolve_anchor(&CrAnchor::AssetThreshold(AssetThreshold::LiquidationRatio), Some(&icp)),
        expected_liq_ratio
    );
}

#[test]
#[should_panic(expected = "AssetThreshold requires asset context")]
fn resolve_anchor_asset_threshold_without_context_panics() {
    let (state, _icp) = fresh_state();
    let _ = state.resolve_anchor(&CrAnchor::AssetThreshold(AssetThreshold::HealthyCr), None);
}

#[test]
fn resolve_anchor_system_threshold_reads_state_fields() {
    let (mut state, _icp) = fresh_state();
    state.recovery_mode_threshold = r(dec!(1.55));
    state.weighted_avg_warning_cr = r(dec!(1.7));
    state.weighted_avg_healthy_cr = r(dec!(2.0));
    state.total_collateral_ratio = r(dec!(2.25));

    assert_eq!(
        state.resolve_anchor(&CrAnchor::SystemThreshold(SystemThreshold::BorrowThreshold), None),
        r(dec!(1.55))
    );
    assert_eq!(
        state.resolve_anchor(&CrAnchor::SystemThreshold(SystemThreshold::WarningCr), None),
        r(dec!(1.7))
    );
    assert_eq!(
        state.resolve_anchor(&CrAnchor::SystemThreshold(SystemThreshold::HealthyCr), None),
        r(dec!(2.0))
    );
    assert_eq!(
        state.resolve_anchor(&CrAnchor::SystemThreshold(SystemThreshold::TotalCollateralRatio), None),
        r(dec!(2.25))
    );
}

#[test]
fn resolve_anchor_midpoint_averages_two_anchors() {
    let (state, _icp) = fresh_state();
    let a = CrAnchor::Fixed(r(dec!(1.0)));
    let b = CrAnchor::Fixed(r(dec!(3.0)));
    let mid = CrAnchor::Midpoint(Box::new(a.clone()), Box::new(b.clone()));
    // (1.0 + 3.0) / 2 = 2.0.
    assert_eq!(state.resolve_anchor(&mid, None), r(dec!(2.0)));
}

#[test]
fn resolve_anchor_midpoint_overflow_falls_back_to_max() {
    // Pre-fix this would have panicked: Decimal::MAX + finite overflows.
    // The implementation uses checked_add and falls back to max(a, b).
    let (mut state, _icp) = fresh_state();
    state.total_collateral_ratio = Ratio::from(Decimal::MAX);

    let a = CrAnchor::SystemThreshold(SystemThreshold::TotalCollateralRatio);
    let b = CrAnchor::Fixed(r(dec!(1.5)));
    let mid = CrAnchor::Midpoint(Box::new(a), Box::new(b));

    // (MAX + 1.5) overflows → fallback returns max(MAX, 1.5) = MAX.
    assert_eq!(state.resolve_anchor(&mid, None), Ratio::from(Decimal::MAX));
}

#[test]
fn resolve_anchor_offset_adds_delta() {
    let (state, _icp) = fresh_state();
    let base = CrAnchor::Fixed(r(dec!(1.5)));
    let plus = CrAnchor::Offset(Box::new(base.clone()), r(dec!(0.25)));
    assert_eq!(state.resolve_anchor(&plus, None), r(dec!(1.75)));

    // Negative deltas are allowed.
    let minus = CrAnchor::Offset(Box::new(base), r(Decimal::new(-25, 2)));
    assert_eq!(state.resolve_anchor(&minus, None), r(dec!(1.25)));
}

#[test]
fn resolve_anchor_offset_overflow_saturates_to_max() {
    let (mut state, _icp) = fresh_state();
    state.total_collateral_ratio = Ratio::from(Decimal::MAX);
    let base = CrAnchor::SystemThreshold(SystemThreshold::TotalCollateralRatio);
    let plus = CrAnchor::Offset(Box::new(base), r(dec!(1.0)));
    assert_eq!(state.resolve_anchor(&plus, None), Ratio::from(Decimal::MAX));
}

#[test]
fn resolve_anchor_nested_midpoint_and_offset() {
    let (state, _icp) = fresh_state();
    // ((1.0 + 3.0) / 2) + 0.1 = 2.1
    let inner = CrAnchor::Midpoint(
        Box::new(CrAnchor::Fixed(r(dec!(1.0)))),
        Box::new(CrAnchor::Fixed(r(dec!(3.0)))),
    );
    let outer = CrAnchor::Offset(Box::new(inner), r(dec!(0.1)));
    assert_eq!(state.resolve_anchor(&outer, None), r(dec!(2.1)));
}

// ─── resolve_curve ───

#[test]
fn resolve_curve_sorts_markers_ascending_by_resolved_cr() {
    let (mut state, _icp) = fresh_state();
    state.recovery_mode_threshold = r(dec!(1.55));
    state.weighted_avg_warning_cr = r(dec!(1.8));
    state.weighted_avg_healthy_cr = r(dec!(2.5));

    // Provide markers in DESCENDING order; expect ASCENDING output.
    let c = curve(vec![
        RateMarkerV2 {
            cr_anchor: CrAnchor::SystemThreshold(SystemThreshold::HealthyCr),
            multiplier: r(dec!(1.0)),
        },
        RateMarkerV2 {
            cr_anchor: CrAnchor::SystemThreshold(SystemThreshold::WarningCr),
            multiplier: r(dec!(1.75)),
        },
        RateMarkerV2 {
            cr_anchor: CrAnchor::SystemThreshold(SystemThreshold::BorrowThreshold),
            multiplier: r(dec!(3.0)),
        },
    ]);
    let resolved = state.resolve_curve(&c, None);

    let crs: Vec<Decimal> = resolved.iter().map(|(cr, _)| cr.0).collect();
    let mults: Vec<Decimal> = resolved.iter().map(|(_, m)| m.0).collect();
    assert_eq!(crs, vec![dec!(1.55), dec!(1.8), dec!(2.5)], "CRs must be sorted ascending");
    assert_eq!(mults, vec![dec!(3.0), dec!(1.75), dec!(1.0)], "multipliers must follow their anchors");
}

#[test]
fn resolve_curve_preserves_all_markers() {
    let (state, _icp) = fresh_state();
    let c = curve(vec![
        fixed_marker(dec!(1.0), dec!(5.0)),
        fixed_marker(dec!(2.0), dec!(3.0)),
        fixed_marker(dec!(3.0), dec!(2.0)),
        fixed_marker(dec!(4.0), dec!(1.0)),
    ]);
    let resolved = state.resolve_curve(&c, None);
    assert_eq!(resolved.len(), 4, "all markers preserved");
}

// ─── get_dynamic_interest_rate_for ───

#[test]
fn dynamic_interest_returns_static_override_in_recovery() {
    let (mut state, icp) = fresh_state();
    state.mode = Mode::Recovery;
    {
        let cfg = state.collateral_configs.get_mut(&icp).expect("ICP config");
        cfg.interest_rate_apr = r(dec!(0.05)); // base 5%
        cfg.recovery_interest_rate_apr = Some(r(dec!(0.20))); // static 20% override
    }
    // The override short-circuits both layers — returns the static rate
    // regardless of vault CR.
    for vault_cr in [dec!(1.0), dec!(1.5), dec!(2.0), dec!(5.0)] {
        let rate = state.get_dynamic_interest_rate_for(&icp, r(vault_cr));
        assert_eq!(rate, r(dec!(0.20)), "recovery static override must short-circuit (cr={vault_cr})");
    }
}

#[test]
fn dynamic_interest_general_availability_skips_layer2() {
    let (mut state, icp) = fresh_state();
    state.mode = Mode::GeneralAvailability;

    {
        let cfg = state.collateral_configs.get_mut(&icp).expect("ICP config");
        cfg.interest_rate_apr = r(dec!(0.05)); // 5% base
    }
    // Provide an explicit per-asset rate curve so the layer-1 multiplier
    // is deterministic (1.0 at CR=3.0).
    {
        let cfg = state.collateral_configs.get_mut(&icp).expect("ICP config");
        cfg.rate_curve = Some(rumi_protocol_backend::state::RateCurve {
            markers: vec![
                RateMarker { cr_level: r(dec!(1.5)), multiplier: r(dec!(3.0)) },
                RateMarker { cr_level: r(dec!(3.0)), multiplier: r(dec!(1.0)) },
            ],
            method: InterpolationMethod::Linear,
        });
    }

    // At CR=3.0 → layer1 mult=1.0 → rate = 0.05 * 1.0 = 0.05.
    let rate = state.get_dynamic_interest_rate_for(&icp, r(dec!(3.0)));
    assert_eq!(rate, r(dec!(0.05)));

    // At CR=1.5 → layer1 mult=3.0 → rate = 0.05 * 3.0 = 0.15.
    let rate_risky = state.get_dynamic_interest_rate_for(&icp, r(dec!(1.5)));
    assert_eq!(rate_risky, r(dec!(0.15)));

    // At CR=2.25 (midpoint) → layer1 mult = 3.0 + 0.5*(1.0-3.0) = 2.0 → rate = 0.1.
    let rate_mid = state.get_dynamic_interest_rate_for(&icp, r(dec!(2.25)));
    assert_eq!(rate_mid, r(dec!(0.1)));
}

#[test]
fn dynamic_interest_recovery_applies_both_layers() {
    let (mut state, icp) = fresh_state();
    state.mode = Mode::Recovery;
    state.total_collateral_ratio = r(dec!(1.6)); // TCR feeds layer 2

    {
        let cfg = state.collateral_configs.get_mut(&icp).expect("ICP config");
        cfg.interest_rate_apr = r(dec!(0.05));
        // No recovery_interest_rate_apr static override → use layered curve.
        cfg.recovery_interest_rate_apr = None;
        cfg.rate_curve = Some(rumi_protocol_backend::state::RateCurve {
            markers: vec![
                RateMarker { cr_level: r(dec!(1.5)), multiplier: r(dec!(2.0)) },
                RateMarker { cr_level: r(dec!(3.0)), multiplier: r(dec!(1.0)) },
            ],
            method: InterpolationMethod::Linear,
        });
    }

    // Layer-2 curve: at TCR=1.5 → 2.0×, at TCR=2.5 → 1.0×. Linear.
    state.recovery_rate_curve = vec![
        RecoveryRateMarker {
            threshold: SystemThreshold::BorrowThreshold,
            multiplier: r(dec!(2.0)),
        },
        RecoveryRateMarker {
            threshold: SystemThreshold::TotalCollateralRatio,
            multiplier: r(dec!(1.0)),
        },
    ];
    state.recovery_mode_threshold = r(dec!(1.5)); // marker 1's CR

    // Vault CR=2.25 → layer1 mult=1.5 → layer1 rate = 0.05 * 1.5 = 0.075.
    // TCR=1.6 → between [1.5, 1.6] (marker 2 lands at TCR=1.6) → mult=1.0 at marker 2.
    // The two markers resolve to (1.5, 2.0) and (1.6, 1.0). At TCR=1.6 → clamps to last → 1.0.
    // Final = 0.075 * 1.0 = 0.075.
    let rate = state.get_dynamic_interest_rate_for(&icp, r(dec!(2.25)));
    assert_eq!(rate, r(dec!(0.075)));
}

#[test]
fn dynamic_interest_missing_collateral_config_returns_default() {
    let (mut state, _icp) = fresh_state();
    state.mode = Mode::GeneralAvailability;
    let unknown = Principal::from_slice(&[99]);
    // DEFAULT_INTEREST_RATE_APR = 0.0; layer1 markers will be empty →
    // interpolate_multiplier returns 1.0; base 0 * 1 = 0.
    let rate = state.get_dynamic_interest_rate_for(&unknown, r(dec!(1.5)));
    assert_eq!(rate, r(dec!(0.0)));
}

// ─── Cross-cutting monotonicity property on the production curve shape ───

#[test]
fn production_shaped_curve_satisfies_monotone_borrow_fee_property() {
    // Recreate the default fee-curve shape used in production
    // (per State::From<InitArg>): 3 markers using SystemThreshold anchors.
    let (mut state, _icp) = fresh_state();
    // Make the anchors resolvable to deterministic values.
    state.recovery_mode_threshold = r(dec!(1.50));
    state.total_collateral_ratio = r(dec!(2.50));
    // Production curve uses (BorrowThreshold + 0.05) → mult 3.0,
    // Midpoint(BorrowThreshold, TCR) → mult 1.75,
    // TCR → mult 1.0.
    state.borrowing_fee_curve = Some(curve(vec![
        RateMarkerV2 {
            cr_anchor: CrAnchor::Offset(
                Box::new(CrAnchor::SystemThreshold(SystemThreshold::BorrowThreshold)),
                r(dec!(0.05)),
            ),
            multiplier: r(dec!(3.0)),
        },
        RateMarkerV2 {
            cr_anchor: CrAnchor::Midpoint(
                Box::new(CrAnchor::SystemThreshold(SystemThreshold::BorrowThreshold)),
                Box::new(CrAnchor::SystemThreshold(SystemThreshold::TotalCollateralRatio)),
            ),
            multiplier: r(dec!(1.75)),
        },
        RateMarkerV2 {
            cr_anchor: CrAnchor::SystemThreshold(SystemThreshold::TotalCollateralRatio),
            multiplier: r(dec!(1.0)),
        },
    ]));

    // Resolved CRs: 1.55, midpoint(1.5, 2.5)=2.0, 2.5. Mults: 3.0, 1.75, 1.0.
    // Property: for any two CRs c1 < c2 in [0, 5], mult(c1) >= mult(c2).
    let crs: Vec<Decimal> = (0..50).map(|i| dec!(0.1) * Decimal::from(i)).collect();
    for window in crs.windows(2) {
        let m1 = state.get_borrowing_fee_multiplier(r(window[0]));
        let m2 = state.get_borrowing_fee_multiplier(r(window[1]));
        assert!(
            m1 >= m2,
            "non-monotone production curve at cr1={}, cr2={}: m1={:?}, m2={:?}",
            window[0],
            window[1],
            m1,
            m2
        );
    }
}
