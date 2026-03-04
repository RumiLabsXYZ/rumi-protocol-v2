# Stability Pool Refactor Design

**Date:** 2026-03-04
**Status:** Approved

## Overview

Refactor the stability pool canister from a broken polling-based, single-token, ICP-only implementation into a push-model, multi-token, multi-collateral pool with stable memory, dynamic registries, and correct liquidation math.

## Architecture

### Push Model

The backend pushes liquidation notifications to the stability pool after `check_vaults()` identifies undercollateralized vaults (triggered by XRC price updates every 300s). The pool no longer polls.

**Primary path:** Backend calls `stability_pool.notify_liquidatable_vaults(vaults)` — fire-and-forget.

**Fallback path:** Public `execute_liquidation(vault_id)` endpoint on the pool with per-caller guard. No polling timer.

### Canister Interaction

```
Backend (check_vaults) --push--> Stability Pool --liquidate_vault_partial_with_stable--> Backend
                                                --liquidate_vault_partial--> Backend
                                 Backend --pending_margin_transfer--> Stability Pool (collateral)
```

## Multi-Token Deposits

Users deposit icUSD, ckUSDT, or ckUSDC. All three are accepted. The pool holds actual tokens in its canister account.

### Token Draw Order (Liquidations)

Stablecoins are consumed by priority number (higher priority = consumed first):

| Priority | Tokens | Rationale |
|----------|--------|-----------|
| 2 (first) | ckUSDT, ckUSDC | Build protocol reserves, preserve icUSD supply |
| 1 (last) | icUSD | Protect circulating supply, incentivize DEX volume |

Same-priority tokens are consumed **proportionally** based on pool composition.

### Stablecoin Registry (Dynamic)

```rust
StablecoinConfig {
    ledger_id: Principal,
    symbol: String,
    decimals: u8,
    priority: u8,       // higher = consumed first
    is_active: bool,     // false = no new deposits, existing balances still usable
}
```

Adding a stablecoin = admin registration call. Removing = set `is_active: false`. No canister upgrade needed. The pool maintains a thin translation layer to speak the backend's current `StableTokenType` enum until the backend gets its own dynamic registry.

## Multi-Collateral Support

### Collateral Registry (Dynamic)

Synced from backend's `collateral_configs`. Current types: ICP, ckBTC, ckXAUT, ckETH (coming).

```rust
CollateralInfo {
    ledger_id: Principal,
    symbol: String,
    decimals: u8,
    status: CollateralStatus,
}
```

Synced on init and re-synced when the pool encounters an unrecognized collateral type during a push notification. Admin can also trigger manual sync.

### Collateral Opt-Out

Depositors are opted-in to all collateral types by default. They can opt out of any collateral type, which means:

- Their stablecoin balances are excluded from the effective pool size for that collateral type
- They do not participate in liquidations for that collateral type
- They receive no collateral gains from those liquidations
- Existing gains for opted-out collateral remain claimable

Opting back in is instant.

If the pool lacks opted-in capital for a collateral type, those vaults remain on the liquidation page for manual liquidators.

## State Model

### Per-Depositor Position

```rust
DepositPosition {
    stablecoin_balances: BTreeMap<Principal, u64>,     // ledger_id -> balance (native decimals)
    collateral_gains: BTreeMap<Principal, u64>,         // collateral_ledger -> claimable gains
    opted_out_collateral: BTreeSet<Principal>,          // collateral ledger principals excluded
    deposit_timestamp: u64,
    total_claimed_gains: BTreeMap<Principal, u64>,      // lifetime claims per collateral
}
```

### Pool-Level State

```rust
StabilityPoolState {
    deposits: BTreeMap<Principal, DepositPosition>,

    // Aggregate balances per stablecoin
    total_stablecoin_balances: BTreeMap<Principal, u64>,

    // Effective pool size per collateral type (recomputed on deposit/withdraw/opt changes)
    effective_pool_per_collateral: BTreeMap<Principal, u64>,

    // Registries
    stablecoin_registry: BTreeMap<Principal, StablecoinConfig>,
    collateral_registry: BTreeMap<Principal, CollateralInfo>,

    // Canister references
    protocol_canister_id: Principal,

    // Admin / operational
    configuration: PoolConfiguration,
    liquidation_history: Vec<PoolLiquidationRecord>,
    in_flight_liquidations: BTreeSet<u64>,  // vault_ids currently being processed
    is_initialized: bool,
}
```

