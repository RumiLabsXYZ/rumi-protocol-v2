# 3USD Stability Pool Integration Design

**Date:** 2026-03-19
**Status:** Draft

## Overview

Allow 3USD (ThreePool LP token) deposits into the stability pool alongside icUSD, ckUSDT, and ckUSDC. During liquidations, 3USD is converted to icUSD via an atomic burn mechanism on the 3pool, with a fallback to protocol reserves if the burn fails. This deepens the capital available for liquidations and increases 3pool TVL.

## Motivation

- **Deeper liquidation capital**: Users currently have no incentive to deposit icUSD when they could earn yield via 3USD. Accepting 3USD captures capital that would otherwise sit idle.
- **Better for users**: 3USD earns yield via virtual price appreciation (~4.9% currently) while waiting for liquidation events.
- **Better for the protocol**: 3USD deposits mean more liquidity locked in the 3pool, deepening DEX depth.
- **Resilience**: Provides a secondary capital pool for liquidations that the bot misses.

## Design Decisions

### Draw Order

icUSD and ckstables are consumed first (existing priority system). 3USD is consumed last — it's the reserve layer.

| Priority | Tokens | Rationale |
|----------|--------|-----------|
| 3 (first) | ckUSDT, ckUSDC | Build protocol reserves, preserve icUSD supply |
| 2 | icUSD | Simpler liquidation path, no intercanister dependency |
| 1 (last) | 3USD | Requires 3pool interaction, fallback complexity |

This isn't about incentivizing icUSD deposits — it's a robustness optimization. Use the simpler path when available, fall back to 3USD when more capital is needed.

### 3USD Valuation

3USD is valued at `virtual_price` for all stability pool accounting. The 3pool's `virtual_price()` returns `D * 10^8 / lp_total_supply` (scaled to ~1e18 when balanced). Currently ~1.0492e18, meaning 1 3USD ≈ $1.0492.

When covering X icUSD of vault debt with 3USD:
- 3USD consumed = `X * 1e18 / virtual_price` (in 8-decimal LP token units)
- This is queried from the 3pool at liquidation time, not cached.

### Deposit Flows

**Direct 3USD deposit**: Users who already hold 3USD LP tokens transfer them directly to the stability pool via ICRC-2. Standard flow, same as existing stablecoin deposits.

**Convenience wrapper (icUSD → 3USD)**: The stability pool offers a `deposit_as_3usd(token_ledger, amount)` endpoint that:
1. Receives icUSD (or ckUSDT/ckUSDC) from user via ICRC-2 transfer_from
2. Deposits into 3pool on user's behalf (approve + `add_liquidity`)
3. Credits the minted 3USD to user's stability pool position

If step 2 fails, the user's tokens are refunded. No intermediate state risk — the stability pool holds the tokens and can always return them.

### Withdrawal

Users withdraw 3USD directly (receive LP tokens). If they want underlying stables, they interact with the 3pool themselves after withdrawal. The stability pool doesn't unwrap on withdraw — keeps it simple.

## Liquidation: Atomic Burn Mechanism

### Happy Path

When a liquidation needs to consume 3USD deposits:

1. **Stability pool** calculates `icusd_to_burn` (the debt portion covered by 3USD) and `lp_to_burn` (3USD amount at current virtual price)
2. **Stability pool** calls 3pool's new `authorized_redeem_and_burn(icusd_amount, lp_amount, caller_authority)` endpoint
3. **3pool** validates the call is from an authorized canister, verifies the math checks out against virtual price (within tolerance), then:
   - Burns `lp_amount` from the stability pool's LP balance
   - Reduces its internal icUSD reserve by `icusd_amount`
   - Calls icUSD ledger to burn the icUSD tokens
   - Returns success with actual amounts burned
4. **Stability pool** reduces 3USD depositors' balances proportionally, distributes collateral gains
5. **Backend** has already written down the vault debt — the icUSD supply reduction from step 3 keeps the system balanced

### Fallback: Protocol Reserves

If the 3pool call fails (upgrade in progress, unexpected error, etc.):

