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

use std::cell::RefCell;
use std::time::Duration;

use candid::{Nat, Principal};
use ic_cdk::api::call::RejectionCode;
use ic_cdk_timers::TimerId;
use icrc_ledger_types::icrc1::account::Account;

use crate::accrual::{self, RawSnapshot};
use crate::events::SourceId;
use crate::snapshot_seed::{sha256, SeedError, SeedManager};
use crate::source_types::balances;
use crate::state;
use crate::types::{AssetType, EpochSummary, OpenEpoch};
use crate::valuation::SnapshotPrices;

/// Length of one epoch. A week, expressed in nanoseconds (IC time unit).
pub const EPOCH_DURATION_NS: u64 = 7 * 24 * 60 * 60 * 1_000_000_000;
fn legacy_reseed_window_allowed(epoch_end_ns: u64, now_ns: u64) -> bool {
    now_ns < epoch_end_ns
}

fn safe_due_epoch_start(scheduled_start_ns: u64, epoch_end_ns: u64, now_ns: u64) -> Option<u64> {
    if now_ns < scheduled_start_ns || now_ns >= epoch_end_ns {
        return None;
    }
    if now_ns == scheduled_start_ns {
        Some(scheduled_start_ns)
    } else if legacy_reseed_window_allowed(epoch_end_ns, now_ns) {
        // A standby can miss the scheduled start. Rebase to this transition
        // instant so the new open epoch never accrues a pre-open interval.
        Some(now_ns)
    } else {
        None
    }
}

fn legacy_open_epoch_requires_review(
    epoch: &OpenEpoch,
    current_entropy: Option<[u8; 32]>,
    secure_seed_chain_v1: bool,
) -> bool {
    if epoch.epoch_index == 0 {
        !secure_seed_chain_v1
    } else {
        current_entropy.is_none()
    }
}

fn hold_expired_unopened_epoch(
    index: u64,
    now_ns: u64,
    season_start_ns: u64,
    season_end_ns: u64,
    review_uncommitted_epoch_zero: bool,
) -> bool {
    // Runtime epoch zero on a fresh uncommitted install has no scheduled work.
    // After an upgrade, however, even that configured window must be reviewed
    // if it has expired before the epoch was opened.
    if index == 0 && !state::snapshot_seed_committed() && !review_uncommitted_epoch_zero {
        return false;
    }
    let (_, epoch_end_ns) = epoch_bounds(index, season_start_ns, season_end_ns);
    if legacy_reseed_window_allowed(epoch_end_ns, now_ns) {
        return false;
    }

    // Do not synthesize a zero-point close or silently advance the epoch index;
    // an operator must review the missed reward interval and explicitly resume.
    state::set_legacy_transition_held(true);
    state::set_legacy_reseed_pending(false);
    ic_cdk::println!(
        "[epoch] held expired unopened epoch {} for admin review without advancing rewards",
        index
    );
    true
}

/// Bounds of epoch `index`: `[season_start + index*EPOCH, min(start + EPOCH,
/// season_end)]`. The last epoch is partial (truncated at season end).
pub fn epoch_bounds(index: u64, season_start_ns: u64, season_end_ns: u64) -> (u64, u64) {
    let start = season_start_ns.saturating_add(index.saturating_mul(EPOCH_DURATION_NS));
    let end = start.saturating_add(EPOCH_DURATION_NS).min(season_end_ns);
    (start, end)
}

/// What the periodic driver should do on this tick (state machine, spec Section 7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriverAction {
    /// Nothing to do yet (waiting for a snapshot time, the epoch end, or the
    /// season start / next epoch).
    Idle,
    /// Open the next epoch (index >= 1 only; epoch 0 is bootstrapped by the admin
    /// `start_season`, which provides the secret seed S0).
    Start,
    CaptureA,
    CaptureB,
    Close,
}

/// Decide the driver's action. Pure: the caller (the timer tick) reads
/// `now`/season/open-epoch from state, then performs the returned action. Assumes
/// the driver is enabled (the tick checks that before calling).
pub fn next_action(
    open: &Option<OpenEpoch>,
    now: u64,
    season_start: u64,
    season_end: u64,
    current_index: u64,
) -> DriverAction {
    match open {
        Some(oe) => {
            if !oe.a_complete {
                step_when(now >= oe.snapshot_a_ns, DriverAction::CaptureA)
            } else if !oe.b_complete {
                step_when(now >= oe.snapshot_b_ns, DriverAction::CaptureB)
            } else {
                step_when(now >= oe.epoch_end_ns, DriverAction::Close)
            }
        }
        // Epoch 0 is operator-bootstrapped (it needs the secret seed); the driver
        // only auto-starts epochs >= 1, whose seed is pre-loaded by the prior close.
        None if current_index == 0 => DriverAction::Idle,
        None => {
            let (start, _) = epoch_bounds(current_index, season_start, season_end);
            step_when(start < season_end && now >= start, DriverAction::Start)
        }
    }
}

fn step_when(ready: bool, action: DriverAction) -> DriverAction {
    if ready {
        action
    } else {
        DriverAction::Idle
    }
}

thread_local! {
    /// The live epoch-driver timer (transient; re-registered in `post_upgrade`).
    static EPOCH_TIMER: RefCell<Option<TimerId>> = RefCell::new(None);
}

/// Principals captured per driver tick. Season-1 scale fits one tick; larger
/// seasons span several (the cursor in `OpenEpoch` resumes between ticks).
const CAPTURE_CHUNK: u64 = 100;

/// Decide the snapshot's next resume cursor and completion flag from one chunk's
/// outcome. Pure, so the capture book-keeping is unit-testable:
///   - `done` only when the chunk was short (registered set exhausted) AND no
///     per-principal fetch errored. An error must never let the snapshot complete
///     with a transient 0 that the close-time `min()` would lock in.
///   - the resume cursor is the last principal we actually captured (or the prior
///     cursor if none were, e.g. the first principal errored), so the next tick
///     retries the failed principal instead of skipping past it.
fn next_capture_cursor(
    chunk_len: usize,
    last_captured: Option<Principal>,
    prev_cursor: Option<Principal>,
    hit_error: bool,
) -> (Option<Principal>, bool) {
    let exhausted = (chunk_len as u64) < CAPTURE_CHUNK;
    let done = exhausted && !hit_error;
    let next_cursor = if done { None } else { last_captured.or(prev_cursor) };
    (next_cursor, done)
}

type CallResult<T> = Result<T, (RejectionCode, String)>;

#[derive(Clone, Copy)]
enum Snapshot {
    A,
    B,
}

/// The 3USD/ICP AMM pool, oriented so `reserve_3usd` is the 3USD leg regardless of
/// the pool's `token_a`/`token_b` order.
#[derive(Clone, Debug, PartialEq)]
pub struct AmmPool {
    pub pool_id: String,
    pub reserve_3usd: u128,
    pub reserve_icp: u128,
    pub total_lp: u128,
}

/// Pick the 3USD/ICP pool from `pools` and orient its reserves. `None` if absent.
pub fn pick_amm_pool(
    pools: &[balances::PoolInfo],
    threeusd: Principal,
    icp: Principal,
) -> Option<AmmPool> {
    pools.iter().find_map(|p| {
        let pair = [p.token_a, p.token_b];
        if !(pair.contains(&threeusd) && pair.contains(&icp)) {
            return None;
        }
        // Orient so reserve_3usd is the 3USD leg regardless of token order.
        let (reserve_3usd, reserve_icp) = if p.token_a == threeusd {
            (p.reserve_a, p.reserve_b)
        } else {
            (p.reserve_b, p.reserve_a)
        };
        Some(AmmPool {
            pool_id: p.pool_id.clone(),
            reserve_3usd,
            reserve_icp,
            total_lp: p.total_lp_shares,
        })
    })
}

