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
use candid::CandidType;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::HashMap;

/// Standard ICRC-1 transfer fee (native units), used as a conservative fallback
/// when a ledger's `icrc1_fee` query cannot be reached. Erring high keeps the
/// pool solvent (we send slightly less) rather than risking an over-send.
const DEFAULT_LEDGER_FEE: u128 = 10_000;

/// Exact identity for one persisted pending-claim payout attempt. The ledger
/// deduplicates only structurally equal calls and only for its configured
/// transaction window, so a retry must reuse this tuple and TooOld must leave
/// the claim held for receipt-backed reconciliation.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayoutAttempt {
    pub fee: u128,
    pub created_at_time: u64,
    pub memo: Vec<u8>,
    pub attempt_no: u32,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PendingPayoutState {
    FreshNoDispatch,
    /// Persisted before a ledger await. A later entry seeing this state must
    /// assume the request may have committed and can only retry the same tuple.
    Submitted(PayoutAttempt),
    /// A typed clean rejection was observed with no earlier ambiguous dispatch.
    ProvenNoEffect(PayoutAttempt),
    /// At least one dispatch may have committed; even a later typed rejection
    /// cannot authorize a new tuple or a refund.
    Ambiguous(PayoutAttempt),
}

pub fn new_payout_attempt(claim_id: u64, fee: u128, attempt: u32) -> PayoutAttempt {
    let mut digest = Sha256::new();
    digest.update(b"rumi-3pool-claim-payout-v1");
    digest.update(claim_id.to_be_bytes());
    digest.update(attempt.to_be_bytes());
    PayoutAttempt {
        fee,
        created_at_time: ic_cdk::api::time(),
        memo: digest.finalize().to_vec(),
        attempt_no: attempt,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PayoutFailure {
    ProvenNoEffect(String),
    Ambiguous(String),
}

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

/// Execute a payout using the exact persisted identity of a pending claim.
/// Duplicate is success only because this uses the same saved call tuple.
pub async fn execute_claim_payout(
    ledger: Principal,
    to: Principal,
    amount: u128,
    attempt: &PayoutAttempt,
    prior_ambiguous: bool,
) -> Result<(), PayoutFailure> {
    if amount <= attempt.fee {
        let detail = format!("amount {} does not exceed ledger fee {}; payout not sent", amount, attempt.fee);
        return Err(if prior_ambiguous { PayoutFailure::Ambiguous(detail) } else { PayoutFailure::ProvenNoEffect(detail) });
    }
    let args = TransferArg {
        from_subaccount: None,
        to: Account { owner: to, subaccount: None },
        amount: candid::Nat::from(amount - attempt.fee),
        fee: Some(candid::Nat::from(attempt.fee)),
        memo: Some(attempt.memo.clone().into()),
        created_at_time: Some(attempt.created_at_time),
    };
    let result: Result<(Result<candid::Nat, TransferError>,), _> =
        ic_cdk::call(ledger, "icrc1_transfer", (args,)).await;
    match result {
        Ok((Ok(_),)) | Ok((Err(TransferError::Duplicate { .. }),)) => Ok(()),
        Ok((Err(TransferError::BadFee { expected_fee }),)) => {
            if let Ok(expected) = expected_fee.0.clone().try_into() {
                LEDGER_FEES.with(|cache| cache.borrow_mut().insert(ledger, expected));
            }
            let detail = format!("icrc1_transfer BadFee; ledger reports fee {}", expected_fee);
            if prior_ambiguous { Err(PayoutFailure::Ambiguous(detail)) }
            else { Err(PayoutFailure::ProvenNoEffect(detail)) }
        }
        Ok((Err(e @ TransferError::BadBurn { .. }),))
        | Ok((Err(e @ TransferError::InsufficientFunds { .. }),))
        | Ok((Err(e @ TransferError::CreatedInFuture { .. }),)) => {
            let detail = format!("icrc1_transfer error: {e:?}");
            if prior_ambiguous { Err(PayoutFailure::Ambiguous(detail)) }
            else { Err(PayoutFailure::ProvenNoEffect(detail)) }
        }
        // TooOld cannot distinguish an earlier committed dispatch after the
        // ledger's finite dedup window, so keep this as an unresolved hold.
        Ok((Err(e @ TransferError::TooOld),))
        | Ok((Err(e @ TransferError::TemporarilyUnavailable),))
        | Ok((Err(e @ TransferError::GenericError { .. }),)) =>
            Err(PayoutFailure::Ambiguous(format!("icrc1_transfer error: {e:?}"))),
        Err((code, msg)) => Err(PayoutFailure::Ambiguous(format!(
            "inter-canister call failed: {:?} - {}", code, msg
        ))),
    }
}
