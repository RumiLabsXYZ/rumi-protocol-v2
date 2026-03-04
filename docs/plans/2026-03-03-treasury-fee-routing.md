# Treasury Fee Routing Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Route all protocol fees (borrowing fees, interest revenue, and a new liquidation protocol fee) to the `rumi_treasury` canister as real, tracked revenue. Integrate with the treasury's `deposit()` endpoint for categorized bookkeeping.

**Architecture:** (1) Add `accrued_interest` to each Vault for interest-vs-principal tracking. (2) On debt reduction (repay/liquidation), compute interest share, mint to treasury, call `treasury.deposit()`. (3) Replace borrowing fee phantom routing with real mint to treasury. (4) Add a global configurable `liquidation_protocol_share` (default 3%) that gives the protocol a cut of the liquidator's profit (bonus portion) in collateral. (5) Rename treasury `DepositType` variants to match actual revenue streams.

**Tech Stack:** Rust (IC canisters), Svelte (frontend), Candid (interface)

---

## Context for Implementer

### Current fee flows (broken)

| Fee | Current behavior | Problem |
|-----|-----------------|---------|
| Borrowing fee | `mint_icusd(amount - fee, user)` then `provide_liquidity(fee, developer)` | Fee is phantom — never minted. Developer gets a liquidity pool credit for tokens that don't exist. |
| Interest on repay | `transfer_icusd_from(full_debt, user)` → canister holds it, `repay_to_vault` reduces debt | Interest portion is dead icUSD stuck in the canister. No revenue. |
| Interest on liquidation | Liquidator pays full debt (incl. interest) via `transfer_icusd_from` | Same — interest portion is dead. |
| Liquidation bonus | Liquidator gets `debt_value * 1.15` in collateral. Protocol gets nothing. | All the bonus goes to the liquidator; protocol earns zero from liquidations. |

### Target fee flows

| Fee | New behavior | Treasury DepositType |
|-----|-------------|---------------------|
| Borrowing fee | `mint_icusd(amount - fee, user)`, `mint_icusd(fee, treasury)`. Call `treasury.deposit()`. | `BorrowingFee` |
| Interest on repay | Compute interest share of payment. `mint_icusd(interest_share, treasury)`. Call `treasury.deposit()`. | `InterestRevenue` |
| Interest on liquidation | Same proportional split. Mint to treasury. | `InterestRevenue` |
| Liquidation protocol fee | Take `liquidation_protocol_share` (global, default 3%) of the liquidator's profit (bonus portion) in collateral, send to treasury. Call `treasury.deposit()`. | `LiquidationFee` |

### Key files

**Backend:**
- `src/rumi_protocol_backend/src/vault.rs` — Vault struct (line 48), borrow/repay/liquidation logic
- `src/rumi_protocol_backend/src/state.rs` — `accrue_single_vault` (1103), `accrue_all_vault_interest` (1142), `repay_to_vault` (1475), `borrow_from_vault` (1447)
- `src/rumi_protocol_backend/src/event.rs` — `record_borrow_from_vault` (778), `record_repayed_to_vault` (795)
- `src/rumi_protocol_backend/src/management.rs` — `mint_icusd` (266), `transfer_icusd` (337), `transfer_collateral` (for sending collateral to treasury)
- `src/rumi_protocol_backend/src/main.rs` — `set_treasury_principal` (980), admin endpoints
- `src/rumi_protocol_backend/src/xrc.rs` — timer tick where pending interest gets minted

**Treasury canister:**
- `src/rumi_treasury/src/types.rs` — `DepositType`, `AssetType`, `DepositArgs`
- `src/rumi_treasury/src/lib.rs` — `deposit()`, `withdraw()`, `get_status()`
- `src/rumi_treasury/src/state.rs` — `TreasuryState`, balance tracking

**Frontend:**
- `src/vault_frontend/src/lib/components/vault/VaultCard.svelte` — live-ticking debt display (line 119)

### Important constraints