/// Why `start_season` was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartSeasonError {
    /// `now` is outside `[season_start, season_end)`.
    SeasonInactive,
    /// Epoch 0 is already open or past (the season was already started).
    AlreadyStarted,
    /// The snapshot-seed commitment `H0` was never set (at init). Opening a season
    /// against an uncommitted seed would let the operator choose the snapshot times
    /// after seeing pre-season activity, defeating the commit-reveal anti-sniping
    /// guarantee. Re-deploy/init with `snapshot_seed_commit = sha256(S0)` first.
    NotCommitted,
    /// The provided seed did not match the committed `H0`.
    Seed(SeedError),
}

// ── Timer (mirrors the poll timer: OFF by default, re-registered post-upgrade) ──

pub fn setup_epoch_timer() {
    EPOCH_TIMER.with(|t| {
        if let Some(id) = t.borrow_mut().take() {
            ic_cdk_timers::clear_timer(id);
        }
    });
    if state::epoch_driver_enabled() {
        let interval = Duration::from_secs(state::epoch_driver_interval_secs());
        let id = ic_cdk_timers::set_timer_interval(interval, || {
            ic_cdk::spawn(async {
                epoch_driver_tick().await;
            });
        });
        EPOCH_TIMER.with(|t| *t.borrow_mut() = Some(id));
    }
}

// ── Bootstrap: the admin opens epoch 0 with the secret seed S0 ──

pub fn start_season(initial_seed: [u8; 32], now: u64) -> Result<(), StartSeasonError> {
    let (season_start, season_end) = state::season_bounds();
    if now < season_start || now >= season_end {
        return Err(StartSeasonError::SeasonInactive);
    }
    if state::current_epoch_index() != 0 || state::get_open_epoch().is_some() {
        return Err(StartSeasonError::AlreadyStarted);
    }
    // Commit-reveal integrity: H0 must have been committed at init, BEFORE the
    // season, so the snapshot times are fixed by a value chosen blind to season
    // activity. Refuse to open epoch 0 against an uncommitted seed (otherwise the
    // operator could pick favourable times now). `open_new_epoch` then verifies
    // `sha256(initial_seed) == H0` via `SeedManager::start_epoch`.
    if !state::snapshot_seed_committed() {
        return Err(StartSeasonError::NotCommitted);
    }
    open_new_epoch(0, Some(initial_seed), now).map_err(StartSeasonError::Seed)
}

/// Derive the epoch's seed + snapshot times, install a fresh `OpenEpoch`, and clear
/// the snapshot buffer. Epoch 0 passes `Some(S0)`; epochs >= 1 pass `None` (the
/// pre-loaded `current_seed` is used).
fn open_new_epoch(index: u64, seed_arg: Option<[u8; 32]>, _now: u64) -> Result<(), SeedError> {
    let (season_start, season_end) = state::season_bounds();
    let (start, end) = epoch_bounds(index, season_start, season_end);
    open_epoch_at(index, start, end, seed_arg)
}

fn open_epoch_at(
    index: u64,
    epoch_start_ns: u64,
    epoch_end_ns: u64,
    seed_arg: Option<[u8; 32]>,
) -> Result<(), SeedError> {
    let (a_ns, b_ns) =
        state::with_state_mut(|s| {
            SeedManager::start_epoch(
                &mut s.snapshot_seed,
                epoch_start_ns,
                epoch_end_ns,
                seed_arg,
            )
        })?;
    state::snapshot_buffer_clear();
    state::set_open_epoch(Some(OpenEpoch {
        epoch_index: index,
        epoch_start_ns,
        epoch_end_ns,
        snapshot_a_ns: a_ns,
        snapshot_b_ns: b_ns,
        a_cursor: None,
        a_complete: false,
        b_cursor: None,
        b_complete: false,
        close_started: false,
        close_cursor: None,
        close_points_accrued: 0,
        close_active: 0,
    }));
    // Opening is only allowed after the epoch's entropy/commit requirements
    // pass. Persist that provenance so old index-0 state that successfully
    // bootstraps with committed S0 is not mistaken for an unsafe legacy open.
    state::with_state_mut(|s| s.snapshot_seed.secure_seed_chain_v1 = true);
    Ok(())
}

async fn management_entropy() -> Result<[u8; 32], SeedError> {
    match ic_cdk::api::management_canister::main::raw_rand().await {
        Ok((bytes,)) => bytes.try_into().map_err(|_| SeedError::InvalidEntropy),
        Err((code, message)) => {
            ic_cdk::println!("[epoch] raw_rand failed ({:?}: {})", code, message);
            Err(SeedError::EntropyUnavailable)
        }
    }
}

/// Synchronously fence legacy state before `post_upgrade` re-registers timers.
/// This closes the interval in which a poll timer or an admin trigger could
/// write points before the asynchronous epoch driver noticed the old seed.
pub fn prepare_legacy_state_after_upgrade(now_ns: u64) {
    if state::legacy_transition_held() {
        return;
    }
    if let Some(open) = state::get_open_epoch() {
        if legacy_open_epoch_requires_review(
            &open,
            state::current_epoch_entropy(),
            state::secure_seed_chain_v1(),
        ) {
            state::set_legacy_transition_held(true);
            state::set_legacy_reseed_pending(false);
            ic_cdk::println!(
                "[epoch] post_upgrade held active legacy epoch {} for admin review",
                open.epoch_index
            );
        }
        return;
    }

    let index = state::current_epoch_index();
    let (season_start, season_end) = state::season_bounds();
    if hold_expired_unopened_epoch(index, now_ns, season_start, season_end, true) {
        return;
    }
    let (scheduled_start, _) = epoch_bounds(index, season_start, season_end);
    if now_ns >= scheduled_start && index > 0 && state::current_epoch_entropy().is_none() {
        state::set_legacy_reseed_pending(true);
    }
}

fn secure_seed_from_history(index: u64, entropy: &[u8; 32]) -> Result<[u8; 32], SeedError> {
    let previous_index = index
        .checked_sub(1)
        .ok_or(SeedError::LegacyHistoryMissing)?;
    let previous_seed =
        state::get_revealed_seed(previous_index).ok_or(SeedError::LegacyHistoryMissing)?;
    let previous_summary = state::epoch_history(previous_index, 1)
        .into_iter()
        .next()
        .ok_or(SeedError::LegacyHistoryMissing)?;
    Ok(sha256(&[
        &previous_seed.seed,
        &summary_hash(&previous_summary),
        entropy,
    ]))
}

