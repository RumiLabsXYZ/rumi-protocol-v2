# Phase 7: HTTP API Layer & Frontend Integration

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expose analytics data via HTTP API endpoints (CSV series, Prometheus metrics, health, supply) and integrate the analytics canister into the vault_frontend explorer.

**Architecture:** The Rust HTTP handler (`http.rs`) is extended with CSV time-series endpoints, a Prometheus `/metrics` scrape target, and a JSON health endpoint. On the frontend, a new `analyticsService.ts` creates an anonymous actor for the analytics canister and exposes typed fetch functions with TTL caching. The explorer overview page is enhanced with analytics-sourced charts and a stats dashboard, and the holders page gains a historical trends section.

**Tech Stack:** Rust (ic-cdk, ic_canisters_http_types), SvelteKit 5 (Svelte 5 runes), TypeScript, @dfinity/agent, inline SVG charts (no charting library).

---

## File Structure

| File | Responsibility |
|---|---|
| `src/rumi_analytics/src/http.rs` | **MODIFY** - Add CSV series endpoints, Prometheus metrics, health JSON, events endpoint |
| `src/rumi_analytics/src/http/csv.rs` | **CREATE** - CSV serialization helpers for each row type |
| `src/rumi_analytics/src/http/metrics.rs` | **CREATE** - Prometheus text format builder |
| `src/rumi_analytics/Cargo.toml` | **MODIFY** - Add `serde_json` dependency |
| `src/rumi_analytics/src/lib.rs` | No changes needed (http_request already wired) |
| `src/vault_frontend/src/lib/config.ts` | **MODIFY** - Add ANALYTICS canister ID |
| `src/vault_frontend/src/lib/services/explorer/analyticsService.ts` | **CREATE** - Analytics canister actor + typed fetch functions with TTL cache |
| `src/vault_frontend/src/routes/explorer/stats/+page.svelte` | **REWRITE** - Analytics dashboard (replace redirect stub) |
| `src/vault_frontend/src/routes/explorer/+layout.svelte` | **MODIFY** - Add Stats nav link |
| `src/vault_frontend/src/routes/explorer/+page.svelte` | **MODIFY** - Source chart data from analytics canister |
| `src/vault_frontend/src/routes/explorer/holders/+page.svelte` | **MODIFY** - Add historical holder trends section |
| `src/rumi_analytics/tests/pocket_ic_analytics.rs` | **MODIFY** - Add HTTP endpoint integration tests |

---

### Task 1: CSV Serialization Helpers

**Files:**
- Create: `src/rumi_analytics/src/http/csv.rs`
- Modify: `src/rumi_analytics/src/http.rs` (convert to `src/rumi_analytics/src/http/mod.rs`)

The existing `http.rs` is a flat file. We'll convert it to a module directory (`http/mod.rs` + `http/csv.rs` + `http/metrics.rs`) to keep things organized.

- [ ] **Step 1: Create http directory and move http.rs to mod.rs**

Rename `src/rumi_analytics/src/http.rs` to `src/rumi_analytics/src/http/mod.rs`. The content stays the same for now.

Run: `mkdir -p src/rumi_analytics/src/http && mv src/rumi_analytics/src/http.rs src/rumi_analytics/src/http/mod.rs`

- [ ] **Step 2: Build to verify the module rename didn't break anything**

Run: `cargo build --target wasm32-unknown-unknown --release -p rumi_analytics 2>&1 | tail -5`
Expected: compiles successfully

- [ ] **Step 3: Create csv.rs with row-to-CSV functions**

Create `src/rumi_analytics/src/http/csv.rs`:

```rust
//! CSV serialization for time-series row types. Each function takes a slice
//! of rows and returns a complete CSV string with header line.

use crate::storage;
use candid::Principal;

pub fn tvl_csv(rows: &[storage::DailyTvlRow]) -> String {
    let mut out = String::from("timestamp_ns,total_icp_collateral_e8s,total_icusd_supply_e8s,system_cr_bps,stability_pool_deposits_e8s,three_pool_reserve_0_e8s,three_pool_reserve_1_e8s,three_pool_reserve_2_e8s\n");
    for r in rows {
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{}\n",
            r.timestamp_ns,
            r.total_icp_collateral_e8s,
            r.total_icusd_supply_e8s,
            r.system_collateral_ratio_bps,
            r.stability_pool_deposits_e8s.map_or(String::new(), |v| v.to_string()),
            r.three_pool_reserve_0_e8s.map_or(String::new(), |v| v.to_string()),
            r.three_pool_reserve_1_e8s.map_or(String::new(), |v| v.to_string()),
            r.three_pool_reserve_2_e8s.map_or(String::new(), |v| v.to_string()),
        ));
    }
    out
}

pub fn vault_csv(rows: &[storage::DailyVaultSnapshotRow]) -> String {
    let mut out = String::from("timestamp_ns,total_vault_count,total_collateral_usd_e8s,total_debt_e8s,median_cr_bps\n");
    for r in rows {
        out.push_str(&format!(
            "{},{},{},{},{}\n",
            r.timestamp_ns, r.total_vault_count, r.total_collateral_usd_e8s, r.total_debt_e8s, r.median_cr_bps,
        ));
    }
    out
}

pub fn stability_csv(rows: &[storage::DailyStabilityRow]) -> String {
    let mut out = String::from("timestamp_ns,total_deposits_e8s,total_depositors,total_liquidations_executed,total_interest_received_e8s\n");
    for r in rows {
        out.push_str(&format!(
            "{},{},{},{},{}\n",
            r.timestamp_ns, r.total_deposits_e8s, r.total_depositors,
            r.total_liquidations_executed, r.total_interest_received_e8s,
        ));
    }
    out
}

pub fn swap_csv(rows: &[storage::rollups::DailySwapRollup]) -> String {
    let mut out = String::from("timestamp_ns,three_pool_swap_count,amm_swap_count,three_pool_volume_e8s,amm_volume_e8s,three_pool_fees_e8s,amm_fees_e8s,unique_swappers\n");
    for r in rows {
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{}\n",
            r.timestamp_ns, r.three_pool_swap_count, r.amm_swap_count,
            r.three_pool_volume_e8s, r.amm_volume_e8s,
            r.three_pool_fees_e8s, r.amm_fees_e8s, r.unique_swappers,
        ));
    }
    out
}

pub fn liquidation_csv(rows: &[storage::rollups::DailyLiquidationRollup]) -> String {
    let mut out = String::from("timestamp_ns,full_count,partial_count,redistribution_count,total_collateral_seized_e8s,total_debt_absorbed_e8s\n");
    for r in rows {
        out.push_str(&format!(
            "{},{},{},{},{},{}\n",
            r.timestamp_ns, r.full_count, r.partial_count, r.redistribution_count,
            r.total_collateral_seized_e8s, r.total_debt_absorbed_e8s,
        ));
    }
    out
}

pub fn fee_csv(rows: &[storage::rollups::DailyFeeRollup]) -> String {
    let mut out = String::from("timestamp_ns,swap_fees_e8s,borrowing_fees_e8s,redemption_fees_e8s,borrow_count,redemption_count\n");
    for r in rows {
        out.push_str(&format!(
            "{},{},{},{},{},{}\n",
            r.timestamp_ns, r.swap_fees_e8s,
            r.borrowing_fees_e8s.map_or(String::new(), |v| v.to_string()),
            r.redemption_fees_e8s.map_or(String::new(), |v| v.to_string()),
            r.borrow_count, r.redemption_count,
        ));
    }
    out
}

pub fn price_csv(rows: &[storage::fast::FastPriceSnapshot]) -> String {
    let mut out = String::from("timestamp_ns,collateral,price_usd,symbol\n");
    for r in rows {
        for (p, price, sym) in &r.prices {
            out.push_str(&format!("{},{},{},{}\n", r.timestamp_ns, p, price, sym));
        }
    }
    out
}
```

- [ ] **Step 4: Add `mod csv;` to http/mod.rs**

Add at the top of `src/rumi_analytics/src/http/mod.rs`:
```rust
mod csv;
```

- [ ] **Step 5: Build to verify**

Run: `cargo build --target wasm32-unknown-unknown --release -p rumi_analytics 2>&1 | tail -5`
Expected: compiles successfully

- [ ] **Step 6: Commit**

```bash
git add src/rumi_analytics/src/http/
git commit -m "refactor(analytics): convert http.rs to module directory, add CSV helpers"
```

---

### Task 2: Prometheus Metrics Endpoint

**Files:**
- Create: `src/rumi_analytics/src/http/metrics.rs`
- Modify: `src/rumi_analytics/src/http/mod.rs`

- [ ] **Step 1: Create metrics.rs**

Create `src/rumi_analytics/src/http/metrics.rs`:

```rust
//! Prometheus text format exposition. All gauges, no histograms.

use crate::{state, storage};

pub fn render() -> String {
    let mut out = String::with_capacity(2048);

    // Supply gauges
    let icusd_supply = state::read_state(|s| s.circulating_supply_icusd_e8s).unwrap_or(0);
    let threeusd_supply = state::read_state(|s| s.circulating_supply_3usd_e8s).unwrap_or(0);
    gauge(&mut out, "rumi_icusd_supply_e8s", "icUSD circulating supply in e8s", icusd_supply as f64);
    gauge(&mut out, "rumi_3usd_supply_e8s", "3USD circulating supply in e8s", threeusd_supply as f64);

    // Latest vault snapshot
    let vault_n = storage::daily_vaults::len();
    if vault_n > 0 {
        if let Some(v) = storage::daily_vaults::get(vault_n - 1) {
            gauge(&mut out, "rumi_total_vault_count", "Total number of vaults", v.total_vault_count as f64);
            gauge(&mut out, "rumi_total_collateral_usd_e8s", "Total collateral value in USD e8s", v.total_collateral_usd_e8s as f64);
            gauge(&mut out, "rumi_total_debt_e8s", "Total icUSD debt in e8s", v.total_debt_e8s as f64);
            gauge(&mut out, "rumi_system_cr_bps", "System collateral ratio in basis points", v.median_cr_bps as f64);
        }
    }

    // Latest TVL
    let tvl_n = storage::daily_tvl::len();
    if tvl_n > 0 {
        if let Some(t) = storage::daily_tvl::get(tvl_n - 1) {
            gauge(&mut out, "rumi_total_icp_collateral_e8s", "Total ICP collateral in e8s", t.total_icp_collateral_e8s as f64);
            gauge(&mut out, "rumi_total_icusd_supply_e8s", "Total icUSD supply from TVL snapshot", t.total_icusd_supply_e8s as f64);
        }
    }

    // Error counters
    let ec = state::read_state(|s| s.error_counters.clone());
    counter(&mut out, "rumi_collector_errors_total", "backend", ec.backend);
    counter(&mut out, "rumi_collector_errors_total", "icusd_ledger", ec.icusd_ledger);
    counter(&mut out, "rumi_collector_errors_total", "three_pool", ec.three_pool);
    counter(&mut out, "rumi_collector_errors_total", "stability_pool", ec.stability_pool);
    counter(&mut out, "rumi_collector_errors_total", "amm", ec.amm);

    // Storage sizes
    gauge(&mut out, "rumi_storage_daily_tvl_rows", "Number of daily TVL rows", storage::daily_tvl::len() as f64);
    gauge(&mut out, "rumi_storage_daily_vault_rows", "Number of daily vault rows", storage::daily_vaults::len() as f64);
    gauge(&mut out, "rumi_storage_evt_swaps_rows", "Number of swap event rows", storage::events::evt_swaps::len() as f64);
    gauge(&mut out, "rumi_storage_evt_liquidations_rows", "Number of liquidation event rows", storage::events::evt_liquidations::len() as f64);
    gauge(&mut out, "rumi_storage_fast_prices_rows", "Number of fast price snapshots", storage::fast::fast_prices::len() as f64);

    out
}

fn gauge(out: &mut String, name: &str, help: &str, value: f64) {
    out.push_str(&format!("# HELP {} {}\n# TYPE {} gauge\n{} {}\n", name, help, name, name, value));
}

fn counter(out: &mut String, name: &str, label: &str, value: u64) {
    // First call for a counter name should emit HELP/TYPE, but for simplicity
    // we emit per-label since Prometheus is tolerant of repeated HELP lines.
    out.push_str(&format!("{}{{source=\"{}\"}} {}\n", name, label, value));
}
```

