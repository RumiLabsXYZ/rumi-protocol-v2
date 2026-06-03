# rumi_points — status

**Phase 1 (scaffold): COMPLETE.** **Phase 2 / 2b / 3 (ingestion + timer +
auto-registration): COMPLETE and PocketIC-E2E-validated, merged to `main`** (PR #217).
**Phase 4 (3USD verification) + Phase 5 (accrual engine): COMPLETE on branch
`feat/airdrop-phase5-accrual`** (NOT yet merged, NOT deployed). 126 lib unit tests +
candid drift + 4 PocketIC E2E (2 ingest + 2 accrual), all green; zero warnings.

The canister is now a FULL airdrop engine: ingestion auto-registers, the weekly
epoch driver captures two randomized intra-epoch snapshots of every registered
principal's balances, takes `min()` (anti-sniping), applies the multiplier table
with 3USD verification, accrues dollar-days, and reveals the commit-reveal seed.
Design doc: `docs/notes/2026-06-02-phase5-accrual-design.md` (gitignored, local).

Phase 5 blocker carried forward: the 5x ckUSDC/ckUSDT repayment window is GATED on
the upstream backend `RepayToVault.repayment_asset` field (not yet built). The full
window machinery + tests exist; `IngestKind::VaultRepay` carries `repayment_asset:
Option<Principal>` (normalized to `None` until upstream lands), and the window is
skipped (logged) while it is `None`. Flip the backend normalizer when it ships.

## Deploy posture (why nothing is on mainnet yet)
Not urgent, because a LATER deploy loses no data: the cursor starts at 0 and the
source event logs are unbounded (backend/3pool) or trim far above Season-1 volume
(SP/AMM), so the first poll backfills all in-season activity. There is no awarded
value until Phase 5 accrual, so there is no rush to go live.

When deploying: do the backend + 3pool endpoint upgrades together with `rumi_points`.
Backend upgrade = ProtocolArg Upgrade variant + description; 3pool = dummy
ThreePoolInitArgs; `rumi_points` = reserve a fresh mainnet id + add to `mainnet-live`,
then admin `set_poll_enabled(true)` once sources are confirmed. The pre-deploy hook
runs the full suite (including the POCKET_IC_BIN E2E).

## Next phases (fresh focused sessions)
- Phase 6: frontend integration (confetti enrollment, personal status, leaderboard).
- Phases 7-8: the claim canister (liquid + lock-tier haircut ladder).
- Upstream: add `repayment_asset` to the backend `RepayToVault` event (separate
  backend branch), then flip `source_types::backend::normalize` to read it and the
  5x window activates with no other change here.

## Phase 5 review follow-ups (deferred, non-blocking)
From the 2026-06-02 code review (no critical/high bugs found). Fixed in-branch:
ICP non-finite-rate guard, repayment-window pruning at close, atomic-trap on the
(unreachable) seed-close failure, per-principal source-id caching. Still open:
- `epoch.rs` capture has no STALL counter if a source stays unreachable past a
  snapshot time (the epoch safely never closes; visible via `get_epoch_status`,
  recoverable via `force_epoch_tick`). Consider surfacing a stall count.
- `set_asset_ledger` / `set_source_canister` accept any `u8` tag (admin-only;
  out-of-range writes a harmless dead entry). Minor range-validation nicety.

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

### PocketIC E2E: DONE (green, merged)
`src/rumi_points/tests/pocket_ic_ingest.rs` + the `rumi_points_e2e_source` mock
canister validate the live path end to end: `trigger_poll` -> `ic_cdk::call` ->
candid decode -> normalize -> auto-register -> cursor advance, with a no-op second
poll. Build + run:
```
cargo build --release --target wasm32-unknown-unknown -p rumi_points -p rumi_points_e2e_source
POCKET_IC_BIN=$PWD/pocket-ic cargo test -p rumi_points --test pocket_ic_ingest
```
This E2E caught and fixed a real bug: the backend forward endpoint had the
`data_certificate().is_none() -> trap` guard, which rejects inter-canister calls;
removed (the endpoint is meant to be polled). A future hardening could swap the
mock for the real backend (mint a vault, poll) to exercise the true 95-variant
wire, though the `source_types` canary already pins the superset-decode rule.

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

## Phase 4/5 modules (now REAL, fully tested; no longer skeleton)
- `accrual.rs` (new) — pure accrual math: `SnapshotWeights` (the MemoryId-11 value),
  the multiplier table (`snapshot_weights`), 3USD `apply_verification`, `min_by_total`
  (keeps the smaller-TOTAL snapshot whole, not field-wise), `scale_by_period`,
  `accrue_principal` (close-time), `build_snapshot_inputs` (AMM share + vp), and the
  `repayment_points` window math. ~29 unit tests.
- `epoch.rs` — the periodic state-machine driver (`next_action`), chunked async
  snapshot capture with resume cursor, `start_season` bootstrap, epoch close, the
  commit-reveal `summary_hash`, and all inter-canister `fetch_*` helpers.
- `valuation.rs` — `value_usd_e8s` (face + ck `*100` + ICP oracle, non-finite-guarded),
  `value_lp_at_vp`, `value_stable_usd_e8s`.
- `snapshot_seed.rs` — `derive_snapshot_times` + `SeedManager::start_epoch/close_epoch`
  implemented per spike 0.3 (13 tests incl. the 3-epoch hash chain).

## Stable memory map (MemoryId -> structure; never reuse an id)
| Id | Structure |
|----|-----------|
| 0  | `StableBTreeMap<Principal, StoredPrincipalState>` (per-principal accrual) |
| 1/2| `StableLog<StoredPointEntry>` (append-only audit ledger) |
| 3/4| `StableLog<StoredEpochSummary>` (per-epoch rollups) |
| 5/6| `StableLog<StoredRevealedSeed>` (commit-reveal audit log; Phase 5, wired) |
| 7  | singleton `State` blob (8-byte LE len prefix + CBOR `StoredState::V2`) |
| 8  | per-source ingestion cursors |
| 9  | per-source canister ids (mainnet-seeded) |
| 10 | poll-timer config |
| 11 | `StableBTreeMap<Principal, StoredSnapshotWeights>` (open-epoch min buffer) |
| 12 | asset-ledger registry (asset tag -> ledger; mainnet-seeded, admin override) |
| 13 | epoch-driver config (enabled / interval) |

`State` is now `StoredState::{ V1(StateV1), V2(State) }`: V2 adds `open_epoch`. Old
V1 blob bytes decode via `From<StateV1>` (defaulting `open_epoch=None`); no wipe.

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

## Operating the epoch driver (Phase 5)
OFF by default (like the poll timer). To run a season: enable the poll + confirm
registrations, then admin `start_season(S0)` (the 32-byte secret seed; commit `H0 =
sha256(S0)` at init via `snapshot_seed_commit`). `start_season` opens epoch 0 and
enables the driver. `get_epoch_status` shows the open epoch + driver state;
`force_epoch_tick` steps the machine manually (ops recovery / E2E);
`get_revealed_seed(i)` + `get_pending_commit` expose the audit chain. Asset/source
ids are admin-overridable for local/test (`set_asset_ledger` / `set_source_canister`).

## Next: Phase 6 (frontend) or merge + deploy this branch (needs Rob's OK).
