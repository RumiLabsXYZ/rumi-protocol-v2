# Treasury Fee Routing Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Route all protocol fees (borrowing fees and interest revenue) to the treasury canister as real, spendable icUSD.

**Architecture:** Add an `accrued_interest` field to each Vault to track how much of the debt is interest vs principal. On any debt reduction (repay, partial repay, liquidation), compute the interest share of the payment and mint that amount to the treasury. Replace the borrowing fee's current phantom-liquidity-pool routing with a real icUSD mint to treasury.

**Tech Stack:** Rust (IC canister), Svelte (frontend), Candid (interface)

---

## Context for Implementer

### Current fee flows (broken)

| Fee | Current behavior | Problem |
|-----|-----------------|---------|
| Borrowing fee | `mint_icusd(amount - fee, user)` then `provide_liquidity(fee, developer)` | Fee is phantom — never minted. Developer gets a liquidity pool credit for tokens that don't exist. |
| Interest on repay | `transfer_icusd_from(full_debt, user)` → canister holds it, `repay_to_vault` reduces debt | Interest portion is dead icUSD stuck in the canister. No revenue. |
| Interest on liquidation | Liquidator pays full debt (incl. interest) via `transfer_icusd_from` | Same — interest portion is dead. |

### Target fee flows

| Fee | New behavior |
|-----|-------------|
| Borrowing fee | `mint_icusd(amount, user)` (full amount). Separately `mint_icusd(fee, treasury)`. Vault debt = `amount`. |
| Interest on repay/partial repay | Compute interest share of payment. `mint_icusd(interest_share, treasury)`. |
| Interest on liquidation | Same split — interest share minted to treasury. |

### Key files

- `src/rumi_protocol_backend/src/vault.rs` — Vault struct (line 48), borrow/repay/liquidation logic
- `src/rumi_protocol_backend/src/state.rs` — `accrue_single_vault` (line 1103), `accrue_all_vault_interest` (line 1142), `repay_to_vault` (line 1475), `borrow_from_vault` (line 1447)
- `src/rumi_protocol_backend/src/event.rs` — `record_borrow_from_vault` (line 778), `record_repayed_to_vault` (line 795)
- `src/rumi_protocol_backend/src/management.rs` — `mint_icusd` (line 266), `transfer_icusd` (line 337)
- `src/rumi_protocol_backend/src/main.rs` — `set_treasury_principal` (line 980), `get_treasury_principal` (line 999)
- `src/vault_frontend/src/lib/components/vault/VaultCard.svelte` — live-ticking debt display (line 119)
- `src/vault_frontend/src/lib/stores/collateralStore.ts` — collateral config fetch

### Important constraints

- **NEVER reinstall the backend canister** — upgrades only. The new `accrued_interest` field must default via `#[serde(default)]` for backward compatibility with existing vaults.
- `treasury_principal` is already set on-chain. Use `read_state(|s| s.treasury_principal)` to get it.
- `mint_icusd` is an async ICRC-1 transfer — it can fail. Treasury mints are non-critical: if the mint fails, log the error and continue (don't block the user's repay/borrow).
- Interest accrual already runs in `accrue_single_vault` and `accrue_all_vault_interest`. The delta tracking hooks into the existing multiplier math.
- Existing tests in `state.rs` (line 2106+) use `accrual_test_state()` helper. New tests should follow the same pattern.

---

### Task 1: Add `accrued_interest` field to Vault struct

**Files:**
- Modify: `src/rumi_protocol_backend/src/vault.rs:48-65` (Vault struct)
- Modify: `src/rumi_protocol_backend/src/vault.rs:68-91` (CandidVault struct + From impl)

**Step 1: Add the field to Vault**

In `vault.rs`, add `accrued_interest` to the `Vault` struct (after `last_accrual_time`):

```rust
/// Accumulated interest on this vault's debt.
/// Sub-component of `borrowed_icusd_amount` — tracks how much is interest vs principal.
/// Defaults to 0 for existing vaults (backward compat).
#[serde(default)]
pub accrued_interest: ICUSD,
```

**Step 2: Add the field to CandidVault and From impl**

In the `CandidVault` struct, add:

```rust
pub accrued_interest: u64,
```

In the `From<Vault> for CandidVault` impl, add:

```rust
accrued_interest: vault.accrued_interest.to_u64(),
```

**Step 3: Update all Vault construction sites in tests**

