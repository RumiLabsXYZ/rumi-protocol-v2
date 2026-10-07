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
use serde::Deserialize;
use sha2::{Digest, Sha224};
use state::{
    init_state, restore_state, with_state, with_state_mut, WithdrawalRequestRecord,
    WithdrawalRequestStatus, WithdrawalStart,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use types::{
    AssetType, DepositArgs, DepositRecord, DepositRecordV1, PendingWithdrawalsPageV2,
    TreasuryAction, TreasuryEventV1, TreasuryInitArgs, TreasuryStatus, TreasuryStatusV2,
    UnknownTreasuryEvidencePageV2, WithdrawArgs, WithdrawResult,
};

// Declare log buffer for debugging
declare_log_buffer!(name = LOG, capacity = 1000);

/// Standard ICRC-1 transfer fee (e8s), used as a conservative fallback when a
/// ledger's `icrc1_fee` query cannot be reached. Erring high keeps the
/// treasury solvent (we send slightly less) rather than risking an over-send.
const DEFAULT_LEDGER_FEE_E8S: u64 = 10_000;

thread_local! {
    /// Per-ledger transfer-fee cache, populated lazily from `icrc1_fee` on the
    /// first withdrawal against a ledger. Heap-only (not persisted), so it is
    /// simply re-warmed after an upgrade.
    static LEDGER_FEES: RefCell<HashMap<Principal, u64>> = RefCell::new(HashMap::new());
    /// Prevent two update executions from dispatching the same durable request
    /// concurrently while either one is suspended at an inter-canister await.
    static WITHDRAWAL_REQUESTS_IN_FLIGHT: RefCell<HashSet<u64>> = RefCell::new(HashSet::new());
}

struct WithdrawalRequestGuard(u64);

impl WithdrawalRequestGuard {
    fn try_new(request_id: u64) -> Option<Self> {
        WITHDRAWAL_REQUESTS_IN_FLIGHT.with(|active| {
            if active.borrow_mut().insert(request_id) {
                Some(Self(request_id))
            } else {
                None
            }
        })
    }
}

impl Drop for WithdrawalRequestGuard {
    fn drop(&mut self) {
        WITHDRAWAL_REQUESTS_IN_FLIGHT.with(|active| {
            active.borrow_mut().remove(&self.0);
        });
    }
}

/// Fetch a ledger's transfer fee, caching only a successful, representable
/// response. On query failure, use the standard ICRC-1 fee for this attempt
/// without caching it, so a transient outage cannot pin the fallback forever.
async fn ledger_fee(ledger: Principal) -> u64 {
    if let Some(fee) = LEDGER_FEES.with(|c| c.borrow().get(&ledger).copied()) {
        return fee;
    }
    let result: Result<(candid::Nat,), _> = ic_cdk::call(ledger, "icrc1_fee", ()).await;
    let fee = result.ok().and_then(|(fee,)| fee.0.try_into().ok());
    remember_queried_ledger_fee(ledger, fee)
}

fn remember_queried_ledger_fee(ledger: Principal, fee: Option<u64>) -> u64 {
    if let Some(fee) = fee {
        LEDGER_FEES.with(|c| c.borrow_mut().insert(ledger, fee));
        fee
    } else {
        DEFAULT_LEDGER_FEE_E8S
    }
}

/// A first-dispatch BadFee proves that its pinned tuple had no effect, so its
/// expected fee can refresh the cache before the reservation is released.
/// After a later BadFee, keep the durable tuple held and invalidate only this
/// heap cache so future request IDs query the ledger again.
fn refresh_ledger_fee_from_bad_fee(
    ledger: Principal,
    dispatch_attempt: Option<u32>,
    expected_fee: &candid::Nat,
) {
    if dispatch_attempt != Some(1) {
        LEDGER_FEES.with(|c| c.borrow_mut().remove(&ledger));
        return;
    }
    let fee = expected_fee.0.clone().try_into().ok();
    LEDGER_FEES.with(|c| {
        let mut fees = c.borrow_mut();
        if let Some(fee) = fee {
            fees.insert(ledger, fee);
        } else {
            fees.remove(&ledger);
        }
    });
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
    assert!(
        validate_distinct_ledgers(&args).is_ok(),
        "refusing to initialize treasury with aliased asset ledgers"
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

    log!(
        LOG,
        "Processing deposit: {:?} {} {:?}",
        args.deposit_type,
        args.amount,
        args.asset_type
    );

    let dep_type = args.deposit_type.clone();
    let asset = args.asset_type.clone();
    let amount = args.amount;
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

    let (deposit_id, is_new) = with_state_mut(|s| s.add_deposit_once(record))?;

    if is_new {
        with_state_mut(|s| {
            s.push_event(
                deposit_caller,
                TreasuryAction::Deposit {
                    deposit_type: dep_type,
                    asset_type: asset,
                    amount,
                },
            )
        });
    }

    log!(LOG, "Deposit {} recorded successfully", deposit_id);
    Ok(deposit_id)
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

/// Withdraw funds from treasury (controllers only).
///
/// Audit Wave-3 (ICRC-002/ICRC-003) hardening:
/// - The recipient bears the ledger fee: bookkeeping is debited `amount` and
///   `amount - fee` goes on the wire, so the canister account drops by exactly
///   `amount` (the fee is queried via `icrc1_fee` with a per-ledger cache and
///   a conservative fallback, mirroring the AMM's PR #230 fix).
/// - `created_at_time` is the FIRST attempt's timestamp for this `request_id`,
///   persisted in stable memory and reused on retries, so the ledger's dedup
///   window actually catches a re-submitted transfer. `Duplicate` is treated
///   as success.
/// - The balance is restored ONLY for clear ledger errors; on a
///   transport-layer error it stays deducted while a reconciliation hint is
///   logged for the controller.
#[update]
#[candid_method(update)]
async fn withdraw(args: WithdrawArgs) -> Result<WithdrawResult, String> {
    ensure_controller()?;
    let caller_principal = caller();
    let request_id = args.request_id.unwrap_or_else(|| {
        derive_request_id(&caller_principal, &args.asset_type, args.amount, &args.to)
    });
    let Some(_request_guard) = WithdrawalRequestGuard::try_new(request_id) else {
        return Err(format!(
            "Withdrawal request {} is already being processed",
            request_id
        ));
    };

    log!(
        LOG,
        "Processing withdrawal: {} {:?} to {}",
        args.amount,
        args.asset_type,
        args.to
    );

    // Resolve the configured ledger before looking up the request. Retries are
    // bound to this exact ledger as well as the caller-supplied tuple.
    let (config, configured_ledger) = with_state(|s| {
        let config = s.get_config();
        let ledger = configured_asset_ledger(&config, &args.asset_type);
        (config, ledger)
    });
    let ledger_principal = configured_ledger.ok_or("Ledger not configured for this asset type")?;
    validate_configured_asset_ledger(&config, &args.asset_type)?;

    let requested = WithdrawalRequestRecord {
        caller: caller_principal,
        asset_type: args.asset_type.clone(),
        ledger: ledger_principal,
        amount: args.amount,
        to: args.to,
        memo: args.memo.clone(),
        created_at_time: 0,
        send_amount: 0,
        fee: 0,
        status: WithdrawalRequestStatus::Pending,
        dispatch_attempts: Some(0),
    };
    let existing = with_state(|s| s.withdrawal_requests.get(&request_id));
    let record = if existing.is_some() {
        match with_state_mut(|s| s.begin_withdrawal(request_id, requested.clone()))? {
            WithdrawalStart::Complete {
                record,
                block_index,
            } => {
                return Ok(WithdrawResult {
                    block_index,
                    amount_transferred: record.send_amount,
                    fee: record.fee,
                });
            }
            WithdrawalStart::Retry(record) => record,
            WithdrawalStart::Transfer(_) => {
                return Err("Withdrawal request state changed unexpectedly".into())
            }
        }
    } else {
        // Query fee only for a genuinely new request. A pending retry reuses
        // the exact fee, send amount, timestamp, and ledger tuple persisted
        // before its first transfer attempt.
        let fee = ledger_fee(ledger_principal).await;
        let send_amount = withdrawal_send_amount(args.amount, fee)?;
        let requested = WithdrawalRequestRecord {
            created_at_time: ic_cdk::api::time(),
            send_amount,
            fee,
            ..requested
        };
        match with_state_mut(|s| s.begin_withdrawal(request_id, requested))? {
            WithdrawalStart::Transfer(record) => record,
            WithdrawalStart::Retry(record) => record,
            WithdrawalStart::Complete {
                record,
                block_index,
            } => {
                return Ok(WithdrawResult {
                    block_index,
                    amount_transferred: record.send_amount,
                    fee: record.fee,
                });
            }
        }
    };

    let transfer_args = TransferArg {
        from_subaccount: None,
        to: Account {
            owner: args.to,
            subaccount: None,
        },
        amount: record.send_amount.into(),
        fee: Some(record.fee.into()),
        memo: args
            .memo
            .clone()
            .map(|m| m.into_bytes().into())
            .or_else(|| Some(request_id.to_be_bytes().to_vec().into())),
        created_at_time: Some(record.created_at_time),
    };

    // The persisted marker makes callback loss, traps, and upgrades
    // distinguishable from a definitive rejection on a proven first attempt.
    let dispatch_attempt = with_state_mut(|s| s.mark_withdrawal_dispatch_attempt(request_id))?;
    let block_index = match call_ledger_transfer(ledger_principal, transfer_args).await {
        Ok(block_index) => block_index,
        Err(LedgerError::Duplicate { duplicate_of }) => {
            verify_withdrawal_receipt(&record, request_id, duplicate_of)
                .await
                .map_err(|error| format!(
                    "Duplicate block {} did not prove the stored withdrawal tuple; request remains held for reconciliation: {}",
                    duplicate_of, error
                ))?;
            log!(
                LOG,
                "Withdrawal Duplicate block {} verified against the stored transfer tuple",
                duplicate_of
            );
            duplicate_of
        }
        Err(LedgerError::Ledger(e)) => {
            if let TransferError::BadFee { expected_fee } = &e {
                refresh_ledger_fee_from_bad_fee(ledger_principal, dispatch_attempt, expected_fee);
            }
            if dispatch_attempt == Some(1) && is_definitive_no_effect_transfer_error(&e) {
                with_state_mut(|s| s.abort_withdrawal(request_id))?;
                return Err(format!("Transfer failed: {:?}", e));
            }
            return Err(format!(
                "Transfer retry rejected: {:?}; an earlier dispatch may have committed, so the reservation remains held for reconciliation (request_id={})",
                e, request_id
            ));
        }
        Err(LedgerError::Transport(msg)) => {
            log!(LOG,
                "RECONCILIATION REQUIRED: transport error during withdrawal of {} {:?} to {} (request_id {}). \
                 Balance NOT restored — the transfer may have committed. Verify on-chain via ledger \
                 icrc3_get_blocks before retrying or reconciling. Error: {}",
                args.amount, args.asset_type, args.to, request_id, msg
            );
            return Err(format!(
                "Transport error: {} (reconciliation required, request_id={})",
                msg, request_id
            ));
        }
    };

    let newly_completed = with_state_mut(|s| s.complete_withdrawal(request_id, block_index))?;
    if newly_completed {
        with_state_mut(|s| {
            s.push_event(
                caller_principal,
                TreasuryAction::Withdraw {
                    asset_type: args.asset_type.clone(),
                    amount: args.amount,
                    to: args.to,
                },
            )
        });
    }

    log!(LOG, "Withdrawal completed, block index: {}", block_index);

    Ok(WithdrawResult {
        block_index,
        amount_transferred: record.send_amount,
        fee: record.fee,
    })
}

fn validate_distinct_ledgers(args: &TreasuryInitArgs) -> Result<(), String> {
    let mut configured = HashSet::new();
    let mut add = |asset: &str, ledger: Option<Principal>| -> Result<(), String> {
        if let Some(ledger) = ledger {
            if ledger == Principal::anonymous() {
                return Err(format!("{} ledger cannot be anonymous", asset));
            }
            if !configured.insert(ledger) {
                return Err(format!(
                    "{} ledger aliases another configured treasury asset ledger",
                    asset
                ));
            }
        }
        Ok(())
    };
    add("icUSD", Some(args.icusd_ledger))?;
    add("ICP", Some(args.icp_ledger))?;
    add("ckBTC", args.ckbtc_ledger)?;
    add("ckUSDT", args.ckusdt_ledger)?;
    add("ckUSDC", args.ckusdc_ledger)?;
    Ok(())
}

fn validate_configured_asset_ledger(
    config: &state::TreasuryConfig,
    asset: &AssetType,
) -> Result<(), String> {
    let selected = configured_asset_ledger(config, asset)
        .ok_or("Ledger not configured for this asset type")?;
    if selected == Principal::anonymous() {
        return Err("Asset ledger cannot be anonymous".into());
    }
    let configured = [
        ("icUSD", Some(config.icusd_ledger)),
        ("ICP", Some(config.icp_ledger)),
        ("ckBTC", config.ckbtc_ledger),
        ("ckUSDT", config.ckusdt_ledger),
        ("ckUSDC", config.ckusdc_ledger),
    ];
    let selected_name = match asset {
        AssetType::ICUSD => "icUSD",
        AssetType::ICP => "ICP",
        AssetType::CKBTC => "ckBTC",
        AssetType::CKUSDT => "ckUSDT",
        AssetType::CKUSDC => "ckUSDC",
        AssetType::Other(_) => "other asset",
    };
    if let Some((other, _)) = configured
        .iter()
        .find(|entry| entry.0 != selected_name && entry.1 == Some(selected))
    {
        return Err(format!(
            "Withdrawals for {:?} are held because its ledger aliases configured asset {}",
            asset, other
        ));
    }
    Ok(())
}

fn configured_asset_ledger(config: &state::TreasuryConfig, asset: &AssetType) -> Option<Principal> {
    match asset {
        AssetType::ICUSD => Some(config.icusd_ledger),
        AssetType::ICP => Some(config.icp_ledger),
        AssetType::CKBTC => config.ckbtc_ledger,
        AssetType::CKUSDT => config.ckusdt_ledger,
        AssetType::CKUSDC => config.ckusdc_ledger,
        AssetType::Other(ledger) => Some(*ledger),
    }
}

/// Derive a stable request_id from withdrawal args when the caller doesn't
/// supply one. Bucketing the timestamp at one-minute resolution so a
/// same-minute retry produces the same id (and therefore the same
/// `created_at_time` at the ledger, enabling dedup).
fn derive_request_id(
    caller_principal: &Principal,
    asset: &AssetType,
    amount: u64,
    to: &Principal,
) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    caller_principal.as_slice().hash(&mut h);
    format!("{:?}", asset).hash(&mut h);
    amount.hash(&mut h);
    to.as_slice().hash(&mut h);
    let bucket = ic_cdk::api::time() / 60_000_000_000;
    bucket.hash(&mut h);
    h.finish()
}

/// Distinguishes a clear ledger rejection from an ambiguous transport error.
/// The withdraw flow restores the bookkeeping balance only on the former.
#[derive(Debug)]
enum LedgerError {
    Duplicate { duplicate_of: u64 },
    Ledger(TransferError),
    Transport(String),
}

/// Only ICRC-1 errors whose documented semantics prove that no transfer was
/// applied may release a first-attempt reservation. GenericError and
/// TemporarilyUnavailable remain ambiguous, even on the first dispatch.
fn is_definitive_no_effect_transfer_error(error: &TransferError) -> bool {
    matches!(
        error,
        TransferError::BadFee { .. }
            | TransferError::BadBurn { .. }
            | TransferError::InsufficientFunds { .. }
            | TransferError::TooOld
            | TransferError::CreatedInFuture { .. }
    )
}

/// Get treasury status
#[query]
#[candid_method(query)]
fn get_status() -> TreasuryStatus {
    with_state(|s| {
        let config = s.get_config();
        let balances = legacy_asset_balances(&s.balances);

        TreasuryStatus {
            total_deposits: s.get_deposits_count(),
            balances,
            controller: ic_cdk::api::id(), // show canister's own principal
            is_paused: config.is_paused,
        }
    })
}

fn legacy_asset_balances(
    balances: &HashMap<AssetType, types::AssetBalance>,
) -> Vec<(types::AssetTypeV1, types::AssetBalance)> {
    balances
        .iter()
        .filter_map(|(asset_type, balance)| {
            types::AssetTypeV1::try_from(asset_type.clone())
                .ok()
                .map(|asset_type| (asset_type, balance.clone()))
        })
        .collect()
}

/// Full treasury status, including balances for address-bound collateral ledgers.
#[query]
#[candid_method(query)]
fn get_status_v2() -> TreasuryStatusV2 {
    with_state(|s| {
        let config = s.get_config();
        TreasuryStatusV2 {
            total_deposits: s.get_deposits_count(),
            balances: s
                .balances
                .iter()
                .map(|(asset_type, balance)| (asset_type.clone(), balance.clone()))
                .collect(),
            controller: ic_cdk::api::id(),
            is_paused: config.is_paused,
        }
    })
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
fn get_deposits(start: Option<u64>, limit: Option<usize>) -> Vec<DepositRecordV1> {
    let limit = limit.unwrap_or(100).min(1000); // Cap at 1000
    with_state(|s| s.get_deposits(start, limit))
}

/// Full deposit history, including address-bound collateral asset identities.
#[query]
#[candid_method(query)]
fn get_deposits_v2(start: Option<u64>, limit: Option<usize>) -> Vec<DepositRecord> {
    let limit = limit.unwrap_or(100).min(1000);
    with_state(|s| s.get_deposits_v2(start, limit))
}

/// Get treasury events (paginated)
#[query]
#[candid_method(query)]
fn get_events(start: Option<u64>, limit: Option<usize>) -> Vec<TreasuryEventV1> {
    let limit = limit.unwrap_or(100).min(1000);
    with_state(|s| s.get_events(start, limit))
}

/// Full event history, including address-bound collateral asset identities.
#[query]
#[candid_method(query)]
fn get_events_v2(start: Option<u64>, limit: Option<usize>) -> Vec<types::TreasuryEvent> {
    let limit = limit.unwrap_or(100).min(1000);
    with_state(|s| s.get_events_v2(start, limit))
}

/// Raw stable records that could not be interpreted by the current schema.
/// V1 history methods omit these records; this V2 method exposes their bytes
/// without presenting them as known fees or actions.
#[query]
#[candid_method(query)]
fn get_unknown_evidence_v2(
    start: Option<u64>,
    limit: Option<usize>,
) -> Result<UnknownTreasuryEvidencePageV2, String> {
    ensure_controller()?;
    let limit = limit.unwrap_or(100).min(100);
    Ok(with_state(|s| s.get_unknown_evidence_v2(start, limit)))
}

/// Controller-only, bounded inventory for pending and legacy-ambiguous
/// withdrawal requests. The cursor advances over raw stable keys.
#[query]
#[candid_method(query)]
fn get_pending_withdrawals_v2(
    start: Option<u64>,
    limit: Option<usize>,
) -> Result<PendingWithdrawalsPageV2, String> {
    ensure_controller()?;
    let limit = limit.unwrap_or(100).clamp(1, 100);
    Ok(with_state(|s| s.get_pending_withdrawals_v2(start, limit)))
}

/// Full pending withdrawals, including address-bound collateral asset IDs.
#[query]
#[candid_method(query)]
fn get_pending_withdrawals_v3(
    start: Option<u64>,
    limit: Option<usize>,
) -> Result<types::PendingWithdrawalsPageV3, String> {
    ensure_controller()?;
    let limit = limit.unwrap_or(100).min(500);
    Ok(with_state(|s| s.get_pending_withdrawals_v3(start, limit)))
}

/// Verify and complete one held withdrawal using positive ledger history
/// evidence. The caller supplies only the candidate block index; all transfer
/// fields are checked against the immutable stored request tuple.
#[update]
#[candid_method(update)]
async fn reconcile_withdrawal_receipt_v2(
    request_id: u64,
    block_index: u64,
) -> Result<WithdrawResult, String> {
    ensure_controller()?;
    let record = with_state(|s| s.withdrawal_requests.get(&request_id))
        .ok_or_else(|| format!("Unknown withdrawal request {request_id}"))?;
    match record.status {
        WithdrawalRequestStatus::Complete { block_index: prior } if prior == block_index => {
            return Ok(WithdrawResult {
                block_index: prior,
                amount_transferred: record.send_amount,
                fee: record.fee,
            });
        }
        WithdrawalRequestStatus::Complete { .. } => {
            return Err("Withdrawal is already complete with a different receipt".into());
        }
        WithdrawalRequestStatus::LegacyUnknown => {
            return Err(
                "Legacy-unknown withdrawal cannot be reconciled without schema evidence".into(),
            );
        }
        WithdrawalRequestStatus::Pending => {}
    }

    verify_withdrawal_receipt(&record, request_id, block_index).await?;
    let newly_completed = with_state_mut(|s| s.complete_withdrawal(request_id, block_index))?;
    if newly_completed {
        with_state_mut(|s| {
            s.push_event(
                record.caller,
                TreasuryAction::Withdraw {
                    asset_type: record.asset_type.clone(),
                    amount: record.amount,
                    to: record.to,
                },
            )
        });
    }
    Ok(WithdrawResult {
        block_index,
        amount_transferred: record.send_amount,
        fee: record.fee,
    })
}

async fn verify_withdrawal_receipt(
    record: &WithdrawalRequestRecord,
    request_id: u64,
    block_index: u64,
) -> Result<(), String> {
    if record.asset_type == AssetType::ICP {
        let block = query_native_icp_block(record.ledger, block_index).await?;
        verify_native_icp_withdrawal(&block, record, request_id, ic_cdk::id())
    } else {
        let block = query_icrc3_block(record.ledger, block_index).await?;
        verify_icrc3_withdrawal(&block, record, request_id, ic_cdk::id())
    }
}

#[derive(candid::CandidType, Deserialize, Clone, Debug, PartialEq)]
enum Icrc3ValueV2 {
    Blob(Vec<u8>),
    Text(String),
    Nat(candid::Nat),
    Int(candid::Int),
    Array(Vec<Icrc3ValueV2>),
    Map(Vec<(String, Icrc3ValueV2)>),
}

#[derive(candid::CandidType, Deserialize, Clone, Debug)]
struct Icrc3BlockWithIdV2 {
    id: candid::Nat,
    block: Icrc3ValueV2,
}

#[derive(candid::CandidType, Deserialize, Clone, Debug)]
struct Icrc3GetBlocksV2 {
    log_length: candid::Nat,
    blocks: Vec<Icrc3BlockWithIdV2>,
    archived_blocks: Vec<Icrc3ArchivedBlocksV2>,
}

#[derive(candid::CandidType, Deserialize, Clone, Debug)]
struct Icrc3ArchivedBlocksV2 {
    args: Vec<Icrc3GetBlocksArgsV2>,
    callback: Icrc3ArchiveCallbackV2,
}

#[derive(candid::CandidType, Deserialize, Clone, Debug)]
struct Icrc3ArchiveCallbackV2 {
    canister_id: Principal,
    method: String,
}

#[derive(candid::CandidType, Deserialize, Clone, Debug)]
struct Icrc3GetBlocksArgsV2 {
    start: candid::Nat,
    length: candid::Nat,
}

async fn query_icrc3_block(ledger: Principal, index: u64) -> Result<Icrc3ValueV2, String> {
    let args = vec![Icrc3GetBlocksArgsV2 {
        start: index.into(),
        length: 1u8.into(),
    }];
    let (main,): (Icrc3GetBlocksV2,) = ic_cdk::call(ledger, "icrc3_get_blocks", (args.clone(),))
        .await
        .map_err(|(code, msg)| format!("ICRC-3 history query failed: {code:?}: {msg}"))?;
    let main_blocks = main.blocks;
    if main_blocks.len() > 1 {
        return Err("ICRC-3 returned more blocks than the one-block proof request".into());
    }
    let found: Vec<_> = main_blocks
        .into_iter()
        .filter(|block| block.id.0 == index.into())
        .collect();
    if found.len() == 1 {
        return Ok(found.into_iter().next().expect("length checked").block);
    } else if found.len() > 1 {
        return Err("ICRC-3 returned duplicate candidate blocks".into());
    }

    let mut covering = Vec::new();
    for archive in main.archived_blocks {
        let mut covers = false;
        for arg in &archive.args {
            let start: u64 = arg
                .start
                .0
                .clone()
                .try_into()
                .map_err(|_| "ICRC-3 archive start exceeds nat64".to_string())?;
            let length: u64 = arg
                .length
                .0
                .clone()
                .try_into()
                .map_err(|_| "ICRC-3 archive length exceeds nat64".to_string())?;
            if icrc3_archive_covers(index, start, length)? {
                covers = true;
            }
        }
        if covers {
            covering.push(archive);
        }
    }
    if covering.len() != 1 {
        return Err("ICRC-3 history has no unique archive range for candidate block".into());
    }
    let archive = covering.pop().expect("one archive checked");
    let (archived,): (Icrc3GetBlocksV2,) = ic_cdk::call(
        archive.callback.canister_id,
        &archive.callback.method,
        (vec![Icrc3GetBlocksArgsV2 {
            start: index.into(),
            length: 1u8.into(),
        }],),
    )
    .await
    .map_err(|(code, msg)| format!("ICRC-3 archive query failed: {code:?}: {msg}"))?;
    if !archived.archived_blocks.is_empty() {
        return Err("Nested ICRC-3 archive responses are unsupported".into());
    }
    if archived.blocks.len() != 1 || archived.blocks[0].id.0 != index.into() {
        return Err("Archive did not return exactly one candidate block".into());
    }
    Ok(archived
        .blocks
        .into_iter()
        .next()
        .expect("length checked")
        .block)
}

fn icrc3_archive_covers(index: u64, start: u64, length: u64) -> Result<bool, String> {
    let end = start
        .checked_add(length)
        .ok_or("ICRC-3 archive range overflow")?;
    Ok(length > 0 && index >= start && index < end)
}

#[derive(candid::CandidType, Deserialize, Clone, Debug)]
struct NativeIcpTimestampV2 {
    timestamp_nanos: u64,
}
#[derive(candid::CandidType, Deserialize, Clone, Debug)]
struct NativeIcpTokensV2 {
    e8s: u64,
}
#[derive(candid::CandidType, Deserialize, Clone, Debug)]
struct NativeIcpTransactionV2 {
    memo: u64,
    icrc1_memo: Option<Vec<u8>>,
    operation: Option<NativeIcpOperationV2>,
    created_at_time: NativeIcpTimestampV2,
}
#[derive(candid::CandidType, Deserialize, Clone, Debug)]
enum NativeIcpOperationV2 {
    Burn {
        from: Vec<u8>,
        spender: Option<Vec<u8>>,
        amount: NativeIcpTokensV2,
    },
    Mint {
        to: Vec<u8>,
        amount: NativeIcpTokensV2,
    },
    Transfer {
        from: Vec<u8>,
        to: Vec<u8>,
        spender: Option<Vec<u8>>,
        amount: NativeIcpTokensV2,
        fee: NativeIcpTokensV2,
    },
    Approve {
        from: Vec<u8>,
        spender: Vec<u8>,
        allowance_e8s: i128,
        allowance: NativeIcpTokensV2,
        fee: NativeIcpTokensV2,
        expires_at: Option<NativeIcpTimestampV2>,
        expected_allowance: Option<NativeIcpTokensV2>,
    },
    TransferFrom {
        from: Vec<u8>,
        to: Vec<u8>,
        spender: Vec<u8>,
        amount: NativeIcpTokensV2,
        fee: NativeIcpTokensV2,
    },
}
#[derive(candid::CandidType, Deserialize, Clone, Debug)]
struct NativeIcpBlockV2 {
    parent_hash: Option<Vec<u8>>,
    transaction: NativeIcpTransactionV2,
    timestamp: NativeIcpTimestampV2,
}
#[derive(candid::CandidType, Deserialize, Clone, Debug)]
struct NativeIcpGetBlocksArgsV2 {
    start: u64,
    length: u64,
}
#[derive(candid::CandidType, Deserialize, Clone, Debug)]
struct NativeIcpBlockRangeV2 {
    blocks: Vec<NativeIcpBlockV2>,
}
#[derive(candid::CandidType, Deserialize, Clone, Debug)]
enum NativeIcpArchiveErrorV2 {
    BadFirstBlockIndex {
        requested_index: u64,
        first_valid_index: u64,
    },
    Other {
        error_code: u64,
        error_message: String,
    },
}
type NativeIcpArchiveResultV2 = Result<NativeIcpBlockRangeV2, NativeIcpArchiveErrorV2>;
candid::define_function!(NativeIcpArchiveFnV2 : (NativeIcpGetBlocksArgsV2) -> (NativeIcpArchiveResultV2) query);
#[derive(candid::CandidType, Deserialize, Clone, Debug)]
struct NativeIcpArchivedRangeV2 {
    start: u64,
    length: u64,
    callback: NativeIcpArchiveFnV2,
}
#[derive(candid::CandidType, Deserialize, Clone, Debug)]
struct NativeIcpQueryBlocksResponseV2 {
    chain_length: u64,
    certificate: Option<Vec<u8>>,
    first_block_index: u64,
    blocks: Vec<NativeIcpBlockV2>,
    archived_blocks: Vec<NativeIcpArchivedRangeV2>,
}

async fn query_native_icp_block(ledger: Principal, index: u64) -> Result<NativeIcpBlockV2, String> {
    let args = NativeIcpGetBlocksArgsV2 {
        start: index,
        length: 1,
    };
    let (response,): (NativeIcpQueryBlocksResponseV2,) =
        ic_cdk::call(ledger, "query_blocks", (args.clone(),))
            .await
            .map_err(|(code, msg)| format!("ICP query_blocks failed: {code:?}: {msg}"))?;
    if index >= response.first_block_index {
        let offset: usize = index
            .checked_sub(response.first_block_index)
            .ok_or("block index underflow")?
            .try_into()
            .map_err(|_| "block offset exceeds usize")?;
        if offset < response.blocks.len() {
            if response.blocks.len() != 1 || offset != 0 {
                return Err(
                    "ICP ledger returned more blocks than the one-block proof request".into(),
                );
            }
            return Ok(response.blocks.into_iter().next().expect("length checked"));
        }
    }
    let mut ranges = Vec::new();
    for range in response.archived_blocks {
        if native_icp_archive_covers(index, range.start, range.length)? {
            ranges.push(range);
        }
    }
    if ranges.len() != 1 {
        return Err("ICP history has no unique archive range for candidate block".into());
    }
    let range = ranges.into_iter().next().expect("length checked");
    let (result,): (NativeIcpArchiveResultV2,) = ic_cdk::call(
        range.callback.0.principal,
        &range.callback.0.method,
        (args,),
    )
    .await
    .map_err(|(code, msg)| format!("ICP archive query failed: {code:?}: {msg}"))?;
    match result {
        Ok(blocks) if blocks.blocks.len() == 1 => {
            Ok(blocks.blocks.into_iter().next().expect("length checked"))
        }
        Ok(_) => Err("ICP archive returned an incomplete or oversized block range".into()),
        Err(
            NativeIcpArchiveErrorV2::BadFirstBlockIndex { .. }
            | NativeIcpArchiveErrorV2::Other { .. },
        ) => Err("ICP archive rejected candidate block query".into()),
    }
}

fn native_icp_archive_covers(index: u64, start: u64, length: u64) -> Result<bool, String> {
    let end = start
        .checked_add(length)
        .ok_or("ICP archive range overflow")?;
    Ok(length > 0 && index >= start && index < end)
}

fn native_account_identifier(owner: Principal) -> [u8; 32] {
    let mut hasher = Sha224::new();
    hasher.update(b"\x0Aaccount-id");
    hasher.update(owner.as_slice());
    hasher.update([0u8; 32]);
    let hash = hasher.finalize();
    let mut crc = !0u32;
    for byte in hash.iter() {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    let mut out = [0u8; 32];
    out[..4].copy_from_slice(&(!crc).to_be_bytes());
    out[4..].copy_from_slice(&hash);
    out
}

fn verify_native_icp_withdrawal(
    block: &NativeIcpBlockV2,
    r: &WithdrawalRequestRecord,
    request_id: u64,
    source: Principal,
) -> Result<(), String> {
    let Some(NativeIcpOperationV2::Transfer {
        from,
        to,
        spender,
        amount,
        fee,
    }) = block.transaction.operation.as_ref()
    else {
        return Err("Native ICP block is not the legacy Transfer variant".into());
    };
    if spender.is_some() {
        return Err("Native ICP transfer has an unexpected spender".into());
    }
    if from.as_slice() != native_account_identifier(source) {
        return Err("Native ICP source account mismatch".into());
    }
    if to.as_slice() != native_account_identifier(r.to) {
        return Err("Native ICP destination account mismatch".into());
    }
    if amount.e8s != r.send_amount {
        return Err("Native ICP amount mismatch".into());
    }
    if fee.e8s != r.fee {
        return Err("Native ICP fee mismatch".into());
    }
    if block.transaction.created_at_time.timestamp_nanos != r.created_at_time {
        return Err("Native ICP created_at_time mismatch".into());
    }
    let expected_memo = r
        .memo
        .as_ref()
        .map(|m| m.as_bytes().to_vec())
        .unwrap_or_else(|| request_id.to_be_bytes().to_vec());
    let matches_memo = match block.transaction.icrc1_memo.as_deref() {
        Some(memo) => memo == expected_memo,
        None => {
            expected_memo.len() == 8
                && block.transaction.memo
                    == u64::from_be_bytes(
                        expected_memo.as_slice().try_into().expect("length checked"),
                    )
        }
    };
    if !matches_memo {
        return Err("Native ICP memo mismatch".into());
    }
    Ok(())
}

fn verify_icrc3_withdrawal(
    block: &Icrc3ValueV2,
    r: &WithdrawalRequestRecord,
    request_id: u64,
    source: Principal,
) -> Result<(), String> {
    fn unique<'a>(
        map: &'a [(String, Icrc3ValueV2)],
        key: &str,
    ) -> Result<Option<&'a Icrc3ValueV2>, String> {
        let mut found = map.iter().filter(|(k, _)| k == key).map(|(_, v)| v);
        let value = found.next();
        if found.next().is_some() {
            return Err(format!("duplicate ICRC-3 field {key}"));
        }
        Ok(value)
    }
    fn map<'a>(
        value: &'a Icrc3ValueV2,
        name: &str,
    ) -> Result<&'a [(String, Icrc3ValueV2)], String> {
        match value {
            Icrc3ValueV2::Map(fields) => Ok(fields),
            _ => Err(format!("ICRC-3 {name} is not a map")),
        }
    }
    fn nat(value: Option<&Icrc3ValueV2>, name: &str) -> Result<u64, String> {
        let Icrc3ValueV2::Nat(n) = value.ok_or_else(|| format!("ICRC-3 block lacks {name}"))?
        else {
            return Err(format!("ICRC-3 {name} has an unsupported type"));
        };
        n.0.clone()
            .try_into()
            .map_err(|_| format!("ICRC-3 {name} exceeds nat64"))
    }
    fn account(value: Option<&Icrc3ValueV2>, expected: Principal) -> Result<(), String> {
        let Some(Icrc3ValueV2::Array(parts)) = value else {
            return Err("ICRC-3 account has unsupported encoding".into());
        };
        if parts.len() != 1 {
            return Err("ICRC-3 withdrawal account has unexpected subaccount".into());
        }
        let Some(Icrc3ValueV2::Blob(owner)) = parts.first() else {
            return Err("ICRC-3 account owner is not a blob".into());
        };
        if owner.as_slice() != expected.as_slice() {
            return Err("ICRC-3 account does not match stored withdrawal tuple".into());
        }
        Ok(())
    }
    let root = map(block, "block")?;
    if root
        .iter()
        .any(|(key, _)| !matches!(key.as_str(), "btype" | "tx" | "phash" | "fee"))
    {
        return Err("ICRC-3 block contains an unknown field".into());
    }
    let btype = unique(root, "btype")?.and_then(|v| {
        if let Icrc3ValueV2::Text(s) = v {
            Some(s.as_str())
        } else {
            None
        }
    });
    let tx = unique(root, "tx")?.ok_or("ICRC-3 block lacks tx")?;
    let tx = map(tx, "tx")?;
    if tx.iter().any(|(key, _)| {
        !matches!(
            key.as_str(),
            "op" | "from" | "to" | "amt" | "fee" | "ts" | "memo"
        )
    }) {
        return Err("ICRC-3 transaction contains an unknown field".into());
    }
    let op = unique(tx, "op")?.and_then(|v| {
        if let Icrc3ValueV2::Text(s) = v {
            Some(s.as_str())
        } else {
            None
        }
    });
    if btype != Some("1xfer") || op != Some("xfer") {
        return Err("ICRC-3 block is not a canonical transfer".into());
    }
    account(unique(tx, "from")?, source)?;
    account(unique(tx, "to")?, r.to)?;
    if unique(tx, "spender")?.is_some() {
        return Err("ICRC-3 transfer has an unexpected spender".into());
    }
    if nat(unique(tx, "amt")?, "amount")? != r.send_amount {
        return Err("ICRC-3 amount mismatch".into());
    }
    let tx_fee = unique(tx, "fee")?;
    let root_fee = unique(root, "fee")?;
    if tx_fee.is_some() && root_fee.is_some() {
        return Err("ICRC-3 fee is ambiguously encoded".into());
    }
    let fee = tx_fee.or(root_fee);
    if nat(fee, "fee")? != r.fee {
        return Err("ICRC-3 fee mismatch".into());
    }
    if nat(unique(tx, "ts")?, "timestamp")? != r.created_at_time {
        return Err("ICRC-3 created_at_time mismatch".into());
    }
    let expected_memo = r
        .memo
        .as_ref()
        .map(|m| m.as_bytes().to_vec())
        .unwrap_or_else(|| request_id.to_be_bytes().to_vec());
    let Some(Icrc3ValueV2::Blob(memo)) = unique(tx, "memo")? else {
        return Err("ICRC-3 memo missing or unsupported".into());
    };
    if memo != &expected_memo {
        return Err("ICRC-3 memo mismatch".into());
    }
    Ok(())
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

/// Make actual ledger transfer call. Distinguishes Duplicate (success),
/// ledger rejections (caller-recoverable), and transport errors (ambiguous).
async fn call_ledger_transfer(
    ledger_principal: Principal,
    args: TransferArg,
) -> Result<u64, LedgerError> {
    let outer: Result<(Result<candid::Nat, TransferError>,), _> =
        ic_cdk::call(ledger_principal, "icrc1_transfer", (args,)).await;

    match outer {
        Err((code, msg)) => Err(LedgerError::Transport(format!("{:?}: {}", code, msg))),
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            match parse_duplicate_block(duplicate_of) {
                Ok(block) => Err(LedgerError::Duplicate {
                    duplicate_of: block,
                }),
                Err(error) => Err(error),
            }
        }
        Ok((Err(e),)) => Err(LedgerError::Ledger(e)),
        Ok((Ok(block_index),)) => parse_success_block(block_index),
    }
}

fn parse_duplicate_block(block: candid::Nat) -> Result<u64, LedgerError> {
    block.0.try_into().map_err(|_| {
        LedgerError::Transport(
            "ledger reported Duplicate but its block index exceeds nat64; transfer remains held for reconciliation".into(),
        )
    })
}

fn parse_success_block(block: candid::Nat) -> Result<u64, LedgerError> {
    block.0.try_into().map_err(|_| {
        LedgerError::Ledger(TransferError::GenericError {
            error_code: candid::Nat::from(501u32),
            message: "Block index too large; transfer may have committed and remains held for reconciliation".to_string(),
        })
    })
}

// Export candid interface
candid::export_service!();

#[query(name = "__get_candid_interface_tmp_hack")]
fn export_candid() -> String {
    __export_service()
}
