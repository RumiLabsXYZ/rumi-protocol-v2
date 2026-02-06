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
const walletType = localStorage.getItem('walletType');
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