- [ ] **Step 2: Add `mod metrics;` to http/mod.rs and wire the `/metrics` route**

In `src/rumi_analytics/src/http/mod.rs`, add:
```rust
mod metrics;
```

And add a match arm in `http_request`:
```rust
"/metrics" => {
    let body = metrics::render();
    HttpResponseBuilder::ok()
        .header("Content-Type", "text/plain; version=0.0.4; charset=utf-8")
        .with_body_and_content_length(body)
        .build()
}
```

- [ ] **Step 3: Build to verify**

Run: `cargo build --target wasm32-unknown-unknown --release -p rumi_analytics 2>&1 | tail -5`

- [ ] **Step 4: Commit**

```bash
git add src/rumi_analytics/src/http/metrics.rs src/rumi_analytics/src/http/mod.rs
git commit -m "feat(analytics): add Prometheus /metrics endpoint"
```

---

### Task 3: CSV Series and Health Endpoints

**Files:**
- Modify: `src/rumi_analytics/src/http/mod.rs`
- Modify: `src/rumi_analytics/Cargo.toml`

- [ ] **Step 1: Add serde_json dependency**

Add to `src/rumi_analytics/Cargo.toml` under `[dependencies]`:
```toml
serde_json = "1"
```

- [ ] **Step 2: Wire all HTTP routes in mod.rs**

Replace the `http_request` function in `src/rumi_analytics/src/http/mod.rs` with:

```rust
mod csv;
mod metrics;

use ic_canisters_http_types::{HttpRequest, HttpResponse, HttpResponseBuilder};
use crate::{state, storage, types};

pub fn http_request(req: HttpRequest) -> HttpResponse {
    let path = req.url.split('?').next().unwrap_or("");
    match path {
        // Supply endpoints (existing)
        "/api/supply" => supply_icusd_f64(),
        "/api/supply/raw" => supply_icusd_raw(),

        // Prometheus
        "/metrics" => {
            let body = metrics::render();
            HttpResponseBuilder::ok()
                .header("Content-Type", "text/plain; version=0.0.4; charset=utf-8")
                .with_body_and_content_length(body)
                .build()
        }

        // Health
        "/api/health" => health_json(),

        // CSV series
        "/api/series/tvl" => csv_response(csv::tvl_csv(&storage::daily_tvl::range(0, u64::MAX, 10_000))),
        "/api/series/vaults" => csv_response(csv::vault_csv(&storage::daily_vaults::range(0, u64::MAX, 10_000))),
        "/api/series/stability" => csv_response(csv::stability_csv(&storage::daily_stability::range(0, u64::MAX, 10_000))),
        "/api/series/swaps" => csv_response(csv::swap_csv(&storage::rollups::daily_swaps::range(0, u64::MAX, 10_000))),
        "/api/series/liquidations" => csv_response(csv::liquidation_csv(&storage::rollups::daily_liquidations::range(0, u64::MAX, 10_000))),
        "/api/series/fees" => csv_response(csv::fee_csv(&storage::rollups::daily_fees::range(0, u64::MAX, 10_000))),
        "/api/series/prices" => csv_response(csv::price_csv(&storage::fast::fast_prices::range(0, u64::MAX, 10_000))),

        _ => HttpResponseBuilder::not_found().build(),
    }
}

fn csv_response(body: String) -> HttpResponse {
    HttpResponseBuilder::ok()
        .header("Content-Type", "text/csv; charset=utf-8")
        .header("Access-Control-Allow-Origin", "*")
        .with_body_and_content_length(body)
        .build()
}

fn health_json() -> HttpResponse {
    let last_daily = state::read_state(|s| s.last_daily_snapshot_ns);
    let last_pull = state::read_state(|s| s.last_pull_cycle_ns).unwrap_or(0);
    let ec = state::read_state(|s| s.error_counters.clone());
    let now = ic_cdk::api::time();

    let body = serde_json::json!({
        "status": "ok",
        "canister_time_ns": now,
        "last_daily_snapshot_ns": last_daily,
        "last_pull_cycle_ns": last_pull,
        "error_counters": {
            "backend": ec.backend,
            "icusd_ledger": ec.icusd_ledger,
            "three_pool": ec.three_pool,
            "stability_pool": ec.stability_pool,
            "amm": ec.amm,
        },
        "storage_rows": {
            "daily_tvl": storage::daily_tvl::len(),
            "daily_vaults": storage::daily_vaults::len(),
            "evt_swaps": storage::events::evt_swaps::len(),
            "evt_liquidations": storage::events::evt_liquidations::len(),
            "fast_prices": storage::fast::fast_prices::len(),
        }
    });

    HttpResponseBuilder::ok()
        .header("Content-Type", "application/json; charset=utf-8")
        .header("Access-Control-Allow-Origin", "*")
        .with_body_and_content_length(body.to_string())
        .build()
}
```

