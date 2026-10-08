// ICRC-1 / ICRC-2 token transfer helpers for the Rumi 3pool.
//
// Audit Wave-3 (ICRC-003/004): every transfer now sets `created_at_time`
// (so the ledger can dedup retries) and treats `Duplicate { duplicate_of }`
// as success — the previous attempt landed at that block, so the operation
// already succeeded.

use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
use crate::payouts::{
    PayoutAttempt, PayoutEntitlement, PayoutFailure, PayoutJournalEventKind, PayoutKind,
    PayoutOutcome, PayoutTransfer,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// Standard ICRC-1 transfer fee (native units), used as a conservative fallback
/// when a ledger's `icrc1_fee` query cannot be reached. Erring high keeps the
/// pool solvent (we send slightly less) rather than risking an over-send.
const DEFAULT_LEDGER_FEE: u128 = 10_000;
const MAX_PAYOUT_ATTEMPTS: u32 = 16;
const MAX_EXACT_REPLAYS: u8 = 3;

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

/// Refresh the fee after a proven no-effect rejection. Cached quotes are not
/// sufficient evidence for choosing the next transfer tuple.
async fn refresh_ledger_fee(ledger: Principal) -> u128 {
    let result: Result<(candid::Nat,), _> = ic_cdk::call(ledger, "icrc1_fee", ()).await;
    let fee = match result {
        Ok((value,)) => value.0.try_into().unwrap_or(DEFAULT_LEDGER_FEE),
        Err(_) => DEFAULT_LEDGER_FEE,
    };
    LEDGER_FEES.with(|cache| cache.borrow_mut().insert(ledger, fee));
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

/// Create and execute a newly entitled outbound payment. Its exact ICRC-1
/// tuple and entitlement are durable before the first transfer call.
pub async fn payout_to_user(
    kind: PayoutKind,
    token_index: u8,
    ledger: Principal,
    symbol: &str,
    to: Principal,
    gross: u128,
    swap_context: Option<crate::payouts::PayoutSwapContext>,
) -> Result<u64, PayoutFailure> {
    payout_to_user_inner(kind, token_index, ledger, symbol, to, gross, swap_context, None).await
}

/// Create and execute the unique refund entitlement for a proven no-effect
/// swap output. The parent/child link is stable before the first refund call.
pub async fn payout_compensation(
    parent_id: u64,
    token_index: u8,
    ledger: Principal,
    symbol: &str,
    to: Principal,
    gross: u128,
) -> Result<u64, PayoutFailure> {
    payout_to_user_inner(
        PayoutKind::SwapInputRefund,
        token_index,
        ledger,
        symbol,
        to,
        gross,
        None,
        Some(parent_id),
    )
    .await
}

async fn payout_to_user_inner(
    kind: PayoutKind,
    token_index: u8,
    ledger: Principal,
    symbol: &str,
    to: Principal,
    gross: u128,
    swap_context: Option<crate::payouts::PayoutSwapContext>,
    compensation_for: Option<u64>,
) -> Result<u64, PayoutFailure> {
    let fee = ledger_fee(ledger).await;
    if let Some(parent_id) = compensation_for {
        let parent = crate::payouts::get(parent_id).ok_or_else(|| PayoutFailure {
            id: parent_id,
            reason: "swap output entitlement missing; compensation refused".into(),
            ambiguous: true,
        })?;
        if let Some(existing) = parent.compensation_id {
            return Err(PayoutFailure {
                id: existing,
                reason: "swap compensation already exists; recover its exact identity".into(),
                ambiguous: true,
            });
        }
    }
    let id = crate::storage::pending_claims::next_id();
    let mut attempt = make_attempt(id, 0, ledger, to, gross, fee);
    let mut entitlement = PayoutEntitlement {
        id,
        owner: to,
        token_index,
        ledger,
        symbol: symbol.to_string(),
        gross,
        kind,
        swap_context,
        compensation_id: None,
        compensation_for,
        settled: false,
        attempts: vec![attempt.clone()],
    };
    if let Some(parent_id) = compensation_for {
        let mut parent = crate::payouts::get(parent_id).expect("validated compensation parent");
        parent.compensation_id = Some(id);
        crate::payouts::append(
            parent_id,
            PayoutJournalEventKind::CompensationBound { compensation_id: id },
        );
        crate::payouts::save(parent);
    }

    crate::payouts::append(
        id,
        PayoutJournalEventKind::Prepared { attempt: attempt.clone() },
    );
    crate::payouts::save(entitlement.clone());

    if gross <= fee {
        let reason = format!("gross entitlement {gross} does not exceed ledger fee {fee}");
        attempt.outcome = PayoutOutcome::RejectedNoTransfer { reason: reason.clone() };
        entitlement.attempts[0] = attempt.clone();
        crate::payouts::append(
            id,
            PayoutJournalEventKind::RejectedNoTransfer { attempt_number: 0, reason: reason.clone() },
        );
        crate::payouts::save(entitlement);
        return Err(PayoutFailure { id, reason, ambiguous: false });
    }
    let payout_id = entitlement.id;
    execute_payout_attempt(&mut entitlement, 0, false).await?;
    if kind != PayoutKind::SwapOutput {
        crate::payouts::mark_settled(payout_id);
    }
    Ok(payout_id)
}

/// Recover a bound payout claim. Ambiguous attempts replay only the exact same
/// transfer inside a conservative deduplication window. A proven no-effect
/// attempt can start a new fee-bound attempt for the same entitlement.
pub async fn retry_payout_claim(id: u64) -> Result<(), PayoutFailure> {
    let mut entitlement = crate::payouts::get(id).ok_or_else(|| PayoutFailure {
        id,
        reason: "claim has no bound payout identity; held for manual adjudication".into(),
        ambiguous: true,
    })?;
    let latest = entitlement.attempts.last().cloned().ok_or_else(|| PayoutFailure {
        id,
        reason: "payout journal has no transfer attempt".into(),
        ambiguous: true,
    })?;
    match latest.outcome {
        PayoutOutcome::Confirmed { .. } => return Ok(()),
        PayoutOutcome::HeldLegacyUnbound => {
            return Err(PayoutFailure { id, reason: "legacy payout identity is unbound".into(), ambiguous: true });
        }
        PayoutOutcome::Unresolved { .. } | PayoutOutcome::Submitted => {
            let now = ic_cdk::api::time();
            let age = now.saturating_sub(latest.transfer.created_at_time);
            // ICRC's standard retry window is 24h. Leave margin for clock skew.
            if age >= 23 * 60 * 60 * 1_000_000_000 {
                return Err(PayoutFailure { id, reason: "original transfer is unresolved and its deduplication window has expired".into(), ambiguous: true });
            }
            if latest.replay_count >= MAX_EXACT_REPLAYS {
                return Err(PayoutFailure { id, reason: "exact replay limit reached; payout remains unresolved".into(), ambiguous: true });
            }
            return execute_payout_attempt(&mut entitlement, latest.number, true).await;
        }
        PayoutOutcome::Prepared => {
            return execute_payout_attempt(&mut entitlement, latest.number, false).await;
        }
        PayoutOutcome::RejectedNoTransfer { .. } => {}
    }

    let number = latest.number.checked_add(1).ok_or_else(|| PayoutFailure {
        id,
        reason: "payout attempt counter exhausted".into(),
        ambiguous: true,
    })?;
    if number >= MAX_PAYOUT_ATTEMPTS {
        return Err(PayoutFailure { id, reason: "payout attempt limit reached; entitlement remains held".into(), ambiguous: true });
    }
    let fee = refresh_ledger_fee(entitlement.ledger).await;
    let attempt = make_attempt(id, number, entitlement.ledger, entitlement.owner, entitlement.gross, fee);
    entitlement.attempts.push(attempt.clone());
    crate::payouts::append(id, PayoutJournalEventKind::Prepared { attempt });
    crate::payouts::save(entitlement.clone());
    execute_payout_attempt(&mut entitlement, number, false).await
}

fn make_attempt(
    id: u64,
    number: u32,
    ledger: Principal,
    owner: Principal,
    gross: u128,
    fee: u128,
) -> PayoutAttempt {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(b"rumi-3pool-payout-v1");
    digest.update(id.to_be_bytes());
    digest.update(number.to_be_bytes());
    PayoutAttempt {
        number,
        replay_count: 0,
        transfer: PayoutTransfer {
            ledger,
            from: Account { owner: ic_cdk::id(), subaccount: None },
            to: Account { owner, subaccount: None },
            gross,
            net: gross.saturating_sub(fee),
            fee,
            memo: digest.finalize().to_vec(),
            created_at_time: ic_cdk::api::time(),
        },
        outcome: PayoutOutcome::Prepared,
    }
}

async fn execute_payout_attempt(
    entitlement: &mut PayoutEntitlement,
    number: u32,
    preserve_ambiguity: bool,
) -> Result<(), PayoutFailure> {
    use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
    let id = entitlement.id;
    let idx = entitlement.attempts.iter().position(|a| a.number == number).ok_or_else(|| PayoutFailure {
        id,
        reason: "payout attempt is missing".into(),
        ambiguous: true,
    })?;
    let mut attempt = entitlement.attempts[idx].clone();
    if preserve_ambiguity {
        attempt.replay_count = attempt.replay_count.saturating_add(1);
        entitlement.attempts[idx].replay_count = attempt.replay_count;
    }
    let transfer = &attempt.transfer;
    if transfer.gross <= transfer.fee {
        let reason = format!("gross entitlement {} does not exceed ledger fee {}", transfer.gross, transfer.fee);
        entitlement.attempts[idx].outcome = PayoutOutcome::RejectedNoTransfer { reason: reason.clone() };
        crate::payouts::append(id, PayoutJournalEventKind::RejectedNoTransfer { attempt_number: number, reason: reason.clone() });
        crate::payouts::save(entitlement.clone());
        return Err(PayoutFailure { id, reason, ambiguous: false });
    }
    entitlement.attempts[idx].outcome = PayoutOutcome::Submitted;
    crate::payouts::append(id, PayoutJournalEventKind::Submitted {
        attempt_number: number,
        replay_count: attempt.replay_count,
    });
    crate::payouts::save(entitlement.clone());
    if entitlement.kind == PayoutKind::SwapOutput {
        crate::storage::payouts::set_fence_for(id);
    }
    let args = TransferArg {
        from_subaccount: transfer.from.subaccount.clone(),
        to: transfer.to,
        amount: candid::Nat::from(transfer.net),
        fee: Some(candid::Nat::from(transfer.fee)),
        memo: Some(transfer.memo.clone().into()),
        created_at_time: Some(transfer.created_at_time),
    };
    let result: Result<(Result<candid::Nat, TransferError>,), _> =
        ic_cdk::call(transfer.ledger, "icrc1_transfer", (args,)).await;
    let outcome = match result {
        Ok((Ok(block),)) => PayoutOutcome::Confirmed { block: block.clone() },
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            PayoutOutcome::Confirmed { block: duplicate_of.clone() }
        }
        Ok((Err(e),)) => {
            let reason = format!("icrc1_transfer error: {e:?}");
            let no_effect = !matches!(e, TransferError::TemporarilyUnavailable | TransferError::GenericError { .. });
            let state = if no_effect && !preserve_ambiguity {
                PayoutOutcome::RejectedNoTransfer { reason: reason.clone() }
            } else {
                PayoutOutcome::Unresolved { reason: reason.clone() }
            };
            state
        }
        Err((code, message)) => {
            let reason = format!("inter-canister call failed: {code:?} - {message}");
            PayoutOutcome::Unresolved { reason }
        }
    };
    entitlement.attempts[idx].outcome = outcome.clone();
    match outcome {
        PayoutOutcome::Confirmed { block } => {
            crate::payouts::append(id, PayoutJournalEventKind::Confirmed { attempt_number: number, block: block.clone() });
            crate::payouts::save(entitlement.clone());
            let _ = block;
            Ok(())
        }
        PayoutOutcome::RejectedNoTransfer { reason } => {
            crate::payouts::append(id, PayoutJournalEventKind::RejectedNoTransfer { attempt_number: number, reason: reason.clone() });
            crate::payouts::save(entitlement.clone());
            Err(PayoutFailure { id, reason, ambiguous: false })
        }
        PayoutOutcome::Unresolved { reason } => {
            crate::payouts::append(id, PayoutJournalEventKind::Unresolved { attempt_number: number, reason: reason.clone() });
            crate::payouts::save(entitlement.clone());
            Err(PayoutFailure { id, reason, ambiguous: true })
        }
        _ => Err(PayoutFailure { id, reason: "invalid payout transition".into(), ambiguous: true }),
    }
}