/// Resume migration only when an old release left the next epoch unopened.
/// Already-open legacy epochs are held for operator review because their
/// remaining snapshot times are publicly predictable.
async fn resume_legacy_reseed() {
    let _poll_guard = match state::PollGuard::new() {
        Some(guard) => guard,
        None => return,
    };
    let open = state::get_open_epoch();
    if open.is_some() {
        // Remaining legacy snapshot times are public. Pause without rewriting
        // cursors, snapshots, accrued points, commitments, or the epoch index.
        state::set_legacy_transition_held(true);
        state::set_legacy_reseed_pending(false);
        ic_cdk::println!(
            "[epoch] active legacy epoch held for admin review; no captures or close will run"
        );
        return;
    }
    let index = state::current_epoch_index();
    if index == 0 {
        state::set_legacy_reseed_pending(false);
        return;
    }
    let (season_start, season_end) = state::season_bounds();
    let (mut scheduled_start, mut epoch_end) = epoch_bounds(index, season_start, season_end);
    let transition_now = ic_cdk::api::time();
    if transition_now < scheduled_start {
        return;
    }
    if !legacy_reseed_window_allowed(epoch_end, transition_now) {
        // Durable hold, visible via the admin epoch status. Do not synthesize a
        // zero-point close or silently advance the epoch index: an operator must
        // review the missed reward interval and explicitly decide how to resume.
        state::set_legacy_transition_held(true);
        state::set_legacy_reseed_pending(false);
        ic_cdk::println!(
            "[epoch] legacy epoch {} missed its unopened snapshot window; held for admin review without advancing rewards",
            index
        );
        return;
    }

    if state::current_epoch_entropy().is_none() {
        let entropy = match management_entropy().await {
            Ok(entropy) => entropy,
            Err(error) => {
                ic_cdk::println!(
                    "[epoch] secure legacy transition remains pending: {:?}",
                    error
                );
                return;
            }
        };
        // The season end is admin-adjustable. Re-read it after the await so a
        // concurrent shortening cannot open this epoch past its current bound.
        let (season_start, season_end) = state::season_bounds();
        let (_, post_entropy_epoch_end) = epoch_bounds(index, season_start, season_end);
        let transition_now = ic_cdk::api::time();
        if !legacy_reseed_window_allowed(post_entropy_epoch_end, transition_now) {
            state::set_legacy_transition_held(true);
            state::set_legacy_reseed_pending(false);
            ic_cdk::println!(
                "[epoch] legacy epoch {} missed its unopened snapshot window during raw_rand; held for admin review",
                index
            );
            return;
        }
        let seed = match secure_seed_from_history(index, &entropy) {
            Ok(seed) => seed,
            Err(error) => {
                ic_cdk::println!(
                    "[epoch] secure legacy transition remains pending: {:?}",
                    error
                );
                return;
            }
        };
        state::install_reseeded_epoch_seed(seed, entropy);
    }

    // Clear any stale legacy snapshot buffer incrementally after entropy is
    // available. A raw_rand rejection therefore leaves reward/capture state
    // untouched; the persisted pending bit only fences new polls during retry.
    if !state::snapshot_buffer_clear_chunk() {
        return;
    }

    // A retry may resume after a season-bound admin update while snapshot-buffer
    // cleanup spans multiple ticks; the final open must use the latest window.
    let (season_start, season_end) = state::season_bounds();
    (scheduled_start, epoch_end) = epoch_bounds(index, season_start, season_end);
    let transition_now = ic_cdk::api::time();
    if !legacy_reseed_window_allowed(epoch_end, transition_now) {
        state::set_legacy_transition_held(true);
        state::set_legacy_reseed_pending(false);
        return;
    }
    if let Err(error) = open_epoch_at(index, transition_now.max(scheduled_start), epoch_end, None) {
        ic_cdk::println!(
            "[epoch] secure legacy transition remains pending: {:?}",
            error
        );
        return;
    }
    state::set_legacy_transition_held(false);
    state::set_legacy_reseed_pending(false);
}

// ── Periodic driver tick ──

/// The timer callback: run a tick only while the driver is enabled.
pub async fn epoch_driver_tick() {
    if state::epoch_driver_enabled() {
        run_tick().await;
    }
}

/// One state-machine step, regardless of the enabled flag (admin `force_epoch_tick`
/// and the E2E drive this directly). The single-tick guard still applies.
pub async fn run_tick() {
    // RAII guard (AR-S-001): released on every exit path INCLUDING a trap (the
    // dropped future runs destructors), so a panicking tick never halts accrual.
    let _guard = match state::EpochGuard::new() {
        Some(g) => g,
        None => return, // a tick is already in flight
    };
    if state::legacy_transition_held() {
        return;
    }
    let now = ic_cdk::api::time();
    if state::legacy_reseed_pending() {
        resume_legacy_reseed().await;
        return;
    }
    if let Some(open) = state::get_open_epoch() {
        if legacy_open_epoch_requires_review(
            &open,
            state::current_epoch_entropy(),
            state::secure_seed_chain_v1(),
        ) {
            // Persist a poll fence before awaiting the poll guard. Existing
            // in-flight polls drain; subsequent poll calls see the pending bit.
            state::set_legacy_reseed_pending(true);
            resume_legacy_reseed().await;
            return;
        }
    }
    let (season_start, season_end) = state::season_bounds();
    let open = state::get_open_epoch();
    let index = state::current_epoch_index();
    if open.is_none()
        && hold_expired_unopened_epoch(index, now, season_start, season_end, false)
    {
        return;
    }
    match next_action(&open, now, season_start, season_end, index) {
        DriverAction::Idle => {}
        DriverAction::Start => {
            let result = if index > 0 && state::current_epoch_entropy().is_none() {
                state::set_legacy_reseed_pending(true);
                resume_legacy_reseed().await;
                return;
            } else {
                let (scheduled_start, epoch_end) = epoch_bounds(index, season_start, season_end);
                match safe_due_epoch_start(scheduled_start, epoch_end, now) {
                    Some(start_ns) => open_epoch_at(index, start_ns, epoch_end, None),
                    None => {
                        if now >= epoch_end {
                            state::set_legacy_transition_held(true);
                            state::set_legacy_reseed_pending(false);
                        }
                        ic_cdk::println!(
                            "[epoch] epoch {} missed its safe start window; held for admin review without advancing rewards",
                            index
                        );
                        return;
                    }
                }
            };
            if let Err(e) = result {
                ic_cdk::println!("[epoch] start of epoch {} failed: {:?}", index, e);
            }
        }
        DriverAction::CaptureA => capture(Snapshot::A).await,
        DriverAction::CaptureB => capture(Snapshot::B).await,
        DriverAction::Close => close_current_epoch(now).await,
    }
}

// ── Snapshot capture (one chunk per tick) ──

async fn capture(which: Snapshot) {
    let mut open = match state::get_open_epoch() {
        Some(o) => o,
        None => return,
    };
    let ctx = match fetch_context().await {
        Some(c) => c,
        None => return, // a snapshot-wide source was unreachable; retry next tick
    };
    let cursor = match which {
        Snapshot::A => open.a_cursor,
        Snapshot::B => open.b_cursor,
    };
    // Capture in a seed-derived, unpredictable order (POINTS-001 defense-in-depth):
    // an attacker who cannot predict when their principal is captured cannot time a
    // flash deposit to land just before their snapshot chunk. The order is stable
    // for the epoch (the cursor resumes correctly). Fall back to principal order
    // only if no seed is available (should not happen once an epoch is open).
    let chunk = match state::current_epoch_seed() {
        Some(seed) => state::registered_chunk_after_shuffled(&seed, cursor, CAPTURE_CHUNK),
        None => state::registered_chunk_after(cursor, CAPTURE_CHUNK),
    };
    let mut last_captured: Option<Principal> = None;
    let mut hit_error = false;
    for p in &chunk {
        if state::is_excluded(p) {
            // Excluded principals are not captured but still advance the cursor
            // past themselves (they are skipped again at close).
            last_captured = Some(*p);
            continue;
        }
        let raw = match fetch_raw_snapshot(*p, &ctx).await {
            Some(r) => r,
            None => {
                // A per-principal source errored (distinct from a real zero
                // balance). Stop WITHOUT recording a transient 0: the close-time
                // min() would otherwise lock that 0 in and zero a held position
                // for the whole epoch. Resume from the last success next tick and
                // retry this principal.
                hit_error = true;
                break;
            }
        };
        let weights = accrual::snapshot_weights(&accrual::build_snapshot_inputs(&raw, &ctx.prices));
        match which {
            Snapshot::A => state::snapshot_buffer_put(*p, weights),
            Snapshot::B => state::snapshot_buffer_merge_min(*p, weights),
        }
        last_captured = Some(*p);
    }
    let (next_cursor, done) = next_capture_cursor(chunk.len(), last_captured, cursor, hit_error);
    match which {
        Snapshot::A => {
            open.a_cursor = next_cursor;
            open.a_complete = done;
        }
        Snapshot::B => {
            open.b_cursor = next_cursor;
            open.b_complete = done;
        }
    }
    state::set_open_epoch(Some(open));
}

