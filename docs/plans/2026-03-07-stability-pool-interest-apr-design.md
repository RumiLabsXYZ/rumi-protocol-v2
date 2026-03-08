# Stability Pool Interest Distribution & APR Display

**Date:** 2026-03-07
**Status:** Approved

## Problem

The backend mints 75% of vault debt interest as icUSD to the stability pool canister, but the pool canister has no mechanism to distribute that interest to individual depositors. The icUSD sits in the canister's wallet and is never credited to `DepositPosition.stablecoin_balances`. Depositor balances appear flat. There is also no APR metric displayed anywhere in the UI.

## Solution Overview

1. Backend notifies the pool canister after each interest mint with the exact amount.
2. Pool distributes the interest pro-rata across all depositors of that token.
3. Backend computes a live stability pool APR from current vault state.
4. Frontend displays the APR prominently and shows per-user interest earned.
5. Liquidation records are enriched with collateral price data for future ROI calculations.

## Design

### 1. Backend → Pool: Interest Notification

After the existing `mint_interest_to_stability_pool()` call in `treasury.rs` successfully mints icUSD, the backend makes an additional inter-canister call to the pool:

```
stability_pool.receive_interest_revenue(token_ledger, amount)
```

- `token_ledger`: the icUSD ledger principal
- `amount`: the exact amount minted (native decimals)
- Fire-and-forget with logging on failure (non-critical, same as the mint itself)
- If this call fails, the icUSD is still in the pool canister's wallet — a future reconciliation mechanism or admin function can recover it

### 2. Pool Canister: `receive_interest_revenue`

New update endpoint on the stability pool canister:

**Auth:** Only callable by `protocol_canister_id` (already stored in pool state).

**Logic:**
```
fn receive_interest_revenue(token_ledger, amount):
    validate caller == protocol_canister_id
    validate token_ledger is in stablecoin_registry

    total = total_stablecoin_balances[token_ledger]
    if total == 0: return (no depositors to credit)

    remainder = amount
    for each depositor with balance > 0 for this token:
        credit = amount * (user_balance / total)
        user_balance += credit
        user.total_interest_earned += normalize_to_e8s(credit)
        remainder -= credit

    // dust from rounding goes to first eligible depositor
    if remainder > 0:
        first_depositor.balance += remainder

    total_stablecoin_balances[token_ledger] += amount
    total_interest_received_e8s += normalize_to_e8s(amount)
```

Integer division will produce rounding dust (at most `n-1` units where n = depositor count). This is sub-cent and the dust-to-first-depositor approach keeps accounting clean.

### 3. Pool State Changes

**`StabilityPoolState`** — add:
- `total_interest_received_e8s: u64` — lifetime interest received by pool (for analytics)

**`DepositPosition`** — add:
- `total_interest_earned_e8s: u64` — per-user lifetime interest earned (displayed in UI)

**`StabilityPoolStatus`** (query response) — add:
- `total_interest_received_e8s: u64` — exposed for frontend

**`UserStabilityPosition`** (query response) — add:
- `total_interest_earned_e8s: u64` — exposed for frontend

All new fields default to 0 for existing state on canister upgrade (serde default).

### 4. Backend: Live APR Calculation

The backend already has `weighted_average_interest_rate()` which computes a debt-weighted average interest rate across all vaults. The stability pool APR is:

```
pool_apr = weighted_avg_rate * interest_pool_share * total_outstanding_debt / pool_tvl
```

Where:
- `weighted_avg_rate`: from existing `weighted_average_interest_rate()` on State
- `interest_pool_share`: configurable Ratio (default 0.75), read from state
- `total_outstanding_debt`: sum of all vault `borrowed_icusd_amount`
- `pool_tvl`: total value deposited in the stability pool (e8s)

Added to `ProtocolStatus` response as `stability_pool_apr: f64`.

Returns 0.0 when pool TVL is zero.

**Note on compounding:** Vault interest compounds every ~300 seconds (each accrual tick applies simple interest on the current debt, which includes prior interest). The displayed figure is an APR. The effective APY is slightly higher but the difference is negligible at typical rates (~13 bps at 5% APR). We display APR, consistent with DeFi convention.

### 5. Future Liquidation ROI Groundwork

Add to `PoolLiquidationRecord`:
- `collateral_price_e8s: u64` — XRC price of the collateral at liquidation time

The backend already has this price (it's what triggered the liquidation check). Pass it through in the liquidation notification. This enables future computation of:

```
liquidation_roi = (collateral_value_at_liquidation - stables_consumed) / stables_consumed
```

No UI for this yet — just laying the data foundation.

### 6. Frontend Changes

**PoolStats component:**
- Add a prominent APR display card reading `stability_pool_apr` from `ProtocolStatus`
- Format as percentage with 2 decimal places (e.g., "3.75% APR")

**UserAccount component:**
- Show `total_interest_earned_e8s` as "Interest earned" below deposit balance
- Format as USD value

### 7. What Does NOT Change

- Deposit/withdraw/claim flows unchanged
- Liquidation distribution logic unchanged
- The existing ICRC-1 mint to pool canister stays — the notification call is additive
- No canister reinstall — upgrade only
- No hardcoded values — `interest_pool_share` read from configurable state

### 8. Scale Considerations

The pro-rata loop in `receive_interest_revenue` is O(n) over depositors. At ~600 instructions per depositor, this uses <1% of the 2B instruction budget at 10,000 depositors. Becomes a concern only around 500K+ depositors. If that happens, migrate to a Liquity-style cumulative scaling factor (O(1) distribution). Not needed now.
