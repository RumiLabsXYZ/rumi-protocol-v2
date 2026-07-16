// ICRC-1 / ICRC-2 token transfer helpers for the Rumi AMM.
// Unlike the 3pool, these helpers support subaccounts for per-pool fund segregation.

use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
use std::cell::RefCell;
use std::collections::HashMap;

/// Standard ICRC-1 transfer fee (e8s), used as a conservative fallback when a
/// ledger's `icrc1_fee` query cannot be reached. Erring high keeps the pool
/// solvent (we send slightly less) rather than risking an over-send.
const DEFAULT_LEDGER_FEE_E8S: u128 = 10_000;

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
        Ok((f,)) => f.0.try_into().unwrap_or(DEFAULT_LEDGER_FEE_E8S),
        Err(_) => DEFAULT_LEDGER_FEE_E8S,
    };
    LEDGER_FEES.with(|c| c.borrow_mut().insert(ledger, fee));
    fee
}

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
        // Set created_at_time for ledger-side deduplication. If a transfer
        // is accidentally submitted twice within the ledger's dedup window
        // (typically 24h), the second will be rejected as a duplicate.
        created_at_time: Some(ic_cdk::api::time()),
    };

    let result: Result<(Result<candid::Nat, TransferFromError>,), _> =
        ic_cdk::call(ledger, "icrc2_transfer_from", (args,)).await;

    match result {
        Ok((Ok(block_index),)) => {
            let idx: u64 = block_index.0.try_into().unwrap_or_else(|_| {
                ic_cdk::println!("WARN: block index exceeds u64::MAX, returning 0");
                0
            });
            Ok(idx)
        }
        // Audit Wave-3 (ICRC-003): Duplicate means the previous attempt's
        // transfer landed at `duplicate_of`. Treat as success.
        Ok((Err(TransferFromError::Duplicate { duplicate_of }),)) => {
            let idx: u64 = duplicate_of.0.try_into().unwrap_or(0);
            Ok(idx)
        }
        Ok((Err(e),)) => Err(format!("icrc2_transfer_from error: {:?}", e)),
        Err((code, msg)) => Err(format!("inter-canister call failed: {:?} - {}", code, msg)),
    }
}

/// Transfer tokens FROM a pool's subaccount TO a user.
///
/// The ICRC-1 ledger debits `sent + fee` from the source subaccount but credits
/// the recipient only `sent`. Callers debit their reserve by the full `amount`,
/// so to keep the tracked reserve exactly in step with the on-chain subaccount
/// balance we send `amount - fee`: the subaccount then drops by exactly `amount`
/// and the recipient (taker/withdrawer) bears the fee, as is standard. This is
/// what lets a 100% withdrawal drain cleanly instead of stranding the final
/// ledger-fee's worth of tokens. (Historically this sent the full `amount`,
/// which drifted reserves above the real balance by one fee per transfer and
/// eventually broke the last withdrawal with InsufficientFunds.)
pub async fn transfer_to_user(
    ledger: Principal,
    from_subaccount: [u8; 32],
    to: Principal,
    amount: u128,
) -> Result<u64, String> {
    let fee = ledger_fee(ledger).await;
    if amount <= fee {
        // Nothing is transferable once the ledger fee is covered. The caller
        // has already debited `amount` from its reserve, so leaving this dust
        // in the subaccount keeps reserves <= the real balance (solvency-safe).
        return Ok(0);
    }
    let send = amount - fee;
    let args = TransferArg {
        from_subaccount: Some(from_subaccount),
        to: Account {
            owner: to,
            subaccount: None,
        },
        amount: candid::Nat::from(send),
        fee: None,
        memo: None,
        // Set created_at_time for ledger-side deduplication.
        created_at_time: Some(ic_cdk::api::time()),
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
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            let idx: u64 = duplicate_of.0.try_into().unwrap_or(0);
            Ok(idx)
        }
        Ok((Err(e),)) => Err(format!("icrc1_transfer error: {:?}", e)),
        Err((code, msg)) => Err(format!("inter-canister call failed: {:?} - {}", code, msg)),
    }
}

/// Transfer reward icUSD from the per-pool reward subaccount to the caller.
///
/// `crate::reward_subaccount_for` and `crate::ICUSD_LEDGER` are `pub` so we
/// can derive the source subaccount and resolve the icUSD ledger principal
/// without duplicating constants.
///
/// Mirrors `transfer_to_user`'s fee handling: `notify_reward_received`
/// credits the FULL donated amount into `acc_reward_per_share`, so
/// Σ(claimable across all LPs) equals exactly the reward subaccount's
/// balance. The ICRC-1 ledger debits `sent + fee` from the source
/// subaccount but credits the recipient only `sent`, so if we sent the
/// full `amount` (fee: None) the subaccount would drop by `amount + fee`
/// per claim -- one ledger fee more than was ever reserved for it. Over
/// enough claims the subaccount balance drifts below the sum of
/// outstanding `claimable`, and a later claim fails with
/// InsufficientFunds even though the claimant is only owed what they're
/// entitled to. Sending `amount - fee` instead keeps the subaccount drop
/// exactly in step with `amount` (the claimant bears the fee, as is
/// standard), preserving Σclaimable == subaccount balance.
///
/// Unlike `transfer_to_user`, an `amount <= fee` here must be a hard
/// error rather than a silent `Ok(0)`: `claim_rewards` has already
/// optimistically zeroed the caller's `claimable` before invoking this
/// and treats any `Ok` as "the funds were sent". Returning `Ok(0)` would
/// silently burn the claim instead of restoring it for retry. In
/// practice this branch is unreachable because `claim_rewards` gates on
/// `MIN_CLAIM_E8S` (10x the live 100_000 e8s icUSD ledger fee) before ever
/// calling here, but it must fail closed if that invariant ever changes.
pub async fn transfer_reward_icusd(
    pool_id: &str,
    to: Principal,
    amount: u128,
) -> Result<u64, String> {
    let icusd_ledger = Principal::from_text(crate::ICUSD_LEDGER)
        .expect("invalid icUSD ledger principal");
    let fee = ledger_fee(icusd_ledger).await;
    if amount <= fee {
        return Err(format!(
            "reward amount {} does not exceed ledger fee {}; refusing to burn the claim",
            amount, fee
        ));
    }
    let send = amount - fee;
    let from_sub = crate::reward_subaccount_for(&pool_id.to_string());
    let args = TransferArg {
        from_subaccount: Some(from_sub),
        to: Account {
            owner: to,
            subaccount: None,
        },
        amount: candid::Nat::from(send),
        fee: None,
        memo: None,
        // Set created_at_time for ledger-side deduplication; matches the
        // pattern used by transfer_to_user above.
        created_at_time: Some(ic_cdk::api::time()),
    };

    let result: Result<(Result<candid::Nat, TransferError>,), _> =
        ic_cdk::call(icusd_ledger, "icrc1_transfer", (args,)).await;

    match result {
        Ok((Ok(block_index),)) => {
            let idx: u64 = block_index.0.try_into().unwrap_or_else(|_| {
                ic_cdk::println!("WARN: block index exceeds u64::MAX, returning 0");
                0
            });
            Ok(idx)
        }
        // Treat duplicates as success — the prior attempt landed.
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            let idx: u64 = duplicate_of.0.try_into().unwrap_or(0);
            Ok(idx)
        }
        Ok((Err(e),)) => Err(format!("icrc1_transfer error: {:?}", e)),
        Err((code, msg)) => Err(format!("inter-canister call failed: {:?} - {}", code, msg)),
    }
}