// ── Epoch close (chunked, POINTS-002) ──

/// One `Close` step. The close is CHUNKED: each call processes a bounded batch of
/// principals (`run_close_accrual_chunk`) and persists the resume cursor + running
/// totals into the open epoch. Only once the whole registered set has been closed
/// (`CloseStep::Done`) does it finalize: reveal the seed, append the summary,
/// advance the epoch index, and clear the open epoch. While batches remain it
/// returns early, leaving the epoch open at `DriverAction::Close` so the next tick
/// resumes. This keeps each message well under the 5B-instruction limit and makes
/// the close re-entrant-safe (the single-tick `EPOCH_IN_PROGRESS` guard already
/// serializes ticks) and idempotent (the cursor advances exactly-once per
/// principal, so a re-run never double-credits).
async fn close_current_epoch(now: u64) {
    // Pair the durable `close_started` cutoff with the poll guard: an already
    // running poll makes this close step wait, and new polls are rejected once
    // the first close chunk persists its cutoff. Keep the guard over raw_rand.
    let _poll_guard = match state::PollGuard::new() {
        Some(guard) => guard,
        None => return,
    };
    let stats = match state::run_close_accrual_chunk(now) {
        // Still principals left to close: persist progress (already done inside the
        // chunk) and return. The epoch stays open; `next_action` returns `Close`
        // again next tick and we resume from the persisted cursor.
        state::CloseStep::More => return,
        state::CloseStep::Done(stats) => stats,
    };
    // Re-read the open epoch (the chunk persisted the final cursor/totals into it).
    let open = match state::get_open_epoch() {
        Some(o) => o,
        // Defensive: the close finished but the epoch vanished (e.g. a concurrent
        // forced close). Nothing left to finalize.
        None => return,
    };
    let summary = EpochSummary {
        epoch_index: open.epoch_index,
        epoch_start_ns: open.epoch_start_ns,
        epoch_end_ns: open.epoch_end_ns,
        total_points_all: stats.total_points_all,
        points_accrued_this_epoch: stats.points_accrued,
        active_principals: stats.active_principals,
        registered_principals: stats.registered_principals,
        snapshot_a_ns: open.snapshot_a_ns,
        snapshot_b_ns: open.snapshot_b_ns,
    };
    let hash = summary_hash(&summary);
    // Obtain fresh, unpredictable entropy only after all epoch captures and
    // accrual are complete. On rejection, retain the open epoch and retry next
    // tick; never fall back to publicly derivable entropy.
    let entropy_for_next_epoch = match management_entropy().await {
        Ok(entropy) => entropy,
        Err(error) => {
            ic_cdk::println!("[epoch] close remains pending: {:?}", error);
            return;
        }
    };
    match close_seed_after_entropy(
        Ok(entropy_for_next_epoch),
        &open,
        ic_cdk::api::time(),
        hash,
    ) {
        Ok(revealed) => state::append_revealed_seed(revealed),
        // Unreachable in the live flow (`current_seed` is always `Some` once an
        // epoch is open). If it ever happens, trap to roll the whole close back
        // atomically rather than advance the index with a broken (reused) seed.
        Err(e) => ic_cdk::trap(&format!(
            "[epoch] seed close of epoch {} failed ({:?}); halting to avoid a broken seed chain",
            open.epoch_index, e
        )),
    }
    state::append_epoch_summary(summary);
    state::advance_epoch_index();
    state::set_open_epoch(None);
}

/// Commit the next seed only after entropy is available. Keeping the failure
/// branch here makes an entropy rejection a no-op on the seed chain, leaving the
/// persisted close cursor/points ready for an exact retry after upgrade.
fn close_seed_after_entropy(
    entropy: Result<[u8; 32], SeedError>,
    open: &OpenEpoch,
    now_ns: u64,
    summary_hash: [u8; 32],
) -> Result<crate::snapshot_seed::RevealedSeed, SeedError> {
    let entropy = entropy?;
    state::with_state_mut(|s| {
        SeedManager::close_epoch(
            &mut s.snapshot_seed,
            open.epoch_index,
            open.snapshot_a_ns,
            open.snapshot_b_ns,
            now_ns,
            summary_hash,
            entropy,
        )
    })
}

/// Deterministic hash binding the next seed to this epoch's chain state (spike 0.3).
fn summary_hash(s: &EpochSummary) -> [u8; 32] {
    let mut buf = Vec::with_capacity(64);
    buf.extend_from_slice(&s.epoch_index.to_le_bytes());
    buf.extend_from_slice(&s.total_points_all.to_le_bytes());
    buf.extend_from_slice(&s.registered_principals.to_le_bytes());
    buf.extend_from_slice(&s.points_accrued_this_epoch.to_le_bytes());
    buf.extend_from_slice(&s.snapshot_a_ns.to_le_bytes());
    buf.extend_from_slice(&s.snapshot_b_ns.to_le_bytes());
    sha256(&[&buf])
}

// ── Inter-canister fetch helpers (validated by the PocketIC E2E) ──

/// Snapshot-wide values fetched once per capture (not per principal), including
/// the resolved source-canister ids so the per-principal pass does not re-read them.
struct SnapshotContext {
    prices: SnapshotPrices,
    amm: Option<AmmPool>,
    icusd_ledger: Principal,
    threeusd_ledger: Principal,
    backend: Principal,
    threepool: Principal,
    sp: Option<Principal>,
    amm_canister: Principal,
}

async fn fetch_context() -> Option<SnapshotContext> {
    let backend = state::get_source_canister(SourceId::Backend.tag())?;
    let threepool = state::get_source_canister(SourceId::ThreePool.tag())?;
    let amm_canister = state::get_source_canister(SourceId::Amm.tag())?;
    let sp = state::get_source_canister(SourceId::StabilityPool.tag());
    let icp_rate = fetch_icp_rate(backend).await?;
    let virtual_price = fetch_virtual_price(threepool).await?;
    let threeusd = state::get_asset_ledger(AssetType::ThreeUsd)?;
    let icp = state::get_asset_ledger(AssetType::Icp)?;
    let icusd = state::get_asset_ledger(AssetType::IcUsd)?;
    let amm = fetch_amm_pool(amm_canister, threeusd, icp).await;
    Some(SnapshotContext {
        prices: SnapshotPrices { icp_rate, virtual_price },
        amm,
        icusd_ledger: icusd,
        threeusd_ledger: threeusd,
        backend,
        threepool,
        sp,
        amm_canister,
    })
}

