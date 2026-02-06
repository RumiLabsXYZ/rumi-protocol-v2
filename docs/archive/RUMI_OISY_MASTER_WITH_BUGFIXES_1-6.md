Rumi Protocol Vault Frontend + Oisy Wallet Integration


Unified implementation notes, deployment steps, and bug fixes


Last updated: January 30, 2026


Sources merged:
- OISY_IMPLEMENTATION_MASTER_MERGED_1234.md
- BUG_VAULT_CREATION_INITIAL_BORROW.md
- OISY_QUERY_SIGNER_BUG.md


---


## 1) Implementation master


Oisy Wallet Integration (Rumi Protocol) - Unified Master Implementation Doc

Last updated: January 30, 2026
Repository: /Users/robertripley/coding/rumi-protocol-v2
GitHub: https://github.com/RumiLabsXYZ/rumi-protocol-v2

---

Purpose
Enable Oisy wallet users to create vaults and interact with Rumi Protocol without relying on ICRC-2 approve/transfer_from calls that Oisy does not support.

Problem summary
Oisy uses ICRC-25/27 signer standards and cannot execute arbitrary canister calls like icrc2_allowance or icrc2_approve against the ICP ledger. Any flow that depends on backend pull-transfers (transfer_from) fails for Oisy. The fix is a push-deposit flow where the user sends ICP directly to a backend-controlled account, and the backend only reads balances and credits new deposits.

---

Current status

Working operations with Oisy (no ICRC-2 required)

These operations work with Oisy because the backend sends tokens TO the user (no approval needed):

| Operation | Function | Status |
|-----------|----------|--------|
| Create Vault | `openVaultWithPushDeposit()` | ✅ Uses push-deposit flow |
| Borrow icUSD | `borrowFromVault()` | ✅ Backend mints icUSD to user |
| Close Vault | `closeVault()` | ✅ Backend sends ICP to user |
| Withdraw Collateral | `withdrawCollateral()` | ✅ Backend sends ICP to user |
| Withdraw & Close | `withdrawCollateralAndCloseVault()` | ✅ Backend sends ICP to user |

Operations still needing Oisy-compatible push-deposit variants

These operations require the user to send tokens TO the backend - need push-deposit variants for Oisy:

| Operation | Function | Issue |
|-----------|----------|-------|
| Add Margin | `addMarginToVault()` | User sends ICP - needs push-deposit variant |
| Repay Debt | `repayToVault()` | User sends icUSD - needs push-deposit variant |

---

---

Reference: deployed canisters and URLs

| Canister | ID | URL |
|----------|----|----|
| Backend | `tfesu-vyaaa-aaaap-qrd7a-cai` | https://a]a4gq6-oaaaa-aaaab-qaa4q-cai.raw.icp0.io/?id=tfesu-vyaaa-aaaap-qrd7a-cai |
| Frontend | `tcfua-yaaaa-aaaap-qrd7q-cai` | https://tcfua-yaaaa-aaaap-qrd7q-cai.icp0.io |
| ICP Ledger | `ryjl3-tyaaa-aaaaa-aaaba-cai` | (system canister) |
| icUSD Ledger | (check dfx.json) | |

**Custom Domain:** https://rumiprotocol.io

---

---

Solution overview: push-deposit flow

Traditional flow (Plug/II) pulls ICP using ICRC-2 approve and transfer_from.
Oisy flow pushes ICP using icrc1_transfer to a deterministic per-user deposit account owned by the backend canister, then calls a backend method that credits only the newly deposited amount and opens the vault.

High level Oisy vault create sequence

1) Frontend asks backend for the caller's deposit Account (owner = backend canister principal, subaccount = derived from caller principal).
2) Frontend transfers ICP to that Account using icrc1_transfer on the ICP ledger.
3) Frontend calls backend open_vault_with_deposit(borrow_amount).
4) Backend checks deposit Account balance, subtracts previously credited amount, validates collateral, updates credited map, and opens the vault with the credited collateral.

---

Repo paths

Backend: /Users/robertripley/coding/rumi-protocol-v2/src/rumi_protocol_backend
Frontend: /Users/robertripley/coding/rumi-protocol-v2/src/vault_frontend

---

Backend implementation details

