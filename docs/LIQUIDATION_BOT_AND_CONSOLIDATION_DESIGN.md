# Liquidation Bot & Backend Consolidation Design

**Date:** 2026-03-14
**Status:** Design Discussion
**Related:** [3Pool Consolidation Proposal](./3POOL_CONSOLIDATION_PROPOSAL.md)

---

## Overview

This document captures design decisions from the March 14, 2026 session covering two related initiatives:

1. **Liquidation Bot (Phase 1 — build now)**: A privileged canister that liquidates unhealthy ICP-collateral vaults using a budgeted system, swaps collateral on DEXes, and deposits proceeds to reserves/treasury.

2. **Backend Consolidation (Phase 2 — future)**: Merging the 3pool AMM + an ICP/icUSD xy=k pool into the backend canister, which would make the bot a simple internal function rather than a separate canister.

---

## Phase 1: Liquidation Bot Canister

### Architecture

The bot is a separate canister with privileged access to the backend. It does NOT scan for liquidatable vaults — the backend notifies it (same fire-and-forget pattern used for the stability pool today).

### Flow

1. Backend's `check_vaults()` identifies unhealthy ICP-collateral vaults
2. Backend calls `notify_liquidatable_vaults()` on the bot canister (fire-and-forget)
3. Notification includes pre-calculated `recommended_liquidation_amount` — the exact icUSD amount needed to restore the vault to minimum healthy CR
4. Bot calls `bot_liquidate(vault_id)` on the backend
5. Backend validates: caller is registered bot principal, budget remaining >= liquidation amount
6. Backend executes liquidation WITHOUT requiring icUSD payment — debt is covered on credit
7. Backend decrements bot budget, increments bot obligation tracker
8. Bot receives ICP collateral
9. Bot swaps ICP → ckUSDC (via ICPSwap or KongSwap) — only enough to cover the debt + 5-10bp buffer
10. Bot swaps ckUSDC → icUSD (via 3pool)
11. Bot deposits icUSD to protocol reserves (burns the obligation)
12. Bot sends remaining ICP to treasury (appreciating asset)

### Liquidation Amount Calculation

To restore a vault to minimum healthy CR (target `T`), given:
- `D` = current debt
- `C` = current collateral value in USD
- `B` = liquidation bonus (e.g., 1.15)

```
L = (T * D - C) / (T - B)
```

Where `L` is the icUSD amount to liquidate. This is included in the notification payload.

### Backend Changes

**New state fields:**
- `liquidation_bot_canister: Option<Principal>` — registered bot canister ID
- `bot_liquidation_budget_e8s: u64` — remaining monthly budget in icUSD terms
- `bot_budget_total_e8s: u64` — total monthly budget (for display)
- `bot_total_debt_covered_e8s: u64` — cumulative obligations taken on
- `bot_total_icusd_deposited_e8s: u64` — cumulative icUSD deposited to reserves

**New endpoints:**
- `set_liquidation_bot_config(principal, monthly_budget)` — admin only
- `bot_liquidate(vault_id) → BotLiquidationResult` — bot canister only, decrements budget
- `bot_deposit_to_reserves(amount)` — bot canister only, decrements obligation
- `get_bot_stats() → BotStats` — public query

**Modified functions:**
- `check_vaults()` — also notifies bot canister with enriched payload including `recommended_liquidation_amount` and `collateral_price`

### Bot Canister State

```rust
struct BotState {
    // Cumulative tracking
    total_debt_covered_e8s: u64,
    total_icusd_burned_e8s: u64,
    total_collateral_received_e8s: u64,
    total_collateral_to_treasury_e8s: u64,

    // Event log (publicly queryable)
    liquidation_events: Vec<BotLiquidationEvent>,
}

struct BotLiquidationEvent {
    timestamp: u64,
    vault_id: u64,
    debt_covered_e8s: u64,
    collateral_received_e8s: u64,
    icusd_burned_e8s: u64,
    collateral_to_treasury_e8s: u64,
    swap_price: f64,          // effective ICP/USD from DEX
    slippage_bps: i32,        // actual vs expected
}
```

### Slippage Handling

Small slippage is acceptable. The backend tracks `bot_total_debt_covered_e8s` vs `bot_total_icusd_deposited_e8s`. The deficit is the running slippage loss. Admin can periodically burn icUSD from the treasury to zero out the deficit if needed. With ICP's on-chain liquidity, this should be basis points on most liquidations.

### DEX Integration

Modular design — the swap logic should be behind a trait/interface so we can swap providers:

```rust
trait DexSwap {
    async fn swap_icp_for_stable(amount_icp: u64, min_output: u64) -> Result<u64, SwapError>;
}
```

**Initial target:** ICPSwap for ICP→ckUSDC, then 3pool for ckUSDC→icUSD.

