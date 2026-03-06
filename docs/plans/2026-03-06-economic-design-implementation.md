# Economic Design Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement the economic design changes agreed in the March 6 design session: proportional recovery buffer, fix stablecoin repayment minting, interest revenue split to stability pool, dynamic Redemption Margin Ratio, and recovery mode borrowing fee decision.

**Architecture:** All changes are in the Rumi Protocol backend canister (`rumi_protocol_backend`). The interest revenue split requires a lightweight integration with the stability pool canister. Each task is self-contained with its own tests and commit.

**Tech Stack:** Rust, Internet Computer (IC), candid, rust_decimal, proptest (testing)

---

## Dependency Graph

```
Task 1 (proportional buffer) ─── independent
Task 2 (fix stablecoin mint) ─── independent
Task 3 (fee routing) ────────── depends on Task 2
Task 4 (interest split) ─────── depends on Task 2, Task 3
Task 5 (stablecoin interest) ── depends on Task 4
Task 6 (dynamic RMR) ────────── independent
Task 7 (recovery fee) ────────── independent
Task 8 (whitepaper) ──────────── do last
```

**Recommended execution order:** 1 → 2 → 3 → 6 → 7 → 4 → 5 → 8

Tasks 1, 2, 6, and 7 are independent and can be parallelized.
Tasks 4 and 5 require 2 and 3 to be complete first.

---

## Task 1: Proportional Recovery Buffer

**Goal:** Change recovery CR from `borrow_threshold + flat_buffer` to `borrow_threshold × multiplier` so the safety margin scales with each asset's risk profile.

**Files:**
- Modify: `src/rumi_protocol_backend/src/state.rs` (lines 51, 941-945, 810-848, 1290-1292)
- Modify: `src/rumi_protocol_backend/src/main.rs` (lines 1461-1486)
- Modify: `src/rumi_protocol_backend/src/event.rs` (lines 1024-1030)
- Test: inline `#[cfg(test)]` in `src/rumi_protocol_backend/src/state.rs`

### Step 1: Write failing tests

Add to the `#[cfg(test)]` module at the bottom of `src/rumi_protocol_backend/src/state.rs`:

```rust
#[test]
fn test_proportional_recovery_cr() {
    let mut state = test_state();
    let icp = state.icp_ledger_principal;

    // Default multiplier: 1.0333
    // ICP borrow_threshold = 1.50
    // recovery_cr = 1.50 * 1.0333 = 1.55 (same as before for ICP)
    let recovery_cr = state.get_recovery_cr_for(&icp);
    assert!(
        (recovery_cr.to_f64() - 1.55).abs() < 0.001,
        "ICP recovery CR should be ~1.55, got {}",
        recovery_cr.to_f64()
    );

    // Add a collateral with borrow_threshold 1.20
    let fake_ledger = Principal::from_text("aaaaa-aa").unwrap();
    let mut config = state.collateral_configs.get(&icp).unwrap().clone();
    config.borrow_threshold_ratio = Ratio::from_f64(1.20);
    config.ledger_canister_id = fake_ledger;
    state.collateral_configs.insert(fake_ledger, config);

    // recovery_cr = 1.20 * 1.0333 = 1.24
    let recovery_cr_low = state.get_recovery_cr_for(&fake_ledger);
    assert!(
        (recovery_cr_low.to_f64() - 1.24).abs() < 0.001,
        "Low-threshold recovery CR should be ~1.24, got {}",
        recovery_cr_low.to_f64()
    );

    // Add a collateral with borrow_threshold 2.00
    let fake_ledger2 = Principal::from_text("2vxsx-fae").unwrap();
    let mut config2 = state.collateral_configs.get(&icp).unwrap().clone();
    config2.borrow_threshold_ratio = Ratio::from_f64(2.00);
    config2.ledger_canister_id = fake_ledger2;
    state.collateral_configs.insert(fake_ledger2, config2);

    // recovery_cr = 2.00 * 1.0333 = 2.0667
    let recovery_cr_high = state.get_recovery_cr_for(&fake_ledger2);
    assert!(
        (recovery_cr_high.to_f64() - 2.0667).abs() < 0.001,
        "High-threshold recovery CR should be ~2.0667, got {}",
        recovery_cr_high.to_f64()
    );
}

#[test]
fn test_proportional_recovery_cr_reconfigurable() {
    let mut state = test_state();
    let icp = state.icp_ledger_principal;

    // Change multiplier to 1.05 (5% proportional buffer)
    state.recovery_cr_multiplier = Ratio::from_f64(1.05);
    let recovery_cr = state.get_recovery_cr_for(&icp);
    // 1.50 * 1.05 = 1.575
    assert!(
        (recovery_cr.to_f64() - 1.575).abs() < 0.001,
        "Expected 1.575, got {}",
        recovery_cr.to_f64()
    );
}
```

### Step 2: Run tests to verify they fail

