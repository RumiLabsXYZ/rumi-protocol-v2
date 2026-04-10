# rumi_analytics

On-chain analytics canister for the Rumi Protocol. Collects time-series data from
five source canisters (backend, icUSD ledger, 3pool, stability pool, AMM), computes
aggregate metrics (TWAP, OHLC, volatility, APY, peg status), and serves them via
Candid query endpoints and an HTTP API.

**Mainnet canister ID:** `tfesu-vyaaa-aaaap-qrd7a-cai` (placeholder, update after first deploy)

## Architecture

Four timer-driven collection cycles run concurrently:

| Cycle   | Interval | What it does                                                |
|---------|----------|-------------------------------------------------------------|
| Pull    | 60s      | Tails events from source canisters, caches supply           |
| Fast    | 300s     | Snapshots collateral prices and 3pool state                 |
| Hourly  | 3600s    | Snapshots cycle balance and fee curve                       |
| Daily   | 86400s   | Rolls up TVL, vault stats, stability, holders, swaps, fees  |

State is split between heap-side `SlimState` (small, hot-path values) and
stable-memory `StableLog`/`StableBTreeMap` collections managed by a `MemoryManager`
with 64 virtual slots.

## Build

```bash
cargo build --target wasm32-unknown-unknown --release -p rumi_analytics
```

## Test

```bash
# Unit tests (40 tests, ~0s)
cargo test -p rumi_analytics --lib

# Integration tests (25 tests, ~2.5 min, requires pocket-ic binary)
POCKET_IC_BIN=./pocket-ic cargo test --test pocket_ic_analytics
```

## Deploy

```bash
# First install (requires init args)
dfx deploy rumi_analytics --network ic --argument '(record {
  admin = principal "YOUR_ADMIN";
  backend = principal "tfesu-vyaaa-aaaap-qrd7a-cai";
  icusd_ledger = principal "t6bor-paaaa-aaaap-qrd5q-cai";
  three_pool = principal "fohh4-yyaaa-aaaap-qtkpa-cai";
  stability_pool = principal "tmhzi-dqaaa-aaaap-qrd6q-cai";
  amm = principal "ijlzs-2yaaa-aaaap-quaaq-cai";
})'

# Upgrade (no args needed, state persists)
dfx deploy rumi_analytics --network ic
```

## Init Args

| Field            | Type        | Description                        |
|------------------|-------------|------------------------------------|
| `admin`          | `Principal` | Admin principal (for start_backfill) |
| `backend`        | `Principal` | rumi_protocol_backend canister     |
| `icusd_ledger`   | `Principal` | icUSD ICRC-1 ledger                |
| `three_pool`     | `Principal` | rumi_3pool AMM canister            |
| `stability_pool` | `Principal` | rumi_stability_pool canister       |
| `amm`            | `Principal` | rumi_amm canister                  |

## HTTP Endpoints

| Path                     | Response       | Description                      |
|--------------------------|----------------|----------------------------------|
| `/api/supply`            | `text/plain`   | icUSD circulating supply (float) |
| `/api/supply/raw`        | `text/plain`   | icUSD supply in e8s (integer)    |
| `/api/health`            | `application/json` | Operational health, error counters, row counts |
| `/api/series/tvl`        | `text/csv`     | Daily TVL time series            |
| `/api/series/vaults`     | `text/csv`     | Daily vault snapshot series      |
| `/api/series/stability`  | `text/csv`     | Daily stability pool series      |
| `/api/series/swaps`      | `text/csv`     | Daily swap rollup series         |
| `/api/series/liquidations` | `text/csv`   | Daily liquidation rollup series  |
| `/api/series/fees`       | `text/csv`     | Daily fee rollup series          |
| `/api/series/prices`     | `text/csv`     | Fast (5-min) price snapshot series |
| `/metrics`               | `text/plain`   | Prometheus-format metrics        |

## Candid Query Endpoints

| Method                 | Input              | Output                      |
|------------------------|--------------------|-----------------------------|
| `ping`                 | ()                 | text                        |
| `get_admin`            | ()                 | Principal                   |
| `get_tvl_series`       | RangeQuery         | TvlSeriesResponse           |
| `get_vault_series`     | RangeQuery         | VaultSeriesResponse         |
| `get_stability_series` | RangeQuery         | StabilitySeriesResponse     |
| `get_holder_series`    | RangeQuery, Principal | HolderSeriesResponse     |
| `get_liquidation_series` | RangeQuery       | LiquidationSeriesResponse   |
| `get_swap_series`      | RangeQuery         | SwapSeriesResponse          |
| `get_fee_series`       | RangeQuery         | FeeSeriesResponse           |
| `get_price_series`     | RangeQuery         | PriceSeriesResponse         |
| `get_three_pool_series`| RangeQuery         | ThreePoolSeriesResponse     |
| `get_cycle_series`     | RangeQuery         | CycleSeriesResponse         |
| `get_fee_curve_series` | RangeQuery         | FeeCurveSeriesResponse      |
| `get_ohlc`             | OhlcQuery          | OhlcResponse                |
| `get_twap`             | TwapQuery          | TwapResponse                |
| `get_volatility`       | VolatilityQuery    | VolatilityResponse          |
| `get_peg_status`       | ()                 | vec PegStatus               |
| `get_apys`             | ApyQuery           | ApyResponse                 |
| `get_protocol_summary` | ()                 | ProtocolSummary             |
| `get_trade_activity`   | TradeActivityQuery | TradeActivityResponse       |
| `get_collector_health` | ()                 | CollectorHealth             |

## Update Endpoints

| Method           | Input      | Output | Description                        |
|------------------|------------|--------|------------------------------------|
| `start_backfill` | Principal  | text   | Admin-only: backfill token balances |

## Monitoring

- **Health check:** `GET /api/health` returns error counters per source canister and row counts.
  Alert if `last_pull_cycle_ns` is more than 5 minutes stale.
- **Prometheus:** `GET /metrics` exposes gauges for supply, TVL, vault count, collateral ratio,
  and row counts per storage tier.

## Storage Layout

| Slot(s)  | Structure       | Contents                                |
|----------|-----------------|-----------------------------------------|
| 0        | StableCell      | SlimState (admin, sources, cursors, counters) |
| 1-7      | StableCell      | Event-tailing cursors (one per source)  |
| 10-25    | StableLog (paired idx/data) | Daily snapshots (TVL, vaults, holders, liquidations, swaps, fees, stability) |
| 30-33    | StableLog       | Fast (5-min) snapshots (prices, 3pool)  |
| 38-41    | StableLog       | Hourly snapshots (cycles, fee curve)    |
| 44-51    | StableLog       | Event mirrors (liquidations, swaps, liquidity, vaults) |
| 56-59    | StableBTreeMap  | BalanceTracker + FirstSeen maps (icUSD, 3USD) |
| 26-29, 34-37, 42-43, 52-55, 60-63 | -- | Reserved |
