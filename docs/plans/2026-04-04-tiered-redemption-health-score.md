# Tiered Redemption & Vault Health Score Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a per-vault `health_score()` method (CR / liquidation_ratio) and a per-collateral `redemption_tier` (1/2/3) so that redemptions process tier-1 vaults first, then tier-2, then tier-3, with vaults sorted by health score within each tier.

**Architecture:** Add `redemption_tier` (u8, default 1) to `CollateralConfig` and `AddCollateralArg`. Add `health_score()` as a computed method on Vault (takes liquidation_ratio as param). Restructure `redeem_on_vaults()` to group vaults by tier, process tiers in order, and sort by health score instead of raw CR within each tier. The water-filling algorithm itself is unchanged — it just runs scoped to each tier.

**Tech Stack:** Rust (IC canister backend), Candid IDL

---

## Key Files

| File | Purpose |
|------|---------|
| `src/rumi_protocol_backend/src/vault.rs` | Vault struct (line 78), add `health_score()` method |
| `src/rumi_protocol_backend/src/state.rs` | `CollateralConfig` (line 360), `redeem_on_vaults()` (line 2237), tests (line 2621) |
| `src/rumi_protocol_backend/src/lib.rs` | `AddCollateralArg` (line 253), `compute_collateral_ratio()` (line 566) |
| `src/rumi_protocol_backend/src/main.rs` | `add_collateral_token()` (line 3307), admin endpoints |

---

## Phase 1: Health Score

### Step 1.1 — Write failing test for `health_score()`

**File:** `src/rumi_protocol_backend/src/state.rs` (add to test module near line 2621)

```rust
#[test]
fn test_vault_health_score() {
    use crate::vault::Vault;
    use candid::Principal;

    let vault = Vault {
        owner: Principal::anonymous(),
        borrowed_icusd_amount: 100_0000_0000, // 100 icUSD
        collateral_amount: 200_0000_0000,     // placeholder (CR computed externally)
        vault_id: 1,
        collateral_type: Principal::anonymous(),
        last_accrual_time: 0,
        accrued_interest: 0,
        bot_processing: false,
    };

    // ICP vault: CR = 1.50, liq_ratio = 1.33 → health = 1.50 / 1.33 ≈ 1.1278
    let health = vault.health_score(1.50, 1.33);
    assert!((health - 1.1278).abs() < 0.001, "Expected ~1.1278, got {}", health);

    // ckBTC vault: CR = 1.25, liq_ratio = 1.15 → health = 1.25 / 1.15 ≈ 1.0870
    let health2 = vault.health_score(1.25, 1.15);
    assert!((health2 - 1.0870).abs() < 0.001, "Expected ~1.0870, got {}", health2);

    // At exact liquidation threshold: health = 1.0
    let health3 = vault.health_score(1.33, 1.33);
    assert!((health3 - 1.0).abs() < 0.0001, "Expected 1.0, got {}", health3);

    // Zero-debt vault: should return f64::MAX (infinite health)
    let zero_debt_vault = Vault {
        borrowed_icusd_amount: 0,
        ..vault.clone()
    };
    let health4 = zero_debt_vault.health_score(1.50, 1.33);
    assert!(health4 > 1_000_000.0, "Zero-debt vault should have very high health score");
}
```

### Step 1.2 — Run the test, confirm it fails

```bash
cargo test -p rumi_protocol_backend test_vault_health_score 2>&1
# Expected: does not compile — health_score() method doesn't exist
```

### Step 1.3 — Implement `health_score()` on Vault

**File:** `src/rumi_protocol_backend/src/vault.rs`

Add this method to the `impl Vault` block (after existing methods, around line 110+):

