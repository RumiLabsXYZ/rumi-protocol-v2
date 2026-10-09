// ICRC-1 / ICRC-2 token transfer helpers for the Rumi 3pool.
//
// Audit Wave-3 (ICRC-003/004): every transfer now sets `created_at_time`
// (so the ledger can dedup retries) and treats `Duplicate { duplicate_of }`
// as confirmation only when replaying a persisted exact tuple.

use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;
use crate::payouts::{
    PayoutAttempt, PayoutEntitlement, PayoutFailure, PayoutJournalEventKind, PayoutKind,
    PayoutOutcome, PayoutTransfer,
};
use std::cell::RefCell;
use std::collections::HashMap;

const MAX_PAYOUT_ATTEMPTS: u32 = 16;
const MAX_EXACT_REPLAYS: u8 = 3;

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

/// Fetch a ledger's transfer fee, caching only successful queries. A guessed
/// fee cannot safely bind a transfer or a recoverable payout identity.
pub async fn ledger_fee(ledger: Principal) -> Result<u128, String> {
    if let Some(fee) = LEDGER_FEES.with(|c| c.borrow().get(&ledger).copied()) {
        return Ok(fee);
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
    let fee = try_current_ledger_fee(ledger).await?;
    LEDGER_FEES.with(|c| c.borrow_mut().insert(ledger, fee));
    Ok(fee)
}

/// Query a fee for an admission decision that cannot safely use a cached fee.
/// This never consults or updates the fee cache and fails closed on rejection
/// or values outside `u128`.
pub async fn try_current_ledger_fee(ledger: Principal) -> Result<u128, String> {
    let result: Result<(candid::Nat,), _> = ic_cdk::call(ledger, "icrc1_fee", ()).await;
    let (fee,) = result.map_err(|(code, message)| {
        format!("icrc1_fee query failed: {code:?} - {message}")
    })?;
    fee.0
        .try_into()
        .map_err(|_| "icrc1_fee result does not fit u128".to_string())
}

/// Refresh the fee after a proven no-effect rejection. Cached quotes are not
/// sufficient evidence for choosing the next transfer tuple.
async fn refresh_ledger_fee(ledger: Principal) -> Result<u128, String> {
    let fee = try_current_ledger_fee(ledger).await?;
    LEDGER_FEES.with(|cache| cache.borrow_mut().insert(ledger, fee));
    Ok(fee)
}

/// Persist a payout identity without dispatching it. Multi-leg operations use
/// this for every leg before their first outbound ledger await.
pub fn prepare_payout(
    kind: PayoutKind,
    token_index: u8,
    ledger: Principal,
    symbol: &str,
    to: Principal,
    gross: u128,
    fee: u128,
    dispatch_ready: bool,
) -> Result<u64, PayoutFailure> {
    Ok(prepare_payout_with_fee(kind, token_index, ledger, symbol, to, gross, None, None, fee, dispatch_ready)?.id)
}

pub fn prepare_swap_output(
    token_index: u8,
    ledger: Principal,
    symbol: &str,
    to: Principal,
    gross: u128,
    fee: u128,
    context: crate::payouts::PayoutSwapContext,
) -> Result<u64, PayoutFailure> {
    Ok(prepare_payout_with_fee(
        PayoutKind::SwapOutput, token_index, ledger, symbol, to, gross,
        Some(context), None, fee, false,
    )?.id)
}

/// Reserve both sides of an add-liquidity leg before its ICRC2 pull. The
/// inbound tuple is persisted with the held refund so a lost callback can be
/// reconciled by exact replay before any compensation is authorized.
pub async fn prepare_add_liquidity_refund(
    token_index: u8,
    ledger: Principal,
    symbol: &str,
    owner: Principal,
    gross: u128,
    fee: u128,
) -> Result<u64, PayoutFailure> {
    prepare_input_payout_with_fee(PayoutKind::AddLiquidityRefund, crate::payouts::PayoutInputAction::AddLiquidity, token_index, ledger, symbol, owner, gross, fee)
}

/// Reserve an inbound transfer and its potential outbound recovery before the
/// inbound call. Used by add-liquidity and ordinary swaps.
pub async fn prepare_input_payout(
    kind: PayoutKind,
    action: crate::payouts::PayoutInputAction,
    token_index: u8,
    ledger: Principal,
    symbol: &str,
    owner: Principal,
    gross: u128,
) -> Result<u64, PayoutFailure> {
    let fee = ledger_fee(ledger).await.map_err(|reason| PayoutFailure {
        id: 0,
        reason: format!("fee lookup failed before an input recovery identity was created: {reason}"),
        ambiguous: false,
    })?;
    prepare_input_payout_with_fee(kind, action, token_index, ledger, symbol, owner, gross, fee)
}

fn prepare_input_payout_with_fee(
    kind: PayoutKind,
    action: crate::payouts::PayoutInputAction,
    token_index: u8,
    ledger: Principal,
    symbol: &str,
    owner: Principal,
    gross: u128,
    fee: u128,
) -> Result<u64, PayoutFailure> {
    let id = prepare_payout_with_fee(
        kind, token_index, ledger, symbol, owner, gross,
        None, None, fee, false,
    )?.id;
    // ICRC-2 deduplication can key on (caller, from, to, amount, fee, memo,
    // created_at_time). IC time is not guaranteed to advance between two
    // sequential calls in one message round, so persist a per-entitlement memo
    // to ensure separate same-round operations cannot be mistaken as one pull.
    use sha2::{Digest, Sha256};
    let mut input_memo = Sha256::new();
    input_memo.update(b"rumi-3pool-input-v1");
    input_memo.update(id.to_be_bytes());
    let mut entitlement = crate::payouts::get(id).expect("just persisted payout reservation");
    entitlement.input_transfer = Some(crate::payouts::PayoutInputTransfer {
        ledger,
        from: Account { owner, subaccount: None },
        to: Account { owner: ic_cdk::id(), subaccount: None },
        amount: gross,
        memo: Some(input_memo.finalize().to_vec()),
        created_at_time: ic_cdk::api::time(),
    });
    entitlement.input_outcome = Some(crate::payouts::PayoutInputOutcome::Prepared);
    entitlement.input_action = Some(action);
    crate::payouts::save(entitlement);
    Ok(id)
}

/// Bind one precreated refund to its output entitlement before any dispatch.
pub fn bind_prepared_compensation(parent_id: u64, child_id: u64) -> Result<(), PayoutFailure> {
    let mut parent = crate::payouts::get(parent_id).ok_or_else(|| PayoutFailure {
        id: parent_id, reason: "output entitlement missing".into(), ambiguous: true,
    })?;
    let mut child = crate::payouts::get(child_id).ok_or_else(|| PayoutFailure {
        id: child_id, reason: "refund entitlement missing".into(), ambiguous: true,
    })?;
    if parent.compensation_id.is_some() || child.compensation_for.is_some() || parent.owner != child.owner {
        return Err(PayoutFailure { id: parent_id, reason: "invalid compensation binding".into(), ambiguous: true });
    }
    parent.compensation_id = Some(child_id);
    child.compensation_for = Some(parent_id);
    crate::payouts::save(parent);
    crate::payouts::save(child);
    Ok(())
}

/// Dispatch one previously pinned ICRC2 pull and persist its exact result.
pub async fn execute_pinned_input(id: u64) -> Result<(), String> {
    use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
    let mut entitlement = crate::payouts::get(id).ok_or_else(|| "input reservation missing".to_string())?;
    let input = entitlement.input_transfer.clone().ok_or_else(|| "input identity missing".to_string())?;
    if !matches!(entitlement.input_outcome, Some(crate::payouts::PayoutInputOutcome::Prepared)) {
        return Err("input request is not in Prepared state".into());
    }
    entitlement.input_outcome = Some(crate::payouts::PayoutInputOutcome::Submitted);
    // PAYOUT_CURRENT is the pre-call durable journal: persist the full exact
    // tuple and Submitted state before awaiting the ledger. Only confirmed or
    // ambiguous outcomes need an append-only evidence row.
    crate::payouts::save(entitlement.clone());
    let args = TransferFromArgs {
        spender_subaccount: None,
        from: input.from,
        to: input.to,
        amount: candid::Nat::from(input.amount),
        fee: None,
        memo: input.memo.clone().map(Into::into),
        created_at_time: Some(input.created_at_time),
    };
    let result: Result<(Result<candid::Nat, TransferFromError>,), _> =
        ic_cdk::call(input.ledger, "icrc2_transfer_from", (args,)).await;
    record_input_result(&mut entitlement, crate::payouts::PayoutInputOutcome::Submitted, result, false)
}

/// Resolve a submitted exact ICRC2 identity. Duplicate confirms the original
/// pull. Any error after a prior submission stays held because ledger duplicate
/// detection order is not a portable proof that the original request had no effect.
pub async fn reconcile_pinned_input(id: u64) -> Result<(), String> {
    use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
    let mut entitlement = crate::payouts::get(id).ok_or_else(|| "input reservation missing".to_string())?;
    let input = entitlement.input_transfer.clone().ok_or_else(|| "input identity missing".to_string())?;
    if !matches!(entitlement.input_outcome, Some(crate::payouts::PayoutInputOutcome::Submitted | crate::payouts::PayoutInputOutcome::Unresolved { .. })) {
        return Err("input request is not unresolved".into());
    }
    // Persist that this exact replay is in flight. The callback accepts only
    // this tuple/outcome pair, preventing a stale callback from overwriting a
    // newer settled or otherwise reconciled entitlement.
    entitlement.input_outcome = Some(crate::payouts::PayoutInputOutcome::Submitted);
    crate::payouts::save(entitlement.clone());
    let args = TransferFromArgs {
        spender_subaccount: None,
        from: input.from,
        to: input.to,
        amount: candid::Nat::from(input.amount),
        fee: None,
        memo: input.memo.clone().map(Into::into),
        created_at_time: Some(input.created_at_time),
    };
    let result: Result<(Result<candid::Nat, TransferFromError>,), _> =
        ic_cdk::call(input.ledger, "icrc2_transfer_from", (args,)).await;
    record_input_result(&mut entitlement, crate::payouts::PayoutInputOutcome::Submitted, result, true)
}

fn record_input_result(
    entitlement: &mut PayoutEntitlement,
    expected_outcome: crate::payouts::PayoutInputOutcome,
    result: Result<(Result<candid::Nat, icrc_ledger_types::icrc2::transfer_from::TransferFromError>,), (ic_cdk::api::call::RejectionCode, String)>,
    preserve_ambiguity: bool,
) -> Result<(), String> {
    use icrc_ledger_types::icrc2::transfer_from::TransferFromError;
    let mut current = crate::payouts::get(entitlement.id)
        .ok_or_else(|| "input identity disappeared while ledger call was in flight".to_string())?;
    if current.input_transfer != entitlement.input_transfer {
        return Err("input identity changed while ledger call was in flight".into());
    }
    let state = match result {
        Ok((Ok(block),)) => crate::payouts::PayoutInputOutcome::Confirmed { block },
        Ok((Err(TransferFromError::Duplicate { duplicate_of }),)) => crate::payouts::PayoutInputOutcome::Confirmed { block: duplicate_of },
        Ok((Err(error),)) if !preserve_ambiguity && !matches!(error, TransferFromError::GenericError { .. }) => crate::payouts::PayoutInputOutcome::RejectedNoTransfer { reason: format!("icrc2_transfer_from error: {error:?}") },
        Ok((Err(error),)) => crate::payouts::PayoutInputOutcome::Unresolved { reason: format!("icrc2_transfer_from error on unresolved replay: {error:?}") },
        Err((code, message)) => crate::payouts::PayoutInputOutcome::Unresolved { reason: format!("call failed: {code:?} - {message}") },
    };
    let Some(current_outcome) = current.input_outcome.as_ref() else {
        return Err("input outcome disappeared while ledger call was in flight".into());
    };
    if matches!(current_outcome, crate::payouts::PayoutInputOutcome::Prepared) {
        return Err("stale input callback cannot replace a prepared identity".into());
    }
    if current_outcome != &expected_outcome
        && !matches!(&state, crate::payouts::PayoutInputOutcome::Confirmed { .. } | crate::payouts::PayoutInputOutcome::Unresolved { .. })
    {
        return Err("stale no-effect input callback cannot overwrite a newer outcome".into());
    }
    let state = merge_input_outcome(current_outcome, state);
    current.input_outcome = Some(state.clone());
    match state {
        crate::payouts::PayoutInputOutcome::Confirmed { block } => {
            crate::payouts::save(current.clone());
            crate::payouts::append(current.id, PayoutJournalEventKind::InputConfirmed { block });
            *entitlement = current;
            Ok(())
        }
        crate::payouts::PayoutInputOutcome::RejectedNoTransfer { reason } => {
            crate::payouts::save(current.clone());
            *entitlement = current;
            Err(reason)
        }
        crate::payouts::PayoutInputOutcome::Unresolved { reason } => {
            crate::payouts::save(current.clone());
            crate::payouts::append(current.id, PayoutJournalEventKind::InputUnresolved { reason: reason.clone() });
            *entitlement = current;
            Err(reason)
        }
        _ => Err("invalid input transfer transition".into()),
    }
}

fn merge_input_outcome(
    current: &crate::payouts::PayoutInputOutcome,
    incoming: crate::payouts::PayoutInputOutcome,
) -> crate::payouts::PayoutInputOutcome {
    use crate::payouts::PayoutInputOutcome as Outcome;
    match (current, incoming) {
        (Outcome::Confirmed { .. }, _) => current.clone(),
        (_, confirmed @ Outcome::Confirmed { .. }) => confirmed,
        (Outcome::Unresolved { .. }, Outcome::RejectedNoTransfer { .. }) => current.clone(),
        (Outcome::RejectedNoTransfer { .. }, unresolved @ Outcome::Unresolved { .. }) => unresolved,
        (_, next) => next,
    }
}

fn merge_payout_outcome(current: &PayoutOutcome, incoming: PayoutOutcome) -> PayoutOutcome {
    match (current, incoming) {
        (PayoutOutcome::Confirmed { .. }, _) => current.clone(),
        (_, confirmed @ PayoutOutcome::Confirmed { .. }) => confirmed,
        (PayoutOutcome::Unresolved { .. }, PayoutOutcome::RejectedNoTransfer { .. }) => current.clone(),
        (PayoutOutcome::RejectedNoTransfer { .. }, unresolved @ PayoutOutcome::Unresolved { .. }) => unresolved,
        (_, next) => next,
    }
}

/// Activate a previously reserved payout only after the operation proves the
/// leg is owed. Persist the authorization before the ledger call.
pub fn activate_prepared_payout(id: u64) -> Result<(), PayoutFailure> {
    let mut entitlement = crate::payouts::get(id).ok_or_else(|| PayoutFailure {
        id, reason: "reserved payout identity is missing".into(), ambiguous: true,
    })?;
    if entitlement.dispatch_ready == Some(true) { return Ok(()); }
    entitlement.dispatch_ready = Some(true);
    crate::payouts::save(entitlement);
    Ok(())
}

/// Close a reserved payout that the operation proves is not required.
pub fn cancel_prepared_payout(id: u64, _reason: &str) -> Result<(), PayoutFailure> {
    let mut entitlement = crate::payouts::get(id).ok_or_else(|| PayoutFailure {
        id, reason: "reserved payout identity is missing".into(), ambiguous: true,
    })?;
    if entitlement.dispatch_ready == Some(true) || entitlement.settled {
        return Err(PayoutFailure { id, reason: "cannot cancel an activated payout".into(), ambiguous: true });
    }
    if matches!(entitlement.input_outcome, Some(crate::payouts::PayoutInputOutcome::Confirmed { .. })) {
        // A confirmed ingress is value-moving evidence and stays owner-visible,
        // even when the reserved refund was not required.
        return if crate::payouts::mark_settled(id) {
            Ok(())
        } else {
            Err(PayoutFailure { id, reason: "could not close confirmed input identity".into(), ambiguous: true })
        };
    }
    if matches!(entitlement.input_outcome, Some(crate::payouts::PayoutInputOutcome::Unresolved { .. } | crate::payouts::PayoutInputOutcome::Submitted)) {
        return Err(PayoutFailure { id, reason: "cannot discard unresolved input identity".into(), ambiguous: true });
    }
    // Definitive no-effect details are intentionally not copied to the
    // append-only journal; the consumed ID and memo are never reused.
    crate::payouts::remove_no_effect(&entitlement);
    Ok(())
}

/// Dispatch a previously persisted, ready payout tuple.
pub async fn execute_prepared_payout(id: u64) -> Result<(), PayoutFailure> {
    let mut entitlement = crate::payouts::get(id).ok_or_else(|| PayoutFailure {
        id, reason: "prepared payout identity is missing".into(), ambiguous: true,
    })?;
    if entitlement.dispatch_ready == Some(false) {
        return Err(PayoutFailure { id, reason: "payout is reserved but not yet authorized for dispatch".into(), ambiguous: true });
    }
    let attempt_number = entitlement.attempts.last().map(|attempt| attempt.number).ok_or_else(|| PayoutFailure {
        id, reason: "payout has no persisted transfer tuple".into(), ambiguous: true,
    })?;
    execute_payout_attempt(&mut entitlement, attempt_number, false).await
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
    let fee = ledger_fee(ledger).await.map_err(|reason| PayoutFailure {
        id: compensation_for.unwrap_or(0),
        reason: format!("fee lookup failed before a payout identity was created: {reason}"),
        ambiguous: false,
    })?;
    let entitlement = prepare_payout_with_fee(kind, token_index, ledger, symbol, to, gross, swap_context, compensation_for, fee, true)?;
    execute_prepared_payout(entitlement.id).await?;
    if kind != PayoutKind::SwapOutput {
        crate::payouts::mark_settled(entitlement.id);
    }
    Ok(entitlement.id)
}

fn prepare_payout_with_fee(
    kind: PayoutKind,
    token_index: u8,
    ledger: Principal,
    symbol: &str,
    to: Principal,
    gross: u128,
    swap_context: Option<crate::payouts::PayoutSwapContext>,
    compensation_for: Option<u64>,
    fee: u128,
    dispatch_ready: bool,
) -> Result<PayoutEntitlement, PayoutFailure> {
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
        dispatch_ready: Some(dispatch_ready),
        input_transfer: None,
        input_outcome: None,
        input_action: None,
        settled: false,
        attempts: vec![attempt.clone()],
    };
    let mut compensation_parent = None;
    if let Some(parent_id) = compensation_for {
        let mut parent = crate::payouts::get(parent_id).expect("validated compensation parent");
        parent.compensation_id = Some(id);
        compensation_parent = Some((parent_id, parent));
    }

    // Store the exact tuple in the stable current projection before any call.
    // Keeping Prepared payloads in StableLog made permissionless rejected
    // operations permanently consume append-only storage.
    crate::payouts::save(entitlement.clone());
    if let Some((parent_id, parent)) = compensation_parent {
        crate::payouts::save(parent);
        crate::payouts::append(
            parent_id,
            PayoutJournalEventKind::CompensationBound { compensation_id: id },
        );
    }

    if gross <= fee && dispatch_ready {
        let reason = format!("gross entitlement {gross} does not exceed ledger fee {fee}");
        attempt.outcome = PayoutOutcome::RejectedNoTransfer { reason: reason.clone() };
        entitlement.attempts[0] = attempt.clone();
        crate::payouts::save(entitlement);
        return Err(PayoutFailure { id, reason, ambiguous: false });
    }
    Ok(entitlement)
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
    if entitlement.dispatch_ready == Some(false) {
        return Err(PayoutFailure { id, reason: "payout reservation is not yet authorized for dispatch".into(), ambiguous: true });
    }
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
    let fee = refresh_ledger_fee(entitlement.ledger).await.map_err(|reason| PayoutFailure {
        id,
        reason: format!("cannot rearm payout without a verified current fee: {reason}"),
        ambiguous: false,
    })?;
    let attempt = make_attempt(id, number, entitlement.ledger, entitlement.owner, entitlement.gross, fee);
    entitlement.attempts.push(attempt.clone());
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
        crate::payouts::save(entitlement.clone());
        return Err(PayoutFailure { id, reason, ambiguous: false });
    }
    entitlement.attempts[idx].outcome = PayoutOutcome::Submitted;
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
    let mut current = crate::payouts::get(id).ok_or_else(|| PayoutFailure {
        id, reason: "payout identity disappeared while ledger call was in flight".into(), ambiguous: true,
    })?;
    let Some(current_attempt) = current.attempts.get(idx).cloned() else {
        return Err(PayoutFailure { id, reason: "payout attempt disappeared while ledger call was in flight".into(), ambiguous: true });
    };
    if current_attempt.transfer != attempt.transfer {
        return Err(PayoutFailure { id, reason: "payout identity changed while ledger call was in flight".into(), ambiguous: true });
    }
    if current_attempt.replay_count != attempt.replay_count
        && !matches!(&outcome, PayoutOutcome::Confirmed { .. } | PayoutOutcome::Unresolved { .. })
    {
        return Err(PayoutFailure { id, reason: "stale no-effect payout callback cannot overwrite a newer replay".into(), ambiguous: true });
    }
    let merged = merge_payout_outcome(&current_attempt.outcome, outcome);
    current.attempts[idx].outcome = merged.clone();
    match merged {
        PayoutOutcome::Confirmed { block } => {
            crate::payouts::save(current.clone());
            if !matches!(&current_attempt.outcome, PayoutOutcome::Confirmed { .. }) {
                crate::payouts::append(id, PayoutJournalEventKind::Confirmed { attempt_number: number, block: block.clone() });
            }
            *entitlement = current;
            if entitlement.attempts.iter().any(|later| {
                later.number > number
                    && matches!(&later.outcome, PayoutOutcome::Submitted | PayoutOutcome::Unresolved { .. })
            }) {
                Err(PayoutFailure { id, reason: "an earlier payout is confirmed while a newer exact identity remains unresolved".into(), ambiguous: true })
            } else {
                Ok(())
            }
        }
        PayoutOutcome::RejectedNoTransfer { reason } => {
            crate::payouts::save(current.clone());
            *entitlement = current;
            Err(PayoutFailure { id, reason, ambiguous: false })
        }
        PayoutOutcome::Unresolved { reason } => {
            crate::payouts::save(current.clone());
            crate::payouts::append(id, PayoutJournalEventKind::Unresolved { attempt_number: number, reason: reason.clone() });
            *entitlement = current;
            Err(PayoutFailure { id, reason, ambiguous: true })
        }
        _ => Err(PayoutFailure { id, reason: "invalid payout transition".into(), ambiguous: true }),
    }
}

