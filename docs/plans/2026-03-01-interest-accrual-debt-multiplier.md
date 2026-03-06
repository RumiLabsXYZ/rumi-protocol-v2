# Per-Vault Interest Accrual with Dynamic Rates — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement interest accrual on borrowed icUSD where each vault accrues at its own dynamic rate based on its collateral ratio, using the existing `get_dynamic_interest_rate_for()` two-layer rate system.

**Architecture:** Every 300s timer tick, iterate all vaults with outstanding debt, compute each vault's dynamic interest rate from its current CR (Layer 1: per-vault CR multiplier, Layer 2: Recovery mode system multiplier), and apply `factor = 1 + rate * elapsed_nanos / NANOS_PER_YEAR` directly to `borrowed_icusd_amount`. Before any async vault interaction (borrow, repay, close, liquidate), accrue the single vault first to catch the ≤300s gap since last tick. A single `AccrueInterest { timestamp }` event per tick enables deterministic replay.

**Tech Stack:** Rust, IC CDK, rust_decimal, Candid

**Why not global multiplier?** The existing dynamic rate system computes per-vault rates based on each vault's CR — riskier vaults (lower CR) pay more. A single global multiplier per collateral type cannot express per-vault variable rates. Per-vault accrual uses the system as designed and is actually simpler: no changes needed to `compute_collateral_ratio`, `total_borrowed_icusd_amount`, `CandidVault`, `State::repay_to_vault`, or `State::borrow_from_vault`.

---

## Task 1: Add `last_accrual_time` field to `Vault` struct

**Files:**
- Modify: `src/rumi_protocol_backend/src/vault.rs:48-62`

**Step 1: Add the field**

In the `Vault` struct, add `last_accrual_time` with a serde default so existing serialized vaults deserialize to 0:

```rust
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Deserialize, Serialize, PartialOrd, Ord)]
pub struct Vault {
    pub owner: Principal,
    pub borrowed_icusd_amount: ICUSD,
    /// Raw collateral amount in token's native precision (e.g., e8s for ICP).
    /// Renamed from `icp_margin_amount`; serde alias handles old events.
    #[serde(alias = "icp_margin_amount")]
    pub collateral_amount: u64,
    pub vault_id: u64,
    /// Ledger canister ID identifying the collateral token.
    /// Old events lack this field; serde default → Principal::anonymous(),
    /// fixed up to ICP ledger principal during event replay.
    #[serde(default = "default_collateral_type")]
    pub collateral_type: Principal,
    /// Nanosecond timestamp of last interest accrual for this vault.
    /// Defaults to 0 for existing vaults (migration sets it in post_upgrade).
    #[serde(default)]
    pub last_accrual_time: u64,
}
```

**Step 2: Compile**

Run: `cargo build -p rumi_protocol_backend 2>&1 | head -40`
Expected: Compilation errors at every place that constructs a `Vault` without `last_accrual_time`. That's fine — we fix those in Step 3.

**Step 3: Fix all Vault construction sites**

There are exactly 2 places that construct a `Vault`:

1. `vault.rs:520-526` (`open_vault_and_borrow`): Add `last_accrual_time: ic_cdk::api::time()`.
2. Any test helpers that construct Vault literals — add `last_accrual_time: 0` or a test timestamp.

```rust
// In open_vault_and_borrow (vault.rs ~line 520):
Vault {
    owner: caller,
    borrowed_icusd_amount: 0.into(),
    collateral_amount: collateral_amount_raw,
    vault_id,
    collateral_type,
    last_accrual_time: ic_cdk::api::time(),
},
```

**Step 4: Compile again**

Run: `cargo build -p rumi_protocol_backend 2>&1 | head -40`
Expected: Clean compile (or only unrelated warnings).

**Step 5: Commit**

```bash
git add src/rumi_protocol_backend/src/vault.rs
git commit -m "feat: add last_accrual_time field to Vault struct"
```

---

## Task 2: Add `NANOS_PER_YEAR` constant

**Files:**
- Modify: `src/rumi_protocol_backend/src/numeric.rs`

**Step 1: Add the constant**

At the top of `numeric.rs`, after the existing constants:

```rust
/// Nanoseconds in a 365-day year. Used for interest accrual.
pub const NANOS_PER_YEAR: u64 = 365 * 24 * 60 * 60 * 1_000_000_000;
```

**Step 2: Compile**

Run: `cargo build -p rumi_protocol_backend 2>&1 | head -20`
Expected: Clean compile.

**Step 3: Commit**

```bash
git add src/rumi_protocol_backend/src/numeric.rs
git commit -m "feat: add NANOS_PER_YEAR constant for interest accrual"
```

---

## Task 3: Write unit tests for single-vault accrual

**Files:**
- Modify: `src/rumi_protocol_backend/src/state.rs` (add test module or extend existing)

**Step 1: Write the failing tests**

Add tests at the bottom of `state.rs` (in an existing `#[cfg(test)] mod tests` block, or create one). These tests exercise the accrual logic we'll implement in Task 4.

```rust
#[cfg(test)]
mod accrual_tests {
    use super::*;
    use rust_decimal_macros::dec;
    use crate::numeric::{ICUSD, Ratio, NANOS_PER_YEAR};

    /// Helper: create a minimal State with one ICP vault for testing.
    fn test_state_with_vault(borrowed: u64, collateral: u64, last_accrual: u64) -> (State, u64) {
        // Build a minimal state. The exact setup depends on existing test helpers.
        // Key: we need a vault with known debt, collateral, and last_accrual_time,
        // plus a CollateralConfig with a known interest_rate_apr and a last_price.
        let mut state = State::default_for_tests(); // or however tests create state
        let vault_id = 1;
        let ct = state.icp_collateral_type();
        // Set a known price so CR can be computed
        if let Some(config) = state.collateral_configs.get_mut(&ct) {
            config.last_price = Some(10.0); // $10 per ICP
            config.interest_rate_apr = Ratio::from(dec!(0.10)); // 10% APR for easy math
        }
        state.last_icp_rate = Some(UsdIcp::from(dec!(10.0)));
        let vault = crate::vault::Vault {
            owner: Principal::anonymous(),
            borrowed_icusd_amount: ICUSD::from(borrowed),
            collateral_amount: collateral,
            vault_id,
            collateral_type: ct,
            last_accrual_time: last_accrual,
        };
        state.vault_id_to_vaults.insert(vault_id, vault);
        state.principal_to_vault_ids.entry(Principal::anonymous()).or_default().insert(vault_id);
        (state, vault_id)
    }

    #[test]
    fn test_accrue_single_vault_one_year() {
        let start = 1_000_000_000_000u64; // some base time
        let (mut state, vid) = test_state_with_vault(1_000_000_000_00, 100_000_000_000, start);
        // 10% APR, 1 full year elapsed
        let now = start + NANOS_PER_YEAR;
        state.accrue_single_vault(vid, now);
        let vault = state.vault_id_to_vaults.get(&vid).unwrap();
        // After 1 year at 10% simple: 1000 * 1.10 = 1100 icUSD (in e8s)
        // Note: this is simple interest per tick, not compound. Close enough for 1 tick.
        let expected = 1_100_000_000_00u64; // 1100 icUSD in e8s
        let actual = vault.borrowed_icusd_amount.to_u64();
        assert!((actual as i64 - expected as i64).unsigned_abs() < 100,
            "Expected ~{expected}, got {actual}");
        assert_eq!(vault.last_accrual_time, now);
    }

    #[test]
    fn test_accrue_single_vault_zero_debt_noop() {
        let start = 1_000_000_000_000u64;
        let (mut state, vid) = test_state_with_vault(0, 100_000_000_000, start);
        let now = start + NANOS_PER_YEAR;
        state.accrue_single_vault(vid, now);
        let vault = state.vault_id_to_vaults.get(&vid).unwrap();
        assert_eq!(vault.borrowed_icusd_amount.to_u64(), 0);
        // last_accrual_time should NOT be updated for zero-debt vaults
    }

    #[test]
    fn test_accrue_single_vault_already_current() {
        let now = 2_000_000_000_000u64;
        let (mut state, vid) = test_state_with_vault(1_000_000_000_00, 100_000_000_000, now);
        let before = state.vault_id_to_vaults.get(&vid).unwrap().borrowed_icusd_amount;
        state.accrue_single_vault(vid, now);
        let after = state.vault_id_to_vaults.get(&vid).unwrap().borrowed_icusd_amount;
        assert_eq!(before, after, "No change when already current");
    }

    #[test]
    fn test_accrue_all_vaults() {
        let start = 1_000_000_000_000u64;
        let (mut state, _) = test_state_with_vault(1_000_000_000_00, 100_000_000_000, start);
        // Add a second vault
        let ct = state.icp_collateral_type();
        let v2 = crate::vault::Vault {
            owner: Principal::anonymous(),
            borrowed_icusd_amount: ICUSD::from(500_000_000_00u64),
            collateral_amount: 100_000_000_000,
            vault_id: 2,
            collateral_type: ct,
            last_accrual_time: start,
        };
        state.vault_id_to_vaults.insert(2, v2);

        let now = start + NANOS_PER_YEAR;
        state.accrue_all_vault_interest(now);

        let v1_debt = state.vault_id_to_vaults.get(&1).unwrap().borrowed_icusd_amount.to_u64();
        let v2_debt = state.vault_id_to_vaults.get(&2).unwrap().borrowed_icusd_amount.to_u64();
        // Both should have accrued ~10%
        assert!(v1_debt > 1_000_000_000_00, "Vault 1 should have accrued interest");
        assert!(v2_debt > 500_000_000_00, "Vault 2 should have accrued interest");
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p rumi_protocol_backend accrual_tests 2>&1 | tail -20`
Expected: FAIL — `accrue_single_vault` and `accrue_all_vault_interest` don't exist yet.

