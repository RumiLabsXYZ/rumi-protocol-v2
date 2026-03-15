# Liquidation Bot Canister — Design Spec

**Date:** 2026-03-14
**Branch:** `feat/liquidation-bot`
**Related:** [Liquidation Bot & Consolidation Design](../../LIQUIDATION_BOT_AND_CONSOLIDATION_DESIGN.md)

---

## Overview

A privileged bot canister that liquidates unhealthy ICP-collateral vaults on credit, swaps the seized collateral for icUSD via DEXes, and deposits proceeds to protocol reserves. No upfront capital required — the backend extends credit and the bot pays it back after selling collateral.

## Architecture

```
Backend (check_vaults every 300s)
    │ fire-and-forget: notify_liquidatable_vaults(Vec<VaultInfo>)
    ▼
Bot Canister
    │ 1. bot_liquidate(vault_id) on backend → receives ICP on credit
    │ 2. ICP → ckUSDC or ckUSDT via KongSwap (best rate wins)
    │ 3. ckStable → icUSD via 3pool swap
    │ 4. bot_deposit_to_reserves(icUSD) on backend → burns obligation
    │ 5. remaining ICP → treasury (appreciating asset)
    ▼
Backend tracks: total_debt_covered vs total_icusd_deposited
```

### Why a Separate Canister (Not in Backend)

The bot makes outbound calls to third-party canisters (KongSwap, 3pool). These can fail, hang, or return unexpected data. In a separate canister, a catastrophic bot failure is isolated — backend keeps running, stability pool keeps working, manual liquidations keep working. The bot is quarantined risk.

Phase 2 (backend consolidation with internal xy=k pool) eliminates third-party risk and makes the bot a function instead of a canister.

## Bot Canister (`src/liquidation_bot/`)

### State

```rust
struct BotState {
    // Config (admin-settable)
    backend_principal: Principal,
    three_pool_principal: Principal,
    kong_swap_principal: Principal,
    treasury_principal: Principal,
    max_slippage_bps: u16,        // default 50bp

    // Cumulative tracking (all-time)
    total_debt_covered_e8s: u64,
    total_icusd_burned_e8s: u64,
    total_collateral_received_e8s: u64,
    total_collateral_to_treasury_e8s: u64,

    // Event log
    liquidation_events: Vec<BotLiquidationEvent>,
}
```

### Processing Model

**No retry queue.** Each notification is processed once:

1. Backend calls `notify_liquidatable_vaults()` on bot
2. Bot iterates through the list, processing each vault:
   a. Call `bot_liquidate(vault_id)` on backend
   b. If budget exceeded or vault no longer unhealthy → skip
   c. Query KongSwap for ICP/ckUSDC and ICP/ckUSDT rates, pick better one
   d. Swap ICP → ckStable via KongSwap `swap_async`
   e. Swap ckStable → icUSD via 3pool `swap(token_index, 0, amount, min_out)`
   f. Call `bot_deposit_to_reserves(icusd_amount)` on backend
   g. Transfer remaining ICP to treasury
   h. Log event
3. If ANY step fails for a vault → **do not retry**. The vault stays liquidatable. The stability pool will pick it up on the next `check_vaults()` cycle (300s). If the stability pool doesn't act, it appears on the manual liquidations page.

### Failure Escalation Chain

```
Bot fails → Stability Pool gets notified next cycle → Manual Liquidation page
                                                       (header indicator lights up)
```

### Timer

30-second tick. Each tick checks if there are pending notifications and processes them. One vault at a time, sequentially. No parallelism — simplicity over speed.

### Endpoints

| Endpoint | Access | Purpose |
|----------|--------|---------|
| `notify_liquidatable_vaults(Vec<VaultInfo>)` | Backend only | Receive liquidation targets |
| `get_bot_stats()` | Public query | Stats + budget info |
| `get_liquidation_events(offset, limit)` | Public query | Paginated event log |
| `set_config(BotConfig)` | Admin only | Update principals, slippage |

