use crate::event::{
    record_withdraw_liquidity_at,
};
use crate::guard::GuardPrincipal;
use crate::logs::INFO;
use crate::management::{
    mint_icusd_with_tuple, DurableMintOutcome,
};
use crate::{mutate_state, read_state, ProtocolError, ICUSD, MIN_LIQUIDITY_AMOUNT};
use ic_canister_log::log;
use icrc_ledger_types::icrc1::transfer::TransferError;

pub async fn provide_liquidity(_amount: u64) -> Result<u64, ProtocolError> {
    // The legacy route pulled icUSD before journaling the contribution and
    // allocated a fresh ledger nonce on each retry. Keep old Candid callers
    // fail-closed until a durable ingress/reconciliation path exists.
    Err(ProtocolError::TemporarilyUnavailable(
        "New deposits to the legacy liquidity pool are disabled. Existing liquidity can still be withdrawn; historical ICP return claims are held for safe reconciliation. Do not retry an earlier deposit with an uncertain outcome; contact support for reconciliation.".into(),
    ))
}

pub async fn withdraw_liquidity(amount: u64) -> Result<u64, ProtocolError> {
    // Keep the old Candid method for compatibility. A backend release that
    // disables it must ship with the frontend using the keyed endpoint; older
    // served clients will receive this explicit error until their asset sync.
    let _ = amount;
    Err(ProtocolError::GenericError("This withdrawal method has no retry identity and is disabled. Use withdraw_liquidity_with_id with a monotonically increasing request_id, then query get_my_liquidity_withdrawal_status if the result is lost.".into()))
}

pub async fn withdraw_liquidity_with_id(
    request_id: u128,
    amount_e8s: u64,
) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let _guard_principal = GuardPrincipal::new(caller, "withdraw_liquidity")?;
    let amount = ICUSD::from(amount_e8s);
    let journal = match prepare_liquidity_withdrawal(caller, request_id, amount_e8s)? {
        PrepareWithdrawal::Completed(block) => return Ok(block),
        PrepareWithdrawal::Rejected => return Err(ProtocolError::GenericError("This withdrawal request was rejected without a mint; use a higher request_id for a new attempt.".into())),
        PrepareWithdrawal::ReceiptRecoveryRequired => return Err(ProtocolError::GenericError("This withdrawal is awaiting exact positive ICRC-3 receipt recovery; no new mint will be sent.".into())),
        PrepareWithdrawal::Dispatch(journal) => journal,
    };
    let journal = record_dispatch_attempt(caller, &journal)?;
    match mint_icusd_with_tuple(&journal.tuple).await {
        DurableMintOutcome::Confirmed(block_index) => {
            let commit_time = ic_cdk::api::time();
            let committed = mutate_state(|s| commit_liquidity_withdrawal_in_state(s, &journal, block_index, commit_time));
            if !committed { return Err(ProtocolError::GenericError(format!("icUSD mint for liquidity withdrawal {request_id} was confirmed at block {block_index}, but the matching journal could not be committed; operator reconciliation is required"))); }
            log!(INFO, "[withdraw_liquidity] {caller} withdrew {amount} (request {request_id})");
            Ok(block_index)
        }
        DurableMintOutcome::ConfirmedBlockOutOfRange => {
            hold_for_receipt_recovery(caller, request_id);
            Err(ProtocolError::GenericError(format!("icUSD ledger confirmed withdrawal {request_id} at a block index outside the supported range; the withdrawal remains held for operator reconciliation. The u64 receipt endpoint cannot represent this block index, so no new mint will be sent.")))
        }
        DurableMintOutcome::Rejected(TransferError::TooOld) => {
            hold_for_receipt_recovery(caller, request_id);
            Err(ProtocolError::GenericError(format!("icUSD ledger returned TooOld for withdrawal {request_id}; prior outcome is ambiguous. Find the exact ICRC-3 mint receipt and call reconcile_liquidity_withdrawal_from_block. No new mint will be sent.")))
        }
        DurableMintOutcome::Rejected(error) if liquidity_mint_definitely_rejected(&error) => {
            match mutate_state(|s| reject_definite_attempt_in_state(s, caller, request_id)) {
                DefiniteAttemptResult::RejectedNoEffect => Err(ProtocolError::TransferError(error)),
                DefiniteAttemptResult::AlreadyCompleted(block_index) => Ok(block_index),
                DefiniteAttemptResult::Held => Err(ProtocolError::GenericError(format!("retry of withdrawal {request_id} was rejected, but an earlier dispatch may have minted. The journal remains held for exact ICRC-3 receipt recovery."))),
            }
        }
        DurableMintOutcome::Rejected(error) => Err(ProtocolError::GenericError(format!("liquidity withdrawal {request_id} has an unresolved mint outcome; retry this same request_id and amount to resubmit the pinned tuple. Ledger response: {error:?}"))),
    }
}

