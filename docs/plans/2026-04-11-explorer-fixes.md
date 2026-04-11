# Explorer Fixes Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix all broken data displays in the Rumi Explorer (dashboard, markets, pools, revenue, activity, risk tabs).
**Architecture:** Two-layer fix: (1) analytics canister backend needs to run daily collectors on init and fix 3pool decimal normalization, (2) frontend needs field name corrections and proper per-token decimal handling.
**Tech Stack:** Rust (IC canister), SvelteKit/TypeScript (frontend)

---

## Context

The analytics canister (`dtlu2-uqaaa-aaaap-qugcq-cai`) was recently deployed. Its data collection runs on timers:
- 60s: event tailing (pull cycle)
- 300s: fast price/3pool snapshots
- 3600s: hourly snapshots
- 86400s: daily snapshots (TVL, vaults, stability, holders, rollups)

`set_timer_interval` does NOT fire immediately -- it waits for the first interval to elapse. Since the daily timer hasn't fired yet, all daily storage is empty, which cascades into zeros everywhere.

Additionally, the 3pool's `get_pool_status()` returns balances in each token's native decimals (icUSD=8, ckUSDT=6, ckUSDC=6). The analytics canister sums these raw values for peg computation, producing a bogus ~196% imbalance even for a balanced pool.

The frontend also references `total_collateral_e8s` and `total_debt_e8s` from the backend's `CollateralTotals`, but those fields are actually named `total_collateral` and `total_debt`.

---

## Step 1: Run daily collectors on canister init/upgrade

**Problem:** `get_protocol_summary()` returns all zeros because `daily_vaults::len() == 0`. No TVL series, no vault series, no stability series, no fee/swap rollups. This causes: dashboard vitals all zeros, all charts blank, APY calculations return None, revenue page all zeros, risk page all zeros.

**Files:**
- `src/rumi_analytics/src/timers.rs`

**Change:** Add a one-shot timer (0-second delay) in `setup_timers()` that runs the daily snapshot immediately on init/upgrade.

```rust
// In setup_timers(), add at the top before the interval timers:
ic_cdk_timers::set_timer(Duration::from_secs(0), || {
    ic_cdk::spawn(daily_snapshot());
});
```

**Why a 0-second timer instead of calling directly:** `daily_snapshot()` is async (makes inter-canister calls), so it can't run synchronously inside `init`/`post_upgrade`. A 0-delay timer fires on the next message execution, which supports async.

**Test:** After deploying, call `get_protocol_summary` and verify `total_vault_count > 0`, `total_collateral_usd_e8s > 0`, `total_debt_e8s > 0`.

---

## Step 2: Fix 3pool peg computation -- normalize balances by decimal

**Problem:** `compute_peg_status` sums raw `u128` balances from the 3pool, but icUSD (8 decimals) has 100x larger raw values per dollar than ckUSDT/ckUSDC (6 decimals). A perfectly balanced pool shows ~196% imbalance.

**Files:**
- `src/rumi_analytics/src/collectors/fast.rs` -- store token decimals alongside balances
- `src/rumi_analytics/src/sources/three_pool.rs` -- capture `tokens` from `PoolStatusRaw`
- `src/rumi_analytics/src/storage/fast.rs` -- add `decimals` field to `Fast3PoolSnapshot`
- `src/rumi_analytics/src/queries/live.rs` -- normalize balances before peg computation

### 2a: Capture token decimals from 3pool response

**File:** `src/rumi_analytics/src/sources/three_pool.rs`

Add `decimals` field to `ThreePoolStatusSubset`:

```rust
pub struct ThreePoolStatusSubset {
    pub balances: Vec<u128>,
    pub lp_total_supply: u128,
    #[allow(dead_code)]
    pub current_a: u64,
    pub virtual_price: u128,
    #[allow(dead_code)]
    pub swap_fee_bps: u64,
    #[allow(dead_code)]
    pub admin_fee_bps: u64,
    pub decimals: Vec<u8>,  // NEW: per-token decimals
}
```

Add `tokens` field to `PoolStatusRaw` to capture the token configs:

```rust
pub struct PoolStatusRaw {
    pub balances: Vec<Nat>,
    pub lp_total_supply: Nat,
    pub current_a: u64,
    pub virtual_price: Nat,
    pub swap_fee_bps: u64,
    pub admin_fee_bps: u64,
    pub tokens: Vec<TokenConfigRaw>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct TokenConfigRaw {
    pub ledger_id: Principal,
    pub symbol: String,
    pub decimals: u8,
    pub precision_mul: u64,
}
```

In `get_pool_status`, extract decimals:

```rust
let decimals = raw.tokens.iter().map(|t| t.decimals).collect();
Ok(ThreePoolStatusSubset {
    // ... existing fields ...
    decimals,
})
```