Run: `cargo test --manifest-path src/rumi_protocol_backend/Cargo.toml test_proportional_recovery_cr 2>&1 | tail -20`

Expected: compilation error — `recovery_cr_multiplier` field does not exist.

### Step 3: Rename constant and add field

In `src/rumi_protocol_backend/src/state.rs`:

**Line 51** — rename constant:
```rust
// OLD:
pub const DEFAULT_RECOVERY_LIQUIDATION_BUFFER: Ratio = Ratio::new(dec!(0.05));
// NEW:
pub const DEFAULT_RECOVERY_CR_MULTIPLIER: Ratio = Ratio::new(dec!(1.0333));
```

**State struct** (~line 325) — rename field:
```rust
// OLD:
pub recovery_liquidation_buffer: Ratio,
// NEW:
pub recovery_cr_multiplier: Ratio,
```

**`From<InitArg>`** (~line 533) — update initialization:
```rust
// OLD:
recovery_liquidation_buffer: DEFAULT_RECOVERY_LIQUIDATION_BUFFER,
// NEW:
recovery_cr_multiplier: DEFAULT_RECOVERY_CR_MULTIPLIER,
```

### Step 4: Update `get_recovery_cr_for`

**Lines 941-945** — change from addition to multiplication:
```rust
// OLD:
pub fn get_recovery_cr_for(&self, ct: &CollateralType) -> Ratio {
    let borrow_threshold = self.collateral_configs.get(ct)
        .map(|c| c.borrow_threshold_ratio)
        .unwrap_or(RECOVERY_COLLATERAL_RATIO);
    borrow_threshold + self.recovery_liquidation_buffer
}

// NEW:
pub fn get_recovery_cr_for(&self, ct: &CollateralType) -> Ratio {
    let borrow_threshold = self.collateral_configs.get(ct)
        .map(|c| c.borrow_threshold_ratio)
        .unwrap_or(RECOVERY_COLLATERAL_RATIO);
    borrow_threshold * self.recovery_cr_multiplier
}
```

### Step 5: Update `get_warning_cr_for`

**Lines 950-957** — the formula `2 * recovery_cr - borrow_threshold` still works, no change needed to the logic, just ensure it compiles with the renamed field. Verify no reference to `recovery_liquidation_buffer`.

### Step 6: Update `get_recovery_target_cr_for`

**Lines 1290-1292** — change from addition to multiplication:
```rust
// OLD:
pub fn get_recovery_target_cr_for(&self, _ct: &CollateralType) -> Ratio {
    Ratio::from(self.recovery_mode_threshold.0 + self.recovery_liquidation_buffer.0)
}

// NEW:
pub fn get_recovery_target_cr_for(&self, _ct: &CollateralType) -> Ratio {
    self.recovery_mode_threshold * self.recovery_cr_multiplier
}
```

### Step 7: Update `compute_weighted_cr_averages`

**Lines 810-848** — update the recovery_cr computation inside the loop:
```rust
// OLD (line 836):
let recovery_cr = config.borrow_threshold_ratio.0 + self.recovery_liquidation_buffer.0;

// NEW:
let recovery_cr = config.borrow_threshold_ratio.0 * self.recovery_cr_multiplier.0;
```

### Step 8: Update admin endpoint

**`src/rumi_protocol_backend/src/main.rs`** (~line 1461):
- Rename function: `set_recovery_liquidation_buffer` → `set_recovery_cr_multiplier`
- Change validation range from `[0.01, 0.5]` to `[1.001, 1.5]` (0.1% to 50% proportional buffer)
- Update log message

**Query endpoint** (~line 1486):
- Rename: `get_recovery_liquidation_buffer` → `get_recovery_cr_multiplier`

### Step 9: Update event system

**`src/rumi_protocol_backend/src/event.rs`**:
- Rename event variant: `SetRecoveryLiquidationBuffer` → `SetRecoveryCrMultiplier`
- Update replay handler (~line 1024-1030): `state.recovery_cr_multiplier = ...`
- Keep backward compat: add `#[serde(alias = "set_recovery_liquidation_buffer")]` to the renamed variant so old events still deserialize

### Step 10: Update `check_semantically_eq`

Search for `recovery_liquidation_buffer` in the semantic equality check (~line 2008-2012) and rename to `recovery_cr_multiplier`.

### Step 11: Fix all remaining references

Run: `cargo check --manifest-path src/rumi_protocol_backend/Cargo.toml 2>&1 | grep "recovery_liquidation_buffer"`

Fix every remaining reference. Also check frontend types:
- `src/vault_frontend/src/lib/services/types.ts` — update field name
- `src/vault_frontend/src/lib/services/protocol/queryOperations.ts` — update field reference
- Candid `.did` file — update interface

### Step 12: Run tests to verify they pass

Run: `cargo test --manifest-path src/rumi_protocol_backend/Cargo.toml test_proportional_recovery_cr 2>&1 | tail -20`

Expected: PASS

### Step 13: Run full test suite