#[derive(Debug)]
enum PrepareWithdrawal {
    Dispatch(crate::state::LiquidityWithdrawJournal),
    Completed(u64),
    ReceiptRecoveryRequired,
    Rejected,
}

fn prepare_liquidity_withdrawal(
    caller: candid::Principal,
    request_id: u128,
    amount_e8s: u64,
) -> Result<PrepareWithdrawal, ProtocolError> {
    let now = ic_cdk::api::time();
    mutate_state(|s| prepare_liquidity_withdrawal_in_state(s, caller, request_id, amount_e8s, now))
}

fn prepare_liquidity_withdrawal_in_state(
    s: &mut crate::state::State,
    caller: candid::Principal,
    request_id: u128,
    amount_e8s: u64,
    now: u64,
) -> Result<PrepareWithdrawal, ProtocolError> {
    if let Some(existing) = s.liquidity_withdraw_journals.get(&caller).cloned() {
        if existing.request_id == request_id {
            if existing.amount_e8s != amount_e8s {
                return Err(ProtocolError::GenericError(
                    "request_id is already bound to a different withdrawal amount".into(),
                ));
            }
            return Ok(match existing.phase {
                crate::state::LiquidityWithdrawPhase::Completed { block_index } => {
                    PrepareWithdrawal::Completed(block_index)
                }
                crate::state::LiquidityWithdrawPhase::ReceiptRecoveryRequired => {
                    PrepareWithdrawal::ReceiptRecoveryRequired
                }
                crate::state::LiquidityWithdrawPhase::RejectedNoEffect => {
                    PrepareWithdrawal::Rejected
                }
                crate::state::LiquidityWithdrawPhase::SubmittedOrUnknown => {
                    PrepareWithdrawal::Dispatch(existing)
                }
            });
        }
        if !matches!(
            existing.phase,
            crate::state::LiquidityWithdrawPhase::Completed { .. }
                | crate::state::LiquidityWithdrawPhase::RejectedNoEffect
        ) {
            return Err(ProtocolError::GenericError(format!("withdrawal {} is still unresolved; finish that exact request before starting another", existing.request_id)));
        }
        if request_id <= existing.request_id {
            return Err(ProtocolError::GenericError("request_id must be greater than the latest recorded withdrawal ID; older IDs are permanently rejected".into()));
        }
    }
    let provided = s.liquidity_pool.get(&caller).copied().unwrap_or_default();
    if provided == 0 {
        return Err(ProtocolError::GenericError(
            "You have no provided liquidity to withdraw".into(),
        ));
    }
    if ICUSD::from(amount_e8s) > provided {
        return Err(ProtocolError::GenericError(format!(
            "cannot withdraw: {} e8s, provided: {provided}",
            amount_e8s
        )));
    }
    // The minimum limits partial exits, but must not strand a smaller final
    // balance (including historical fee credits). This only applies to a new
    // request: an exact journal replay above is independent of today's balance.
    if amount_e8s < MIN_LIQUIDITY_AMOUNT.to_u64() && ICUSD::from(amount_e8s) != provided {
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: MIN_LIQUIDITY_AMOUNT.to_u64(),
        });
    }
    let op_nonce = s.next_op_nonce_at(now);
    let journal = crate::state::LiquidityWithdrawJournal {
        owner: caller,
        request_id,
        amount_e8s,
        tuple: crate::state::BorrowMintTuple {
            ledger: s.icusd_ledger_principal,
            destination: caller,
            amount_e8s,
            memo: op_nonce.to_be_bytes(),
            created_at_time_ns: crate::management::nonce_to_created_at_time(op_nonce),
            op_nonce,
        },
        attempt_count: 0,
        phase: crate::state::LiquidityWithdrawPhase::SubmittedOrUnknown,
    };
    s.liquidity_withdraw_journals
        .insert(caller, journal.clone());
    Ok(PrepareWithdrawal::Dispatch(journal))
}

