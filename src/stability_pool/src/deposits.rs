use crate::logs::INFO;
use crate::state::{mutate_state, read_state};
use crate::types::*;
use candid::{Nat, Principal};
use icrc_ledger_types::icrc::generic_value::ICRC3Value;
use ic_canister_log::log;
use ic_cdk::call;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
use icrc_ledger_types::icrc3::blocks::{GetBlocksRequest, GetBlocksResult};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use num_traits::ToPrimitive;

/// Conservative fallback for a stablecoin ledger's transfer fee (native units),
/// used when the live `icrc1_fee` query fails. Known stablecoin registry values
/// are normalized on registration/upgrade; for unknown ledgers, erring high
/// keeps the pool solvent rather than risking an over-send.
const DEFAULT_LEDGER_FEE: u64 = 10_000;

thread_local! {
    /// Per-ledger transfer-fee cache for transfer/refund math, populated lazily from
    /// `icrc1_fee`. Heap-only (not persisted), so it is simply re-warmed after
    /// an upgrade. Mirrors rumi_3pool::transfers::LEDGER_FEES.
    static LEDGER_FEES: RefCell<HashMap<Principal, u64>> = RefCell::new(HashMap::new());
    /// Prevent concurrent claims for one stable refund while its callback is
    /// outstanding. The stable journal remains authoritative after traps.
    static ACTIVE_PENDING_REFUND_CLAIMS: RefCell<BTreeSet<u64>> = RefCell::new(BTreeSet::new());
}

const MAX_PENDING_REFUND_HISTORY_BLOCKS_PER_CLAIM: u64 = 8;
const PENDING_REFUND_MEMO_PREFIX: &[u8; 8] = b"RSPRFND:";

struct PendingRefundClaimGuard(u64);

impl PendingRefundClaimGuard {
    fn reserve(refund_id: u64) -> Result<Self, StabilityPoolError> {
        ACTIVE_PENDING_REFUND_CLAIMS.with(|active| {
            if active.borrow_mut().insert(refund_id) {
                Ok(Self(refund_id))
            } else {
                Err(StabilityPoolError::SystemBusy)
            }
        })
    }
}

impl Drop for PendingRefundClaimGuard {
    fn drop(&mut self) {
        ACTIVE_PENDING_REFUND_CLAIMS.with(|active| {
            active.borrow_mut().remove(&self.0);
        });
    }
}