#[cfg(test)]
mod outcome_merge_tests {
    use super::{merge_input_outcome, merge_payout_outcome};
    use crate::payouts::{PayoutInputOutcome, PayoutOutcome};
    use candid::Nat;

    #[test]
    fn a_late_confirmed_input_callback_upgrades_unresolved_and_is_never_downgraded() {
        let submitted = PayoutInputOutcome::Submitted;
        let unresolved = PayoutInputOutcome::Unresolved { reason: "callback B".into() };
        let confirmed = PayoutInputOutcome::Confirmed { block: Nat::from(7u8) };
        let after_b = merge_input_outcome(&submitted, unresolved);
        assert!(matches!(after_b, PayoutInputOutcome::Unresolved { .. }));
        assert_eq!(merge_input_outcome(&after_b, confirmed.clone()), confirmed);

        let already_confirmed = PayoutInputOutcome::Confirmed { block: Nat::from(8u8) };
        let late_error = PayoutInputOutcome::Unresolved { reason: "late error".into() };
        assert_eq!(merge_input_outcome(&already_confirmed, late_error), already_confirmed);
    }

    #[test]
    fn a_late_confirmed_payout_replay_upgrades_unresolved_same_tuple() {
        let submitted = PayoutOutcome::Submitted;
        let unresolved = PayoutOutcome::Unresolved { reason: "callback B".into() };
        let confirmed = PayoutOutcome::Confirmed { block: Nat::from(9u8) };
        let after_b = merge_payout_outcome(&submitted, unresolved);
        assert!(matches!(after_b, PayoutOutcome::Unresolved { .. }));
        assert_eq!(merge_payout_outcome(&after_b, confirmed.clone()), confirmed);

        let late_error = PayoutOutcome::Unresolved { reason: "late error".into() };
        assert_eq!(merge_payout_outcome(&confirmed, late_error), confirmed);
    }
}
