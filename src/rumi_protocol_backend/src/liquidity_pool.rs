use crate::event::{
    record_claim_liquidity_returns, record_provide_liquidity, record_withdraw_liquidity,
};
use crate::guard::GuardPrincipal;
use crate::logs::INFO;
use crate::management::{mint_icusd, transfer_icp, transfer_icusd_from};
use crate::{mutate_state, read_state, ProtocolError, ICP, ICUSD, MIN_LIQUIDITY_AMOUNT};
use candid::Principal;
use ic_canister_log::log;
use icrc_ledger_types::icrc1::transfer::TransferError;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{Memo, TransferArg};
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
use candid::Nat;
use num_traits::ToPrimitive;

const LIQUIDITY_V2_MEMO_PREFIX: &[u8] = b"RUMI-LP-V2:";
const MAX_ACTIVE_LIQUIDITY_V2_REQUESTS: usize = 1_000;
const MAX_LIQUIDITY_V2_CANDIDATES: usize = 64;
const MAX_LIQUIDITY_V2_HISTORY_BLOCKS_PER_CALL: u64 = 8;
const MAX_LIQUIDITY_V2_RECEIPT_VERIFICATIONS_PER_WINDOW: u8 = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
enum AttemptResult {
    Candidate(u64),
    NoEffect(String),
    Ambiguous(String),
}

const MAX_CONCURRENT_LIQUIDITY_OPERATIONS: usize = 100;

thread_local! {
    /// Same-owner liquidity mutations share balances and must stay serialized
    /// for the entire ledger await. Unlike `GuardPrincipal`, this safety lock
    /// has no age-based takeover: an external ledger call may outlast any
    /// fixed lease while still being capable of committing.
    static LIQUIDITY_OPERATIONS: RefCell<HashMap<Principal, u128>> = RefCell::new(HashMap::new());
    static LIQUIDITY_OPERATION_NEXT_TOKEN: Cell<u128> = Cell::new(0);
    #[cfg(feature = "liquidity-returns-pic-test")]
    static TRAP_AFTER_CLAIM_RETURNS_DISPATCH: Cell<bool> = const { Cell::new(false) };
}

#[cfg(feature = "liquidity-returns-pic-test")]
pub fn arm_claim_returns_callback_trap_for_test() {
    TRAP_AFTER_CLAIM_RETURNS_DISPATCH.with(|armed| armed.set(true));
}

/// A transient owner lock held across provide, withdraw, and claim ledger calls.
/// Token checks make a late Drop unable to release a different lock instance.
#[must_use]
struct LiquidityOperationGuard {
    owner: Principal,
    token: u128,
}

impl LiquidityOperationGuard {
    fn new(owner: Principal) -> Result<Self, ProtocolError> {
        LIQUIDITY_OPERATIONS.with(|active| {
            let mut active = active.borrow_mut();
            if active.contains_key(&owner) {
                return Err(ProtocolError::TemporarilyUnavailable(
                    "A liquidity operation for this owner is still in flight".to_string(),
                ));
            }
            if active.len() >= MAX_CONCURRENT_LIQUIDITY_OPERATIONS {
                return Err(ProtocolError::TemporarilyUnavailable(
                    "Too many liquidity operations are in flight".to_string(),
                ));
            }
            let token = LIQUIDITY_OPERATION_NEXT_TOKEN
                .with(|next| {
                    let token = next.get().checked_add(1).ok_or(())?;
                    next.set(token);
                    Ok::<u128, ()>(token)
                })
                .map_err(|()| {
                    ProtocolError::TemporarilyUnavailable(
                        "Liquidity operation lock tokens are exhausted".to_string(),
                    )
                })?;
            active.insert(owner, token);
            Ok(Self { owner, token })
        })
    }
}

impl Drop for LiquidityOperationGuard {
    fn drop(&mut self) {
        LIQUIDITY_OPERATIONS.with(|active| {
            let mut active = active.borrow_mut();
            if active.get(&self.owner) == Some(&self.token) {
                active.remove(&self.owner);
            }
        });
    }
}

pub async fn provide_liquidity(amount: u64) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let _liquidity_guard = LiquidityOperationGuard::new(caller)?;
    let _guard_principal = GuardPrincipal::new(caller, "provide_liquidity")?;

    let amount: ICUSD = amount.into();

    if amount < MIN_LIQUIDITY_AMOUNT {
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: MIN_LIQUIDITY_AMOUNT.to_u64(),
        });
    }

    match transfer_icusd_from(amount, caller).await {
        Ok(block_index) => {
            log!(INFO, "[provide_liquidity] {caller} provided {amount}",);
            mutate_state(|s| {
                record_provide_liquidity(s, amount, caller, block_index);
            });
            Ok(block_index)
        }
        Err(transfer_from_error) => Err(ProtocolError::TransferFromError(
            transfer_from_error,
            amount.to_u64(),
        )),
    }
}

pub async fn withdraw_liquidity(amount: u64) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::caller();
    let _liquidity_guard = LiquidityOperationGuard::new(caller)?;
    let _guard_principal = GuardPrincipal::new(caller, "withdraw_liquidity")?;

    let amount: ICUSD = amount.into();

    if amount < MIN_LIQUIDITY_AMOUNT {
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: MIN_LIQUIDITY_AMOUNT.to_u64(),
        });
    }

    let provided_liquidity =
        read_state(|s| s.liquidity_pool.get(&caller).cloned()).ok_or_else(|| {
            ProtocolError::GenericError("You have no provided liquidity to withdraw".to_string())
        })?;
    if amount > provided_liquidity {
        return Err(ProtocolError::GenericError(format!(
            "cannot withdraw: {amount}, provided: {provided_liquidity}"
        )));
    }

    match mint_icusd(amount, caller).await {
        Ok(block_index) => {
            log!(INFO, "[withdraw_liquidity] {caller} withdrew {amount}",);
            mutate_state(|s| {
                record_withdraw_liquidity(s, amount, caller, block_index);
            });
            Ok(block_index)
        }
        Err(transfer_error) => Err(ProtocolError::TransferError(transfer_error)),
    }
}

pub async fn claim_liquidity_returns() -> Result<u64, ProtocolError> {
    let caller = ic_cdk::caller();
    let _liquidity_guard = LiquidityOperationGuard::new(caller)?;
    let _guard_principal = GuardPrincipal::new(caller, "claim_liquidity_returns")?;

    let return_amount =
        read_state(|s| s.liquidity_returns.get(&caller).cloned()).ok_or_else(|| {
            ProtocolError::GenericError("You have no liquidity rewards to claim".to_string())
        })?;

    match transfer_icp(return_amount, caller).await {
        Ok(block_index) => {
            log!(
                INFO,
                "[claim_liquidity_returns] {caller} claimed {return_amount}",
            );
            mutate_state(|s| {
                record_claim_liquidity_returns(s, return_amount, caller, block_index);
            });
            Ok(block_index)
        }
        Err(transfer_error) => {
            if let TransferError::BadFee { expected_fee } = transfer_error.clone() {
                mutate_state(|s| {
                    let expected_fee: u64 = expected_fee
                        .0
                        .try_into()
                        .expect("failed to convert Nat to u64");
                    s.icp_ledger_fee = ICP::from(expected_fee);
                });
            };
            Err(ProtocolError::TransferError(transfer_error))
        }
    }
}