### Stable Memory

Candid serialization in `pre_upgrade`/`post_upgrade`. State is a handful of BTreeMaps — well within stable memory limits even at scale.

## Liquidation Flow

1. Backend pushes `notify_liquidatable_vaults(Vec<LiquidatableVaultInfo>)` with vault_id, collateral_type, debt_amount, collateral_amount, collateral_price.
2. Pool filters: skip vaults already in-flight, skip vaults where opted-in capital < debt.
3. Pool sorts by debt size (largest first).
4. For each vault:
   a. Calculate ckstable portion (priority 2 tokens proportionally) vs icUSD portion (priority 1).
   b. Approve backend on appropriate ledgers.
   c. Call `liquidate_vault_partial_with_stable` for ckstable portions.
   d. Call `liquidate_vault_partial` for icUSD portion.
   e. Backend validates, seizes collateral, returns excess to vault owner, sends protocol fee to treasury.
   f. On success: reduce opted-in depositors' stablecoin balances proportionally, distribute collateral gains proportionally by share of stables consumed.
   g. Record liquidation in history.
5. Clear in-flight set.

### What the Backend Already Handles

- Partial liquidation restores vault to borrow threshold CR (`compute_partial_liquidation_cap`)
- Excess collateral returned to vault owner (`pending_excess_transfers`)
- Protocol fee subtracted from bonus and sent to treasury (`liquidation_protocol_share`)
- Multi-collateral price feeds and liquidation ratios (`collateral_configs`)

## User Operations

### Deposit
`deposit(token_ledger: Principal, amount: u64)` — ICRC-2 transfer_from, add to user's balance for that token, update aggregates.

### Withdraw
`withdraw(token_ledger: Principal, amount: u64)` — validate balance, ICRC-1 transfer to user, decrement balance.

### Claim Collateral
`claim_collateral(collateral_ledger: Principal)` — transfer collateral gains to user, zero out pending.
`claim_all_collateral()` — claim all nonzero collateral types in one call.

### Opt-Out/In
`opt_out_collateral(collateral_type: Principal)` — add to exclusion set, recompute effective pool.
`opt_in_collateral(collateral_type: Principal)` — remove from exclusion set, recompute effective pool.

## Backend Changes Required

1. **Store stability pool canister ID** in backend state (new field, admin-configurable).
2. **Add push call in `check_vaults()`**: if unhealthy vaults exist and pool ID is configured, fire-and-forget call to `notify_liquidatable_vaults`.
3. **Add `get_collateral_configs()` query endpoint**: expose the subset of collateral config the pool needs to sync its registry.

Existing liquidation endpoints (`liquidate_vault_partial`, `liquidate_vault_partial_with_stable`) work as-is. The pool is just another liquidator.

## Error Handling

- **Push fails**: vaults stay on liquidation page for manual liquidators. Backend logs failure.
- **Liquidation call rejected**: vault no longer liquidatable (topped up, price recovered). Pool skips, moves on. No state change.
- **Token transfer fails**: return error to user, no state committed.
- **Upgrade during liquidation**: pre_upgrade serializes state. In-flight liquidation lost but no state was committed. Post-upgrade reconciliation check compares on-chain balances vs tracked state, flags discrepancies for admin.
- **Duplicate push notifications**: pool deduplicates via `in_flight_liquidations` set.

## What Gets Removed

- Polling timer (`setup_liquidation_monitoring`, interval-based `scan_and_liquidate`)
- `get_vault_for_liquidation` call (nonexistent endpoint)
- `VaultLiquidationInfo` phantom struct reference
- Hardcoded 10% liquidation discount
- String-based share percentages
- `rumi_stability_pool/` v1 canister (remove from dfx.json)

## What Gets Kept

- Emergency pause/resume
- Admin configuration
- Logging infrastructure
- State validation (updated for new model)
- Analytics/history queries (updated for multi-token/multi-collateral)

## Future Work (Not This Refactor)

- Backend dynamic stablecoin registry (replace `StableTokenType` enum)
- Parameter auto-tuning based on collateral opt-out data
