//! Wave-14a follow-up: `LstWrapped` collateral price must derive from the
//! cached underlying-asset rate (e.g. ICP/USD from Timer A) rather than
//! issuing its own redundant XRC call.
//!
//! Pre-fix, `management::fetch_collateral_price` made an independent
//! `get_exchange_rate` call to XRC for the LST's `base_asset` even though
//! Timer A (`xrc::fetch_icp_rate`) had just cached the very same rate.
//! On mainnet this duplicated every ICP/USD fetch (~1B cycles per call,
//! ~6B cycles/day extra) and also doubled every CDP-14 source-count
//! rejection event (one for ICP collateral, a paired one for nICP).
//!
//! The pure helper `management::compute_lst_wrapped_price` enforces the
//! contract: given an already-fetched underlying rate, the WaterNeuron-
//! style canister's `exchange_rate`, and the configured haircut, produce
//! the final nICP/USD price (or `None` if any input would yield a
//! non-positive or non-finite result).

use rumi_protocol_backend::management::compute_lst_wrapped_price;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal_macros::dec;

/// E8S scale used by WaterNeuron's `exchange_rate` field. Repeating the
/// constant locally keeps the test independent of backend internals.
const E8S: u64 = 100_000_000;

#[test]
fn nicp_price_matches_observed_mainnet_computation() {
    // Reconstructs the LstWrapped log line observed on mainnet:
    //   "LstWrapped final: 3.8778636004174468138893800984
    //    (underlying=3.261682912, multiplier=1.2514894288565178633220861978,
    //     haircut=0.05)"
    //
    // multiplier = E8S / wn_exchange_rate  =>
    // wn_exchange_rate = E8S / multiplier  ≈ 79_904_804 (truncated)
    let underlying = dec!(3.261682912);
    let wn_exchange_rate: u64 = 79_904_804; // ~0.799 ICP per nICP, the mainnet value
    let haircut = 0.05_f64;

    let got = compute_lst_wrapped_price(underlying, wn_exchange_rate, haircut)
        .expect("happy-path inputs must produce a price");

    // Compare on `f64` because the underlying multiplier is irrational
    // in decimal. The conversion must succeed — a None here would mean
    // the helper produced a non-representable value, which is itself a bug.
    let got_f64 = got
        .to_f64()
        .expect("helper output must be representable as f64");
    let expected = 3.877863600_f64;
    assert!(
        (got_f64 - expected).abs() < 1e-5,
        "expected ≈{}, got {}",
        expected,
        got_f64
    );
}

#[test]
fn zero_wn_exchange_rate_returns_none() {
    // WaterNeuron unhealthy / not yet initialized: must NOT publish a price.
    let got = compute_lst_wrapped_price(dec!(3.26), 0, 0.05);
    assert!(
        got.is_none(),
        "exchange_rate of 0 indicates the LST canister is unhealthy; \
         must return None so the cached price stays in place",
    );
}

#[test]
fn negative_haircut_returns_none() {
    // A negative haircut would inflate the price above the underlying-derived
    // value, which only ever makes sense as a misconfiguration. Refuse.
    let got = compute_lst_wrapped_price(dec!(3.26), 80_000_000, -0.05);
    assert!(
        got.is_none(),
        "negative haircut must be refused as a misconfiguration",
    );
}

#[test]
fn haircut_at_or_above_one_returns_none() {
    // Haircut of 1.0 or higher collapses the price to zero/negative.
    assert!(
        compute_lst_wrapped_price(dec!(3.26), 80_000_000, 1.0).is_none(),
        "haircut = 1.0 produces zero price; must return None",
    );
    assert!(
        compute_lst_wrapped_price(dec!(3.26), 80_000_000, 1.5).is_none(),
        "haircut > 1.0 produces negative price; must return None",
    );
}

#[test]
fn nan_or_infinite_haircut_returns_none() {
    // Pre-fix behavior used `Decimal::from_f64(haircut).unwrap_or(ZERO)`,
    // which silently treated `NaN` and `Infinity` as a 0% haircut and
    // published an inflated price. The new helper refuses both. (The
    // admin endpoint `set_lst_haircut` uses `<` / `>` validation that
    // returns false for `NaN`, so a NaN haircut COULD be stored in
    // state; the helper is the last line of defense.)
    assert!(
        compute_lst_wrapped_price(dec!(3.26), 80_000_000, f64::NAN).is_none(),
        "NaN haircut must be refused, not silently treated as zero",
    );
    assert!(
        compute_lst_wrapped_price(dec!(3.26), 80_000_000, f64::INFINITY).is_none(),
        "+infinity haircut must be refused",
    );
    assert!(
        compute_lst_wrapped_price(dec!(3.26), 80_000_000, f64::NEG_INFINITY).is_none(),
        "-infinity haircut must be refused",
    );
}

#[test]
fn zero_or_negative_underlying_returns_none() {
    // No price means no LST price. A non-positive underlying must not
    // propagate into a non-positive LST price.
    assert!(
        compute_lst_wrapped_price(dec!(0), 80_000_000, 0.05).is_none(),
        "zero underlying rate must return None",
    );
    assert!(
        compute_lst_wrapped_price(dec!(-1), 80_000_000, 0.05).is_none(),
        "negative underlying rate must return None",
    );
}

#[test]
fn zero_haircut_just_applies_multiplier() {
    // multiplier = E8S / E8S = 1.0 → price equals underlying exactly.
    let got = compute_lst_wrapped_price(dec!(3.26), E8S, 0.0)
        .expect("zero-haircut, unit-multiplier inputs must succeed");
    assert_eq!(got, dec!(3.26));
}

#[test]
fn pure_helper_does_not_depend_on_state_or_async() {
    // Documenting the contract: this is a pure helper. The test runs in a
    // synchronous context with no canister state, no XRC mock, no LST
    // canister mock. If a future change reintroduces a hidden dependency
    // (e.g. an inter-canister call inside the helper), this file fails to
    // compile as an integration test — which is the point of the fence.
    let _ = compute_lst_wrapped_price(dec!(1), E8S, 0.0);
}
