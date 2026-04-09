# Analytics Phase 3: Current-State Daily Collectors - Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add three daily collectors (extended TVL, vault snapshots, stability pool) to the live rumi_analytics canister, pulling from existing source canister queries.

**Architecture:** Extends the Phase 1 skeleton with two new StableLogs (vaults MemoryIds 12-13, stability MemoryIds 24-25), three source wrappers, three collectors running concurrently on the daily timer, two new query endpoints, and PocketIC integration tests. All new data types use Candid encoding with `Bound::Unbounded` for Storable, matching the existing DailyTvlRow pattern.

**Tech Stack:** Rust, ic-cdk 0.12.0, ic-stable-structures 0.6.7, candid 0.10.6, PocketIC 6.0.0

**Spec:** `docs/superpowers/specs/2026-04-09-analytics-phase3-collectors-design.md`

---

### Task 1: Source wrappers (stability_pool + three_pool + backend extensions)

**Files:**
- Create: `src/rumi_analytics/src/sources/stability_pool.rs`
- Create: `src/rumi_analytics/src/sources/three_pool.rs`
- Modify: `src/rumi_analytics/src/sources/backend.rs`
- Modify: `src/rumi_analytics/src/sources/mod.rs`

- [ ] **Step 1: Create `sources/stability_pool.rs`**

```rust
//! Typed wrapper around rumi_stability_pool queries.

use candid::{CandidType, Deserialize, Principal};

/// Subset of StabilityPoolStatus. Candid record subtyping lets us decode
/// only the fields we need; extra fields on the source side are ignored.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct StabilityPoolStatusSubset {
    pub total_deposits_e8s: u64,
    pub total_depositors: u64,
    pub total_liquidations_executed: u64,
    pub total_interest_received_e8s: u64,
    pub stablecoin_balances: Vec<(Principal, u64)>,
    pub collateral_gains: Vec<(Principal, u64)>,
}

pub async fn get_pool_status(canister: Principal) -> Result<StabilityPoolStatusSubset, String> {
    let res: Result<(StabilityPoolStatusSubset,), _> =
        ic_cdk::api::call::call(canister, "get_pool_status", ()).await;
    match res {
        Ok((status,)) => Ok(status),
        Err((code, msg)) => Err(format!("stability_pool.get_pool_status: {:?} {}", code, msg)),
    }
}
```

- [ ] **Step 2: Create `sources/three_pool.rs`**

```rust
//! Typed wrapper around rumi_3pool queries.

use candid::{CandidType, Deserialize, Nat, Principal};

/// Raw response subset from 3pool. Fields are Candid `nat` (arbitrary precision).
#[derive(CandidType, Deserialize, Clone, Debug)]
struct PoolStatusRaw {
    pub balances: Vec<Nat>,
    pub lp_total_supply: Nat,
    pub virtual_price: Nat,
}

/// Converted subset with u128 fields.
#[derive(Clone, Debug)]
pub struct ThreePoolStatusSubset {
    pub balances: Vec<u128>,
    pub lp_total_supply: u128,
    pub virtual_price: u128,
}

fn nat_to_u128(n: &Nat, field: &str) -> Result<u128, String> {
    n.0.clone()
        .try_into()
        .map_err(|_| format!("three_pool.{}: Nat overflows u128", field))
}

pub async fn get_pool_status(canister: Principal) -> Result<ThreePoolStatusSubset, String> {
    let res: Result<(PoolStatusRaw,), _> =
        ic_cdk::api::call::call(canister, "get_pool_status", ()).await;
    match res {
        Ok((raw,)) => {
            let balances: Result<Vec<u128>, String> = raw
                .balances
                .iter()
                .enumerate()
                .map(|(i, n)| nat_to_u128(n, &format!("balances[{}]", i)))
                .collect();
            Ok(ThreePoolStatusSubset {
                balances: balances?,
                lp_total_supply: nat_to_u128(&raw.lp_total_supply, "lp_total_supply")?,
                virtual_price: nat_to_u128(&raw.virtual_price, "virtual_price")?,
            })
        }
        Err((code, msg)) => Err(format!("three_pool.get_pool_status: {:?} {}", code, msg)),
    }
}
```

- [ ] **Step 3: Add `get_all_vaults` and `get_collateral_totals` to `sources/backend.rs`**

Append after the existing `get_protocol_status` function:

```rust
/// Subset of CandidVault from rumi_protocol_backend.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct CandidVaultSubset {
    pub vault_id: u64,
    pub collateral_amount: u64,
    pub borrowed_icusd_amount: u64,
    pub collateral_type: Principal,
    pub accrued_interest: u64,
}

pub async fn get_all_vaults(backend: Principal) -> Result<Vec<CandidVaultSubset>, String> {
    let res: Result<(Vec<CandidVaultSubset>,), _> =
        ic_cdk::api::call::call(backend, "get_all_vaults", ()).await;
    match res {
        Ok((vaults,)) => Ok(vaults),
        Err((code, msg)) => Err(format!("get_all_vaults: {:?} {}", code, msg)),
    }
}

/// Subset of CollateralTotals from rumi_protocol_backend.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct CollateralTotalsSubset {
    pub collateral_type: Principal,
    pub price: f64,
    pub total_collateral: u64,
    pub total_debt: u64,
    pub vault_count: u64,
}

pub async fn get_collateral_totals(backend: Principal) -> Result<Vec<CollateralTotalsSubset>, String> {
    let res: Result<(Vec<CollateralTotalsSubset>,), _> =
        ic_cdk::api::call::call(backend, "get_collateral_totals", ()).await;
    match res {
        Ok((totals,)) => Ok(totals),
        Err((code, msg)) => Err(format!("get_collateral_totals: {:?} {}", code, msg)),
    }
}
```

