# Airdrop Phase 6 — frontend design

- **Date:** 2026-06-08
- **Status:** approved (brainstorm), pending spec review
- **Scope:** Phase 6 of the airdrop. The public-facing UI in `vault_frontend`
  for the `rumi_points` accrual engine.
- **Related:** `src/rumi_points/STATUS.md`, `src/rumi_points/DEPLOY_RUNBOOK.md`,
  the airdrop spec (`docs/specs/rumi-airdrop-spec-v2.md`, local).

## Goal

Give users a public surface to (1) see whether they're earning airdrop points
and how to start, (2) view their own points and rank, and (3) browse a public
leaderboard. The `rumi_points` canister (Phases 1-5) is the backend; this phase
is read-only UI on top of it.

## Key constraint that shapes everything

**The entire feature is read-only.** Every frontend-relevant endpoint is a public
`query`, and there is **no user-facing register or claim endpoint**:

- Registration is **automatic** — a wallet is enrolled on its first qualifying
  in-season action (mint icUSD, deposit to the stability pool, add 3pool
  liquidity), picked up by the canister's poller. `register_test_principal` is
  admin-only and not used by this UI.
- Claiming is **out of scope** (Phases 7-8, a separate claim canister).

So the wallet connection only tells us *which* principal to query — there is no
signing, no PnP authenticated actor, and no on-chain write anywhere in Phase 6.

## Decisions locked (brainstorm)

1. **Enrollment surface = status + earn CTA.** A "My Points" view shows enrollment
   status; if not enrolled, it explains the qualifying actions and deep-links to
   the existing flows. No on-chain write.
2. **Points + rank only.** Do **not** surface `estimated_share_bps` anywhere — it's
   a fluctuating live estimate and the true allocation is computed later from the
   frozen ledger. Avoids setting precise airdrop-size expectations.
3. **New top-level `Points` section** with `/points` (My Points) and
   `/points/leaderboard`.

## Approach

Mirror the existing explorer/analytics pattern (least new surface area, consistent
with conventions):

- `src/lib/services/pointsService.ts` — anonymous `HttpAgent` + lazy actor +
  TTL-cached query functions, modeled on `analyticsService.ts`.
- Thin Svelte stores for the connected user's points state; reactive refetch on
  `principal` change (from the existing `walletStore` / `principal` derived store).
- Reuse `DataTable.svelte` for the leaderboard and the existing card/metric/
  `EmptyState`/toast patterns for everything else.

*Rejected alternatives:* folding points into the central `appDataStore` (couples
it into a large shared store — invasive); building bespoke components instead of
reusing `DataTable` (more control, more divergence).

## Routes & navigation

- New top-level `Points` nav item in `+layout.svelte`.
- `/points` — My Points (default landing).
- `/points/leaderboard` — ranked table.
- The nav item and routes are **gated on `CANISTER_ID_RUMI_POINTS` being
  configured** (via `config.ts`), so the section stays hidden until the canister
  is deployed and its id is added to config. No dead links pre-launch.

## My Points page (`/points`)

A **season banner** at the top, driven by `get_epoch_status()`
(`PublicEpochStatus`):

- No canister id / not deployed → section hidden (see route gating).
- Deployed, `open_epoch == None` and `!driver_enabled` → "Season 1 starts soon".
- `open_epoch == Some` → "Season 1 live — Epoch N" + countdown to
  `season_end_ns` (from `get_points_config`).
- Now past `season_end_ns` → "Season 1 ended — allocations being finalized;
  claiming coming soon".

Body states:

| State | Condition | Content |
|-------|-----------|---------|
| Disconnected | no wallet | Connect-wallet prompt + short "what is this" + the earn CTA |
| Connected, not enrolled | `is_registered(p) == false` | "You're not earning yet" + earn CTA |
| Connected, enrolled | `get_principal_state(p) == Some` | Points dashboard (below) + secondary "earn more" CTA |

**Earn CTA** — a list of qualifying actions, each deep-linking to the existing
flow:
- Mint / borrow icUSD → `/`
- Deposit to the stability pool → `/stability-pool`
- Provide 3pool / AMM liquidity → `/liquidity`

**Points dashboard** (enrolled), from `PrincipalState`:
- `total_points` — primary stat, formatted as USD-days (see Formatting).
- Rank — best-effort (see Rank lookup); always show points even when rank is unknown.
- Enrolled-since — from `registered_at_ns`.
- First qualifying action — from `first_qualifying_action`.
- Earning breakdown — a small list derived from `active_deposits` showing where
  points are currently accruing (vault debt, SP, 3pool, AMM), so users see which
  positions count.