/// Capture one principal's raw balances across all sources. Returns `None` if ANY
/// per-principal inter-canister call ERRORED (transport/canister error), so the
/// caller can retry rather than record a transient 0 (a genuine zero balance is a
/// successful `Some(0)`). The snapshot-wide values (prices, AMM pool, reserves)
/// were already fetched once in `fetch_context`.
async fn fetch_raw_snapshot(p: Principal, ctx: &SnapshotContext) -> Option<RawSnapshot> {
    let vault_debt = fetch_vault_debt(ctx.backend, p).await?;
    let wallet_3usd = fetch_wallet_3usd(ctx.threepool, p).await?;
    let (sp_icusd, sp_3usd) = match ctx.sp {
        Some(c) => fetch_sp_position(c, p, ctx.icusd_ledger, ctx.threeusd_ledger).await?,
        None => (0, 0),
    };
    let amm_user_lp = match &ctx.amm {
        Some(pool) => fetch_amm_lp(ctx.amm_canister, &pool.pool_id, p).await?,
        None => 0,
    };

    let (recorded_icusd, recorded_usdc, recorded_usdt) = state::recorded_3pool_composition(&p);
    let (amm_total_lp, amm_reserve_3usd, amm_reserve_icp) = match &ctx.amm {
        Some(pool) => (pool.total_lp, pool.reserve_3usd, pool.reserve_icp),
        None => (0, 0, 0),
    };
    Some(RawSnapshot {
        vault_debt,
        recorded_icusd,
        recorded_usdc,
        recorded_usdt,
        wallet_3usd,
        sp_icusd,
        sp_3usd,
        amm_user_lp,
        amm_total_lp,
        amm_reserve_3usd,
        amm_reserve_icp,
    })
}

async fn fetch_icp_rate(backend: Principal) -> Option<f64> {
    let res: CallResult<(balances::ProtocolStatus,)> =
        ic_cdk::call(backend, "get_protocol_status", ()).await;
    match res {
        Ok((s,)) if s.last_icp_rate.is_finite() && s.last_icp_rate > 0.0 => Some(s.last_icp_rate),
        Ok((s,)) => {
            // A corrupt (non-finite / non-positive) oracle rate aborts the whole
            // capture to retry next tick, rather than valuing ICP positions wrong.
            ic_cdk::println!("[epoch] get_protocol_status returned bad icp_rate {}; aborting capture", s.last_icp_rate);
            None
        }
        Err((c, m)) => {
            ic_cdk::println!("[epoch] get_protocol_status failed: {:?} {}", c, m);
            None
        }
    }
}

async fn fetch_virtual_price(threepool: Principal) -> Option<u128> {
    let res: CallResult<(balances::PoolStatus,)> =
        ic_cdk::call(threepool, "get_pool_status", ()).await;
    match res {
        Ok((s,)) => Some(s.virtual_price),
        Err((c, m)) => {
            ic_cdk::println!("[epoch] get_pool_status failed: {:?} {}", c, m);
            None
        }
    }
}

async fn fetch_amm_pool(amm: Principal, threeusd: Principal, icp: Principal) -> Option<AmmPool> {
    let res: CallResult<(Vec<balances::PoolInfo>,)> = ic_cdk::call(amm, "get_pools", ()).await;
    match res {
        Ok((pools,)) => pick_amm_pool(&pools, threeusd, icp),
        Err((c, m)) => {
            ic_cdk::println!("[epoch] get_pools failed: {:?} {}", c, m);
            None
        }
    }
}

/// `None` on call error (retry); `Some(debt)` on success (a debt-free principal is
/// `Some(0)`). Same Option contract for all four per-principal fetch helpers.
async fn fetch_vault_debt(backend: Principal, p: Principal) -> Option<u128> {
    let res: CallResult<(Vec<balances::CandidVault>,)> =
        ic_cdk::call(backend, "get_vaults", (Some(p),)).await;
    match res {
        Ok((vaults,)) => Some(
            vaults
                .iter()
                .fold(0u128, |acc, v| acc.saturating_add(v.borrowed_icusd_amount as u128)),
        ),
        Err((c, m)) => {
            ic_cdk::println!("[epoch] get_vaults failed: {:?} {}", c, m);
            None
        }
    }
}

async fn fetch_wallet_3usd(threepool: Principal, p: Principal) -> Option<u128> {
    let account = Account { owner: p, subaccount: None };
    let res: CallResult<(Nat,)> = ic_cdk::call(threepool, "icrc1_balance_of", (account,)).await;
    match res {
        Ok((bal,)) => Some(nat_to_u128(&bal)),
        Err((c, m)) => {
            ic_cdk::println!("[epoch] icrc1_balance_of failed: {:?} {}", c, m);
            None
        }
    }
}

async fn fetch_sp_position(
    sp: Principal,
    p: Principal,
    icusd: Principal,
    threeusd: Principal,
) -> Option<(u128, u128)> {
    let res: CallResult<(Option<balances::UserStabilityPosition>,)> =
        ic_cdk::call(sp, "get_user_position", (Some(p),)).await;
    match res {
        Ok((Some(pos),)) => {
            let bal = |l: &Principal| pos.stablecoin_balances.get(l).copied().unwrap_or(0) as u128;
            Some((bal(&icusd), bal(&threeusd)))
        }
        Ok((None,)) => Some((0, 0)),
        Err((c, m)) => {
            ic_cdk::println!("[epoch] get_user_position failed: {:?} {}", c, m);
            None
        }
    }
}

async fn fetch_amm_lp(amm: Principal, pool_id: &str, p: Principal) -> Option<u128> {
    let res: CallResult<(u128,)> =
        ic_cdk::call(amm, "get_lp_balance", (pool_id.to_string(), p)).await;
    match res {
        Ok((lp,)) => Some(lp),
        Err((c, m)) => {
            ic_cdk::println!("[epoch] get_lp_balance failed: {:?} {}", c, m);
            None
        }
    }
}

