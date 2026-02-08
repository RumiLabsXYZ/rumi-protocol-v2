# PR #1 Audit Review: ckUSDT/ckUSDC Repayment & Liquidation

**Date:** 2026-02-07 (updated 2026-02-08)
**Reviewer:** Rob (with AI audit assistance)
**PR:** `feature/ckusdt-ckusdc-repayment` → `main`
**Author:** Agnes
**Status:** ❌ DO NOT MERGE — critical issues identified
**Design spec:** See `docs/STABLE_REPAYMENT_DESIGN.md` for agreed fixes

---

## Summary

This PR adds the ability to repay vault debt or liquidate underwater vaults using ckUSDT or ckUSDC instead of icUSD, at a hardcoded 1:1 exchange rate. Changes span the full stack: Rust backend (new canister endpoints, transfer functions, state fields), Candid interface (new types and methods), treasury (new asset types), and Svelte frontend (token selector dropdown, new API client method).

---

## 🔴 Critical Issues (Must Fix Before Merge)

### 1. Decimal Mismatch — 6-decimal tokens treated as 8-decimal

**Severity:** CRITICAL — potential 100x fund loss
**Files:** `vault.rs`, `management.rs`, `apiClient.ts`

ckUSDT and ckUSDC on ICP use **6 decimal places** (1 USDT = 1,000,000 raw units). icUSD uses **8 decimal places** (1 icUSD = 100,000,000 raw units).

The frontend sends amounts as e8s (`amount * 10^8`), the backend treats them as e8s for debt accounting, but then passes the same raw number to 6-decimal stable ledgers. A user repaying 1 icUSD would have 100 USDT pulled from their wallet.

**Resolution:** All conversion happens in the backend. `stable_e6s = icusd_e8s / 100`. Truncate to nearest 100 e8s for clean division. See `STABLE_REPAYMENT_DESIGN.md` for full spec.

---

### 2. No icUSD Burned on Stable Repayment

**Severity:** CRITICAL (originally) → **RESOLVED BY DESIGN**
**Files:** `vault.rs`

When repaying with ckUSDT/ckUSDC, the vault's icUSD debt decreases but no icUSD is burned from circulation.

**Resolution:** This is intentional. No burn needed. The accumulated ckstables are held in the backend canister as a **protocol reserve** that can later be used to meet redemptions during icUSD depeg scenarios (future feature). The system-wide collateral ratio formula is adjusted to account for the reserve:

```
total_icusd_repaid_via_stables = total_ckstable_held / (1 + ckstable_repay_fee)
Adjusted CR = total_icp_collateral_value / (total_icusd_minted - total_icusd_repaid_via_stables)
```

More icUSD in circulation is desirable for liquidity. The reserve backs the "excess" supply. See `STABLE_REPAYMENT_DESIGN.md` for full details.

---

### 3. No ICRC-2 Approval Flow for ckUSDT/ckUSDC in Frontend

**Severity:** CRITICAL — all stable repayments will fail
**Files:** `VaultDetails.svelte`, `apiClient.ts`

The new stable repayment path calls `ApiClient.repayToVaultWithStable()` which does a `transfer_from` on the backend, but the user never approved the protocol canister to spend their ckUSDT/ckUSDC. Every stable repayment will fail with `InsufficientAllowance`.

**Resolution:** Add ICRC-2 `approve` call to the appropriate stable token ledger before calling the backend endpoint. Follow the same pattern as the existing icUSD approval flow in `protocolManager.repayToVault()`. Ledger canister IDs are already in `config.ts`.

---

### 4. Missing `validate_call()` on `liquidate_vault_partial_with_stable`

**Severity:** CRITICAL — allows anonymous callers, skips price staleness check
**File:** `main.rs`

The new liquidation endpoint is missing `validate_call()?;` which every other mutation endpoint has. This means anonymous principals can call it and the ICP price staleness check is skipped.

**Resolution:** Add `validate_call()?;` before `check_postcondition` in `liquidate_vault_partial_with_stable`. One-line fix.

---

## 🟡 Significant Concerns (Should Fix)

### 5. Hardcoded 1:1 Peg Assumption — No Depeg Protection

**Files:** `vault.rs`, design-level

The feature assumes ckUSDT = ckUSDC = 1 icUSD at all times with no protection against stablecoin depegs.

