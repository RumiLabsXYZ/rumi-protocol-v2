# Design Spec: Stable Token Repayment (ckUSDT/ckUSDC)

**Date:** 2026-02-08
**Author:** Rob
**Status:** Draft — pending review before implementation
**Related:** PR #1, `docs/PR1_AUDIT_REVIEW.md`

---

## Overview

Allow vault owners to repay icUSD debt using ckUSDT or ckUSDC at a 1:1 rate plus a configurable fee. The protocol pulls slightly more stablecoin than the debt being cleared, keeps the difference as a fee, and reduces vault debt by the exact amount requested. The accumulated stablecoins are held in the backend canister as a **protocol reserve** that can later be used to meet redemptions (e.g., if icUSD depegs downward).

---

## Stablecoin Reserve Strategy

### Why No icUSD Burn

When the protocol accepts ckUSDT/ckUSDC for debt repayment, the icUSD that was originally minted against that vault's debt remains in circulation. This is intentional and desirable:

- **More icUSD in circulation** improves liquidity and adoption
- **The protocol accumulates a stablecoin reserve** (ckUSDT/ckUSDC held by the backend canister) that backs the "excess" icUSD
- **In a downward depeg scenario**, the protocol can offer redemptions for these stables, giving icUSD holders a floor

### Where Stables Are Held

All received ckUSDT/ckUSDC (both the 1:1 repayment amounts and the fees) accumulate in the **backend canister's own account**. This is the same canister that already holds ICP collateral from vaults.

No internal counter is needed to track the reserve — the ckUSDT and ckUSDC ledgers are the source of truth. Query the backend canister's balance on each ledger via standard `icrc1_balance_of` calls to get the current reserve amounts. These should be surfaced on the frontend stats at a later date.

### Adjusted Collateral Ratio Formula

The system-wide collateral ratio must account for the stablecoin reserve. The reserve effectively reduces the amount of icUSD that needs to be backed by ICP collateral:

```
total_icusd_repaid_via_stables = total_ckstable_held / (1 + ckstable_repay_fee)

Adjusted CR = total_icp_collateral_value / (total_icusd_minted - total_icusd_repaid_via_stables)
```

**Why `/ (1 + fee)` and not `* (1 - fee)`:** For every 100 icUSD of debt repaid, the protocol holds 100.02 ckstable (at 0.02% fee). To recover the exact icUSD amount from the ckstable total: `100.02 / 1.0002 = 100.00`. Multiplying by `0.9998` gives `99.9999604` — close but not exact. Division is the precise inverse.

**Note:** `total_ckstable_held` is the sum of ckUSDT + ckUSDC balances held by the backend canister, queried from the respective ledgers. `ckstable_repay_fee` is the current value of the configurable fee parameter (see below).

---

## Decimal Conversion

### The Problem

| Token  | Decimals | 1.00 human-readable = raw units |
|--------|----------|---------------------------------|
| icUSD  | 8        | 100,000,000                     |
| ckUSDT | 6        | 1,000,000                       |
| ckUSDC | 6        | 1,000,000                       |

All internal vault debt accounting is in 8-decimal icUSD units (e8s). Stable token ledgers operate in 6-decimal units (e6s). icUSD's decimals cannot be changed (immutable ledger init param, already deployed on mainnet with existing balances and transactions).

### Conversion Rule

```
stable_e6s = icusd_e8s / 100
```

When pulling ckUSDT/ckUSDC from a user's wallet to cover X icUSD of debt:
- Debt credit (icUSD side): exact X e8s reduction
- Ledger transfer (stable side): X / 100 e6s pulled from user (plus fee — see below)

### Rounding

Truncate in the protocol's favor. If the requested icUSD amount doesn't divide evenly by 100, round the debt credit **down** to the nearest amount that maps cleanly to a whole number of stable e6s units.

In practice this means at most 99 raw e8s units (~0.00000099 icUSD, far less than a millionth of a cent) of rounding per transaction. This is negligible and always favors the protocol.