- [ ] **Step 4: Register new source modules in `sources/mod.rs`**

Replace contents with:

```rust
pub mod backend;
pub mod icusd_ledger;
pub mod stability_pool;
pub mod three_pool;
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo check -p rumi_analytics`

- [ ] **Step 6: Commit**

```bash
git add src/rumi_analytics/src/sources/
git commit -m "feat(analytics): add source wrappers for stability pool, 3pool, and backend vault queries"
```

---

### Task 2: Storage types + StableLog instantiation

**Files:**
- Modify: `src/rumi_analytics/src/storage.rs`

- [ ] **Step 1: Add `DailyVaultSnapshotRow` and `CollateralStats` types**

Add after the `DailyTvlRow` impl block (after line 190):

```rust
// --- DailyVaultSnapshotRow ---

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct CollateralStats {
    pub collateral_type: Principal,
    pub vault_count: u32,
    pub total_collateral_e8s: u64,
    pub total_debt_e8s: u64,
    pub min_cr_bps: u32,
    pub max_cr_bps: u32,
    pub median_cr_bps: u32,
    pub price_usd_e8s: u64,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct DailyVaultSnapshotRow {
    pub timestamp_ns: u64,
    pub total_vault_count: u32,
    pub total_collateral_usd_e8s: u64,
    pub total_debt_e8s: u64,
    pub median_cr_bps: u32,
    pub collaterals: Vec<CollateralStats>,
}

impl Storable for DailyVaultSnapshotRow {
    fn to_bytes(&self) -> Cow<[u8]> {
        Cow::Owned(Encode!(self).expect("DailyVaultSnapshotRow encode"))
    }
    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        Decode!(bytes.as_ref(), Self).expect("DailyVaultSnapshotRow decode")
    }
    const BOUND: Bound = Bound::Unbounded;
}

// --- DailyStabilityRow ---

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct DailyStabilityRow {
    pub timestamp_ns: u64,
    pub total_deposits_e8s: u64,
    pub total_depositors: u64,
    pub total_liquidations_executed: u64,
    pub total_interest_received_e8s: u64,
    pub stablecoin_balances: Vec<(Principal, u64)>,
    pub collateral_gains: Vec<(Principal, u64)>,
}

impl Storable for DailyStabilityRow {
    fn to_bytes(&self) -> Cow<[u8]> {
        Cow::Owned(Encode!(self).expect("DailyStabilityRow encode"))
    }
    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        Decode!(bytes.as_ref(), Self).expect("DailyStabilityRow decode")
    }
    const BOUND: Bound = Bound::Unbounded;
}
```

- [ ] **Step 2: Add new optional fields to `DailyTvlRow`**

Modify the existing `DailyTvlRow` struct (lines 174-180) to add the new fields:

```rust
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct DailyTvlRow {
    pub timestamp_ns: u64,
    pub total_icp_collateral_e8s: u128,
    pub total_icusd_supply_e8s: u128,
    pub system_collateral_ratio_bps: u32,
    // Phase 3 additions (Option for backward compat with existing rows)
    pub stability_pool_deposits_e8s: Option<u64>,
    pub three_pool_reserve_0_e8s: Option<u128>,
    pub three_pool_reserve_1_e8s: Option<u128>,
    pub three_pool_reserve_2_e8s: Option<u128>,
    pub three_pool_virtual_price_e18: Option<u128>,
    pub three_pool_lp_supply_e8s: Option<u128>,
}
```

- [ ] **Step 3: Instantiate the two new StableLogs in the `thread_local!` block**

Add after the `DAILY_TVL_LOG` definition (after line 211):

```rust
    static DAILY_VAULTS_LOG: RefCell<ic_stable_structures::StableLog<DailyVaultSnapshotRow, Memory, Memory>> =
        RefCell::new({
            let idx = MEMORY_MANAGER.with(|m| m.borrow().get(MEM_DAILY_VAULTS_IDX));
            let data = MEMORY_MANAGER.with(|m| m.borrow().get(MEM_DAILY_VAULTS_DATA));
            ic_stable_structures::StableLog::init(idx, data)
                .expect("init DAILY_VAULTS log")
        });

    static DAILY_STABILITY_LOG: RefCell<ic_stable_structures::StableLog<DailyStabilityRow, Memory, Memory>> =
        RefCell::new({
            let idx = MEMORY_MANAGER.with(|m| m.borrow().get(MEM_DAILY_STABILITY_IDX));
            let data = MEMORY_MANAGER.with(|m| m.borrow().get(MEM_DAILY_STABILITY_DATA));
            ic_stable_structures::StableLog::init(idx, data)
                .expect("init DAILY_STABILITY log")
        });
```

- [ ] **Step 4: Add `daily_vaults` and `daily_stability` accessor modules**

Add after the existing `pub mod daily_tvl` block (after line 278):

