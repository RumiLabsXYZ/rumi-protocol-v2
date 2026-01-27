# Oisy Wallet ICRC-2 Support Investigation

**Date:** January 27, 2026  
**Status:** Testing in Progress  
**Branch:** `test/oisy-icrc2-repayment`  
**Author:** Development Team

---

## Table of Contents

1. [Executive Summary](#executive-summary)
2. [Background & Problem Statement](#background--problem-statement)
3. [Technical Analysis](#technical-analysis)
4. [What We Implemented](#what-we-implemented)
5. [Test Protocol](#test-protocol)
6. [Expected Outcomes & Next Steps](#expected-outcomes--next-steps)
7. [File Reference](#file-reference)
8. [Historical Context](#historical-context)

---

## Executive Summary

Oisy wallet users cannot perform vault operations (repayment, borrowing, etc.) because the frontend preemptively blocks all ICRC-2 calls for Oisy wallets. This block was added after vault creation failed with "Unsupported Canister Call: icrc2_allowance" errors on the **ICP ledger**.

However, new evidence suggests Oisy may actually support ICRC-2 operations. The key insight is that the original failure was specifically on the **ICP ledger**, while repayment uses the **icUSD ledger** - a different canister that may have different behavior.

We created a test branch that:
1. Keeps ICP ICRC-2 blocks in place (known to fail)
2. Removes icUSD ICRC-2 blocks with detailed logging
3. Allows us to capture the exact error (if any) when Oisy attempts `icrc2_allowance` and `icrc2_approve` on the icUSD ledger

---

## Background & Problem Statement

### The User Experience Issue

When a user connects with Oisy wallet and attempts to:
- Repay icUSD to a vault
- Borrow additional icUSD
- Add margin to a vault
- Close a vault

They immediately see: **"Oisy wallet does not currently support vault operations. Please use Plug Wallet or Internet Identity."**

This error appears **before** any actual ICRC-2 call is attempted.

### Why the Block Exists

The block was added after vault **creation** failed. The original error flow was:

```
User attempts: Create vault with ICP collateral
Frontend calls: icrc2_allowance on ICP ledger (ryjl3-tyaaa-aaaaa-aaaba-cai)
Oisy returns: "Unsupported Canister Call: The function provided is not supported: icrc2_allowance"
```

To prevent this confusing error, we added a preemptive check in `walletOperations.ts`:

```typescript
function supportsIcrc2CanisterCalls(): boolean {
  return !isOisyWallet();  // Returns false for Oisy
}
```

This check was applied to **all** ICRC-2 operations, including icUSD operations that were never actually tested.

### The Key Question

**Does Oisy actually support ICRC-2 calls on the icUSD ledger?**

The original failure was on the ICP ledger. The icUSD ledger is a different canister with potentially different ICRC-21 consent message configuration. We never tested whether icUSD ICRC-2 calls work with Oisy.

---

## Technical Analysis

### Evidence That Oisy Might Support ICRC-2

From Oisy's documentation (shared by external reviewer):

> "Oisy handles `icrc2_approve` and `icrc2_transfer_from`, including ICRC-21 consent messages and multisig functionality 'in line with ICRC-2 standards'"

This suggests:
1. Oisy has ICRC-2 support at the protocol level
2. The failure might be specific to how the ICP ledger handles consent messages
3. The icUSD ledger might work differently

### The Two Ledgers

| Ledger | Canister ID | Used For | ICRC-2 Status with Oisy |
|--------|-------------|----------|-------------------------|
| ICP Ledger | `ryjl3-tyaaa-aaaaa-aaaba-cai` | Vault creation (collateral deposit) | **KNOWN FAIL** - "Unsupported Canister Call" |
| icUSD Ledger | (from CONFIG.currentIcusdLedgerId) | Repayment, borrowing | **UNKNOWN** - Never tested |

### Why They Might Behave Differently

1. **ICRC-21 Consent Messages**: The icUSD ledger may have different consent message configuration than the ICP ledger
2. **Canister Implementation**: Different canisters can implement ICRC-2 differently
3. **Oisy's Signer Logic**: Oisy's signer may have specific handling for certain canisters

### Current Code Flow for Repayment

```
User clicks "Repay"
  → apiClient.repayToVault()
    → walletOperations.supportsVaultOperations()  ← BLOCKS HERE FOR OISY
    → (never reached) walletOperations.checkIcusdAllowance()
    → (never reached) walletOperations.approveIcusdTransfer()
    → (never reached) backend.repay_to_vault()
```

---

## What We Implemented

### Test Branch: `test/oisy-icrc2-repayment`

**Commit:** `91a2347`  
**GitHub:** https://github.com/RumiLabsXYZ/rumi-protocol-v2/tree/test/oisy-icrc2-repayment

### Changes Made

#### 1. `walletOperations.ts` - icUSD Functions Modified

**`checkIcusdAllowance()` (line ~232)**

Before:
```typescript
static async checkIcusdAllowance(spenderCanisterId: string): Promise<bigint> {
  if (!supportsIcrc2CanisterCalls()) {
    return BigInt(0);  // Silent early return for Oisy
  }
  // ... actual ICRC-2 call
}
```

After (test branch):
```typescript
static async checkIcusdAllowance(spenderCanisterId: string): Promise<bigint> {
  // TEST: Enhanced logging for Oisy ICRC-2 test
  const walletType = isOisyWallet() ? 'Oisy' : 'other';
  console.log(`[TEST-ICRC2] checkIcusdAllowance:`, {
    walletType,
    icusdLedgerCanisterId: CONFIG.currentIcusdLedgerId,
    spenderCanisterId,
    method: 'icrc2_allowance'
  });
  
  // ... actual ICRC-2 call (no early return)
  
  // On success:
  console.log(`[TEST-ICRC2] icrc2_allowance SUCCESS:`, { allowance: result.allowance.toString() });
  
  // On failure:
  console.error(`[TEST-ICRC2] icrc2_allowance FAILED:`, {
    walletType,
    icusdLedgerCanisterId: CONFIG.currentIcusdLedgerId,
    spenderCanisterId,
    method: 'icrc2_allowance',
    errorMessage: err?.message || String(err),
    errorName: err?.name,
    errorStack: err?.stack,
    fullError: err
  });
}
```

**`approveIcusdTransfer()` (line ~310)**

Same pattern - removed early return, added detailed logging for both success and failure cases.

#### 2. `apiClient.ts` - repayToVault Modified

**`repayToVault()` (line ~871)**

Before:
```typescript
static async repayToVault(vaultId: number, icusdAmount: number): Promise<VaultOperationResult> {
  // ... 
  if (!walletOperations.supportsVaultOperations()) {
    return { success: false, error: "Oisy wallet does not support..." };
  }
  // ...
}
```

After (test branch):
```typescript
static async repayToVault(vaultId: number, icusdAmount: number): Promise<VaultOperationResult> {
  // ...
  // TEST: Intentionally bypassing Oisy check to test if ICRC-2 works on icUSD ledger
  console.log('[TEST-ICRC2] repayToVault: Oisy wallet check BYPASSED - intentionally attempting ICRC-2 flow');
  console.log('[TEST-ICRC2] repayToVault: If this fails, capture the exact error from walletOperations logs above');
  // ...
}
```

#### 3. What Was NOT Changed

The following still block Oisy (ICP ledger operations):
- `checkIcpAllowance()` - Still has `if (!supportsIcrc2CanisterCalls())` check
- `approveIcpTransfer()` - Still has `if (!supportsIcrc2CanisterCalls())` check
- `openVault()` in apiClient.ts - Still has `supportsVaultOperations()` check

This is intentional - we know ICP ledger fails with Oisy, so we're only testing icUSD.

---

## Test Protocol

### Prerequisites

1. Have an Oisy wallet with some icUSD balance
2. Have an existing vault with outstanding debt (icUSD borrowed)
3. Access to browser developer console

### Deployment

```bash
# Switch to test branch
cd /Users/robertripley/coding/rumi-protocol-v2
git checkout test/oisy-icrc2-repayment

# Deploy frontend
dfx deploy vault_frontend --network ic
```

### Test Steps

1. Open the deployed frontend (https://tcfua-yaaaa-aaaap-qrd7q-cai.icp0.io or rumiprotocol.io)
2. Open browser developer console (F12 → Console tab)
3. Connect with Oisy wallet
4. Navigate to an existing vault with debt
5. Attempt to repay some icUSD
6. **Capture all console output** - especially lines starting with `[TEST-ICRC2]`

### What to Look For

#### Success Case
```
[TEST-ICRC2] repayToVault: Oisy wallet check BYPASSED - intentionally attempting ICRC-2 flow
[TEST-ICRC2] checkIcusdAllowance: {walletType: "Oisy", icusdLedgerCanisterId: "xxx", spenderCanisterId: "yyy", method: "icrc2_allowance"}
[TEST-ICRC2] icrc2_allowance SUCCESS: {allowance: "0"}
[TEST-ICRC2] approveIcusdTransfer: {walletType: "Oisy", icusdLedgerCanisterId: "xxx", spenderCanisterId: "yyy", method: "icrc2_approve", amount: "100000000"}
[TEST-ICRC2] icrc2_approve SUCCESS: {blockIndex: "12345"}
```

#### Failure Case
```
[TEST-ICRC2] repayToVault: Oisy wallet check BYPASSED - intentionally attempting ICRC-2 flow
[TEST-ICRC2] checkIcusdAllowance: {walletType: "Oisy", ...}
[TEST-ICRC2] icrc2_allowance FAILED: {
  walletType: "Oisy",
  icusdLedgerCanisterId: "xxx",
  spenderCanisterId: "yyy", 
  method: "icrc2_allowance",
  errorMessage: "Unsupported Canister Call: ...",  ← THE KEY INFO
  errorName: "Error",
  errorStack: "...",
  fullError: {...}
}
```

---

## Expected Outcomes & Next Steps

### Outcome 1: Repayment Works ✅

**What it means:** Oisy supports ICRC-2 on the icUSD ledger. The original failure was ICP-ledger-specific.

**Action:** 
1. Remove Oisy blocks from icUSD operations permanently
2. Keep Oisy blocks for ICP operations (vault creation still needs push-deposit)
3. Update error messages to be operation-specific

### Outcome 2: "Unsupported Canister Call" Error ❌

**What it means:** Oisy's signer doesn't support `icrc2_allowance`/`icrc2_approve` regardless of ledger.

**Action:**
1. Implement push-style repayment for Oisy (similar to how vault creation now works)
2. User transfers icUSD directly to protocol, then calls repay
3. Reference: `/src/rumi_protocol_backend/src/vault.rs` lines 196-340 for push-deposit pattern

### Outcome 3: ICRC-21 Consent Message Error ❌

**What it means:** The icUSD ledger needs ICRC-21 consent message configuration for Oisy to work.

**Action:**
1. Investigate ICRC-21 setup on icUSD ledger canister
2. May need to deploy updated icUSD ledger with proper consent messages
3. Reference Oisy documentation for required ICRC-21 format

### Outcome 4: Different/Unexpected Error ❌

**What it means:** Unknown issue - need to investigate based on specific error.

**Action:**
1. Document the exact error
2. Research the error in context of Oisy/ICP/ICRC standards
3. May need to reach out to Oisy team for guidance

---

## File Reference

### Frontend Files

| File | Path | Purpose |
|------|------|---------|
| walletOperations.ts | `/src/vault_frontend/src/lib/services/protocol/walletOperations.ts` | ICRC-2 operations, Oisy detection |
| apiClient.ts | `/src/vault_frontend/src/lib/services/protocol/apiClient.ts` | High-level vault operations |
| auth.ts | `/src/vault_frontend/src/lib/services/auth.ts` | Wallet connection, Oisy integration |
| config.ts | `/src/vault_frontend/src/lib/config.ts` | Canister IDs, ledger configuration |

### Backend Files

| File | Path | Purpose |
|------|------|---------|
| vault.rs | `/src/rumi_protocol_backend/src/vault.rs` | Vault operations, push-deposit pattern (lines 196-340) |
| lib.rs | `/src/rumi_protocol_backend/src/lib.rs` | Main backend entry points |

### Key Functions

| Function | File | Line | Purpose |
|----------|------|------|---------|
| `isOisyWallet()` | walletOperations.ts | ~45 | Detects if current wallet is Oisy |
| `supportsIcrc2CanisterCalls()` | walletOperations.ts | ~55 | Returns false for Oisy |
| `supportsVaultOperations()` | walletOperations.ts | ~65 | Public API for Oisy check |
| `checkIcusdAllowance()` | walletOperations.ts | ~232 | ICRC-2 allowance check on icUSD |
| `approveIcusdTransfer()` | walletOperations.ts | ~310 | ICRC-2 approve on icUSD |
| `checkIcpAllowance()` | walletOperations.ts | ~157 | ICRC-2 allowance check on ICP |
| `approveIcpTransfer()` | walletOperations.ts | ~100 | ICRC-2 approve on ICP |
| `repayToVault()` | apiClient.ts | ~871 | High-level repay operation |
| `open_vault_with_deposit()` | vault.rs | ~196 | Push-deposit pattern reference |

### Canister IDs

| Canister | ID | Network |
|----------|-----|---------|
| Backend | `tfesu-vyaaa-aaaap-qrd7a-cai` | Mainnet |
| Frontend | `tcfua-yaaaa-aaaap-qrd7q-cai` | Mainnet |
| ICP Ledger | `ryjl3-tyaaa-aaaaa-aaaba-cai` | Mainnet |
| icUSD Ledger | (see CONFIG.currentIcusdLedgerId) | Mainnet |

---

## Historical Context

### Previous Documentation

- `/docs/OISY_IMPLEMENTATION_COMPLETE.md` - Original Oisy integration and ICP ledger failure
- `/docs/VAULT_CLOSE_NAVIGATION_BUG.md` - Unrelated UI bug (vault close navigation)
- `/docs/PLUG_WALLET_RECONNECT.md` - Plug wallet session persistence issue

### Timeline

1. **Initial Oisy Integration**: Added Oisy as third wallet option alongside Plug and Internet Identity
2. **Vault Creation Failure**: Users attempting to create vaults with Oisy got "Unsupported Canister Call: icrc2_allowance"
3. **Preemptive Block Added**: Added `supportsVaultOperations()` check to prevent confusing errors
4. **Push-Deposit Implemented**: For vault creation, implemented direct ICP transfer flow for Oisy
5. **Repayment Issue Discovered**: Users reported they can't repay with Oisy (blocked by same check)
6. **Current Investigation**: Testing whether icUSD ICRC-2 actually works with Oisy

### Key Insight from External Review

ChatGPT analysis challenged our assumption that "Oisy doesn't support ICRC-2":

> "The original failure was `icrc2_allowance` on the **ICP ledger** during vault creation. Repayment uses ICRC-2 calls on the **icUSD ledger** (different canister), which may have different consent message setup. The preemptive block may be overly conservative."

This led to the current test-first approach rather than immediately implementing push-repayment.

---

## Appendix: Console Log Patterns

### Identifying Log Source

All test logs are prefixed with `[TEST-ICRC2]` for easy filtering:

```javascript
// In browser console, filter with:
[TEST-ICRC2]
```

### Full Success Flow Logs

```
Repaying 1.5 icUSD to vault #42
[TEST-ICRC2] repayToVault: Oisy wallet check BYPASSED - intentionally attempting ICRC-2 flow
[TEST-ICRC2] repayToVault: If this fails, capture the exact error from walletOperations logs above
[TEST-ICRC2] checkIcusdAllowance: {walletType: "Oisy", icusdLedgerCanisterId: "xxx-cai", spenderCanisterId: "tfesu-vyaaa-aaaap-qrd7a-cai", method: "icrc2_allowance"}
[TEST-ICRC2] icrc2_allowance SUCCESS: {allowance: "0"}
[TEST-ICRC2] approveIcusdTransfer: {walletType: "Oisy", icusdLedgerCanisterId: "xxx-cai", spenderCanisterId: "tfesu-vyaaa-aaaap-qrd7a-cai", method: "icrc2_approve", amount: "150000000"}
Approving 150000000 e8s icUSD for tfesu-vyaaa-aaaap-qrd7a-cai
[TEST-ICRC2] icrc2_approve SUCCESS: {blockIndex: "12345"}
```

### Full Failure Flow Logs

```
Repaying 1.5 icUSD to vault #42
[TEST-ICRC2] repayToVault: Oisy wallet check BYPASSED - intentionally attempting ICRC-2 flow
[TEST-ICRC2] repayToVault: If this fails, capture the exact error from walletOperations logs above
[TEST-ICRC2] checkIcusdAllowance: {walletType: "Oisy", icusdLedgerCanisterId: "xxx-cai", spenderCanisterId: "tfesu-vyaaa-aaaap-qrd7a-cai", method: "icrc2_allowance"}
[TEST-ICRC2] icrc2_allowance FAILED (attempt 1/3): {
  walletType: "Oisy",
  icusdLedgerCanisterId: "xxx-cai",
  spenderCanisterId: "tfesu-vyaaa-aaaap-qrd7a-cai",
  method: "icrc2_allowance",
  errorMessage: "Unsupported Canister Call: The function provided is not supported: icrc2_allowance",
  errorName: "Error",
  errorStack: "Error: Unsupported Canister Call...\n    at ...",
  fullError: Error object
}
```

---

## Contact & Resources

- **GitHub Repo:** https://github.com/RumiLabsXYZ/rumi-protocol-v2
- **Test Branch:** https://github.com/RumiLabsXYZ/rumi-protocol-v2/tree/test/oisy-icrc2-repayment
- **Live Site:** https://rumiprotocol.io or https://tcfua-yaaaa-aaaap-qrd7q-cai.icp0.io
- **Oisy Wallet:** https://oisy.com
- **ICP Developer Docs:** https://internetcomputer.org/docs