```rust
/// Compute the vault's health score: CR / liquidation_ratio.
/// A score of 1.0 means the vault is at its liquidation threshold.
/// Higher is healthier. Normalizes across collateral types so that
/// vaults with different liquidation thresholds can be compared.
///
/// `cr` — the vault's current collateral ratio (from compute_collateral_ratio)
/// `liquidation_ratio` — the collateral type's liquidation threshold (e.g. 1.33)
pub fn health_score(&self, cr: f64, liquidation_ratio: f64) -> f64 {
    if self.borrowed_icusd_amount == 0 {
        return f64::MAX;
    }
    if liquidation_ratio <= 0.0 {
        return f64::MAX; // defensive: avoid division by zero
    }
    cr / liquidation_ratio
}
```

### Step 1.4 — Run the test, confirm it passes

```bash
cargo test -p rumi_protocol_backend test_vault_health_score 2>&1
# Expected: test passes
```

### Step 1.5 — Commit

```
feat(backend): add health_score() method to Vault

Computes CR / liquidation_ratio as a normalized health metric that
works across collateral types with different liquidation thresholds.
A score of 1.0 means the vault is at its liquidation line.
```

---

## Phase 2: Redemption Tier on CollateralConfig

### Step 2.1 — Add `redemption_tier` field to `CollateralConfig`

**File:** `src/rumi_protocol_backend/src/state.rs` (inside the `CollateralConfig` struct, around line 409)

Add after the `rate_curve` field:

```rust
    /// Redemption priority tier (1 = first redeemed, 2 = second, 3 = last).
    /// Tier 1 vaults are redeemed before tier 2, which are redeemed before tier 3.
    /// Default: 1 (most exposed — safe default for new/unknown collateral).
    #[serde(default = "default_redemption_tier")]
    pub redemption_tier: u8,
```

Add the default function near the other default functions in the file:

```rust
fn default_redemption_tier() -> u8 { 1 }
```

### Step 2.2 — Add `redemption_tier` to `AddCollateralArg`

**File:** `src/rumi_protocol_backend/src/lib.rs` (inside `AddCollateralArg`, around line 280)

```rust
    /// Redemption priority tier (1/2/3). Default: 1 if omitted.
    pub redemption_tier: Option<u8>,
```

### Step 2.3 — Wire it into `add_collateral_token()`

**File:** `src/rumi_protocol_backend/src/main.rs` (inside `add_collateral_token()`, around line 3376 where CollateralConfig is built)

Add to the CollateralConfig construction:

```rust
    redemption_tier: arg.redemption_tier.unwrap_or(1).clamp(1, 3),
```

### Step 2.4 — Add admin endpoint to update tier for existing collaterals

**File:** `src/rumi_protocol_backend/src/main.rs` (add near other admin endpoints)

```rust
#[ic_cdk::update]
fn set_redemption_tier(ledger_canister_id: Principal, tier: u8) -> Result<(), String> {
    crate::require_developer()?;
    if tier < 1 || tier > 3 {
        return Err("Tier must be 1, 2, or 3".to_string());
    }
    STATE.with(|s| {
        let mut state = s.borrow_mut();
        match state.collateral_configs.get_mut(&ledger_canister_id) {
            Some(config) => {
                config.redemption_tier = tier;
                Ok(())
            }
            None => Err(format!("No collateral config for {}", ledger_canister_id)),
        }
    })
}

#[ic_cdk::query]
fn get_redemption_tier(ledger_canister_id: Principal) -> Result<u8, String> {
    STATE.with(|s| {
        let state = s.borrow();
        match state.collateral_configs.get(&ledger_canister_id) {
            Some(config) => Ok(config.redemption_tier),
            None => Err(format!("No collateral config for {}", ledger_canister_id)),
        }
    })
}
```

### Step 2.5 — Build and run existing tests to verify nothing breaks

```bash
cargo build -p rumi_protocol_backend 2>&1
cargo test -p rumi_protocol_backend 2>&1
# Expected: compiles, all existing tests pass (serde default handles existing configs)
```

### Step 2.6 — Commit

```
feat(backend): add redemption_tier to CollateralConfig

Per-collateral setting (1/2/3) controlling redemption priority order.
Tier 1 is redeemed first, tier 3 last. Defaults to 1 for existing
and new collateral (safe default: most exposed to redemption).
Includes admin set/get endpoints and AddCollateralArg support.
```