Every test that constructs a `Vault { ... }` needs `accrued_interest: ICUSD::new(0)`. There are ~12 instances in `state.rs` tests (lines 2113-2547). Use find-and-replace: after every `last_accrual_time: 0,` in test code, add `accrued_interest: ICUSD::new(0),`. Also in `state.rs` non-test code where Vaults are constructed (e.g., `open_vault` event replay in `event.rs`).

Search all files for `last_accrual_time:` and add the new field after each occurrence.

**Step 4: Run tests to verify compilation**

Run: `cargo test -p rumi_protocol_backend 2>&1 | tail -20`
Expected: All existing tests pass. No compilation errors.

**Step 5: Commit**

```bash
git add src/rumi_protocol_backend/src/vault.rs src/rumi_protocol_backend/src/state.rs src/rumi_protocol_backend/src/event.rs
git commit -m "feat: add accrued_interest field to Vault struct"
```

---

### Task 2: Track interest delta during accrual

**Files:**
- Modify: `src/rumi_protocol_backend/src/state.rs:1103-1138` (`accrue_single_vault`)
- Modify: `src/rumi_protocol_backend/src/state.rs:1142-1178` (`accrue_all_vault_interest`)

**Step 1: Write failing test — single vault interest tracking**

Add to `state.rs` tests (after existing accrual tests, ~line 2570):

```rust
#[test]
fn test_accrue_single_vault_tracks_accrued_interest() {
    let mut state = accrual_test_state();
    let icp = state.icp_ledger_principal;

    // Vault: 1.5 ICP, 5 icUSD debt, CR=300% → multiplier 1.0x, rate 5%
    state.vault_id_to_vaults.insert(1, Vault {
        owner: Principal::anonymous(),
        vault_id: 1,
        collateral_amount: 150_000_000,
        borrowed_icusd_amount: ICUSD::new(500_000_000),
        collateral_type: icp,
        last_accrual_time: 0,
        accrued_interest: ICUSD::new(0),
    });

    let one_year_nanos = crate::numeric::NANOS_PER_YEAR;
    state.accrue_single_vault(1, one_year_nanos);

    let vault = state.vault_id_to_vaults.get(&1).unwrap();
    // debt should be 525M (500M * 1.05)
    assert_eq!(vault.borrowed_icusd_amount.0, 525_000_000);
    // accrued_interest should be 25M (the delta)
    assert_eq!(vault.accrued_interest.0, 25_000_000,
        "accrued_interest should track the 25M interest delta, got {}", vault.accrued_interest.0);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p rumi_protocol_backend test_accrue_single_vault_tracks_accrued_interest -- --nocapture 2>&1 | tail -10`
Expected: FAIL — `accrued_interest` is still 0 because the accrual logic doesn't update it yet.

**Step 3: Update `accrue_single_vault` to track the delta**

In `state.rs`, in `accrue_single_vault`, replace the Phase 2 block (lines ~1124-1137):

```rust
// Phase 2: apply (mutable borrow)
if let Some((rate, elapsed)) = rate_and_elapsed {
    if elapsed == 0 {
        return;
    }
    if let Some(vault) = self.vault_id_to_vaults.get_mut(&vault_id) {
        let debt = Decimal::from(vault.borrowed_icusd_amount.0);
        let factor = Decimal::ONE
            + rate.0 * Decimal::from(elapsed)
                / Decimal::from(crate::numeric::NANOS_PER_YEAR);
        let new_debt = (debt * factor).to_u64().unwrap_or(vault.borrowed_icusd_amount.0);
        let interest_delta = new_debt.saturating_sub(vault.borrowed_icusd_amount.0);
        vault.borrowed_icusd_amount = ICUSD::from(new_debt);
        vault.accrued_interest += ICUSD::from(interest_delta);
        vault.last_accrual_time = now_nanos;
    }
}
```

The only change is adding 2 lines: computing `interest_delta` and adding it to `vault.accrued_interest`.

**Step 4: Update `accrue_all_vault_interest` with the same delta tracking**

In the Phase 2 loop (lines ~1164-1177), apply the same pattern:

```rust
for (vault_id, rate, elapsed) in accruals {
    if elapsed == 0 {
        continue;
    }
    if let Some(vault) = self.vault_id_to_vaults.get_mut(&vault_id) {
        let debt = Decimal::from(vault.borrowed_icusd_amount.0);
        let factor = Decimal::ONE
            + rate.0 * Decimal::from(elapsed)
                / Decimal::from(crate::numeric::NANOS_PER_YEAR);
        let new_debt = (debt * factor).to_u64().unwrap_or(vault.borrowed_icusd_amount.0);
        let interest_delta = new_debt.saturating_sub(vault.borrowed_icusd_amount.0);
        vault.borrowed_icusd_amount = ICUSD::from(new_debt);
        vault.accrued_interest += ICUSD::from(interest_delta);
        vault.last_accrual_time = now_nanos;
    }
}
```

**Step 5: Run test to verify it passes**

Run: `cargo test -p rumi_protocol_backend test_accrue_single_vault_tracks_accrued_interest -- --nocapture 2>&1 | tail -10`
Expected: PASS

**Step 6: Run all tests**

Run: `cargo test -p rumi_protocol_backend 2>&1 | tail -20`
Expected: All tests pass.

**Step 7: Commit**

```bash
git add src/rumi_protocol_backend/src/state.rs
git commit -m "feat: track accrued_interest delta during interest accrual"
```

---

### Task 3: Route interest share to treasury on repay

**Files:**
- Modify: `src/rumi_protocol_backend/src/state.rs:1475-1483` (`repay_to_vault`)
- Modify: `src/rumi_protocol_backend/src/event.rs:795-807` (`record_repayed_to_vault`)
- Modify: `src/rumi_protocol_backend/src/vault.rs:673-735` (`repay_to_vault` async fn)
- Modify: `src/rumi_protocol_backend/src/vault.rs:2268-2330` (`partial_repay_to_vault`)
- Modify: `src/rumi_protocol_backend/src/vault.rs:738-827` (`repay_to_vault_with_stable`)

**Step 1: Write failing test — repay reduces accrued_interest proportionally**

Add to `state.rs` tests:

```rust
#[test]
fn test_repay_reduces_accrued_interest_proportionally() {
    let mut state = accrual_test_state();
    let icp = state.icp_ledger_principal;

    // Vault with 500M debt, 100M of which is accrued interest
    state.vault_id_to_vaults.insert(1, Vault {
        owner: Principal::anonymous(),
        vault_id: 1,
        collateral_amount: 150_000_000,
        borrowed_icusd_amount: ICUSD::new(500_000_000),
        collateral_type: icp,
        last_accrual_time: 0,
        accrued_interest: ICUSD::new(100_000_000), // 1 icUSD of interest
    });

    // Repay 250M (half the debt)
    let (interest_share, _) = state.repay_to_vault(1, ICUSD::new(250_000_000));

    let vault = state.vault_id_to_vaults.get(&1).unwrap();
    // Debt should be 250M
    assert_eq!(vault.borrowed_icusd_amount.0, 250_000_000);
    // Interest was 100/500 = 20% of debt, so 20% of 250M repay = 50M interest share
    assert_eq!(interest_share.0, 50_000_000,
        "Interest share should be 50M, got {}", interest_share.0);
    // Remaining accrued_interest should be 100M - 50M = 50M
    assert_eq!(vault.accrued_interest.0, 50_000_000,
        "Remaining accrued_interest should be 50M, got {}", vault.accrued_interest.0);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p rumi_protocol_backend test_repay_reduces_accrued_interest_proportionally -- --nocapture 2>&1 | tail -10`
Expected: FAIL — `repay_to_vault` doesn't return a tuple yet.

**Step 3: Update `State::repay_to_vault` to return interest share**

In `state.rs`, change `repay_to_vault` (line 1475):

```rust
/// Repay debt to a vault. Returns (interest_share, principal_share) of the payment.
pub fn repay_to_vault(&mut self, vault_id: u64, repayed_amount: ICUSD) -> (ICUSD, ICUSD) {
    match self.vault_id_to_vaults.get_mut(&vault_id) {
        Some(vault) => {
            assert!(repayed_amount <= vault.borrowed_icusd_amount);

            // Compute interest share: proportional to accrued_interest / total_debt
            let interest_share = if vault.accrued_interest.0 > 0 && vault.borrowed_icusd_amount.0 > 0 {
                let share = (rust_decimal::Decimal::from(repayed_amount.0)
                    * rust_decimal::Decimal::from(vault.accrued_interest.0)
                    / rust_decimal::Decimal::from(vault.borrowed_icusd_amount.0))
                    .to_u64()
                    .unwrap_or(0);
                ICUSD::new(share.min(vault.accrued_interest.0))
            } else {
                ICUSD::new(0)
            };
            let principal_share = repayed_amount - interest_share;

            vault.borrowed_icusd_amount -= repayed_amount;
            vault.accrued_interest -= interest_share;

            (interest_share, principal_share)
        }
        None => ic_cdk::trap("repaying to unknown vault"),
    }
}
```