```rust
pub mod daily_vaults {
    use super::*;

    pub fn push(row: DailyVaultSnapshotRow) {
        DAILY_VAULTS_LOG.with(|log| {
            log.borrow_mut().append(&row).expect("append DAILY_VAULTS");
        });
    }

    pub fn len() -> u64 {
        DAILY_VAULTS_LOG.with(|log| log.borrow().len())
    }

    pub fn get(index: u64) -> Option<DailyVaultSnapshotRow> {
        DAILY_VAULTS_LOG.with(|log| log.borrow().get(index))
    }

    pub fn range(from_ts: u64, to_ts: u64, limit: usize) -> Vec<DailyVaultSnapshotRow> {
        let mut out = Vec::new();
        DAILY_VAULTS_LOG.with(|log| {
            let log = log.borrow();
            let n = log.len();
            for i in 0..n {
                if let Some(row) = log.get(i) {
                    if row.timestamp_ns >= to_ts {
                        break;
                    }
                    if row.timestamp_ns >= from_ts {
                        out.push(row);
                        if out.len() >= limit {
                            break;
                        }
                    }
                }
            }
        });
        out
    }
}

pub mod daily_stability {
    use super::*;

    pub fn push(row: DailyStabilityRow) {
        DAILY_STABILITY_LOG.with(|log| {
            log.borrow_mut().append(&row).expect("append DAILY_STABILITY");
        });
    }

    pub fn len() -> u64 {
        DAILY_STABILITY_LOG.with(|log| log.borrow().len())
    }

    pub fn get(index: u64) -> Option<DailyStabilityRow> {
        DAILY_STABILITY_LOG.with(|log| log.borrow().get(index))
    }

    pub fn range(from_ts: u64, to_ts: u64, limit: usize) -> Vec<DailyStabilityRow> {
        let mut out = Vec::new();
        DAILY_STABILITY_LOG.with(|log| {
            let log = log.borrow();
            let n = log.len();
            for i in 0..n {
                if let Some(row) = log.get(i) {
                    if row.timestamp_ns >= to_ts {
                        break;
                    }
                    if row.timestamp_ns >= from_ts {
                        out.push(row);
                        if out.len() >= limit {
                            break;
                        }
                    }
                }
            }
        });
        out
    }
}
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo check -p rumi_analytics`

- [ ] **Step 6: Commit**

```bash
git add src/rumi_analytics/src/storage.rs
git commit -m "feat(analytics): add vault + stability storage types and StableLogs"
```

---

### Task 3: Extend TVL collector for stability pool + 3pool

**Files:**
- Modify: `src/rumi_analytics/src/collectors/tvl.rs`

- [ ] **Step 1: Rewrite `collectors/tvl.rs` with concurrent calls and partial failure**

Replace the entire file contents:

```rust
//! Daily TVL collector (Phase 3).
//!
//! Pulls from three sources concurrently: backend (protocol status),
//! stability pool (deposits), and 3pool (reserves). If stability pool
//! or 3pool calls fail, the row is still written with those fields as None.
//! Backend failure still aborts the entire row (same as Phase 1).

use crate::{sources, state, storage};

pub async fn run() -> Result<(), String> {
    let (backend_id, sp_id, tp_id) = state::read_state(|s| {
        (s.sources.backend, s.sources.stability_pool, s.sources.three_pool)
    });

    // Fire all three calls concurrently.
    let (backend_res, sp_res, tp_res) = futures::join!(
        sources::backend::get_protocol_status(backend_id),
        sources::stability_pool::get_pool_status(sp_id),
        sources::three_pool::get_pool_status(tp_id),
    );

    // Backend is required; if it fails, abort.
    let status = match backend_res {
        Ok(s) => s,
        Err(e) => {
            state::mutate_state(|s| s.error_counters.backend += 1);
            return Err(e);
        }
    };

    // Stability pool is optional.
    let sp_deposits = match sp_res {
        Ok(sp) => Some(sp.total_deposits_e8s),
        Err(e) => {
            ic_cdk::println!("rumi_analytics: stability pool TVL fetch failed: {}", e);
            state::mutate_state(|s| s.error_counters.stability_pool += 1);
            None
        }
    };

    // 3pool is optional.
    let (tp_r0, tp_r1, tp_r2, tp_vp, tp_lp) = match tp_res {
        Ok(tp) => (
            tp.balances.first().copied(),
            tp.balances.get(1).copied(),
            tp.balances.get(2).copied(),
            Some(tp.virtual_price),
            Some(tp.lp_total_supply),
        ),
        Err(e) => {
            ic_cdk::println!("rumi_analytics: 3pool TVL fetch failed: {}", e);
            state::mutate_state(|s| s.error_counters.three_pool += 1);
            (None, None, None, None, None)
        }
    };

    let cr_bps = (status.total_collateral_ratio * 10_000.0)
        .clamp(0.0, u32::MAX as f64) as u32;

    let row = storage::DailyTvlRow {
        timestamp_ns: ic_cdk::api::time(),
        total_icp_collateral_e8s: status.total_icp_margin as u128,
        total_icusd_supply_e8s: status.total_icusd_borrowed as u128,
        system_collateral_ratio_bps: cr_bps,
        stability_pool_deposits_e8s: sp_deposits,
        three_pool_reserve_0_e8s: tp_r0,
        three_pool_reserve_1_e8s: tp_r1,
        three_pool_reserve_2_e8s: tp_r2,
        three_pool_virtual_price_e18: tp_vp,
        three_pool_lp_supply_e8s: tp_lp,
    };
    storage::daily_tvl::push(row);

    state::mutate_state(|s| s.last_daily_snapshot_ns = ic_cdk::api::time());
    Ok(())
}
```

