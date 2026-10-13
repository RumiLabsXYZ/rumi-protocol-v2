mod state;
mod types;

#[cfg(test)]
mod tests;

use candid::{candid_method, Principal};
use ic_canister_log::{declare_log_buffer, log};
use ic_cdk::api::caller;
use ic_cdk::{init, post_upgrade, pre_upgrade, query, update};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
use state::{init_state, restore_state, with_state, with_state_mut};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use types::{
    AssetType, DepositArgs, DepositRecord, DepositType, ExternalTokenDepositArgs, TreasuryAction,
    TreasuryEvent, TreasuryInitArgs, TreasuryStatus, WithdrawArgs, WithdrawResult,
    WithdrawalIntent, WithdrawalIntentStatus,
};

// Declare log buffer for debugging
declare_log_buffer!(name = LOG, capacity = 1000);

/// Standard ICRC-1 transfer fee (e8s), used as a conservative fallback when a
/// ledger's `icrc1_fee` query cannot be reached. Erring high keeps the
/// treasury solvent (we send slightly less) rather than risking an over-send.
const DEFAULT_LEDGER_FEE_E8S: u64 = 10_000;
// ICRC-1 ledgers must support at least 32 bytes; stay within that interoperable
// floor so a valid Treasury memo cannot become an avoidable ambiguous hold.
const MAX_WITHDRAWAL_MEMO_BYTES: usize = 32;

thread_local! {
    /// Per-ledger transfer-fee cache, populated lazily from `icrc1_fee` on the
    /// first withdrawal against a ledger. Heap-only (not persisted), so it is
    /// simply re-warmed after an upgrade.
    static LEDGER_FEES: RefCell<HashMap<Principal, u64>> = RefCell::new(HashMap::new());
    /// Prevent simultaneous same-ID ledger dispatches in one canister
    /// incarnation. A persisted InFlight row after upgrade is treated as
    /// ambiguous and never automatically dispatched again.
    static WITHDRAWALS_IN_FLIGHT: RefCell<HashSet<u64>> = RefCell::new(HashSet::new());
}

struct WithdrawalGuard(u64);
impl WithdrawalGuard {
    fn acquire(request_id: u64) -> Result<Self, String> {
        WITHDRAWALS_IN_FLIGHT.with(|ids| {
            if ids.borrow_mut().insert(request_id) {
                Ok(Self(request_id))
            } else {
                Err("Withdrawal request_id is already being processed".into())
            }
        })
    }
}
impl Drop for WithdrawalGuard {
    fn drop(&mut self) {
        WITHDRAWALS_IN_FLIGHT.with(|ids| {
            ids.borrow_mut().remove(&self.0);
        });
    }
}

/// Fetch a ledger's transfer fee, caching the result per ledger. On query
/// failure, falls back to the standard ICRC-1 fee (the solvency-safe direction).
async fn ledger_fee(ledger: Principal) -> u64 {
    if let Some(fee) = LEDGER_FEES.with(|c| c.borrow().get(&ledger).copied()) {
        return fee;
    }
    let result: Result<(candid::Nat,), _> = ic_cdk::call(ledger, "icrc1_fee", ()).await;
    let fee: u64 = match result {
        Ok((f,)) => f.0.try_into().unwrap_or(DEFAULT_LEDGER_FEE_E8S),
        Err(_) => DEFAULT_LEDGER_FEE_E8S,
    };
    LEDGER_FEES.with(|c| c.borrow_mut().insert(ledger, fee));
    fee
}

fn refresh_cached_fee_from_bad_fee(ledger: Principal, error: &TransferError) {
    if let TransferError::BadFee { expected_fee } = error {
        if let Ok(fee) = expected_fee.0.clone().try_into() {
            LEDGER_FEES.with(|cache| {
                cache.borrow_mut().insert(ledger, fee);
            });
        }
    }
}

/// ICRC-002: amount to put on the wire for a withdrawal of `amount` given the
/// ledger `fee`. The recipient bears the fee: the ledger debits `send + fee`
/// from the canister account, so sending `amount - fee` makes the account drop
/// by exactly `amount`, keeping tracked balances in step with real holdings.
/// (Historically the full `amount` was sent, drifting the tracked balance one
/// fee above the real balance per withdrawal.)
fn withdrawal_send_amount(amount: u64, fee: u64) -> Result<u64, String> {
    if amount <= fee {
        return Err(format!(
            "Withdrawal amount {} does not exceed the ledger fee {}",
            amount, fee
        ));
    }
    Ok(amount - fee)
}

