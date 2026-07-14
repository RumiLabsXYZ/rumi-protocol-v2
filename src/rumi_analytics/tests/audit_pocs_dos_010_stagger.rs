//! Wave-9d DoS hardening: stagger `rumi_analytics` pull cycle
//! (DOS-010).
//!
//! Audit report:
//!   * `audit-reports/2026-04-22-28e9896/findings.json` finding DOS-010
//!     (`pull_cycle fires every 60s with N concurrent inter-canister
//!     calls`).
//!   * Wave plan: `audit-reports/2026-04-22-28e9896/remediation-plan.md`
//!     §"Wave 9 — DoS hardening".
//!
//! # What the gap is
//!
//! Pre-Wave-9d `pull_cycle()` ran every 60s and fired all source pulls
//! (refresh_supply_cache + 7 event tailers + 1 ICRC-3 tailer = 9 calls)
//! through `futures::join!` — every minute the canister generated a
//! synchronized burst of inter-canister calls against the upstream
//! protocol/3pool/AMM/SP/ledgers. Cycle cost is paid every tick even
//! when no new data flows through.
//!
//! # How this file pins the fix
//!
//! Wave-9d switches to a **per-source schedule**: every source has its
//! own `next_pull_at_ns` deadline, persisted in `SlimState` (so offsets
//! survive upgrade). A fast tick (every `PULL_CYCLE_TICK_SECS` seconds)
//! walks the schedule, fires only sources whose deadline has passed, and
//! bumps their next deadline by the effective window. Initial offsets
//! spread the 9 sources across the window at `source_offset_ns(period)`
//! spacing.
//!
//! Cycle-burn tuning (2026-07-02): the default window widened from 60s to
//! 300s and the walker tick from 5s to 30s to cut the poll rate ~5x, and
//! both became runtime-overridable. The per-source offset is now DERIVED
//! from the effective window (`source_offset_ns`) so the stagger stays even
//! at any cadence. These fences pin the *default* constants and the pure
//! scheduling invariants that hold at any window.
//!
//! Layered fences (mirrors the LIQ-002 / DOS-005 file structure):
//!
//!  1. **Constant fences** — TICK / PERIOD defaults pinned; offset derives
//!     from the window. Source-id list pinned (changes need a deliberate
//!     edit).
//!  2. **Initial schedule** — `seed_initial_schedule` produces a
//!     deterministic offset per source so they spread across the window
//!     instead of all firing at `now`.
//!  3. **Due-source selection** — `compute_due_sources` returns ONLY
//!     sources whose `next_pull_at_ns` is in the past.
//!  4. **Schedule advance** — `advance_schedule_for_fired` bumps each fired
//!     source by exactly the effective window. Each source must fire
//!     exactly once per window across many ticks.
//!  5. **Upgrade hygiene** — pre-Wave-9d snapshots decode with the new
//!     `source_next_pull_ns` field populated from `serde(default)`. The
//!     re-seed on `post_upgrade` must NOT clobber existing offsets when
//!     the field is already populated.
//!  6. **Late-added source** — adding a new source to `ALL_SOURCE_IDS`
//!     after deploy must seed an offset for it without disturbing the rest.

use std::collections::HashMap;

use rumi_analytics::pull_schedule::{
    self, advance_schedule_for_fired, compute_due_sources, seed_initial_schedule, source_offset_ns,
    validate_schedule_config, ALL_SOURCE_IDS, MAX_PERIOD_SECS, MAX_TICK_SECS, MIN_PERIOD_SECS,
    MIN_TICK_SECS, PULL_CYCLE_PERIOD_NS, PULL_CYCLE_TICK_SECS, SOURCE_ID_3POOL_BLOCKS,
    SOURCE_ID_3POOL_LIQUIDITY, SOURCE_ID_3POOL_SWAPS, SOURCE_ID_AMM_LIQUIDITY, SOURCE_ID_AMM_SWAPS,
    SOURCE_ID_BACKEND_EVENTS, SOURCE_ID_ICUSD_BLOCKS, SOURCE_ID_STABILITY_EVENTS,
    SOURCE_ID_SUPPLY_CACHE,
};

const SEC_NANOS: u64 = 1_000_000_000;

/// The window used throughout these tests: the compiled-in default.
const PERIOD: u64 = PULL_CYCLE_PERIOD_NS;

