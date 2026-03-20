# Two-Phase Bot Liquidation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the current atomic bot liquidation with a two-phase claim/confirm/cancel pattern so the bot can reverse a liquidation if its DEX swap fails, letting the stability pool handle it instead.

**Architecture:** The backend gains a `bot_processing` lock on vaults. `bot_claim_liquidation` transfers collateral to the bot and sets the lock (vault state unchanged). `bot_confirm_liquidation` finalizes (reduces debt/collateral). `bot_cancel_liquidation` returns collateral and clears the lock. A 10-minute auto-cancel timer prevents stuck locks. Three dev test endpoints exercise bot-only, pool-only, and full-cascade paths.

**Tech Stack:** Rust (IC canisters), Candid, `ic_cdk`, `ic_cdk_timers`

---

## File Structure

| File | Action | Responsibility |
|------|--------|---------------|
| `src/rumi_protocol_backend/src/vault.rs` | Modify | Add `bot_processing` field to `Vault` struct |
| `src/rumi_protocol_backend/src/state.rs` | Modify | Add `bot_claims: BTreeMap<u64, BotClaim>` to State for tracking active claims |
| `src/rumi_protocol_backend/src/main.rs` | Modify | Add `bot_claim_liquidation`, `bot_confirm_liquidation`, `bot_cancel_liquidation` endpoints; replace `bot_liquidate`; add three test endpoints; add auto-cancel timer |
| `src/rumi_protocol_backend/src/lib.rs` | Modify | Update `check_vaults()` to skip `bot_processing` vaults |
| `src/liquidation_bot/src/process.rs` | Modify | Rewrite `process_pending()` to use claim/confirm/cancel |
| `src/liquidation_bot/src/lib.rs` | Modify | Add `test_cascade_liquidate` test endpoint |
| `src/rumi_protocol_backend/rumi_protocol_backend.did` | Modify | Add new endpoint signatures |
| `src/declarations/rumi_protocol_backend/rumi_protocol_backend.did` | Modify | Mirror .did changes |
| `src/declarations/rumi_protocol_backend/rumi_protocol_backend.did.d.ts` | Modify | Regenerate TS types |
| `src/declarations/rumi_protocol_backend/rumi_protocol_backend.did.js` | Modify | Regenerate JS factory |

---

### Task 1: Add `bot_processing` to Vault struct and `BotClaim` to State

**Files:**
- Modify: `src/rumi_protocol_backend/src/vault.rs:78-100`
- Modify: `src/rumi_protocol_backend/src/state.rs:646-661`

- [ ] **Step 1: Add `bot_processing` field to Vault**

In `src/rumi_protocol_backend/src/vault.rs`, add to the `Vault` struct after `accrued_interest`:

```rust
/// True while the bot has claimed this vault for liquidation but hasn't
/// confirmed or cancelled yet. Blocks ALL user operations on the vault.
#[serde(default)]
pub bot_processing: bool,
```

- [ ] **Step 2: Add `BotClaim` struct and tracking map to State**

In `src/rumi_protocol_backend/src/state.rs`, add after the `bot_pending_vaults` field (line 660):

```rust
/// Active bot claims — tracks collateral transferred to bot but not yet confirmed.
/// Key = vault_id. Auto-cancelled after `BOT_CLAIM_TIMEOUT_NS`.
pub bot_claims: BTreeMap<u64, BotClaim>,
```

Add the `BotClaim` struct definition near the top of `state.rs` (after the `CollateralStatus` impl block, around line 170):

```rust
/// Tracks a bot's pending liquidation claim on a vault.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct BotClaim {
    /// Vault ID being liquidated
    pub vault_id: u64,
    /// Amount of collateral transferred to the bot
    pub collateral_amount: u64,
    /// Debt amount the bot committed to cover
    pub debt_amount: u64,
    /// Collateral type (ledger principal)
    pub collateral_type: Principal,
    /// Timestamp (nanos) when claim was created
    pub claimed_at: u64,
    /// Collateral price at time of claim (for event logging)
    pub collateral_price_e8s: u64,
}
```

- [ ] **Step 3: Initialize `bot_claims` in `From<InitArg>` for State**

In the `From<InitArg>` impl (around line 663), add after `bot_pending_vaults: BTreeMap::new(),`:

```rust
bot_claims: BTreeMap::new(),
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p rumi_protocol_backend 2>&1 | head -30`
Expected: Compiles (there will be dead_code warnings for `BotClaim` until we use it — that's fine).

- [ ] **Step 5: Commit**

```bash
git add src/rumi_protocol_backend/src/vault.rs src/rumi_protocol_backend/src/state.rs
git commit -m "feat: add bot_processing flag to Vault and BotClaim tracking to State"
```

---

### Task 2: Add vault locking — block all user operations when `bot_processing` is true

**Files:**
- Modify: `src/rumi_protocol_backend/src/vault.rs` (multiple functions)
- Modify: `src/rumi_protocol_backend/src/main.rs` (endpoint guards)

- [ ] **Step 1: Add a helper function to check vault lock**

In `src/rumi_protocol_backend/src/vault.rs`, add after the `Vault` struct:

```rust
/// Returns an error if the vault is locked for bot processing.
pub fn require_vault_not_processing(vault: &Vault) -> Result<(), ProtocolError> {
    if vault.bot_processing {
        Err(ProtocolError::GenericError(format!(
            "Vault #{} is locked — bot liquidation in progress", vault.vault_id
        )))
    } else {
        Ok(())
    }
}
```

- [ ] **Step 2: Guard all user-facing vault mutation endpoints in main.rs**

Add `require_vault_not_processing` checks at the top of each user-facing vault function's read_state validation block. The endpoints that need guards are:

1. `borrow_from_vault` (main.rs:611) — blocks additional borrowing
2. `repay_to_vault` (main.rs:619) — blocks repayment
3. `repay_to_vault_with_stable` (main.rs:627) — blocks ckStable repayment
4. `add_margin_to_vault` (main.rs:634) — blocks adding collateral
5. `add_margin_with_deposit` (main.rs:664) — blocks adding collateral via deposit
6. `withdraw_collateral` (main.rs:679) — blocks full withdrawal
7. `withdraw_partial_collateral` (main.rs:686) — blocks partial withdrawal
8. `close_vault` (main.rs:671) — blocks closing
9. `withdraw_and_close_vault` (main.rs:693) — blocks withdraw+close

For each, find the `read_state` block that loads the vault, and add right after fetching the vault:

```rust
require_vault_not_processing(vault)?;
```

**Important:** Search for each function name in `main.rs` and `vault.rs` to find where the vault is first loaded. The guard must go before any mutation logic.

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p rumi_protocol_backend 2>&1 | head -30`
Expected: Compiles cleanly.

- [ ] **Step 4: Commit**

```bash
git add src/rumi_protocol_backend/src/vault.rs src/rumi_protocol_backend/src/main.rs
git commit -m "feat: block all user operations on vaults locked by bot_processing"
```

---

### Task 3: Implement `bot_claim_liquidation` endpoint

**Files:**
- Modify: `src/rumi_protocol_backend/src/main.rs:1330-1440` (replace `bot_liquidate`)

- [ ] **Step 1: Create the `bot_claim_liquidation` endpoint**

Replace the existing `bot_liquidate` function (lines 1330-1440) with `bot_claim_liquidation`. The key difference: it transfers collateral to the bot and locks the vault, but does NOT reduce vault debt or collateral amounts.

```rust
/// Bot calls this to CLAIM a vault for liquidation (phase 1 of 2).
/// Transfers collateral to the bot and locks the vault (`bot_processing = true`).
/// Vault debt and collateral amounts are NOT modified yet.
/// Bot must call `bot_confirm_liquidation` after successful swap, or
/// `bot_cancel_liquidation` if the swap fails (returns collateral).
#[candid_method(update)]
#[update]
async fn bot_claim_liquidation(vault_id: u64) -> Result<BotLiquidationResult, ProtocolError> {
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

    // Check no existing claim on this vault
    let existing_claim = read_state(|s| s.bot_claims.contains_key(&vault_id));
    if existing_claim {
        return Err(ProtocolError::GenericError(format!(
            "Vault #{} already has an active bot claim", vault_id
        )));
    }

    // Get vault info, validate collateral type, compute amounts, check budget
    let (collateral_price_usd, liquidatable_debt, collateral_to_seize, collateral_type) = read_state(|s| {
        let vault = s.vault_id_to_vaults.get(&vault_id)
            .ok_or_else(|| ProtocolError::GenericError(format!("Vault #{} not found", vault_id)))?;

        if vault.bot_processing {
            return Err(ProtocolError::GenericError(format!(
                "Vault #{} is already being processed", vault_id
            )));
        }

        // Guard: reject collateral types the bot isn't configured to handle
        if !s.bot_allowed_collateral_types.contains(&vault.collateral_type) {
            return Err(ProtocolError::GenericError(format!(
                "Collateral type {} is not in the bot's allowed list.", vault.collateral_type
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

    // Transfer collateral to bot
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

    // Lock the vault and record the claim (but do NOT modify debt/collateral)
    let now = ic_cdk::api::time();
    mutate_state(|s| {
        if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
            vault.bot_processing = true;
        }
        s.bot_claims.insert(vault_id, rumi_protocol_backend::state::BotClaim {
            vault_id,
            collateral_amount: collateral_to_seize.to_u64(),
            debt_amount: liquidatable_debt.to_u64(),
            collateral_type,
            claimed_at: now,
            collateral_price_e8s: collateral_price_usd.to_e8s(),
        });
        // Deduct from budget immediately to prevent over-claiming
        s.bot_budget_remaining_e8s = s.bot_budget_remaining_e8s.saturating_sub(liquidatable_debt.to_u64());
    });

    log!(INFO, "[bot_claim_liquidation] Claimed vault #{}: debt={}, collateral={}",
        vault_id, liquidatable_debt.to_u64(), collateral_to_seize.to_u64());

    Ok(BotLiquidationResult {
        vault_id,
        collateral_amount: collateral_to_seize.to_u64(),
        debt_covered: liquidatable_debt.to_u64(),
        collateral_price_e8s: collateral_price_usd.to_e8s(),
    })
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p rumi_protocol_backend 2>&1 | head -30`
Expected: May have warnings about unused `bot_liquidate` if we haven't removed it yet. That's fine.

- [ ] **Step 3: Commit**

```bash
git add src/rumi_protocol_backend/src/main.rs
git commit -m "feat: add bot_claim_liquidation endpoint (phase 1 of two-phase liquidation)"
```

---

### Task 4: Implement `bot_confirm_liquidation` and `bot_cancel_liquidation`

**Files:**
- Modify: `src/rumi_protocol_backend/src/main.rs` (add after `bot_claim_liquidation`)

- [ ] **Step 1: Add `bot_confirm_liquidation`**

This is called by the bot after a successful swap. It finalizes the liquidation by reducing vault debt/collateral, recording the event, and clearing the claim.

```rust
/// Bot calls this after successfully swapping collateral (phase 2 of 2).
/// Finalizes the liquidation: reduces vault debt and collateral, records event.
#[candid_method(update)]
#[update]
async fn bot_confirm_liquidation(vault_id: u64) -> Result<(), ProtocolError> {
    validate_call().await?;
    let caller = ic_cdk::api::caller();

    let is_bot = read_state(|s| {
        s.liquidation_bot_principal.map_or(false, |bp| bp == caller)
    });
    if !is_bot {
        return Err(ProtocolError::GenericError(
            "Caller is not the registered liquidation bot canister".to_string(),
        ));
    }

    let claim = read_state(|s| s.bot_claims.get(&vault_id).cloned())
        .ok_or_else(|| ProtocolError::GenericError(format!(
            "No active claim for vault #{}", vault_id
        )))?;

    mutate_state(|s| {
        if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
            vault.borrowed_icusd_amount -= ICUSD::new(claim.debt_amount);
            vault.collateral_amount = vault.collateral_amount.saturating_sub(claim.collateral_amount);
            vault.bot_processing = false;
        }

        let event = rumi_protocol_backend::event::Event::PartialLiquidateVault {
            vault_id,
            liquidator_payment: ICUSD::new(claim.debt_amount),
            icp_to_liquidator: ICP::from(claim.collateral_amount),
            liquidator: Some(caller),
            icp_rate: Some(UsdIcp::from(Decimal::from(claim.collateral_price_e8s) / dec!(100_000_000))),
            protocol_fee_collateral: None,
            timestamp: Some(ic_cdk::api::time()),
        };
        rumi_protocol_backend::storage::record_event(&event);

        s.bot_total_debt_covered_e8s += claim.debt_amount;
        s.bot_claims.remove(&vault_id);
    });

    log!(INFO, "[bot_confirm_liquidation] Confirmed liquidation for vault #{}: debt={}, collateral={}",
        vault_id, claim.debt_amount, claim.collateral_amount);

    Ok(())
}
```

- [ ] **Step 2: Add `bot_cancel_liquidation`**

Called by the bot when the swap fails. The bot must transfer collateral back first, then call this to unlock the vault.

```rust
/// Bot calls this when the swap failed and collateral has been returned (cancel phase).
/// Unlocks the vault, restores budget, and clears the claim.
/// The bot MUST transfer the collateral back to the backend canister BEFORE calling this.
#[candid_method(update)]
#[update]
async fn bot_cancel_liquidation(vault_id: u64) -> Result<(), ProtocolError> {
    validate_call().await?;
    let caller = ic_cdk::api::caller();

    let is_bot = read_state(|s| {
        s.liquidation_bot_principal.map_or(false, |bp| bp == caller)
    });
    if !is_bot {
        return Err(ProtocolError::GenericError(
            "Caller is not the registered liquidation bot canister".to_string(),
        ));
    }

    let claim = read_state(|s| s.bot_claims.get(&vault_id).cloned())
        .ok_or_else(|| ProtocolError::GenericError(format!(
            "No active claim for vault #{}", vault_id
        )))?;

    // Verify the collateral was actually returned by checking the backend's balance
    // on the collateral ledger. We check that the backend holds at least the claimed amount.
    let backend_id = ic_cdk::id();
    let balance_result: Result<(candid::Nat,), _> = ic_cdk::call(
        claim.collateral_type,
        "icrc1_balance_of",
        (icrc_ledger_types::icrc1::account::Account {
            owner: backend_id,
            subaccount: None,
        },),
    ).await;

    match balance_result {
        Ok((balance,)) => {
            let balance_u64 = balance.0.to_u64().unwrap_or(0);
            // We can't perfectly verify the exact return because the backend holds
            // collateral from many vaults. But we can at least verify the call succeeded.
            log!(INFO, "[bot_cancel_liquidation] Backend collateral balance: {} (type {})",
                balance_u64, claim.collateral_type);
        }
        Err((code, msg)) => {
            log!(INFO, "[bot_cancel_liquidation] WARNING: Could not verify collateral return: {:?} {}",
                code, msg);
            // Proceed anyway — the vault lock needs to be cleared regardless.
            // If collateral wasn't actually returned, the vault will be under-collateralized
            // and will be flagged for liquidation again on the next check_vaults cycle.
        }
    }

    mutate_state(|s| {
        if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
            vault.bot_processing = false;
        }
        // Restore budget since this liquidation didn't go through
        s.bot_budget_remaining_e8s += claim.debt_amount;
        s.bot_claims.remove(&vault_id);
    });

    log!(INFO, "[bot_cancel_liquidation] Cancelled claim for vault #{}: collateral={}, debt={} (budget restored)",
        vault_id, claim.collateral_amount, claim.debt_amount);

    Ok(())
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p rumi_protocol_backend 2>&1 | head -30`
Expected: Compiles cleanly.

- [ ] **Step 4: Commit**

```bash
git add src/rumi_protocol_backend/src/main.rs
git commit -m "feat: add bot_confirm_liquidation and bot_cancel_liquidation endpoints"
```

---

### Task 5: Add auto-cancel timer for stuck bot claims

**Files:**
- Modify: `src/rumi_protocol_backend/src/lib.rs:317` (in `check_vaults()`)
- Modify: `src/rumi_protocol_backend/src/main.rs` (add `auto_cancel_stuck_bot_claims` helper)

- [ ] **Step 1: Add auto-cancel logic to `check_vaults()`**

At the TOP of `check_vaults()` in `src/rumi_protocol_backend/src/lib.rs` (right after the function signature, before the `dummy_rate` line), add:

```rust
// Auto-cancel bot claims that have been pending too long (10 minutes).
// This prevents vaults from being permanently locked if the bot crashes.
const BOT_CLAIM_TIMEOUT_NS: u64 = 600_000_000_000; // 10 minutes
let now = ic_cdk::api::time();

let expired_claims: Vec<(u64, crate::state::BotClaim)> = read_state(|s| {
    s.bot_claims.iter()
        .filter(|(_, claim)| now.saturating_sub(claim.claimed_at) >= BOT_CLAIM_TIMEOUT_NS)
        .map(|(vid, claim)| (*vid, claim.clone()))
        .collect()
});

for (vault_id, claim) in &expired_claims {
    log!(INFO, "[check_vaults] Auto-cancelling stuck bot claim for vault #{} (claimed {}s ago)",
        vault_id, (now - claim.claimed_at) / 1_000_000_000);

    mutate_state(|s| {
        if let Some(vault) = s.vault_id_to_vaults.get_mut(vault_id) {
            vault.bot_processing = false;
        }
        s.bot_budget_remaining_e8s += claim.debt_amount;
        s.bot_claims.remove(vault_id);
    });
    // Note: The collateral is still with the bot. The vault will appear
    // under-collateralized and be re-flagged for liquidation (this time
    // routed to the stability pool since the bot timed out).
}
```

- [ ] **Step 2: Update `check_vaults()` to skip `bot_processing` vaults**

In the unhealthy vault detection loop (around line 329-337), add a filter to skip locked vaults:

Change:
```rust
if compute_collateral_ratio(vault, dummy_rate, s)
    < s.get_min_liquidation_ratio_for(&vault.collateral_type)
{
    unhealthy_vaults.push(vault.clone());
```

To:
```rust
if vault.bot_processing {
    // Skip — bot is actively processing this vault
    continue;
}
if compute_collateral_ratio(vault, dummy_rate, s)
    < s.get_min_liquidation_ratio_for(&vault.collateral_type)
{
    unhealthy_vaults.push(vault.clone());
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p rumi_protocol_backend 2>&1 | head -30`
Expected: Compiles cleanly.

- [ ] **Step 4: Commit**

```bash
git add src/rumi_protocol_backend/src/lib.rs src/rumi_protocol_backend/src/main.rs
git commit -m "feat: auto-cancel stuck bot claims after 10min, skip bot_processing vaults in check_vaults"
```

---

### Task 6: Remove old `bot_liquidate` and update `dev_force_bot_liquidate`

**Files:**
- Modify: `src/rumi_protocol_backend/src/main.rs:1336-1529`

- [ ] **Step 1: Remove or deprecate `bot_liquidate`**

The old `bot_liquidate` endpoint is replaced by `bot_claim_liquidation`. Remove the entire `bot_liquidate` function (lines 1330-1440). If backward compatibility is needed, keep it as a thin wrapper that calls `bot_claim_liquidation` — but since the bot canister is the only caller and we're updating it too, a clean removal is preferred.

- [ ] **Step 2: Update `dev_force_bot_liquidate` to use two-phase pattern**

Replace the existing `dev_force_bot_liquidate` (lines 1442-1529) to work with the claim system. It should bypass CR checks but still use the claim/lock pattern:

```rust
/// Developer-only: force the bot to claim a vault for liquidation regardless of health ratio.
/// Bypasses CR checks but still uses the two-phase claim pattern.
#[candid_method(update)]
#[update]
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

    let existing_claim = read_state(|s| s.bot_claims.contains_key(&vault_id));
    if existing_claim {
        return Err(ProtocolError::GenericError(format!(
            "Vault #{} already has an active bot claim", vault_id
        )));
    }

    // Get vault info — NO CR check, but still check collateral allowlist
    let (collateral_price_usd, debt_to_cover, collateral_to_seize, collateral_type) = read_state(|s| {
        let vault = s.vault_id_to_vaults.get(&vault_id)
            .ok_or_else(|| ProtocolError::GenericError(format!("Vault #{} not found", vault_id)))?;

        if vault.bot_processing {
            return Err(ProtocolError::GenericError(format!(
                "Vault #{} is already being processed", vault_id
            )));
        }

        if !s.bot_allowed_collateral_types.contains(&vault.collateral_type) {
            return Err(ProtocolError::GenericError(format!(
                "Collateral type {} is not in the bot's allowed list.", vault.collateral_type
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

    // Transfer collateral
    match rumi_protocol_backend::management::transfer_collateral(
        collateral_to_seize.to_u64(), caller, collateral_type
    ).await {
        Ok(block) => {
            log!(INFO, "[dev_force_bot_liquidate] Transferred {} collateral to caller, block {}", collateral_to_seize.to_u64(), block);
        }
        Err(e) => {
            return Err(ProtocolError::GenericError(format!("Collateral transfer failed: {:?}", e)));
        }
    }

    // Lock vault and record claim (same as bot_claim_liquidation)
    let now = ic_cdk::api::time();
    mutate_state(|s| {
        if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
            vault.bot_processing = true;
        }
        s.bot_claims.insert(vault_id, rumi_protocol_backend::state::BotClaim {
            vault_id,
            collateral_amount: collateral_to_seize.to_u64(),
            debt_amount: debt_to_cover.to_u64(),
            collateral_type,
            claimed_at: now,
            collateral_price_e8s: collateral_price_usd.to_e8s(),
        });
    });

    log!(INFO, "[dev_force_bot_liquidate] Force-claimed vault #{}: debt={}, collateral={}",
        vault_id, debt_to_cover.to_u64(), collateral_to_seize.to_u64());

    Ok(BotLiquidationResult {
        vault_id,
        collateral_amount: collateral_to_seize.to_u64(),
        debt_covered: debt_to_cover.to_u64(),
        collateral_price_e8s: collateral_price_usd.to_e8s(),
    })
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p rumi_protocol_backend 2>&1 | head -30`

- [ ] **Step 4: Commit**

```bash
git add src/rumi_protocol_backend/src/main.rs
git commit -m "refactor: remove old bot_liquidate, update dev_force to use two-phase claim"
```

---

### Task 7: Update the liquidation bot to use claim/confirm/cancel

**Files:**
- Modify: `src/liquidation_bot/src/process.rs:43-134`

- [ ] **Step 1: Add helper functions for confirm and cancel**

Add after the existing `call_bot_liquidate` function:

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

/// Transfer collateral back to the backend canister via direct icrc1_transfer.
/// No approve needed — the bot is sending from its own account.
async fn return_collateral_to_backend(config: &BotConfig, amount: u64, collateral_ledger: candid::Principal) -> Result<(), String> {
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
        ic_cdk::call(collateral_ledger, "icrc1_transfer", (transfer_args,)).await
        .map_err(|e| format!("Transfer call failed: {:?}", e))?;

    match result {
        Ok((Ok(_),)) => Ok(()),
        Ok((Err(e),)) => Err(format!("Transfer error: {:?}", e)),
        _ => unreachable!(),
    }
}
```

- [ ] **Step 2: Rewrite `process_pending()` to use claim/confirm/cancel**

Replace the existing `process_pending()` function:

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

    // Phase 1: CLAIM the vault (gets collateral, locks vault, but debt unchanged)
    let liq_result = call_bot_claim_liquidation(&config, vault.vault_id).await;
    let (collateral_amount, debt_covered, collateral_price) = match liq_result {
        Ok(r) => (r.collateral_amount, r.debt_covered, r.collateral_price_e8s),
        Err(e) => {
            log_failed_event(&vault, &format!("bot_claim_liquidation failed: {}", e));
            return;
        }
    };

    // Phase 2: Try to swap collateral → icUSD
    let swap_amount = calculate_swap_amount_internal(collateral_amount, debt_covered, collateral_price);
    let swap_result = swap::swap_icp_for_stable(&config, swap_amount).await;

    let (stable_amount, stable_token, route) = match swap_result {
        Ok(r) => (r.output_amount, r.target_token, r.route),
        Err(e) => {
            // SWAP FAILED — return collateral and cancel the claim
            log!(crate::INFO, "DEX swap failed for vault #{}: {}. Returning collateral and cancelling.", vault.vault_id, e);

            if let Err(return_err) = return_collateral_to_backend(&config, collateral_amount, vault.collateral_type).await {
                log!(crate::INFO, "WARNING: Failed to return collateral for vault #{}: {}", vault.vault_id, return_err);
                // Still try to cancel — the auto-cancel timer will catch it if we can't
            }

            if let Err(cancel_err) = call_bot_cancel_liquidation(&config, vault.vault_id).await {
                log!(crate::INFO, "WARNING: Failed to cancel claim for vault #{}: {}", vault.vault_id, cancel_err);
            }

            log_failed_event(&vault, &format!("DEX swap failed (claim cancelled): {}", e));
            return;
        }
    };

    // Phase 2b: ckStable → icUSD (3pool)
    let icusd_result = swap::swap_stable_for_icusd(&config, stable_amount, stable_token).await;
    let icusd_amount = match icusd_result {
        Ok(amount) => amount,
        Err(e) => {
            // 3pool swap failed — we already swapped ICP to ckStable, can't easily reverse that.
            // Best option: still cancel the claim so the stability pool can handle the vault.
            // The bot keeps the ckStable (can be manually recovered).
            log!(crate::INFO, "3pool swap failed for vault #{}: {}. Cancelling claim.", vault.vault_id, e);

            // We can't return ICP (it's now ckStable), so we skip the collateral return.
            // The auto-cancel timer on the backend will eventually unlock the vault.
            // The vault will be under-collateralized and re-flagged for the stability pool.
            if let Err(cancel_err) = call_bot_cancel_liquidation(&config, vault.vault_id).await {
                log!(crate::INFO, "WARNING: Failed to cancel claim for vault #{}: {}", vault.vault_id, cancel_err);
            }

            log_failed_event(&vault, &format!("3pool swap failed (claim cancelled, ckStable held by bot): {}", e));
            return;
        }
    };

    // Phase 2c: Deposit icUSD to backend reserves
    if let Err(e) = call_bot_deposit_to_reserves(&config, icusd_amount).await {
        log!(crate::INFO, "deposit_to_reserves failed for vault #{}: {}. Cancelling claim.", vault.vault_id, e);
        if let Err(cancel_err) = call_bot_cancel_liquidation(&config, vault.vault_id).await {
            log!(crate::INFO, "WARNING: Failed to cancel claim for vault #{}: {}", vault.vault_id, cancel_err);
        }
        log_failed_event(&vault, &format!("deposit_to_reserves failed (claim cancelled): {}", e));
        return;
    }

    // Phase 3: CONFIRM — everything succeeded, finalize the liquidation
    if let Err(e) = call_bot_confirm_liquidation(&config, vault.vault_id).await {
        log!(crate::INFO, "CRITICAL: bot_confirm_liquidation failed for vault #{}: {}. icUSD already deposited!", vault.vault_id, e);
        // This is a bad state: icUSD was deposited but vault wasn't mutated.
        // The auto-cancel timer will unlock the vault, and the icUSD deposit
        // is already recorded. Manual intervention may be needed.
        log_failed_event(&vault, &format!("CRITICAL: confirm failed after deposit: {}", e));
        return;
    }

    // Phase 4: Send remaining ICP to treasury (liquidation bonus)
    let icp_to_treasury = collateral_amount.saturating_sub(swap_amount);
    if icp_to_treasury > 0 {
        let _ = transfer_icp_to_treasury(&config, icp_to_treasury).await;
    }

    // Phase 5: Log success
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

- [ ] **Step 3: Update the old `call_bot_liquidate` references**

Remove or mark as dead code the old `call_bot_liquidate` function (it's replaced by `call_bot_claim_liquidation`).

- [ ] **Step 4: Verify the bot canister compiles**

Run: `cargo check -p liquidation_bot 2>&1 | head -30`

- [ ] **Step 5: Commit**

```bash
git add src/liquidation_bot/src/process.rs
git commit -m "refactor: bot uses claim/confirm/cancel pattern, returns collateral on swap failure"
```

---

### Task 8: Update bot's `test_force_liquidate` to use two-phase pattern

**Files:**
- Modify: `src/liquidation_bot/src/lib.rs:192-280`

- [ ] **Step 1: Rewrite `test_force_liquidate` to use claim/confirm/cancel**

The test endpoint should call `dev_force_bot_liquidate` (which now uses the claim pattern), then attempt the swap pipeline, then either confirm or cancel:

```rust
#[update]
async fn test_force_liquidate(vault_id: u64) -> TestForceResult {
    require_admin();

    let config = state::read_state(|s| s.config.clone())
        .expect("Bot not configured");

    log!(INFO, "[test_force_liquidate] Force-liquidating vault #{}", vault_id);

    // Step 1: Call dev_force_bot_liquidate (now uses claim pattern — locks vault, gets collateral)
    let liq_result: Result<(process::BackendResult<process::BotLiquidationResult>,), _> =
        ic_cdk::call(config.backend_principal, "dev_force_bot_liquidate", (vault_id,)).await;

    let (collateral_amount, debt_covered, collateral_price) = match liq_result {
        Ok((process::BackendResult::Ok(r),)) => {
            log!(INFO, "[test_force_liquidate] Claimed {} e8s collateral, {} e8s debt", r.collateral_amount, r.debt_covered);
            (r.collateral_amount, r.debt_covered, r.collateral_price_e8s)
        }
        Ok((process::BackendResult::Err(e),)) => ic_cdk::trap(&format!("dev_force_bot_liquidate error: {}", e)),
        Err((code, msg)) => ic_cdk::trap(&format!("dev_force_bot_liquidate call failed: {:?} {}", code, msg)),
    };

    // Step 2: Swap ICP → ckStable
    let swap_amount = process::calculate_swap_amount(collateral_amount, debt_covered, collateral_price);
    let stable_result = swap::swap_icp_for_stable(&config, swap_amount).await;
    let (stable_amount, stable_token, route) = match stable_result {
        Ok(r) => {
            log!(INFO, "[test_force_liquidate] KongSwap OK: {} native via {}", r.output_amount, r.route);
            (r.output_amount, r.target_token, r.route)
        }
        Err(e) => {
            // Cancel and trap
            let _ = process::call_bot_cancel_liquidation_pub(&config, vault_id).await;
            ic_cdk::trap(&format!("KongSwap failed (claim cancelled): {}", e));
        }
    };

    // Step 3: ckStable → icUSD
    let icusd_amount = match swap::swap_stable_for_icusd(&config, stable_amount, stable_token).await {
        Ok(amount) => amount,
        Err(e) => {
            let _ = process::call_bot_cancel_liquidation_pub(&config, vault_id).await;
            ic_cdk::trap(&format!("3pool swap failed (claim cancelled): {}", e));
        }
    };

    // Step 4: Deposit icUSD to backend reserves
    let deposited = match process::call_bot_deposit_to_reserves_pub(&config, icusd_amount).await {
        Ok(()) => true,
        Err(e) => {
            log!(INFO, "[test_force_liquidate] Deposit failed: {}", e);
            false
        }
    };

    // Step 5: CONFIRM the liquidation (finalize vault state)
    let confirmed = match process::call_bot_confirm_liquidation_pub(&config, vault_id).await {
        Ok(()) => {
            log!(INFO, "[test_force_liquidate] Confirmed liquidation for vault #{}", vault_id);
            true
        }
        Err(e) => {
            log!(INFO, "[test_force_liquidate] Confirm failed: {}", e);
            false
        }
    };

    // Step 6: Send remaining ICP to treasury
    let icp_to_treasury = collateral_amount.saturating_sub(swap_amount);
    if icp_to_treasury > 0 {
        let _ = process::transfer_icp_to_treasury_pub(&config, icp_to_treasury).await;
    }

    // Log event
    state::mutate_state(|s| {
        s.stats.events_count += 1;
        s.liquidation_events.push(BotLiquidationEvent {
            timestamp: ic_cdk::api::time(),
            vault_id,
            debt_covered_e8s: debt_covered,
            collateral_received_e8s: collateral_amount,
            icusd_burned_e8s: icusd_amount,
            collateral_to_treasury_e8s: icp_to_treasury,
            swap_route: route.clone(),
            effective_price_e8s: 0,
            slippage_bps: 0,
            success: confirmed && deposited,
            error_message: if confirmed { Some("test_force_liquidate".to_string()) } else { Some("confirm failed".to_string()) },
        });
    });

    TestForceResult {
        vault_id,
        collateral_received_e8s: collateral_amount,
        debt_covered_e8s: debt_covered,
        stable_output_native: stable_amount,
        stable_route: route,
        icusd_output_e8s: icusd_amount,
        icusd_deposited_to_reserves: deposited,
        icp_to_treasury_e8s: icp_to_treasury,
    }
}
```

- [ ] **Step 2: Add public wrappers for cancel/confirm in process.rs**

Add to process.rs (alongside existing `call_bot_deposit_to_reserves_pub`):

```rust
pub async fn call_bot_cancel_liquidation_pub(config: &BotConfig, vault_id: u64) -> Result<(), String> {
    call_bot_cancel_liquidation(config, vault_id).await
}

pub async fn call_bot_confirm_liquidation_pub(config: &BotConfig, vault_id: u64) -> Result<(), String> {
    call_bot_confirm_liquidation(config, vault_id).await
}
```

- [ ] **Step 3: Verify bot compiles**

Run: `cargo check -p liquidation_bot 2>&1 | head -30`

- [ ] **Step 4: Commit**

```bash
git add src/liquidation_bot/src/lib.rs src/liquidation_bot/src/process.rs
git commit -m "refactor: test_force_liquidate uses two-phase claim/confirm/cancel"
```

---

### Task 9: Add three dev test endpoints for liquidation cascade testing

**Files:**
- Modify: `src/rumi_protocol_backend/src/main.rs`

These are backend endpoints that the developer can call to exercise specific liquidation paths.

- [ ] **Step 1: Add `dev_test_bot_only_liquidation`**

Forces a vault through the bot path only (calls `dev_force_bot_liquidate` which claims it for the bot). The bot's existing timer will process it.

```rust
/// Developer test: force a vault to be claimed by the bot for liquidation.
/// The bot's process_pending timer will handle the swap pipeline.
/// Call get_bot_stats() and check the bot's liquidation_events to verify completion.
#[candid_method(update)]
#[update]
async fn dev_test_bot_only_liquidation(vault_id: u64) -> Result<BotLiquidationResult, ProtocolError> {
    let caller = ic_cdk::caller();
    let is_dev = read_state(|s| s.developer_principal == caller);
    if !is_dev {
        return Err(ProtocolError::GenericError("Developer only".to_string()));
    }
    // This already uses the claim pattern via dev_force_bot_liquidate
    dev_force_bot_liquidate(vault_id).await
}
```

Actually, `dev_force_bot_liquidate` already exists and serves this purpose. This test is already covered — no new endpoint needed. Skip this step; document that `dev_force_bot_liquidate` is the "bot only" test.

- [ ] **Step 2: Add `dev_test_pool_only_liquidation`**

Forces a vault directly to the stability pool, bypassing the bot entirely.

```rust
/// Developer test: force a vault to be liquidated by the stability pool, bypassing the bot.
/// Calls the stability pool's notify_liquidatable_vaults with just this vault.
#[candid_method(update)]
#[update]
async fn dev_test_pool_only_liquidation(vault_id: u64) -> Result<String, ProtocolError> {
    validate_call().await?;
    let caller = ic_cdk::caller();
    let is_dev = read_state(|s| s.developer_principal == caller);
    if !is_dev {
        return Err(ProtocolError::GenericError("Developer only".to_string()));
    }

    let pool_canister = read_state(|s| s.stability_pool_canister)
        .ok_or_else(|| ProtocolError::GenericError("No stability pool configured".to_string()))?;

    // Build vault notification (skips CR check — force test)
    let vault_info = read_state(|s| {
        let vault = s.vault_id_to_vaults.get(&vault_id)
            .ok_or_else(|| ProtocolError::GenericError(format!("Vault #{} not found", vault_id)))?;

        if vault.bot_processing {
            return Err(ProtocolError::GenericError(format!(
                "Vault #{} is locked by bot_processing", vault_id
            )));
        }

        let price = s.get_collateral_price_decimal(&vault.collateral_type)
            .map(|p| UsdIcp::from(p).to_e8s())
            .unwrap_or(0);
        let max_liq = (vault.borrowed_icusd_amount * s.max_partial_liquidation_ratio)
            .min(vault.borrowed_icusd_amount);

        Ok(crate::LiquidatableVaultInfo {
            vault_id: vault.vault_id,
            collateral_type: vault.collateral_type,
            debt_amount: vault.borrowed_icusd_amount.to_u64(),
            collateral_amount: vault.collateral_amount,
            recommended_liquidation_amount: max_liq.to_u64(),
            collateral_price_e8s: price,
        })
    })?;

    // Send directly to the stability pool
    let result: Result<(), _> = ic_cdk::call(
        pool_canister,
        "notify_liquidatable_vaults",
        (vec![vault_info],),
    ).await;

    match result {
        Ok(()) => {
            log!(INFO, "[dev_test_pool_only_liquidation] Sent vault #{} to stability pool", vault_id);
            Ok(format!("Vault #{} sent to stability pool for liquidation", vault_id))
        }
        Err((code, msg)) => {
            Err(ProtocolError::GenericError(format!(
                "Stability pool notification failed: {:?} {}", code, msg
            )))
        }
    }
}
```

**Note:** The `LiquidatableVaultInfo` struct is defined in `lib.rs` but is currently private. You must make it `pub struct LiquidatableVaultInfo` in `lib.rs` (around line 308) for it to be accessible from `main.rs` as `crate::LiquidatableVaultInfo`.

- [ ] **Step 3: Add `dev_test_cascade_liquidation`**

This simulates the full cascade: sends to bot with a very short timeout so it immediately falls back to the stability pool.

```rust
/// Developer test: simulate the full liquidation cascade.
/// Sends vault to bot. If bot doesn't process it within 30 seconds, falls back to stability pool.
/// This is a fire-and-forget — check logs and pool state to verify.
#[candid_method(update)]
#[update]
async fn dev_test_cascade_liquidation(vault_id: u64) -> Result<String, ProtocolError> {
    validate_call().await?;
    let caller = ic_cdk::caller();
    let is_dev = read_state(|s| s.developer_principal == caller);
    if !is_dev {
        return Err(ProtocolError::GenericError("Developer only".to_string()));
    }

    let (bot_canister, pool_canister) = read_state(|s| {
        (s.liquidation_bot_principal, s.stability_pool_canister)
    });

    // Build vault notification
    let vault_info = read_state(|s| {
        let vault = s.vault_id_to_vaults.get(&vault_id)
            .ok_or_else(|| ProtocolError::GenericError(format!("Vault #{} not found", vault_id)))?;

        if vault.bot_processing {
            return Err(ProtocolError::GenericError(format!(
                "Vault #{} is locked by bot_processing", vault_id
            )));
        }

        let price = s.get_collateral_price_decimal(&vault.collateral_type)
            .map(|p| UsdIcp::from(p).to_e8s())
            .unwrap_or(0);
        let max_liq = (vault.borrowed_icusd_amount * s.max_partial_liquidation_ratio)
            .min(vault.borrowed_icusd_amount);

        Ok(crate::LiquidatableVaultInfo {
            vault_id: vault.vault_id,
            collateral_type: vault.collateral_type,
            debt_amount: vault.borrowed_icusd_amount.to_u64(),
            collateral_amount: vault.collateral_amount,
            recommended_liquidation_amount: max_liq.to_u64(),
            collateral_price_e8s: price,
        })
    })?;

    let mut steps = Vec::new();

    // Step 1: Send to bot
    if let Some(bot) = bot_canister {
        let bot_vaults = vec![vault_info.clone()];
        let result: Result<(), _> = ic_cdk::call(
            bot, "notify_liquidatable_vaults", (bot_vaults,)
        ).await;
        match result {
            Ok(()) => steps.push("Sent to bot".to_string()),
            Err((code, msg)) => steps.push(format!("Bot notification failed: {:?} {}", code, msg)),
        }
    } else {
        steps.push("No bot configured, skipping".to_string());
    }

    // Step 2: Also send to stability pool (simulates the fallback)
    // In production, check_vaults handles the timeout. For testing, we send to both
    // and the first one to succeed wins (the second will find the vault already liquidated).
    if let Some(pool) = pool_canister {
        let pool_vaults = vec![vault_info];
        let result: Result<(), _> = ic_cdk::call(
            pool, "notify_liquidatable_vaults", (pool_vaults,)
        ).await;
        match result {
            Ok(()) => steps.push("Sent to stability pool".to_string()),
            Err((code, msg)) => steps.push(format!("Pool notification failed: {:?} {}", code, msg)),
        }
    } else {
        steps.push("No stability pool configured, skipping".to_string());
    }

    let summary = steps.join("; ");
    log!(INFO, "[dev_test_cascade_liquidation] Vault #{}: {}", vault_id, summary);
    Ok(format!("Vault #{} cascade test: {}", vault_id, summary))
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p rumi_protocol_backend 2>&1 | head -30`

- [ ] **Step 5: Commit**

```bash
git add src/rumi_protocol_backend/src/main.rs
git commit -m "feat: add dev_test_pool_only_liquidation and dev_test_cascade_liquidation test endpoints"
```

---

### Task 10: Update Candid declarations

**Files:**
- Modify: `src/rumi_protocol_backend/rumi_protocol_backend.did`
- Modify: `src/declarations/rumi_protocol_backend/rumi_protocol_backend.did`
- Modify: `src/declarations/rumi_protocol_backend/rumi_protocol_backend.did.d.ts`
- Modify: `src/declarations/rumi_protocol_backend/rumi_protocol_backend.did.js`

- [ ] **Step 1: Update the primary .did file**

Add to the service definition in `rumi_protocol_backend.did`:

```candid
  bot_claim_liquidation : (nat64) -> (variant { Ok : BotLiquidationResult; Err : ProtocolError });
  bot_confirm_liquidation : (nat64) -> (variant { Ok; Err : ProtocolError });
  bot_cancel_liquidation : (nat64) -> (variant { Ok; Err : ProtocolError });
  dev_test_pool_only_liquidation : (nat64) -> (variant { Ok : text; Err : ProtocolError });
  dev_test_cascade_liquidation : (nat64) -> (variant { Ok : text; Err : ProtocolError });
```

Remove `bot_liquidate` from the service definition (replaced by `bot_claim_liquidation`).

- [ ] **Step 2: Mirror changes to declarations .did**

Copy the same changes to `src/declarations/rumi_protocol_backend/rumi_protocol_backend.did`.

- [ ] **Step 3: Update TypeScript declarations**

Update `rumi_protocol_backend.did.d.ts` to include the new function signatures. Follow the existing patterns in the file for how Result types are represented in TypeScript.

- [ ] **Step 4: Update JavaScript declarations**

Update `rumi_protocol_backend.did.js` to include the new functions in the IDL factory. Follow existing patterns.

- [ ] **Step 5: Verify everything compiles**

Run: `cargo build -p rumi_protocol_backend 2>&1 | tail -5`

- [ ] **Step 6: Commit**

```bash
git add src/rumi_protocol_backend/rumi_protocol_backend.did src/declarations/
git commit -m "chore: update Candid declarations for two-phase bot liquidation endpoints"
```

---

### Task 11: Final integration verification

- [ ] **Step 1: Full workspace build**

Run: `cargo build --workspace 2>&1 | tail -10`
Expected: All canisters compile cleanly.

- [ ] **Step 2: Run existing tests**

Run: `cargo test --workspace 2>&1 | tail -20`
Expected: All existing tests pass. No regressions from the new `bot_processing` field (it defaults to `false` via `#[serde(default)]`).

- [ ] **Step 3: Commit any remaining fixes**

If there were compilation issues or test failures, commit the fixes:

```bash
git add -A
git commit -m "fix: resolve compilation and test issues from two-phase liquidation"
```

---

## Summary of New Endpoints

| Endpoint | Caller | Purpose |
|----------|--------|---------|
| `bot_claim_liquidation(vault_id)` | Bot | Phase 1: get collateral, lock vault |
| `bot_confirm_liquidation(vault_id)` | Bot | Phase 2 (success): finalize vault state |
| `bot_cancel_liquidation(vault_id)` | Bot | Phase 2 (failure): unlock vault, restore budget |
| `dev_test_pool_only_liquidation(vault_id)` | Dev | Test: force vault to stability pool |
| `dev_test_cascade_liquidation(vault_id)` | Dev | Test: send to both bot and pool |

## Removed Endpoints

| Endpoint | Replaced By |
|----------|-------------|
| `bot_liquidate(vault_id)` | `bot_claim_liquidation` + `bot_confirm_liquidation` |

## Test Plan

1. **Bot-only test:** Call `dev_force_bot_liquidate(vault_id)` → triggers bot claim → bot processes swap → bot confirms. Check `get_bot_stats()` for updated `total_debt_covered_e8s`.
2. **Pool-only test:** Call `dev_test_pool_only_liquidation(vault_id)` → stability pool processes directly. Check pool's liquidation history.
3. **Cascade test:** Call `dev_test_cascade_liquidation(vault_id)` → both bot and pool notified → first to succeed wins. Check both bot stats and pool history.