**Max slippage parameter:** Configurable (e.g., 50bp). If DEX slippage exceeds tolerance, bot holds the ICP and retries on next timer tick rather than dumping at a bad price.

### Collateral Routing

- **ICP-collateral vaults** → bot handles (DEX liquidity is deep)
- **All other collateral types** (ckBTC, ckETH, ckXAUT) → stability pool handles (as today)
- Future: extend bot to handle other collateral types once DEX liquidity is verified

### Fallback Chain

1. Bot tries to liquidate (fastest, no idle capital needed)
2. If bot fails (out of budget, DEX too thin, bot down) → stability pool depositors can liquidate
3. If stability pool doesn't act → manual liquidators (the existing liquidation page)

### Frontend

New "Liquidation Bot" tab on the `/liquidations` page (first tab, highest priority):
- Bot status: active/paused, budget remaining/total
- Running totals: debt covered, icUSD burned, collateral received, collateral to treasury
- Deficit tracker: debt covered minus icUSD burned
- Scrollable log of every liquidation event with full details

### Alerting for Manual Liquidations

For cases where vaults fall through to manual liquidation, use a Telegram/Discord bot that watches for liquidatable vaults and pings a channel. Simpler and lower friction than web3 email, and Rob already has active Telegram/Discord communities.

---

## Phase 2: Backend Consolidation (Future)

See [3Pool Consolidation Proposal](./3POOL_CONSOLIDATION_PROPOSAL.md) for the full rationale.

### What Changes for the Bot

If the 3pool AMM and an ICP/icUSD xy=k pool are both consolidated into the backend:

1. The bot canister becomes unnecessary — liquidation + swap is a single atomic `mutate_state()` call
2. Backend's `check_vaults()` finds unhealthy vault → atomically liquidates it → swaps ICP→icUSD via internal xy=k pool → burns icUSD → sends remaining ICP to treasury
3. No inter-canister calls, no slippage risk from async gaps, no separate budget system
4. The "liquidation bot" becomes a function, not a canister

### icUSD in the Pool — Virtual Balance Model

Since the backend is the icUSD minting canister (icUSD sent to it gets burned), the pool can't "hold" icUSD on the ledger. Instead:

- Pool's icUSD balance is tracked as a number in internal state (virtual)
- When someone swaps token→icUSD: backend mints fresh icUSD to the user, internal pool balance decreases
- When someone swaps icUSD→token: backend burns the icUSD, internal pool balance increases
- Net effect on icUSD total supply is always correct

**LP deposits still work:** User deposits icUSD → it burns → internal pool balance increases → 3USD minted. User withdraws → internal pool balance decreases → backend mints icUSD → 3USD burned. Economically identical to holding real icUSD.

### Multiple Pools in One Canister

The backend can host any number of pool types:
- **3pool (StableSwap):** icUSD/ckUSDC/ckUSDT — already built, would be moved in
- **xy=k pool:** ICP/icUSD — new, enables atomic liquidations
- **Future pools:** ckBTC/icUSD, etc.

Each pool has its own state structures and endpoints. LP tokens:
- **3USD** stays as a separate ICRC-1 ledger canister (same canister ID, backend becomes minting authority)
- **ICP/icUSD LP** can be tracked internally in backend state (no need for a separate token canister unless users need to trade LP positions)

### 3USD Integrity

3USD is not "directly redeemable" today either — users must call `remove_liquidity()` on the 3pool canister. After consolidation, they'd call `remove_liquidity()` on the backend instead. The 3USD token stays at the same canister ID with the same ICRC-1/2/3 interfaces. Only the minting authority changes (backend instead of 3pool logic). No impact on token integrity.

### Why Phase 1 First

1. The bot is immediately useful — liquidations need to work reliably now
2. Consolidation is a bigger project with migration risk and required downtime
3. Building the bot validates the consolidation thesis — experiencing the async pain firsthand provides real data on whether consolidation is worth the risk
4. The bot's DEX integration module will be reusable even after consolidation (for non-ICP collateral types that aren't in any internal pool)

---

## Open Questions

1. **Budget reset mechanism:** Monthly timer? Admin manual reset? Both?
2. **Should the bot prioritize vaults by CR (lowest first) or by size (largest debt first)?** Lowest CR is probably safer — most at-risk vaults first.
3. **KongSwap vs ICPSwap for ICP→ckUSDC:** Need to check current liquidity depth on both. Could also support both with a best-rate selector.
4. **Recovery mode behavior:** Should the bot respect recovery mode caps (only liquidate enough to restore to recovery target), or should it liquidate more aggressively?
5. **Consolidation timeline:** When do we pull the trigger on Phase 2? After the bot is proven? After a certain TVL threshold?
