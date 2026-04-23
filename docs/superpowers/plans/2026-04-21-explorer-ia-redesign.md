# Explorer IA Redesign — Plan

**Date:** 2026-04-21
**Status:** Implemented. All 7 steps + 4 backlog sessions landed to mainnet by 2026-04-23.
**Scope:** `src/vault_frontend/src/routes/explorer/**` + supporting services. No backend code changes required for Phase 1; backend gaps listed separately.

> **Archive note (2026-04-23):** every Tier 1/2/3 gap listed below either shipped or was explicitly deferred as a v1-acceptable approximation. See the "Implementation order" section at the bottom for the shipped-vs-deferred breakdown. Future work should branch off a fresh plan rather than extending this one.

---

## Direction

**Rumi Explorer is a CDP + AMM protocol observatory, not a chain explorer.** Primary nouns are vaults, pools, tokens, principals, canisters, and events — and the **relationships between them**.

**Signature element — the relationship web.** Every entity page surfaces its joins as first-class UI: a token page shows the pools it trades in, the vaults holding it as collateral, the principals moving it. Every facet in every event row is clickable and lenses the activity stream. This is the cohesion Rob wants — it makes the Explorer feel like "everything connects to everything."

**Secondary signature — the CR dial.** Recurring component across vault rows, address summaries, and a protocol-level CR distribution histogram. Only makes sense in CDP-land.

**Defaults rejected:**
1. "Explorer = blocks + txs + addresses" (Etherscan shape) → Rumi is a protocol, not a chain.
2. "Dashboard = KPI tiles + chart + recent table" → narrative protocol header + lens tabs instead.
3. "Activity = one big mixed feed, filter by type" → facets AND-combined query layer, not OR-tabs.

---

## Three-section IA

Collapses current 17 pages → 3 sections:

### 1. Protocol  (absorbs: landing, /revenue, /pools, /markets, /stats, /liquidations, /risk, /holders)

Narrative health strip + lens tabs. Lens switches re-scope charts + activity on the same URL (`?lens=collateral`).

**Lenses:** Overview · Collateral · Stability Pool · Redemptions · Revenue · DEXs · Admin

**Landing behavior:** `/explorer` defaults to Protocol → Overview for all users (no personalization in v1).

### 2. Activity  (absorbs: /activity, /events)

Universal query layer. Every "see all events" link in the app deep-links here pre-filtered.

- Facets AND-combined: type, token, pool, entity (vault/principal/canister), size, time
- URL-as-state (every facet → query param, shareable)
- Clickable facets in rows (click a principal pill → filter narrows)
- Saved views (localStorage) replace hardcoded pages like /liquidations

### 3. Entities  (absorbs: /vault, /token, /address, /pool, /canister, /event, /dex)

Unified URL namespace: `/e/{type}/{id}`. Every entity page uses the same **4-zone shell**:

1. **Identity** — what it is + invariants
2. **Relationships** — "connected to" grid, every cell links (the web)
3. **Activity** — universal stream, pre-filtered to this entity
4. **Analytics** — entity-specific charts

Rule: no entity page ships without all 4 zones populated. If a backend call is missing for a cell, add it before shipping.

---

## Migration map

| Current URL | New home |
|---|---|
| `/explorer` | `/explorer` = Protocol, Overview lens |
| `/explorer/activity` | `/explorer/activity` (promoted to query layer) |
| `/explorer/events` | merged into `/explorer/activity` |
| `/explorer/liquidations` | `/explorer/activity?type=liquidation` |
| `/explorer/stats` | Protocol → Overview lens |
| `/explorer/holders` | per-token Relationships + `/activity?view=holders` |
| `/explorer/markets` | Protocol → DEXs lens |
| `/explorer/markets/[id]` | `/e/pool/{id}` |
| `/explorer/pools` | Protocol → DEXs lens |
| `/explorer/revenue` | Protocol → Revenue lens |
| `/explorer/risk` | Protocol → Collateral lens |
| `/explorer/vault/[id]` | `/e/vault/{id}` |
| `/explorer/token/[id]` | `/e/token/{id}` |
| `/explorer/address/[principal]` | `/e/address/{principal}` |
| `/explorer/canister/[id]` | `/e/canister/{id}` |
| `/explorer/event/[index]` | `/e/event/{index}` |
| `/explorer/dex/[source]/[id]` | `/e/event/dex:{source}:{id}` |

Old URLs must redirect (301 in the SvelteKit `+page.ts`) so nothing breaks.

---

## Wireframes

### Protocol → Collateral lens