Edge: if `is_excluded(p)` (protocol-owned canisters), show a brief "this address
is excluded" note instead of the dashboard. Normal users never hit this.

## Leaderboard page (`/points/leaderboard`)

- Header stats from `get_points_config()`: total participants
  (`registered_count`) and the season window.
- Paginated `DataTable` over `get_leaderboard(offset, limit)`
  (`vec LeaderboardEntry`): columns **rank**, **principal** (truncated
  `abcd…wxyz`, with copy + link to the explorer address page), **total points**
  (USD-days). `estimated_share_bps` is fetched-but-ignored (not rendered).
- The connected user's row is highlighted when present on the current page.
- Empty state via `EmptyState.svelte` when there are no entries yet (pre-accrual).

### Rank lookup

There is **no `rank` field on `PrincipalState` and no `get_my_rank` endpoint** —
`rank` exists only on `LeaderboardEntry`. So a user's rank is derived by locating
their principal in `get_leaderboard` output. Approach for launch: fetch a bounded
top slice (e.g. top 1000) and, if the principal appears, show that `rank`;
otherwise show `total_points` with rank as "unranked / outside top N" rather than
paging the entire set. This keeps the My Points page to one bounded query and
avoids unbounded scans. (If precise deep ranks become important, add a dedicated
canister endpoint later — out of scope here.)

## Data layer

`pointsService.ts` exported functions (all anonymous public queries, TTL-cached):

| Function | Canister method | Notes |
|----------|-----------------|-------|
| `getEpochStatus()` | `get_epoch_status` | season banner; short TTL (~15s) |
| `getPointsConfig()` | `get_points_config` | header stats; medium TTL (~60s) |
| `isRegistered(p)` | `is_registered` | enrollment gate |
| `getPrincipalState(p)` | `get_principal_state` | personal dashboard |
| `isExcluded(p)` | `is_excluded` | excluded-address edge |
| `getLeaderboard(offset, limit)` | `get_leaderboard` | paginated table |

Stores: a `myPointsStore` (writable) holding `{ registered, state, loading,
error }`, refetched reactively when `principal` changes; leaderboard fetched
per-page in the page component (no global store needed).

## Formatting

`total_points` is `nat` in **`usd_e8s`-days** (USD value ×1e8 held over time in
days). Display = `Number(total_points) / 1e8`, rendered as a grouped number
labeled "USD-days" (e.g. holding $100 for 1 day → `100` USD-days). Use `bigint`
math before the divide to avoid precision loss; a shared `formatPoints()` helper
in `src/lib/utils`.

## Error / empty / loading

- Loading: existing spinner / skeleton pattern; never a blank page.
- Query failure: non-blocking toast + inline retry; stale cache shown if present.
- Empty leaderboard / no points yet: `EmptyState.svelte` with copy pointing to
  the earn CTA.

## Components

- **Reuse:** `DataTable.svelte`, `EmptyState.svelte`, toast store, card/metric
  styling (`ProtocolVitals`-style), explorer address-link helper, app.css tokens.
- **New:** `routes/points/+page.svelte`, `routes/points/leaderboard/+page.svelte`,
  a `SeasonBanner.svelte`, an `EarnCta.svelte`, a `PointsSummary.svelte`
  (dashboard card), `pointsService.ts`, `myPointsStore`, `formatPoints()`.

## Out of scope (Phase 6)

Claiming (Phases 7-8), `estimated_share_bps` display, any admin/operate controls,
any on-chain writes, historical per-epoch charts (could reuse `get_epoch_history`
later).

## Testing

- Unit: `formatPoints()` (e8s-days → USD-days, large values, zero).
- Component/logic: state selection (disconnected / not-enrolled / enrolled /
  excluded), season-banner state from `PublicEpochStatus` + `season_end_ns`.
- Manual against a local replica with a registered test principal
  (`register_test_principal`) once the canister is deployed locally; verify the
  three body states, the leaderboard pagination, and the self-row highlight.

## Launch dependency

The section is hidden until `CANISTER_ID_RUMI_POINTS` is configured. It can be
built and merged now; it goes live when the canister is deployed (trio deploy per
`DEPLOY_RUNBOOK.md`) and its id is added to `config.ts`.
