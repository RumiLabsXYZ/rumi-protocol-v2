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

### Backend event-query pagination: RESOLVED (Rob chose the backend endpoint)
`get_events_filtered` paginates newest-first by page number (no stable cursor) and
`get_events` returns all ~95 variants (a subset mirror cannot decode unknown-variant
values; confirmed by canary). Rob chose to add a clean forward endpoint.

DONE on branch `feat/backend-forward-filtered-events` (commit `51aa812`, NOT merged,
local-only): `rumi_protocol_backend.get_events_forward_filtered(start, max_scan,
opt vec EventTypeFilter) -> record { events: vec record { nat64; Event };
next_start; reached_end }`. Read-only additive query (no Event/state change, no
UPG-002 risk), O(max_scan) per call, unit-tested, .did regenerated + candid test
green. The poller passes `start := next_start` until `reached_end`.

### Remaining Phase 2 work (the poll layer + E2E) — next focused chunk
1. Verify 3pool/SP/AMM `get_*_events(start,length)` paging (forward global-id window
   vs newest-first/page). DO NOT assume forward-id after the backend surprise. If any
   is page-based, it needs the same forward-endpoint treatment on that canister.
2. Wire the inter-canister poll. IMPORTANT: the backend cursor advances to the
   endpoint's `next_start` (a scan position), NOT `ingest_batch`'s `max(event_id)+1`.
   Refactor an `apply_events` (no cursor) out of `ingest_batch` so the backend poll
   reuses it and sets the cursor from `next_start`.
3. Source-canister-id config (configurable per env; init args or a stable map +
   admin setter — local ids differ from mainnet), a poll timer, and admin
   trigger/status endpoints.
4. PocketIC E2E (the plan's Phase 2 verification): deploy the sources + rumi_points,
   generate real events, poll, assert auto-registration + cursor advance. Needs both
   the rumi_points branch and the backend-endpoint branch together.

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
