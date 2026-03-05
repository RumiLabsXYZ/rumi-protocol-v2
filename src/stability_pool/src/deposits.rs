use std::collections::BTreeMap;
use candid::Principal;
use ic_canister_log::log;
use ic_cdk::call;
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
use icrc_ledger_types::icrc1::account::Account;
use crate::logs::INFO;
use crate::types::*;
use crate::state::{read_state, mutate_state};

/// Deposit a stablecoin into the pool. User must have pre-approved the pool canister.
pub async fn deposit(token_ledger: Principal, amount: u64) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();

    // Validate token is accepted
    let config = read_state(|s| s.get_stablecoin_config(&token_ledger).cloned())
        .ok_or(StabilityPoolError::TokenNotAccepted { ledger: token_ledger })?;

    if !config.is_active {
        return Err(StabilityPoolError::TokenNotActive { ledger: token_ledger });
    }

    // Validate minimum deposit (normalize to e8s for comparison)
    let amount_e8s = normalize_to_e8s(amount, config.decimals);
    let min_deposit = read_state(|s| s.configuration.min_deposit_e8s);
    if amount_e8s < min_deposit {
        return Err(StabilityPoolError::AmountTooLow { minimum_e8s: min_deposit });
    }

    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    log!(INFO, "Deposit: {} {} ({}) from {}", amount, config.symbol, token_ledger, caller);

    // ICRC-2 transfer_from: pull tokens from user to pool canister
    let transfer_args = TransferFromArgs {
        from: Account { owner: caller, subaccount: None },
        to: Account { owner: ic_cdk::api::id(), subaccount: None },
        amount: amount.into(),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()),
        spender_subaccount: None,
    };

    let result: Result<(Result<candid::Nat, TransferFromError>,), _> = call(
        token_ledger, "icrc2_transfer_from", (transfer_args,)
    ).await;

    match result {
        Ok((Ok(block_index),)) => {
            log!(INFO, "Transfer succeeded, block: {}", block_index);
            mutate_state(|s| s.add_deposit(caller, token_ledger, amount));
            log!(INFO, "Deposit recorded for {}", caller);
            Ok(())
        },
        Ok((Err(transfer_error),)) => {
            log!(INFO, "Transfer failed: {:?}", transfer_error);
            Err(StabilityPoolError::LedgerTransferFailed {
                reason: format!("{:?}", transfer_error),
            })
        },
        Err(call_error) => {
            log!(INFO, "Inter-canister call failed: {:?}", call_error);
            Err(StabilityPoolError::InterCanisterCallFailed {
                target: format!("{}", token_ledger),
                method: "icrc2_transfer_from".to_string(),
            })
        }
    }
}

/// Withdraw a stablecoin from the pool (only unconsumed balances).
///
/// Uses deduct-before-transfer pattern to prevent TOCTOU double-spend:
/// 1. Deduct balance from state (prevents concurrent withdrawals from passing balance check)
/// 2. Transfer tokens to user
/// 3. If transfer fails, rollback the deduction
pub async fn withdraw(token_ledger: Principal, amount: u64) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();

    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    // Deduct balance BEFORE transfer to prevent double-spend during async call.
    // If the transfer fails, we rollback below.
    mutate_state(|s| s.process_withdrawal(caller, token_ledger, amount))?;

    log!(INFO, "Withdraw: {} from {} by {}", amount, token_ledger, caller);

    // ICRC-1 transfer: send tokens from pool to user
    let transfer_args = TransferArg {
        to: Account { owner: caller, subaccount: None },
        amount: amount.into(),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()),
        from_subaccount: None,
    };

    let result: Result<(Result<candid::Nat, TransferError>,), _> = call(
        token_ledger, "icrc1_transfer", (transfer_args,)
    ).await;

    match result {
        Ok((Ok(block_index),)) => {
            log!(INFO, "Withdrawal transfer succeeded, block: {}", block_index);
            Ok(())
        },
        Ok((Err(transfer_error),)) => {
            log!(INFO, "Withdrawal transfer failed, rolling back deduction: {:?}", transfer_error);
            // Rollback: re-credit the user's balance
            mutate_state(|s| s.add_deposit(caller, token_ledger, amount));
            Err(StabilityPoolError::LedgerTransferFailed {
                reason: format!("{:?}", transfer_error),
            })
        },
        Err(call_error) => {
            log!(INFO, "Inter-canister call failed, rolling back deduction: {:?}", call_error);
            // Rollback: re-credit the user's balance
            mutate_state(|s| s.add_deposit(caller, token_ledger, amount));
            Err(StabilityPoolError::InterCanisterCallFailed {
                target: format!("{}", token_ledger),
                method: "icrc1_transfer".to_string(),
            })
        }
    }
}

