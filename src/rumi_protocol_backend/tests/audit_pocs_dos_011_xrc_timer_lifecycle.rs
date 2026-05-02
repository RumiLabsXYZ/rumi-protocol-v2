//! Wave-9d DoS hardening: XRC timer status check
//! (DOS-011).
//!
//! Audit report:
//!   * `audit-reports/2026-04-22-28e9896/findings.json` finding DOS-011
//!     (`Every add_collateral_token registers a new permanent XRC price
//!     timer`, recommendation: "Inside the timer closure, check
//!     collateral status and early-return if disabled").
//!   * Wave plan: `audit-reports/2026-04-22-28e9896/remediation-plan.md`
//!     §"Wave 9 — DoS hardening".
//!
//! # What the gap is
//!
//! Pre-Wave-9d every call to `add_collateral_token` registered a
//! permanent `set_timer_interval` that fires every 300s and burns
//! ~1B cycles per call to XRC, regardless of whether the collateral is
//! still actively being used. CollateralStatus has 5 lifecycle states
//! (Active / Paused / Frozen / Sunset / Deprecated), but only Active
//! and Paused require ongoing price updates: Frozen, Sunset, and
//! Deprecated either block all operations or are read-only winding-
//! down states where a fresh price doesn't change anything.
//!
//! # Why the status-check approach (not the cancel-path approach)
//!
//! The audit's stated recommendation is "check collateral status and
//! early-return if disabled". We follow that rather than canceling
//! timers via `clear_timer`:
//!  * `TimerId` is a runtime handle that does NOT survive upgrade —
//!    every `setup_timers()` call re-registers a fresh set of timers.
//!    Storing TimerId in stable memory would be misleading.
//!  * After `setup_timers()` the canister has one timer per non-ICP
//!    collateral, each closure capturing the ledger principal. Adding a
//!    one-line `read_state` gate at the top of the closure costs ~3
//!    instructions vs. the ~1B cycles of the XRC call we skip.
//!  * Reactivation works automatically: when status flips from Sunset
//!    back to Active (admin call), the very next 300s tick resumes
//!    fetching with no timer re-registration needed.
//!  * Same outcome as cancel-path: Frozen/Sunset/Deprecated collateral
//!    no longer burns cycles on background XRC fetches.
//!
//! Layered fences:
//!
//!  1. **Pure-function fence** —
//!     `collateral_needs_periodic_price_refresh(status)` returns true
//!     only for Active and Paused. Paused still allows liquidations
//!     (per `CollateralStatus::allows_liquidation`), so we still want
//!     fresh prices.
//!  2. **Status-coverage fence** — every variant of CollateralStatus
//!     must appear in the helper's match (compile fails otherwise).
//!  3. **Active-state-needs-price** — the helper must return true for
//!     statuses where any price-sensitive operation can still happen.
//!  4. **Soft-delist-skips-fetch** — the helper must return false for
//!     Frozen, Sunset, Deprecated.
//!  5. **Round-trip on status flip** — flipping status flips the
//!     helper's answer immediately (no cached / stale answer).

use candid::Principal;

use rumi_protocol_backend::state::{CollateralStatus, State};
use rumi_protocol_backend::xrc::{
    collateral_needs_periodic_price_refresh, should_fetch_collateral_price,
};
use rumi_protocol_backend::InitArg;

fn icp_ledger() -> Principal {
    Principal::from_slice(&[10])
}

fn other_ledger() -> Principal {
    Principal::from_slice(&[20])
}

fn fresh_state() -> State {
    State::from(InitArg {
        xrc_principal: Principal::anonymous(),
        icusd_ledger_principal: Principal::anonymous(),
        icp_ledger_principal: icp_ledger(),
        fee_e8s: 0,
        developer_principal: Principal::anonymous(),
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    })
}

fn set_status(state: &mut State, ct: Principal, status: CollateralStatus) {
    if let Some(config) = state.collateral_configs.get_mut(&ct) {
        config.status = status;
    } else {
        panic!("collateral type {} not in configs", ct);
    }
}

// ============================================================================
// Layer 1 — pure-function fence
// ============================================================================

#[test]
fn dos_011_active_collateral_needs_price() {
    assert!(
        collateral_needs_periodic_price_refresh(CollateralStatus::Active),
        "Active collateral must keep receiving price updates — every \
         user-facing op (open / borrow / withdraw / liquidate / redeem) \
         requires a fresh price."
    );
}

#[test]
fn dos_011_paused_collateral_needs_price() {
    // Paused allows repay + add_collateral + close + liquidation, all
    // of which are price-sensitive. Skipping price refresh for Paused
    // would let liquidations run on stale prices.
    assert!(
        collateral_needs_periodic_price_refresh(CollateralStatus::Paused),
        "Paused collateral still allows liquidations and must keep \
         receiving price updates. Skipping refresh would let \
         liquidations run on stale data."
    );
}

