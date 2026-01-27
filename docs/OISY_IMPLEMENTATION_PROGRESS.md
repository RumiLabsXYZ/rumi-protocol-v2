# Oisy Wallet Push-Deposit Implementation - Progress Report

## Document Purpose

This document provides a detailed account of the implementation work completed to enable Oisy wallet support for Rumi Protocol vault creation. It is intended as a reference for continuing development in future chat sessions.

## Starting Point

Refer to `OISY_ICP_DEPOSIT_IMPLEMENTATION_PLAN.md` for the original 6-phase plan. This document describes what was implemented and the current state.

---

## Problem Recap

Oisy wallet uses the ICRC-25/27 signer protocol, which cannot execute arbitrary canister calls like `icrc2_allowance` and `icrc2_approve`. Our existing vault creation flow required:

1. Check user's ICP allowance (`icrc2_allowance`)
2. Request approval if needed (`icrc2_approve`)
3. Backend pulls ICP via `icrc1_transfer_from`

This fails for Oisy because steps 1 and 2 are not supported by Oisy's signer interface.

**Solution**: Implement a "push-deposit" flow where:
1. User transfers ICP directly to a unique deposit address
2. Backend queries the balance and credits the deposit
3. Vault is created using the credited amount

---

## Implementation Completed

### Phase 1: Backend State Management (COMPLETED)

**File**: `src/rumi_protocol_backend/src/state.rs`

Added state field to track credited deposits per user:
```rust
pub credited_icp_e8s: BTreeMap<Principal, u64>,
```

This prevents double-crediting the same deposit across multiple vault creation attempts.

### Phase 2: Backend Methods (COMPLETED)

**File**: `src/rumi_protocol_backend/src/management.rs`

#### Deposit Account Generation
```rust
pub fn compute_deposit_subaccount(caller: Principal) -> [u8; 32]
```
- Uses SHA-256 hash of `b"rumi-deposit" || caller.as_slice()`
- Creates a unique, deterministic subaccount for each user

#### Query Method for Deposit Address
```rust
#[query]
pub fn get_icp_deposit_account() -> Account
```
- Returns the caller's unique deposit account (backend canister + derived subaccount)
- Exposed in Candid for frontend to call

#### Vault Creation with Push Deposit
```rust
#[update]
pub async fn open_vault_with_deposit(borrow_amount: u64) -> Result<Vault, ProtocolError>
```
- Queries ICP balance at caller's deposit address
- Subtracts previously credited amount to get new deposit
- Validates minimum collateral requirement
- Updates credited amount to prevent double-crediting
- Creates vault using existing internal logic

**Candid Updates**: `src/rumi_protocol_backend/rumi_protocol_backend.did`
- Added `get_icp_deposit_account` query
- Added `open_vault_with_deposit` update method

### Phase 3: Frontend Routing Logic (COMPLETED)

**File**: `src/vault_frontend/src/lib/services/protocol/apiClient.ts`

#### Wallet Type Detection Fix
**Critical Bug Found**: The code was checking `localStorage.getItem('walletType')` but the app stores the wallet type under `'rumi_last_wallet'`.

**Line 389** - Changed from:
```typescript
const walletType = localStorage.getItem('walletType');
```
To:
```typescript
const walletType = localStorage.getItem('rumi_last_wallet');
```

This fix ensures the push-deposit flow is correctly triggered for Oisy users.

#### Push-Deposit Flow Implementation
**Lines 534-580**: New method `openVaultWithPushDeposit()`

```typescript
static async openVaultWithPushDeposit(icpAmountE8s: bigint): Promise<{...}> {
    // 1. Get deposit address from backend
    const depositAccount = await actor.get_icp_deposit_account();
    
    // 2. Transfer ICP to deposit address via icrc1_transfer
    const icpLedgerActor = await ApiClient.getIcpLedgerActor();
    const transferResult = await icpLedgerActor.icrc1_transfer({
        to: depositAccount,
        amount: icpAmountE8s,
        // ... transfer args
    });
    
    // 3. Call backend to create vault with deposited ICP
    const vaultResult = await actor.open_vault_with_deposit(0n);
    
    return { success: true, vaultId: vaultResult.Ok.vault_id };
}
```

#### Routing Branch in `openVault()`
**Lines 389-393**: Added conditional to route Oisy to push-deposit flow:
```typescript
const walletType = localStorage.getItem('rumi_last_wallet');
if (walletType === 'oisy') {
    console.log('[openVault] Oisy detected, using push deposit flow');
    return ApiClient.openVaultWithPushDeposit(amountE8s);
}
```

### Phase 4: ICP Ledger Actor (COMPLETED)

**File**: `src/vault_frontend/src/lib/services/protocol/apiClient.ts`

**Lines 583-622**: New method `getIcpLedgerActor()`

Initial implementation created an anonymous agent, which failed because it couldn't sign transactions. 

**Fix Applied**: Changed to use `walletStore.getActor()` which respects the current wallet connection:
```typescript
private static async getIcpLedgerActor(): Promise<any> {
    const ICP_LEDGER_ID = 'ryjl3-tyaaa-aaaaa-aaaba-cai';
    const icpLedgerIdl = ({ IDL }: any) => {
        // ... IDL definition for icrc1_transfer
    };
    return walletStore.getActor(ICP_LEDGER_ID, icpLedgerIdl);
}
```

### Phase 5: PNP Wallet Routing Fix (COMPLETED)

**File**: `src/vault_frontend/src/lib/services/pnp.ts`

**Critical Bug Found**: The `getActor()` method was checking if Plug wallet was installed/connected and using it directly, regardless of which wallet the user actually connected with.