/// Initialize the treasury canister
#[init]
#[candid_method(init)]
fn init(args: TreasuryInitArgs) {
    // UPG-006: refuse to init with non-empty stable memory. Catches accidental
    // reinstalls of a canister that already has persisted state. Reinstall mode
    // wipes stable memory before init runs (per IC spec), so this primarily
    // documents intent and guards against future IC behavior changes.
    assert!(
        ic_cdk::api::stable::stable64_size() == 0,
        "refusing to init: stable memory non-empty; use upgrade mode not reinstall"
    );
    log!(
        LOG,
        "Initializing treasury with controller: {}",
        args.controller
    );
    init_state(args);
}

/// Pre-upgrade hook to save state
#[pre_upgrade]
fn pre_upgrade() {
    log!(LOG, "Starting treasury upgrade");
}

/// Post-upgrade hook to restore state from stable memory
#[post_upgrade]
fn post_upgrade() {
    restore_state();
    log!(
        LOG,
        "Treasury upgrade completed — state restored from stable memory"
    );
}

/// Reject callers that are not an IC-level controller of this canister.
/// Controllers are set via `dfx canister update-settings --add-controller`.
fn ensure_controller() -> Result<(), String> {
    let caller = caller();
    if !ic_cdk::api::is_controller(&caller) {
        return Err(format!(
            "Access denied. {} is not a controller of this canister",
            caller
        ));
    }
    Ok(())
}

/// Deposit funds to treasury (controllers only)
#[update]
#[candid_method(update)]
async fn deposit(args: DepositArgs) -> Result<u64, String> {
    ensure_controller()?;

    // Check if treasury is paused
    let is_paused = with_state(|s| s.get_config().is_paused);
    if is_paused {
        return Err("Treasury is paused and not accepting deposits".to_string());
    }
    validate_legacy_deposit_asset(&args.asset_type)?;

    log!(
        LOG,
        "Processing deposit: {:?} {} {:?}",
        args.deposit_type,
        args.amount,
        args.asset_type
    );

    let deposit_caller = caller();
    let record = DepositRecord {
        id: 0, // Will be set by add_deposit
        deposit_type: args.deposit_type,
        asset_type: args.asset_type,
        amount: args.amount,
        block_index: args.block_index,
        timestamp: ic_cdk::api::time(),
        memo: args.memo,
    };
    let deposit_id = record_deposit_with_event(record, deposit_caller, ic_cdk::api::time())?;

    log!(LOG, "Deposit {} recorded successfully", deposit_id);
    Ok(deposit_id)
}

fn validate_legacy_deposit_asset(_asset_type: &AssetType) -> Result<(), String> {
    // TODO(external-token-receipts): remove CKUSDT/CKUSDC support here after
    // every backend collateral/fee caller has migrated to the durable outbox
    // and `record_external_token_deposit_once`; until then rejecting this
    // route can strand already-transferred fees whose notifier uses `deposit`.
    Ok(())
}

/// Record a BorrowingFee/ICUSD transfer with receiver-side idempotency. This
/// dedicated method is absent from older Treasury Wasms, so a new backend
/// cannot accidentally retry its fee outbox through the legacy non-idempotent
/// `deposit` method during a mixed-version rollout.
#[update]
#[candid_method(update)]
fn deposit_borrowing_fee_once(args: DepositArgs) -> Result<u64, String> {
    ensure_controller()?;
    validate_borrowing_fee_args(&args)?;
    if with_state(|s| s.get_config().is_paused) {
        return Err("Treasury is paused and not accepting deposits".to_string());
    }

    let now = ic_cdk::api::time();
    record_deposit_with_event(
        DepositRecord {
            id: 0,
            deposit_type: args.deposit_type,
            asset_type: args.asset_type,
            amount: args.amount,
            block_index: args.block_index,
            timestamp: now,
            memo: args.memo,
        },
        caller(),
        now,
    )
}

fn validate_borrowing_fee_args(args: &DepositArgs) -> Result<(), String> {
    if args.deposit_type != DepositType::BorrowingFee || args.asset_type != AssetType::ICUSD {
        return Err("only BorrowingFee/ICUSD deposits are accepted".to_string());
    }
    Ok(())
}

fn validate_interest_revenue_args(args: &DepositArgs) -> Result<(), String> {
    if args.deposit_type != DepositType::InterestRevenue || args.asset_type != AssetType::ICUSD {
        return Err("only InterestRevenue/ICUSD deposits are accepted".to_string());
    }
    Ok(())
}

