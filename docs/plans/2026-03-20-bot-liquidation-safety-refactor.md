# Bot Liquidation Safety Refactor

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix two critical bugs in `bot_liquidate` / `dev_force_bot_liquidate` (hardcoded ICP transfer, premature state mutation) and make the bot liquidation path collateral-agnostic with a configurable allowlist.

**Architecture:** Add a `bot_allowed_collateral_types` set to backend state. `bot_liquidate` rejects vaults whose collateral type isn't in the set (stability pool handles those instead). Replace `transfer_icp` with `transfer_collateral` using the vault's actual ledger. Move state mutation to *after* successful transfer so a failed transfer never corrupts vault state.

**Tech Stack:** Rust, IC CDK, ICRC-1 ledger standard

---

## Bug 1: Premature State Mutation

**The problem:** In both `bot_liquidate` (`main.rs:1362`) and `dev_force_bot_liquidate` (`main.rs:1443`), the vault's debt and collateral are deducted via `mutate_state` *before* the collateral transfer. If the transfer fails, the vault has been debited but no collateral was sent — the collateral is effectively destroyed.

**The fix:** Transfer first, mutate on success. On transfer failure, return an error with vault state untouched.

## Bug 2: Hardcoded `transfer_icp`

**The problem:** Both functions call `transfer_icp()` (`management.rs:489`) which always uses the ICP ledger, regardless of the vault's `collateral_type`. For a BOB or EXE vault, this either drains unrelated ICP from the canister or fails (after state was already mutated — compounding Bug 1).

**The fix:** Look up `vault.collateral_type` (which IS the ledger canister ID) and call `transfer_collateral(amount, to, collateral_type)` instead.

## Feature: Bot Collateral Allowlist

**The design:** Add `bot_allowed_collateral_types: BTreeSet<Principal>` to `State`. `bot_liquidate` checks this set and rejects vaults with non-allowed collateral. A setter function `set_bot_allowed_collateral_types` lets the developer configure which types the bot handles. Default: ICP only (set during `set_liquidation_bot_config` or separately). The stability pool handles everything regardless — it already works with all collateral types.

---

## Tasks

### 1. Add `bot_allowed_collateral_types` to State

**File:** `src/rumi_protocol_backend/src/state.rs`

Add after line 652 (`bot_total_icusd_deposited_e8s`):

```rust
pub bot_allowed_collateral_types: BTreeSet<Principal>,
```

Add to `State::new()` default initialization (after line 828):

```rust
bot_allowed_collateral_types: BTreeSet::new(),
```

Ensure `use std::collections::BTreeSet;` is imported (likely already present).

### 2. Add setter function `set_bot_allowed_collateral_types`

**File:** `src/rumi_protocol_backend/src/main.rs`

Add near `set_liquidation_bot_config` (after line 1284):

```rust
/// Set which collateral types the bot is allowed to liquidate (developer only).
/// Pass an empty vec to disable bot liquidations entirely.
#[candid_method(update)]
#[update]
async fn set_bot_allowed_collateral_types(collateral_types: Vec<Principal>) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only developer can set bot allowed collateral types".to_string(),
        ));
    }
    mutate_state(|s| {
        s.bot_allowed_collateral_types = collateral_types.iter().copied().collect();
    });
    log!(INFO, "[set_bot_allowed_collateral_types] Set {} allowed types: {:?}",
        collateral_types.len(), collateral_types);
    Ok(())
}
```

Also add a query to read the current config:

```rust
#[candid_method(query)]
#[query]
fn get_bot_allowed_collateral_types() -> Vec<Principal> {
    read_state(|s| s.bot_allowed_collateral_types.iter().copied().collect())
}
```

### 3. Add Candid declarations

**File:** `src/rumi_protocol_backend/rumi_protocol_backend.did`

Add to the service block:

```candid
set_bot_allowed_collateral_types : (vec principal) -> (variant { Ok; Err : ProtocolError });
get_bot_allowed_collateral_types : () -> (vec principal) query;
```

### 4. Fix `bot_liquidate` — collateral guard, agnostic transfer, safe mutation ordering

**File:** `src/rumi_protocol_backend/src/main.rs`, function `bot_liquidate` (lines 1305–1402)

Replace the entire function body with:

```rust
async fn bot_liquidate(vault_id: u64) -> Result<BotLiquidationResult, ProtocolError> {
    validate_call().await?;
    validate_price_for_liquidation()?;
    let caller = ic_cdk::api::caller();

    let is_bot = read_state(|s| {
        s.liquidation_bot_principal.map_or(false, |bp| bp == caller)
    });
    if !is_bot {
        return Err(ProtocolError::GenericError(
            "Caller is not the registered liquidation bot canister".to_string(),
        ));
    }

    // Get vault info, validate collateral type is allowed, compute liquidatable amount, check budget
    let (collateral_price_usd, liquidatable_debt, collateral_to_seize, collateral_type) = read_state(|s| {
        let vault = s.vault_id_to_vaults.get(&vault_id)
            .ok_or_else(|| ProtocolError::GenericError(format!("Vault #{} not found", vault_id)))?;

        // Guard: reject collateral types the bot isn't configured to handle
        if !s.bot_allowed_collateral_types.contains(&vault.collateral_type) {
            return Err(ProtocolError::GenericError(format!(
                "Collateral type {} is not in the bot's allowed list. Stability pool should handle this vault.",
                vault.collateral_type
            )));
        }

        let price = s.get_collateral_price_decimal(&vault.collateral_type)
            .ok_or_else(|| ProtocolError::GenericError("No price available".to_string()))?;
        let collateral_price_usd = UsdIcp::from(price);
        let ratio = rumi_protocol_backend::compute_collateral_ratio(vault, collateral_price_usd, s);
        let min_ratio = s.get_min_liquidation_ratio_for(&vault.collateral_type);

        if ratio >= min_ratio {
            return Err(ProtocolError::GenericError(format!(
                "Vault #{} is not liquidatable (CR {:.2}% >= {:.2}%)",
                vault_id, ratio.to_f64() * 100.0, min_ratio.to_f64() * 100.0
            )));
        }

        let max_liquidatable = vault.borrowed_icusd_amount * s.max_partial_liquidation_ratio;
        let actual = max_liquidatable.min(vault.borrowed_icusd_amount);

        if s.bot_budget_remaining_e8s < actual.to_u64() {
            return Err(ProtocolError::GenericError(format!(
                "Bot budget insufficient: {} remaining, need {}",
                s.bot_budget_remaining_e8s, actual.to_u64()
            )));
        }

        let decimals = s.get_collateral_config(&vault.collateral_type)
            .map(|c| c.decimals)
            .unwrap_or(8);
        let liq_bonus = s.get_liquidation_bonus_for(&vault.collateral_type);
        let collateral_raw = rumi_protocol_backend::numeric::icusd_to_collateral_amount(actual, price, decimals);
        let collateral_with_bonus = ICP::from(collateral_raw) * liq_bonus;
        let collateral_to_seize = collateral_with_bonus.min(ICP::from(vault.collateral_amount));

        Ok((collateral_price_usd, actual, collateral_to_seize, vault.collateral_type))
    })?;

    // TRANSFER FIRST — vault state is untouched until this succeeds
    match rumi_protocol_backend::management::transfer_collateral(
        collateral_to_seize.to_u64(), caller, collateral_type
    ).await {
        Ok(block) => {
            log!(INFO, "[bot_liquidate] Transferred {} collateral ({}) to bot for vault #{}, block {}",
                collateral_to_seize.to_u64(), collateral_type, vault_id, block);
        }
        Err(e) => {
            log!(INFO, "[bot_liquidate] Collateral transfer failed for vault #{}: {:?}", vault_id, e);
            return Err(ProtocolError::GenericError(format!("Collateral transfer failed: {:?}", e)));
        }
    }

    // Transfer succeeded — NOW mutate vault state
    mutate_state(|s| {
        if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
            vault.borrowed_icusd_amount -= liquidatable_debt;
            vault.collateral_amount -= collateral_to_seize.to_u64();
        }

        let event = rumi_protocol_backend::event::Event::PartialLiquidateVault {
            vault_id,
            liquidator_payment: liquidatable_debt,
            icp_to_liquidator: collateral_to_seize,
            liquidator: Some(caller),
            icp_rate: Some(collateral_price_usd),
            protocol_fee_collateral: None,
            timestamp: Some(ic_cdk::api::time()),
        };
        rumi_protocol_backend::storage::record_event(&event);

        let debt = liquidatable_debt.to_u64();
        s.bot_budget_remaining_e8s = s.bot_budget_remaining_e8s.saturating_sub(debt);
        s.bot_total_debt_covered_e8s += debt;
    });

    Ok(BotLiquidationResult {
        vault_id,
        collateral_amount: collateral_to_seize.to_u64(),
        debt_covered: liquidatable_debt.to_u64(),
        collateral_price_e8s: collateral_price_usd.to_e8s(),
    })
}
```

