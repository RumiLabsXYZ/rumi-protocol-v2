//! Data model for the Season 1 points engine (spec `rumi-airdrop-spec-v2.md`
//! Section 7). These are the candid-facing, logical types used throughout the
//! crate. Their AT-REST representation is wrapped in versioned `Stored*` enums
//! in `state.rs` so fields can be added later without triggering a stable-memory
//! wipe (UPG-002). See the recipe doc-comment on each `Stored*` enum.
//!
//! Two deliberate refinements of the spec's literal struct shapes, both made for
//! stable-storage scalability and both called out in the Phase 1 status note:
//!   1. `PrincipalState` does NOT carry an inline `point_ledger: Vec<PointEntry>`.
//!      The implementation plan (Phase 1, task 4) mandates a SEPARATE global
//!      `StableLog<PointEntry>` audit ledger; the per-principal view is derived
//!      by filtering that log. Storing an unbounded Vec inside every BTreeMap
//!      value would rewrite the whole value on every accrual.
//!   2. (Naming) `pro_rata_share` is intentionally absent. That value is computed
//!      from the FROZEN ledger by the LATER claim canister, not here (spec
//!      Section 9). This canister only accrues `total_points`.

use candid::{CandidType, Principal};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ── Enums ───────────────────────────────────────────────────────────────────

/// Asset a position / deposit / repayment is denominated in. USD-pegged stables
/// are valued at $1.00 (spec Section 7, no peg-aware cap). ICP is valued via the
/// protocol XRC oracle at epoch time (Phase 5, see `valuation.rs`).
#[derive(
    CandidType, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub enum AssetType {
    IcUsd,
    CkUsdc,
    CkUsdt,
    ThreeUsd,
    Icp,
}

/// Where a position is held. Determines which multiplier-table row applies
/// (spec Section 4).
#[derive(
    CandidType, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub enum Venue {
    /// Vault debt outstanding + ckUSDC/ckUSDT repayment window.
    Vault,
    /// `rumi_3pool` deposits (icUSD 1x, single ck-stable 3x, matched pair 5x).
    ThreePool,
    /// `rumi_stability_pool` deposits (icUSD 1x, 3USD 2x).
    StabilityPool,
    /// `rumi_amm` 3USD/ICP LP (2x).
    Amm,
}

/// The first qualifying action that auto-registered a principal (spec Section 8).
/// Registration is implicit: the first event of any of these kinds enrolls the
/// principal. Activity before registration does NOT retroactively earn points.
#[derive(CandidType, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum QualifyingAction {
    MintIcUsd,
    RepayVault,
    Deposit3Pool,
    DepositStabilityPool,
    ProvideAmmLiquidity,
}

/// What activity produced a ledger row, for audit / personal breakdown display.
#[derive(CandidType, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointSource {
    /// Auto-registration marker (zero points, records enrollment).
    Registration,
    IcUsdDebt,
    IcUsd3Pool,
    CkStable3PoolUnmatched,
    CkStable3PoolMatched,
    VaultRepayment,
    IcUsdStabilityPool,
    ThreeUsdStabilityPool,
    AmmLp,
}

// ── Per-principal records (spec Section 7) ──────────────────────────────────