/// Record a configured ckUSDT/ckUSDC ledger receipt exactly once. The supplied
/// ledger and block are a controller report used for idempotency; Treasury does
/// not independently verify that a transfer exists on that ledger.
#[update]
#[candid_method(update)]
fn record_external_token_deposit_once(args: ExternalTokenDepositArgs) -> Result<u64, String> {
    ensure_controller()?;
    if with_state(|s| s.get_config().is_paused) {
        return Err("Treasury is paused and not accepting deposits".to_string());
    }
    let deposit_caller = caller();
    let timestamp = ic_cdk::api::time();
    record_external_token_deposit_with_event(args, deposit_caller, timestamp)
}

/// Continue the bounded migration of historical CKUSDT/CKUSDC block indexes.
/// Legacy rows lack ledger provenance, so their asset/block pairs remain
/// conservatively unavailable to the new receipt API.
#[update]
#[candid_method(update)]
fn continue_external_token_deposit_backfill() -> Result<String, String> {
    ensure_controller()?;
    let (processed, next_deposit_id, complete) =
        with_state_mut(|s| s.continue_external_token_deposit_backfill())?;
    Ok(if complete {
        format!(
            "External token deposit block backfill complete; processed {} rows",
            processed
        )
    } else {
        format!(
            "External token deposit block backfill processed {} rows; next deposit ID {}",
            processed, next_deposit_id
        )
    })
}

/// Return the durable external-token index migration state for release
/// preflight. The legacy ambiguity count is the stable map length and does
/// not scan deposit history.
#[query]
#[candid_method(query)]
fn get_external_token_deposit_backfill_status() -> types::ExternalTokenDepositBackfillStatus {
    with_state(|s| types::ExternalTokenDepositBackfillStatus {
        state: s.external_token_backfill.get().clone(),
        indexed_through_deposit_id: *s.external_token_indexed_through.get(),
        ambiguous_legacy_blocks: s.external_token_legacy_blocks.len(),
    })
}

/// Record an InterestRevenue/ICUSD ledger transfer exactly once. This
/// versioned endpoint lets a newer backend fail closed against an older
/// Treasury that does not expose the method.
#[update]
#[candid_method(update)]
fn deposit_interest_revenue_once(args: DepositArgs) -> Result<u64, String> {
    ensure_controller()?;
    validate_interest_revenue_args(&args)?;
    if with_state(|s| s.get_config().is_paused) {
        return Err("Treasury is paused and not accepting deposits".to_string());
    }

    let now = ic_cdk::api::time();
    let record = DepositRecord {
        id: 0,
        deposit_type: args.deposit_type,
        asset_type: args.asset_type,
        amount: args.amount,
        block_index: args.block_index,
        timestamp: now,
        memo: args.memo,
    };
    record_interest_revenue_with_event(record, caller(), now)
}

fn record_interest_revenue_with_event(
    record: DepositRecord,
    deposit_caller: Principal,
    timestamp: u64,
) -> Result<u64, String> {
    let amount = record.amount;
    let (deposit_id, newly_recorded) = with_state_mut(|s| s.record_interest_revenue_once(record))?;
    if newly_recorded {
        with_state_mut(|s| {
            s.push_event_at(
                deposit_caller,
                TreasuryAction::Deposit {
                    deposit_type: DepositType::InterestRevenue,
                    asset_type: AssetType::ICUSD,
                    amount,
                },
                timestamp,
            )
        });
    }
    Ok(deposit_id)
}

fn record_external_token_deposit_with_event(
    args: ExternalTokenDepositArgs,
    deposit_caller: Principal,
    timestamp: u64,
) -> Result<u64, String> {
    let (deposit_id, newly_recorded) =
        with_state_mut(|s| s.record_external_token_deposit_once(args, timestamp))?;
    if newly_recorded {
        let record = with_state(|s| {
            s.deposits
                .get(&deposit_id)
                .ok_or_else(|| format!("new external token deposit {} is missing", deposit_id))
        })?;
        with_state_mut(|s| {
            s.push_event_at(
                deposit_caller,
                TreasuryAction::Deposit {
                    deposit_type: record.deposit_type,
                    asset_type: record.asset_type,
                    amount: record.amount,
                },
                timestamp,
            )
        });
    }
    Ok(deposit_id)
}

