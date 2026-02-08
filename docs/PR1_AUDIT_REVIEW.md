# PR #1 Audit Review: ckUSDT/ckUSDC Repayment & Liquidation

**Date:** 2026-02-07  
**Reviewer:** Rob (with AI audit assistance)  
**PR:** `feature/ckusdt-ckusdc-repayment` → `main`  
**Author:** Agnes  
**Status:** ❌ DO NOT MERGE — critical issues identified

---

## Summary

This PR adds the ability to repay vault debt or liquidate underwater vaults using ckUSDT or ckUSDC instead of icUSD, at a hardcoded 1:1 exchange rate. Changes span the full stack: Rust backend (new canister endpoints, transfer functions, state fields), Candid interface (new types and methods), treasury (new asset types), and Svelte frontend (token selector dropdown, new API client method).

---

## 🔴 Critical Issues (Must Fix Before Merge)

### 1. Decimal Mismatch — 6-decimal tokens treated as 8-decimal

**Severity:** CRITICAL — potential 100x fund loss  
**Files:** `vault.rs`, `management.rs`, `apiClient.ts`

ckUSDT and ckUSDC on ICP use **6 decimal places** (1 USDT = 1,000,000 raw units). icUSD uses **8 decimal places** (1 icUSD = 100,000,000 raw units).

The code does:
```rust
let amount: ICUSD = arg.amount.into(); // Treat as 1:1 with icUSD
```

And the frontend sends:
```typescript
amount: BigInt(Math.floor(amount * E8S)),  // E8S = 10^8
```

But `transfer_ckusdt_from` / `transfer_ckusdc_from` pass the raw amount directly to the 6-decimal ledger:
```rust
amount: Nat::from(amount),
```

**Impact:** A user trying to repay 1 icUSD sends `100_000_000` as the raw amount. The backend pulls `100_000_000` raw units from their ckUSDT wallet — that's **100 USDT**, not 1 USDT. This is a 100x overcharge.

**Fix required:** Convert between 8-decimal (icUSD internal) and 6-decimal (ckUSDT/ckUSDC ledger) representations. The backend needs a scaling factor: `stable_amount = icusd_e8s_amount / 100` when calling the stable ledger, and the debt reduction should still use the 8-decimal icUSD value.

---

### 2. No icUSD Burned on Stable Repayment — Protocol Accounting Breaks

**Severity:** CRITICAL — breaks protocol invariants  
**Files:** `vault.rs` (`repay_to_vault_with_stable`, `liquidate_vault_partial_with_stable`)

When repaying with icUSD via `repay_to_vault`, the protocol receives icUSD and the vault's debt counter decreases. The icUSD supply naturally shrinks because the protocol holds those tokens.

When repaying with ckUSDT/ckUSDC via `repay_to_vault_with_stable`, the protocol receives ckUSDT, reduces the vault's icUSD debt counter, but **no icUSD is burned or removed from circulation**. 

**Impact:** Total icUSD supply stays the same while recorded debt goes down. Over time this breaks the fundamental accounting invariant: `total_icusd_minted ≈ total_outstanding_vault_debt`. The protocol becomes undercollateralized in terms of icUSD backing. This is exploitable: mint icUSD from vault, sell it on market, repay with cheap ckUSDT, keep the difference.

**Fix required:** When accepting ckUSDT/ckUSDC for repayment, the protocol must also burn the equivalent icUSD amount from somewhere (protocol reserves, or require the user to also surrender icUSD). Alternatively, the protocol needs to swap the received stablecoins for icUSD and burn that. This is an architectural question that needs a design decision before implementation.

---

### 3. No ICRC-2 Approval Flow for ckUSDT/ckUSDC in Frontend

**Severity:** CRITICAL — all stable repayments will fail  
**Files:** `VaultDetails.svelte`, `apiClient.ts`

The existing icUSD repayment goes through `protocolManager.repayToVault()` which handles ICRC-2 approval internally. The new stable path calls `ApiClient.repayToVaultWithStable()` directly, which just does the `transfer_from` backend call — but the user **never approved the protocol canister to spend their ckUSDT/ckUSDC**.

The code has this comment:
```typescript
// Note: User needs to approve the stable token transfer first
```

But no approval is actually requested anywhere in the code.

**Impact:** Every stable repayment will fail with an `InsufficientAllowance` error from the ledger. The feature is non-functional as written.

**Fix required:** Add ICRC-2 `approve` call to the ckUSDT/ckUSDC ledger before calling `repay_to_vault_with_stable`. This needs the ledger canister IDs (already in `config.ts`) and proper actor creation for those ledgers.

---

### 4. Missing `validate_call()` on `liquidate_vault_partial_with_stable`

**Severity:** CRITICAL — allows anonymous callers, skips price staleness check  
**File:** `main.rs`

The new liquidation endpoint:
```rust
async fn liquidate_vault_partial_with_stable(vault_id: u64, amount: u64, token_type: StableTokenType) -> Result<SuccessWithFee, ProtocolError> {
    check_postcondition(rumi_protocol_backend::vault::liquidate_vault_partial_with_stable(vault_id, amount, token_type).await)
}
```

Compare with `repay_to_vault_with_stable` which correctly has:
```rust
    validate_call()?;  // ← this is missing from the liquidation function
    check_postcondition(...)
```

