// ICRC-1 / ICRC-2 token transfer helpers for the Rumi AMM.
// These helpers support subaccounts for per-pool fund segregation, except for
// THREEPOOL: its ledger rejects non-default account transfers.

use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
use std::cell::RefCell;
use std::collections::HashMap;

/// Standard ICRC-1 transfer fee (e8s), used as a conservative fallback when a
/// ledger's `icrc1_fee` query cannot be reached. Erring high keeps the pool
/// solvent (we send slightly less) rather than risking an over-send.
const DEFAULT_LEDGER_FEE_E8S: u128 = 10_000;

/// THREEPOOL only accepts the canister's default account. There can therefore
/// be only one AMM pool containing this ledger (enforced in `create_pool`).
pub(crate) fn pool_subaccount(ledger: Principal, subaccount: [u8; 32]) -> Option<[u8; 32]> {
    let threepool =
        Principal::from_text(crate::THREEPOOL).expect("invalid THREEPOOL ledger principal");
    (ledger != threepool).then_some(subaccount)
}

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
    let result: Result<(candid::Nat,), _> = ic_cdk::call(ledger, "icrc1_fee", ()).await;
    let fee: u128 = match result {
        Ok((f,)) => f.0.try_into().unwrap_or(DEFAULT_LEDGER_FEE_E8S),
        Err(_) => DEFAULT_LEDGER_FEE_E8S,
    };
    LEDGER_FEES.with(|c| c.borrow_mut().insert(ledger, fee));
    fee
}

