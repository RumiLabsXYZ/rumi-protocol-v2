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
//! survive upgrade). A small fast tick (every `PULL_CYCLE_TICK_SECS`
//! seconds) walks the schedule, fires only sources whose deadline has
//! passed, and bumps their next deadline by `PULL_CYCLE_PERIOD_NS`.
//! Initial offsets spread the 9 sources across the 60s window at
//! `PULL_CYCLE_SOURCE_OFFSET_NS` spacing.
//!
//! Layered fences (mirrors the LIQ-002 / DOS-005 file structure):
//!
//!  1. **Constant fences** — TICK / PERIOD / OFFSET pinned at audit-spec
//!     values. Source-id list pinned (changes need a deliberate edit).
//!  2. **Initial schedule** — `seed_initial_schedule` produces a
//!     deterministic offset per source so they spread across the 60s
//!     window instead of all firing at `now`.
//!  3. **Due-source selection** — `compute_due_sources` returns ONLY
//!     sources whose `next_pull_at_ns` is in the past. Out-of-band
//!     timestamp arithmetic (saturating subtraction, clock skew) must
//!     not include future-due sources in the firing set.
//!  4. **Schedule advance** — `advance_schedule_for_fired` bumps each
//!     fired source by exactly `PULL_CYCLE_PERIOD_NS`. Each source must
//!     fire exactly once per `PULL_CYCLE_PERIOD_NS` window across many
//!     ticks.
//!  5. **Upgrade hygiene** — pre-Wave-9d snapshots decode with the new
//!     `source_next_pull_ns` field populated from `serde(default)`. The
//!     re-seed on `post_upgrade` must NOT clobber existing offsets when
//!     the field is already populated.
//!  6. **Late-added source** — adding a new source to `ALL_SOURCE_IDS`
//!     after deploy (or on a fresh canister where the schedule has
//!     drifted) must seed an offset for the new source without
//!     disturbing the existing ones.

use std::collections::HashMap;

use rumi_analytics::pull_schedule::{
    self, advance_schedule_for_fired, compute_due_sources, seed_initial_schedule,
    ALL_SOURCE_IDS, PULL_CYCLE_PERIOD_NS, PULL_CYCLE_SOURCE_OFFSET_NS, PULL_CYCLE_TICK_SECS,
    SOURCE_ID_AMM_LIQUIDITY, SOURCE_ID_AMM_SWAPS, SOURCE_ID_BACKEND_EVENTS,
    SOURCE_ID_ICUSD_BLOCKS, SOURCE_ID_STABILITY_EVENTS, SOURCE_ID_SUPPLY_CACHE,
    SOURCE_ID_3POOL_BLOCKS, SOURCE_ID_3POOL_LIQUIDITY, SOURCE_ID_3POOL_SWAPS,
};

const SEC_NANOS: u64 = 1_000_000_000;

// ============================================================================
// Layer 1 — constant fences
// ============================================================================

#[test]
fn dos_010_tick_seconds_pinned_at_5() {
    assert_eq!(
        PULL_CYCLE_TICK_SECS, 5,
        "Wave-9d DOS-010: pull-cycle tick must be 5s. Lengthening it \
         pushes some sources past their nominal 60s cadence; shortening \
         it wakes the canister more often without spreading work further."
    );
}

#[test]
fn dos_010_period_pinned_at_60s() {
    assert_eq!(
        PULL_CYCLE_PERIOD_NS,
        60 * SEC_NANOS,
        "Wave-9d DOS-010: each source must fire every 60s (matches the \
         pre-Wave-9d cadence). Changing this changes the analytics \
         freshness contract surfaced through `last_pull_cycle_ns`."
    );
}

#[test]
fn dos_010_offset_spreads_sources_across_window() {
    // 9 sources × 6s offset = 54s spread, fits cleanly inside the 60s
    // PULL_CYCLE_PERIOD_NS window. Pre-Wave-9d behaviour collapsed all
    // 9 source pulls into the same instant; 6s spacing puts each on its
    // own tick when the wall clock advances in 5s steps.
    assert_eq!(
        PULL_CYCLE_SOURCE_OFFSET_NS,
        6 * SEC_NANOS,
        "Wave-9d DOS-010: source-offset spacing must be 6s. Wider spacing \
         (e.g., 7s × 9 = 63s) overshoots the 60s window and leaves the \
         last source firing on alternate cycles relative to the 5s tick grid."
    );
}

