// ICRC-1 / ICRC-2 token transfer helpers for the Rumi 3pool.
//
// Audit Wave-3 (ICRC-003/004): every transfer now sets `created_at_time`
// (so the ledger can dedup retries) and treats `Duplicate { duplicate_of }`
// as success — the previous attempt landed at that block, so the operation
// already succeeded.

use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};

/// Transfer tokens FROM a user TO this canister (requires prior ICRC-2 approval).
pub async fn transfer_from_user(
    ledger: Principal,
    from: Principal,
    amount: u128,
) -> Result<(), String> {
    let args = TransferFromArgs {
        spender_subaccount: None,
        from: Account {
            owner: from,
            subaccount: None,
        },
        to: Account {
            owner: ic_cdk::id(),
            subaccount: None,
        },
        amount: candid::Nat::from(amount),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()),
    };

    let result: Result<(Result<candid::Nat, TransferFromError>,), _> =
        ic_cdk::call(ledger, "icrc2_transfer_from", (args,)).await;

    match result {
        Ok((Ok(_block_index),)) => Ok(()),
        Ok((Err(TransferFromError::Duplicate { duplicate_of }),)) => {
            ic_cdk::println!(
                "[transfer_from_user] ledger {} reported Duplicate (block {}); treating as success",
                ledger, duplicate_of
            );
            Ok(())
        }
        Ok((Err(e),)) => Err(format!("icrc2_transfer_from error: {:?}", e)),
        Err((code, msg)) => Err(format!(
            "inter-canister call failed: {:?} - {}",
            code, msg
        )),
    }
}

/// Transfer tokens FROM this canister TO a user.
pub async fn transfer_to_user(
    ledger: Principal,
    to: Principal,
    amount: u128,
) -> Result<(), String> {
    let args = TransferArg {
        from_subaccount: None,
        to: Account {
            owner: to,
            subaccount: None,
        },
        amount: candid::Nat::from(amount),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()),
    };

    let result: Result<(Result<candid::Nat, TransferError>,), _> =
        ic_cdk::call(ledger, "icrc1_transfer", (args,)).await;

    match result {
        Ok((Ok(_block_index),)) => Ok(()),
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            ic_cdk::println!(
                "[transfer_to_user] ledger {} reported Duplicate (block {}); treating as success",
                ledger, duplicate_of
            );
            Ok(())
        }
        Ok((Err(e),)) => Err(format!("icrc1_transfer error: {:?}", e)),
        Err((code, msg)) => Err(format!(
            "inter-canister call failed: {:?} - {}",
            code, msg
        )),
    }
}
