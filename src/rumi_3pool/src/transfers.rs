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
use std::cell::RefCell;
use std::collections::HashMap;

/// Standard ICRC-1 transfer fee (native units), used as a conservative fallback
/// when a ledger's `icrc1_fee` query cannot be reached. Erring high keeps the
/// pool solvent (we send slightly less) rather than risking an over-send.
const DEFAULT_LEDGER_FEE: u128 = 10_000;

thread_local! {
    /// Per-ledger transfer-fee cache, populated lazily from `icrc1_fee` on the
    /// first outbound transfer to a ledger. Heap-only (not persisted), so it is
    /// simply re-warmed after an upgrade.
    static LEDGER_FEES: RefCell<HashMap<Principal, u128>> = RefCell::new(HashMap::new());
}

/// Fetch a ledger's transfer fee, caching the result per ledger. On query
/// failure, falls back to the standard ICRC-1 fee (the solvency-safe direction).
pub async fn ledger_fee(ledger: Principal) -> u128 {
    if let Some(fee) = LEDGER_FEES.with(|c| c.borrow().get(&ledger).copied()) {
        return fee;
    }
    let result: Result<(candid::Nat,), _> =
        ic_cdk::call(ledger, "icrc1_fee", ()).await;
    let fee: u128 = match result {
        Ok((f,)) => f.0.try_into().unwrap_or(DEFAULT_LEDGER_FEE),
        Err(_) => DEFAULT_LEDGER_FEE,
    };
    LEDGER_FEES.with(|c| c.borrow_mut().insert(ledger, fee));
    fee
}

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
///
/// The ICRC-1 ledger debits `sent + fee` from this canister but credits the
/// recipient only `sent`. Callers debit the pool balance by the full `amount`,
/// so to keep tracked balances in step with the real on-chain holdings we send
/// `amount - fee`: the canister balance then drops by exactly `amount` and the
/// recipient (taker/withdrawer) bears the fee. This lets a 100% withdrawal drain
/// cleanly instead of drifting balances above real holdings (one fee per
/// transfer) and eventually failing the last withdrawal with InsufficientFunds.
pub async fn transfer_to_user(
    ledger: Principal,
    to: Principal,
    amount: u128,
) -> Result<(), String> {
    let fee = ledger_fee(ledger).await;
    if amount <= fee {
        // Nothing transferable once the ledger fee is covered. The caller has
        // already debited `amount` from the pool balance, so leaving this dust
        // keeps tracked balances <= real holdings (solvency-safe).
        return Ok(());
    }
    let send = amount - fee;
    let args = TransferArg {
        from_subaccount: None,
        to: Account {
            owner: to,
            subaccount: None,
        },
        amount: candid::Nat::from(send),
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
