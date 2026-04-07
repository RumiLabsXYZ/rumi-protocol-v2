# 3pool Dynamic Fees, Bot Endpoints, and Event Log Migration

**Date:** 2026-04-06
**Canister:** `rumi_3pool` (fohh4-yyaaa-aaaap-qtkpa-cai)
**Status:** Design (approved, ready for implementation plan)

## Problem

The 3pool currently uses a static 20 bps swap fee with amplification A=500. The dominant user flow (mint icUSD, swap to ckUSDT/ckUSDC, exit) creates a persistent imbalance: icUSD accumulates, the two external stablecoins drain. At A=500 the curve heavily suppresses the price signal from imbalance, so there is no organic arbitrage incentive to bring ckUSDT/ckUSDC back into the pool. The protocol needs a long-term, market-driven mechanism (no token emissions) that prices the externality of imbalancing trades and rewards rebalancing trades.

(Note: a separate concern about heap-Vec event storage and the canister's `pre_upgrade` brick risk has been deferred to a dedicated follow-up PR that migrates the entire canister to `MemoryManager` + stable structures. That work is too large and too risky to bundle with dynamic fees.)

## Goals

1. Replace the static swap fee with a directional dynamic fee that taxes imbalancing trades and minimally charges rebalancing trades.
2. Apply the same dynamic fee model to single-sided liquidity add/remove operations.
3. Extend event schemas to capture the data needed by explorers and the rebalancing bot.
4. Add bot-facing query endpoints so the rebalancing bot can compute profitability locally without polling.
5. Add explorer/admin endpoints for pool analytics, time-series, and health.

## Non-Goals

- Token emissions, LP incentive programs, or any non-organic incentive (out of scope on purpose).
- Changing the amplification coefficient A (stays at 500).
- **Migrating canister state to `MemoryManager` / stable structures.** Deferred to a dedicated follow-up PR that touches every growable collection (lp_balances, ICRC-3 blocks, vp_snapshots, events, etc.) in one careful refactor. Events stay in heap `Vec` for this PR.
- ICRC-3 wrapping of swap/liquidity events.
- Frontend redesign (the dynamic fee UI affordances are noted but not specified here).
- Changes to the rumi_amm icUSD/ICP pool.

## Design

### 1. Dynamic Fee Mechanism

#### Imbalance Metric: Sum of Squared Deviations (SSD)

For pool balances normalized to weights `w_i = b_i * precision_mul_i / total`:

```
SSD = sum((w_i - 1/N)^2 for i in 0..N)
imb = SSD / MAX_SSD     // normalized to [0, 1] in fixed-point
```

For N=3, `MAX_SSD = 2/3`. We represent `imb` as a `u64` in 1e9 fixed-point (so 1.0 = 1_000_000_000).

Implementation: a new function `compute_imbalance(balances, precision_muls) -> u64` in `math.rs`. Pure function, no state, fully testable.

#### Fee Curve

```
fn compute_fee_bps(imb_before, imb_after, params) -> u16:
    if imb_after <= imb_before:
        return params.min_fee_bps           // strict binary: rebalancing always = MIN
    severity = min(imb_after / params.imb_saturation, 1.0)
    return params.min_fee_bps + (params.max_fee_bps - params.min_fee_bps) * severity
```

Linear interpolation. Strict binary on rebalancing: any swap that strictly reduces imbalance pays exactly `MIN_FEE`, regardless of the absolute imbalance level. This maximizes the arb incentive on the rebalancing leg, which is the whole point of the mechanism.

#### Initial Parameters

| Param | Value | Notes |
|---|---|---|
| `min_fee_bps` | 1 | Floor for any swap and for all rebalancing trades |
| `max_fee_bps` | 99 | Cap for severely imbalancing trades |
| `imb_saturation` | 0.25 (250_000_000 in 1e9 fp) | Imbalance level at which fee saturates to MAX |

Reference points at `imb_saturation = 0.25`:

| Pool state | Normalized imb | Imbalancing fee |
|---|---|---|
| 33/33/33 (perfect) | 0.000 | 1 bps |
| 50/25/25 (current) | 0.060 | ~25 bps |
| 60/20/20 | 0.160 | ~64 bps |
| 75/12/12 | 0.380 | 99 bps (saturated) |
| 90/5/5 | 0.685 | 99 bps |

All three params are admin-tunable post-deployment via `set_dynamic_fee_params`.

#### Application to Swaps

In `swap.rs`:

1. Compute `imb_before` from current balances.
2. Simulate the swap (existing curve math), get `dy_normalized` and the post-swap balances.
3. Compute `imb_after` from post-swap balances.
4. Compute `fee_bps = compute_fee_bps(imb_before, imb_after, params)`.
5. Apply fee: `dy_after_fee = dy_normalized * (10000 - fee_bps) / 10000`.
6. Record `fee_bps`, `imb_before`, `imb_after`, `is_rebalancing` in the swap event.

#### Application to Liquidity Operations

The current code in `liquidity.rs` uses a `3/8 * swap_fee` imbalance fee on single-sided deposits. **Replace this** with the dynamic model:

- `add_liquidity` (any token mix): compute imbalance before/after, apply `fee_bps` on the imbalanced portion.
- `remove_liquidity_one_coin`: compute imbalance before/after, apply `fee_bps`.
- `remove_liquidity` (proportional): no fee (does not change pool weights).

This means an LP who deposits the underrepresented token effectively gets near-zero fee entry, providing an additional rebalancing pathway for participants who prefer LP yield over arb.

### 2. Event Schema v2

Migrate `SwapEvent` and `LiquidityEvent` to v2 structs with additional fields. Use CBOR encoding via `ciborium` with `Bound::Unbounded` so future field additions don't break deserialization.

```rust
pub struct SwapEventV2 {
    // existing
    pub id: u64,
    pub timestamp: u64,
    pub caller: Principal,
    pub token_in: u8,
    pub token_out: u8,
    pub amount_in: u128,
    pub amount_out: u128,
    pub fee: u128,                    // fee in output token native units
    // new
    pub fee_bps: u16,                 // actual rate charged
    pub imbalance_before: u64,        // 1e9 fixed-point
    pub imbalance_after: u64,
    pub is_rebalancing: bool,
    pub pool_balances_after: [u128; 3],
}

pub struct LiquidityEventV2 {
    // existing
    pub id: u64,
    pub timestamp: u64,
    pub caller: Principal,
    pub action: LiquidityAction,
    pub amounts: [u128; 3],
    pub lp_amount: u128,
    pub coin_index: Option<u8>,
    pub fee: Option<u128>,
    // new
    pub fee_bps: Option<u16>,
    pub imbalance_before: u64,
    pub imbalance_after: u64,
    pub is_rebalancing: bool,
    pub pool_balances_after: [u128; 3],
    pub virtual_price_after: u128,
}
```

### 3. In-Place Event Schema Migration

Events remain in heap `Vec` (no `StableLog` in this PR — see Non-Goals). The existing `swap_events: Option<Vec<SwapEvent>>` and `liquidity_events: Option<Vec<LiquidityEvent>>` fields are **renamed and re-typed** to hold v2 events:

- Rename current `SwapEvent` -> `SwapEventV1`, `LiquidityEvent` -> `LiquidityEventV1`. Keep them defined for deserialization of legacy state only.
- New `SwapEvent` and `LiquidityEvent` types are the v2 schemas.
- Add new fields `swap_events_v2: Option<Vec<SwapEvent>>` and `liquidity_events_v2: Option<Vec<LiquidityEvent>>` to `ThreePoolState`.
- In `post_upgrade`, after `load_from_stable_memory()`, run a one-shot migration: drain `swap_events` (v1) into `swap_events_v2` (v2) using a `From<SwapEventV1> for SwapEvent` conversion that fills new fields with sentinel values. Same for liquidity. Then `swap_events = None`.
- All read/write paths use the v2 fields. The v1 fields stay in the struct definition until a follow-up upgrade removes them.

Sentinel values for migrated v1 events: `fee_bps = 0`, `imbalance_before = 0`, `imbalance_after = 0`, `is_rebalancing = false`, `pool_balances_after = [0; 3]`, `virtual_price_after = 0`. Explorer/bot consumers must treat these zero values as "unknown — pre-migration event."

Migration is idempotent: it checks `swap_events.is_some()` before running, takes the vec, and leaves `swap_events = None`. Re-running `post_upgrade` is a no-op.

### 4. Bot Endpoints

All query methods. Read-only.

| # | Method | Purpose |
|---|---|---|
| B1 | `quote_swap(token_in: u8, token_out: u8, amount_in: u128) -> SwapQuote` | Returns `amount_out`, `fee_bps`, `imbalance_before`, `imbalance_after`, `price_impact_bps`, `is_rebalancing`. The bot's primary endpoint. |
| B2 | `get_pool_state() -> PoolState` | Single call: balances, normalized weights, current imbalance, current A, fee params, virtual price, total LP supply, last swap timestamp. Bot polls this each loop iteration. |
| B3 | `quote_optimal_rebalance(token_to_add: u8) -> RebalanceQuote` | "How much of this token can I add at MIN_FEE before the pool starts over-rebalancing?" Returns max profitable size. |
| B4 | `get_imbalance_history(window_seconds: u64) -> Vec<ImbalanceSnapshot>` | Time series of `(timestamp, imbalance)` derived from event log. |
| B5 | `simulate_swap_path(legs: Vec<SwapLeg>) -> SimulatedPath` | Multi-hop simulation; returns final amounts and total fee. |
| B6 | `get_fee_curve_params() -> FeeCurveParams` | `(min_fee_bps, max_fee_bps, imb_saturation, metric_type)`. Lets the bot replicate the math locally. |
| B7 | `get_swap_events(start, length) -> Vec<SwapEventV2>` | Already exists; updated to return v2 schema. |

`SwapQuote`, `PoolState`, `RebalanceQuote`, `ImbalanceSnapshot`, `SimulatedPath`, `FeeCurveParams` are new candid types defined in `types.rs`.

### 5. Explorer / Admin Endpoints

#### Raw data (paginated, query)

| # | Method |
|---|---|
| E1 | `get_swap_events_by_principal(principal, start, length) -> Vec<SwapEventV2>` |
| E2 | `get_swap_events_by_time_range(from_ts, to_ts, limit) -> Vec<SwapEventV2>` |
| E3 | `get_liquidity_events_by_principal(principal, start, length) -> Vec<LiquidityEventV2>` |
| E4 | `get_admin_events(start, length) -> Vec<AdminEvent>` |

#### Aggregated stats (query)

| # | Method | Notes |
|---|---|---|
| E5 | `get_pool_stats(window: StatsWindow) -> PoolStats` | swap_count, volume, fees collected, unique swappers, liquidity flow, avg_fee_bps (volume-weighted), arb_volume |
| E6 | `get_imbalance_stats(window) -> ImbalanceStats` | current/min/max/avg + bucketed time series |
| E7 | `get_fee_stats(window) -> FeeStats` | fee bucket distribution, rebalancing %, protocol revenue |
| E8 | `get_top_swappers(window, limit) -> Vec<(Principal, count, volume)>` | |
| E9 | `get_top_lps(limit) -> Vec<(Principal, lp_balance, pct_of_pool)>` | |

`StatsWindow` enum: `Last24h | Last7d | Last30d | AllTime`.

#### Time series (query)

| # | Method |
|---|---|
| E10 | `get_volume_series(window, bucket_seconds) -> Vec<VolumePoint>` |
| E11 | `get_balance_series(window, bucket_seconds) -> Vec<BalancePoint>` |
| E12 | `get_virtual_price_series(window, bucket_seconds) -> Vec<VirtualPricePoint>` |
| E13 | `get_fee_series(window, bucket_seconds) -> Vec<FeePoint>` |

#### Health summary (query)

| # | Method |
|---|---|
| E14 | `get_pool_health() -> PoolHealth` — `current_imbalance`, `imbalance_trend_1h`, `last_swap_age_seconds`, `fee_at_perfect_rebalance`, `fee_at_max_imbalance_swap`, `arb_opportunity_score (0-100)` |

The aggregations (E5-E14) iterate the StableLog for the requested window. For typical windows this is fine. If we ever see hot-path performance issues we can add a cached summary updated on each event append, but no premature optimization.

### 6. Admin Endpoint

```rust
#[update]
pub fn set_dynamic_fee_params(params: FeeCurveParams) -> Result<(), ThreePoolError>
```

Admin-only. Validations:
- `min_fee_bps >= 1`
- `max_fee_bps <= 200` (hard ceiling, prevent admin error)
- `min_fee_bps < max_fee_bps`
- `imb_saturation > 0` and `<= 1e9` (the 1.0 fixed-point)

Records a `ThreePoolAdminAction::SetFeeCurveParams { ... }` audit log entry.

### 7. Migration / Deploy Sequencing

This is a single canister upgrade. Order of operations on deploy:

1. Code carries `SwapEventV2` and `LiquidityEventV2`, new `StableLog` declarations, new fee params with defaults baked into config.
2. `pre_upgrade` (if any) is a no-op for the legacy event vecs — they'll be migrated post-upgrade.
3. `post_upgrade` runs `migrate_events_to_stable_log()` once.
4. New config field `dynamic_fee_params: FeeCurveParams` initialized to `{ min: 1, max: 99, sat: 0.25 }` if not present in restored state.
5. The legacy `swap_fee_bps: u16` field stays in `PoolConfig` for one upgrade cycle (deprecated, unread) and is removed in a follow-up. This avoids candid type-shift mid-upgrade.

The upgrade `description` arg should read something like: `"Dynamic fees + StableLog event migration + bot/explorer endpoints"`.

## Tests

### Unit Tests (math.rs, swap.rs, liquidity.rs)

- `compute_imbalance` returns 0 for perfectly balanced pool, ~0.06 for 50/25/25, monotonic increase as one weight grows
- `compute_fee_bps` returns MIN_FEE for rebalancing, scales linearly for imbalancing, saturates at MAX_FEE
- Symmetry: same trade in opposite direction (swapping the imbalance state) gives consistent fees
- Round-trip: imbalance + reverse trade leaves pool nearly balanced; total fees = (imbalancing fee) + MIN_FEE
- Bounds: fee_bps always in `[min_fee_bps, max_fee_bps]`
- Virtual price never decreases across any swap or LP op (existing invariant, must hold under dynamic fees)
- Liquidity ops: rebalancing single-sided deposit pays MIN_FEE, imbalancing pays scaled

### Integration Tests (pocket_ic_3usd)

- Existing test suite must pass with v2 events and dynamic fees enabled
- New test: simulate the dominant flow (mint icUSD -> swap to ckUSDT) repeatedly, verify fee_bps grows as imbalance grows
- New test: arb counter-flow (swap ckUSDT -> icUSD when imbalanced) pays MIN_FEE
- Migration test: pre-populate heap Vec with legacy events, upgrade, verify StableLog contains migrated entries with sentinel values
- Migration idempotency: run post_upgrade twice, verify no duplicates
- Admin test: `set_dynamic_fee_params` validation rejects invalid configs, audit log records changes

## Risks & Mitigations

| Risk | Mitigation |
|---|---|
| Sentinel zero values in migrated v1 events confuse explorer | Explorer code treats `imbalance_after == 0 && pool_balances_after == [0; 3]` as "pre-migration unknown" and renders accordingly |
| Heap-Vec event growth still risks pre_upgrade brick (long-term) | Tracked in a separate follow-up PR (`MemoryManager` migration). Out of scope here. Event count is small today, runway exists. |
| Aggregation queries get slow at scale | Acceptable for now. Add cached running totals only if metrics show actual problem. |
| Admin sets `imb_saturation` too low and fees saturate constantly | Validation enforces bounds; audit log records every change; can be reverted in seconds |
| Fee param change mid-flight desyncs the bot's local fee math | Bot polls `get_fee_curve_params` each loop. Change is atomic per swap. |
| MEV / pre-imbalancing for cheap rebalance | IC has no public mempool, pool is small, non-issue at this scale. Flagged for future. |
| Removing legacy `swap_fee_bps` field | Defer to a follow-up upgrade after v2 is verified on mainnet |

## Open Questions

None blocking. Items deferred to implementation:

- Frontend changes to display dynamic fees in the swap UI (separate task)
- Bot updates to consume new endpoints (separate session, separate working folder per Rob)
- Full canister state migration to MemoryManager (separate follow-up PR)

## Files Touched

| File | Change |
|---|---|
| `src/rumi_3pool/src/math.rs` | Add `compute_imbalance`, `compute_fee_bps` |
| `src/rumi_3pool/src/swap.rs` | Replace static fee with dynamic; populate v2 event fields |
| `src/rumi_3pool/src/liquidity.rs` | Replace 3/8 imbalance fee with dynamic model; populate v2 event fields |
| `src/rumi_3pool/src/types.rs` | `SwapEventV2`, `LiquidityEventV2`, `FeeCurveParams`, `SwapQuote`, `PoolState`, `RebalanceQuote`, etc. |
| `src/rumi_3pool/src/state.rs` | v1/v2 event field rename, post_upgrade in-place migration, dynamic_fee_params in config |
| `src/rumi_3pool/src/admin.rs` | `set_dynamic_fee_params` |
| `src/rumi_3pool/src/lib.rs` | All new bot + explorer endpoints |
| `src/rumi_3pool/rumi_3pool.did` | Candid updates for new types and methods |
| `src/rumi_3pool/tests/integration_test.rs` | New tests per above |
| `src/rumi_3pool/tests/pocket_ic_3usd.rs` | Migration test, dynamic fee scenario tests |