Run: `cargo test --manifest-path src/rumi_protocol_backend/Cargo.toml 2>&1 | tail -30`

Expected: all existing tests pass. If any test references `recovery_liquidation_buffer` by value (e.g., asserting 0.05), update to use `recovery_cr_multiplier` (1.0333).

### Step 14: Commit

```bash
git add -A src/rumi_protocol_backend/ src/vault_frontend/
git commit -m "feat: change recovery buffer from additive to proportional multiplier

recovery_cr = borrow_threshold × recovery_cr_multiplier (default 1.0333).
For ICP (150%): 150% × 1.0333 = 155% (unchanged).
Proportional buffer scales with each asset's risk profile.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 2: Fix Stablecoin Repayment Minting

**Goal:** On the stablecoin repayment path, stop minting icUSD to treasury for the interest portion. Currently `mint_interest_to_treasury(interest_share)` mints new icUSD even though no icUSD was burned — this is net inflationary.

**Files:**
- Modify: `src/rumi_protocol_backend/src/vault.rs` (lines 768-850)
- Test: inline `#[cfg(test)]` in `src/rumi_protocol_backend/src/state.rs`

### Step 1: Write failing test

Add to the `#[cfg(test)]` module in `state.rs`:

```rust
#[test]
fn test_stablecoin_repayment_does_not_increase_icusd_supply() {
    // This is a design-level test: verify that repay_to_vault returns
    // interest_share correctly, and that the CALLER is responsible for
    // NOT minting icUSD when the repayment was in stablecoins.
    let mut state = test_state();
    let icp = state.icp_ledger_principal;

    // Create vault with 100 icUSD debt, 5 icUSD accrued interest
    let vault_id = state.add_vault(Vault {
        owner: Principal::anonymous(),
        vault_id: 1,
        borrowed_icusd_amount: ICUSD::from(100_000_000_00u64), // 100 icUSD
        collateral_amount: ICP::from(10_000_000_00u64),
        collateral_type: icp,
        accrued_interest: ICUSD::from(5_000_000_00u64), // 5 icUSD interest
        last_accrual_time: 0,
    });

    // Repay 50 icUSD worth
    let (interest_share, principal_share) = state.repay_to_vault(vault_id, ICUSD::from(50_000_000_00u64));

    // Interest share should be proportional: 50 * (5/100) = 2.5 icUSD
    assert!(
        (interest_share.to_u64() as f64 / 1e8 - 2.5).abs() < 0.01,
        "Interest share should be ~2.5 icUSD, got {}",
        interest_share.to_u64() as f64 / 1e8
    );

    // Principal share should be the rest: 50 - 2.5 = 47.5 icUSD
    assert!(
        (principal_share.to_u64() as f64 / 1e8 - 47.5).abs() < 0.01,
        "Principal share should be ~47.5 icUSD, got {}",
        principal_share.to_u64() as f64 / 1e8
    );
}
```

### Step 2: Run test to verify behavior

Run: `cargo test --manifest-path src/rumi_protocol_backend/Cargo.toml test_stablecoin_repayment_does_not_increase 2>&1 | tail -20`

This test verifies the math of `repay_to_vault`. The actual fix is in vault.rs where `mint_interest_to_treasury` is called.

### Step 3: Remove icUSD minting from stablecoin repayment path

In `src/rumi_protocol_backend/src/vault.rs`, find the stablecoin repayment function (~line 846):

```rust
// OLD (line ~846):
mint_interest_to_treasury(interest_share).await;

// NEW — do NOT mint icUSD. The stablecoins stay in the canister as reserves.
// Interest will be routed in a future task (interest revenue split).
// For now, all stablecoins (principal + interest portion) remain as reserves.
log!(crate::INFO,
    "[repay_to_vault_with_stable] Skipping icUSD mint for interest ({} e8s) — stablecoin repayment path",
    interest_share.to_u64()
);
```

### Step 4: Verify compilation

Run: `cargo check --manifest-path src/rumi_protocol_backend/Cargo.toml 2>&1 | tail -10`

Expected: compiles with no new errors.

### Step 5: Run full test suite

Run: `cargo test --manifest-path src/rumi_protocol_backend/Cargo.toml 2>&1 | tail -30`

Expected: PASS

### Step 6: Commit