**Lines 228-245** - Original problematic code:
```typescript
async getActor(canisterId: string, idl: any) {
    // This always used Plug if it was installed, even for Oisy users!
    if (window.ic?.plug && await window.ic.plug.isConnected()) {
        return await window.ic.plug.createActor({...});
    }
    return globalPnp?.getActor({ canisterId, idl });
}
```

**Fix Applied**: Check the actual wallet type from localStorage:
```typescript
async getActor(canisterId: string, idl: any) {
    const currentWalletType = localStorage.getItem('rumi_last_wallet');
    
    // Only use Plug direct API if Plug is the selected wallet
    if (currentWalletType === 'plug' && window.ic?.plug && ...) {
        return await window.ic.plug.createActor({...});
    }
    
    // For Oisy and others, use PNP
    return globalPnp?.getActor({ canisterId, idl });
}
```

### Phase 6: Declaration Regeneration (COMPLETED)

After adding new Candid methods, the frontend TypeScript declarations needed regeneration:
```bash
dfx generate rumi_protocol_backend
dfx deploy vault_frontend --ic
```

This ensured the frontend TypeScript types included `get_icp_deposit_account` and `open_vault_with_deposit`.

---

## Current State (As of January 26, 2026)

### What Works ✅

1. **Oisy wallet detection** - Correctly identifies when user is connected with Oisy
2. **Push-deposit routing** - Oisy users are routed to `openVaultWithPushDeposit()` instead of the ICRC-2 flow
3. **Deposit address generation** - Backend generates unique deposit addresses per user
4. **Vault creation** - Vaults can be created with Oisy wallet

### What Does NOT Work ❌

1. **icUSD minting after vault creation** - Error: "Signer window could not be opened"
   - The vault is created successfully
   - But subsequent operations (like borrowing icUSD) fail
   - This is likely because other operations still try to use ICRC-2 approve flow

### Console Evidence

The screenshot shows:
- `[openVaultWithPushDeposit] Starting push deposit flow for 10000000n e8s` ✅
- Vault creation proceeds
- Later error: `Error refreshing vault data: Ws: Signer window could not be opened` ❌
- UI shows: "Oisy wallet does not currently support vault operations..."

---

## Root Cause of Remaining Issue

The `borrowFromVault()` function (and possibly others like `repayToVault()`, `addMarginToVault()`) still require ICRC-2 `approve` calls:

1. When minting icUSD, the backend may need to approve icUSD transfers
2. The current flow calls `approveIcusdTransfer()` which fails for Oisy
3. The "Signer window could not be opened" error suggests PNP is trying to open an Oisy signer popup but failing

---

## Files Modified

| File | Changes |
|------|---------|
| `src/rumi_protocol_backend/src/state.rs` | Added `credited_icp_e8s` BTreeMap |
| `src/rumi_protocol_backend/src/management.rs` | Added `compute_deposit_subaccount()`, `icp_balance_of()`, `get_icp_deposit_account()`, `open_vault_with_deposit()` |
| `src/rumi_protocol_backend/rumi_protocol_backend.did` | Added Candid definitions for new methods |
| `src/vault_frontend/src/lib/services/protocol/apiClient.ts` | Fixed localStorage key, added `openVaultWithPushDeposit()`, fixed `getIcpLedgerActor()` |
| `src/vault_frontend/src/lib/services/pnp.ts` | Fixed wallet type detection in `getActor()` |

---

## Next Steps

To complete Oisy support, the following work remains:

### 1. Investigate icUSD Minting Flow

The `borrowFromVault()` function needs to either:
- Use a push-deposit pattern for icUSD (if icUSD approval is needed)
- Or skip approval if the backend can mint directly to the user

### 2. Review All Vault Operations

These methods in `apiClient.ts` need Oisy-compatible implementations:
- `borrowFromVault()` - Mint additional icUSD
- `repayToVault()` - Repay icUSD debt
- `addMarginToVault()` - Add more ICP collateral
- `withdrawCollateral()` - Remove ICP collateral
- `withdrawCollateralAndCloseVault()` - Close vault entirely

### 3. Consider Unified Flow

Instead of having separate flows for Oisy vs Plug/II, consider:
- Using push-deposit for ALL wallets (simpler code, consistent UX)
- This would eliminate the need for ICRC-2 entirely

### 4. Debug Signer Window Issue

The "Signer window could not be opened" error needs investigation:
- Is PNP configured correctly for Oisy?
- Are the ICRC-28 trusted origins properly set?
- Is there a Safari/browser-specific issue?

---

## Deployment Commands

```bash
# Navigate to project
cd /Users/robertripley/coding/rumi-protocol-v2

# Build and deploy backend
dfx deploy rumi_protocol_backend --ic

# Regenerate frontend types
dfx generate rumi_protocol_backend

# Deploy frontend
dfx deploy vault_frontend --ic
```

---

## Key Learnings

1. **localStorage key mismatch** - Always verify the exact key names used across the codebase
2. **PNP wallet detection** - The PNP library doesn't track which wallet is "active" - you must track this yourself
3. **Actor creation matters** - Using an anonymous agent vs authenticated agent determines whether transactions can be signed
4. **Declaration regeneration** - After Candid changes, always run `dfx generate` before frontend deployment

---

## References

- Original plan: `docs/OISY_ICP_DEPOSIT_IMPLEMENTATION_PLAN.md`
- ICP Wallet Integration Guide: `/mnt/project/icp-wallet-integration-guide.md`
- Oisy debugging notes: Google Doc linked in project context

---

*Last updated: January 26, 2026*
