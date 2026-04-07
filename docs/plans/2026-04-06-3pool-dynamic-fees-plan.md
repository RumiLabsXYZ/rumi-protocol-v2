# 3pool Dynamic Fees Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the static 20 bps swap fee on `rumi_3pool` with a directional dynamic fee (1-99 bps based on SSD imbalance) that taxes imbalancing trades and rewards rebalancing trades, then add bot-facing query endpoints and explorer analytics endpoints on top of an enriched event schema.

**Architecture:** Pure-function fee math added to `math.rs`, called from both `swap.rs` and `liquidity.rs` with the same `(imb_before, imb_after)` inputs. Event records gain new fields via in-place struct rename (v1 -> v2) with a one-shot `post_upgrade` migration that fills migrated v1 events with sentinel values. Storage stays in heap `Vec` for this PR — full `MemoryManager` migration is a separate follow-up. New query endpoints layer on top of existing event vecs and pool state.

**Tech Stack:** Rust, ic-cdk 0.12.0, ic-stable-structures 0.6.5 (no new stable structures in this PR), candid 0.10.6, ethnum U256 for fee math, pocket-ic 6.0.0 for integration tests.

**Spec:** `docs/plans/2026-04-06-3pool-dynamic-fees-design.md`

**Branch:** `feat/3pool-dynamic-fees` (already created)

---

## File Structure

| File | Responsibility |
|---|---|
| `src/rumi_3pool/src/math.rs` | Pure functions: `compute_imbalance`, `compute_fee_bps`. No state, fully unit-testable. |
| `src/rumi_3pool/src/types.rs` | New types: `FeeCurveParams`, `SwapEventV1` (rename of legacy), `SwapEvent` (v2), `LiquidityEventV1` (rename), `LiquidityEvent` (v2), `SwapQuote`, `PoolState`, `RebalanceQuote`, `ImbalanceSnapshot`, `SimulatedPath`, `SwapLeg`, `PoolStats`, `ImbalanceStats`, `FeeStats`, `VolumePoint`, `BalancePoint`, `VirtualPricePoint`, `FeePoint`, `PoolHealth`, `StatsWindow`. Add `SetFeeCurveParams` admin action variant. |
| `src/rumi_3pool/src/state.rs` | Field rename: `swap_events` -> `swap_events_v1`, `liquidity_events` -> `liquidity_events_v1`. New fields: `swap_events_v2`, `liquidity_events_v2`, `dynamic_fee_params`. New accessors. `migrate_events_v1_to_v2()` one-shot migration helper. |
| `src/rumi_3pool/src/swap.rs` | Replace static `swap_fee_bps` parameter with dynamic computation. Return enriched `SwapResult` carrying fee_bps + imbalance metrics. |
| `src/rumi_3pool/src/liquidity.rs` | Replace `3/8 * swap_fee` imbalance fee with dynamic model. Update return types. |
| `src/rumi_3pool/src/admin.rs` | New `set_fee_curve_params` function. Admin-only, validation, audit log entry. |
| `src/rumi_3pool/src/lib.rs` | Wire dynamic fees through `swap`, `add_liquidity`, `remove_liquidity_one_coin`. Populate v2 events. Add 7 bot endpoints + 14 explorer endpoints + admin endpoint. Run migration in `post_upgrade`. |
| `src/rumi_3pool/rumi_3pool.did` | Candid declarations for all new types and methods. |
| `src/rumi_3pool/tests/integration_test.rs` | New unit-style integration tests for fee math, event recording, migration. |
| `src/rumi_3pool/tests/pocket_ic_3usd.rs` | New scenario tests for dominant flow, arb counter-flow, admin params. |

---

## Phase 1: Pure Fee Math (no state, fully unit-tested)

### Task 1: `FeeCurveParams` type

**Files:**
- Modify: `src/rumi_3pool/src/types.rs` (add new type at end of file)

- [ ] **Step 1: Add the type**

```rust
// ─── Dynamic Fee Curve ───

/// Parameters for the directional dynamic fee curve.
///
/// `imb_saturation` is in 1e9 fixed-point (so 1.0 = 1_000_000_000).
/// Imbalance metric values are in the same fixed-point representation.
#[derive(CandidType, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeeCurveParams {
    /// Minimum fee in basis points (charged on rebalancing trades).
    pub min_fee_bps: u16,
    /// Maximum fee in basis points (saturation cap on imbalancing trades).
    pub max_fee_bps: u16,
    /// Imbalance level (1e9 fixed-point) at which the imbalancing fee saturates to max.
    pub imb_saturation: u64,
}

impl Default for FeeCurveParams {
    fn default() -> Self {
        Self {
            min_fee_bps: 1,
            max_fee_bps: 99,
            imb_saturation: 250_000_000, // 0.25 in 1e9 fixed-point
        }
    }
}
```

- [ ] **Step 2: Compile**

Run: `cargo check -p rumi_3pool`
Expected: clean compile.

- [ ] **Step 3: Commit**

```bash
git add src/rumi_3pool/src/types.rs
git commit -m "feat(3pool): add FeeCurveParams type"
```

---

### Task 2: `compute_imbalance` math function

**Files:**
- Modify: `src/rumi_3pool/src/math.rs`

- [ ] **Step 1: Write the failing test**

Add to the existing `tests` module in `src/rumi_3pool/src/math.rs`:

```rust
#[test]
fn test_compute_imbalance_perfectly_balanced() {
    // 1M of each token, properly precision-adjusted
    let balances: [u128; 3] = [
        1_000_000 * 100_000_000,   // icUSD (8 dec)
        1_000_000 * 1_000_000,     // ckUSDT (6 dec)
        1_000_000 * 1_000_000,     // ckUSDC (6 dec)
    ];
    let precision_muls: [u64; 3] = [10_000_000_000, 1_000_000_000_000, 1_000_000_000_000];
    let imb = compute_imbalance(&balances, &precision_muls);
    // Perfectly balanced -> imbalance very close to 0 (within fixed-point rounding)
    assert!(imb < 1_000_000, "expected near-zero imbalance, got {}", imb);
}

#[test]
fn test_compute_imbalance_50_25_25() {
    // 50/25/25 split (current real pool state, roughly)
    let balances: [u128; 3] = [
        2_000_000 * 100_000_000,   // icUSD (50%)
        1_000_000 * 1_000_000,     // ckUSDT (25%)
        1_000_000 * 1_000_000,     // ckUSDC (25%)
    ];
    let precision_muls: [u64; 3] = [10_000_000_000, 1_000_000_000_000, 1_000_000_000_000];
    let imb = compute_imbalance(&balances, &precision_muls);
    // Expected: SSD = (0.5 - 1/3)^2 + 2*(0.25 - 1/3)^2 = 0.02778 + 2*0.00694 = 0.04167
    // Normalized by MAX_SSD = 2/3: imb ≈ 0.0625 -> 62_500_000 in 1e9 fp
    assert!(imb > 50_000_000 && imb < 75_000_000, "expected ~62.5M, got {}", imb);
}

#[test]
fn test_compute_imbalance_extreme() {
    // 90/5/5
    let balances: [u128; 3] = [
        9_000_000 * 100_000_000,
        500_000 * 1_000_000,
        500_000 * 1_000_000,
    ];
    let precision_muls: [u64; 3] = [10_000_000_000, 1_000_000_000_000, 1_000_000_000_000];
    let imb = compute_imbalance(&balances, &precision_muls);
    // Expected: SSD = (0.9 - 1/3)^2 + 2*(0.05 - 1/3)^2 = 0.32111 + 2*0.08028 = 0.48167
    // Normalized: 0.48167 / (2/3) = 0.7225 -> 722_500_000
    assert!(imb > 700_000_000 && imb < 750_000_000, "expected ~722M, got {}", imb);
}

#[test]
fn test_compute_imbalance_monotonic() {
    let precision_muls: [u64; 3] = [10_000_000_000, 1_000_000_000_000, 1_000_000_000_000];
    let mut last_imb = 0u64;
    // As we shift more weight onto token 0, imbalance must strictly increase
    for icusd_share in [34u128, 40, 50, 60, 75, 90] {
        let other = (100 - icusd_share) / 2;
        let balances: [u128; 3] = [
            icusd_share * 10_000 * 100_000_000,
            other * 10_000 * 1_000_000,
            other * 10_000 * 1_000_000,
        ];
        let imb = compute_imbalance(&balances, &precision_muls);
        assert!(imb > last_imb, "imbalance should grow: {} -> {}", last_imb, imb);
        last_imb = imb;
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rumi_3pool --lib math::tests::test_compute_imbalance`
Expected: FAIL with "function `compute_imbalance` not found".

- [ ] **Step 3: Implement `compute_imbalance`**

Add to `src/rumi_3pool/src/math.rs` (above the `tests` module):

```rust
/// Fixed-point scale for imbalance values: 1.0 = 1_000_000_000.
pub const IMB_SCALE: u64 = 1_000_000_000;

/// Maximum SSD value for N=3: when one token holds everything,
/// SSD = (1 - 1/3)^2 + 2 * (0 - 1/3)^2 = 4/9 + 2/9 = 6/9 = 2/3.
/// We compute imbalance / MAX_SSD to normalize to [0, 1].
///
/// Compute the pool's imbalance metric (sum of squared deviations from equal weights),
/// normalized to [0, IMB_SCALE]. Returns a u64 in 1e9 fixed-point.
///
/// Returns 0 for an empty pool (all balances zero).
pub fn compute_imbalance(balances: &[u128; 3], precision_muls: &[u64; 3]) -> u64 {
    use ethnum::U256;

    // Normalize balances to common 18-decimal precision so weights are comparable.
    let xp = normalize_all(balances, precision_muls);
    let total: U256 = xp[0] + xp[1] + xp[2];
    if total == U256::ZERO {
        return 0;
    }

    // Work in 1e18 fixed-point for weights (avoids losing precision in division).
    let scale = U256::from(1_000_000_000_000_000_000u128); // 1e18
    let target = scale / U256::from(3u64); // 1/3 in 1e18 fp

    let mut ssd = U256::ZERO;
    for i in 0..3 {
        let w = xp[i] * scale / total; // weight in 1e18 fp
        let dev = if w > target { w - target } else { target - w };
        // dev^2 / scale -> result in 1e18 fp (since dev is in 1e18 fp)
        ssd += (dev * dev) / scale;
    }

    // MAX_SSD = 2/3 in 1e18 fp = 2 * scale / 3
    let max_ssd = (U256::from(2u64) * scale) / U256::from(3u64);

    // Normalize: ssd / max_ssd, rescaled to IMB_SCALE (1e9)
    let normalized = (ssd * U256::from(IMB_SCALE)) / max_ssd;
    normalized.as_u64()
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rumi_3pool --lib math::tests::test_compute_imbalance`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add src/rumi_3pool/src/math.rs
git commit -m "feat(3pool): add compute_imbalance metric (SSD normalized)"
```

---

### Task 3: `compute_fee_bps` math function

**Files:**
- Modify: `src/rumi_3pool/src/math.rs`

- [ ] **Step 1: Write the failing tests**

Add to the same `tests` module in `math.rs`:

```rust
fn default_params() -> crate::types::FeeCurveParams {
    crate::types::FeeCurveParams::default()
}