/// Apply one deposit and its audit event as a single synchronous state
/// transition. Exact BorrowingFee/ICUSD retries return the original ID and do
/// not create another event.
fn record_deposit_with_event(
    record: DepositRecord,
    deposit_caller: Principal,
    timestamp: u64,
) -> Result<u64, String> {
    let deposit_type = record.deposit_type.clone();
    let asset_type = record.asset_type.clone();
    let amount = record.amount;
    let (deposit_id, newly_recorded) = with_state_mut(|s| {
        if asset_type == AssetType::ICUSD {
            s.record_icusd_deposit_once(record)
        } else {
            Ok((s.try_add_deposit(record, true)?, true))
        }
    })?;

    if newly_recorded {
        with_state_mut(|s| {
            s.push_event_at(
                deposit_caller,
                TreasuryAction::Deposit {
                    deposit_type,
                    asset_type,
                    amount,
                },
                timestamp,
            )
        });
    }
    Ok(deposit_id)
}

/// Continue the bounded one-time migration of historical ICUSD deposit block
/// receipts. ICUSD ingress remains held until the final batch completes.
#[update]
#[candid_method(update)]
fn continue_icusd_deposit_block_backfill() -> Result<String, String> {
    ensure_controller()?;
    let (processed, next_deposit_id, complete) =
        with_state_mut(|s| s.continue_icusd_deposit_block_backfill())?;
    Ok(if complete {
        format!(
            "ICUSD deposit block backfill complete; processed {} rows",
            processed
        )
    } else {
        format!(
            "ICUSD deposit block backfill processed {} rows; next deposit ID {}",
            processed, next_deposit_id
        )
    })
}

/// Configure the only canister that may report a Stability Pool's own
/// unallocated icUSD interest. This is deliberately narrower than controller
/// access and cannot authorize withdrawals.
#[update]
#[candid_method(update)]
fn set_stability_pool_reporter(reporter: Option<Principal>) -> Result<(), String> {
    ensure_controller()?;
    with_state_mut(|s| s.set_stability_pool_reporter(reporter))
}

/// Record an icUSD transfer already made by the configured Stability Pool when
/// no opted-in icUSD depositor existed. The backend mint receipts make the
/// record exactly-once across SP retries and lost callback responses.
#[update]
#[candid_method(update)]
fn record_stability_pool_unallocated_interest(
    amount: u64,
    transfer_block_index: u64,
    source_mint_blocks: Vec<u64>,
) -> Result<u64, String> {
    let reporter = caller();
    let config = with_state(|s| s.get_config());
    if config.is_paused {
        return Err("Treasury is paused and not accepting deposits".to_string());
    }
    if config.stability_pool_reporter != Some(reporter) {
        return Err(
            "Access denied: caller is not the configured stability pool reporter".to_string(),
        );
    }
    let (deposit_id, newly_recorded) = with_state_mut(|s| {
        s.record_sp_unallocated_interest_once(amount, transfer_block_index, &source_mint_blocks)
    })?;
    if newly_recorded {
        with_state_mut(|s| {
            s.push_event(
                reporter,
                TreasuryAction::Deposit {
                    deposit_type: types::DepositType::InterestRevenue,
                    asset_type: types::AssetType::ICUSD,
                    amount,
                },
            )
        });
    }
    Ok(deposit_id)
}