```
┌─────────────────────────────────────────────────────────────────────┐
│  PROTOCOL  · Overview [Collateral] SP Redemptions Revenue DEXs Admin │
├─────────────────────────────────────────────────────────────────────┤
│  COLLATERAL HEALTH                                                  │
│  Aggregate CR 243%  ·  At-risk vaults 4  ·  Liquidated 7d 2         │
│  Global liq threshold 110%  ·  Redeem tier spread 1-3               │
├─────────────────────────────────────────────────────────────────────┤
│  Collateral mix (stacked area)  │  CR distribution (histogram)      │
├─────────────────────────────────────────────────────────────────────┤
│  COLLATERAL ASSETS (table: Token, Price, Deposited, $Value,         │
│  Vaults, Debt, Ceiling, Effective CR — row → /e/token/{id})         │
├─────────────────────────────────────────────────────────────────────┤
│  AT-RISK VAULTS (table sorted by CR asc — row → /e/vault/{id})      │
├─────────────────────────────────────────────────────────────────────┤
│  ACTIVITY (lens-scoped: open/adjust/close/liq/redeem. Admin hidden) │
└─────────────────────────────────────────────────────────────────────┘
```

Key choices: no empty $0 cells (hide columns with no data for this token); CR distribution histogram is the signature scaled to protocol level; the three views (mix, histogram, at-risk) together answer "is the protocol healthy?"

### Entity shell: /e/vault/{id}

```
┌─────────────────────────────────────────────────────────────────────┐
│  VAULT #42  [Active]                                                │
│  Owner hg7sz…4mqe 🔗  ·  Opened 2026-02-14  ·  Rate 4.8%            │
├─────────────────────────────────────────────────────────────────────┤
│  [CR dial — signature component]  │  Collateral  1,240 ICP  $6,820  │
│                                   │  Debt        2,800 icUSD        │
│                                   │  Liq price   $2.61 / ICP        │
│                                   │  Headroom    $3,950             │
│                                   │  Redeem tier 1                  │
├─────────────────────────────────────────────────────────────────────┤
│  RELATIONSHIPS                                                      │
│  Owner (+ other vaults) │ Collateral token │ Touched by             │
│  (liquidators, redeemers — click → their address/event pages)       │
├─────────────────────────────────────────────────────────────────────┤
│  ACTIVITY (vault-scoped)                                            │
├─────────────────────────────────────────────────────────────────────┤
│  ANALYTICS                                                          │
│  CR-over-time (dual axis CR% + ICP price overlay, liq zone shaded)  │
│  Debt timeline   │   Collateral timeline                            │
└─────────────────────────────────────────────────────────────────────┘
```

**Closed vaults show full historical state** (collateral/debt at close, final CR, timelines still render — just end at close).

### Entity shell: /e/token/{id}

```
┌─────────────────────────────────────────────────────────────────────┐
│  icUSD  (ledger t6bor-…)                                            │
│  Supply $X · Holders N · Price $1.0003 · Peg +0.03%                 │
├─────────────────────────────────────────────────────────────────────┤
│  RELATIONSHIPS                                                      │
│  Pools trading it │ Used as debt in N vaults │ Top holders │ Top movers 24h │
├─────────────────────────────────────────────────────────────────────┤
│  ACTIVITY (token-scoped)                                            │
├─────────────────────────────────────────────────────────────────────┤
│  ANALYTICS                                                          │
│  Holder distribution pie  │  Supply over time (mints − burns)       │
│  Flow (SP ↔ AMM ↔ wallets)  │  Peg deviation timeseries             │
└─────────────────────────────────────────────────────────────────────┘
```

### Entity shell: /e/address/{principal}

```
┌─────────────────────────────────────────────────────────────────────┐
│  hg7sz-…-4mqe  🔗  [Principal]                                      │
│  Net value $18,430 · First seen 2025-11-03 · Last active 2h ago     │
│  3 vaults · 2 LP positions · 5 tokens held                          │
├─────────────────────────────────────────────────────────────────────┤
│  PORTFOLIO                                                          │
│  Allocation donut (vault equity / LP / liquid)  │  Balance timeline │
├─────────────────────────────────────────────────────────────────────┤
│  VAULTS table (mini CR dials per row → /e/vault/{id})               │
├─────────────────────────────────────────────────────────────────────┤
│  LP POSITIONS table (→ /e/pool/{id})                                │
├─────────────────────────────────────────────────────────────────────┤
│  TOKEN BALANCES table (→ /e/token/{id})                             │
├─────────────────────────────────────────────────────────────────────┤
│  RELATIONSHIPS                                                      │
│  Top counterparties · Canisters interacted with · Sub-accounts seen │
├─────────────────────────────────────────────────────────────────────┤
│  ACTIVITY (everything touching this principal)                      │
└─────────────────────────────────────────────────────────────────────┘
```