- [ ] **Step 2: Add `futures` crate to Cargo.toml**

Add to `[dependencies]` in `src/rumi_analytics/Cargo.toml`:

```toml
futures = "0.3"
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p rumi_analytics`

- [ ] **Step 4: Commit**

```bash
git add src/rumi_analytics/src/collectors/tvl.rs src/rumi_analytics/Cargo.toml
git commit -m "feat(analytics): extend TVL collector with stability pool + 3pool data"
```

---

### Task 4: Vault snapshot collector

**Files:**
- Create: `src/rumi_analytics/src/collectors/vaults.rs`
- Modify: `src/rumi_analytics/src/collectors/mod.rs`

- [ ] **Step 1: Create `collectors/vaults.rs`**

```rust
//! Daily vault snapshot collector.
//!
//! Calls get_all_vaults() and get_collateral_totals() concurrently, groups
//! vaults by collateral type, computes per-collateral and protocol-wide
//! aggregate stats (vault count, total collateral, total debt, min/max/median CR).

use std::collections::HashMap;

use candid::Principal;

use crate::{sources, state, storage};

pub async fn run() -> Result<(), String> {
    let backend = state::read_state(|s| s.sources.backend);

    let (vaults_res, totals_res) = futures::join!(
        sources::backend::get_all_vaults(backend),
        sources::backend::get_collateral_totals(backend),
    );

    let vaults = match vaults_res {
        Ok(v) => v,
        Err(e) => {
            state::mutate_state(|s| s.error_counters.backend += 1);
            return Err(format!("vaults collector: {}", e));
        }
    };

    let totals = match totals_res {
        Ok(t) => t,
        Err(e) => {
            state::mutate_state(|s| s.error_counters.backend += 1);
            return Err(format!("vaults collector (collateral totals): {}", e));
        }
    };

    // Build a price lookup: collateral_type -> price (USD, as f64).
    let prices: HashMap<Principal, f64> = totals
        .iter()
        .map(|t| (t.collateral_type, t.price))
        .collect();

    // Group vaults by collateral type and compute CRs.
    let mut groups: HashMap<Principal, Vec<u32>> = HashMap::new();
    let mut all_crs: Vec<u32> = Vec::with_capacity(vaults.len());

    for vault in &vaults {
        let debt = vault.borrowed_icusd_amount.saturating_add(vault.accrued_interest);
        if debt == 0 {
            continue;
        }
        let price = prices.get(&vault.collateral_type).copied().unwrap_or(0.0);
        let collateral_usd = (vault.collateral_amount as f64) * price;
        let cr_bps = ((collateral_usd / debt as f64) * 10_000.0)
            .clamp(0.0, u32::MAX as f64) as u32;

        groups.entry(vault.collateral_type).or_default().push(cr_bps);
        all_crs.push(cr_bps);
    }

    // Build per-collateral stats.
    let mut collaterals: Vec<storage::CollateralStats> = Vec::new();
    let mut total_collateral_usd: f64 = 0.0;
    let mut total_debt: u64 = 0;

    for ct in &totals {
        let crs = groups.get(&ct.collateral_type);
        let (min_cr, max_cr, median_cr) = match crs {
            Some(crs) if !crs.is_empty() => {
                let mut sorted = crs.clone();
                sorted.sort_unstable();
                (sorted[0], sorted[sorted.len() - 1], median_of_sorted(&sorted))
            }
            _ => (0, 0, 0),
        };

        let price_usd_e8s = (ct.price * 1e8).clamp(0.0, u64::MAX as f64) as u64;
        total_collateral_usd += ct.total_collateral as f64 * ct.price;
        total_debt += ct.total_debt;

        collaterals.push(storage::CollateralStats {
            collateral_type: ct.collateral_type,
            vault_count: ct.vault_count as u32,
            total_collateral_e8s: ct.total_collateral,
            total_debt_e8s: ct.total_debt,
            min_cr_bps: min_cr,
            max_cr_bps: max_cr,
            median_cr_bps: median_cr,
            price_usd_e8s,
        });
    }

    // Protocol-wide median CR.
    all_crs.sort_unstable();
    let global_median = if all_crs.is_empty() { 0 } else { median_of_sorted(&all_crs) };

    let total_collateral_usd_e8s = total_collateral_usd.clamp(0.0, u64::MAX as f64) as u64;

    // total_vault_count counts only vaults with debt > 0 (matching the CR stats).
    // Zero-debt vaults are excluded since they have no meaningful collateral ratio.
    let active_vault_count = all_crs.len() as u32;

    let row = storage::DailyVaultSnapshotRow {
        timestamp_ns: ic_cdk::api::time(),
        total_vault_count: active_vault_count,
        total_collateral_usd_e8s,
        total_debt_e8s: total_debt,
        median_cr_bps: global_median,
        collaterals,
    };
    storage::daily_vaults::push(row);

    Ok(())
}

fn median_of_sorted(sorted: &[u32]) -> u32 {
    let n = sorted.len();
    if n == 0 {
        return 0;
    }
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        // Average of two middle values, rounded down.
        let a = sorted[n / 2 - 1] as u64;
        let b = sorted[n / 2] as u64;
        ((a + b) / 2) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_of_sorted_odd() {
        assert_eq!(median_of_sorted(&[100, 200, 300]), 200);
    }

    #[test]
    fn median_of_sorted_even() {
        assert_eq!(median_of_sorted(&[100, 200, 300, 400]), 250);
    }

    #[test]
    fn median_of_sorted_single() {
        assert_eq!(median_of_sorted(&[500]), 500);
    }

    #[test]
    fn median_of_sorted_empty() {
        assert_eq!(median_of_sorted(&[]), 0);
    }
}
```