/// Convenience: the derived per-source offset for the default window.
fn offset() -> u64 {
    source_offset_ns(PERIOD)
}

// ============================================================================
// Layer 1 — constant fences
// ============================================================================

#[test]
fn dos_010_tick_seconds_default_is_30() {
    assert_eq!(
        PULL_CYCLE_TICK_SECS, 30,
        "DOS-010 / burn tuning: default walker tick is 30s. It is \
         runtime-overridable, but the compiled-in default is pinned so a \
         careless edit can't silently regress the poll rate."
    );
}

#[test]
fn dos_010_period_default_is_300s() {
    assert_eq!(
        PULL_CYCLE_PERIOD_NS,
        300 * SEC_NANOS,
        "DOS-010 / burn tuning: default per-source window is 300s (widened \
         from 60s to cut the inter-canister poll rate ~5x). Runtime- \
         overridable via set_pull_schedule; the default is pinned here."
    );
}

#[test]
fn dos_010_runtime_period_floor_preserves_the_300s_burn_reduction() {
    assert_eq!(
        MIN_PERIOD_SECS, 300,
        "the runtime override must not restore or exceed the pre-tuning poll rate"
    );
}

#[test]
fn dos_010_runtime_config_enforces_static_and_stagger_bounds() {
    assert!(validate_schedule_config(MIN_PERIOD_SECS, MIN_TICK_SECS).is_ok());
    assert!(validate_schedule_config(MAX_PERIOD_SECS, MAX_TICK_SECS).is_ok());

    assert!(validate_schedule_config(MIN_PERIOD_SECS - 1, MIN_TICK_SECS).is_err());
    assert!(validate_schedule_config(MAX_PERIOD_SECS + 1, MIN_TICK_SECS).is_err());
    assert!(validate_schedule_config(MIN_PERIOD_SECS, MIN_TICK_SECS - 1).is_err());
    assert!(validate_schedule_config(MIN_PERIOD_SECS, MAX_TICK_SECS + 1).is_err());

    // Nine sources across a 300s period have ~33s spacing. A 66s walker
    // can collect at most two adjacent slots; 67s can collect three and
    // therefore violates the scheduler's documented 1-2 source bound.
    assert!(validate_schedule_config(300, 66).is_ok());
    assert!(
        validate_schedule_config(300, 67).is_err(),
        "a runtime override must not collapse three or more stagger slots into one tick"
    );
}

#[test]
fn dos_010_largest_safe_tick_fires_at_most_two_sources() {
    let period_ns = 300 * SEC_NANOS;
    let tick_ns = 66 * SEC_NANOS;
    let mut now = 0;
    let mut schedule = seed_initial_schedule(now, ALL_SOURCE_IDS, period_ns);
    let mut max_due = 0;

    for _ in 0..200 {
        let due = compute_due_sources(now, &schedule, ALL_SOURCE_IDS);
        max_due = max_due.max(due.len());
        advance_schedule_for_fired(&mut schedule, now, &due, period_ns);
        now += tick_ns;
    }

    assert!(
        max_due <= 2,
        "accepted minimum-period cadence fired {max_due} sources in one tick"
    );
}

#[test]
fn dos_010_offset_derives_from_window_and_spreads_sources() {
    // The offset is period / N so the 9 sources fill the window. For the
    // 300s default that is ~33.3s, and 9 × offset stays inside the window.
    let off = offset();
    assert_eq!(
        off,
        PERIOD / (ALL_SOURCE_IDS.len() as u64),
        "source offset must be an even fraction of the window"
    );
    assert!(
        (ALL_SOURCE_IDS.len() as u64) * off <= PERIOD,
        "all sources must fit inside one window without wrapping (last slot \
         = {} , window = {})",
        (ALL_SOURCE_IDS.len() as u64) * off,
        PERIOD
    );
    // Offset must exceed nothing in particular, but it should be positive so
    // sources don't all collapse onto `now`.
    assert!(off > 0, "derived offset must be positive");
}

