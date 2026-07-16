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
use std::collections::HashMap;
use types::{
    AssetType, DepositArgs, DepositRecord, TreasuryAction, TreasuryEvent, TreasuryInitArgs,
    TreasuryStatus, WithdrawArgs, WithdrawResult,
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

    let deposit_id = with_state_mut(|s| s.add_deposit(record));

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

    log!(
        LOG,
        "Processing withdrawal: {} {:?} to {}",
        args.amount,
        args.asset_type,
        args.to
    );

    // Resolve the ledger and fee BEFORE debiting, so an unconfigured ledger
    // or a dust amount can't leave the bookkeeping debited with no transfer.
    let ledger_principal = with_state(|s| {
        let config = s.get_config();
        match args.asset_type {
            AssetType::ICUSD => Some(config.icusd_ledger),
            AssetType::ICP => Some(config.icp_ledger),
            AssetType::CKBTC => config.ckbtc_ledger,
            AssetType::CKUSDT => config.ckusdt_ledger,
            AssetType::CKUSDC => config.ckusdc_ledger,
        }
    })
    .ok_or("Ledger not configured for this asset type")?;

    let fee = ledger_fee(ledger_principal).await;
    let send_amount = withdrawal_send_amount(args.amount, fee)?;

    with_state_mut(|s| s.withdraw(args.asset_type.clone(), args.amount))?;

    let request_id = args.request_id.unwrap_or_else(|| {
        derive_request_id(&caller_principal, &args.asset_type, args.amount, &args.to)
    });
    let created_at_time =
        with_state_mut(|s| s.created_at_time_for_request(request_id, ic_cdk::api::time()));

    let transfer_args = TransferArg {
        from_subaccount: None,
        to: Account {
            owner: args.to,
            subaccount: None,
        },
        amount: send_amount.into(),
        fee: None,
        memo: args
            .memo
            .clone()
            .map(|m| m.into_bytes().into())
            .or_else(|| Some(request_id.to_be_bytes().to_vec().into())),
        created_at_time: Some(created_at_time),
    };

    let block_index = match call_ledger_transfer(ledger_principal, transfer_args).await {
        Ok(block_index) => block_index,
        Err(LedgerError::Duplicate { duplicate_of }) => {
            log!(LOG,
                "Withdrawal returned Duplicate (block {}); treating as success — prior attempt landed",
                duplicate_of
            );
            duplicate_of
        }
        Err(LedgerError::Ledger(e)) => {
            with_state_mut(|s| {
                // A TooOld/CreatedInFuture rejection means the persisted
                // timestamp can never be accepted; drop it so the next
                // attempt gets a fresh one.
                if matches!(
                    e,
                    TransferError::TooOld | TransferError::CreatedInFuture { .. }
                ) {
                    s.clear_request_created_at(request_id);
                }
                s.restore_balance(&args.asset_type, args.amount)
            });
            return Err(format!("Transfer failed: {:?}", e));
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

    log!(LOG, "Withdrawal completed, block index: {}", block_index);

    Ok(WithdrawResult {
        block_index,
        amount_transferred: send_amount,
        fee,
    })
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
enum LedgerError {
    Duplicate { duplicate_of: u64 },
    Ledger(TransferError),
    Transport(String),
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
            let block: u64 = duplicate_of.0.try_into().unwrap_or(0);
            Err(LedgerError::Duplicate {
                duplicate_of: block,
            })
        }
        Ok((Err(e),)) => Err(LedgerError::Ledger(e)),
        Ok((Ok(block_index),)) => {
            let block_index: u64 = block_index.0.try_into().map_err(|_| {
                LedgerError::Ledger(TransferError::GenericError {
                    error_code: candid::Nat::from(501u32),
                    message: "Block index too large".to_string(),
                })
            })?;
            Ok(block_index)
        }
    }
}

// Export candid interface
candid::export_service!();

#[query(name = "__get_candid_interface_tmp_hack")]
fn export_candid() -> String {
    __export_service()
}