/// Claim collateral gains for a single collateral type.
///
/// Uses deduct-before-transfer pattern to prevent TOCTOU double-claim:
/// 1. Deduct gains from state (prevents concurrent claims from reading same gains)
/// 2. Transfer collateral to user
/// 3. If transfer fails, rollback the deduction
pub async fn claim_collateral(collateral_ledger: Principal) -> Result<u64, StabilityPoolError> {
    let caller = ic_cdk::api::caller();

    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    // Read and deduct gains atomically BEFORE transfer.
    // mark_gains_claimed uses saturating_sub and cleans up zero entries.
    let gains = mutate_state(|s| {
        let amount = s.deposits.get(&caller)
            .and_then(|pos| pos.collateral_gains.get(&collateral_ledger).copied())
            .unwrap_or(0);
        if amount > 0 {
            s.mark_gains_claimed(&caller, &collateral_ledger, amount);
        }
        amount
    });

    if gains == 0 {
        return Ok(0);
    }

    log!(INFO, "Claim: {} of collateral {} by {}", gains, collateral_ledger, caller);

    let transfer_args = TransferArg {
        to: Account { owner: caller, subaccount: None },
        amount: gains.into(),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()),
        from_subaccount: None,
    };

    let result: Result<(Result<candid::Nat, TransferError>,), _> = call(
        collateral_ledger, "icrc1_transfer", (transfer_args,)
    ).await;

    match result {
        Ok((Ok(block_index),)) => {
            log!(INFO, "Collateral claim transfer succeeded, block: {}", block_index);
            Ok(gains)
        },
        Ok((Err(transfer_error),)) => {
            log!(INFO, "Collateral claim failed, rolling back: {:?}", transfer_error);
            // Rollback: restore the gains
            mutate_state(|s| {
                if let Some(pos) = s.deposits.get_mut(&caller) {
                    *pos.collateral_gains.entry(collateral_ledger).or_insert(0) += gains;
                    // Undo the total_claimed_gains increment from mark_gains_claimed
                    if let Some(claimed) = pos.total_claimed_gains.get_mut(&collateral_ledger) {
                        *claimed = claimed.saturating_sub(gains);
                    }
                }
            });
            Err(StabilityPoolError::LedgerTransferFailed {
                reason: format!("{:?}", transfer_error),
            })
        },
        Err(call_error) => {
            log!(INFO, "Inter-canister call failed, rolling back: {:?}", call_error);
            // Rollback: restore the gains
            mutate_state(|s| {
                if let Some(pos) = s.deposits.get_mut(&caller) {
                    *pos.collateral_gains.entry(collateral_ledger).or_insert(0) += gains;
                    // Undo the total_claimed_gains increment from mark_gains_claimed
                    if let Some(claimed) = pos.total_claimed_gains.get_mut(&collateral_ledger) {
                        *claimed = claimed.saturating_sub(gains);
                    }
                }
            });
            Err(StabilityPoolError::InterCanisterCallFailed {
                target: format!("{}", collateral_ledger),
                method: "icrc1_transfer".to_string(),
            })
        }
    }
}

/// Claim all nonzero collateral gains across all collateral types.
pub async fn claim_all_collateral() -> Result<BTreeMap<Principal, u64>, StabilityPoolError> {
    let caller = ic_cdk::api::caller();

    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    let all_gains = read_state(|s| s.get_collateral_gains(&caller));
    let nonzero_gains: BTreeMap<Principal, u64> = all_gains.into_iter()
        .filter(|(_, v)| *v > 0)
        .collect();

    if nonzero_gains.is_empty() {
        return Ok(BTreeMap::new());
    }

    let mut claimed = BTreeMap::new();
    for (collateral_ledger, amount) in &nonzero_gains {
        match claim_collateral(*collateral_ledger).await {
            Ok(claimed_amount) => {
                claimed.insert(*collateral_ledger, claimed_amount);
            },
            Err(e) => {
                log!(INFO, "Failed to claim {} from {}: {:?}", amount, collateral_ledger, e);
                // Continue claiming others — partial success is fine
            }
        }
    }

    Ok(claimed)
}
