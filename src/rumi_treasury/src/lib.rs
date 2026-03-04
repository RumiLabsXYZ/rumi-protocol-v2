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
    AssetType, DepositArgs, DepositRecord, TreasuryInitArgs,
    TreasuryStatus, WithdrawArgs, WithdrawResult
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

    log!(LOG, "Deposit {} recorded successfully", deposit_id);
    Ok(deposit_id)
}

/// Withdraw funds from treasury (controllers only)
#[update]
#[candid_method(update)]
async fn withdraw(args: WithdrawArgs) -> Result<WithdrawResult, String> {
    ensure_controller()?;

    log!(LOG, "Processing withdrawal: {} {:?} to {}",
         args.amount, args.asset_type, args.to);

    // Deduct from bookkeeping before attempting transfer
    with_state_mut(|s| s.withdraw(args.asset_type.clone(), args.amount))?;

    // Get the appropriate ledger principal
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

    // Make the transfer
    let transfer_args = TransferArg {
        from_subaccount: None,
        to: Account {
            owner: args.to,
            subaccount: None,
        },
        amount: args.amount.into(),
        fee: None,
        memo: args.memo.map(|m| m.into_bytes().into()),
        created_at_time: None,
    };

    // Make the actual inter-canister transfer call to the ledger
    let block_index = match call_ledger_transfer(ledger_principal, transfer_args).await {
        Ok(block_index) => block_index,
        Err(e) => {
            // Restore the balance if transfer failed
            with_state_mut(|s| s.restore_balance(&args.asset_type, args.amount));
            return Err(format!("Transfer failed: {:?}", e));
        }
    };

    log!(LOG, "Withdrawal completed, block index: {}", block_index);

    Ok(WithdrawResult {
        block_index,
        amount_transferred: args.amount,
        fee: 0,
    })
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

/// Pause/unpause treasury (controllers only)
#[update]
#[candid_method(update)]
fn set_paused(paused: bool) -> Result<(), String> {
    ensure_controller()?;
    log!(LOG, "Setting treasury paused state to: {}", paused);
    with_state_mut(|s| s.set_paused(paused))
}

/// Make actual ledger transfer call
async fn call_ledger_transfer(
    ledger_principal: Principal,
    args: TransferArg,
) -> Result<u64, TransferError> {
    let (result,): (Result<candid::Nat, TransferError>,) = ic_cdk::call(
        ledger_principal,
        "icrc1_transfer",
        (args,),
    ).await
    .map_err(|e| TransferError::GenericError {
        error_code: candid::Nat::from(500u32),
        message: format!("Call failed: {:?}", e),
    })?;

    match result {
        Ok(block_index) => {
            let block_index: u64 = block_index.0.try_into()
                .map_err(|_| TransferError::GenericError {
                    error_code: candid::Nat::from(501u32),
                    message: "Block index too large".to_string(),
                })?;
            Ok(block_index)
        }
        Err(e) => Err(e),
    }
}

// Export candid interface
candid::export_service!();

#[query(name = "__get_candid_interface_tmp_hack")]
fn export_candid() -> String {
    __export_service()
}
