//! LIQ-007 + ORACLE-009 regression fence: outlier-price sanity band.
//!
//! Audit reports:
//!   * `audit-reports/2026-04-22-28e9896/raw-pass-results/liquidation-mechanics.json`
//!     finding LIQ-007.
//!   * `audit-reports/2026-04-22-28e9896/dry-run-findings.md` finding ORACLE-009.
//!
//! # What the bugs were
//!
//! Pre-Wave-5, `xrc::fetch_icp_rate` and `management::fetch_collateral_price`
//! wrote any new XRC sample directly to `last_price` as long as its timestamp
//! was newer than the stored one. The only sanity check on ICP was
//! `if rate < $0.01 -> latch ReadOnly` — no rolling median, no rejection of
//! moves outside a tolerance band, no multi-sample confirmation.
//!
//! That meant:
//!   * **LIQ-007**: a single outlier from XRC (CEX glitch, momentary
//!     thin-liquidity print, wrong asset routing) immediately drove the cached
//!     price the protocol uses for liquidation/redemption decisions.
//!   * **ORACLE-009**: a single sub-$0.01 ICP sample latched the protocol into
//!     ReadOnly with no auto-recovery, even if the subsequent fetch returned a
//!     normal price.
//!
//! # How this file tests the fix
//!
//! Wave-5 adds `State::check_price_sanity_band(collateral, new_rate) -> bool`
//! plus per-collateral `pending_outlier_prices: BTreeMap<Principal, (f64, u8)>`
//! state. New samples within `[PRICE_SANITY_BAND_RATIO, 1/PRICE_SANITY_BAND_RATIO]`
//! of the stored price are accepted immediately. Outside-band samples are
//! queued; only after `PRICE_OUTLIER_CONFIRM_COUNT` consecutive samples agree
//! (each within band of the queued candidate) does the protocol accept the
//! new price. The ReadOnly latch in `fetch_icp_rate` was moved BEHIND the
//! sanity gate, so a single sub-$0.01 sample is rejected and never latches.
//!
//! These tests cover the gate at the state layer (the gate the live code
//! calls): sanity-band accept/reject, multi-sample confirmation, candidate
//! reset on divergence, and round-trip serialization of the new field.

use candid::Principal;

use rumi_protocol_backend::state::{
    PRICE_OUTLIER_CONFIRM_COUNT, PRICE_SANITY_BAND_RATIO, State,
};
use rumi_protocol_backend::InitArg;

fn icp_ledger() -> Principal {
    Principal::from_slice(&[10])
}

fn fresh_state_with_collateral() -> State {
    let mut state = State::from(InitArg {
        xrc_principal: Principal::anonymous(),
        icusd_ledger_principal: Principal::anonymous(),
        icp_ledger_principal: icp_ledger(),
        fee_e8s: 0,
        developer_principal: Principal::anonymous(),
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    });
    let icp = state.icp_collateral_type();
    if let Some(config) = state.collateral_configs.get_mut(&icp) {
        config.last_price = Some(10.0);
    }
    state
}

#[test]
fn liq_007_first_ever_price_accepted() {
    let mut state = fresh_state_with_collateral();
    let icp = state.icp_collateral_type();
    if let Some(config) = state.collateral_configs.get_mut(&icp) {
        config.last_price = None;
    }
    assert!(state.check_price_sanity_band(&icp, 8.5));
    assert!(state.pending_outlier_prices.is_empty());
}

#[test]
fn liq_007_in_band_sample_accepted_clears_candidate() {
    let mut state = fresh_state_with_collateral();
    let icp = state.icp_collateral_type();

    state.pending_outlier_prices.insert(icp, (1.0, 1));
    assert!(state.check_price_sanity_band(&icp, 9.0));
    assert!(
        !state.pending_outlier_prices.contains_key(&icp),
        "an in-band sample must clear any prior outlier candidate"
    );
}

#[test]
fn liq_007_single_outlier_rejected() {
    let mut state = fresh_state_with_collateral();
    let icp = state.icp_collateral_type();
    assert!(!state.check_price_sanity_band(&icp, 1.0));
    let entry = state
        .pending_outlier_prices
        .get(&icp)
        .expect("outlier should be queued as candidate");
    assert_eq!(entry.0, 1.0);
    assert_eq!(entry.1, 1);
}

#[test]
fn oracle_009_sub_one_cent_sample_does_not_latch_on_first_hit() {
    let mut state = fresh_state_with_collateral();
    let icp = state.icp_collateral_type();
    assert!(!state.check_price_sanity_band(&icp, 0.005));
    let entry = state
        .pending_outlier_prices
        .get(&icp)
        .expect("sub-$0.01 sample must be queued, not accepted");
    assert_eq!(entry.0, 0.005);
    assert_eq!(entry.1, 1);
    let stored = state
        .collateral_configs
        .get(&icp)
        .and_then(|c| c.last_price)
        .expect("stored price untouched by rejected sample");
    assert_eq!(stored, 10.0);
}

