# PR #1 Audit Review: ckUSDT/ckUSDC Repayment & Liquidation

**Date:** 2026-02-07 (updated 2026-02-08)
**Reviewer:** Rob (with AI audit assistance)
**PR:** `feature/ckusdt-ckusdc-repayment` → `main`
**Author:** Agnes
**Status:** 🟡 FIXES IMPLEMENTED — pending Agnes review
**Design spec:** See `docs/STABLE_REPAYMENT_DESIGN.md`
**Fix branch:** `feature/ckusdt-ckusdc-repayment-fixes` (5 commits)

---

## Summary

This PR adds the ability to repay vault debt or liquidate underwater vaults using ckUSDT or ckUSDC instead of icUSD, at a hardcoded 1:1 exchange rate. Changes span the full stack: Rust backend (new canister endpoints, transfer functions, state fields), Candid interface (new types and methods), treasury (new asset types), and Svelte frontend (token selector dropdown, new API client method).

The original PR had 4 critical vulnerabilities and several significant concerns. All have been addressed on the fixes branch.

---

## 🔴 Critical Issues

### 1. Decimal Mismatch — 6-decimal tokens treated as 8-decimal

**Severity:** CRITICAL — potential 100x fund loss
**Files:** `vault.rs`, `management.rs`, `apiClient.ts`

ckUSDT/ckUSDC use 6 decimal places (1 USDT = 1,000,000 raw units). icUSD uses 8 (1 icUSD = 100,000,000 raw units). The code passed e8s amounts directly to 6-decimal ledgers — a user repaying 1 icUSD would have 100 USDT pulled from their wallet.

**Status:** ✅ FIXED in `ff3fcaf`
**What was done:**
- Backend `vault.rs`: Truncate `debt_reduction_e8s` to nearest 100 for clean division, then `stable_e6s = debt_reduction_e8s / 100`
- Backend `vault.rs`: Added `ckstable_repay_fee` surcharge (default 0.02%) applied on the payment side — user pays slightly more ckstable, debt reduction is exact. Fee calculated as `fee_e6s = base_stable_e6s * ckstable_repay_fee`
- `management.rs`: Transfer functions now receive amounts in e6s after conversion
- Frontend unchanged — sends human-readable amounts, backend does all conversion

---

### 2. No icUSD Burned on Stable Repayment

**Severity:** CRITICAL (originally) → ✅ **RESOLVED BY DESIGN**
**Files:** `vault.rs`

When repaying with ckUSDT/ckUSDC, the vault's icUSD debt decreases but no icUSD is burned from circulation.

**Status:** ✅ RESOLVED — no code change needed
**Design decision:** This is intentional. The accumulated ckstables are held in the backend canister as a **protocol reserve** for future redemptions during icUSD depeg scenarios. More icUSD in circulation is desirable for liquidity. The system-wide CR formula is adjusted:

```
total_icusd_repaid_via_stables = total_ckstable_held / (1 + ckstable_repay_fee)
Adjusted CR = total_icp_collateral_value / (total_icusd_minted - total_icusd_repaid_via_stables)
```

See `STABLE_REPAYMENT_DESIGN.md` for full details on the reserve strategy.

---

### 3. No ICRC-2 Approval Flow for ckUSDT/ckUSDC in Frontend

**Severity:** CRITICAL — all stable repayments would fail with `InsufficientAllowance`
**Files:** `VaultDetails.svelte`, `apiClient.ts`

The stable repayment path called the backend's `transfer_from` directly without the user ever approving the protocol canister to spend their ckUSDT/ckUSDC.

**Status:** ✅ FIXED in `49d65c6`
**What was done:**
- `walletOperations.ts`: Added `approveStableTransfer()` — calls `icrc2_approve` on the ckUSDT or ckUSDC ledger, with retry logic on stale actor errors. Reuses the icUSD ledger IDL since all ICRC-2 ledgers share the same interface.
- `walletOperations.ts`: Added `checkStableAllowance()` — queries current allowance on the stable token ledger before deciding whether to approve.
- `ProtocolManager.ts`: Added `repayToVaultWithStable()` — full approval flow mirroring the existing icUSD pattern: check current allowance → approve with 5% buffer if insufficient → wait 1.5s for approval to settle → verify approval → call `ApiClient.repayToVaultWithStable()`. Works in e6s (6 decimals) for the stable token ledger.
- `config.ts`: Added `getStableLedgerId(tokenType)` helper to resolve ckUSDT or ckUSDC ledger canister ID.
- `VaultDetails.svelte`: Updated stable repay path to call `protocolManager.repayToVaultWithStable()` instead of `ApiClient` directly. Removed manual `isApproving` flag since ProtocolManager handles processing stages.