State: track credited deposits to prevent double-crediting
**File**: `src/rumi_protocol_backend/src/state.rs`

Added state field to track credited deposits per user:
```rust
pub credited_icp_e8s: BTreeMap<Principal, u64>,
```

This prevents double-crediting the same deposit across multiple vault creation attempts.

Core backend methods and candid surface
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

Full step-by-step implementation (backend + candid + frontend) follows. This section is copy-paste oriented.

---

Push-deposit implementation steps (copy-paste)

Oisy ICP Deposit Flow (Push-Deposit) – Unified Implementation Doc

Purpose
Oisy wallet cannot create vaults with the current approach because the current collateral flow uses ICRC-2 approve/transfer_from, which Oisy’s signer does not support.

Solution overview
Implement a push-deposit flow:
- Backend derives a deterministic deposit subaccount for each user principal.
- User sends ICP directly to that deposit address using ICP Ledger icrc1_transfer.
- Backend reads the current balance at that address, computes the “new deposit” as (current_balance - previously_credited), updates previously_credited, then opens a vault using the new deposit amount.

Canister IDs
- ICP Ledger: ryjl3-tyaaa-aaaaa-aaaba-cai
- Rumi Backend: tfesu-vyaaa-aaaap-qrd7a-cai

Repo paths
- Backend: /Users/robertripley/coding/rumi-protocol-v2/src/rumi_protocol_backend/src/
- Frontend: /Users/robertripley/coding/rumi-protocol-v2/src/vault_frontend/src/lib/services/protocol/

Backend pieces that already exist (do not add again)
1) state.rs: pub credited_icp_e8s: BTreeMap<Principal, u64>
2) management.rs: compute_deposit_subaccount(caller: Principal) -> [u8; 32] (SHA-256 over b"rumi-deposit" || caller.as_slice())
3) management.rs: icp_balance_of(account: Account) -> Result<u64, ProtocolError>

Core idea
Deposit address is Account { owner = backend canister principal (ic_cdk::id()), subaccount = compute_deposit_subaccount(user_principal) }. The backend credits only current_balance - previously_credited, then sets credited = current_balance.

Backend implementation

Step 1: Add push-deposit vault opener and deposit account query
File: /Users/robertripley/coding/rumi-protocol-v2/src/rumi_protocol_backend/src/vault.rs
Placement: after the existing open_vault function

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

Notes
- The snippet currently sets borrowed_icusd_amount to 0 and calls open_vault_with_deposit(0n) from the frontend. If you want this to open-and-borrow in one call, wire borrow_amount into your existing internal borrowing path.

Step 2: Expose methods in main.rs
File: /Users/robertripley/coding/rumi-protocol-v2/src/rumi_protocol_backend/src/main.rs

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

Step 3: Update Candid
File: /Users/robertripley/coding/rumi-protocol-v2/src/rumi_protocol_backend/rumi_protocol_backend.did

Add Account if missing:
```candid
type Account = record {
  owner : principal;
  subaccount : opt blob;
};
```

Add service methods:
```candid
get_icp_deposit_account : () -> (Account) query;
open_vault_with_deposit : (nat64) -> (variant { Ok : OpenVaultSuccess; Err : ProtocolError });
```

Frontend implementation

Step 4: Branch in apiClient.ts openVault for Oisy
File: /Users/robertripley/coding/rumi-protocol-v2/src/vault_frontend/src/lib/services/protocol/apiClient.ts
Find openVault (original note: around line ~382). Replace the Oisy blocker:

BEFORE
```ts
if (!walletOperations.supportsVaultOperations()) {
  return {
    success: false,
    error: walletOperations.getWalletLimitationMessage()
  };
}
```

AFTER
```ts
// Check if using Oisy - use push-deposit flow instead of ICRC-2
const walletType = localStorage.getItem('rumi_last_wallet');
if (walletType === 'oisy') {
  return this.openVaultWithPushDeposit(icpAmount);
}
```

Step 5: Add push-deposit method and ICP ledger actor
Add these methods inside the ApiClient class:

```ts
async openVaultWithPushDeposit(icpAmountE8s: bigint): Promise<{ success: boolean; vaultId?: bigint; error?: string }> {
  try {
    console.log('[openVaultWithPushDeposit] Starting push deposit flow for', icpAmountE8s, 'e8s');

    const depositAccount = await this.actor.get_icp_deposit_account();

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
      return { success: false, error: `ICP transfer failed: ${JSON.stringify(transferResult.Err)}` };
    }

    const vaultResult = await this.actor.open_vault_with_deposit(0n);

    if ('Err' in vaultResult) {
      return { success: false, error: `Vault creation failed: ${JSON.stringify(vaultResult.Err)}` };
    }

    return { success: true, vaultId: vaultResult.Ok.vault_id };
  } catch (error) {
    return { success: false, error: String(error) };
  }
}

private async getIcpLedgerActor(): Promise<any> {
  const { HttpAgent, Actor } = await import('@dfinity/agent');
  const ICP_LEDGER_ID = 'ryjl3-tyaaa-aaaaa-aaaba-cai';

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

    const TransferResult = IDL.Variant({ Ok: IDL.Nat, Err: TransferError });

    return IDL.Service({
      icrc1_transfer: IDL.Func([TransferArg], [TransferResult], []),
    });
  };

  const agent = new HttpAgent({ host: 'https://icp0.io' });
  return Actor.createActor(idlFactory, { agent, canisterId: ICP_LEDGER_ID });
}
```

Notes
- The example calls open_vault_with_deposit(0n). If you want this path to open-and-borrow, pass borrow amount and update the backend logic accordingly.

Known Oisy blockers to review and adjust
apiClient.ts has multiple guards that may still block Oisy even after updating openVault.
- Plan note: lines 390, 543, 611, 769, 836, 1034, 1493 contain `if (!walletOperations.supportsVaultOperations()) {`
Recommended strategy:
- Keep isOisyWallet() detection.
- In vault methods, branch:
  - Oisy -> use push-deposit flow
  - Other wallets -> existing ICRC-2 flow
- Remove or narrow any guard that blanket-blocks Oisy when the method can route to the push-deposit flow.

walletOperations.ts also contains early returns that block Oisy:
- isOisyWallet(), supportsIcrc2CanisterCalls(), supportsVaultOperations(), limitation message
- approveIcpTransfer() early return for Oisy
- approveIcusdTransfer() early return for Oisy

Testing checklist
Test with:
1) Plug wallet: existing ICRC-2 flow still works
2) Internet Identity: existing ICRC-2 flow still works
3) Oisy wallet: push-deposit flow works end-to-end

Edge cases:
- Deposit ICP, refresh page, then open vault: should still work (delta-credit logic)
- Insufficient deposit: should fail minimum collateral check
- Double-credit attempt: should not be possible because credited is set to current_balance

Build and deploy
```bash
cd /Users/robertripley/coding/rumi-protocol-v2
cargo build --target wasm32-unknown-unknown -p rumi_protocol_backend

dfx deploy rumi_protocol_backend --network ic
```

Quick verification commands
```bash
cd /Users/robertripley/coding/rumi-protocol-v2

git status

grep -rn "supportsVaultOperations\|isOisyWallet\|Oisy" src/vault_frontend/

grep -rn "open_vault_with_deposit\|get_icp_deposit_account" src/
```

Naming note
Earlier planning used names like `open_vault_with_icp_deposit` and `get_my_icp_deposit_account`. The consolidated implementation here uses:
- open_vault_with_deposit
- get_icp_deposit_account
Either naming works if you keep frontend + candid aligned.

---

Frontend critical note: wallet type key

The app stores the last wallet type in localStorage under 'rumi_last_wallet'. If you branch on 'walletType' the Oisy flow will not trigger even when connected to Oisy.

Expected value check
```javascript
localStorage.getItem('rumi_last_wallet')  // should be 'oisy' when connected with Oisy
```

---

ICRC-28 trusted origins check

Oisy and other signer flows can depend on ICRC-28 trusted origins. Verify these are present in the backend trusted origins list.

```rust
vec![
    "https://tcfua-yaaaa-aaaap-qrd7q-cai.icp0.io".to_string(),
    "https://rumiprotocol.io".to_string(),
    "https://app.rumiprotocol.io".to_string(),
]
```

---

Testing checklist

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

---

Deployment commands

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

---


---


