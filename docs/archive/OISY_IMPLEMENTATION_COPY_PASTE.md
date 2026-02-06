# Oisy ICP Deposit - COPY-PASTE Implementation

**DO NOT RESEARCH. Just copy-paste the code blocks below into the specified files.**

## Canister IDs
- ICP Ledger: `ryjl3-tyaaa-aaaaa-aaaba-cai`
- Rumi Backend: `tfesu-vyaaa-aaaap-qrd7a-cai`

---

## STEP 1: Backend - Add to vault.rs

**File**: `/Users/robertripley/coding/rumi-protocol-v2/src/rumi_protocol_backend/src/vault.rs`

**Location**: Add this function after the existing `open_vault` function (around line 200):

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

/// Returns the deposit account for the caller to send ICP to.
pub fn get_icp_deposit_account() -> Account {
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

## STEP 2: Backend - Expose in main.rs

**File**: `/Users/robertripley/coding/rumi-protocol-v2/src/rumi_protocol_backend/src/main.rs`

**Location**: Find the `#[update]` and `#[query]` canister methods section. Add these two:

```rust
#[update]
async fn open_vault_with_deposit(borrow_amount: u64) -> Result<OpenVaultSuccess, ProtocolError> {
    vault::open_vault_with_deposit(borrow_amount).await
}

#[query]
fn get_icp_deposit_account() -> icrc_ledger_types::icrc1::account::Account {
    vault::get_icp_deposit_account()
}
```

---

## STEP 3: Backend - Update Candid file

**File**: `/Users/robertripley/coding/rumi-protocol-v2/src/rumi_protocol_backend/rumi_protocol_backend.did`

**Add these type definitions if not present**:
```candid
type Account = record {
  owner : principal;
  subaccount : opt blob;
};
```

**Add these service methods**:
```candid
  get_icp_deposit_account : () -> (Account) query;
  open_vault_with_deposit : (nat64) -> (variant { Ok : OpenVaultSuccess; Err : ProtocolError });
```

---

## STEP 4: Frontend - Modify apiClient.ts openVault method

**File**: `/Users/robertripley/coding/rumi-protocol-v2/src/vault_frontend/src/lib/services/protocol/apiClient.ts`

**Find the `openVault` method (around line 382)**. Replace the Oisy blocker check with a branch:

**BEFORE** (around line 390):
```typescript
if (!walletOperations.supportsVaultOperations()) {
    return {
        success: false,
        error: walletOperations.getWalletLimitationMessage()
    };
}
```

**AFTER**:
```typescript
// Check if using Oisy - use push-deposit flow instead of ICRC-2
const walletType = localStorage.getItem('walletType');
if (walletType === 'oisy') {
    return this.openVaultWithPushDeposit(icpAmount);
}
```

---

## STEP 5: Frontend - Add push deposit method to apiClient.ts

**File**: `/Users/robertripley/coding/rumi-protocol-v2/src/vault_frontend/src/lib/services/protocol/apiClient.ts`

**Add this new method** (anywhere in the class, e.g., after `openVault`):

```typescript
/**
 * Opens a vault using the push-deposit flow (for Oisy wallet).
 * User transfers ICP to their deposit address, then we create the vault.
 */
async openVaultWithPushDeposit(icpAmountE8s: bigint): Promise<{ success: boolean; vaultId?: bigint; error?: string }> {
    try {
        console.log('[openVaultWithPushDeposit] Starting push deposit flow for', icpAmountE8s, 'e8s');

        // 1. Get deposit address from backend
        const depositAccount = await this.actor.get_icp_deposit_account();
        console.log('[openVaultWithPushDeposit] Deposit account:', depositAccount);

        // 2. Transfer ICP to deposit address
        const icpLedgerActor = await this.getIcpLedgerActor();
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
        const vaultResult = await this.actor.open_vault_with_deposit(0n); // borrow_amount = 0 for now

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

/**
 * Gets an actor for the ICP ledger canister.
 */
private async getIcpLedgerActor(): Promise<any> {
    const { HttpAgent, Actor } = await import('@dfinity/agent');
    
    const ICP_LEDGER_ID = 'ryjl3-tyaaa-aaaaa-aaaba-cai';
    
    // Minimal IDL for icrc1_transfer
    const idlFactory = ({ IDL }: any) => {
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

    // Use existing agent from the class or create new one
    const agent = new HttpAgent({ host: 'https://icp0.io' });
    return Actor.createActor(idlFactory, { agent, canisterId: ICP_LEDGER_ID });
}
```

---

## STEP 6: Compile and Test

```bash
cd /Users/robertripley/coding/rumi-protocol-v2

# Compile backend
cargo build --target wasm32-unknown-unknown -p rumi_protocol_backend

# If successful, deploy
dfx deploy rumi_protocol_backend --network ic
```

---

## What Already Exists (DO NOT ADD AGAIN)

These are already in the codebase (added in previous session):

1. `state.rs` line 159: `pub credited_icp_e8s: BTreeMap<Principal, u64>`
2. `management.rs` line 269: `compute_deposit_subaccount(caller: Principal) -> [u8; 32]`
3. `management.rs` line 279: `icp_balance_of(account: Account) -> Result<u64, ProtocolError>`

---

## Summary

| Step | File | Action |
|------|------|--------|
| 1 | vault.rs | Add `open_vault_with_deposit` and `get_icp_deposit_account` |
| 2 | main.rs | Add `#[update]` and `#[query]` wrappers |
| 3 | .did | Add Candid definitions |
| 4 | apiClient.ts | Replace Oisy blocker with branch to push flow |
| 5 | apiClient.ts | Add `openVaultWithPushDeposit` and `getIcpLedgerActor` |
| 6 | Terminal | Compile and deploy |