Keep the existing `supply_icusd_f64` and `supply_icusd_raw` private functions unchanged below.

- [ ] **Step 3: Build to verify**

Run: `cargo build --target wasm32-unknown-unknown --release -p rumi_analytics 2>&1 | tail -5`

- [ ] **Step 4: Commit**

```bash
git add src/rumi_analytics/Cargo.toml src/rumi_analytics/src/http/mod.rs
git commit -m "feat(analytics): add CSV series, health JSON, and route all HTTP endpoints"
```

---

### Task 4: HTTP Endpoint Integration Tests

**Files:**
- Modify: `src/rumi_analytics/tests/pocket_ic_analytics.rs`

- [ ] **Step 1: Add HTTP endpoint tests**

Add after the existing Phase 6 tests:

```rust
// ─── Phase 7 HTTP tests ───

#[test]
fn http_health_returns_json() {
    let env = setup();
    advance_pull_cycle(&env);

    let resp = http_get(&env, "/api/health");
    assert_eq!(resp.status_code, 200);
    let body = String::from_utf8(resp.body).unwrap();
    assert!(body.contains("\"status\":\"ok\""), "health body: {}", body);
    assert!(body.contains("\"storage_rows\""), "health body: {}", body);
}

#[test]
fn http_metrics_returns_prometheus() {
    let env = setup();
    advance_pull_cycle(&env);

    let resp = http_get(&env, "/metrics");
    assert_eq!(resp.status_code, 200);
    let body = String::from_utf8(resp.body).unwrap();
    assert!(body.contains("rumi_icusd_supply_e8s"), "metrics body missing supply gauge");
}

#[test]
fn http_csv_tvl_returns_header() {
    let env = setup();
    // Trigger daily snapshot so there's at least one TVL row
    advance_pull_cycle(&env);
    env.pic.advance_time(std::time::Duration::from_secs(86_400 + 65));
    for _ in 0..10 { env.pic.tick(); }

    let resp = http_get(&env, "/api/series/tvl");
    assert_eq!(resp.status_code, 200);
    let body = String::from_utf8(resp.body).unwrap();
    assert!(body.starts_with("timestamp_ns,"), "CSV should start with header, got: {}", &body[..50.min(body.len())]);
    // Should have at least header + 1 data row
    let lines: Vec<&str> = body.lines().collect();
    assert!(lines.len() >= 2, "expected header + data row, got {} lines", lines.len());
}

#[test]
fn http_not_found() {
    let env = setup();
    let resp = http_get(&env, "/nonexistent");
    assert_eq!(resp.status_code, 404);
}
```

- [ ] **Step 2: Run integration tests**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test --test pocket_ic_analytics -- --nocapture 2>&1 | tail -10`
Expected: all tests pass (24 total: 20 existing + 4 new)

- [ ] **Step 3: Commit**

```bash
git add src/rumi_analytics/tests/pocket_ic_analytics.rs
git commit -m "test(analytics): add Phase 7 HTTP endpoint integration tests"
```

---

### Task 5: Frontend Analytics Service

**Files:**
- Modify: `src/vault_frontend/src/lib/config.ts`
- Create: `src/vault_frontend/src/lib/services/explorer/analyticsService.ts`

- [ ] **Step 1: Add analytics canister ID to config.ts**

In `src/vault_frontend/src/lib/config.ts`, add to the `CANISTER_IDS` object:
```typescript
  ANALYTICS: "dtlu2-uqaaa-aaaap-qugcq-cai",
```

And add the IDL import at the top:
```typescript
import { idlFactory as analyticsIDL } from '$declarations/rumi_analytics/rumi_analytics.did.js';
```

And add to the `CONFIG` object (around line 43+, wherever the IDL references are):
```typescript
  analyticsIDL,
```

- [ ] **Step 2: Create analyticsService.ts**

Create `src/vault_frontend/src/lib/services/explorer/analyticsService.ts`:

```typescript
/**
 * analyticsService.ts — Typed data service for the rumi_analytics canister.
 *
 * Creates an anonymous actor and exposes fetch functions with TTL caching,
 * following the same pattern as explorerService.ts.
 */

import { Actor, HttpAgent } from '@dfinity/agent';
import { CANISTER_IDS, CONFIG } from '$lib/config';
import type { _SERVICE } from '$declarations/rumi_analytics/rumi_analytics.did';

// ── TTL constants (ms) ──────────────────────────────────────────────────────

const TTL = {
  SUMMARY: 15_000,
  SERIES: 60_000,
  FAST: 30_000,
} as const;

// ── Cache infrastructure ────────────────────────────────────────────────────