Three role-sections (Vaults / LP / Balances) instead of one generic holdings list — each role has different columns and different follow-up links.

### Activity page (facet filter)

```
┌─────────────────────────────────────────────────────────────────────┐
│  ACTIVITY                                                           │
├─────────────────────────────────────────────────────────────────────┤
│  FACET BAR (AND-combined, every change → URL param)                 │
│  Type [▼] Token [▼] Pool [▼] Entity [🔍] Size [>$N] Time [1h/24h/7d/…] │
│  Active: type:swap,liq  token:icp,icusd  >$1k  24h   [Clear] [Save…]│
├─────────────────────────────────────────────────────────────────────┤
│  SAVED VIEWS · My vaults · Liquidations 7d · 3pool swaps >$10k · …  │
├─────────────────────────────────────────────────────────────────────┤
│  263 events  [Export CSV]  sort [newest▼]                           │
│  (MixedEventsTable rows — every facet pill in-row is clickable      │
│   and adds itself to the current filter)                            │
│  [Load more]                                                        │
└─────────────────────────────────────────────────────────────────────┘
```

---

## Relationships spec per entity type

| Entity | Relationships panel shows |
|---|---|
| **Token** | Pools trading it · Vaults using as collateral · Top holders · Top movers 24h |
| **Vault** | Owner (+ other vaults) · Collateral token · Debt token · Liquidators · Redeemers |
| **Pool** | Tokens in pool · LP holders · Routes that use it (multi-hop) · Recent swappers |
| **Address** | Vaults owned · LP positions · Token balances · Canisters interacted · Top counterparties |
| **Canister** | Tokens managed · Pools managed · Principals that called it · Controllers |
| **Event** | "Entities touched" — every vault/token/pool/principal/canister referenced |

---

## Backend endpoint inventory (summary)

Full catalog is by canister. Canisters + headline endpoints:

- **rumi_protocol_backend** — `get_all_vaults`, `get_vaults(p)`, `get_vault_history`, `get_events`, `get_events_by_principal`, `get_events_filtered`, `get_protocol_status`, `get_protocol_snapshots`, `get_collateral_totals`, `get_liquidatable_vaults`, ~18 other queries
- **rumi_analytics** — pre-computed timeseries (TVL, vault, liquidation, swap, three-pool, stability, fee, holder, price, cycle, fee-curve) + `get_protocol_summary`, `get_peg_status`, `get_ohlc`, `get_twap`, `get_volatility`, `get_apys`, `get_trade_activity`
- **rumi_3pool** — very rich: `get_pool_state/status/stats/health`, `get_top_swappers(window,n)`, `get_top_lps(n)`, `get_swap/liquidity_events_by_principal`, `get_volume/balance/fee/virtual_price_series`, `get_imbalance_history`, `get_all_lp_holders`
- **rumi_amm** — `get_pools`, `get_pool(text)`, `get_lp_balance(pool,p)`, `get_holder_snapshots`, `get_amm_swap/liquidity/admin_events`
- **rumi_stability_pool** — `get_pool_status`, `get_user_position`, `get_liquidation_history`, `get_pool_events`
- **icusd_index / threeusd_index** — `get_account_transactions`, `get_blocks`, `icrc1_balance_of`, `list_subaccounts`
- **liquidation_bot** — `get_bot_stats`, `get_liquidation_events`
- **rumi_treasury** — `get_status`, `get_deposits`, `get_events`

Frontend already exposes 45 wrappers in `src/vault_frontend/src/lib/services/explorer/explorerService.ts` plus an `analyticsService.ts`.

---

## Gap list — ranked

**Tier 1 — blocks core IA goals:**
1. **Server-side event filtering** — `get_events_filtered` takes only `(start, length)`. Add filter params (type set, token, principal, time range, min size). Without this, `/activity` falls back to client-side filtering (fine at current scale, breaks at ~10k+).
2. **Top holders for icUSD and 3USD** — `analytics.get_holder_series` gives a count, not a list. Token page Relationships + holder pie needs `get_top_holders(token_id, n)`.
3. **Address aggregator** (nice to have) — replace N+1 calls (`get_vaults` + per-pool `get_lp_balance` + per-token `icrc1_balance_of`) with a single `rumi_analytics.get_address_summary(p)`.

