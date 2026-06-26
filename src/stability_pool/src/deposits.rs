use crate::logs::INFO;
use crate::state::{mutate_state, read_state};
use crate::types::*;
use candid::Principal;
use ic_canister_log::log;
use ic_cdk::call;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
use icrc_ledger_types::icrc2::approve::{ApproveArgs, ApproveError};
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

/// Conservative fallback for a stablecoin ledger's transfer fee (native units),
/// used when the live `icrc1_fee` query fails (IC-S-001). Erring high keeps the
/// pool solvent (we refund slightly less) rather than risking an over-send.
/// Mirrors rumi_3pool::transfers::DEFAULT_LEDGER_FEE.
const DEFAULT_REFUND_LEDGER_FEE: u64 = 10_000;

thread_local! {
    /// Per-ledger transfer-fee cache for refund math, populated lazily from
    /// `icrc1_fee`. Heap-only (not persisted), so it is simply re-warmed after
    /// an upgrade. Mirrors rumi_3pool::transfers::LEDGER_FEES.
    static REFUND_LEDGER_FEES: RefCell<HashMap<Principal, u64>> = RefCell::new(HashMap::new());
}

fn record_deposit_credit_after_async(
    caller: Principal,
    token_ledger: Principal,
    amount: u64,
) -> Result<(), StabilityPoolError> {
    crate::ensure_pool_balance_mutation_allowed()?;
    mutate_state(|s| {
        s.add_deposit(caller, token_ledger, amount);
        s.push_event(
            caller,
            PoolEventType::Deposit {
                token_ledger,
                amount,
            },
        );
    });
    Ok(())
}

fn record_deposit_as_3usd_credit_after_async(
    caller: Principal,
    token_ledger: Principal,
    amount: u64,
    three_usd_ledger: Principal,
    lp_amount: u64,
) -> Result<(), StabilityPoolError> {
    crate::ensure_pool_balance_mutation_allowed()?;
    mutate_state(|s| {
        s.add_deposit(caller, three_usd_ledger, lp_amount);
        s.push_event(
            caller,
            PoolEventType::DepositAs3USD {
                token_ledger,
                amount_in: amount,
                lp_minted: lp_amount,
            },
        );
    });
    Ok(())
}

/// Fetch a stablecoin ledger's transfer fee, caching successful lookups per
/// ledger. On query failure, falls back to the larger of the registry's
/// configured fee and the standard ICRC-1 fee (the solvency-safe direction),
/// without caching so the next refund re-queries.
async fn refund_ledger_fee(ledger: Principal) -> u64 {
    if let Some(fee) = REFUND_LEDGER_FEES.with(|c| c.borrow().get(&ledger).copied()) {
        return fee;
    }
    match call::<(), (candid::Nat,)>(ledger, "icrc1_fee", ()).await {
        Ok((fee_nat,)) => {
            let fee: u64 = fee_nat.0.try_into().unwrap_or(DEFAULT_REFUND_LEDGER_FEE);
            REFUND_LEDGER_FEES.with(|c| c.borrow_mut().insert(ledger, fee));
            fee
        }
        Err(e) => {
            let registry_fee = read_state(|s| {
                s.stablecoin_registry
                    .get(&ledger)
                    .and_then(|c| c.transfer_fee)
                    .unwrap_or(0)
            });
            let fallback = registry_fee.max(DEFAULT_REFUND_LEDGER_FEE);
            log!(
                INFO,
                "icrc1_fee query failed for {}: {:?}; using conservative fallback {}",
                ledger,
                e,
                fallback
            );
            fallback
        }
    }
}

