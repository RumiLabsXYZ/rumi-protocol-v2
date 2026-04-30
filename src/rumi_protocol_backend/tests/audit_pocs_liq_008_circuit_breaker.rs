//! Wave-10 LIQ-008: mass-liquidation circuit breaker — Layer 1+2 unit tests.
//!
//! Layer 1: state-model invariants, CBOR round-trip, default values, append-
//! and-prune behavior, ceiling-cross trip behavior, admin-clear semantics.
//! Layer 2: edge cases (window=0 disables, ceiling=0 disables, single big
//! liquidation, prune past window boundary).
//!
//! No canister, no async, no PocketIC. PocketIC fences live in
//! `audit_pocs_liq_008_circuit_breaker_pic.rs`.

use rumi_protocol_backend::state::{record_recent_liquidation, State};

const NS_PER_SEC: u64 = 1_000_000_000;
const DEFAULT_WINDOW_NS: u64 = 30 * 60 * NS_PER_SEC;

#[test]
fn liq_008_state_defaults_disabled_breaker() {
    let s = State::default();
    assert!(s.recent_liquidations.is_empty());
    assert_eq!(s.breaker_window_ns, DEFAULT_WINDOW_NS);
    assert_eq!(s.breaker_window_debt_ceiling_e8s, 0);
    assert!(!s.liquidation_breaker_tripped);
    assert_eq!(s.windowed_liquidation_total(0), 0);
}

#[test]
fn liq_008_state_round_trip_preserves_breaker_fields() {
    let mut s = State::default();
    s.recent_liquidations = vec![(1_000_000, 5_000), (2_000_000, 7_500)];
    s.breaker_window_ns = 60 * NS_PER_SEC;
    s.breaker_window_debt_ceiling_e8s = 1_000_000_000;
    s.liquidation_breaker_tripped = true;

    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&s, &mut bytes).expect("encode");
    let decoded: State = ciborium::de::from_reader(bytes.as_slice()).expect("decode");

    assert_eq!(decoded.recent_liquidations, s.recent_liquidations);
    assert_eq!(decoded.breaker_window_ns, 60 * NS_PER_SEC);
    assert_eq!(decoded.breaker_window_debt_ceiling_e8s, 1_000_000_000);
    assert!(decoded.liquidation_breaker_tripped);
}

#[test]
fn liq_008_state_decodes_pre_wave_10_blob_with_defaults() {
    // Encode a current State, then drop the four LIQ-008 keys via a CBOR
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
                    "recent_liquidations"
                        | "breaker_window_ns"
                        | "breaker_window_debt_ceiling_e8s"
                        | "liquidation_breaker_tripped"
                ),
                _ => true,
            });
            assert_eq!(
                entries.len(),
                original_len - 4,
                "expected to strip exactly the four LIQ-008 fields"
            );
            ciborium::Value::Map(entries)
        }
        _ => panic!("expected CBOR map for State"),
    };

    let mut stripped = Vec::new();
    ciborium::ser::into_writer(&stripped_value, &mut stripped).expect("encode stripped");
    let decoded: State =
        ciborium::de::from_reader(stripped.as_slice()).expect("decode old-shape");

    assert!(decoded.recent_liquidations.is_empty());
    assert_eq!(decoded.breaker_window_ns, DEFAULT_WINDOW_NS);
    assert_eq!(decoded.breaker_window_debt_ceiling_e8s, 0);
    assert!(!decoded.liquidation_breaker_tripped);
}

#[test]
fn liq_008_record_appends_and_prunes_window() {
    let mut s = State::default();
    s.breaker_window_ns = 60 * NS_PER_SEC;

    record_recent_liquidation(&mut s, 1_000, 100 * NS_PER_SEC);
    record_recent_liquidation(&mut s, 2_000, 110 * NS_PER_SEC);
    record_recent_liquidation(&mut s, 3_000, 120 * NS_PER_SEC);
    assert_eq!(s.recent_liquidations.len(), 3);
    assert_eq!(s.windowed_liquidation_total(120 * NS_PER_SEC), 6_000);

    // Advance past the window and add another entry — earlier ones evicted
    // in-place by the new write's prune step.
    record_recent_liquidation(&mut s, 4_000, 200 * NS_PER_SEC);
    assert_eq!(s.recent_liquidations.len(), 1);
    assert_eq!(s.recent_liquidations[0].1, 4_000);
    assert_eq!(s.windowed_liquidation_total(200 * NS_PER_SEC), 4_000);
}

#[test]
fn liq_008_record_skips_when_window_zero() {
    let mut s = State::default();
    s.breaker_window_ns = 0;

    record_recent_liquidation(&mut s, 5_000, 100 * NS_PER_SEC);
    assert!(s.recent_liquidations.is_empty());
    assert!(!s.liquidation_breaker_tripped);
}

