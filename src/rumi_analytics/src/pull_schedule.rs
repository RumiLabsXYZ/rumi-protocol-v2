//! Wave-9d DoS hardening (DOS-010): per-source pull schedule.
//!
//! Pre-Wave-9d the pull cycle ran every 60s and fired all 9 sources
//! through `futures::join!`, producing a synchronized burst of inter-
//! canister calls every minute. Wave-9d replaces that with a per-source
//! `next_pull_at_ns` schedule walked by a fast tick. Each source fires
//! once per `PULL_CYCLE_PERIOD_NS` window — they just no longer all fire
//! together.
//!
//! Cycle-burn tuning (2026-07-02): the per-source window widened from 60s
//! to 300s (default) to cut the inter-canister call rate ~5x, since the
//! protocol produces only a handful of events per day and 60s polling
//! spent almost every call learning "nothing new". The window and the
//! walker tick are both runtime-overridable by the admin (see
//! `effective_period_ns` / `effective_tick_secs` and `set_pull_schedule`)
//! so cadence can be retuned without a redeploy. The initial source offset
//! is now DERIVED from the effective period (`source_offset_ns`) rather
//! than a fixed constant, so the stagger stays even at any window size.
//!
//! All logic in this module is pure (no `ic_cdk::api::time()`, no
//! state mutation) so it is unit-testable from
//! `tests/audit_pocs_dos_010_stagger.rs`.
//!
//! See `audit-reports/2026-04-22-28e9896/findings.json` finding DOS-010
//! and `audit-reports/2026-04-22-28e9896/remediation-plan.md`
//! §"Wave 9 — DoS hardening".

use std::collections::HashMap;

use crate::storage::cursors;

const SEC_NANOS: u64 = 1_000_000_000;

/// How often the schedule walker fires (default). A due source is picked
/// up within one tick of becoming due. Runtime-overridable via
/// `effective_tick_secs`; the live value re-arms the walker timer.
pub const PULL_CYCLE_TICK_SECS: u64 = 30;

/// How often each source fires (default window, in ns). Widened from 60s
/// to 300s to reduce the inter-canister poll rate. Runtime-overridable via
/// `effective_period_ns`. Surfaced (indirectly) through `last_pull_cycle_ns`
/// and the per-cursor `last_success_ns` freshness fields.
pub const PULL_CYCLE_PERIOD_NS: u64 = 300 * SEC_NANOS;

/// Number of scheduled sources. The per-source offset derives from this so
/// the sources spread evenly across whatever window is in effect.
pub const N_SOURCES: u64 = ALL_SOURCE_IDS.len() as u64;

// Admin-setter bounds (see `set_pull_schedule`). Kept here so the pure
// logic and the endpoint validation share one source of truth.
/// Minimum accepted pull period. The 300s floor preserves the tuned call rate;
/// runtime overrides may trade freshness for lower burn, but cannot restore
/// the former 60s cadence or create a hotter loop.
pub const MIN_PERIOD_SECS: u64 = 300;
/// Maximum accepted pull period (1 hour). Beyond this analytics freshness
/// degrades past usefulness.
pub const MAX_PERIOD_SECS: u64 = 3600;
/// Minimum accepted walker tick.
pub const MIN_TICK_SECS: u64 = 5;
/// Maximum accepted walker tick.
pub const MAX_TICK_SECS: u64 = 300;

/// Maximum number of adjacent source slots a single walker tick may collect.
/// The scheduler's DOS-010 guarantee is one or two inter-canister calls per
/// tick, never a reconstructed all-source burst.
pub const MAX_SOURCES_PER_TICK: u64 = 2;

/// Schedule-layout version. Bumping this forces a one-time re-spread of the
/// persisted schedule on the next upgrade (see `timers::setup_timers`). It
/// exists so the 60s→300s window change re-staggers the deadlines that were
/// seeded under the old window, instead of leaving all sources bunched (or,
/// worse, all firing together every window because their old deadlines are
/// stale after the upgrade).
pub const CURRENT_SCHEDULE_LAYOUT: u32 = 2;

/// Validate a runtime cadence using the same bounds enforced by the admin
/// endpoint. Kept pure so boundary behavior can be proved without an IC
/// execution environment.
pub fn validate_schedule_config(period_secs: u64, tick_secs: u64) -> Result<(), String> {
    if !(MIN_PERIOD_SECS..=MAX_PERIOD_SECS).contains(&period_secs) {
        return Err(format!(
            "period_secs must be in [{MIN_PERIOD_SECS}, {MAX_PERIOD_SECS}]"
        ));
    }
    if !(MIN_TICK_SECS..=MAX_TICK_SECS).contains(&tick_secs) {
        return Err(format!(
            "tick_secs must be in [{MIN_TICK_SECS}, {MAX_TICK_SECS}]"
        ));
    }
    if tick_secs > period_secs {
        return Err("tick_secs must be <= period_secs".to_string());
    }
    let max_stagger_tick_secs = max_stagger_preserving_tick_secs(period_secs);
    if tick_secs > max_stagger_tick_secs {
        return Err(format!(
            "tick_secs must be <= {max_stagger_tick_secs} for period_secs={period_secs} \
             to keep at most {MAX_SOURCES_PER_TICK} sources due per tick"
        ));
    }
    Ok(())
}

// Source ids. Tailers reuse their CURSOR_ID so the schedule key matches
// the cursor metadata key. Source 0 is synthetic (supply cache has no
// cursor).