## 2) Known issues and fixes



### 2.1 Oisy signer popup triggered by query calls (vault list not loading)


**Symptom**
When connected with Oisy, read-only calls like loading vaults trigger a signer popup and fail with “Signer window could not be opened”, which prevents vault data from loading.

**Root cause**
The PNP `getActor()` path routes *all* canister calls (including queries) through the signer channel for Oisy. Query calls should not be signed and should use an anonymous `HttpAgent`.

**Fix**
Add a dedicated query actor that always uses an anonymous `HttpAgent`, then switch query-only API methods to use it.

**Exact changes**
1) `src/vault_frontend/src/lib/services/auth.ts`
- Add `getQueryActor<T>(canisterId, idl)` that creates an actor with an anonymous `HttpAgent` (mainnet host `https://icp0.io`, local `http://localhost:4943`, and `fetchRootKey()` for local).

2) `src/vault_frontend/src/lib/services/protocol/apiClient.ts`
- Add `private static async getQueryActor()` that calls `walletStore.getQueryActor(...)`.
- Update query-only methods (at minimum `getUserVaults`, and any calls to `get_vault`, `get_vaults`, etc.) to use `getQueryActor()` instead of `getAuthenticatedActor()`.

**Keep `getAuthenticatedActor()` for update calls**
Use `getAuthenticatedActor()` for methods that mutate state such as opening a vault, borrowing, adding margin, repaying, withdrawing, and closing.

**Testing checklist**
- Connect Oisy and load `/vaults`: vault list loads with no signer popup.
- Open an existing vault: details load with no signer popup.
- Perform an update action (create vault, borrow, withdraw, close): signer popup appears as expected.
- Re-test Plug and Internet Identity flows to ensure no regression.

**Note**
The original bug note file lists “Date: January 26, 2025”, but the debugging context and canister IDs match the January 2026 workstream. Use the logic above regardless of the typo.



### 2.2 Vault creation “initial borrow” shows 0 icUSD borrowed


**Symptom**
User enters a borrow amount during vault creation, UI reports success, but the vault is created with `Borrowed: 0 icUSD` and an infinite collateral ratio.

**Root causes**
- Frontend: `open_vault_with_deposit` was called with `0n` (hardcoded) instead of the user’s intended `borrow_amount`.
- Backend: `open_vault_with_deposit` ignored the `borrow_amount` parameter and hardcoded `borrowed_icusd_amount` to zero.

**Fix overview**
- Frontend passes the requested borrow amount into `open_vault_with_deposit(borrowAmount)`.
- Backend uses `borrow_amount` and, after vault creation, mints icUSD (minus fee), records the borrow, and routes the fee to treasury. It should also validate the max borrowable amount based on collateral and the minimum collateral ratio.

**Files involved**
- Frontend: `src/vault_frontend/src/lib/services/protocol/apiClient.ts` (pass borrow amount)
- Frontend: `src/vault_frontend/src/routes/.../+page.svelte` (wallet-type detection / flow selection if applicable)
- Backend: `src/rumi_protocol_backend/src/vault.rs` (`open_vault_with_deposit`)

**Validation**
Before minting, compute `max_borrowable_amount` using ICP price and `min_collateral_ratio` and return an error if `borrow_amount` exceeds it.

**Error handling expectation**
If minting fails, the vault can still exist and the user can borrow later via the standard borrow flow (but mint failure should be logged clearly).

**Testing checklist**
- Oisy: create vault with an initial borrow (for example deposit 0.10 ICP, borrow 0.05 icUSD), verify icUSD balance increases and vault card shows non-zero borrowed.
- Plug and Internet Identity: repeat the same flow to confirm parity.
- Try an intentionally too-high borrow and confirm it is rejected with a clear error.


---


## 3) Current priorities


1) Implement push-deposit variants for operations where the user must send assets *to* the backend
- `addMarginToVault()` (ICP in)
- `repayToVault()` (icUSD in)

2) Keep the credited balance accounting correct
- Ensure `credited_*` maps prevent double-credit across retries
- Make partial deposits and repeated calls idempotent where possible

3) Confirm ICRC-28 trusted origins
- Ensure the frontend origins are whitelisted in the backend (including the deployed canister URL and custom domains).