#[test]
fn liq_007_outlier_accepted_after_n_consecutive_confirmations() {
    let mut state = fresh_state_with_collateral();
    let icp = state.icp_collateral_type();
    for i in 1..PRICE_OUTLIER_CONFIRM_COUNT {
        assert!(
            !state.check_price_sanity_band(&icp, 1.0),
            "sample {} of {} must still be rejected",
            i,
            PRICE_OUTLIER_CONFIRM_COUNT
        );
        let entry = state.pending_outlier_prices.get(&icp).unwrap();
        assert_eq!(entry.0, 1.0);
        assert_eq!(entry.1, i);
    }
    assert!(
        state.check_price_sanity_band(&icp, 1.0),
        "Nth consistent sample must be accepted"
    );
    assert!(
        state.pending_outlier_prices.is_empty(),
        "queued candidate must be cleared after acceptance"
    );
}

#[test]
fn liq_007_outlier_candidate_resets_when_new_outlier_diverges() {
    let mut state = fresh_state_with_collateral();
    let icp = state.icp_collateral_type();

    assert!(!state.check_price_sanity_band(&icp, 1.0));
    let after_first = state.pending_outlier_prices.get(&icp).copied().unwrap();
    assert_eq!(after_first, (1.0, 1));

    assert!(!state.check_price_sanity_band(&icp, 0.1));
    let after_second = state.pending_outlier_prices.get(&icp).copied().unwrap();
    assert_eq!(
        after_second,
        (0.1, 1),
        "diverging outlier must reset the candidate, not increment the count"
    );
}

#[test]
fn liq_007_band_constants_match_documented_defaults() {
    assert!(PRICE_SANITY_BAND_RATIO > 0.0);
    assert!(PRICE_SANITY_BAND_RATIO < 1.0);
    assert!(PRICE_OUTLIER_CONFIRM_COUNT >= 2);
    assert!(PRICE_OUTLIER_CONFIRM_COUNT < u8::MAX);
}

#[test]
fn liq_007_zero_or_non_finite_samples_rejected_outright() {
    let mut state = fresh_state_with_collateral();
    let icp = state.icp_collateral_type();
    assert!(!state.check_price_sanity_band(&icp, 0.0));
    assert!(!state.check_price_sanity_band(&icp, -1.0));
    assert!(!state.check_price_sanity_band(&icp, f64::NAN));
    assert!(!state.check_price_sanity_band(&icp, f64::INFINITY));
    assert!(
        state.pending_outlier_prices.is_empty(),
        "garbage samples must NOT seed a candidate"
    );
}

#[test]
fn liq_007_state_round_trips_pending_outlier_prices_through_cbor() {
    let mut state = fresh_state_with_collateral();
    let icp = state.icp_collateral_type();
    state.pending_outlier_prices.insert(icp, (0.5, 2));
    state.liquidation_frozen = true;

    let mut buf = Vec::new();
    ciborium::ser::into_writer(&state, &mut buf).expect("encode state");
    let restored: State = ciborium::de::from_reader(buf.as_slice()).expect("decode state");

    assert!(restored.liquidation_frozen);
    let entry = restored
        .pending_outlier_prices
        .get(&icp)
        .expect("queued outlier survives round-trip");
    assert_eq!(entry.0, 0.5);
    assert_eq!(entry.1, 2);
}

/// Pre-Wave-5 snapshots have neither `pending_outlier_prices` nor
/// `liquidation_frozen`. `#[serde(default)]` must fill them with the empty
/// map and `false` so existing canister state decodes cleanly.
#[test]
fn liq_007_legacy_snapshot_decodes_with_defaults() {
    #[derive(serde::Serialize)]
    struct LegacyMinimalState {
        // Empty struct: no fields means every State field falls back to
        // its serde default. We're only exercising the new ones.
    }

    let mut buf = Vec::new();
    ciborium::ser::into_writer(&LegacyMinimalState {}, &mut buf)
        .expect("encode legacy minimal state");
    let restored: State =
        ciborium::de::from_reader(buf.as_slice()).expect("decode legacy snapshot");

    assert!(
        restored.pending_outlier_prices.is_empty(),
        "legacy snapshot must decode with an empty pending_outlier_prices map"
    );
    assert!(
        !restored.liquidation_frozen,
        "legacy snapshot must decode with liquidation_frozen=false"
    );
}
