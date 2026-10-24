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
use num_traits::ToPrimitive;
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};

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
    /// Prevent overlapping retries for one durable deposit intent while an
    /// inter-canister call or ICRC-3 reconciliation is outstanding.
    static ACTIVE_DEPOSIT_INTENTS: RefCell<BTreeSet<Principal>> = RefCell::new(BTreeSet::new());
}

const MAX_DEPOSIT_INTENT_HISTORY_BLOCKS_PER_CALL: u64 = 8;
const MAX_DEPOSIT_INTENT_ARCHIVE_PROOF_BLOCKS_PER_CALL: u64 = 8;
const MAX_DEPOSIT_INTENT_ARCHIVE_CALLBACKS_PER_CALL: usize = 8;
const DEPOSIT_INTENT_MEMO_PREFIX: &[u8; 8] = b"RSPDI001";
const DEPOSIT_INTENT_MEMO_DIGEST_BYTES: usize = 24;

struct DepositIntentGuard(Principal);

impl DepositIntentGuard {
    fn reserve(caller: Principal) -> Result<Self, StabilityPoolError> {
        ACTIVE_DEPOSIT_INTENTS.with(|active| {
            if active.borrow_mut().insert(caller) {
                Ok(Self(caller))
            } else {
                Err(StabilityPoolError::SystemBusy)
            }
        })
    }
}