/// Query an output fee for an inbound operation without the legacy fallback.
/// A guessed fee becomes immutable once the user's input is accepted, so this
/// path must fail before reserving/pulling input when the ledger is unavailable.
pub async fn ledger_fee_strict(ledger: Principal) -> Result<u128, String> {
    let (fee,): (candid::Nat,) = ic_cdk::call(ledger, "icrc1_fee", ())
        .await
        .map_err(|(code, message)| format!("icrc1_fee query failed ({code:?}): {message}"))?;
    fee.0
        .try_into()
        .map_err(|_| "icrc1_fee value exceeds u128".to_string())
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
            subaccount: pool_subaccount(ledger, to_subaccount),
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

/// Dispatch one persisted inbound leg. Replays use byte-for-byte identical
/// transfer arguments; only Ok or Duplicate is positive receipt evidence.
pub async fn dispatch_inbound_leg(
    caller: Principal,
    request_id: &[u8],
    leg_index: usize,
) -> Result<u64, String> {
    use crate::state::{set_inbound_leg_status, InboundLegStatus as Status};
    let operation = crate::state::inbound_operation(caller, request_id)?;
    let leg = operation
        .legs
        .get(leg_index)
        .cloned()
        .ok_or("inbound leg not found")?;
    if let Status::Confirmed(index) = leg.status {
        return Ok(index);
    }
    if leg.status == Status::ProvenNoEffect {
        return Err("inbound leg previously proven to have no effect".to_string());
    }
    if leg.status == Status::Ambiguous {
        let window = ledger_tx_window(leg.ledger).await.ok_or_else(|| {
            "ledger transaction window unavailable; ambiguous inbound leg remains held".to_string()
        })?;
        if !tx_window_allows_retry(leg.created_at_time, ic_cdk::api::time(), window) {
            return Err("ambiguous inbound leg is outside the ledger-reported transaction window; held pending exact receipt proof".to_string());
        }
    }
    // Mark before yielding to the ledger. A trap or lost callback therefore
    // cannot make this identity look undispatched after upgrade.
    set_inbound_leg_status(caller, request_id, leg_index, Status::Ambiguous)?;
    let args = inbound_transfer_arg(&leg, ic_cdk::id());
    let result: Result<(Result<candid::Nat, TransferFromError>,), _> =
        ic_cdk::call(leg.ledger, "icrc2_transfer_from", (args,)).await;
    let idx = match result {
        Ok((Ok(index),))
        | Ok((Err(TransferFromError::Duplicate {
            duplicate_of: index,
        }),)) => index,
        Ok((Err(e),)) => {
            let proven_no_effect = matches!(
                e,
                TransferFromError::BadFee { .. }
                    | TransferFromError::BadBurn { .. }
                    | TransferFromError::InsufficientFunds { .. }
                    | TransferFromError::InsufficientAllowance { .. }
                    | TransferFromError::CreatedInFuture { .. }
            );
            if proven_no_effect {
                set_inbound_leg_status(caller, request_id, leg_index, Status::ProvenNoEffect)?;
            }
            return Err(format!(
                "transfer_from result held as {:?}: {:?}",
                if proven_no_effect {
                    Status::ProvenNoEffect
                } else {
                    Status::Ambiguous
                },
                e
            ));
        }
        Err((code, msg)) => {
            return Err(format!("transfer_from call ambiguous: {:?}: {}", code, msg))
        }
    };
    let index: u64 = match idx.0.try_into() {
        Ok(index) => index,
        Err(_) => {
            return Err("ledger block index exceeds u64; inbound leg remains ambiguous".to_string())
        }
    };
    set_inbound_leg_status(caller, request_id, leg_index, Status::Confirmed(index))?;
    Ok(index)
}

/// Read the ledger's configured transaction window. Missing metadata fails
/// closed for ambiguous replay; no generic 24-hour value is assumed.
pub(crate) async fn ledger_tx_window(ledger: Principal) -> Option<u64> {
    use icrc_ledger_types::icrc::generic_metadata_value::MetadataValue;
    let result: Result<(Vec<(String, MetadataValue)>,), _> =
        ic_cdk::call(ledger, "icrc1_metadata", ()).await;
    let entries = result.ok()?.0;
    entries.into_iter().find_map(|(k, v)| {
        if k != "icrc1:tx_window" {
            return None;
        }
        match v {
            MetadataValue::Nat(n) => n.0.try_into().ok(),
            _ => None,
        }
    })
}

fn tx_window_allows_retry(created_at_time: u64, now: u64, window: u64) -> bool {
    now <= created_at_time.saturating_add(window)
}

fn inbound_transfer_arg(leg: &crate::state::InboundLeg, spender: Principal) -> TransferFromArgs {
    TransferFromArgs {
        spender_subaccount: None,
        from: Account {
            owner: leg.from,
            subaccount: None,
        },
        to: Account {
            owner: spender,
            subaccount: leg.to_subaccount,
        },
        amount: candid::Nat::from(leg.amount),
        fee: leg.fee.map(candid::Nat::from),
        memo: Some(leg.memo.clone().into()),
        created_at_time: Some(leg.created_at_time),
    }
}

/// Pin the fee and persist the exact outbound transfer identity before any
/// value-moving call. The caller must reserve this before its first external
/// transfer and must only clear it after confirmed success accounting.
pub async fn prepare_transfer_to_user(
    ledger: Principal,
    from_subaccount: [u8; 32],
    to: Principal,
    amount: u128,
    operation_id: String,
) -> Result<u64, String> {
    if let Some(existing) = crate::state::outbound_payout_by_operation(&operation_id) {
        if existing.ledger == ledger
            && existing.from_subaccount == pool_subaccount(ledger, from_subaccount)
            && existing.to == to
            && existing.to_subaccount.is_none()
            && existing.gross_amount == amount
        {
            return Ok(existing.id);
        }
        return Err(format!(
            "outbound operation {} is already bound to a different transfer tuple",
            operation_id
        ));
    }
    let fee = ledger_fee_strict(ledger).await?;
    prepare_transfer_to_user_with_fee(ledger, from_subaccount, to, amount, fee, operation_id)
}

/// Reserve a payout using a fee already pinned in the inbound request. This
/// path intentionally performs no await and cannot silently pick up fee drift.
pub fn prepare_transfer_to_user_with_fee(
    ledger: Principal,
    from_subaccount: [u8; 32],
    to: Principal,
    amount: u128,
    fee: u128,
    operation_id: String,
) -> Result<u64, String> {
    if let Some(existing) = crate::state::outbound_payout_by_operation(&operation_id) {
        if existing.ledger == ledger
            && existing.from_subaccount == pool_subaccount(ledger, from_subaccount)
            && existing.to == to
            && existing.to_subaccount.is_none()
            && existing.gross_amount == amount
            && existing.fee == fee
        {
            return Ok(existing.id);
        }
        return Err(format!(
            "outbound operation {} is already bound to a different transfer tuple",
            operation_id
        ));
    }
    if amount <= fee {
        return Err(format!(
            "amount {} does not exceed ledger fee {}; refusing zero-net payout",
            amount, fee
        ));
    }
    crate::state::reserve_outbound_payout(
        operation_id,
        ledger,
        pool_subaccount(ledger, from_subaccount),
        to,
        None,
        amount,
        fee,
        ic_cdk::api::time(),
    )
}

pub async fn prepare_reward_transfer(
    pool_id: &str,
    to: Principal,
    amount: u128,
    operation_id: String,
) -> Result<u64, String> {
    let ledger = Principal::from_text(crate::ICUSD_LEDGER).expect("invalid icUSD ledger principal");
    if let Some(existing) = crate::state::outbound_payout_by_operation(&operation_id) {
        if existing.ledger == ledger
            && existing.from_subaccount
                == Some(crate::reward_subaccount_for(&pool_id.to_string()))
            && existing.to == to
            && existing.to_subaccount.is_none()
            && existing.gross_amount == amount
        {
            return Ok(existing.id);
        }
        return Err(format!(
            "outbound operation {} is already bound to a different reward tuple",
            operation_id
        ));
    }
    let fee = ledger_fee_strict(ledger).await?;
    if amount <= fee {
        return Err(format!(
            "reward amount {} does not exceed ledger fee {}; refusing zero-net payout",
            amount, fee
        ));
    }
    crate::state::reserve_outbound_payout(
        operation_id,
        ledger,
        Some(crate::reward_subaccount_for(&pool_id.to_string())),
        to,
        None,
        amount,
        fee,
        ic_cdk::api::time(),
    )
}

/// Dispatch only a previously persisted payout. Every call uses the pinned
/// fee, compact 32-byte memo, and timestamp from the durable intent. An error
/// after dispatch remains ambiguous and is never converted into a new payout.
pub async fn dispatch_outbound_payout(id: u64) -> Result<u64, String> {
    use icrc_ledger_types::icrc1::transfer::TransferError;
    let payout = crate::state::outbound_payout(id)?;
    if payout.status != crate::state::OutboundPayoutStatus::Reserved {
        let window = ledger_tx_window(payout.ledger).await.ok_or_else(|| {
            format!(
            "outbound payout {} is ambiguous and ledger transaction window is unavailable; held", id
        )
        })?;
        if ic_cdk::api::time() > payout.created_at_time.saturating_add(window) {
            return Err(format!("outbound payout {} is outside ledger transaction window; held pending exact receipt proof", id));
        }
    }
    crate::state::set_outbound_payout_status(id, crate::state::OutboundPayoutStatus::Dispatched)?;
    let args = outbound_transfer_arg(&payout);

    let result: Result<(Result<candid::Nat, TransferError>,), _> =
        ic_cdk::call(payout.ledger, "icrc1_transfer", (args,)).await;

    match result {
        Ok((Ok(block_index),)) => payout_block_index(id, block_index, "block index"),
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            payout_block_index(id, duplicate_of, "duplicate block index")
        }
        Ok((Err(e),)) => {
            crate::state::set_outbound_payout_status(
                id,
                crate::state::OutboundPayoutStatus::Ambiguous,
            )?;
            Err(format!(
                "icrc1_transfer error (intent {} held): {:?}",
                id, e
            ))
        }
        Err((code, msg)) => {
            crate::state::set_outbound_payout_status(
                id,
                crate::state::OutboundPayoutStatus::Ambiguous,
            )?;
            Err(format!(
                "inter-canister call failed (intent {} held): {:?} - {}",
                id, code, msg
            ))
        }
    }
}