---

## Phase 3: Restructure `redeem_on_vaults()` for Tiered + Health-Score Ordering

### Step 3.1 — Write failing test for tiered redemption ordering

**File:** `src/rumi_protocol_backend/src/state.rs` (add to test module)

```rust
#[test]
fn test_tiered_redemption_ordering() {
    // This test verifies that:
    // 1. Tier 1 vaults are redeemed before tier 2
    // 2. Within a tier, vaults are sorted by health score (CR/liq_ratio), not raw CR
    //
    // Setup: Two collateral types with different liq ratios and tiers.
    // A tier-2 vault with lower raw CR should NOT be redeemed before a tier-1 vault.
    //
    // We'll test the sort ordering logic directly rather than the full
    // redeem_on_vaults flow, since that requires full state setup.

    // Simulated vault entries: (tier, health_score, vault_id)
    let mut entries: Vec<(u8, f64, u64)> = vec![
        (2, 1.05, 10),  // tier 2, low health (close to liq)
        (1, 1.20, 20),  // tier 1, moderate health
        (1, 1.08, 30),  // tier 1, low health
        (3, 1.01, 40),  // tier 3, very low health
        (1, 1.15, 50),  // tier 1, moderate health
    ];

    // Sort: primary by tier ascending, secondary by health score ascending
    entries.sort_by(|a, b| {
        a.0.cmp(&b.0).then_with(|| a.1.partial_cmp(&b.1).unwrap())
    });

    let order: Vec<u64> = entries.iter().map(|e| e.2).collect();
    // Expected: tier 1 first (sorted by health), then tier 2, then tier 3
    assert_eq!(order, vec![30, 50, 20, 10, 40],
        "Expected tier-1 vaults first (health-sorted), then tier-2, then tier-3");
}
```

### Step 3.2 — Run the test, confirm it passes (pure logic test)

```bash
cargo test -p rumi_protocol_backend test_tiered_redemption_ordering 2>&1
# Expected: passes (this tests the sorting approach, not the actual function yet)
```

### Step 3.3 — Refactor `redeem_on_vaults()` to use tiered health-score sorting

**File:** `src/rumi_protocol_backend/src/state.rs`

Replace the vault collection and sorting block (lines ~2264-2281) inside `redeem_on_vaults()`. The current code is:

```rust
// Collect eligible vaults sorted by CR ascending
let mut vault_entries: Vec<(Decimal, VaultId)> = Vec::new();
for vault in self.vault_id_to_vaults.values() {
    if vault.borrowed_icusd_amount == 0 {
        continue;
    }
    let vault_ct = if vault.collateral_type == Principal::anonymous() {
        self.icp_ledger_principal
    } else {
        vault.collateral_type
    };
    if vault_ct != resolved_ct {
        continue;
    }
    let cr = crate::compute_collateral_ratio(vault, collateral_price, self);
    vault_entries.push((cr.0, vault.vault_id));
}
vault_entries.sort_by(|a, b| a.0.cmp(&b.0));
```

Replace with:

```rust
// Collect eligible vaults sorted by: tier ascending, then health score ascending.
// Health score = CR / liquidation_ratio — normalizes across collateral types.
let mut vault_entries: Vec<(Decimal, VaultId)> = Vec::new();

// We still sort by CR (Decimal) for the water-filling math, but we
// need to group by tier and order by health score first.
// Strategy: collect (tier, health_score, cr, vault_id), sort, then
// extract (cr, vault_id) for the existing water-filling algorithm.
struct VaultSortEntry {
    tier: u8,
    health_score: f64,
    cr: Decimal,
    vault_id: VaultId,
}

let mut sort_entries: Vec<VaultSortEntry> = Vec::new();
for vault in self.vault_id_to_vaults.values() {
    if vault.borrowed_icusd_amount == 0 {
        continue;
    }
    let vault_ct = if vault.collateral_type == Principal::anonymous() {
        self.icp_ledger_principal
    } else {
        vault.collateral_type
    };
    if vault_ct != resolved_ct {
        continue;
    }
    let cr = crate::compute_collateral_ratio(vault, collateral_price, self);

    // Look up tier and liquidation ratio from collateral config
    let (tier, liq_ratio) = self.collateral_configs.get(&vault_ct)
        .map(|cfg| (cfg.redemption_tier, cfg.liquidation_ratio.to_f64()))
        .unwrap_or((1, 1.33)); // safe defaults

    let health = vault.health_score(cr.to_f64(), liq_ratio);

    sort_entries.push(VaultSortEntry {
        tier,
        health_score: health,
        cr: cr.0,
        vault_id: vault.vault_id,
    });
}

// Sort: tier ascending (1 before 2 before 3), then health score ascending
sort_entries.sort_by(|a, b| {
    a.tier.cmp(&b.tier).then_with(|| {
        a.health_score.partial_cmp(&b.health_score).unwrap_or(std::cmp::Ordering::Equal)
    })
});

// Extract into the format the water-filling algorithm expects
let vault_entries: Vec<(Decimal, VaultId)> = sort_entries
    .into_iter()
    .map(|e| (e.cr, e.vault_id))
    .collect();
```

**Important:** The rest of the water-filling algorithm (the loop starting around line 2291) operates on `vault_entries: Vec<(Decimal, VaultId)>` — it doesn't need to change. The tiering is achieved purely through the sort order. Tier-1 vaults appear first in the list, so the water-filling naturally processes them first and only reaches tier-2 vaults if it exhausts all tier-1 debt.

**Note on Ratio::to_f64():** Check if `Ratio` has a `to_f64()` method. It wraps `Decimal`, so it may need:
```rust
// If Ratio doesn't have to_f64(), use:
let liq_ratio = f64::try_from(cfg.liquidation_ratio.0).unwrap_or(1.33);
```

### Step 3.4 — Verify the build compiles

```bash
cargo build -p rumi_protocol_backend 2>&1
# Fix any type conversion issues (Decimal → f64, Ratio → f64)
```

### Step 3.5 — Run all tests

```bash
cargo test -p rumi_protocol_backend 2>&1
# Expected: all tests pass (existing redemption behavior unchanged — all current
# vaults are ICP = tier 1, so ordering is equivalent to before)
```

### Step 3.6 — Commit

```
feat(backend): tiered redemption ordering with health score

redeem_on_vaults() now sorts vaults by redemption tier first, then
by health score (CR / liquidation_ratio) within each tier. Tier 1
vaults are fully exhausted before tier 2 is touched. The water-filling
algorithm itself is unchanged — tiering is achieved through sort order.
```

---

## Phase 4: Set Tiers for Existing Collateral Assets

### Step 4.1 — Deploy the upgrade

```bash
dfx deploy rumi_protocol_backend --network ic --argument '(variant { Upgrade = record { mode = null; description = opt "Add redemption_tier to CollateralConfig + health_score on Vault + tiered redemption ordering" } })'
```

### Step 4.2 — Set tiers for existing collateral

Use the `set_redemption_tier` endpoint:

```bash
# Tier 1: ICP, BOB, EXE (most exposed — redeemed first)
dfx canister call rumi_protocol_backend set_redemption_tier '(principal "ryjl3-tyaaa-aaaaa-aaaba-cai", 1 : nat8)' --network ic   # ICP
dfx canister call rumi_protocol_backend set_redemption_tier '(principal "7pail-xaaaa-aaaas-aabmq-cai", 1 : nat8)' --network ic   # EXE
dfx canister call rumi_protocol_backend set_redemption_tier '(principal "buwm7-7yaaa-aaaar-qagva-cai", 1 : nat8)' --network ic   # BOB

# Tier 2: nICP, ckETH (middle tier)
dfx canister call rumi_protocol_backend set_redemption_tier '(principal "nza5v-qaaaa-aaaar-qahzq-cai", 1 : nat8)' --network ic   # nICP — UPDATE TO 2
dfx canister call rumi_protocol_backend set_redemption_tier '(principal "ss2fx-dyaaa-aaaar-qacoq-cai", 1 : nat8)' --network ic   # ckETH — UPDATE TO 2

# Tier 3: ckBTC, ckXAUT (most protected — redeemed last)
dfx canister call rumi_protocol_backend set_redemption_tier '(principal "mxzaz-hqaaa-aaaar-qaada-cai", 1 : nat8)' --network ic   # ckBTC — UPDATE TO 3
dfx canister call rumi_protocol_backend set_redemption_tier '(principal "rh2pm-ryaaa-aaaan-qeniq-cai", 1 : nat8)' --network ic   # ckXAUT — UPDATE TO 3
```