**Key changes:**
- Added collateral type allowlist guard
- Replaced `transfer_icp` with `transfer_collateral(..., collateral_type)`
- Moved `mutate_state` to AFTER the transfer succeeds

### 5. Fix `dev_force_bot_liquidate` — same three fixes

**File:** `src/rumi_protocol_backend/src/main.rs`, function `dev_force_bot_liquidate` (lines 1408–1477)

Replace the entire function body with:

```rust
async fn dev_force_bot_liquidate(vault_id: u64) -> Result<BotLiquidationResult, ProtocolError> {
    validate_call().await?;
    let caller = ic_cdk::caller();
    let is_authorized = read_state(|s| {
        s.developer_principal == caller
            || s.liquidation_bot_principal.map_or(false, |bp| bp == caller)
    });
    if !is_authorized {
        return Err(ProtocolError::GenericError("Only developer or bot can force bot liquidation".to_string()));
    }

    // Get vault info — NO CR check (force liquidation), but still check collateral allowlist
    let (collateral_price_usd, debt_to_cover, collateral_to_seize, collateral_type) = read_state(|s| {
        let vault = s.vault_id_to_vaults.get(&vault_id)
            .ok_or_else(|| ProtocolError::GenericError(format!("Vault #{} not found", vault_id)))?;

        // Guard: reject collateral types the bot isn't configured to handle
        if !s.bot_allowed_collateral_types.contains(&vault.collateral_type) {
            return Err(ProtocolError::GenericError(format!(
                "Collateral type {} is not in the bot's allowed list. Use stability pool for this vault.",
                vault.collateral_type
            )));
        }

        let price = s.get_collateral_price_decimal(&vault.collateral_type)
            .ok_or_else(|| ProtocolError::GenericError("No price available".to_string()))?;
        let collateral_price_usd = UsdIcp::from(price);
        let decimals = s.get_collateral_config(&vault.collateral_type)
            .map(|c| c.decimals)
            .unwrap_or(8);

        let debt = vault.borrowed_icusd_amount;
        let collateral_raw = rumi_protocol_backend::numeric::icusd_to_collateral_amount(debt, price, decimals);
        let liq_bonus = s.get_liquidation_bonus_for(&vault.collateral_type);
        let collateral_with_bonus = ICP::from(collateral_raw) * liq_bonus;
        let collateral_to_seize = collateral_with_bonus.min(ICP::from(vault.collateral_amount));

        Ok::<_, ProtocolError>((collateral_price_usd, debt, collateral_to_seize, vault.collateral_type))
    })?;

    // TRANSFER FIRST — vault state is untouched until this succeeds
    match rumi_protocol_backend::management::transfer_collateral(
        collateral_to_seize.to_u64(), caller, collateral_type
    ).await {
        Ok(block) => {
            log!(INFO, "[dev_force_bot_liquidate] Transferred {} collateral ({}) to caller, block {}",
                collateral_to_seize.to_u64(), collateral_type, block);
        }
        Err(e) => {
            log!(INFO, "[dev_force_bot_liquidate] Collateral transfer failed: {:?}", e);
            return Err(ProtocolError::GenericError(format!("Collateral transfer failed: {:?}", e)));
        }
    }

    // Transfer succeeded — NOW mutate vault state
    mutate_state(|s| {
        if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
            vault.borrowed_icusd_amount -= debt_to_cover;
            vault.collateral_amount -= collateral_to_seize.to_u64();
        }

        let event = rumi_protocol_backend::event::Event::PartialLiquidateVault {
            vault_id,
            liquidator_payment: debt_to_cover,
            icp_to_liquidator: collateral_to_seize,
            liquidator: Some(caller),
            icp_rate: Some(collateral_price_usd),
            protocol_fee_collateral: None,
            timestamp: Some(ic_cdk::api::time()),
        };
        rumi_protocol_backend::storage::record_event(&event);
    });

    log!(INFO, "[dev_force_bot_liquidate] Force-liquidated vault #{}: debt={}, collateral={}",
        vault_id, debt_to_cover.to_u64(), collateral_to_seize.to_u64());

    Ok(BotLiquidationResult {
        vault_id,
        collateral_amount: collateral_to_seize.to_u64(),
        debt_covered: debt_to_cover.to_u64(),
        collateral_price_e8s: collateral_price_usd.to_e8s(),
    })
}
```