interface CacheEntry<T> {
  data: T;
  ts: number;
}

const cache = new Map<string, CacheEntry<unknown>>();

function getCached<T>(key: string, ttlMs: number): T | null {
  const entry = cache.get(key);
  if (!entry) return null;
  if (Date.now() - entry.ts > ttlMs) {
    cache.delete(key);
    return null;
  }
  return entry.data as T;
}

function setCache<T>(key: string, data: T): T {
  cache.set(key, { data, ts: Date.now() });
  return data;
}

export function invalidateAnalyticsCache(prefix?: string): void {
  if (!prefix) { cache.clear(); return; }
  for (const key of cache.keys()) {
    if (key.startsWith(prefix)) cache.delete(key);
  }
}

// ── Actor ───────────────────────────────────────────────────────────────────

let _actor: _SERVICE | null = null;

function getActor(): _SERVICE {
  if (_actor) return _actor;
  const agent = HttpAgent.createSync({ host: 'https://icp-api.io' });
  _actor = Actor.createActor<_SERVICE>(CONFIG.analyticsIDL, {
    agent,
    canisterId: CANISTER_IDS.ANALYTICS,
  });
  return _actor;
}

// ── Typed query helpers ─────────────────────────────────────────────────────

function rangeQuery(from?: bigint, to?: bigint, limit?: number) {
  return {
    from_ts: from !== undefined ? [from] : [],
    to_ts: to !== undefined ? [to] : [],
    limit: limit !== undefined ? [limit] : [],
    offset: [],
  };
}

// ── Public fetch functions ──────────────────────────────────────────────────

export async function fetchProtocolSummary() {
  const key = 'analytics:summary';
  const cached = getCached<any>(key, TTL.SUMMARY);
  if (cached) return cached;

  try {
    const result = await getActor().get_protocol_summary();
    return setCache(key, result);
  } catch (err) {
    console.error('[analyticsService] fetchProtocolSummary failed:', err);
    return null;
  }
}

export async function fetchTvlSeries(limit = 365) {
  const key = `analytics:tvl:${limit}`;
  const cached = getCached<any>(key, TTL.SERIES);
  if (cached) return cached;

  try {
    const result = await getActor().get_tvl_series(rangeQuery(undefined, undefined, limit));
    return setCache(key, result.rows);
  } catch (err) {
    console.error('[analyticsService] fetchTvlSeries failed:', err);
    return [];
  }
}

export async function fetchVaultSeries(limit = 365) {
  const key = `analytics:vaults:${limit}`;
  const cached = getCached<any>(key, TTL.SERIES);
  if (cached) return cached;

  try {
    const result = await getActor().get_vault_series(rangeQuery(undefined, undefined, limit));
    return setCache(key, result.rows);
  } catch (err) {
    console.error('[analyticsService] fetchVaultSeries failed:', err);
    return [];
  }
}

export async function fetchStabilitySeries(limit = 365) {
  const key = `analytics:stability:${limit}`;
  const cached = getCached<any>(key, TTL.SERIES);
  if (cached) return cached;

  try {
    const result = await getActor().get_stability_series(rangeQuery(undefined, undefined, limit));
    return setCache(key, result.rows);
  } catch (err) {
    console.error('[analyticsService] fetchStabilitySeries failed:', err);
    return [];
  }
}

export async function fetchSwapSeries(limit = 365) {
  const key = `analytics:swaps:${limit}`;
  const cached = getCached<any>(key, TTL.SERIES);
  if (cached) return cached;

  try {
    const result = await getActor().get_swap_series(rangeQuery(undefined, undefined, limit));
    return setCache(key, result.rows);
  } catch (err) {
    console.error('[analyticsService] fetchSwapSeries failed:', err);
    return [];
  }
}

export async function fetchFeeSeries(limit = 365) {
  const key = `analytics:fees:${limit}`;
  const cached = getCached<any>(key, TTL.SERIES);
  if (cached) return cached;

  try {
    const result = await getActor().get_fee_series(rangeQuery(undefined, undefined, limit));
    return setCache(key, result.rows);
  } catch (err) {
    console.error('[analyticsService] fetchFeeSeries failed:', err);
    return [];
  }
}

export async function fetchLiquidationSeries(limit = 365) {
  const key = `analytics:liquidations:${limit}`;
  const cached = getCached<any>(key, TTL.SERIES);
  if (cached) return cached;

  try {
    const result = await getActor().get_liquidation_series(rangeQuery(undefined, undefined, limit));
    return setCache(key, result.rows);
  } catch (err) {
    console.error('[analyticsService] fetchLiquidationSeries failed:', err);
    return [];
  }
}

export async function fetchHolderSeries(token: string, limit = 365) {
  const key = `analytics:holders:${token}:${limit}`;
  const cached = getCached<any>(key, TTL.SERIES);
  if (cached) return cached;

  try {
    const principal = typeof token === 'string' ? (await import('@dfinity/principal')).Principal.fromText(token) : token;
    const result = await getActor().get_holder_series(rangeQuery(undefined, undefined, limit), principal);
    return setCache(key, result.rows);
  } catch (err) {
    console.error('[analyticsService] fetchHolderSeries failed:', err);
    return [];
  }
}

export async function fetchTwap(windowSecs = 3600) {
  const key = `analytics:twap:${windowSecs}`;
  const cached = getCached<any>(key, TTL.FAST);
  if (cached) return cached;

  try {
    const result = await getActor().get_twap({ window_secs: [BigInt(windowSecs)] });
    return setCache(key, result);
  } catch (err) {
    console.error('[analyticsService] fetchTwap failed:', err);
    return null;
  }
}