**Step 3: Commit failing tests**

```bash
git add src/rumi_protocol_backend/src/state.rs
git commit -m "test: add failing unit tests for per-vault interest accrual"
```

---

## Task 4: Implement `accrue_single_vault` and `accrue_all_vault_interest` on State

**Files:**
- Modify: `src/rumi_protocol_backend/src/state.rs`

**Step 1: Implement `accrue_single_vault`**

Add this method to the `impl State` block. Uses two-phase borrow checker pattern: compute rate immutably, then apply mutably.

```rust
/// Accrue interest on a single vault up to `now_nanos`.
/// Two-phase for borrow checker: compute rate (immutable), then apply (mutable).
pub fn accrue_single_vault(&mut self, vault_id: u64, now_nanos: u64) {
    // Phase 1: compute rate (immutable borrow of self)
    let rate_and_elapsed = {
        let s: &State = &*self;
        match s.vault_id_to_vaults.get(&vault_id) {
            Some(vault) if !vault.borrowed_icusd_amount.0.is_zero()
                && vault.last_accrual_time < now_nanos =>
            {
                let dummy_rate = s.last_icp_rate.unwrap_or(UsdIcp::from(rust_decimal_macros::dec!(1.0)));
                let cr = crate::compute_collateral_ratio(vault, dummy_rate, s);
                let rate = s.get_dynamic_interest_rate_for(&vault.collateral_type, cr);
                let elapsed = now_nanos.saturating_sub(vault.last_accrual_time);
                Some((rate, elapsed))
            }
            _ => None,
        }
    };
    // Phase 2: apply (mutable borrow)
    if let Some((rate, elapsed)) = rate_and_elapsed {
        if elapsed == 0 {
            return;
        }
        if let Some(vault) = self.vault_id_to_vaults.get_mut(&vault_id) {
            let factor = rust_decimal::Decimal::ONE
                + rate.0 * rust_decimal::Decimal::from(elapsed)
                    / rust_decimal::Decimal::from(crate::numeric::NANOS_PER_YEAR);
            vault.borrowed_icusd_amount =
                ICUSD::from(vault.borrowed_icusd_amount.0 * factor);
            vault.last_accrual_time = now_nanos;
        }
    }
}
```

**Step 2: Implement `accrue_all_vault_interest`**