**Step 4: Update `record_repayed_to_vault` in event.rs**

The event recorder calls `state.repay_to_vault` — update it to capture and return the interest share:

```rust
pub fn record_repayed_to_vault(
    state: &mut State,
    vault_id: u64,
    repayed_amount: ICUSD,
    block_index: u64,
) -> ICUSD {
    record_event(&Event::RepayToVault {
        vault_id,
        block_index,
        repayed_amount,
    });
    let (interest_share, _) = state.repay_to_vault(vault_id, repayed_amount);
    interest_share
}
```

**Step 5: Update all callers of `record_repayed_to_vault` to handle treasury mint**

There are 4 call sites in `vault.rs`. Each needs the same pattern — capture interest share, mint to treasury if nonzero.

Add a helper function at the top of `vault.rs` (after the imports):

```rust
/// Mint the interest share of a repayment to the treasury canister.
/// Non-critical: if treasury is unset or mint fails, log and continue.
async fn mint_interest_to_treasury(interest_share: ICUSD) {
    if interest_share.0 == 0 {
        return;
    }
    let treasury = read_state(|s| s.treasury_principal);
    if let Some(treasury_principal) = treasury {
        match management::mint_icusd(interest_share, treasury_principal).await {
            Ok(block_index) => {
                log!(INFO, "[treasury] Minted {} icUSD interest to treasury (block {})",
                    interest_share.to_u64(), block_index);
            }
            Err(e) => {
                log!(INFO, "[treasury] WARNING: Failed to mint {} icUSD interest to treasury: {:?}",
                    interest_share.to_u64(), e);
            }
        }
    }
}
```

Then update each call site:

**5a. `repay_to_vault` (line ~723):**

```rust
Ok(block_index) => {
    let interest_share = mutate_state(|s| record_repayed_to_vault(s, arg.vault_id, amount, block_index));
    mint_interest_to_treasury(interest_share).await;
    guard_principal.complete();
    Ok(block_index)
}
```

**5b. `repay_to_vault_with_stable` (line ~815):**

```rust
Ok(block_index) => {
    let interest_share = mutate_state(|s| record_repayed_to_vault(s, arg.vault_id, amount, block_index));
    mint_interest_to_treasury(interest_share).await;
    guard_principal.complete();
    Ok(block_index)
}
```

**5c. `partial_repay_to_vault` (line ~2318):**

```rust
Ok(block_index) => {
    let interest_share = mutate_state(|s| record_repayed_to_vault(s, arg.vault_id, amount, block_index));
    mint_interest_to_treasury(interest_share).await;
    guard_principal.complete();
    Ok(block_index)
}
```

**5d. Search for any other callers of `record_repayed_to_vault`** — there may be one in the redemption flow. Grep for `record_repayed_to_vault` and update all.

**Step 6: Run tests**

Run: `cargo test -p rumi_protocol_backend 2>&1 | tail -20`
Expected: All tests pass. (Some callers may need `_ =` to ignore the return value during event replay in `event.rs`.)

**Step 7: Commit**

```bash
git add src/rumi_protocol_backend/src/state.rs src/rumi_protocol_backend/src/vault.rs src/rumi_protocol_backend/src/event.rs
git commit -m "feat: route interest revenue to treasury on repay"
```

---

### Task 4: Route interest share to treasury on liquidation

**Files:**
- Modify: `src/rumi_protocol_backend/src/vault.rs:1645-1801` (`liquidate_vault_partial`)
- Modify: `src/rumi_protocol_backend/src/vault.rs:1804+` (`liquidate_vault_partial_with_stable`)
- Modify: `src/rumi_protocol_backend/src/vault.rs:2332+` (`partial_liquidate_vault` — the VaultArg version)
- Modify: `src/rumi_protocol_backend/src/state.rs` (`liquidate_vault` internal method if it reduces debt directly)

**Step 1: Identify all liquidation debt-reduction paths**

Grep for places where `borrowed_icusd_amount -=` appears in liquidation code:

```bash
grep -n "borrowed_icusd_amount -=" src/rumi_protocol_backend/src/vault.rs
```