### 6. Seed ICP into allowlist during `set_liquidation_bot_config`

**File:** `src/rumi_protocol_backend/src/main.rs`, function `set_liquidation_bot_config` (line 1272)

After line 1281 (`record_set_bot_budget`), add inside the `mutate_state` closure:

```rust
// Default: allow ICP if the allowlist is empty (first-time setup)
if s.bot_allowed_collateral_types.is_empty() {
    s.bot_allowed_collateral_types.insert(s.icp_ledger_principal);
}
```

This ensures existing deployments get ICP auto-added on the next `set_liquidation_bot_config` call, without overwriting a manually configured allowlist.

### 7. Build and verify compilation

```bash
cd /Users/robertripley/coding/rumi-protocol-v2
cargo build --target wasm32-unknown-unknown --release -p rumi_protocol_backend 2>&1
```

Expected: compiles cleanly with no errors.

### 8. Commit

Commit message: `fix(backend): make bot liquidation collateral-agnostic and fix premature state mutation`

---

## What This Does NOT Change

- **Bot canister** (`src/liquidation_bot/`): No changes. The bot still only knows how to swap ICP. When we want it to handle BOB/EXE, we update its swap logic and add those types to the backend allowlist.
- **Stability pool**: No changes needed. It already handles all collateral types correctly.
- **`check_vaults()` notification logic**: Still sends all unhealthy vaults to both bot and pool. The bot's `bot_liquidate` call will now cleanly reject non-allowed types, and the stability pool handles them independently.
- **Race condition between bot and pool**: Pre-existing concern, not introduced or worsened by this change.

## Testing After Deployment

1. Call `set_bot_allowed_collateral_types` with just the ICP ledger principal
2. Create a BOB vault, force it undercollateralized
3. Call `dev_force_bot_liquidate` on it — should get a clean rejection: "Collateral type X is not in the bot's allowed list"
4. Wait for `check_vaults()` or call `execute_liquidation` on the stability pool — pool should handle it
5. Create an ICP vault, force it undercollateralized
6. Call `dev_force_bot_liquidate` — should succeed, transfer ICP correctly, mutate state only after