### 2b: Store decimals in Fast3PoolSnapshot

**File:** `src/rumi_analytics/src/storage/fast.rs`

Add `decimals` field:

```rust
pub struct Fast3PoolSnapshot {
    pub timestamp_ns: u64,
    pub balances: Vec<u128>,
    pub virtual_price: u128,
    pub lp_total_supply: u128,
    pub decimals: Vec<u8>,  // NEW
}
```

### 2c: Pass decimals through fast collector

**File:** `src/rumi_analytics/src/collectors/fast.rs`

In the `Ok(tp)` match arm, include decimals:

```rust
storage::fast::fast_3pool::push(storage::fast::Fast3PoolSnapshot {
    timestamp_ns: now,
    balances: tp.balances,
    virtual_price: tp.virtual_price,
    lp_total_supply: tp.lp_total_supply,
    decimals: tp.decimals,  // NEW
});
```

### 2d: Normalize balances in peg computation

**File:** `src/rumi_analytics/src/queries/live.rs`

Update `compute_peg_status` to normalize to highest decimal (8):

```rust
pub fn compute_peg_status(snap: &storage::fast::Fast3PoolSnapshot) -> types::PegStatus {
    // Normalize all balances to a common scale (highest decimal = 8).
    let max_dec = snap.decimals.iter().copied().max().unwrap_or(8);
    let normalized: Vec<u128> = snap.balances.iter()
        .enumerate()
        .map(|(i, b)| {
            let dec = snap.decimals.get(i).copied().unwrap_or(max_dec);
            let scale = 10u128.pow((max_dec - dec) as u32);
            b * scale
        })
        .collect();

    let total: u128 = normalized.iter().sum();
    let count = normalized.len();

    let (balance_ratios, max_imbalance_pct) = if count > 0 && total > 0 {
        let target = total as f64 / count as f64;
        let ratios: Vec<f64> = normalized.iter()
            .map(|b| *b as f64 / target)
            .collect();
        let max_dev = ratios.iter()
            .map(|r| (r - 1.0).abs())
            .fold(0.0f64, f64::max);
        (ratios, max_dev * 100.0)
    } else {
        (vec![], 0.0)
    };

    types::PegStatus {
        timestamp_ns: snap.timestamp_ns,
        pool_balances: snap.balances.clone(),  // Keep raw for display
        virtual_price: snap.virtual_price,
        balance_ratios,
        max_imbalance_pct,
    }
}
```

### 2e: Update existing unit tests

**File:** `src/rumi_analytics/src/queries/live.rs`

Update `Fast3PoolSnapshot` construction in tests to include `decimals: vec![8, 8, 8]` (existing tests use equal decimals so behavior stays the same).

Add new test for mixed-decimal normalization:

```rust
#[test]
fn peg_mixed_decimals_balanced() {
    // $100 each: icUSD (8 dec) = 10_000_000_000, ckUSDT (6 dec) = 100_000_000, ckUSDC (6 dec) = 100_000_000
    let snap = Fast3PoolSnapshot {
        timestamp_ns: 1_000_000_000,
        balances: vec![10_000_000_000, 100_000_000, 100_000_000],
        virtual_price: 1_000_000_000_000_000_000,
        lp_total_supply: 300_000_000,
        decimals: vec![8, 6, 6],
    };
    let status = compute_peg_status(&snap);
    // After normalization all should be 10_000_000_000 -> perfectly balanced
    assert!(status.max_imbalance_pct < 0.01, "expected near-zero imbalance, got {}", status.max_imbalance_pct);
}
```

**Test:** `cargo test -p rumi_analytics`

---

## Step 3: Fix frontend field name mismatch for collateral totals

**Problem:** Backend `CollateralTotals` has `total_collateral` and `total_debt`, but the frontend references `total_collateral_e8s` and `total_debt_e8s`, getting `undefined` and falling to 0.

**Files (find/replace in each):**
- `src/vault_frontend/src/routes/explorer/+page.svelte` (dashboard)
- `src/vault_frontend/src/routes/explorer/markets/+page.svelte`
- `src/vault_frontend/src/routes/explorer/risk/+page.svelte`

**Change:** In each file, replace all occurrences of:
- `total_collateral_e8s` with `total_collateral` when accessing backend collateral totals (`backendStats?.total_collateral_e8s` or `tot?.total_collateral_e8s`)
- `total_debt_e8s` with `total_debt` when accessing backend collateral totals

Be careful NOT to change references to analytics canister `CollateralStats` fields, which genuinely use `_e8s` suffixes (e.g., `stats.total_collateral_e8s` from `latestVaultSnapshot.collaterals`).

### Dashboard (+page.svelte) specific changes:

Lines ~196-199: Change `backendStats` fallback field names:
```typescript
// BEFORE:
const totalCollateralE8s = stats
  ? e8sToNumber(stats.total_collateral_e8s)
  : (backendStats?.total_collateral_e8s != null ? e8sToNumber(backendStats.total_collateral_e8s) : 0);
const totalDebtE8s = stats
  ? e8sToNumber(stats.total_debt_e8s)
  : (backendStats?.total_debt_e8s != null ? e8sToNumber(backendStats.total_debt_e8s) : 0);

// AFTER:
const totalCollateralE8s = stats
  ? e8sToNumber(stats.total_collateral_e8s)
  : (backendStats?.total_collateral != null ? e8sToNumber(backendStats.total_collateral) : 0);
const totalDebtE8s = stats
  ? e8sToNumber(stats.total_debt_e8s)
  : (backendStats?.total_debt != null ? e8sToNumber(backendStats.total_debt) : 0);
```

### Markets (+page.svelte) specific changes:

Lines ~93-97: Change `tot` field names:
```typescript
// BEFORE:
const totalCollateral = tot?.total_collateral_e8s != null
  ? e8sToNumber(tot.total_collateral_e8s) : 0;
const totalDebt = tot?.total_debt_e8s != null
  ? e8sToNumber(tot.total_debt_e8s) : 0;

// AFTER:
const totalCollateral = tot?.total_collateral != null
  ? e8sToNumber(tot.total_collateral) : 0;
const totalDebt = tot?.total_debt != null
  ? e8sToNumber(tot.total_debt) : 0;
```

### Risk (+page.svelte) specific changes:

Lines ~115-116:
```typescript
// BEFORE:
const totalColl = tot?.total_collateral_e8s != null ? e8sToNumber(tot.total_collateral_e8s) : 0;
const debt = tot?.total_debt_e8s != null ? e8sToNumber(tot.total_debt_e8s) : 0;

// AFTER:
const totalColl = tot?.total_collateral != null ? e8sToNumber(tot.total_collateral) : 0;
const debt = tot?.total_debt != null ? e8sToNumber(tot.total_debt) : 0;
```

**Test:** Build frontend (`npm run build` in `src/vault_frontend`), deploy, verify collateral overview shows actual values.

---

## Step 4: Fix 3pool balance display in frontend (decimal-aware)

**Problem:** The pools page uses `e8sToNumber()` (divides by 1e8) for all 3 token balances, but ckUSDT and ckUSDC use 6 decimals, so their values display 100x too small.

**File:** `src/vault_frontend/src/routes/explorer/pools/+page.svelte`

### 4a: Add per-token decimal info

Update `POOL_TOKENS` to include decimals:

```typescript
const POOL_TOKENS = [
  { symbol: 'icUSD', color: '#2DD4BF', decimals: 8 },
  { symbol: 'ckUSDT', color: '#26A17B', decimals: 6 },
  { symbol: 'ckUSDC', color: '#2775CA', decimals: 6 },
];
```

### 4b: Use correct decimals for balance conversion

Replace the `poolTokenBalances` derived:

```typescript
const poolTokenBalances = $derived.by(() => {
  if (!pegStatus?.pool_balances || pegStatus.pool_balances.length < 3) {
    return POOL_TOKENS.map(t => ({ ...t, balance: 0 }));
  }
  return POOL_TOKENS.map((t, i) => ({
    ...t,
    balance: Number(pegStatus!.pool_balances[i]) / Math.pow(10, t.decimals),
  }));
});
```

**Test:** Deploy frontend, verify pool composition shows reasonable percentages (not 98.6/0.5/0.6).

---

## Step 5: Fix Stability Pool deposit label

**Problem:** Pools page says "537 icUSD" for SP total deposits, but the user deposited 3USD (ckUSDC via 3pool). The stability pool now holds multiple stablecoin types.

**File:** `src/vault_frontend/src/routes/explorer/pools/+page.svelte`

Change the label from "icUSD" to a more generic label. Line ~260:

```svelte
<!-- BEFORE -->
{formatCompact(spTotalDeposits)} icUSD

<!-- AFTER -->
{formatCompact(spTotalDeposits)} stablecoins
```

Also verify: is `spTotalDeposits` correctly capturing the full value? The SP status may report in icUSD-equivalent regardless (check the `total_deposits` field). If it only counts icUSD deposits, this is a deeper backend issue. For now just fix the label.

---

## Step 6: Remove clipboard emoji from event links

**Problem:** Events show a 📋 emoji that the user doesn't want. Should just say "Event #123".

**File:** `src/vault_frontend/src/lib/components/explorer/EntityLink.svelte`

Remove the event icon:

```typescript
// BEFORE:
const icons: Record<string, string> = {
  vault: '🏦',
  address: '👤',
  canister: '📦',
  token: '🪙',
  event: '📋',
  block_index: '🔗',
};

// AFTER:
const icons: Record<string, string> = {
  vault: '🏦',
  address: '👤',
  canister: '📦',
  token: '🪙',
  event: '',
  block_index: '🔗',
};
```

**Test:** Deploy frontend, verify activity tab shows "Event #123" without any emoji.

---

## Step 7: Fix system tab missing timestamps

**Problem:** System/admin events show no timestamps. The `getEventTimestamp` function looks for `data.timestamp` inside the event variant, but many admin events store timestamps as `opt nat64` which comes through as `[bigint]` or `[]`. The function handles this, so the issue is likely that the admin events simply don't have timestamp fields in their Candid type.

**File:** `src/vault_frontend/src/lib/utils/eventFormatters.ts`

The `getEventTimestamp` already handles `[bigint]` arrays and raw `bigint`. The issue is that backend admin events (like `set_borrowing_fee`, `set_recovery_rate_curve`, etc.) genuinely don't include timestamp fields in the Candid response. These are fire-and-forget config changes.

**Workaround:** For the activity page's system tab, use the event's position in the event log as a proxy. Backend events are sequential, so we can estimate timing from neighboring events. However, this is complex and fragile.

**Simpler approach:** Remove the Principal column from the system tab (since only admin can call these) and show a note like "Admin config change" instead of a timestamp. The events are still ordered by event index, which provides temporal ordering.

**File:** `src/vault_frontend/src/routes/explorer/activity/+page.svelte`

No structural change needed for now. The system events display correctly with "--" for timestamp and principal. The user said "there doesn't really need to be a principal column" for system events, but since the activity page uses a shared table layout across all filter tabs, removing the column just for system would require tab-specific column rendering. This is low priority.

---

## Step 8: Build and deploy analytics canister

**Commands:**

```bash
# Build
cargo build -p rumi_analytics --target wasm32-unknown-unknown --release

# Run unit tests
cargo test -p rumi_analytics

# Deploy to mainnet (upgrade, not reinstall!)
dfx deploy rumi_analytics --network ic --argument '(variant { Upgrade = record { mode = null; description = opt "Run daily collectors on init; fix 3pool decimal normalization" } })'
```

Wait -- check the analytics canister's upgrade arg format. It may not use the same `Upgrade` variant as the backend. Check `dfx.json` for any specified args.

Actually, the analytics canister uses a simple `InitArgs` for init and raw `post_upgrade` with no args. So just:

```bash
dfx deploy rumi_analytics --network ic
```

**Verify:** After deploy, wait 60 seconds (for the 0-delay timer + fast snapshot to run), then:

```bash
dfx canister call rumi_analytics get_protocol_summary --network ic
dfx canister call rumi_analytics get_peg_status --network ic
dfx canister call rumi_analytics get_collector_health --network ic
```

Check that:
- `total_vault_count > 0`
- `total_collateral_usd_e8s > 0`
- `max_imbalance_pct` is a reasonable number (< 50% for a moderately balanced pool)

---

## Step 9: Build and deploy frontend

**Commands:**

```bash
cd src/vault_frontend
npm run build
cd ../..
dfx deploy vault_frontend --network ic
```

**Verify:** Open https://app.rumiprotocol.com/explorer and check:
- Dashboard vitals show real numbers
- TVL chart has data points
- Collateral overview shows prices, vault counts, collateral values, debt values
- Pool composition shows reasonable percentages
- Markets page loads and shows data
- Risk page loads and shows data
- Revenue page shows fee data (after daily rollup runs)
- Activity events show "Event #123" without clipboard emoji

---

## Summary of issues NOT addressed in this plan

1. **Redemption + RedemptionTransfer showing separately** -- skipped per user request.
2. **ckXAUT price not showing** -- needs investigation of what the backend returns for this collateral's price. Check `dfx canister call rumi_protocol_backend get_collateral_totals --network ic` and look at the ckXAUT entry's `price` field. If it's 0, the issue is in the backend's XRC price feed, not the analytics canister.
3. **Markets/Risk page not loading** -- most likely caused by the all-zeros data creating weird display states. Steps 1-3 should fix this. If they still don't load after fixing the data, check browser console for JS errors.
4. **No volume data** -- genuine absence of DEX trading volume. Not a bug.
5. **Virtual price display** -- the frontend divides by 1e18, which is correct per the 3pool's docs (`virtual_price` scaled by 1e18). The odd display ("2.8000 3USD = $1.4000") is likely an artifact of extremely imbalanced pool state. Should resolve once peg computation is fixed and pool TVL displays correctly.
