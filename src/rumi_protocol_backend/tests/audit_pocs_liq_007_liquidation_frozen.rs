//! LIQ-007 regression fence: `liquidation_frozen` admin gate.
//!
//! Audit report:
//!   `audit-reports/2026-04-22-28e9896/raw-pass-results/liquidation-mechanics.json`
//!   finding LIQ-007 (second sub-fix: ReadOnly-style gating for liquidations).
//!
//! # What the gap was
//!
//! `Mode::ReadOnly` auto-latches when the system TCR drops below 100% (see
//! `update_total_collateral_ratio_and_mode`). It blocks mints, borrows, and
//! margin top-ups via `validate_mode`, but liquidations were never gated by
//! `validate_mode` — and intentionally so, since liquidations REDUCE bad debt
//! and should remain open during stress.
//!
//! That left no admin lever to halt liquidations specifically. If the oracle
//! degraded but `last_price` was still within freshness limits, liquidators
//! could still execute against a stale value and either over- or
//! under-liquidate vaults. Wave-5 adds `liquidation_frozen: bool` (default
//! false) plus `set_liquidation_frozen(bool)` controller-only endpoint, and
//! `validate_liquidation_not_frozen()` is wired into every liquidation entry
//! point in main.rs.
//!
//! # How this file tests it
//!
//! The gate is a single bool on State. We can cover:
//!   * Default value is false (production behavior unchanged when admin
//!     hasn't intervened).
//!   * Round-trip through CBOR preserves the bool (so toggling persists
//!     across canister upgrades).
//!   * Decoupling: flipping `liquidation_frozen` does NOT mutate `mode`
//!     and vice versa. The two switches are independent.
//!
//! The wiring of the bool into the liquidation endpoints' `validate_liquidation_not_frozen()`
//! call sites is exercised by the live PocketIC liquidation suite — once
//! `liquidation_frozen` is true, every endpoint that takes `validate_call().await?`
//! will subsequently hit the new `validate_liquidation_not_frozen()` short-circuit.

use candid::Principal;

use rumi_protocol_backend::state::{Mode, State};
use rumi_protocol_backend::InitArg;

fn fresh_state() -> State {
    State::from(InitArg {
        xrc_principal: Principal::anonymous(),
        icusd_ledger_principal: Principal::anonymous(),
        icp_ledger_principal: Principal::anonymous(),
        fee_e8s: 0,
        developer_principal: Principal::anonymous(),
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    })
}

#[test]
fn liq_007_liquidation_frozen_defaults_to_false() {
    let state = fresh_state();
    assert!(
        !state.liquidation_frozen,
        "fresh state must NOT have liquidations frozen"
    );
}

#[test]
fn liq_007_liquidation_frozen_round_trips_through_cbor() {
    let mut state = fresh_state();
    state.liquidation_frozen = true;

    let mut buf = Vec::new();
    ciborium::ser::into_writer(&state, &mut buf).expect("encode state");
    let restored: State = ciborium::de::from_reader(buf.as_slice()).expect("decode state");

    assert!(
        restored.liquidation_frozen,
        "liquidation_frozen=true must survive an upgrade round-trip"
    );
}

#[test]
fn liq_007_freezing_liquidations_does_not_change_mode() {
    let mut state = fresh_state();
    let mode_before = state.mode;
    state.liquidation_frozen = true;
    assert_eq!(
        state.mode, mode_before,
        "liquidation_frozen must NOT mutate Mode (and vice versa)"
    );
}

#[test]
fn liq_007_readonly_mode_does_not_imply_liquidation_frozen() {
    let mut state = fresh_state();
    state.mode = Mode::ReadOnly;
    assert!(
        !state.liquidation_frozen,
        "ReadOnly auto-latch must leave liquidations open by default"
    );
}
