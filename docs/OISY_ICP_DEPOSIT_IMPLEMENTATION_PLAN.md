# Oisy ICP Deposit Implementation Plan

## Problem Statement

Oisy wallet cannot create vaults because our current flow uses ICRC-2 `approve`/`transfer_from`, which Oisy's signer doesn't support. We need a push-based deposit flow where users transfer ICP directly to a deposit address, then the backend credits it.

## Canister IDs

- **ICP Ledger**: `ryjl3-tyaaa-aaaaa-aaaba-cai`
- **Rumi Backend**: `tfesu-vyaaa-aaaap-qrd7a-cai`

## File Paths

- **Backend**: `/Users/robertripley/coding/rumi-protocol-v2/src/rumi_protocol_backend/src/`
- **Frontend**: `/Users/robertripley/coding/rumi-protocol-v2/src/vault_frontend/src/lib/services/protocol/`

## Already Implemented (in working directory, not committed)

Three foundation pieces exist in the backend:

1. **State field** in `state.rs` (line 159):
   ```rust
   pub credited_icp_e8s: BTreeMap<Principal, u64>,
   ```
   Initialized to empty BTreeMap in `impl From<InitArg> for State` (line 193).

2. **Subaccount derivation** in `management.rs` (line 269):
   ```rust
   pub fn compute_deposit_subaccount(caller: Principal) -> [u8; 32]
   ```
   Uses SHA-256 over `b"rumi-deposit" || caller.as_slice()`.

3. **ICP balance query** in `management.rs` (line 279):
   ```rust
   pub async fn icp_balance_of(account: Account) -> Result<u64, ProtocolError>
   ```
   Queries ICP ledger, converts Nat to u64 safely.

---

## Phase 1: Backend - New Canister Method for Vault Creation

**File**: `management.rs` (and expose in `lib.rs`)

Create `open_vault_with_icp_deposit` update method:

```rust
#[update]
pub async fn open_vault_with_icp_deposit(borrow_amount: u64) -> Result<VaultId, ProtocolError> {
    let caller = ic_cdk::caller();
    
    // 1. Compute caller's deposit subaccount
    let subaccount = compute_deposit_subaccount(caller);
    
    // 2. Build the deposit account (backend canister + subaccount)
    let deposit_account = Account {
        owner: ic_cdk::id(),
        subaccount: Some(subaccount),
    };
    
    // 3. Query current ICP balance at deposit address
    let current_balance = icp_balance_of(deposit_account).await?;
    
    // 4. Get previously credited amount (default 0)
    let previously_credited = STATE.with(|s| {
        s.borrow().credited_icp_e8s.get(&caller).copied().unwrap_or(0)
    });
    
    // 5. Calculate new deposit
    let new_deposit = current_balance.saturating_sub(previously_credited);
    
    // 6. Validate minimum collateral (use existing constant)
    if new_deposit < MINIMUM_COLLATERAL_E8S {
        return Err(ProtocolError::InsufficientCollateral);
    }
    
    // 7. Update credited amount to prevent double-crediting
    STATE.with(|s| {
        s.borrow_mut().credited_icp_e8s.insert(caller, current_balance);
    });
    
    // 8. Create vault using existing internal logic
    // Convert new_deposit from e8s to ICP type, then call internal vault creation
    let collateral = ICP::from_e8s(new_deposit);
    let borrow = ICUSD::from_e8s(borrow_amount);
    
    // Call existing open_vault internal logic (adjust based on actual function signature)
    internal_open_vault(caller, collateral, borrow).await
}
```

**Note**: Adapt to actual internal vault creation function. Check how existing `open_vault` works and reuse that logic.

---

## Phase 2: Backend - Query for Deposit Address

**File**: `management.rs` (and expose in `lib.rs`)

```rust
#[query]
pub fn get_my_icp_deposit_account() -> Account {
    let caller = ic_cdk::caller();
    let subaccount = compute_deposit_subaccount(caller);
    
    Account {
        owner: ic_cdk::id(),  // Backend canister ID
        subaccount: Some(subaccount),
    }
}
```

**Candid**: Add to `.did` file:
```candid
get_my_icp_deposit_account : () -> (Account) query;
open_vault_with_icp_deposit : (nat64) -> (variant { Ok : nat64; Err : ProtocolError });
```

---

## Phase 3: Frontend - Wallet-Aware Collateral Flow

### 3a. Update `walletOperations.ts`

The Oisy blocker is in this file - look for `supportsVaultOperations()` or `isOisyWallet()` checks.

Add helper to detect if we should use push-deposit flow:

```typescript
function shouldUsePushDeposit(): boolean {
    // For ICP collateral with Oisy, use push deposit
    // Could also use for all wallets to simplify
    const walletType = get(currentWalletType) || localStorage.getItem('walletType');
    return walletType === 'oisy';
}
```

### 3b. Update `apiClient.ts`

Modify `openVault` (or equivalent) method:

```typescript
async openVault(collateralAmount: bigint, borrowAmount: bigint): Promise<Result> {
    // Check if we should use the new push-deposit flow
    if (shouldUsePushDeposit()) {
        return this.openVaultWithPushDeposit(collateralAmount, borrowAmount);
    }
    
    // Existing ICRC-2 flow for Plug/II
    // ... existing code ...
}

async openVaultWithPushDeposit(collateralAmount: bigint, borrowAmount: bigint): Promise<Result> {
    // 1. Get deposit address from backend
    const depositAccount = await this.actor.get_my_icp_deposit_account();
    
    // 2. Transfer ICP to deposit address using icrc1_transfer
    const icpLedger = await this.getIcpLedgerActor();
    const transferResult = await icpLedger.icrc1_transfer({
        to: depositAccount,
        amount: collateralAmount,
        fee: [10_000n],  // 0.0001 ICP fee
        memo: [],
        from_subaccount: [],
        created_at_time: [],
    });
    
    if ('Err' in transferResult) {
        return { success: false, error: `ICP transfer failed: ${JSON.stringify(transferResult.Err)}` };
    }
    
    // 3. Call backend to create vault with deposited ICP
    const vaultResult = await this.actor.open_vault_with_icp_deposit(borrowAmount);
    
    if ('Err' in vaultResult) {
        return { success: false, error: `Vault creation failed: ${JSON.stringify(vaultResult.Err)}` };
    }
    
    return { success: true, vaultId: vaultResult.Ok };
}
```

### 3c. ICP Ledger Actor

Ensure there's a way to get an ICP ledger actor. May need to add:

```typescript
const ICP_LEDGER_CANISTER_ID = 'ryjl3-tyaaa-aaaaa-aaaba-cai';

async getIcpLedgerActor() {
    // Create actor for ICP ledger with icrc1_transfer method
    // Use existing actor creation pattern from the codebase
}
```

---

## Phase 4: Remove Oisy Blocker

### Blockers in `apiClient.ts` (7 locations)

Lines 390, 543, 611, 769, 836, 1034, 1493 all have:
```typescript
if (!walletOperations.supportsVaultOperations()) {
```

These checks need to be either:
- Removed entirely (if using push-deposit for all wallets), OR
- Modified to only block for non-ICP collateral types

### Blockers in `walletOperations.ts`

- **Line 30**: `isOisyWallet()` helper function
- **Line 45**: `supportsIcrc2CanisterCalls()` returns `!isOisyWallet()`
- **Line 69**: `static supportsVaultOperations()` method
- **Line 78**: Error message getter
- **Lines 105-108**: `approveIcpTransfer()` early return for Oisy
- **Lines 300-303**: `approveIcusdTransfer()` early return for Oisy

### Strategy

Instead of removing these entirely, modify the flow:
1. Keep `isOisyWallet()` detection
2. In `apiClient.ts` vault methods, branch based on wallet type:
   - Oisy → use push-deposit flow (new)
   - Others → use existing ICRC-2 flow
3. Remove the early-return blockers that prevent Oisy from proceeding

---

## Phase 5: Testing

Test with all three wallets:

1. **Plug Wallet**: Verify existing ICRC-2 flow still works
2. **Internet Identity**: Verify existing ICRC-2 flow still works  
3. **Oisy Wallet**: Verify new push-deposit flow works

Edge cases to test:
- User refreshes page after depositing but before creating vault (should still work - deposit is tracked)
- User tries to create vault with insufficient deposit
- User tries to double-credit the same deposit

---

## Commit Strategy

1. Commit backend helpers (already in working directory)
2. Commit Phase 1 (open_vault_with_icp_deposit)
3. Commit Phase 2 (get_my_icp_deposit_account)
4. Commit Phase 3 + 4 (frontend changes + remove blocker)
5. Deploy and test

---

## Quick Verification Commands

```bash
# Check if code compiles
cd /Users/robertripley/coding/rumi-protocol-v2
cargo build --target wasm32-unknown-unknown -p rumi_protocol_backend

# See current git status
git status

# Find the Oisy blocker
grep -rn "supportsVaultOperations\|Oisy wallet does not" src/vault_frontend/
```