pub(crate) fn record_deposit_credit_after_async(
    caller: Principal,
    token_ledger: Principal,
    amount: u64,
) -> Result<(), StabilityPoolError> {
    crate::ensure_pool_token_balance_mutation_allowed(&[token_ledger])?;
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
    crate::ensure_pool_token_balance_mutation_allowed(&[token_ledger, three_usd_ledger])?;
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

/// Read the current fee without using the long-lived display/accounting cache.
/// Value-moving calls use the result as an explicit `fee` so a fee change can
/// cause a typed `BadFee` before transfer instead of an untracked debit.
async fn current_ledger_transfer_fee(ledger: Principal) -> Result<u64, StabilityPoolError> {
    let (fee,): (candid::Nat,) = call(ledger, "icrc1_fee", ()).await.map_err(|_| {
        StabilityPoolError::InterCanisterCallFailed {
            target: ledger.to_string(),
            method: "icrc1_fee".to_string(),
        }
    })?;
    fee.0
        .try_into()
        .map_err(|_| StabilityPoolError::LedgerTransferFailed {
            reason: "icrc1_fee does not fit u64".to_string(),
        })
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

pub(crate) fn prepare_withdrawal_after_ledger_check(
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
    if crate::pool_token_balance_mutation_blocked(&[token_ledger]) {
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

    // Give each fresh pull a durable unique ICRC-2 dedup identity.
    let transfer_created_at_time =
        mutate_state(|s| s.reserve_deposit_transfer_timestamp(ic_cdk::api::time()))
            .map_err(|_| StabilityPoolError::SystemBusy)?;

    // Every successful pull must hold a slot in case a later balance check
    // requires compensation. Reserve before the first await so concurrent
    // deposits cannot all assume the same final queue slot.
    let pending_refund_slot = crate::pool_guard::PendingRefundSlotGuard::reserve()?;

    // ICRC-2 transfer_from: pull tokens from user to pool canister
    let _balance_async_guard = crate::pool_guard::PoolBalanceAsyncGuard::new();
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
        created_at_time: Some(transfer_created_at_time),
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
                    pending_refund_slot,
                )
                .await;
                return Err(error);
            }
            log!(INFO, "Deposit recorded for {}", caller);
            Ok(())
        }
        // This fresh attempt has a unique timestamp; Duplicate does not prove
        // this request's pull landed and cannot authorize credit.
        Ok((Err(TransferFromError::Duplicate { duplicate_of }),)) => {
            log!(
                INFO,
                "Deposit transfer Duplicate (block {}); fresh attempt not credited",
                duplicate_of
            );
            Err(StabilityPoolError::LedgerTransferFailed {
                reason: format!("Duplicate {{ duplicate_of: {} }}", duplicate_of),
            })
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
    if crate::pool_token_balance_mutation_blocked(&[token_ledger]) {
        return Err(StabilityPoolError::SystemBusy);
    }
    let caller = ic_cdk::api::caller();

    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    // The ledger debits `transfer_amount + fee` from the pool. Query the live
    // fee first so a max withdrawal drains the user's recorded position without
    // overdrawing the pool ledger account.
    let ledger_fee = ledger_transfer_fee(token_ledger).await;
    let pool_ledger_balance = ledger_pool_balance(token_ledger).await;

    // Fee/balance queries above await other canisters. Recheck after them so a
    // burn intent created during those calls cannot race this withdrawal.
    crate::ensure_pool_token_balance_mutation_allowed(&[token_ledger])?;

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

fn outbound_payout_memo(caller: Principal, ledger: Principal, created_at_time_ns: u64) -> Vec<u8> {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"rumi-sp-collateral-claim-v1\0");
    hasher.update((caller.as_slice().len() as u64).to_be_bytes());
    hasher.update(caller.as_slice());
    hasher.update((ledger.as_slice().len() as u64).to_be_bytes());
    hasher.update(ledger.as_slice());
    hasher.update(created_at_time_ns.to_be_bytes());
    hasher.finalize().to_vec()
}

fn outbound_transfer_args(payout: &PendingOutboundPayout) -> TransferArg {
    TransferArg {
        from_subaccount: payout.from_subaccount.clone(),
        to: payout.recipient,
        amount: payout.transfer_amount.into(),
        fee: Some(payout.transfer_fee.into()),
        memo: Some(payout.transfer_memo.clone().into()),
        created_at_time: Some(payout.transfer_created_at_time_ns),
    }
}

fn recover_stale_outbound_dispatch(caller: Principal, ledger: Principal) {
    let pending = read_state(|s| s.pending_outbound_payout(&caller, &ledger));
    if let Some(payout) = pending.filter(|p| p.dispatch_in_flight) {
        if !crate::pool_guard::balance_async_in_flight() {
            mutate_state(|s| {
                s.mark_outbound_payout_ambiguous(
                    caller,
                    ledger,
                    &payout,
                    "dispatch continuation ended without a recorded result".into(),
                )
            });
        }
    }
}

fn icrc3_nat(value: &ICRC3Value, name: &str) -> Result<u128, String> {
    match value {
        ICRC3Value::Nat(value) => value
            .0
            .to_u128()
            .ok_or_else(|| format!("ICRC-3 {name} exceeds u128")),
        _ => Err(format!("ICRC-3 {name} is not Nat")),
    }
}

fn icrc3_account(value: &ICRC3Value, name: &str) -> Result<Account, String> {
    let parts = match value {
        ICRC3Value::Array(parts) if (1..=2).contains(&parts.len()) => parts,
        _ => return Err(format!("ICRC-3 {name} account has malformed shape")),
    };
    let owner = match &parts[0] {
        ICRC3Value::Blob(bytes) => Principal::try_from_slice(bytes)
            .map_err(|_| format!("ICRC-3 {name} owner is malformed"))?,
        _ => return Err(format!("ICRC-3 {name} owner is not a blob")),
    };
    let subaccount = parts
        .get(1)
        .map(|value| match value {
            ICRC3Value::Blob(bytes) if bytes.len() == 32 => {
                let mut subaccount = [0; 32];
                subaccount.copy_from_slice(bytes);
                Ok(subaccount)
            }
            _ => Err(format!("ICRC-3 {name} subaccount is malformed")),
        })
        .transpose()?;
    Ok(Account { owner, subaccount })
}

async fn verify_collateral_payout_block(
    ledger: Principal,
    block_index: u64,
    payout: &PendingOutboundPayout,
) -> Result<(), String> {
    if rumi_protocol_backend::native_icp_proof::is_native_icp_ledger(ledger) {
        // The canonical native ICP ledger lacks ICRC-3. Its advertised
        // query_blocks archive callback is trusted as part of that configured
        // ledger's interface, but this is not a cryptographic archive-membership
        // proof. The returned block is still checked against every saved field.
        let block =
            rumi_protocol_backend::native_icp_proof::query_block(ledger, block_index).await?;
        let fee = rumi_protocol_backend::native_icp_proof::verify_transfer_block(
            &block,
            ic_cdk::id(),
            payout.recipient.owner,
            payout.transfer_amount,
            &payout.transfer_memo,
            payout.transfer_created_at_time_ns,
        )?;
        if fee != payout.transfer_fee {
            return Err(
                "native ICP candidate fee differs from the exact pinned fee; payout remains held"
                    .into(),
            );
        }
        return Ok(());
    }
    let request = vec![GetBlocksRequest {
        start: Nat::from(block_index),
        length: Nat::from(1u64),
    }];
    let (response,): (GetBlocksResult,) = call(ledger, "icrc3_get_blocks", (request,))
        .await
        .map_err(|(code, message)| format!("direct icrc3_get_blocks failed: {code:?} {message}"))?;
    let log_length = response
        .log_length
        .0
        .to_u64()
        .ok_or_else(|| "ICRC-3 log length exceeds u64; payout remains held".to_string())?;
    if log_length <= block_index {
        return Err(
            "ICRC-3 log length does not include candidate block; payout remains held".into(),
        );
    }
    if !response.archived_blocks.is_empty() {
        return Err("archive-backed ICRC-3 response is unsupported; payout remains held".into());
    }
    if response.blocks.len() != 1 || response.blocks[0].id.0.to_u64() != Some(block_index) {
        return Err(
            "ledger did not directly return the exact candidate block; payout remains held".into(),
        );
    }
    let block = match &response.blocks[0].block {
        ICRC3Value::Map(map) => map,
        _ => return Err("ICRC-3 block is not a map; payout remains held".into()),
    };
    block
        .get("ts")
        .ok_or_else(|| "ICRC-3 block has no timestamp".to_string())
        .and_then(|value| icrc3_nat(value, "block timestamp"))?;
    let tx = match block.get("tx") {
        Some(ICRC3Value::Map(map)) => map,
        _ => return Err("ICRC-3 block has no transaction map; payout remains held".into()),
    };
    if let Some(btype) = block.get("btype") {
        if btype != &ICRC3Value::Text("1xfer".into()) {
            return Err("candidate block is not an ICRC-1 transfer; payout remains held".into());
        }
    }
    if tx.get("op") != Some(&ICRC3Value::Text("xfer".into())) {
        return Err("candidate block operation is not xfer; payout remains held".into());
    }
    if tx.contains_key("spender") {
        return Err("candidate block is a delegated transfer; payout remains held".into());
    }
    let from = tx
        .get("from")
        .ok_or_else(|| "ICRC-3 transfer has no source account".to_string())
        .and_then(|value| icrc3_account(value, "source"))?;
    let to = tx
        .get("to")
        .ok_or_else(|| "ICRC-3 transfer has no destination account".to_string())
        .and_then(|value| icrc3_account(value, "destination"))?;
    let amount = tx
        .get("amt")
        .ok_or_else(|| "ICRC-3 transfer has no amount".to_string())
        .and_then(|value| icrc3_nat(value, "amount"))?;
    let tx_fee = tx
        .get("fee")
        .map(|value| icrc3_nat(value, "fee"))
        .transpose()?;
    let block_fee = block
        .get("fee")
        .map(|value| icrc3_nat(value, "block fee"))
        .transpose()?;
    if tx_fee
        .zip(block_fee)
        .is_some_and(|(tx_fee, block_fee)| tx_fee != block_fee)
    {
        return Err("ICRC-3 block has conflicting fee fields".into());
    }
    let fee = tx_fee.or(block_fee);
    let memo = match tx.get("memo") {
        Some(ICRC3Value::Blob(bytes)) => bytes.to_vec(),
        _ => return Err("ICRC-3 transfer memo is missing or malformed".into()),
    };
    let ts = tx
        .get("ts")
        .map(|value| icrc3_nat(value, "timestamp"))
        .transpose()?;
    let named_ts = tx
        .get("created_at_time")
        .map(|value| icrc3_nat(value, "created_at_time"))
        .transpose()?;
    if ts.zip(named_ts).is_some_and(|(ts, named)| ts != named) {
        return Err("ICRC-3 block has conflicting transfer timestamps".into());
    }
    let created_at_time = ts.or(named_ts).and_then(|value| u64::try_from(value).ok());
    let expected_from = Account {
        owner: ic_cdk::id(),
        subaccount: payout.from_subaccount,
    };
    if from != expected_from
        || to != payout.recipient
        || amount != u128::from(payout.transfer_amount)
        || fee != Some(u128::from(payout.transfer_fee))
        || memo != payout.transfer_memo
        || created_at_time != Some(payout.transfer_created_at_time_ns)
    {
        return Err(
            "candidate block does not prove the exact persisted payout tuple; payout remains held"
                .into(),
        );
    }
    Ok(())
}

/// Confirm an aged ambiguous claim only from an exact, directly served ICRC-3
/// block. Missing, archived, malformed, or partial proof leaves the row held.
pub async fn reconcile_collateral_claim(
    collateral_ledger: Principal,
    claim_owner: Principal,
    block_index: u64,
) -> Result<u64, StabilityPoolError> {
    if crate::pool_balance_mutation_blocked() {
        return Err(StabilityPoolError::SystemBusy);
    }
    let caller = ic_cdk::api::caller();
    let authorized = caller == claim_owner || read_state(|state| state.is_admin(&caller));
    if !authorized {
        return Err(StabilityPoolError::Unauthorized);
    }
    let payout =
        read_state(|state| state.pending_outbound_payout(&claim_owner, &collateral_ledger))
            .ok_or(StabilityPoolError::SystemBusy)?;
    if !payout.ambiguous_seen || payout.dispatch_in_flight {
        return Err(StabilityPoolError::SystemBusy);
    }
    if payout
        .candidate_block_index
        .is_some_and(|candidate| candidate != block_index)
        || (payout.candidate_block_index_raw.is_some() && payout.candidate_block_index.is_none())
    {
        return Err(StabilityPoolError::LedgerTransferFailed {
            reason: "candidate block conflicts with the ledger-reported payout index; payout remains held".into(),
        });
    }
    let _balance_async_guard = crate::pool_guard::PoolBalanceAsyncGuard::new();
    let reserved = mutate_state(|state| {
        state.begin_outbound_payout_reconciliation(
            claim_owner,
            collateral_ledger,
            ic_cdk::api::time(),
        )
    })?;
    let proof = verify_collateral_payout_block(collateral_ledger, block_index, &reserved).await;
    if let Err(reason) = proof {
        mutate_state(|state| {
            state.mark_outbound_payout_ambiguous(
                claim_owner,
                collateral_ledger,
                &reserved,
                reason.clone(),
            )
        });
        return Err(StabilityPoolError::LedgerTransferFailed { reason });
    }
    let completed = mutate_state(|state| {
        state.complete_outbound_payout(
            claim_owner,
            collateral_ledger,
            &reserved,
            block_index,
            ic_cdk::api::time(),
        )
    });
    if !completed {
        return Err(StabilityPoolError::LedgerTransferFailed {
            reason: "exact ledger proof returned but payout row changed; claim remains held".into(),
        });
    }
    Ok(reserved.transfer_amount)
}

async fn dispatch_outbound_payout(
    caller: Principal,
    ledger: Principal,
    payout: PendingOutboundPayout,
) -> Result<(), StabilityPoolError> {
    let result: Result<(Result<candid::Nat, TransferError>,), _> =
        call(ledger, "icrc1_transfer", (outbound_transfer_args(&payout),)).await;
    match result {
        Ok((Ok(block),)) => {
            let block_index: u64 = match block.0.clone().try_into() {
                Ok(index) => index,
                Err(_) => {
                    let raw = block.0.to_string();
                    mutate_state(|s| {
                        s.record_outbound_payout_candidate(
                            caller,
                            ledger,
                            &payout,
                            None,
                            raw.clone(),
                        );
                        s.mark_outbound_payout_ambiguous(
                            caller, ledger, &payout,
                            "successful transfer block index exceeds u64; exact payout remains held".into(),
                        )
                    });
                    return Err(StabilityPoolError::LedgerTransferFailed {
                        reason: format!("successful transfer block index {raw} exceeds supported range; exact payout remains held"),
                    });
                }
            };
            let completed = mutate_state(|s| {
                s.complete_outbound_payout(
                    caller,
                    ledger,
                    &payout,
                    block_index,
                    ic_cdk::api::time(),
                )
            });
            if completed {
                Ok(())
            } else {
                Err(StabilityPoolError::SystemBusy)
            }
        }
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            let raw_candidate = duplicate_of.0.to_string();
            let parsed_candidate = duplicate_of.0.to_u64();
            if !mutate_state(|s| {
                s.record_outbound_payout_candidate(
                    caller,
                    ledger,
                    &payout,
                    parsed_candidate,
                    raw_candidate.clone(),
                )
            }) {
                mutate_state(|s| {
                    s.mark_outbound_payout_ambiguous(
                        caller,
                        ledger,
                        &payout,
                        "duplicate candidate could not be attached to the exact pending tuple"
                            .into(),
                    )
                });
                return Err(StabilityPoolError::LedgerTransferFailed {
                    reason: "duplicate candidate could not be attached; payout remains held".into(),
                });
            }
            let block_index: u64 = match duplicate_of.0.try_into() {
                Ok(index) => index,
                Err(_) => {
                    mutate_state(|s| {
                        s.mark_outbound_payout_ambiguous(
                            caller,
                            ledger,
                            &payout,
                            "duplicate block index exceeds u64; exact payout remains held".into(),
                        )
                    });
                    return Err(StabilityPoolError::LedgerTransferFailed {
                        reason: "duplicate block index exceeds supported range; exact payout remains held".into(),
                    });
                }
            };
            if let Err(reason) = verify_collateral_payout_block(ledger, block_index, &payout).await
            {
                mutate_state(|s| {
                    s.mark_outbound_payout_ambiguous(caller, ledger, &payout, reason.clone())
                });
                return Err(StabilityPoolError::LedgerTransferFailed { reason });
            }
            let completed = mutate_state(|s| {
                s.complete_outbound_payout(
                    caller,
                    ledger,
                    &payout,
                    block_index,
                    ic_cdk::api::time(),
                )
            });
            if completed {
                Ok(())
            } else {
                Err(StabilityPoolError::SystemBusy)
            }
        }
        Ok((Err(error),))
            if matches!(
                &error,
                TransferError::BadFee { .. }
                    | TransferError::BadBurn { .. }
                    | TransferError::InsufficientFunds { .. }
                    | TransferError::CreatedInFuture { .. }
                    | TransferError::TooOld
            ) =>
        {
            let restored = mutate_state(|s| {
                s.reject_outbound_payout_without_effect(
                    caller,
                    ledger,
                    &payout,
                    format!("{:?}", error),
                    ic_cdk::api::time(),
                )
            })?;
            if restored {
                Err(StabilityPoolError::LedgerTransferFailed {
                    reason: format!("{:?}", error),
                })
            } else {
                Err(StabilityPoolError::LedgerTransferFailed {
                    reason:
                        "payout remains held because an earlier attempt had an ambiguous outcome"
                            .into(),
                })
            }
        }
        Ok((Err(error),)) => {
            mutate_state(|s| {
                s.mark_outbound_payout_ambiguous(caller, ledger, &payout, format!("{:?}", error))
            });
            Err(StabilityPoolError::LedgerTransferFailed {
                reason: "outbound payout outcome unresolved; exact transfer tuple retained".into(),
            })
        }
        Err(error) => {
            mutate_state(|s| {
                s.mark_outbound_payout_ambiguous(caller, ledger, &payout, format!("{:?}", error))
            });
            Err(StabilityPoolError::InterCanisterCallFailed {
                target: format!("{}", ledger),
                method: "icrc1_transfer; exact payout tuple retained for retry".into(),
            })
        }
    }
}

