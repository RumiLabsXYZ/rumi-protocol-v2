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
    #[cfg(any(feature = "test_endpoints", test))]
    static GATE_NEXT_FEE_LOOKUP: RefCell<bool> = const { RefCell::new(false) };
}

#[cfg(any(feature = "test_endpoints", test))]
pub(crate) fn gate_next_fee_lookup() {
    GATE_NEXT_FEE_LOOKUP.with(|gate| *gate.borrow_mut() = true);
}

/// Fetch a ledger's transfer fee, caching the result per ledger. On query
/// failure, falls back to the standard ICRC-1 fee (the solvency-safe direction).
pub async fn ledger_fee(ledger: Principal) -> u128 {
    if let Some(fee) = LEDGER_FEES.with(|c| c.borrow().get(&ledger).copied()) {
        return fee;
    }
    // Test-only observable barrier. PocketIC holds this HTTPS reply until the
    // regression test's competing LP transfer has completed, proving the
    // withdrawal passed its first ownership check before that transfer.
    #[cfg(any(feature = "test_endpoints", test))]
    if GATE_NEXT_FEE_LOOKUP.with(|gate| gate.replace(false)) {
        use ic_cdk::api::management_canister::http_request::{
            http_request, CanisterHttpRequestArgument, HttpMethod,
        };
        let _ = http_request(
            CanisterHttpRequestArgument {
                url: "https://3pool-fee-gate.test/hold".into(),
                max_response_bytes: Some(1),
                method: HttpMethod::GET,
                headers: vec![],
                body: None,
                transform: None,
            },
            1_000_000_000,
        )
        .await;
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

/// Refresh a cached fee from the ledger. Used only when a payout is no larger
/// than the cached fee, where stale-high cache data would otherwise prevent a
/// transfer attempt (and thus never receive BadFee after the ledger fee drops).
pub async fn refresh_ledger_fee(ledger: Principal) -> Result<u128, String> {
    let result: Result<(candid::Nat,), _> = ic_cdk::call(ledger, "icrc1_fee", ()).await;
    let fee: u128 = match result {
        Ok((fee,)) => fee
            .0
            .try_into()
            .map_err(|_| format!("ledger {} returned an unsupported icrc1_fee", ledger))?,
        Err((code, message)) => {
            return Err(format!("icrc1_fee query failed: {:?} - {}", code, message));
        }
    };
    LEDGER_FEES.with(|cache| cache.borrow_mut().insert(ledger, fee));
    Ok(fee)
}

pub async fn ledger_fee_for_amount(ledger: Principal, amount: u128) -> u128 {
    let cached = ledger_fee(ledger).await;
    if amount <= cached {
        refresh_ledger_fee(ledger).await.unwrap_or(cached)
    } else {
        cached
    }
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
    let fee = ledger_fee_for_amount(ledger, amount).await;
    if amount <= fee {
        // Returning success here would make admin/debt callers clear a value
        // obligation without making any ledger transfer. Preserve it by
        // treating fee-sized dust as a failed payout.
        return Err(format!(
            "amount {} does not exceed ledger fee {}; payout not sent",
            amount, fee
        ));
    }
    let send = amount - fee;
    let args = TransferArg {
        from_subaccount: None,
        to: Account {
            owner: to,
            subaccount: None,
        },
        amount: candid::Nat::from(send),
        // Supplying the fee we used to calculate `send` makes fee-cache drift
        // fail atomically with BadFee instead of letting the ledger charge a
        // different fee while the pool debits only the cached amount.
        fee: Some(candid::Nat::from(fee)),
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
        Ok((Err(TransferError::BadFee { expected_fee }),)) => {
            if let Ok(expected) = expected_fee.0.clone().try_into() {
                LEDGER_FEES.with(|c| c.borrow_mut().insert(ledger, expected));
            }
            Err(format!(
                "icrc1_transfer BadFee; refreshed fee cache, retry operation: expected {}",
                expected_fee
            ))
        }
        Ok((Err(e),)) => Err(format!("icrc1_transfer error: {:?}", e)),
        Err((code, msg)) => Err(format!(
            "inter-canister call failed: {:?} - {}",
            code, msg
        )),
    }
}
