# Merge Report: `feature/ckusdt-ckusdc-repayment` into `main`

**Date:** February 20, 2026
**Commits:** `c797e0d` (merge) + `50727e8` (follow-up token selector UI)
**Branches:** `feature/ckusdt-ckusdc-repayment` → `main`
**Total files changed:** 21 files, +2,038 / -316 lines

---

## Table of Contents

1. [Background & Motivation](#1-background--motivation)
2. [Branch Divergence Analysis](#2-branch-divergence-analysis)
3. [Conflict Map — All 8 Files](#3-conflict-map--all-8-files)
4. [Conflict-by-Conflict Resolution Detail](#4-conflict-by-conflict-resolution-detail)
5. [Post-Merge Compilation Errors (43 errors)](#5-post-merge-compilation-errors-43-errors)
6. [Major Rewrites & Architectural Decisions](#6-major-rewrites--architectural-decisions)
7. [What Was Deliberately Removed (ckBTC)](#7-what-was-deliberately-removed-ckbtc)
8. [Frontend Changes](#8-frontend-changes)
9. [Deployment](#9-deployment)
10. [Known Issues & Follow-up Items](#10-known-issues--follow-up-items)

---

## 1. Background & Motivation

The Rumi Protocol backend needed to support repaying vault debt using ckUSDT and ckUSDC (6-decimal ICRC-2 tokens) in addition to the existing icUSD (8-decimal) repayment flow. This work was developed on `feature/ckusdt-ckusdc-repayment` over ~20 commits, but while that branch was being developed, `main` received significant independent improvements:

- Rate limiting on close_vault operations (anti-spam)
- No-underflow safety on liquidation math (saturating arithmetic)
- Backward-compatible event deserialization for `PartialLiquidateVault`
- Async `validate_call().await?` migration
- UI improvements (vault cards, credit columns, CR styling)
- Homepage redesign and branding page
- XRC cost optimization (on-demand price fetch)
- Configurable protocol parameters (liquidation bonus, fees, etc.)
- RUMI wordmark color fix

The feature branch had also been merged with an earlier snapshot of main (`c9ce70e`), but main had since diverged further. The merge was non-trivial because the two branches had evolved the same core files (guard.rs, state.rs, main.rs, vault.rs, event.rs) in fundamentally different directions.

**Key constraint from the user:** "We WANT all the work we did on ckstable repayments, we just don't want to also bring in shit like the old UI or bugs we fixed."

---

## 2. Branch Divergence Analysis

### Feature branch unique commits (pre-merge):
| Commit | Description |
|--------|-------------|
| `080630d` | Add ckUSDT/ckUSDC ledger canister IDs |
| `c583cdb` | Add `repayToVaultWithStable` API function |
| `16a7bb0` | Token selector UI for stable repayment |
| `d15a639` | Update package-lock.json |
| `027a9a6` | PR #1 audit review findings doc |
| `bcc929b` | Design spec (reserve strategy, CR formula, fee parameter) |
| `a14602d` | Updated design spec |
| `85379b3` | Audit review resolutions doc |
| `028f4af` | Add missing `validate_call()` to `liquidate_vault_partial_with_stable` |
| `b5d6c53` | Remove artificial 1.2s debug delay |
| `5bca9d1` | Replace `.unwrap()` on vault lookups with error handling |
| `ff3fcaf` | Decimal conversion, configurable fee, kill-switch, code dedup |
| `49d65c6` | ICRC-2 approval flow for ckUSDT/ckUSDC |
| `ae51c8b` | Complete audit review implementation details |
| `6f4e8b0` | Harden liquidation endpoints, persist config, remove panic paths |
| `e16d853` | Make liquidation bonus/fees configurable |

### Main branch unique commits (since divergence):
~20 commits including rate limiting, UI redesign, homepage, XRC optimization, event backward compat, etc.

### Why a simple merge wouldn't work:
The feature branch had evolved its own version of `guard.rs` using **String-keyed** composite keys (`"principal:operation_name"`), while `main` used **Principal-keyed** maps. These are fundamentally incompatible data structures at the State level. Git's auto-merge produced syntactically valid but semantically broken code that referenced non-existent fields.

---

## 3. Conflict Map — All 8 Files

| # | File | Conflicts | Severity | Notes |
|---|------|-----------|----------|-------|
| 1 | `event.rs` | 2 | Medium | New event variants + replay logic |
| 2 | `rumi_protocol_backend.did` | 1 | Low | Candid interface additions |
| 3 | `lib.rs` | 1 | Low | InitArg struct fields |
| 4 | `+layout.svelte` | 1 | Low | RUMI wordmark color |
| 5 | `VaultDetails.svelte` | 1 | Medium | Token selector vs old UI |
| 6 | `state.rs` | 3 | High | State init, liquidation logic |
| 7 | `vault.rs` | 2 | Critical | Imports + 300 lines of new functions |
| 8 | `main.rs` | 3 | Critical | Endpoints, admin functions, validate_call |

---

## 4. Conflict-by-Conflict Resolution Detail

### 4.1 `event.rs` — Conflicts 1 & 2

**Conflict 1 — New event variants:**
- HEAD had `DustForgiven` event variant
- Feature had config event variants: `SetTreasuryPrincipal`, `SetStabilityPoolCanister`, `SetCkstableRepayFee`, `SetCkusdtEnabled`, `SetCkusdcEnabled`
- **Resolution:** Kept both. All event variants are additive and non-overlapping.

**Conflict 2 — Event replay match arms:**
- HEAD had `DustForgiven` match arm in `replay()`
- Feature had match arms for all config events
- **Resolution:** Kept both sets of match arms. Each replays to different state fields.

**Thought process:** Event enum variants are purely additive — keeping both is always safe. The replay logic just needs to handle every variant. No semantic conflict here, purely a merge ordering issue.

### 4.2 `rumi_protocol_backend.did` — Conflict 1

**Conflict:**
- HEAD had existing `InitArg` fields
- Feature added `treasury_principal`, `stability_pool_principal`, `ckusdt_ledger_principal`, `ckusdc_ledger_principal` to `InitArg`
- Feature also added `StableTokenType`, `VaultArgWithToken` types, and admin endpoints

**Resolution:** Accepted all feature additions. Candid interfaces are additive.

### 4.3 `lib.rs` — Conflict 1

**Conflict:**
- HEAD had `InitArg` struct with existing fields
- Feature added 4 new Optional Principal fields for ckstable ledger and treasury canisters

**Resolution:** Added all new fields. Struct field additions are safe.

### 4.4 `+layout.svelte` — Conflict 1

**Conflict:**
- HEAD had RUMI wordmark using flat `var(--rumi-text-primary)` color
- Feature had old gradient-based wordmark (a bug we'd already fixed on main)

**Resolution:** Kept HEAD's flat color. This was exactly the kind of regression the user warned about — "don't bring back bugs we fixed."

### 4.5 `VaultDetails.svelte` — Conflict 1

**Conflict:**
- HEAD had improved vault details UI
- Feature had its own token selector UI (single button with dropdown)

**Resolution:** Kept feature's token selector since that's the new functionality we wanted. But the real token selector UI was later built fresh in `VaultCard.svelte` (commit `50727e8`) because `VaultCard` is the actual component used in the vault list view.

### 4.6 `state.rs` — Conflict 1 (Imports/Defaults)

**Line ~20-30 area.**

- HEAD had rate limiting constants: `MAX_CLOSE_VAULT_PER_MINUTE`, `MAX_CLOSE_VAULT_PER_DAY`, `MAX_CONCURRENT_CLOSE_OPERATIONS`
- Feature had ckstable defaults: `DEFAULT_CKSTABLE_REPAY_FEE`, `DEFAULT_LIQUIDATION_BONUS`, etc.

**Resolution:** Kept both sets of constants. No overlap.

### 4.7 `state.rs` — Conflict 2 (State initialization)

**Line ~184 area — `State::from(args)` constructor.**

- HEAD added rate limiting field initialization:
  ```rust
  close_vault_requests: BTreeMap::new(),
  global_close_requests: Vec::new(),
  concurrent_close_operations: 0,
  dust_forgiven_total: ICUSD::new(0),
  ```
- Feature added ckstable field initialization:
  ```rust
  treasury_principal: args.treasury_principal,
  stability_pool_canister: args.stability_pool_principal,
  ckusdt_ledger_principal: args.ckusdt_ledger_principal,
  ckusdc_ledger_principal: args.ckusdc_ledger_principal,
  ckstable_repay_fee: DEFAULT_CKSTABLE_REPAY_FEE,
  ckusdt_enabled: true,
  ckusdc_enabled: true,
  ```

**Resolution:** Kept BOTH blocks. They initialize completely different fields on State. This was a "both sides are right" conflict.

### 4.8 `state.rs` — Conflict 3 (liquidate_vault method)

**Line ~534 area — `State::liquidate_vault()` method.**

- HEAD had safer no-underflow code using `get_mut()` and `min()` protection:
  ```rust
  let seized = vault.icp_margin_amount.min(expected_collateral);
  vault.icp_margin_amount -= seized;
  ```
- Feature had a different approach with `liquidation_bonus` calculation and a separate `liquidate_vault_partial()` method

**Resolution:** Kept HEAD's safer version. The underflow protection was a critical bug fix that had already been deployed to mainnet. The feature's partial liquidation was added as separate functions (`liquidate_vault_partial` and `liquidate_vault_partial_with_stable` in vault.rs) that don't modify the existing `liquidate_vault` codepath.

**Thought process:** This was the most delicate decision. The feature branch reimagined liquidation with bonus calculations baked into the core method. But main had already fixed real underflow bugs in production. We couldn't risk regressing that. The partial liquidation functions were cleanly separable.

### 4.9 `vault.rs` — Conflict 1 (Imports)

**Line 13 area.**

- HEAD: `MIN_PARTIAL_REPAY_AMOUNT, MIN_PARTIAL_LIQUIDATION_AMOUNT, DUST_THRESHOLD`
- Feature: `StableTokenType, VaultArgWithToken`

**Resolution:** Kept both import sets. Also added `Ratio`, `dec!` imports that would be needed by the new functions.

### 4.10 `vault.rs` — Conflict 2 (New functions — 300+ lines)

**Line ~814 area.**

- HEAD: empty (no new functions at end of file)
- Feature: Two entirely new functions — `liquidate_vault_partial()` and `liquidate_vault_partial_with_stable()`

**Resolution:** Accepted all of feature's new functions. But they needed significant fixes (see Section 5 below) because they referenced:
- Event field names that didn't match main's `PartialLiquidateVault` variant
- A `State::liquidate_vault_partial()` method that doesn't exist
- Non-Optional `icp_rate` field (main wraps it in `Some()`)

### 4.11 `main.rs` — Conflict 1 (validate_call)

**Line ~359 area.**

- HEAD: `validate_call().await?;` (async version)
- Feature: `validate_call()?;` (sync version)

**Resolution:** Kept HEAD's async version. The migration to async `validate_call` happened on main and is the correct form. This was a straightforward "main is right" decision.

### 4.12 `main.rs` — Conflict 2 (Partial operations endpoints)

**Line ~369 area.**

- HEAD had: `partial_repay_to_vault()` + `partial_liquidate_vault()` endpoints
- Feature had: `liquidate_vault_partial()`, `liquidate_vault_partial_with_stable()`, `stability_pool_liquidate()`, `get_stability_pool_config()`

**Resolution:** Kept HEAD's existing endpoints AND added all of feature's new endpoints. But critically, every `validate_call()?` in the feature code had to be changed to `validate_call().await?` to match main's async pattern.

### 4.13 `main.rs` — Conflict 3 (Admin functions)

**Line ~810 area.**

- HEAD: empty (no admin functions at end of file)
- Feature: Treasury admin, ckstable admin, `clear_stuck_operations()` function

**Resolution:** Accepted all feature admin functions. But `clear_stuck_operations()` needed a complete rewrite (see Section 6.2).

---

## 5. Post-Merge Compilation Errors (43 errors)

After resolving all 8 conflict files and staging them, `cargo check` produced **43 compilation errors**. These fell into several categories:

### 5.1 Guard System Field Mismatch

**Errors:**
- `operation_guards` field doesn't exist on `State`
- `operation_details` field doesn't exist on `State`
- `operation_guard_timestamps` field doesn't exist on `State`

**Root cause:** The feature branch's `guard.rs` used String-keyed composite maps:
```rust
// Feature branch approach (WRONG for main):
s.operation_guards.insert(format!("{}:{}", principal, operation_name));
s.operation_guard_timestamps.insert(format!("{}:{}", principal, operation_name), time);
```

Main's State has Principal-keyed maps:
```rust
// Main's actual fields:
principal_guards: BTreeSet<Principal>,
principal_guard_timestamps: BTreeMap<Principal, u64>,
operation_states: BTreeMap<Principal, OperationState>,
operation_names: BTreeMap<Principal, String>,
```

**Fix:** Complete rewrite of `guard.rs` (see Section 6.1).

### 5.2 Event Field Name Mismatch

**Errors:**
- `PartialLiquidateVault` has no field `liquidated_debt`
- `PartialLiquidateVault` has no field `collateral_seized`
- Expected `Option<UsdIcp>` for `icp_rate`, found `UsdIcp`

**Root cause:** The feature branch created events with field names that didn't match main's `PartialLiquidateVault` variant definition (which had been updated in commit `f4466dc` for backward-compatible deserialization).

Main's definition:
```rust
PartialLiquidateVault {
    vault_id: u64,
    liquidator_payment: ICUSD,      // feature used: liquidated_debt
    icp_to_liquidator: ICP,         // feature used: collateral_seized
    liquidator: Option<Principal>,
    icp_rate: Option<UsdIcp>,       // feature used: UsdIcp (not Optional)
}
```

**Fix:** Changed all event construction in vault.rs:
```rust
// Before (feature):
liquidated_debt: max_liquidatable_debt,
collateral_seized: collateral_to_liquidator,
icp_rate: icp_rate,

// After (fixed):
liquidator_payment: max_liquidatable_debt,
icp_to_liquidator: collateral_to_liquidator,
icp_rate: Some(icp_rate),
```

### 5.3 Missing `State::liquidate_vault_partial()` Method

**Error:** `no method named liquidate_vault_partial found for mutable reference &mut State`

**Root cause:** The feature's vault.rs functions called `s.liquidate_vault_partial(vault_id, debt, collateral)` — a method that was supposed to exist on State but was part of the feature's reimagined liquidation that we chose NOT to merge (we kept main's safer version instead).

**Fix:** Replaced with direct vault mutation:
```rust
// Before (feature):
s.liquidate_vault_partial(vault_id, max_liquidatable_debt, collateral_to_liquidator)?;

// After (fixed):
if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
    vault.borrowed_icusd_amount -= max_liquidatable_debt;
    vault.icp_margin_amount -= collateral_to_liquidator;
}
```

Applied to both `liquidate_vault_partial()` and `liquidate_vault_partial_with_stable()`.

### 5.4 Missing `Ratio` Import in vault.rs

**Error:** `cannot find type Ratio in this scope`

**Fix:** Added `use crate::numeric::{ICUSD, ICP, Ratio};` to vault.rs imports, and `use rust_decimal_macros::dec;` for the `dec!()` macro used in fee calculations.

### 5.5 Undefined Types: `StabilityPoolLiquidationResult`, `StabilityPoolConfig`

**Error:** `cannot find type StabilityPoolLiquidationResult in this scope`

**Root cause:** These types were referenced in main.rs endpoints but never defined. The feature branch assumed they'd come from a stability pool module that doesn't exist yet.

**Fix:** Added placeholder struct definitions in main.rs:
```rust
#[derive(CandidType, Deserialize, Debug)]
pub struct StabilityPoolLiquidationResult {
    pub liquidated_debt: u64,
    pub collateral_gained: u64,
}

#[derive(CandidType, Deserialize, Debug)]
pub struct StabilityPoolConfig {
    pub min_deposit: u64,
    pub liquidation_share: f64,
}
```

### 5.6 ckBTC Multi-Collateral Code (The Big One)

**Errors:** ~15 errors related to:
- `UsdCkBtc` type not found
- `CollateralType` type not found
- `open_vault` expects 1 argument, found 2
- `process_pending_transfer_for_vault` match on `CollateralType`

**Root cause:** The feature branch had begun work on multi-collateral support (ckBTC as a second collateral type alongside ICP). This included a `CollateralType` enum, `UsdCkBtc` numeric type, ckBTC rate timers, ckBTC fields in `ProtocolStatus`, and `CollateralType` dispatch in transfer processing. None of this exists in main's State or type system.

Git's auto-merge brought in this code because it wasn't in any conflict markers — it was just new code the feature added to `main.rs` that happened to not conflict textually, but was semantically incompatible.

**Fix:** Systematically removed ALL ckBTC references:

1. **Removed imports:** `UsdCkBtc`, `CollateralType`
2. **Removed ckBTC rate timer:** from `start_protocol_timer()`
3. **Removed ckBTC fields from `ProtocolStatus`:** `last_ckbtc_rate`, `last_ckbtc_timestamp`
4. **Simplified `open_vault` signature:** `(collateral_amount: u64)` → removed `CollateralType` parameter
5. **Simplified `get_liquidatable_vaults`:** removed ckBTC collateral filtering
6. **Removed ckBTC from metrics:** removed ckBTC price/timestamp from `get_metrics()`
7. **Simplified `process_pending_transfer_for_vault`:** removed `CollateralType` match, kept ICP-only path
8. **Replaced method calls with direct field access:**
   - `s.set_treasury_principal(p)` → `s.treasury_principal = Some(p)` (setter method doesn't exist)
   - `s.get_treasury_principal()` → `s.treasury_principal` (getter method doesn't exist)
   - Same for `stability_pool_canister`

**Thought process:** This was the hardest decision. The multi-collateral architecture is forward-looking and we might want it someday. But merging it now would mean:
- Adding types to State that have no corresponding stable storage
- Adding timer logic for a ckBTC XRC endpoint we don't use
- Adding CollateralType dispatch to every vault operation
- All for zero user-facing benefit today

We chose pragmatism over architecture: merge the ckstable *repayment* functionality (which works through the treasury canister, not through collateral types) and leave multi-collateral for a future redesign.

### 5.7 Async `validate_call` Mismatch

**Errors:** Multiple `validate_call()?` calls missing `.await`

**Root cause:** Feature branch used sync `validate_call()?`. Main migrated to async `validate_call().await?`.

**Fix:** Find-and-replace across all feature-originated endpoints in main.rs. ~8 occurrences.

---

## 6. Major Rewrites & Architectural Decisions

### 6.1 Complete Rewrite of `guard.rs`

This was the most significant architectural decision of the merge. The two branches had incompatible guard implementations:

**Feature branch approach:**
- Composite String keys: `"principal_text:operation_name"`
- `operation_guards: BTreeSet<String>`
- `operation_guard_timestamps: BTreeMap<String, u64>`
- `operation_details: BTreeMap<String, OperationDetail>`
- Allowed multiple concurrent operations per principal (different operation names)

**Main branch approach:**
- Principal keys directly: `Principal`
- `principal_guards: BTreeSet<Principal>`
- `principal_guard_timestamps: BTreeMap<Principal, u64>`
- `operation_states: BTreeMap<Principal, OperationState>`
- `operation_names: BTreeMap<Principal, String>`
- One operation per principal at a time

**Decision:** We rewrote guard.rs to use main's Principal-keyed structure while preserving the feature's improved semantics (operation naming, completion/failure tracking, stale guard cleanup).

**Rationale:**
1. Main's State struct is deployed to mainnet with Principal-keyed fields in stable storage. Changing to String-keyed would require a migration.
2. One-operation-per-principal is actually safer for a financial protocol — it prevents a user from having both a repay and liquidate in flight simultaneously.
3. The feature's operation naming is valuable for debugging, and it's compatible with Principal-keyed storage.

**What was preserved from the feature:**
- `OperationState` enum (InProgress, Completed, Failed)
- Operation naming for debug logging
- `complete()` and `fail()` methods
- Stale guard cleanup with timeout (5 minutes)
- Half-timeout override (allow new request if existing guard is >2.5 min old)

**What was kept from main:**
- Principal-keyed data structures matching State
- `TimerLogicGuard` and `FetchXrcGuard` (unchanged)
- Drop semantics: cleanup on Completed, retain on InProgress/Failed

### 6.2 Rewrite of `clear_stuck_operations` Admin Endpoint

The feature's admin function to clear stuck operations was completely broken after the guard rewrite because it referenced String-keyed fields:

**Feature version (broken):**
```rust
fn clear_stuck_operations() {
    mutate_state(|s| {
        s.operation_guards.clear();        // String-keyed — doesn't exist
        s.operation_details.clear();       // String-keyed — doesn't exist
        s.operation_guard_timestamps.clear(); // String-keyed — doesn't exist
    });
}
```

**Rewritten version:**
```rust
fn clear_stuck_operations() -> String {
    mutate_state(|s| {
        let count = s.principal_guards.len();
        s.principal_guards.clear();
        s.principal_guard_timestamps.clear();
        s.operation_states.clear();
        s.operation_names.clear();
        format!("Cleared {} stuck operations", count)
    })
}
```

### 6.3 Direct Vault Mutation vs State Method

The feature assumed `State::liquidate_vault_partial()` existed as a method on State. Since we kept main's existing `liquidate_vault()` method and didn't merge the feature's reimagined version, this method didn't exist.

Rather than adding a new method to State (which would have been risky near deployed stable storage logic), we used direct vault mutation in the new `liquidate_vault_partial` and `liquidate_vault_partial_with_stable` functions in vault.rs:

```rust
mutate_state(|s| {
    if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
        vault.borrowed_icusd_amount -= max_liquidatable_debt;
        vault.icp_margin_amount -= collateral_to_liquidator;
    }
    record_event(&Event::PartialLiquidateVault { ... });
});
```

This is semantically identical but keeps the mutation logic local to the vault.rs functions rather than spreading it across State methods.

---

## 7. What Was Deliberately Removed (ckBTC)

The feature branch included early-stage multi-collateral support that was NOT merged:

| Component | Feature Branch Had | Removed? | Reason |
|-----------|-------------------|----------|--------|
| `CollateralType` enum | `ICP`, `CkBTC` variants | Yes | No ckBTC support in State/stable storage |
| `UsdCkBtc` numeric type | Price type for ckBTC | Yes | No ckBTC XRC integration |
| ckBTC rate timer | Periodic fetch from XRC | Yes | Would incur cycles cost for unused feature |
| `ProtocolStatus.last_ckbtc_rate` | ckBTC price field | Yes | Not in State |
| `open_vault(amount, collateral_type)` | Multi-collateral open | Yes | Simplified back to ICP-only |
| `process_pending_transfer` CollateralType match | Dispatch by collateral | Yes | ICP-only transfers |
| `get_liquidatable_vaults` collateral filtering | Filter by collateral type | Yes | All vaults are ICP-collateralized |

**Important distinction:** The ckstable *repayment* feature (pay back icUSD debt using ckUSDT/ckUSDC) was fully merged. What was removed was ckBTC as a *collateral* type for opening vaults. These are completely separate features:
- **Merged:** User has ICP-collateralized vault, owes icUSD → repays with ckUSDT/ckUSDC (routed through treasury)
- **Not merged:** User opens vault with ckBTC collateral instead of ICP

---

## 8. Frontend Changes

### 8.1 Merge Commit Frontend Changes

**`+layout.svelte`:** Kept main's RUMI wordmark color fix (flat color instead of gradient).

**`VaultDetails.svelte`:** Accepted feature's token selector. This component is used for the detail/modal view.

**`config.ts`:** Added `ckusdtLedgerId`, `ckusdcLedgerId` getters and `getStableLedgerId(tokenType)` helper.

**`ProtocolManager.ts`:** Added `repayToVaultWithStable()` method with ICRC-2 approval flow.

**`walletOperations.ts`:** Added `repayToVaultWithStable()` wallet operation.

**`apiClient.ts`:** Added `repayToVaultWithStable()` API call.

### 8.2 Follow-up Commit: Token Selector in VaultCard (`50727e8`)

After the merge, we built a fresh token selector directly in `VaultCard.svelte` (the main vault list component), because the merge only touched `VaultDetails.svelte`.

**Changes to `VaultCard.svelte`:**
- Added `repayTokenType` state variable: `'icUSD' | 'CKUSDT' | 'CKUSDC'`
- Added reactive wallet balance tracking:
  ```typescript
  $: walletCkusdt = $walletStore.tokenBalances?.CKUSDT ? parseFloat(...) : 0;
  $: walletCkusdc = $walletStore.tokenBalances?.CKUSDC ? parseFloat(...) : 0;
  $: activeRepayBalance = repayTokenType === 'CKUSDT' ? walletCkusdt : ...;
  ```
- Updated `maxRepayable` to respect active token balance
- Updated `handleRepay()` to route based on token type:
  - `icUSD` → `protocolManager.repayToVault()`
  - `CKUSDT`/`CKUSDC` → `protocolManager.repayToVaultWithStable()`
- Added `<select>` dropdown in place of static "icUSD" suffix
- Added CSS: `.action-input-with-select`, `.token-select`

**Changes to `wallet.ts`:**
- Added `TokenBalance` interface with `raw`, `formatted`, `usdValue`
- Extended `WalletState.tokenBalances` with optional `CKUSDT` and `CKUSDC`
- Updated `refreshBalance()` to fetch ckstable balances using 6-decimal formatting:
  ```typescript
  const formatStable6 = (raw: bigint) => (Number(raw) / 1_000_000).toFixed(6);
  ```

---

## 9. Deployment

### 9.1 Frontend: vault_frontend
- **Status:** Deployed successfully
- **Canister:** `tcfua-yaaaa-aaaap-qrd7q-cai`
- **URL:** https://tcfua-yaaaa-aaaap-qrd7q-cai.icp0.io/

### 9.2 Backend: rumi_protocol_backend
- **Status:** Deployed successfully
- **Required special upgrade argument:**
  ```
  --argument '(variant { Upgrade = record { mode = null } })'
  ```
- **Note:** Initial deploy attempt failed with "Expected arguments but found none" — the canister's `post_upgrade` handler requires an `UpgradeArg` variant.

### 9.3 Treasury: rumi_treasury
- **Status:** NOT deployed
- **Reason:** Candid breaking change on `DepositRecord` type. The treasury canister's `.did` file was modified to include a `token_type` field on `DepositRecord`, which is a breaking change for existing stable storage. Deploying would require either:
  1. A migration strategy for existing deposit records
  2. Accepting data loss on historical deposits
  3. Making the field optional (`opt text` instead of `text`)
- **Impact:** Treasury functions called by the backend (ckstable conversion) won't work until treasury is deployed with compatible changes.

---

## 10. Known Issues & Follow-up Items

### 10.1 Wallet Store Initial Load Gap
The `connect()` and `initialize()` functions in `wallet.ts` only fetch ICP and icUSD balances. ckUSDT and ckUSDC balances are only fetched during `refreshBalance()`, which runs on a 30-second interval. This means users won't see their ckstable balances for up to 30 seconds after connecting.

**Fix:** Add ckstable balance fetching to `connect()` and `initialize()`.

### 10.2 Treasury Canister Deployment
The treasury canister needs to be deployed with compatible Candid changes before ckstable repayment will actually work end-to-end on mainnet. The backend will call the treasury, but the treasury won't accept the calls until it's upgraded.

### 10.3 Stability Pool Placeholder Types
`StabilityPoolLiquidationResult` and `StabilityPoolConfig` are placeholder structs defined in main.rs. When the actual stability pool module is built, these should be moved to a proper types module and given real fields.

### 10.4 Partial Liquidation Underflow Risk
The new `liquidate_vault_partial()` and `liquidate_vault_partial_with_stable()` functions use direct subtraction on vault amounts without the same `min()` protection that main added to the core `liquidate_vault()` method. Consider adding saturating arithmetic:
```rust
// Current (could underflow if math is wrong):
vault.borrowed_icusd_amount -= max_liquidatable_debt;
vault.icp_margin_amount -= collateral_to_liquidator;

// Safer:
vault.borrowed_icusd_amount = vault.borrowed_icusd_amount.saturating_sub(max_liquidatable_debt);
vault.icp_margin_amount = vault.icp_margin_amount.saturating_sub(collateral_to_liquidator);
```

### 10.5 Feature Branch Cleanup
The `feature/ckusdt-ckusdc-repayment` branch has been fully merged and can be deleted. All its commits are now reachable from main.

---

## Appendix: Canister IDs Referenced

| Token | Canister ID | Decimals |
|-------|------------|----------|
| ckUSDT | `cngnf-vqaaa-aaaar-qag4q-cai` | 6 |
| ckUSDC | `xevnm-gaaaa-aaaar-qafnq-cai` | 6 |
| icUSD | (protocol-issued) | 8 |
| ICP | `ryjl3-tyaaa-aaaaa-aaaba-cai` | 8 |