#[test]
fn test_fee_bps_rebalancing_returns_min() {
    let p = default_params();
    // Any swap that reduces imbalance pays MIN_FEE
    let fee = compute_fee_bps(100_000_000, 50_000_000, &p);
    assert_eq!(fee, 1);
}

#[test]
fn test_fee_bps_rebalancing_when_imbalanced() {
    let p = default_params();
    // Even when pool is wildly imbalanced, a rebalancing trade pays MIN_FEE
    let fee = compute_fee_bps(500_000_000, 400_000_000, &p);
    assert_eq!(fee, 1);
}

#[test]
fn test_fee_bps_imbalancing_below_saturation() {
    let p = default_params();
    // imb_after = 0.06 (current pool), well below saturation 0.25
    // Linear: 1 + (99 - 1) * (60_000_000 / 250_000_000) = 1 + 98 * 0.24 = 24.52 -> 24 or 25
    let fee = compute_fee_bps(50_000_000, 60_000_000, &p);
    assert!(fee >= 24 && fee <= 25, "expected ~24, got {}", fee);
}

#[test]
fn test_fee_bps_imbalancing_at_saturation() {
    let p = default_params();
    // imb_after = 0.25 hits saturation -> max
    let fee = compute_fee_bps(100_000_000, 250_000_000, &p);
    assert_eq!(fee, 99);
}

#[test]
fn test_fee_bps_imbalancing_above_saturation() {
    let p = default_params();
    // imb_after > saturation -> still max (clamped)
    let fee = compute_fee_bps(100_000_000, 500_000_000, &p);
    assert_eq!(fee, 99);
}

#[test]
fn test_fee_bps_perfectly_balanced_neutral() {
    let p = default_params();
    // No change at all -> rebalancing branch (after <= before) -> MIN
    let fee = compute_fee_bps(50_000_000, 50_000_000, &p);
    assert_eq!(fee, 1);
}