fn liquidity_v2_memo(owner: Principal, request_id: u128, kind: crate::LiquidityV2Kind) -> Vec<u8> {
    // ICRC ledgers commonly cap memos at 32 bytes. Hash the domain-separated
    // owner/sequence/kind identity to one fixed-size memo while retaining the
    // full unhashed tuple in stable state.
    use sha2::{Digest, Sha256};
    let mut identity = LIQUIDITY_V2_MEMO_PREFIX.to_vec();
    identity.extend_from_slice(owner.as_slice());
    identity.extend_from_slice(&request_id.to_be_bytes());
    identity.push(match kind {
        crate::LiquidityV2Kind::Provide => 1,
        crate::LiquidityV2Kind::Withdraw => 2,
        crate::LiquidityV2Kind::ClaimReturns => 3,
    });
    Sha256::digest(identity).to_vec()
}

fn request_status(
    row: &crate::state::LiquidityV2Journal,
) -> crate::LiquidityV2StatusView {
    use crate::{LiquidityV2Phase as Phase};
    use crate::state::LiquidityV2Outcome as Outcome;
    let (phase, result_block_index) = match &row.outcome {
        Outcome::Pending => (Phase::Pending, None),
        Outcome::Held => (Phase::Held, None),
        Outcome::Complete { block_index } => (Phase::Complete, Some(*block_index)),
        Outcome::Rejected { .. } => (Phase::Rejected, None),
    };
    crate::LiquidityV2StatusView {
        owner: row.owner,
        request_id: row.request_id,
        kind: row.tuple.kind,
        amount_raw: row.tuple.amount_raw,
        ledger: row.tuple.ledger,
        phase,
        candidate_block_index: row.candidate_block_index,
        result_block_index,
        had_ambiguous_attempt: row.had_ambiguous_attempt,
        last_error: row.last_error.clone(),
        created_at_ns: row.created_at_ns,
    }
}

pub fn liquidity_v2_request_state(owner: Principal) -> crate::LiquidityV2RequestState {
    let state = read_state(|s| {
        let high_water = s.liquidity_v2_high_water.get(&owner).copied().unwrap_or(0);
        crate::LiquidityV2RequestState {
            next_request_id: high_water.checked_add(1).unwrap_or(0),
            active_request: s.liquidity_v2_active.get(&owner).map(request_status),
            latest_result: s.liquidity_v2_latest_result.get(&owner).map(request_status),
        }
    });
    state
}

fn liquidity_error(message: impl Into<String>) -> ProtocolError {
    ProtocolError::TemporarilyUnavailable(message.into())
}

fn next_liquidity_candidate_cursor(position: usize, count: usize) -> u64 {
    if count == 0 { 0 } else { ((position + 1) % count) as u64 }
}

fn liquidity_v2_requires_history_scan(row: &crate::state::LiquidityV2Journal) -> bool {
    row.had_ambiguous_attempt || row.dispatch_attempts > 0
}

fn plan_liquidity_v2_history_page(
    scan: Option<&crate::state::LiquidityV2HistoryScan>,
    current_tip: u64,
) -> Result<(crate::state::LiquidityV2HistoryScan, u64), String> {
    let scan = match scan {
        // Rows written before history discovery have no safe lower bound.
        // Backfill from genesis rather than guessing from wall-clock time.
        None => crate::state::LiquidityV2HistoryScan {
            next_index: 0,
            end_exclusive: current_tip,
        },
        Some(scan) => scan.clone(),
    };
    if scan.next_index > scan.end_exclusive || current_tip < scan.end_exclusive {
        return Err("liquidity history cursor is inconsistent with the pinned ledger tip".into());
    }
    let end_exclusive = current_tip.max(scan.end_exclusive);
    let page_end = end_exclusive.min(
        scan.next_index
            .saturating_add(MAX_LIQUIDITY_V2_HISTORY_BLOCKS_PER_CALL),
    );
    Ok((
        crate::state::LiquidityV2HistoryScan {
            next_index: scan.next_index,
            end_exclusive,
        },
        page_end,
    ))
}

async fn liquidity_v2_history_tip(
    ledger: Principal,
    kind: crate::LiquidityV2Kind,
) -> Result<u64, ProtocolError> {
    let tip = match kind {
        crate::LiquidityV2Kind::Provide | crate::LiquidityV2Kind::Withdraw => {
            crate::icrc3_proof::icrc3_log_length(ledger).await
        }
        crate::LiquidityV2Kind::ClaimReturns => {
            crate::treasury::native_icp_cancel_history_length(ledger).await
        }
    };
    tip.map_err(liquidity_error)
}

fn existing_liquidity_v2_request(
    owner: Principal,
    request_id: u128,
    kind: crate::LiquidityV2Kind,
    amount_raw: Option<u64>,
) -> Result<Option<crate::state::LiquidityV2Journal>, ProtocolError> {
    read_state(|state| {
        let row = if let Some(active) = state.liquidity_v2_active.get(&owner) {
            if active.request_id != request_id {
                return Err(liquidity_error("A different liquidity request is unresolved for this owner"));
            }
            Some(active)
        } else {
            state.liquidity_v2_latest_result.get(&owner)
                .filter(|completed| completed.request_id == request_id)
        };
        let Some(row) = row else { return Ok(None) };
        if row.tuple.kind != kind || amount_raw.is_some_and(|amount| amount != row.tuple.amount_raw) {
            return Err(ProtocolError::GenericError("Request ID was already used for a different liquidity operation".into()));
        }
        Ok(Some(row.clone()))
    })
}

fn check_provide_capacity(
    state: &crate::state::State,
    owner: Principal,
    amount_raw: u64,
) -> Result<(), ProtocolError> {
    let owner_balance = state
        .liquidity_pool
        .get(&owner)
        .map(|value| value.to_u64())
        .unwrap_or(0);
    owner_balance.checked_add(amount_raw).ok_or_else(|| {
        ProtocolError::GenericError("Your liquidity position would overflow".into())
    })?;
    let total = state.liquidity_pool.values().try_fold(0u64, |sum, value| {
        sum.checked_add(value.to_u64())
    }).ok_or_else(|| liquidity_error("Aggregate provided liquidity is invalid"))?;
    let reserved = state.liquidity_v2_active.values().try_fold(0u64, |sum, row| {
        if row.tuple.kind == crate::LiquidityV2Kind::Provide {
            sum.checked_add(row.tuple.amount_raw)
        } else {
            Some(sum)
        }
    }).ok_or_else(|| liquidity_error("Pending liquidity reservations overflow"))?;
    total.checked_add(reserved).and_then(|value| value.checked_add(amount_raw))
        .ok_or_else(|| ProtocolError::GenericError(
            "Aggregate provided liquidity would overflow".into(),
        ))?;
    Ok(())
}