/// Claim collateral gains for a single collateral type.
///
/// The exact ICRC-1 tuple is persisted with the gain debit before dispatch.
/// Any transport or ledger outcome that does not prove success or no effect
/// leaves the debit and tuple held for an identical retry.
pub async fn claim_collateral(collateral_ledger: Principal) -> Result<u64, StabilityPoolError> {
    if crate::pool_balance_mutation_blocked() {
        return Err(StabilityPoolError::SystemBusy);
    }
    let caller = ic_cdk::api::caller();
    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }
    read_state(|s| s.ensure_icrc_claimable_collateral(&collateral_ledger))?;

    recover_stale_outbound_dispatch(caller, collateral_ledger);
    let _balance_async_guard = crate::pool_guard::PoolBalanceAsyncGuard::new();

    if let Some(existing) = read_state(|s| s.pending_outbound_payout(&caller, &collateral_ledger)) {
        let payout = mutate_state(|s| s.begin_outbound_payout_retry(caller, collateral_ledger))?;
        dispatch_outbound_payout(caller, collateral_ledger, payout).await?;
        return Ok(existing.transfer_amount);
    }

    let gains = read_state(|s| {
        s.deposits
            .get(&caller)
            .and_then(|position| position.collateral_gains.get(&collateral_ledger).copied())
            .unwrap_or(0)
    });
    if gains == 0 {
        return Ok(0);
    }

    // Never guess a collateral fee: a stale 10,000-unit fallback is unsafe for
    // ledgers such as BOB whose fee is 1,000,000 units.
    let (fee_nat,): (candid::Nat,) =
        call(collateral_ledger, "icrc1_fee", ())
            .await
            .map_err(|error| StabilityPoolError::InterCanisterCallFailed {
                target: format!("{}", collateral_ledger),
                method: format!("icrc1_fee: {:?}; claim gains unchanged", error),
            })?;
    let ledger_fee: u64 =
        fee_nat
            .0
            .try_into()
            .map_err(|_| StabilityPoolError::LedgerTransferFailed {
                reason: "collateral ledger fee exceeds supported u64 range; claim gains unchanged"
                    .into(),
            })?;
    if gains <= ledger_fee {
        return Ok(0);
    }

    let created_at_time_ns =
        mutate_state(|s| s.allocate_outbound_payout_timestamp(ic_cdk::api::time()))?;
    let memo = outbound_payout_memo(caller, collateral_ledger, created_at_time_ns);
    let recipient = Account {
        owner: caller,
        subaccount: None,
    };
    let payout = mutate_state(|s| {
        s.prepare_collateral_payout(
            caller,
            collateral_ledger,
            ledger_fee,
            recipient,
            created_at_time_ns,
            memo,
        )
    })?
    .ok_or(StabilityPoolError::SystemBusy)?;
    let transfer_amount = payout.transfer_amount;
    log!(
        INFO,
        "Claim: {} of collateral {} (transfer {} - fee {}) by {}",
        gains,
        collateral_ledger,
        transfer_amount,
        ledger_fee,
        caller
    );
    dispatch_outbound_payout(caller, collateral_ledger, payout).await?;
    Ok(transfer_amount)
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

    let claims_by_ledger = read_state(|state| {
        let mut claims: BTreeMap<Principal, u64> = state
            .get_claimable_icrc_collateral_gains(&caller)
            .into_iter()
            .filter(|(_, amount)| *amount > 0)
            .collect();
        if let Some(pending) = state.pending_outbound_payouts.as_ref() {
            for ((owner, ledger), _) in pending {
                if *owner == caller {
                    claims.entry(*ledger).or_insert(0);
                }
            }
        }
        claims
    });

    if claims_by_ledger.is_empty() {
        return Ok(BTreeMap::new());
    }

    let mut claimed = BTreeMap::new();
    for (collateral_ledger, amount) in &claims_by_ledger {
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
/// Convenience conversion is temporarily fail-closed until a durable conversion
/// saga can recover every pull, approval, 3pool, allowance-revocation, and refund
/// outcome. The current Candid contract has no caller-funded fee budget or
/// persistent phase journal, so even a zero-fee ledger is unsafe after an
/// ambiguous inter-canister outcome. This is containment, not restoration of
/// the convenience feature.
pub async fn deposit_as_3usd(
    _token_ledger: Principal,
    _amount: u64,
) -> Result<u64, StabilityPoolError> {
    Err(StabilityPoolError::SystemBusy)
}

/// Refund the pulled tokens to the user after a failed deposit_as_3usd.
///
/// IC-S-001: the refund sends `amount` NET of the ledger transfer fee so the
/// pool's ledger balance drops by exactly `amount` (a gross refund cost
/// amount+fee, drifting the pool one fee below its tracked deposits per
/// refund). The liability is journaled before any fee/history/transfer await;
/// its exact tuple is persisted before dispatch and verified against ICRC-3
/// before the row is removed. Unsent rows retain an initialization marker,
/// while legacy attemptless rows fail closed for manual reconciliation.
async fn refund_user(
    user: Principal,
    token_ledger: Principal,
    amount: u64,
    reason: &str,
    pending_refund_slot: crate::pool_guard::PendingRefundSlotGuard,
) {
    // Journal the liability before any await that could later lead to a
    // compensating transfer. The initialization marker proves this row has
    // never been dispatched if fee discovery or history lookup fails.
    let refund = record_pending_refund(user, token_ledger, amount, reason, pending_refund_slot);
    let _claim_guard = match PendingRefundClaimGuard::reserve(refund.id) {
        Ok(guard) => guard,
        Err(error) => {
            log!(
                INFO,
                "refund_user: refund #{} remains durably queued before payout setup: {:?}",
                refund.id,
                error
            );
            return;
        }
    };

    let fee = match current_ledger_transfer_fee(token_ledger).await {
        Ok(fee) => fee,
        Err(error) => {
            log!(
                INFO,
                "refund_user: refund #{} remains queued; live fee unavailable: {:?}",
                refund.id,
                error
            );
            return;
        }
    };
    if amount <= fee {
        // Keep the never-dispatched marker so a later lower fee can initialize
        // a tuple without confusing this row with a legacy ambiguous payout.
        log!(
            INFO,
            "refund_user: refund #{} remains queued; amount {} is not above fee {}",
            refund.id,
            amount,
            fee
        );
        return;
    }

    let history_start = match pending_refund_history_tip(token_ledger).await {
        Ok(tip) => tip,
        Err(error) => {
            log!(
                INFO,
                "refund_user: refund #{} remains queued; ledger history boundary unavailable: {:?}",
                refund.id,
                error
            );
            return;
        }
    };
    let mut attempt = match build_pending_refund_attempt(&refund, fee, history_start, None) {
        Ok(attempt) => attempt,
        Err(error) => {
            log!(
                INFO,
                "refund_user: refund #{} remains queued; payout tuple unavailable: {:?}",
                refund.id,
                error
            );
            return;
        }
    };
    attempt.dispatch_started = true;
    if !mutate_state(|state| {
        if !state.pending_refund_attempt_initializable(refund.id) {
            return false;
        }
        state.put_pending_refund_attempt(attempt.clone());
        true
    }) {
        log!(
            INFO,
            "refund_user: refund #{} remains queued; durable payout intent could not be installed",
            refund.id
        );
        return;
    }

    let transfer_args = TransferArg {
        from_subaccount: attempt.from.subaccount,
        to: attempt.to.clone(),
        amount: attempt.amount.into(),
        fee: Some(attempt.fee.into()),
        memo: Some(attempt.memo.clone().into()),
        created_at_time: Some(attempt.created_at_time_ns.into()),
    };
    let result: Result<(Result<candid::Nat, TransferError>,), _> =
        call(attempt.token_ledger, "icrc1_transfer", (transfer_args,)).await;
    match result {
        Ok((Ok(block),)) => {
            let block_index: u64 = match block.0.try_into() {
                Ok(index) => index,
                Err(_) => {
                    hold_pending_refund_attempt(
                        &attempt,
                        "refund receipt block index exceeds u64; obligation remains held"
                            .to_string(),
                    );
                    return;
                }
            };
            attempt.expected_block_index = Some(block_index);
            mutate_state(|state| state.update_pending_refund_attempt(attempt.clone()));
            if let Err(error) = verify_pending_refund_attempt(&refund, &attempt).await {
                log!(
                    INFO,
                    "refund_user: refund #{} payout remains held after receipt verification: {:?}",
                    refund.id,
                    error
                );
            }
        }
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            let block_index: u64 = match duplicate_of.0.try_into() {
                Ok(index) => index,
                Err(_) => {
                    hold_pending_refund_attempt(
                        &attempt,
                        "duplicate refund block index exceeds u64; obligation remains held"
                            .to_string(),
                    );
                    return;
                }
            };
            attempt.expected_block_index = Some(block_index);
            mutate_state(|state| state.update_pending_refund_attempt(attempt.clone()));
            if let Err(error) = verify_pending_refund_attempt(&refund, &attempt).await {
                log!(INFO, "refund_user: duplicate refund #{} remains held after receipt verification: {:?}", refund.id, error);
            }
        }
        Ok((Err(transfer_error),)) => {
            hold_pending_refund_attempt(
                &attempt,
                format!("{reason}; refund transfer returned unresolved error: {transfer_error:?}"),
            );
        }
        Err(call_error) => {
            hold_pending_refund_attempt(
                &attempt,
                format!("{reason}; refund call outcome is ambiguous: {call_error:?}"),
            );
        }
    }
}