---

### 4. Missing `validate_call()` on `liquidate_vault_partial_with_stable`

**Severity:** CRITICAL — allowed anonymous callers, skipped price staleness check
**File:** `main.rs`

The new liquidation endpoint was missing `validate_call()?;` which every other mutation endpoint has.

**Status:** ✅ FIXED in `028f4af`
**What was done:**
- `main.rs`: Added `validate_call()?;` before `check_postcondition` in `liquidate_vault_partial_with_stable`. One-line fix.

---

## 🟡 Significant Concerns

### 5. Hardcoded 1:1 Peg Assumption — No Depeg Protection

**Files:** `vault.rs`, design-level

The feature assumed ckUSDT = ckUSDC = 1 icUSD at all times with no protection against stablecoin depegs.

**Status:** ✅ FIXED in `ff3fcaf`
**What was done:**
- `state.rs`: Added `ckstable_repay_fee` (default 0.0002 / 0.02%) and `ckusdt_enabled` / `ckusdc_enabled` (default true) fields to protocol state.
- `main.rs`: Added `set_ckstable_repay_fee(new_rate)` admin endpoint — developer-only, rejects values outside 0.0–0.05 (0–5%) range. Can be cranked up during depeg events as a soft kill-switch.
- `main.rs`: Added `set_stable_token_enabled(token_type, enabled)` admin endpoint — developer-only hard kill-switch to completely disable acceptance of a specific token.
- `main.rs`: Added `get_ckstable_repay_fee()` and `get_stable_token_enabled(token_type)` query endpoints for transparency.
- `vault.rs`: Both `repay_to_vault_with_stable` and `liquidate_vault_partial_with_stable` check the enabled flag before processing and reject with error if disabled.
- `.did` file: Updated Candid interface with all new admin endpoints.
- Price freshness: Backend assumes $1 for ckstables but actions require a fresh ICP price from XRC (calls under 30s old reused, otherwise new call). On-demand only, no interval timer.

---

### 6. Frontend Debug Code Left In — Artificial 1.2s Delay

**File:** `apiClient.ts` → `repayToVaultWithStable`

Hardcoded `setTimeout(resolve, 1200)` with a comment saying "simulate processing delay." Not compensating for anything — just dead debug code.

**Status:** ✅ FIXED in `b5d6c53`
**What was done:**
- `apiClient.ts`: Removed the `await new Promise(resolve => setTimeout(resolve, 1200));` and its comment. 3 lines deleted.

---

### 7. Protocol Accumulates ckUSDT/ckUSDC With No Management Mechanism

**Files:** `management.rs`, `vault.rs`

Stablecoins accumulate in the backend canister with no path to manage or use them.

**Status:** ✅ RESOLVED — no code change needed
**Design decision:** Intentional. The backend canister holds ckstables as a **protocol reserve**. Balances are queried directly from ckUSDT/ckUSDC ledgers via `icrc1_balance_of` (source of truth, no internal counter needed). Future feature: allow redemptions of icUSD for ckstables during depeg scenarios. Frontend stats should surface reserve balances at a later date.

---

### 8. Excessive Code Duplication

**Files:** `management.rs`, `vault.rs`

`transfer_ckusdt_from` and `transfer_ckusdc_from` were identical except for which ledger principal they read. Liquidation function was ~150 lines of duplicated logic.

**Status:** ✅ FIXED in `ff3fcaf`
**What was done:**
- `management.rs`: Consolidated `transfer_ckusdt_from` and `transfer_ckusdc_from` into a single `transfer_stable_from(token_type, amount_e6s, caller)`. Resolves the correct ledger principal internally via `match token_type`. Net reduction: ~40 lines removed.
- `vault.rs`: Liquidation logic refactored to share common implementation with token-type parameter.