fn admit_liquidity_v2(
    owner: Principal,
    request_id: u128,
    kind: crate::LiquidityV2Kind,
    amount_raw: u64,
    ledger: Principal,
    fee: Option<u64>,
    history_tip: u64,
) -> Result<crate::state::LiquidityV2Journal, ProtocolError> {
    if request_id == 0 || amount_raw == 0 {
        return Err(ProtocolError::GenericError(
            "Liquidity request ID and amount must be nonzero".into(),
        ));
    }
    mutate_state(|state| {
        let configured_ledger = match kind {
            crate::LiquidityV2Kind::Provide | crate::LiquidityV2Kind::Withdraw => state.icusd_ledger_principal,
            crate::LiquidityV2Kind::ClaimReturns => state.icp_ledger_principal,
        };
        if configured_ledger != ledger {
            return Err(liquidity_error("Configured ledger changed while preparing the liquidity request"));
        }
        if let Some(active) = state.liquidity_v2_active.get(&owner) {
            if active.request_id != request_id
                || active.tuple.kind != kind
                || active.tuple.amount_raw != amount_raw
            {
                return Err(liquidity_error(
                    "A different liquidity request is unresolved for this owner",
                ));
            }
            return Ok(active.clone());
        }
        if let Some(completed) = state.liquidity_v2_latest_result.get(&owner) {
            if completed.request_id == request_id {
                if completed.tuple.kind != kind || completed.tuple.amount_raw != amount_raw {
                    return Err(ProtocolError::GenericError(
                        "Request ID was already used for a different liquidity operation".into(),
                    ));
                }
                return Ok(completed.clone());
            }
        }
        let high_water = state.liquidity_v2_high_water.get(&owner).copied().unwrap_or(0);
        let expected = high_water.checked_add(1).ok_or_else(|| {
            liquidity_error("Liquidity request ID sequence is exhausted")
        })?;
        if request_id != expected {
            return Err(ProtocolError::GenericError(format!(
                "Liquidity request ID must be the next value ({expected})"
            )));
        }
        if state.liquidity_v2_active.len() >= MAX_ACTIVE_LIQUIDITY_V2_REQUESTS {
            return Err(liquidity_error("Too many unresolved liquidity requests"));
        }

        // Protect both the existing owner's position and total pool capacity.
        // Pending Provide rows reserve capacity even while another owner's
        // ledger await is unresolved.
        if kind == crate::LiquidityV2Kind::Provide {
            check_provide_capacity(state, owner, amount_raw)?;
        } else if kind == crate::LiquidityV2Kind::Withdraw {
            let owner_balance = state
                .liquidity_pool
                .get(&owner)
                .map(|value| value.to_u64())
                .unwrap_or(0);
            if amount_raw > owner_balance {
                return Err(ProtocolError::GenericError(
                    "Requested withdrawal exceeds provided liquidity".into(),
                ));
            }
        }

        let now = ic_cdk::api::time();
        let tuple = crate::state::LiquidityV2Tuple {
            kind,
            ledger,
            from: Account { owner: if kind == crate::LiquidityV2Kind::Provide { owner } else { ic_cdk::id() }, subaccount: None },
            to: Account { owner: if kind == crate::LiquidityV2Kind::Provide { ic_cdk::id() } else { owner }, subaccount: None },
            spender: (kind == crate::LiquidityV2Kind::Provide).then_some(Account { owner: ic_cdk::id(), subaccount: None }),
            amount_raw,
            fee,
            memo: liquidity_v2_memo(owner, request_id, kind),
            created_at_time_ns: now,
        };
        let row = crate::state::LiquidityV2Journal {
            owner,
            request_id,
            tuple,
            created_at_ns: now,
            candidate_block_index: None,
            candidate_block_indices: Vec::new(),
            candidate_scan_cursor: 0,
            history_scan: Some(crate::state::LiquidityV2HistoryScan {
                next_index: history_tip,
                end_exclusive: history_tip,
            }),
            candidate_attach_window_start_ns: 0,
            candidate_attach_attempts: 0,
            had_ambiguous_attempt: false,
            dispatch_attempts: 0,
            last_error: None,
            outcome: crate::state::LiquidityV2Outcome::Pending,
        };
        crate::storage::mark_liquidity_v2_journal_used()
            .map_err(liquidity_error)?;
        state.liquidity_v2_active.insert(owner, row.clone());
        state.liquidity_v2_high_water.insert(owner, request_id);
        crate::storage::save_state_to_stable(state);
        Ok(row)
    })
}

fn is_transfer_from_no_effect(error: &TransferFromError) -> bool {
    matches!(error,
        TransferFromError::BadFee { .. }
        | TransferFromError::BadBurn { .. }
        | TransferFromError::InsufficientFunds { .. }
        | TransferFromError::InsufficientAllowance { .. }
        | TransferFromError::TooOld
        | TransferFromError::CreatedInFuture { .. })
}

fn is_transfer_no_effect(error: &TransferError) -> bool {
    matches!(error,
        TransferError::BadFee { .. }
        | TransferError::BadBurn { .. }
        | TransferError::InsufficientFunds { .. }
        | TransferError::TooOld
        | TransferError::CreatedInFuture { .. })
}

async fn dispatch_liquidity_v2(row: &crate::state::LiquidityV2Journal) -> AttemptResult {
    let tuple = &row.tuple;
    match tuple.kind {
        crate::LiquidityV2Kind::Provide => {
            let args = TransferFromArgs {
                spender_subaccount: None,
                from: tuple.from.clone(),
                to: tuple.to.clone(),
                amount: Nat::from(tuple.amount_raw),
                fee: tuple.fee.map(Nat::from),
                created_at_time: Some(tuple.created_at_time_ns),
                memo: Some(Memo::from(tuple.memo.clone())),
            };
            match ic_cdk::call::<_, (Result<Nat, TransferFromError>,)>(tuple.ledger, "icrc2_transfer_from", (args,)).await {
                Ok((Ok(index),)) => index.0.to_u64().map(AttemptResult::Candidate).unwrap_or_else(|| AttemptResult::Ambiguous("ledger returned an out-of-range block index".into())),
                Ok((Err(TransferFromError::Duplicate { duplicate_of }),)) => duplicate_of.0.to_u64().map(AttemptResult::Candidate).unwrap_or_else(|| AttemptResult::Ambiguous("duplicate response had an out-of-range block index".into())),
                Ok((Err(error),)) if is_transfer_from_no_effect(&error) => AttemptResult::NoEffect(format!("{error:?}")),
                Ok((Err(error),)) => AttemptResult::Ambiguous(format!("{error:?}")),
                Err((code, message)) => AttemptResult::Ambiguous(format!("ledger call rejected ({code:?}): {message}")),
            }
        }
        crate::LiquidityV2Kind::Withdraw => {
            let args = TransferArg {
                from_subaccount: None,
                to: tuple.to.clone(),
                amount: Nat::from(tuple.amount_raw),
                fee: tuple.fee.map(Nat::from),
                memo: Some(Memo::from(tuple.memo.clone())),
                created_at_time: Some(tuple.created_at_time_ns),
            };
            match ic_cdk::call::<_, (Result<Nat, TransferError>,)>(tuple.ledger, "icrc1_transfer", (args,)).await {
                Ok((Ok(index),)) => index.0.to_u64().map(AttemptResult::Candidate).unwrap_or_else(|| AttemptResult::Ambiguous("ledger returned an out-of-range block index".into())),
                Ok((Err(TransferError::Duplicate { duplicate_of }),)) => duplicate_of.0.to_u64().map(AttemptResult::Candidate).unwrap_or_else(|| AttemptResult::Ambiguous("duplicate response had an out-of-range block index".into())),
                Ok((Err(error),)) if is_transfer_no_effect(&error) => AttemptResult::NoEffect(format!("{error:?}")),
                Ok((Err(error),)) => AttemptResult::Ambiguous(format!("{error:?}")),
                Err((code, message)) => AttemptResult::Ambiguous(format!("ledger call rejected ({code:?}): {message}")),
            }
        }
        crate::LiquidityV2Kind::ClaimReturns => {
            let args = TransferArg {
                from_subaccount: None,
                to: tuple.to.clone(),
                amount: Nat::from(tuple.amount_raw),
                fee: tuple.fee.map(Nat::from),
                memo: Some(Memo::from(tuple.memo.clone())),
                created_at_time: Some(tuple.created_at_time_ns),
            };
            match ic_cdk::call::<_, (Result<Nat, TransferError>,)>(tuple.ledger, "icrc1_transfer", (args,)).await {
                Ok((Ok(index),)) => index.0.to_u64().map(AttemptResult::Candidate).unwrap_or_else(|| AttemptResult::Ambiguous("ICP ledger returned an out-of-range block index".into())),
                Ok((Err(TransferError::Duplicate { duplicate_of }),)) => duplicate_of.0.to_u64().map(AttemptResult::Candidate).unwrap_or_else(|| AttemptResult::Ambiguous("ICP duplicate response had an out-of-range block index".into())),
                Ok((Err(error),)) if is_transfer_no_effect(&error) => AttemptResult::NoEffect(format!("{error:?}")),
                Ok((Err(error),)) => AttemptResult::Ambiguous(format!("{error:?}")),
                Err((code, message)) => AttemptResult::Ambiguous(format!("ICP ledger call rejected ({code:?}): {message}")),
            }
        }
    }
}