1. **Stability pool** transfers the `lp_to_burn` amount of 3USD to the backend's protocol reserve address
2. **Stability pool** reduces 3USD depositors' balances proportionally, distributes collateral gains as normal
3. **Backend** holds the 3USD in protocol reserves for manual resolution
4. The vault debt is still written down — there's a temporary accounting gap where icUSD supply hasn't been reduced, but the protocol holds 3USD worth ≥ the gap amount
5. Protocol admin can later: redeem 3USD for icUSD and burn it, redeem for ckstables and hold as reserves, or wait for favorable 3pool conditions

This fallback is acceptable because:
- It only triggers on 3pool failure, which should be rare
- The 3USD is real value — the protocol isn't losing anything
- The accounting gap is bounded and trackable
- Manual resolution is straightforward

## 3Pool: `authorized_redeem_and_burn` Endpoint

### Design: General-Purpose, Not Liquidation-Specific

This is a general authorized redemption + burn function, not tied to stability pool liquidation. It enables future use cases like peg management (redeem 3USD for icUSD when the pool is heavy on icUSD, then burn).

```rust
#[update]
pub async fn authorized_redeem_and_burn(
    args: AuthorizedRedeemAndBurnArgs,
) -> Result<RedeemAndBurnResult, ThreePoolError>

pub struct AuthorizedRedeemAndBurnArgs {
    /// Which token to remove from the pool and burn (by ledger principal)
    pub token_ledger: Principal,
    /// Amount of the token to remove and burn
    pub token_amount: u128,
    /// Amount of LP tokens to burn in exchange
    pub lp_amount: u128,
    /// Maximum acceptable slippage from virtual price (basis points)
    pub max_slippage_bps: u16,
}

pub struct RedeemAndBurnResult {
    pub token_amount_burned: u128,
    pub lp_amount_burned: u128,
    pub burn_block_index: u64,
}
```

### Authorization

The 3pool maintains a set of authorized canisters that can call this function. Initially just the stability pool, but extensible to governance or other protocol canisters.

```rust
// In 3pool state
authorized_burn_callers: BTreeSet<Principal>,
```

Admin-managed. Not hardcoded.

### Validation

The function validates:
1. Caller is in `authorized_burn_callers`
2. `token_ledger` is one of the pool's registered tokens
3. The LP-to-token ratio is within `max_slippage_bps` of the current virtual price
4. The pool has sufficient balance of the target token
5. The stability pool's LP balance covers `lp_amount`

### Execution

1. Deduct `lp_amount` from caller's LP balance
2. Reduce `lp_total_supply` by `lp_amount`
3. Reduce pool's internal balance for `token_ledger` by `token_amount`
4. Call `token_ledger.icrc1_transfer` to burn address (or minter for icUSD) for `token_amount`
5. Return result with actual amounts and block index

If the ledger burn call fails, the entire operation reverts (LP and balances restored).

## State Changes

### Stability Pool

```rust
// StablecoinConfig addition for 3USD
StablecoinConfig {
    ledger_id: Principal,    // 3USD LP token ledger
    symbol: "3USD",
    decimals: 8,
    priority: 1,             // consumed last
    is_active: true,
    is_lp_token: true,       // new field — signals this token needs special liquidation handling
    underlying_pool: Option<Principal>,  // 3pool canister ID — used for atomic burn calls
}

// New field on DepositPosition (already supports BTreeMap<Principal, u64>)
// 3USD just gets a new entry in stablecoin_balances keyed by 3USD ledger principal
// No structural change needed

// Pool-level state addition
StabilityPoolState {
    // ... existing fields ...
    protocol_reserve_address: Option<Principal>,  // backend canister for fallback 3USD transfers
}
```

### 3Pool

```rust
// New state fields
authorized_burn_callers: BTreeSet<Principal>,

// New admin endpoints
add_authorized_burn_caller(caller: Principal)
remove_authorized_burn_caller(caller: Principal)
get_authorized_burn_callers() -> Vec<Principal>
```

### Backend