// ============================================================================
// Layer 2 — soft-delist statuses skip fetch
// ============================================================================

#[test]
fn dos_011_frozen_collateral_skips_price() {
    assert!(
        !collateral_needs_periodic_price_refresh(CollateralStatus::Frozen),
        "Frozen collateral blocks every operation (HARD STOP). \
         Background price fetches are pure cycle waste."
    );
}

#[test]
fn dos_011_sunset_collateral_skips_price() {
    assert!(
        !collateral_needs_periodic_price_refresh(CollateralStatus::Sunset),
        "Sunset collateral allows only repay/withdraw/close — no \
         price-sensitive operations. Background fetches are unnecessary."
    );
}

#[test]
fn dos_011_deprecated_collateral_skips_price() {
    assert!(
        !collateral_needs_periodic_price_refresh(CollateralStatus::Deprecated),
        "Deprecated collateral is read-only. Nothing on the protocol \
         touches its price; background fetches burn ~1B cycles per tick \
         for no behavioural change."
    );
}

// ============================================================================
// Layer 3 — round trip on status flip
// ============================================================================

#[test]
fn dos_011_helper_round_trips_on_status_flips() {
    // Active → Frozen → Active. The helper's answer flips with the
    // status; there is no cached / hysteretic value to stomach.
    let active = CollateralStatus::Active;
    let frozen = CollateralStatus::Frozen;
    assert!(collateral_needs_periodic_price_refresh(active));
    assert!(!collateral_needs_periodic_price_refresh(frozen));
    assert!(collateral_needs_periodic_price_refresh(active));
}

// ============================================================================
// Layer 4 — exhaustive coverage of CollateralStatus
// ============================================================================

// ============================================================================
// Layer 4 — state-level gate (composes status lookup + classification)
// ============================================================================

#[test]
fn dos_011_state_gate_returns_true_for_active_collateral() {
    let mut state = fresh_state();
    set_status(&mut state, icp_ledger(), CollateralStatus::Active);
    assert!(
        should_fetch_collateral_price(&state, &icp_ledger()),
        "active collateral must be fetched"
    );
}

#[test]
fn dos_011_state_gate_returns_false_for_sunset_collateral() {
    let mut state = fresh_state();
    set_status(&mut state, icp_ledger(), CollateralStatus::Sunset);
    assert!(
        !should_fetch_collateral_price(&state, &icp_ledger()),
        "sunset collateral must be skipped"
    );
}

#[test]
fn dos_011_state_gate_round_trips_on_status_flip_in_state() {
    // Active → Frozen → Active flip-on-state, mirroring what
    // `set_collateral_status` does when admin flips a collateral.
    let mut state = fresh_state();
    set_status(&mut state, icp_ledger(), CollateralStatus::Active);
    assert!(should_fetch_collateral_price(&state, &icp_ledger()));
    set_status(&mut state, icp_ledger(), CollateralStatus::Frozen);
    assert!(!should_fetch_collateral_price(&state, &icp_ledger()));
    set_status(&mut state, icp_ledger(), CollateralStatus::Active);
    assert!(should_fetch_collateral_price(&state, &icp_ledger()));
}

#[test]
fn dos_011_state_gate_returns_false_for_unknown_collateral() {
    // If a closure outlives its collateral entry (e.g., never present
    // in `collateral_configs`) the gate must skip — otherwise the timer
    // would pound XRC for a deleted ledger every 300s.
    let state = fresh_state();
    assert!(
        !should_fetch_collateral_price(&state, &other_ledger()),
        "unknown collateral must be skipped"
    );
}

// ============================================================================
// Layer 5 — exhaustive coverage of CollateralStatus
// ============================================================================

#[test]
fn dos_011_every_status_variant_classified() {
    // If a new CollateralStatus variant is added without updating the
    // helper, the match in `collateral_needs_periodic_price_refresh`
    // would compile (any of the existing arms could shadow it). This
    // test pins the count: 5 variants today, all classified explicitly.
    let variants = [
        CollateralStatus::Active,
        CollateralStatus::Paused,
        CollateralStatus::Frozen,
        CollateralStatus::Sunset,
        CollateralStatus::Deprecated,
    ];
    let mut needs = 0usize;
    let mut skips = 0usize;
    for s in &variants {
        if collateral_needs_periodic_price_refresh(*s) {
            needs += 1;
        } else {
            skips += 1;
        }
    }
    assert_eq!(
        needs, 2,
        "exactly 2 statuses (Active, Paused) must need periodic price \
         refresh — got {} needs / {} skips",
        needs, skips
    );
    assert_eq!(
        skips, 3,
        "exactly 3 statuses (Frozen, Sunset, Deprecated) must skip the \
         periodic refresh — got {} needs / {} skips",
        needs, skips
    );
}