fn record_pending_refund(
    user: Principal,
    token_ledger: Principal,
    amount: u64,
    reason: &str,
    _slot: crate::pool_guard::PendingRefundSlotGuard,
) -> PendingRefund {
    let created_at = ic_cdk::api::time();
    let result = mutate_state(|s| {
        s.record_pending_refund(user, token_ledger, amount, reason.to_string(), created_at)
    });
    let id = result;
    log!(
        INFO,
        "refund_user: refund of {} {} to {} was journaled as pending #{} ({})",
        amount,
        token_ledger,
        user,
        id,
        reason
    );
    PendingRefund {
        id,
        user,
        token_ledger,
        amount,
        reason: reason.to_string(),
        created_at,
    }
}

async fn pending_refund_history_tip(ledger: Principal) -> Result<u64, StabilityPoolError> {
    let request = vec![GetBlocksRequest {
        start: candid::Nat::from(0u64),
        length: candid::Nat::from(1u64),
    }];
    let result: Result<(GetBlocksResult,), _> = call(ledger, "icrc3_get_blocks", (request,)).await;
    let (response,) = result.map_err(|_| StabilityPoolError::InterCanisterCallFailed {
        target: ledger.to_string(),
        method: "icrc3_get_blocks".to_string(),
    })?;
    response
        .log_length
        .0
        .try_into()
        .map_err(|_| StabilityPoolError::LedgerTransferFailed {
            reason: "ICRC-3 log length does not fit u64; refund remains held".to_string(),
    })
}