- [ ] **Step 2: Register vaults module in `collectors/mod.rs`**

Add the vaults module (stability is added in Task 5 when the file exists):

```rust
pub mod tvl;
pub mod vaults;
```

- [ ] **Step 3: Run unit tests**

Run: `cargo test -p rumi_analytics --lib`
Expected: 4 median tests + 2 existing range_query tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/rumi_analytics/src/collectors/
git commit -m "feat(analytics): add vault snapshot collector with per-collateral + protocol-wide stats"
```

---

### Task 5: Stability pool collector

**Files:**
- Create: `src/rumi_analytics/src/collectors/stability.rs`

- [ ] **Step 1: Create `collectors/stability.rs`**

```rust
//! Daily stability pool snapshot collector.
//!
//! Calls stability_pool.get_pool_status() and writes a DailyStabilityRow.

use crate::{sources, state, storage};

pub async fn run() -> Result<(), String> {
    let sp_id = state::read_state(|s| s.sources.stability_pool);

    let status = match sources::stability_pool::get_pool_status(sp_id).await {
        Ok(s) => s,
        Err(e) => {
            state::mutate_state(|s| s.error_counters.stability_pool += 1);
            return Err(e);
        }
    };

    let row = storage::DailyStabilityRow {
        timestamp_ns: ic_cdk::api::time(),
        total_deposits_e8s: status.total_deposits_e8s,
        total_depositors: status.total_depositors,
        total_liquidations_executed: status.total_liquidations_executed,
        total_interest_received_e8s: status.total_interest_received_e8s,
        stablecoin_balances: status.stablecoin_balances,
        collateral_gains: status.collateral_gains,
    };
    storage::daily_stability::push(row);

    Ok(())
}
```

- [ ] **Step 2: Add stability module to `collectors/mod.rs`**

Update to include all three modules:

```rust
pub mod tvl;
pub mod vaults;
pub mod stability;
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p rumi_analytics`

- [ ] **Step 4: Commit**

```bash
git add src/rumi_analytics/src/collectors/
git commit -m "feat(analytics): add stability pool snapshot collector"
```

---

### Task 6: Wire collectors into daily timer

**Files:**
- Modify: `src/rumi_analytics/src/timers.rs`

- [ ] **Step 1: Rewrite `timers.rs` to run all three collectors concurrently**

Replace the entire file:

```rust
//! Timer wiring. Phase 3 populates the daily tier with three collectors:
//! TVL, vault snapshots, and stability pool. The 60s pull cycle refreshes
//! the supply cache (unchanged from Phase 1).

use std::time::Duration;

use crate::{collectors, sources, state};

pub fn setup_timers() {
    ic_cdk_timers::set_timer_interval(Duration::from_secs(60), || {
        ic_cdk::spawn(pull_cycle());
    });
    ic_cdk_timers::set_timer_interval(Duration::from_secs(300), || {
        // Phase 5: fast snapshot.
    });
    ic_cdk_timers::set_timer_interval(Duration::from_secs(3600), || {
        // Phase 5: hourly snapshot.
    });
    ic_cdk_timers::set_timer_interval(Duration::from_secs(86400), || {
        ic_cdk::spawn(daily_snapshot());
    });
}

async fn pull_cycle() {
    refresh_supply_cache().await;
}

async fn refresh_supply_cache() {
    let ledger = state::read_state(|s| s.sources.icusd_ledger);
    match sources::icusd_ledger::icrc1_total_supply(ledger).await {
        Ok(total) => {
            state::mutate_state(|s| s.circulating_supply_icusd_e8s = Some(total));
        }
        Err(e) => {
            ic_cdk::println!("rumi_analytics: supply refresh failed: {}", e);
            state::mutate_state(|s| s.error_counters.icusd_ledger += 1);
        }
    }
}