export async function fetchApys(windowDays = 7) {
  const key = `analytics:apys:${windowDays}`;
  const cached = getCached<any>(key, TTL.FAST);
  if (cached) return cached;

  try {
    const result = await getActor().get_apys({ window_days: [windowDays] });
    return setCache(key, result);
  } catch (err) {
    console.error('[analyticsService] fetchApys failed:', err);
    return null;
  }
}

export async function fetchPegStatus() {
  const key = 'analytics:peg';
  const cached = getCached<any>(key, TTL.FAST);
  if (cached) return cached;

  try {
    const result = await getActor().get_peg_status();
    return setCache(key, result.length > 0 ? result[0] : null);
  } catch (err) {
    console.error('[analyticsService] fetchPegStatus failed:', err);
    return null;
  }
}

export async function fetchTradeActivity(windowSecs = 86400) {
  const key = `analytics:trades:${windowSecs}`;
  const cached = getCached<any>(key, TTL.FAST);
  if (cached) return cached;

  try {
    const result = await getActor().get_trade_activity({ window_secs: [BigInt(windowSecs)] });
    return setCache(key, result);
  } catch (err) {
    console.error('[analyticsService] fetchTradeActivity failed:', err);
    return null;
  }
}

export async function fetchCollectorHealth() {
  const key = 'analytics:health';
  const cached = getCached<any>(key, TTL.FAST);
  if (cached) return cached;

  try {
    const result = await getActor().get_collector_health();
    return setCache(key, result);
  } catch (err) {
    console.error('[analyticsService] fetchCollectorHealth failed:', err);
    return null;
  }
}
```

- [ ] **Step 3: Verify frontend builds**

Run: `cd src/vault_frontend && npm run build 2>&1 | tail -10`

- [ ] **Step 4: Commit**

```bash
git add src/vault_frontend/src/lib/config.ts src/vault_frontend/src/lib/services/explorer/analyticsService.ts
git commit -m "feat(frontend): add analyticsService with typed fetch functions for analytics canister"
```

---

### Task 6: Explorer Stats Dashboard Page

**Files:**
- Rewrite: `src/vault_frontend/src/routes/explorer/stats/+page.svelte`
- Modify: `src/vault_frontend/src/routes/explorer/+layout.svelte`

- [ ] **Step 1: Add Stats link to layout navigation**

In `src/vault_frontend/src/routes/explorer/+layout.svelte`, add to the `navLinks` array:
```typescript
{ href: '/explorer/stats', label: 'Stats', exact: false },
```

- [ ] **Step 2: Rewrite stats page**

Replace `src/vault_frontend/src/routes/explorer/stats/+page.svelte` with a dashboard that shows:
- Protocol summary (TVL, debt, CR, vault count, 24h volume)
- Collateral prices with TWAPs
- Peg status
- APYs (LP + SP)
- Trade activity
- Collector health status

```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import StatCard from '$components/explorer/StatCard.svelte';
  import {
    fetchProtocolSummary, fetchTwap, fetchPegStatus,
    fetchApys, fetchTradeActivity, fetchCollectorHealth
  } from '$services/explorer/analyticsService';
  import { formatE8s, formatUsd, formatCR } from '$utils/explorerHelpers';

  const E8S = 100_000_000;

  let summary: any = $state(null);
  let twap: any = $state(null);
  let peg: any = $state(null);
  let apys: any = $state(null);
  let trades: any = $state(null);
  let health: any = $state(null);
  let loading = $state(true);
  let error: string | null = $state(null);

  onMount(async () => {
    try {
      const [s, t, p, a, tr, h] = await Promise.all([
        fetchProtocolSummary(),
        fetchTwap(3600),
        fetchPegStatus(),
        fetchApys(7),
        fetchTradeActivity(86400),
        fetchCollectorHealth(),
      ]);
      summary = s;
      twap = t;
      peg = p;
      apys = a;
      trades = tr;
      health = h;
    } catch (e: any) {
      error = e.message || 'Failed to load analytics data';
    } finally {
      loading = false;
    }
  });

  function fmtPct(v: number | undefined | null): string {
    if (v == null) return '--';
    return `${v.toFixed(2)}%`;
  }

  function fmtPrice(v: number | undefined | null): string {
    if (v == null) return '--';
    return `$${v.toFixed(4)}`;
  }

  function fmtBigE8s(v: bigint | number | undefined | null): string {
    if (v == null) return '--';
    return formatE8s(BigInt(v));
  }
</script>

<svelte:head>
  <title>Analytics | Rumi Explorer</title>
</svelte:head>