fn build_pending_refund_attempt(
    refund: &PendingRefund,
    fee: u64,
    history_start_index: u64,
    previous_timestamp: Option<u64>,
) -> Result<PendingRefundPayoutAttempt, StabilityPoolError> {
    if refund.amount <= fee {
        return Err(StabilityPoolError::AmountTooLow {
            minimum_e8s: fee.saturating_add(1),
        });
    }
    let now = ic_cdk::api::time();
    let created_at_time_ns = match previous_timestamp {
        Some(previous) if now <= previous => {
            previous
                .checked_add(1)
                .ok_or_else(|| StabilityPoolError::LedgerTransferFailed {
                reason: "refund timestamp exhausted; obligation remains held".to_string(),
                })?
            }
        _ => now,
    };
    let mut memo = PENDING_REFUND_MEMO_PREFIX.to_vec();
    memo.extend_from_slice(&refund.id.to_be_bytes());
    memo.extend_from_slice(&created_at_time_ns.to_be_bytes());
    Ok(PendingRefundPayoutAttempt {
        refund_id: refund.id,
        token_ledger: refund.token_ledger,
        from: Account {
            owner: ic_cdk::api::id(),
            subaccount: None,
        },
        to: Account {
            owner: refund.user,
            subaccount: None,
        },
        amount: refund.amount - fee,
        fee,
        memo,
        created_at_time_ns,
        history_start_index,
        dispatch_started: false,
        history_next_index: Some(history_start_index),
        history_tip: None,
        expected_block_index: None,
        last_error: None,
    })
}