impl Drop for DepositIntentGuard {
    fn drop(&mut self) {
        ACTIVE_DEPOSIT_INTENTS.with(|active| {
            active.borrow_mut().remove(&self.0);
        });
    }
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
pub(crate) async fn current_ledger_transfer_fee(ledger: Principal) -> Result<u64, StabilityPoolError> {
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

/// Submit or reconcile one caller-scoped monotonic deposit intent. The caller
/// must reuse the same sequence and payload until this returns a terminal
/// result. A new sequence is never allocated while an older intent is pending.
pub async fn deposit_with_intent(
    intent_seq: u64,
    token_ledger: Principal,
    amount: u64,
) -> Result<DepositIntentResult, DepositIntentError> {
    let caller = ic_cdk::api::caller();
    if caller == Principal::anonymous() {
        return Err(DepositIntentError::Unauthorized);
    }
    let _intent_guard = DepositIntentGuard::reserve(caller)?;
    let _balance_async_guard = crate::pool_guard::PoolBalanceAsyncGuard::new();

    if let Some(completed) = read_state(|state| state.completed_deposit_intent(caller, intent_seq)) {
        if completed.token_ledger != token_ledger || completed.amount != amount {
            return Err(DepositIntentError::DepositIntentPayloadMismatch);
        }
        return Ok(completed.result);
    }

    let existing = read_state(|state| state.pending_deposit_intent(caller));
    let intent = if let Some(intent) = existing {
        if intent.intent_seq != intent_seq {
            return Err(DepositIntentError::DepositIntentUnresolved {
                active_seq: intent.intent_seq,
            });
        }
        if intent.token_ledger != token_ledger || intent.amount != amount {
            return Err(DepositIntentError::DepositIntentPayloadMismatch);
        }
        intent
    } else {
        let expected = read_state(|state| {
            state
                .deposit_intent_high_watermarks
                .as_ref()
                .and_then(|watermarks| watermarks.get(&caller).copied())
                .unwrap_or(0)
                .checked_add(1)
        })
        .ok_or(DepositIntentError::DepositIntentSequenceExhausted)?;
        if intent_seq != expected {
            return Err(DepositIntentError::DepositIntentSequenceMismatch {
                expected_seq: expected,
            });
        }
        if crate::pool_token_balance_mutation_blocked(&[token_ledger]) {
            return Err(StabilityPoolError::SystemBusy.into());
        }
        let config = read_state(|state| state.get_stablecoin_config(&token_ledger).cloned())
            .ok_or(StabilityPoolError::TokenNotAccepted { ledger: token_ledger })?;
        if !config.is_active {
            return Err(StabilityPoolError::TokenNotActive { ledger: token_ledger }.into());
        }
        let amount_e8s = normalize_to_e8s(amount, config.decimals);
        let minimum = read_state(|state| state.configuration.min_deposit_e8s);
        if amount_e8s < minimum {
            return Err(StabilityPoolError::AmountTooLow {
                minimum_e8s: minimum,
            }.into());
        }
        if read_state(|state| state.configuration.emergency_pause) {
            return Err(StabilityPoolError::EmergencyPaused.into());
        }

        let created_at_time_ns = mutate_state(|state| {
            state.reserve_deposit_transfer_timestamp(ic_cdk::api::time())
        })
        .map_err(|_| StabilityPoolError::SystemBusy)?;
        let memo = deposit_intent_memo(caller, intent_seq, token_ledger, amount);
        let intent = DepositIntent {
            caller,
            intent_seq,
            token_ledger,
            amount,
            from: Account {
                owner: caller,
                subaccount: None,
            },
            to: Account {
                owner: ic_cdk::api::id(),
                subaccount: None,
            },
            memo,
            created_at_time_ns,
            history_start_index: None,
            history_next_index: None,
            history_tip: None,
            expected_block_index: None,
            dispatch_started: false,
            last_error: None,
        };
        let reserve_result = mutate_state(|state| state.reserve_deposit_intent(intent.clone()));
        if let Err(reserve_error) = reserve_result {
            return Err(match reserve_error {
                crate::state::DepositIntentReserveError::Active(active_seq) => {
                    DepositIntentError::DepositIntentUnresolved { active_seq }
                }
                crate::state::DepositIntentReserveError::SequenceMismatch(expected_seq) => {
                    DepositIntentError::DepositIntentSequenceMismatch { expected_seq }
                }
                crate::state::DepositIntentReserveError::SequenceExhausted => {
                    DepositIntentError::DepositIntentSequenceExhausted
                }
                crate::state::DepositIntentReserveError::CapacityReached => {
                    DepositIntentError::DepositIntentCapacityReached
                }
            });
        }
        intent
    };

    resume_deposit_intent(intent).await.map_err(Into::into)
}

/// Authoritative caller-scoped status. This is intentionally exposed as an
/// update method by `lib.rs`; an uncertified query is not enough to unlock or
/// allocate a financial retry.
pub fn deposit_intent_status(caller: Principal, intent_seq: u64) -> DepositIntentStatus {
    read_state(|state| state.deposit_intent_status(caller, intent_seq))
}

fn deposit_intent_transfer_args(intent: &DepositIntent) -> TransferFromArgs {
    TransferFromArgs {
        from: intent.from.clone(),
        to: intent.to.clone(),
        amount: intent.amount.into(),
        fee: None,
        memo: Some(intent.memo.clone().into()),
        created_at_time: Some(intent.created_at_time_ns.into()),
        spender_subaccount: None,
    }
}

fn deposit_intent_memo(
    caller: Principal,
    intent_seq: u64,
    token_ledger: Principal,
    amount: u64,
) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update((caller.as_slice().len() as u8).to_be_bytes());
    hasher.update(caller.as_slice());
    hasher.update(intent_seq.to_be_bytes());
    hasher.update((token_ledger.as_slice().len() as u8).to_be_bytes());
    hasher.update(token_ledger.as_slice());
    hasher.update(amount.to_be_bytes());
    let digest = hasher.finalize();
    let mut memo = DEPOSIT_INTENT_MEMO_PREFIX.to_vec();
    memo.extend_from_slice(&digest[..DEPOSIT_INTENT_MEMO_DIGEST_BYTES]);
    memo
}

fn pending_deposit_intent_result(
    intent: &DepositIntent,
    phase: DepositIntentPhase,
    reason: Option<String>,
) -> DepositIntentResult {
    DepositIntentResult::Pending {
        intent_seq: intent.intent_seq,
        token_ledger: intent.token_ledger,
        amount: intent.amount,
        phase,
        reason,
    }
}

fn save_deposit_intent(intent: &DepositIntent) -> bool {
    mutate_state(|state| state.update_deposit_intent(intent.clone()))
}

fn finish_deposit_intent_no_effect(
    intent: &DepositIntent,
    reason: String,
) -> Result<DepositIntentResult, StabilityPoolError> {
    let result = DepositIntentResult::NoEffect {
        intent_seq: intent.intent_seq,
        token_ledger: intent.token_ledger,
        amount: intent.amount,
        reason,
    };
    if !mutate_state(|state| {
        state.release_deposit_intent_no_effect(intent.caller, intent.intent_seq)
    }) {
        return Err(StabilityPoolError::SystemBusy);
    }
    Ok(result)
}

fn finish_deposit_intent_completed(
    intent: &DepositIntent,
    block_index: u64,
) -> Result<DepositIntentResult, StabilityPoolError> {
    crate::ensure_pool_token_balance_mutation_allowed(&[intent.token_ledger])?;
    let result = DepositIntentResult::Completed {
        intent_seq: intent.intent_seq,
        token_ledger: intent.token_ledger,
        amount: intent.amount,
        block_index,
    };
    if !mutate_state(|state| {
        state.finalize_deposit_intent(
            intent.caller,
            intent.intent_seq,
            result.clone(),
            ic_cdk::api::time(),
        )
    }) {
        return Err(StabilityPoolError::SystemBusy);
    }
    Ok(result)
}

fn exact_deposit_intent_block(
    intent: &DepositIntent,
    pool: Principal,
    block: &rumi_protocol_backend::icrc3_proof::DecodedBlock,
) -> bool {
    (block.op == "transfer" || block.op == "xfer")
        // The in-tree pinned ICRC-1 ledger encodes this transaction as
        // `tx.op = "xfer"` without top-level btype. If present, btype must be
        // the ICRC-2 transfer_from type; exact spender and tuple fields below
        // still distinguish it from an ordinary transfer.
        && block.btype.as_deref().is_none_or(|kind| kind == "2xfer")
        && block.from.as_ref() == Some(&intent.from)
        && block.to.as_ref() == Some(&intent.to)
        && block.spender.as_ref().is_some_and(|spender| {
            spender.owner == pool && spender.subaccount.is_none()
        })
        && block.amount == u128::from(intent.amount)
        && block.memo.as_deref() == Some(intent.memo.as_slice())
        && block.created_at_time == Some(intent.created_at_time_ns)
}

fn possible_deposit_intent_block(intent: &DepositIntent, block: &rumi_protocol_backend::icrc3_proof::DecodedBlock) -> bool {
    block.memo.as_deref() == Some(intent.memo.as_slice())
        || (block.from.as_ref() == Some(&intent.from)
            && block.to.as_ref() == Some(&intent.to)
            && block.amount == u128::from(intent.amount)
            && block.created_at_time == Some(intent.created_at_time_ns))
}

async fn deposit_intent_history_tip(ledger: Principal) -> Result<u64, StabilityPoolError> {
    pending_refund_history_tip(ledger).await
}

#[derive(Clone, Debug)]
struct FetchedDepositIntentBlock {
    index: u64,
    block: ICRC3Value,
    direct_from_ledger: bool,
}

/// Fetch a bounded complete block prefix, following only the exact archive
/// callbacks advertised by the configured ledger. Archive contents can
/// identify a receipt candidate, but only a phash chain ending at a directly
/// served ledger block can authorize credit.
async fn fetch_deposit_intent_page(
    ledger: Principal,
    start: u64,
    length: u64,
) -> Result<Vec<FetchedDepositIntentBlock>, String> {
    if length == 0 || length > MAX_DEPOSIT_INTENT_ARCHIVE_PROOF_BLOCKS_PER_CALL {
        return Err("ICRC-3 deposit proof page exceeds its fixed bound".into());
    }
    let requested_end = start
        .checked_add(length)
        .ok_or_else(|| "ICRC-3 deposit proof range overflow".to_string())?;
    let request = vec![GetBlocksRequest {
        start: start.into(),
        length: length.into(),
    }];
    let result: Result<(GetBlocksResult,), _> =
        call(ledger, "icrc3_get_blocks", (request,)).await;
    let (response,) = result.map_err(|(code, message)| {
        format!("icrc3_get_blocks call failed: {code:?} {message}")
    })?;
    let log_length = response
        .log_length
        .0
        .to_u64()
        .ok_or_else(|| "ICRC-3 deposit log length exceeds u64".to_string())?;
    if log_length <= start {
        return Err("ICRC-3 deposit log does not include the requested block".into());
    }
    let page_end = requested_end.min(log_length);
    let expected_count = usize::try_from(page_end - start)
        .map_err(|_| "ICRC-3 deposit page exceeds platform limits".to_string())?;
    let mut blocks = BTreeMap::<u64, FetchedDepositIntentBlock>::new();
    for block in response.blocks {
        insert_deposit_intent_page_block(&mut blocks, block.id, block.block, true, start, page_end)?;
    }
    if response.archived_blocks.len() > MAX_DEPOSIT_INTENT_ARCHIVE_CALLBACKS_PER_CALL
        || response.archived_blocks.len() > expected_count
    {
        return Err("ledger returned too many archive callbacks for the bounded deposit page".into());
    }
    let mut callback_ids = BTreeSet::new();
    for archived in response.archived_blocks {
        if archived.callback.method.trim().is_empty()
            || archived.callback.canister_id == Principal::anonymous()
            || archived.args.is_empty()
            || archived.args.len() > expected_count
        {
            return Err("ledger returned an invalid ICRC-3 deposit archive callback".into());
        }
        let mut allowed_ids = BTreeSet::new();
        for arg in &archived.args {
            let arg_start = arg.start.0.to_u64()
                .ok_or_else(|| "deposit archive range start exceeds u64".to_string())?;
            let arg_length = arg.length.0.to_u64()
                .ok_or_else(|| "deposit archive range length exceeds u64".to_string())?;
            let arg_end = arg_start.checked_add(arg_length)
                .ok_or_else(|| "deposit archive range overflow".to_string())?;
            if arg_length == 0 || arg_start < start || arg_end > page_end {
                return Err("deposit archive callback exceeds the requested proof page".into());
            }
            for index in arg_start..arg_end {
                if !callback_ids.insert(index) || !allowed_ids.insert(index) {
                    return Err("deposit archive callbacks contain overlapping ranges".into());
                }
            }
        }
        let (archive_response,): (GetBlocksResult,) = call(
            archived.callback.canister_id,
            archived.callback.method.as_str(),
            (archived.args,),
        ).await.map_err(|(code, message)| {
            format!("ICRC-3 deposit archive callback failed: {code:?} {message}")
        })?;
        if !archive_response.archived_blocks.is_empty()
            || archive_response.blocks.len() > allowed_ids.len()
        {
            return Err("deposit archive callback returned nested or oversized history".into());
        }
        for block in archive_response.blocks {
            let index = block.id.0.to_u64()
                .ok_or_else(|| "archived deposit block ID exceeds u64".to_string())?;
            if !allowed_ids.contains(&index) {
                return Err("deposit archive callback returned a block outside its range".into());
            }
            insert_deposit_intent_page_block(
                &mut blocks, block.id, block.block, false, start, page_end,
            )?;
        }
    }
    let mut prefix_end = start;
    while prefix_end < page_end && blocks.contains_key(&prefix_end) {
        prefix_end += 1;
    }
    if (prefix_end..page_end).any(|index| blocks.contains_key(&index)) {
        return Err("ICRC-3 deposit history has a gap before a later block".into());
    }
    if prefix_end == start {
        return Err("ICRC-3 deposit page contains no complete block prefix".into());
    }
    Ok(blocks.into_values().collect())
}

fn insert_deposit_intent_page_block(
    blocks: &mut BTreeMap<u64, FetchedDepositIntentBlock>,
    id: Nat,
    block: ICRC3Value,
    direct_from_ledger: bool,
    start: u64,
    end: u64,
) -> Result<(), String> {
    let index = id.0.to_u64()
        .ok_or_else(|| "ICRC-3 deposit block ID exceeds u64".to_string())?;
    if index < start || index >= end {
        return Err("ICRC-3 deposit response returned a block outside its requested range".into());
    }
    if blocks.insert(index, FetchedDepositIntentBlock { index, block, direct_from_ledger }).is_some() {
        return Err("ICRC-3 deposit response contains duplicate block IDs".into());
    }
    Ok(())
}

async fn fetch_deposit_intent_block(
    ledger: Principal,
    block_index: u64,
) -> Result<FetchedDepositIntentBlock, String> {
    fetch_deposit_intent_page(ledger, block_index, 1).await?
        .into_iter()
        .find(|block| block.index == block_index)
        .ok_or_else(|| "ledger did not return the exact deposit block".to_string())
}

/// Verify an ambiguous conversion ingress against a directly served ICRC-3
/// block. This deliberately reuses the bounded exact-block fetcher used by
/// durable deposit intents; archive contents or a balance query never prove a
/// conversion pull.
pub(crate) async fn verify_exact_conversion_pull_block(
    ledger: Principal,
    block_index: u64,
    from: &Account,
    to: &Account,
    amount: u64,
    fee_raw: Option<u64>,
    memo: &[u8],
    created_at_time_ns: u64,
) -> Result<(), String> {
    let fetched = fetch_deposit_intent_block(ledger, block_index).await?;
    if !fetched.direct_from_ledger {
        return Err("candidate conversion block came from an archive callback".into());
    }
    let decoded = rumi_protocol_backend::icrc3_proof::decode_block(&fetched.block)?;
    if (decoded.op != "transfer" && decoded.op != "xfer")
        || decoded.btype.as_deref().is_some_and(|kind| kind != "2xfer")
        || decoded.from.as_ref() != Some(from)
        || decoded.to.as_ref() != Some(to)
        || !decoded.spender.as_ref().is_some_and(|spender| {
            spender.owner == ic_cdk::api::id() && spender.subaccount.is_none()
        })
        || decoded.amount != u128::from(amount)
        || fee_raw.is_some_and(|fee| decoded.fee != Some(fee))
        || decoded.memo.as_deref() != Some(memo)
        || decoded.created_at_time != Some(created_at_time_ns)
    {
        return Err("candidate ledger block does not match the exact conversion pull tuple".into());
    }
    Ok(())
}

fn deposit_intent_parent_hash(block: &ICRC3Value, index: u64) -> Result<Option<[u8; 32]>, String> {
    let map = match block {
        ICRC3Value::Map(map) => map,
        _ => return Err("ICRC-3 deposit block is not a map".into()),
    };
    match map.get("phash") {
        Some(ICRC3Value::Blob(bytes)) if bytes.len() == 32 => {
            let mut hash = [0; 32];
            hash.copy_from_slice(bytes);
            Ok(Some(hash))
        }
        Some(_) => Err("ICRC-3 deposit parent hash is not a 32-byte blob".into()),
        None if index == 0 => Ok(None),
        None => Err("non-genesis ICRC-3 deposit block has no parent hash".into()),
    }
}

fn deposit_intent_block_hash(block: &ICRC3Value) -> Result<[u8; 32], String> {
    fn contains_int(value: &ICRC3Value) -> bool {
        match value {
            ICRC3Value::Int(_) => true,
            ICRC3Value::Array(values) => values.iter().any(contains_int),
            ICRC3Value::Map(values) => values.values().any(contains_int),
            ICRC3Value::Blob(_) | ICRC3Value::Text(_) | ICRC3Value::Nat(_) => false,
        }
    }
    if contains_int(block) {
        return Err("ICRC-3 deposit block contains Int unsupported by the pinned hash implementation".into());
    }
    Ok(block.clone().hash())
}

fn deposit_intent_transfer_digest(intent: &DepositIntent) -> Result<[u8; 32], String> {
    let bytes = candid::encode_args((
        intent.caller,
        intent.intent_seq,
        intent.token_ledger,
        intent.amount,
        &intent.from,
        &intent.to,
        &intent.memo,
        intent.created_at_time_ns,
    )).map_err(|error| format!("could not encode deposit transfer identity: {error}"))?;
    Ok(Sha256::digest(bytes).into())
}

fn hold_deposit_intent(intent: &DepositIntent, reason: String) -> DepositIntentResult {
    let mut held = intent.clone();
    held.last_error = Some(reason.clone());
    save_deposit_intent(&held);
    pending_deposit_intent_result(&held, DepositIntentPhase::Reconciling, Some(reason))
}

async fn verify_deposit_intent_receipt(
    intent: &DepositIntent,
) -> Result<DepositIntentResult, StabilityPoolError> {
    let block_index = intent.expected_block_index.expect("receipt index was checked");
    let digest = match deposit_intent_transfer_digest(intent) {
        Ok(digest) => digest,
        Err(reason) => return Ok(hold_deposit_intent(intent, reason)),
    };
    let mut progress = match read_state(|state| state.deposit_intent_archive_proof(intent.caller)) {
        Some(progress)
            if !progress.history_segment
                && progress.token_ledger == intent.token_ledger
                && progress.intent_seq == intent.intent_seq
                && progress.segment_start_index == block_index
                && progress.transfer_digest == digest
                && progress.exact_match.as_ref().is_some_and(|(index, _)| *index == block_index)
                && progress.latest_verified_block_index.checked_add(1)
                    == Some(progress.next_block_index) => progress,
        Some(_) => return Ok(hold_deposit_intent(intent, "saved archive proof cursor does not match this exact deposit intent".into())),
        None => {
            let candidate = match fetch_deposit_intent_block(intent.token_ledger, block_index).await {
                Ok(candidate) => candidate,
                Err(reason) => return Ok(hold_deposit_intent(intent, format!("exact ICRC-3 receipt is unavailable: {reason}"))),
            };
            let decoded = match rumi_protocol_backend::icrc3_proof::decode_block(&candidate.block) {
                Ok(decoded) => decoded,
                Err(reason) => return Ok(hold_deposit_intent(intent, format!("deposit candidate block is malformed: {reason}"))),
            };
            if !exact_deposit_intent_block(intent, ic_cdk::api::id(), &decoded) {
                return Ok(hold_deposit_intent(intent, "ledger block does not match the persisted ICRC-2 transfer tuple".into()));
            }
            if candidate.direct_from_ledger {
                return finish_deposit_intent_completed(intent, block_index);
            }
            let prior_block_hash = match deposit_intent_block_hash(&candidate.block) {
                Ok(hash) => hash,
                Err(reason) => return Ok(hold_deposit_intent(intent, reason)),
            };
            let next_block_index = match block_index.checked_add(1) {
                Some(next) => next,
                None => return Ok(hold_deposit_intent(intent, "deposit candidate block index overflow".into())),
            };
            let progress = crate::state::DepositIntentArchiveProofProgress {
                token_ledger: intent.token_ledger,
                intent_seq: intent.intent_seq,
                transfer_digest: digest,
                history_segment: false,
                segment_start_index: block_index,
                pinned_history_tip: None,
                exact_match: Some((block_index, prior_block_hash)),
                possible_mismatch: false,
                multiple_exact_matches: false,
                latest_verified_block_index: block_index,
                next_block_index,
                prior_block_hash,
            };
            if !mutate_state(|state| state.save_deposit_intent_archive_proof(intent.caller, progress.clone())) {
                return Err(StabilityPoolError::SystemBusy);
            }
            progress
        }
    };
    let page = match fetch_deposit_intent_page(
        intent.token_ledger,
        progress.next_block_index,
        MAX_DEPOSIT_INTENT_ARCHIVE_PROOF_BLOCKS_PER_CALL,
    ).await {
        Ok(page) => page,
        Err(reason) => return Ok(hold_deposit_intent(intent, format!("archived receipt proof remains pending: {reason}"))),
    };
    match advance_deposit_intent_archive_proof(intent, &mut progress, page)? {
        DepositIntentArchiveAdvance::Exact(block_index) => finish_deposit_intent_completed(intent, block_index),
        DepositIntentArchiveAdvance::Restart => {
            mutate_state(|state| state.remove_deposit_intent_archive_proof(intent.caller));
            Ok(hold_deposit_intent(intent, "archived receipt chain changed or contained a malformed row; proof will restart from the exact candidate".into()))
        }
        DepositIntentArchiveAdvance::Ambiguous => Ok(hold_deposit_intent(intent, "archived receipt proof found conflicting or plausible transfer rows; credit remains held".into())),
        DepositIntentArchiveAdvance::AnchoredNoMatch => Ok(hold_deposit_intent(intent, "archived receipt proof anchored without the exact transfer tuple".into())),
        DepositIntentArchiveAdvance::Pending => Ok(hold_deposit_intent(intent, "archived receipt proof is still progressing; retry to continue".into())),
    }
}

/// Classify one authenticated-chain row. Classification is provisional until
/// the page reaches a block the configured ledger served directly.
fn classify_deposit_intent_archive_row(
    intent: &DepositIntent,
    progress: &mut crate::state::DepositIntentArchiveProofProgress,
    block: &FetchedDepositIntentBlock,
) -> Result<(), String> {
    let decoded = rumi_protocol_backend::icrc3_proof::decode_block(&block.block)?;
    if exact_deposit_intent_block(intent, ic_cdk::api::id(), &decoded) {
        let hash = deposit_intent_block_hash(&block.block)?;
        if progress.exact_match.is_some() {
            progress.multiple_exact_matches = true;
        } else {
            progress.exact_match = Some((block.index, hash));
        }
    } else if possible_deposit_intent_block(intent, &decoded) {
        progress.possible_mismatch = true;
    }
    Ok(())
}

/// Advance one archived segment in linear time. The caller persists the
/// initial archived row before awaiting the next page; each accepted row is
/// then saved before the next update can continue the walk.
enum DepositIntentArchiveAdvance {
    Pending,
    Restart,
    AnchoredNoMatch,
    Ambiguous,
    Exact(u64),
}

fn advance_deposit_intent_archive_proof(
    intent: &DepositIntent,
    progress: &mut crate::state::DepositIntentArchiveProofProgress,
    page: Vec<FetchedDepositIntentBlock>,
) -> Result<DepositIntentArchiveAdvance, StabilityPoolError> {
    for block in page {
        if block.index != progress.next_block_index
            || deposit_intent_parent_hash(&block.block, block.index).ok().flatten()
                != Some(progress.prior_block_hash)
        {
            return Ok(DepositIntentArchiveAdvance::Restart);
        }
        if let Err(reason) = classify_deposit_intent_archive_row(intent, progress, &block) {
            log!(INFO, "deposit archive proof row {} is malformed: {}", block.index, reason);
            return Ok(DepositIntentArchiveAdvance::Restart);
        }
        if block.direct_from_ledger {
            if progress.possible_mismatch || progress.multiple_exact_matches {
                return Ok(DepositIntentArchiveAdvance::Ambiguous);
            }
            if let Some((exact_index, _)) = progress.exact_match {
                if progress.history_segment {
                    crate::ensure_pool_token_balance_mutation_allowed(&[intent.token_ledger])?;
                    let mut advanced = intent.clone();
                    advanced.expected_block_index = Some(exact_index);
                    advanced.history_next_index = Some(exact_index);
                    advanced.last_error = None;
                    if !save_deposit_intent(&advanced) {
                        return Err(StabilityPoolError::SystemBusy);
                    }
                }
                return Ok(DepositIntentArchiveAdvance::Exact(exact_index));
            }
            if progress.history_segment {
                let mut advanced = intent.clone();
                advanced.history_next_index = block.index.checked_add(1);
                advanced.last_error = None;
                if !save_deposit_intent(&advanced) {
                    return Err(StabilityPoolError::SystemBusy);
                }
                mutate_state(|state| state.remove_deposit_intent_archive_proof(intent.caller));
            }
            return Ok(DepositIntentArchiveAdvance::AnchoredNoMatch);
        }
        progress.prior_block_hash = match deposit_intent_block_hash(&block.block) {
            Ok(hash) => hash,
            Err(reason) => {
                log!(INFO, "deposit archive proof hash failed at {}: {}", block.index, reason);
                return Ok(DepositIntentArchiveAdvance::Restart);
            }
        };
        progress.latest_verified_block_index = block.index;
        progress.next_block_index = match block.index.checked_add(1) {
            Some(next) => next,
            None => return Ok(DepositIntentArchiveAdvance::Pending),
        };
        if !mutate_state(|state| state.save_deposit_intent_archive_proof(intent.caller, progress.clone())) {
            return Err(StabilityPoolError::SystemBusy);
        }
    }
    Ok(DepositIntentArchiveAdvance::Pending)
}

async fn reconcile_deposit_intent_history(
    intent: &mut DepositIntent,
) -> Result<Option<DepositIntentResult>, StabilityPoolError> {
    let latest_tip = match deposit_intent_history_tip(intent.token_ledger).await {
        Ok(tip) => tip,
        Err(error) => {
            return Ok(Some(hold_deposit_intent(
                intent,
                format!("ledger history tip unavailable: {error:?}"),
            )));
        }
    };
    let start = intent.history_start_index.ok_or_else(|| {
        StabilityPoolError::LedgerTransferFailed {
            reason: "deposit intent has no persisted ledger history boundary".to_string(),
        }
    })?;
    let mut cursor = intent.history_next_index.unwrap_or(start);
    let mut pinned_tip = intent.history_tip.unwrap_or(latest_tip);
    if latest_tip < start || latest_tip < pinned_tip || cursor > pinned_tip {
        return Ok(Some(hold_deposit_intent(
            intent,
            "ledger tip or saved history cursor precedes the persisted dispatch boundary".to_string(),
        )));
    }

    let digest = match deposit_intent_transfer_digest(intent) {
        Ok(digest) => digest,
        Err(reason) => return Ok(Some(hold_deposit_intent(intent, reason))),
    };
    if let Some(mut progress) = read_state(|state| state.deposit_intent_archive_proof(intent.caller)) {
        if !progress.history_segment
            || progress.token_ledger != intent.token_ledger
            || progress.intent_seq != intent.intent_seq
            || progress.transfer_digest != digest
            || progress.segment_start_index != cursor
            || progress.pinned_history_tip != Some(pinned_tip)
            || progress.latest_verified_block_index.checked_add(1) != Some(progress.next_block_index)
        {
            return Ok(Some(hold_deposit_intent(intent, "saved archive history cursor does not match this exact deposit intent".into())));
        }
        if latest_tip > pinned_tip {
            pinned_tip = latest_tip;
            intent.history_tip = Some(pinned_tip);
            progress.pinned_history_tip = Some(pinned_tip);
            if !mutate_state(|state| {
                state.update_deposit_intent(intent.clone())
                    && state.save_deposit_intent_archive_proof(intent.caller, progress.clone())
            }) {
                return Err(StabilityPoolError::SystemBusy);
            }
        }
        if progress.next_block_index >= pinned_tip {
            return Ok(Some(hold_deposit_intent(intent, "archived history prefix has no directly served anchor yet".into())));
        }
        let length = (pinned_tip - progress.next_block_index)
            .min(MAX_DEPOSIT_INTENT_ARCHIVE_PROOF_BLOCKS_PER_CALL);
        let page = match fetch_deposit_intent_page(intent.token_ledger, progress.next_block_index, length).await {
            Ok(page) => page,
            Err(reason) => return Ok(Some(hold_deposit_intent(intent, format!("archived history proof remains pending: {reason}")))),
        };
        return match advance_deposit_intent_archive_proof(intent, &mut progress, page)? {
            DepositIntentArchiveAdvance::Exact(index) => Ok(Some(finish_deposit_intent_completed(intent, index)?)),
            DepositIntentArchiveAdvance::Restart => {
                mutate_state(|state| state.remove_deposit_intent_archive_proof(intent.caller));
                Ok(Some(hold_deposit_intent(
                    intent,
                    "archived history chain changed or contained a malformed row; proof will restart from the saved history boundary".into(),
                )))
            }
            DepositIntentArchiveAdvance::AnchoredNoMatch => Ok(Some(pending_deposit_intent_result(
                intent,
                DepositIntentPhase::Reconciling,
                Some("authenticated archived history continues after its direct anchor".into()),
            ))),
            DepositIntentArchiveAdvance::Ambiguous => Ok(Some(hold_deposit_intent(
                intent,
                "authenticated archived history contains a plausible or repeated transfer candidate".into(),
            ))),
            DepositIntentArchiveAdvance::Pending => Ok(Some(hold_deposit_intent(
                intent,
                "archived history proof is incomplete; retry to continue".into(),
            ))),
        };
    }

    if cursor == pinned_tip {
        if latest_tip > pinned_tip {
            intent.history_tip = Some(latest_tip);
            if !save_deposit_intent(intent) {
                return Err(StabilityPoolError::SystemBusy);
            }
            return Ok(Some(pending_deposit_intent_result(
                intent,
                DepositIntentPhase::Reconciling,
                Some("ledger history advanced; continuing exact receipt reconciliation".to_string()),
            )));
        }
        return Ok(None);
    }
    intent.history_tip = Some(pinned_tip);
    let length = (pinned_tip - cursor).min(MAX_DEPOSIT_INTENT_HISTORY_BLOCKS_PER_CALL);
    let page = match fetch_deposit_intent_page(intent.token_ledger, cursor, length).await {
        Ok(page) => page,
        Err(reason) => return Ok(Some(hold_deposit_intent(intent, format!("ICRC-3 history page is unavailable or unverified: {reason}")))),
    };
    for (offset, fetched) in page.iter().enumerate() {
        if fetched.index != cursor {
            return Ok(Some(hold_deposit_intent(intent, "ICRC-3 history page contains a gap or wrong block ID".into())));
        }
        if !fetched.direct_from_ledger {
            let hash = match deposit_intent_block_hash(&fetched.block) {
                Ok(hash) => hash,
                Err(reason) => return Ok(Some(hold_deposit_intent(intent, reason))),
            };
            let mut progress = crate::state::DepositIntentArchiveProofProgress {
                token_ledger: intent.token_ledger,
                intent_seq: intent.intent_seq,
                transfer_digest: digest,
                history_segment: true,
                segment_start_index: cursor,
                pinned_history_tip: Some(pinned_tip),
                exact_match: None,
                possible_mismatch: false,
                multiple_exact_matches: false,
                latest_verified_block_index: cursor,
                next_block_index: cursor.checked_add(1).ok_or_else(|| StabilityPoolError::LedgerTransferFailed { reason: "archive proof block index overflow".into() })?,
                prior_block_hash: hash,
            };
            if let Err(reason) = classify_deposit_intent_archive_row(intent, &mut progress, fetched) {
                return Ok(Some(hold_deposit_intent(intent, format!("archived history block {cursor} is malformed: {reason}"))));
            }
            if !mutate_state(|state| state.save_deposit_intent_archive_proof(intent.caller, progress.clone())) {
                return Err(StabilityPoolError::SystemBusy);
            }
            let remainder = page.iter().skip(offset + 1).cloned().collect();
            return match advance_deposit_intent_archive_proof(intent, &mut progress, remainder)? {
                DepositIntentArchiveAdvance::Exact(index) => Ok(Some(finish_deposit_intent_completed(intent, index)?)),
                DepositIntentArchiveAdvance::Restart => {
                    mutate_state(|state| state.remove_deposit_intent_archive_proof(intent.caller));
                    Ok(Some(hold_deposit_intent(
                        intent,
                        "archived history chain changed or contained a malformed row; proof will restart from the saved history boundary".into(),
                    )))
                }
                DepositIntentArchiveAdvance::AnchoredNoMatch => Ok(Some(pending_deposit_intent_result(
                    intent,
                    DepositIntentPhase::Reconciling,
                    Some("authenticated archived history continues after its direct anchor".into()),
                ))),
                DepositIntentArchiveAdvance::Ambiguous => Ok(Some(hold_deposit_intent(
                    intent,
                    "authenticated archived history contains a plausible or repeated transfer candidate".into(),
                ))),
                DepositIntentArchiveAdvance::Pending => Ok(Some(hold_deposit_intent(
                    intent,
                    "archived history proof is incomplete; retry to continue".into(),
                ))),
            };
        }
        let block = match rumi_protocol_backend::icrc3_proof::decode_block(&fetched.block) {
            Ok(block) => block,
            Err(reason) => return Ok(Some(hold_deposit_intent(intent, format!("ICRC-3 history block {cursor} is malformed: {reason}")))),
        };
        if exact_deposit_intent_block(intent, ic_cdk::api::id(), &block) {
            intent.expected_block_index = Some(cursor);
            intent.history_next_index = Some(cursor);
            if !save_deposit_intent(intent) { return Err(StabilityPoolError::SystemBusy); }
            return Ok(Some(verify_deposit_intent_receipt(intent).await?));
        }
        if possible_deposit_intent_block(intent, &block) {
            return Ok(Some(hold_deposit_intent(intent, format!("ICRC-3 block {cursor} resembles this intent but does not prove its exact tuple"))));
        }
        cursor += 1;
        intent.history_next_index = Some(cursor);
        if !save_deposit_intent(intent) { return Err(StabilityPoolError::SystemBusy); }
    }
    if cursor < pinned_tip {
        return Ok(Some(pending_deposit_intent_result(
            intent,
            DepositIntentPhase::Reconciling,
            Some(format!("history reconciliation continues at ledger block {cursor}")),
        )));
    }
    if latest_tip > pinned_tip {
        intent.history_tip = Some(latest_tip);
        if !save_deposit_intent(intent) {
            return Err(StabilityPoolError::SystemBusy);
        }
        return Ok(Some(pending_deposit_intent_result(
            intent,
            DepositIntentPhase::Reconciling,
            Some("ledger history advanced; continuing exact receipt reconciliation".to_string()),
        )));
    }
    Ok(None)
}

fn confirmed_no_effect_transfer_error(error: &TransferFromError) -> bool {
    matches!(
        error,
        TransferFromError::BadFee { .. }
            | TransferFromError::BadBurn { .. }
            | TransferFromError::InsufficientFunds { .. }
            | TransferFromError::InsufficientAllowance { .. }
            | TransferFromError::TooOld
            | TransferFromError::CreatedInFuture { .. }
    )
}

async fn resume_deposit_intent(
    mut intent: DepositIntent,
) -> Result<DepositIntentResult, StabilityPoolError> {
    if intent.expected_block_index.is_some() {
        return verify_deposit_intent_receipt(&intent).await;
    }

    let previously_dispatched = intent.dispatch_started;
    let mut absence_proven = false;
    if previously_dispatched {
        if let Some(result) = reconcile_deposit_intent_history(&mut intent).await? {
            if matches!(result, DepositIntentResult::Completed { .. }) {
                return Ok(result);
            }
            return Ok(result);
        }
        absence_proven = true;
    } else if intent.history_start_index.is_none() {
        let tip = match deposit_intent_history_tip(intent.token_ledger).await {
            Ok(tip) => tip,
            Err(error) => {
                return Ok(hold_deposit_intent(
                    &intent,
                    format!("pre-dispatch ledger history boundary unavailable: {error:?}"),
                ));
            }
        };
        intent.history_start_index = Some(tip);
        intent.history_next_index = Some(tip);
        intent.history_tip = Some(tip);
        if !save_deposit_intent(&intent) {
            return Err(StabilityPoolError::SystemBusy);
        }
    }

    // Reconciliation may have taken multiple update calls. Recheck the shared
    // mutation gate immediately before every initial or replayed ledger pull;
    // a liquidation that began before this call acquired its async guard must
    // not race a new transfer_from.
    if crate::pool_token_balance_mutation_blocked(&[intent.token_ledger]) {
        return Ok(hold_deposit_intent(
            &intent,
            "pool token balance mutation is blocked; exact intent remains pending".to_string(),
        ));
    }
    let (token_is_active, emergency_paused) = read_state(|state| {
        (
            state
                .get_stablecoin_config(&intent.token_ledger)
                .is_some_and(|config| config.is_active),
            state.configuration.emergency_pause,
        )
    });
    if !token_is_active || emergency_paused {
        return Ok(hold_deposit_intent(
            &intent,
            if emergency_paused {
                "emergency pause prevents a new transfer; exact intent remains pending".to_string()
            } else {
                "token is inactive; exact intent remains pending".to_string()
            },
        ));
    }

    let args = deposit_intent_transfer_args(&intent);
    intent.dispatch_started = true;
    intent.last_error = None;
    if !save_deposit_intent(&intent) {
        return Err(StabilityPoolError::SystemBusy);
    }
    let call_result: Result<(Result<candid::Nat, TransferFromError>,), _> =
        call(intent.token_ledger, "icrc2_transfer_from", (args,)).await;
    match call_result {
        Ok((Ok(block),)) => {
            let block_index = match block.0.try_into() {
                Ok(index) => index,
                Err(_) => {
                    return Ok(hold_deposit_intent(
                        &intent,
                        "transfer block index exceeds u64; exact receipt remains unresolved".to_string(),
                    ));
                }
            };
            intent.expected_block_index = Some(block_index);
            if !save_deposit_intent(&intent) {
                return Err(StabilityPoolError::SystemBusy);
            }
            verify_deposit_intent_receipt(&intent).await
        }
        Ok((Err(TransferFromError::Duplicate { duplicate_of }),)) => {
            let block_index = match duplicate_of.0.try_into() {
                Ok(index) => index,
                Err(_) => {
                    return Ok(hold_deposit_intent(
                        &intent,
                        "duplicate block index exceeds u64; exact receipt remains unresolved".to_string(),
                    ));
                }
            };
            intent.expected_block_index = Some(block_index);
            if !save_deposit_intent(&intent) {
                return Err(StabilityPoolError::SystemBusy);
            }
            verify_deposit_intent_receipt(&intent).await
        }
        Ok((Err(error),)) if confirmed_no_effect_transfer_error(&error)
            && (!previously_dispatched || absence_proven) =>
        {
            finish_deposit_intent_no_effect(&intent, format!("ledger confirmed no transfer: {error:?}"))
        }
        Ok((Err(error),)) => Ok(hold_deposit_intent(
            &intent,
            format!("ledger returned an unresolved transfer error: {error:?}"),
        )),
        Err(error) => Ok(hold_deposit_intent(
            &intent,
            format!("transfer call outcome is ambiguous: {error:?}"),
        )),
    }
}

/// Withdraw with a durable exact transfer tuple. Unknown ledger outcomes keep
/// the original debit and tuple pending; they never restore spendable credit.
pub async fn withdraw(token_ledger: Principal, amount: u64) -> Result<(), StabilityPoolError> {
    crate::withdrawals::withdraw(token_ledger, amount).await
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

pub(crate) async fn verify_collateral_payout_block(
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
    let completed = complete_outbound_payout_and_append(
        claim_owner,
        collateral_ledger,
        &reserved,
        block_index,
        ic_cdk::api::time(),
    );
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
            let completed = complete_outbound_payout_and_append(
                caller,
                ledger,
                &payout,
                block_index,
                ic_cdk::api::time(),
            );
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
            let completed = complete_outbound_payout_and_append(
                caller,
                ledger,
                &payout,
                block_index,
                ic_cdk::api::time(),
            );
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

/// Apply the completion to heap accounting and append its permanent receipt in
/// this same synchronous response message. A trap in either write rolls both
/// back under IC message atomicity.
fn complete_outbound_payout_and_append(
    caller: Principal,
    ledger: Principal,
    payout: &PendingOutboundPayout,
    block_index: u64,
    now_ns: u64,
) -> bool {
    let timestamp = payout.transfer_created_at_time_ns;
    let existing = match crate::receipt_store::get(caller, timestamp) {
        Ok(existing) => existing,
        Err(reason) => ic_cdk::trap(&reason),
    };
    if let Some(existing) = existing {
        let has_pending = read_state(|state| {
            state
                .pending_outbound_payout(&caller, &ledger)
                .is_some()
        });
        if let Err(reason) = validate_existing_completion(
            &existing,
            ledger,
            payout,
            block_index,
            has_pending,
        ) {
            ic_cdk::trap(reason);
        }
        // The permanent journal is authoritative. A replay after completion
        // must not reapply counters or append a duplicate event.
        return true;
    }

    let Some(completed) = mutate_state(|state| {
        state.complete_outbound_payout_record(caller, ledger, payout, block_index, now_ns)
    }) else {
        return false;
    };
    if let Err(reason) = crate::receipt_store::append(
        caller,
        crate::receipt_store::StoredReceipt {
            ledger,
            receipt: completed,
        },
    ) {
        ic_cdk::trap(&reason);
    }
    true
}

fn validate_existing_completion(
    existing: &crate::receipt_store::StoredReceipt,
    ledger: Principal,
    payout: &PendingOutboundPayout,
    block_index: u64,
    has_pending: bool,
) -> Result<(), &'static str> {
    let saved = &existing.receipt.payout;
    if existing.ledger != ledger
        || saved.gross_amount != payout.gross_amount
        || saved.transfer_amount != payout.transfer_amount
        || saved.transfer_fee != payout.transfer_fee
        || saved.recipient != payout.recipient
        || saved.from_subaccount != payout.from_subaccount
        || saved.transfer_memo != payout.transfer_memo
        || saved.transfer_created_at_time_ns != payout.transfer_created_at_time_ns
        || existing.receipt.block_index != block_index
    {
        return Err("SP claim completion conflicts with an existing owner/timestamp receipt");
    }
    if has_pending {
        return Err("SP claim receipt exists while a pending tuple remains; refusing duplicate accounting");
    }
    Ok(())
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
    let mut first_error = None;
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
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
        }
    }

