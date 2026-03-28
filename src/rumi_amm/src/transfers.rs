// ICRC-1 / ICRC-2 token transfer helpers for the Rumi AMM.
// Unlike the 3pool, these helpers support subaccounts for per-pool fund segregation.

use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};

/// Transfer tokens FROM a user TO a pool's subaccount (requires prior ICRC-2 approval).
pub async fn transfer_from_user(
    ledger: Principal,
    from: Principal,
    to_subaccount: [u8; 32],
    amount: u128,
) -> Result<u64, String> {
    let args = TransferFromArgs {
        spender_subaccount: None,
        from: Account {
            owner: from,
            subaccount: None,
        },
        to: Account {
            owner: ic_cdk::id(),
            subaccount: Some(to_subaccount),
        },
        amount: candid::Nat::from(amount),
        fee: None,
        memo: None,
        created_at_time: None,
    };

    let result: Result<(Result<candid::Nat, TransferFromError>,), _> =
        ic_cdk::call(ledger, "icrc2_transfer_from", (args,)).await;

    match result {
        Ok((Ok(block_index),)) => {
            // Convert Nat to u64
            let idx: u64 = block_index.0.try_into().unwrap_or_else(|_| {
                ic_cdk::println!("WARN: block index exceeds u64::MAX, returning 0");
                0
            });
            Ok(idx)
        }
        Ok((Err(e),)) => Err(format!("icrc2_transfer_from error: {:?}", e)),
        Err((code, msg)) => Err(format!("inter-canister call failed: {:?} - {}", code, msg)),
    }
}

/// Transfer tokens FROM a pool's subaccount TO a user.
pub async fn transfer_to_user(
    ledger: Principal,
    from_subaccount: [u8; 32],
    to: Principal,
    amount: u128,
) -> Result<u64, String> {
    let args = TransferArg {
        from_subaccount: Some(from_subaccount),
        to: Account {
            owner: to,
            subaccount: None,
        },
        amount: candid::Nat::from(amount),
        fee: None,
        memo: None,
        created_at_time: None,
    };

    let result: Result<(Result<candid::Nat, TransferError>,), _> =
        ic_cdk::call(ledger, "icrc1_transfer", (args,)).await;

    match result {
        Ok((Ok(block_index),)) => {
            let idx: u64 = block_index.0.try_into().unwrap_or_else(|_| {
                ic_cdk::println!("WARN: block index exceeds u64::MAX, returning 0");
                0
            });
            Ok(idx)
        }
        Ok((Err(e),)) => Err(format!("icrc1_transfer error: {:?}", e)),
        Err((code, msg)) => Err(format!("inter-canister call failed: {:?} - {}", code, msg)),
    }
}