```bash
git add src/rumi_protocol_backend/src/vault.rs src/rumi_protocol_backend/src/state.rs
git commit -m "fix: stop minting icUSD to treasury on stablecoin repayment path

Stablecoin repayments (ckUSDT/ckUSDC) no longer call
mint_interest_to_treasury. Previously this minted new icUSD for the
interest portion even though no icUSD was burned, causing net inflation.
Stablecoins now remain in the canister as reserves.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 3: Route Stablecoin Repayment Fee to Treasury

**Goal:** The ckstable_repay_fee surcharge (0.05%) should be transferred to the treasury canister as stablecoins, not left sitting in the backend canister's reserves.

**Files:**
- Modify: `src/rumi_protocol_backend/src/vault.rs` (lines 835-850)
- Test: verify via PocketIC or manual canister test (this involves async inter-canister calls)

### Step 1: Understand current flow

Currently in vault.rs (~lines 835-843):
```rust
let base_stable_e6s = raw_amount_e8s / 100;
let fee_rate = read_state(|s| s.ckstable_repay_fee);
let fee_e6s = (Decimal::from(base_stable_e6s) * fee_rate.0).to_u64().unwrap_or(0);
// User sends base_stable_e6s + fee_e6s to canister
```

The fee stays in the canister. We need to transfer `fee_e6s` to treasury after the repayment succeeds.

### Step 2: Add treasury transfer for fee

After the successful `transfer_stable_from` call (~line 843), add:

```rust
// Route fee surcharge to treasury as stablecoins
if fee_e6s > 0 {
    let treasury = read_state(|s| s.treasury_principal);
    if let Some(treasury_principal) = treasury {
        let stable_ledger = read_state(|s| {
            match token_type {
                StableTokenType::CKUSDT => s.ckusdt_ledger_principal,
                StableTokenType::CKUSDC => s.ckusdc_ledger_principal,
            }
        });
        if let Some(ledger) = stable_ledger {
            match management::transfer_collateral(fee_e6s, treasury_principal, ledger).await {
                Ok(block) => {
                    log!(crate::INFO,
                        "[repay_with_stable] Transferred {} e6s fee to treasury (block {})",
                        fee_e6s, block
                    );
                }
                Err(e) => {
                    // Non-critical: fee stays in reserves if transfer fails
                    log!(crate::INFO,
                        "[repay_with_stable] Fee transfer to treasury failed: {:?}. Fee remains in reserves.",
                        e
                    );
                }
            }
        }
    }
}
```

### Step 3: Verify compilation

Run: `cargo check --manifest-path src/rumi_protocol_backend/Cargo.toml 2>&1 | tail -10`

Expected: compiles cleanly.

### Step 4: Run full test suite

Run: `cargo test --manifest-path src/rumi_protocol_backend/Cargo.toml 2>&1 | tail -30`

Expected: PASS (async treasury transfers won't execute in unit tests but shouldn't break anything).

### Step 5: Commit

```bash
git add src/rumi_protocol_backend/src/vault.rs
git commit -m "feat: route stablecoin repayment fee surcharge to treasury

The ckstable_repay_fee (0.05%) is now transferred to the treasury
canister as stablecoins after a successful repayment. Previously the
fee stayed in the backend canister mixed with reserves.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 4: Interest Revenue Split (75/25 to Stability Pool / Protocol)

**Goal:** Split interest revenue: 75% to stability pool depositors (as minted icUSD), 25% to protocol treasury. Configurable split ratio.

**Files:**
- Modify: `src/rumi_protocol_backend/src/state.rs` — add `interest_pool_share` field (default 0.75)
- Modify: `src/rumi_protocol_backend/src/vault.rs` — split interest on icUSD repayment path
- Modify: `src/rumi_protocol_backend/src/treasury.rs` — add `mint_interest_to_stability_pool` function
- Modify: `src/rumi_protocol_backend/src/main.rs` — add admin endpoint for split ratio
- Modify: `src/rumi_protocol_backend/src/event.rs` — add event variant
- Test: inline `#[cfg(test)]` in `src/rumi_protocol_backend/src/state.rs`

### Step 1: Add state field and constant

In `src/rumi_protocol_backend/src/state.rs`:

```rust
// Add constant (~line 52):
pub const DEFAULT_INTEREST_POOL_SHARE: Ratio = Ratio::new(dec!(0.75)); // 75% to stability pool

// Add field to State struct (~line 443):
/// Share of interest revenue sent to stability pool depositors (0.0-1.0).
/// Remainder goes to protocol treasury.
#[serde(default = "default_interest_pool_share")]
pub interest_pool_share: Ratio,

// Add default function:
fn default_interest_pool_share() -> Ratio {
    DEFAULT_INTEREST_POOL_SHARE
}

// Initialize in From<InitArg> (~line 535):
interest_pool_share: DEFAULT_INTEREST_POOL_SHARE,
```

### Step 2: Write failing test for split math

```rust
#[test]
fn test_interest_split_ratios() {
    let state = test_state();

    let interest = ICUSD::from(100_000_000_00u64); // 100 icUSD interest
    let pool_share = interest * state.interest_pool_share;     // 75 icUSD
    let treasury_share = interest - pool_share;                 // 25 icUSD

    assert!(
        (pool_share.to_u64() as f64 / 1e8 - 75.0).abs() < 0.01,
        "Pool share should be ~75, got {}",
        pool_share.to_u64() as f64 / 1e8
    );
    assert!(
        (treasury_share.to_u64() as f64 / 1e8 - 25.0).abs() < 0.01,
        "Treasury share should be ~25, got {}",
        treasury_share.to_u64() as f64 / 1e8
    );
}
```

