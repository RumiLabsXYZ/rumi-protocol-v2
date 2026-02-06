# Oisy Wallet Query Signer Bug - Complete Implementation Guide

**Date:** January 26, 2025  
**Status:** Root cause identified, fix ready to implement  
**Priority:** High - affects all Oisy users viewing vault data

---

## Problem Summary

When connected with Oisy wallet, **read-only query operations** (like loading vault lists) incorrectly attempt to open the Oisy signer popup window, which fails with:

```
Ws: Signer window could not be opened
```

This causes vault data to not load/refresh for Oisy users, even though the wallet connects successfully.

---

## Root Cause

The PNP (Plug-N-Play) library's `getActor()` method creates actors that route **ALL** canister calls through the signer channel, including query calls that don't require signing.

**The problem flow:**
1. User connects Oisy wallet ✅
2. App calls `getUserVaults()` to load vaults
3. `getUserVaults()` calls `getAuthenticatedActor()` 
4. `getAuthenticatedActor()` calls `walletStore.getActor()` (auth.ts line 547)
5. For Oisy, this calls `pnp.getActor()` (pnp.ts line 243)
6. PNP's actor routes the query through signer channel
7. Signer window fails to open → Error

**Query calls should use an anonymous HttpAgent and never require signing.**

---

## Console Log Pattern (from debugging session)

```
✅ oisy wallet connected successfully
🔧 Using PNP beta getActor for: tfesu-vyaaa-aaaap-qrd7a-cai (wallet: oisy)
❌ Error: Ws: Signer window could not be opened
   at C6.openChannel
   at x0.call
   at x0.query  ← Query operation triggering signer!
```

---

## Affected Files & Exact Code Locations

### 1. `/src/vault_frontend/src/lib/services/auth.ts`

**Lines 524-556** - `getActor` method (THE PROBLEM):

```typescript
async getActor<T>(canisterId: string, idl: any): Promise<T> {
  const state = get(store);
  
  if (!state.isConnected) {
    throw new Error('Wallet not connected');
  }

  if (state.walletType === WALLET_TYPES.INTERNET_IDENTITY) {
    if (!agent) {
      throw new Error('Internet Identity agent not initialized');
    }
    
    // Create actor using Internet Identity agent
    const { Actor } = await import('@dfinity/agent');
    return Actor.createActor(idl, {
      agent,
      canisterId
    }) as T;
  } else if (state.walletType === WALLET_TYPES.OISY || state.walletType === WALLET_TYPES.PLUG) {
    // Use PNP for both Oisy and Plug wallets
    // THIS IS THE PROBLEM - PNP routes queries through signer
    return pnp.getActor(canisterId, idl) as unknown as T;
  } else {
    return pnp.getActor(canisterId, idl) as unknown as T;
  }
}
```

### 2. `/src/vault_frontend/src/lib/services/pnp.ts`

**Lines 228-253** - `getActor` method in pnp wrapper:

```typescript
async getActor(canisterId: string, idl: any) {
  try {
    const currentWalletType = localStorage.getItem('rumi_last_wallet');
    
    // For Plug wallet, use direct Plug API
    if (currentWalletType === 'plug' && window.ic?.plug && await window.ic.plug.isConnected()) {
      console.log('🔧 Using Plug direct API for actor:', canisterId);
      return await window.ic.plug.createActor({
        canisterId,
        interfaceFactory: idl
      });
    }
    
    // For Oisy, uses PNP beta API - THIS REQUIRES SIGNER FOR ALL CALLS
    console.log('🔧 Using PNP beta getActor for:', canisterId, '(wallet:', currentWalletType, ')');
    return globalPnp?.getActor({ canisterId, idl });
  } catch (err) {
    console.error('Error getting actor for canister', canisterId, err);
    throw err;
  }
}
```

### 3. `/src/vault_frontend/src/lib/services/protocol/apiClient.ts`

**Lines 178-190** - `getAuthenticatedActor` (calls the problematic getActor):

```typescript
private static async getAuthenticatedActor(): Promise<_SERVICE> {
  if (USE_MOCK_DATA) {
    return publicActor; // Use anonymous actor for mock data
  }
  
  try {
    return walletStore.getActor(CONFIG.currentCanisterId, rumi_backendIDL);
  } catch (err) {
    console.error('Failed to get authenticated actor:', err);
    throw new Error('Failed to initialize protocol actor');
  }
}
```

**Lines 1007-1045** - `getUserVaults` (uses getAuthenticatedActor for QUERIES):

```typescript
static async getUserVaults(forceRefresh = false): Promise<VaultDTO[]> {
  // ... cache logic ...
  
  const actor = await ApiClient.getAuthenticatedActor();  // ← Gets signing actor
  const canisterVaults = await actor.get_vaults([principalForQuery]);  // ← Query triggers signer!
  
  // ... transform results ...
}
```

**Line 251** - `getPublicData` (ALREADY uses anonymous actor correctly):