- **NEVER reinstall the backend canister** — upgrades only. New fields must default via `#[serde(default)]`.
- **NEVER reinstall the treasury canister** — same rule. Renaming `DepositType` variants requires `#[serde(alias = "OldName")]` for backward compat of any existing stored records. (Currently zero deposits stored, but be safe.)
- `treasury_principal` is already set on-chain to `tlg74-oiaaa-aaaap-qrd6a-cai`.
- `mint_icusd` and inter-canister calls to `treasury.deposit()` are async and can fail. Treasury operations are non-critical: if they fail, log and continue — never block the user's repay/borrow/liquidation.
- The `treasury.deposit()` function is open to anyone (no controller check for deposits). Only `withdraw()` requires controller.
- Liquidation bonus varies per collateral type (currently 1.15 for ICP, lower for others) and is already configurable per-collateral. The protocol share is a fraction of the *bonus portion* (the profit the liquidator earns), not the total collateral seized.
- `liquidation_protocol_share` is a **global** parameter (NOT per-collateral). Default: `0.03` (3%). Example: if a liquidator earns 10 ICP profit from the bonus, the protocol gets 0.3 ICP.
- Collateral transfers to treasury use `management::transfer_collateral(amount, treasury_principal, ledger)`.

---

### Task 1: Rename treasury `DepositType` variants

**Files:**
- Modify: `src/rumi_treasury/src/types.rs`

**Step 1: Rename the enum variants**

```rust
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum DepositType {
    /// Fee collected when users borrow/mint icUSD
    #[serde(alias = "MintingFee")]
    BorrowingFee,
    /// Interest revenue accrued on vault debt
    #[serde(alias = "StabilityFee")]
    InterestRevenue,
    /// Fee collected when users redeem icUSD
    RedemptionFee,
    /// Protocol's share of liquidation bonus (in collateral)
    #[serde(alias = "LiquidationSurplus")]
    LiquidationFee,
}
```

The `#[serde(alias = "...")]` attributes allow deserialization of any previously-stored records with old names.

**Step 2: Run tests**

Run: `cargo test -p rumi_treasury 2>&1 | tail -10`
Expected: All tests pass.

**Step 3: Commit**

```bash
git add src/rumi_treasury/src/types.rs
git commit -m "refactor: rename treasury DepositType variants to match revenue streams"
```

---

### Task 2: Add `accrued_interest` field to Vault struct

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

**Step 3: Update all Vault construction sites in tests and event replay**

Every `Vault { ... }` literal needs `accrued_interest: ICUSD::new(0)`. Search all files for `last_accrual_time:` and add the new field after each occurrence. This includes ~12 test vaults in `state.rs` and event replay code in `event.rs`.

**Step 4: Run tests to verify compilation**

Run: `cargo test -p rumi_protocol_backend 2>&1 | tail -20`
Expected: All existing tests pass.

**Step 5: Commit**

```bash
git add src/rumi_protocol_backend/
git commit -m "feat: add accrued_interest field to Vault struct"
```

---

### Task 3: Track interest delta during accrual

**Files:**
- Modify: `src/rumi_protocol_backend/src/state.rs:1103-1138` (`accrue_single_vault`)
- Modify: `src/rumi_protocol_backend/src/state.rs:1142-1178` (`accrue_all_vault_interest`)

**Step 1: Write failing test**

```rust
#[test]
fn test_accrue_single_vault_tracks_accrued_interest() {
    let mut state = accrual_test_state();
    let icp = state.icp_ledger_principal;

    state.vault_id_to_vaults.insert(1, Vault {
        owner: Principal::anonymous(),
        vault_id: 1,
        collateral_amount: 150_000_000, // 1.5 ICP
        borrowed_icusd_amount: ICUSD::new(500_000_000), // 5 icUSD
        collateral_type: icp,
        last_accrual_time: 0,
        accrued_interest: ICUSD::new(0),
    });

    state.accrue_single_vault(1, crate::numeric::NANOS_PER_YEAR);

    let vault = state.vault_id_to_vaults.get(&1).unwrap();
    assert_eq!(vault.borrowed_icusd_amount.0, 525_000_000);
    assert_eq!(vault.accrued_interest.0, 25_000_000,
        "accrued_interest should track the 25M delta, got {}", vault.accrued_interest.0);
}
```

