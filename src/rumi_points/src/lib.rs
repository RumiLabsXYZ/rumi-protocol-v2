//! # rumi_points
//!
//! Season 1 airdrop points engine for Rumi Protocol (spec `rumi-airdrop-spec-v2.md`).
//! Accrues dollar-days of qualifying activity per principal during the season
//! (June 1 .. Aug 31, 2026), publishes a leaderboard, and freezes a points ledger
//! that the LATER claim canister (`rumi_airdrop_claim`) reads to compute each
//! user's `pro_rata_share` of the 5% Season-1 token pool.
//!
//! ## Phase 1 status (this scaffold)
//! Real and tested: the stable-storage layout (`state.rs`), the data model
//! (`types.rs`), the configurable excluded-principals set, test-only registration,
//! and the versioned-snapshot upgrade-safety pattern. Skeleton (signatures + doc
//! comments, land in later phases): `events.rs` (Phase 2 ingestion), `epoch.rs`
//! and `valuation.rs` (Phase 5 multiplier / snapshot math), and the commit-reveal
//! ALGORITHM in `snapshot_seed.rs` (Phase 5; its STATE types are real today).
//!
//! Out of scope here: event ingestion, the backend `repayment_asset` change,
//! 3USD verification, epoch math, and the entire claim / lock-tier surface.

pub mod accrual;
pub mod epoch;
pub mod events;
pub mod poll;
pub mod snapshot_seed;
pub mod source_types;
pub mod state;
pub mod types;
pub mod valuation;

/// Nanoseconds in one day (IC `ic_cdk::api::time()` is nanoseconds since epoch).
pub const NANOS_PER_DAY: u64 = 24 * 60 * 60 * 1_000_000_000;

/// Default season window, overridable via `InitArgs`. These are the locked
/// hard dates from spec Section 15, precomputed in nanoseconds since the Unix
/// epoch:
///   - start: 2026-06-01T00:00:00Z
///   - end:   2026-08-31T23:59:59Z
pub const DEFAULT_SEASON_START_NS: u64 = 1_780_272_000_000_000_000;
pub const DEFAULT_SEASON_END_NS: u64 = 1_788_220_799_000_000_000;