#[test]
fn liq_008_record_skips_when_debt_zero() {
    let mut s = State::default();
    s.breaker_window_ns = 60 * NS_PER_SEC;

    record_recent_liquidation(&mut s, 0, 100 * NS_PER_SEC);
    assert!(s.recent_liquidations.is_empty());
}

#[test]
fn liq_008_breaker_does_not_trip_when_ceiling_zero() {
    let mut s = State::default();
    s.breaker_window_ns = 60 * NS_PER_SEC;
    s.breaker_window_debt_ceiling_e8s = 0;

    record_recent_liquidation(&mut s, u64::MAX / 2, 100 * NS_PER_SEC);
    assert!(!s.liquidation_breaker_tripped);
}

#[test]
fn liq_008_breaker_trips_at_ceiling() {
    let mut s = State::default();
    s.breaker_window_ns = 60 * NS_PER_SEC;
    s.breaker_window_debt_ceiling_e8s = 1_000;

    record_recent_liquidation(&mut s, 600, 100 * NS_PER_SEC);
    assert!(!s.liquidation_breaker_tripped);

    record_recent_liquidation(&mut s, 500, 110 * NS_PER_SEC);
    assert!(s.liquidation_breaker_tripped);
    assert_eq!(s.windowed_liquidation_total(110 * NS_PER_SEC), 1_100);
}

#[test]
fn liq_008_single_huge_liquidation_trips_immediately() {
    let mut s = State::default();
    s.breaker_window_ns = 60 * NS_PER_SEC;
    s.breaker_window_debt_ceiling_e8s = 1_000;

    record_recent_liquidation(&mut s, 5_000, 100 * NS_PER_SEC);
    assert!(s.liquidation_breaker_tripped);
}

#[test]
fn liq_008_breaker_stays_tripped_after_window_rolls() {
    let mut s = State::default();
    s.breaker_window_ns = 60 * NS_PER_SEC;
    s.breaker_window_debt_ceiling_e8s = 1_000;

    record_recent_liquidation(&mut s, 1_500, 100 * NS_PER_SEC);
    assert!(s.liquidation_breaker_tripped);

    // Advance time past the window so windowed_liquidation_total drops to 0,
    // but the tripped flag stays — admin clear is required (T2 semantics).
    assert_eq!(s.windowed_liquidation_total(200 * NS_PER_SEC), 0);
    assert!(s.liquidation_breaker_tripped);
}

#[test]
fn liq_008_clear_breaker_resets_latch_but_preserves_log() {
    let mut s = State::default();
    s.breaker_window_ns = 60 * NS_PER_SEC;
    s.breaker_window_debt_ceiling_e8s = 1_000;

    record_recent_liquidation(&mut s, 1_500, 100 * NS_PER_SEC);
    assert!(s.liquidation_breaker_tripped);
    assert_eq!(s.recent_liquidations.len(), 1);

    s.liquidation_breaker_tripped = false;
    assert_eq!(s.recent_liquidations.len(), 1);
    assert_eq!(s.windowed_liquidation_total(100 * NS_PER_SEC), 1_500);
}

#[test]
fn liq_008_window_one_ns_evicts_immediately() {
    let mut s = State::default();
    s.breaker_window_ns = 1;
    s.breaker_window_debt_ceiling_e8s = 1_000;

    record_recent_liquidation(&mut s, 500, 100);
    record_recent_liquidation(&mut s, 500, 200);
    record_recent_liquidation(&mut s, 500, 300);
    assert_eq!(s.recent_liquidations.len(), 1);
    assert_eq!(s.recent_liquidations[0], (300, 500));
    assert!(!s.liquidation_breaker_tripped);
}

#[test]
fn liq_008_windowed_total_filters_without_mutation() {
    let mut s = State::default();
    s.breaker_window_ns = 60 * NS_PER_SEC;

    record_recent_liquidation(&mut s, 100, 100 * NS_PER_SEC);
    record_recent_liquidation(&mut s, 200, 110 * NS_PER_SEC);
    let len_before = s.recent_liquidations.len();

    let total_now = s.windowed_liquidation_total(120 * NS_PER_SEC);
    assert_eq!(total_now, 300);
    assert_eq!(s.recent_liquidations.len(), len_before);

    // Reading from a future timestamp filters older entries out of the sum
    // without removing them.
    let total_future = s.windowed_liquidation_total(200 * NS_PER_SEC);
    assert_eq!(total_future, 0);
    assert_eq!(s.recent_liquidations.len(), len_before);
}