**Step 2: Run test → expect FAIL**

**Step 3: Update both accrual functions**

In both `accrue_single_vault` and `accrue_all_vault_interest`, add 2 lines after computing `new_debt`:

```rust
let interest_delta = new_debt.saturating_sub(vault.borrowed_icusd_amount.0);
vault.accrued_interest += ICUSD::from(interest_delta);
```

**Step 4: Run test → expect PASS. Run all tests → all pass.**

**Step 5: Commit**

```bash
git add src/rumi_protocol_backend/src/state.rs
git commit -m "feat: track accrued_interest delta during interest accrual"
```

---

### Task 4: Add treasury helper for inter-canister deposit calls

**Files:**
- Modify: `src/rumi_protocol_backend/src/vault.rs` (add helper functions)

**Step 1: Add helper functions**

These helpers handle the two-step pattern: (1) mint/transfer to treasury, (2) call `treasury.deposit()` for bookkeeping.

```rust
use rumi_treasury::types::{DepositType, DepositArgs, AssetType};

/// Mint icUSD interest to treasury and record the deposit.
async fn mint_interest_to_treasury(interest_share: ICUSD) {
    if interest_share.0 == 0 { return; }
    let treasury = read_state(|s| s.treasury_principal);
    if let Some(tp) = treasury {
        match management::mint_icusd(interest_share, tp).await {
            Ok(block_index) => {
                log!(INFO, "[treasury] Minted {} icUSD interest (block {})", interest_share.to_u64(), block_index);
                let _ = notify_treasury_deposit(tp, DepositType::InterestRevenue, AssetType::ICUSD, interest_share.to_u64(), block_index).await;
            }
            Err(e) => log!(INFO, "[treasury] WARNING: interest mint failed: {:?}", e),
        }
    }
}

/// Mint icUSD borrowing fee to treasury and record the deposit.
async fn mint_borrowing_fee_to_treasury(fee: ICUSD) {
    if fee.0 == 0 { return; }
    let treasury = read_state(|s| s.treasury_principal);
    if let Some(tp) = treasury {
        match management::mint_icusd(fee, tp).await {
            Ok(block_index) => {
                log!(INFO, "[treasury] Minted {} icUSD borrowing fee (block {})", fee.to_u64(), block_index);
                let _ = notify_treasury_deposit(tp, DepositType::BorrowingFee, AssetType::ICUSD, fee.to_u64(), block_index).await;
            }
            Err(e) => log!(INFO, "[treasury] WARNING: borrowing fee mint failed: {:?}", e),
        }
    }
}

/// Transfer collateral (liquidation fee) to treasury and record the deposit.
async fn send_liquidation_fee_to_treasury(amount: u64, collateral_ledger: Principal, asset_type: AssetType) {
    if amount == 0 { return; }
    let treasury = read_state(|s| s.treasury_principal);
    if let Some(tp) = treasury {
        match management::transfer_collateral(amount, tp, collateral_ledger).await {
            Ok(block_index) => {
                log!(INFO, "[treasury] Sent {} collateral liquidation fee (block {})", amount, block_index);
                let _ = notify_treasury_deposit(tp, DepositType::LiquidationFee, asset_type, amount, block_index).await;
            }
            Err(e) => log!(INFO, "[treasury] WARNING: liquidation fee transfer failed: {:?}", e),
        }
    }
}

/// Notify the treasury canister about a deposit (for bookkeeping).
/// Non-critical: failures are logged but don't affect protocol operation.
async fn notify_treasury_deposit(
    treasury: Principal,
    deposit_type: DepositType,
    asset_type: AssetType,
    amount: u64,
    block_index: u64,
) -> Result<u64, String> {
    let args = DepositArgs {
        deposit_type,
        asset_type,
        amount,
        block_index,
        memo: None,
    };
    let result: Result<(Result<u64, String>,), _> = ic_cdk::call(treasury, "deposit", (args,)).await;
    match result {
        Ok((Ok(deposit_id),)) => {
            log!(INFO, "[treasury] Deposit recorded: id={}", deposit_id);
            Ok(deposit_id)
        }
        Ok((Err(e),)) => {
            log!(INFO, "[treasury] WARNING: deposit recording failed: {}", e);
            Err(e)
        }
        Err((code, msg)) => {
            log!(INFO, "[treasury] WARNING: inter-canister call failed: {:?} {}", code, msg);
            Err(msg)
        }
    }
}
```