**Tier 2 — analytics depth for Rob's dream:**
4. Top counterparties per principal (derive client-side first, pre-compute later)
5. Token flow Sankey (SP ↔ AMM ↔ wallets) — biggest new backend work
6. **AMM time-series parity with 3pool** — AMM has events but no `get_volume_series` / `get_top_swappers`. Pool page shape is inconsistent.
7. Top depositors in stability pool
8. Routes using pool (multi-hop path stats per pool)

**Tier 3 — nice to have:**
9. Address value-over-time (expensive: historical balances × historical prices)
10. Labeled admin event filter on backend events

**Zero-gap (ship first):** Protocol page all 7 lenses, `/e/vault/{id}`, `/e/pool/{id}` for 3pool, `/e/event/{id}`, `/activity` with client-side facets.

---

## Implementation order

Original plan:

1. **Unify event renderers** — collapse `eventFormatters.ts` + `explorerFormatters.ts` into one. Prerequisite for everything below because the same event now renders in 4 surfaces (Activity, entity streams, event detail, every Protocol lens). **Shipped:** PR #82.
2. **Protocol page + 7 lenses** — all zero-gap, biggest visible upgrade. **Shipped:** PR #83.
3. **Entity shell template + migrate `/vault`, `/pool` (3pool), `/event`** — establishes the shell pattern. **Shipped:** PR #84.
4. **/activity facet bar (client-side filtering)** — establishes the query layer, enables deep-links from everywhere. **Shipped:** PR #85.
5. **Migrate `/address`** using N+1 calls — add aggregator endpoint later. **Shipped:** PR #86.
6. **`/token`** — requires Tier 1 gap #2 (top holders endpoint) before shipping meaningfully. **Shipped:** PR #87 (backend) + PR #88 (UI).
7. **AMM pool parity** — Tier 2 gap #6; AMM pool pages degraded until then. **Shipped:** PR #89 (backend) + PR #90 (UI).

Post-redesign backlog (closes Tier 1-3 gaps from the list above):

- **Session A — server-side event filter dispatch** (Tier 1 #1). Backend PR #91 adds real filter args to `get_events_filtered`; frontend PR #92 wires the Activity page's facet bar into the typed endpoint call.
- **Session B — top-N principals + admin breakdown** (Tier 2 #4, Tier 2 #7, Tier 3 #10). Backend PR #93 and PR #94 ship the three aggregated endpoints (`get_top_counterparties`, `get_top_sp_depositors`, `admin_labels` filter + `get_admin_event_breakdown`); frontend PR #95 surfaces them on `/e/address/{principal}`, `/protocol?lens=stability`, and the Admin chip's sub-facet list.
- **Session C — flow aggregator** (Tier 2 #5, Tier 2 #8). Backend PR #96 ships the shared swap+liquidity walker with two endpoints (`get_token_flow`, `get_pool_routes`); frontend PR #97 wires them into the Protocol Sankey, `/e/token/{id}` Relationships, and `/e/pool/{id}` Relationships with multi-hop badges.
- **Session D — address value-over-time** (Tier 3 #9). Backend PR #98 adds `get_address_value_series` (with a followup PR #99 hotfix for legacy `Fast3PoolSnapshot` rows); frontend PR #100 wires the stacked-area **Portfolio value** chart into `/e/address/{principal}` with a 7D / 30D / 90D / 1Y / All window selector.

**Deferred (Phase 2, not in this plan):**
- Watching / bookmarking (cool idea, revisit later)
- Personalized landing ("My positions" for logged-in users)
- CSV export on Activity (small, add when someone needs it)
- ICRC-3 per-delta ledger log → would promote icUSD / 3USD bands in the portfolio chart from "current-balance projection" to full historical reconstruction.
- AMM liquidity event tailer → would add an "AMM LP" band to the portfolio chart and let `/e/pool/{id}` (AMM) ship a holder snapshot + routes feed without the current 3pool-only shortcuts.
- Standalone non-stable ledger balances (ICP, ckBTC, ckETH, nICP, BOB, EXE, ckXAUT) in the portfolio chart. Currently the chart covers 5 of 6 position types named in the Tier 3 #9 scope; these are the 6th and require either a per-ledger tailer or a client-side projection layer.
- Backfill legacy `Fast3PoolSnapshot` rows so `range()` can iterate them (or make `decimals` optional); PR #99 sidesteps this with a latest-snapshot-only lookup for virtual_price.

---

## Open decisions locked in this session

- Lens name: **DEXs** (covers 3pool + AMM)
- **Admin** lens exists; admin events filtered out of other lenses by default
- Landing: Protocol → Overview for all users
- Closed vaults: full historical state (not gravestones)
- Watching/bookmarking: skipped for v1