async fn verify_liquidity_candidate(
    row: &crate::state::LiquidityV2Journal,
    block_index: u64,
) -> Result<(), String> {
    let tuple = &row.tuple;
    match tuple.kind {
        crate::LiquidityV2Kind::Provide | crate::LiquidityV2Kind::Withdraw => {
            let block = crate::icrc3_proof::fetch_icrc3_block(tuple.ledger, block_index).await?;
            validate_liquidity_icrc3_block(tuple, &block)
        }
        crate::LiquidityV2Kind::ClaimReturns => crate::treasury::verify_native_icp_transfer_receipt(
            tuple.ledger,
            ic_cdk::id(),
            row.owner,
            tuple.amount_raw,
            tuple.fee.unwrap_or(0),
            &tuple.memo,
            tuple.created_at_time_ns,
            block_index,
        ).await,
    }
}

fn persist_liquidity_v2_history_scan(
    owner: Principal,
    request_id: u128,
    scan: crate::state::LiquidityV2HistoryScan,
    message: String,
) -> Result<crate::state::LiquidityV2Journal, ProtocolError> {
    mutate_state(|state| {
        let row = state.liquidity_v2_active.get_mut(&owner)
            .filter(|row| row.request_id == request_id)
            .ok_or_else(|| liquidity_error("liquidity request disappeared during history scan"))?;
        row.history_scan = Some(scan);
        row.outcome = crate::state::LiquidityV2Outcome::Held;
        row.had_ambiguous_attempt = true;
        row.last_error = Some(message);
        let copy = row.clone();
        crate::storage::save_state_to_stable(state);
        Ok(copy)
    })
}

async fn scan_liquidity_v2_history(
    row: crate::state::LiquidityV2Journal,
) -> Result<Option<u64>, ProtocolError> {
    let current_tip = liquidity_v2_history_tip(row.tuple.ledger, row.tuple.kind).await?;
    let (scan, page_end) = plan_liquidity_v2_history_page(row.history_scan.as_ref(), current_tip)
        .map_err(liquidity_error)?;
    persist_liquidity_v2_history_scan(
        row.owner,
        row.request_id,
        scan.clone(),
        "receipt history discovery is in progress; the request remains held".into(),
    )?;
    if scan.next_index == page_end {
        return Ok(None);
    }

    let mut index = scan.next_index;
    while index < page_end {
        reserve_liquidity_v2_history_verification(row.owner, row.request_id)?;
        let matches = match row.tuple.kind {
            crate::LiquidityV2Kind::Provide | crate::LiquidityV2Kind::Withdraw => {
                let block = crate::icrc3_proof::fetch_icrc3_block(row.tuple.ledger, index)
                    .await
                    .map_err(|message| liquidity_error(format!("history block {index} unavailable or malformed; cursor retained: {message}")))?;
                validate_liquidity_icrc3_block(&row.tuple, &block).is_ok()
            }
            crate::LiquidityV2Kind::ClaimReturns => {
                crate::treasury::native_icp_transfer_history_matches(
                    row.tuple.ledger,
                    ic_cdk::id(),
                    row.owner,
                    row.tuple.amount_raw,
                    row.tuple.fee.unwrap_or(0),
                    &row.tuple.memo,
                    row.tuple.created_at_time_ns,
                    index,
                )
                .await
                .map_err(|message| liquidity_error(format!("native ICP history block {index} unavailable or malformed; cursor retained: {message}")))?
            }
        };
        if matches {
            let candidate = if row.candidate_block_indices.len() < MAX_LIQUIDITY_V2_CANDIDATES
                || row.candidate_block_indices.contains(&index)
            {
                record_liquidity_v2_result(
                    row.owner,
                    row.request_id,
                    crate::state::LiquidityV2Outcome::Held,
                    Some(index),
                    true,
                    Some("history scan found an exact receipt candidate".into()),
                )?
            } else {
                // A full user-candidate vector must not make automatic receipt
                // discovery fail. Keep the cursor on this index; commit either
                // settles now or holds and re-proves it on the next scan call.
                row.clone()
            };
            // The scanner used the same complete tuple predicate as the known-
            // index verifier. Persist before synchronous bookkeeping.
            return commit_liquidity_v2_verified(candidate, index).map(Some);
        }
        index += 1;
    }

    let mut advanced = scan;
    advanced.next_index = page_end;
    persist_liquidity_v2_history_scan(
        row.owner,
        row.request_id,
        advanced,
        format!("receipt history scanned through block {}; outcome remains held", page_end.saturating_sub(1)),
    )?;
    Ok(None)
}

async fn recover_ambiguous_liquidity_v2(
    row: crate::state::LiquidityV2Journal,
) -> Result<u64, ProtocolError> {
    if let Some(block_index) = scan_liquidity_v2_history(row).await? {
        return Ok(block_index);
    }
    Err(liquidity_error(
        "Liquidity request remains held after bounded receipt-history scan; no new ledger dispatch was made",
    ))
}

fn validate_liquidity_icrc3_block(
    tuple: &crate::state::LiquidityV2Tuple,
    block: &crate::icrc3_proof::DecodedBlock,
) -> Result<(), String> {
    let ok = match tuple.kind {
        crate::LiquidityV2Kind::Provide => {
            block.op == "burn"
                && block.btype.as_deref().is_none_or(|value| value == "1burn")
                && block.from.as_ref().is_some_and(|account| crate::icrc3_proof::accounts_match(account, &tuple.from))
                && block.spender.as_ref().is_some_and(|account| tuple.spender.as_ref().is_some_and(|expected| crate::icrc3_proof::accounts_match(account, expected)))
                && block.to.is_none()
                && block.amount == u128::from(tuple.amount_raw)
                && block.transaction_fee.is_none()
                && block.fee.is_none()
                && block.memo.as_deref() == Some(tuple.memo.as_slice())
                && block.created_at_time == Some(tuple.created_at_time_ns)
                && block.expected_allowance.is_none()
                && block.expires_at.is_none()
        }
        crate::LiquidityV2Kind::Withdraw => {
            block.op == "mint"
                && block.btype.as_deref().is_none_or(|value| value == "1mint")
                && block.to.as_ref().is_some_and(|account| crate::icrc3_proof::accounts_match(account, &tuple.to))
                && block.from.is_none()
                && block.spender.is_none()
                && block.amount == u128::from(tuple.amount_raw)
                && block.transaction_fee.is_none()
                && block.fee.is_none()
                && block.memo.as_deref() == Some(tuple.memo.as_slice())
                && block.created_at_time == Some(tuple.created_at_time_ns)
                && block.expected_allowance.is_none()
                && block.expires_at.is_none()
        }
        crate::LiquidityV2Kind::ClaimReturns => false,
    };
    if ok { Ok(()) } else { Err("ICRC-3 block does not match the pinned liquidity burn/mint identity".into()) }
}