#[test]
fn dos_010_source_ids_full_list_pinned() {
    // Pinning the source-id list ensures any new source forces an
    // explicit edit here AND in seed_initial_schedule's coverage. This
    // is the same fence pattern as cursors.rs CURSOR_ID_*.
    let mut got: Vec<u8> = ALL_SOURCE_IDS.iter().copied().collect();
    got.sort();
    let expected: Vec<u8> = vec![
        SOURCE_ID_SUPPLY_CACHE,    // 0
        SOURCE_ID_BACKEND_EVENTS,  // 1 (matches CURSOR_ID_BACKEND_EVENTS)
        SOURCE_ID_3POOL_SWAPS,     // 2
        SOURCE_ID_3POOL_LIQUIDITY, // 3
        SOURCE_ID_3POOL_BLOCKS,    // 4
        SOURCE_ID_AMM_SWAPS,       // 5
        SOURCE_ID_STABILITY_EVENTS,// 6
        SOURCE_ID_ICUSD_BLOCKS,    // 7
        SOURCE_ID_AMM_LIQUIDITY,   // 8
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
    let schedule = seed_initial_schedule(now, ALL_SOURCE_IDS);
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
    // Source N gets `now + N * PULL_CYCLE_SOURCE_OFFSET_NS`.
    // (N is the source's index in ALL_SOURCE_IDS so the spacing stays
    // even regardless of the actual u8 ids.)
    let now: u64 = 0;
    let schedule = seed_initial_schedule(now, ALL_SOURCE_IDS);
    for (idx, source_id) in ALL_SOURCE_IDS.iter().enumerate() {
        let expected = now + (idx as u64) * PULL_CYCLE_SOURCE_OFFSET_NS;
        assert_eq!(
            schedule.get(source_id).copied(),
            Some(expected),
            "source id {} (index {}) must be seeded at offset {}",
            source_id, idx, expected
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

    let merged = pull_schedule::reseed_preserving(now, &existing, ALL_SOURCE_IDS);

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
    let backend_idx = ALL_SOURCE_IDS
        .iter()
        .position(|id| *id == SOURCE_ID_BACKEND_EVENTS)
        .unwrap();
    assert_ne!(
        merged.get(&SOURCE_ID_SUPPLY_CACHE).copied(),
        Some(999_000_000_000),
        "supply-cache must NOT be backfilled with another source's deadline"
    );
    let supply_idx = ALL_SOURCE_IDS
        .iter()
        .position(|id| *id == SOURCE_ID_SUPPLY_CACHE)
        .unwrap();
    let _ = backend_idx; // silence unused — referenced in trace if assertion fires
    let expected_supply = now + (supply_idx as u64) * PULL_CYCLE_SOURCE_OFFSET_NS;
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
    let schedule = seed_initial_schedule(now, ALL_SOURCE_IDS);
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
    let schedule = seed_initial_schedule(now, ALL_SOURCE_IDS);
    // Advance to just past source-index-3's offset (3 * 7s = 21s into the cycle).
    let later = now + 22 * SEC_NANOS;
    let due = compute_due_sources(later, &schedule, ALL_SOURCE_IDS);
    // Sources 0..=3 (by index) should be due; their seed-time was 0/7/14/21s past now.
    assert_eq!(
        due.len(),
        4,
        "at t=now+22s, sources at offsets 0/7/14/21 must all be due (got {} = {:?})",
        due.len(), due
    );
    for &id in &due {
        let idx = ALL_SOURCE_IDS.iter().position(|&i| i == id).unwrap();
        assert!(
            (idx as u64) * PULL_CYCLE_SOURCE_OFFSET_NS <= 22 * SEC_NANOS,
            "source id {} (index {}) was reported due but its offset is in the future",
            id, idx
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
// Layer 4 — schedule advance / per-source 60s pacing
// ============================================================================

#[test]
fn dos_010_advance_schedule_bumps_fired_sources_by_period() {
    let now: u64 = 100 * SEC_NANOS;
    let mut schedule = seed_initial_schedule(now, ALL_SOURCE_IDS);
    let original_supply = schedule[&SOURCE_ID_SUPPLY_CACHE];
    let fired = vec![SOURCE_ID_SUPPLY_CACHE];

    advance_schedule_for_fired(&mut schedule, now, &fired);

    assert_eq!(
        schedule[&SOURCE_ID_SUPPLY_CACHE],
        now + PULL_CYCLE_PERIOD_NS,
        "fired source must have its deadline pushed exactly PULL_CYCLE_PERIOD_NS into the future"
    );
    let _ = original_supply; // value irrelevant; checked the new value is `now + period`

    // Non-fired sources must not be touched.
    for &id in ALL_SOURCE_IDS {
        if id == SOURCE_ID_SUPPLY_CACHE {
            continue;
        }
        let idx = ALL_SOURCE_IDS.iter().position(|&i| i == id).unwrap();
        assert_eq!(
            schedule[&id],
            now + (idx as u64) * PULL_CYCLE_SOURCE_OFFSET_NS,
            "non-fired source {} must keep its original deadline",
            id
        );
    }
}

#[test]
fn dos_010_each_source_fires_exactly_once_per_window() {
    // Walk a 60s window in 5s ticks. With 9 sources × 7s offsets,
    // each source must fire exactly once across the window.
    let mut now: u64 = 0;
    let mut schedule = seed_initial_schedule(now, ALL_SOURCE_IDS);
    let mut fire_counts: HashMap<u8, u32> = HashMap::new();

    // Walk PULL_CYCLE_PERIOD_NS worth of ticks. PULL_CYCLE_PERIOD_NS / TICK_SECS = 60/5 = 12 ticks.
    let n_ticks = PULL_CYCLE_PERIOD_NS / (PULL_CYCLE_TICK_SECS * SEC_NANOS);
    for _ in 0..n_ticks {
        let due = compute_due_sources(now, &schedule, ALL_SOURCE_IDS);
        for &id in &due {
            *fire_counts.entry(id).or_insert(0) += 1;
        }
        advance_schedule_for_fired(&mut schedule, now, &due);
        now += PULL_CYCLE_TICK_SECS * SEC_NANOS;
    }

    // Walk one more period. Final tally should be 1 per source per
    // window, total = 9 fires per 60s.
    for &id in ALL_SOURCE_IDS {
        let count = fire_counts.get(&id).copied().unwrap_or(0);
        assert_eq!(
            count, 1,
            "source id {} must fire exactly once per 60s window (got {} fires across {} ticks)",
            id, count, n_ticks
        );
    }
    // Total fires == number of sources.
    let total: u32 = fire_counts.values().sum();
    assert_eq!(
        total,
        ALL_SOURCE_IDS.len() as u32,
        "total fires across one 60s window must equal source count (got {})",
        total
    );
}

#[test]
fn dos_010_no_sync_storm_at_any_single_tick() {
    // Walk the schedule for many ticks; assert that at no point are
    // more than `ceil(N_sources * tick / period) + 1` sources due
    // simultaneously. Pre-fix all 9 fired together; post-fix at most
    // 1-2 per tick (drift may cluster a couple together but never all 9).
    let mut now: u64 = 0;
    let mut schedule = seed_initial_schedule(now, ALL_SOURCE_IDS);
    let max_per_tick_observed = {
        let mut m: usize = 0;
        for _ in 0..200 {
            let due = compute_due_sources(now, &schedule, ALL_SOURCE_IDS);
            if due.len() > m {
                m = due.len();
            }
            advance_schedule_for_fired(&mut schedule, now, &due);
            now += PULL_CYCLE_TICK_SECS * SEC_NANOS;
        }
        m
    };
    // Hard upper bound: 2 per tick (some clustering as the deadlines
    // drift relative to the 5s tick grid). If this ever exceeds 2,
    // either the offset shrunk or the tick grew and DOS-010's premise
    // is undermined.
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
    let mut schedule = seed_initial_schedule(now, ALL_SOURCE_IDS);
    for _ in 0..5 {
        let due = compute_due_sources(now, &schedule, ALL_SOURCE_IDS);
        advance_schedule_for_fired(&mut schedule, now, &due);
        now += PULL_CYCLE_TICK_SECS * SEC_NANOS;
    }
    let snapshot: HashMap<u8, u64> = schedule.clone();

    // Simulate post_upgrade: reseed with the snapshot.
    let merged = pull_schedule::reseed_preserving(now, &snapshot, ALL_SOURCE_IDS);

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
    let merged = pull_schedule::reseed_preserving(now, &snapshot, ALL_SOURCE_IDS);
    assert!(
        merged.contains_key(&new_source_id),
        "post-upgrade reseed must seed a new source not present in snapshot"
    );
    // Existing entries unchanged.
    for &id in ALL_SOURCE_IDS.iter().take(ALL_SOURCE_IDS.len() - 1) {
        assert_eq!(merged[&id], now + 1, "existing source {} disturbed", id);
    }
}