fn same_default_account(actual: &Account, expected: &Account) -> bool {
    actual.owner == expected.owner
        && match (&actual.subaccount, &expected.subaccount) {
            (None, None) => true,
            (Some(actual), Some(expected)) => actual == expected,
            (Some(actual), None) | (None, Some(actual)) => *actual == [0; 32],
        }
}

fn exact_pending_refund_block(
    attempt: &PendingRefundPayoutAttempt,
    block: &rumi_protocol_backend::icrc3_proof::DecodedBlock,
) -> bool {
    (block.op == "xfer" || block.op == "transfer")
        && block.btype.as_deref().is_none_or(|kind| kind == "1xfer")
        && block.spender.is_none()
        && block
            .from
            .as_ref()
            .is_some_and(|from| same_default_account(from, &attempt.from))
        && block
            .to
            .as_ref()
            .is_some_and(|to| same_default_account(to, &attempt.to))
        && block.amount == u128::from(attempt.amount)
        && block.fee == Some(attempt.fee)
        && block.memo.as_deref() == Some(attempt.memo.as_slice())
        && block.created_at_time == Some(attempt.created_at_time_ns)
}

fn possible_pending_refund_block(
    attempt: &PendingRefundPayoutAttempt,
    block: &rumi_protocol_backend::icrc3_proof::DecodedBlock,
) -> bool {
    if block.op != "xfer" && block.op != "transfer" {
        return false;
    }
    // A present exact discriminator can safely rule out this attempt. Missing
    // fields cannot: some ICRC-3 block layouts omit transfer metadata, and a
    // partial record with no contradiction must keep the liability held.
    let conclusively_different = block
        .from
        .as_ref()
        .is_some_and(|from| !same_default_account(from, &attempt.from))
        || block
            .to
            .as_ref()
            .is_some_and(|to| !same_default_account(to, &attempt.to))
        || block.spender.is_some()
        || block.amount != u128::from(attempt.amount)
        || block.fee.is_some_and(|fee| fee != attempt.fee)
        || block
            .memo
            .as_deref()
            .is_some_and(|memo| memo != attempt.memo.as_slice())
        || block
            .created_at_time
            .is_some_and(|timestamp| timestamp != attempt.created_at_time_ns)
        || block.btype.as_deref().is_some_and(|kind| kind != "1xfer");
    !conclusively_different
}

fn refresh_pending_refund_history_tip(
    attempt: &mut PendingRefundPayoutAttempt,
    latest_tip: u64,
) -> Result<(), String> {
    if latest_tip < attempt.history_start_index {
        return Err("ledger history tip precedes the persisted dispatch boundary".to_string());
    }
    let cursor = attempt
        .history_next_index
        .unwrap_or(attempt.history_start_index);
    if cursor > latest_tip {
        return Err("persisted refund history cursor exceeds the refreshed ledger tip".to_string());
    }
    attempt.history_tip = Some(latest_tip);
    attempt.history_next_index = Some(cursor);
    Ok(())
}

fn hold_pending_refund_attempt(attempt: &PendingRefundPayoutAttempt, reason: String) {
    let mut held = attempt.clone();
    held.last_error = Some(reason);
    mutate_state(|state| {
        state.update_pending_refund_attempt(held);
    });
}

fn finish_verified_pending_refund(
    refund: &PendingRefund,
    attempt: &PendingRefundPayoutAttempt,
) -> Result<u64, StabilityPoolError> {
    let finalized = mutate_state(|state| state.finalize_pending_refund_payout(refund.id, attempt));
    if !finalized {
        return Err(StabilityPoolError::LedgerTransferFailed {
            reason: "verified refund receipt did not match the live durable attempt; obligation remains held".to_string(),
        });
    }
    log!(INFO, "claim_pending_refund: refund #{} exact payout verified on {} to {}, net {}, fee {}, timestamp {}, block {:?}",
        refund.id, attempt.token_ledger, refund.user, attempt.amount, attempt.fee,
        attempt.created_at_time_ns, attempt.expected_block_index);
    Ok(attempt.amount)
}

async fn verify_pending_refund_attempt(
    refund: &PendingRefund,
    attempt: &PendingRefundPayoutAttempt,
) -> Result<u64, StabilityPoolError> {
    let block_index = attempt
        .expected_block_index
        .expect("verified attempt has index");
    let block =
        rumi_protocol_backend::icrc3_proof::fetch_icrc3_block(attempt.token_ledger, block_index)
    .await
    .map_err(|reason| {
        hold_pending_refund_attempt(attempt, format!("receipt lookup failed: {reason}"));
        StabilityPoolError::LedgerTransferFailed {
                    reason: format!(
                        "refund payout is held; exact ICRC-3 receipt is unavailable: {reason}"
                    ),
        }
    })?;
    if !exact_pending_refund_block(attempt, &block) {
        let reason =
            "returned block index does not match the exact refund transfer tuple".to_string();
        hold_pending_refund_attempt(attempt, reason.clone());
        return Err(StabilityPoolError::LedgerTransferFailed {
            reason: format!("refund payout is held: {reason}"),
        });
    }
    finish_verified_pending_refund(refund, attempt)
}