#[test]
fn dos_010_source_ids_full_list_pinned() {
    // Pinning the source-id list ensures any new source forces an
    // explicit edit here AND in seed_initial_schedule's coverage. This
    // is the same fence pattern as cursors.rs CURSOR_ID_*.
    let mut got: Vec<u8> = ALL_SOURCE_IDS.iter().copied().collect();
    got.sort();
    let expected: Vec<u8> = vec![
        SOURCE_ID_SUPPLY_CACHE,     // 0
        SOURCE_ID_BACKEND_EVENTS,   // 1 (matches CURSOR_ID_BACKEND_EVENTS)
        SOURCE_ID_3POOL_SWAPS,      // 2
        SOURCE_ID_3POOL_LIQUIDITY,  // 3
        SOURCE_ID_3POOL_BLOCKS,     // 4
        SOURCE_ID_AMM_SWAPS,        // 5
        SOURCE_ID_STABILITY_EVENTS, // 6
        SOURCE_ID_ICUSD_BLOCKS,     // 7
        SOURCE_ID_AMM_LIQUIDITY,    // 8
    ];
    assert_eq!(got, expected, "ALL_SOURCE_IDS must list every source pull. Adding a new tailer means adding a new id here and a new branch in run_source.");
}

#[test]
fn dos_010_supply_cache_id_is_zero() {
    // Source IDs 1..=8 mirror the CURSOR_ID_* range so the schedule key
    // for a tailer matches its cursor metadata key. Source 0 is the
    // synthetic supply-cache id (no cursor exists).
    assert_eq!(SOURCE_ID_SUPPLY_CACHE, 0);
}

// ============================================================================
// Layer 2 — initial schedule (offsets spread across the window)
// ============================================================================

#[test]
fn dos_010_seed_initial_schedule_assigns_unique_offsets() {
    let now: u64 = 1_000_000_000_000;
    let schedule = seed_initial_schedule(now, ALL_SOURCE_IDS, PERIOD);
    assert_eq!(
        schedule.len(),
        ALL_SOURCE_IDS.len(),
        "every source must receive an initial deadline"
    );
    let mut deadlines: Vec<u64> = schedule.values().copied().collect();
    deadlines.sort();
    deadlines.dedup();
    assert_eq!(
        deadlines.len(),
        ALL_SOURCE_IDS.len(),
        "all source deadlines must be unique (no two sources share a slot)"
    );
}

#[test]
fn dos_010_seed_initial_schedule_offsets_are_evenly_spaced() {
    // Source N gets `now + N * source_offset_ns(period)`.
    // (N is the source's index in ALL_SOURCE_IDS so the spacing stays
    // even regardless of the actual u8 ids.)
    let now: u64 = 0;
    let off = offset();
    let schedule = seed_initial_schedule(now, ALL_SOURCE_IDS, PERIOD);
    for (idx, source_id) in ALL_SOURCE_IDS.iter().enumerate() {
        let expected = now + (idx as u64) * off;
        assert_eq!(
            schedule.get(source_id).copied(),
            Some(expected),
            "source id {} (index {}) must be seeded at offset {}",
            source_id,
            idx,
            expected
        );
    }
}

#[test]
fn dos_010_seed_initial_schedule_preserves_existing_entries() {
    // Re-seeding (e.g., on post_upgrade) must NOT overwrite existing
    // deadlines — that would reset the staggering window every upgrade.
    let now: u64 = 1_000_000_000_000;
    let mut existing = HashMap::new();
    existing.insert(SOURCE_ID_BACKEND_EVENTS, 999_000_000_000);
    existing.insert(SOURCE_ID_3POOL_SWAPS, 998_000_000_000);

    let merged = pull_schedule::reseed_preserving(now, &existing, ALL_SOURCE_IDS, PERIOD);

    assert_eq!(
        merged.get(&SOURCE_ID_BACKEND_EVENTS).copied(),
        Some(999_000_000_000),
        "existing deadline for backend_events must be preserved"
    );
    assert_eq!(
        merged.get(&SOURCE_ID_3POOL_SWAPS).copied(),
        Some(998_000_000_000),
        "existing deadline for 3pool_swaps must be preserved"
    );
    // New sources get fresh offsets relative to `now`.
    assert_ne!(
        merged.get(&SOURCE_ID_SUPPLY_CACHE).copied(),
        Some(999_000_000_000),
        "supply-cache must NOT be backfilled with another source's deadline"
    );
    let supply_idx = ALL_SOURCE_IDS
        .iter()
        .position(|id| *id == SOURCE_ID_SUPPLY_CACHE)
        .unwrap();
    let expected_supply = now + (supply_idx as u64) * offset();
    assert_eq!(
        merged.get(&SOURCE_ID_SUPPLY_CACHE).copied(),
        Some(expected_supply),
        "newly added source must be seeded at its index offset"
    );
}

