# Design Spec: Stable Token Repayment (ckUSDT/ckUSDC)

**Date:** 2026-02-08
**Author:** Rob
**Status:** Draft — pending review before implementation
**Related:** PR #1, `docs/PR1_AUDIT_REVIEW.md`

---

## Overview

Allow vault owners to repay icUSD debt using ckUSDT or ckUSDC at a 1:1 rate plus a small fee. The protocol pulls slightly more stablecoin than the debt being cleared, keeps the difference as a fee, and reduces vault debt by the exact amount requested.

---

## Decimal Conversion

### The Problem

| Token  | Decimals | 1.00 human-readable = raw units |
|--------|----------|---------------------------------|
| icUSD  | 8        | 100,000,000                     |
| ckUSDT | 6        | 1,000,000                       |
| ckUSDC | 6        | 1,000,000                       |

All internal vault debt accounting is in 8-decimal icUSD units (e8s). Stable token ledgers operate in 6-decimal units (e6s).

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

### Stable Repayment Fee: 0.02% (adjustable)

The fee is a **surcharge on the stablecoin payment**, not a discount on debt reduction. This ensures vault debt reduces by the exact amount the user intended, with no leftover dust.

### Example

User has a vault with 100.00 icUSD debt and wants to fully repay with ckUSDT:

| Step | Description | Amount |
|------|-------------|--------|
| 1 | Debt to clear | 100.00 icUSD |
| 2 | Base ckUSDT needed (1:1) | 100.00 ckUSDT |
| 3 | Fee (0.02%) | 0.02 ckUSDT |
| 4 | Total pulled from user | 100.02 ckUSDT |
| 5 | Vault debt reduced by | 100.00 icUSD (exact) |
| 6 | Protocol keeps | 0.02 ckUSDT |

The "Max Repay" button in the UI should display `100.02 ckUSDT` as the total cost to clear 100.00 icUSD of debt.

### Fee Math (Backend)

```
debt_reduction_e8s = requested_amount  // exact icUSD debt reduction
base_stable_e6s = debt_reduction_e8s / 100  // 1:1 conversion
fee_e6s = base_stable_e6s * fee_rate  // 0.02% = 0.0002
total_pull_e6s = base_stable_e6s + fee_e6s  // what we pull from user's wallet
```

### Fee Destination

The fee (in ckUSDT/ckUSDC) stays in the protocol canister. Treasury routing for accumulated stablecoins should be implemented separately (see Open Questions).

### Adjustable Fee (Admin Function)

Add a developer-only admin function to update the stable repayment fee rate:

```
set_stable_repayment_fee(new_rate: f64) -> Result<(), ProtocolError>
```

- Only callable by `developer_principal`
- Stored in protocol state, survives upgrades
- Default: 0.0002 (0.02%)
- Can be raised to discourage usage during depeg events (acts as soft kill-switch)
- Can be set to 0 to waive the fee entirely
- Reasonable upper bound: 0.05 (5%) to prevent accidental misconfiguration

This should be persisted in state and included in the event log so fee changes are auditable.

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
- Add `stable_repayment_fee_rate: Ratio` field (default 0.0002)

### Management (`management.rs`)
- Consolidate `transfer_ckusdt_from` and `transfer_ckusdc_from` into a single function:
  ```rust
  pub async fn transfer_stable_from(
      token_type: StableTokenType,
      amount_e6s: u64,
      caller: Principal
  ) -> Result<u64, TransferFromError>
  ```

### Vault (`vault.rs`)
- `repay_to_vault_with_stable`:
  1. Truncate `debt_reduction_e8s` to nearest 100 for clean conversion
  2. Calculate `fee_e6s` from fee rate
  3. Calculate `total_pull_e6s = (debt_reduction_e8s / 100) + fee_e6s`
  4. Call `transfer_stable_from(token_type, total_pull_e6s, caller)`
  5. On success, reduce vault debt by `debt_reduction_e8s`
  6. Route fee to treasury (if configured)

- `liquidate_vault_partial_with_stable`: Same conversion and fee logic

### Main (`main.rs`)
- Add `validate_call()?` to `liquidate_vault_partial_with_stable`
- Add `set_stable_repayment_fee` admin endpoint
- Add `get_stable_repayment_fee` query endpoint

### Candid (`.did`)
- Add `set_stable_repayment_fee` and `get_stable_repayment_fee` to service definition

---

## Open Questions

1. **icUSD burn mechanism:** When the protocol accepts ckUSDT/ckUSDC for debt repayment, the icUSD that was originally minted for that debt is still in circulation. How do we handle this? Options:
   - Accept the accounting gap for now (icUSD supply slightly exceeds total debt) — revisit when DEX liquidity exists to swap stablecoins back to icUSD
   - Mint-and-burn: protocol mints icUSD to itself, immediately burns it, to keep supply aligned (wasteful but keeps accounting clean)
   - Use accumulated stablecoins as protocol reserves backing the excess icUSD

2. **Treasury routing for stablecoins:** Should accumulated ckUSDT/ckUSDC fees be routed to treasury automatically (like minting fees), or held in the protocol canister until a manual withdrawal?

3. **Depeg circuit breaker:** Beyond the adjustable fee, should there be a hard disable flag per token type? e.g., `set_stable_token_enabled(token_type, bool)`

---

## Implementation Order

1. Fix `validate_call()` on liquidation endpoint (one-line fix, merge immediately)
2. Add decimal conversion + fee logic to backend
3. Consolidate duplicate transfer functions
4. Add admin fee endpoints
5. Update frontend: approval flow, fee display, remove debug delay
6. Test on local replica with mock ckUSDT/ckUSDC ledgers
7. Deploy to staging for validation