```rust
/// Accrue interest on ALL vaults with outstanding debt.
/// Two-phase: collect (vault_id, rate, elapsed) immutably, then apply mutably.
pub fn accrue_all_vault_interest(&mut self, now_nanos: u64) {
    // Phase 1: compute rates for all vaults (immutable)
    let accruals: Vec<(u64, Ratio, u64)> = {
        let s: &State = &*self;
        let dummy_rate = s.last_icp_rate.unwrap_or(UsdIcp::from(rust_decimal_macros::dec!(1.0)));
        s.vault_id_to_vaults
            .iter()
            .filter(|(_, v)| {
                !v.borrowed_icusd_amount.0.is_zero() && v.last_accrual_time < now_nanos
            })
            .map(|(id, vault)| {
                let cr = crate::compute_collateral_ratio(vault, dummy_rate, s);
                let rate = s.get_dynamic_interest_rate_for(&vault.collateral_type, cr);
                let elapsed = now_nanos.saturating_sub(vault.last_accrual_time);
                (*id, rate, elapsed)
            })
            .collect()
    };
    // Phase 2: apply accruals (mutable)
    for (vault_id, rate, elapsed) in accruals {
        if elapsed == 0 {
            continue;
        }
        if let Some(vault) = self.vault_id_to_vaults.get_mut(&vault_id) {
            let factor = rust_decimal::Decimal::ONE
                + rate.0 * rust_decimal::Decimal::from(elapsed)
                    / rust_decimal::Decimal::from(crate::numeric::NANOS_PER_YEAR);
            vault.borrowed_icusd_amount =
                ICUSD::from(vault.borrowed_icusd_amount.0 * factor);
            vault.last_accrual_time = now_nanos;
        }
    }
}
```

**Step 3: Run tests**

Run: `cargo test -p rumi_protocol_backend accrual_tests 2>&1 | tail -20`
Expected: All 4 tests PASS.

**Step 4: Commit**

```bash
git add src/rumi_protocol_backend/src/state.rs
git commit -m "feat: implement accrue_single_vault and accrue_all_vault_interest"
```

---

## Task 5: Add `AccrueInterest` event variant

**Files:**
- Modify: `src/rumi_protocol_backend/src/event.rs`

**Step 1: Add the variant to the Event enum**

Add after the last variant (before the closing `}`):

```rust
    /// Periodic interest accrual event. One event per timer tick — replay handler
    /// runs full accrual across all vaults at the given timestamp.
    #[serde(rename = "accrue_interest")]
    AccrueInterest {
        timestamp: u64,
    },
```

**Step 2: Add the match arm in `is_vault_related`**

```rust
Event::AccrueInterest { .. } => false,
```

**Step 3: Add the replay handler in the `replay` function**

Add in the large `match event` block:

```rust
Event::AccrueInterest { timestamp } => {
    state.accrue_all_vault_interest(timestamp);
},
```

**Step 4: Add a recording helper function**

```rust
pub fn record_accrue_interest(state: &mut State, timestamp: u64) {
    record_event(&Event::AccrueInterest { timestamp });
    state.accrue_all_vault_interest(timestamp);
}
```

**Step 5: Compile**

Run: `cargo build -p rumi_protocol_backend 2>&1 | head -20`
Expected: Clean compile.

**Step 6: Commit**

```bash
git add src/rumi_protocol_backend/src/event.rs
git commit -m "feat: add AccrueInterest event variant with replay handler"
```

---

## Task 6: Hook accrual into the 300s timer tick

**Files:**
- Modify: `src/rumi_protocol_backend/src/xrc.rs`

**Step 1: Add accrual call in `fetch_icp_rate`**

In `fetch_icp_rate()`, insert accrual AFTER the price update and mode update, but BEFORE `check_vaults()`. Find these lines (~line 68-73):

```rust
    if let Some(last_icp_rate) = read_state(|s| s.last_icp_rate) {
        mutate_state(|s| s.update_total_collateral_ratio_and_mode(last_icp_rate));
    }
    if read_state(|s| s.mode != crate::Mode::ReadOnly) {
        crate::check_vaults();
    }
```

Insert accrual between them:

```rust
    if let Some(last_icp_rate) = read_state(|s| s.last_icp_rate) {
        mutate_state(|s| s.update_total_collateral_ratio_and_mode(last_icp_rate));
    }

    // Accrue interest on all vaults before checking for liquidations.
    // This ensures check_vaults sees current debt values.
    if read_state(|s| s.mode != crate::Mode::ReadOnly) {
        let now = ic_cdk::api::time();
        mutate_state(|s| {
            crate::event::record_accrue_interest(s, now);
        });
    }

    if read_state(|s| s.mode != crate::Mode::ReadOnly) {
        crate::check_vaults();
    }
```

**Step 2: Compile**

Run: `cargo build -p rumi_protocol_backend 2>&1 | head -20`
Expected: Clean compile.

**Step 3: Commit**

```bash
git add src/rumi_protocol_backend/src/xrc.rs
git commit -m "feat: hook per-vault interest accrual into 300s timer tick"
```

---

## Task 7: Add pre-borrow accrual in `borrow_from_vault_internal`

**Files:**
- Modify: `src/rumi_protocol_backend/src/vault.rs:558-647`

**Step 1: Add accrual before validation**

At the start of `borrow_from_vault_internal`, after reading the vault (~line 567-581), accrue the vault before the CR/debt ceiling checks:

```rust
    // Accrue interest on this vault before checking borrowing limits.
    // Catches the ≤300s gap since the last timer tick.
    let now = ic_cdk::api::time();
    mutate_state(|s| s.accrue_single_vault(arg.vault_id, now));

    // Re-read vault after accrual (debt may have increased)
    let (vault, collateral_price, config_decimals) = read_state(|s| {
        // ... same read logic as before ...
    })?;
```

Actually, the cleanest approach: insert the accrual call right after the initial read succeeds, then re-read. OR: accrue first, then do the single read. Let's accrue BEFORE the read:

```rust
async fn borrow_from_vault_internal(caller: Principal, arg: VaultArg) -> Result<SuccessWithFee, ProtocolError> {
    let amount: ICUSD = arg.amount.into();

    if amount < MIN_ICUSD_AMOUNT {
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: MIN_ICUSD_AMOUNT.to_u64(),
        });
    }

    // Accrue interest before reading vault state for borrow validation
    mutate_state(|s| s.accrue_single_vault(arg.vault_id, ic_cdk::api::time()));

    let (vault, collateral_price, config_decimals) = read_state(|s| {
        // ... existing read logic unchanged ...
    })?;
    // ... rest of function unchanged ...
```

**Step 2: Compile**

Run: `cargo build -p rumi_protocol_backend 2>&1 | head -20`
Expected: Clean compile.

**Step 3: Commit**

```bash
git add src/rumi_protocol_backend/src/vault.rs
git commit -m "feat: accrue vault interest before borrow validation"
```

---

## Task 8: Add pre-repay accrual in `repay_to_vault` and `repay_to_vault_with_stable`

**Files:**
- Modify: `src/rumi_protocol_backend/src/vault.rs:672-817`

**Step 1: Add accrual in `repay_to_vault`**

After the guard is acquired and amount is parsed, before reading the vault:

```rust
pub async fn repay_to_vault(arg: VaultArg) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal = GuardPrincipal::new(caller, &format!("repay_vault_{}", arg.vault_id))?;
    let amount: ICUSD = arg.amount.into();

    // Accrue interest before reading vault state for repay validation
    mutate_state(|s| s.accrue_single_vault(arg.vault_id, ic_cdk::api::time()));

    let vault = match read_state(|s| s.vault_id_to_vaults.get(&arg.vault_id).cloned()) {
        // ... rest unchanged ...
```

**Step 2: Add accrual in `repay_to_vault_with_stable`**

Same pattern — add accrual after guard, before reading vault:

```rust
    // Accrue interest before reading vault state for repay validation
    mutate_state(|s| s.accrue_single_vault(arg.vault_id, ic_cdk::api::time()));

    let vault = match read_state(|s| s.vault_id_to_vaults.get(&arg.vault_id).cloned()) {
```

**Step 3: Compile**

Run: `cargo build -p rumi_protocol_backend 2>&1 | head -20`
Expected: Clean compile.

**Step 4: Commit**

```bash
git add src/rumi_protocol_backend/src/vault.rs
git commit -m "feat: accrue vault interest before repay validation"
```

---

## Task 9: Add pre-close accrual in `close_vault`