Note: the `rumi_treasury::types` import requires adding `rumi_treasury` as a dependency in the backend's `Cargo.toml` (types-only, no canister dependency). Alternatively, duplicate the type definitions in the backend or use raw Candid encoding. **Prefer adding the crate dependency** if the workspace structure allows it; otherwise define minimal mirror types in `vault.rs`.

**Step 2: Commit**

```bash
git add src/rumi_protocol_backend/
git commit -m "feat: add treasury inter-canister deposit helpers"
```

---

### Task 5: Route interest share to treasury on repay

**Files:**
- Modify: `src/rumi_protocol_backend/src/state.rs:1475-1483` (`repay_to_vault`)
- Modify: `src/rumi_protocol_backend/src/event.rs:795-807` (`record_repayed_to_vault`)
- Modify: `src/rumi_protocol_backend/src/vault.rs` (all `repay_to_vault` callers)

**Step 1: Write failing test**

```rust
#[test]
fn test_repay_reduces_accrued_interest_proportionally() {
    let mut state = accrual_test_state();
    let icp = state.icp_ledger_principal;

    state.vault_id_to_vaults.insert(1, Vault {
        owner: Principal::anonymous(),
        vault_id: 1,
        collateral_amount: 150_000_000,
        borrowed_icusd_amount: ICUSD::new(500_000_000), // 5 icUSD total
        collateral_type: icp,
        last_accrual_time: 0,
        accrued_interest: ICUSD::new(100_000_000), // 1 icUSD is interest
    });

    let (interest_share, _) = state.repay_to_vault(1, ICUSD::new(250_000_000));

    let vault = state.vault_id_to_vaults.get(&1).unwrap();
    assert_eq!(vault.borrowed_icusd_amount.0, 250_000_000);
    // 100/500 = 20% is interest, so 20% of 250M = 50M
    assert_eq!(interest_share.0, 50_000_000);
    assert_eq!(vault.accrued_interest.0, 50_000_000);
}
```

**Step 2: Run test → expect FAIL**

**Step 3: Update `State::repay_to_vault` to return `(interest_share, principal_share)`**

```rust
pub fn repay_to_vault(&mut self, vault_id: u64, repayed_amount: ICUSD) -> (ICUSD, ICUSD) {
    match self.vault_id_to_vaults.get_mut(&vault_id) {
        Some(vault) => {
            assert!(repayed_amount <= vault.borrowed_icusd_amount);
            let interest_share = if vault.accrued_interest.0 > 0 && vault.borrowed_icusd_amount.0 > 0 {
                let share = (rust_decimal::Decimal::from(repayed_amount.0)
                    * rust_decimal::Decimal::from(vault.accrued_interest.0)
                    / rust_decimal::Decimal::from(vault.borrowed_icusd_amount.0))
                    .to_u64().unwrap_or(0);
                ICUSD::new(share.min(vault.accrued_interest.0))
            } else { ICUSD::new(0) };
            let principal_share = repayed_amount - interest_share;
            vault.borrowed_icusd_amount -= repayed_amount;
            vault.accrued_interest -= interest_share;
            (interest_share, principal_share)
        }
        None => ic_cdk::trap("repaying to unknown vault"),
    }
}
```

**Step 4: Update `record_repayed_to_vault` to return interest share**

