# Oisy Wallet Integration - Complete Implementation Reference

**Last Updated:** January 26, 2026  
**Status:** Vault creation ✅ | icUSD minting ✅ | Withdraw/Close ✅ | Add margin ❌ | Repay ❌  
**Repository:** `/Users/robertripley/coding/rumi-protocol-v2`  
**GitHub:** https://github.com/RumiLabsXYZ/rumi-protocol-v2

---

## Table of Contents
1. [Problem Statement](#problem-statement)
2. [Solution Architecture](#solution-architecture)
3. [Deployed Canister IDs](#deployed-canister-ids)
4. [Backend Implementation](#backend-implementation)
5. [Frontend Implementation](#frontend-implementation)
6. [Current Blocking Issue](#current-blocking-issue)
7. [Next Steps](#next-steps)
8. [Testing Instructions](#testing-instructions)

---

## Problem Statement

**Root Cause:** Oisy wallet uses ICRC-25/27 signer standards which CANNOT execute arbitrary canister calls like `icrc2_allowance` and `icrc2_approve` on the ICP ledger.

**Console Error (before fix):**
```
Unsupported Canister Call: The function provided is not supported: icrc2_allowance
Failed to check ICP allowance…
Requesting approval for 0.1 ICP
Approving … e8s ICP for tfesu-vyaaa-aaaap-qrd7a-cai
```

**Why Other Wallets Work:**
- **Plug:** Provides full agent access via `window.ic.plug.createActor()`
- **Internet Identity:** Provides delegated identity with full signing capability
- **Oisy:** Only supports ICRC-1 transfers and specific signer operations

---

## Solution Architecture

**Push-Deposit Flow (bypasses ICRC-2):**

```
┌─────────────────────────────────────────────────────────────────┐
│ TRADITIONAL FLOW (Plug/II) - Uses ICRC-2 approve/transfer_from │
├─────────────────────────────────────────────────────────────────┤
│ 1. Frontend calls icrc2_approve() on ICP ledger                 │
│ 2. Backend calls icrc2_transfer_from() to pull ICP              │
│ 3. Backend creates vault with pulled ICP                        │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│ PUSH-DEPOSIT FLOW (Oisy) - Only uses ICRC-1 transfer           │
├─────────────────────────────────────────────────────────────────┤
│ 1. Frontend calls get_icp_deposit_account() → unique subaccount │
│ 2. Frontend calls icrc1_transfer() to deposit address           │
│ 3. Frontend calls open_vault_with_deposit()                     │
│ 4. Backend queries deposit balance & credits to vault           │
└─────────────────────────────────────────────────────────────────┘
```

---

## Deployed Canister IDs

| Canister | ID | URL |
|----------|----|----|
| Backend | `tfesu-vyaaa-aaaap-qrd7a-cai` | https://a]a4gq6-oaaaa-aaaab-qaa4q-cai.raw.icp0.io/?id=tfesu-vyaaa-aaaap-qrd7a-cai |
| Frontend | `tcfua-yaaaa-aaaap-qrd7q-cai` | https://tcfua-yaaaa-aaaap-qrd7q-cai.icp0.io |
| ICP Ledger | `ryjl3-tyaaa-aaaaa-aaaba-cai` | (system canister) |
| icUSD Ledger | (check dfx.json) | |

**Custom Domain:** https://rumiprotocol.io

---

## Backend Implementation

### File: `src/rumi_protocol_backend/src/state.rs`

**Line ~157-159** - Added `credited_icp_e8s` field to State struct:

```rust
    /// Tracks total ICP (in e8s) already credited from deposit subaccounts per user.
    /// Used to prevent double-crediting the same deposit.
    pub credited_icp_e8s: BTreeMap<Principal, u64>,
```

**Line ~186** - Initialize in State::from():

```rust
            credited_icp_e8s: BTreeMap::new(),
```

---

### File: `src/rumi_protocol_backend/src/management.rs`

**Lines 268-276** - Deposit subaccount derivation:

```rust
/// Derives a deposit subaccount for a given caller principal.
/// Used for the push-deposit flow where users transfer ICP directly
/// to a per-user subaccount owned by this canister.
pub fn compute_deposit_subaccount(caller: Principal) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"rumi-deposit");
    hasher.update(caller.as_slice());
    hasher.finalize().into()
}
```

**Lines 278-298** - Balance query helper:

```rust
/// Queries the ICP ledger for the balance of a given account.
/// Returns the balance in e8s, or a ProtocolError if the call fails
/// or the balance exceeds u64::MAX.
pub async fn icp_balance_of(account: Account) -> Result<u64, ProtocolError> {
    let client = ICRC1Client {
        runtime: CdkRuntime,
        ledger_canister_id: read_state(|s| s.icp_ledger_principal),
    };
    let balance: Nat = client
        .balance_of(account)
        .await
        .map_err(|(code, msg)| {
            ProtocolError::GenericError(format!(
                "Failed to query ICP balance: code={:?}, msg={}",
                code, msg
            ))
        })?;
    
    balance.0.to_u64().ok_or_else(|| {
        ProtocolError::GenericError("ICP balance exceeds u64::MAX".to_string())
    })
}
```

---

### File: `src/rumi_protocol_backend/src/vault.rs`

**Lines 198-280** - Complete `open_vault_with_deposit` implementation:

```rust
/// Opens a vault using ICP that was pre-deposited to the user's deposit subaccount.
/// This bypasses ICRC-2 approve/transfer_from, enabling Oisy wallet support.
pub async fn open_vault_with_deposit(borrow_amount: u64) -> Result<OpenVaultSuccess, ProtocolError> {
    use crate::management::{compute_deposit_subaccount, icp_balance_of};
    use icrc_ledger_types::icrc1::account::Account;
    
    let caller = ic_cdk::api::caller();
    let guard_principal = match GuardPrincipal::new(caller, "open_vault_with_deposit") {
        Ok(guard) => guard,
        Err(GuardError::AlreadyProcessing) => {
            log!(INFO, "[open_vault_with_deposit] Principal {:?} already has an ongoing operation", caller);
            return Err(ProtocolError::AlreadyProcessing);
        },
        Err(GuardError::StaleOperation) => {
            log!(INFO, "[open_vault_with_deposit] Principal {:?} has a stale operation being cleaned up", caller);
            return Err(ProtocolError::TemporarilyUnavailable(
                "Previous operation is being cleaned up. Please try again.".to_string()
            ));
        },
        Err(err) => return Err(err.into()),
    };

    // 1. Compute deposit account
    let subaccount = compute_deposit_subaccount(caller);
    let deposit_account = Account {
        owner: ic_cdk::id(),
        subaccount: Some(subaccount),
    };

    // 2. Query current balance at deposit address
    let current_balance = match icp_balance_of(deposit_account).await {
        Ok(bal) => bal,
        Err(e) => {
            guard_principal.fail();
            return Err(e);
        }
    };

    // 3. Get previously credited amount
    let previously_credited = read_state(|s| {
        s.credited_icp_e8s.get(&caller).copied().unwrap_or(0)
    });

    // 4. Calculate new deposit
    let new_deposit = current_balance.saturating_sub(previously_credited);
    let icp_margin_amount: ICP = new_deposit.into();

    if icp_margin_amount < MIN_ICP_AMOUNT {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: MIN_ICP_AMOUNT.to_u64(),
        });
    }

    // 5. Update credited amount to prevent double-crediting
    mutate_state(|s| {
        s.credited_icp_e8s.insert(caller, current_balance);
    });

    // 6. Create the vault (no transfer needed - ICP already deposited)
    let vault_id = mutate_state(|s| {
        let vault_id = s.increment_vault_id();
        record_open_vault(
            s,
            Vault {
                owner: caller,
                borrowed_icusd_amount: 0.into(),
                icp_margin_amount,
                vault_id,
            },
            0, // block_index = 0 since we're not doing a transfer here
        );
        vault_id
    });

    log!(INFO, "[open_vault_with_deposit] opened vault {} with {} e8s from deposit", vault_id, new_deposit);
    guard_principal.complete();

    Ok(OpenVaultSuccess {
        vault_id,
        block_index: 0,
    })
}
```

**Lines 281-293** - `get_icp_deposit_account` implementation:

```rust
/// Returns the deposit account for the caller to send ICP to.
pub fn get_icp_deposit_account() -> icrc_ledger_types::icrc1::account::Account {
    use crate::management::compute_deposit_subaccount;
    use icrc_ledger_types::icrc1::account::Account;
    
    let caller = ic_cdk::api::caller();
    let subaccount = compute_deposit_subaccount(caller);
    
    Account {
        owner: ic_cdk::id(),
        subaccount: Some(subaccount),
    }
}
```

---

### File: `src/rumi_protocol_backend/src/main.rs`

**Lines 303-311** - Candid exports:

```rust
#[update]
async fn open_vault_with_deposit(borrow_amount: u64) -> Result<OpenVaultSuccess, ProtocolError> {
    check_anonymous_caller()?;
    check_postcondition(rumi_protocol_backend::vault::open_vault_with_deposit(borrow_amount).await)
}

#[query]
fn get_icp_deposit_account() -> icrc_ledger_types::icrc1::account::Account {
    rumi_protocol_backend::vault::get_icp_deposit_account()
}
```

---

### File: `src/rumi_protocol_backend/rumi_protocol_backend.did`

**Candid interface additions:**

```candid
service : (ProtocolArg) -> {
  // ... existing methods ...
  
  // NEW: Push-deposit methods for Oisy
  open_vault_with_deposit : (nat64) -> (variant { Ok : OpenVaultSuccess; Err : ProtocolError });
  get_icp_deposit_account : () -> (Account) query;
  
  // ... rest of methods ...
}
```

---

## Frontend Implementation

### File: `src/vault_frontend/src/lib/services/protocol/apiClient.ts`

**Lines 388-396** - Oisy detection in `openVault()`:

```typescript
static async openVault(icpAmount: number): Promise<VaultOperationResult> {
    // ... setup code ...
    
    // Check if using Oisy - use push-deposit flow instead of ICRC-2
    const walletType = localStorage.getItem('rumi_last_wallet');
    if (walletType === 'oisy') {
        const amountE8s = BigInt(Math.floor(icpAmount * E8S));
        return ApiClient.openVaultWithPushDeposit(amountE8s);
    }
    
    // ... rest of ICRC-2 flow for Plug/II ...
}
```

**Lines 520-570** - Complete `openVaultWithPushDeposit()`:

```typescript
/**
 * Opens a vault using the push-deposit flow (for Oisy wallet).
 * User transfers ICP to their deposit address, then we create the vault.
 */
static async openVaultWithPushDeposit(icpAmountE8s: bigint): Promise<{ success: boolean; vaultId?: bigint; error?: string }> {
    try {
        console.log('[openVaultWithPushDeposit] Starting push deposit flow for', icpAmountE8s, 'e8s');

        const actor = await ApiClient.getAuthenticatedActor();

        // 1. Get deposit address from backend
        const depositAccount = await actor.get_icp_deposit_account();
        console.log('[openVaultWithPushDeposit] Deposit account:', depositAccount);

        // 2. Transfer ICP to deposit address
        const icpLedgerActor = await ApiClient.getIcpLedgerActor();
        const transferResult = await icpLedgerActor.icrc1_transfer({
            to: depositAccount,
            amount: icpAmountE8s,
            fee: [10_000n],
            memo: [],
            from_subaccount: [],
            created_at_time: [],
        });

        if ('Err' in transferResult) {
            console.error('[openVaultWithPushDeposit] Transfer failed:', transferResult.Err);
            return { success: false, error: `ICP transfer failed: ${JSON.stringify(transferResult.Err)}` };
        }

        console.log('[openVaultWithPushDeposit] Transfer successful, block:', transferResult.Ok);

        // 3. Call backend to create vault with deposited ICP
        const vaultResult = await actor.open_vault_with_deposit(0n); // borrow_amount = 0 for now

        if ('Err' in vaultResult) {
            console.error('[openVaultWithPushDeposit] Vault creation failed:', vaultResult.Err);
            return { success: false, error: `Vault creation failed: ${JSON.stringify(vaultResult.Err)}` };
        }

        console.log('[openVaultWithPushDeposit] Vault created:', vaultResult.Ok);
        return { success: true, vaultId: vaultResult.Ok.vault_id };

    } catch (error) {
        console.error('[openVaultWithPushDeposit] Error:', error);
        return { success: false, error: String(error) };
    }
}
```

**Lines 571-610** - `getIcpLedgerActor()` with inline IDL:

```typescript
/**
 * Gets an authenticated actor for the ICP ledger canister.
 * Uses the connected wallet's identity for signing transfers.
 */
private static async getIcpLedgerActor(): Promise<any> {
    const ICP_LEDGER_ID = 'ryjl3-tyaaa-aaaaa-aaaba-cai';
    
    // Minimal IDL for icrc1_transfer
    const icpLedgerIdl = ({ IDL }: any) => {
        const Account = IDL.Record({
            owner: IDL.Principal,
            subaccount: IDL.Opt(IDL.Vec(IDL.Nat8)),
        });
        const TransferArg = IDL.Record({
            to: Account,
            fee: IDL.Opt(IDL.Nat),
            memo: IDL.Opt(IDL.Vec(IDL.Nat8)),
            from_subaccount: IDL.Opt(IDL.Vec(IDL.Nat8)),
            created_at_time: IDL.Opt(IDL.Nat64),
            amount: IDL.Nat,
        });
        const TransferError = IDL.Variant({
            BadFee: IDL.Record({ expected_fee: IDL.Nat }),
            BadBurn: IDL.Record({ min_burn_amount: IDL.Nat }),
            InsufficientFunds: IDL.Record({ balance: IDL.Nat }),
            TooOld: IDL.Null,
            CreatedInFuture: IDL.Record({ ledger_time: IDL.Nat64 }),
            Duplicate: IDL.Record({ duplicate_of: IDL.Nat }),
            TemporarilyUnavailable: IDL.Null,
            GenericError: IDL.Record({ error_code: IDL.Nat, message: IDL.Text }),
        });
        const TransferResult = IDL.Variant({
            Ok: IDL.Nat,
            Err: TransferError,
        });
        return IDL.Service({
            icrc1_transfer: IDL.Func([TransferArg], [TransferResult], []),
        });
    };

    // Use wallet store to get authenticated actor (respects current wallet type)
    return walletStore.getActor(ICP_LEDGER_ID, icpLedgerIdl);
}
```

---

### File: `src/vault_frontend/src/lib/services/pnp.ts`

**Lines 228-256** - Critical `getActor()` routing fix:

```typescript
// Override getActor to use the correct wallet based on current connection
async getActor(canisterId: string, idl: any) {
    try {
        // Check which wallet type is actually connected via localStorage
        const currentWalletType = localStorage.getItem('rumi_last_wallet');
        
        // For Plug wallet, use the direct Plug API to avoid permission prompts
        // But ONLY if Plug is the currently selected wallet
        if (currentWalletType === 'plug' && window.ic?.plug && await window.ic.plug.isConnected()) {
            console.log('🔧 Using Plug direct API for actor:', canisterId);
            return await window.ic.plug.createActor({
                canisterId,
                interfaceFactory: idl
            });
        }
        
        // For other wallets (including Oisy), use PNP beta API with options object
        console.log('🔧 Using PNP beta getActor for:', canisterId, '(wallet:', currentWalletType, ')');
        return globalPnp?.getActor({ canisterId, idl });
    } catch (err) {
        console.error('Error getting actor for canister', canisterId, err);
        throw err;
    }
}
```

**CRITICAL BUG THAT WAS FIXED:**
The localStorage key was inconsistent:
- Some code used `'walletType'`
- Other code used `'rumi_last_wallet'`

**Correct key is: `'rumi_last_wallet'`**

This is set in `src/vault_frontend/src/lib/stores/wallet.ts` line ~89:
```typescript
localStorage.setItem('rumi_last_wallet', walletType);
```

---

### File: `src/declarations/rumi_protocol_backend/rumi_protocol_backend.did.js`

**Regenerated with `dfx generate`** - Contains the JavaScript IDL for the new methods:

```javascript
export const idlFactory = ({ IDL }) => {
  // ... type definitions ...
  
  return IDL.Service({
    // ... existing methods ...
    'get_icp_deposit_account' : IDL.Func([], [Account], ['query']),
    'open_vault_with_deposit' : IDL.Func(
        [IDL.Nat64],
        [IDL.Variant({ 'Ok' : OpenVaultSuccess, 'Err' : ProtocolError })],
        [],
      ),
    // ... rest of methods ...
  });
};
```

---

## Operations Status with Oisy

### ✅ Working Operations (No ICRC-2 Required)

These operations work with Oisy because the backend sends tokens TO the user (no approval needed):

| Operation | Function | Status |
|-----------|----------|--------|
| Create Vault | `openVaultWithPushDeposit()` | ✅ Uses push-deposit flow |
| Borrow icUSD | `borrowFromVault()` | ✅ Backend mints icUSD to user |
| Close Vault | `closeVault()` | ✅ Backend sends ICP to user |
| Withdraw Collateral | `withdrawCollateral()` | ✅ Backend sends ICP to user |
| Withdraw & Close | `withdrawCollateralAndCloseVault()` | ✅ Backend sends ICP to user |

### ❌ Operations Needing Push-Deposit (ICRC-2 Required)

These operations require the user to send tokens TO the backend - need push-deposit variants for Oisy:

| Operation | Function | Issue |
|-----------|----------|-------|
| Add Margin | `addMarginToVault()` | User sends ICP - needs push-deposit variant |
| Repay Debt | `repayToVault()` | User sends icUSD - needs push-deposit variant |

---

## Next Steps

### Priority 1: Fix icUSD Minting for Oisy

**Option A: Create `borrowFromVaultWithPushDeposit()`**

This would require:
1. Backend: No changes needed - `borrow_from_vault()` just mints icUSD to caller
2. Frontend: Skip the approve step for Oisy since borrowing doesn't require approval

Wait - actually borrowing icUSD from a vault should NOT require ICRC-2 approval because:
- The user is receiving icUSD, not sending it
- The backend mints icUSD to the user's principal

**The bug may be elsewhere.** Check if `borrowFromVault()` is incorrectly calling approve:

```typescript
// Check this code path - why would borrowing require approval?
static async borrowFromVault(vaultId: number, icusdAmount: number)
```

### Priority 2: Implement Push-Deposit for Other Operations

Operations that DO require the user to send ICP/icUSD:
- `addMarginToVault()` - sends ICP
- `repayToVault()` - sends icUSD

These will need push-deposit variants for Oisy.

### Priority 3: Verify ICRC-28 Trusted Origins

**File:** `src/rumi_protocol_backend/src/main.rs` (search for `icrc28_trusted_origins`)

Ensure these origins are listed:
```rust
vec![
    "https://tcfua-yaaaa-aaaap-qrd7q-cai.icp0.io".to_string(),
    "https://rumiprotocol.io".to_string(),
    "https://app.rumiprotocol.io".to_string(),
]
```

---

## Testing Instructions

### Test Vault Creation with Oisy

1. Go to https://tcfua-yaaaa-aaaap-qrd7q-cai.icp0.io
2. Click "Connect Wallet"
3. Select "Oisy"
4. Approve connection in Oisy popup
5. Enter ICP amount (e.g., 0.5 ICP)
6. Click "Create Vault"
7. Approve ICP transfer in Oisy popup
8. **Expected:** Vault appears in the list

### Verify localStorage Key

Open browser console and run:
```javascript
localStorage.getItem('rumi_last_wallet')
// Should return 'oisy' when connected with Oisy
```

### Check Console for Errors

Look for these log patterns:
```
✅ Good: [openVaultWithPushDeposit] Starting push deposit flow
✅ Good: [openVaultWithPushDeposit] Deposit account: {...}
✅ Good: [openVaultWithPushDeposit] Transfer successful
✅ Good: [openVaultWithPushDeposit] Vault created

❌ Bad: Unsupported Canister Call: icrc2_allowance
❌ Bad: Ws: Signer window could not be opened
```

---

## Deployment Commands

```bash
# Navigate to project
cd /Users/robertripley/coding/rumi-protocol-v2

# Build and deploy backend
dfx deploy rumi_protocol_backend --network ic

# Regenerate declarations
dfx generate rumi_protocol_backend

# Build and deploy frontend
dfx deploy vault_frontend --network ic

# Check canister status
dfx canister status rumi_protocol_backend --network ic
dfx canister status vault_frontend --network ic
```

---

## Key Principals

| Role | Principal |
|------|-----------|
| Rob (main controller) | `fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae` |
| Agnes | `wrppb-amng2-jzskb-wcmam-mwrmi-ci52r-bkkre-tzu35-hjfpb-dnl4p-6qe` |
| Gurleen | `bsu7v-jz2ty-tyonm-dmkdj-nir27-num7e-dtlff-4vmjj-gagxl-xiljg-lqe` |
| CycleOps | `cpbhu-5iaaa-aaaad-aalta-cai` |

---

## Summary

**What's Working:**
- ✅ Oisy wallet connection via PNP
- ✅ Push-deposit flow for vault creation
- ✅ ICP transfer to deposit subaccount
- ✅ Backend vault creation from deposit
- ✅ Vault display in UI

**What's Not Working:**
- ❌ icUSD minting (borrowFromVault) fails with signer window error
- ❌ Other vault operations (repay, add margin) not yet converted to push-deposit

**Key Files Modified:**
1. `src/rumi_protocol_backend/src/state.rs` - Added `credited_icp_e8s`
2. `src/rumi_protocol_backend/src/management.rs` - Added `compute_deposit_subaccount()`, `icp_balance_of()`
3. `src/rumi_protocol_backend/src/vault.rs` - Added `open_vault_with_deposit()`, `get_icp_deposit_account()`
4. `src/rumi_protocol_backend/src/main.rs` - Added Candid exports
5. `src/rumi_protocol_backend/rumi_protocol_backend.did` - Added interface
6. `src/vault_frontend/src/lib/services/protocol/apiClient.ts` - Added `openVaultWithPushDeposit()`, `getIcpLedgerActor()`
7. `src/vault_frontend/src/lib/services/pnp.ts` - Fixed `getActor()` routing
