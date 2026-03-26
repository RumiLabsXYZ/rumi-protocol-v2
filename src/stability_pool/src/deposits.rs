use std::collections::BTreeMap;
use candid::Principal;
use ic_canister_log::log;
use ic_cdk::call;
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
use icrc_ledger_types::icrc2::approve::{ApproveArgs, ApproveError};
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
            mutate_state(|s| {
                s.add_deposit(caller, token_ledger, amount);
                s.push_event(caller, PoolEventType::Deposit { token_ledger, amount });
            });
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

    // Look up the transfer fee so we can deduct it from what the user receives.
    // The ledger charges the fee on top of the transfer amount, so without this
    // the pool's ledger balance drifts below its recorded state on every withdrawal.
    let ledger_fee = read_state(|s| {
        s.stablecoin_registry.get(&token_ledger)
            .and_then(|c| c.transfer_fee)
            .unwrap_or(0)
    });

    if amount <= ledger_fee {
        return Err(StabilityPoolError::AmountTooLow {
            minimum_e8s: ledger_fee + 1,
        });
    }

    // Deduct full amount from state BEFORE transfer to prevent double-spend.
    // If the transfer fails, we rollback below.
    mutate_state(|s| s.process_withdrawal(caller, token_ledger, amount))?;

    // User receives amount minus fee; pool pays amount total (transfer + fee)
    let transfer_amount = amount - ledger_fee;
    log!(INFO, "Withdraw: {} (transfer {} - fee {}) from {} by {}", amount, transfer_amount, ledger_fee, token_ledger, caller);

    let transfer_args = TransferArg {
        to: Account { owner: caller, subaccount: None },
        amount: transfer_amount.into(),
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
            mutate_state(|s| s.push_event(caller, PoolEventType::Withdraw { token_ledger, amount }));
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

    // Query the collateral ledger's transfer fee so we can deduct it from
    // what the user receives, keeping the pool's ledger balance in sync.
    let ledger_fee: u64 = match call::<(), (candid::Nat,)>(collateral_ledger, "icrc1_fee", ()).await {
        Ok((fee_nat,)) => {
            let fee_u128: u128 = fee_nat.0.try_into().unwrap_or(0);
            fee_u128 as u64
        },
        Err(_) => 0, // If we can't query the fee, transfer the full amount (old behavior)
    };

    if gains <= ledger_fee {
        // Gains too small to cover the fee — rollback and return 0
        mutate_state(|s| {
            if let Some(pos) = s.deposits.get_mut(&caller) {
                *pos.collateral_gains.entry(collateral_ledger).or_insert(0) += gains;
                if let Some(claimed) = pos.total_claimed_gains.get_mut(&collateral_ledger) {
                    *claimed = claimed.saturating_sub(gains);
                }
            }
        });
        return Ok(0);
    }

    let transfer_amount = gains - ledger_fee;
    log!(INFO, "Claim: {} of collateral {} (transfer {} - fee {}) by {}", gains, collateral_ledger, transfer_amount, ledger_fee, caller);

    let transfer_args = TransferArg {
        to: Account { owner: caller, subaccount: None },
        amount: transfer_amount.into(),
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
            mutate_state(|s| s.push_event(caller, PoolEventType::ClaimCollateral {
                collateral_ledger,
                amount: transfer_amount,
            }));
            Ok(transfer_amount)
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

/// Convenience deposit: user sends icUSD (or ckUSDT/ckUSDC) and the pool
/// deposits it into the 3pool on their behalf, crediting the resulting 3USD.
pub async fn deposit_as_3usd(
    token_ledger: Principal,
    amount: u64,
) -> Result<u64, StabilityPoolError> {
    let caller = ic_cdk::api::caller();

    let config = read_state(|s| s.get_stablecoin_config(&token_ledger).cloned())
        .ok_or(StabilityPoolError::TokenNotAccepted { ledger: token_ledger })?;
    if !config.is_active {
        return Err(StabilityPoolError::TokenNotActive { ledger: token_ledger });
    }
    if config.is_lp_token.unwrap_or(false) {
        return Err(StabilityPoolError::TokenNotAccepted { ledger: token_ledger });
    }

    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    let amount_e8s = normalize_to_e8s(amount, config.decimals);
    let min_deposit = read_state(|s| s.configuration.min_deposit_e8s);
    if amount_e8s < min_deposit {
        return Err(StabilityPoolError::AmountTooLow { minimum_e8s: min_deposit });
    }

    // Find the 3USD config (LP token with underlying_pool set)
    let (three_usd_ledger, three_pool_canister) = read_state(|s| {
        s.stablecoin_registry.iter()
            .find(|(_, c)| c.is_lp_token.unwrap_or(false) && c.underlying_pool.is_some() && c.is_active)
            .map(|(ledger, c)| (*ledger, c.underlying_pool.unwrap()))
            .ok_or(StabilityPoolError::TokenNotAccepted { ledger: token_ledger })
    })?;

    log!(INFO, "deposit_as_3usd: {} depositing {} of {} via 3pool", caller, amount, token_ledger);

    // Step 1: Pull tokens from user
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
        Ok((Ok(_),)) => {},
        Ok((Err(e),)) => return Err(StabilityPoolError::LedgerTransferFailed {
            reason: format!("{:?}", e),
        }),
        Err(_e) => return Err(StabilityPoolError::InterCanisterCallFailed {
            target: format!("{}", token_ledger),
            method: "icrc2_transfer_from".to_string(),
        }),
    }

    // Step 2: Approve 3pool to spend the token
    let approve_args = ApproveArgs {
        from_subaccount: None,
        spender: Account { owner: three_pool_canister, subaccount: None },
        amount: candid::Nat::from(amount as u128 * 2), // 2x buffer for fees
        expected_allowance: None,
        expires_at: Some(ic_cdk::api::time() + 300_000_000_000), // 5 min
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()),
    };

    let approve_result: Result<(Result<candid::Nat, ApproveError>,), _> = call(
        token_ledger, "icrc2_approve", (approve_args,)
    ).await;

    if let Err(_) | Ok((Err(_),)) = approve_result {
        refund_user(caller, token_ledger, amount).await;
        return Err(StabilityPoolError::InterCanisterCallFailed {
            target: format!("{}", token_ledger),
            method: "icrc2_approve".to_string(),
        });
    }

    // Step 3: Query 3pool to find which coin index this token is
    let pool_status_result: Result<(ThreePoolStatus,), _> = call(
        three_pool_canister, "get_pool_status", ()
    ).await;

    let pool_status = match pool_status_result {
        Ok((status,)) => status,
        Err(_) => {
            refund_user(caller, token_ledger, amount).await;
            return Err(StabilityPoolError::InterCanisterCallFailed {
                target: "3pool".to_string(),
                method: "get_pool_status".to_string(),
            });
        }
    };

    let coin_index = pool_status.tokens.iter().position(|t| t.ledger_id == token_ledger);
    let coin_index = match coin_index {
        Some(idx) => idx,
        None => {
            refund_user(caller, token_ledger, amount).await;
            return Err(StabilityPoolError::TokenNotAccepted { ledger: token_ledger });
        }
    };

    let mut amounts = vec![0u128; 3];
    amounts[coin_index] = amount as u128;

    // Step 4: Call add_liquidity on the 3pool
    let lp_result: Result<(Result<u128, ThreePoolErrorRemote>,), _> = call(
        three_pool_canister, "add_liquidity", (amounts, 0u128)
    ).await;

    let lp_minted = match lp_result {
        Ok((Ok(lp),)) => lp,
        _ => {
            refund_user(caller, token_ledger, amount).await;
            return Err(StabilityPoolError::InterCanisterCallFailed {
                target: "3pool".to_string(),
                method: "add_liquidity".to_string(),
            });
        }
    };

    // Step 5: Credit user's 3USD balance
    let lp_amount_u64 = lp_minted as u64;
    mutate_state(|s| {
        s.add_deposit(caller, three_usd_ledger, lp_amount_u64);
        s.push_event(caller, PoolEventType::DepositAs3USD {
            token_ledger,
            amount_in: amount,
            lp_minted: lp_amount_u64,
        });
    });

    log!(INFO, "deposit_as_3usd: {} deposited {} of {} → {} 3USD LP", caller, amount, token_ledger, lp_amount_u64);

    Ok(lp_amount_u64)
}

/// Best-effort refund of tokens to user after a failed deposit_as_3usd.
async fn refund_user(user: Principal, token_ledger: Principal, amount: u64) {
    let transfer_args = TransferArg {
        to: Account { owner: user, subaccount: None },
        amount: amount.into(),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()),
        from_subaccount: None,
    };
    let _ = call::<_, (Result<candid::Nat, TransferError>,)>(
        token_ledger, "icrc1_transfer", (transfer_args,)
    ).await;
}