### Where Conversion Happens

**All conversion happens in the backend.** The frontend always works in human-readable amounts (e.g., "100.00"). The backend is responsible for:
1. Converting the human-readable amount to e8s (icUSD debt side)
2. Converting to e6s (stable ledger side) by dividing by 100
3. Adding the fee surcharge (see below)

The frontend does NOT need to know about decimal differences between tokens.

---

## Fee Structure

### Configurable Fee Parameter: `ckstable_repay_fee`

A protocol-level configuration parameter stored in canister state:

- **Parameter name:** `ckstable_repay_fee`
- **Type:** `Ratio` (same type used for borrowing fee)
- **Default:** `0.0002` (0.02%)
- **Adjustable at runtime** via developer-only admin function
- **Persisted across upgrades** (stored in `State` struct, replayed from event log)
- **Reasonable bounds:** 0 (fee waived) to 0.05 (5% max, prevents accidental misconfiguration)

### Design Rationale for Starting at 0.02%

The primary purpose of this feature is to provide an escape hatch during low-liquidity launch conditions where users may not be able to acquire icUSD to repay their vaults. Starting with a near-zero fee encourages usage. The fee can be raised later as icUSD liquidity deepens:

- **Launch phase:** 0.02% — encourages adoption, covers rounding dust
- **Growth phase:** Could raise to 0.1%-0.5% as icUSD becomes liquid on DEXes
- **Depeg defense:** Can crank to 2%-5% to discourage usage if a ckstable depegs
- **Emergency:** Set to maximum to effectively disable the feature without a code change

### Fee Application: Surcharge on Payment (Not Discount on Credit)

The fee is added to what the user pays, NOT subtracted from the debt reduction. This ensures clean UX: vault debt reduces by the exact amount the user intended with no leftover dust.

### Example

User has a vault with 100.00 icUSD debt and wants to fully repay with ckUSDT:

| Step | Description | Amount |
|------|-------------|--------|
| 1 | Debt to clear | 100.00 icUSD |
| 2 | Base ckUSDT needed (1:1) | 100.00 ckUSDT |
| 3 | Fee (0.02%) | 0.02 ckUSDT |
| 4 | Total pulled from user | 100.02 ckUSDT |
| 5 | Vault debt reduced by | 100.00 icUSD (exact) |
| 6 | Protocol keeps all 100.02 ckUSDT | (reserve + fee) |

The "Max Repay" button in the UI should display `100.02 ckUSDT` as the total cost to clear 100.00 icUSD of debt.

### Fee Math (Backend)

```
debt_reduction_e8s = requested_amount               // exact icUSD debt reduction
debt_reduction_e8s = debt_reduction_e8s - (debt_reduction_e8s % 100)  // truncate for clean conversion
base_stable_e6s = debt_reduction_e8s / 100           // 1:1 conversion to 6-decimal
fee_e6s = base_stable_e6s * ckstable_repay_fee       // configurable fee
total_pull_e6s = base_stable_e6s + fee_e6s           // what we pull from user's wallet
```

### Admin Functions

```
set_ckstable_repay_fee(new_rate: f64) -> Result<(), ProtocolError>
get_ckstable_repay_fee() -> f64
```

- `set` is developer-only (checks `caller == developer_principal`)
- Fee changes are recorded in the event log for auditability
- Rejects values outside 0.0 to 0.05 range

---

## Stable Token Liquidation

The same fee and conversion logic applies to `liquidate_vault_partial_with_stable`. The liquidator pays in ckUSDT/ckUSDC (with fee surcharge), the protocol reduces vault debt by the exact icUSD amount, and the liquidator receives ICP collateral with the standard 10% bonus.

---

## Frontend Changes

### Repay Panel

When the user selects ckUSDT or ckUSDC from the token dropdown:

1. The "Max Repay" amount should show the total cost **including fee** in the selected stablecoin
2. Display the fee clearly: e.g., "100.00 icUSD debt → 100.02 ckUSDT (includes 0.02% fee)"
3. The amount input represents the **debt being repaid** (in icUSD terms), not the stablecoin amount pulled

### ICRC-2 Approval

Before calling `repay_to_vault_with_stable`, the frontend must:
1. Create an actor for the selected stable token ledger (ckUSDT or ckUSDC canister ID from config)
2. Call `icrc2_approve` on that ledger, approving the protocol backend canister to spend the total amount (base + fee)
3. Then call the backend `repay_to_vault_with_stable` endpoint

This should follow the same pattern as the existing icUSD approval flow in `protocolManager.repayToVault()`.

### Remove Debug Code

Remove the artificial delay from `apiClient.ts`:
```typescript
// DELETE THIS:
await new Promise(resolve => setTimeout(resolve, 1200));
```

---

## Backend Changes Summary

### State (`state.rs`)
- Add `ckstable_repay_fee: Ratio` field (default `Ratio::new(dec!(0.0002))`)
- Ensure field is included in event replay for upgrade persistence

### Management (`management.rs`)
- Consolidate `transfer_ckusdt_from` and `transfer_ckusdc_from` into a single function:
  ```rust
  pub async fn transfer_stable_from(
      token_type: StableTokenType,
      amount_e6s: u64,
      caller: Principal
  ) -> Result<u64, TransferFromError>
  ```
  Resolves the correct ledger principal internally based on `token_type`.

### Vault (`vault.rs`)
- `repay_to_vault_with_stable`:
  1. Truncate `debt_reduction_e8s` to nearest 100 for clean conversion
  2. Read `ckstable_repay_fee` from state
  3. Calculate `fee_e6s` from fee rate
  4. Calculate `total_pull_e6s = (debt_reduction_e8s / 100) + fee_e6s`
  5. Call `transfer_stable_from(token_type, total_pull_e6s, caller)`
  6. On success, reduce vault debt by `debt_reduction_e8s`

- `liquidate_vault_partial_with_stable`: Same conversion and fee logic

### Main (`main.rs`)
- Add `validate_call()?` to `liquidate_vault_partial_with_stable`
- Add `set_ckstable_repay_fee` admin endpoint (developer-only)
- Add `get_ckstable_repay_fee` query endpoint

### Collateral Ratio Calculation
- Update the system-wide CR calculation to use the adjusted formula:
  ```
  adjusted_cr = total_icp_value / (total_icusd_minted - (total_ckstable_held / (1 + ckstable_repay_fee)))
  ```
- `total_ckstable_held` is queried from ckUSDT + ckUSDC ledgers via `icrc1_balance_of`

### Candid (`.did`)
- Add `set_ckstable_repay_fee` and `get_ckstable_repay_fee` to service definition

---

## Open Questions

1. **Depeg circuit breaker:** Beyond the adjustable fee, should there be a hard disable flag per token type? e.g., `set_stable_token_enabled(token_type, bool)` — or is cranking the fee to 5% sufficient?

2. **Redemption for stables:** The reserve strategy envisions offering redemptions for ckstables during icUSD downward depeg events. This is a separate feature to be designed later, but the reserve accumulation mechanism should be built with this in mind.

3. **Frontend stats:** Surface ckUSDT/ckUSDC reserve balances and adjusted CR on the frontend dashboard. Low priority — can be added after core feature works.

---

## Implementation Order

1. Fix `validate_call()` on liquidation endpoint (one-line fix)
2. Add `ckstable_repay_fee` config parameter to state
3. Add decimal conversion + fee logic to backend
4. Consolidate duplicate transfer functions into `transfer_stable_from`
5. Add admin fee endpoints (`set_ckstable_repay_fee` / `get_ckstable_repay_fee`)
6. Update CR calculation with adjusted formula
7. Update frontend: approval flow, fee display, remove debug delay
8. Test on local replica with mock ckUSDT/ckUSDC ledgers
9. Deploy to staging for validation