No backend changes required for the core feature. The backend doesn't need to know about 3USD — it just writes down vault debt when the stability pool calls `stability_pool_liquidate`. The icUSD supply reduction happens on the 3pool side.

The only backend addition is accepting 3USD transfers for the fallback path, which works automatically since the backend can receive any ICRC-1 token.

## Sequence Diagrams

### Liquidation Happy Path (3USD portion)

```
Backend                    Stability Pool              3Pool                 icUSD Ledger
  |                              |                       |                       |
  |--notify_liquidatable_vaults->|                       |                       |
  |                              |                       |                       |
  |                              |  (draws down icUSD/ckstables first per priority)
  |                              |                       |                       |
  |                              |  (needs 3USD for remaining debt)               |
  |                              |                       |                       |
  |                              |--authorized_redeem_    |                       |
  |                              |  and_burn(icusd,lp)--->|                       |
  |                              |                       |--icrc1_transfer(burn)->|
  |                              |                       |<----block_index--------|
  |                              |<--RedeemAndBurnResult--|                       |
  |                              |                       |                       |
  |<-stability_pool_liquidate----|                       |                       |
  |----collateral transfer------>|                       |                       |
  |                              |  (distribute gains, reduce 3USD balances)      |
```

### Liquidation Fallback Path

```
Backend                    Stability Pool              3Pool                 icUSD Ledger
  |                              |                       |                       |
  |                              |--authorized_redeem_    |                       |
  |                              |  and_burn(icusd,lp)--->|                       |
  |                              |                       |  (FAILS - upgrade/error)
  |                              |<------error------------|                       |
  |                              |                       |                       |
  |<--icrc1_transfer(3USD)-------|  (send 3USD to protocol reserves)              |
  |                              |                       |                       |
  |<-stability_pool_liquidate----|  (proceed with liquidation anyway)             |
  |----collateral transfer------>|                       |                       |
  |                              |  (distribute gains, reduce 3USD balances)      |
```

### Convenience Deposit (icUSD → 3USD)

```
User                    Stability Pool              3Pool
  |                           |                       |
  |--deposit_as_3usd(icUSD)-->|                       |
  |                           |--icrc2_transfer_from-->|  (user's icUSD)
  |                           |--add_liquidity-------->|
  |                           |<--3USD LP tokens-------|
  |                           |                       |
  |                           |  (credit user's 3USD balance)
  |<---deposit confirmation---|                       |
```

## Edge Cases

### 3Pool imbalance after repeated liquidations
Each liquidation removes icUSD from the 3pool, making it lighter on icUSD. This creates arbitrage opportunity — traders buy cheap icUSD from the pool, rebalancing it. This is pro-peg behavior and a feature, not a bug.

### Virtual price changes between calculation and execution
The `max_slippage_bps` parameter on `authorized_redeem_and_burn` handles this. If virtual price moves significantly between the stability pool's calculation and the 3pool's execution, the call fails and falls back to protocol reserves.

### All stability pool capital is 3USD
If no one deposits icUSD/ckstables (likely scenario since 3USD is strictly better for users), every liquidation goes through the 3pool path. This is fine — the 3pool is a core protocol canister and should be highly available. The fallback exists for the rare case it isn't.

### 3USD virtual price drops below 1.0
Unlikely but possible if the 3pool's underlying stables depeg. In this case 3USD depositors absorb more loss per dollar of debt covered. This is the correct behavior — they accepted the 3pool risk when they deposited 3USD instead of raw stables.

## What This Design Does NOT Include

- **Redemption routing through 3USD**: Redemptions remain icUSD-only. 3USD in the stability pool is for liquidations, not redemptions.
- **Automatic peg management**: The `authorized_redeem_and_burn` function enables future peg management (redeem when pool is heavy on icUSD), but the automation logic is out of scope.
- **Yield distribution**: 3USD virtual price appreciation accrues to depositors implicitly (their 3USD is worth more when they withdraw). No explicit yield tracking needed.
- **Frontend changes**: UI for depositing 3USD into the stability pool is out of scope for this design. Implement after backend is proven.