    finish_claim_all(claimed, first_error)
}

fn finish_claim_all(
    claimed: BTreeMap<Principal, u64>,
    first_error: Option<StabilityPoolError>,
) -> Result<BTreeMap<Principal, u64>, StabilityPoolError> {
    if claimed.is_empty() {
        if let Some(error) = first_error {
            return Err(error);
        }
    }
    Ok(claimed)
}
pub async fn deposit_as_3usd(
    token_ledger: Principal,
    amount: u64,
) -> Result<u64, StabilityPoolError> {
    crate::conversion::legacy_deposit_as_3usd(token_ledger, amount).await
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

    #[test]
    fn deposit_intent_receipt_binds_full_icrc2_identity() {
        let caller = principal(1);
        let pool = principal(2);
        let ledger = principal(3);
        let intent = DepositIntent {
            caller,
            intent_seq: 4,
            token_ledger: ledger,
            amount: 900,
            from: Account { owner: caller, subaccount: None },
            to: Account { owner: pool, subaccount: None },
            memo: b"RSPDI001caller-seq-4".to_vec(),
            created_at_time_ns: 12345,
            history_start_index: Some(10),
            history_next_index: Some(10),
            history_tip: Some(10),
            expected_block_index: Some(10),
            dispatch_started: true,
            last_error: None,
        };
        let block = rumi_protocol_backend::icrc3_proof::DecodedBlock {
            btype: Some("2xfer".to_string()),
            op: "transfer".to_string(),
            from: Some(intent.from.clone()),
            to: Some(intent.to.clone()),
            spender: Some(Account { owner: pool, subaccount: None }),
            amount: u128::from(intent.amount),
            fee: Some(7),
            created_at_time: Some(intent.created_at_time_ns),
            memo: Some(intent.memo.clone()),
        };
        assert!(exact_deposit_intent_block(&intent, pool, &block));

        let mut block_without_btype = block.clone();
        block_without_btype.btype = None;
        block_without_btype.op = "xfer".to_string();
        assert!(exact_deposit_intent_block(&intent, pool, &block_without_btype));

        let mut changed = block.clone();
        changed.amount += 1;
        assert!(!exact_deposit_intent_block(&intent, pool, &changed));
        let mut changed = block.clone();
        changed.memo = Some(b"different-sequence".to_vec());
        assert!(!exact_deposit_intent_block(&intent, pool, &changed));
        let mut changed = block.clone();
        changed.spender = Some(Account { owner: principal(9), subaccount: None });
        assert!(!exact_deposit_intent_block(&intent, pool, &changed));
        let mut changed = block;
        changed.created_at_time = Some(intent.created_at_time_ns + 1);
        assert!(!exact_deposit_intent_block(&intent, pool, &changed));
    }

    #[test]
    fn deposit_intent_archive_parent_hash_requires_a_well_formed_link() {
        let mut block = BTreeMap::new();
        assert!(deposit_intent_parent_hash(&ICRC3Value::Map(block.clone()), 7).is_err());
        block.insert("phash".into(), ICRC3Value::Blob(vec![1; 31].into()));
        assert!(deposit_intent_parent_hash(&ICRC3Value::Map(block.clone()), 7).is_err());
        block.insert("phash".into(), ICRC3Value::Blob(vec![1; 32].into()));
        assert_eq!(deposit_intent_parent_hash(&ICRC3Value::Map(block), 7).unwrap(), Some([1; 32]));
    }

    #[test]
    fn deposit_intent_memo_is_bounded_and_binds_the_pinned_payload() {
        let caller = principal(1);
        let ledger = principal(2);
        let memo = deposit_intent_memo(caller, 4, ledger, 900);
        assert_eq!(memo.len(), 32);
        assert!(memo.starts_with(DEPOSIT_INTENT_MEMO_PREFIX));
        assert_eq!(memo, deposit_intent_memo(caller, 4, ledger, 900));
        assert_ne!(memo, deposit_intent_memo(caller, 5, ledger, 900));
        assert_ne!(memo, deposit_intent_memo(caller, 4, ledger, 901));
        assert_ne!(memo, deposit_intent_memo(principal(9), 4, ledger, 900));
    }

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
            burn_receipt_nat: Some(candid::Nat::from(44u8)),
            legacy_ambiguous_burn: None,
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
    fn claim_all_reports_total_failure_but_keeps_partial_successes() {
        let error = StabilityPoolError::LedgerTransferFailed {
            reason: "pending transfer remains unresolved".into(),
        };
        assert!(matches!(finish_claim_all(BTreeMap::new(), None), Ok(result) if result.is_empty()));
        assert!(matches!(finish_claim_all(BTreeMap::new(), Some(error.clone())), Err(StabilityPoolError::LedgerTransferFailed { .. })));

        let mut partial = BTreeMap::new();
        partial.insert(principal(8), 123);
        assert!(matches!(finish_claim_all(partial.clone(), Some(error)), Ok(result) if result == partial));
    }

    #[test]
    fn completion_appends_permanent_receipt_when_cache_trims_new_low_key() {
        let caller = principal(1);
        let ledger = principal(11);
        let cached_owner = principal(2);
        let cached_ledger = principal(12);
        let cached_recipient = Account {
            owner: cached_owner,
            subaccount: None,
        };
        let mut state = crate::state::StabilityPoolState::default();
        let completed_cache = state.completed_outbound_payouts.as_mut().unwrap();
        for timestamp in 1..=crate::state::MAX_RECENT_OUTBOUND_PAYOUTS as u64 {
            let payout = PendingOutboundPayout {
                gross_amount: 3,
                transfer_amount: 2,
                transfer_fee: 1,
                recipient: cached_recipient,
                from_subaccount: None,
                transfer_memo: vec![2],
                transfer_created_at_time_ns: timestamp,
                dispatch_in_flight: false,
                ambiguous_seen: false,
                last_error: None,
                dispatch_generation: 1,
                reconciliation_attempts: 0,
                last_reconciliation_at_ns: None,
                candidate_block_index: None,
                candidate_block_index_raw: None,
            };
            completed_cache.insert(
                (cached_owner, cached_ledger, timestamp),
                CompletedOutboundPayout {
                    payout,
                    block_index: timestamp,
                    completed_at_ns: timestamp,
                },
            );
        }

        let mut position = DepositPosition::new(1);
        position.collateral_gains.insert(ledger, 1_000);
        state.deposits.insert(caller, position);
        let pending = state
            .prepare_collateral_payout(
                caller,
                ledger,
                100,
                Account {
                    owner: caller,
                    subaccount: None,
                },
                500,
                vec![1],
            )
            .unwrap()
            .unwrap();
        let timestamp = pending.transfer_created_at_time_ns;
        crate::receipt_store::init_layout();
        crate::state::replace_state(state);

        assert!(complete_outbound_payout_and_append(caller, ledger, &pending, 700, 800));

        let permanent = crate::receipt_store::get(caller, timestamp)
            .expect("permanent receipt decodes")
            .expect("permanent journal retains receipt evicted from compatibility cache");
        assert_eq!(permanent.ledger, ledger);
        assert_eq!(permanent.receipt.payout.transfer_created_at_time_ns, timestamp);
        assert_eq!(permanent.receipt.block_index, 700);
        assert_eq!(permanent.receipt.completed_at_ns, 800);
        read_state(|state| {
            assert!(state.pending_outbound_payout(&caller, &ledger).is_none());
            assert_eq!(state.completed_outbound_payouts.as_ref().unwrap().len(), crate::state::MAX_RECENT_OUTBOUND_PAYOUTS);
            assert!(!state.completed_outbound_payouts.as_ref().unwrap().contains_key(&(caller, ledger, timestamp)));
            assert_eq!(state.deposits.get(&caller).unwrap().total_claimed_gains.get(&ledger), Some(&1_000));
            assert_eq!(state.pool_events().len(), 1);
        });
    }

    #[test]
    fn exact_receipt_replay_is_idempotent_only_after_pending_row_is_gone() {
        let owner = principal(7);
        let ledger = principal(8);
        let payout = PendingOutboundPayout {
            gross_amount: 120,
            transfer_amount: 100,
            transfer_fee: 20,
            recipient: Account { owner, subaccount: None },
            from_subaccount: None,
            transfer_memo: vec![1, 2, 3],
            transfer_created_at_time_ns: 10,
            dispatch_in_flight: false,
            ambiguous_seen: false,
            last_error: None,
            dispatch_generation: 1,
            reconciliation_attempts: 0,
            last_reconciliation_at_ns: None,
            candidate_block_index: Some(110),
            candidate_block_index_raw: Some("110".into()),
        };
        let existing = crate::receipt_store::StoredReceipt {
            ledger,
            receipt: CompletedOutboundPayout {
                payout: payout.clone(),
                block_index: 110,
                completed_at_ns: 210,
            },
        };

        assert!(validate_existing_completion(&existing, ledger, &payout, 110, false).is_ok());
        assert!(validate_existing_completion(&existing, ledger, &payout, 110, true).is_err());
        assert!(validate_existing_completion(&existing, ledger, &payout, 111, false).is_err());
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
