//! CDP-12 regression fence: the post-XRC bookkeeping (interest accrual,
//! treasury drains, vault-health sweep) runs in independently scheduled
//! timers, not a single chained closure where one trap skips everything
//! downstream.
//!
//! Pre-fix `fetch_icp_rate` (Timer A) chained: XRC fetch → interest
//! accrual O(V) → drain_pending_treasury_interest →
//! drain_pending_treasury_collateral → flush_pending_interest →
//! check_vaults O(V) → spawn bot/SP. A trap anywhere in this chain
//! skipped all downstream work for the next 5 minutes.
//!
//! Audit fence per `.claude/security-docs/2026-05-02-wave-14-avai-parity-plan.md`.
//!
//! Layered fences:
//!  1. `fetch_icp_rate` (Timer A), `interest_and_treasury_tick` (Timer B),
//!     `vault_check_tick` (Timer C) all exist as separate `pub async fn`
//!     entry points.
//!  2. Their cadence constants (`FETCHING_ICP_RATE_INTERVAL`,
//!     `INTEREST_AND_TREASURY_TICK_INTERVAL`, `VAULT_CHECK_TICK_INTERVAL`)
//!     are exposed and have sensible defaults.
//!
//! IC message-level trap isolation gives us the runtime behavior the plan
//! calls for: a trap inside Timer B's callback kills only that message,
//! Timer A and C continue firing on their own intervals. (`catch_unwind`
//! at the application layer is not a thing on wasm/IC; the runtime
//! handles it.)

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use rumi_protocol_backend::xrc::{
    fetch_icp_rate, interest_and_treasury_tick, vault_check_tick,
    FETCHING_ICP_RATE_INTERVAL, INTEREST_AND_TREASURY_TICK_INTERVAL,
    VAULT_CHECK_TICK_INTERVAL,
};

#[test]
fn cdp_12_three_timer_entry_points_exist_with_unit_return() {
    // Compilation alone proves the functions exist with the expected
    // signatures. Boxing into a trait object enforces `pub async fn () -> ()`
    // (any other return type or arg list would fail to coerce).
    fn assert_async_unit_fn<F>(_: F)
    where
        F: Fn() -> Pin<Box<dyn Future<Output = ()>>>,
    {
    }

    // The three entry points the plan calls for. All async + unit return.
    assert_async_unit_fn(|| Box::pin(fetch_icp_rate()) as _);
    assert_async_unit_fn(|| Box::pin(interest_and_treasury_tick()) as _);
    assert_async_unit_fn(|| Box::pin(vault_check_tick()) as _);
}

#[test]
fn cdp_12_intervals_have_sensible_defaults() {
    // Timer A (XRC) keeps the legacy 300s cadence to avoid a 5x cycle
    // increase that would come from going to the plan-suggested 60s
    // without an ops review.
    assert_eq!(
        FETCHING_ICP_RATE_INTERVAL,
        Duration::from_secs(300),
        "Timer A interval must remain 300s to preserve XRC cycle budget",
    );

    // Timer B (interest + treasury) at 60s per the plan; cheap in cycles.
    assert_eq!(
        INTEREST_AND_TREASURY_TICK_INTERVAL,
        Duration::from_secs(60),
        "Timer B interval should be 60s for fast interest accrual",
    );

    // Timer C (check_vaults + aggregate-snapshot refresh) at 300s — same
    // cadence the chained version had, so liquidation latency is unchanged.
    assert_eq!(
        VAULT_CHECK_TICK_INTERVAL,
        Duration::from_secs(300),
        "Timer C interval must match the legacy 300s check_vaults cadence",
    );
}
