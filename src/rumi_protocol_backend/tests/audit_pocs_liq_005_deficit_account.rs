//! Wave-8e LIQ-005: bad-debt deficit account — Layer 1+2 unit tests.
//!
//! Layer 1: state-model invariants, CBOR round-trip, default values, predicate
//! arithmetic for deficit accrual and repayment.
//! Layer 2: deterministic decimal math across edge cases (fee = 0, fraction = 0,
//! fraction = 1, deficit = 0, repay capped at remaining deficit).
//!
//! No canister, no async, no PocketIC. PocketIC fences live in
//! `audit_pocs_liq_005_deficit_account_pic.rs`.

use rumi_protocol_backend::numeric::{ICUSD, Ratio};
use rumi_protocol_backend::state::{Mode, State};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

#[test]
fn liq_005_state_defaults_zero_deficit_and_half_fraction() {
    let s = State::default();
    assert_eq!(s.protocol_deficit_icusd, ICUSD::new(0));
    assert_eq!(s.total_deficit_repaid_icusd, ICUSD::new(0));
    assert_eq!(s.deficit_repayment_fraction.0, dec!(0.5));
    assert_eq!(s.deficit_readonly_threshold_e8s, 0);
}

#[test]
fn liq_005_state_round_trip_preserves_all_four_fields() {
    let mut s = State::default();
    s.protocol_deficit_icusd = ICUSD::new(123_456_789);
    s.total_deficit_repaid_icusd = ICUSD::new(987_654_321);
    s.deficit_repayment_fraction = Ratio::from(dec!(0.75));
    s.deficit_readonly_threshold_e8s = 1_000_000_000;

    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&s, &mut bytes).expect("encode");
    let decoded: State = ciborium::de::from_reader(bytes.as_slice()).expect("decode");

    assert_eq!(decoded.protocol_deficit_icusd, s.protocol_deficit_icusd);
    assert_eq!(decoded.total_deficit_repaid_icusd, s.total_deficit_repaid_icusd);
    assert_eq!(decoded.deficit_repayment_fraction.0, dec!(0.75));
    assert_eq!(decoded.deficit_readonly_threshold_e8s, 1_000_000_000);
}

#[test]
fn liq_005_state_decodes_pre_8e_blob_with_defaults() {
    // Encode a current State, then drop the four LIQ-005 keys via a CBOR
    // Value round-trip and re-encode. Tests that `serde(default)` on every
    // new field decodes to the documented default — the upgrade-safety fence.
    let s_full = State::default();
    let mut full_bytes = Vec::new();
    ciborium::ser::into_writer(&s_full, &mut full_bytes).expect("encode full");

    let value: ciborium::Value = ciborium::de::from_reader(full_bytes.as_slice())
        .expect("decode to value");
    let stripped_value = match value {
        ciborium::Value::Map(mut entries) => {
            let original_len = entries.len();
            entries.retain(|(k, _)| match k {
                ciborium::Value::Text(t) => !matches!(
                    t.as_str(),
                    "protocol_deficit_icusd"
                        | "total_deficit_repaid_icusd"
                        | "deficit_repayment_fraction"
                        | "deficit_readonly_threshold_e8s"
                ),
                _ => true,
            });
            assert_eq!(
                entries.len(),
                original_len - 4,
                "expected to strip exactly the four LIQ-005 fields"
            );
            ciborium::Value::Map(entries)
        }
        _ => panic!("expected CBOR map for State"),
    };

    let mut stripped = Vec::new();
    ciborium::ser::into_writer(&stripped_value, &mut stripped).expect("encode stripped");
    let decoded: State =
        ciborium::de::from_reader(stripped.as_slice()).expect("decode old-shape");

    assert_eq!(decoded.protocol_deficit_icusd, ICUSD::new(0));
    assert_eq!(decoded.total_deficit_repaid_icusd, ICUSD::new(0));
    assert_eq!(decoded.deficit_repayment_fraction.0, dec!(0.5));
    assert_eq!(decoded.deficit_readonly_threshold_e8s, 0);
}

// ─── Task 2: state-helper fences ───

#[test]
fn liq_005_accrue_shortfall_increments_deficit() {
    let mut s = State::default();
    let added = s.accrue_deficit_shortfall(ICUSD::new(500));
    assert_eq!(added, ICUSD::new(500));
    assert_eq!(s.protocol_deficit_icusd, ICUSD::new(500));
    assert_eq!(s.total_deficit_repaid_icusd, ICUSD::new(0));
}