pub const SOURCE_ID_SUPPLY_CACHE: u8 = 0;
pub const SOURCE_ID_BACKEND_EVENTS: u8 = cursors::CURSOR_ID_BACKEND_EVENTS;
pub const SOURCE_ID_3POOL_SWAPS: u8 = cursors::CURSOR_ID_3POOL_SWAPS;
pub const SOURCE_ID_3POOL_LIQUIDITY: u8 = cursors::CURSOR_ID_3POOL_LIQUIDITY;
pub const SOURCE_ID_3POOL_BLOCKS: u8 = cursors::CURSOR_ID_3POOL_BLOCKS;
pub const SOURCE_ID_AMM_SWAPS: u8 = cursors::CURSOR_ID_AMM_SWAPS;
pub const SOURCE_ID_STABILITY_EVENTS: u8 = cursors::CURSOR_ID_STABILITY_EVENTS;
pub const SOURCE_ID_ICUSD_BLOCKS: u8 = cursors::CURSOR_ID_ICUSD_BLOCKS;
pub const SOURCE_ID_AMM_LIQUIDITY: u8 = cursors::CURSOR_ID_AMM_LIQUIDITY;

/// All source ids in the order they should be seeded with offsets.
/// Order matters: `seed_initial_schedule` assigns offset = index ×
/// `source_offset_ns(period_ns)`, so reordering this list shifts the
/// staggering pattern.
pub const ALL_SOURCE_IDS: &[u8] = &[
    SOURCE_ID_SUPPLY_CACHE,
    SOURCE_ID_BACKEND_EVENTS,
    SOURCE_ID_3POOL_SWAPS,
    SOURCE_ID_3POOL_LIQUIDITY,
    SOURCE_ID_3POOL_BLOCKS,
    SOURCE_ID_AMM_SWAPS,
    SOURCE_ID_STABILITY_EVENTS,
    SOURCE_ID_ICUSD_BLOCKS,
    SOURCE_ID_AMM_LIQUIDITY,
];

/// Effective per-source window in ns: the admin override (in seconds) if a
/// positive one is set, else the compiled-in default `PULL_CYCLE_PERIOD_NS`.
pub fn effective_period_ns(override_secs: Option<u64>) -> u64 {
    match override_secs {
        Some(secs) if secs > 0 => secs.saturating_mul(SEC_NANOS),
        _ => PULL_CYCLE_PERIOD_NS,
    }
}

/// Effective walker tick in seconds: the admin override if a positive one
/// is set, else the compiled-in default `PULL_CYCLE_TICK_SECS`.
pub fn effective_tick_secs(override_secs: Option<u64>) -> u64 {
    match override_secs {
        Some(secs) if secs > 0 => secs,
        _ => PULL_CYCLE_TICK_SECS,
    }
}

/// Even spacing between adjacent source slots for a window of `period_ns`.
/// Deriving the offset from the period keeps the 9 sources spread across
/// the whole window at any (retuned) cadence. With the 300s default this is
/// ~33s, which lands each source on its own 30s walker tick.
pub fn source_offset_ns(period_ns: u64) -> u64 {
    period_ns / N_SOURCES.max(1)
}

/// Largest whole-second walker tick that preserves the 1-2 source-per-tick
/// stagger for a period measured in seconds. Floor before multiplying so the
/// bound stays conservative when the source spacing is fractional.
pub fn max_stagger_preserving_tick_secs(period_secs: u64) -> u64 {
    (period_secs / N_SOURCES.max(1)).saturating_mul(MAX_SOURCES_PER_TICK)
}

/// Build a fresh schedule with sources spread across the `period_ns` window.
/// Source at index N gets deadline `now + N * source_offset_ns(period_ns)`.
pub fn seed_initial_schedule(now_ns: u64, sources: &[u8], period_ns: u64) -> HashMap<u8, u64> {
    let offset = source_offset_ns(period_ns);
    let mut schedule = HashMap::with_capacity(sources.len());
    for (idx, &source_id) in sources.iter().enumerate() {
        let deadline = now_ns.saturating_add((idx as u64) * offset);
        schedule.insert(source_id, deadline);
    }
    schedule
}

/// Re-seed on `post_upgrade`: copy every entry from `existing` verbatim
/// (preserving in-flight offsets) and seed any source missing from
/// `existing` at its index offset relative to `now_ns`. Never overwrites
/// an existing entry.
pub fn reseed_preserving(
    now_ns: u64,
    existing: &HashMap<u8, u64>,
    sources: &[u8],
    period_ns: u64,
) -> HashMap<u8, u64> {
    let offset = source_offset_ns(period_ns);
    let mut merged = existing.clone();
    for (idx, &source_id) in sources.iter().enumerate() {
        merged
            .entry(source_id)
            .or_insert_with(|| now_ns.saturating_add((idx as u64) * offset));
    }
    merged
}

/// Return source ids whose deadline has passed and that should fire
/// this tick. Sources missing from `schedule` are treated as due (so a
/// newly added source fires on its next tick instead of being silently
/// skipped until the next reseed).
pub fn compute_due_sources(now_ns: u64, schedule: &HashMap<u8, u64>, sources: &[u8]) -> Vec<u8> {
    sources
        .iter()
        .copied()
        .filter(|id| match schedule.get(id) {
            Some(deadline) => *deadline <= now_ns,
            None => true,
        })
        .collect()
}

/// Bump each fired source's deadline by `period_ns` from the tick's
/// `now_ns`. Idempotent if the same source fires twice in a tick (only the
/// last write wins, but the next deadline is the same).
pub fn advance_schedule_for_fired(
    schedule: &mut HashMap<u8, u64>,
    now_ns: u64,
    fired: &[u8],
    period_ns: u64,
) {
    for &source_id in fired {
        let next = now_ns.saturating_add(period_ns);
        schedule.insert(source_id, next);
    }
}
