# rumi_points — status

**Phase 1 (scaffold): COMPLETE.** **Phase 2/3 (ingestion + auto-registration): IN PROGRESS.**
Branch `feat/airdrop-points-canister` (off `main`). Local-only; not deployed to
mainnet, no mainnet canister id reserved.

Spec: `docs/specs/rumi-airdrop-spec-v2.md` (Section 7 data model, Section 11 excluded
principals). Plan: `docs/plans/2026-05-03-airdrop-implementation-plan.md`.

## Phase 2/3: ingestion machinery COMPLETE in code (one E2E step remains)
Two unmerged branches (both off `main`, NOT merged, NOT deployed):

**`feat/airdrop-points-canister`** (the points canister): normalized event model +
the five Section-8 qualifying triggers + `apply_ingested_event`/`apply_events`
(auto-register on first qualifying in-season action; idempotent; excluded rejected);
per-source cursors (MemoryId 8) + poll guard + `in_season`; per-source candid mirror
types + `normalize_*` (validated at the candid layer: subset decode, SP all-16
variants, migrated-row exclusion, forward-response round-trips); the inter-canister
`poll.rs` (backend/3pool advance to the endpoint's `next_start`, SP/AMM advance by
count via `ingest_batch`; single-poll guard; per-source failures logged/skipped, no
trap); source-canister config (MemoryId 9, mainnet-seeded, admin `set_source_canister`);
admin `trigger_poll` + `get_ingest_status`. 33 unit tests + candid drift, all green.

**`feat/source-forward-event-endpoints`** (source-side forward read endpoints):
- backend `get_events_forward_filtered(start, max_scan, opt vec EventTypeFilter)`
  (commit `51aa812`).
- 3pool `get_liquidity_events_v2_forward(start, max_scan)` (commit `d35ed70`).
Both read-only additive (no Event/state change, no UPG-002 risk), O(max_scan),
unit-tested, .did + candid checks green. SP/AMM keep their existing oldest-first
index APIs for Season 1 (their logs trim at 10k/50k, far above current volume, so
`id==index` holds and advance-by-count is gap-free; documented; revisit at scale).

Verified on a local replica: source config seeded; `trigger_poll` handles
unreachable sources gracefully (Ok 0, cursors unchanged) and the poll guard releases
(a second poll is not blocked); source config (MemoryId 9), cursors, registered
principals, and excluded set all survive a real upgrade.

### The ONLY remaining Phase 2 step: PocketIC E2E (live validation)
Deploy the sources (backend+3pool with their forward endpoints, SP, AMM, ledgers) +
rumi_points, generate real events (mint a vault, add 3pool liquidity, SP deposit),
`trigger_poll`, assert auto-registration + correct cursor advance. This needs BOTH
branches' code together (the project's PocketIC tests `include_bytes!` prebuilt
wasms), so it requires a branch-combination decision (combine into one integration
PR, or merge both to main first). The inter-canister poll is the one piece unit
tests cannot cover; the E2E is its validation.

## What is real (and tested)
- Stable-storage layout via `MemoryManager` (mirrors `rumi_protocol_backend::storage`).
- Data model (`types.rs`), configurable excluded-principals set + admin setters,
  admin-gated `register_test_principal`, leaderboard, epoch-history reads.
- Versioned-snapshot upgrade-safety pattern from day one (`Stored*` enums; recipe in
  the `state.rs` module doc). 33 unit tests + a candid drift test, all green.
- Verified on a local replica: every endpoint returns default state; a register +
  exclude survived a real upgrade (module hash changed, state persisted).
- `events.rs` / `source_types.rs` / `poll.rs` are the real Phase 2/3 ingestion (see
  the "ingestion machinery" section above), no longer skeleton.

## What is skeleton (later phases; signatures + doc comments only)
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