fn record_dispatch_attempt(
    caller: candid::Principal,
    journal: &crate::state::LiquidityWithdrawJournal,
) -> Result<crate::state::LiquidityWithdrawJournal, ProtocolError> {
    mutate_state(|s| record_dispatch_attempt_in_state(s, caller, journal))
}

fn record_dispatch_attempt_in_state(
    s: &mut crate::state::State,
    caller: candid::Principal,
    journal: &crate::state::LiquidityWithdrawJournal,
) -> Result<crate::state::LiquidityWithdrawJournal, ProtocolError> {
    let Some(current) = s.liquidity_withdraw_journals.get_mut(&caller) else {
        return Err(ProtocolError::TemporarilyUnavailable(
            "withdrawal journal disappeared before mint dispatch".into(),
        ));
    };
    if current.request_id != journal.request_id
        || current.amount_e8s != journal.amount_e8s
        || current.phase != crate::state::LiquidityWithdrawPhase::SubmittedOrUnknown
    {
        return Err(ProtocolError::TemporarilyUnavailable(
            "withdrawal journal changed before mint dispatch".into(),
        ));
    }
    current.attempt_count = current.attempt_count.checked_add(1).ok_or_else(|| {
        ProtocolError::GenericError(
            "withdrawal dispatch counter exhausted; journal remains held".into(),
        )
    })?;
    Ok(current.clone())
}

#[derive(Debug, PartialEq, Eq)]
enum DefiniteAttemptResult {
    RejectedNoEffect,
    AlreadyCompleted(u64),
    Held,
}

fn reject_definite_attempt_in_state(
    s: &mut crate::state::State,
    caller: candid::Principal,
    request_id: u128,
) -> DefiniteAttemptResult {
    let Some(current) = s.liquidity_withdraw_journals.get_mut(&caller) else {
        return DefiniteAttemptResult::Held;
    };
    if current.request_id != request_id {
        return DefiniteAttemptResult::Held;
    }
    match current.phase.clone() {
        crate::state::LiquidityWithdrawPhase::Completed { block_index } => {
            DefiniteAttemptResult::AlreadyCompleted(block_index)
        }
        crate::state::LiquidityWithdrawPhase::RejectedNoEffect => {
            DefiniteAttemptResult::RejectedNoEffect
        }
        crate::state::LiquidityWithdrawPhase::ReceiptRecoveryRequired => {
            DefiniteAttemptResult::Held
        }
        crate::state::LiquidityWithdrawPhase::SubmittedOrUnknown if current.attempt_count == 1 => {
            current.phase = crate::state::LiquidityWithdrawPhase::RejectedNoEffect;
            DefiniteAttemptResult::RejectedNoEffect
        }
        crate::state::LiquidityWithdrawPhase::SubmittedOrUnknown => {
            current.phase = crate::state::LiquidityWithdrawPhase::ReceiptRecoveryRequired;
            DefiniteAttemptResult::Held
        }
    }
}

fn hold_for_receipt_recovery(caller: candid::Principal, request_id: u128) {
    mutate_state(|s| hold_for_receipt_recovery_in_state(s, caller, request_id));
}

fn hold_for_receipt_recovery_in_state(
    s: &mut crate::state::State,
    caller: candid::Principal,
    request_id: u128,
) {
    if let Some(current) = s.liquidity_withdraw_journals.get_mut(&caller) {
        if current.request_id == request_id
            && current.phase == crate::state::LiquidityWithdrawPhase::SubmittedOrUnknown
        {
            current.phase = crate::state::LiquidityWithdrawPhase::ReceiptRecoveryRequired;
        }
    }
}