```rust
pub fn record_repayed_to_vault(...) -> ICUSD {
    record_event(&Event::RepayToVault { vault_id, block_index, repayed_amount });
    let (interest_share, _) = state.repay_to_vault(vault_id, repayed_amount);
    interest_share
}
```

**Step 5: Update all callers in vault.rs**

At each `repay_to_vault` / `partial_repay_to_vault` / `repay_to_vault_with_stable` call site:

```rust
let interest_share = mutate_state(|s| record_repayed_to_vault(s, arg.vault_id, amount, block_index));
mint_interest_to_treasury(interest_share).await;
```

**Step 6: Run tests → all pass. Commit.**

```bash
git commit -m "feat: route interest revenue to treasury on repay"
```

---

### Task 6: Route interest share to treasury on liquidation

**Files:**
- Modify: `src/rumi_protocol_backend/src/vault.rs` (all `liquidate_*` functions)
- Modify: `src/rumi_protocol_backend/src/state.rs` (add `pending_treasury_interest`, update `liquidate_vault`)
- Modify: `src/rumi_protocol_backend/src/xrc.rs` (drain pending interest in timer)

**Step 1: Add `pending_treasury_interest` to State**

```rust
/// Interest revenue from sync liquidations, minted to treasury in next timer tick.
#[serde(default)]
pub pending_treasury_interest: ICUSD,
```

**Step 2: Apply proportional interest split in all liquidation debt-reduction paths**

Same pattern as repay: compute interest share proportionally, reduce `accrued_interest`, extract from `mutate_state` closure, call `mint_interest_to_treasury()`.

For the sync `State::liquidate_vault` (used by `check_vaults` during price tick), add to `self.pending_treasury_interest` instead of async mint.

**Step 3: Drain pending interest in XRC timer**

In `xrc.rs`, after `record_accrue_interest`, add:

```rust
let pending = read_state(|s| s.pending_treasury_interest);
if pending.0 > 0 {
    let treasury = read_state(|s| s.treasury_principal);
    if let Some(tp) = treasury {
        match crate::management::mint_icusd(pending, tp).await {
            Ok(bi) => {
                mutate_state(|s| s.pending_treasury_interest = ICUSD::new(0));
                // also call notify_treasury_deposit(...)
            }
            Err(e) => log!(INFO, "[treasury] WARNING: pending interest mint failed: {:?}", e),
        }
    }
}
```

**Step 4: Run tests → all pass. Commit.**

```bash
git commit -m "feat: route interest revenue to treasury on liquidation"
```

---

### Task 7: Route borrowing fee to treasury (replace liquidity pool)

**Files:**
- Modify: `src/rumi_protocol_backend/src/vault.rs:630-638` (`borrow_from_vault_internal`)
- Modify: `src/rumi_protocol_backend/src/event.rs:778-793` (`record_borrow_from_vault`)

**Step 1: Write failing test**

```rust
#[test]
fn test_borrow_fee_does_not_credit_liquidity_pool() {
    let mut state = accrual_test_state();
    let dev = state.developer_principal;
    let icp = state.icp_ledger_principal;

    state.open_vault(Vault {
        owner: Principal::anonymous(), vault_id: 1,
        collateral_amount: 500_000_000, borrowed_icusd_amount: ICUSD::new(0),
        collateral_type: icp, last_accrual_time: 0, accrued_interest: ICUSD::new(0),
    });

    crate::event::record_borrow_from_vault(&mut state, 1, ICUSD::new(100_000_000), ICUSD::new(500_000), 0);
    assert_eq!(state.get_provided_liquidity(dev).0, 0,
        "Borrowing fee should NOT go to developer liquidity pool");
}
```

**Step 2: Run test → expect FAIL**

**Step 3: Remove `provide_liquidity` from `record_borrow_from_vault`**

```rust
pub fn record_borrow_from_vault(state: &mut State, vault_id: u64, borrowed_amount: ICUSD, fee_amount: ICUSD, block_index: u64) {
    record_event(&Event::BorrowFromVault { vault_id, block_index, fee_amount, borrowed_amount });
    state.borrow_from_vault(vault_id, borrowed_amount);
    // Fee is now minted to treasury in the async caller — no longer credited to liquidity pool.
}
```