fn record_liquidity_v2_result(
    owner: Principal,
    request_id: u128,
    outcome: crate::state::LiquidityV2Outcome,
    candidate_block_index: Option<u64>,
    ambiguous: bool,
    message: Option<String>,
) -> Result<crate::state::LiquidityV2Journal, ProtocolError> {
    mutate_state(|state| {
        let row = state.liquidity_v2_active.get_mut(&owner).ok_or_else(|| liquidity_error("liquidity request disappeared"))?;
        if row.request_id != request_id {
            return Err(liquidity_error("liquidity request identity changed"));
        }
        row.outcome = outcome.clone();
        if let Some(block_index) = candidate_block_index {
            if !row.candidate_block_indices.contains(&block_index) {
                if row.candidate_block_indices.len() >= MAX_LIQUIDITY_V2_CANDIDATES {
                    return Err(liquidity_error("Liquidity candidate history is full; existing candidates remain retained"));
                }
                row.candidate_block_indices.push(block_index);
            }
            if row.candidate_block_index.is_none() {
                row.candidate_block_index = Some(block_index);
            }
        }
        row.had_ambiguous_attempt = ambiguous;
        row.last_error = message;
        let row = row.clone();
        if matches!(outcome, crate::state::LiquidityV2Outcome::Complete { .. } | crate::state::LiquidityV2Outcome::Rejected { .. }) {
            state.liquidity_v2_active.remove(&owner);
            state.liquidity_v2_latest_result.insert(owner, row.clone());
        }
        crate::storage::save_state_to_stable(state);
        Ok(row)
    })
}

fn reserve_liquidity_v2_dispatch(
    owner: Principal,
    request_id: u128,
) -> Result<(crate::state::LiquidityV2Journal, bool), ProtocolError> {
    mutate_state(|state| {
        let row = state.liquidity_v2_active.get_mut(&owner)
            .filter(|row| row.request_id == request_id)
            .ok_or_else(|| liquidity_error("liquidity request disappeared before dispatch"))?;
        let prior_ambiguous = row.had_ambiguous_attempt;
        if row.candidate_block_indices.len() >= MAX_LIQUIDITY_V2_CANDIDATES {
            return Err(liquidity_error("Candidate history is full; refusing a dispatch whose receipt could not be retained"));
        }
        row.dispatch_attempts = row.dispatch_attempts.checked_add(1)
            .ok_or_else(|| liquidity_error("liquidity dispatch attempt counter is exhausted"))?;
        // Persist ambiguity before awaiting: an upgrade or reply loss after the
        // external call can then only scan history or attach proof; it cannot
        // dispatch the tuple a second time.
        row.had_ambiguous_attempt = true;
        row.outcome = crate::state::LiquidityV2Outcome::Pending;
        let copy = row.clone();
        crate::storage::save_state_to_stable(state);
        Ok((copy, prior_ambiguous))
    })
}

fn reserve_candidate_verification(
    owner: Principal,
    request_id: u128,
    candidate_position: usize,
) -> Result<(), ProtocolError> {
    reserve_liquidity_v2_receipt_verification(owner, request_id, Some(candidate_position))
}

fn reserve_liquidity_v2_history_verification(
    owner: Principal,
    request_id: u128,
) -> Result<(), ProtocolError> {
    reserve_liquidity_v2_receipt_verification(owner, request_id, None)
}

fn reserve_liquidity_v2_receipt_verification(
    owner: Principal,
    request_id: u128,
    candidate_position: Option<usize>,
) -> Result<(), ProtocolError> {
    const WINDOW_NS: u64 = 60_000_000_000;
    const MAX_ATTEMPTS: u8 = MAX_LIQUIDITY_V2_RECEIPT_VERIFICATIONS_PER_WINDOW;
    mutate_state(|state| {
        let row = state.liquidity_v2_active.get_mut(&owner)
            .filter(|row| row.request_id == request_id)
            .ok_or_else(|| liquidity_error("liquidity request disappeared before candidate verification"))?;
        let now = ic_cdk::api::time();
        if row.candidate_attach_window_start_ns == 0
            || now.saturating_sub(row.candidate_attach_window_start_ns) >= WINDOW_NS
        {
            row.candidate_attach_window_start_ns = now;
            row.candidate_attach_attempts = 0;
        }
        if row.candidate_attach_attempts >= MAX_ATTEMPTS {
            return Err(liquidity_error("Receipt verification limit reached; retry after the 60-second window"));
        }
        row.candidate_attach_attempts += 1;
        if let Some(position) = candidate_position {
            row.candidate_scan_cursor = next_liquidity_candidate_cursor(
                position,
                row.candidate_block_indices.len(),
            );
        }
        crate::storage::save_state_to_stable(state);
        Ok(())
    })
}

fn commit_liquidity_v2_verified(
    row: crate::state::LiquidityV2Journal,
    block_index: u64,
) -> Result<u64, ProtocolError> {
    let result = mutate_state(|state| {
        let active = state.liquidity_v2_active.get(&row.owner).ok_or_else(|| liquidity_error("liquidity request disappeared before bookkeeping"))?;
        if active.request_id != row.request_id || active.tuple != row.tuple {
            return Err(liquidity_error("liquidity request tuple changed before bookkeeping"));
        }
        match row.tuple.kind {
            crate::LiquidityV2Kind::Provide => {
                let owner_value = state.liquidity_pool.get(&row.owner).map(|v| v.to_u64()).unwrap_or(0);
                if owner_value.checked_add(row.tuple.amount_raw).is_none() {
                    let active = state.liquidity_v2_active.get_mut(&row.owner).expect("active request checked above");
                    active.outcome = crate::state::LiquidityV2Outcome::Held;
                    active.candidate_block_index.get_or_insert(block_index);
                    active.last_error = Some("verified Provide receipt is held because per-owner bookkeeping capacity is exhausted".into());
                    crate::storage::save_state_to_stable(state);
                    return Err(liquidity_error("Provide proof is valid but per-owner bookkeeping capacity is exhausted; request remains held"));
                }
                let total = state.liquidity_pool.values().try_fold(0u64, |sum, value| sum.checked_add(value.to_u64())).ok_or_else(|| liquidity_error("Aggregate liquidity accounting is invalid"))?;
                if total.checked_add(row.tuple.amount_raw).is_none() {
                    let active = state.liquidity_v2_active.get_mut(&row.owner).expect("active request checked above");
                    active.outcome = crate::state::LiquidityV2Outcome::Held;
                    active.candidate_block_index.get_or_insert(block_index);
                    active.last_error = Some("verified Provide receipt is held because aggregate bookkeeping capacity is exhausted".into());
                    crate::storage::save_state_to_stable(state);
                    return Err(liquidity_error("Provide proof is valid but aggregate bookkeeping capacity is exhausted; request remains held"));
                }
                crate::event::record_provide_liquidity(state, ICUSD::from(row.tuple.amount_raw), row.owner, block_index);
            }
            crate::LiquidityV2Kind::Withdraw => crate::event::record_withdraw_liquidity(state, ICUSD::from(row.tuple.amount_raw), row.owner, block_index),
            crate::LiquidityV2Kind::ClaimReturns => crate::event::record_claim_liquidity_returns(state, ICP::from(row.tuple.amount_raw), row.owner, block_index),
        }
        let mut completed = state.liquidity_v2_active.remove(&row.owner).expect("active request checked above");
        completed.outcome = crate::state::LiquidityV2Outcome::Complete { block_index };
        completed.candidate_block_index = Some(block_index);
        completed.had_ambiguous_attempt |= row.had_ambiguous_attempt;
        state.liquidity_v2_latest_result.insert(row.owner, completed.clone());
        crate::storage::save_state_to_stable(state);
        Ok(())
    });
    result?;
    Ok(block_index)
}