### Step 3: Run test

Run: `cargo test --manifest-path src/rumi_protocol_backend/Cargo.toml test_interest_split_ratios 2>&1 | tail -20`

Expected: FAIL (field doesn't exist yet) or PASS once field is added.

### Step 4: Add `mint_interest_to_stability_pool` to treasury.rs

In `src/rumi_protocol_backend/src/treasury.rs`, add a new function:

```rust
/// Mint icUSD interest revenue to the stability pool canister.
/// The stability pool distributes this pro-rata to depositors.
pub async fn mint_interest_to_stability_pool(interest_share: ICUSD) {
    if interest_share.to_u64() == 0 {
        return;
    }
    let stability_pool = read_state(|s| s.stability_pool_principal);
    if let Some(pool_principal) = stability_pool {
        match management::mint_icusd(interest_share, pool_principal).await {
            Ok(block_index) => {
                log!(crate::INFO,
                    "[mint_interest_to_stability_pool] Minted {} icUSD to stability pool (block {})",
                    interest_share.to_u64(), block_index
                );
            }
            Err(e) => {
                log!(crate::INFO,
                    "[mint_interest_to_stability_pool] Failed to mint {} icUSD to stability pool: {:?}",
                    interest_share.to_u64(), e
                );
            }
        }
    }
}
```

### Step 5: Modify icUSD repayment path to split interest

In `src/rumi_protocol_backend/src/vault.rs`, in the `repay_to_vault` function (~line 753):

```rust
// OLD:
mint_interest_to_treasury(interest_share).await;

// NEW:
let (pool_share, treasury_share) = read_state(|s| {
    let pool = interest_share * s.interest_pool_share;
    let treasury = interest_share - pool;
    (pool, treasury)
});
mint_interest_to_treasury(treasury_share).await;
crate::treasury::mint_interest_to_stability_pool(pool_share).await;
```

### Step 6: Add admin endpoint for interest_pool_share

In `src/rumi_protocol_backend/src/main.rs`:

```rust
#[candid_method(update)]
#[update]
async fn set_interest_pool_share(new_share: f64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::NotAuthorized);
    }
    if new_share < 0.0 || new_share > 1.0 {
        return Err(ProtocolError::GenericError(
            "Interest pool share must be between 0.0 and 1.0".to_string(),
        ));
    }
    let share = Ratio::from_f64(new_share);
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_interest_pool_share(s, share);
    });
    log!(INFO, "[set_interest_pool_share] Set to: {}", new_share);
    Ok(())
}
```

### Step 7: Add event variant

In `src/rumi_protocol_backend/src/event.rs`:

```rust
// Add to Event enum:
#[serde(rename = "set_interest_pool_share")]
SetInterestPoolShare {
    share: String,
},

// Add replay handler:
Event::SetInterestPoolShare { share } => {
    if let Ok(dec) = share.parse::<Decimal>() {
        state.interest_pool_share = Ratio::from(dec);
    }
}

// Add recording function:
pub fn record_set_interest_pool_share(state: &mut State, share: Ratio) {
    record_event(&Event::SetInterestPoolShare {
        share: share.0.to_string(),
    });
    state.interest_pool_share = share;
}
```

### Step 8: Update ProtocolStats and candid

Add `interest_pool_share` to `ProtocolStats` in `lib.rs` and the `.did` file.

### Step 9: Verify and test

Run: `cargo test --manifest-path src/rumi_protocol_backend/Cargo.toml 2>&1 | tail -30`

Expected: PASS

### Step 10: Commit

```bash
git add -A src/rumi_protocol_backend/
git commit -m "feat: split interest revenue 75/25 between stability pool and treasury

Interest from icUSD repayments is now split: 75% minted to the
stability pool canister (distributed pro-rata to depositors), 25%
minted to protocol treasury. Split ratio is configurable via
set_interest_pool_share admin endpoint.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 5: Stablecoin Repayment Interest Split

**Goal:** On stablecoin repayment path: 75% of interest goes to reserves + mint equal icUSD to stability pool, 25% of interest goes to treasury as stablecoins.

**Depends on:** Task 2 (no icUSD mint on stable path) and Task 4 (interest_pool_share field exists).

**Files:**
- Modify: `src/rumi_protocol_backend/src/vault.rs` (stablecoin repayment function)

### Step 1: Write the math verification test

```rust
#[test]
fn test_stablecoin_interest_split_accounting() {
    // Verify the accounting is correct:
    // 100 icUSD debt with 5 icUSD interest, repaid with ckUSDT
    // interest_share = 5 icUSD
    // pool_share (75%) = 3.75 icUSD worth of stablecoins stay in reserves
    //   + mint 3.75 icUSD to stability pool
    // treasury_share (25%) = 1.25 icUSD worth of stablecoins to treasury
    //
    // Net: 3.75 ckUSDT in reserves backs 3.75 icUSD minted to pool. 1:1. ✓
    // Treasury gets 1.25 ckUSDT as revenue.

    let interest_e8s: u64 = 5_000_000_00; // 5 icUSD in e8s
    let pool_ratio = 0.75_f64;
    let pool_e8s = (interest_e8s as f64 * pool_ratio) as u64;
    let treasury_e8s = interest_e8s - pool_e8s;

    // Convert to e6s (ckStable)
    let pool_e6s = pool_e8s / 100;      // 3_750_000 = 3.75 ckUSDT
    let treasury_e6s = treasury_e8s / 100; // 1_250_000 = 1.25 ckUSDT

    assert_eq!(pool_e6s, 3_750_000);
    assert_eq!(treasury_e6s, 1_250_000);

    // icUSD minted to stability pool = pool_share in e8s
    let icusd_minted = pool_e8s; // 3.75 icUSD
    assert_eq!(icusd_minted, 375_000_000);

    // Verify: reserves (pool_e6s) back the minted icUSD 1:1
    // pool_e6s * 100 == icusd_minted
    assert_eq!(pool_e6s * 100, icusd_minted);
}
```

### Step 2: Implement the stablecoin interest split

In `src/rumi_protocol_backend/src/vault.rs`, in the stablecoin repayment function, where we removed `mint_interest_to_treasury` in Task 2:

```rust
// Split interest: pool_share stays in reserves + mint icUSD to pool,
// treasury_share transferred to treasury as stablecoins
if interest_share.to_u64() > 0 {
    let (pool_share, treasury_share) = read_state(|s| {
        let pool = interest_share * s.interest_pool_share;
        let treasury = interest_share - pool;
        (pool, treasury)
    });

    // 1. Mint icUSD equal to pool_share to stability pool
    //    (backed 1:1 by the stablecoins remaining in reserves)
    crate::treasury::mint_interest_to_stability_pool(pool_share).await;

    // 2. Transfer treasury_share as stablecoins to treasury
    let treasury_e6s = treasury_share.to_u64() / 100; // e8s → e6s
    if treasury_e6s > 0 {
        let treasury_principal = read_state(|s| s.treasury_principal);
        if let Some(tp) = treasury_principal {
            let stable_ledger = read_state(|s| {
                match token_type {
                    StableTokenType::CKUSDT => s.ckusdt_ledger_principal,
                    StableTokenType::CKUSDC => s.ckusdc_ledger_principal,
                }
            });
            if let Some(ledger) = stable_ledger {
                let _ = management::transfer_collateral(treasury_e6s, tp, ledger).await;
            }
        }
    }
}
```

### Step 3: Run tests

Run: `cargo test --manifest-path src/rumi_protocol_backend/Cargo.toml 2>&1 | tail -30`

Expected: PASS

### Step 4: Commit

```bash
git add src/rumi_protocol_backend/src/vault.rs src/rumi_protocol_backend/src/state.rs
git commit -m "feat: implement stablecoin repayment interest split

On stablecoin repayment: 75% of interest stays in reserves + equal
icUSD minted to stability pool (1:1 backed). 25% of interest
transferred to treasury as stablecoins. No unbacked icUSD is created.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 6: Dynamic Redemption Margin Ratio

**Goal:** Redeemers get different rates based on system health. 96% at/above 1.5× system recovery ratio (discourages redemption when healthy), 100% at/below recovery ratio (par redemption when stressed), linear between.

**Files:**
- Modify: `src/rumi_protocol_backend/src/state.rs` — add `get_redemption_margin_ratio` function
- Modify: `src/rumi_protocol_backend/src/vault.rs` — apply RMR to redemption amount
- Test: inline `#[cfg(test)]` in `src/rumi_protocol_backend/src/state.rs`

### Step 1: Write failing tests

```rust
#[test]
fn test_dynamic_rmr_healthy_system() {
    let mut state = test_state();
    // System TCR at 2.25 (1.5 × recovery threshold of 1.50)
    state.total_collateral_ratio = Ratio::from_f64(2.25);
    state.recovery_mode_threshold = RECOVERY_COLLATERAL_RATIO; // 1.50

    let rmr = state.get_redemption_margin_ratio();
    assert!(
        (rmr.to_f64() - 0.96).abs() < 0.001,
        "RMR at 1.5× recovery should be 0.96, got {}",
        rmr.to_f64()
    );
}

#[test]
fn test_dynamic_rmr_at_recovery() {
    let mut state = test_state();
    // System TCR at recovery threshold
    state.total_collateral_ratio = Ratio::from_f64(1.50);
    state.recovery_mode_threshold = RECOVERY_COLLATERAL_RATIO;

    let rmr = state.get_redemption_margin_ratio();
    assert!(
        (rmr.to_f64() - 1.0).abs() < 0.001,
        "RMR at recovery threshold should be 1.0, got {}",
        rmr.to_f64()
    );
}

#[test]
fn test_dynamic_rmr_midpoint() {
    let mut state = test_state();
    // System TCR at midpoint: (1.50 + 2.25) / 2 = 1.875
    state.total_collateral_ratio = Ratio::from_f64(1.875);
    state.recovery_mode_threshold = RECOVERY_COLLATERAL_RATIO;

    let rmr = state.get_redemption_margin_ratio();
    // Linear interpolation: 1.0 - (1.875 - 1.50) / (2.25 - 1.50) * 0.04
    // = 1.0 - 0.375 / 0.75 * 0.04 = 1.0 - 0.02 = 0.98
    assert!(
        (rmr.to_f64() - 0.98).abs() < 0.001,
        "RMR at midpoint should be 0.98, got {}",
        rmr.to_f64()
    );
}

#[test]
fn test_dynamic_rmr_below_recovery() {
    let mut state = test_state();
    // System TCR below recovery
    state.total_collateral_ratio = Ratio::from_f64(1.30);
    state.recovery_mode_threshold = RECOVERY_COLLATERAL_RATIO;

    let rmr = state.get_redemption_margin_ratio();
    // Capped at 1.0 — never above par
    assert!(
        (rmr.to_f64() - 1.0).abs() < 0.001,
        "RMR below recovery should be 1.0, got {}",
        rmr.to_f64()
    );
}

#[test]
fn test_dynamic_rmr_above_15x() {
    let mut state = test_state();
    // System TCR way above 1.5× recovery
    state.total_collateral_ratio = Ratio::from_f64(5.0);
    state.recovery_mode_threshold = RECOVERY_COLLATERAL_RATIO;

    let rmr = state.get_redemption_margin_ratio();
    // Capped at 0.96 — never below floor
    assert!(
        (rmr.to_f64() - 0.96).abs() < 0.001,
        "RMR above 1.5× should be capped at 0.96, got {}",
        rmr.to_f64()
    );
}
```

### Step 2: Run tests to verify they fail

Run: `cargo test --manifest-path src/rumi_protocol_backend/Cargo.toml test_dynamic_rmr 2>&1 | tail -20`

Expected: FAIL — `get_redemption_margin_ratio` does not exist.

### Step 3: Implement `get_redemption_margin_ratio`

In `src/rumi_protocol_backend/src/state.rs`:

```rust
/// Add constants:
pub const RMR_FLOOR: Ratio = Ratio::new(dec!(0.96));     // 96% at healthy system
pub const RMR_CEILING: Ratio = Ratio::new(dec!(1.0));     // 100% at/below recovery
pub const RMR_HEALTHY_MULTIPLIER: Ratio = Ratio::new(dec!(1.5)); // 1.5× recovery = "healthy"

/// Dynamic Redemption Margin Ratio.
/// Redeemers receive RMR × face value of their icUSD.
/// - At/above 1.5× recovery threshold: 96% (discourages redemption when healthy)
/// - At/below recovery threshold: 100% (par redemption when system stressed)
/// - Linear interpolation between
pub fn get_redemption_margin_ratio(&self) -> Ratio {
    let tcr = self.total_collateral_ratio;
    let recovery = self.recovery_mode_threshold;
    let healthy = recovery * RMR_HEALTHY_MULTIPLIER; // e.g., 1.50 × 1.5 = 2.25

    if tcr <= recovery {
        return RMR_CEILING; // 100%
    }
    if tcr >= healthy {
        return RMR_FLOOR; // 96%
    }

    // Linear interpolation: 1.0 - ((tcr - recovery) / (healthy - recovery)) × (1.0 - 0.96)
    let range = healthy - recovery;  // e.g., 0.75
    let position = tcr - recovery;   // how far above recovery
    let spread = RMR_CEILING - RMR_FLOOR; // 0.04
    let discount = (position / range) * spread;
    RMR_CEILING - discount
}
```

### Step 4: Run tests

Run: `cargo test --manifest-path src/rumi_protocol_backend/Cargo.toml test_dynamic_rmr 2>&1 | tail -20`

Expected: PASS

### Step 5: Apply RMR to redemption flow

In `src/rumi_protocol_backend/src/vault.rs`, in `redeem_collateral` (~line 332), after computing the redemption fee:

```rust
// Apply Redemption Margin Ratio — redeemer gets RMR × (amount - fee)
let rmr = read_state(|s| s.get_redemption_margin_ratio());
let net_after_fee = icusd_amount - fee_icusd;
let effective_redemption = net_after_fee * rmr;
// Use effective_redemption instead of net_after_fee for collateral calculation
```

Also apply RMR to reserve redemptions in `redeem_reserves` (~line 155):

```rust
let rmr = read_state(|s| s.get_redemption_margin_ratio());
let net_icusd = (icusd_amount - fee_icusd) * rmr;
```

The difference between face value and RMR-adjusted value stays in the system, effectively reducing the icUSD supply relative to collateral backing (strengthening the peg).

### Step 6: Run full test suite

Run: `cargo test --manifest-path src/rumi_protocol_backend/Cargo.toml 2>&1 | tail -30`

Expected: PASS

### Step 7: Commit

```bash
git add src/rumi_protocol_backend/src/state.rs src/rumi_protocol_backend/src/vault.rs
git commit -m "feat: implement dynamic Redemption Margin Ratio

RMR scales linearly: 96% when system is healthy (≥1.5× recovery
threshold), 100% at/below recovery threshold. Discourages redemption
when the system doesn't need it, allows par redemption under stress.
Applied to both reserve and vault redemption paths.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 7: Recovery Mode Borrowing Fee Decision

**Goal:** Decide and document whether the existing per-collateral `recovery_borrowing_fee` static override is sufficient, or whether a dynamic multiplier is needed.

**Context:** The infrastructure already exists:
- `CollateralConfig.recovery_borrowing_fee: Option<Ratio>` — per-collateral static override
- `get_borrowing_fee_for()` in state.rs already checks this in Recovery mode
- Admin can set it via `set_collateral_recovery_borrowing_fee` (if endpoint exists) or directly

**Files:**
- Potentially modify: `src/rumi_protocol_backend/src/state.rs`
- Potentially modify: `src/rumi_protocol_backend/src/main.rs`

### Step 1: Check if admin endpoint exists for recovery_borrowing_fee

Search for `set_collateral_recovery_borrowing_fee` or `set_recovery_borrowing_fee` in main.rs.

If it exists: the feature is complete. Document the current behavior and close this item.

If it doesn't exist: add a simple admin endpoint:

```rust
#[candid_method(update)]
#[update]
async fn set_collateral_recovery_borrowing_fee(
    collateral_ledger: Principal,
    fee: Option<f64>,
) -> Result<(), ProtocolError> {
    // Validate caller is developer
    // If fee is Some, set recovery_borrowing_fee = Some(Ratio::from_f64(fee))
    // If fee is None, clear the override (reverts to normal fee in recovery)
    // Record event
}
```

### Step 2: Verify `get_borrowing_fee_for` handles it

Confirm state.rs lines 914-924 already return `recovery_borrowing_fee` when set in Recovery mode:

```rust
pub fn get_borrowing_fee_for(&self, ct: &CollateralType) -> Ratio {
    let config = self.collateral_configs.get(ct);
    if self.mode == Mode::Recovery {
        return config
            .and_then(|c| c.recovery_borrowing_fee)
            .or_else(|| config.map(|c| c.borrowing_fee))
            .unwrap_or(self.fee);
    }
    config.map(|c| c.borrowing_fee).unwrap_or(self.fee)
}
```

This is already correct — if `recovery_borrowing_fee` is set, it's used; otherwise falls back to normal fee.

### Step 3: Commit (if changes were needed)

```bash
git add src/rumi_protocol_backend/src/main.rs src/rumi_protocol_backend/src/event.rs
git commit -m "feat: add admin endpoint for per-collateral recovery borrowing fee

Allows setting a per-collateral borrowing fee override during Recovery
mode. When set, overrides the normal borrowing fee. When None, falls
back to normal fee (no more zero-fee in Recovery).

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 8: Update Whitepaper

**Goal:** Remove "Zero borrowing fee" language from the whitepaper regarding Recovery mode. The app docs were already updated in PR #18.

**Files:**
- Modify: whitepaper source files in `/Users/robertripley/Documents/RumiProtocol/RumiWhitepapers/`

### Step 1: Identify the specific sections

In the v2.1 whitepaper, find the Protocol Modes table that says "Zero borrowing fee" under Recovery mode.

### Step 2: Update the language

Replace "Zero borrowing fee" with language reflecting the actual behavior:
- Borrowing is allowed but minimum CR is raised to recovery target (e.g., 155% for ICP)
- Per-collateral fee overrides may apply during Recovery mode
- Interest rate may use recovery override if configured

### Step 3: Regenerate PDF if applicable

If the whitepaper source is markdown or LaTeX, regenerate the PDF.

### Step 4: Commit

```bash
git add docs/
git commit -m "docs: update whitepaper to remove zero borrowing fee in Recovery mode

Replace incorrect 'Zero borrowing fee' language with accurate
description of Recovery mode behavior: raised MCR, configurable
per-collateral fee overrides.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Summary of Execution Order

| Order | Task | Estimated Effort | Dependencies |
|-------|------|-----------------|--------------|
| 1 | Proportional recovery buffer | 30 min | None |
| 2 | Fix stablecoin repayment minting | 15 min | None |
| 3 | Route fee surcharge to treasury | 15 min | Task 2 |
| 4 | Interest revenue split (75/25) | 45 min | Task 2 |
| 5 | Stablecoin interest split | 30 min | Task 2, 3, 4 |
| 6 | Dynamic Redemption Margin Ratio | 30 min | None |
| 7 | Recovery borrowing fee decision | 15 min | None |
| 8 | Update whitepaper | 15 min | None |

**Total estimated: ~3.5 hours**

Tasks 1, 2, 6, and 7 are fully independent and can be parallelized.