{#if loading}
  <div class="flex items-center justify-center py-20">
    <div class="h-8 w-8 animate-spin rounded-full border-2 border-indigo-500 border-t-transparent"></div>
  </div>
{:else if error}
  <div class="rounded-lg border border-red-500/30 bg-red-500/10 p-4 text-red-400">{error}</div>
{:else}
  <div class="space-y-8">
    <!-- Protocol Summary -->
    <section>
      <h2 class="mb-4 text-lg font-semibold text-white">Protocol Overview</h2>
      <div class="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-5">
        <StatCard label="Total TVL" value={summary ? fmtBigE8s(summary.total_collateral_usd_e8s) : '--'} />
        <StatCard label="Total Debt" value={summary ? fmtBigE8s(summary.total_debt_e8s) : '--'} />
        <StatCard label="System CR" value={summary ? formatCR(summary.system_cr_bps) : '--'} />
        <StatCard label="Vaults" value={summary?.total_vault_count?.toString() ?? '--'} />
        <StatCard label="24h Volume" value={summary ? fmtBigE8s(summary.volume_24h_e8s) : '--'} />
      </div>
    </section>

    <!-- Prices -->
    {#if summary?.prices?.length > 0}
    <section>
      <h2 class="mb-4 text-lg font-semibold text-white">Collateral Prices (1h TWAP)</h2>
      <div class="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4">
        {#each summary.prices as p}
          <StatCard label={p.symbol} value={fmtPrice(p.twap_price)} sub="Latest: {fmtPrice(p.latest_price)}" />
        {/each}
      </div>
    </section>
    {/if}

    <!-- Peg & APYs -->
    <section>
      <h2 class="mb-4 text-lg font-semibold text-white">Pool Health</h2>
      <div class="grid grid-cols-2 gap-3 sm:grid-cols-4">
        <StatCard label="3pool Imbalance" value={peg ? fmtPct(peg.max_imbalance_pct) : '--'} />
        <StatCard label="LP APY (7d)" value={apys ? fmtPct(apys.lp_apy_pct?.[0]) : '--'} />
        <StatCard label="SP APY (7d)" value={apys ? fmtPct(apys.sp_apy_pct?.[0]) : '--'} />
        <StatCard label="24h Swaps" value={trades?.total_swaps?.toString() ?? '--'} />
      </div>
    </section>

    <!-- Trade Activity -->
    {#if trades}
    <section>
      <h2 class="mb-4 text-lg font-semibold text-white">Trade Activity (24h)</h2>
      <div class="grid grid-cols-2 gap-3 sm:grid-cols-4">
        <StatCard label="3pool Swaps" value={trades.three_pool_swaps?.toString() ?? '0'} />
        <StatCard label="AMM Swaps" value={trades.amm_swaps?.toString() ?? '0'} />
        <StatCard label="Total Fees" value={fmtBigE8s(trades.total_fees_e8s)} />
        <StatCard label="Unique Traders" value={trades.unique_traders?.toString() ?? '0'} />
      </div>
    </section>
    {/if}

    <!-- Collector Health -->
    {#if health}
    <section>
      <h2 class="mb-4 text-lg font-semibold text-white">Collector Health</h2>
      <div class="overflow-x-auto rounded-lg border border-white/10">
        <table class="w-full text-sm text-white/70">
          <thead>
            <tr class="border-b border-white/10 text-left text-xs uppercase text-white/40">
              <th class="px-4 py-2">Cursor</th>
              <th class="px-4 py-2">Position</th>
              <th class="px-4 py-2">Source Count</th>
              <th class="px-4 py-2">Status</th>
            </tr>
          </thead>
          <tbody>
            {#each health.cursors as c}
              <tr class="border-b border-white/5">
                <td class="px-4 py-2 font-mono text-white/90">{c.name}</td>
                <td class="px-4 py-2">{c.cursor_position.toString()}</td>
                <td class="px-4 py-2">{c.source_count.toString()}</td>
                <td class="px-4 py-2">
                  {#if c.last_error?.[0]}
                    <span class="text-red-400" title={c.last_error[0]}>Error</span>
                  {:else}
                    <span class="text-green-400">OK</span>
                  {/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    </section>
    {/if}
  </div>
{/if}
```

- [ ] **Step 3: Verify frontend builds**

Run: `cd src/vault_frontend && npm run build 2>&1 | tail -10`

- [ ] **Step 4: Commit**

```bash
git add src/vault_frontend/src/routes/explorer/stats/+page.svelte src/vault_frontend/src/routes/explorer/+layout.svelte
git commit -m "feat(frontend): add analytics stats dashboard page with nav link"
```

---

### Task 7: Overview Page Analytics Integration

**Files:**
- Modify: `src/vault_frontend/src/routes/explorer/+page.svelte`

Replace the "Section 7: Historical Charts" data source from `fetchAllSnapshots` (backend snapshots) to use `fetchTvlSeries` and `fetchVaultSeries` from `analyticsService`. This gives the overview page proper daily time-series data from the analytics canister instead of the backend's raw snapshots.

- [ ] **Step 1: Add analytics imports and replace chart data source**

In `src/vault_frontend/src/routes/explorer/+page.svelte`:

Add import:
```typescript
import { fetchTvlSeries, fetchVaultSeries } from '$services/explorer/analyticsService';
```

Replace the Section 7 loading block (around line 490-499) from:
```typescript
const chartsPromise = (async () => {
    allSnapshots = await fetchAllSnapshots();
    ...
})();
```

To:
```typescript
const chartsPromise = (async () => {
    const [tvl, vaultSnaps] = await Promise.all([
        fetchTvlSeries(365),
        fetchVaultSeries(365),
    ]);
    allSnapshots = tvl.map((r: any) => ({
        timestamp_ns: r.timestamp_ns,
        total_icp_collateral_e8s: r.total_icp_collateral_e8s,
        total_icusd_supply_e8s: r.total_icusd_supply_e8s,
        system_collateral_ratio_bps: r.system_collateral_ratio_bps,
    }));
    chartsLoading = false;
})();
```

This preserves the existing chart rendering code (tvlData, debtData, crData deriveds) which already reads from `allSnapshots`, while sourcing from analytics.

- [ ] **Step 2: Verify frontend builds**

Run: `cd src/vault_frontend && npm run build 2>&1 | tail -10`

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/routes/explorer/+page.svelte
git commit -m "feat(frontend): source overview charts from analytics canister"
```

---

### Task 8: Holders Page Historical Trends

**Files:**
- Modify: `src/vault_frontend/src/routes/explorer/holders/+page.svelte`

Add a "Holder Trends" section below the existing holder table that shows a chart of holder count over time, sourced from `get_holder_series`.

- [ ] **Step 1: Add analytics import and trends section**

In `src/vault_frontend/src/routes/explorer/holders/+page.svelte`, add import:
```typescript
import { fetchHolderSeries } from '$services/explorer/analyticsService';
```

Add state:
```typescript
let holderTrends: any[] = $state([]);
let trendsLoading = $state(false);
```

Add a function to load trends (call it from the existing `loadHolders` after the holder data loads):
```typescript
async function loadTrends(token: TokenTab) {
    trendsLoading = true;
    try {
        const ledger = token === 'icusd' ? CANISTER_IDS.ICUSD_LEDGER : CANISTER_IDS.THREEPOOL;
        holderTrends = await fetchHolderSeries(ledger, 90);
    } catch (e) {
        console.error('[holders] trends load failed:', e);
    } finally {
        trendsLoading = false;
    }
}
```

Call `loadTrends(token)` at the end of `loadHolders`.

Add an SVG chart section after the existing data table in the template:

```svelte
<!-- Holder Trends -->
{#if holderTrends.length > 1}
<section class="mt-8">
  <h3 class="mb-3 text-base font-semibold text-white">Holder Count Over Time</h3>
  <div class="rounded-lg border border-white/10 bg-gray-900/50 p-4">
    {@const chartW = 600}
    {@const chartH = 140}
    {@const pad = 4}
    {@const points = holderTrends.map((r: any) => ({
      x: Number(r.timestamp_ns) / 1e9,
      y: Number(r.total_holders)
    }))}
    {@const xMin = Math.min(...points.map((p: any) => p.x))}
    {@const xMax = Math.max(...points.map((p: any) => p.x))}
    {@const yMin = Math.min(...points.map((p: any) => p.y))}
    {@const yMax = Math.max(...points.map((p: any) => p.y)) || 1}
    {@const xRange = xMax - xMin || 1}
    {@const yRange = yMax - yMin || 1}
    <svg viewBox="0 0 {chartW} {chartH}" class="w-full h-auto" preserveAspectRatio="none">
      <polyline
        fill="none"
        stroke="#6366f1"
        stroke-width="2"
        points={points.map((p: any) => {
          const x = pad + ((p.x - xMin) / xRange) * (chartW - pad * 2);
          const y = pad + (chartH - pad * 2) - ((p.y - yMin) / yRange) * (chartH - pad * 2);
          return `${x},${y}`;
        }).join(' ')}
      />
    </svg>
    <div class="mt-1 flex justify-between text-xs text-white/30">
      <span>{new Date(xMin * 1000).toLocaleDateString()}</span>
      <span>Latest: {points[points.length - 1]?.y ?? 0} holders</span>
      <span>{new Date(xMax * 1000).toLocaleDateString()}</span>
    </div>
  </div>
</section>
{/if}
```

- [ ] **Step 2: Verify frontend builds**

Run: `cd src/vault_frontend && npm run build 2>&1 | tail -10`

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/routes/explorer/holders/+page.svelte
git commit -m "feat(frontend): add historical holder trends chart from analytics canister"
```

---

### Task 9: Regenerate Candid and Final Review

- [ ] **Step 1: Rebuild wasm and regenerate candid (in case any changes)**

```bash
cargo build --target wasm32-unknown-unknown --release -p rumi_analytics
candid-extractor target/wasm32-unknown-unknown/release/rumi_analytics.wasm > src/rumi_analytics/rumi_analytics.did
dfx generate rumi_analytics
```

- [ ] **Step 2: Run full unit test suite**

Run: `cargo test -p rumi_analytics -- --nocapture 2>&1 | tail -15`

- [ ] **Step 3: Run full integration test suite**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test --test pocket_ic_analytics -- --nocapture 2>&1 | tail -10`

- [ ] **Step 4: Run frontend build**

Run: `cd src/vault_frontend && npm run build 2>&1 | tail -10`

- [ ] **Step 5: Review all changed files for issues**

- [ ] **Step 6: Fix any issues and recommit**

---

## Design Notes

**Why CSV over JSON for /api/series/*:** CSV is smaller, streams naturally, and is directly ingestable by data tools (pandas, Excel, Grafana). The frontend uses the candid actor interface for typed data; the CSV endpoints are for external consumers and tooling.

**Why not /api/events/since:** The spec mentions this endpoint, but the analytics canister already exposes `get_swap_series`, `get_liquidation_series`, etc. via candid queries. An HTTP events endpoint would duplicate that with worse typing. Deferred unless there's a specific consumer that needs it.

**Why serde_json for health only:** The health endpoint benefits from JSON since it has nested objects. CSV endpoints use hand-rolled formatting which avoids pulling in serde_json for row types (they're already CandidType, not Serialize for JSON). The serde_json dep is only used for the health endpoint.

**Frontend actor pattern:** `analyticsService.ts` follows the exact same pattern as `explorerService.ts` (TTL-based Map cache, anonymous HttpAgent, lazy actor creation) for consistency. No new patterns introduced.

**StatCard reuse:** The stats page reuses the existing `StatCard` component from the explorer components, maintaining visual consistency with the overview page.