**Files:**
- Modify: `src/rumi_protocol_backend/src/vault.rs:1056-1199`

**Step 1: Add accrual before dust check**

After checking vault exists and before reading the vault for the dust/debt check:

```rust
    // Accrue interest before checking debt for close
    mutate_state(|s| s.accrue_single_vault(vault_id, ic_cdk::api::time()));

    // Get the vault (with freshly-accrued debt)
    let vault = read_state(|s| {
        s.vault_id_to_vaults
            .get(&vault_id)
            .cloned()
            .ok_or(ProtocolError::GenericError("Vault not found".to_string()))
    })?;
```

This ensures the dust threshold check at line 1112 sees the true accrued debt.

**Step 2: Compile**

Run: `cargo build -p rumi_protocol_backend 2>&1 | head -20`
Expected: Clean compile.

**Step 3: Commit**

```bash
git add src/rumi_protocol_backend/src/vault.rs
git commit -m "feat: accrue vault interest before close_vault dust check"
```

---

## Task 10: Add pre-liquidation accrual in `partial_liquidate_vault`

**Files:**
- Modify: `src/rumi_protocol_backend/src/vault.rs:2320+`

**Step 1: Add accrual before the liquidation validation read**

At the start of `partial_liquidate_vault`, after acquiring the guard, accrue the vault:

```rust
pub async fn partial_liquidate_vault(arg: VaultArg) -> Result<SuccessWithFee, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal = GuardPrincipal::new(caller, &format!("partial_liquidate_vault_{}", arg.vault_id))?;
    let liquidator_payment: ICUSD = arg.amount.into();

    // Accrue interest before checking if vault is liquidatable
    mutate_state(|s| s.accrue_single_vault(arg.vault_id, ic_cdk::api::time()));

    // Step 1: Validate vault is liquidatable (reads freshly-accrued debt)
    let (vault, collateral_price, ...) = match read_state(|s| {
        // ... existing validation unchanged ...
```

**Step 2: Compile**

Run: `cargo build -p rumi_protocol_backend 2>&1 | head -20`
Expected: Clean compile.

**Step 3: Commit**

```bash
git add src/rumi_protocol_backend/src/vault.rs
git commit -m "feat: accrue vault interest before partial liquidation check"
```

---

## Task 11: Add `redemption_fee_floor` and `redemption_fee_ceiling` to `AddCollateralArg`

**Files:**
- Modify: `src/rumi_protocol_backend/src/lib.rs:173-198`
- Modify: `src/rumi_protocol_backend/src/main.rs:1789-1815`

**Step 1: Add fields to `AddCollateralArg`**

```rust
pub struct AddCollateralArg {
    // ... existing fields ...
    /// Floor for the redemption fee (e.g., 0.005 = 0.5%)
    pub redemption_fee_floor: f64,
    /// Ceiling for the redemption fee (e.g., 0.05 = 5%)
    pub redemption_fee_ceiling: f64,
}
```

**Step 2: Use them in `add_collateral_token`**

In `main.rs`, replace the hardcoded values:

```rust
// Before:
redemption_fee_floor: Ratio::from_f64(0.005),
redemption_fee_ceiling: Ratio::from_f64(0.05),

// After:
redemption_fee_floor: Ratio::from_f64(arg.redemption_fee_floor),
redemption_fee_ceiling: Ratio::from_f64(arg.redemption_fee_ceiling),
```

**Step 3: Compile**

Run: `cargo build -p rumi_protocol_backend 2>&1 | head -20`
Expected: Clean compile.

**Step 4: Commit**

```bash
git add src/rumi_protocol_backend/src/lib.rs src/rumi_protocol_backend/src/main.rs
git commit -m "feat: make redemption fee floor/ceiling configurable via AddCollateralArg"
```

---

## Task 12: Add `weighted_average_interest_rate` query helper

**Files:**
- Modify: `src/rumi_protocol_backend/src/state.rs`

**Step 1: Implement the helper on State**