---

## 🟢 Minor / Pre-existing Issues

### 9. Unsafe `.unwrap()` on Vault Lookup — CANISTER TRAP RISK

**File:** `vault.rs`

Vault lookups used `.cloned().unwrap()` which traps (panics) the canister if a vault ID doesn't exist. Pre-existing in `repay_to_vault`, `borrow_from_vault`, `add_margin_to_vault`, and carried into new `repay_to_vault_with_stable`.

**Status:** ✅ FIXED in `5bca9d1`
**What was done:**
- `vault.rs`: Replaced all 3 instances of `.cloned().unwrap()` with `.cloned().ok_or_else(|| ProtocolError::GenericError("Vault not found".to_string()))?`
- For functions with `guard_principal`: added `guard_principal.fail()` in the error closure to properly clean up the guard on vault-not-found.
- For `add_margin_to_vault` which uses `_guard_principal` (auto-drop): simple `.ok_or_else()` without explicit fail since the guard drops on scope exit.

---

### 10. Unrelated package-lock.json Changes

The `package-lock.json` diff is all `"peer": true` annotation additions. Harmless — Agnes may have had a reason (dependency resolution). No action taken.

---

## Final Status

| # | Issue | Severity | Status | Commit |
|---|-------|----------|--------|--------|
| 1 | Decimal mismatch (6 vs 8 decimals) | 🔴 CRITICAL | ✅ Fixed | `ff3fcaf` |
| 2 | No icUSD burn on stable repayment | 🔴→✅ | Resolved by design | N/A |
| 3 | Missing ICRC-2 approval flow | 🔴 CRITICAL | ✅ Fixed | `49d65c6` |
| 4 | Missing `validate_call()` | 🔴 CRITICAL | ✅ Fixed | `028f4af` |
| 5 | No depeg protection | 🟡 SIGNIFICANT | ✅ Fixed | `ff3fcaf` |
| 6 | Debug `setTimeout` delay | 🟡 SIGNIFICANT | ✅ Fixed | `b5d6c53` |
| 7 | No stablecoin management | 🟡→✅ | Resolved by design | N/A |
| 8 | Code duplication | 🟡 SIGNIFICANT | ✅ Fixed | `ff3fcaf` |
| 9 | Unsafe `.unwrap()` on vault lookup | 🟢 PRE-EXISTING | ✅ Fixed | `5bca9d1` |
| 10 | package-lock.json noise | 🟢 COSMETIC | Noted | N/A |

---

## Fix Branch: `feature/ckusdt-ckusdc-repayment-fixes`

5 commits, branched off `feature/ckusdt-ckusdc-repayment`:

| Commit | Files Changed | Description |
|--------|--------------|-------------|
| `028f4af` | `main.rs` (+1 line) | #4: Add `validate_call()` to stable liquidation |
| `b5d6c53` | `apiClient.ts` (-3 lines) | #6: Remove debug setTimeout |
| `5bca9d1` | `vault.rs` (+12/-3) | #9: Replace `.unwrap()` panics with proper error handling |
| `ff3fcaf` | `vault.rs`, `management.rs`, `main.rs`, `state.rs`, `.did` (+148/-81) | #1, #5, #8: Decimal conversion, configurable fee, kill-switch, code dedup |
| `49d65c6` | `walletOperations.ts`, `ProtocolManager.ts`, `config.ts`, `VaultDetails.svelte` (+212/-5) | #3: Full ICRC-2 approval flow for stable repayments |

**Next step:** PR `feature/ckusdt-ckusdc-repayment-fixes` → `feature/ckusdt-ckusdc-repayment` for Agnes to review.

---

## Future Work (Not in Scope for This PR)

- **Redemption feature:** Allow icUSD holders to redeem for ckstables from the protocol reserve during depeg scenarios
- **Frontend stats:** Surface ckUSDT/ckUSDC reserve balances and adjusted CR on dashboard
- **Adjusted CR calculation:** Implement the formula in the backend query that returns system-wide stats