// ============================================================================
// Layer 3 — due-source selection
// ============================================================================

#[test]
fn dos_010_compute_due_sources_returns_empty_when_nothing_is_due() {
    // Schedule is now+offset; at exactly `now` only the source whose
    // offset == 0 (index 0 = supply cache) is due.
    let now: u64 = 100 * SEC_NANOS;
    let schedule = seed_initial_schedule(now, ALL_SOURCE_IDS, PERIOD);
    let due = compute_due_sources(now, &schedule, ALL_SOURCE_IDS);
    assert_eq!(
        due,
        vec![SOURCE_ID_SUPPLY_CACHE],
        "at t=now, only the source seeded at offset 0 (supply cache) is due"
    );
}

#[test]
fn dos_010_compute_due_sources_includes_only_past_deadlines() {
    let now: u64 = 100 * SEC_NANOS;
    let off = offset();
    let schedule = seed_initial_schedule(now, ALL_SOURCE_IDS, PERIOD);
    // Advance to just past source-index-3's offset (3 × offset into the
    // cycle), so indices 0..=3 are due and index 4 is not.
    let later = now + 3 * off + SEC_NANOS;
    let due = compute_due_sources(later, &schedule, ALL_SOURCE_IDS);
    assert_eq!(
        due.len(),
        4,
        "at t=now+3*offset+1s, sources at offsets 0/1/2/3*offset must all be \
         due (got {} = {:?})",
        due.len(),
        due
    );
    for &id in &due {
        let idx = ALL_SOURCE_IDS.iter().position(|&i| i == id).unwrap();
        assert!(
            (idx as u64) * off <= later - now,
            "source id {} (index {}) was reported due but its offset is in the future",
            id,
            idx
        );
    }
}

#[test]
fn dos_010_compute_due_sources_treats_missing_entry_as_due() {
    // If `next_pull_at_ns` is missing for some source (e.g., new source
    // added between upgrades and re-seed has not yet run), it MUST be
    // due. Otherwise a never-seeded source would never fire.
    let mut schedule = HashMap::new();
    schedule.insert(SOURCE_ID_SUPPLY_CACHE, u64::MAX); // far future
    let now: u64 = 100 * SEC_NANOS;
    let due = compute_due_sources(now, &schedule, ALL_SOURCE_IDS);
    // Only sources NOT in schedule should be due (everything except SOURCE_ID_SUPPLY_CACHE).
    assert!(
        !due.contains(&SOURCE_ID_SUPPLY_CACHE),
        "source with future deadline must not be reported due"
    );
    assert_eq!(
        due.len(),
        ALL_SOURCE_IDS.len() - 1,
        "every source except SUPPLY_CACHE has no schedule entry — all should be reported due"
    );
}

// ============================================================================
// Layer 4 — schedule advance / per-source window pacing
// ============================================================================

#[test]
fn dos_010_advance_schedule_bumps_fired_sources_by_period() {
    let now: u64 = 100 * SEC_NANOS;
    let off = offset();
    let mut schedule = seed_initial_schedule(now, ALL_SOURCE_IDS, PERIOD);
    let fired = vec![SOURCE_ID_SUPPLY_CACHE];

    advance_schedule_for_fired(&mut schedule, now, &fired, PERIOD);

    assert_eq!(
        schedule[&SOURCE_ID_SUPPLY_CACHE],
        now + PERIOD,
        "fired source must have its deadline pushed exactly one window into the future"
    );

    // Non-fired sources must not be touched.
    for &id in ALL_SOURCE_IDS {
        if id == SOURCE_ID_SUPPLY_CACHE {
            continue;
        }
        let idx = ALL_SOURCE_IDS.iter().position(|&i| i == id).unwrap();
        assert_eq!(
            schedule[&id],
            now + (idx as u64) * off,
            "non-fired source {} must keep its original deadline",
            id
        );
    }
}

