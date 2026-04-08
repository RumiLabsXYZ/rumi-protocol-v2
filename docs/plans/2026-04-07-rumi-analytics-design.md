# rumi_analytics Design

**Date**: 2026-04-07
**Status**: Design approved, pending spec review and implementation plan
**Author**: Brainstormed with Claude

## Motivation

Rumi Protocol v2 has no centralized analytics today. Three concrete needs are converging:

1. **CoinGecko listing** requires a public HTTP `/api/supply` endpoint for icUSD circulating supply.
2. **Explorer holders page** (work in progress in `src/vault_frontend/src/routes/explorer/holders/`) needs top-N holder data and 24h historical snapshots for icUSD and 3USD.
3. **Trader, arbitrageur, and protocol-health analytics** (TVL, vault distribution, swap volumes, peg deviation, liquidation queue depth, quant signals) for human consumption via the explorer and for external researchers.

Three options were considered for where this lives:

- **icusd_index / threeusd_index**: ruled out. They are stock DFINITY `ic-icrc1-index-ng` wasms (confirmed in `dfx.json`), not forks we can modify.
- **rumi_amm**: ruled out. Mixing observability state into a money-handling canister is a code smell, and the AMM still uses the heap-blob upgrade pattern that 3pool just spent a week migrating away from in PR #59. Adding unbounded snapshot data to the AMM heap recreates the upgrade-trap risk we just eliminated.
- **A new dedicated `rumi_analytics` canister**: chosen. ~$1-2 deploy cost, isolates analytics from money-handling canisters, gives us a clean home for everything we will want to add over time.

## Critical design rule

**All snapshot and historical data lives in stable memory from day one.** `StableLog` for append-only series, `StableBTreeMap` for keyed lookups, `StableCell` for config. Never put unbounded collections on the heap. We just fixed this exact mistake in 3pool (Phase A migration in `src/rumi_3pool/src/storage.rs` and `src/rumi_3pool/src/state.rs`). This canister reuses that pattern from line one.

## Architectural decisions (locked during brainstorming)