async fn daily_snapshot() {
    // Run all three collectors concurrently. Each handles its own errors
    // independently; one failure does not block the others.
    let (tvl_res, vaults_res, stability_res) = futures::join!(
        collectors::tvl::run(),
        collectors::vaults::run(),
        collectors::stability::run(),
    );

    if let Err(e) = tvl_res {
        ic_cdk::println!("rumi_analytics: daily TVL snapshot failed: {}", e);
    }
    if let Err(e) = vaults_res {
        ic_cdk::println!("rumi_analytics: daily vault snapshot failed: {}", e);
    }
    if let Err(e) = stability_res {
        ic_cdk::println!("rumi_analytics: daily stability snapshot failed: {}", e);
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p rumi_analytics`

- [ ] **Step 3: Commit**

```bash
git add src/rumi_analytics/src/timers.rs
git commit -m "feat(analytics): wire vault + stability collectors into daily timer with concurrent execution"
```

---

### Task 7: Query endpoints + response types

**Files:**
- Modify: `src/rumi_analytics/src/types.rs`
- Modify: `src/rumi_analytics/src/queries/historical.rs`
- Modify: `src/rumi_analytics/src/queries/mod.rs` (no change needed if already `pub mod historical`)
- Modify: `src/rumi_analytics/src/lib.rs`

- [ ] **Step 1: Add response types to `types.rs`**

Append to `src/rumi_analytics/src/types.rs`:

```rust
use crate::storage::{DailyVaultSnapshotRow, DailyStabilityRow, CollateralStats};

#[derive(CandidType, Clone, Debug)]
pub struct VaultSeriesResponse {
    pub rows: Vec<DailyVaultSnapshotRow>,
    pub next_from_ts: Option<u64>,
}

#[derive(CandidType, Clone, Debug)]
pub struct StabilitySeriesResponse {
    pub rows: Vec<DailyStabilityRow>,
    pub next_from_ts: Option<u64>,
}
```

Also update the existing import at the top of `types.rs` to include the new types:

```rust
use crate::storage::{DailyTvlRow, DailyVaultSnapshotRow, DailyStabilityRow};
```

- [ ] **Step 2: Add query functions to `queries/historical.rs`**

Append after the existing `get_tvl_series` function (before the `#[cfg(test)]` block):

```rust
use crate::types::{VaultSeriesResponse, StabilitySeriesResponse};

pub fn get_vault_series(query: RangeQuery) -> VaultSeriesResponse {
    let limit = query.resolved_limit() as usize;
    let from = query.resolved_from();
    let to = query.resolved_to();

    let rows = storage::daily_vaults::range(from, to, limit);

    let next_from_ts = if rows.len() == limit && limit > 0 {
        rows.last().map(|r| r.timestamp_ns.saturating_add(1))
    } else {
        None
    };

    VaultSeriesResponse { rows, next_from_ts }
}

pub fn get_stability_series(query: RangeQuery) -> StabilitySeriesResponse {
    let limit = query.resolved_limit() as usize;
    let from = query.resolved_from();
    let to = query.resolved_to();

    let rows = storage::daily_stability::range(from, to, limit);

    let next_from_ts = if rows.len() == limit && limit > 0 {
        rows.last().map(|r| r.timestamp_ns.saturating_add(1))
    } else {
        None
    };

    StabilitySeriesResponse { rows, next_from_ts }
}
```

- [ ] **Step 3: Add canister entry points in `lib.rs`**

Add after the existing `get_tvl_series` endpoint (after line 69):

```rust
#[ic_cdk_macros::query]
fn get_vault_series(query: types::RangeQuery) -> types::VaultSeriesResponse {
    queries::historical::get_vault_series(query)
}

#[ic_cdk_macros::query]
fn get_stability_series(query: types::RangeQuery) -> types::StabilitySeriesResponse {
    queries::historical::get_stability_series(query)
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p rumi_analytics`

- [ ] **Step 5: Commit**

```bash
git add src/rumi_analytics/src/types.rs src/rumi_analytics/src/queries/ src/rumi_analytics/src/lib.rs
git commit -m "feat(analytics): add get_vault_series + get_stability_series query endpoints"
```

---

### Task 8: Update Candid interface

**Files:**
- Modify: `src/rumi_analytics/rumi_analytics.did`

- [ ] **Step 1: Regenerate the .did file**

The canister uses `ic_cdk::export_candid!()` which auto-generates the interface. Build the canister to get the new .did:

Run: `cargo build -p rumi_analytics --target wasm32-unknown-unknown --release`

Then extract the Candid interface:

Run: `candid-extractor target/wasm32-unknown-unknown/release/rumi_analytics.wasm > src/rumi_analytics/rumi_analytics.did`

If `candid-extractor` is not available, manually update the .did file by adding these types and methods:

```candid
type CollateralStats = record {
  collateral_type : principal;
  vault_count : nat32;
  total_collateral_e8s : nat64;
  total_debt_e8s : nat64;
  min_cr_bps : nat32;
  max_cr_bps : nat32;
  median_cr_bps : nat32;
  price_usd_e8s : nat64;
};

type DailyVaultSnapshotRow = record {
  timestamp_ns : nat64;
  total_vault_count : nat32;
  total_collateral_usd_e8s : nat64;
  total_debt_e8s : nat64;
  median_cr_bps : nat32;
  collaterals : vec CollateralStats;
};

type DailyStabilityRow = record {
  timestamp_ns : nat64;
  total_deposits_e8s : nat64;
  total_depositors : nat64;
  total_liquidations_executed : nat64;
  total_interest_received_e8s : nat64;
  stablecoin_balances : vec record { principal; nat64 };
  collateral_gains : vec record { principal; nat64 };
};

type VaultSeriesResponse = record {
  rows : vec DailyVaultSnapshotRow;
  next_from_ts : opt nat64;
};

type StabilitySeriesResponse = record {
  rows : vec DailyStabilityRow;
  next_from_ts : opt nat64;
};
```

Add to the existing `DailyTvlRow` type the new optional fields:

```candid
type DailyTvlRow = record {
  timestamp_ns : nat64;
  total_icp_collateral_e8s : nat;
  total_icusd_supply_e8s : nat;
  system_collateral_ratio_bps : nat32;
  stability_pool_deposits_e8s : opt nat64;
  three_pool_reserve_0_e8s : opt nat;
  three_pool_reserve_1_e8s : opt nat;
  three_pool_reserve_2_e8s : opt nat;
  three_pool_virtual_price_e18 : opt nat;
  three_pool_lp_supply_e8s : opt nat;
};
```

Add to the service block:

```candid
  get_vault_series : (RangeQuery) -> (VaultSeriesResponse) query;
  get_stability_series : (RangeQuery) -> (StabilitySeriesResponse) query;
```

- [ ] **Step 2: Regenerate declarations**

Run: `dfx generate rumi_analytics` (ensure dfx is on PATH: `export PATH="$HOME/Library/Application Support/org.dfinity.dfx/bin:$PATH"`)

- [ ] **Step 3: Verify full compile**

Run: `cargo check -p rumi_analytics`

- [ ] **Step 4: Commit**

```bash
git add src/rumi_analytics/rumi_analytics.did src/declarations/rumi_analytics/
git commit -m "feat(analytics): update candid interface + declarations for Phase 3 endpoints"
```

---

### Task 9: PocketIC integration tests

**Files:**
- Modify: `src/rumi_analytics/tests/pocket_ic_analytics.rs`

The existing test harness deploys `analytics + backend + icusd_ledger` but passes `Principal::anonymous()` for `three_pool` and `stability_pool`. Phase 3 tests need real 3pool and stability pool canisters deployed so the collectors can pull data. However, deploying real 3pool/stability pool requires complex init args and token setups. A simpler approach: the collectors already handle failures gracefully (TVL writes partial rows, vaults/stability skip the row). We test:

1. **Extended TVL with partial failure** - stability pool and 3pool are anonymous (calls will fail), verify TVL row still has backend fields populated and new fields are None.
2. **Vault snapshot with real backend** - the backend already deploys with no vaults, so `get_all_vaults` returns an empty vec and `get_collateral_totals` returns an empty vec. Verify an empty vault snapshot row is written with zero counts.
3. **Stability snapshot skipped on failure** - stability pool is anonymous, verify the call fails gracefully and no stability row is written.
4. **Upgrade preserves all three logs** - write to all logs, upgrade, verify counts.

For testing with actual populated data (vaults with collateral, SP with deposits), that requires the full protocol test fixture which is already covered by `pocket_ic_tests`. Phase 3 integration tests focus on the analytics canister wiring working correctly.

- [ ] **Step 1: Add test-side type definitions**

Add after the existing `TvlSeriesResponse` struct (after line 132):

```rust
#[derive(CandidType, Deserialize, Debug)]
struct CollateralStats {
    collateral_type: Principal,
    vault_count: u32,
    total_collateral_e8s: u64,
    total_debt_e8s: u64,
    min_cr_bps: u32,
    max_cr_bps: u32,
    median_cr_bps: u32,
    price_usd_e8s: u64,
}

#[derive(CandidType, Deserialize, Debug)]
struct DailyVaultSnapshotRow {
    timestamp_ns: u64,
    total_vault_count: u32,
    total_collateral_usd_e8s: u64,
    total_debt_e8s: u64,
    median_cr_bps: u32,
    collaterals: Vec<CollateralStats>,
}

#[derive(CandidType, Deserialize, Debug)]
struct VaultSeriesResponse {
    rows: Vec<DailyVaultSnapshotRow>,
    next_from_ts: Option<u64>,
}

#[derive(CandidType, Deserialize, Debug)]
struct DailyStabilityRow {
    timestamp_ns: u64,
    total_deposits_e8s: u64,
    total_depositors: u64,
    total_liquidations_executed: u64,
    total_interest_received_e8s: u64,
    stablecoin_balances: Vec<(Principal, u64)>,
    collateral_gains: Vec<(Principal, u64)>,
}

#[derive(CandidType, Deserialize, Debug)]
struct StabilitySeriesResponse {
    rows: Vec<DailyStabilityRow>,
    next_from_ts: Option<u64>,
}
```

- [ ] **Step 2: Add query helper functions**

Add after the existing `get_tvl_series` helper (after line 315):

```rust
fn get_vault_series(env: &Env, q: RangeQueryArg) -> VaultSeriesResponse {
    let result = env
        .pic
        .query_call(
            env.analytics,
            env.admin,
            "get_vault_series",
            encode_one(q).unwrap(),
        )
        .expect("get_vault_series query");
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).unwrap(),
        WasmResult::Reject(msg) => panic!("get_vault_series rejected: {}", msg),
    }
}

fn get_stability_series(env: &Env, q: RangeQueryArg) -> StabilitySeriesResponse {
    let result = env
        .pic
        .query_call(
            env.analytics,
            env.admin,
            "get_stability_series",
            encode_one(q).unwrap(),
        )
        .expect("get_stability_series query");
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).unwrap(),
        WasmResult::Reject(msg) => panic!("get_stability_series rejected: {}", msg),
    }
}
```

- [ ] **Step 3: Update `DailyTvlRow` test type to include new optional fields**

Update the existing `DailyTvlRow` struct in the test file (lines 120-126):

```rust
#[derive(CandidType, Deserialize, Debug)]
struct DailyTvlRow {
    timestamp_ns: u64,
    total_icp_collateral_e8s: candid::Nat,
    total_icusd_supply_e8s: candid::Nat,
    system_collateral_ratio_bps: u32,
    stability_pool_deposits_e8s: Option<u64>,
    three_pool_reserve_0_e8s: Option<candid::Nat>,
    three_pool_reserve_1_e8s: Option<candid::Nat>,
    three_pool_reserve_2_e8s: Option<candid::Nat>,
    three_pool_virtual_price_e18: Option<candid::Nat>,
    three_pool_lp_supply_e8s: Option<candid::Nat>,
}
```

- [ ] **Step 4: Add new test: `tvl_extended_fields_none_when_sources_unavailable`**

```rust
#[test]
fn tvl_extended_fields_none_when_sources_unavailable() {
    let env = setup();
    // stability_pool and three_pool are Principal::anonymous(), so calls fail.
    // TVL row should still be written with backend data, new fields as None.
    env.pic.advance_time(std::time::Duration::from_secs(86_400 + 65));
    for _ in 0..10 {
        env.pic.tick();
    }

    let resp = get_tvl_series(
        &env,
        RangeQueryArg { from_ts: None, to_ts: None, limit: None, offset: None },
    );
    assert!(!resp.rows.is_empty(), "TVL row should be written even with SP/3pool failures");
    let row = &resp.rows[0];
    assert!(row.stability_pool_deposits_e8s.is_none(), "SP should be None when unavailable");
    assert!(row.three_pool_reserve_0_e8s.is_none(), "3pool reserve should be None when unavailable");
}
```

- [ ] **Step 5: Add new test: `vault_snapshot_written_with_empty_protocol`**

```rust
#[test]
fn vault_snapshot_written_with_empty_protocol() {
    let env = setup();
    env.pic.advance_time(std::time::Duration::from_secs(86_400 + 65));
    for _ in 0..10 {
        env.pic.tick();
    }

    let resp = get_vault_series(
        &env,
        RangeQueryArg { from_ts: None, to_ts: None, limit: None, offset: None },
    );
    assert!(!resp.rows.is_empty(), "vault snapshot should be written even with 0 vaults");
    let row = &resp.rows[0];
    assert_eq!(row.total_vault_count, 0);
    assert_eq!(row.total_debt_e8s, 0);
    assert!(row.collaterals.is_empty(), "no collaterals when no vaults exist");
}
```

- [ ] **Step 6: Add new test: `stability_snapshot_skipped_when_source_unavailable`**

```rust
#[test]
fn stability_snapshot_skipped_when_source_unavailable() {
    let env = setup();
    // stability_pool is Principal::anonymous(), call will fail, no row written.
    env.pic.advance_time(std::time::Duration::from_secs(86_400 + 65));
    for _ in 0..10 {
        env.pic.tick();
    }

    let resp = get_stability_series(
        &env,
        RangeQueryArg { from_ts: None, to_ts: None, limit: None, offset: None },
    );
    assert!(resp.rows.is_empty(), "no stability row when source canister unavailable");
}
```

- [ ] **Step 7: Update the `upgrade_preserves_supply_cache_and_tvl_log` test**

Rename to `upgrade_preserves_all_logs` and extend to also check vault log count survives upgrade. Add after the existing supply/tvl checks:

```rust
    let before_vault_rows = get_vault_series(
        &env,
        RangeQueryArg { from_ts: None, to_ts: None, limit: None, offset: None },
    )
    .rows
    .len();
```

And after the upgrade, add:

```rust
    let after_vault_rows = get_vault_series(
        &env,
        RangeQueryArg { from_ts: None, to_ts: None, limit: None, offset: None },
    )
    .rows
    .len();
    assert_eq!(before_vault_rows, after_vault_rows, "vault log lost rows on upgrade");
```

- [ ] **Step 8: Build the wasm and run integration tests**

Run:
```bash
cargo build -p rumi_analytics --target wasm32-unknown-unknown --release && \
cargo build -p rumi_protocol_backend --target wasm32-unknown-unknown --release && \
POCKET_IC_BIN=./pocket-ic cargo test --test pocket_ic_analytics -- --nocapture
```

Expected: All tests pass (existing 4 + new 3 = 7 tests, with the upgrade test extended).

- [ ] **Step 9: Commit**

```bash
git add src/rumi_analytics/tests/pocket_ic_analytics.rs
git commit -m "test(analytics): add Phase 3 integration tests for vault + stability + extended TVL"
```

---

### Task 10: Deploy to mainnet

**Files:** None (operational task)

- [ ] **Step 1: Run full test suite one final time**

```bash
cargo test -p rumi_analytics --lib && \
cargo build -p rumi_analytics --target wasm32-unknown-unknown --release && \
POCKET_IC_BIN=./pocket-ic cargo test --test pocket_ic_analytics -- --nocapture
```

- [ ] **Step 2: Deploy upgrade to mainnet**

The analytics canister's `post_upgrade` takes no arguments, so deploy without `--argument`:

```bash
export PATH="$HOME/Library/Application Support/org.dfinity.dfx/bin:$HOME/.cargo/bin:$PATH"
dfx deploy rumi_analytics --network ic
```

- [ ] **Step 3: Smoke test**

Wait 2 minutes for the daily timer to NOT have fired yet (it's a 24h interval), but verify the canister is healthy:

```bash
dfx canister call rumi_analytics ping --network ic
# Expected: ("rumi_analytics ok")

dfx canister call rumi_analytics get_vault_series '(record {})' --network ic
# Expected: (record { rows = vec {}; next_from_ts = null }) -- no rows yet, daily hasn't fired

dfx canister call rumi_analytics get_stability_series '(record {})' --network ic
# Expected: (record { rows = vec {}; next_from_ts = null })
```

Verify the existing TVL rows survived the upgrade:

```bash
dfx canister call rumi_analytics get_tvl_series '(record {})' --network ic
# Expected: rows from Phase 1 deployment still present
```

- [ ] **Step 4: Commit canister_ids.json if changed**

Check if `canister_ids.json` was modified (it shouldn't be since the canister already exists).

```bash
git status
```