async fn run_liquidity_v2_request(
    row: crate::state::LiquidityV2Journal,
) -> Result<u64, ProtocolError> {
    match &row.outcome {
        crate::state::LiquidityV2Outcome::Complete { block_index } => return Ok(*block_index),
        crate::state::LiquidityV2Outcome::Rejected { message } => {
            return Err(ProtocolError::GenericError(format!("Liquidity request was rejected: {message}")));
        }
        crate::state::LiquidityV2Outcome::Pending | crate::state::LiquidityV2Outcome::Held => {}
    }
    // The ledger transaction window is configurable and is not established by
    // this canister's source. Once an attempt may have reached the ledger, do
    // not resubmit the tuple based on a wall-clock assumption. Search the exact
    // pinned ledger history instead; no match is not proof of no effect.
    if liquidity_v2_requires_history_scan(&row) {
        return recover_ambiguous_liquidity_v2(row).await;
    }

    // This is the sole dispatch path: the first and only attempt for a newly
    // admitted identity. Its exact tuple is persisted before the await below.
    let (row, prior_ambiguous) = reserve_liquidity_v2_dispatch(row.owner, row.request_id)?;
    let attempt = dispatch_liquidity_v2(&row).await;
    #[cfg(feature = "liquidity-returns-pic-test")]
    if row.tuple.kind == crate::LiquidityV2Kind::ClaimReturns
        && TRAP_AFTER_CLAIM_RETURNS_DISPATCH.with(|armed| armed.replace(false))
    {
        // The ledger's committed update has completed, but the canister traps
        // before consuming its reply. Stable pre-dispatch state then drives
        // history-based recovery on the next request after upgrade.
        ic_cdk::trap("test-only trap after committed ClaimReturns dispatch");
    }
    match attempt {
        AttemptResult::Candidate(block_index) => {
            let row = record_liquidity_v2_result(row.owner, row.request_id, crate::state::LiquidityV2Outcome::Held, Some(block_index), true, Some("ledger returned a candidate block; exact receipt verification is pending".into()))?;
            let position = row.candidate_block_indices.iter().position(|candidate| *candidate == block_index).expect("candidate persisted");
            reserve_candidate_verification(row.owner, row.request_id, position)?;
            if verify_liquidity_candidate(&row, block_index).await.is_ok() {
                commit_liquidity_v2_verified(row, block_index)
            } else {
                let row = record_liquidity_v2_result(row.owner, row.request_id, crate::state::LiquidityV2Outcome::Held, None, true, Some("ledger returned candidate did not yet prove the exact tuple; candidate retained".into()))?;
                recover_ambiguous_liquidity_v2(row).await
            }
        }
        AttemptResult::NoEffect(message) => {
            if prior_ambiguous {
                let row = record_liquidity_v2_result(row.owner, row.request_id, crate::state::LiquidityV2Outcome::Held, None, true, Some(format!("later attempt proved no effect, but an earlier attempt remains ambiguous: {message}")))?;
                recover_ambiguous_liquidity_v2(row).await
            } else {
                record_liquidity_v2_result(row.owner, row.request_id, crate::state::LiquidityV2Outcome::Rejected { message: message.clone() }, None, false, Some(message.clone()))?;
                Err(ProtocolError::GenericError(format!("Liquidity request had no ledger effect: {message}")))
            }
        }
        AttemptResult::Ambiguous(message) => {
            let row = record_liquidity_v2_result(row.owner, row.request_id, crate::state::LiquidityV2Outcome::Held, None, true, Some(message.clone()))?;
            recover_ambiguous_liquidity_v2(row).await
        }
    }
}

fn check_legacy_liquidity_enabled() -> Result<(), ProtocolError> {
    Err(ProtocolError::GenericError("This liquidity endpoint requires a request ID; refresh the app to continue safely".into()))
}

pub async fn provide_liquidity_v2(request_id: u128, amount_raw: u64) -> Result<u64, ProtocolError> {
    let owner = ic_cdk::api::caller();
    let _guard = LiquidityOperationGuard::new(owner)?;
    let amount: ICUSD = amount_raw.into();
    if amount < MIN_LIQUIDITY_AMOUNT { return Err(ProtocolError::AmountTooLow { minimum_amount: MIN_LIQUIDITY_AMOUNT.to_u64() }); }
    let ledger = read_state(|s| s.icusd_ledger_principal);
    if let Some(row) = existing_liquidity_v2_request(owner, request_id, crate::LiquidityV2Kind::Provide, Some(amount_raw))? {
        return run_liquidity_v2_request(row).await;
    }
    let history_tip = liquidity_v2_history_tip(ledger, crate::LiquidityV2Kind::Provide).await?;
    let row = admit_liquidity_v2(owner, request_id, crate::LiquidityV2Kind::Provide, amount_raw, ledger, None, history_tip)?;
    run_liquidity_v2_request(row).await
}

pub async fn withdraw_liquidity_v2(request_id: u128, amount_raw: u64) -> Result<u64, ProtocolError> {
    let owner = ic_cdk::api::caller();
    let _guard = LiquidityOperationGuard::new(owner)?;
    let amount: ICUSD = amount_raw.into();
    if amount < MIN_LIQUIDITY_AMOUNT { return Err(ProtocolError::AmountTooLow { minimum_amount: MIN_LIQUIDITY_AMOUNT.to_u64() }); }
    let ledger = read_state(|s| s.icusd_ledger_principal);
    if let Some(row) = existing_liquidity_v2_request(owner, request_id, crate::LiquidityV2Kind::Withdraw, Some(amount_raw))? {
        return run_liquidity_v2_request(row).await;
    }
    let history_tip = liquidity_v2_history_tip(ledger, crate::LiquidityV2Kind::Withdraw).await?;
    let row = admit_liquidity_v2(owner, request_id, crate::LiquidityV2Kind::Withdraw, amount_raw, ledger, None, history_tip)?;
    run_liquidity_v2_request(row).await
}

