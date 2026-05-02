//! Wave-9d DoS hardening (DOS-010): per-source pull schedule.
//!
//! Pre-Wave-9d the pull cycle ran every 60s and fired all 9 sources
//! through `futures::join!`, producing a synchronized burst of inter-
//! canister calls every minute. Wave-9d replaces that with a per-source
//! `next_pull_at_ns` schedule walked by a fast 5s tick. Each source
//! still fires every 60s — they just no longer all fire together.
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

/// How often the schedule walker fires. 5s gives a max latency of one
/// tick between a source becoming due and being pulled — well below the
/// 60s pacing contract.
pub const PULL_CYCLE_TICK_SECS: u64 = 5;

/// How often each source fires. 60s preserves the pre-Wave-9d analytics
/// freshness contract surfaced via `last_pull_cycle_ns` and the per-
/// cursor `last_success_ns` fields.
pub const PULL_CYCLE_PERIOD_NS: u64 = 60 * SEC_NANOS;

/// Initial offset between adjacent source slots. 6s × 9 sources = 54s
/// fits comfortably inside the 60s `PULL_CYCLE_PERIOD_NS` window so
/// every source fires exactly once per window even on the discretized
/// 5s tick grid. Keeps every source 1s past the previous tick's slot
/// (sources land at t=0/6/12/.../48 → ticks 0/10/15/.../50).
pub const PULL_CYCLE_SOURCE_OFFSET_NS: u64 = 6 * SEC_NANOS;

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
/// `PULL_CYCLE_SOURCE_OFFSET_NS`, so reordering this list shifts the
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

/// Build a fresh schedule with sources spread across the 60s window.
/// Source at index N gets deadline `now + N * PULL_CYCLE_SOURCE_OFFSET_NS`.
pub fn seed_initial_schedule(now_ns: u64, sources: &[u8]) -> HashMap<u8, u64> {
    let mut schedule = HashMap::with_capacity(sources.len());
    for (idx, &source_id) in sources.iter().enumerate() {
        let deadline = now_ns.saturating_add((idx as u64) * PULL_CYCLE_SOURCE_OFFSET_NS);
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
) -> HashMap<u8, u64> {
    let mut merged = existing.clone();
    for (idx, &source_id) in sources.iter().enumerate() {
        merged.entry(source_id).or_insert_with(|| {
            now_ns.saturating_add((idx as u64) * PULL_CYCLE_SOURCE_OFFSET_NS)
        });
    }
    merged
}

/// Return source ids whose deadline has passed and that should fire
/// this tick. Sources missing from `schedule` are treated as due (so a
/// newly added source fires on its next tick instead of being silently
/// skipped until the next reseed).
pub fn compute_due_sources(
    now_ns: u64,
    schedule: &HashMap<u8, u64>,
    sources: &[u8],
) -> Vec<u8> {
    sources
        .iter()
        .copied()
        .filter(|id| match schedule.get(id) {
            Some(deadline) => *deadline <= now_ns,
            None => true,
        })
        .collect()
}

/// Bump each fired source's deadline by `PULL_CYCLE_PERIOD_NS` from the
/// tick's `now_ns`. Idempotent if the same source fires twice in a tick
/// (only the last write wins, but the next deadline is the same).
pub fn advance_schedule_for_fired(
    schedule: &mut HashMap<u8, u64>,
    now_ns: u64,
    fired: &[u8],
) {
    for &source_id in fired {
        let next = now_ns.saturating_add(PULL_CYCLE_PERIOD_NS);
        schedule.insert(source_id, next);
    }
}
