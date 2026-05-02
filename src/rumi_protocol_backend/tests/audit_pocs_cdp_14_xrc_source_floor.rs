//! CDP-14 regression fence: reject XRC price updates whose
//! `metadata.num_sources_used` is below a configurable floor.
//!
//! Pre-fix, both `xrc::fetch_icp_rate` and
//! `management::fetch_collateral_price` consumed
//! `exchange_rate_result.metadata.decimals` for normalization but never
//! looked at `num_sources_used`. A rate aggregated from a single CEX is
//! cheaper to manipulate than one aggregated from three or more, and the
//! Wave-5 LIQ-007 sanity band catches outlier *values* but not the
//! *thinness* of the underlying aggregation. Audit fence per
//! `.claude/security-docs/2026-05-02-wave-14-avai-parity-plan.md`.
//!
//! Layered fences:
//!  1. Pure helper `xrc_metadata_meets_source_floor(num_sources_used,
//!     min_required)` returns true iff the sample is acceptable.
//!  2. Defaults: when `min_required == 0` the helper short-circuits to
//!     true (kill switch for ops).
//!  3. The default floor (`MIN_XRC_SOURCES`) is at least 3 so the
//!     production policy is meaningful.

use rumi_protocol_backend::xrc::{xrc_metadata_meets_source_floor, MIN_XRC_SOURCES};

#[test]
fn cdp_14_default_floor_is_at_least_three() {
    assert!(
        MIN_XRC_SOURCES >= 3,
        "the production source floor must require aggregation across at least three CEXs",
    );
}

#[test]
fn cdp_14_below_floor_is_rejected() {
    assert!(
        !xrc_metadata_meets_source_floor(1, 3),
        "single-source XRC samples must be rejected when the floor is 3",
    );
    assert!(
        !xrc_metadata_meets_source_floor(2, 3),
        "two-source XRC samples must be rejected when the floor is 3",
    );
}

#[test]
fn cdp_14_at_or_above_floor_is_accepted() {
    assert!(
        xrc_metadata_meets_source_floor(3, 3),
        "samples meeting the floor exactly must be accepted",
    );
    assert!(
        xrc_metadata_meets_source_floor(7, 3),
        "samples above the floor must be accepted",
    );
}

#[test]
fn cdp_14_zero_floor_is_kill_switch() {
    // Setting `min_required == 0` disables the gate. Operators can do this
    // if XRC's source aggregation degrades industry-wide and the floor is
    // doing more harm than good. Helper must short-circuit to true.
    assert!(
        xrc_metadata_meets_source_floor(0, 0),
        "min_required == 0 disables the gate",
    );
    assert!(
        xrc_metadata_meets_source_floor(1, 0),
        "min_required == 0 disables the gate (single source)",
    );
}
