# rumi_points — status

**Phase 1 (scaffold): COMPLETE.** **Phase 2/3 (ingestion + auto-registration): IN PROGRESS.**
Branch `feat/airdrop-points-canister` (off `main`). Local-only; not deployed to
mainnet, no mainnet canister id reserved.

Spec: `docs/specs/rumi-airdrop-spec-v2.md` (Section 7 data model, Section 11 excluded
principals). Plan: `docs/plans/2026-05-03-airdrop-implementation-plan.md`.

## Phase 2/3 progress
Committed and tested (31 unit tests + candid drift test, all green):
- `events.rs`: normalized `IngestedEvent`/`IngestKind`, the five Section-8
  qualifying-action triggers, `apply_ingested_event` (auto-registers on first
  qualifying in-season action; idempotent; excluded rejected), `ingest_batch`
  (advances the per-source cursor). Pull-ingestion CORE.
- `state.rs`: per-source cursors (StableBTreeMap, MemoryId 8), transient poll
  guard, `in_season` gate.
- `source_types.rs`: per-source candid mirror types + `normalize_*`, validated at
  the candid layer (subset-decode, SP all-16-variants, migrated-row exclusion).

NOT yet built (the remaining Phase 2 machinery), blocked on a design decision:
- The inter-canister poll loop, the source-canister-id config, the timer, and the
  admin trigger/status endpoints.
- The PocketIC end-to-end ingestion test (the plan's Phase 2 verification).

### Blocker / decision needed: backend event-query pagination
`rumi_protocol_backend.get_events_filtered` paginates NEWEST-FIRST by PAGE NUMBER
(not a forward global-id cursor) and returns no `scan_end`, so it cannot drive
stable incremental forward ingestion. The clean-forward endpoint `get_events`
returns the full ~95-variant `Event`, which the 9-variant mirror cannot decode
(candid rejects unknown-variant values; confirmed by canary). Options:
  1. Mirror all ~95 backend variants and poll `get_events` forward by global id
     (robust cursor, brittle/large mirror that breaks when upstream adds a variant).
  2. Page-scan the filtered set each cycle keyed on a cached max global id (more
     cross-canister work; the filtered set is recomputed O(N) per call).
  3. Add a small FORWARD-filtered, id-cursored endpoint to the backend (cleanest,
     but the backend has active Monad branches and a "do not touch" caution -> needs
     Rob's OK; would ride a separate branch).
The 3pool/SP/AMM `get_*_events(start,length)` pagination semantics are NOT yet
verified; do that before wiring their poll cursors (do not assume forward-id).

## What is real (and tested)
- Stable-storage layout via `MemoryManager` (mirrors `rumi_protocol_backend::storage`).
- Data model (`types.rs`), configurable excluded-principals set + admin setters,
  admin-gated `register_test_principal`, leaderboard, epoch-history reads.
- Versioned-snapshot upgrade-safety pattern from day one (`Stored*` enums; recipe in
  the `state.rs` module doc). 12 unit tests + a candid drift test, all green.
- Verified on a local replica: every endpoint returns default state; a register +
  exclude survived a real upgrade (module hash changed, state persisted).

## What is skeleton (later phases; signatures + doc comments only)
- `events.rs`  — Phase 2 pull-based ingestion.
- `epoch.rs`   — Phase 5 weekly two-snapshot `min()` accrual.
- `valuation.rs` — Phase 4/5 asset valuation + 3USD verification.
- `snapshot_seed.rs` — Phase 5 commit-reveal algorithm (its STATE types are real
  and already in the stable layout).

## Stable memory map (MemoryId -> structure; never reuse an id)
| Id | Structure |
|----|-----------|
| 0  | `StableBTreeMap<Principal, StoredPrincipalState>` (per-principal accrual) |
| 1/2| `StableLog<StoredPointEntry>` (append-only audit ledger) |
| 3/4| `StableLog<StoredEpochSummary>` (per-epoch rollups) |
| 5/6| `StableLog<StoredRevealedSeed>` (commit-reveal audit log; reserved, Phase 5) |
| 7  | singleton `State` blob (8-byte LE len prefix + CBOR `StoredState::V1`) |

## Excluded-principals seed (confirmed against canister_ids.json, 2026-06-01)
9 protocol-owned canisters: rumi_protocol_backend, rumi_3pool, rumi_amm,
rumi_stability_pool, rumi_treasury, liquidation_bot, icusd_ledger, icusd_index,
threeusd_index. Deliberately excluded from the seed: rumi_analytics (no qualifying
balances; admin can add). Founder/team are deliberately NOT excluded (spec Section 11).
The set is admin-configurable; the seed is applied at init, enforcement reads the
mutable set.

## Two deliberate refinements of the spec's literal Section-7 shapes
1. `PrincipalState` carries no inline `point_ledger: Vec<PointEntry>`; the audit
   ledger is the separate global `StableLog<PointEntry>` the plan mandates.
2. No `pro_rata_share` field — that is computed from the FROZEN ledger by the later
   claim canister, not here.

## Deferred from the Phase 1 task list (needs Rob)
- Reserve a mainnet canister id + add to `canister_ids.json` / `mainnet-live`
  (irreversible; requires explicit OK).

## Do NOT start Phase 2 from here without re-reading the handoff + spec.