pub async fn claim_liquidity_returns_v2(request_id: u128) -> Result<u64, ProtocolError> {
    let owner = ic_cdk::api::caller();
    let _guard = LiquidityOperationGuard::new(owner)?;
    if let Some(row) = existing_liquidity_v2_request(owner, request_id, crate::LiquidityV2Kind::ClaimReturns, None)? {
        return run_liquidity_v2_request(row).await;
    }
    let amount_raw = read_state(|s| s.liquidity_returns.get(&owner).map(|value| value.to_u64()).unwrap_or(0));
    if amount_raw == 0 { return Err(ProtocolError::GenericError("You have no liquidity rewards to claim".into())); }
    let ledger = read_state(|s| s.icp_ledger_principal);
    let fee = crate::management::get_ledger_fee(ledger).await.map_err(liquidity_error)?;
    let history_tip = liquidity_v2_history_tip(ledger, crate::LiquidityV2Kind::ClaimReturns).await?;
    let row = admit_liquidity_v2(owner, request_id, crate::LiquidityV2Kind::ClaimReturns, amount_raw, ledger, Some(fee), history_tip)?;
    run_liquidity_v2_request(row).await
}

pub async fn attach_liquidity_v2_candidate(request_id: u128, block_index: u64) -> Result<u64, ProtocolError> {
    let owner = ic_cdk::api::caller();
    let _guard = LiquidityOperationGuard::new(owner)?;
    let row = read_state(|s| s.liquidity_v2_active.get(&owner).filter(|row| row.request_id == request_id).cloned())
        .ok_or_else(|| ProtocolError::GenericError("No active liquidity request has that ID".into()))?;
    if !row.had_ambiguous_attempt && row.candidate_block_index.is_none() {
        return Err(ProtocolError::GenericError("A receipt candidate can only be attached to an ambiguous liquidity request".into()));
    }
    let row = record_liquidity_v2_result(owner, request_id, crate::state::LiquidityV2Outcome::Held, Some(block_index), true, Some("owner supplied a candidate block for exact receipt verification".into()))?;
    let position = row.candidate_block_indices.iter().position(|candidate| *candidate == block_index).expect("candidate persisted");
    reserve_candidate_verification(owner, request_id, position)?;
    if verify_liquidity_candidate(&row, block_index).await.is_ok() {
        commit_liquidity_v2_verified(row, block_index)
    } else {
        record_liquidity_v2_result(owner, request_id, crate::state::LiquidityV2Outcome::Held, None, true, Some("candidate receipt did not prove the exact liquidity tuple; candidate retained".into()))?;
        Err(liquidity_error("Candidate block did not prove the exact liquidity ledger tuple; request remains held"))
    }
}

pub fn liquidity_v2_legacy_endpoint_guard() -> Result<(), ProtocolError> {
    check_legacy_liquidity_enabled()
}

#[cfg(test)]
mod liquidity_v2_state_tests {
    use super::{
        check_provide_capacity, liquidity_v2_requires_history_scan,
        next_liquidity_candidate_cursor, plan_liquidity_v2_history_page,
        validate_liquidity_icrc3_block, MAX_LIQUIDITY_V2_HISTORY_BLOCKS_PER_CALL,
    };
    use crate::{state::*, LiquidityV2Kind};
    use candid::Principal;
    use icrc_ledger_types::icrc1::account::Account;

    fn journal(owner: Principal, request_id: u128, amount_raw: u64) -> LiquidityV2Journal {
        LiquidityV2Journal {
            owner,
            request_id,
            tuple: LiquidityV2Tuple {
                kind: LiquidityV2Kind::Provide,
                ledger: Principal::from_slice(&[9]),
                from: Account { owner, subaccount: None },
                to: Account { owner: Principal::from_slice(&[8]), subaccount: None },
                spender: Some(Account { owner: Principal::from_slice(&[8]), subaccount: None }),
                amount_raw,
                fee: None,
                memo: vec![1, 2, 3],
                created_at_time_ns: 4,
            },
            created_at_ns: 4,
            candidate_block_index: None,
            candidate_block_indices: Vec::new(),
            candidate_scan_cursor: 0,
            history_scan: None,
            candidate_attach_window_start_ns: 0,
            candidate_attach_attempts: 0,
            had_ambiguous_attempt: false,
            dispatch_attempts: 0,
            last_error: None,
            outcome: LiquidityV2Outcome::Pending,
        }
    }

    #[test]
    fn provide_capacity_includes_pending_other_owner_reservations() {
        let owner = Principal::from_slice(&[1]);
        let other = Principal::from_slice(&[2]);
        let mut state = State::default();
        state.liquidity_pool.insert(owner, crate::numeric::ICUSD::from(u64::MAX - 10));
        state.liquidity_v2_active.insert(other, journal(other, 1, 6));

        assert!(check_provide_capacity(&state, owner, 5).is_err());
        assert!(check_provide_capacity(&state, owner, 4).is_ok());
    }

    #[test]
    fn provide_capacity_checks_per_owner_headroom() {
        let owner = Principal::from_slice(&[1]);
        let mut state = State::default();
        state.liquidity_pool.insert(owner, crate::numeric::ICUSD::from(u64::MAX - 2));
        assert!(check_provide_capacity(&state, owner, 3).is_err());
        assert!(check_provide_capacity(&state, owner, 2).is_ok());
    }

