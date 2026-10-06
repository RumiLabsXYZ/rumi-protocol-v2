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
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Conservative fallback for a stablecoin ledger's transfer fee (native units),
/// used when the live `icrc1_fee` query fails. Known stablecoin registry values
/// are normalized on registration/upgrade; for unknown ledgers, erring high
/// keeps the pool solvent rather than risking an over-send.
const DEFAULT_LEDGER_FEE: u64 = 10_000;
const MAX_PENDING_REFUND_HISTORY_BLOCKS_PER_CALL: u64 = 64;

thread_local! {
    /// Per-ledger transfer-fee cache for transfer/refund math, populated lazily from
    /// `icrc1_fee`. Heap-only (not persisted), so it is simply re-warmed after
    /// an upgrade. Mirrors rumi_3pool::transfers::LEDGER_FEES.
    static LEDGER_FEES: RefCell<HashMap<Principal, u64>> = RefCell::new(HashMap::new());
    static PENDING_REFUND_CLAIMS: RefCell<BTreeSet<u64>> = RefCell::new(BTreeSet::new());
}

struct PendingRefundClaimGuard(u64);

impl PendingRefundClaimGuard {
    fn new(refund_id: u64) -> Result<Self, StabilityPoolError> {
        PENDING_REFUND_CLAIMS.with(|active| {
            if !active.borrow_mut().insert(refund_id) {
                return Err(StabilityPoolError::SystemBusy);
            }
            Ok(Self(refund_id))
        })
    }
}

impl Drop for PendingRefundClaimGuard {
    fn drop(&mut self) {
        PENDING_REFUND_CLAIMS.with(|active| {
            active.borrow_mut().remove(&self.0);
        });
    }
}

fn complete_deposit_credit_after_async(
    caller: Principal,
    token_ledger: Principal,
    amount: u64,
    timestamp: u64,
) -> Result<(), StabilityPoolError> {
    crate::ensure_pool_balance_mutation_allowed()?;
    complete_deposit_credit_after_async_at(
        caller,
        token_ledger,
        amount,
        timestamp,
        ic_cdk::api::time(),
    )
}

fn complete_deposit_credit_after_async_at(
    caller: Principal,
    token_ledger: Principal,
    amount: u64,
    timestamp: u64,
    now_ns: u64,
) -> Result<(), StabilityPoolError> {
    crate::ensure_pool_balance_mutation_allowed()?;
    // A retry may receive Duplicate after an earlier concurrent callback has
    // already completed this exact intent. In that case, leave the credited
    // balance and event untouched.
    mutate_state(|s| {
        s.complete_deposit_intent(
            caller,
            token_ledger,
            amount,
            timestamp,
            now_ns,
        )
    });
    Ok(())
}

fn complete_saved_deposit_receipt_before_admission(
    caller: Principal,
    token_ledger: Principal,
    amount: u64,
    now_ns: u64,
) -> Result<Option<()>, StabilityPoolError> {
    let receipt = read_state(|s| {
        s.pending_deposit_intents
            .as_ref()
            .and_then(|intents| intents.get(&caller))
            .filter(|intent| intent.token_ledger == token_ledger && intent.amount == amount)
            .and_then(|intent| {
                intent
                    .transfer_block_index
                    .map(|_| intent.transfer_created_at_time_ns)
            })
    });
    let Some(timestamp) = receipt else {
        return Ok(None);
    };

    // A recorded ledger receipt proves the pull already happened. Finalize
    // that exact credit before applying admission policy to a new deposit,
    // while still respecting accounting guards.
    crate::ensure_pool_balance_mutation_allowed()?;
    let completed = mutate_state(|s| {
        s.complete_deposit_intent(caller, token_ledger, amount, timestamp, now_ns)
    });
    if completed {
        Ok(Some(()))
    } else {
        Err(StabilityPoolError::SystemBusy)
    }
}