**Step 4: Update `borrow_from_vault_internal`**

Keep `mint_icusd(amount - fee, caller)` (user still receives amount minus fee). After recording the event, call:

```rust
mint_borrowing_fee_to_treasury(fee).await;
```

**Step 5: Run tests → all pass. Commit.**

```bash
git commit -m "feat: route borrowing fee to treasury as real icUSD"
```

---

### Task 8: Add liquidation protocol fee (collateral share)

**Files:**
- Modify: `src/rumi_protocol_backend/src/state.rs` (add global `liquidation_protocol_share` field + default + getter)
- Modify: `src/rumi_protocol_backend/src/main.rs` (add `set_liquidation_protocol_share` admin endpoint)
- Modify: `src/rumi_protocol_backend/src/event.rs` (add event variant)
- Modify: `src/rumi_protocol_backend/src/vault.rs` (split bonus in all liquidation paths)

**Design note:** `liquidation_protocol_share` is a **global** parameter on `State`, NOT per-collateral. The liquidation *bonus* already varies per collateral type (e.g. 1.15 for ICP, lower for others), but the protocol's cut of that bonus is a single global percentage. Default: `0.03` (3%). Example: if a liquidator earns 10 ICP profit from a liquidation bonus, the protocol gets 0.3 ICP.

**Step 1: Add the configurable parameter**

In `state.rs` constants:

```rust
pub const DEFAULT_LIQUIDATION_PROTOCOL_SHARE: Ratio = Ratio::new(dec!(0.03)); // 3% of liquidator's bonus profit
```

On `State` struct (NOT CollateralConfig):

```rust
/// Global fraction of the liquidation bonus (liquidator's profit) that goes to the protocol treasury.
/// e.g., 0.03 = protocol gets 3% of the bonus, liquidator keeps 97%.
#[serde(default = "default_liquidation_protocol_share")]
pub liquidation_protocol_share: Ratio,
```

Add default fn and getter:

```rust
fn default_liquidation_protocol_share() -> Ratio { DEFAULT_LIQUIDATION_PROTOCOL_SHARE }

impl State {
    pub fn get_liquidation_protocol_share(&self) -> Ratio {
        self.liquidation_protocol_share
    }
}
```

**Step 2: Add admin endpoint and event**

In `event.rs`, add `SetLiquidationProtocolShare { share: String }`.

In `main.rs`, add:

```rust
#[update]
async fn set_liquidation_protocol_share(share: f64) -> Result<(), ProtocolError> {
    // developer-only, validate 0.0..=1.0, record event, update state.liquidation_protocol_share
}
```

**Step 3: Split the liquidation bonus in all liquidation paths**

Currently (in `liquidate_vault_partial`, line ~1696-1700):

```rust
let liq_bonus = s.get_liquidation_bonus_for(&vault.collateral_type);
let collateral_raw = icusd_to_collateral_amount(actual_liquidation_amount, price, decimals);
let collateral_with_bonus = ICP::from(collateral_raw) * liq_bonus;
let collateral_to_transfer = collateral_with_bonus.min(ICP::from(vault.collateral_amount));
```

Change to:

```rust
let liq_bonus = s.get_liquidation_bonus_for(&vault.collateral_type);
let protocol_share = s.get_liquidation_protocol_share(); // global, not per-collateral
let collateral_raw = icusd_to_collateral_amount(actual_liquidation_amount, price, decimals);
let collateral_with_bonus = ICP::from(collateral_raw) * liq_bonus;
let total_to_seize = collateral_with_bonus.min(ICP::from(vault.collateral_amount));

// Split: protocol gets a share of the bonus portion (liquidator's profit)
let bonus_portion = total_to_seize.to_u64().saturating_sub(collateral_raw);
let protocol_cut = (rust_decimal::Decimal::from(bonus_portion) * protocol_share.0)
    .to_u64().unwrap_or(0);
let collateral_to_liquidator = ICP::from(total_to_seize.to_u64() - protocol_cut);
let collateral_to_treasury = protocol_cut;
```

