//! Weekly epoch driver (spec Section 7, implementation plan Phase 5).
//!
//! PHASE 5 SCOPE (skeleton only here). Once per week the driver:
//!   1. Derives two intra-epoch snapshot times via the commit-reveal seed
//!      (`snapshot_seed::derive_snapshot_times`).
//!   2. Captures per-principal balances at each snapshot into a transient buffer.
//!   3. Accrues `dollar_days = active_value * multiplier * period / day` per
//!      position, takes `min(snapshot_a, snapshot_b)` (closes end-of-epoch
//!      sniping), and adds to `total_points`.
//!   4. For matched ckUSDC+ckUSDT: `2 * min(USDC, USDT)` at 5x, remainder at 3x
//!      (the dust-gaming fix).
//!   5. For open repayment windows: `amount * elapsed_days_in_window * 5`,
//!      truncated at season end.
//!   6. Appends `PointEntry` rows and one `EpochSummary`, advances
//!      `last_epoch_processed`, and closes the seed epoch.
//!
//! None of the multiplier / snapshot / min() math is implemented in Phase 1.

#![allow(dead_code)] // Phase 5 surface.

/// Length of one epoch. A week, expressed in nanoseconds (IC time unit).
pub const EPOCH_DURATION_NS: u64 = 7 * 24 * 60 * 60 * 1_000_000_000;

/// Run one full weekly epoch: schedule both snapshots, accrue on close. Registered
/// on a 1-week timer in `setup_timers` (Phase 5).
pub async fn run_epoch() {
    unimplemented!("Phase 5: weekly two-snapshot min() accrual");
}

/// Capture per-principal balances across all sources at a single snapshot.
pub async fn take_snapshot(_epoch_index: u64, _snapshot_index: u8) {
    unimplemented!("Phase 5");
}

/// Accrue one principal's points for a closing epoch from its two snapshots,
/// returning the weekly `min()` delta. The multiplier table lives here.
pub fn accrue_principal_epoch(_principal: candid::Principal, _epoch_index: u64) -> u128 {
    unimplemented!("Phase 5");
}
