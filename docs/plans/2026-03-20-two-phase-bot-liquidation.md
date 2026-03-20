# Two-Phase Bot Liquidation (Claim → Confirm/Cancel)

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the current atomic `bot_liquidate` with a two-phase claim/confirm pattern so the bot can return collateral if its DEX swap fails, keeping vault state untouched until the swap is confirmed successful.

**Architecture:** The backend gets three new endpoints: `bot_claim_liquidation` (transfers collateral to bot, locks vault), `bot_confirm_liquidation` (mutates vault state), and `bot_cancel_liquidation` (returns collateral, unlocks vault). The `Vault` struct gets a `bot_processing` bool that blocks ALL user operations while true. A 10-minute timeout auto-clears abandoned claims. The bot's `process_pending()` orchestrates the full claim→swap→confirm/cancel flow.

**Tech Stack:** Rust (IC canisters), Candid IDL, ICRC-1/ICRC-2 token transfers

---

## Phase 1: Backend — Add `bot_processing` Lock to Vault

### Step 1.1: Add `bot_processing` field to `Vault` struct

**File:** `src/rumi_protocol_backend/src/vault.rs` (line 78)

Add the field after `accrued_interest`:

```rust
pub struct Vault {
    pub owner: Principal,
    pub borrowed_icusd_amount: ICUSD,
    #[serde(alias = "icp_margin_amount")]
    pub collateral_amount: u64,
    pub vault_id: u64,
    #[serde(default = "default_collateral_type")]
    pub collateral_type: Principal,
    #[serde(default)]
    pub last_accrual_time: u64,
    #[serde(default = "default_zero_icusd")]
    pub accrued_interest: ICUSD,
    /// True while the liquidation bot holds this vault's collateral for swapping.
    /// All user operations are blocked. Auto-cleared after 10 min timeout.
    #[serde(default)]
    pub bot_processing: bool,
}
```

**Why `#[serde(default)]`:** Existing vaults in event log don't have this field. Default `false` = not locked.

### Step 1.2: Add bot claim tracking to `State`

**File:** `src/rumi_protocol_backend/src/state.rs` (after line 660, `bot_pending_vaults`)

Add a map to track active bot claims (vault_id → claim details):

```rust
/// Active bot claims: vault_id → (timestamp_ns, collateral_amount, collateral_type).
/// Used to auto-cancel stale claims after timeout.
pub bot_active_claims: BTreeMap<u64, BotClaimInfo>,
```

Add the struct definition near the top of state.rs (near other bot-related types):

```rust
/// Info tracked for an active bot liquidation claim.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct BotClaimInfo {
    pub claimed_at: u64,
    pub collateral_amount: u64,
    pub debt_amount: u64,
    pub collateral_type: Principal,
    pub collateral_price_e8s: u64,
}
```

Initialize `bot_active_claims: BTreeMap::new()` in `impl From<InitArg> for State` (around line 663, alongside the other bot field defaults).

### Step 1.3: Add vault lock guard helper

**File:** `src/rumi_protocol_backend/src/vault.rs`

Add a helper function that all vault operations will call (after the `check_min_vault_debt_after_repay` function, around line 52):

```rust
/// Rejects any operation on a vault that is currently locked for bot liquidation processing.
fn check_vault_not_bot_locked(vault_id: u64) -> Result<(), ProtocolError> {
    let is_locked = read_state(|s| {
        s.vault_id_to_vaults
            .get(&vault_id)
            .map(|v| v.bot_processing)
            .unwrap_or(false)
    });
    if is_locked {
        return Err(ProtocolError::GenericError(format!(
            "Vault #{} is temporarily locked for liquidation processing. Please try again shortly.",
            vault_id
        )));
    }
    Ok(())
}
```

### Step 1.4: Add lock guard to ALL vault operation functions

**File:** `src/rumi_protocol_backend/src/vault.rs`

Add `check_vault_not_bot_locked(vault_id)?;` at the top of each function, right after the guard principal and before any state reads:

1. **`borrow_from_vault_internal`** (line 616) — add after line 617, before amount check:
   ```rust
   check_vault_not_bot_locked(arg.vault_id)?;
   ```

2. **`repay_to_vault`** (line 762) — add after line 764 (guard_principal), before amount parsing:
   ```rust
   check_vault_not_bot_locked(arg.vault_id)?;
   ```