fn commit_liquidity_withdrawal_in_state(
    s: &mut crate::state::State,
    journal: &crate::state::LiquidityWithdrawJournal,
    block_index: u64,
    timestamp_ns: u64,
) -> bool {
    let Some(current) = s.liquidity_withdraw_journals.get(&journal.owner) else {
        return false;
    };
    if current.request_id != journal.request_id
        || current.amount_e8s != journal.amount_e8s
        || current.tuple != journal.tuple
    {
        return false;
    }
    if let crate::state::LiquidityWithdrawPhase::Completed { block_index: prior } = current.phase {
        return prior == block_index;
    }
    if !matches!(
        current.phase,
        crate::state::LiquidityWithdrawPhase::SubmittedOrUnknown
            | crate::state::LiquidityWithdrawPhase::ReceiptRecoveryRequired
    ) {
        return false;
    }
    record_withdraw_liquidity_at(
        s,
        ICUSD::from(journal.amount_e8s),
        journal.owner,
        block_index,
        timestamp_ns,
    );
    s.liquidity_withdraw_journals
        .get_mut(&journal.owner)
        .unwrap()
        .phase = crate::state::LiquidityWithdrawPhase::Completed { block_index };
    true
}

fn liquidity_mint_definitely_rejected(error: &TransferError) -> bool {
    matches!(
        error,
        TransferError::BadFee { .. }
            | TransferError::BadBurn { .. }
            | TransferError::InsufficientFunds { .. }
            | TransferError::CreatedInFuture { .. }
    )
}

pub async fn reconcile_liquidity_withdrawal_from_block(
    request_id: u128,
    candidate_block_index: u64,
) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let journal =
        read_state(|s| s.liquidity_withdraw_journals.get(&caller).cloned()).ok_or_else(|| {
            ProtocolError::GenericError("no liquidity withdrawal is awaiting reconciliation".into())
        })?;
    if journal.request_id != request_id
        || journal.phase != crate::state::LiquidityWithdrawPhase::ReceiptRecoveryRequired
    {
        return Err(ProtocolError::GenericError(
            "withdrawal ID is not awaiting receipt recovery".into(),
        ));
    }
    crate::icrc3_proof::verify_icrc3_borrow_mint_block(journal.tuple.ledger, candidate_block_index, &journal.tuple).await.map_err(|error| ProtocolError::GenericError(format!("candidate ICRC-3 block does not prove the exact withdrawal mint; journal remains held: {error}")))?;
    let commit_time = ic_cdk::api::time();
    let committed = mutate_state(|s| {
        commit_liquidity_withdrawal_in_state(s, &journal, candidate_block_index, commit_time)
    });
    if !committed {
        return Err(ProtocolError::TemporarilyUnavailable(
            "withdrawal journal changed during receipt recovery".into(),
        ));
    }
    Ok(candidate_block_index)
}

pub fn my_liquidity_withdrawal_status() -> Option<crate::state::LiquidityWithdrawStatus> {
    let caller = ic_cdk::api::caller();
    read_state(|s| {
        s.liquidity_withdraw_journals.get(&caller).map(|journal| {
            crate::state::LiquidityWithdrawStatus {
                request_id: journal.request_id,
                amount_e8s: journal.amount_e8s,
                phase: journal.phase.clone(),
            }
        })
    })
}

#[cfg(test)]
mod withdrawal_journal_tests {
    use super::*;
    use crate::state::{LiquidityWithdrawPhase, State};
    use candid::Principal;

    fn units(value: u64) -> u64 { value * 100_000_000 }

    fn funded_state(owner: Principal) -> State {
        let mut state = State::default();
        state.icusd_ledger_principal = Principal::from_slice(&[9]);
        state.liquidity_pool.insert(owner, ICUSD::from(units(100)));
        state
    }

    #[test]
    fn legacy_deposits_fail_before_calling_the_ledger() {
        let result = futures::executor::block_on(provide_liquidity(20));
        assert!(matches!(result, Err(ProtocolError::TemporarilyUnavailable(_))));
    }

    #[test]
    fn legacy_icp_claims_fail_before_calling_the_ledger() {
        let result = futures::executor::block_on(claim_liquidity_returns());
        assert!(matches!(result, Err(ProtocolError::TemporarilyUnavailable(_))));
    }

    fn prepared(
        state: &mut State,
        owner: Principal,
        id: u128,
        amount: u64,
    ) -> crate::state::LiquidityWithdrawJournal {
        match prepare_liquidity_withdrawal_in_state(state, owner, id, units(amount), 123) {
            Ok(PrepareWithdrawal::Dispatch(row)) => row,
            other => panic!("expected a new withdrawal, got {other:?}"),
        }
    }