/// Withdraw with a permanent request journal. A newly reserved intent may be
/// submitted once; unresolved existing intents are held pending reconciliation.
#[update]
#[candid_method(update)]
async fn withdraw(args: WithdrawArgs) -> Result<WithdrawResult, String> {
    ensure_controller()?;
    let caller_principal = caller();
    let request_id = args
        .request_id
        .ok_or_else(|| "An explicit request_id is required for withdrawals".to_string())?;
    validate_withdrawal_memo(args.memo.as_deref())?;
    let _guard = WithdrawalGuard::acquire(request_id)?;

    let existing = with_state(|s| s.withdrawal_intent(request_id));
    let intent = if let Some(mut existing) = existing {
        validate_withdrawal_identity(&existing, caller_principal, &args)?;
        if matches!(existing.status, WithdrawalIntentStatus::InFlight) {
            // An InFlight row belongs to a prior invocation or incarnation.
            // Without ledger-specific positive proof, it cannot be redispatched.
            existing.may_have_committed = true;
            existing.status = WithdrawalIntentStatus::TransportAmbiguous;
            with_state_mut(|s| s.set_withdrawal_intent(existing.clone()));
        }
        match &existing.status {
            WithdrawalIntentStatus::Completed(result) => return Ok(result.clone()),
            WithdrawalIntentStatus::Rejected { error } => {
                return Err(format!("Withdrawal request was rejected: {}", error))
            }
            WithdrawalIntentStatus::ConfirmedUnrepresentable { block_index } => {
                return Err(format!("Transfer confirmed with unrepresentable block index {}; request remains held (request_id={})", block_index, request_id))
            }
            WithdrawalIntentStatus::Held { reason } => {
                return Err(format!("Withdrawal request is held: {}", reason))
            }
            WithdrawalIntentStatus::InFlight | WithdrawalIntentStatus::TransportAmbiguous => {
                return Err(format!("Withdrawal request may have reached the ledger and is held; automatic redispatch is disabled (request_id={})", request_id))
            }
        }
    } else {
        let ledger = with_state(|s| {
            let c = s.get_config();
            match args.asset_type {
                AssetType::ICUSD => Some(c.icusd_ledger),
                AssetType::ICP => Some(c.icp_ledger),
                AssetType::CKBTC => c.ckbtc_ledger,
                AssetType::CKUSDT => c.ckusdt_ledger,
                AssetType::CKUSDC => c.ckusdc_ledger,
            }
        })
        .ok_or("Ledger not configured for this asset type")?;
        let fee = ledger_fee(ledger).await;
        let send_amount = withdrawal_send_amount(args.amount, fee)?;
        let memo = args
            .memo
            .clone()
            .map(String::into_bytes)
            .unwrap_or_else(|| request_id.to_be_bytes().to_vec());
        let proposed = WithdrawalIntent {
            request_id,
            caller: caller_principal,
            asset_type: args.asset_type.clone(),
            amount: args.amount,
            to: args.to,
            requested_memo: args.memo.clone(),
            ledger,
            send_amount,
            fee,
            transfer_memo: memo,
            created_at_time: ic_cdk::api::time(),
            may_have_committed: false,
            status: WithdrawalIntentStatus::InFlight,
        };
        let (intent, created) = with_state_mut(|s| s.reserve_withdrawal_intent(proposed))?;
        validate_withdrawal_identity(&intent, caller_principal, &args)?;
        if !created {
            return match &intent.status {
                WithdrawalIntentStatus::Completed(result) => Ok(result.clone()),
                WithdrawalIntentStatus::Rejected { error } => {
                    Err(format!("Withdrawal request was rejected: {}", error))
                }
                WithdrawalIntentStatus::ConfirmedUnrepresentable { block_index } => Err(format!(
                    "Transfer confirmed with unrepresentable block index {}; request remains held (request_id={})",
                    block_index, request_id
                )),
                WithdrawalIntentStatus::Held { reason } => {
                    Err(format!("Withdrawal request is held: {}", reason))
                }
                WithdrawalIntentStatus::InFlight | WithdrawalIntentStatus::TransportAmbiguous => {
                    let mut held = intent;
                    held.may_have_committed = true;
                    held.status = WithdrawalIntentStatus::TransportAmbiguous;
                    with_state_mut(|s| s.set_withdrawal_intent(held));
                    Err(format!("Withdrawal request may have reached the ledger and is held; automatic redispatch is disabled (request_id={})", request_id))
                }
            };
        }
        intent
    };

    match &intent.status {
        WithdrawalIntentStatus::Completed(result) => return Ok(result.clone()),
        WithdrawalIntentStatus::Rejected { error } => return Err(format!("Withdrawal request was rejected: {}", error)),
        WithdrawalIntentStatus::ConfirmedUnrepresentable { block_index } => return Err(format!("Transfer confirmed with unrepresentable block index {}; request remains held (request_id={})", block_index, request_id)),
        WithdrawalIntentStatus::Held { reason } => return Err(format!("Withdrawal request is held: {}", reason)),
        WithdrawalIntentStatus::InFlight => {}
        WithdrawalIntentStatus::TransportAmbiguous => {
            return Err(format!("Withdrawal request may have reached the ledger and is held; automatic redispatch is disabled (request_id={})", request_id));
        }
    }
    if !withdrawal_intent_is_dispatchable(&intent) {
        return Err(format!(
            "Withdrawal request is held pending positive ledger-specific reconciliation (request_id={})",
            request_id
        ));
    }
    let transfer = TransferArg {
        from_subaccount: None,
        to: Account {
            owner: intent.to,
            subaccount: None,
        },
        amount: intent.send_amount.into(),
        fee: Some(intent.fee.into()),
        memo: Some(intent.transfer_memo.clone().into()),
        created_at_time: Some(intent.created_at_time),
    };
    match call_ledger_transfer_exact(intent.ledger, transfer).await {
        Ok(TransferOutcome::Applied(block)) | Ok(TransferOutcome::Duplicate(block)) => {
            with_state_mut(|s| {
                s.complete_withdrawal_intent(
                    request_id,
                    WithdrawResult {
                        block_index: block,
                        amount_transferred: intent.send_amount,
                        fee: intent.fee,
                    },
                    intent.caller,
                    ic_cdk::api::time(),
                )
            })
        }
        Ok(TransferOutcome::ConfirmedUnrepresentable(block)) => {
            let mut intent = intent;
            intent.may_have_committed = true;
            intent.status = WithdrawalIntentStatus::ConfirmedUnrepresentable {
                block_index: block.clone(),
            };
            with_state_mut(|s| s.set_withdrawal_intent(intent));
            Err(format!("Transfer confirmed with unrepresentable block index {}; request held (request_id={})", block, request_id))
        }
        Err(LedgerError::Ledger(error)) => {
            let message = format!("{:?}", error);
            refresh_cached_fee_from_bad_fee(intent.ledger, &error);
            let no_effect = transfer_error_proves_no_effect(&error);
            if !no_effect || intent.may_have_committed {
                let mut intent = intent;
                intent.may_have_committed = true;
                intent.status = WithdrawalIntentStatus::TransportAmbiguous;
                with_state_mut(|s| s.set_withdrawal_intent(intent));
                return Err(format!("Ledger response does not prove that this request had no prior effect; request held (request_id={})", request_id));
            }
            with_state_mut(|s| s.reject_withdrawal_intent(request_id, message.clone()));
            Err(format!("Transfer failed: {}", message))
        }
        Err(LedgerError::Transport(error)) => {
            let mut intent = intent;
            intent.may_have_committed = true;
            intent.status = WithdrawalIntentStatus::TransportAmbiguous;
            with_state_mut(|s| s.set_withdrawal_intent(intent));
            log!(LOG, "RECONCILIATION REQUIRED: ambiguous withdrawal request {}. Automatic redispatch is disabled absent positive ledger-specific proof. Error: {}", request_id, error);
            Err(format!(
                "Transport error: {} (reconciliation required, request_id={})",
                error, request_id
            ))
        }
    }
}