The vault loses `total_to_seize` collateral (unchanged from vault owner's perspective). The liquidator gets `total_to_seize - protocol_cut`. The treasury gets `protocol_cut`.

After the liquidation completes, transfer the protocol cut:

```rust
if collateral_to_treasury > 0 {
    let asset_type = map_collateral_to_asset_type(&vault.collateral_type);
    let ledger = read_state(|s| s.get_collateral_config(&vault.collateral_type)
        .map(|c| c.ledger_canister_id)).unwrap();
    send_liquidation_fee_to_treasury(collateral_to_treasury, ledger, asset_type).await;
}
```

Add a helper to map collateral principal → `AssetType`:

```rust
fn map_collateral_to_asset_type(ct: &Principal) -> AssetType {
    // Compare against known ledger principals from state
    read_state(|s| {
        if *ct == s.icp_ledger_principal { return AssetType::ICP; }
        // Check collateral configs for ckBTC, etc.
        // Default to ICP if unknown
        AssetType::ICP
    })
}
```

**Step 4: Apply the same split in `liquidate_vault_partial_with_stable`, `partial_liquidate_vault`, and `State::liquidate_vault`**

For the sync `State::liquidate_vault`, accumulate `collateral_to_treasury` into a new `pending_treasury_collateral: Vec<(u64, Principal)>` field and drain it in the XRC timer, similar to `pending_treasury_interest`.

**Step 5: Write test**

```rust
#[test]
fn test_liquidation_protocol_share_splits_bonus() {
    // Setup: vault at 130% CR, liq_bonus=1.15, protocol_share=0.03 (3%)
    // Liquidate: bonus = 15% of debt in collateral (liquidator's profit)
    // Protocol should get 3% of that bonus
    // Liquidator should get 97% of that bonus + the base collateral
    // Example: 10 ICP bonus → protocol gets 0.3 ICP, liquidator gets 9.7 ICP + base
}
```

**Step 6: Run tests → all pass. Commit.**

```bash
git commit -m "feat: add configurable liquidation protocol fee routed to treasury"
```

---

### Task 9: Add treasury stats query endpoint

**Files:**
- Modify: `src/rumi_protocol_backend/src/main.rs`

**Step 1: Add endpoint**

```rust
#[derive(CandidType, Deserialize)]
pub struct TreasuryStats {
    pub treasury_principal: Option<Principal>,
    pub total_accrued_interest_system: u64,
    pub pending_treasury_interest: u64,
}

#[query]
fn get_treasury_stats() -> TreasuryStats {
    read_state(|s| TreasuryStats {
        treasury_principal: s.treasury_principal,
        total_accrued_interest_system: s.vault_id_to_vaults.values()
            .map(|v| v.accrued_interest.to_u64()).sum(),
        pending_treasury_interest: s.pending_treasury_interest.to_u64(),
    })
}
```

**Step 2: Commit**

```bash
git commit -m "feat: add get_treasury_stats query endpoint"
```

---

### Task 10: Update frontend to show accrued interest

**Files:**
- Modify: `src/vault_frontend/src/lib/components/vault/VaultCard.svelte`

**Step 1: Display accrued interest on vault cards**

After the debt display, add:

```svelte
{#if vault.accrued_interest > 0}
  <span class="cell-sub interest-accrued">
    incl. {formatNumber(vault.accrued_interest / 1e8, 4)} interest
  </span>
{/if}
```

**Step 2: Commit**

```bash
git commit -m "feat: display accrued interest on vault cards"
```

---

### Task 11: Final build, test, and PR

**Step 1:** Run `cargo test -p rumi_protocol_backend` and `cargo test -p rumi_treasury` — all pass.
**Step 2:** Run `dfx build rumi_protocol_backend` and `dfx build vault_frontend` — both succeed.
**Step 3:** Push branch and create PR.
**DO NOT deploy** — user deploys from controller identity after review.
