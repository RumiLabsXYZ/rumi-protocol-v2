//! CDP-01 regression fence: a sustained XRC outage between the 300-second
//! poll cadence and the 10-minute hard-staleness gate must trip the
//! protocol into ReadOnly via a consecutive-failure counter, and the
//! ReadOnly mode must auto-clear when the oracle recovers (but only when
//! it was the oracle that triggered ReadOnly in the first place; an
//! operator-set ReadOnly stays put).
//!
//! Audit fence per `.claude/security-docs/2026-05-02-wave-14-avai-parity-plan.md`.
//!
//! Layered fences:
//!  1. Default-on: a fresh State has counter 0 and is not oracle-tripped.
//!  2. N consecutive failures (N == MAX_CONSECUTIVE_XRC_FAILURES) flips
//!     mode to ReadOnly and emits `Event::OracleCircuitBreaker`.
//!  3. A subsequent success resets the counter and clears ReadOnly only
//!     if the trip was oracle-triggered.
//!  4. Operator-set ReadOnly is NOT cleared by oracle recovery.
//!  5. CBOR backwards-compat: pre-Wave-14 snapshots decode with counter 0
//!     and `mode_triggered_by_oracle: false` (serde defaults).

use candid::Principal;

use rumi_protocol_backend::event::Event;
use rumi_protocol_backend::state::{Mode, State};
use rumi_protocol_backend::xrc::{
    note_xrc_failure_at, note_xrc_success, MAX_CONSECUTIVE_XRC_FAILURES,
};
use rumi_protocol_backend::InitArg;

const TEST_NOW_NS: u64 = 1_700_000_000_000_000_000;

fn note_xrc_failure(state: &mut rumi_protocol_backend::state::State) -> Option<Event> {
    note_xrc_failure_at(state, TEST_NOW_NS)
}

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
fn cdp_01_default_state_has_no_failures() {
    let state = fresh_state();
    assert_eq!(state.consecutive_xrc_failures, 0);
    assert!(!state.mode_triggered_by_oracle);
    assert_eq!(state.mode, Mode::GeneralAvailability);
}

#[test]
fn cdp_01_threshold_is_at_least_two() {
    // A threshold of 1 would mean any single Err trips ReadOnly, which
    // is too aggressive given normal XRC transient errors. The plan
    // recommends 3.
    assert!(
        MAX_CONSECUTIVE_XRC_FAILURES >= 2,
        "threshold must allow at least one transient failure without tripping",
    );
}

#[test]
fn cdp_01_reaching_threshold_trips_readonly() {
    let mut state = fresh_state();

    let mut last_event = None;
    for _ in 0..MAX_CONSECUTIVE_XRC_FAILURES {
        last_event = note_xrc_failure(&mut state);
    }

    assert_eq!(state.consecutive_xrc_failures, MAX_CONSECUTIVE_XRC_FAILURES);
    assert_eq!(state.mode, Mode::ReadOnly);
    assert!(state.mode_triggered_by_oracle);

    let Some(Event::OracleCircuitBreaker {
        consecutive_failures,
        timestamp: _,
    }) = last_event
    else {
        panic!(
            "expected OracleCircuitBreaker on the trip-tick, got {:?}",
            last_event
        );
    };
    assert_eq!(consecutive_failures, MAX_CONSECUTIVE_XRC_FAILURES);
}

#[test]
fn cdp_01_subthreshold_failures_do_not_trip() {
    let mut state = fresh_state();

    for _ in 0..(MAX_CONSECUTIVE_XRC_FAILURES - 1) {
        let event = note_xrc_failure(&mut state);
        assert!(
            event.is_none(),
            "no event should be emitted before the threshold; got {:?}",
            event
        );
    }
    assert_eq!(state.mode, Mode::GeneralAvailability);
    assert!(!state.mode_triggered_by_oracle);
}

#[test]
fn cdp_01_success_resets_counter_and_clears_oracle_readonly() {
    let mut state = fresh_state();

    // Trip ReadOnly via the oracle path.
    for _ in 0..MAX_CONSECUTIVE_XRC_FAILURES {
        note_xrc_failure(&mut state);
    }
    assert_eq!(state.mode, Mode::ReadOnly);
    assert!(state.mode_triggered_by_oracle);

    // Recovery tick: counter resets, ReadOnly clears (oracle-triggered).
    note_xrc_success(&mut state);

    assert_eq!(state.consecutive_xrc_failures, 0);
    assert_eq!(state.mode, Mode::GeneralAvailability);
    assert!(!state.mode_triggered_by_oracle);
}

#[test]
fn cdp_01_operator_set_readonly_does_not_auto_clear() {
    let mut state = fresh_state();

    // Operator manually sets ReadOnly. mode_triggered_by_oracle stays false.
    state.mode = Mode::ReadOnly;
    assert!(!state.mode_triggered_by_oracle);

    // XRC recovers (or never failed). The success path must NOT clear
    // operator-set ReadOnly — only the oracle-triggered case auto-recovers.
    note_xrc_success(&mut state);

    assert_eq!(
        state.mode,
        Mode::ReadOnly,
        "operator-set ReadOnly must persist after oracle success",
    );
}

#[test]
fn cdp_01_failure_after_recovery_starts_counter_fresh() {
    let mut state = fresh_state();

    // Two failures, then a success, then two failures: counter must
    // restart from 1, not pick up at 3.
    note_xrc_failure(&mut state);
    note_xrc_failure(&mut state);
    note_xrc_success(&mut state);
    note_xrc_failure(&mut state);
    note_xrc_failure(&mut state);

    assert_eq!(state.consecutive_xrc_failures, 2);
    assert_eq!(state.mode, Mode::GeneralAvailability);
}