fn validate_withdrawal_identity(
    intent: &WithdrawalIntent,
    caller: Principal,
    args: &WithdrawArgs,
) -> Result<(), String> {
    if intent.caller != caller
        || intent.asset_type != args.asset_type
        || intent.amount != args.amount
        || intent.to != args.to
        || intent.requested_memo != args.memo
    {
        return Err("Withdrawal request_id is already bound to a different request".into());
    }
    Ok(())
}

fn validate_withdrawal_memo(memo: Option<&str>) -> Result<(), String> {
    if memo.is_some_and(|memo| memo.len() > MAX_WITHDRAWAL_MEMO_BYTES) {
        return Err(format!(
            "Withdrawal memo exceeds {} bytes",
            MAX_WITHDRAWAL_MEMO_BYTES
        ));
    }
    Ok(())
}

fn withdrawal_intent_is_dispatchable(intent: &WithdrawalIntent) -> bool {
    !intent.may_have_committed && matches!(intent.status, WithdrawalIntentStatus::InFlight)
}

/// Distinguishes a clear ledger rejection from an ambiguous transport error.
/// The withdraw flow restores the bookkeeping balance only on the former.
enum LedgerError {
    Ledger(TransferError),
    Transport(String),
}

enum TransferOutcome {
    Applied(u64),
    Duplicate(u64),
    ConfirmedUnrepresentable(String),
}

fn classify_block_index(block: candid::Nat, duplicate: bool) -> TransferOutcome {
    match block.0.clone().try_into() {
        Ok(index) if duplicate => TransferOutcome::Duplicate(index),
        Ok(index) => TransferOutcome::Applied(index),
        Err(_) => TransferOutcome::ConfirmedUnrepresentable(block.0.to_string()),
    }
}

fn transfer_error_proves_no_effect(error: &TransferError) -> bool {
    matches!(
        error,
        TransferError::BadFee { .. }
            | TransferError::BadBurn { .. }
            | TransferError::InsufficientFunds { .. }
            | TransferError::TooOld
            | TransferError::CreatedInFuture { .. }
    )
}