fn pending_deposit_admission_error(
    error: StabilityPoolError,
    pending_without_receipt: bool,
) -> StabilityPoolError {
    if pending_without_receipt {
        StabilityPoolError::LedgerTransferFailed {
            reason: format!(
                "pending deposit has no saved receipt; admission policy now rejects retry ({error:?}); exact ledger reconciliation required"
            ),
        }
    } else {
        error
    }
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

fn fallback_ledger_fee(ledger: Principal) -> u64 {
    read_state(|s| {
        let Some(config) = s.stablecoin_registry.get(&ledger) else {
            return DEFAULT_LEDGER_FEE;
        };
        let configured = config.transfer_fee.unwrap_or(0);
        let known = crate::state::known_stablecoin_transfer_fee(&config.symbol, config.decimals)
            .unwrap_or(DEFAULT_LEDGER_FEE);
        configured.max(known)
    })
}

/// Fetch a stablecoin ledger's transfer fee, caching successful lookups per
/// ledger. On query failure, falls back to normalized registry metadata without
/// caching so the next transfer re-queries.
pub(crate) async fn ledger_transfer_fee(ledger: Principal) -> u64 {
    if let Some(fee) = LEDGER_FEES.with(|c| c.borrow().get(&ledger).copied()) {
        return fee;
    }
    match call::<(), (candid::Nat,)>(ledger, "icrc1_fee", ()).await {
        Ok((fee_nat,)) => {
            let fee: u64 = fee_nat.0.try_into().unwrap_or(DEFAULT_LEDGER_FEE);
            LEDGER_FEES.with(|c| c.borrow_mut().insert(ledger, fee));
            fee
        }
        Err(e) => {
            let fallback = fallback_ledger_fee(ledger);
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

/// Result of an unallocated-interest transfer attempt. A `BadFee` is known not
/// to have moved funds, so the durable receipt may safely update its fee and
/// retry with the same timestamp/memo.
pub enum UnallocatedInterestTransferResult {
    Sent(u64),
    BadFee(u64),
    TooOld,
}

pub async fn unallocated_interest_transfer_fee(token_ledger: Principal) -> u64 {
    ledger_transfer_fee(token_ledger).await
}

/// Submit one persisted unallocated-interest batch to treasury. Callers must
/// reuse the stored fee, timestamp, and memo on every retry; ICRC-003
/// `Duplicate` is therefore the same successful transfer.
pub async fn transfer_unallocated_interest_to_treasury(
    token_ledger: Principal,
    treasury: Principal,
    net_amount: u64,
    fee: u64,
    created_at_time: u64,
    memo: Vec<u8>,
) -> Result<UnallocatedInterestTransferResult, StabilityPoolError> {
    crate::ensure_pool_balance_mutation_allowed_for_ledger(token_ledger)?;
    let transfer_args = TransferArg {
        from_subaccount: None,
        to: Account {
            owner: treasury,
            subaccount: None,
        },
        amount: net_amount.into(),
        fee: Some(fee.into()),
        memo: Some(memo.into()),
        created_at_time: Some(created_at_time),
    };

    let result: Result<(Result<candid::Nat, TransferError>,), _> =
        call(token_ledger, "icrc1_transfer", (transfer_args,)).await;
    match result {
        Ok((Ok(block_index),)) => {
            let block_index: u64 =
                block_index
                    .0
                    .try_into()
                    .map_err(|_| StabilityPoolError::LedgerTransferFailed {
                        reason: "treasury transfer block index exceeds u64".to_string(),
                    })?;
            Ok(UnallocatedInterestTransferResult::Sent(block_index))
        }
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            let block_index: u64 = duplicate_of.0.try_into().map_err(|_| {
                StabilityPoolError::LedgerTransferFailed {
                    reason: "duplicate treasury transfer block index exceeds u64".to_string(),
                }
            })?;
            Ok(UnallocatedInterestTransferResult::Sent(block_index))
        }
        Ok((Err(TransferError::BadFee { expected_fee }),)) => {
            let expected_fee: u64 = expected_fee.0.try_into().map_err(|_| {
                StabilityPoolError::LedgerTransferFailed {
                    reason: "treasury transfer fee exceeds u64".to_string(),
                }
            })?;
            Ok(UnallocatedInterestTransferResult::BadFee(expected_fee))
        }
        Ok((Err(TransferError::TooOld),)) => Ok(UnallocatedInterestTransferResult::TooOld),
        Ok((Err(error),)) => Err(StabilityPoolError::LedgerTransferFailed {
            reason: format!("unallocated interest transfer failed: {:?}", error),
        }),
        Err(_) => Err(StabilityPoolError::InterCanisterCallFailed {
            target: format!("{}", token_ledger),
            method: "icrc1_transfer".to_string(),
        }),
    }
}

pub(crate) async fn ledger_pool_balance(ledger: Principal) -> Option<u64> {
    let account = Account {
        owner: ic_cdk::api::id(),
        subaccount: None,
    };
    match call::<(Account,), (candid::Nat,)>(ledger, "icrc1_balance_of", (account,)).await {
        Ok((balance_nat,)) => Some(balance_nat.0.try_into().unwrap_or(u64::MAX)),
        Err(e) => {
            log!(
                INFO,
                "icrc1_balance_of query failed for {}: {:?}; continuing without ledger shortfall reconciliation",
                ledger,
                e
            );
            None
        }
    }
}

fn prepare_withdrawal_after_ledger_check(
    caller: Principal,
    token_ledger: Principal,
    requested_amount: u64,
    ledger_balance: Option<u64>,
    ledger_fee: u64,
) -> Result<(u64, Option<String>), StabilityPoolError> {
    mutate_state(|s| {
        let mut withdrawal_amount = requested_amount;
        let mut correction_msg = None;

        if let Some(live_balance) = ledger_balance {
            let user_balance = s
                .deposits
                .get(&caller)
                .and_then(|pos| pos.stablecoin_balances.get(&token_ledger).copied())
                .unwrap_or(0);
            let aggregate_balance = s
                .total_stablecoin_balances
                .get(&token_ledger)
                .copied()
                .unwrap_or(0);

            if live_balance < aggregate_balance {
                if live_balance <= ledger_fee {
                    return Err(StabilityPoolError::AmountTooLow {
                        minimum_e8s: ledger_fee + 1,
                    });
                }

                let sole_holder = user_balance > 0 && user_balance == aggregate_balance;
                let drains_recorded_position = requested_amount == user_balance;
                let drains_live_balance = requested_amount == live_balance;

                if sole_holder && (drains_recorded_position || drains_live_balance) {
                    let msg = s.correct_balance(caller, token_ledger, live_balance);
                    s.push_event(
                        caller,
                        PoolEventType::BalanceCorrected {
                            user: caller,
                            token_ledger,
                            new_amount: live_balance,
                        },
                    );
                    correction_msg = Some(msg);
                    withdrawal_amount = live_balance;
                } else {
                    return Err(StabilityPoolError::InsufficientPoolBalance);
                }
            } else if live_balance < requested_amount {
                return Err(StabilityPoolError::InsufficientPoolBalance);
            }
        }

        s.process_withdrawal(caller, token_ledger, withdrawal_amount)?;
        Ok((withdrawal_amount, correction_msg))
    })
}

/// Deposit a stablecoin into the pool. User must have pre-approved the pool canister.
pub async fn deposit(token_ledger: Principal, amount: u64) -> Result<(), StabilityPoolError> {
    // SP-102: refuse balance-mutating ops while a liquidation is apportioning.
    crate::ensure_pool_balance_mutation_allowed_for_ledger(token_ledger)?;
    let caller = ic_cdk::api::caller();

    if complete_saved_deposit_receipt_before_admission(
        caller,
        token_ledger,
        amount,
        ic_cdk::api::time(),
    )?
    .is_some()
    {
        return Ok(());
    }

    let pending_without_receipt = read_state(|s| {
        s.pending_deposit_intents
            .as_ref()
            .and_then(|intents| intents.get(&caller))
            .is_some_and(|intent| {
                intent.token_ledger == token_ledger
                    && intent.amount == amount
                    && intent.transfer_block_index.is_none()
            })
    });
    // Validate token is accepted
    let config = read_state(|s| s.get_stablecoin_config(&token_ledger).cloned())
        .ok_or(StabilityPoolError::TokenNotAccepted {
            ledger: token_ledger,
        })
        .map_err(|error| pending_deposit_admission_error(error, pending_without_receipt))?;

    if !config.is_active {
        return Err(pending_deposit_admission_error(
            StabilityPoolError::TokenNotActive {
                ledger: token_ledger,
            },
            pending_without_receipt,
        ));
    }

    // Validate minimum deposit (normalize to e8s for comparison)
    let amount_e8s = normalize_to_e8s(amount, config.decimals);
    let min_deposit = read_state(|s| s.configuration.min_deposit_e8s);
    if amount_e8s < min_deposit {
        return Err(pending_deposit_admission_error(
            StabilityPoolError::AmountTooLow {
                minimum_e8s: min_deposit,
            },
            pending_without_receipt,
        ));
    }

    if read_state(|s| s.configuration.emergency_pause) {
        return Err(pending_deposit_admission_error(
            StabilityPoolError::EmergencyPaused,
            pending_without_receipt,
        ));
    }

    log!(
        INFO,
        "Deposit: {} {} ({}) from {}",
        amount,
        config.symbol,
        token_ledger,
        caller
    );

    // Persist the exact transfer identity before dispatch. Same-caller retries
    // reuse this identity; a distinct request gets a strictly newer timestamp.
    let (transfer_timestamp, prior_receipt) = mutate_state(|s| {
        let timestamp =
            s.begin_deposit_intent(caller, token_ledger, amount, ic_cdk::api::time())?;
        let receipt = s
            .pending_deposit_intents
            .as_ref()
            .and_then(|intents| intents.get(&caller))
            .and_then(|intent| intent.transfer_block_index);
        Ok::<_, ()>((timestamp, receipt))
    })
    .map_err(|_| StabilityPoolError::SystemBusy)?;

    if prior_receipt.is_some() {
        return complete_deposit_credit_after_async(
            caller,
            token_ledger,
            amount,
            transfer_timestamp,
        );
    }

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
        created_at_time: Some(transfer_timestamp),
        spender_subaccount: None,
    };

    let result: Result<(Result<candid::Nat, TransferFromError>,), _> =
        call(token_ledger, "icrc2_transfer_from", (transfer_args,)).await;

    match result {
        Ok((Ok(block_index),)) => {
            log!(INFO, "Transfer succeeded, block: {}", block_index);
            let block_index: u64 =
                block_index
                    .0
                    .try_into()
                    .map_err(|_| StabilityPoolError::LedgerTransferFailed {
                        reason: "deposit transfer block index exceeds u64; intent retained"
                            .to_string(),
                    })?;
            let recorded = mutate_state(|s| {
                s.record_deposit_receipt(
                    caller,
                    token_ledger,
                    amount,
                    transfer_timestamp,
                    block_index,
                )
            });
            if !recorded {
                return Err(StabilityPoolError::SystemBusy);
            }
            complete_deposit_credit_after_async(caller, token_ledger, amount, transfer_timestamp)?;
            log!(INFO, "Deposit recorded for {}", caller);
            Ok(())
        }
        // Duplicate is success only for the exact persisted intent. Completion
        // removes that intent atomically, so concurrent replies cannot credit
        // this one ledger transfer twice.
        Ok((Err(TransferFromError::Duplicate { duplicate_of }),)) => {
            log!(
                INFO,
                "Deposit transfer Duplicate (block {}); completing exact pending intent",
                duplicate_of
            );
            let block_index: u64 = duplicate_of.0.try_into().map_err(|_| {
                StabilityPoolError::LedgerTransferFailed {
                    reason: "duplicate deposit block index exceeds u64; intent retained"
                        .to_string(),
                }
            })?;
            let recorded = mutate_state(|s| {
                s.record_deposit_receipt(
                    caller,
                    token_ledger,
                    amount,
                    transfer_timestamp,
                    block_index,
                )
            });
            if !recorded {
                return Err(StabilityPoolError::SystemBusy);
            }
            complete_deposit_credit_after_async(caller, token_ledger, amount, transfer_timestamp)
        }
        Ok((Err(transfer_error),))
            if matches!(
                &transfer_error,
                TransferFromError::BadFee { .. }
                    | TransferFromError::BadBurn { .. }
                    | TransferFromError::InsufficientFunds { .. }
                    | TransferFromError::InsufficientAllowance { .. }
                    | TransferFromError::CreatedInFuture { .. }
            ) =>
        {
            let cleared = mutate_state(|s| {
                s.clear_deposit_intent_after_no_effect(
                    caller,
                    token_ledger,
                    amount,
                    transfer_timestamp,
                )
            });
            if !cleared {
                return Err(StabilityPoolError::LedgerTransferFailed {
                    reason: "deposit intent retained because another dispatch is in flight or an earlier outcome was ambiguous"
                        .to_string(),
                });
            }
            log!(INFO, "Transfer failed: {:?}", transfer_error);
            Err(StabilityPoolError::LedgerTransferFailed {
                reason: format!("{:?}", transfer_error),
            })
        }
        Ok((Err(transfer_error),)) => {
            // TooOld, TemporarilyUnavailable, and GenericError do not prove
            // whether a prior dispatch committed. Keep the identity for retry.
            // If every reply was lost until the ledger's duplicate window
            // expires, exact ICRC-3 receipt reconciliation is still required;
            // this narrow change deliberately fails closed in that case.
            mutate_state(|s| {
                s.mark_deposit_intent_ambiguous(
                    caller,
                    token_ledger,
                    amount,
                    transfer_timestamp,
                )
            });
            log!(INFO, "Transfer failed: {:?}", transfer_error);
            Err(StabilityPoolError::LedgerTransferFailed {
                reason: "deposit outcome unresolved; original transfer identity retained"
                    .to_string(),
            })
        }
        Err(call_error) => {
            mutate_state(|s| {
                s.mark_deposit_intent_ambiguous(
                    caller,
                    token_ledger,
                    amount,
                    transfer_timestamp,
                )
            });
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
    crate::ensure_pool_balance_mutation_allowed_for_ledger(token_ledger)?;
    let caller = ic_cdk::api::caller();

    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    // The ledger debits `transfer_amount + fee` from the pool. Query the live
    // fee first so a max withdrawal drains the user's recorded position without
    // overdrawing the pool ledger account.
    let ledger_fee = ledger_transfer_fee(token_ledger).await;
    let pool_ledger_balance = ledger_pool_balance(token_ledger).await;

    if amount <= ledger_fee {
        return Err(StabilityPoolError::AmountTooLow {
            minimum_e8s: ledger_fee + 1,
        });
    }

    // Deduct full amount from state BEFORE transfer to prevent double-spend.
    // If the transfer fails, we rollback below.
    let (withdrawal_amount, correction_msg) = prepare_withdrawal_after_ledger_check(
        caller,
        token_ledger,
        amount,
        pool_ledger_balance,
        ledger_fee,
    )?;
    if let Some(msg) = correction_msg {
        log!(INFO, "Withdrawal reconciled ledger shortfall: {}", msg);
    }
    let _balance_async_guard = crate::pool_guard::PoolBalanceAsyncGuard::new();

    // User receives amount minus fee; pool pays amount total (transfer + fee)
    let transfer_amount = withdrawal_amount - ledger_fee;
    log!(
        INFO,
        "Withdraw: {} (transfer {} - fee {}) from {} by {}",
        withdrawal_amount,
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
                        amount: withdrawal_amount,
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
                        amount: withdrawal_amount,
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
            mutate_state(|s| s.add_deposit(caller, token_ledger, withdrawal_amount));
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
            mutate_state(|s| s.add_deposit(caller, token_ledger, withdrawal_amount));
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
    crate::ensure_pool_balance_mutation_allowed_for_ledger(collateral_ledger)?;
    let caller = ic_cdk::api::caller();

    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    read_state(|s| s.ensure_icrc_claimable_collateral(&collateral_ledger))?;

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

    let all_gains = read_state(|s| s.get_claimable_icrc_collateral_gains(&caller));
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

/// Keep the 3USD conversion entry point closed until the 3pool supports a
/// durable add-liquidity receipt. The legacy flow below remains available for
/// reference while the cross-canister receipt API is coordinated, but this
/// wrapper never enters it and therefore performs no ledger call.
pub async fn deposit_as_3usd(
    token_ledger: Principal,
    amount: u64,
) -> Result<u64, StabilityPoolError> {
    let continue_conversion = require_receipt_backed_3usd_conversion(|| {
        deposit_as_3usd_inner(token_ledger, amount)
    })?;
    continue_conversion.await
}

fn require_receipt_backed_3usd_conversion<T>(
    _continue_conversion: impl FnOnce() -> T,
) -> Result<T, StabilityPoolError> {
    // Method availability alone does not prove that the 3pool pull and LP mint
    // can be reconciled across lost replies. Keep this before the input-token
    // pull until the 3pool exposes a durable receipt for that operation.
    Err(StabilityPoolError::InterCanisterCallFailed {
        method: "deposit_as_3usd: receipt-backed conversion unavailable; no input tokens were pulled"
            .to_string(),
        target: "3pool add-liquidity recovery".to_string(),
    })
}

/// Legacy financial path retained until it can be replaced with the complete
/// receipt-backed saga. It is reachable only through the fail-closed wrapper
/// above, whose operation closure is never invoked.
async fn deposit_as_3usd_inner(
    token_ledger: Principal,
    amount: u64,
) -> Result<u64, StabilityPoolError> {
    // SP-102: refuse balance-mutating ops while a liquidation is apportioning.
    crate::ensure_pool_balance_mutation_allowed_for_ledger(token_ledger)?;
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
    crate::ensure_pool_balance_mutation_allowed_for_ledger(three_usd_ledger)?;

    log!(
        INFO,
        "deposit_as_3usd: {} depositing {} of {} via 3pool",
        caller,
        amount,
        token_ledger
    );

    // Share the timestamp allocator with ordinary deposits. Otherwise these
    // distinct workflows can submit identical ICRC-2 arguments in one IC time
    // round and have one physical pull satisfy both accounting paths.
    let transfer_timestamp = mutate_state(|s| {
        s.reserve_deposit_transfer_timestamp(ic_cdk::api::time())
    })
    .map_err(|_| StabilityPoolError::SystemBusy)?;

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
        created_at_time: Some(transfer_timestamp),
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
/// The full principal obligation is journaled before any transfer attempt.
/// Protocol-funded fee capacity is reserved separately; without it, the row
/// remains pending and no user principal is netted to pay the ledger fee.
async fn refund_user(user: Principal, token_ledger: Principal, amount: u64, reason: &str) {
    let id = mutate_state(|s| {
        s.record_pending_refund(
            user,
            token_ledger,
            amount,
            reason.to_string(),
            ic_cdk::api::time(),
        )
    });
    match claim_pending_refund(id).await {
        Ok(paid) => log!(
            INFO,
            "refund_user: refund #{} paid full principal {} to {}",
            id,
            paid,
            user
        ),
        Err(error) => log!(
            INFO,
            "refund_user: full-principal refund #{} remains journaled for {} on {}: {:?}",
            id,
            user,
            token_ledger,
            error
        ),
    }
}

/// Recover tokens the pool owes after a failed deposit_as_3usd refund
/// (IC-S-001). Callable by the original user or a pool admin. The durable
/// obligation and exact tuple remain stored across the await. Returns the full
/// principal only after an exact ICRC-3 receipt is verified.
pub async fn claim_pending_refund(refund_id: u64) -> Result<u64, StabilityPoolError> {
    // SP-102: refuse balance-mutating ops while a liquidation is apportioning.
    if crate::pool_balance_mutation_blocked() {
        return Err(StabilityPoolError::SystemBusy);
    }
    let _refund_guard = PendingRefundClaimGuard::new(refund_id)?;
    let _balance_async_guard = crate::pool_guard::PoolBalanceAsyncGuard::new();
    let caller = ic_cdk::api::caller();

    let refund = read_state(|s| {
        s.pending_refunds
            .as_ref()
            .and_then(|refunds| refunds.get(&refund_id).cloned())
    })
    .ok_or(StabilityPoolError::RefundClaimNotFound)?;

    if caller != refund.user && !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }

    // Missing markers belong to legacy state and cannot establish whether an
    // earlier transfer committed. Keep the full obligation held for evidence.
    if refund.transfer_attempted.is_none() {
        return Err(StabilityPoolError::LedgerTransferFailed {
            reason: "legacy refund has unknown transfer history and remains held".into(),
        });
    }
    if refund.transfer_too_old_rejected == Some(true) {
        return Err(StabilityPoolError::LedgerTransferFailed {
            reason: "this exact refund tuple was rejected as TooOld; reconcile its receipt or complete the audited icUSD history scan before retrying".into(),
        });
    }
    let refund = if refund.transfer_attempted == Some(true) {
        mutate_state(|s| {
            s.prepare_pending_refund_transfer(
                refund_id,
                refund.transfer_fee.unwrap_or(0),
                refund.transfer_created_at_time_ns.unwrap_or(0),
                refund.transfer_memo.clone().unwrap_or_default(),
            )
        })
        .map_err(|reason| StabilityPoolError::LedgerTransferFailed {
            reason: reason.to_string(),
        })?
    } else {
        let fee = ledger_transfer_fee(refund.token_ledger).await;
        let timestamp = ic_cdk::api::time().max(refund.created_at);
        let attempt_no = refund.transfer_attempt_no.unwrap_or(0);
        let mut memo = b"rumi-sp-refund-v1:".to_vec();
        memo.extend_from_slice(&refund.id.to_be_bytes());
        memo.extend_from_slice(&attempt_no.to_be_bytes());
        mutate_state(|s| s.prepare_pending_refund_transfer(refund_id, fee, timestamp, memo))
            .map_err(|reason| StabilityPoolError::LedgerTransferFailed {
                reason: reason.to_string(),
            })?
    };
    let fee = refund
        .transfer_fee
        .ok_or_else(|| StabilityPoolError::LedgerTransferFailed {
            reason: "pending refund fee is missing from its transfer journal".into(),
        })?;
    let timestamp = refund.transfer_created_at_time_ns.ok_or_else(|| {
        StabilityPoolError::LedgerTransferFailed {
            reason: "pending refund timestamp is missing from its transfer journal".into(),
        }
    })?;
    let memo =
        refund
            .transfer_memo
            .clone()
            .ok_or_else(|| StabilityPoolError::LedgerTransferFailed {
                reason: "pending refund memo is missing from its transfer journal".into(),
            })?;

    let transfer_args = TransferArg {
        to: Account {
            owner: refund.user,
            subaccount: None,
        },
        amount: refund.amount.into(),
        fee: Some(fee.into()),
        memo: Some(memo.clone().into()),
        created_at_time: Some(timestamp),
        from_subaccount: None,
    };
    let result: Result<(Result<candid::Nat, TransferError>,), _> =
        call(refund.token_ledger, "icrc1_transfer", (transfer_args,)).await;

    match result {
        Ok((Ok(block_index),)) => {
            let block_index: u64 =
                block_index
                    .0
                    .try_into()
                    .map_err(|_| StabilityPoolError::LedgerTransferFailed {
                        reason: "refund block index exceeds u64".into(),
                    })?;
            rumi_protocol_backend::icrc3_proof::verify_icrc3_transfer_block_with_fee(
                refund.token_ledger,
                block_index,
                Account {
                    owner: ic_cdk::id(),
                    subaccount: None,
                },
                Account {
                    owner: refund.user,
                    subaccount: None,
                },
                refund.amount,
                fee,
                Some(&memo),
                Some(timestamp),
            )
            .await
            .map_err(|reason| StabilityPoolError::LedgerTransferFailed {
                reason: format!("refund receipt did not verify; obligation remains held: {reason}"),
            })?;
            log!(
                INFO,
                "claim_pending_refund: refund #{} paid full principal {} (protocol fee {}) of {} to {}, block {}",
                refund_id,
                refund.amount,
                fee,
                refund.token_ledger,
                refund.user,
                block_index
            );
            mutate_state(|s| {
                s.complete_pending_refund(refund_id);
            });
            Ok(refund.amount)
        }
        // Duplicate: a previous claim attempt already paid the user.
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            let block_index: u64 = duplicate_of.0.try_into().map_err(|_| {
                StabilityPoolError::LedgerTransferFailed {
                    reason: "duplicate refund block index exceeds u64".into(),
                }
            })?;
            rumi_protocol_backend::icrc3_proof::verify_icrc3_transfer_block_with_fee(
                refund.token_ledger,
                block_index,
                Account {
                    owner: ic_cdk::id(),
                    subaccount: None,
                },
                Account {
                    owner: refund.user,
                    subaccount: None,
                },
                refund.amount,
                fee,
                Some(&memo),
                Some(timestamp),
            )
            .await
            .map_err(|reason| StabilityPoolError::LedgerTransferFailed {
                reason: format!(
                    "duplicate refund receipt did not verify; obligation remains held: {reason}"
                ),
            })?;
            log!(
                INFO,
                "claim_pending_refund: refund #{} Duplicate (block {}); previous attempt landed",
                refund_id,
                block_index
            );
            mutate_state(|s| {
                s.complete_pending_refund(refund_id);
            });
            Ok(refund.amount)
        }
        Ok((Err(TransferError::BadFee { expected_fee }),)) => {
            let corrected_fee: u64 = expected_fee.0.try_into().map_err(|_| {
                StabilityPoolError::LedgerTransferFailed {
                    reason: "ledger's corrected refund fee exceeds u64".into(),
                }
            })?;
            mutate_state(|s| {
                s.refresh_pending_refund_fee_after_bad_fee(refund_id, corrected_fee)
            })
            .map_err(|reason| StabilityPoolError::LedgerTransferFailed {
                reason: format!(
                    "ledger proved the prior refund had no effect, but corrected fee remains held: {reason}"
                ),
            })?;
            Err(StabilityPoolError::LedgerTransferFailed {
                reason: format!(
                    "ledger proved prior refund had no effect; corrected fee {corrected_fee} is journaled for an explicit retry"
                ),
            })
        }
        Ok((Err(TransferError::TooOld),)) => {
            mutate_state(|s| s.mark_pending_refund_too_old(refund_id))
                .map_err(|reason| StabilityPoolError::LedgerTransferFailed {
                    reason: format!("ledger returned TooOld but its refund identity could not be marked: {reason}"),
                })?;
            Err(StabilityPoolError::LedgerTransferFailed {
                reason: "the exact refund tuple was rejected as TooOld and remains held; reconcile a positive ledger receipt or complete the audited icUSD history scan before a new identity".into(),
            })
        }
        Ok((Err(transfer_error),)) => {
            log!(
                INFO,
                "claim_pending_refund: refund #{} transfer failed, re-inserting: {:?}",
                refund_id,
                transfer_error
            );
            let reason = format!("{:?}", transfer_error);
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
            Err(StabilityPoolError::InterCanisterCallFailed {
                target,
                method: "icrc1_transfer".to_string(),
            })
        }
    }
}

/// Reconcile a refund whose transfer reply was lost using one exact positive
/// ICRC-3 receipt. The original user or an admin may supply the block index;
/// no new transfer is sent by this endpoint.
pub async fn reconcile_pending_refund(
    refund_id: u64,
    block_index: u64,
) -> Result<u64, StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    let refund = read_state(|s| {
        s.pending_refunds
            .as_ref()
            .and_then(|refunds| refunds.get(&refund_id).cloned())
    })
    .ok_or(StabilityPoolError::RefundClaimNotFound)?;
    if caller != refund.user && !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    let _refund_guard = PendingRefundClaimGuard::new(refund_id)?;
    validate_pending_refund_journal(&refund)?;
    let block = rumi_protocol_backend::icrc3_proof::fetch_icrc3_block(
        refund.token_ledger,
        block_index,
    )
    .await
    .map_err(|reason| StabilityPoolError::LedgerTransferFailed {
        reason: format!("refund receipt history is incomplete; obligation remains held: {reason}"),
    })?;
    validate_pending_refund_block(&refund, &block).map_err(|reason| {
        StabilityPoolError::LedgerTransferFailed {
            reason: format!("supplied refund block is not the exact journaled payout: {reason}"),
        }
    })?;
    let completed = mutate_state(|state| {
        let Some(refunds) = state.pending_refunds.as_mut() else {
            return false;
        };
        if refunds.get(&refund_id) != Some(&refund) {
            return false;
        }
        refunds.remove(&refund_id).is_some()
    });
    if !completed {
        return Err(StabilityPoolError::LedgerTransferFailed {
            reason: "exact refund receipt verified but the pending refund changed; obligation remains held".into(),
        });
    }
    Ok(refund.amount)
}

/// Advance an archive-aware, bounded history scan after the configured icUSD
/// ledger returned typed TooOld for this refund's exact persisted tuple. A new
/// tuple is permitted only after the complete log prefix contains no match.
pub async fn reconcile_pending_refund_history(
    refund_id: u64,
) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    let refund = read_state(|s| {
        s.pending_refunds
            .as_ref()
            .and_then(|refunds| refunds.get(&refund_id).cloned())
    })
    .ok_or(StabilityPoolError::RefundClaimNotFound)?;
    if caller != refund.user && !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    let _refund_guard = PendingRefundClaimGuard::new(refund_id)?;
    validate_pending_refund_journal(&refund)?;
    if refund.transfer_too_old_rejected != Some(true) {
        return Err(StabilityPoolError::LedgerTransferFailed {
            reason: "a typed TooOld response for the exact transfer tuple is required before scanning".into(),
        });
    }
    if read_state(|s| s.icusd_ledger()) != Some(refund.token_ledger) {
        return Err(StabilityPoolError::LedgerTransferFailed {
            reason: "automatic identity rotation is restricted to the configured icUSD ledger; this ledger remains held for exact receipt evidence".into(),
        });
    }

    let refund = if refund.transfer_history_scan_cursor.is_none()
        && refund.transfer_history_scan_tip.is_none()
    {
        let log_length = rumi_protocol_backend::icrc3_proof::icrc3_log_length(refund.token_ledger)
            .await
            .map_err(|reason| StabilityPoolError::LedgerTransferFailed {
                reason: format!("could not establish the icUSD history tip; refund remains held: {reason}"),
            })?;
        mutate_state(|s| s.start_pending_refund_history_scan(refund_id, log_length))
            .map_err(|reason| StabilityPoolError::LedgerTransferFailed {
                reason: format!("could not persist refund history scan: {reason}"),
            })?
    } else {
        refund
    };
    let cursor = refund.transfer_history_scan_cursor.ok_or_else(|| {
        StabilityPoolError::LedgerTransferFailed {
            reason: "refund history scan cursor is missing; obligation remains held".into(),
        }
    })?;
    let tip = refund.transfer_history_scan_tip.ok_or_else(|| {
        StabilityPoolError::LedgerTransferFailed {
            reason: "refund history scan tip is missing; obligation remains held".into(),
        }
    })?;
    if cursor > tip {
        return Err(StabilityPoolError::LedgerTransferFailed {
            reason: "refund history scan cursor exceeds its fixed tip; obligation remains held".into(),
        });
    }
    let end = cursor
        .saturating_add(MAX_PENDING_REFUND_HISTORY_BLOCKS_PER_CALL)
        .min(tip);
    for index in cursor..end {
        let block = rumi_protocol_backend::icrc3_proof::fetch_icrc3_block(
            refund.token_ledger,
            index,
        )
        .await
        .map_err(|reason| StabilityPoolError::LedgerTransferFailed {
            reason: format!("icUSD history is incomplete at block {index}; refund remains held: {reason}"),
        })?;
        match validate_pending_refund_block(&refund, &block) {
            Ok(()) => {
                let completed = mutate_state(|state| {
                    let Some(refunds) = state.pending_refunds.as_mut() else {
                        return false;
                    };
                    if refunds.get(&refund_id) != Some(&refund) {
                        return false;
                    }
                    refunds.remove(&refund_id).is_some()
                });
                return if completed {
                    Ok(())
                } else {
                    Err(StabilityPoolError::LedgerTransferFailed {
                        reason: "exact refund receipt found but the pending row changed; obligation remains held".into(),
                    })
                };
            }
            Err(reason) if refund_block_identity_may_match(&refund, &block) => {
                return Err(StabilityPoolError::LedgerTransferFailed {
                    reason: format!("refund-like ledger block {index} conflicts with the exact fee identity: {reason}; obligation remains held"),
                });
            }
            Err(_) => {}
        }
    }
    if end < tip {
        mutate_state(|s| {
            s.advance_pending_refund_history_scan(refund_id, cursor, tip, end)
        })
        .map_err(|reason| StabilityPoolError::LedgerTransferFailed {
            reason: format!("could not persist refund history scan progress: {reason}"),
        })?;
        return Ok(());
    }
    mutate_state(|s| s.advance_pending_refund_history_scan(refund_id, cursor, tip, end))
        .map_err(|reason| StabilityPoolError::LedgerTransferFailed {
            reason: format!("could not persist complete refund history scan: {reason}"),
        })?;
    // Configuration may have changed while archive callbacks were awaited.
    // Recheck immediately before identity rotation so only the still-configured
    // audited icUSD ledger can authorize this no-effect path.
    if read_state(|s| s.icusd_ledger()) != Some(refund.token_ledger) {
        return Err(StabilityPoolError::LedgerTransferFailed {
            reason: "configured icUSD ledger changed during history scan; refund remains held for exact receipt evidence".into(),
        });
    }
    mutate_state(|s| s.rotate_pending_refund_after_no_effect(refund_id))
        .map_err(|reason| StabilityPoolError::LedgerTransferFailed {
            reason: format!("complete icUSD history proves no refund mint; retry identity remains held: {reason}"),
        })?;
    Ok(())
}

fn validate_pending_refund_journal(refund: &PendingRefund) -> Result<(), StabilityPoolError> {
    if refund.transfer_attempted != Some(true)
        || refund.transfer_created_at_time_ns.is_none()
        || refund.transfer_fee.is_none()
        || refund.transfer_memo.is_none()
        || refund.protocol_fee_reserved != refund.transfer_fee
    {
        return Err(StabilityPoolError::LedgerTransferFailed {
            reason: "pending refund has no complete exact transfer identity; legacy ambiguity remains held".into(),
        });
    }
    Ok(())
}

fn refund_block_identity_may_match(
    refund: &PendingRefund,
    block: &rumi_protocol_backend::icrc3_proof::DecodedBlock,
) -> bool {
    let source = Account {
        owner: ic_cdk::id(),
        subaccount: None,
    };
    let destination = Account {
        owner: refund.user,
        subaccount: None,
    };
    rumi_protocol_backend::icrc3_proof::validate_icrc3_transfer_block(
        block,
        Some(source),
        destination,
        refund.amount,
        refund.transfer_memo.as_deref(),
        refund.transfer_created_at_time_ns,
    )
    .is_ok()
}

fn validate_pending_refund_block(
    refund: &PendingRefund,
    block: &rumi_protocol_backend::icrc3_proof::DecodedBlock,
) -> Result<(), String> {
    validate_pending_refund_block_from(
        refund,
        block,
        Account {
            owner: ic_cdk::id(),
            subaccount: None,
        },
    )
}

fn validate_pending_refund_block_from(
    refund: &PendingRefund,
    block: &rumi_protocol_backend::icrc3_proof::DecodedBlock,
    source: Account,
) -> Result<(), String> {
    rumi_protocol_backend::icrc3_proof::validate_icrc3_direct_transfer_block(
        block,
        source,
        Account {
            owner: refund.user,
            subaccount: None,
        },
        refund.amount,
        refund.transfer_memo.as_deref(),
        refund.transfer_created_at_time_ns,
    )?;
    let expected_fee = refund
        .transfer_fee
        .ok_or_else(|| "refund fee is missing from its exact transfer journal".to_string())?;
    if block.fee != Some(expected_fee as u128) {
        return Err("refund block fee does not match the journaled source-paid fee".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rumi_protocol_backend::chains::config::ChainId;

    #[test]
    fn pending_refund_receipt_requires_exact_direct_full_principal_tuple() {
        let user = principal(44);
        let memo = b"rumi-sp-refund-v1:exact".to_vec();
        let refund = PendingRefund {
            id: 9,
            user,
            token_ledger: principal(10),
            amount: 1_000_000,
            reason: "failed deposit".into(),
            created_at: 7,
            transfer_attempted: Some(true),
            transfer_created_at_time_ns: Some(123),
            transfer_fee: Some(10_000),
            transfer_memo: Some(memo.clone()),
            transfer_attempt_no: Some(0),
            transfer_too_old_rejected: Some(true),
            transfer_history_scan_cursor: None,
            transfer_history_scan_tip: None,
            protocol_fee_reserved: Some(10_000),
        };
        let exact = rumi_protocol_backend::icrc3_proof::DecodedBlock {
            btype: Some("1xfer".into()),
            op: "xfer".into(),
            from: Some(Account { owner: Principal::anonymous(), subaccount: None }),
            to: Some(Account { owner: user, subaccount: None }),
            spender: None,
            amount: refund.amount as u128,
            transaction_fee: None,
            fee: Some(10_000),
            memo: Some(memo),
            created_at_time: Some(123),
        };
        let source = Account { owner: Principal::anonymous(), subaccount: None };
        let mut exact = exact;
        exact.from = Some(source.clone());
        assert!(validate_pending_refund_block_from(&refund, &exact, source.clone()).is_ok());

        let mut wrong_fee = exact.clone();
        wrong_fee.fee = Some(9_999);
        assert!(validate_pending_refund_block_from(&refund, &wrong_fee, source.clone()).is_err());
        let mut transfer_from = exact;
        transfer_from.btype = Some("2xfer".into());
        transfer_from.spender = Some(Account { owner: principal(55), subaccount: None });
        assert!(validate_pending_refund_block_from(&refund, &transfer_from, source).is_err());
    }

    fn principal(byte: u8) -> Principal {
        Principal::from_slice(&[byte])
    }

    #[test]
    fn public_3usd_conversion_fails_before_entering_legacy_pull_path() {
        let mut operation_entered = false;
        let result = require_receipt_backed_3usd_conversion(|| {
            operation_entered = true;
        });

        assert!(matches!(
            result,
            Err(StabilityPoolError::InterCanisterCallFailed { method, target })
                if method.contains("receipt-backed conversion unavailable")
                    && method.contains("no input tokens were pulled")
                    && target.contains("3pool")
        ));
        assert!(
            !operation_entered,
            "legacy conversion closure must not run while 3pool lacks durable receipts"
        );

        let public_result = futures::executor::block_on(deposit_as_3usd(principal(10), 100));
        assert!(matches!(
            public_result,
            Err(StabilityPoolError::InterCanisterCallFailed { method, .. })
                if method.contains("receipt-backed conversion unavailable")
        ));
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
            burn_attempted: Some(true),
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
    fn post_transfer_system_busy_keeps_receipt_for_local_retry() {
        crate::state::replace_state(crate::state::StabilityPoolState::default());
        mutate_state(|s| {
            s.deposits.insert(principal(1), DepositPosition::new(0));
            s.begin_deposit_intent(principal(1), principal(10), 50_00000000, 1)
                .unwrap();
            assert!(s.record_deposit_receipt(principal(1), principal(10), 50_00000000, 1, 9,));
            s.put_pending_chain_absorb(pending_intent()).unwrap();
        });
        let result = complete_deposit_credit_after_async_at(
            principal(1),
            principal(10),
            50_00000000,
            1,
            2,
        );

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
        assert_eq!(
            read_state(|s| s
                .pending_deposit_intents
                .as_ref()
                .and_then(|intents| intents.get(&principal(1)))
                .and_then(|intent| intent.transfer_block_index)),
            Some(9),
            "verified ledger receipt must survive the post-transfer guard",
        );
        mutate_state(|s| {
            s.take_pending_chain_absorb(77).expect("remove test guard");
        });
        complete_deposit_credit_after_async_at(
            principal(1),
            principal(10),
            50_00000000,
            1,
            3,
        )
            .expect("retry finalizes from the saved receipt");
        assert_eq!(
            read_state(|s| s.total_stablecoin_balances.get(&principal(10)).copied()),
            Some(50_00000000),
        );
        assert!(read_state(|s| s
            .pending_deposit_intents
            .as_ref()
            .map_or(true, |intents| intents.is_empty())));
        crate::state::replace_state(crate::state::StabilityPoolState::default());
    }

    #[test]
    fn saved_receipt_completes_even_after_deposit_policy_changes() {
        crate::state::replace_state(crate::state::StabilityPoolState::default());
        mutate_state(|s| {
            let user = principal(1);
            let ledger = principal(10);
            s.deposits.insert(user, DepositPosition::new(0));
            s.stablecoin_registry.insert(
                ledger,
                StablecoinConfig {
                    ledger_id: ledger,
                    symbol: "TEST".to_string(),
                    decimals: 8,
                    priority: 1,
                    is_active: false,
                    transfer_fee: None,
                    is_lp_token: None,
                    underlying_pool: None,
                },
            );
            s.configuration.min_deposit_e8s = 90_00000000;
            s.configuration.emergency_pause = true;
            s.begin_deposit_intent(user, ledger, 50_00000000, 1)
                .unwrap();
            assert!(s.record_deposit_receipt(user, ledger, 50_00000000, 1, 9));
        });

        assert!(matches!(
            complete_saved_deposit_receipt_before_admission(
                principal(1),
                principal(10),
                50_00000000,
                2,
            ),
            Ok(Some(()))
        ), "a proven transfer must complete despite changed admission policy");
        assert_eq!(
            read_state(|s| s.total_stablecoin_balances.get(&principal(10)).copied()),
            Some(50_00000000),
        );
        assert!(read_state(|s| s
            .pending_deposit_intents
            .as_ref()
            .map_or(true, |intents| intents.is_empty())));
        crate::state::replace_state(crate::state::StabilityPoolState::default());
    }

    #[test]
    fn blocked_admission_with_unreceipted_intent_requires_reconciliation() {
        let error = pending_deposit_admission_error(
            StabilityPoolError::EmergencyPaused,
            true,
        );
        assert!(matches!(
            error,
            StabilityPoolError::LedgerTransferFailed { reason }
                if reason.contains("exact ledger reconciliation required")
        ));
        assert!(matches!(
            pending_deposit_admission_error(StabilityPoolError::EmergencyPaused, false),
            StabilityPoolError::EmergencyPaused
        ));
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