## Backend Changes

### New State Fields

```rust
// Bot configuration
liquidation_bot_principal: Option<Principal>,
bot_budget_total_e8s: u64,        // monthly budget (default $10,000 = 1_000_000_000_000 e8s)
bot_budget_remaining_e8s: u64,
bot_budget_start_timestamp: u64,  // fiscal month start

// Cumulative tracking
bot_total_debt_covered_e8s: u64,
bot_total_icusd_deposited_e8s: u64,
```

### New Endpoints

| Endpoint | Access | Purpose |
|----------|--------|---------|
| `set_liquidation_bot_config(principal, monthly_budget)` | Admin | Register bot + set budget |
| `bot_liquidate(vault_id) → BotLiquidationResult` | Bot only | Liquidate on credit, return ICP amount |
| `bot_deposit_to_reserves(amount)` | Bot only | Burn obligation with icUSD |
| `get_bot_stats() → BotStats` | Public query | Budget, deficit, all-time totals |
| `reset_bot_budget()` | Admin | Start new fiscal month |

### `bot_liquidate` Logic

1. Validate caller == registered bot principal
2. Validate budget remaining >= recommended liquidation amount
3. Calculate `L = (T*D - C) / (T - B)` to restore vault to minimum healthy CR
4. Execute liquidation: reduce vault debt by L, seize proportional collateral + bonus
5. Decrement bot budget by L
6. Increment `bot_total_debt_covered_e8s` by L
7. Transfer ICP collateral to bot canister
8. Return `BotLiquidationResult { collateral_amount, debt_covered, collateral_price }`

### Modified: `check_vaults()`

After notifying stability pool, also notify bot canister with the same `notify_liquidatable_vaults` call pattern (fire-and-forget). Include `recommended_liquidation_amount` and `collateral_price` in the payload.

## DEX Integration

### Trait Interface

```rust
#[async_trait]
trait DexSwap {
    async fn get_quote(
        &self,
        amount_icp: u64,
        target_token: Principal,
    ) -> Result<DexQuote, SwapError>;

    async fn execute_swap(
        &self,
        amount_icp: u64,
        target_token: Principal,
        min_output: u64,
    ) -> Result<SwapResult, SwapError>;
}
```

### KongSwap Implementation

- Query both ICP/ckUSDC and ICP/ckUSDT via `swap_amounts`
- Pick whichever gives better output
- Execute via `swap_async`, poll status via `requests(request_id)`
- Then route winning stablecoin through 3pool: `swap(token_index, 0, amount, min_out)` → icUSD

### ICPSwap (Future)

3-step pattern from Oisy research: factory `getPool()` → pool `deposit()` → `swap()` → `withdraw()`. Plugs into the same trait. Not built in this phase.

## Frontend Tab

First tab on `/liquidations`: "Liquidation Bot"

### Status Card
- Active / Paused indicator
- Budget: remaining / total (e.g., "$7,234 / $10,000")
- Days remaining in fiscal month

### All-Time Stats
- Total debt covered (icUSD)
- Total icUSD burned (deposited to reserves)
- Current deficit (covered - burned)
- Total ICP sent to treasury

### Event Log
Scrollable table, newest first:
- Timestamp
- Vault ID
- Debt covered (icUSD)
- ICP received
- icUSD burned
- ICP to treasury
- Effective swap price (ICP/USD)
- Slippage (bps vs oracle price)

### Header Indicator
When there are liquidatable vaults that the bot AND stability pool have failed to handle, show a visual indicator in the app header that links to the manual liquidations tab.

## Testing

Unit tests for:
- Liquidation amount formula: `L = (T*D - C) / (T - B)` with various CR scenarios
- Budget decrement / exhaustion
- Obligation tracking: debt_covered vs icusd_deposited arithmetic
- Slippage calculation

Integration tests (PocketIC, future):
- Full flow: notify → liquidate → mock swap → deposit to reserves
- Budget exhaustion → vault not liquidated
- Fallback: bot skip → stability pool notified