/// candid `Nat` -> `u128`, failing CLOSED to 0 if the balance does not fit in
/// `u128`. Balances never exceed `u128` in practice; a value that does (a corrupt
/// or hostile ledger) must value to 0, NOT `u128::MAX` — a fail-open MAX would
/// dwarf every other principal's points and route the whole pool to one reading.
fn nat_to_u128(n: &Nat) -> u128 {
    let digits: String = n.to_string().chars().filter(|c| c.is_ascii_digit()).collect();
    digits.parse::<u128>().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const E: u64 = EPOCH_DURATION_NS;

    #[test]
    fn epoch_bounds_full_partial_and_offset() {
        // Full epochs from season start 0.
        assert_eq!(epoch_bounds(0, 0, 100 * E), (0, E));
        assert_eq!(epoch_bounds(1, 0, 100 * E), (E, 2 * E));
        // Season-start offset carries through.
        assert_eq!(epoch_bounds(0, 1_000, 1_000 + 100 * E), (1_000, 1_000 + E));
        // The last epoch is truncated at season end.
        assert_eq!(epoch_bounds(1, 0, E + 100), (E, E + 100));
    }

    #[test]
    fn legacy_transition_rederives_from_history_plus_fresh_entropy() {
        state::init_state(None, Principal::anonymous());
        let summary = EpochSummary {
            epoch_index: 0,
            epoch_start_ns: 0,
            epoch_end_ns: E,
            total_points_all: 123,
            points_accrued_this_epoch: 100,
            active_principals: 1,
            registered_principals: 1,
            snapshot_a_ns: E / 4,
            snapshot_b_ns: E * 3 / 4,
        };
        let previous_seed = [7u8; 32];
        state::append_epoch_summary(summary.clone());
        state::append_revealed_seed(crate::snapshot_seed::RevealedSeed {
            epoch_index: 0,
            seed: previous_seed,
            snapshot_time_a_ns: summary.snapshot_a_ns,
            snapshot_time_b_ns: summary.snapshot_b_ns,
            revealed_at_ns: E,
            derivation_entropy: None,
        });

        let entropy = [19u8; 32];
        let migrated = secure_seed_from_history(1, &entropy).unwrap();
        assert_eq!(
            migrated,
            sha256(&[&previous_seed, &summary_hash(&summary), &entropy])
        );
        assert_ne!(
            migrated,
            sha256(&[&previous_seed, &summary_hash(&summary)]),
            "the legacy public derivation cannot survive into the new epoch"
        );
        state::install_reseeded_epoch_seed(migrated, entropy);
        assert_eq!(
            state::get_pending_commit(),
            crate::snapshot_seed::commitment(&migrated)
        );
    }

    #[test]
    fn no_open_epoch_rebases_any_nonexpired_window_and_holds_expired_window() {
        let scheduled_start = E;
        let end = E * 2;
        // The state-machine trigger remains due while standby, so the Start
        // handler must enforce the end/cutoff independently before opening.
        assert_eq!(
            next_action(&None, end, 0, E * 3, 1),
            DriverAction::Start
        );
        assert_eq!(
            safe_due_epoch_start(scheduled_start, end, scheduled_start),
            Some(scheduled_start)
        );
        let late = scheduled_start + 1_000;
        assert_eq!(safe_due_epoch_start(scheduled_start, end, late), Some(late));
        assert_eq!(safe_due_epoch_start(scheduled_start, end, end), None);
        assert_eq!(safe_due_epoch_start(scheduled_start, end, end + 1), None);
        assert_eq!(safe_due_epoch_start(scheduled_start, end, end - 1), Some(end - 1));
    }

    #[test]
    fn post_upgrade_holds_active_legacy_epoch_before_ingress() {
        state::init_state(None, Principal::anonymous());
        let mut open = oe(true, false);
        open.epoch_index = 4;
        open.epoch_start_ns = E * 4;
        open.epoch_end_ns = E * 5;
        open.close_started = true;
        open.close_cursor = Some(pr(8));
        open.close_points_accrued = 99;
        state::with_state_mut(|s| {
            s.current_epoch_index = 4;
            s.snapshot_seed.current_seed = Some([7; 32]);
            s.snapshot_seed.current_entropy = None;
        });
        state::set_open_epoch(Some(open.clone()));

        prepare_legacy_state_after_upgrade(E * 4 + 100);

        let status = state::epoch_status();
        assert!(status.legacy_transition_held);
        assert!(!status.legacy_reseed_pending);
        assert_eq!(status.current_epoch_index, 4);
        assert_eq!(status.open_epoch, Some(open.clone()));
        assert!(state::try_poll_guard().is_none());

        // A due driver tick after upgrade must not sample or settle this
        // predictable legacy epoch while the review hold is active.
        let seed_before = state::with_state(|s| s.snapshot_seed.clone());
        struct NoopWake;
        impl std::task::Wake for NoopWake {
            fn wake(self: std::sync::Arc<Self>) {}
        }
        let waker = std::task::Waker::from(std::sync::Arc::new(NoopWake));
        let mut context = std::task::Context::from_waker(&waker);
        let mut tick = Box::pin(run_tick());
        assert!(std::future::Future::poll(tick.as_mut(), &mut context).is_ready());
        drop(tick);
        assert_eq!(state::get_open_epoch(), Some(open));
        assert_eq!(state::current_epoch_index(), 4);
        assert!(state::epoch_history(0, u64::MAX).is_empty());
        assert_eq!(state::revealed_seed_count(), 0);
        assert_eq!(state::with_state(|s| s.snapshot_seed.clone()), seed_before);

        // Older singleton blobs default the scheme marker to false. Even an
        // already-open epoch zero is held; fresh init sets the marker true.
        state::init_state(None, Principal::anonymous());
        let legacy_zero = oe(true, false);
        state::with_state_mut(|s| {
            s.snapshot_seed.secure_seed_chain_v1 = false;
            s.snapshot_seed.current_seed = Some([6; 32]);
        });
        state::set_open_epoch(Some(legacy_zero.clone()));
        prepare_legacy_state_after_upgrade(100);
        let status = state::epoch_status();
        assert!(status.legacy_transition_held);
        assert_eq!(status.open_epoch, Some(legacy_zero));
        assert!(state::try_poll_guard().is_none());
    }

    #[test]
    fn post_upgrade_fences_nonexpired_legacy_and_holds_any_expired_unopened_epoch() {
        state::init_state(
            Some(crate::types::InitArgs {
                season_start_ns: Some(0),
                season_end_ns: Some(3 * E),
                ..Default::default()
            }),
            Principal::anonymous(),
        );
        state::with_state_mut(|s| {
            s.current_epoch_index = 1;
            s.snapshot_seed.current_seed = Some([7; 32]);
            s.snapshot_seed.current_entropy = None;
        });

        prepare_legacy_state_after_upgrade(E + 100);
        assert!(state::legacy_reseed_pending());
        assert!(!state::legacy_transition_held());
        assert!(state::try_poll_guard().is_none());

        // A snapshot with no remaining epoch window is held for operator review;
        // no summary is fabricated and no epoch index is consumed.
        state::init_state(
            Some(crate::types::InitArgs {
                season_start_ns: Some(0),
                season_end_ns: Some(3 * E),
                ..Default::default()
            }),
            Principal::anonymous(),
        );
        state::with_state_mut(|s| {
            s.current_epoch_index = 1;
            s.snapshot_seed.current_seed = Some([7; 32]);
            s.snapshot_seed.current_entropy = None;
        });
        prepare_legacy_state_after_upgrade(2 * E);
        let status = state::epoch_status();
        assert!(status.legacy_transition_held);
        assert!(!status.legacy_reseed_pending);
        assert_eq!(status.current_epoch_index, 1);
        assert!(status.open_epoch.is_none());
        assert!(state::epoch_history(0, u64::MAX).is_empty());
        assert_eq!(state::revealed_seed_count(), 0);
        assert!(state::try_poll_guard().is_none());

        // Expiry is independently a review condition; it applies even when
        // the epoch already has secure entropy and needs no legacy reseed.
        state::init_state(
            Some(crate::types::InitArgs {
                season_start_ns: Some(0),
                season_end_ns: Some(3 * E),
                ..Default::default()
            }),
            Principal::anonymous(),
        );
        state::with_state_mut(|s| {
            s.current_epoch_index = 1;
            s.snapshot_seed.current_seed = Some([8; 32]);
            s.snapshot_seed.current_entropy = Some([9; 32]);
        });
        prepare_legacy_state_after_upgrade(2 * E);
        let status = state::epoch_status();
        assert!(status.legacy_transition_held);
        assert!(!status.legacy_reseed_pending);
        assert_eq!(status.current_epoch_index, 1);
        assert!(status.open_epoch.is_none());
        assert!(state::epoch_history(0, u64::MAX).is_empty());
        assert_eq!(state::revealed_seed_count(), 0);
        assert!(state::try_poll_guard().is_none());
    }

    #[test]
    fn expired_unopened_epoch_zero_requires_review_even_without_commit() {
        // An upgrade with a configured but unopened epoch-zero window must pause
        // for review after that window expires, even when S0 was never committed.
        state::init_state(
            Some(crate::types::InitArgs {
                season_start_ns: Some(0),
                season_end_ns: Some(E),
                ..Default::default()
            }),
            Principal::anonymous(),
        );
        assert!(!hold_expired_unopened_epoch(0, E, 0, E, false));
        prepare_legacy_state_after_upgrade(E);
        assert!(state::legacy_transition_held());
        assert!(!state::legacy_reseed_pending());
        assert_eq!(state::current_epoch_index(), 0);
        assert!(state::get_open_epoch().is_none());
        assert!(state::epoch_history(0, u64::MAX).is_empty());
        assert_eq!(state::revealed_seed_count(), 0);
        assert!(state::try_poll_guard().is_none());

        let seed = [10; 32];
        state::init_state(
            Some(crate::types::InitArgs {
                snapshot_seed_commit: Some(crate::snapshot_seed::commitment(&seed)),
                season_start_ns: Some(0),
                season_end_ns: Some(3 * E),
                ..Default::default()
            }),
            Principal::anonymous(),
        );
        prepare_legacy_state_after_upgrade(3 * E);
        let status = state::epoch_status();
        assert!(status.legacy_transition_held);
        assert!(!status.legacy_reseed_pending);
        assert_eq!(status.current_epoch_index, 0);
        assert!(status.open_epoch.is_none());
        assert!(state::epoch_history(0, u64::MAX).is_empty());
        assert_eq!(state::revealed_seed_count(), 0);

        // A committed epoch zero whose window is still in the future remains
        // available for the normal operator bootstrap when its start arrives.
        state::init_state(
            Some(crate::types::InitArgs {
                snapshot_seed_commit: Some(crate::snapshot_seed::commitment(&seed)),
                season_start_ns: Some(2 * E),
                season_end_ns: Some(4 * E),
                ..Default::default()
            }),
            Principal::anonymous(),
        );
        prepare_legacy_state_after_upgrade(2 * E - 1);
        assert!(!state::legacy_transition_held());
        assert!(!state::legacy_reseed_pending());
        assert_eq!(state::current_epoch_index(), 0);
        assert!(state::get_open_epoch().is_none());
    }

    #[test]
    fn runtime_holds_expired_unopened_epoch_after_season_shortening() {
        state::init_state(
            Some(crate::types::InitArgs {
                season_start_ns: Some(0),
                season_end_ns: Some(3 * E),
                ..Default::default()
            }),
            Principal::anonymous(),
        );
        state::with_state_mut(|s| {
            s.current_epoch_index = 1;
            s.snapshot_seed.current_seed = Some([8; 32]);
            s.snapshot_seed.current_entropy = Some([9; 32]);
            // The admin shortens the season so it ends before epoch 1 starts.
            s.season_end_ns = E / 2;
        });

        let (season_start, season_end) = state::season_bounds();
        assert_eq!(epoch_bounds(1, season_start, season_end), (E, E / 2));
        assert_eq!(
            next_action(&None, E / 2, season_start, season_end, 1),
            DriverAction::Idle,
            "the ordinary start action is unreachable after the schedule is clipped"
        );
        assert!(!hold_expired_unopened_epoch(
            1,
            E / 2 - 1,
            season_start,
            season_end,
            false
        ));
        assert!(!state::legacy_transition_held());

        assert!(hold_expired_unopened_epoch(
            1,
            E / 2,
            season_start,
            season_end,
            false
        ));
        let status = state::epoch_status();
        assert!(status.legacy_transition_held);
        assert!(!status.legacy_reseed_pending);
        assert_eq!(status.current_epoch_index, 1);
        assert!(status.open_epoch.is_none());
        assert!(state::epoch_history(0, u64::MAX).is_empty());
        assert_eq!(state::revealed_seed_count(), 0);
    }

    #[test]
    fn active_and_closing_legacy_epochs_require_review_without_rewriting_state() {
        let mut epoch = oe(true, true);
        epoch.epoch_index = 4;
        epoch.epoch_start_ns = E;
        epoch.epoch_end_ns = E * 2;
        epoch.a_cursor = Some(pr(1));
        epoch.a_complete = true;
        epoch.b_cursor = Some(pr(2));
        epoch.b_complete = false;
        epoch.close_started = true;
        epoch.close_cursor = Some(pr(3));
        epoch.close_points_accrued = 99;
        epoch.close_active = 7;
        let preserved = epoch.clone();
        assert!(legacy_open_epoch_requires_review(&epoch, None, true));
        assert_eq!(epoch, preserved, "review detection leaves an active epoch unchanged");
        epoch.close_started = false;
        assert!(legacy_open_epoch_requires_review(&epoch, None, true));
        assert_eq!(epoch.epoch_start_ns, preserved.epoch_start_ns);
        assert_eq!(epoch.snapshot_a_ns, preserved.snapshot_a_ns);
        assert_eq!(epoch.snapshot_b_ns, preserved.snapshot_b_ns);
        assert_eq!(epoch.close_cursor, preserved.close_cursor);
        assert_eq!(epoch.close_points_accrued, preserved.close_points_accrued);
        assert!(!legacy_open_epoch_requires_review(&epoch, Some([9; 32]), true));
        assert!(!legacy_reseed_window_allowed(epoch.epoch_end_ns, epoch.epoch_end_ns));
    }

    #[test]
    fn old_open_epoch_zero_is_held_but_fresh_precommitted_epoch_zero_is_not() {
        let epoch_zero = oe(true, false);
        assert!(legacy_open_epoch_requires_review(&epoch_zero, None, false));
        assert!(
            !legacy_open_epoch_requires_review(&epoch_zero, None, true),
            "fresh epoch 0 is authorized by S0's init-time commitment"
        );

        let s0 = [4; 32];
        state::init_state(
            Some(crate::types::InitArgs {
                snapshot_seed_commit: Some(crate::snapshot_seed::commitment(&s0)),
                season_start_ns: Some(0),
                season_end_ns: Some(3 * E),
                ..Default::default()
            }),
            Principal::anonymous(),
        );
        state::with_state_mut(|s| s.snapshot_seed.secure_seed_chain_v1 = false);
        start_season(s0, 1).expect("old but committed bootstrap remains valid");
        assert!(state::secure_seed_chain_v1());
        prepare_legacy_state_after_upgrade(100);
        assert!(
            !state::legacy_transition_held(),
            "a secure bootstrap on an upgraded install must survive a second upgrade"
        );
    }

    #[test]
    fn entropy_failure_keeps_closed_epoch_pending_and_retryable() {
        state::init_state(None, Principal::anonymous());
        let mut open = oe(true, true);
        open.epoch_index = 3;
        open.epoch_start_ns = E * 3;
        open.epoch_end_ns = E * 4;
        open.close_started = true;
        open.close_cursor = Some(pr(8));
        open.close_points_accrued = 777;
        state::with_state_mut(|s| {
            s.current_epoch_index = 3;
            s.snapshot_seed.current_seed = Some([7; 32]);
            s.snapshot_seed.current_entropy = None; // migrated legacy seed
        });
        state::set_open_epoch(Some(open.clone()));

        let seed_before = state::with_state(|s| s.snapshot_seed.clone());
        let err = close_seed_after_entropy(
            Err(SeedError::EntropyUnavailable),
            &open,
            E * 4,
            [11; 32],
        )
        .unwrap_err();
        assert_eq!(err, SeedError::EntropyUnavailable);
        assert_eq!(state::with_state(|s| s.snapshot_seed.clone()), seed_before);
        assert_eq!(state::current_epoch_index(), 3);
        assert_eq!(state::get_open_epoch(), Some(open.clone()));
        assert!(state::try_poll_guard().is_none());

        // The same completed close can retry with a later raw_rand response;
        // it does not repeat accrual or alter the legacy epoch being revealed.
        let entropy = [19; 32];
        let revealed = close_seed_after_entropy(Ok(entropy), &open, E * 4 + 1, [11; 32])
            .expect("entropy retry should commit the next seed");
        assert_eq!(revealed.epoch_index, 3);
        assert_eq!(revealed.seed, [7; 32]);
        assert_eq!(revealed.derivation_entropy, None);
        let expected_next = sha256(&[&[7; 32], &[11; 32], &entropy]);
        assert_eq!(state::current_epoch_seed(), Some(expected_next));
        assert_eq!(state::current_epoch_entropy(), Some(entropy));
        assert_eq!(state::get_pending_commit(), crate::snapshot_seed::commitment(&expected_next));
        assert_eq!(state::current_epoch_index(), 3, "index advances after reveal is persisted");
    }

    fn oe(a_complete: bool, b_complete: bool) -> OpenEpoch {
        OpenEpoch {
            epoch_index: 0,
            epoch_start_ns: 0,
            epoch_end_ns: 1_000,
            snapshot_a_ns: 100,
            snapshot_b_ns: 500,
            a_cursor: None,
            a_complete,
            b_cursor: None,
            b_complete,
            close_started: false,
            close_cursor: None,
            close_points_accrued: 0,
            close_active: 0,
        }
    }

    #[test]
    fn open_epoch_captures_a_then_b_then_closes() {
        assert_eq!(next_action(&Some(oe(false, false)), 99, 0, 2_000, 0), DriverAction::Idle);
        assert_eq!(next_action(&Some(oe(false, false)), 100, 0, 2_000, 0), DriverAction::CaptureA);
        assert_eq!(next_action(&Some(oe(true, false)), 499, 0, 2_000, 0), DriverAction::Idle);
        assert_eq!(next_action(&Some(oe(true, false)), 500, 0, 2_000, 0), DriverAction::CaptureB);
        assert_eq!(next_action(&Some(oe(true, true)), 999, 0, 2_000, 0), DriverAction::Idle);
        assert_eq!(next_action(&Some(oe(true, true)), 1_000, 0, 2_000, 0), DriverAction::Close);
    }

    #[test]
    fn epoch_zero_is_idle_until_start_season_opens_it() {
        // No open epoch and index 0: the driver waits for the operator bootstrap.
        assert_eq!(next_action(&None, u64::MAX, 0, 2_000, 0), DriverAction::Idle);
    }

    #[test]
    fn subsequent_epoch_starts_when_due_and_in_season() {
        // Index 1, now at epoch-1 start, well inside the season -> Start.
        assert_eq!(next_action(&None, E, 0, 100 * E, 1), DriverAction::Start);
        // Before epoch-1 start -> Idle.
        assert_eq!(next_action(&None, E - 1, 0, 100 * E, 1), DriverAction::Idle);
        // Next epoch's start is at/after season end -> season over -> Idle.
        assert_eq!(next_action(&None, u64::MAX, 0, E, 1), DriverAction::Idle);
    }

    use crate::source_types::balances::PoolInfo;

    fn pr(n: u8) -> Principal {
        Principal::from_slice(&[n, n, n, n, n])
    }
    fn pool(id: &str, ta: Principal, tb: Principal, ra: u128, rb: u128, lp: u128) -> PoolInfo {
        PoolInfo {
            pool_id: id.into(),
            token_a: ta,
            token_b: tb,
            reserve_a: ra,
            reserve_b: rb,
            total_lp_shares: lp,
        }
    }

    #[test]
    fn pick_amm_pool_orients_reserves_when_token_a_is_3usd() {
        let (three, icp) = (pr(1), pr(2));
        let got = pick_amm_pool(&[pool("x", three, icp, 100, 200, 50)], three, icp).unwrap();
        assert_eq!(
            got,
            AmmPool { pool_id: "x".into(), reserve_3usd: 100, reserve_icp: 200, total_lp: 50 }
        );
    }

    #[test]
    fn pick_amm_pool_orients_reserves_when_token_b_is_3usd() {
        let (three, icp) = (pr(1), pr(2));
        let got = pick_amm_pool(&[pool("y", icp, three, 200, 100, 50)], three, icp).unwrap();
        assert_eq!(got.reserve_3usd, 100);
        assert_eq!(got.reserve_icp, 200);
    }

    #[test]
    fn pick_amm_pool_none_when_pair_absent() {
        let (three, icp) = (pr(1), pr(2));
        assert!(pick_amm_pool(&[pool("z", pr(3), pr(4), 1, 1, 1)], three, icp).is_none());
    }

    // ── nat_to_u128: a corrupt/oversized ledger balance must fail CLOSED ──

    #[test]
    fn nat_to_u128_converts_normal_values() {
        assert_eq!(nat_to_u128(&Nat::from(0u64)), 0);
        assert_eq!(nat_to_u128(&Nat::from(12_345u64)), 12_345);
        assert_eq!(nat_to_u128(&Nat::from(u128::MAX)), u128::MAX);
    }

    #[test]
    fn nat_to_u128_saturates_overflow_to_zero_not_max() {
        // A balance that does not fit in u128 must value to 0 (fail CLOSED), never
        // u128::MAX: a fail-OPEN MAX would dwarf every other principal's points and
        // hand the whole airdrop pool to one corrupt reading. Real ledgers never
        // return this; the guard is defense-in-depth for a corrupt/hostile source.
        let over = Nat::from(u128::MAX) + Nat::from(1u8);
        assert_eq!(nat_to_u128(&over), 0);
    }

    // ── capture cursor/completion decision (the F2 resume-on-error logic) ──

    #[test]
    fn next_capture_cursor_completes_on_short_chunk_with_no_error() {
        // A short chunk (fewer than CAPTURE_CHUNK) fully captured -> snapshot done.
        let (cursor, done) = next_capture_cursor(10, Some(pr(5)), None, false);
        assert!(done);
        assert_eq!(cursor, None);
    }

    #[test]
    fn next_capture_cursor_advances_on_full_chunk() {
        // A full chunk with no error -> not done, resume after the last captured.
        let (cursor, done) =
            next_capture_cursor(CAPTURE_CHUNK as usize, Some(pr(7)), None, false);
        assert!(!done);
        assert_eq!(cursor, Some(pr(7)));
    }

    #[test]
    fn next_capture_cursor_does_not_complete_when_an_error_was_hit() {
        // Even a short chunk must NOT complete if a per-principal fetch errored:
        // resume after the last success so the failed principal is retried, and a
        // transient 0 is never locked in by the close-time min().
        let (cursor, done) = next_capture_cursor(3, Some(pr(2)), None, true);
        assert!(!done);
        assert_eq!(cursor, Some(pr(2)));
    }

    #[test]
    fn next_capture_cursor_holds_position_when_first_principal_errors() {
        // No principal captured (first one errored): leave the cursor unchanged so
        // the same chunk is retried from the start next tick.
        let (cursor, done) = next_capture_cursor(3, None, Some(pr(9)), true);
        assert!(!done);
        assert_eq!(cursor, Some(pr(9)));
    }

    // ── start_season requires a committed H0 (commit-reveal integrity) ──

    fn init_season(commit: Option<[u8; 32]>) {
        crate::state::init_state(
            Some(crate::types::InitArgs {
                admin: Some(pr(9)),
                season_start_ns: Some(0),
                season_end_ns: Some(1_000_000),
                snapshot_seed_commit: commit,
                ..Default::default()
            }),
            pr(9),
        );
    }

    #[test]
    fn start_season_rejects_uncommitted_seed() {
        init_season(None); // H0 never committed
        assert_eq!(start_season([7u8; 32], 1), Err(StartSeasonError::NotCommitted));
        assert!(state::get_open_epoch().is_none(), "no epoch may open uncommitted");
    }

    #[test]
    fn start_season_opens_epoch_zero_against_committed_h0() {
        let s0 = [7u8; 32];
        init_season(Some(crate::snapshot_seed::commitment(&s0)));
        assert_eq!(start_season(s0, 1), Ok(()));
        assert_eq!(state::get_open_epoch().expect("epoch 0 open").epoch_index, 0);
    }

    #[test]
    fn start_season_rejects_seed_not_matching_commit() {
        let s0 = [7u8; 32];
        init_season(Some(crate::snapshot_seed::commitment(&s0)));
        // A seed that does not hash to the committed H0 is rejected (no re-roll).
        assert_eq!(
            start_season([8u8; 32], 1),
            Err(StartSeasonError::Seed(SeedError::CommitMismatch))
        );
        assert!(state::get_open_epoch().is_none());
    }
}