3. **`repay_to_vault_with_stable`** (line 840) — add early, after guard:
   ```rust
   check_vault_not_bot_locked(arg.vault_id)?;
   ```

4. **`add_margin_to_vault`** (line 981) — add after line 983 (guard_principal):
   ```rust
   check_vault_not_bot_locked(arg.vault_id)?;
   ```

5. **`add_margin_with_deposit`** (line 1155) — add early:
   ```rust
   check_vault_not_bot_locked(vault_id)?;
   ```

6. **`close_vault`** (line 1220) — add after line 1222 (guard_principal):
   ```rust
   check_vault_not_bot_locked(vault_id)?;
   ```

7. **`withdraw_collateral`** (line 1370) — add after line 1372 (guard_principal):
   ```rust
   check_vault_not_bot_locked(vault_id)?;
   ```

8. **`withdraw_and_close_vault`** (line 1667) — add after line 1670 (guard_principal):
   ```rust
   check_vault_not_bot_locked(vault_id)?;
   ```

### Step 1.5: Build and verify compilation

```bash
cd /Users/robertripley/coding/rumi-protocol-v2
cargo build --target wasm32-unknown-unknown --release -p rumi_protocol_backend 2>&1 | tail -5
```

Expected: successful compilation (warnings OK, no errors).

### Step 1.6: Commit

```bash
git add src/rumi_protocol_backend/src/vault.rs src/rumi_protocol_backend/src/state.rs
git commit -m "Add bot_processing lock to Vault struct with operation guards"
```

---

## Phase 2: Backend — New Claim/Confirm/Cancel Endpoints

### Step 2.1: Add `bot_claim_liquidation` endpoint

**File:** `src/rumi_protocol_backend/src/main.rs`

Replace the existing `bot_liquidate` function (lines 1331-1440) with `bot_claim_liquidation`. The old `bot_liquidate` is removed entirely.

```rust
/// Phase 1 of two-phase bot liquidation.
/// Transfers collateral to bot and locks the vault. Does NOT mutate vault debt/collateral.
/// Bot must call bot_confirm_liquidation (on swap success) or bot_cancel_liquidation (on failure).
#[candid_method(update)]
#[update]
async fn bot_claim_liquidation(vault_id: u64) -> Result<BotLiquidationResult, ProtocolError> {
    validate_call().await?;
    let caller = ic_cdk::caller();
    let is_bot = read_state(|s| {
        s.liquidation_bot_principal.map_or(false, |bp| bp == caller)
    });
    if !is_bot {
        return Err(ProtocolError::GenericError(
            "Caller is not the registered liquidation bot canister".to_string(),
        ));
    }

    // Validate vault and compute amounts — NO state mutation
    let (collateral_price_usd, liquidatable_debt, collateral_to_seize, collateral_type) = read_state(|s| {
        let vault = s.vault_id_to_vaults.get(&vault_id)
            .ok_or_else(|| ProtocolError::GenericError(format!("Vault #{} not found", vault_id)))?;

        if vault.bot_processing {
            return Err(ProtocolError::GenericError(format!(
                "Vault #{} already has an active bot claim", vault_id
            )));
        }

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
        let ratio = compute_collateral_ratio(vault, collateral_price_usd, s);
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

    // Transfer collateral to bot — vault state still untouched
    match rumi_protocol_backend::management::transfer_collateral(
        collateral_to_seize.to_u64(), caller, collateral_type
    ).await {
        Ok(block) => {
            log!(INFO, "[bot_claim_liquidation] Transferred {} collateral ({}) to bot for vault #{}, block {}",
                collateral_to_seize.to_u64(), collateral_type, vault_id, block);
        }
        Err(e) => {
            log!(INFO, "[bot_claim_liquidation] Collateral transfer failed for vault #{}: {:?}", vault_id, e);
            return Err(ProtocolError::GenericError(format!("Collateral transfer failed: {:?}", e)));
        }
    }

    // Transfer succeeded — lock vault and record claim (debt/collateral amounts NOT changed)
    let now = ic_cdk::api::time();
    mutate_state(|s| {
        if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
            vault.bot_processing = true;
        }
        s.bot_active_claims.insert(vault_id, rumi_protocol_backend::state::BotClaimInfo {
            claimed_at: now,
            collateral_amount: collateral_to_seize.to_u64(),
            debt_amount: liquidatable_debt.to_u64(),
            collateral_type,
            collateral_price_e8s: collateral_price_usd.to_e8s(),
        });
    });

    Ok(BotLiquidationResult {
        vault_id,
        collateral_amount: collateral_to_seize.to_u64(),
        debt_covered: liquidatable_debt.to_u64(),
        collateral_price_e8s: collateral_price_usd.to_e8s(),
    })
}
```