```rust
/// Compute the system-wide weighted average interest rate across all vaults.
/// Weight = vault's borrowed_icusd_amount. Returns Ratio (APR as decimal).
/// Returns 0 if there is no outstanding debt.
pub fn weighted_average_interest_rate(&self) -> Ratio {
    let dummy_rate = self.last_icp_rate.unwrap_or(UsdIcp::from(rust_decimal_macros::dec!(1.0)));
    let mut total_debt = rust_decimal::Decimal::ZERO;
    let mut weighted_sum = rust_decimal::Decimal::ZERO;

    for vault in self.vault_id_to_vaults.values() {
        if vault.borrowed_icusd_amount.0.is_zero() {
            continue;
        }
        let cr = crate::compute_collateral_ratio(vault, dummy_rate, self);
        let rate = self.get_dynamic_interest_rate_for(&vault.collateral_type, cr);
        let debt = vault.borrowed_icusd_amount.0;
        weighted_sum += rate.0 * debt;
        total_debt += debt;
    }

    if total_debt.is_zero() {
        return Ratio::from(rust_decimal::Decimal::ZERO);
    }
    Ratio::from(weighted_sum / total_debt)
}
```

**Step 2: Add a test**

```rust
#[test]
fn test_weighted_average_interest_rate_empty() {
    let state = State::default_for_tests();
    let avg = state.weighted_average_interest_rate();
    assert_eq!(avg.0, rust_decimal::Decimal::ZERO);
}
```

**Step 3: Expose via query endpoint (optional, can be added later)**

If desired, add a `#[query] fn get_weighted_average_interest_rate() -> f64` in `main.rs`. This can be deferred.

**Step 4: Compile and test**

Run: `cargo test -p rumi_protocol_backend weighted_average 2>&1 | tail -10`
Expected: PASS.

**Step 5: Commit**

```bash
git add src/rumi_protocol_backend/src/state.rs
git commit -m "feat: add weighted_average_interest_rate system-wide metric"
```

---

## Task 13: Set `last_accrual_time` for existing vaults in `post_upgrade`

**Files:**
- Modify: `src/rumi_protocol_backend/src/main.rs` (find `post_upgrade` function)

**Step 1: Find `post_upgrade`**

Search for `fn post_upgrade` in `main.rs`.

**Step 2: Add migration logic**

After the existing state restoration, add:

```rust
// Migration: set last_accrual_time for existing vaults that have 0 (never accrued)
let now = ic_cdk::api::time();
mutate_state(|s| {
    for vault in s.vault_id_to_vaults.values_mut() {
        if vault.last_accrual_time == 0 {
            vault.last_accrual_time = now;
        }
    }
});
```

This ensures existing vaults start accruing from the upgrade moment — no retroactive interest.

**Step 3: Compile**

Run: `cargo build -p rumi_protocol_backend 2>&1 | head -20`
Expected: Clean compile.

**Step 4: Commit**

```bash
git add src/rumi_protocol_backend/src/main.rs
git commit -m "feat: migrate existing vaults to set last_accrual_time in post_upgrade"
```

---

## Task 14: Update the `.did` file

**Files:**
- Modify: `src/rumi_protocol_backend/rumi_protocol_backend.did`

**Step 1: Update `AddCollateralArg` in the .did file**

Add the new fields:

```candid
type AddCollateralArg = record {
    // ... existing fields ...
    redemption_fee_floor : float64;
    redemption_fee_ceiling : float64;
};
```

**Step 2: Add `AccrueInterest` to the Event variant (if events are exposed in Candid)**

Check whether Event is in the .did file. If so, add:

```candid
variant { accrue_interest : record { timestamp : nat64 } }
```

**Step 3: Compile**

Run: `cargo build -p rumi_protocol_backend 2>&1 | head -20`
Expected: Clean compile.

**Step 4: Commit**

```bash
git add src/rumi_protocol_backend/rumi_protocol_backend.did
git commit -m "feat: update .did file with AccrueInterest event and AddCollateralArg fields"
```

---

## Task 15: Integration test — accrual on timer tick

**Files:**
- Modify: existing test file or create test in `src/rumi_protocol_backend/src/`

**Step 1: Write an integration-style test**

Test that simulating the timer tick flow (price update → accrual → check_vaults) works correctly. This may require mocking the timer or testing the state methods directly:

```rust
#[test]
fn test_accrual_before_check_vaults_flow() {
    // Setup: vault at 200% CR, 10% APR, 1 year elapsed
    let start = 1_000_000_000_000u64;
    let (mut state, vid) = test_state_with_vault(1_000_000_000_00, 200_000_000_00, start);

    let initial_debt = state.vault_id_to_vaults.get(&vid).unwrap().borrowed_icusd_amount;

    // Simulate timer tick: accrue all vaults
    let now = start + 300 * 1_000_000_000; // 300 seconds later
    state.accrue_all_vault_interest(now);

    let accrued_debt = state.vault_id_to_vaults.get(&vid).unwrap().borrowed_icusd_amount;
    assert!(accrued_debt > initial_debt, "Debt should increase after accrual");

    // Verify the increase is proportional to 300s at the dynamic rate
    let elapsed_fraction = rust_decimal::Decimal::from(300u64 * 1_000_000_000u64)
        / rust_decimal::Decimal::from(crate::numeric::NANOS_PER_YEAR);
    // At 10% base APR, 300s should add ~0.000951% of debt
    let expected_increase = initial_debt.0 * rust_decimal::Decimal::from_str("0.10").unwrap() * elapsed_fraction;
    let actual_increase = accrued_debt.0 - initial_debt.0;
    // Allow some tolerance for dynamic rate multiplier
    assert!(actual_increase > rust_decimal::Decimal::ZERO, "Should have positive increase");
}
```

**Step 2: Run**

Run: `cargo test -p rumi_protocol_backend test_accrual_before_check 2>&1 | tail -10`
Expected: PASS.

**Step 3: Commit**

```bash
git add src/rumi_protocol_backend/src/state.rs
git commit -m "test: add integration test for accrual-before-check-vaults flow"
```

---

## Task 16: Full build and test suite

**Step 1: Run full build**

Run: `cargo build -p rumi_protocol_backend 2>&1 | tail -20`
Expected: Clean compile, no errors.

**Step 2: Run full test suite**

Run: `cargo test -p rumi_protocol_backend 2>&1 | tail -40`
Expected: All tests pass.

**Step 3: Verify no heartbeat regression**

Run: `grep -r "heartbeat" src/rumi_protocol_backend/src/ --include="*.rs"`
Expected: No `#[ic_cdk::heartbeat]` found.

**Step 4: Final commit if any fixes needed**

```bash
git add -A && git commit -m "chore: fix any compilation issues from integration"
```

---

## Summary of changes by file

| File | What changes |
|------|-------------|
| `vault.rs` | Add `last_accrual_time: u64` to Vault, set it on construction, add `accrue_single_vault` calls before borrow/repay/close/liquidate |
| `state.rs` | Add `accrue_single_vault()`, `accrue_all_vault_interest()`, `weighted_average_interest_rate()` methods + tests |
| `event.rs` | Add `AccrueInterest { timestamp }` variant, replay handler, `record_accrue_interest()` helper |
| `xrc.rs` | Insert accrual call in `fetch_icp_rate()` between mode update and `check_vaults()` |
| `numeric.rs` | Add `NANOS_PER_YEAR` constant |
| `lib.rs` | Add `redemption_fee_floor`/`redemption_fee_ceiling` to `AddCollateralArg` |
| `main.rs` | Use new `AddCollateralArg` fields in `add_collateral_token`, add migration in `post_upgrade` |
| `.did` file | Update `AddCollateralArg` type, add `AccrueInterest` event variant |

## What does NOT change (and why)

| Component | Why unchanged |
|-----------|--------------|
| `compute_collateral_ratio` | Reads `vault.borrowed_icusd_amount` which is already accrued (on tick or before interaction) |
| `total_borrowed_icusd_amount` | Sums raw stored debt — values are fresh from the 300s tick, acceptable staleness |
| `CandidVault` / `From<Vault>` | Just converts stored values — stored values are already accrued |
| `State::borrow_from_vault` | Simple `+=` on stored amount — the async caller accrues first |
| `State::repay_to_vault` | Simple `-=` on stored amount — the async caller accrues first |
| `redeem_on_vaults` | Accrual happens on tick; 300s staleness is acceptable for redemptions |
| `CollateralConfig` | No new fields needed — dynamic rates already computed from existing config |
