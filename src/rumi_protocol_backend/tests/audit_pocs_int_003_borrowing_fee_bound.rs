//! INT-003 regression fence: bound the borrowing fee multiplier.
//!
//! Audit report:
//!   * `audit-reports/2026-04-22-28e9896/raw-pass-results/debt-interest.json`
//!     finding INT-003.
//!
//! # What the bug was
//!
//! `set_borrowing_fee_curve` validated only that each marker's `multiplier`
//! was strictly positive. Combined with the 10% cap on the per-collateral
//! base fee (`set_borrowing_fee` accepts 0..=0.10), an admin who fat-fingered
//! a curve with `multiplier = 1000` could push `fee = amount * 0.10 * 1000`
//! past `amount`. `borrow_from_vault_internal` then ran
//! `mint_icusd(amount - fee, caller)`, and `Token::Sub` panicked with
//! `underflow` (numeric.rs:187-196), trapping every borrow on the affected
//! collateral until the curve was reset.
//!
//! # How this file tests the fix
//!
//! Two layers, mirroring the two-part fix:
//!
//!  1. `int_003_validate_rejects_extreme_multiplier` — the curve validator
//!     rejects markers with multiplier > `MAX_BORROWING_FEE_MULTIPLIER`. The
//!     same validator is invoked by `set_borrowing_fee_curve` so admin
//!     mistakes never make it into state.
//!
//!  2. `int_003_runtime_fee_clamp_preserves_one_e8s` — the runtime clamp in
//!     `clamp_borrow_fee` guarantees `amount - fee >= 1 e8s` even if a curve
//!     somehow bypasses the validator (legacy state, future drift). 1 e8s is
//!     well under the per-collateral `min_icusd_amount`, so this clamp never
//!     reduces a legitimate borrow output below the protocol minimum; it only
//!     fences the panic path.

use rust_decimal_macros::dec;

use rumi_protocol_backend::numeric::{ICUSD, Ratio};
use rumi_protocol_backend::state::{
    InterpolationMethod, RateCurveV2, RateMarkerV2, CrAnchor, SystemThreshold,
    MAX_BORROWING_FEE_MULTIPLIER,
};
use rumi_protocol_backend::vault::clamp_borrow_fee;

fn marker(multiplier: Ratio) -> RateMarkerV2 {
    RateMarkerV2 {
        cr_anchor: CrAnchor::SystemThreshold(SystemThreshold::TotalCollateralRatio),
        multiplier,
    }
}

#[test]
fn int_003_validate_accepts_default_curve_multipliers() {
    // The shipped default curve uses multipliers up to 3.0 — must remain valid.
    let curve = RateCurveV2 {
        markers: vec![
            marker(Ratio::new(dec!(3.0))),
            marker(Ratio::new(dec!(1.75))),
            marker(Ratio::new(dec!(1.0))),
        ],
        method: InterpolationMethod::Linear,
    };
    curve
        .validate()
        .expect("default curve multipliers must validate");
}

#[test]
fn int_003_validate_accepts_at_max_boundary() {
    let curve = RateCurveV2 {
        markers: vec![marker(MAX_BORROWING_FEE_MULTIPLIER)],
        method: InterpolationMethod::Linear,
    };
    curve
        .validate()
        .expect("multiplier exactly at MAX_BORROWING_FEE_MULTIPLIER must validate");
}

#[test]
fn int_003_validate_rejects_extreme_multiplier() {
    let curve = RateCurveV2 {
        markers: vec![marker(Ratio::new(dec!(1000.0)))],
        method: InterpolationMethod::Linear,
    };
    let err = curve
        .validate()
        .expect_err("multiplier 1000 must be rejected");
    assert!(
        err.contains("multiplier") || err.contains("Multiplier"),
        "error must mention multiplier; got: {err}"
    );
}

#[test]
fn int_003_validate_rejects_just_above_max() {
    // Tests that the boundary is upper-inclusive of MAX, not generous.
    let just_above = Ratio::new(MAX_BORROWING_FEE_MULTIPLIER.0 + dec!(0.000001));
    let curve = RateCurveV2 {
        markers: vec![marker(just_above)],
        method: InterpolationMethod::Linear,
    };
    curve
        .validate()
        .expect_err("multiplier just above MAX must be rejected");
}

#[test]
fn int_003_validate_still_rejects_zero_or_negative() {
    let curve_zero = RateCurveV2 {
        markers: vec![marker(Ratio::new(dec!(0.0)))],
        method: InterpolationMethod::Linear,
    };
    curve_zero
        .validate()
        .expect_err("zero multiplier must still be rejected");

    let curve_neg = RateCurveV2 {
        markers: vec![marker(Ratio::new(dec!(-1.0)))],
        method: InterpolationMethod::Linear,
    };
    curve_neg
        .validate()
        .expect_err("negative multiplier must still be rejected");
}

#[test]
fn int_003_validate_rejects_empty_markers() {
    let curve = RateCurveV2 {
        markers: vec![],
        method: InterpolationMethod::Linear,
    };
    curve
        .validate()
        .expect_err("empty markers must be rejected");
}

#[test]
fn int_003_runtime_fee_clamp_preserves_one_e8s() {
    // 2 icUSD borrow with a runaway raw fee (e.g. 1000 icUSD computed before
    // any sanity check). The clamp must yield `raw_fee.min(amount - 1)` so
    // that `amount - clamped_fee == 1 e8s`, never an underflow.
    let amount = ICUSD::new(2_000_000_000); // 20 icUSD (2e9 e8s)
    let raw_fee = ICUSD::new(100_000_000_000); // 1000 icUSD — way past amount
    let clamped = clamp_borrow_fee(amount, raw_fee);
    assert!(
        clamped < amount,
        "clamped fee ({}) must be less than amount ({})",
        clamped.to_u64(),
        amount.to_u64()
    );
    assert_eq!(
        amount.saturating_sub(clamped),
        ICUSD::new(1),
        "amount - clamped_fee must be exactly 1 e8s post-clamp"
    );
}

#[test]
fn int_003_runtime_fee_clamp_passes_through_normal_fees() {
    // A 0.5% borrowing fee on 100 icUSD borrowing — well under amount.
    // The clamp must not perturb legitimate fees.
    let amount = ICUSD::new(10_000_000_000); // 100 icUSD
    let raw_fee = ICUSD::new(50_000_000); // 0.5 icUSD (0.5%)
    let clamped = clamp_borrow_fee(amount, raw_fee);
    assert_eq!(
        clamped, raw_fee,
        "clamp must pass through normal fees unchanged"
    );
}

#[test]
fn int_003_runtime_fee_clamp_handles_amount_equal_to_one() {
    // Edge case: amount = 1 e8s (would never pass min_icusd_amount in
    // practice, but the clamp must not overflow even on this minimum input).
    let amount = ICUSD::new(1);
    let raw_fee = ICUSD::new(50);
    let clamped = clamp_borrow_fee(amount, raw_fee);
    assert_eq!(
        clamped,
        ICUSD::new(0),
        "amount - 1 = 0; clamp must yield 0 fee, not panic"
    );
}