#[test]
fn liq_005_accrue_zero_shortfall_is_noop() {
    let mut s = State::default();
    let added = s.accrue_deficit_shortfall(ICUSD::new(0));
    assert_eq!(added, ICUSD::new(0));
    assert_eq!(s.protocol_deficit_icusd, ICUSD::new(0));
}

#[test]
fn liq_005_compute_repay_amount_zero_when_deficit_zero() {
    let s = State::default();
    let repay = s.compute_deficit_repay_amount(ICUSD::new(1_000_000));
    assert_eq!(repay, ICUSD::new(0));
}

#[test]
fn liq_005_compute_repay_amount_caps_at_remaining_deficit() {
    let mut s = State::default();
    s.protocol_deficit_icusd = ICUSD::new(500);
    // 50% of 10_000 = 5_000, but deficit is only 500.
    let repay = s.compute_deficit_repay_amount(ICUSD::new(10_000));
    assert_eq!(repay, ICUSD::new(500));
}

#[test]
fn liq_005_compute_repay_amount_uses_fraction() {
    let mut s = State::default();
    s.protocol_deficit_icusd = ICUSD::new(1_000_000_000_000);
    s.deficit_repayment_fraction = Ratio::from(dec!(0.25));
    let repay = s.compute_deficit_repay_amount(ICUSD::new(1_000_000));
    assert_eq!(repay, ICUSD::new(250_000));
}

#[test]
fn liq_005_compute_repay_amount_zero_when_fraction_zero() {
    let mut s = State::default();
    s.protocol_deficit_icusd = ICUSD::new(10_000_000);
    s.deficit_repayment_fraction = Ratio::from(Decimal::ZERO);
    let repay = s.compute_deficit_repay_amount(ICUSD::new(1_000_000));
    assert_eq!(repay, ICUSD::new(0));
}

#[test]
fn liq_005_compute_repay_amount_full_fee_when_fraction_one() {
    let mut s = State::default();
    s.protocol_deficit_icusd = ICUSD::new(1_000_000_000);
    s.deficit_repayment_fraction = Ratio::from(dec!(1.0));
    let repay = s.compute_deficit_repay_amount(ICUSD::new(1_000_000));
    assert_eq!(repay, ICUSD::new(1_000_000));
}

#[test]
fn liq_005_apply_repayment_decrements_and_increments_counters() {
    let mut s = State::default();
    s.protocol_deficit_icusd = ICUSD::new(800);
    s.apply_deficit_repayment(ICUSD::new(300));
    assert_eq!(s.protocol_deficit_icusd, ICUSD::new(500));
    assert_eq!(s.total_deficit_repaid_icusd, ICUSD::new(300));
}

#[test]
fn liq_005_apply_repayment_saturates_to_zero() {
    let mut s = State::default();
    s.protocol_deficit_icusd = ICUSD::new(100);
    // Caller asks for 500 but only 100 outstanding — saturate to zero.
    s.apply_deficit_repayment(ICUSD::new(500));
    assert_eq!(s.protocol_deficit_icusd, ICUSD::new(0));
    // total_deficit_repaid still records the requested amount so the
    // event-log invariant remains:
    //   sum(DeficitRepaid.amount) - sum(DeficitAccrued.amount) >= -deficit
    assert_eq!(s.total_deficit_repaid_icusd, ICUSD::new(500));
}

#[test]
fn liq_005_check_readonly_latch_disabled_when_threshold_zero() {
    let mut s = State::default();
    s.protocol_deficit_icusd = ICUSD::new(u64::MAX);
    let latched = s.check_deficit_readonly_latch();
    assert!(!latched, "threshold=0 must disable the latch");
    assert_ne!(s.mode, Mode::ReadOnly);
}

#[test]
fn liq_005_check_readonly_latch_fires_at_threshold() {
    let mut s = State::default();
    s.deficit_readonly_threshold_e8s = 1_000;
    s.protocol_deficit_icusd = ICUSD::new(1_000);
    let latched = s.check_deficit_readonly_latch();
    assert!(latched, "deficit at threshold must latch");
    assert_eq!(s.mode, Mode::ReadOnly);
}

#[test]
fn liq_005_check_readonly_latch_does_not_fire_below_threshold() {
    let mut s = State::default();
    s.deficit_readonly_threshold_e8s = 1_000;
    s.protocol_deficit_icusd = ICUSD::new(999);
    let latched = s.check_deficit_readonly_latch();
    assert!(!latched, "deficit below threshold must not latch");
    assert_ne!(s.mode, Mode::ReadOnly);
}