1. **Pull-based event capture, not push.** A 60s timer in analytics pulls cursored event streams from each source canister. Source canisters never call analytics. This means analytics being down, mid-upgrade, or out of cycles can never affect swaps, mints, or liquidations. New event types can be added without coordinated upgrades. Latency is ~60s, which is acceptable for everything except live arbitrage queries (which are on-demand anyway).
2. **Forever retention, no downsampling.** Stable memory is cheap on IC ($5/GB/year) and the data volumes for this protocol are small enough that downsampling logic would be more bug surface than benefit. Raw truth is preserved for forensics. Rollup series can be added later as a non-breaking addition if query latency ever becomes a real problem.
3. **Ledger replay for both icUSD and 3USD holders.** A single `BalanceTracker` module tails ICRC-3 blocks from both ledgers via cursors and maintains `StableBTreeMap<Account, u128>` balance maps plus `StableBTreeMap<Account, FirstSeen>` first-seen maps. This one subsystem powers every metric in the "Holders & distribution" section.
4. **Best-effort observability, never on the money path.** Analytics is for humans (explorer, CoinGecko, historical research) and for non-trading external consumers. Trading bots (including the user's own bot) query source canisters directly for their hot path. The live-query endpoints exist for exploration and are explicitly documented as "not for production trading." This stance lets us upgrade analytics without coordinating with downstream consumers, and removes analytics from the critical-infrastructure tier.
5. **On-demand compute for live queries, no caching tier.** TWAPs, VWAPs, realized vol, EV calc, cascade simulator, etc., are computed fresh from `StableLog` reads on every call. Data volumes are small enough that on-demand reads are fast. No cache means no cache-coherency bugs. The sole exception is the BalanceTracker, which is a `StableBTreeMap` that is the source of truth for holder data, not a cache.

## Canister architecture

### Crate layout

New Rust workspace member at `src/rumi_analytics/`, package name `rumi_analytics`, registered in `dfx.json` as a `rust` canister with `rumi_analytics.did`. Declarations generated into `src/declarations/rumi_analytics/` and tracked in git per project convention.

```
src/rumi_analytics/
  Cargo.toml
  rumi_analytics.did
  src/
    lib.rs              // canister entry: init/post_upgrade/pre_upgrade, candid_method exports
    state.rs            // SlimState + hydrate_from_slim + snapshot_slim (mirrors 3pool/state.rs)
    storage.rs          // MemoryId constants, StableLog/StableBTreeMap/StableCell handles
    types.rs            // shared candid types (snapshot rows, query params, responses)
    sources/            // one file per source canister, typed pull functions
      backend.rs        // rumi_protocol_backend queries
      threepool.rs      // rumi_3pool queries
      amm.rs            // rumi_amm queries
      stability.rs      // rumi_stability_pool queries
      icusd_ledger.rs   // icusd_ledger ICRC-3 blocks cursor
      xrc.rs            // (read from backend snapshot, not direct)
    collectors/         // one file per metric family, owns its cadence + stable log
      tvl.rs
      holders.rs        // BalanceTracker + daily holder snapshots, Gini, churn
      liquidations.rs
      swaps.rs
      prices.rs
      vaults.rs
      cycles.rs
    queries/            // on-demand readers from stable storage
      historical.rs     // paginated series readers
      live.rs           // TWAP, VWAP, realized vol, EV calc, cascade sim, queue depth
      holders.rs        // top-N, Gini, concentration
      leaderboards.rs
    http.rs             // ic-canisters-http-types router
    timers.rs           // setup_timers(): one timer per cadence tier
    backfill.rs         // one-shot historical backfills (admin-gated)
```

### State discipline (non-negotiable)

Reused verbatim from the 3pool Phase A migration:

- `SlimState` lives in `StableCell<MemoryId(0)>` and holds only: admin principal, source-canister ids config, last-snapshot timestamps per collector, per-source error counters, and the cached `circulating_supply_icusd` / `circulating_supply_3usd` values that the HTTP `/api/supply` endpoint reads (refreshed by the pull cycle, see below).
- **Cursors are NOT in SlimState.** Each source stream gets its own dedicated `StableCell<u64>` at a reserved MemoryId so cursor advancement is atomic with the StableLog write that uses it. This avoids the failure mode where a SlimState write succeeds but the corresponding event log write fails (or vice versa). Bookmark and cursor are written in the same tier callback.
- All series go to `StableLog`s with reserved MemoryIds.
- All keyed lookups go to `StableBTreeMap`s.
- `pre_upgrade` does `set_slim(snapshot_slim())`. That is the entire body. No giant blob serialization.
- `post_upgrade` calls `hydrate_from_slim(get_slim())`, then re-arms timers via `setup_timers()`. No legacy drain path needed (greenfield canister).
- Heap holds only: ephemeral computation buffers, the live `SlimState` mirror, in-flight cursor results.

### Authorization model

- All mutating endpoints (config setters, backfill triggers, `set_admin`, `set_collector_enabled`, `set_top_n_holders`, `set_source_canister_id`) gated to a single `admin: Principal` set at init, mutable by the admin. Same pattern as 3pool.
- All snapshot and query endpoints are public `query` calls.
- All HTTP endpoints are public.
- The canister has zero authority over any other canister. It only makes inter-canister `query` calls (never `update`) to source canisters. Even if analytics is fully compromised, no money moves.

## Storage layout and MemoryId map

Reserve MemoryIds 0-63 with deliberate gaps so each metric family has room to grow. Defined in `storage.rs`:

```rust
pub const MEM_SLIM_STATE: MemoryId = MemoryId::new(0);

// Cursors (StableCell<u64> each, one per source stream)
pub const MEM_CURSOR_BACKEND_EVENTS:    MemoryId = MemoryId::new(1);
pub const MEM_CURSOR_3POOL_SWAPS:       MemoryId = MemoryId::new(2);
pub const MEM_CURSOR_3POOL_LIQUIDITY:   MemoryId = MemoryId::new(3);
pub const MEM_CURSOR_3POOL_BLOCKS:      MemoryId = MemoryId::new(4);
pub const MEM_CURSOR_AMM_SWAPS:         MemoryId = MemoryId::new(5);
pub const MEM_CURSOR_STABILITY_EVENTS:  MemoryId = MemoryId::new(6);
pub const MEM_CURSOR_ICUSD_BLOCKS:      MemoryId = MemoryId::new(7);
// 8-9 reserved

// Daily snapshot logs (each StableLog needs index + data memories)
pub const MEM_DAILY_TVL_IDX:            MemoryId = MemoryId::new(10);
pub const MEM_DAILY_TVL_DATA:           MemoryId = MemoryId::new(11);
pub const MEM_DAILY_VAULTS_IDX:         MemoryId = MemoryId::new(12);
pub const MEM_DAILY_VAULTS_DATA:        MemoryId = MemoryId::new(13);
pub const MEM_DAILY_HOLDERS_ICUSD_IDX:  MemoryId = MemoryId::new(14);
pub const MEM_DAILY_HOLDERS_ICUSD_DATA: MemoryId = MemoryId::new(15);
pub const MEM_DAILY_HOLDERS_3USD_IDX:   MemoryId = MemoryId::new(16);
pub const MEM_DAILY_HOLDERS_3USD_DATA:  MemoryId = MemoryId::new(17);
pub const MEM_DAILY_LIQUIDATIONS_IDX:   MemoryId = MemoryId::new(18);
pub const MEM_DAILY_LIQUIDATIONS_DATA:  MemoryId = MemoryId::new(19);
pub const MEM_DAILY_SWAPS_IDX:          MemoryId = MemoryId::new(20);
pub const MEM_DAILY_SWAPS_DATA:         MemoryId = MemoryId::new(21);
pub const MEM_DAILY_FEES_IDX:           MemoryId = MemoryId::new(22);
pub const MEM_DAILY_FEES_DATA:          MemoryId = MemoryId::new(23);
pub const MEM_DAILY_STABILITY_IDX:      MemoryId = MemoryId::new(24);
pub const MEM_DAILY_STABILITY_DATA:     MemoryId = MemoryId::new(25);
// 26-29 reserved

// 5-minute snapshot logs
pub const MEM_FAST_PRICES_IDX:          MemoryId = MemoryId::new(30);
pub const MEM_FAST_PRICES_DATA:         MemoryId = MemoryId::new(31);
pub const MEM_FAST_3POOL_IDX:           MemoryId = MemoryId::new(32);
pub const MEM_FAST_3POOL_DATA:          MemoryId = MemoryId::new(33);
// 34-37 reserved

// Hourly snapshot logs
pub const MEM_HOURLY_CYCLES_IDX:        MemoryId = MemoryId::new(38);
pub const MEM_HOURLY_CYCLES_DATA:       MemoryId = MemoryId::new(39);
pub const MEM_HOURLY_FEE_CURVE_IDX:     MemoryId = MemoryId::new(40);
pub const MEM_HOURLY_FEE_CURVE_DATA:    MemoryId = MemoryId::new(41);
// 42-43 reserved

// Per-event mirror logs
pub const MEM_EVT_LIQUIDATIONS_IDX:     MemoryId = MemoryId::new(44);
pub const MEM_EVT_LIQUIDATIONS_DATA:    MemoryId = MemoryId::new(45);
pub const MEM_EVT_SWAPS_IDX:            MemoryId = MemoryId::new(46);
pub const MEM_EVT_SWAPS_DATA:           MemoryId = MemoryId::new(47);
pub const MEM_EVT_LIQUIDITY_IDX:        MemoryId = MemoryId::new(48);
pub const MEM_EVT_LIQUIDITY_DATA:       MemoryId = MemoryId::new(49);
pub const MEM_EVT_VAULTS_IDX:           MemoryId = MemoryId::new(50);
pub const MEM_EVT_VAULTS_DATA:          MemoryId = MemoryId::new(51);
// 52-55 reserved

// BalanceTracker maps (StableBTreeMap<Account, u128>)
pub const MEM_BAL_ICUSD:                MemoryId = MemoryId::new(56);
pub const MEM_BAL_3USD:                 MemoryId = MemoryId::new(57);
// First-seen maps (StableBTreeMap<Account, u64> = first-non-zero timestamp)
pub const MEM_FIRSTSEEN_ICUSD:          MemoryId = MemoryId::new(58);
pub const MEM_FIRSTSEEN_3USD:           MemoryId = MemoryId::new(59);
// 60-63 reserved
```

**Structure type per MemoryId**:
- MemoryId 0: `StableCell<SlimState>`.
- MemoryIds 1-7: `StableCell<u64>` (one per cursor).
- MemoryIds 10-51 (paired IDX/DATA): `StableLog<RowType>`.
- MemoryIds 56-59: `StableBTreeMap`.

This labeling is non-negotiable. Wrapping the wrong structure type around a MemoryId at Phase 1 corrupts the layout for the canister's lifetime.

### Row types

Every snapshot row carries a `timestamp_ns: u64` as its first field. That is the natural primary key for time-range queries (binary-searchable on the StableLog index). All row types are versioned via Candid with `Option<T>` for new fields, same upgrade-compat discipline as 3pool's state.

### BalanceTracker FirstSeen semantics

`FirstSeen[account]` is set to the timestamp of the first ICRC-3 block that gives the account a non-zero balance, and is **never overwritten thereafter**. If a holder drains to zero and later receives funds again, FirstSeen is preserved as the original first-seen-ever timestamp. The "new vs returning" daily metric is then computed as: an account is "new today" if `FirstSeen >= today_start`, otherwise "returning". Accounts that go to zero and back are always "returning."

### Pagination scheme

Uniform across every historical endpoint:

```candid
type RangeQuery = record {
  from_ts: opt nat64;       // inclusive, null = beginning
  to_ts: opt nat64;         // exclusive, null = now
  limit: opt nat32;         // default 500, max 2000
  offset: opt nat64;        // for chunked pulls past the limit
};
```

Implementation: binary-search the StableLog index for `from_ts`, walk forward emitting rows until `to_ts` or `limit`. Bounded response size, no full-log scans.

## Metrics catalog

Conventions: `Daily` = 00:00 UTC timer. `Fast` = 5min timer. `Hourly` = top of hour. `Event` = pulled on the 60s cursor poll, mirrored into a per-event StableLog. `Live` = computed on demand, no storage.

### `collectors/tvl.rs` - Daily TVL & system health

| Metric | Source | Cadence | Storage |
|---|---|---|---|
| Total ICP collateral | backend `get_protocol_status` | Daily | DAILY_TVL |
| Total icUSD minted/circulating | icusd_ledger `icrc1_total_supply` | Daily | DAILY_TVL |
| Total 3pool reserves per coin | 3pool `get_pool_state` | Daily + Fast | DAILY_TVL + FAST_3POOL |
| System collateralization ratio | backend (derived) | Daily | DAILY_TVL |
| Stability pool TVL, depositor count, avg deposit | stability_pool `get_pool_stats` | Daily | DAILY_STABILITY |
| Stability pool realized ICP gains, APY estimate | stability_pool | Daily | DAILY_STABILITY |

### `collectors/vaults.rs` - Daily vault behavior

| Metric | Source | Cadence | Storage |
|---|---|---|---|
| Vault count: open / opened today / closed today / liquidated today | backend (derived) | Daily | DAILY_VAULTS |
| Avg & median vault collateral and debt | backend `list_open_vaults` (NEW) | Daily | DAILY_VAULTS |
| CR distribution histogram (10 buckets) | backend `list_open_vaults` (NEW) | Daily | DAILY_VAULTS |
| Largest vaults top-N (collateral, debt) | backend `list_open_vaults` (NEW) | Daily | DAILY_VAULTS |
| Vault lifetime distribution | derived from EVT_VAULTS | Live | (computed) |
| Most-active borrowers leaderboard | derived from EVT_VAULTS | Live | (computed) |
| Vault create→first borrow latency | derived from EVT_VAULTS | Live | (computed) |
| Borrow size distribution | derived from EVT_VAULTS | Live | (computed) |

### `collectors/holders.rs` - Holders & distribution

Powered by BalanceTracker: tails ICRC-3 blocks per ledger via cursors, maintains `StableBTreeMap<Account, u128>` and `StableBTreeMap<Account, FirstSeen>`. Updated every 60s as part of the pull cycle.

| Metric | Source | Cadence | Storage |
|---|---|---|---|
| Top-100 holders icUSD / 3USD (configurable N) | BalanceTracker | Daily | DAILY_HOLDERS_* |
| Total holder count | BalanceTracker | Daily | DAILY_HOLDERS_* |
| Gini coefficient | BalanceTracker | Daily | DAILY_HOLDERS_* |
| Top-10, top-100 share | BalanceTracker | Daily | DAILY_HOLDERS_* |
| New vs returning, churn | BalanceTracker + FirstSeen | Daily | DAILY_HOLDERS_* |
| First-seen lookup per account | FirstSeen map | Live | FIRSTSEEN_* |

### `collectors/liquidations.rs`

| Metric | Source | Cadence | Storage |
|---|---|---|---|
| Liquidation events full record | backend events cursor | Event | EVT_LIQUIDATIONS |
| Daily liquidation volume rollup | derived | Daily | DAILY_LIQUIDATIONS |
| Liquidator leaderboard (cumulative) | derived | Live | (computed) |
| Time-to-liquidation per event | derived | Live | (computed) |
| Liquidation profit margin per event | derived | Live | (computed) |

### `collectors/swaps.rs`

| Metric | Source | Cadence | Storage |
|---|---|---|---|
| Swap events full record (3pool) | 3pool swap-events cursor | Event | EVT_SWAPS |
| Swap events full record (AMM) | rumi_amm cursor (NEW) | Event | EVT_SWAPS (tagged by venue) |
| Liquidity events (3pool) | 3pool liquidity-events cursor | Event | EVT_LIQUIDITY |
| Daily volume per pool/pair | derived | Daily | DAILY_SWAPS |
| Daily fee revenue, admin/LP split | derived | Daily | DAILY_FEES |
| 3pool imbalance + virtual price | 3pool `get_virtual_price`, `get_pool_state` | Fast | FAST_3POOL |
| Top-100 largest swaps per pool (rolling) | derived | Live | (computed) |
| Top traders by swap count | derived | Live | (computed) |

### `collectors/prices.rs`

| Metric | Source | Cadence | Storage |
|---|---|---|---|
| ICP/USD from XRC reading | backend `get_protocol_status` | Fast | FAST_PRICES |
| icUSD market price (3pool & AMM derived) | 3pool / amm | Fast | FAST_PRICES |
| 3USD market price | 3pool virtual price | Fast | FAST_PRICES |
| Borrowing fee curve position | backend | Hourly | HOURLY_FEE_CURVE |
| Cumulative stability fee accrual | backend | Daily | DAILY_FEES |

### `collectors/cycles.rs`

**Important**: management canister `canister_status` is an `update` call requiring controller status, which would violate the "zero update calls to source canisters" invariant and require analytics to be a controller of every Rumi canister. Instead, **each Rumi canister exposes its own `get_cycle_balance() -> nat` query method**, callable by anyone. Analytics calls those public queries. This is added to the Phase 0 source-canister query checklist.

| Metric | Source | Cadence | Storage |
|---|---|---|---|
| Cycle balance per canister | each canister's `get_cycle_balance()` (NEW) | Hourly | HOURLY_CYCLES |
| Cycle burn rate | derived | Live | (computed) |
| Time-to-empty projection | derived | Live | (computed) |
| XRC fetch success/failure rate | backend (NEW counters) | Hourly | HOURLY_CYCLES |
| Analytics canister's own cycle balance | `ic_cdk::api::canister_balance()` | Hourly | HOURLY_CYCLES |

### `queries/live.rs` - Quant signals (on-demand, derived from existing logs)

All of these are computed on demand from already-stored data. None require their own snapshot log.

| Metric | Computed from | Notes |
|---|---|---|
| TWAP / VWAP (5min, 1h, 24h, configurable window) | FAST_PRICES, EVT_SWAPS | Window length is a query parameter |
| Rolling realized volatility (1h, 24h, 7d) | FAST_PRICES | |
| Daily OHLC for icUSD / 3USD | FAST_PRICES | |
| Peg deviation (current) | FAST_PRICES (latest row) | History readable via `/api/series/prices` |
| Cross-venue spread | FAST_PRICES (3pool vs AMM legs) | |
| Fee curve position | HOURLY_FEE_CURVE (latest) | |
| Liquidation queue depth | DAILY_VAULTS (latest histogram) plus FAST_PRICES (current ICP price) | Day-stale on the vault list; for fresher data the explorer can call backend's `list_open_vaults` directly |
| Trade size distribution | EVT_SWAPS (last N rows) | |
| Order flow imbalance | EVT_SWAPS (last 1h rows) | |
| Price impact curves | 3pool current state via cached FAST_3POOL row | |
| Stability pool ICP gain projection | DAILY_STABILITY (recent rows) | |
| Liquidation EV calculator | DAILY_VAULTS + FAST_PRICES | |
| Vault risk score | DAILY_VAULTS + FAST_PRICES | |
| 3pool LP APY | FAST_3POOL (virtual price growth) plus DAILY_FEES | |
| Stability pool APY | DAILY_STABILITY | |
| Effective annualized CDP cost | DAILY_FEES + HOURLY_FEE_CURVE | |
| Open interest | DAILY_VAULTS (sum of debt) | |
| Liquidation cascade simulator | DAILY_VAULTS + FAST_PRICES | |

Each function reads a bounded window from the relevant `StableLog` and computes. No caching, no inter-canister calls (so each is callable from a `query` context).

**Note on liquidation queue depth freshness**: the most accurate version requires the current vault list, which is only refreshed daily in DAILY_VAULTS. If the explorer needs sub-daily freshness it should call `rumi_protocol_backend.list_open_vaults` directly rather than going through analytics; analytics will not proxy that call (per the best-effort architecture).

### New source-canister queries needed

Each is additive, no semantic changes. Verified precisely in Phase 0 by reading every source `.did` file:

1. **rumi_protocol_backend**:
   - `list_open_vaults(offset, limit) -> (Vec<VaultSummary>, next_offset)` - paginated, returns id/owner/collateral/debt/cr.
   - `get_events_since(cursor, limit) -> (Vec<BackendEvent>, next_cursor)` - unified event stream (vault open/close/borrow/repay, liquidations).
   - `get_cycle_balance() -> nat` (public query).
   - Optional: `get_xrc_stats() -> XrcStats`.
2. **rumi_3pool**: `get_swap_events_since`, `get_liquidity_events_since`, `get_cycle_balance`. ICRC-3 `icrc3_get_blocks` already exists on the standard interface.
3. **rumi_amm**: `get_swap_events_since`, `get_cycle_balance`. Almost certainly needs to be added since the AMM still uses heap-blob upgrade.
4. **rumi_stability_pool**: `get_events_since` cursor for deposit/withdraw/gain-distribution events, `get_cycle_balance`.
5. **rumi_treasury**, **liquidation_bot**: `get_cycle_balance`.
6. **icusd_ledger / icusd_index**: standard ICRC-3 `get_blocks` already exists on the stock wasm. No changes needed. Cycle balance comes from existing wallet/dashboard tooling and is omitted from analytics.

### Row type sketches (load-bearing for Phase 2 PRs)

```candid
type VaultSummary = record {
  id: nat64;
  owner: principal;
  collateral_e8s: nat;
  debt_e8s: nat;
  cr_bps: nat32;          // collateralization ratio in basis points
  opened_at_ns: nat64;
};

type BackendEvent = record {
  block_index: nat64;
  timestamp_ns: nat64;
  payload: variant {
    VaultOpened: record { id: nat64; owner: principal; collateral_e8s: nat };
    VaultClosed: record { id: nat64 };
    Borrow:      record { id: nat64; amount_e8s: nat };
    Repay:       record { id: nat64; amount_e8s: nat };
    Liquidation: record {
      id: nat64;
      collateral_seized_e8s: nat;
      debt_cleared_e8s: nat;
      liquidator: principal;
      icp_price_at_time_e8s: nat;
    };
  };
};

type SwapEvent = record {
  block_index: nat64;
  timestamp_ns: nat64;
  venue: variant { ThreePool; Amm };
  user: principal;
  in_coin: text; out_coin: text;
  in_amount: nat; out_amount: nat;
  fee: nat;
  realized_slippage_bps: nat32;
};

type LiquidityEvent = record {
  block_index: nat64;
  timestamp_ns: nat64;
  user: principal;
  kind: variant { Add; Remove };
  amounts: vec nat;
  lp_delta: nat;
};

type StabilityEvent = record {
  block_index: nat64;
  timestamp_ns: nat64;
  user: principal;
  kind: variant { Deposit; Withdraw; GainDistribution };
  icusd_amount: nat;
  icp_gain: nat;
};

type XrcStats = record {
  success_count: nat64;
  failure_count: nat64;
  last_error: opt text;
  last_success_ts: nat64;
};
```

These shapes are the contract between source-canister PRs and analytics. Source canisters may already have similar internal types; the Phase 0 audit determines whether to expose those directly or wrap them.

## Cadences, timers, cycle cost

### Four timers, all in `setup_timers()`

```rust
pub fn setup_timers() {
    ic_cdk_timers::set_timer_interval(Duration::from_secs(60),    pull_cycle);
    ic_cdk_timers::set_timer_interval(Duration::from_secs(300),   fast_snapshot);
    ic_cdk_timers::set_timer_interval(Duration::from_secs(3600),  hourly_snapshot);
    ic_cdk_timers::set_timer_interval(Duration::from_secs(86400), daily_snapshot);
}
```

**No `#[ic_cdk::heartbeat]`** - that macro is the #1 cycle burner per project memory and is forbidden.

### Tier 1 - Pull cycle (60s)

Drains every cursor. One inter-canister `query` call per source stream (currently 7 streams). Each call is bounded by `limit` (default 500 events). The cursor advances atomically only after the events are written to the corresponding EVT_* StableLog. If a source is unreachable, that cursor doesn't advance and the next pull retries. **No silent failures**: any non-`Ok` result increments a per-source error counter in `SlimState`, gets logged via `ic_cdk::println!`, and is exposed via `get_collector_health()`. The pull cycle also drives BalanceTracker for icusd_ledger and 3pool blocks.

The pull cycle additionally **refreshes the cached `circulating_supply_*` values** in `SlimState` by calling `icrc1_total_supply` on each ledger and subtracting the configured exclusion list. This is what `/api/supply` reads from a query context.

### Tier 2 - Fast snapshot (5min)

Reads current state from 3pool (`get_pool_state`, `get_virtual_price`) and backend (`get_protocol_status`). Writes one row to FAST_PRICES and one row to FAST_3POOL. ~5 inter-canister calls per tick.

### Tier 3 - Hourly snapshot

Walks every Rumi canister, calls each one's `get_cycle_balance()` query (added in Phase 2, see source-canister query list), writes one HOURLY_CYCLES row containing all balances. Reads borrowing fee curve params, writes one HOURLY_FEE_CURVE row. ~12 calls per hour.

### Tier 4 - Daily snapshot (00:00 UTC)

The big rollup. Calls `list_open_vaults` paginated, aggregates into vault distribution / histogram / leaderboard rows, writes DAILY_TVL, DAILY_VAULTS, DAILY_LIQUIDATIONS, DAILY_SWAPS, DAILY_FEES, DAILY_STABILITY, DAILY_HOLDERS_ICUSD, DAILY_HOLDERS_3USD. The holder snapshots come from BalanceTracker which is already up-to-date from the 60s pull cycle, so they are free.

### Daily housekeeping (folded into the Tier 4 callback, runs after rollups)

Recompute Gini, top-10/100 share, churn from BalanceTracker. All in-memory work over one BTreeMap, no external calls. This is part of the same daily timer callback, not a separate timer.

### Wall-clock alignment

Tiers 3 and 4 use a fixed-interval timer but the callback first checks `if (now % bucket) > tolerance: skip`. Simpler than juggling `set_timer` reschedules.

### Cycle cost estimate

- Pull cycle: 7 × 1440 ≈ 10k query calls/day. Negligible.
- Fast: 5 × 288 ≈ 1.4k calls/day. Negligible.
- Hourly: 12 × 24 = 288 calls/day. Negligible.
- Daily: ~30 calls plus paginated vault list. Negligible.
- Stable memory growth: ~5.5MB/year for all dailies combined; per-event logs scale with protocol activity but well under 100MB/year at current volumes.
- **Total estimated annual cycle burn: ~5-10B cycles ≈ a few dollars/year.**

The dominant cost scales with vault count (daily vault-list pagination). That stays bounded at O(open_vaults) once a day and is the only thing worth re-checking when the protocol scales.

### Failure handling philosophy

Per the project's "no silent failures" rule: each tier wraps its work in a top-level result. Errors bump per-source counters in `SlimState`, get logged, and are exposed via `get_collector_health()` so the explorer can show "icUSD ledger tail: last success 14min ago, 3 errors today." Cursors never advance on error. No try/catch that swallows the error and writes a partial row.

## HTTP endpoints

Implemented via `ic-canisters-http-types`.

### CoinGecko-compatible supply endpoints

| Path | Response | Notes |
|---|---|---|
| `GET /api/supply` | `text/plain` decimal `f64` icUSD circulating | CoinGecko's strict format. **Served from a cached value in `SlimState`** because `http_request` runs in a query context and cannot make inter-canister calls. The pull cycle (60s) refreshes the cache by calling `icusd_ledger.icrc1_total_supply()` and subtracting any addresses on the `excluded_from_circulating` list (default: protocol treasury, stability pool, backend canister). The exclusion list is admin-configurable. |
| `GET /api/supply/raw` | `text/plain` u128 e8s integer | For consumers that want exact precision. |
| `GET /api/supply/3usd` | same `f64` format | 3USD circulating. |
| `GET /api/supply/3usd/raw` | u128 e8s | |
| `GET /api/supply/icp-collateral` | `f64` total ICP collateral | Bonus. |
| `GET /metrics` | Prometheus text format | Headline numbers (TVL, supply, vault count, last-snapshot-age per collector). |
| `GET /api/health` | JSON: per-collector `last_success_ts`, `error_count`, `cursor` | Public health surface. |

### Bulk historical export

All paginated, all share the `RangeQuery` shape. CSV by default; `Accept: application/json` for JSON. Pagination uses HTTP `Link: <...>; rel="next"` headers in addition to `next_cursor` in JSON responses. The `next` URL is the same path with the query string `?from_ts=<last_ts+1>&to_ts=<original_to>&limit=<original_limit>` so consumers can follow the chain without parsing the body.

| Path | Returns |
|---|---|
| `GET /api/series/tvl` | DAILY_TVL rows |
| `GET /api/series/holders/icusd` | DAILY_HOLDERS_ICUSD rows |
| `GET /api/series/holders/3usd` | DAILY_HOLDERS_3USD rows |
| `GET /api/series/liquidations` | DAILY_LIQUIDATIONS rows |
| `GET /api/series/swaps` | DAILY_SWAPS rows |
| `GET /api/series/prices` | FAST_PRICES rows |
| `GET /api/series/3pool` | FAST_3POOL rows |
| `GET /api/series/cycles` | HOURLY_CYCLES rows |
| `GET /api/series/fees` | DAILY_FEES rows |
| `GET /api/series/stability` | DAILY_STABILITY rows |
| `GET /api/series/vaults` | DAILY_VAULTS rows |
| `GET /api/events/since?cursor=&limit=` | JSON event stream for external pollers. **Explicitly documented as "not for production trading."** |

## Frontend integration (vault_frontend explorer)

New explorer routes, each a Svelte page calling one or two analytics queries via `@dfinity/agent`:

```
/explorer/holders/icusd      // top-100 table + Gini/concentration card + 24h churn
/explorer/holders/3usd       // same shape
/explorer/tvl                // DAILY_TVL line chart + system CR + vault count
/explorer/liquidations       // event feed + leaderboard + daily volume chart
/explorer/swaps              // 3pool & AMM volume + top traders + largest swaps
/explorer/vaults             // CR distribution histogram + leaderboards + lifetime
/explorer/3pool              // imbalance + virtual price chart + LP APY + fee curve
/explorer/quant              // TWAPs, peg deviation, queue depth, EV calc widgets
/explorer/health             // collector health (cycles, error counts, last-success)
```

The in-progress holders work (`src/vault_frontend/src/routes/explorer/holders/`) gets pointed at the new analytics canister. A new `analyticsService.ts` sits next to the existing `explorerService.ts` and owns all analytics calls so the existing service stays focused on backend/3pool reads.

Charts reuse whatever library `vault_frontend` already uses (verified in implementation phase). All charts read paginated data via the same `RangeQuery` shape so the data layer is uniform.

## Security considerations

Analytics is read-only-from-source, public-write-nothing, and never on a money path. Explicit invariants:

1. **Zero update calls to source canisters.** Every `sources/*.rs` function uses `ic_cdk::call::call` against `query` methods only. Enforced by code review. This is the backstop: even a fully compromised analytics canister cannot move funds, change config, or pause anything.
2. **Admin gating on mutating analytics endpoints.** `set_admin`, `trigger_backfill`, `set_collector_enabled`, `set_top_n_holders`, `set_source_canister_id` all require `caller() == admin`. Public queries are open.
3. **No new PII surface.** ICP principals are already public on-chain via existing `icrc3_get_blocks` on the ledgers and `get_all_lp_holders` on 3pool. Analytics doesn't add a new disclosure beyond existing public data.
4. **DoS resistance.** Every paginated query is hard-capped at `limit=2000`. HTTP endpoints answer from the same pagination layer. No unbounded reads. The 60s pull cycle has bounded work per tick.
5. **Upgrade safety.** `pre_upgrade` is `set_slim` only (small, fast, can't OOM). `post_upgrade` re-arms timers; cursor consistency is checked lazily on the next pull cycle by comparing the current cursor against the next batch returned from the source (if the source returns 0 new events for a cursor that should have data, the per-source error counter increments and admin can investigate). No `log_length` query is required on source canisters.
6. **No sensitive admin secrets stored.** The admin principal is the only privileged value, stored in `SlimState`. No API keys, no signing material.
7. **Public API documented as best-effort.** `/api/events/since` and live-query endpoints carry an explicit warning that they are observability tooling, not a trading oracle. This is the social contract that lets us upgrade analytics without coordinating with downstream consumers.

## Test strategy

### Unit tests (`cargo test`)

- Pure functions in `queries/live.rs` (TWAP, VWAP, Gini, OHLC, EV calc, cascade sim) - property-tested with synthetic logs.
- Pagination correctness in `queries/historical.rs`.
- Cursor advancement and idempotency in `sources/*.rs`.
- BalanceTracker state machine: applying ICRC-3 blocks in order produces correct balances.
- HTTP path routing.

### Integration tests (PocketIC, `cargo test --test pocket_ic_analytics`)

New test suite at `src/rumi_analytics/tests/pocket_ic_analytics.rs`, registered in the existing pocket-ic test infrastructure.

Fixtures: mock-or-real `rumi_protocol_backend`, `rumi_3pool`, `icusd_ledger`. The existing test suites already wire these up; we extend rather than duplicate.

Scenarios:
- Deploy analytics, advance time 1 day, verify daily row written.
- Generate swap events, verify event tail.
- Generate transfers, verify BalanceTracker.
- Trigger backfill, verify idempotency.
- **Upgrade canister mid-run, verify cursors and balance maps survive** (the critical Phase A-style upgrade test).
- Hit `/api/supply` and `/api/series/tvl` over HTTP, verify CoinGecko format and pagination.

### Pre-deploy hook

The existing `.claude/hooks/pre-deploy-test.sh` runs both unit and integration tests before any mainnet deploy. The new `pocket_ic_analytics` suite gets registered there.

### Mainnet smoke checks

A `scripts/check-analytics.sh` (or documented `dfx canister call` invocations) hits the freshly-deployed canister and verifies headline endpoints respond. Run after each phase deploy.

## Phased implementation plan

### Phase 0 - Source canister audit (no analytics code yet)

Read every `.did` file for source canisters. Produce a precise diff: which queries already exist vs which we need to add. Output is a checklist appended to this doc. Reading task, gates Phase 2.

### Phase 1 - Skeleton + storage layer + one metric end-to-end

Smallest thing that proves the architecture works.

- Create `src/rumi_analytics/` crate, `Cargo.toml`, `rumi_analytics.did`, register in `dfx.json`.
- Implement `storage.rs` with the full MemoryId map. Each MemoryId is documented with its structure type (StableLog / StableBTreeMap / StableCell).
- Implement `state.rs` (SlimState including the `circulating_supply_*` cache fields, snapshot/hydrate, pre/post_upgrade).
- Implement `timers.rs` with all four timers (60s, 5min, hourly, daily) wired but most callbacks empty. The 60s pull cycle ships in Phase 1 with **only** the supply-cache refresh logic populated; the event-tail logic comes in Phase 4.
- Implement **one collector**: `collectors/tvl.rs` daily snapshot only, sourced from `backend.get_protocol_status()`, written to DAILY_TVL.
- Implement **one query**: `get_tvl_series(RangeQuery)` reading DAILY_TVL.
- Implement **one HTTP endpoint**: `/api/supply`, served from the cached `circulating_supply_icusd` value in `SlimState` which the 60s pull cycle refreshes by calling `icusd_ledger.icrc1_total_supply()`. This validates the cache-then-serve pattern that all CoinGecko endpoints use.
- Generate declarations into `src/declarations/rumi_analytics/`.
- PocketIC integration test: deploy analytics + mock backend + mock icusd_ledger, advance time past one pull cycle, confirm supply cache populated. Advance time past one daily tick, confirm daily row written, query series, confirm pagination.
- Deploy to mainnet. Verify `/api/supply` responds (after the first 60s pull) and the daily timer fires.

**Exit criteria**: `curl https://<analytics-id>.icp0.io/api/supply` returns a number, and `dfx canister call rumi_analytics get_tvl_series '(record {})'` returns rows.

### Phase 2 - Source-canister query additions

Land the small additive PRs identified in Phase 0. Each is its own PR against the relevant canister, gated by the normal pre-deploy hook. Likely list:
- `rumi_protocol_backend`: `list_open_vaults`, `get_events_since`, optional `get_xrc_stats`.
- `rumi_3pool`: `get_swap_events_since`, `get_liquidity_events_since`.
- `rumi_amm`: `get_swap_events_since`.
- `rumi_stability_pool`: `get_events_since`.

Each PR is purely additive (new query method, no changes to existing logic) so risk is low.

### Phase 3 - Current-state daily collectors (no event dependency)

These collectors derive entirely from current-state queries against source canisters and do **not** depend on the event tail. Safe to ship before Phase 4.

- `collectors/vaults.rs` - daily snapshot from `list_open_vaults` (CR distribution, leaderboards, avg/median).
- `collectors/tvl.rs` - extend Phase 1 to also write the stability-pool TVL fields and 3pool reserves.
- A `collectors/stability.rs` daily snapshot from `stability_pool.get_pool_stats`.

Each collector is one file, one timer hookup, one stable log already reserved. PocketIC tests for each.

**Daily rollups for liquidations and swaps are deferred to Phase 5** because they derive from EVT_LIQUIDATIONS / EVT_SWAPS, which only exist after Phase 4 lands the event tail.

### Phase 4 - Event tailing & BalanceTracker

The pull cycle goes live.
- `sources/*.rs` cursor functions for all source streams.
- 60s pull tick draining every cursor into EVT_* logs.
- `collectors/holders.rs` BalanceTracker walking icusd_ledger and 3pool ICRC-3 blocks, populating BAL_* and FIRSTSEEN_* maps.
- One-shot historical backfill in `backfill.rs`, admin-gated, idempotent via the cursor StableCells (not flags). Run once per ledger, then leave the steady-state cursor running.
- Daily holder snapshots writing into DAILY_HOLDERS_*.
- `get_collector_health()` query.

PocketIC test: simulate transfers, advance time, confirm BalanceTracker map matches expected balances and Gini computes correctly.

### Phase 5 - Event-derived rollups + Fast & hourly tiers

Now that EVT_LIQUIDATIONS / EVT_SWAPS / EVT_LIQUIDITY exist (Phase 4), implement the rollups and fast/hourly cadences:

- `collectors/liquidations.rs` daily rollup over EVT_LIQUIDATIONS.
- `collectors/swaps.rs` daily rollup over EVT_SWAPS.
- `collectors/swaps.rs` Fast snapshot for 3pool imbalance & virtual price.
- `collectors/prices.rs` Fast snapshot for icUSD/3USD/ICP.
- `collectors/cycles.rs` Hourly snapshot (after the per-canister `get_cycle_balance` queries land in Phase 2).
- Borrowing fee curve hourly snapshot.

### Phase 6 - Live query layer

`queries/live.rs`: TWAPs, VWAPs, realized vol, OHLC, peg deviation, queue depth, trade size distribution, EV calc, vault risk score, cascade simulator, LP/SP APYs, OI, effective CDP cost. Each function is independent and reads from existing StableLogs. Heavy unit-test target - pure functions, easy to property-test.

### Phase 7 - HTTP layer & frontend

- All `/api/series/*` CSV endpoints + `/metrics` Prometheus + `/api/events/since` + `/api/health`.
- Submit CoinGecko listing application using `/api/supply`.
- New explorer routes in `vault_frontend`, sourced from `analyticsService.ts`. Re-target the in-progress holders page at the new analytics canister.

### Phase 8 - Polish

- Hardening based on whatever Phases 1-7 surface.
- `src/rumi_analytics/README.md` covering storage map, cadences, how to add a new metric.
- `docs/analytics-runbook.md` covering operational concerns: how to inspect collector health, how to trigger a backfill, what to do if a cursor gets stuck.

## Backfill plan

The greenfield canister has no historical data on day one. Backfill is selective and admin-triggered, not automatic.

- **BalanceTracker (icUSD, 3USD)**: walk ICRC-3 blocks from index 0 once per ledger.
- **Liquidation events**: backfill from `backend.get_events_since(0)` after Phase 2 lands the cursor query.
- **Swap events**: same pattern from `3pool.get_swap_events_since(0)` and `amm.get_swap_events_since(0)`.
- **Daily snapshots**: forward-only. Historical daily TVL/CR/etc. is reconstructed lazily-or-not-at-all from event logs as needed.

Backfill is admin-gated via `trigger_backfill(source)`. Each source has its own **high-water-mark cursor** stored in a dedicated `StableCell<u64>` (the same cursor cells used in steady state - the backfill simply walks the cursor forward from 0 until it catches up to the source, then steady-state polling takes over). Resume on partial runs is automatic because the cursor is persisted after every batch. Idempotency comes from the cursor itself, not a separate boolean flag.

## Open questions deferred to implementation

- Exact charting library used by `vault_frontend` (verify in Phase 7).
- Whether `rumi_protocol_backend` already has an internal event log we can expose via `get_events_since`, or whether that needs to be built. Determined in Phase 0.
- Whether `rumi_amm` swap events live in any form today, or whether the cursor query has to ship alongside an internal event log. Determined in Phase 0.

## Conventions

- No em dashes anywhere in code or docs.
- Rust workspace, ic-cdk 0.12.0, ic-stable-structures 0.6.5 (matching the workspace pin), candid 0.10.6.
- Forked DFINITY IC repo at `github.com/Rumi-Protocol/ic` rev `fc278709`.
- DFX-managed (not icp-cli yet).
- Declarations tracked in git.
- Local replica is system subnet, port 4943.
- PocketIC binary at project root.