    #[test]
    fn subminimum_balance_can_exit_in_full_but_cannot_be_split_into_dust() {
        let owner = Principal::from_slice(&[7]);
        let mut state = State::default();
        state.icusd_ledger_principal = Principal::from_slice(&[9]);
        // Matches the observed 0.015 icUSD aggregate scale, without assuming
        // this test owner is the owner of the live fee-credit balance.
        state.liquidity_pool.insert(owner, ICUSD::from(1_500_000));
        assert!(matches!(
            prepare_liquidity_withdrawal_in_state(&mut state, owner, 1, 1_000_000, 123),
            Err(ProtocolError::AmountTooLow { .. })
        ));
        assert!(state.liquidity_withdraw_journals.is_empty());
        let row = match prepare_liquidity_withdrawal_in_state(&mut state, owner, 1, 1_500_000, 123) {
            Ok(PrepareWithdrawal::Dispatch(row)) => row,
            other => panic!("expected full-balance dust exit, got {other:?}"),
        };
        assert_eq!(row.amount_e8s, 1_500_000);
        assert!(commit_liquidity_withdrawal_in_state(&mut state, &row, 70, 500));
        assert_eq!(state.liquidity_pool.get(&owner).copied().unwrap_or_default(), ICUSD::from(0));
        assert!(matches!(
            prepare_liquidity_withdrawal_in_state(&mut state, owner, 1, 1_500_000, 999),
            Ok(PrepareWithdrawal::Completed(70))
        ));
    }

    #[test]
    fn exact_id_retry_reuses_tuple_and_changed_amount_is_rejected() {
        let owner = Principal::from_slice(&[1]);
        let mut state = funded_state(owner);
        let first = prepared(&mut state, owner, 10, 20);
        let retry = prepare_liquidity_withdrawal_in_state(&mut state, owner, 10, units(20), 999).unwrap();
        let PrepareWithdrawal::Dispatch(retry) = retry else {
            panic!("pending retry must dispatch")
        };
        assert_eq!(first.tuple, retry.tuple);
        assert!(prepare_liquidity_withdrawal_in_state(&mut state, owner, 11, units(20), 999).is_err());
        assert!(prepare_liquidity_withdrawal_in_state(&mut state, owner, 10, units(21), 999).is_err());
        assert_eq!(state.liquidity_withdraw_journals[&owner].attempt_count, 0);
    }

    #[test]
    fn older_completed_id_stays_rejected_after_a_newer_withdrawal_completes() {
        let owner = Principal::from_slice(&[2]);
        let mut state = funded_state(owner);
        let first = prepared(&mut state, owner, 10, 20);
        assert!(commit_liquidity_withdrawal_in_state(
            &mut state, &first, 40, 500
        ));
        let second = prepared(&mut state, owner, 11, 20);
        assert!(commit_liquidity_withdrawal_in_state(
            &mut state, &second, 41, 501
        ));
        assert!(prepare_liquidity_withdrawal_in_state(&mut state, owner, 10, units(20), 999).is_err());
        assert_eq!(state.liquidity_pool[&owner], ICUSD::from(units(60)));
    }

    #[test]
    fn ambiguous_first_reply_then_bad_fee_retry_remains_held_for_receipt() {
        let owner = Principal::from_slice(&[3]);
        let mut state = funded_state(owner);
        let first = prepared(&mut state, owner, 12, 20);
        let first = record_dispatch_attempt_in_state(&mut state, owner, &first).unwrap();
        // Simulate a lost reply: the persisted phase stays SubmittedOrUnknown.
        let retry = record_dispatch_attempt_in_state(&mut state, owner, &first).unwrap();
        assert_eq!(retry.attempt_count, 2);
        assert_eq!(
            reject_definite_attempt_in_state(&mut state, owner, 12),
            DefiniteAttemptResult::Held
        );
        assert_eq!(
            state.liquidity_withdraw_journals[&owner].phase,
            LiquidityWithdrawPhase::ReceiptRecoveryRequired
        );
        assert_eq!(state.liquidity_pool[&owner], ICUSD::from(units(100)));
    }