/// Logical key for a principal's active deposits. A principal holds at most one
/// active deposit per (venue, asset) pair; a new deposit event for the same pair
/// updates the existing record's recorded value rather than appending.
#[derive(
    CandidType, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub struct DepositKey {
    pub venue: Venue,
    pub asset: AssetType,
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DepositRecord {
    pub asset: AssetType,
    pub venue: Venue,
    /// Recorded USD value at deposit time. The accrued value each epoch is
    /// `min(recorded_value_usd, verified_3usd)` for 3pool deposits (spec
    /// Section 5); this is the recorded upper bound.
    pub recorded_value_usd: u128,
    pub deposited_at: u64,
    pub last_verified_at: u64,
}

impl DepositRecord {
    pub fn key(&self) -> DepositKey {
        DepositKey {
            venue: self.venue,
            asset: self.asset,
        }
    }
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RepaymentEvent {
    /// Only ckUSDC | ckUSDT repayments qualify for the 5x window (spec Section 6).
    pub asset: AssetType,
    pub amount_usd: u128,
    pub repaid_at: u64,
    /// `repaid_at + 90 days`, capped at season end.
    pub window_end: u64,
}

/// Per-principal accrual state. The bulk of canister storage: one of these per
/// registered principal, held in a `StableBTreeMap<Principal, _>` (see
/// `state.rs`). `total_points` is the season-to-date sum; the claim canister
/// later derives `pro_rata_share` from the frozen totals.
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PrincipalState {
    pub principal: Principal,
    pub total_points: u128,
    /// Keyed by (venue, asset). See `DepositKey`.
    pub active_deposits: BTreeMap<DepositKey, DepositRecord>,
    pub repayment_events: Vec<RepaymentEvent>,
    pub last_epoch_processed: u64,
    pub registered_at_ns: u64,
    pub first_qualifying_action: QualifyingAction,
}

impl PrincipalState {
    /// A freshly registered principal with no accrual yet.
    pub fn new(principal: Principal, registered_at_ns: u64, action: QualifyingAction) -> Self {
        Self {
            principal,
            total_points: 0,
            active_deposits: BTreeMap::new(),
            repayment_events: Vec::new(),
            last_epoch_processed: 0,
            registered_at_ns,
            first_qualifying_action: action,
        }
    }
}

// ── Append-only ledger rows ─────────────────────────────────────────────────

/// One row in the global append-only audit ledger (`StableLog<PointEntry>`).
/// Every accrual and the registration marker write one of these, tagged with the
/// principal so a per-principal view can be reconstructed by filtering.
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PointEntry {
    pub principal: Principal,
    pub epoch_index: u64,
    pub points_delta: u128,
    pub source: PointSource,
    pub recorded_at_ns: u64,
}

/// Per-epoch rollup written once when an epoch closes (`StableLog<EpochSummary>`).
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct EpochSummary {
    pub epoch_index: u64,
    pub epoch_start_ns: u64,
    pub epoch_end_ns: u64,
    pub total_points_all: u128,
    pub points_accrued_this_epoch: u128,
    pub active_principals: u64,
    pub registered_principals: u64,
    /// Snapshot timestamps actually used this epoch (revealed after close;
    /// 0 until Phase 5 wires the snapshot scheduler).
    pub snapshot_a_ns: u64,
    pub snapshot_b_ns: u64,
}

/// In-flight state of the OPEN weekly epoch, persisted inside `State` so the
/// periodic driver survives upgrades (resume from here, re-derive nothing). `None`
/// between epochs and before the season starts. The snapshot cursors are the
/// resume points for the chunked capture (last-processed principal; `None` means
/// "not started"). `a_complete` / `b_complete` gate the close on both snapshots
/// having been captured.
///
/// The `close_*` fields drive the chunked epoch CLOSE (POINTS-002): the close is
/// no longer a single atomic O(N principals x sources) pass (which can exceed the
/// 5B-instruction limit and trap, never advancing the epoch index -> permanent
/// accrual stall). Instead it processes a bounded batch of principals per tick,
/// persisting the resume cursor and the running totals here, and only finalizes
/// (seed close + summary + index advance) once the cursor reaches the end. The
/// cursor advances strictly past each closed principal, so none is double-credited
/// across resumed batches.
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct OpenEpoch {
    pub epoch_index: u64,
    pub epoch_start_ns: u64,
    /// `min(epoch_start + EPOCH_DURATION, season_end)` (the last epoch is partial).
    pub epoch_end_ns: u64,
    pub snapshot_a_ns: u64,
    pub snapshot_b_ns: u64,
    pub a_cursor: Option<Principal>,
    pub a_complete: bool,
    pub b_cursor: Option<Principal>,
    pub b_complete: bool,
    /// Whether the chunked close pass has begun. Distinguishes "close not started"
    /// (`false`) from "close in progress with no principal yet closed"
    /// (`true`, `close_cursor == None`).
    pub close_started: bool,
    /// Resume point for the chunked close: the last principal whose accrual was
    /// committed. `None` = none closed yet. The next batch processes principals
    /// strictly AFTER this, guaranteeing exactly-once close per principal.
    pub close_cursor: Option<Principal>,
    /// Running sum of points accrued across all close batches so far (for the
    /// `EpochSummary` written at finalization).
    pub close_points_accrued: u128,
    /// Running count of principals that accrued > 0 across all close batches.
    pub close_active: u64,
}

// ── Query view types ────────────────────────────────────────────────────────

/// One row of the public leaderboard (spec Section 10). The canister returns the
/// full principal; the frontend truncates it for display ("abcde...wxyz").
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LeaderboardEntry {
    pub rank: u32,
    pub principal: Principal,
    pub total_points: u128,
    /// Estimated share of the 5% Season-1 pool, in basis points (0..=10000).
    /// 0 during the season's early life when totals are tiny / zero.
    pub estimated_share_bps: u32,
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RegistrationInfo {
    pub principal: Principal,
    pub registered_at_ns: u64,
    pub first_qualifying_action: QualifyingAction,
}

/// Read-only snapshot of admin-configurable parameters, for the ops dashboard.
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PointsConfig {
    pub admin: Principal,
    pub season_start_ns: u64,
    pub season_end_ns: u64,
    pub excluded_count: u32,
    pub registered_count: u64,
    pub current_epoch_index: u64,
    pub snapshot_seed_committed: bool,
}

/// FULL epoch-driver status, including the in-flight capture/close cursors. This
/// is ADMIN-ONLY (`get_epoch_status_admin`): exposing `a_cursor`/`b_cursor`/
/// `a_complete`/`b_complete` to the public lets a not-yet-captured principal watch
/// the capture cursor and flash-inflate its balance right before its snapshot
/// chunk, defeating the `min(A,B)` anti-snipe defense (POINTS-001). The public
/// `get_epoch_status` returns `PublicEpochStatus` instead.
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct EpochStatus {
    pub current_epoch_index: u64,
    pub driver_enabled: bool,
    pub driver_interval_secs: u64,
    pub open_epoch: Option<OpenEpoch>,
    pub revealed_seed_count: u64,
    pub snapshot_seed_committed: bool,
}

/// PUBLIC epoch-driver status for the ops dashboard (POINTS-001). Mirrors
/// `EpochStatus` but the open epoch is reduced to its non-sensitive window
/// (bounds + snapshot times), with the in-flight capture/close cursors and
/// completion flags OMITTED so an attacker cannot time a flash deposit to a
/// snapshot it has not yet been captured into.
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PublicEpochStatus {
    pub current_epoch_index: u64,
    pub driver_enabled: bool,
    pub driver_interval_secs: u64,
    /// The open epoch's public window (no capture/close progress). `None` between
    /// epochs and before the season starts.
    pub open_epoch: Option<PublicOpenEpoch>,
    pub revealed_seed_count: u64,
    pub snapshot_seed_committed: bool,
}

/// Public view of the open epoch: its bounds, plus each snapshot time only AFTER
/// that moment has passed (PTS-002). A FUTURE snapshot time is exactly when a
/// flash deposit must land to game the `min(A,B)` anti-snipe defense, so it stays
/// `None` until `now >= time`; once fired it is history and safe to show. The
/// capture/close cursors and completion flags are not exposed at all (POINTS-001).
/// Admins keep full visibility via `get_epoch_status_admin`.
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct PublicOpenEpoch {
    pub epoch_index: u64,
    pub epoch_start_ns: u64,
    pub epoch_end_ns: u64,
    /// `None` while the snapshot time is still in the future (PTS-002).
    pub snapshot_a_ns: Option<u64>,
    /// `None` while the snapshot time is still in the future (PTS-002).
    pub snapshot_b_ns: Option<u64>,
}

impl PublicOpenEpoch {
    /// Reduce the full open epoch to its public view as of `now_ns`, revealing
    /// each snapshot time only once it has fired.
    pub fn redacted(o: &OpenEpoch, now_ns: u64) -> Self {
        let fired = |t: u64| if now_ns >= t { Some(t) } else { None };
        PublicOpenEpoch {
            epoch_index: o.epoch_index,
            epoch_start_ns: o.epoch_start_ns,
            epoch_end_ns: o.epoch_end_ns,
            snapshot_a_ns: fired(o.snapshot_a_ns),
            snapshot_b_ns: fired(o.snapshot_b_ns),
        }
    }
}

/// One source's ingestion state (Phase 2 ops observability).
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SourceStatus {
    /// Source tag: 0 = backend, 1 = 3pool, 2 = stability pool, 3 = AMM.
    pub tag: u8,
    pub canister: Principal,
    /// Next `start` to poll from (the forward cursor).
    pub cursor: u64,
}

/// Pull-ingestion status across all configured sources.
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct IngestStatus {
    pub sources: Vec<SourceStatus>,
    pub registered_count: u64,
    /// Whether the periodic poll timer is running (Phase 2b).
    pub poll_enabled: bool,
    pub poll_interval_secs: u64,
}

/// Error surface for the admin / registration update endpoints.
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum PointsError {
    /// Caller is not the configured admin.
    Unauthorized,
    /// Principal is in the excluded set (protocol-owned canisters, etc.) and
    /// cannot register or accrue (spec Section 11).
    Excluded,
}

/// Optional init / upgrade argument. All fields default sensibly so a bare local
/// deploy (`'(null)'`) works; mainnet deploy supplies the real admin + season.
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, Default)]
pub struct InitArgs {
    /// Admin principal. Defaults to the deploying caller if `None`.
    pub admin: Option<Principal>,
    /// Excluded principals. Defaults to the protocol-owned canister seed if
    /// `None` (see `state::protocol_owned_canister_seed`).
    pub excluded_principals: Option<Vec<Principal>>,
    /// Season window. Defaults to June 1 2026 .. Aug 31 2026 23:59:59 UTC.
    pub season_start_ns: Option<u64>,
    pub season_end_ns: Option<u64>,
    /// Commit-reveal seed hash H0 (spike 0.3). `None` until Phase 5 wires it.
    pub snapshot_seed_commit: Option<[u8; 32]>,
}