#[test]
fn dos_010_each_source_fires_exactly_once_per_window() {
    // Walk one window in TICK-second steps. Each of the 9 sources must fire
    // exactly once across the window.
    let mut now: u64 = 0;
    let mut schedule = seed_initial_schedule(now, ALL_SOURCE_IDS, PERIOD);
    let mut fire_counts: HashMap<u8, u32> = HashMap::new();

    // Number of ticks in one window (300 / 30 = 10 for the defaults).
    let n_ticks = PERIOD / (PULL_CYCLE_TICK_SECS * SEC_NANOS);
    for _ in 0..n_ticks {
        let due = compute_due_sources(now, &schedule, ALL_SOURCE_IDS);
        for &id in &due {
            *fire_counts.entry(id).or_insert(0) += 1;
        }
        advance_schedule_for_fired(&mut schedule, now, &due, PERIOD);
        now += PULL_CYCLE_TICK_SECS * SEC_NANOS;
    }

    for &id in ALL_SOURCE_IDS {
        let count = fire_counts.get(&id).copied().unwrap_or(0);
        assert_eq!(
            count, 1,
            "source id {} must fire exactly once per window (got {} fires across {} ticks)",
            id, count, n_ticks
        );
    }
    // Total fires == number of sources.
    let total: u32 = fire_counts.values().sum();
    assert_eq!(
        total,
        ALL_SOURCE_IDS.len() as u32,
        "total fires across one window must equal source count (got {})",
        total
    );
}

#[test]
fn dos_010_no_sync_storm_at_any_single_tick() {
    // Walk the schedule for many ticks; assert that at no point are more
    // than 2 sources due simultaneously. Pre-fix all 9 fired together;
    // post-fix the derived offset (~33s) exceeds the 30s tick so at most one
    // source is due per tick (2 tolerated for grid drift), never all 9.
    let mut now: u64 = 0;
    let mut schedule = seed_initial_schedule(now, ALL_SOURCE_IDS, PERIOD);
    let max_per_tick_observed = {
        let mut m: usize = 0;
        for _ in 0..200 {
            let due = compute_due_sources(now, &schedule, ALL_SOURCE_IDS);
            if due.len() > m {
                m = due.len();
            }
            advance_schedule_for_fired(&mut schedule, now, &due, PERIOD);
            now += PULL_CYCLE_TICK_SECS * SEC_NANOS;
        }
        m
    };
    assert!(
        max_per_tick_observed <= 2,
        "no single tick must fire more than 2 sources concurrently (got {})",
        max_per_tick_observed
    );
}

// ============================================================================
// Layer 5 — upgrade hygiene
// ============================================================================

#[test]
fn dos_010_reseed_after_upgrade_preserves_in_flight_offsets() {
    // Simulate: tick a few times, snapshot the schedule, "upgrade" by
    // re-seeding with the snapshot, assert the schedule is unchanged.
    let mut now: u64 = 0;
    let mut schedule = seed_initial_schedule(now, ALL_SOURCE_IDS, PERIOD);
    for _ in 0..5 {
        let due = compute_due_sources(now, &schedule, ALL_SOURCE_IDS);
        advance_schedule_for_fired(&mut schedule, now, &due, PERIOD);
        now += PULL_CYCLE_TICK_SECS * SEC_NANOS;
    }
    let snapshot: HashMap<u8, u64> = schedule.clone();

    // Simulate post_upgrade: reseed with the snapshot.
    let merged = pull_schedule::reseed_preserving(now, &snapshot, ALL_SOURCE_IDS, PERIOD);

    // Every entry from snapshot must survive verbatim.
    for (id, deadline) in snapshot.iter() {
        assert_eq!(
            merged.get(id).copied(),
            Some(*deadline),
            "post-upgrade reseed must preserve in-flight deadline for source {}",
            id
        );
    }
}

#[test]
fn dos_010_reseed_after_upgrade_seeds_newly_added_source() {
    // Pre-upgrade snapshot is missing a source (simulating a new tailer
    // added in the upgrade). Reseed must add it without disturbing the
    // existing entries.
    let now: u64 = 100 * SEC_NANOS;
    let mut snapshot = HashMap::new();
    for &id in ALL_SOURCE_IDS.iter().take(ALL_SOURCE_IDS.len() - 1) {
        snapshot.insert(id, now + 1);
    }
    let new_source_id = *ALL_SOURCE_IDS.last().unwrap();
    let merged = pull_schedule::reseed_preserving(now, &snapshot, ALL_SOURCE_IDS, PERIOD);
    assert!(
        merged.contains_key(&new_source_id),
        "post-upgrade reseed must seed a new source not present in snapshot"
    );
    // Existing entries unchanged.
    for &id in ALL_SOURCE_IDS.iter().take(ALL_SOURCE_IDS.len() - 1) {
        assert_eq!(merged[&id], now + 1, "existing source {} disturbed", id);
    }
}