/// Preserve oversized Nat indices as a confirmed-but-unrepresentable result.
/// Converting them to zero or a typed failure would lose payment evidence.
async fn call_ledger_transfer_exact(
    ledger_principal: Principal,
    args: TransferArg,
) -> Result<TransferOutcome, LedgerError> {
    let outer: Result<(Result<candid::Nat, TransferError>,), _> =
        ic_cdk::call(ledger_principal, "icrc1_transfer", (args,)).await;
    match outer {
        Err((code, msg)) => Err(LedgerError::Transport(format!("{:?}: {}", code, msg))),
        Ok((Ok(block),)) => Ok(classify_block_index(block, false)),
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            Ok(classify_block_index(duplicate_of, true))
        }
        Ok((Err(error),)) => Err(LedgerError::Ledger(error)),
    }
}

/// Get treasury status
#[query]
#[candid_method(query)]
fn get_status() -> TreasuryStatus {
    with_state(|s| {
        let config = s.get_config();
        let balances = s
            .balances
            .iter()
            .map(|(asset_type, balance)| (asset_type.clone(), balance.clone()))
            .collect();

        TreasuryStatus {
            total_deposits: s.get_deposits_count(),
            balances,
            controller: ic_cdk::api::id(), // show canister's own principal
            is_paused: config.is_paused,
        }
    })
}

/// Bounded lookup for one durable withdrawal request, restricted to
/// controllers because it exposes treasury operations and reconciliation state.
#[query]
#[candid_method(query)]
fn get_withdrawal_intent_status(
    request_id: u64,
) -> Result<Option<types::WithdrawalIntentStatusView>, String> {
    ensure_controller()?;
    Ok(with_state(|s| {
        s.withdrawal_intent(request_id)
            .map(|intent| types::WithdrawalIntentStatusView {
                request_id: intent.request_id,
                asset_type: intent.asset_type,
                amount: intent.amount,
                to: intent.to,
                ledger: intent.ledger,
                created_at_time: intent.created_at_time,
                may_have_committed: intent.may_have_committed,
                status: intent.status,
            })
    }))
}

/// Return one unresolved withdrawal so controllers can recover the request ID
/// even if browser-local request storage is lost.
#[query]
#[candid_method(query)]
fn get_active_withdrawal_intent_status() -> Result<Option<types::WithdrawalIntentStatusView>, String>
{
    ensure_controller()?;
    Ok(with_state(|s| {
        s.active_withdrawal_intent()
            .map(|intent| types::WithdrawalIntentStatusView {
                request_id: intent.request_id,
                asset_type: intent.asset_type,
                amount: intent.amount,
                to: intent.to,
                ledger: intent.ledger,
                created_at_time: intent.created_at_time,
                may_have_committed: intent.may_have_committed,
                status: intent.status,
            })
    }))
}

/// Let controller frontends determine their authorization without comparing
/// the wallet principal to the Treasury canister principal.
#[query]
#[candid_method(query)]
fn is_caller_controller() -> bool {
    ic_cdk::api::is_controller(&caller())
}

#[query]
#[candid_method(query)]
fn cycles_status() -> rumi_cycle_manager::CycleManagerCyclesStatus {
    let operational = with_state(|s| !s.get_config().is_paused);
    rumi_cycle_manager::self_cycles_status(
        3_000_000_000_000,
        operational,
        rumi_cycle_manager::DEFAULT_FREEZE_THRESHOLD_SECS,
    )
}

#[query]
#[candid_method(query)]
fn cycle_manager_metrics() -> Vec<rumi_cycle_manager::CycleManagerMetric> {
    with_state(|s| {
        vec![
            rumi_cycle_manager::metric(
                "op:deposit:count",
                s.get_deposits_count(),
                s.get_deposits_count(),
                Some("treasury deposit records"),
            ),
            rumi_cycle_manager::metric(
                "op:event:count",
                s.get_events_count(),
                s.get_events_count(),
                Some("treasury event records"),
            ),
            rumi_cycle_manager::metric(
                "ledger:asset:count",
                s.balances.len() as u64,
                s.balances.len() as u64,
                Some("tracked treasury assets"),
            ),
        ]
    })
}

/// Get deposit history (paginated)
#[query]
#[candid_method(query)]
fn get_deposits(start: Option<u64>, limit: Option<usize>) -> Vec<DepositRecord> {
    let limit = limit.unwrap_or(100).min(1000); // Cap at 1000
    with_state(|s| s.get_deposits(start, limit))
}

/// Get treasury events (paginated)
#[query]
#[candid_method(query)]
fn get_events(start: Option<u64>, limit: Option<usize>) -> Vec<TreasuryEvent> {
    let limit = limit.unwrap_or(100).min(1000);
    with_state(|s| s.get_events(start, limit))
}