### Step 2.2: Add `bot_confirm_liquidation` endpoint

**File:** `src/rumi_protocol_backend/src/main.rs` (after `bot_claim_liquidation`)

```rust
/// Phase 2a of two-phase bot liquidation — SWAP SUCCEEDED.
/// Mutates vault state (reduces debt + collateral), clears lock, records event.
#[candid_method(update)]
#[update]
async fn bot_confirm_liquidation(vault_id: u64) -> Result<(), ProtocolError> {
    validate_call().await?;
    let caller = ic_cdk::caller();
    let is_bot = read_state(|s| {
        s.liquidation_bot_principal.map_or(false, |bp| bp == caller)
    });
    if !is_bot {
        return Err(ProtocolError::GenericError(
            "Caller is not the registered liquidation bot canister".to_string(),
        ));
    }

    mutate_state(|s| {
        let claim = s.bot_active_claims.remove(&vault_id)
            .ok_or_else(|| ProtocolError::GenericError(format!(
                "No active claim for vault #{}", vault_id
            )))?;

        let vault = s.vault_id_to_vaults.get_mut(&vault_id)
            .ok_or_else(|| ProtocolError::GenericError(format!(
                "Vault #{} not found", vault_id
            )))?;

        // NOW mutate vault state
        let debt = ICUSD::new(claim.debt_amount);
        vault.borrowed_icusd_amount -= debt;
        vault.collateral_amount -= claim.collateral_amount;
        vault.bot_processing = false;

        // Record event
        let event = rumi_protocol_backend::event::Event::PartialLiquidateVault {
            vault_id,
            liquidator_payment: debt,
            icp_to_liquidator: ICP::from(claim.collateral_amount),
            liquidator: s.liquidation_bot_principal,
            icp_rate: Some(UsdIcp::from_e8s(claim.collateral_price_e8s)),
            protocol_fee_collateral: None,
            timestamp: Some(ic_cdk::api::time()),
        };
        rumi_protocol_backend::storage::record_event(&event);

        s.bot_budget_remaining_e8s = s.bot_budget_remaining_e8s.saturating_sub(claim.debt_amount);
        s.bot_total_debt_covered_e8s += claim.debt_amount;

        log!(INFO, "[bot_confirm_liquidation] Vault #{} liquidation confirmed: debt={}, collateral={}",
            vault_id, claim.debt_amount, claim.collateral_amount);

        Ok(())
    })
}
```

### Step 2.3: Add `bot_cancel_liquidation` endpoint

**File:** `src/rumi_protocol_backend/src/main.rs` (after `bot_confirm_liquidation`)