```typescript
static async getPublicData<T>(method: keyof typeof publicActor, ...args: any[]): Promise<T> {
  // Uses publicActor which is anonymous - this works fine
  return (await (publicActor[method] as any)(...args)) as T;
}
```

---

## The Solution: Add Query Actor Method

### Step 1: Add `getQueryActor` to auth.ts

Add this new method after line 556 in auth.ts:

```typescript
/**
 * Get an actor for READ-ONLY query operations.
 * Uses anonymous HttpAgent - no wallet signing required.
 * Use this for: get_vaults, get_protocol_status, get_vault, etc.
 */
async getQueryActor<T>(canisterId: string, idl: any): Promise<T> {
  const { Actor, HttpAgent } = await import('@dfinity/agent');
  
  const queryAgent = new HttpAgent({
    host: CONFIG.isLocal ? 'http://localhost:4943' : 'https://icp0.io'
  });
  
  // Fetch root key for local development
  if (CONFIG.isLocal) {
    await queryAgent.fetchRootKey();
  }
  
  return Actor.createActor(idl, {
    agent: queryAgent,
    canisterId
  }) as T;
}
```

### Step 2: Export the method from walletStore

The method is already part of the store object, so it will be accessible via `walletStore.getQueryActor()`.

### Step 3: Add `getQueryActor` to apiClient.ts

Add this after line 190:

```typescript
/**
 * Get an actor for read-only queries (no signing required)
 * This avoids the Oisy signer popup issue for query operations
 */
private static async getQueryActor(): Promise<_SERVICE> {
  if (USE_MOCK_DATA) {
    return publicActor;
  }
  
  try {
    return walletStore.getQueryActor(CONFIG.currentCanisterId, rumi_backendIDL);
  } catch (err) {
    console.error('Failed to get query actor:', err);
    throw new Error('Failed to initialize query actor');
  }
}
```

### Step 4: Update getUserVaults to use query actor

Change line ~1038 in apiClient.ts from:

```typescript
const actor = await ApiClient.getAuthenticatedActor();
```

To:

```typescript
const actor = await ApiClient.getQueryActor();
```

### Step 5: Update other query-only methods

Search for methods that call `getAuthenticatedActor()` but only perform queries:

- `getVaultById()` - uses `get_vault` query → change to `getQueryActor()`
- `verifyVaultAccess()` - reads vault data → change to `getQueryActor()` for the read part
- Any method that only calls query methods on the actor

**Keep `getAuthenticatedActor()` for:**
- `openVault()` - update call
- `borrow()` - update call  
- `repay()` - update call
- `close_vault()` - update call
- `withdraw_collateral()` - update call
- Any method that modifies state

---

## Operations Classification

### Query Operations (use `getQueryActor`):
- `get_vaults(principal)` - Load user's vault list
- `get_vault(vault_id)` - Load single vault details
- `get_protocol_status()` - Dashboard stats (already uses publicActor ✅)
- `get_stability_pool_info()` - Pool data
- Any read-only canister method

### Update Operations (keep `getAuthenticatedActor`):
- `open_vault()` ✅ working
- `borrow()` ✅ working
- `close_vault()` ✅ working
- `withdraw_collateral()` ✅ working
- `add_margin()` - needs push-deposit (separate issue)
- `repay()` - needs push-deposit (separate issue)

---

## Testing Checklist

After implementing the fix:

1. [ ] Connect with Oisy wallet
2. [ ] Navigate to /vaults - should load vault list WITHOUT popup
3. [ ] Click a vault - should load details WITHOUT popup
4. [ ] Create a new vault - should show Oisy approval popup (update operation)
5. [ ] Close a vault - should show Oisy approval popup (update operation)
6. [ ] Test with Plug wallet - should still work normally
7. [ ] Test with Internet Identity - should still work normally

---

## Key Configuration Values

```typescript
// Canister IDs
Frontend: tcfua-yaaaa-aaaap-qrd7q-cai
Backend:  tfesu-vyaaa-aaaap-qrd7a-cai
ICP Ledger: ryjl3-tyaaa-aaaaa-aaaba-cai
icUSD Ledger: (check CONFIG)

// Hosts
Mainnet: https://icp0.io
Local: http://localhost:4943

// PNP Library
Package: @windoge98/plug-n-play
```

---

## Import Statements Needed

For auth.ts `getQueryActor` method:
```typescript
// Already imported at top of file:
import { CONFIG } from '../config';
// Need to dynamically import:
const { Actor, HttpAgent } = await import('@dfinity/agent');
```

For apiClient.ts:
```typescript
// Already has walletStore imported
// No new imports needed
```

---

## Summary

The fix is straightforward:
1. Add `getQueryActor()` method that uses anonymous HttpAgent
2. Use it for read operations that don't need signing
3. Keep `getAuthenticatedActor()` for write operations

This avoids the PNP signer channel entirely for queries, fixing the Oisy popup issue while maintaining proper signing for transactions.