    #[test]
    fn exact_liquidity_journal_roundtrips_and_legacy_state_defaults_empty() {
        let owner = Principal::from_slice(&[1]);
        let mut state = State::default();
        let mut row = journal(owner, 7, 123);
        row.dispatch_attempts = 2;
        row.had_ambiguous_attempt = true;
        row.candidate_block_index = Some(44);
        row.candidate_block_indices = vec![44, 45];
        row.candidate_attach_attempts = 2;
        row.last_error = Some("proof query unavailable".into());
        row.outcome = LiquidityV2Outcome::Held;
        state.liquidity_v2_active.insert(owner, row.clone());
        state.liquidity_v2_high_water.insert(owner, 7);
        let mut encoded = Vec::new();
        ciborium::ser::into_writer(&state, &mut encoded).unwrap();
        let restored: State = ciborium::de::from_reader(encoded.as_slice()).unwrap();
        assert_eq!(restored.liquidity_v2_active.get(&owner).unwrap().tuple.memo, row.tuple.memo);
        assert_eq!(restored.liquidity_v2_active.get(&owner).unwrap().candidate_block_indices, vec![44, 45]);
        assert_eq!(restored.liquidity_v2_active.get(&owner).unwrap().dispatch_attempts, 2);
        assert_eq!(restored.liquidity_v2_high_water.get(&owner), Some(&7));

        // The serde(default) State decoder keeps pre-feature snapshots valid.
        let legacy = State::default();
        let mut legacy_bytes = Vec::new();
        ciborium::ser::into_writer(&legacy, &mut legacy_bytes).unwrap();
        let mut value: ciborium::value::Value = ciborium::de::from_reader(legacy_bytes.as_slice()).unwrap();
        if let ciborium::value::Value::Map(fields) = &mut value {
            fields.retain(|(key, _)| {
                !matches!(key, ciborium::value::Value::Text(name)
                    if name == "liquidity_v2_active" || name == "liquidity_v2_latest_result" || name == "liquidity_v2_high_water")
            });
        }
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&value, &mut bytes).unwrap();
        let restored: State = ciborium::de::from_reader(bytes.as_slice()).unwrap();
        assert!(restored.liquidity_v2_active.is_empty());
        assert!(restored.liquidity_v2_latest_result.is_empty());
        assert!(restored.liquidity_v2_high_water.is_empty());
    }

    #[test]
    fn ambiguous_request_is_history_scanned_and_never_replayed() {
        let owner = Principal::from_slice(&[1]);
        let mut row = journal(owner, 7, 123);
        assert!(!liquidity_v2_requires_history_scan(&row));

        // A dispatch reservation is persisted before awaiting the ledger, so
        // upgrade/reply loss must select scan/hold even if no reply was saved.
        row.dispatch_attempts = 1;
        row.had_ambiguous_attempt = true;
        row.outcome = LiquidityV2Outcome::Held;
        row.history_scan = Some(LiquidityV2HistoryScan {
            next_index: 44,
            end_exclusive: 44,
        });
        assert!(liquidity_v2_requires_history_scan(&row));

        // With no matching block in the current bounded page, history progress
        // advances from the pre-dispatch tip. There is no clock-based retry
        // branch, regardless of how old the request becomes.
        let (page, page_end) = plan_liquidity_v2_history_page(row.history_scan.as_ref(), 60).unwrap();
        assert_eq!(page.next_index, 44);
        assert_eq!(page_end, 52);
        assert!(liquidity_v2_requires_history_scan(&row));
    }

    #[test]
    fn history_cursor_starts_at_dispatch_tip_and_legacy_rows_backfill_from_zero() {
        let new_row = LiquidityV2HistoryScan { next_index: 100, end_exclusive: 100 };
        let (scan, page_end) = plan_liquidity_v2_history_page(Some(&new_row), 105).unwrap();
        assert_eq!(scan.next_index, 100);
        assert_eq!(scan.end_exclusive, 105);
        assert_eq!(page_end, 105);

        let (legacy_scan, legacy_page_end) = plan_liquidity_v2_history_page(None, 105).unwrap();
        assert_eq!(legacy_scan.next_index, 0);
        assert_eq!(legacy_scan.end_exclusive, 105);
        assert_eq!(legacy_page_end, MAX_LIQUIDITY_V2_HISTORY_BLOCKS_PER_CALL);

        let resumed = LiquidityV2HistoryScan {
            next_index: legacy_page_end,
            end_exclusive: 105,
        };
        let (scan, page_end) = plan_liquidity_v2_history_page(Some(&resumed), 106).unwrap();
        assert_eq!(scan.next_index, legacy_page_end);
        assert_eq!(scan.end_exclusive, 106);
        assert_eq!(page_end, legacy_page_end + MAX_LIQUIDITY_V2_HISTORY_BLOCKS_PER_CALL);
    }

    #[test]
    fn attached_candidates_cannot_disable_persistent_history_progress() {
        let mut row = journal(Principal::from_slice(&[1]), 7, 123);
        row.candidate_block_indices = (0..10).collect();
        row.candidate_scan_cursor = next_liquidity_candidate_cursor(7, 10);
        row.dispatch_attempts = 1;
        row.had_ambiguous_attempt = true;
        row.history_scan = Some(LiquidityV2HistoryScan {
            next_index: 9,
            end_exclusive: 9,
        });
        assert!(liquidity_v2_requires_history_scan(&row));
        let legacy_history = plan_liquidity_v2_history_page(None, 17).unwrap();
        assert_eq!(legacy_history.0.next_index, 0);
        assert_eq!(legacy_history.1, MAX_LIQUIDITY_V2_HISTORY_BLOCKS_PER_CALL);
        let (next, end) = plan_liquidity_v2_history_page(row.history_scan.as_ref(), 17).unwrap();
        assert_eq!(next.next_index, 9);
        assert_eq!(end, 17);
    }

    fn icrc3_block(op: &str, from: Option<Account>, to: Option<Account>, spender: Option<Account>) -> crate::icrc3_proof::DecodedBlock {
        crate::icrc3_proof::DecodedBlock {
            btype: None,
            op: op.into(),
            from,
            to,
            spender,
            amount: 123,
            transaction_fee: None,
            fee: None,
            memo: Some(vec![1, 2, 3]),
            created_at_time: Some(4),
            expected_allowance: None,
            expires_at: None,
        }
    }

    #[test]
    fn provide_receipt_requires_exact_fee_free_minter_burn_and_spender() {
        let owner = Principal::from_slice(&[1]);
        let row = journal(owner, 7, 123);
        let exact = icrc3_block("burn", Some(row.tuple.from.clone()), None, row.tuple.spender.clone());
        assert!(validate_liquidity_icrc3_block(&row.tuple, &exact).is_ok());

        let mut wrong = exact.clone();
        wrong.spender = Some(Account { owner, subaccount: None });
        assert!(validate_liquidity_icrc3_block(&row.tuple, &wrong).is_err());
        let mut wrong_type = exact;
        wrong_type.btype = Some("2xfer".into());
        assert!(validate_liquidity_icrc3_block(&row.tuple, &wrong_type).is_err());
    }

    #[test]
    fn withdraw_receipt_requires_exact_fee_free_mint_to_owner() {
        let owner = Principal::from_slice(&[1]);
        let mut row = journal(owner, 7, 123);
        row.tuple.kind = LiquidityV2Kind::Withdraw;
        row.tuple.from = Account { owner: Principal::from_slice(&[8]), subaccount: None };
        row.tuple.to = Account { owner, subaccount: None };
        row.tuple.spender = None;
        let exact = icrc3_block("mint", None, Some(row.tuple.to.clone()), None);
        assert!(validate_liquidity_icrc3_block(&row.tuple, &exact).is_ok());

        let mut wrong = exact;
        wrong.to = Some(Account { owner: Principal::anonymous(), subaccount: None });
        assert!(validate_liquidity_icrc3_block(&row.tuple, &wrong).is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LiquidityOperationGuard, LIQUIDITY_OPERATIONS, MAX_CONCURRENT_LIQUIDITY_OPERATIONS,
    };
    use candid::Principal;

    fn principal(byte: u8) -> Principal {
        Principal::from_slice(&[byte])
    }

    #[test]
    fn owner_lock_blocks_all_same_owner_reentry_and_releases_on_drop() {
        let owner = principal(1);
        let other = principal(2);
        let first = LiquidityOperationGuard::new(owner).expect("first owner lock");
        assert!(LiquidityOperationGuard::new(owner).is_err());
        let independent = LiquidityOperationGuard::new(other).expect("independent owner lock");
        drop(independent);
        drop(first);
        assert!(LiquidityOperationGuard::new(owner).is_ok());
    }

    #[test]
    fn late_drop_cannot_clear_a_different_tokenized_owner_lock() {
        let owner = principal(3);
        let old = LiquidityOperationGuard::new(owner).expect("old owner lock");
        let successor_token = old.token.checked_add(1).expect("successor token");
        LIQUIDITY_OPERATIONS.with(|active| {
            active.borrow_mut().insert(owner, successor_token);
        });

        drop(old);

        LIQUIDITY_OPERATIONS.with(|active| {
            assert_eq!(active.borrow().get(&owner), Some(&successor_token));
            active.borrow_mut().remove(&owner);
        });
    }

    #[test]
    fn owner_lock_caps_distinct_in_flight_operations() {
        let guards: Vec<_> = (0..MAX_CONCURRENT_LIQUIDITY_OPERATIONS)
            .map(|byte| {
                LiquidityOperationGuard::new(principal(byte as u8)).expect("within operation cap")
            })
            .collect();
        assert!(LiquidityOperationGuard::new(principal(200)).is_err());
        drop(guards);
        assert!(LiquidityOperationGuard::new(principal(200)).is_ok());
    }
}