/// Get total number of treasury events
#[query]
#[candid_method(query)]
fn get_event_count() -> u64 {
    with_state(|s| s.get_events_count())
}

/// Pause/unpause treasury (controllers only)
#[update]
#[candid_method(update)]
fn set_paused(paused: bool) -> Result<(), String> {
    ensure_controller()?;
    let c = caller();
    log!(LOG, "Setting treasury paused state to: {}", paused);
    let result = with_state_mut(|s| s.set_paused(paused));
    if result.is_ok() {
        with_state_mut(|s| s.push_event(c, TreasuryAction::SetPaused { paused }));
    }
    result
}

// Export the generated Candid service definition.
// Export candid interface
candid::export_service!();

#[query(name = "__get_candid_interface_tmp_hack")]
fn export_candid() -> String {
    __export_service()
}

#[cfg(test)]
mod withdrawal_conversion_tests {
    use super::*;

    #[test]
    fn oversized_ok_and_duplicate_blocks_remain_confirmed_and_unrepresentable() {
        let oversized = candid::Nat::from((u64::MAX as u128) + 1);
        assert!(matches!(
            classify_block_index(oversized.clone(), false),
            TransferOutcome::ConfirmedUnrepresentable(ref value) if value == "18446744073709551616"
        ));
        assert!(matches!(
            classify_block_index(oversized, true),
            TransferOutcome::ConfirmedUnrepresentable(ref value) if value == "18446744073709551616"
        ));
    }

    #[test]
    fn overlong_utf8_memo_is_rejected_before_intent_reservation() {
        // `é` is two UTF-8 bytes, so this is 33 bytes despite 17 characters.
        let memo = format!("{}a", "é".repeat(16));
        assert_eq!(memo.len(), 33);
        assert!(validate_withdrawal_memo(Some(&memo)).is_err());
        assert!(validate_withdrawal_memo(Some(&"a".repeat(32))).is_ok());
    }

    #[test]
    fn bad_fee_refreshes_cache_only_for_representable_expected_fee() {
        let ledger = Principal::from_slice(&[77]);
        LEDGER_FEES.with(|cache| cache.borrow_mut().insert(ledger, 10));
        refresh_cached_fee_from_bad_fee(
            ledger,
            &TransferError::BadFee {
                expected_fee: 25u64.into(),
            },
        );
        assert_eq!(
            LEDGER_FEES.with(|cache| cache.borrow().get(&ledger).copied()),
            Some(25)
        );

        refresh_cached_fee_from_bad_fee(
            ledger,
            &TransferError::BadFee {
                expected_fee: candid::Nat::from((u64::MAX as u128) + 1),
            },
        );
        assert_eq!(
            LEDGER_FEES.with(|cache| cache.borrow().get(&ledger).copied()),
            Some(25)
        );
        LEDGER_FEES.with(|cache| cache.borrow_mut().remove(&ledger));
    }

    #[test]
    fn persisted_or_ambiguous_intents_are_never_automatically_redispatched() {
        let mut intent = WithdrawalIntent {
            request_id: 99,
            caller: Principal::from_slice(&[1]),
            asset_type: AssetType::ICUSD,
            amount: 100_000,
            to: Principal::from_slice(&[2]),
            requested_memo: None,
            ledger: Principal::from_slice(&[3]),
            send_amount: 99_990,
            fee: 10,
            transfer_memo: 99u64.to_be_bytes().to_vec(),
            created_at_time: 1,
            may_have_committed: false,
            status: WithdrawalIntentStatus::InFlight,
        };
        assert!(withdrawal_intent_is_dispatchable(&intent));
        intent.may_have_committed = true;
        assert!(!withdrawal_intent_is_dispatchable(&intent));
        intent.may_have_committed = false;
        intent.status = WithdrawalIntentStatus::TransportAmbiguous;
        assert!(!withdrawal_intent_is_dispatchable(&intent));
    }

    #[test]
    fn only_known_no_effect_ledger_variants_allow_compensation() {
        assert!(transfer_error_proves_no_effect(&TransferError::TooOld));
        assert!(transfer_error_proves_no_effect(
            &TransferError::CreatedInFuture { ledger_time: 1 }
        ));
        assert!(!transfer_error_proves_no_effect(
            &TransferError::TemporarilyUnavailable
        ));
        assert!(!transfer_error_proves_no_effect(
            &TransferError::GenericError {
                error_code: 1u64.into(),
                message: "unknown commit state".into(),
            }
        ));
    }
}