Each one needs the same interest-share split. The pattern is:

```rust
// Before (in liquidation code):
vault.borrowed_icusd_amount -= liquidation_payment;

// After:
let interest_share = if vault.accrued_interest.0 > 0 && vault.borrowed_icusd_amount.0 > 0 {
    let share = (rust_decimal::Decimal::from(liquidation_payment.to_u64())
        * rust_decimal::Decimal::from(vault.accrued_interest.0)
        / rust_decimal::Decimal::from(vault.borrowed_icusd_amount.0))
        .to_u64()
        .unwrap_or(0);
    ICUSD::new(share.min(vault.accrued_interest.0))
} else {
    ICUSD::new(0)
};
vault.borrowed_icusd_amount -= liquidation_payment;
vault.accrued_interest -= interest_share;
```

Then after the `mutate_state` block, call `mint_interest_to_treasury(interest_share).await;`.

Note: For `State::liquidate_vault` (the internal synchronous method in `state.rs` that handles protocol-initiated liquidations during `check_vaults`), we can't do an async mint. Instead, accumulate the interest into a `State` field `pending_treasury_interest: ICUSD` and mint it in the next timer tick. Add this as a sub-task.

**Step 2: Add `pending_treasury_interest` to State**

In `state.rs`, in the `State` struct (near `dust_forgiven_total`):

```rust
/// Interest revenue from liquidations awaiting mint to treasury.
/// Accumulated synchronously during check_vaults, minted async in next timer tick.
#[serde(default)]
pub pending_treasury_interest: ICUSD,
```

**Step 3: Drain pending treasury interest in the XRC timer**

In `xrc.rs`, after `mutate_state(|s| crate::event::record_accrue_interest(s, now));`, add:

```rust
// Mint any pending treasury interest from liquidations
let pending = read_state(|s| s.pending_treasury_interest);
if pending.0 > 0 {
    let treasury = read_state(|s| s.treasury_principal);
    if let Some(treasury_principal) = treasury {
        match crate::management::mint_icusd(pending, treasury_principal).await {
            Ok(block_index) => {
                mutate_state(|s| s.pending_treasury_interest = ICUSD::new(0));
                log!(INFO, "[treasury] Minted {} pending interest to treasury (block {})",
                    pending.to_u64(), block_index);
            }
            Err(e) => {
                log!(INFO, "[treasury] WARNING: Failed to mint pending interest: {:?}", e);
            }
        }
    }
}
```

**Step 4: Update `State::liquidate_vault` to track interest share**

In the internal `liquidate_vault` method (in `state.rs`), wherever debt is reduced for a liquidation, apply the proportional split and add the interest share to `self.pending_treasury_interest`.

**Step 5: Update async liquidation functions in `vault.rs`**

For `liquidate_vault_partial`, `liquidate_vault_partial_with_stable`, and `partial_liquidate_vault`: these do the debt reduction inside a `mutate_state` closure. Extract the interest share from the closure, then call `mint_interest_to_treasury` after.

Pattern for each:

```rust
let interest_share = mutate_state(|s| {
    if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
        let interest_share = /* proportional calc */;
        vault.borrowed_icusd_amount -= max_liquidatable_debt;
        vault.accrued_interest -= interest_share;
        // ... rest of existing logic
        interest_share
    } else {
        ICUSD::new(0)
    }
});
mint_interest_to_treasury(interest_share).await;
```

**Step 6: Run tests**

Run: `cargo test -p rumi_protocol_backend 2>&1 | tail -20`
Expected: All tests pass.

**Step 7: Commit**

```bash
git add src/rumi_protocol_backend/src/state.rs src/rumi_protocol_backend/src/vault.rs src/rumi_protocol_backend/src/xrc.rs
git commit -m "feat: route interest revenue to treasury on liquidation"
```

---

### Task 5: Route borrowing fee to treasury (replace liquidity pool)

**Files:**
- Modify: `src/rumi_protocol_backend/src/vault.rs:630-638` (`borrow_from_vault_internal`)
- Modify: `src/rumi_protocol_backend/src/event.rs:778-793` (`record_borrow_from_vault`)

**Step 1: Write failing test — borrowing fee no longer goes to liquidity pool**

This is hard to unit-test directly (async mint), but we can test that `provide_liquidity` is no longer called. Instead, verify the event recorder no longer credits the developer's pool. Add test:

```rust
#[test]
fn test_borrow_fee_does_not_credit_liquidity_pool() {
    let mut state = accrual_test_state();
    let dev = state.developer_principal;
    let icp = state.icp_ledger_principal;

    state.open_vault(Vault {
        owner: Principal::anonymous(),
        vault_id: 1,
        collateral_amount: 500_000_000,
        borrowed_icusd_amount: ICUSD::new(0),
        collateral_type: icp,
        last_accrual_time: 0,
        accrued_interest: ICUSD::new(0),
    });

    // Simulate borrow: the event recorder should NOT credit liquidity pool
    crate::event::record_borrow_from_vault(&mut state, 1, ICUSD::new(100_000_000), ICUSD::new(500_000), 0);

    // Developer's liquidity pool balance should be 0
    assert_eq!(state.get_provided_liquidity(dev).0, 0,
        "Borrowing fee should NOT go to developer's liquidity pool anymore");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p rumi_protocol_backend test_borrow_fee_does_not_credit_liquidity_pool -- --nocapture 2>&1 | tail -10`
Expected: FAIL — `provide_liquidity` is still called.

**Step 3: Update `record_borrow_from_vault` — remove `provide_liquidity`**

In `event.rs`, change `record_borrow_from_vault`:

```rust
pub fn record_borrow_from_vault(
    state: &mut State,
    vault_id: u64,
    borrowed_amount: ICUSD,
    fee_amount: ICUSD,
    block_index: u64,
) {
    record_event(&Event::BorrowFromVault {
        vault_id,
        block_index,
        fee_amount,
        borrowed_amount,
    });
    state.borrow_from_vault(vault_id, borrowed_amount);
    // Fee is now minted to treasury in the async caller — no longer credited to liquidity pool.
}
```

**Step 4: Update `borrow_from_vault_internal` to mint fee to treasury**

In `vault.rs`, change the borrow flow (lines ~631-638):

```rust
let fee: ICUSD = read_state(|s| amount * s.get_borrowing_fee_for(&vault.collateral_type));

// Mint full amount to borrower (not amount-fee like before)
match mint_icusd(amount, caller).await {
    Ok(block_index) => {
        mutate_state(|s| {
            record_borrow_from_vault(s, arg.vault_id, amount, fee, block_index);
        });

        // Mint fee to treasury as real icUSD revenue
        if fee.0 > 0 {
            let treasury = read_state(|s| s.treasury_principal);
            if let Some(treasury_principal) = treasury {
                match mint_icusd(fee, treasury_principal).await {
                    Ok(fee_block) => {
                        log!(INFO, "[treasury] Minted {} icUSD borrowing fee to treasury (block {})",
                            fee.to_u64(), fee_block);
                    }
                    Err(e) => {
                        log!(INFO, "[treasury] WARNING: Failed to mint borrowing fee to treasury: {:?}", e);
                    }
                }
            }
        }

        Ok(SuccessWithFee {
```

Note: The vault debt is `amount` (full borrow including fee) — this hasn't changed. What changed is:
- **Before:** `mint_icusd(amount - fee, user)` and `provide_liquidity(fee, developer)` — user gets less, fee is phantom
- **After:** `mint_icusd(amount, user)` and `mint_icusd(fee, treasury)` — user gets full amount, treasury gets real fee

Wait — this changes the user-facing behavior. The user used to receive `amount - fee`. Now they receive `amount` and the fee is a separate mint. **This means the debt-to-received ratio changes.** The vault debt stays as `amount`, but the user gets `amount` instead of `amount - fee`.

**IMPORTANT DESIGN DECISION:** We need to preserve the existing economic behavior where the borrowing fee reduces the user's received amount. The correct change is:

- Keep `mint_icusd(amount - fee, caller)` — user still receives less
- Replace `provide_liquidity(fee, developer)` with `mint_icusd(fee, treasury)` — fee goes to treasury as real icUSD
- Vault debt stays as `amount`

Updated code:

```rust
let fee: ICUSD = read_state(|s| amount * s.get_borrowing_fee_for(&vault.collateral_type));

match mint_icusd(amount - fee, caller).await {
    Ok(block_index) => {
        mutate_state(|s| {
            record_borrow_from_vault(s, arg.vault_id, amount, fee, block_index);
        });

        // Mint fee to treasury as real icUSD (replaces phantom liquidity pool credit)
        if fee.0 > 0 {
            let treasury = read_state(|s| s.treasury_principal);
            if let Some(treasury_principal) = treasury {
                match mint_icusd(fee, treasury_principal).await {
                    Ok(fee_block) => {
                        log!(INFO, "[treasury] Minted {} icUSD borrowing fee to treasury (block {})",
                            fee.to_u64(), fee_block);
                    }
                    Err(e) => {
                        log!(INFO, "[treasury] WARNING: Failed to mint borrowing fee to treasury: {:?}", e);
                    }
                }
            }
        }

        Ok(SuccessWithFee {
```