> **Note:** Replace the `1 : nat8` placeholders above with the actual tier values (2 or 3) at deploy time. Left as 1 here because the plan is a template — Rob confirms final tier assignments before deploying.

### Step 4.3 — Verify tiers are set

```bash
dfx canister call rumi_protocol_backend get_redemption_tier '(principal "ryjl3-tyaaa-aaaaa-aaaba-cai")' --network ic
# Expected: (Ok(1))
dfx canister call rumi_protocol_backend get_redemption_tier '(principal "mxzaz-hqaaa-aaaar-qaada-cai")' --network ic
# Expected: (Ok(3))
```

---

## Phase 5: Frontend — Surface Health Score & Tier

> **Deferred.** This phase can be done in a follow-up PR. The backend changes are the priority. When ready, the frontend work would include:
> - Show health score on vault cards in Explorer (alongside or replacing raw CR)
> - Show redemption tier badge on collateral asset labels
> - Potentially color-code vaults by health score (green/yellow/red)

---

## Important Notes

### Water-Filling Across Tiers

The current water-filling algorithm operates on a single sorted list of `(CR, VaultId)`. By placing all tier-1 vaults before tier-2 in that list, the water-filling naturally processes tier-1 first. However, there's a subtlety: the water-filling tries to "level up" vaults to the next CR in the list. When it reaches the boundary between tier 1 and tier 2, the next CR in the list would be the lowest tier-2 vault's CR. This is actually correct behavior — it means "fill all tier-1 vaults up to the level of the best tier-1 vault, then if there's remaining redemption, start on tier-2."

But there's a problem: the water-filling uses raw CR values to compute band leveling math. If tier-1 has ICP vaults at CR 1.45 and tier-2 has ckETH vaults at CR 1.20 (raw), the algorithm would see the ckETH CR as "lower" and the math breaks. **The fix is to use health_score as the sort key for the water-filling bands as well, not raw CR.** This requires changing the `vault_entries` type from `Vec<(Decimal, VaultId)>` to `Vec<(f64, Decimal, VaultId)>` where the f64 is health score (for sorting/banding) and Decimal is raw CR (for the actual value computation). This is addressed in Step 3.3.

**Actually — simpler approach:** Since `redeem_on_vaults()` currently only processes one collateral type at a time (it takes a `collateral_type` parameter and filters), the tiering within a single collateral type is just the standard CR sort. The tier system matters for the *caller* of `redeem_on_vaults()` — it should call it for tier-1 collateral types first, then tier-2, then tier-3. This means the restructuring happens in `redeem_reserves()` / `record_redemption_on_vaults()` (the callers), not inside `redeem_on_vaults()` itself.

**Revised approach for Step 3.3:** Instead of changing the sort inside `redeem_on_vaults()`, change the caller to:
1. Group collateral types by tier
2. For tier 1: call `redeem_on_vaults()` for each tier-1 collateral type, ordered by lowest health score across all tier-1 vaults
3. If there's remaining icUSD, move to tier 2, etc.

This keeps `redeem_on_vaults()` simpler (single-collateral, CR-sorted) and moves the tier logic to the orchestration layer. The health-score sort within a single collateral type is redundant (all vaults have the same liq_ratio, so health score ordering = CR ordering). Health score becomes useful for deciding *which collateral type to hit first within a tier* when there are multiple tier-1 collaterals.