`validate_call()` does two things: (1) blocks anonymous principals, and (2) checks the ICP price isn't stale. Without it, an attacker could call with `Principal::anonymous()` and liquidate vaults using an outdated ICP price.

**Fix required:** Add `validate_call()?;` before `check_postcondition` in `liquidate_vault_partial_with_stable`.

---

## 🟡 Significant Concerns (Should Fix)

### 5. Hardcoded 1:1 Peg Assumption — No Oracle, No Kill-Switch

**Files:** `vault.rs`, design-level

The entire feature assumes `ckUSDT = ckUSDC = 1 icUSD` at all times. Stablecoins do depeg — USDC dropped to ~$0.87 in March 2023, USDT has had episodes.

**Risk:** If ckUSDT depegs to $0.90, someone could buy cheap ckUSDT and repay $1-denominated icUSD debt at a 10% discount, extracting value from the protocol.

**Recommendation:** At minimum, add an admin function to enable/disable specific stable tokens. Ideally, use a price feed or set acceptable deviation bounds. Document this as a known risk in the protocol docs.

---

### 6. Frontend Debug Code Left In — Artificial 1.2s Delay

**File:** `apiClient.ts` → `repayToVaultWithStable`

```typescript
// Simulate processing delay
await new Promise(resolve => setTimeout(resolve, 1200));
```

This is a hardcoded 1.2-second artificial delay with a comment literally saying "simulate." It adds unnecessary latency to every stable repayment.

**Fix:** Remove the delay entirely.

---

### 7. Protocol Accumulates ckUSDT/ckUSDC With No Management Mechanism

**Files:** `management.rs`, `vault.rs`

The backend transfers ckUSDT/ckUSDC to the protocol canister's own account. But there's no function to:
- Convert/swap these stablecoins
- Withdraw them to treasury
- Route them anywhere useful

The treasury canister was extended with CKUSDT/CKUSDC asset types, but there's no routing logic (compare with the existing `route_minting_fee_to_treasury` pattern for icUSD fees). The stablecoins just accumulate in the canister with no way to manage them.

**Recommendation:** Add treasury routing for received stablecoins, or at minimum an admin withdrawal function.

---

### 8. Excessive Code Duplication

**Files:** `management.rs`, `vault.rs`

`transfer_ckusdt_from` and `transfer_ckusdc_from` are identical functions except for which ledger principal they read from state. Should be a single generic function:

```rust
pub async fn transfer_stable_from(token_type: StableTokenType, amount: u64, caller: Principal) -> Result<u64, TransferFromError> {
    let ledger_principal = match token_type {
        StableTokenType::CKUSDT => read_state(|s| s.ckusdt_ledger_principal),
        StableTokenType::CKUSDC => read_state(|s| s.ckusdc_ledger_principal),
    }.ok_or_else(|| ...)?;
    // ... single implementation
}
```

Similarly, `liquidate_vault_partial_with_stable` is a near-complete copy of `liquidate_vault_partial` (~150 lines of duplicated logic). Should be refactored to share a common implementation with a token-type parameter.

---

## 🟢 Minor / Pre-existing Issues

### 9. Unsafe `.unwrap()` on Vault Lookup

**File:** `vault.rs`

```rust
let vault = read_state(|s| s.vault_id_to_vaults.get(&arg.vault_id).cloned().unwrap());
```

This will panic and trap the canister if the vault doesn't exist. The existing `repay_to_vault` has the same pattern, so it's not new to this PR, but worth noting.

### 10. Unrelated package-lock.json Changes

The `package-lock.json` diff is all `"peer": true` annotation additions — harmless but adds noise to the PR.

---

## Action Items

| # | Issue | Severity | Owner | Status |
|---|-------|----------|-------|--------|
| 1 | Decimal mismatch (6 vs 8 decimals) | 🔴 CRITICAL | Agnes | Open |
| 2 | No icUSD burn on stable repayment | 🔴 CRITICAL | Agnes + Rob (design decision) | Open |
| 3 | Missing ICRC-2 approval flow in frontend | 🔴 CRITICAL | Agnes | Open |
| 4 | Missing `validate_call()` on liquidation | 🔴 CRITICAL | Agnes | Open |
| 5 | No depeg protection / kill-switch | 🟡 SIGNIFICANT | Rob (design decision) | Open |
| 6 | Remove debug `setTimeout` delay | 🟡 SIGNIFICANT | Agnes | Open |
| 7 | No mechanism to manage accumulated stablecoins | 🟡 SIGNIFICANT | Rob (design decision) | Open |
| 8 | Code duplication | 🟡 SIGNIFICANT | Agnes | Open |
| 9 | Unsafe `.unwrap()` on vault lookup | 🟢 MINOR | — | Pre-existing |
| 10 | Unrelated package-lock changes | 🟢 MINOR | — | Cosmetic |

---

## Design Questions to Resolve Before Re-implementation

1. **Burn mechanism:** When the protocol accepts ckUSDT/ckUSDC for debt repayment, how does the equivalent icUSD get removed from circulation? Options:
   - Protocol holds icUSD reserves and burns from those
   - Require a DEX swap (ckUSDT → icUSD) before burning
   - Don't reduce vault debt 1:1 — apply a conversion rate
   
2. **Depeg protection:** Should we use an oracle for ckUSDT/ckUSDC pricing, or is a manual admin toggle sufficient for now?

3. **Stablecoin management:** Where do received ckUSDT/ckUSDC go? Treasury? Protocol reserve? Automatically swapped?