/// Deposit a stablecoin into the pool. User must have pre-approved the pool canister.
pub async fn deposit(token_ledger: Principal, amount: u64) -> Result<(), StabilityPoolError> {
    // SP-102: refuse balance-mutating ops while a liquidation is apportioning.
    if crate::pool_balance_mutation_blocked() {
        return Err(StabilityPoolError::SystemBusy);
    }
    let caller = ic_cdk::api::caller();

    // Validate token is accepted
    let config = read_state(|s| s.get_stablecoin_config(&token_ledger).cloned()).ok_or(
        StabilityPoolError::TokenNotAccepted {
            ledger: token_ledger,
        },
    )?;

    if !config.is_active {
        return Err(StabilityPoolError::TokenNotActive {
            ledger: token_ledger,
        });
    }

    // Validate minimum deposit (normalize to e8s for comparison)
    let amount_e8s = normalize_to_e8s(amount, config.decimals);
    let min_deposit = read_state(|s| s.configuration.min_deposit_e8s);
    if amount_e8s < min_deposit {
        return Err(StabilityPoolError::AmountTooLow {
            minimum_e8s: min_deposit,
        });
    }

    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    log!(
        INFO,
        "Deposit: {} {} ({}) from {}",
        amount,
        config.symbol,
        token_ledger,
        caller
    );

    // ICRC-2 transfer_from: pull tokens from user to pool canister
    let transfer_args = TransferFromArgs {
        from: Account {
            owner: caller,
            subaccount: None,
        },
        to: Account {
            owner: ic_cdk::api::id(),
            subaccount: None,
        },
        amount: amount.into(),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()),
        spender_subaccount: None,
    };

    let result: Result<(Result<candid::Nat, TransferFromError>,), _> =
        call(token_ledger, "icrc2_transfer_from", (transfer_args,)).await;

    match result {
        Ok((Ok(block_index),)) => {
            log!(INFO, "Transfer succeeded, block: {}", block_index);
            if let Err(error) = record_deposit_credit_after_async(caller, token_ledger, amount) {
                refund_user(
                    caller,
                    token_ledger,
                    amount,
                    "deposit: pool balance mutation blocked after transfer",
                )
                .await;
                return Err(error);
            }
            log!(INFO, "Deposit recorded for {}", caller);
            Ok(())
        }
        // Audit Wave-3 (ICRC-003): Duplicate from the ledger means the
        // previous transfer landed; the tokens are already in the pool.
        // Credit the deposit and treat as success.
        Ok((Err(TransferFromError::Duplicate { duplicate_of }),)) => {
            log!(
                INFO,
                "Deposit transfer Duplicate (block {}); previous attempt landed, crediting deposit",
                duplicate_of
            );
            if let Err(error) = record_deposit_credit_after_async(caller, token_ledger, amount) {
                refund_user(
                    caller,
                    token_ledger,
                    amount,
                    "deposit: pool balance mutation blocked after duplicate transfer",
                )
                .await;
                return Err(error);
            }
            Ok(())
        }
        Ok((Err(transfer_error),)) => {
            log!(INFO, "Transfer failed: {:?}", transfer_error);
            Err(StabilityPoolError::LedgerTransferFailed {
                reason: format!("{:?}", transfer_error),
            })
        }
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
    // SP-102: refuse balance-mutating ops while a liquidation is apportioning,
    // so a withdraw cannot land between a liquidation's snapshot and its burn
    // apportionment and escape the depositor's share of the loss.
    if crate::pool_balance_mutation_blocked() {
        return Err(StabilityPoolError::SystemBusy);
    }
    let caller = ic_cdk::api::caller();

    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    // Look up the transfer fee so we can deduct it from what the user receives.
    // The ledger charges the fee on top of the transfer amount, so without this
    // the pool's ledger balance drifts below its recorded state on every withdrawal.
    let ledger_fee = read_state(|s| {
        s.stablecoin_registry
            .get(&token_ledger)
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
    let _balance_async_guard = crate::pool_guard::PoolBalanceAsyncGuard::new();

    // User receives amount minus fee; pool pays amount total (transfer + fee)
    let transfer_amount = amount - ledger_fee;
    log!(
        INFO,
        "Withdraw: {} (transfer {} - fee {}) from {} by {}",
        amount,
        transfer_amount,
        ledger_fee,
        token_ledger,
        caller
    );

    let transfer_args = TransferArg {
        to: Account {
            owner: caller,
            subaccount: None,
        },
        amount: transfer_amount.into(),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()),
        from_subaccount: None,
    };

    let result: Result<(Result<candid::Nat, TransferError>,), _> =
        call(token_ledger, "icrc1_transfer", (transfer_args,)).await;

    match result {
        Ok((Ok(block_index),)) => {
            log!(
                INFO,
                "Withdrawal transfer succeeded, block: {}",
                block_index
            );
            mutate_state(|s| {
                s.push_event(
                    caller,
                    PoolEventType::Withdraw {
                        token_ledger,
                        amount,
                    },
                )
            });
            Ok(())
        }
        // Audit Wave-3 (ICRC-003): Duplicate means the previous withdrawal
        // attempt already paid the user. Don't restore — that would double-spend.
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            log!(
                INFO,
                "Withdrawal Duplicate (block {}); previous attempt landed, NOT restoring balance",
                duplicate_of
            );
            mutate_state(|s| {
                s.push_event(
                    caller,
                    PoolEventType::Withdraw {
                        token_ledger,
                        amount,
                    },
                )
            });
            Ok(())
        }
        Ok((Err(transfer_error),)) => {
            log!(
                INFO,
                "Withdrawal transfer failed, rolling back deduction: {:?}",
                transfer_error
            );
            // Rollback: re-credit the user's balance (clear ledger rejection,
            // tokens did NOT leave the pool).
            mutate_state(|s| s.add_deposit(caller, token_ledger, amount));
            Err(StabilityPoolError::LedgerTransferFailed {
                reason: format!("{:?}", transfer_error),
            })
        }
        Err(call_error) => {
            log!(
                INFO,
                "Inter-canister call failed, rolling back deduction: {:?}",
                call_error
            );
            // Rollback: re-credit the user's balance.
            // NOTE: this is the audit ICRC-002 risk pattern — if the ledger
            // committed but the reply was lost, restoring creates a phantom
            // credit. The dedup hash (created_at_time) makes the user's
            // immediate retry land as Duplicate (handled above), so no
            // double-spend in practice; lose-then-don't-retry leaves the
            // deduction restored AND the tokens transferred — a known
            // operational risk that requires manual reconciliation.
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
    // SP-102: refuse balance-mutating ops while a liquidation is apportioning.
    if crate::pool_balance_mutation_blocked() {
        return Err(StabilityPoolError::SystemBusy);
    }
    let caller = ic_cdk::api::caller();

    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    // Read and deduct gains atomically BEFORE transfer.
    // mark_gains_claimed uses saturating_sub and cleans up zero entries.
    let gains = mutate_state(|s| {
        let amount = s
            .deposits
            .get(&caller)
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
    let ledger_fee: u64 = match call::<(), (candid::Nat,)>(collateral_ledger, "icrc1_fee", ()).await
    {
        Ok((fee_nat,)) => {
            let fee_u128: u128 = fee_nat.0.try_into().unwrap_or(0);
            fee_u128 as u64
        }
        Err(e) => {
            // ICRC-004 / SP-203: do NOT fall back to fee=0. The transfer still
            // deducts the real ledger fee, so a zero fallback over-credits the
            // claimant and leaves the pool short by one fee. Use the same
            // conservative fallback as the liquidation gains path (SP-104).
            log!(INFO, "icrc1_fee query failed for collateral {}: {:?}; using conservative fallback {} e8s",
                collateral_ledger, e, crate::liquidation::FALLBACK_COLLATERAL_FEE_E8S);
            crate::liquidation::FALLBACK_COLLATERAL_FEE_E8S
        }
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
    log!(
        INFO,
        "Claim: {} of collateral {} (transfer {} - fee {}) by {}",
        gains,
        collateral_ledger,
        transfer_amount,
        ledger_fee,
        caller
    );

    let transfer_args = TransferArg {
        to: Account {
            owner: caller,
            subaccount: None,
        },
        amount: transfer_amount.into(),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()),
        from_subaccount: None,
    };

    let result: Result<(Result<candid::Nat, TransferError>,), _> =
        call(collateral_ledger, "icrc1_transfer", (transfer_args,)).await;

    match result {
        Ok((Ok(block_index),)) => {
            log!(
                INFO,
                "Collateral claim transfer succeeded, block: {}",
                block_index
            );
            mutate_state(|s| {
                s.push_event(
                    caller,
                    PoolEventType::ClaimCollateral {
                        collateral_ledger,
                        amount: transfer_amount,
                    },
                )
            });
            Ok(transfer_amount)
        }
        // Audit Wave-3 (ICRC-003): Duplicate means the previous claim already
        // paid the user. Don't restore — that would let them claim again.
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            log!(
                INFO,
                "Collateral claim Duplicate (block {}); previous attempt landed, NOT restoring",
                duplicate_of
            );
            mutate_state(|s| {
                s.push_event(
                    caller,
                    PoolEventType::ClaimCollateral {
                        collateral_ledger,
                        amount: transfer_amount,
                    },
                )
            });
            Ok(transfer_amount)
        }
        Ok((Err(transfer_error),)) => {
            log!(
                INFO,
                "Collateral claim failed, rolling back: {:?}",
                transfer_error
            );
            // Rollback: restore the gains (clear ledger rejection — tokens
            // did NOT leave the pool).
            mutate_state(|s| {
                if let Some(pos) = s.deposits.get_mut(&caller) {
                    *pos.collateral_gains.entry(collateral_ledger).or_insert(0) += gains;
                    if let Some(claimed) = pos.total_claimed_gains.get_mut(&collateral_ledger) {
                        *claimed = claimed.saturating_sub(gains);
                    }
                }
            });
            Err(StabilityPoolError::LedgerTransferFailed {
                reason: format!("{:?}", transfer_error),
            })
        }
        Err(call_error) => {
            log!(
                INFO,
                "Inter-canister call failed, rolling back: {:?}",
                call_error
            );
            // Rollback: restore the gains. See withdraw() for the same
            // ICRC-002 caveat about transport-error-then-no-retry.
            mutate_state(|s| {
                if let Some(pos) = s.deposits.get_mut(&caller) {
                    *pos.collateral_gains.entry(collateral_ledger).or_insert(0) += gains;
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
    // SP-102: refuse balance-mutating ops while a liquidation is apportioning.
    if crate::pool_balance_mutation_blocked() {
        return Err(StabilityPoolError::SystemBusy);
    }
    let caller = ic_cdk::api::caller();

    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    let all_gains = read_state(|s| s.get_collateral_gains(&caller));
    let nonzero_gains: BTreeMap<Principal, u64> =
        all_gains.into_iter().filter(|(_, v)| *v > 0).collect();

    if nonzero_gains.is_empty() {
        return Ok(BTreeMap::new());
    }

    let mut claimed = BTreeMap::new();
    for (collateral_ledger, amount) in &nonzero_gains {
        match claim_collateral(*collateral_ledger).await {
            Ok(claimed_amount) => {
                claimed.insert(*collateral_ledger, claimed_amount);
            }
            Err(e) => {
                log!(
                    INFO,
                    "Failed to claim {} from {}: {:?}",
                    amount,
                    collateral_ledger,
                    e
                );
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
    // SP-102: refuse balance-mutating ops while a liquidation is apportioning.
    if crate::pool_balance_mutation_blocked() {
        return Err(StabilityPoolError::SystemBusy);
    }
    let caller = ic_cdk::api::caller();

    let config = read_state(|s| s.get_stablecoin_config(&token_ledger).cloned()).ok_or(
        StabilityPoolError::TokenNotAccepted {
            ledger: token_ledger,
        },
    )?;
    if !config.is_active {
        return Err(StabilityPoolError::TokenNotActive {
            ledger: token_ledger,
        });
    }
    if config.is_lp_token.unwrap_or(false) {
        return Err(StabilityPoolError::TokenNotAccepted {
            ledger: token_ledger,
        });
    }

    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    let amount_e8s = normalize_to_e8s(amount, config.decimals);
    let min_deposit = read_state(|s| s.configuration.min_deposit_e8s);
    if amount_e8s < min_deposit {
        return Err(StabilityPoolError::AmountTooLow {
            minimum_e8s: min_deposit,
        });
    }

    // Find the 3USD config (LP token with underlying_pool set)
    let (three_usd_ledger, three_pool_canister) = read_state(|s| {
        s.stablecoin_registry
            .iter()
            .find(|(_, c)| {
                c.is_lp_token.unwrap_or(false) && c.underlying_pool.is_some() && c.is_active
            })
            .map(|(ledger, c)| (*ledger, c.underlying_pool.unwrap()))
            .ok_or(StabilityPoolError::TokenNotAccepted {
                ledger: token_ledger,
            })
    })?;

    log!(
        INFO,
        "deposit_as_3usd: {} depositing {} of {} via 3pool",
        caller,
        amount,
        token_ledger
    );

    // Step 1: Pull tokens from user
    let transfer_args = TransferFromArgs {
        from: Account {
            owner: caller,
            subaccount: None,
        },
        to: Account {
            owner: ic_cdk::api::id(),
            subaccount: None,
        },
        amount: amount.into(),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()),
        spender_subaccount: None,
    };

    let result: Result<(Result<candid::Nat, TransferFromError>,), _> =
        call(token_ledger, "icrc2_transfer_from", (transfer_args,)).await;

    match result {
        Ok((Ok(_),)) => {}
        Ok((Err(e),)) => {
            return Err(StabilityPoolError::LedgerTransferFailed {
                reason: format!("{:?}", e),
            })
        }
        Err(_e) => {
            return Err(StabilityPoolError::InterCanisterCallFailed {
                target: format!("{}", token_ledger),
                method: "icrc2_transfer_from".to_string(),
            })
        }
    }

    // Step 2: Approve 3pool to spend the token
    let approve_args = ApproveArgs {
        from_subaccount: None,
        spender: Account {
            owner: three_pool_canister,
            subaccount: None,
        },
        amount: candid::Nat::from(amount as u128 * 2), // 2x buffer for fees
        expected_allowance: None,
        expires_at: Some(ic_cdk::api::time() + 300_000_000_000), // 5 min
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()),
    };

    let approve_result: Result<(Result<candid::Nat, ApproveError>,), _> =
        call(token_ledger, "icrc2_approve", (approve_args,)).await;

    if let Err(_) | Ok((Err(_),)) = approve_result {
        refund_user(
            caller,
            token_ledger,
            amount,
            "deposit_as_3usd: icrc2_approve failed",
        )
        .await;
        return Err(StabilityPoolError::InterCanisterCallFailed {
            target: format!("{}", token_ledger),
            method: "icrc2_approve".to_string(),
        });
    }

    // Step 3: Query 3pool to find which coin index this token is
    let pool_status_result: Result<(ThreePoolStatus,), _> =
        call(three_pool_canister, "get_pool_status", ()).await;

    let pool_status = match pool_status_result {
        Ok((status,)) => status,
        Err(_) => {
            refund_user(
                caller,
                token_ledger,
                amount,
                "deposit_as_3usd: get_pool_status failed",
            )
            .await;
            return Err(StabilityPoolError::InterCanisterCallFailed {
                target: "3pool".to_string(),
                method: "get_pool_status".to_string(),
            });
        }
    };

    let coin_index = pool_status
        .tokens
        .iter()
        .position(|t| t.ledger_id == token_ledger);
    let coin_index = match coin_index {
        Some(idx) => idx,
        None => {
            refund_user(
                caller,
                token_ledger,
                amount,
                "deposit_as_3usd: token not in 3pool",
            )
            .await;
            return Err(StabilityPoolError::TokenNotAccepted {
                ledger: token_ledger,
            });
        }
    };

    let mut amounts = vec![0u128; 3];
    amounts[coin_index] = amount as u128;

    // Step 4: Call add_liquidity on the 3pool
    let lp_result: Result<(Result<u128, ThreePoolErrorRemote>,), _> =
        call(three_pool_canister, "add_liquidity", (amounts, 0u128)).await;

    let lp_minted = match lp_result {
        Ok((Ok(lp),)) => lp,
        Ok((Err(e),)) => {
            log!(
                INFO,
                "deposit_as_3usd: 3pool add_liquidity returned error {:?}; refunding {}",
                e,
                amount
            );
            refund_user(
                caller,
                token_ledger,
                amount,
                "deposit_as_3usd: add_liquidity rejected",
            )
            .await;
            return Err(StabilityPoolError::InterCanisterCallFailed {
                target: "3pool".to_string(),
                method: "add_liquidity".to_string(),
            });
        }
        Err((code, msg)) => {
            log!(
                INFO,
                "deposit_as_3usd: 3pool add_liquidity call failed: {:?} {}; refunding {}",
                code,
                msg,
                amount
            );
            refund_user(
                caller,
                token_ledger,
                amount,
                "deposit_as_3usd: add_liquidity call failed",
            )
            .await;
            return Err(StabilityPoolError::InterCanisterCallFailed {
                target: "3pool".to_string(),
                method: "add_liquidity".to_string(),
            });
        }
    };

    // Step 5: Credit user's 3USD balance
    let lp_amount_u64 = lp_minted as u64;
    if let Err(error) = record_deposit_as_3usd_credit_after_async(
        caller,
        token_ledger,
        amount,
        three_usd_ledger,
        lp_amount_u64,
    ) {
        refund_user(
            caller,
            three_usd_ledger,
            lp_amount_u64,
            "deposit_as_3usd: pool balance mutation blocked after LP mint",
        )
        .await;
        return Err(error);
    }

    log!(
        INFO,
        "deposit_as_3usd: {} deposited {} of {} → {} 3USD LP",
        caller,
        amount,
        token_ledger,
        lp_amount_u64
    );

    Ok(lp_amount_u64)
}

/// Refund the pulled tokens to the user after a failed deposit_as_3usd.
///
/// IC-S-001: the refund sends `amount` NET of the ledger transfer fee so the
/// pool's ledger balance drops by exactly `amount` (a gross refund cost
/// amount+fee, drifting the pool one fee below its tracked deposits per
/// refund). If the refund transfer itself fails, the amount is persisted as a
/// pending refund recoverable via `claim_pending_refund` instead of being
/// silently stranded.
async fn refund_user(user: Principal, token_ledger: Principal, amount: u64, reason: &str) {
    let fee = refund_ledger_fee(token_ledger).await;
    if amount <= fee {
        // Nothing transferable once the ledger fee is covered; the dust stays
        // in the pool (solvency-safe, mirrors rumi_3pool::transfer_to_user).
        log!(
            INFO,
            "refund_user: {} of {} for {} not refundable (<= ledger fee {}); leaving as pool dust",
            amount,
            token_ledger,
            user,
            fee
        );
        return;
    }
    let transfer_args = TransferArg {
        to: Account {
            owner: user,
            subaccount: None,
        },
        amount: (amount - fee).into(),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()),
        from_subaccount: None,
    };
    let result: Result<(Result<candid::Nat, TransferError>,), _> =
        call(token_ledger, "icrc1_transfer", (transfer_args,)).await;
    let failure = match result {
        Ok((Ok(_),)) => None,
        // Duplicate: a previous refund attempt already paid the user.
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            log!(
                INFO,
                "refund_user: Duplicate (block {}); previous refund landed",
                duplicate_of
            );
            None
        }
        Ok((Err(e),)) => Some(format!("{}; refund transfer failed: {:?}", reason, e)),
        Err(e) => Some(format!("{}; refund call failed: {:?}", reason, e)),
    };
    if let Some(why) = failure {
        let id = mutate_state(|s| {
            s.record_pending_refund(user, token_ledger, amount, why.clone(), ic_cdk::api::time())
        });
        log!(
            INFO,
            "refund_user: refund of {} {} to {} failed ({}); recorded pending refund #{}",
            amount,
            token_ledger,
            user,
            why,
            id
        );
    }
}

/// Recover tokens the pool owes after a failed deposit_as_3usd refund
/// (IC-S-001). Callable by the original user or a pool admin. The record is
/// removed BEFORE the async transfer (so two concurrent claims cannot both pay
/// out) and re-inserted if the transfer fails. Returns the net amount sent
/// (gross minus the ledger fee). Mirrors rumi_3pool::claim_pending.
pub async fn claim_pending_refund(refund_id: u64) -> Result<u64, StabilityPoolError> {
    // SP-102: refuse balance-mutating ops while a liquidation is apportioning.
    if crate::pool_balance_mutation_blocked() {
        return Err(StabilityPoolError::SystemBusy);
    }
    let caller = ic_cdk::api::caller();

    let refund = mutate_state(|s| s.take_pending_refund(refund_id))
        .ok_or(StabilityPoolError::RefundClaimNotFound)?;

    if caller != refund.user && !read_state(|s| s.is_admin(&caller)) {
        // Not authorized; re-insert before returning so the record is not lost.
        mutate_state(|s| s.put_pending_refund(refund));
        return Err(StabilityPoolError::Unauthorized);
    }

    let fee = refund_ledger_fee(refund.token_ledger).await;
    if refund.amount <= fee {
        // Nothing transferable once the fee is covered; drop the record and
        // leave the dust in the pool (solvency-safe).
        log!(INFO, "claim_pending_refund: refund #{} of {} {} not payable (<= ledger fee {}); record dropped",
            refund_id, refund.amount, refund.token_ledger, fee);
        return Ok(0);
    }
    let net = refund.amount - fee;

    let transfer_args = TransferArg {
        to: Account {
            owner: refund.user,
            subaccount: None,
        },
        amount: net.into(),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()),
        from_subaccount: None,
    };
    let result: Result<(Result<candid::Nat, TransferError>,), _> =
        call(refund.token_ledger, "icrc1_transfer", (transfer_args,)).await;

    match result {
        Ok((Ok(block_index),)) => {
            log!(
                INFO,
                "claim_pending_refund: refund #{} paid {} (net of fee {}) of {} to {}, block {}",
                refund_id,
                net,
                fee,
                refund.token_ledger,
                refund.user,
                block_index
            );
            Ok(net)
        }
        // Duplicate: a previous claim attempt already paid the user.
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            log!(
                INFO,
                "claim_pending_refund: refund #{} Duplicate (block {}); previous attempt landed",
                refund_id,
                duplicate_of
            );
            Ok(net)
        }
        Ok((Err(transfer_error),)) => {
            log!(
                INFO,
                "claim_pending_refund: refund #{} transfer failed, re-inserting: {:?}",
                refund_id,
                transfer_error
            );
            let reason = format!("{:?}", transfer_error);
            mutate_state(|s| s.put_pending_refund(refund));
            Err(StabilityPoolError::LedgerTransferFailed { reason })
        }
        Err(call_error) => {
            log!(
                INFO,
                "claim_pending_refund: refund #{} call failed, re-inserting: {:?}",
                refund_id,
                call_error
            );
            let target = format!("{}", refund.token_ledger);
            mutate_state(|s| s.put_pending_refund(refund));
            Err(StabilityPoolError::InterCanisterCallFailed {
                target,
                method: "icrc1_transfer".to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rumi_protocol_backend::chains::config::ChainId;

    fn principal(byte: u8) -> Principal {
        Principal::from_slice(&[byte])
    }

    fn pending_intent() -> ChainSpAbsorbIntent {
        let mut stables_consumed = BTreeMap::new();
        stables_consumed.insert(principal(10), 100_00000000);
        ChainSpAbsorbIntent {
            vault_id: 77,
            chain_id: ChainId(1030),
            chain_sentinel: crate::state::chain_collateral_sentinel(1030),
            icusd_ledger: principal(10),
            icusd_minting_account: Account {
                owner: principal(90),
                subaccount: None,
            },
            icusd_to_burn_e8s: 100_00000000,
            stables_consumed,
            burn_created_at_time_ns: 123,
            status: ChainSpAbsorbIntentStatus::Burned,
            burn_proof: Some(rumi_protocol_backend::icrc3_proof::SpWritedownProof {
                block_index: 44,
                ledger_kind: rumi_protocol_backend::icrc3_proof::SpProofLedger::IcusdBurn,
                vault_id_memo: 77,
            }),
            backend_result: None,
            last_error: None,
            created_at_ns: 123,
            updated_at_ns: 456,
        }
    }

    #[test]
    fn post_await_deposit_credit_rechecks_pending_chain_absorbs() {
        crate::state::replace_state(crate::state::StabilityPoolState::default());
        mutate_state(|s| s.put_pending_chain_absorb(pending_intent()).unwrap());

        let result = record_deposit_credit_after_async(principal(1), principal(10), 50_00000000);

        assert!(
            matches!(result, Err(StabilityPoolError::SystemBusy)),
            "post-await deposit credit must stop when chain absorb is pending",
        );
        assert_eq!(
            read_state(|s| s
                .total_stablecoin_balances
                .get(&principal(10))
                .copied()
                .unwrap_or(0)),
            0,
            "blocked post-await credit must not mutate the SP denominator",
        );
        crate::state::replace_state(crate::state::StabilityPoolState::default());
    }

    #[test]
    fn post_await_deposit_as_3usd_credit_rechecks_pending_chain_absorbs() {
        crate::state::replace_state(crate::state::StabilityPoolState::default());
        mutate_state(|s| s.put_pending_chain_absorb(pending_intent()).unwrap());

        let result = record_deposit_as_3usd_credit_after_async(
            principal(1),
            principal(10),
            50_00000000,
            principal(30),
            49_00000000,
        );

        assert!(
            matches!(result, Err(StabilityPoolError::SystemBusy)),
            "post-await 3USD credit must stop when chain absorb is pending",
        );
        assert_eq!(
            read_state(|s| s
                .total_stablecoin_balances
                .get(&principal(30))
                .copied()
                .unwrap_or(0)),
            0,
            "blocked post-await LP credit must not mutate the SP denominator",
        );
        crate::state::replace_state(crate::state::StabilityPoolState::default());
    }
}