    #[test]
    fn late_rejected_retry_cannot_regress_completed_withdrawal_or_double_debit() {
        let owner = Principal::from_slice(&[6]);
        let mut state = funded_state(owner);
        let first = prepared(&mut state, owner, 15, 20);
        let first = record_dispatch_attempt_in_state(&mut state, owner, &first).unwrap();
        let _overlapping_retry = record_dispatch_attempt_in_state(&mut state, owner, &first).unwrap();
        assert!(commit_liquidity_withdrawal_in_state(&mut state, &first, 53, 500));
        let events_after_commit = crate::storage::count_events();
        assert_eq!(state.liquidity_pool[&owner], ICUSD::from(units(80)));

        assert_eq!(
            reject_definite_attempt_in_state(&mut state, owner, 15),
            DefiniteAttemptResult::AlreadyCompleted(53)
        );
        assert_eq!(
            state.liquidity_withdraw_journals[&owner].phase,
            LiquidityWithdrawPhase::Completed { block_index: 53 }
        );
        assert!(commit_liquidity_withdrawal_in_state(&mut state, &first, 53, 501));
        assert_eq!(crate::storage::count_events(), events_after_commit);
        assert_eq!(state.liquidity_pool[&owner], ICUSD::from(units(80)));
    }

    #[test]
    fn too_old_hold_and_positive_receipt_commit_debit_once() {
        let owner = Principal::from_slice(&[4]);
        let mut state = funded_state(owner);
        let row = prepared(&mut state, owner, 13, 20);
        hold_for_receipt_recovery_in_state(&mut state, owner, 13);
        assert_eq!(
            state.liquidity_withdraw_journals[&owner].phase,
            LiquidityWithdrawPhase::ReceiptRecoveryRequired
        );
        let events_before = crate::storage::count_events();
        assert!(commit_liquidity_withdrawal_in_state(
            &mut state, &row, 52, 500
        ));
        let events_after_commit = crate::storage::count_events();
        assert_eq!(events_after_commit, events_before + 1);
        assert!(commit_liquidity_withdrawal_in_state(
            &mut state, &row, 52, 501
        ));
        assert_eq!(crate::storage::count_events(), events_after_commit);
        assert!(!commit_liquidity_withdrawal_in_state(
            &mut state, &row, 53, 502
        ));
        assert_eq!(state.liquidity_pool[&owner], ICUSD::from(units(80)));
    }

    #[test]
    fn journal_and_attempt_count_survive_upgrade_roundtrip_and_old_snapshots_decode() {
        let owner = Principal::from_slice(&[5]);
        let mut state = funded_state(owner);
        let mut row = prepared(&mut state, owner, 14, 20);
        row.attempt_count = 2;
        row.phase = LiquidityWithdrawPhase::ReceiptRecoveryRequired;
        state.liquidity_withdraw_journals.insert(owner, row.clone());
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&state, &mut bytes).unwrap();
        let restored: State = ciborium::de::from_reader(bytes.as_slice()).unwrap();
        assert_eq!(restored.liquidity_withdraw_journals[&owner], row);

        let value: ciborium::value::Value = ciborium::de::from_reader(bytes.as_slice()).unwrap();
        let mut map = match value {
            ciborium::value::Value::Map(map) => map,
            other => panic!("State must be map: {other:?}"),
        };
        map.retain(|(key, _)| {
            key != &ciborium::value::Value::Text("liquidity_withdraw_journals".into())
        });
        let mut legacy = Vec::new();
        ciborium::ser::into_writer(&ciborium::value::Value::Map(map), &mut legacy).unwrap();
        let restored_old: State = ciborium::de::from_reader(legacy.as_slice()).unwrap();
        assert!(restored_old.liquidity_withdraw_journals.is_empty());
    }
}

pub async fn claim_liquidity_returns() -> Result<u64, ProtocolError> {
    // LIQ-CLAIM-01: the legacy route sent a fresh-nonce ICP transfer and only
    // debited the claim after the await. A lost callback could pay twice. Keep
    // historical return balances intact until a durable payout journal exists.
    Err(ProtocolError::TemporarilyUnavailable(
        "Historical ICP return claims are held pending safe payout reconciliation. No ICP transfer was sent.".into(),
    ))
}