fn outbound_transfer_arg(
    payout: &crate::state::OutboundPayout,
) -> icrc_ledger_types::icrc1::transfer::TransferArg {
    icrc_ledger_types::icrc1::transfer::TransferArg {
        from_subaccount: payout.from_subaccount,
        to: icrc_ledger_types::icrc1::account::Account {
            owner: payout.to,
            subaccount: payout.to_subaccount,
        },
        amount: candid::Nat::from(payout.net_amount),
        fee: Some(candid::Nat::from(payout.fee)),
        memo: Some(payout.memo.clone().into()),
        created_at_time: Some(payout.created_at_time),
    }
}

fn payout_block_index(id: u64, index: candid::Nat, label: &str) -> Result<u64, String> {
    match index.0.try_into() {
        Ok(index) => Ok(index),
        Err(_) => {
            crate::state::set_outbound_payout_status(
                id,
                crate::state::OutboundPayoutStatus::Ambiguous,
            )?;
            Err(format!("{} exceeds u64::MAX; intent {} held", label, id))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{inbound_transfer_arg, outbound_transfer_arg, pool_subaccount};
    use candid::Principal;

    #[test]
    fn threepool_uses_default_account_while_other_ledgers_keep_pool_subaccounts() {
        let threepool = Principal::from_text(crate::THREEPOOL).unwrap();
        let other_ledger = Principal::from_text(crate::ICUSD_LEDGER).unwrap();
        let subaccount = [7; 32];

        assert_eq!(pool_subaccount(threepool, subaccount), None);
        assert_eq!(pool_subaccount(other_ledger, subaccount), Some(subaccount));
    }

    #[test]
    fn outbound_arguments_pin_fee_and_compact_dedup_identity() {
        let owner = Principal::from_text("aaaaa-aa").unwrap();
        let payout = crate::state::OutboundPayout {
            id: 4,
            operation_id: "test".to_string(),
            ledger: owner,
            from: owner,
            from_subaccount: Some([1; 32]),
            to: owner,
            to_subaccount: None,
            gross_amount: 1_000,
            net_amount: 900,
            fee: 100,
            memo: vec![7; 32],
            created_at_time: 123,
            status: crate::state::OutboundPayoutStatus::Reserved,
        };
        let args = outbound_transfer_arg(&payout);
        assert_eq!(args.amount, candid::Nat::from(900u128));
        assert_eq!(args.fee, Some(candid::Nat::from(100u128)));
        assert_eq!(args.memo.as_ref().map(|memo| memo.0.len()), Some(32));
        assert_eq!(args.created_at_time, Some(123));
    }

    #[test]
    fn inbound_arguments_retain_exact_replay_tuple() {
        let owner = Principal::from_text("aaaaa-aa").unwrap();
        let leg = crate::state::InboundLeg {
            ledger: owner,
            from: owner,
            to_subaccount: Some([3; 32]),
            amount: 987,
            fee: Some(5),
            memo: vec![9; 32],
            created_at_time: 1234,
            status: crate::state::InboundLegStatus::Ambiguous,
        };
        let args = inbound_transfer_arg(&leg, owner);
        assert_eq!(args.from.owner, owner);
        assert_eq!(args.to.subaccount, Some([3; 32]));
        assert_eq!(args.amount, candid::Nat::from(987u128));
        assert_eq!(args.fee, Some(candid::Nat::from(5u128)));
        assert_eq!(args.memo.as_ref().map(|m| m.0.to_vec()), Some(vec![9; 32]));
        assert_eq!(args.created_at_time, Some(1234));
    }

    #[test]
    fn ambiguous_replay_is_rejected_after_reported_ledger_window() {
        assert!(super::tx_window_allows_retry(100, 150, 50));
        assert!(!super::tx_window_allows_retry(100, 151, 50));
        assert!(super::tx_window_allows_retry(100, 100, 0));
        assert!(!super::tx_window_allows_retry(100, 101, 0));
    }
}