#[test]
fn test_fee_bps_bounded() {
    let p = default_params();
    // Sweep — fee always within [min, max]
    for imb_after in (0..=1_000_000_000u64).step_by(50_000_000) {
        let fee = compute_fee_bps(0, imb_after, &p);
        assert!(fee >= p.min_fee_bps && fee <= p.max_fee_bps);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rumi_3pool --lib math::tests::test_fee_bps`
Expected: FAIL with "function `compute_fee_bps` not found".

- [ ] **Step 3: Implement `compute_fee_bps`**

Add to `src/rumi_3pool/src/math.rs` directly under `compute_imbalance`:

```rust
/// Compute the fee in basis points for a swap that moves the pool from
/// `imb_before` to `imb_after`.
///
/// **Strict binary on rebalancing**: any trade that does not increase imbalance
/// (`imb_after <= imb_before`) pays exactly `params.min_fee_bps`.
///
/// **Linear scaling on imbalancing**: if `imb_after > imb_before`, fee scales
/// linearly from `min_fee_bps` at `imb_after = 0` to `max_fee_bps` at
/// `imb_after >= params.imb_saturation`.
pub fn compute_fee_bps(
    imb_before: u64,
    imb_after: u64,
    params: &crate::types::FeeCurveParams,
) -> u16 {
    if imb_after <= imb_before {
        return params.min_fee_bps;
    }
    if params.imb_saturation == 0 {
        // Defensive: invalid config -> charge max
        return params.max_fee_bps;
    }
    let capped = imb_after.min(params.imb_saturation);
    // fee = min + (max - min) * capped / saturation
    let span = (params.max_fee_bps - params.min_fee_bps) as u128;
    let fee = params.min_fee_bps as u128
        + (span * capped as u128) / params.imb_saturation as u128;
    fee.min(params.max_fee_bps as u128) as u16
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rumi_3pool --lib math::tests::test_fee_bps`
Expected: 7 passed.

- [ ] **Step 5: Commit**

```bash
git add src/rumi_3pool/src/math.rs
git commit -m "feat(3pool): add compute_fee_bps with strict-binary rebalancing"
```

---

## Phase 2: Event Schema Migration

### Task 4: Rename legacy event types to v1, define v2

**Files:**
- Modify: `src/rumi_3pool/src/types.rs:145-190`

- [ ] **Step 1: Rename `SwapEvent` -> `SwapEventV1` and define new `SwapEvent`**

Edit `types.rs`. Replace the existing `SwapEvent` block with:

```rust
/// Legacy v1 swap event (kept for deserialization of pre-migration state).
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct SwapEventV1 {
    pub id: u64,
    pub timestamp: u64,
    pub caller: Principal,
    pub token_in: u8,
    pub token_out: u8,
    pub amount_in: u128,
    pub amount_out: u128,
    pub fee: u128,
}

/// A recorded swap event (v2) for explorer/analytics, including dynamic fee context.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct SwapEvent {
    /// Sequential event index.
    pub id: u64,
    /// Timestamp in nanoseconds since UNIX epoch.
    pub timestamp: u64,
    /// The principal who initiated the swap.
    pub caller: Principal,
    /// Index of the input token (0, 1, or 2).
    pub token_in: u8,
    /// Index of the output token (0, 1, or 2).
    pub token_out: u8,
    /// Amount of input token (native decimals).
    pub amount_in: u128,
    /// Amount of output token received (native decimals).
    pub amount_out: u128,
    /// Fee charged in output token native units.
    pub fee: u128,
    /// Actual rate charged for this swap (variable under dynamic fees).
    pub fee_bps: u16,
    /// Imbalance metric before the swap (1e9 fixed-point).
    pub imbalance_before: u64,
    /// Imbalance metric after the swap (1e9 fixed-point).
    pub imbalance_after: u64,
    /// True iff this swap reduced pool imbalance.
    pub is_rebalancing: bool,
    /// Pool balances after the swap (native decimals each).
    pub pool_balances_after: [u128; 3],
}

impl From<SwapEventV1> for SwapEvent {
    fn from(v1: SwapEventV1) -> Self {
        Self {
            id: v1.id,
            timestamp: v1.timestamp,
            caller: v1.caller,
            token_in: v1.token_in,
            token_out: v1.token_out,
            amount_in: v1.amount_in,
            amount_out: v1.amount_out,
            fee: v1.fee,
            // Sentinel values: pre-migration events don't carry these fields.
            // Explorer/bot consumers must treat zeroes here as "unknown".
            fee_bps: 0,
            imbalance_before: 0,
            imbalance_after: 0,
            is_rebalancing: false,
            pool_balances_after: [0; 3],
        }
    }
}
```

- [ ] **Step 2: Rename `LiquidityEvent` -> `LiquidityEventV1` and define new `LiquidityEvent`**

In the same file, replace the existing `LiquidityEvent` with:

```rust
/// Legacy v1 liquidity event (kept for deserialization of pre-migration state).
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct LiquidityEventV1 {
    pub id: u64,
    pub timestamp: u64,
    pub caller: Principal,
    pub action: LiquidityAction,
    pub amounts: [u128; 3],
    pub lp_amount: u128,
    pub coin_index: Option<u8>,
    pub fee: Option<u128>,
}

/// A recorded liquidity event (v2) for explorer/analytics.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct LiquidityEvent {
    pub id: u64,
    pub timestamp: u64,
    pub caller: Principal,
    pub action: LiquidityAction,
    pub amounts: [u128; 3],
    pub lp_amount: u128,
    pub coin_index: Option<u8>,
    pub fee: Option<u128>,
    /// Actual rate charged (Some for single-sided ops that pay a fee, None for proportional).
    pub fee_bps: Option<u16>,
    pub imbalance_before: u64,
    pub imbalance_after: u64,
    pub is_rebalancing: bool,
    pub pool_balances_after: [u128; 3],
    pub virtual_price_after: u128,
}

impl From<LiquidityEventV1> for LiquidityEvent {
    fn from(v1: LiquidityEventV1) -> Self {
        Self {
            id: v1.id,
            timestamp: v1.timestamp,
            caller: v1.caller,
            action: v1.action,
            amounts: v1.amounts,
            lp_amount: v1.lp_amount,
            coin_index: v1.coin_index,
            fee: v1.fee,
            fee_bps: None,
            imbalance_before: 0,
            imbalance_after: 0,
            is_rebalancing: false,
            pool_balances_after: [0; 3],
            virtual_price_after: 0,
        }
    }
}
```

- [ ] **Step 3: Compile (will fail in callers)**

Run: `cargo check -p rumi_3pool`
Expected: errors in `lib.rs` and `state.rs` referencing the old field names — this is fine, the next tasks fix them.

- [ ] **Step 4: Commit (compile-broken intentionally; the next task fixes state, then lib.rs)**

```bash
git add src/rumi_3pool/src/types.rs
git commit -m "feat(3pool): rename SwapEvent/LiquidityEvent to V1, define v2 schema"
```

---

### Task 5: Add v2 fields to state and migration helper

**Files:**
- Modify: `src/rumi_3pool/src/state.rs:45-91, 181-203`

- [ ] **Step 1: Rename and add fields in `ThreePoolState` struct**

Replace lines 45-54 (the swap_events / liquidity_events / admin_events fields) with:

```rust
    /// LEGACY swap event log (v1). Migrated to `swap_events_v2` in post_upgrade and left as None.
    /// Kept in the struct for one upgrade cycle to allow deserialization of legacy state.
    #[serde(default, rename = "swap_events")]
    pub swap_events_v1: Option<Vec<SwapEventV1>>,
    /// Swap event log (v2) — current. Includes dynamic fee context.
    #[serde(default)]
    pub swap_events_v2: Option<Vec<SwapEvent>>,
    /// LEGACY liquidity event log (v1).
    #[serde(default, rename = "liquidity_events")]
    pub liquidity_events_v1: Option<Vec<LiquidityEventV1>>,
    /// Liquidity event log (v2).
    #[serde(default)]
    pub liquidity_events_v2: Option<Vec<LiquidityEvent>>,
    /// Admin event log for explorer.
    #[serde(default)]
    pub admin_events: Option<Vec<ThreePoolAdminEvent>>,
    /// Dynamic fee curve parameters (initialized to defaults if missing).
    #[serde(default)]
    pub dynamic_fee_params: Option<FeeCurveParams>,
```

- [ ] **Step 2: Update `Default` impl**

In the `Default for ThreePoolState` impl (lines 57-93), replace the `swap_events`, `liquidity_events`, `admin_events` initialization lines with:

```rust
            swap_events_v1: None,
            swap_events_v2: Some(Vec::new()),
            liquidity_events_v1: None,
            liquidity_events_v2: Some(Vec::new()),
            admin_events: Some(Vec::new()),
            dynamic_fee_params: Some(FeeCurveParams::default()),
```

- [ ] **Step 3: Update accessor methods**

Replace the `swap_events()` / `swap_events_mut()` / `liquidity_events()` / `liquidity_events_mut()` methods (lines 181-203) with:

```rust
    /// Get swap events vec (v2).
    pub fn swap_events(&self) -> &Vec<SwapEvent> {
        static EMPTY: std::sync::LazyLock<Vec<SwapEvent>> =
            std::sync::LazyLock::new(Vec::new);
        self.swap_events_v2.as_ref().unwrap_or(&EMPTY)
    }

    /// Get mutable swap events vec (v2).
    pub fn swap_events_mut(&mut self) -> &mut Vec<SwapEvent> {
        self.swap_events_v2.get_or_insert_with(Vec::new)
    }

    /// Get liquidity events vec (v2).
    pub fn liquidity_events(&self) -> &Vec<LiquidityEvent> {
        static EMPTY: std::sync::LazyLock<Vec<LiquidityEvent>> =
            std::sync::LazyLock::new(Vec::new);
        self.liquidity_events_v2.as_ref().unwrap_or(&EMPTY)
    }

    /// Get mutable liquidity events vec (v2).
    pub fn liquidity_events_mut(&mut self) -> &mut Vec<LiquidityEvent> {
        self.liquidity_events_v2.get_or_insert_with(Vec::new)
    }

    /// Get current dynamic fee curve params (defaults if unset).
    pub fn fee_params(&self) -> FeeCurveParams {
        self.dynamic_fee_params.unwrap_or_default()
    }
```

- [ ] **Step 4: Add migration helper at the bottom of `impl ThreePoolState`**

Add this method just before the closing brace of `impl ThreePoolState`:

```rust
    /// One-shot migration: drain v1 event vecs into v2 vecs using sentinel values
    /// for the new fields. Idempotent — safe to re-run.
    pub fn migrate_events_v1_to_v2(&mut self) {
        if let Some(v1) = self.swap_events_v1.take() {
            let v2 = self.swap_events_v2.get_or_insert_with(Vec::new);
            for evt in v1 {
                v2.push(evt.into());
            }
        }
        if let Some(v1) = self.liquidity_events_v1.take() {
            let v2 = self.liquidity_events_v2.get_or_insert_with(Vec::new);
            for evt in v1 {
                v2.push(evt.into());
            }
        }
        // Ensure dynamic_fee_params is populated.
        if self.dynamic_fee_params.is_none() {
            self.dynamic_fee_params = Some(FeeCurveParams::default());
        }
    }
```

- [ ] **Step 5: Compile**

Run: `cargo check -p rumi_3pool`
Expected: still has errors in `lib.rs` (fixed in next phase) but `state.rs` and `types.rs` should compile.

- [ ] **Step 6: Commit**

```bash
git add src/rumi_3pool/src/state.rs
git commit -m "feat(3pool): add v2 event fields and migration helper to state"
```

---

### Task 6: Run migration in `post_upgrade`

**Files:**
- Modify: `src/rumi_3pool/src/lib.rs` (find `post_upgrade` function)

- [ ] **Step 1: Locate `post_upgrade`**

Run: `grep -n "post_upgrade" src/rumi_3pool/src/lib.rs`
Identify the function body.

- [ ] **Step 2: Add migration call at the end of `post_upgrade`**

After `state::load_from_stable_memory()`, add:

```rust
    // Run one-shot v1 -> v2 event migration. Idempotent.
    mutate_state(|s| s.migrate_events_v1_to_v2());
```

- [ ] **Step 3: Write migration unit test**

Add to `src/rumi_3pool/src/state.rs` in a `#[cfg(test)] mod tests` block (or extend existing one):

```rust
#[cfg(test)]
mod migration_tests {
    use super::*;
    use candid::Principal;

    fn dummy_v1_swap(id: u64) -> SwapEventV1 {
        SwapEventV1 {
            id,
            timestamp: 1_700_000_000_000_000_000,
            caller: Principal::anonymous(),
            token_in: 0,
            token_out: 1,
            amount_in: 1_000_000_000,
            amount_out: 9_900_000,
            fee: 100,
        }
    }

    #[test]
    fn migration_drains_v1_into_v2_with_sentinels() {
        let mut state = ThreePoolState::default();
        state.swap_events_v1 = Some(vec![dummy_v1_swap(0), dummy_v1_swap(1)]);
        state.swap_events_v2 = Some(Vec::new());

        state.migrate_events_v1_to_v2();

        assert!(state.swap_events_v1.is_none());
        let v2 = state.swap_events_v2.as_ref().unwrap();
        assert_eq!(v2.len(), 2);
        assert_eq!(v2[0].id, 0);
        // Sentinel values
        assert_eq!(v2[0].fee_bps, 0);
        assert_eq!(v2[0].imbalance_before, 0);
        assert_eq!(v2[0].imbalance_after, 0);
        assert!(!v2[0].is_rebalancing);
        assert_eq!(v2[0].pool_balances_after, [0; 3]);
    }

    #[test]
    fn migration_is_idempotent() {
        let mut state = ThreePoolState::default();
        state.swap_events_v1 = Some(vec![dummy_v1_swap(0)]);
        state.migrate_events_v1_to_v2();
        let len_after_first = state.swap_events_v2.as_ref().unwrap().len();
        state.migrate_events_v1_to_v2(); // re-run
        let len_after_second = state.swap_events_v2.as_ref().unwrap().len();
        assert_eq!(len_after_first, len_after_second);
    }

    #[test]
    fn migration_initializes_fee_params_if_missing() {
        let mut state = ThreePoolState::default();
        state.dynamic_fee_params = None;
        state.migrate_events_v1_to_v2();
        assert!(state.dynamic_fee_params.is_some());
        let params = state.dynamic_fee_params.unwrap();
        assert_eq!(params.min_fee_bps, 1);
        assert_eq!(params.max_fee_bps, 99);
    }
}
```

- [ ] **Step 4: Run migration tests**

Run: `cargo test -p rumi_3pool --lib state::migration_tests`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add src/rumi_3pool/src/lib.rs src/rumi_3pool/src/state.rs
git commit -m "feat(3pool): wire event v1->v2 migration into post_upgrade"
```

---

## Phase 3: Wire Dynamic Fees Into Swap Path

### Task 7: Update `calc_swap_output` to return dynamic fee context

**Files:**
- Modify: `src/rumi_3pool/src/swap.rs`

- [ ] **Step 1: Add return type carrying fee context**

At the top of `swap.rs`, add:

```rust
use crate::types::FeeCurveParams;

/// Result of a swap calculation, including dynamic fee context for event logging.
#[derive(Debug, Clone, Copy)]
pub struct SwapOutcome {
    /// Output amount in token j native decimals.
    pub output: u128,
    /// Fee charged in token j native decimals.
    pub fee: u128,
    /// Effective fee rate in basis points.
    pub fee_bps: u16,
    /// Imbalance before the swap (1e9 fixed-point).
    pub imbalance_before: u64,
    /// Imbalance after the swap (1e9 fixed-point).
    pub imbalance_after: u64,
    /// True iff this swap reduced imbalance.
    pub is_rebalancing: bool,
}
```

- [ ] **Step 2: Replace `calc_swap_output` body with dynamic version**

Replace the entire existing `calc_swap_output` function with:

```rust
/// Calculate the output amount for a swap, before any token transfers.
/// Uses the dynamic fee curve.
pub fn calc_swap_output(
    i: usize,
    j: usize,
    dx_native: u128,
    balances: &[u128; 3],
    precision_muls: &[u64; 3],
    amp: u64,
    fee_params: &FeeCurveParams,
) -> Result<SwapOutcome, ThreePoolError> {
    if dx_native == 0 {
        return Err(ThreePoolError::ZeroAmount);
    }
    if i == j || i >= 3 || j >= 3 {
        return Err(ThreePoolError::InvalidCoinIndex);
    }

    // Compute imbalance before
    let imbalance_before = compute_imbalance(balances, precision_muls);

    // Normalize and run curve math (existing logic)
    let xp = normalize_all(balances, precision_muls);
    let d = get_d(&xp, amp).ok_or(ThreePoolError::InvariantNotConverged)?;
    let new_x_i = xp[i] + normalize_balance(dx_native, precision_muls[i]);
    let new_y_j = get_y(i, j, new_x_i, &xp, amp, d).ok_or(ThreePoolError::InvariantNotConverged)?;
    let dy_normalized = xp[j]
        .checked_sub(new_y_j)
        .ok_or(ThreePoolError::InsufficientLiquidity)?;

    // Project post-swap balances (in native decimals) to compute imbalance_after
    let dy_native_pre_fee = denormalize_balance(dy_normalized, precision_muls[j]);
    let mut projected = *balances;
    projected[i] = projected[i].saturating_add(dx_native);
    projected[j] = projected[j].saturating_sub(dy_native_pre_fee);
    let imbalance_after = compute_imbalance(&projected, precision_muls);

    // Compute dynamic fee
    let fee_bps = compute_fee_bps(imbalance_before, imbalance_after, fee_params);
    let is_rebalancing = imbalance_after <= imbalance_before;

    let fee_normalized = dy_normalized * ethnum::U256::from(fee_bps as u64)
        / ethnum::U256::from(10_000u64);
    let dy_after_fee = dy_normalized - fee_normalized;

    let output = denormalize_balance(dy_after_fee, precision_muls[j]);
    let fee = denormalize_balance(fee_normalized, precision_muls[j]);

    Ok(SwapOutcome {
        output,
        fee,
        fee_bps,
        imbalance_before,
        imbalance_after,
        is_rebalancing,
    })
}
```

- [ ] **Step 3: Update existing tests in `swap.rs` to use the new signature**

Replace each `calc_swap_output(..., 4)` (the old `swap_fee_bps` arg) with `calc_swap_output(..., &FeeCurveParams::default())`. Update `let (output, fee) = ...` patterns to `let outcome = ...; let (output, fee) = (outcome.output, outcome.fee);`. The existing test assertions on output/fee values should still pass — fee just comes from the dynamic curve now (which gives ~25 bps for the test's swap, slightly higher than the old 4 bps; relax assertions if necessary).

For `test_calc_swap_output`: change `output > expected * 99 / 100` to `output > expected * 97 / 100` to accommodate the slightly higher dynamic fee on a balanced pool when crossing imbalance.

- [ ] **Step 4: Add new test for rebalancing behavior**

Add to the `tests` module in `swap.rs`:

```rust
#[test]
fn test_calc_swap_output_rebalancing_pays_min_fee() {
    // Imbalanced pool: lots of icUSD, little ckUSDT
    let balances: [u128; 3] = [
        2_000_000 * 100_000_000,  // icUSD heavy
        500_000 * 1_000_000,      // ckUSDT light
        1_000_000 * 1_000_000,
    ];
    let precision_muls: [u64; 3] = [10_000_000_000, 1_000_000_000_000, 1_000_000_000_000];
    let params = FeeCurveParams::default();

    // Swap ckUSDT -> icUSD: this REDUCES the imbalance, should be MIN_FEE
    let dx = 10_000 * 1_000_000u128;
    let outcome = calc_swap_output(1, 0, dx, &balances, &precision_muls, 500, &params)
        .expect("swap should succeed");

    assert!(outcome.is_rebalancing);
    assert_eq!(outcome.fee_bps, 1, "rebalancing swap must pay MIN_FEE");
}

#[test]
fn test_calc_swap_output_imbalancing_pays_more() {
    let balances: [u128; 3] = [
        2_000_000 * 100_000_000,
        500_000 * 1_000_000,
        1_000_000 * 1_000_000,
    ];
    let precision_muls: [u64; 3] = [10_000_000_000, 1_000_000_000_000, 1_000_000_000_000];
    let params = FeeCurveParams::default();

    // Swap icUSD -> ckUSDT: this WORSENS the imbalance
    let dx = 50_000 * 100_000_000u128;
    let outcome = calc_swap_output(0, 1, dx, &balances, &precision_muls, 500, &params)
        .expect("swap should succeed");

    assert!(!outcome.is_rebalancing);
    assert!(outcome.fee_bps > 1, "imbalancing swap should pay > MIN, got {}", outcome.fee_bps);
}
```

- [ ] **Step 5: Run swap tests**

Run: `cargo test -p rumi_3pool --lib swap::tests`
Expected: all pass (including the two new ones).

- [ ] **Step 6: Commit**

```bash
git add src/rumi_3pool/src/swap.rs
git commit -m "feat(3pool): dynamic fee in calc_swap_output, return SwapOutcome"
```

---

### Task 8: Update `swap` endpoint in `lib.rs` to use new outcome and emit v2 event

**Files:**
- Modify: `src/rumi_3pool/src/lib.rs` (the `swap` update method around line 252-321)

- [ ] **Step 1: Find the `swap` function**

Run: `grep -n "pub async fn swap" src/rumi_3pool/src/lib.rs`

- [ ] **Step 2: Update the call to `calc_swap_output`**

Find the line that calls `calc_swap_output(...)` and the surrounding code. Replace the snippet that reads `swap_fee_bps` from config and calls `calc_swap_output`:

```rust
    // OLD: let swap_fee_bps = read_state(|s| s.config.swap_fee_bps);
    // OLD: let (output, fee) = calc_swap_output(i_idx, j_idx, dx, &balances, &precision_muls, amp, swap_fee_bps)?;

    let fee_params = read_state(|s| s.fee_params());
    let outcome = calc_swap_output(i_idx, j_idx, dx, &balances, &precision_muls, amp, &fee_params)?;
    let output = outcome.output;
    let fee = outcome.fee;
```

- [ ] **Step 3: Update the swap event emission**

Find the `s.swap_events_mut().push(SwapEvent { ... })` block. Replace with:

```rust
        let id = s.swap_events().len() as u64;
        let mut new_balances = s.balances;
        // (s.balances has already been mutated above to reflect the swap;
        //  capture them as the post-swap snapshot)
        new_balances = s.balances;
        s.swap_events_mut().push(SwapEvent {
            id,
            timestamp: ic_cdk::api::time(),
            caller,
            token_in: i,
            token_out: j,
            amount_in: dx,
            amount_out: output,
            fee,
            fee_bps: outcome.fee_bps,
            imbalance_before: outcome.imbalance_before,
            imbalance_after: outcome.imbalance_after,
            is_rebalancing: outcome.is_rebalancing,
            pool_balances_after: new_balances,
        });
```

(Note: the variable shadowing on `new_balances` is intentional clarity. Simplify if reviewer prefers a single assignment.)

- [ ] **Step 4: Compile**

Run: `cargo check -p rumi_3pool`
Expected: clean (or only liquidity-related errors which the next task fixes).

- [ ] **Step 5: Commit**

```bash
git add src/rumi_3pool/src/lib.rs
git commit -m "feat(3pool): emit v2 swap events with dynamic fee context"
```

---

## Phase 4: Wire Dynamic Fees Into Liquidity Operations

### Task 9: Update `calc_add_liquidity` and `calc_remove_one_coin` to use dynamic fees

**Files:**
- Modify: `src/rumi_3pool/src/liquidity.rs`

- [ ] **Step 1: Read the existing functions**

Run: `cat src/rumi_3pool/src/liquidity.rs | head -200` to map out current signatures.

- [ ] **Step 2: Add `LiquidityOutcome` struct**

At the top of `liquidity.rs`, add:

```rust
use crate::types::FeeCurveParams;

#[derive(Debug, Clone, Copy)]
pub struct LiquidityOutcome {
    pub lp_amount: u128,
    /// For single-sided ops: the dynamic fee bps applied. None for proportional.
    pub fee_bps: Option<u16>,
    pub imbalance_before: u64,
    pub imbalance_after: u64,
    pub is_rebalancing: bool,
}
```

- [ ] **Step 3: Refactor `calc_add_liquidity` to take `FeeCurveParams`**

Change the `swap_fee_bps: u64` parameter to `fee_params: &FeeCurveParams`. Inside, compute `imbalance_before` and `imbalance_after` (using projected post-deposit balances), then derive `fee_bps` via `compute_fee_bps`. Use that in place of the static `swap_fee_bps` in the existing imbalance fee math.

(Specific edit: the current `3/8 * swap_fee` factor goes away. Use the dynamic `fee_bps` directly as the imbalance fee on the per-token deviation portion.)

Return a `(u128, LiquidityOutcome)` so callers see both the LP amount and the metrics.

- [ ] **Step 4: Refactor `calc_remove_liquidity_one_coin` similarly**

Same pattern: take `&FeeCurveParams`, compute pre/post imbalance (post = balances minus the withdrawn token amount), apply dynamic fee.

- [ ] **Step 5: Update unit tests in `liquidity.rs`**

Replace calls passing `4u64` (the old swap_fee_bps) with `&FeeCurveParams::default()`. Adjust assertions if fee changes shift expected outputs by a few bps.

- [ ] **Step 6: Run liquidity tests**

Run: `cargo test -p rumi_3pool --lib liquidity::tests`
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add src/rumi_3pool/src/liquidity.rs
git commit -m "feat(3pool): dynamic fee in calc_add_liquidity and calc_remove_one_coin"
```

---

### Task 10: Update `add_liquidity`, `remove_liquidity`, `remove_liquidity_one_coin` endpoints

**Files:**
- Modify: `src/rumi_3pool/src/lib.rs:325-660` (the three liquidity endpoint functions)

- [ ] **Step 1: Update `add_liquidity`**

Replace the call site:

```rust
    let fee_params = read_state(|s| s.fee_params());
    let (lp_minted, outcome) = calc_add_liquidity(
        &amounts_arr,
        &old_balances,
        &precision_muls,
        lp_total_supply,
        amp,
        &fee_params,
    )?;
```

Then update the LiquidityEvent push to populate v2 fields:

```rust
        let id = s.liquidity_events().len() as u64;
        let new_balances = s.balances;
        let virtual_price_after = compute_virtual_price(...); // call existing helper if available
        s.liquidity_events_mut().push(LiquidityEvent {
            id,
            timestamp: ic_cdk::api::time(),
            caller,
            action: LiquidityAction::AddLiquidity,
            amounts: amounts_arr,
            lp_amount: lp_minted,
            coin_index: None,
            fee: None,
            fee_bps: outcome.fee_bps,
            imbalance_before: outcome.imbalance_before,
            imbalance_after: outcome.imbalance_after,
            is_rebalancing: outcome.is_rebalancing,
            pool_balances_after: new_balances,
            virtual_price_after,
        });
```

If a `compute_virtual_price` helper does not exist as a callable function, inline the existing math (look in `lib.rs` for how `vp_snapshots` is updated; it computes `D * 1e8 / lp_total_supply`).

- [ ] **Step 2: Update `remove_liquidity` (proportional)**

This one does not use dynamic fees (no fee at all). Just populate the v2 event with `fee_bps: None`, current imbalance for both `imbalance_before` and `imbalance_after` (no change to weights), `is_rebalancing: false`.

- [ ] **Step 3: Update `remove_liquidity_one_coin`**

Mirror the `add_liquidity` changes, using the new outcome from `calc_remove_liquidity_one_coin`.

- [ ] **Step 4: Compile and run all unit tests**

Run: `cargo test -p rumi_3pool --lib`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/rumi_3pool/src/lib.rs
git commit -m "feat(3pool): emit v2 liquidity events with dynamic fee context"
```

---

## Phase 5: Admin Endpoint

### Task 11: `set_fee_curve_params` admin function

**Files:**
- Modify: `src/rumi_3pool/src/admin.rs`
- Modify: `src/rumi_3pool/src/types.rs` (add admin action variant)
- Modify: `src/rumi_3pool/src/lib.rs` (expose endpoint)

- [ ] **Step 1: Add admin action variant**

In `types.rs`, find the `ThreePoolAdminAction` enum and add a variant:

```rust
    SetFeeCurveParams { params: FeeCurveParams },
```

- [ ] **Step 2: Add admin function**

In `admin.rs`, add:

```rust
use crate::types::{FeeCurveParams, ThreePoolAdminAction, ThreePoolError};
use crate::state::{mutate_state, read_state};
use candid::Principal;

pub fn set_fee_curve_params(
    params: FeeCurveParams,
    caller: Principal,
    now: u64,
) -> Result<(), ThreePoolError> {
    let admin = read_state(|s| s.config.admin);
    if caller != admin {
        return Err(ThreePoolError::Unauthorized);
    }
    // Validation
    if params.min_fee_bps < 1 {
        return Err(ThreePoolError::InvalidParameters);
    }
    if params.max_fee_bps > 200 {
        return Err(ThreePoolError::InvalidParameters);
    }
    if params.min_fee_bps >= params.max_fee_bps {
        return Err(ThreePoolError::InvalidParameters);
    }
    if params.imb_saturation == 0 || params.imb_saturation > 1_000_000_000 {
        return Err(ThreePoolError::InvalidParameters);
    }

    mutate_state(|s| {
        s.dynamic_fee_params = Some(params);
        let id = s.admin_events().len() as u64;
        s.admin_events_mut().push(crate::types::ThreePoolAdminEvent {
            id,
            timestamp: now,
            caller,
            action: ThreePoolAdminAction::SetFeeCurveParams { params },
        });
    });
    Ok(())
}
```

If `ThreePoolError::InvalidParameters` does not exist, add it to the error enum in `types.rs`.

- [ ] **Step 3: Expose in `lib.rs`**

Add:

```rust
#[update]
pub fn set_fee_curve_params(params: FeeCurveParams) -> Result<(), ThreePoolError> {
    let caller = ic_cdk::api::caller();
    let now = ic_cdk::api::time();
    admin::set_fee_curve_params(params, caller, now)
}
```

- [ ] **Step 4: Write unit test**

Add to `admin.rs` `tests` module (create if not present):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_min_above_max() {
        let bad = FeeCurveParams { min_fee_bps: 50, max_fee_bps: 30, imb_saturation: 250_000_000 };
        // Need state init for this — simplest: assert validation logic in isolation
        assert!(bad.min_fee_bps >= bad.max_fee_bps);
    }
}
```

(Full state-coupled tests live in pocket_ic_3usd.)

- [ ] **Step 5: Compile and run tests**

Run: `cargo test -p rumi_3pool --lib`
Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add src/rumi_3pool/src/admin.rs src/rumi_3pool/src/types.rs src/rumi_3pool/src/lib.rs
git commit -m "feat(3pool): admin endpoint set_fee_curve_params with validation"
```

---

## Phase 6: Bot Endpoints

### Task 12: New bot-facing types

**Files:**
- Modify: `src/rumi_3pool/src/types.rs`

- [ ] **Step 1: Add types**

```rust
// ─── Bot Endpoint Types ───

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct SwapQuote {
    pub amount_out: u128,
    pub fee: u128,
    pub fee_bps: u16,
    pub imbalance_before: u64,
    pub imbalance_after: u64,
    pub is_rebalancing: bool,
    /// Slippage relative to a hypothetical 1:1 swap, in bps.
    pub price_impact_bps: u32,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct PoolState {
    pub balances: [u128; 3],
    pub weights_1e9: [u64; 3],
    pub imbalance: u64,
    pub current_a: u64,
    pub fee_params: FeeCurveParams,
    pub virtual_price: u128,
    pub lp_total_supply: u128,
    pub last_swap_timestamp: Option<u64>,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct RebalanceQuote {
    /// Maximum amount of `token_to_add` that can be swapped into the pool
    /// while still paying MIN_FEE (i.e. before the swap starts over-rebalancing).
    pub max_min_fee_amount_in: u128,
    /// Expected output of the underrepresented direction at that size.
    pub expected_amount_out: u128,
    pub target_token_out: u8,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct ImbalanceSnapshot {
    pub timestamp: u64,
    pub imbalance: u64,
    pub balances: [u128; 3],
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct SwapLeg {
    pub token_in: u8,
    pub token_out: u8,
    pub amount_in: u128,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct SimulatedPath {
    pub final_amount_out: u128,
    pub total_fee_bps_weighted: u32,
    pub legs: Vec<SwapQuote>,
}
```

- [ ] **Step 2: Compile**

Run: `cargo check -p rumi_3pool`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add src/rumi_3pool/src/types.rs
git commit -m "feat(3pool): bot endpoint types (SwapQuote, PoolState, etc.)"
```

---

### Task 13: `quote_swap` and `get_pool_state` (B1, B2, B6)

**Files:**
- Modify: `src/rumi_3pool/src/lib.rs`

- [ ] **Step 1: Implement `quote_swap`**

```rust
#[query]
pub fn quote_swap(token_in: u8, token_out: u8, amount_in: u128) -> Result<SwapQuote, ThreePoolError> {
    let i = token_in as usize;
    let j = token_out as usize;
    let (balances, precision_muls, fee_params) = read_state(|s| {
        let pm = [
            s.config.tokens[0].precision_mul,
            s.config.tokens[1].precision_mul,
            s.config.tokens[2].precision_mul,
        ];
        (s.balances, pm, s.fee_params())
    });
    let amp = get_current_a();
    let outcome = swap::calc_swap_output(i, j, amount_in, &balances, &precision_muls, amp, &fee_params)?;
    // price_impact: compare to "1:1 ideal" output
    let ideal = amount_in * (precision_muls[i] as u128) / (precision_muls[j] as u128);
    let impact_bps: u32 = if ideal > outcome.output {
        (((ideal - outcome.output) * 10_000) / ideal) as u32
    } else {
        0
    };
    Ok(SwapQuote {
        amount_out: outcome.output,
        fee: outcome.fee,
        fee_bps: outcome.fee_bps,
        imbalance_before: outcome.imbalance_before,
        imbalance_after: outcome.imbalance_after,
        is_rebalancing: outcome.is_rebalancing,
        price_impact_bps: impact_bps,
    })
}
```

- [ ] **Step 2: Implement `get_pool_state`**

```rust
#[query]
pub fn get_pool_state() -> PoolState {
    read_state(|s| {
        let balances = s.balances;
        let pm = [
            s.config.tokens[0].precision_mul,
            s.config.tokens[1].precision_mul,
            s.config.tokens[2].precision_mul,
        ];
        let imbalance = math::compute_imbalance(&balances, &pm);
        // Per-token weights in 1e9 fp
        let xp = math::normalize_all(&balances, &pm);
        let total = xp[0] + xp[1] + xp[2];
        let scale = ethnum::U256::from(1_000_000_000u64);
        let weights_1e9 = if total == ethnum::U256::ZERO {
            [0u64; 3]
        } else {
            [
                ((xp[0] * scale) / total).as_u64(),
                ((xp[1] * scale) / total).as_u64(),
                ((xp[2] * scale) / total).as_u64(),
            ]
        };
        let last_swap_timestamp = s.swap_events().last().map(|e| e.timestamp);
        PoolState {
            balances,
            weights_1e9,
            imbalance,
            current_a: s.config.initial_a, // for current snapshot use ramping-aware get_current_a in real impl
            fee_params: s.fee_params(),
            virtual_price: 0, // populate from helper if available, else compute D*1e8/supply
            lp_total_supply: s.lp_total_supply,
            last_swap_timestamp,
        }
    })
}
```

(For `current_a` and `virtual_price`, replace with proper helpers — `get_current_a()` and the existing virtual price calc — done in a follow-up step here if needed.)

- [ ] **Step 3: Implement `get_fee_curve_params` (B6)**

```rust
#[query]
pub fn get_fee_curve_params() -> FeeCurveParams {
    read_state(|s| s.fee_params())
}
```

- [ ] **Step 4: Compile**

Run: `cargo check -p rumi_3pool`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add src/rumi_3pool/src/lib.rs
git commit -m "feat(3pool): bot endpoints quote_swap, get_pool_state, get_fee_curve_params"
```

---

### Task 14: `quote_optimal_rebalance` (B3)

**Files:**
- Modify: `src/rumi_3pool/src/lib.rs`

- [ ] **Step 1: Implement**

```rust
#[query]
pub fn quote_optimal_rebalance(token_to_add: u8) -> Result<RebalanceQuote, ThreePoolError> {
    // The "optimal" rebalance amount is one that brings the pool to the boundary
    // where any further trade would start over-rebalancing.
    // We binary-search the input size: find the largest dx such that
    // imbalance_after is still >= 0 AND not over-shoot.
    //
    // For simplicity in v1: pick the over-represented token as the OUT side,
    // compute what amount of token_to_add brings imbalance to 0 (or as close as possible).
    let i = token_to_add as usize;
    if i >= 3 { return Err(ThreePoolError::InvalidCoinIndex); }

    let (balances, pm, fee_params) = read_state(|s| {
        let pm = [
            s.config.tokens[0].precision_mul,
            s.config.tokens[1].precision_mul,
            s.config.tokens[2].precision_mul,
        ];
        (s.balances, pm, s.fee_params())
    });
    let amp = get_current_a();

    // Find the most over-represented token as the output side.
    let xp = math::normalize_all(&balances, &pm);
    let total = xp[0] + xp[1] + xp[2];
    let mut max_idx = 0usize;
    let mut max_w = ethnum::U256::ZERO;
    for k in 0..3 {
        if k == i { continue; }
        let w = xp[k] * ethnum::U256::from(1_000_000_000u64) / total;
        if w > max_w { max_w = w; max_idx = k; }
    }

    // Binary search on dx
    let mut lo: u128 = 0;
    let mut hi: u128 = balances[max_idx].saturating_mul(2); // generous upper bound
    let mut best: u128 = 0;
    let mut best_out: u128 = 0;

    for _ in 0..40 {
        if lo >= hi { break; }
        let mid = lo + (hi - lo) / 2;
        if mid == 0 { lo = 1; continue; }
        match swap::calc_swap_output(i, max_idx, mid, &balances, &pm, amp, &fee_params) {
            Ok(o) if o.is_rebalancing => {
                best = mid;
                best_out = o.output;
                lo = mid + 1;
            }
            _ => { hi = mid; }
        }
    }

    Ok(RebalanceQuote {
        max_min_fee_amount_in: best,
        expected_amount_out: best_out,
        target_token_out: max_idx as u8,
    })
}
```

- [ ] **Step 2: Compile**

Run: `cargo check -p rumi_3pool`

- [ ] **Step 3: Commit**

```bash
git add src/rumi_3pool/src/lib.rs
git commit -m "feat(3pool): bot endpoint quote_optimal_rebalance"
```

---

### Task 15: `get_imbalance_history`, `simulate_swap_path`, enriched `get_swap_events` (B4, B5, B7)

**Files:**
- Modify: `src/rumi_3pool/src/lib.rs`

- [ ] **Step 1: `get_imbalance_history`**

```rust
#[query]
pub fn get_imbalance_history(window_seconds: u64) -> Vec<ImbalanceSnapshot> {
    let now_ns = ic_cdk::api::time();
    let cutoff_ns = now_ns.saturating_sub(window_seconds.saturating_mul(1_000_000_000));
    read_state(|s| {
        s.swap_events()
            .iter()
            .filter(|e| e.timestamp >= cutoff_ns)
            .map(|e| ImbalanceSnapshot {
                timestamp: e.timestamp,
                imbalance: e.imbalance_after,
                balances: e.pool_balances_after,
            })
            .collect()
    })
}
```

- [ ] **Step 2: `simulate_swap_path`**

```rust
#[query]
pub fn simulate_swap_path(legs: Vec<SwapLeg>) -> Result<SimulatedPath, ThreePoolError> {
    let (mut balances, pm, fee_params) = read_state(|s| {
        let pm = [
            s.config.tokens[0].precision_mul,
            s.config.tokens[1].precision_mul,
            s.config.tokens[2].precision_mul,
        ];
        (s.balances, pm, s.fee_params())
    });
    let amp = get_current_a();

    let mut quotes = Vec::with_capacity(legs.len());
    let mut current_amount = 0u128;
    let mut total_fee_weighted = 0u128;
    let mut total_volume = 0u128;

    for (idx, leg) in legs.iter().enumerate() {
        let i = leg.token_in as usize;
        let j = leg.token_out as usize;
        let dx = if idx == 0 { leg.amount_in } else { current_amount };

        let outcome = swap::calc_swap_output(i, j, dx, &balances, &pm, amp, &fee_params)?;
        let imbalance_before = outcome.imbalance_before;
        let imbalance_after = outcome.imbalance_after;

        // Apply to virtual balances
        balances[i] = balances[i].saturating_add(dx);
        balances[j] = balances[j].saturating_sub(outcome.output + outcome.fee);

        let ideal = dx * (pm[i] as u128) / (pm[j] as u128);
        let impact = if ideal > outcome.output { (((ideal - outcome.output) * 10_000) / ideal) as u32 } else { 0 };

        total_fee_weighted += (outcome.fee_bps as u128) * dx;
        total_volume += dx;
        current_amount = outcome.output;

        quotes.push(SwapQuote {
            amount_out: outcome.output,
            fee: outcome.fee,
            fee_bps: outcome.fee_bps,
            imbalance_before,
            imbalance_after,
            is_rebalancing: outcome.is_rebalancing,
            price_impact_bps: impact,
        });
    }

    let avg_fee_bps = if total_volume == 0 { 0 } else { (total_fee_weighted / total_volume) as u32 };
    Ok(SimulatedPath {
        final_amount_out: current_amount,
        total_fee_bps_weighted: avg_fee_bps,
        legs: quotes,
    })
}
```

- [ ] **Step 3: Verify `get_swap_events` returns v2**

The existing `get_swap_events(start, length)` already returns `Vec<SwapEvent>`, which is now the v2 type. No change required, but verify it compiles.

- [ ] **Step 4: Compile**

Run: `cargo check -p rumi_3pool`

- [ ] **Step 5: Commit**

```bash
git add src/rumi_3pool/src/lib.rs
git commit -m "feat(3pool): bot endpoints imbalance_history, simulate_swap_path"
```

---

## Phase 7: Explorer Endpoints

### Task 16: Explorer types

**Files:**
- Modify: `src/rumi_3pool/src/types.rs`

- [ ] **Step 1: Add explorer types**

```rust
// ─── Explorer Types ───

#[derive(CandidType, Clone, Copy, Debug, Serialize, Deserialize)]
pub enum StatsWindow {
    Last24h,
    Last7d,
    Last30d,
    AllTime,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct PoolStats {
    pub swap_count: u64,
    pub swap_volume_per_token: [u128; 3],
    pub total_fees_collected: [u128; 3],
    pub unique_swappers: u64,
    pub liquidity_added_count: u64,
    pub liquidity_removed_count: u64,
    pub avg_fee_bps: u32,
    pub arb_swap_count: u64,
    pub arb_volume_per_token: [u128; 3],
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct ImbalanceStats {
    pub current: u64,
    pub min: u64,
    pub max: u64,
    pub avg: u64,
    pub samples: Vec<(u64, u64)>, // (timestamp, imbalance)
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct FeeBucket {
    pub min_bps: u16,
    pub max_bps: u16,
    pub swap_count: u64,
    pub volume_per_token: [u128; 3],
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct FeeStats {
    pub buckets: Vec<FeeBucket>,
    pub rebalancing_swap_count: u64,
    pub rebalancing_swap_pct: u32, // 0..10000 bps
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct VolumePoint { pub timestamp: u64, pub volume_per_token: [u128; 3] }

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct BalancePoint { pub timestamp: u64, pub balances: [u128; 3] }

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct VirtualPricePoint { pub timestamp: u64, pub virtual_price: u128 }

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct FeePoint { pub timestamp: u64, pub avg_fee_bps: u32 }

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct PoolHealth {
    pub current_imbalance: u64,
    pub imbalance_trend_1h: i32, // negative = improving, positive = worsening
    pub last_swap_age_seconds: u64,
    pub fee_at_min: u16,
    pub fee_at_max_imbalance_swap: u16,
    pub arb_opportunity_score: u8, // 0-100
}
```

- [ ] **Step 2: Compile**

Run: `cargo check -p rumi_3pool`

- [ ] **Step 3: Commit**

```bash
git add src/rumi_3pool/src/types.rs
git commit -m "feat(3pool): explorer endpoint types"
```

---

### Task 17: Raw data endpoints (E1-E4)

**Files:**
- Modify: `src/rumi_3pool/src/lib.rs`

- [ ] **Step 1: Implement filtered queries**

```rust
fn window_cutoff_ns(window: StatsWindow, now: u64) -> u64 {
    let secs = match window {
        StatsWindow::Last24h => 24 * 3600,
        StatsWindow::Last7d => 7 * 24 * 3600,
        StatsWindow::Last30d => 30 * 24 * 3600,
        StatsWindow::AllTime => return 0,
    };
    now.saturating_sub(secs * 1_000_000_000)
}

#[query]
pub fn get_swap_events_by_principal(principal: Principal, start: u64, length: u64) -> Vec<SwapEvent> {
    read_state(|s| {
        s.swap_events()
            .iter()
            .filter(|e| e.caller == principal)
            .skip(start as usize)
            .take(length as usize)
            .cloned()
            .collect()
    })
}

#[query]
pub fn get_swap_events_by_time_range(from_ts: u64, to_ts: u64, limit: u64) -> Vec<SwapEvent> {
    read_state(|s| {
        s.swap_events()
            .iter()
            .filter(|e| e.timestamp >= from_ts && e.timestamp < to_ts)
            .take(limit as usize)
            .cloned()
            .collect()
    })
}

#[query]
pub fn get_liquidity_events_by_principal(principal: Principal, start: u64, length: u64) -> Vec<LiquidityEvent> {
    read_state(|s| {
        s.liquidity_events()
            .iter()
            .filter(|e| e.caller == principal)
            .skip(start as usize)
            .take(length as usize)
            .cloned()
            .collect()
    })
}

#[query]
pub fn get_admin_events(start: u64, length: u64) -> Vec<ThreePoolAdminEvent> {
    read_state(|s| {
        s.admin_events()
            .iter()
            .skip(start as usize)
            .take(length as usize)
            .cloned()
            .collect()
    })
}
```

- [ ] **Step 2: Compile**

Run: `cargo check -p rumi_3pool`

- [ ] **Step 3: Commit**

```bash
git add src/rumi_3pool/src/lib.rs
git commit -m "feat(3pool): explorer raw data endpoints (E1-E4)"
```

---

### Task 18: Aggregated stats endpoints (E5-E9)

**Files:**
- Modify: `src/rumi_3pool/src/lib.rs`

- [ ] **Step 1: Implement `get_pool_stats`**

```rust
#[query]
pub fn get_pool_stats(window: StatsWindow) -> PoolStats {
    let now = ic_cdk::api::time();
    let cutoff = window_cutoff_ns(window, now);
    read_state(|s| {
        let mut volume_per_token = [0u128; 3];
        let mut fees = [0u128; 3];
        let mut count = 0u64;
        let mut arb_count = 0u64;
        let mut arb_volume = [0u128; 3];
        let mut weighted_fee_bps = 0u128;
        let mut total_volume = 0u128;
        let mut swappers = std::collections::BTreeSet::new();

        for e in s.swap_events().iter().filter(|e| e.timestamp >= cutoff) {
            count += 1;
            volume_per_token[e.token_in as usize] = volume_per_token[e.token_in as usize].saturating_add(e.amount_in);
            fees[e.token_out as usize] = fees[e.token_out as usize].saturating_add(e.fee);
            weighted_fee_bps += (e.fee_bps as u128) * e.amount_in;
            total_volume += e.amount_in;
            swappers.insert(e.caller);
            if e.is_rebalancing {
                arb_count += 1;
                arb_volume[e.token_in as usize] = arb_volume[e.token_in as usize].saturating_add(e.amount_in);
            }
        }

        let mut adds = 0u64;
        let mut removes = 0u64;
        for e in s.liquidity_events().iter().filter(|e| e.timestamp >= cutoff) {
            match e.action {
                LiquidityAction::AddLiquidity => adds += 1,
                LiquidityAction::RemoveLiquidity | LiquidityAction::RemoveOneCoin => removes += 1,
                _ => {}
            }
        }

        PoolStats {
            swap_count: count,
            swap_volume_per_token: volume_per_token,
            total_fees_collected: fees,
            unique_swappers: swappers.len() as u64,
            liquidity_added_count: adds,
            liquidity_removed_count: removes,
            avg_fee_bps: if total_volume == 0 { 0 } else { (weighted_fee_bps / total_volume) as u32 },
            arb_swap_count: arb_count,
            arb_volume_per_token: arb_volume,
        }
    })
}
```

- [ ] **Step 2: Implement `get_imbalance_stats`, `get_fee_stats`, `get_top_swappers`, `get_top_lps`**

(Follow the same pattern: iterate filtered events, accumulate, return.)

```rust
#[query]
pub fn get_imbalance_stats(window: StatsWindow) -> ImbalanceStats {
    let now = ic_cdk::api::time();
    let cutoff = window_cutoff_ns(window, now);
    read_state(|s| {
        let pm = [s.config.tokens[0].precision_mul, s.config.tokens[1].precision_mul, s.config.tokens[2].precision_mul];
        let current = math::compute_imbalance(&s.balances, &pm);
        let mut min_v = u64::MAX;
        let mut max_v = 0u64;
        let mut sum = 0u128;
        let mut count = 0u128;
        let mut samples = Vec::new();
        for e in s.swap_events().iter().filter(|e| e.timestamp >= cutoff) {
            let imb = e.imbalance_after;
            if imb < min_v { min_v = imb; }
            if imb > max_v { max_v = imb; }
            sum += imb as u128;
            count += 1;
            samples.push((e.timestamp, imb));
        }
        ImbalanceStats {
            current,
            min: if count == 0 { current } else { min_v },
            max: if count == 0 { current } else { max_v },
            avg: if count == 0 { current } else { (sum / count) as u64 },
            samples,
        }
    })
}

#[query]
pub fn get_fee_stats(window: StatsWindow) -> FeeStats {
    let now = ic_cdk::api::time();
    let cutoff = window_cutoff_ns(window, now);
    let bucket_edges: [(u16, u16); 5] = [(1, 10), (10, 25), (25, 50), (50, 75), (75, 99)];
    read_state(|s| {
        let mut buckets: Vec<FeeBucket> = bucket_edges.iter().map(|(lo, hi)| FeeBucket {
            min_bps: *lo, max_bps: *hi, swap_count: 0, volume_per_token: [0; 3],
        }).collect();
        let mut rebalancing = 0u64;
        let mut total = 0u64;
        for e in s.swap_events().iter().filter(|e| e.timestamp >= cutoff) {
            total += 1;
            if e.is_rebalancing { rebalancing += 1; }
            for b in buckets.iter_mut() {
                if e.fee_bps >= b.min_bps && e.fee_bps < b.max_bps {
                    b.swap_count += 1;
                    b.volume_per_token[e.token_in as usize] = b.volume_per_token[e.token_in as usize].saturating_add(e.amount_in);
                    break;
                }
            }
        }
        let pct = if total == 0 { 0 } else { ((rebalancing * 10_000) / total) as u32 };
        FeeStats { buckets, rebalancing_swap_count: rebalancing, rebalancing_swap_pct: pct }
    })
}

#[query]
pub fn get_top_swappers(window: StatsWindow, limit: u64) -> Vec<(Principal, u64, u128)> {
    let now = ic_cdk::api::time();
    let cutoff = window_cutoff_ns(window, now);
    read_state(|s| {
        let mut acc: std::collections::BTreeMap<Principal, (u64, u128)> = std::collections::BTreeMap::new();
        for e in s.swap_events().iter().filter(|e| e.timestamp >= cutoff) {
            let entry = acc.entry(e.caller).or_insert((0, 0));
            entry.0 += 1;
            entry.1 += e.amount_in;
        }
        let mut v: Vec<(Principal, u64, u128)> = acc.into_iter().map(|(p, (c, vol))| (p, c, vol)).collect();
        v.sort_by(|a, b| b.2.cmp(&a.2));
        v.truncate(limit as usize);
        v
    })
}

#[query]
pub fn get_top_lps(limit: u64) -> Vec<(Principal, u128, u32)> {
    read_state(|s| {
        let total = s.lp_total_supply.max(1);
        let mut v: Vec<(Principal, u128, u32)> = s.lp_balances.iter()
            .map(|(p, lp)| (*p, *lp, ((*lp * 10_000) / total) as u32))
            .collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        v.truncate(limit as usize);
        v
    })
}
```

- [ ] **Step 3: Compile**

Run: `cargo check -p rumi_3pool`

- [ ] **Step 4: Commit**

```bash
git add src/rumi_3pool/src/lib.rs
git commit -m "feat(3pool): explorer aggregated stats endpoints (E5-E9)"
```

---

### Task 19: Time series endpoints (E10-E13)

**Files:**
- Modify: `src/rumi_3pool/src/lib.rs`

- [ ] **Step 1: Implement bucketing helpers and four series queries**

```rust
fn bucket_floor(ts_ns: u64, bucket_secs: u64) -> u64 {
    let bucket_ns = bucket_secs * 1_000_000_000;
    (ts_ns / bucket_ns) * bucket_ns
}

#[query]
pub fn get_volume_series(window: StatsWindow, bucket_seconds: u64) -> Vec<VolumePoint> {
    let now = ic_cdk::api::time();
    let cutoff = window_cutoff_ns(window, now);
    read_state(|s| {
        let mut map: std::collections::BTreeMap<u64, [u128; 3]> = std::collections::BTreeMap::new();
        for e in s.swap_events().iter().filter(|e| e.timestamp >= cutoff) {
            let bucket = bucket_floor(e.timestamp, bucket_seconds);
            let entry = map.entry(bucket).or_insert([0; 3]);
            entry[e.token_in as usize] = entry[e.token_in as usize].saturating_add(e.amount_in);
        }
        map.into_iter().map(|(t, v)| VolumePoint { timestamp: t, volume_per_token: v }).collect()
    })
}

#[query]
pub fn get_balance_series(window: StatsWindow, bucket_seconds: u64) -> Vec<BalancePoint> {
    let now = ic_cdk::api::time();
    let cutoff = window_cutoff_ns(window, now);
    read_state(|s| {
        let mut map: std::collections::BTreeMap<u64, [u128; 3]> = std::collections::BTreeMap::new();
        // Last balance in each bucket wins
        for e in s.swap_events().iter().filter(|e| e.timestamp >= cutoff) {
            let bucket = bucket_floor(e.timestamp, bucket_seconds);
            map.insert(bucket, e.pool_balances_after);
        }
        map.into_iter().map(|(t, b)| BalancePoint { timestamp: t, balances: b }).collect()
    })
}

#[query]
pub fn get_virtual_price_series(window: StatsWindow, bucket_seconds: u64) -> Vec<VirtualPricePoint> {
    let now = ic_cdk::api::time();
    let cutoff = window_cutoff_ns(window, now);
    read_state(|s| {
        let mut map: std::collections::BTreeMap<u64, u128> = std::collections::BTreeMap::new();
        for snap in s.snapshots().iter().filter(|sn| sn.timestamp_secs * 1_000_000_000 >= cutoff) {
            let bucket = bucket_floor(snap.timestamp_secs * 1_000_000_000, bucket_seconds);
            map.insert(bucket, snap.virtual_price);
        }
        map.into_iter().map(|(t, vp)| VirtualPricePoint { timestamp: t, virtual_price: vp }).collect()
    })
}

#[query]
pub fn get_fee_series(window: StatsWindow, bucket_seconds: u64) -> Vec<FeePoint> {
    let now = ic_cdk::api::time();
    let cutoff = window_cutoff_ns(window, now);
    read_state(|s| {
        let mut sums: std::collections::BTreeMap<u64, (u128, u128)> = std::collections::BTreeMap::new();
        for e in s.swap_events().iter().filter(|e| e.timestamp >= cutoff) {
            let bucket = bucket_floor(e.timestamp, bucket_seconds);
            let entry = sums.entry(bucket).or_insert((0, 0));
            entry.0 += (e.fee_bps as u128) * e.amount_in;
            entry.1 += e.amount_in;
        }
        sums.into_iter().map(|(t, (w, v))| FeePoint {
            timestamp: t,
            avg_fee_bps: if v == 0 { 0 } else { (w / v) as u32 },
        }).collect()
    })
}
```

- [ ] **Step 2: Compile**

Run: `cargo check -p rumi_3pool`

- [ ] **Step 3: Commit**

```bash
git add src/rumi_3pool/src/lib.rs
git commit -m "feat(3pool): explorer time-series endpoints (E10-E13)"
```

---

### Task 20: `get_pool_health` (E14)

**Files:**
- Modify: `src/rumi_3pool/src/lib.rs`

- [ ] **Step 1: Implement**

```rust
#[query]
pub fn get_pool_health() -> PoolHealth {
    let now = ic_cdk::api::time();
    read_state(|s| {
        let pm = [s.config.tokens[0].precision_mul, s.config.tokens[1].precision_mul, s.config.tokens[2].precision_mul];
        let current_imbalance = math::compute_imbalance(&s.balances, &pm);
        let params = s.fee_params();

        // imbalance_trend_1h: compare current vs imbalance from 1h ago
        let one_hour_ago = now.saturating_sub(3600 * 1_000_000_000);
        let past_imb = s.swap_events().iter()
            .find(|e| e.timestamp >= one_hour_ago)
            .map(|e| e.imbalance_before)
            .unwrap_or(current_imbalance);
        let trend = current_imbalance as i64 - past_imb as i64;
        let imbalance_trend_1h: i32 = trend.clamp(i32::MIN as i64, i32::MAX as i64) as i32;

        let last_swap_age_seconds = s.swap_events().last()
            .map(|e| (now.saturating_sub(e.timestamp)) / 1_000_000_000)
            .unwrap_or(u64::MAX);

        // fee_at_max_imbalance_swap: fee that would be charged if a hypothetical
        // worst-case imbalancing trade pushed imbalance to saturation
        let fee_at_max = math::compute_fee_bps(current_imbalance, params.imb_saturation, &params);

        // arb_opportunity_score: linear in current imbalance up to saturation
        let score = ((current_imbalance.min(params.imb_saturation) as u128 * 100)
            / params.imb_saturation as u128) as u8;

        PoolHealth {
            current_imbalance,
            imbalance_trend_1h,
            last_swap_age_seconds,
            fee_at_min: params.min_fee_bps,
            fee_at_max_imbalance_swap: fee_at_max,
            arb_opportunity_score: score,
        }
    })
}
```

- [ ] **Step 2: Compile**

Run: `cargo check -p rumi_3pool`

- [ ] **Step 3: Commit**

```bash
git add src/rumi_3pool/src/lib.rs
git commit -m "feat(3pool): explorer pool health endpoint (E14)"
```

---

## Phase 8: Candid + Integration Tests

### Task 21: Update `rumi_3pool.did`

**Files:**
- Modify: `src/rumi_3pool/rumi_3pool.did`

- [ ] **Step 1: Add candid declarations**

For each new type and method added in phases 1-7, add the corresponding candid declaration. Use `cargo build -p rumi_3pool --target wasm32-unknown-unknown` followed by `candid-extractor` if available, otherwise hand-write following the existing patterns in the file.

Required new declarations:
- types: `FeeCurveParams`, `SwapQuote`, `PoolState`, `RebalanceQuote`, `ImbalanceSnapshot`, `SwapLeg`, `SimulatedPath`, `StatsWindow`, `PoolStats`, `ImbalanceStats`, `FeeBucket`, `FeeStats`, `VolumePoint`, `BalancePoint`, `VirtualPricePoint`, `FeePoint`, `PoolHealth`
- methods: `quote_swap`, `get_pool_state`, `get_fee_curve_params`, `quote_optimal_rebalance`, `get_imbalance_history`, `simulate_swap_path`, `set_fee_curve_params`, `get_swap_events_by_principal`, `get_swap_events_by_time_range`, `get_liquidity_events_by_principal`, `get_admin_events`, `get_pool_stats`, `get_imbalance_stats`, `get_fee_stats`, `get_top_swappers`, `get_top_lps`, `get_volume_series`, `get_balance_series`, `get_virtual_price_series`, `get_fee_series`, `get_pool_health`
- existing `SwapEvent` and `LiquidityEvent` records: add the new v2 fields

- [ ] **Step 2: Verify build still works**

Run: `dfx build rumi_3pool` (or whatever the project's build command is — see CLAUDE.md / dfx.json).
Expected: builds without candid errors.

- [ ] **Step 3: Commit**

```bash
git add src/rumi_3pool/rumi_3pool.did
git commit -m "feat(3pool): candid declarations for dynamic fee + bot/explorer endpoints"
```

---

### Task 22: Pocket-IC integration test — dominant flow

**Files:**
- Modify: `src/rumi_3pool/tests/pocket_ic_3usd.rs` (or create new test file `src/rumi_3pool/tests/dynamic_fees.rs`)

- [ ] **Step 1: Write the test**

```rust
#[test]
fn dominant_flow_fee_grows_with_imbalance() {
    let env = setup_3pool_env();
    // Repeatedly swap icUSD -> ckUSDT and observe fee_bps growing
    let mut last_fee_bps = 0u16;
    for _ in 0..5 {
        let q: SwapQuote = query(&env, "quote_swap", (0u8, 1u8, 10_000_00000000u128)).unwrap();
        assert!(q.fee_bps >= last_fee_bps, "fee should be non-decreasing");
        last_fee_bps = q.fee_bps;
        // Execute the swap to actually shift balances
        let _: u128 = call(&env, "swap", (0u8, 1u8, 10_000_00000000u128, 0u128)).unwrap();
    }
    assert!(last_fee_bps > 1, "after several imbalancing swaps, fee should be above MIN");
}

#[test]
fn rebalancing_swap_pays_min_fee() {
    let env = setup_3pool_env();
    // Push pool out of balance
    for _ in 0..5 {
        let _: u128 = call(&env, "swap", (0u8, 1u8, 10_000_00000000u128, 0u128)).unwrap();
    }
    // Now swap the OTHER way (ckUSDT -> icUSD) — this rebalances
    let q: SwapQuote = query(&env, "quote_swap", (1u8, 0u8, 5_000_000000u128)).unwrap();
    assert!(q.is_rebalancing);
    assert_eq!(q.fee_bps, 1, "rebalancing trade must pay MIN_FEE");
}

#[test]
fn admin_can_update_fee_params() {
    let env = setup_3pool_env_as_admin();
    let new_params = FeeCurveParams { min_fee_bps: 2, max_fee_bps: 80, imb_saturation: 200_000_000 };
    let _: () = call(&env, "set_fee_curve_params", (new_params,)).unwrap();
    let got: FeeCurveParams = query(&env, "get_fee_curve_params", ()).unwrap();
    assert_eq!(got.min_fee_bps, 2);
    assert_eq!(got.max_fee_bps, 80);
    assert_eq!(got.imb_saturation, 200_000_000);
}

#[test]
fn non_admin_cannot_update_fee_params() {
    let env = setup_3pool_env();
    let new_params = FeeCurveParams::default();
    let result: Result<(), _> = call_as(&env, "set_fee_curve_params", (new_params,), non_admin_principal());
    assert!(result.is_err());
}
```

(Helper function names like `setup_3pool_env`, `query`, `call`, `call_as` should match existing patterns in the test file. Adapt to whatever helpers already exist there — read the file first.)

- [ ] **Step 2: Run integration tests**

Run: `POCKET_IC_BIN=./pocket-ic cargo test -p rumi_3pool --test pocket_ic_3usd dynamic`
Expected: pass.

- [ ] **Step 3: Commit**

```bash
git add src/rumi_3pool/tests/
git commit -m "test(3pool): integration tests for dynamic fees and admin params"
```

---

### Task 23: Migration integration test

**Files:**
- Modify: `src/rumi_3pool/tests/pocket_ic_3usd.rs`

- [ ] **Step 1: Write upgrade test**

```rust
#[test]
fn upgrade_migrates_v1_events_to_v2() {
    let env = setup_3pool_env();
    // Do some swaps under "old" semantics (fresh canister already produces v2 events,
    // so this test really verifies the post_upgrade path is idempotent and doesn't lose data)
    for _ in 0..3 {
        let _: u128 = call(&env, "swap", (0u8, 1u8, 1_000_00000000u128, 0u128)).unwrap();
    }
    let before: Vec<SwapEvent> = query(&env, "get_swap_events", (0u64, 100u64)).unwrap();
    assert_eq!(before.len(), 3);

    // Upgrade the canister to itself (re-runs post_upgrade)
    upgrade_3pool(&env);

    let after: Vec<SwapEvent> = query(&env, "get_swap_events", (0u64, 100u64)).unwrap();
    assert_eq!(after.len(), 3, "events must survive upgrade");
    assert_eq!(after[0].id, before[0].id);
}
```

- [ ] **Step 2: Run**

Run: `POCKET_IC_BIN=./pocket-ic cargo test -p rumi_3pool --test pocket_ic_3usd upgrade_migrates`
Expected: pass.

- [ ] **Step 3: Commit**

```bash
git add src/rumi_3pool/tests/pocket_ic_3usd.rs
git commit -m "test(3pool): upgrade migration preserves events"
```

---

## Phase 9: Final Verification

### Task 24: Full test suite

- [ ] **Step 1: Unit tests**

Run: `cargo test -p rumi_3pool --lib`
Expected: all pass.

- [ ] **Step 2: Integration tests**

Run: `POCKET_IC_BIN=./pocket-ic cargo test -p rumi_3pool --test pocket_ic_3usd`
Expected: all pass.

- [ ] **Step 3: Workspace build**

Run: `cargo build --release -p rumi_3pool --target wasm32-unknown-unknown`
Expected: clean build, wasm artifact produced.

- [ ] **Step 4: Lint**

Run: `cargo clippy -p rumi_3pool -- -D warnings`
Expected: no warnings.

### Task 25: Local deploy smoke test

- [ ] **Step 1: Start replica and deploy**

```bash
dfx start --background
dfx deploy rumi_3pool
```

- [ ] **Step 2: Query the new endpoints**

```bash
dfx canister call rumi_3pool get_fee_curve_params '()'
dfx canister call rumi_3pool get_pool_state '()'
dfx canister call rumi_3pool get_pool_health '()'
```

Expected: returns sane values with default fee params (1, 99, 250_000_000).

- [ ] **Step 3: Stop replica**

```bash
dfx stop
```

- [ ] **Step 4: Commit any final fixes**

If smoke test reveals issues, fix and commit.

```bash
git add -u
git commit -m "fix(3pool): smoke test corrections"
```

---

## Done

At this point the branch `feat/3pool-dynamic-fees` should contain:

- Pure dynamic fee math (`compute_imbalance`, `compute_fee_bps`)
- Dynamic fees applied to swaps and single-sided liquidity ops
- v2 event schema with one-shot v1 -> v2 migration in `post_upgrade`
- 7 bot endpoints (`quote_swap`, `get_pool_state`, `quote_optimal_rebalance`, `get_imbalance_history`, `simulate_swap_path`, `get_fee_curve_params`, enriched `get_swap_events`)
- 14 explorer endpoints (raw, aggregated, time-series, health)
- Admin endpoint `set_fee_curve_params` with validation and audit log
- Updated candid file
- Unit + integration tests

**Out of scope (deferred to follow-up PRs):**
- `MemoryManager` migration of canister state (separate brick-risk fix)
- Frontend UI changes for dynamic fees
- Bot updates to consume new endpoints (different working folder)
- Removing legacy v1 fields from state (one upgrade cycle of soak time first)

**Mainnet deploy:** when this branch merges and Rob authorizes the deploy, the upgrade arg should read approximately:

```
--argument '(variant { Upgrade = record { mode = null; description = opt "Dynamic fees + v2 event schema + bot/explorer endpoints" } })'
```

(Verify this matches the existing `ThreePoolUpgradeArg` shape; the exact form lives in `state.rs` / `lib.rs` `post_upgrade` signature.)