**Step 5: Handle event replay backward compatibility**

In `event.rs`, when replaying old `BorrowFromVault` events, the old code called `provide_liquidity`. For backward compat during replay, keep the old behavior for historical events. The simplest approach: check if `treasury_principal` is set — if not, fall back to old behavior. Actually, since we're removing the line entirely and the liquidity pool entries are historical, this is fine. The developer's pool balance will just stop growing. Old entries remain.

**Step 6: Run tests**

Run: `cargo test -p rumi_protocol_backend 2>&1 | tail -20`
Expected: All tests pass.

**Step 7: Commit**

```bash
git add src/rumi_protocol_backend/src/vault.rs src/rumi_protocol_backend/src/event.rs src/rumi_protocol_backend/src/state.rs
git commit -m "feat: route borrowing fee to treasury as real icUSD"
```

---

### Task 6: Add query endpoint for treasury stats

**Files:**
- Modify: `src/rumi_protocol_backend/src/main.rs`

**Step 1: Add a query endpoint for transparency**

In `main.rs`, add after `get_treasury_principal`:

```rust
#[derive(CandidType, Deserialize)]
pub struct TreasuryStats {
    pub treasury_principal: Option<Principal>,
    pub total_accrued_interest_system: u64,
    pub pending_treasury_interest: u64,
}

#[candid_method(query)]
#[query]
fn get_treasury_stats() -> TreasuryStats {
    read_state(|s| {
        let total_accrued = s.vault_id_to_vaults.values()
            .map(|v| v.accrued_interest.to_u64())
            .sum::<u64>();
        TreasuryStats {
            treasury_principal: s.treasury_principal,
            total_accrued_interest_system: total_accrued,
            pending_treasury_interest: s.pending_treasury_interest.to_u64(),
        }
    })
}
```

**Step 2: Commit**

```bash
git add src/rumi_protocol_backend/src/main.rs
git commit -m "feat: add get_treasury_stats query endpoint"
```

---

### Task 7: Update frontend to show accrued interest

**Files:**
- Modify: `src/vault_frontend/src/lib/components/vault/VaultCard.svelte`

**Step 1: Parse `accrued_interest` from CandidVault**

The `CandidVault` now includes `accrued_interest: u64`. The frontend vault store should already pick this up via the Candid interface. Check where `CandidVault` → JS vault mapping happens and ensure `accrued_interest` is mapped (likely in the IDL/declarations auto-generated from `dfx generate`).

**Step 2: Display accrued interest in VaultCard**

In `VaultCard.svelte`, after the debt display (line ~511-512), add an interest indicator:

```svelte
<span class="cell-value">{fmtBorrowed} icUSD</span>
<span class="cell-sub">${fmtBorrowedUsd}</span>
{#if vault.accrued_interest > 0}
  <span class="cell-sub interest-accrued">
    incl. {formatNumber(vault.accrued_interest / 1e8, 4)} interest
  </span>
{/if}
```

Add minimal CSS:

```css
.interest-accrued {
  color: var(--warning-text, #f59e0b);
  font-size: 0.75rem;
}
```

**Step 3: Commit**

```bash
git add src/vault_frontend/
git commit -m "feat: display accrued interest on vault cards"
```

---

### Task 8: Final integration test and deploy

**Step 1: Run full test suite**

Run: `cargo test -p rumi_protocol_backend 2>&1 | tail -30`
Expected: All tests pass.

**Step 2: Build canister**

Run: `dfx build rumi_protocol_backend 2>&1 | tail -10`
Expected: Build succeeds.

**Step 3: Build frontend**

Run: `dfx build vault_frontend 2>&1 | tail -10`
Expected: Build succeeds.

**Step 4: Commit any remaining changes**

```bash
git add -A && git status
```

**Step 5: Push branch and create PR**

```bash
git push -u origin feat/treasury-fee-routing
```

Create PR with summary of all changes.

**DO NOT deploy** — the user will deploy from their controller identity after PR review.
