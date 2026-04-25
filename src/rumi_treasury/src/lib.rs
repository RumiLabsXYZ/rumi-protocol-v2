mod types;
mod state;

#[cfg(test)]
mod tests;

use candid::{candid_method, Principal};
use ic_cdk::{init, post_upgrade, pre_upgrade, query, update};
use ic_cdk::api::caller;
use ic_canister_log::{log, declare_log_buffer};
use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
use icrc_ledger_types::icrc1::account::Account;
use state::{init_state, restore_state, with_state, with_state_mut};
use types::{
    AssetType, DepositArgs, DepositRecord, TreasuryAction, TreasuryEvent,
    TreasuryInitArgs, TreasuryStatus, WithdrawArgs, WithdrawResult
};

// Declare log buffer for debugging
declare_log_buffer!(name = LOG, capacity = 1000);

/// Initialize the treasury canister
#[init]
#[candid_method(init)]
fn init(args: TreasuryInitArgs) {
    log!(LOG, "Initializing treasury with controller: {}", args.controller);
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
    log!(LOG, "Treasury upgrade completed — state restored from stable memory");
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

    log!(LOG, "Processing deposit: {:?} {} {:?}",
         args.deposit_type, args.amount, args.asset_type);

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

    with_state_mut(|s| s.push_event(deposit_caller, TreasuryAction::Deposit {
        deposit_type: dep_type,
        asset_type: asset,
        amount,
    }));

    log!(LOG, "Deposit {} recorded successfully", deposit_id);
    Ok(deposit_id)
}

/// Withdraw funds from treasury (controllers only).
///
/// Audit Wave-3 (ICRC-002): the previous implementation passed
/// `created_at_time: None` and restored the local balance on ANY error,
/// turning a lost-reply transient into a silent double-spend on retry.
/// This version sets a deterministic `created_at_time` derived from the
/// caller-supplied (or auto-derived) `request_id`, treats `Duplicate` as
/// success, restores the balance ONLY for clear ledger errors, and on a
/// transport-layer error keeps the balance deducted while logging a
/// reconciliation hint for the controller.
#[update]
#[candid_method(update)]
async fn withdraw(args: WithdrawArgs) -> Result<WithdrawResult, String> {
    ensure_controller()?;
    let caller_principal = caller();

    log!(LOG, "Processing withdrawal: {} {:?} to {}",
         args.amount, args.asset_type, args.to);

    with_state_mut(|s| s.withdraw(args.asset_type.clone(), args.amount))?;

    let ledger_principal = with_state(|s| {
        let config = s.get_config();
        match args.asset_type {
            AssetType::ICUSD => Some(config.icusd_ledger),
            AssetType::ICP => Some(config.icp_ledger),
            AssetType::CKBTC => config.ckbtc_ledger,
            AssetType::CKUSDT => config.ckusdt_ledger,
            AssetType::CKUSDC => config.ckusdc_ledger,
        }
    }).ok_or("Ledger not configured for this asset type")?;

    let request_id = args.request_id.unwrap_or_else(|| {
        derive_request_id(&caller_principal, &args.asset_type, args.amount, &args.to)
    });
    let created_at_time = ic_cdk::api::time();

    let transfer_args = TransferArg {
        from_subaccount: None,
        to: Account {
            owner: args.to,
            subaccount: None,
        },
        amount: args.amount.into(),
        fee: None,
        memo: args.memo.clone()
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
            with_state_mut(|s| s.restore_balance(&args.asset_type, args.amount));
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

    with_state_mut(|s| s.push_event(caller_principal, TreasuryAction::Withdraw {
        asset_type: args.asset_type.clone(),
        amount: args.amount,
        to: args.to,
    }));

    log!(LOG, "Withdrawal completed, block index: {}", block_index);

    Ok(WithdrawResult {
        block_index,
        amount_transferred: args.amount,
        fee: 0,
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
        let balances = s.balances.iter()
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
    let outer: Result<(Result<candid::Nat, TransferError>,), _> = ic_cdk::call(
        ledger_principal,
        "icrc1_transfer",
        (args,),
    ).await;

    match outer {
        Err((code, msg)) => Err(LedgerError::Transport(format!("{:?}: {}", code, msg))),
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            let block: u64 = duplicate_of.0.try_into().unwrap_or(0);
            Err(LedgerError::Duplicate { duplicate_of: block })
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