**Resolution:** Three layers of protection:
1. **Configurable fee (`ckstable_repay_fee`):** Starting at 0.02%, can be cranked up to 5% to discourage usage during depeg events. Acts as a soft kill-switch.
2. **Hard kill-switch:** Add `set_stable_token_enabled(token_type, bool)` admin function to completely disable acceptance of a specific token. Developer-only (no anonymous callers).
3. **Price freshness:** Backend assumes $1 value for ckstables but all actions require a fresh ICP price from XRC (calls under 30s old can be reused, otherwise a new call is made). No regular interval calls needed — on-demand only.

---

### 6. Frontend Debug Code Left In — Artificial 1.2s Delay

**File:** `apiClient.ts` → `repayToVaultWithStable`

```typescript
// Simulate processing delay
await new Promise(resolve => setTimeout(resolve, 1200));
```

This is a hardcoded artificial delay with a comment saying "simulate." Not compensating for anything architectural — just dead debug code adding unnecessary latency.

**Resolution:** Remove entirely.

---

### 7. Protocol Accumulates ckUSDT/ckUSDC With No Management Mechanism

**Files:** `management.rs`, `vault.rs`

Stablecoins accumulate in the backend canister with no way to manage or use them.

**Resolution:** This is now intentional — the backend canister holds ckstables as a **protocol reserve**. Balances are queried directly from ckUSDT/ckUSDC ledgers via `icrc1_balance_of` (source of truth, no internal counter needed). Future feature: allow redemptions of icUSD for ckstables during depeg scenarios. Frontend stats should surface these reserve balances at a later date.

---

### 8. Excessive Code Duplication

**Files:** `management.rs`, `vault.rs`

`transfer_ckusdt_from` and `transfer_ckusdc_from` are identical except for which ledger principal they read. Similarly, `liquidate_vault_partial_with_stable` is a near-complete copy of `liquidate_vault_partial` (~150 lines).

**Resolution:** Consolidate into single generic functions: `transfer_stable_from(token_type, amount, caller)` in management.rs, and refactor liquidation to share a common implementation with a token-type parameter.

---

## 🟢 Minor / Pre-existing Issues

### 9. Unsafe `.unwrap()` on Vault Lookup — CANISTER TRAP RISK

**File:** `vault.rs`

```rust
let vault = read_state(|s| s.vault_id_to_vaults.get(&arg.vault_id).cloned().unwrap());
```

If called with a vault ID that doesn't exist, the canister **traps** (panics) — the entire call crashes instead of returning a clean error. This is a pre-existing issue not introduced by this PR, but it appears in multiple functions:
- `repay_to_vault`
- `repay_to_vault_with_stable` (new in this PR)
- `borrow_from_vault`
- `add_margin_to_vault`

**Resolution:** Fix all instances. Replace `.unwrap()` with `.ok_or(ProtocolError::GenericError("Vault not found".to_string()))?` and add proper guard cleanup on error. This should be included in the fix branch since we're touching these functions anyway.

---

### 10. Unrelated package-lock.json Changes

The `package-lock.json` diff is all `"peer": true` annotation additions. Harmless — Agnes may have had a reason (dependency resolution). Just noting for completeness.

---

## Action Items

| # | Issue | Severity | Resolution | Status |
|---|-------|----------|------------|--------|
| 1 | Decimal mismatch (6 vs 8 decimals) | 🔴 CRITICAL | Backend conversion: `stable_e6s = icusd_e8s / 100` | To implement |
| 2 | No icUSD burn on stable repayment | 🔴→✅ RESOLVED | Intentional: ckstables held as protocol reserve, CR formula adjusted | Design complete |
| 3 | Missing ICRC-2 approval flow | 🔴 CRITICAL | Add approval call following existing icUSD pattern | To implement |
| 4 | Missing `validate_call()` | 🔴 CRITICAL | Add one line to `main.rs` | To implement |
| 5 | No depeg protection | 🟡 SIGNIFICANT | Configurable fee + hard kill-switch + price freshness | To implement |
| 6 | Debug `setTimeout` delay | 🟡 SIGNIFICANT | Remove entirely | To implement |
| 7 | No stablecoin management | 🟡→✅ RESOLVED | Intentional reserve in backend canister, queried from ledgers | Design complete |
| 8 | Code duplication | 🟡 SIGNIFICANT | Consolidate into generic functions | To implement |
| 9 | Unsafe `.unwrap()` on vault lookup | 🟢 PRE-EXISTING | Fix all instances with proper error handling | To implement |
| 10 | package-lock.json noise | 🟢 COSMETIC | No action needed | Noted |

---

## Workflow

Fixes will be implemented on a branch off `feature/ckusdt-ckusdc-repayment` (e.g., `feature/ckusdt-ckusdc-repayment-fixes`) and PR'd back into `feature/ckusdt-ckusdc-repayment` for Agnes to review before the main PR is merged.