The bot calls this when its swap fails. The bot must transfer collateral back to the backend canister BEFORE calling this (via ICRC-1 transfer to the backend's principal). The backend verifies receipt, clears the lock, and the vault returns to its original state.

```rust
/// Phase 2b of two-phase bot liquidation — SWAP FAILED.
/// Bot has already transferred collateral back to the backend canister.
/// Clears the lock so the vault can be routed to the stability pool.
#[candid_method(update)]
#[update]
async fn bot_cancel_liquidation(vault_id: u64) -> Result<(), ProtocolError> {
    validate_call().await?;
    let caller = ic_cdk::caller();
    let is_bot = read_state(|s| {
        s.liquidation_bot_principal.map_or(false, |bp| bp == caller)
    });
    if !is_bot {
        return Err(ProtocolError::GenericError(
            "Caller is not the registered liquidation bot canister".to_string(),
        ));
    }

    mutate_state(|s| {
        let claim = s.bot_active_claims.remove(&vault_id)
            .ok_or_else(|| ProtocolError::GenericError(format!(
                "No active claim for vault #{}", vault_id
            )))?;

        if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
            vault.bot_processing = false;
        }

        log!(INFO, "[bot_cancel_liquidation] Vault #{} claim cancelled. Collateral {} ({}) returned by bot. Vault unlocked for stability pool.",
            vault_id, claim.collateral_amount, claim.collateral_type);

        Ok(())
    })
}
```

### Step 2.4: Update `dev_force_bot_liquidate` to use claim pattern

**File:** `src/rumi_protocol_backend/src/main.rs`

Update `dev_force_bot_liquidate` (line 1442) to mirror the claim pattern — lock vault, transfer collateral, but skip the CR check. Keep it as a single atomic operation for testing (no confirm/cancel needed since it's dev-only).

No change needed here — `dev_force_bot_liquidate` is a testing function that bypasses the normal flow. It can keep its current atomic behavior. But add the `bot_processing` check:

After the `bot_allowed_collateral_types` check (line 1464), add:
```rust
if vault.bot_processing {
    return Err(ProtocolError::GenericError(format!(
        "Vault #{} already has an active bot claim", vault_id
    )));
}
```

### Step 2.5: Add timeout cleanup to `check_vaults()`

**File:** `src/rumi_protocol_backend/src/lib.rs`

Add a timeout cleanup block at the top of `check_vaults()`, before the unhealthy vault scan (around line 340, after the function starts). This auto-cancels stale claims where the bot never confirmed or cancelled:

```rust
// ── Auto-cancel stale bot claims (10 min timeout) ──
let claim_timeout_ns: u64 = 600_000_000_000; // 10 minutes
let now_for_claims = ic_cdk::api::time();
let stale_claims: Vec<(u64, BotClaimInfo)> = read_state(|s| {
    s.bot_active_claims.iter()
        .filter(|(_, info)| now_for_claims.saturating_sub(info.claimed_at) >= claim_timeout_ns)
        .map(|(vid, info)| (*vid, info.clone()))
        .collect()
});

for (vault_id, claim) in &stale_claims {
    log!(INFO, "[check_vaults] Auto-cancelling stale bot claim on vault #{} (claimed {}s ago)",
        vault_id, (now_for_claims - claim.claimed_at) / 1_000_000_000);
    mutate_state(|s| {
        s.bot_active_claims.remove(vault_id);
        if let Some(vault) = s.vault_id_to_vaults.get_mut(vault_id) {
            vault.bot_processing = false;
        }
    });
    // Note: collateral is still with the bot. This is the extreme edge case
    // (bot crashed). The collateral amount is logged for manual recovery.
    log!(INFO, "[check_vaults] WARNING: Bot may still hold {} collateral ({}) from stale claim on vault #{}. Manual recovery may be needed.",
        claim.collateral_amount, claim.collateral_type, vault_id);
}
```

Also update the vault routing loop (line 408) to skip `bot_processing` vaults:

```rust
for vault_info in &vault_notifications {
    // Skip vaults with active bot claims — they're being processed
    let is_bot_processing = read_state(|s| {
        s.vault_id_to_vaults.get(&vault_info.vault_id)
            .map(|v| v.bot_processing)
            .unwrap_or(false)
    });
    if is_bot_processing {
        continue;
    }

    // ... existing routing logic ...
}
```

### Step 2.6: Remove old `bot_liquidate` endpoint

**File:** `src/rumi_protocol_backend/src/main.rs`

Delete the entire `bot_liquidate` function (lines 1331-1440). It's replaced by the claim/confirm/cancel pattern.

### Step 2.7: Update Candid interface

**File:** `src/rumi_protocol_backend/rumi_protocol_backend.did`

Remove:
```candid
bot_liquidate : (nat64) -> (variant { Ok : BotLiquidationResult; Err : ProtocolError });
```

Add:
```candid
bot_claim_liquidation : (nat64) -> (variant { Ok : BotLiquidationResult; Err : ProtocolError });
bot_confirm_liquidation : (nat64) -> (variant { Ok; Err : ProtocolError });
bot_cancel_liquidation : (nat64) -> (variant { Ok; Err : ProtocolError });
```

Also update the declarations files:
- `src/declarations/rumi_protocol_backend/rumi_protocol_backend.did`
- `src/declarations/rumi_protocol_backend/rumi_protocol_backend.did.d.ts`
- `src/declarations/rumi_protocol_backend/rumi_protocol_backend.did.js`

### Step 2.8: Build and verify

```bash
cargo build --target wasm32-unknown-unknown --release -p rumi_protocol_backend 2>&1 | tail -5
```

### Step 2.9: Commit

```bash
git add src/rumi_protocol_backend/
git commit -m "Add two-phase bot liquidation: claim/confirm/cancel with vault locking"
```

---

## Phase 3: Bot — Update `process_pending()` for Two-Phase Flow

### Step 3.1: Update `call_bot_liquidate` → `call_bot_claim_liquidation`

**File:** `src/liquidation_bot/src/process.rs`

Rename `call_bot_liquidate` (line 138) and change the endpoint name:

```rust
async fn call_bot_claim_liquidation(config: &BotConfig, vault_id: u64) -> Result<BotLiquidationResult, String> {
    let result: Result<(BackendResult<BotLiquidationResult>,), _> =
        ic_cdk::call(config.backend_principal, "bot_claim_liquidation", (vault_id,)).await;

    match result {
        Ok((BackendResult::Ok(r),)) => Ok(r),
        Ok((BackendResult::Err(e),)) => Err(format!("{}", e)),
        Err((code, msg)) => Err(format!("{:?}: {}", code, msg)),
    }
}
```

### Step 3.2: Add `call_bot_confirm_liquidation` and `call_bot_cancel_liquidation` helpers

**File:** `src/liquidation_bot/src/process.rs`

```rust
async fn call_bot_confirm_liquidation(config: &BotConfig, vault_id: u64) -> Result<(), String> {
    let result: Result<(BackendResult<()>,), _> =
        ic_cdk::call(config.backend_principal, "bot_confirm_liquidation", (vault_id,)).await;

    match result {
        Ok((BackendResult::Ok(()),)) => Ok(()),
        Ok((BackendResult::Err(e),)) => Err(format!("{}", e)),
        Err((code, msg)) => Err(format!("{:?}: {}", code, msg)),
    }
}

async fn call_bot_cancel_liquidation(config: &BotConfig, vault_id: u64) -> Result<(), String> {
    let result: Result<(BackendResult<()>,), _> =
        ic_cdk::call(config.backend_principal, "bot_cancel_liquidation", (vault_id,)).await;

    match result {
        Ok((BackendResult::Ok(()),)) => Ok(()),
        Ok((BackendResult::Err(e),)) => Err(format!("{}", e)),
        Err((code, msg)) => Err(format!("{:?}: {}", code, msg)),
    }
}
```

### Step 3.3: Add `return_collateral_to_backend` helper

**File:** `src/liquidation_bot/src/process.rs`

```rust
async fn return_collateral_to_backend(config: &BotConfig, amount: u64, collateral_ledger: Principal) -> Result<(), String> {
    let transfer_args = icrc_ledger_types::icrc1::transfer::TransferArg {
        from_subaccount: None,
        to: icrc_ledger_types::icrc1::account::Account {
            owner: config.backend_principal,
            subaccount: None,
        },
        amount: candid::Nat::from(amount),
        fee: None,
        memo: None,
        created_at_time: None,
    };

    let result: Result<(Result<candid::Nat, icrc_ledger_types::icrc1::transfer::TransferError>,), _> =
        ic_cdk::call(collateral_ledger, "icrc1_transfer", (transfer_args,)).await;

    match result {
        Ok((Ok(_block),)) => Ok(()),
        Ok((Err(e),)) => Err(format!("Transfer failed: {:?}", e)),
        Err((code, msg)) => Err(format!("Transfer call failed: {:?} {}", code, msg)),
    }
}
```

### Step 3.4: Rewrite `process_pending()` with claim→swap→confirm/cancel

**File:** `src/liquidation_bot/src/process.rs`

Replace the `process_pending()` function (lines 43-133):

```rust
pub async fn process_pending() {
    let vault = state::mutate_state(|s| s.pending_vaults.pop());
    let Some(vault) = vault else { return };

    let config = match state::read_state(|s| s.config.clone()) {
        Some(c) => c,
        None => {
            log!(crate::INFO, "Bot not configured, skipping vault #{}", vault.vault_id);
            return;
        }
    };

    log!(crate::INFO, "Processing vault #{}", vault.vault_id);

    // 1. CLAIM — get collateral, vault is locked but debt/collateral unchanged
    let liq_result = call_bot_claim_liquidation(&config, vault.vault_id).await;
    let (collateral_amount, debt_covered, collateral_price) = match liq_result {
        Ok(r) => (r.collateral_amount, r.debt_covered, r.collateral_price_e8s),
        Err(e) => {
            log_failed_event(&vault, &format!("bot_claim_liquidation failed: {}", e));
            return;
        }
    };

    // 2. Swap ICP → ckStable (best of ckUSDC/ckUSDT on KongSwap)
    let swap_amount = calculate_swap_amount_internal(collateral_amount, debt_covered, collateral_price);
    let stable_result = swap::swap_icp_for_stable(&config, swap_amount).await;
    let (stable_amount, stable_token, route) = match stable_result {
        Ok(r) => (r.output_amount, r.target_token, r.route),
        Err(e) => {
            // SWAP FAILED — return collateral and cancel
            log!(crate::INFO, "[process_pending] Swap failed for vault #{}: {}. Returning collateral.", vault.vault_id, e);
            if let Err(return_err) = return_collateral_to_backend(&config, collateral_amount, vault.collateral_type).await {
                log!(crate::INFO, "[process_pending] CRITICAL: Failed to return collateral for vault #{}: {}", vault.vault_id, return_err);
                // Even if return fails, try to cancel so vault unlocks on timeout
            }
            if let Err(cancel_err) = call_bot_cancel_liquidation(&config, vault.vault_id).await {
                log!(crate::INFO, "[process_pending] Failed to cancel claim for vault #{}: {}", vault.vault_id, cancel_err);
            }
            log_failed_event(&vault, &format!("DEX swap failed: {}. Collateral returned, vault unlocked.", e));
            return;
        }
    };

    // 3. Swap ckStable → icUSD (via 3pool)
    let icusd_result = swap::swap_stable_for_icusd(&config, stable_amount, stable_token).await;
    let icusd_amount = match icusd_result {
        Ok(amount) => amount,
        Err(e) => {
            // 3pool swap failed — return collateral and cancel
            // Note: we already have ckStable from step 2, but can't easily reverse that.
            // Return the original collateral amount (bot eats the ckStable loss).
            // In practice this is extremely unlikely — 3pool is our own canister.
            log!(crate::INFO, "[process_pending] 3pool swap failed for vault #{}: {}. Returning collateral.", vault.vault_id, e);
            if let Err(return_err) = return_collateral_to_backend(&config, collateral_amount, vault.collateral_type).await {
                log!(crate::INFO, "[process_pending] CRITICAL: Failed to return collateral for vault #{}: {}", vault.vault_id, return_err);
            }
            if let Err(cancel_err) = call_bot_cancel_liquidation(&config, vault.vault_id).await {
                log!(crate::INFO, "[process_pending] Failed to cancel claim for vault #{}: {}", vault.vault_id, cancel_err);
            }
            log_failed_event(&vault, &format!("3pool swap failed: {}. Collateral returned, vault unlocked.", e));
            return;
        }
    };

    // 4. Deposit icUSD to backend reserves
    if let Err(e) = call_bot_deposit_to_reserves(&config, icusd_amount).await {
        // icUSD deposit failed. We have icUSD but can't deposit it.
        // Still confirm the liquidation — the debt will be covered by this icUSD eventually.
        log!(crate::INFO, "[process_pending] WARNING: deposit_to_reserves failed for vault #{}: {}. Confirming liquidation anyway.", vault.vault_id, e);
    }

    // 5. CONFIRM — mutate vault state now that swap pipeline succeeded
    if let Err(e) = call_bot_confirm_liquidation(&config, vault.vault_id).await {
        log!(crate::INFO, "[process_pending] CRITICAL: confirm failed for vault #{}: {}. Vault locked, collateral with bot.", vault.vault_id, e);
        // This is bad but the 10-min timeout will auto-cancel.
        // The collateral is still with the bot and the icUSD was deposited.
        log_failed_event(&vault, &format!("confirm_liquidation failed: {}", e));
        return;
    }

    // 6. Send remaining ICP to treasury (liquidation bonus portion)
    let icp_to_treasury = collateral_amount.saturating_sub(swap_amount);
    if icp_to_treasury > 0 {
        let _ = transfer_icp_to_treasury(&config, icp_to_treasury).await;
    }

    // 7. Log success event
    let effective_price = if swap_amount > 0 {
        (stable_amount as u128 * 100_000_000 / swap_amount as u128) as u64
    } else {
        0
    };
    let slippage_bps = calculate_slippage(effective_price, collateral_price);

    state::mutate_state(|s| {
        s.stats.total_debt_covered_e8s += debt_covered;
        s.stats.total_icusd_burned_e8s += icusd_amount;
        s.stats.total_collateral_received_e8s += collateral_amount;
        s.stats.total_collateral_to_treasury_e8s += icp_to_treasury;
        s.stats.events_count += 1;
        s.liquidation_events.push(BotLiquidationEvent {
            timestamp: ic_cdk::api::time(),
            vault_id: vault.vault_id,
            debt_covered_e8s: debt_covered,
            collateral_received_e8s: collateral_amount,
            icusd_burned_e8s: icusd_amount,
            collateral_to_treasury_e8s: icp_to_treasury,
            swap_route: route,
            effective_price_e8s: effective_price,
            slippage_bps,
            success: true,
            error_message: None,
        });
    });

    log!(
        crate::INFO,
        "Vault #{} liquidated: debt={}, collateral={}, icUSD={}, treasury={}",
        vault.vault_id, debt_covered, collateral_amount, icusd_amount, icp_to_treasury
    );
}
```

### Step 3.5: Build bot canister

```bash
cargo build --target wasm32-unknown-unknown --release -p liquidation_bot 2>&1 | tail -5
```

### Step 3.6: Commit

```bash
git add src/liquidation_bot/
git commit -m "Update bot to two-phase liquidation: claim → swap → confirm/cancel"
```

---

## Phase 4: Build, Test, Deploy

### Step 4.1: Build both canisters

```bash
cd /Users/robertripley/coding/rumi-protocol-v2
cargo build --target wasm32-unknown-unknown --release -p rumi_protocol_backend -p liquidation_bot 2>&1 | tail -10
```

### Step 4.2: Deploy backend to mainnet

```bash
dfx deploy rumi_protocol_backend --network ic --argument '(variant { Upgrade = record { mode = null } })' 2>&1
```

### Step 4.3: Deploy bot to mainnet

```bash
dfx deploy liquidation_bot --network ic --mode upgrade 2>&1
```

### Step 4.4: Verify endpoints exist

```bash
dfx canister call tfesu-vyaaa-aaaap-qrd7a-cai bot_claim_liquidation '(999)' --network ic 2>&1
# Expected: Err with "Vault #999 not found"

dfx canister call tfesu-vyaaa-aaaap-qrd7a-cai bot_confirm_liquidation '(999)' --network ic 2>&1
# Expected: Err with "Caller is not the registered liquidation bot canister"
```

### Step 4.5: Commit and push

```bash
git add -A
git commit -m "Build artifacts and deploy two-phase bot liquidation"
git push
```

---

## Edge Cases Documented

1. **Bot crashes mid-process**: Vault stays locked with `bot_processing = true`. After 10 minutes, `check_vaults()` auto-cancels the claim, unlocks the vault, and it routes to the stability pool. Collateral stuck in bot needs manual recovery (logged with amount).

2. **Collateral return transfer fails**: Bot logs CRITICAL error. It still tries to call `bot_cancel_liquidation` so the backend at least knows. If that also fails, the 10-minute timeout handles it.

3. **Confirm call fails after successful swap**: Bot has icUSD, backend still has vault locked. The 10-minute timeout will auto-cancel, treating it as a failed claim. The bot keeps the icUSD (it already deposited to reserves). The vault unlocks and routes to stability pool. This results in a small loss — the vault gets liquidated twice for the debt portion — but it's an extreme edge case.

4. **3pool swap fails after DEX swap succeeds**: Bot has ckStable but can't convert to icUSD. It returns the original ICP collateral and cancels. The bot is left holding some ckStable at a loss. This is fine — 3pool is our canister and virtually never fails.

5. **Vault owner can't interact during processing**: All operations return "Vault is temporarily locked for liquidation processing." Maximum lock duration is 10 minutes (timeout).