/// Recover tokens the pool owes after a failed deposit_as_3usd refund
/// (IC-S-001). The liability stays in stable state across every await and is
/// removed only after the exact ICRC-1 tuple is verified in ICRC-3 history.
/// Ambiguous outcomes are reconciled by a bounded scan that follows ledger-
/// advertised archive callbacks; incomplete or conflicting evidence is held.
pub async fn claim_pending_refund(refund_id: u64) -> Result<u64, StabilityPoolError> {
    if crate::pool_balance_mutation_blocked() {
        return Err(StabilityPoolError::SystemBusy);
    }
    // Keep a liquidation absorb from starting while this claim holds the
    // shared SP ledger balance across fee/history/transfer awaits.
    let _balance_async_guard = crate::pool_guard::PoolBalanceAsyncGuard::new();
    let _claim_guard = PendingRefundClaimGuard::reserve(refund_id)?;
    let caller = ic_cdk::api::caller();
    let refund = read_state(|state| {
        state
            .pending_refunds
            .as_ref()
            .and_then(|refunds| refunds.get(&refund_id).cloned())
    })
    .ok_or(StabilityPoolError::RefundClaimNotFound)?;
    if caller != refund.user && !read_state(|state| state.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }

    let mut attempt = read_state(|state| state.pending_refund_attempt(refund_id));
    if attempt.is_none() {
        if !read_state(|state| state.pending_refund_attempt_initializable(refund_id)) {
            return Err(StabilityPoolError::LedgerTransferFailed {
                reason:
                    "legacy refund has no durable payout identity; held for manual reconciliation"
                        .to_string(),
            });
        }
        let fee = current_ledger_transfer_fee(refund.token_ledger).await?;
        let history_start = pending_refund_history_tip(refund.token_ledger).await?;
        let created = build_pending_refund_attempt(&refund, fee, history_start, None)?;
        let stored = mutate_state(|state| {
            if !state.pending_refund_attempt_initializable(refund_id) {
                return false;
            }
            state.put_pending_refund_attempt(created.clone());
            true
        });
        if !stored {
            return Err(StabilityPoolError::SystemBusy);
        }
        attempt = Some(created);
    }
    let mut attempt = attempt.expect("attempt created or loaded");

    if attempt.expected_block_index.is_some() {
        return verify_pending_refund_attempt(&refund, &attempt).await;
    }

    if attempt.dispatch_started {
        // The previous call may have committed even if its reply was lost or
        // a later replay returned BadFee. Refresh the ledger tip on every
        // reconciliation pass so history appended after the pre-dispatch
        // boundary is included before any replacement tuple is considered.
        let latest_tip = match pending_refund_history_tip(attempt.token_ledger).await {
            Ok(tip) => tip,
            Err(error) => {
                hold_pending_refund_attempt(
                    &attempt,
                    format!("history tip unavailable: {error:?}"),
                );
                return Err(error);
            }
        };
        if let Err(reason) = refresh_pending_refund_history_tip(&mut attempt, latest_tip) {
            hold_pending_refund_attempt(&attempt, reason.clone());
            return Err(StabilityPoolError::LedgerTransferFailed {
                reason: format!("refund payout is held: {reason}"),
            });
        }
        let tip = attempt.history_tip.expect("refreshed history tip");
        let mut cursor = attempt
            .history_next_index
            .unwrap_or(attempt.history_start_index);
        let end = tip.min(cursor.saturating_add(MAX_PENDING_REFUND_HISTORY_BLOCKS_PER_CLAIM));
        while cursor < end {
            let block = match rumi_protocol_backend::icrc3_proof::fetch_icrc3_block(
                attempt.token_ledger,
                cursor,
            )
            .await
            {
                Ok(block) => block,
                Err(reason) => {
                    hold_pending_refund_attempt(
                        &attempt,
                        format!("history block {cursor} unavailable or unverified: {reason}"),
                    );
                    return Err(StabilityPoolError::LedgerTransferFailed {
                        reason: format!("refund payout is held at ICRC-3 block {cursor}: {reason}"),
                    });
                }
            };
            if exact_pending_refund_block(&attempt, &block) {
                attempt.expected_block_index = Some(cursor);
                if !mutate_state(|state| state.update_pending_refund_attempt(attempt.clone())) {
                    return Err(StabilityPoolError::SystemBusy);
                }
                return verify_pending_refund_attempt(&refund, &attempt).await;
            }
            if possible_pending_refund_block(&attempt, &block) {
                let reason = format!("ICRC-3 block {cursor} resembles this refund but does not prove its exact tuple");
                hold_pending_refund_attempt(&attempt, reason.clone());
                return Err(StabilityPoolError::LedgerTransferFailed {
                    reason: format!("refund payout is held: {reason}"),
                });
            }
            cursor += 1;
            attempt.history_next_index = Some(cursor);
            if !mutate_state(|state| state.update_pending_refund_attempt(attempt.clone())) {
                return Err(StabilityPoolError::SystemBusy);
            }
        }

        if cursor < tip {
            return Err(StabilityPoolError::LedgerTransferFailed {
                reason: format!("refund payout reconciliation continues at ICRC-3 block {cursor}; liability remains pending"),
            });
        }

        // The persisted scan reached the refreshed current tip with no exact
        // or plausible receipt. Only now may a changed fee get a new tuple.
        let fee = current_ledger_transfer_fee(refund.token_ledger).await?;
        let next_attempt =
            build_pending_refund_attempt(&refund, fee, tip, Some(attempt.created_at_time_ns))?;
        attempt = next_attempt;
        if !mutate_state(|state| state.update_pending_refund_attempt(attempt.clone())) {
            return Err(StabilityPoolError::SystemBusy);
        }
    }

    // Persist dispatch intent before the external call. A callback trap or
    // upgrade after this point will reconcile fresh ICRC-3 history next time.
    attempt.dispatch_started = true;
    if !mutate_state(|state| state.update_pending_refund_attempt(attempt.clone())) {
        return Err(StabilityPoolError::SystemBusy);
    }
    let transfer_args = TransferArg {
        from_subaccount: attempt.from.subaccount,
        to: attempt.to.clone(),
        amount: attempt.amount.into(),
        fee: Some(attempt.fee.into()),
        memo: Some(attempt.memo.clone().into()),
        created_at_time: Some(attempt.created_at_time_ns.into()),
    };
    let result: Result<(Result<candid::Nat, TransferError>,), _> =
        call(attempt.token_ledger, "icrc1_transfer", (transfer_args,)).await;
    match result {
        Ok((Ok(block),)) => {
            let block_index: u64 =
                block
                    .0
                    .try_into()
                    .map_err(|_| StabilityPoolError::LedgerTransferFailed {
                        reason: "refund transfer block index exceeds u64; obligation remains held"
                            .to_string(),
            })?;
            attempt.expected_block_index = Some(block_index);
            if !mutate_state(|state| state.update_pending_refund_attempt(attempt.clone())) {
                return Err(StabilityPoolError::SystemBusy);
            }
            verify_pending_refund_attempt(&refund, &attempt).await
        }
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            let block_index: u64 = duplicate_of.0.try_into().map_err(|_| {
                StabilityPoolError::LedgerTransferFailed {
                    reason: "duplicate refund block index exceeds u64; obligation remains held"
                        .to_string(),
                }
            })?;
            attempt.expected_block_index = Some(block_index);
            if !mutate_state(|state| state.update_pending_refund_attempt(attempt.clone())) {
                return Err(StabilityPoolError::SystemBusy);
            }
            verify_pending_refund_attempt(&refund, &attempt).await
        }
        Ok((Err(TransferError::BadFee { expected_fee }),)) => {
            let expected_fee: u64 = expected_fee.0.try_into().unwrap_or(u64::MAX);
            // BadFee proves only this invocation made no transfer. Keep the
            // exact attempt so the next call scans the complete pinned ledger
            // range before it may create a replacement tuple; a prior
            // ambiguous dispatch of this attempt must still be reconcilable.
            hold_pending_refund_attempt(
                &attempt,
                format!(
                    "BadFee expected {expected_fee}; reconcile this exact attempt before re-arming"
                ),
            );
            Err(StabilityPoolError::LedgerTransferFailed {
                reason: format!(
                    "ledger rejected quoted fee; expected {expected_fee}; refund remains pending"
                ),
            })
        }
        Ok((Err(transfer_error),)) => {
            hold_pending_refund_attempt(
                &attempt,
                format!("ledger returned unresolved error: {transfer_error:?}"),
            );
            Err(StabilityPoolError::LedgerTransferFailed {
                reason: format!(
                    "refund payout is held pending exact ICRC-3 reconciliation: {transfer_error:?}"
                ),
            })
        }
        Err(call_error) => {
            hold_pending_refund_attempt(
                &attempt,
                format!("inter-canister outcome is ambiguous: {call_error:?}"),
            );
            Err(StabilityPoolError::InterCanisterCallFailed {
                target: attempt.token_ledger.to_string(),
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

    fn refund_attempt_for_test(refund: &PendingRefund) -> PendingRefundPayoutAttempt {
        PendingRefundPayoutAttempt {
            refund_id: refund.id,
            token_ledger: refund.token_ledger,
            from: Account {
                owner: principal(90),
                subaccount: None,
            },
            to: Account {
                owner: refund.user,
                subaccount: None,
            },
            amount: refund.amount - 3,
            fee: 3,
            memo: b"RSPRFND:exact-test".to_vec(),
            created_at_time_ns: 1234,
            history_start_index: 7,
            dispatch_started: true,
            history_next_index: Some(7),
            history_tip: Some(8),
            expected_block_index: Some(7),
            last_error: None,
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

    #[test]
    fn ambiguous_commit_then_bad_fee_keeps_tuple_for_receipt_reconciliation() {
        let mut state = crate::state::StabilityPoolState::default();
        let id = state.record_pending_refund(
            principal(1),
            principal(10),
            103,
            "transfer reply was ambiguous".to_string(),
            100,
        );
        let refund = state.pending_refunds_for(&principal(1)).remove(0);
        assert!(state.pending_refund_attempt_initializable(id));
        let mut attempt = refund_attempt_for_test(&refund);
        attempt.history_tip = None;
        attempt.expected_block_index = None;
        state.put_pending_refund_attempt(attempt.clone());
        assert!(!state.pending_refund_attempt_initializable(id));
        crate::state::replace_state(state);

        // The initial compensating transfer can commit while its callback is
        // lost. The attempt was journaled before dispatch, so the next claim
        // refreshes beyond the old pre-dispatch boundary and can find it.
        hold_pending_refund_attempt(&attempt, "BadFee expected 4; reconcile first".to_string());
        attempt = read_state(|s| s.pending_refund_attempt(id)).expect("attempt remains held");
        assert_eq!(
            read_state(|s| s.pending_refunds_for(&principal(1)).len()),
            1
        );
        assert!(
            attempt.dispatch_started,
            "BadFee must not downgrade a submitted tuple to an unsent one"
        );
        assert_eq!(attempt.history_tip, None);
        refresh_pending_refund_history_tip(&mut attempt, 8)
            .expect("retry must extend reconciliation through newly appended blocks");
        assert_eq!(attempt.history_tip, Some(8));
        assert_eq!(attempt.history_next_index, Some(7));
        mutate_state(|s| assert!(s.update_pending_refund_attempt(attempt.clone())));

        let exact = rumi_protocol_backend::icrc3_proof::DecodedBlock {
            btype: Some("1xfer".to_string()),
            op: "xfer".to_string(),
            from: Some(attempt.from.clone()),
            to: Some(attempt.to.clone()),
            spender: None,
            amount: attempt.amount.into(),
            fee: Some(attempt.fee),
            created_at_time: Some(attempt.created_at_time_ns),
            memo: Some(attempt.memo.clone()),
        };
        let mut ambiguous = exact.clone();
        ambiguous.memo = None;
        assert!(!exact_pending_refund_block(&attempt, &ambiguous));
        assert!(possible_pending_refund_block(&attempt, &ambiguous));

        let mut missing_account = exact.clone();
        missing_account.from = None;
        missing_account.memo = None;
        missing_account.created_at_time = None;
        assert!(
            possible_pending_refund_block(&attempt, &missing_account),
            "missing identity fields cannot count as proof that a transfer is unrelated",
        );

        let mut unrelated_source = exact.clone();
        unrelated_source.from = Some(Account {
            owner: principal(91),
            subaccount: None,
        });
        assert!(
            !possible_pending_refund_block(&attempt, &unrelated_source),
            "a present, different source conclusively excludes this exact transfer",
        );
        assert_eq!(
            read_state(|s| s.pending_refunds_for(&principal(1)).len()),
            1
        );
        assert!(read_state(|s| s.pending_refund_attempt(id).is_some()));

        assert!(exact_pending_refund_block(&attempt, &exact));
        attempt.expected_block_index = Some(7);
        mutate_state(|s| assert!(s.update_pending_refund_attempt(attempt.clone())));
        assert_eq!(
            finish_verified_pending_refund(&refund, &attempt).unwrap(),
            100
        );
        assert!(read_state(|s| s
            .pending_refunds_for(&principal(1))
            .is_empty()));
        assert!(read_state(|s| s.pending_refund_attempt(id).is_none()));
        crate::state::replace_state(crate::state::StabilityPoolState::default());
    }
}
