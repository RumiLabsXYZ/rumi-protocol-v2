use candid::{CandidType, Deserialize};
use ic_canister_log::log;

use crate::history::{self, LiquidationRecordV1, LiquidationRecordVersioned, LiquidationStatus};
use crate::state::{self, BotConfig};
use crate::swap;

const CONFIRM_ATTEMPTS: u8 = 5;
const CANCEL_ATTEMPTS: u8 = 3;

/// Max number of times the bot will re-attempt `bot_claim_liquidation` for a
/// single vault before giving up and letting the cascade escalate to the SP.
/// Each retry costs ~30s of bot processing time. Three is enough to ride out
/// the scan→claim oracle TOCTOU window in most cases without burning the full
/// 300s cascade-timeout budget on a single uncooperative vault.
pub const CLAIM_RETRY_LIMIT: u8 = 3;

/// Decision tree for what to do after `bot_claim_liquidation` returns Err.
/// `current_count` is the number of attempts already made (0 = first try just
/// failed). Returns `GiveUp` when the limit has been reached, otherwise
/// `Retry` with the incremented count to persist.
#[derive(Debug, PartialEq)]
pub(crate) enum ClaimRetryAction {
    Retry { new_count: u8 },
    GiveUp,
}

pub(crate) fn next_claim_retry_action(current_count: u8, max: u8) -> ClaimRetryAction {
    let attempted = current_count.saturating_add(1);
    if attempted >= max {
        ClaimRetryAction::GiveUp
    } else {
        ClaimRetryAction::Retry { new_count: attempted }
    }
}

fn required_ckusdc_gross(debt_e8s: u64, fee_e6: u64) -> u64 {
    let net = debt_e8s / 100 + u64::from(debt_e8s % 100 != 0);
    net.saturating_add(fee_e6)
}

fn required_ckusdc_net(debt_e8s: u64) -> u64 {
    debt_e8s / 100 + u64::from(debt_e8s % 100 != 0)
}

fn payment_receipt_is_short(
    journal: &state::BotPaymentJournal,
    receipt: &swap::TransferReceipt,
) -> bool {
    receipt.amount < required_ckusdc_net(journal.debt_covered_e8s)
}

async fn choose_bounded_topup_amount(
    config: &BotConfig,
    remaining_icp: u64,
    shortfall_e6: u64,
) -> Result<Option<u64>, String> {
    if remaining_icp == 0 || shortfall_e6 == 0 {
        return Ok(None);
    }
    let conservative_output = |quoted: u64| {
        (quoted as u128)
            .saturating_mul(10_000u128.saturating_sub(config.max_slippage_bps as u128))
            .checked_div(10_000)
            .unwrap_or(0)
    };
    let mut low = 1u64;
    let mut high = remaining_icp;
    // At most 16 bounded quote calls. If the remaining collateral cannot
    // conservatively cover the shortfall, do not spend it on a futile swap.
    for _ in 0..16 {
        if low >= high {
            break;
        }
        let mid = low + (high - low) / 2;
        let quoted = swap::quote_icp_for_ckusdc(config, mid).await?;
        if conservative_output(quoted) >= u128::from(shortfall_e6) {
            high = mid;
        } else {
            low = mid.saturating_add(1);
        }
    }
    let quoted = swap::quote_icp_for_ckusdc(config, low).await?;
    if conservative_output(quoted) >= u128::from(shortfall_e6) {
        Ok(Some(low))
    } else {
        Ok(None)
    }
}

fn write_short_payment_recovery(
    id: u64,
    vault_id: u64,
    timestamp: u64,
    collateral_amount: u64,
    debt_covered: u64,
    swap_amount: u64,
    ckusdc_received: u64,
    collateral_price: u64,
    effective_price: u64,
    slippage_bps: i32,
    reason: &str,
) {
    state::mutate_state(|s| {
        if let Some(journal) = s.pending_claims.get_mut(&vault_id) {
            journal.status = state::BotClaimJournalStatus::PaymentShortfall;
        }
    });
    state::save_config_to_stable();
    write_record(LiquidationRecordV1 {
        id, vault_id, timestamp,
        status: LiquidationStatus::TransferFailed,
        collateral_claimed_e8s: collateral_amount,
        debt_to_cover_e8s: debt_covered,
        icp_swapped_e8s: swap_amount,
        ckusdc_received_e6: ckusdc_received,
        ckusdc_transferred_e6: 0,
        icp_to_treasury_e8s: 0,
        oracle_price_e8s: collateral_price,
        effective_price_e8s: effective_price,
        slippage_bps,
        error_message: Some(reason.to_string()),
        confirm_retry_count: 0,
    });
}

/// Per-claim ckUSDC accounting decision after a swap.
///
/// `process_pending` brackets the swap with two `balance_of_self_ckusdc`
/// reads. The delta between them is the ckUSDC this specific liquidation
/// actually deposited in the bot's wallet — independent of any pre-existing
/// balance left over from earlier runs, and independent of whatever the
/// swap router *claims* it deposited.
///
/// Why bother: the bot's wallet is shared state across runs. If a swap
/// router reports "4.22 ckUSDC out" but the wallet only gained 0.37 ckUSDC
/// (pool delivery delay, partial fill, accounting bug, future concurrency
/// hazard), the bot must not transfer 4.22 — that hits InsufficientFunds
/// and leaves the vault stuck pending admin resolution. With per-claim
/// reservation, the bot only spends what this claim actually earned.
///
/// `to_transfer_e6` is the amount that will be passed to
/// `transfer_ckusdc_to_backend` (caller still subtracts the ledger fee).
/// `recorded_received_e6` is the truth value stored in LiquidationRecord
/// (= actual delta, not the router's claim).
#[derive(Debug, PartialEq)]
pub(crate) struct SwapReservation {
    pub to_transfer_e6: u64,
    pub recorded_received_e6: u64,
    /// When the router's claimed output exceeds the wallet's actual delta,
    /// `discrepancy_note` carries a human-readable explanation that gets
    /// appended to the LiquidationRecord's `error_message`. None = clean.
    pub discrepancy_note: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct ClaimPaymentAllocation {
    gross_to_transfer_e6: u64,
    held_surplus_e6: u64,
}

fn allocate_claim_payment(reservation: &SwapReservation, required_gross_e6: u64) -> ClaimPaymentAllocation {
    let gross_to_transfer_e6 = reservation.to_transfer_e6.min(required_gross_e6);
    ClaimPaymentAllocation {
        gross_to_transfer_e6,
        held_surplus_e6: reservation
            .recorded_received_e6
            .saturating_sub(gross_to_transfer_e6),
    }
}

/// Apply post-confirmation totals exactly once and consume their journal in
/// the same stable-state mutation. The journal identity protects against a
/// stale completion for a later claim on the same vault.
fn apply_confirmed_payment_totals_once(
    bot_state: &mut state::BotState,
    journal: &state::BotPaymentJournal,
    transferred_amount_e6: u64,
) -> bool {
    let matches_pending = bot_state
        .pending_payments
        .get(&journal.vault_id)
        .is_some_and(|pending| {
            pending.claim_generation == journal.claim_generation
                && pending.ledger_principal == journal.ledger_principal
                && pending.created_at_time == journal.created_at_time
                && pending.memo == journal.memo
        });
    if !matches_pending {
        return false;
    }

    bot_state.stats.total_debt_covered_e8s = bot_state
        .stats
        .total_debt_covered_e8s
        .saturating_add(journal.debt_covered_e8s);
    bot_state.stats.total_ckusdc_deposited_e6 = bot_state
        .stats
        .total_ckusdc_deposited_e6
        .saturating_add(transferred_amount_e6);
    bot_state.stats.total_ckusdc_surplus_held_e6 = bot_state
        .stats
        .total_ckusdc_surplus_held_e6
        .saturating_add(journal.held_surplus_e6);
    bot_state.stats.total_collateral_received_e8s = bot_state
        .stats
        .total_collateral_received_e8s
        .saturating_add(journal.collateral_amount_e8s);
    bot_state.stats.events_count = bot_state.stats.events_count.saturating_add(1);
    bot_state.pending_payments.remove(&journal.vault_id);
    true
}

/// Compute the per-claim ckUSDC reservation from balance snapshots.
///
/// `router_received_e6` is whatever ICPSwap's `depositFromAndSwap` told us.
/// `balance_before_e6` / `balance_after_e6` are wallet balance reads
/// bracketing the swap call.
///
/// The router's number is treated as advisory. The truth is `after - before`.
/// If the truth is lower we use the truth and surface the gap to the record
/// (operator visibility); if the truth is higher (someone else funded the
/// wallet mid-flight) we use the router's number to avoid accidentally
/// spending funds that weren't earmarked for this claim.
pub(crate) fn compute_swap_reservation(
    vault_id: u64,
    router_received_e6: u64,
    balance_before_e6: u64,
    balance_after_e6: u64,
) -> SwapReservation {
    let actual_delta = balance_after_e6.saturating_sub(balance_before_e6);

    // Truth-low case: pool didn't deliver in full. Use the delta and
    // record the gap so the operator can see why the per-claim debt
    // coverage came up short.
    if actual_delta < router_received_e6 {
        let gap = router_received_e6 - actual_delta;
        let note = format!(
            "per-claim reservation: router reported {} ckUSDC for vault #{}, \
             wallet delta only {} (gap {}); using delta to avoid InsufficientFunds.",
            router_received_e6, vault_id, actual_delta, gap
        );
        return SwapReservation {
            to_transfer_e6: actual_delta,
            recorded_received_e6: actual_delta,
            discrepancy_note: Some(note),
        };
    }

    // Truth-high case: wallet gained more than the router claimed. Could be
    // a stale prior swap finally arriving, or an unrelated deposit. Either
    // way, do NOT spend the surplus on this claim — only ours to spend is
    // what the router said was ours.
    SwapReservation {
        to_transfer_e6: router_received_e6,
        recorded_received_e6: router_received_e6,
        discrepancy_note: None,
    }
}

/// Outcome of the swap-failure cleanup path. Pure data, produced by
/// `decide_swap_failure_outcome` and consumed by `process_pending` to write
/// the LiquidationRecord and emit the STUCK log line if applicable.
#[derive(Debug, PartialEq)]
pub(crate) struct SwapFailureOutcome {
    pub status: history::LiquidationStatus,
    pub error_message: String,
    /// Some(line) when the bot is leaving a claim active that the protocol
    /// must be reconciled through its exact payment or collateral-return proof.
    /// None when both cleanup steps succeeded.
    pub stuck_log: Option<String>,
}

/// Wave 13 (BOT-002): decide which `LiquidationStatus` and `error_message`
/// to record after a swap failure, given the outcomes of the return-collateral
/// transfer and the cancel-claim retry loop.
///
/// Before Wave 13 the bot ignored both call results with `let _ = ...` and
/// always wrote `SwapFailed`, even when the protocol's claim was still active
/// (budget unrestored, vault still flagged). With the Wave-12 BOT-001b balance
/// gate a failed return guarantees the cancel rejects, so the bot record must
/// reflect that the claim is stuck pending proof-backed reconciliation.
///
/// Status mapping reuses existing variants (no `.did` change):
///   * Both cleanup steps OK     -> SwapFailed (happy cleanup)
///   * Return failed             -> TransferFailed (bot couldn't return ICP)
///   * Return OK, cancel stuck   -> ConfirmFailed (protocol-side bookkeeping stuck)
///
/// `return_err` takes priority over `cancel_err` defensively: when the return
/// fails the integration never attempts cancel, so cancel_err should be None,
/// but the helper still picks deterministically if both are somehow set.
pub(crate) fn decide_swap_failure_outcome(
    vault_id: u64,
    swap_err: &str,
    return_err: Option<&str>,
    cancel_err: Option<(u8, &str)>,
) -> SwapFailureOutcome {
    if let Some(rerr) = return_err {
        return SwapFailureOutcome {
            status: history::LiquidationStatus::TransferFailed,
            error_message: format!("swap: {} | return: {}", swap_err, rerr),
            stuck_log: Some(format!(
                "STUCK: ICP return failed after swap failure for vault #{}; claim still active. Retry/reconcile the exact claim-bound return or payment proof; proofless legacy admin recovery is disabled.",
                vault_id
            )),
        };
    }

    if let Some((attempts, cerr)) = cancel_err {
        return SwapFailureOutcome {
            status: history::LiquidationStatus::ConfirmFailed,
            error_message: format!(
                "swap: {} | cancel after {} retries: {}",
                swap_err, attempts, cerr
            ),
            stuck_log: Some(format!(
                "STUCK: cancel failed after {} attempts for vault #{}; ICP returned but claim still active. Reconcile its exact collateral-return proof and retry proof-backed cancellation.",
                attempts, vault_id
            )),
        };
    }

    SwapFailureOutcome {
        status: history::LiquidationStatus::SwapFailed,
        error_message: swap_err.to_string(),
        stuck_log: None,
    }
}

/// Result returned by the backend's `bot_claim_liquidation` endpoint.
#[derive(CandidType, Deserialize, Debug)]
pub struct BotLiquidationResult {
    pub vault_id: u64,
    pub collateral_amount: u64,
    pub debt_covered: u64,
    pub collateral_price_e8s: u64,
    pub claim_generation: u64,
    pub payment_memo: Vec<u8>,
    pub collateral_return_memo: Vec<u8>,
    pub payment_ledger_principal: Option<candid::Principal>,
}

#[derive(CandidType, Deserialize, Debug)]
pub struct BotPaymentProof {
    pub vault_id: u64,
    pub claim_generation: u64,
    pub ledger_principal: candid::Principal,
    pub block_index: u64,
    pub amount_e6s: u64,
    pub created_at_time: u64,
}

#[derive(CandidType, Deserialize, Debug)]
pub struct BotCollateralReturnProofArg {
    pub vault_id: u64,
    pub claim_generation: u64,
    pub block_index: u64,
    pub amount: u64,
    pub created_at_time: u64,
}

#[derive(CandidType, Deserialize, Debug)]
pub enum BackendResult<T> {
    #[serde(rename = "Ok")]
    Ok(T),
    #[serde(rename = "Err")]
    Err(BackendError),
}

#[derive(CandidType, Deserialize, Debug)]
pub enum BackendError {
    GenericError(String),
    TemporarilyUnavailable(String),
    AnonymousCallerNotAllowed,
    AmountTooLow { minimum_amount: u64 },
    InsufficientFunds { balance: u64 },
    VaultNotFound { vault_id: u64 },
    TransferError(String),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

pub async fn process_pending() {
    if state::read_state(|s| s.processing_paused) {
        return;
    }
    let _guard = match crate::ProcessingGuard::acquire() {
        Ok(g) => g,
        Err(_) => return, // Another liquidation is already in flight
    };

    let mut config = match state::read_state(|s| s.config.clone()) {
        Some(c) => c,
        None => {
            log!(crate::INFO, "Bot not configured; skipping pending work");
            return;
        }
    };

    if resume_pending_payment(&config).await {
        return;
    }
    if resume_pending_return(&config).await {
        return;
    }
    if let Some((vault_id, phase)) = state::read_state(|s| {
        s.pending_claims.iter().next().map(|(id, claim)| (*id, claim.status.clone()))
    }) {
        log!(crate::INFO, "STUCK: claim #{} is held in {:?}; refuse to replay a swap or transfer without operator reconciliation", vault_id, phase);
        return;
    }

    let vault = state::mutate_state(|s| s.pending_vaults.pop());
    let Some(vault) = vault else { return };

    let prior_retry_count = state::read_state(|s| {
        s.claim_retry_counts.get(&vault.vault_id).copied().unwrap_or(0)
    });
    log!(
        crate::INFO,
        "Processing vault #{} (attempt {}/{})",
        vault.vault_id,
        prior_retry_count + 1,
        CLAIM_RETRY_LIMIT
    );
    let record_id = history::next_id();
    let timestamp = ic_cdk::api::time();

    // -- Phase 1: CLAIM --
    let liq_result = call_bot_claim_liquidation(&config, vault.vault_id).await;
    let (collateral_amount, debt_covered, collateral_price, claim_generation, payment_memo, return_memo) = match liq_result {
        Ok(r) => {
            state::mutate_state(|s| {
                s.claim_retry_counts.remove(&vault.vault_id);
                s.pending_claims.insert(vault.vault_id, state::BotClaimJournal {
                    vault_id: r.vault_id,
                    claim_generation: r.claim_generation,
                    debt_covered_e8s: r.debt_covered,
                    collateral_amount_e8s: r.collateral_amount,
                    collateral_price_e8s: r.collateral_price_e8s,
                    payment_memo: r.payment_memo.clone(),
                    collateral_return_memo: r.collateral_return_memo.clone(),
                    collateral_return: None,
                    status: state::BotClaimJournalStatus::SwapMayHaveStarted,
                });
            });
            state::save_config_to_stable();
            let Some(payment_ledger) = r.payment_ledger_principal else {
                write_short_payment_recovery(
                    record_id, r.vault_id, timestamp,
                    r.collateral_amount, r.debt_covered, 0, 0,
                    r.collateral_price_e8s, 0, 0,
                    "claim has no pinned ckUSDC ledger; operator reconciliation required",
                );
                return;
            };
            config.ckusdc_ledger = payment_ledger;
            (r.collateral_amount, r.debt_covered, r.collateral_price_e8s, r.claim_generation, r.payment_memo, r.collateral_return_memo)
        }
        Err(e) => {
            let action = next_claim_retry_action(prior_retry_count, CLAIM_RETRY_LIMIT);
            let (status, message) = match &action {
                ClaimRetryAction::Retry { new_count } => (
                    LiquidationStatus::ClaimFailed,
                    format!(
                        "Attempt {}/{}: {}",
                        new_count, CLAIM_RETRY_LIMIT, e
                    ),
                ),
                ClaimRetryAction::GiveUp => (
                    LiquidationStatus::ClaimFailed,
                    format!(
                        "Final attempt {}/{} (giving up; cascade will escalate to SP): {}",
                        CLAIM_RETRY_LIMIT, CLAIM_RETRY_LIMIT, e
                    ),
                ),
            };
            log!(crate::INFO, "Claim failed for vault #{}: {}", vault.vault_id, message);
            write_record(LiquidationRecordV1 {
                id: record_id, vault_id: vault.vault_id, timestamp,
                status,
                collateral_claimed_e8s: 0, debt_to_cover_e8s: 0, icp_swapped_e8s: 0,
                ckusdc_received_e6: 0, ckusdc_transferred_e6: 0, icp_to_treasury_e8s: 0,
                oracle_price_e8s: 0, effective_price_e8s: 0, slippage_bps: 0,
                error_message: Some(message), confirm_retry_count: 0,
            });
            match action {
                ClaimRetryAction::Retry { new_count } => {
                    let vault_id = vault.vault_id;
                    state::mutate_state(|s| {
                        s.claim_retry_counts.insert(vault_id, new_count);
                        // Re-queue at the front of the Vec so other pending
                        // vaults pop first (LIFO); skip if a concurrent
                        // `notify_liquidatable_vaults` already re-added us.
                        if !s.pending_vaults.iter().any(|v| v.vault_id == vault_id) {
                            s.pending_vaults.insert(0, vault);
                        }
                    });
                }
                ClaimRetryAction::GiveUp => {
                    state::mutate_state(|s| {
                        s.claim_retry_counts.remove(&vault.vault_id);
                    });
                }
            }
            return;
        }
    };

    // -- Phase 2: SWAP ICP -> ckUSDC --
    let mut swap_amount = calculate_swap_amount(collateral_amount, debt_covered, collateral_price);

    // Per-claim reservation: bracket the swap with wallet balance reads so
    // we know the EXACT ckUSDC this claim earned, independent of any leftover
    // balance or what the swap router claims. The transfer in Phase 3 spends
    // only this delta (see compute_swap_reservation for rationale).
    //
    // If the pre-swap balance read fails we proceed without bracketing —
    // worse than capped, but better than skipping the swap entirely.
    let bal_before_swap = swap::balance_of_self_ckusdc(&config).await.unwrap_or_else(|e| {
        log!(crate::INFO, "Pre-swap balance read failed for vault #{}: {} (proceeding without per-claim cap)", vault.vault_id, e);
        u64::MAX
    });

    let swap_result = swap::swap_icp_for_ckusdc(&config, swap_amount).await;

    let (router_received, effective_price) = match swap_result {
        Ok(r) => (r.ckusdc_received_e6, r.effective_price_e8s),
        Err(swap_err) => {
            log!(crate::INFO, "Swap failed for vault #{}: {}. Returning ICP.", vault.vault_id, swap_err);

            state::mutate_state(|s| {
                if let Some(journal) = s.pending_claims.get_mut(&vault.vault_id) {
                    journal.status = state::BotClaimJournalStatus::ReturnPending;
                }
            });
            state::save_config_to_stable();

            // Step 1: return seized ICP to the backend.
            let return_fee = config.icp_fee_e8s.unwrap_or(10_000);
            let return_amount = collateral_amount.saturating_sub(return_fee);
            let return_created_at_time = ic_cdk::api::time();
            state::mutate_state(|s| {
                if let Some(journal) = s.pending_claims.get_mut(&vault.vault_id) {
                    journal.collateral_return = Some(state::BotReturnTransferJournal {
                        ledger_principal: config.icp_ledger,
                        backend_principal: config.backend_principal,
                        amount_e8s: return_amount,
                        fee_e8s: return_fee,
                        memo: return_memo.clone(),
                        created_at_time: return_created_at_time,
                        receipt: None,
                        status: state::BotReturnTransferStatus::Prepared,
                    });
                }
            });
            state::save_config_to_stable();
            let return_result = swap::return_collateral_to_backend(
                &config,
                collateral_amount,
                config.icp_ledger,
                return_memo.clone(),
                return_amount,
                return_created_at_time,
            )
            .await;

            // Step 2: cancel the protocol-side claim, only if the return succeeded.
            // The Wave-12 BOT-001b balance gate rejects cancel until the protocol's
            // collateral balance is back to (>=) `claim.collateral_amount - fee`,
            // so attempting cancel after a failed return is pointless and would
            // just produce noisy `[BOT-001b] cancel rejected` log lines.
            let return_proof_result = match return_result {
                Ok(receipt) => {
                    state::mutate_state(|s| {
                        if let Some(journal) = s.pending_claims.get_mut(&vault.vault_id) {
                            if let Some(return_intent) = journal.collateral_return.as_mut() {
                                return_intent.receipt = Some(receipt.clone());
                            }
                        }
                    });
                    state::save_config_to_stable();
                    call_bot_record_collateral_return_proof(
                        &config,
                        BotCollateralReturnProofArg {
                            vault_id: vault.vault_id,
                            claim_generation,
                            block_index: receipt.block_index,
                            amount: receipt.amount,
                            created_at_time: receipt.created_at_time,
                        },
                    ).await
                }
                Err(e) => {
                    state::mutate_state(|s| {
                        if let Some(intent) = s.pending_claims.get_mut(&vault.vault_id)
                            .and_then(|claim| claim.collateral_return.as_mut()) {
                            intent.status = match &e {
                                swap::TransferAttemptError::NoEffect(_) => state::BotReturnTransferStatus::NoEffect,
                                swap::TransferAttemptError::Ambiguous(_) => state::BotReturnTransferStatus::Ambiguous,
                            };
                        }
                    });
                    state::save_config_to_stable();
                    Err(format!("{:?}", e))
                },
            };
            let cancel_err = if return_proof_result.is_ok() {
                let mut last_err = String::new();
                let mut succeeded = false;
                let mut attempts: u8 = 0;
                for attempt in 0..CANCEL_ATTEMPTS {
                    attempts = attempt + 1;
                    match call_bot_cancel_liquidation(&config, vault.vault_id).await {
                        Ok(()) => {
                            succeeded = true;
                            break;
                        }
                        Err(e) => {
                            last_err = e;
                            if attempt + 1 < CANCEL_ATTEMPTS {
                                log!(
                                    crate::INFO,
                                    "Cancel attempt {}/{} failed for vault #{}: {}. Retrying.",
                                    attempt + 1,
                                    CANCEL_ATTEMPTS,
                                    vault.vault_id,
                                    last_err
                                );
                            }
                        }
                    }
                }
                if succeeded { None } else { Some((attempts, last_err)) }
            } else {
                None
            };

            let outcome = decide_swap_failure_outcome(
                vault.vault_id,
                &swap_err,
                return_proof_result.as_ref().err().map(|s| s.as_str()),
                cancel_err.as_ref().map(|(n, e)| (*n, e.as_str())),
            );

            if let Some(line) = &outcome.stuck_log {
                log!(crate::INFO, "{}", line);
            }
            if return_proof_result.is_ok() && cancel_err.is_none() {
                state::mutate_state(|s| { s.pending_claims.remove(&vault.vault_id); });
                state::save_config_to_stable();
            }

            write_record(LiquidationRecordV1 {
                id: record_id, vault_id: vault.vault_id, timestamp,
                status: outcome.status,
                collateral_claimed_e8s: collateral_amount, debt_to_cover_e8s: debt_covered,
                icp_swapped_e8s: swap_amount, ckusdc_received_e6: 0, ckusdc_transferred_e6: 0,
                icp_to_treasury_e8s: 0, oracle_price_e8s: collateral_price,
                effective_price_e8s: 0, slippage_bps: 0,
                error_message: Some(outcome.error_message), confirm_retry_count: 0,
            });
            return;
        }
    };

    let slippage_bps = calculate_slippage(effective_price, collateral_price);

    // Per-claim reservation: read wallet balance again, compute delta, and use
    // that — not the router's claim — as the amount this claim is allowed to
    // spend. See compute_swap_reservation for the decision rules. When the
    // pre-swap balance read failed we set it to u64::MAX, which makes the
    // delta saturate to 0 and falls back to the router's number unchanged.
    let bal_after_swap = swap::balance_of_self_ckusdc(&config).await.unwrap_or_else(|e| {
        log!(crate::INFO, "Post-swap balance read failed for vault #{}: {} (using router-reported amount)", vault.vault_id, e);
        bal_before_swap.saturating_add(router_received) // synthesise delta = router_received
    });
    let mut reservation = compute_swap_reservation(
        vault.vault_id,
        router_received,
        bal_before_swap,
        bal_after_swap,
    );
    if let Some(note) = &reservation.discrepancy_note {
        log!(crate::INFO, "[per-claim-reservation] {}", note);
    }
    let ckusdc_fee = config.ckusdc_fee_e6.unwrap_or(10_000);
    let required_gross = required_ckusdc_gross(debt_covered, ckusdc_fee);
    if reservation.to_transfer_e6 < required_gross {
        let shortfall = required_gross - reservation.to_transfer_e6;
        let remaining_icp = collateral_amount.saturating_sub(swap_amount);
        match choose_bounded_topup_amount(&config, remaining_icp, shortfall).await {
            Ok(Some(topup_icp)) => {
                let before = match swap::balance_of_self_ckusdc(&config).await {
                    Ok(balance) => balance,
                    Err(error) => {
                        log!(crate::INFO, "STUCK: pre-top-up ckUSDC balance unavailable for vault #{}: {}", vault.vault_id, error);
                        write_short_payment_recovery(record_id, vault.vault_id, timestamp, collateral_amount, debt_covered, swap_amount, reservation.recorded_received_e6, collateral_price, effective_price, slippage_bps, "pre-top-up balance unavailable");
                        return;
                    }
                };
                match swap::swap_icp_for_ckusdc(&config, topup_icp).await {
                    Ok(extra) => {
                        let after = match swap::balance_of_self_ckusdc(&config).await {
                            Ok(balance) => balance,
                            Err(error) => {
                                log!(crate::INFO, "STUCK: post-top-up ckUSDC balance unavailable for vault #{}: {}", vault.vault_id, error);
                                write_short_payment_recovery(record_id, vault.vault_id, timestamp, collateral_amount, debt_covered, swap_amount.saturating_add(topup_icp), reservation.recorded_received_e6, collateral_price, effective_price, slippage_bps, "post-top-up balance unavailable");
                                return;
                            }
                        };
                        let extra_reservation = compute_swap_reservation(vault.vault_id, extra.ckusdc_received_e6, before, after);
                        reservation.to_transfer_e6 = reservation.to_transfer_e6.saturating_add(extra_reservation.to_transfer_e6);
                        reservation.recorded_received_e6 = reservation.recorded_received_e6.saturating_add(extra_reservation.recorded_received_e6);
                        swap_amount = swap_amount.saturating_add(topup_icp);
                    }
                    Err(error) => {
                        log!(crate::INFO, "STUCK: bounded short-payment top-up failed for vault #{}: {}", vault.vault_id, error);
                        write_short_payment_recovery(record_id, vault.vault_id, timestamp, collateral_amount, debt_covered, swap_amount, reservation.recorded_received_e6, collateral_price, effective_price, slippage_bps, &format!("bounded top-up failed: {error}"));
                        return;
                    }
                }
            }
            Ok(None) => {}
            Err(error) => {
                log!(crate::INFO, "STUCK: cannot quote bounded short-payment top-up for vault #{}: {}", vault.vault_id, error);
                write_short_payment_recovery(record_id, vault.vault_id, timestamp, collateral_amount, debt_covered, swap_amount, reservation.recorded_received_e6, collateral_price, effective_price, slippage_bps, &format!("top-up quote unavailable: {error}"));
                return;
            }
        }
    }
    let payment_allocation = allocate_claim_payment(&reservation, required_gross);
    let ckusdc_received = reservation.recorded_received_e6;
    let min_ckusdc_net = required_gross.saturating_sub(ckusdc_fee);
    if reservation.to_transfer_e6 < required_gross {
        // Never transfer a short amount: that would credit the backend without
        // authorizing the debt write-down. The active claim and both asset
        // balances are left for explicit recovery instead.
        let message = format!(
            "ckUSDC output is short; no payment sent (available gross {}, required gross {}, net debt minimum {})",
            payment_allocation.gross_to_transfer_e6, required_gross, min_ckusdc_net
        );
        log!(crate::INFO, "STUCK: {} for vault #{}; claim remains active for recovery", message, vault.vault_id);
        write_short_payment_recovery(record_id, vault.vault_id, timestamp, collateral_amount, debt_covered, swap_amount, ckusdc_received, collateral_price, effective_price, slippage_bps, &message);
        return;
    }

    // -- Phase 3: TRANSFER ckUSDC to backend (NO RETRY) --
    // Send only the exact gross debt-plus-fee requirement. Favorable swap
    // overage remains in the bot and is accounted as held surplus; it cannot
    // increase the debt write-down or the treasury distribution.
    let created_at_time = ic_cdk::api::time();
    let payment_journal = state::BotPaymentJournal {
        vault_id: vault.vault_id,
        backend_principal: config.backend_principal,
        ledger_principal: config.ckusdc_ledger,
        claim_generation,
        debt_covered_e8s: debt_covered,
        collateral_amount_e8s: collateral_amount,
        collateral_price_e8s: collateral_price,
        icp_swapped_e8s: swap_amount,
        ckusdc_received_e6: ckusdc_received,
        held_surplus_e6: payment_allocation.held_surplus_e6,
        gross_amount_e6: payment_allocation.gross_to_transfer_e6,
        amount_e6: payment_allocation.gross_to_transfer_e6.saturating_sub(ckusdc_fee),
        fee_e6: ckusdc_fee,
        created_at_time,
        memo: payment_memo.clone(),
        status: state::BotPaymentStatus::Prepared,
        receipt: None,
        shortfall_receipt_observed: false,
    };
    state::mutate_state(|s| {
        s.pending_claims.remove(&vault.vault_id);
        s.pending_payments.insert(vault.vault_id, payment_journal.clone());
    });
    state::save_config_to_stable();

    let transfer_result = swap::transfer_ckusdc_to_backend(
        &config,
        payment_journal.amount_e6,
        payment_memo,
        created_at_time,
        ckusdc_fee,
    ).await;

    let ckusdc_transferred = match transfer_result {
        Ok(receipt) => {
            state::mutate_state(|s| {
                if let Some(journal) = s.pending_payments.get_mut(&vault.vault_id) {
                    journal.status = state::BotPaymentStatus::ReceiptObserved;
                    journal.receipt = Some(receipt.clone());
                }
            });
            state::save_config_to_stable();
            receipt
        }
        Err(e) => {
            let (status, message) = match e {
                swap::TransferAttemptError::NoEffect(message) => (state::BotPaymentStatus::NoEffect, message),
                swap::TransferAttemptError::Ambiguous(message) => (state::BotPaymentStatus::Ambiguous, message),
            };
            state::mutate_state(|s| {
                if let Some(journal) = s.pending_payments.get_mut(&vault.vault_id) {
                    journal.status = status;
                }
            });
            state::save_config_to_stable();
            let stuck_msg = match &reservation.discrepancy_note {
                Some(note) => format!("{} | transfer: {}", note, message),
                None => message,
            };
            log!(crate::INFO,
            "STUCK: ckUSDC transfer failed for vault #{}. Bot holding {} ckUSDC e6 for this claim, including {} held surplus (router said {}). Error: {}. Reconcile this claim's exact ckUSDC payment/transfer evidence; after a confirmed transfer, submit its ledger block via bot_confirm_liquidation_with_proof. The legacy boolean admin endpoint is disabled.",
                vault.vault_id, payment_allocation.gross_to_transfer_e6.saturating_add(payment_allocation.held_surplus_e6), payment_allocation.held_surplus_e6, router_received, stuck_msg);
            write_record(LiquidationRecordV1 {
                id: record_id, vault_id: vault.vault_id, timestamp,
                status: LiquidationStatus::TransferFailed,
                collateral_claimed_e8s: collateral_amount, debt_to_cover_e8s: debt_covered,
                icp_swapped_e8s: swap_amount, ckusdc_received_e6: ckusdc_received,
                ckusdc_transferred_e6: 0, icp_to_treasury_e8s: 0,
                oracle_price_e8s: collateral_price, effective_price_e8s: effective_price,
                slippage_bps, error_message: Some(stuck_msg), confirm_retry_count: 0,
            });
            return;
        }
    };

    if payment_receipt_is_short(&payment_journal, &ckusdc_transferred) {
        state::mutate_state(|s| {
            if let Some(journal) = s.pending_payments.get_mut(&vault.vault_id) {
                journal.shortfall_receipt_observed = true;
            }
        });
        state::save_config_to_stable();
        let minimum = required_ckusdc_net(debt_covered);
        let message = format!(
            "exact ckUSDC receipt is short (received {}, required {}); claim and receipt held pending a cumulative-payment recovery implementation",
            ckusdc_transferred.amount, minimum
        );
        log!(crate::INFO, "STUCK: {} for vault #{}; do not retry this proof or release the claim", message, vault.vault_id);
        write_record(LiquidationRecordV1 {
            id: record_id, vault_id: vault.vault_id, timestamp,
            status: LiquidationStatus::ConfirmFailed,
            collateral_claimed_e8s: collateral_amount, debt_to_cover_e8s: debt_covered,
            icp_swapped_e8s: swap_amount, ckusdc_received_e6: ckusdc_received,
            ckusdc_transferred_e6: ckusdc_transferred.amount, icp_to_treasury_e8s: 0,
            oracle_price_e8s: collateral_price, effective_price_e8s: effective_price,
            slippage_bps, error_message: Some(message), confirm_retry_count: 0,
        });
        return;
    }

    // -- Phase 4: CONFIRM (with retry, idempotent) --
    let mut confirm_ok = false;
    let mut confirm_retries: u8 = 0;
    let mut last_confirm_err = String::new();

    for attempt in 0..CONFIRM_ATTEMPTS {
        match call_bot_confirm_liquidation_with_proof(&config, BotPaymentProof {
            vault_id: vault.vault_id,
            claim_generation,
            ledger_principal: config.ckusdc_ledger,
            block_index: ckusdc_transferred.block_index,
            amount_e6s: ckusdc_transferred.amount,
            created_at_time: ckusdc_transferred.created_at_time,
        }).await {
            Ok(()) => {
                confirm_ok = true;
                confirm_retries = attempt + 1;
                break;
            }
            Err(e) => {
                last_confirm_err = e;
                confirm_retries = attempt + 1;
                if attempt + 1 < CONFIRM_ATTEMPTS {
                    log!(crate::INFO, "Confirm attempt {}/{} failed for vault #{}: {}. Retrying.",
                        attempt + 1, CONFIRM_ATTEMPTS, vault.vault_id, last_confirm_err);
                }
            }
        }
    }

    if !confirm_ok {
        log!(crate::INFO,
            "STUCK: Confirm failed after {} attempts for vault #{}. ckUSDC is in backend but debt not written down. Error: {}. Retry proof-backed confirmation with this claim's exact ckUSDC transfer ledger block via bot_confirm_liquidation_with_proof. The legacy boolean admin endpoint is disabled.",
            CONFIRM_ATTEMPTS, vault.vault_id, last_confirm_err);
        write_record(LiquidationRecordV1 {
            id: record_id, vault_id: vault.vault_id, timestamp,
            status: LiquidationStatus::ConfirmFailed,
            collateral_claimed_e8s: collateral_amount, debt_to_cover_e8s: debt_covered,
            icp_swapped_e8s: swap_amount, ckusdc_received_e6: ckusdc_received,
            ckusdc_transferred_e6: ckusdc_transferred.amount, icp_to_treasury_e8s: 0,
            oracle_price_e8s: collateral_price, effective_price_e8s: effective_price,
            slippage_bps, error_message: Some(last_confirm_err), confirm_retry_count: confirm_retries,
        });
        return;
    }

    state::mutate_state(|s| {
        if let Some(journal) = s.pending_payments.get_mut(&vault.vault_id) {
            journal.status = state::BotPaymentStatus::Confirmed;
        }
    });
    state::save_config_to_stable();

    // -- Phase 5: TREASURY (liquidation bonus) --
    let icp_to_treasury = collateral_amount.saturating_sub(swap_amount);
    if icp_to_treasury > 0 {
        let _ = swap::transfer_icp_to_treasury(&config, icp_to_treasury).await;
    }

    // -- Phase 6: SUCCESS --
    log!(crate::INFO, "Vault #{} liquidated: debt={} e8s, ckUSDC={} e6, treasury={} e8s ICP",
        vault.vault_id, debt_covered, ckusdc_received, icp_to_treasury);

    write_record(LiquidationRecordV1 {
        id: record_id, vault_id: vault.vault_id, timestamp,
        status: LiquidationStatus::Completed,
        collateral_claimed_e8s: collateral_amount, debt_to_cover_e8s: debt_covered,
        icp_swapped_e8s: swap_amount, ckusdc_received_e6: ckusdc_received,
        ckusdc_transferred_e6: ckusdc_transferred.amount, icp_to_treasury_e8s: icp_to_treasury,
        oracle_price_e8s: collateral_price, effective_price_e8s: effective_price,
        slippage_bps, error_message: None, confirm_retry_count: confirm_retries,
    });

    // Update legacy stats for backward compat with explorer UI
    state::mutate_state(|s| {
        if apply_confirmed_payment_totals_once(s, &payment_journal, ckusdc_transferred.amount) {
            s.stats.total_collateral_to_treasury_e8s = s
                .stats
                .total_collateral_to_treasury_e8s
                .saturating_add(icp_to_treasury);
        }
    });
    state::save_config_to_stable();
}

// -- Helpers --

fn write_record(record: LiquidationRecordV1) {
    history::insert_record(LiquidationRecordVersioned::V1(record));
}

/// Reconcile a payment intent before processing another vault. The intent is
/// written to stable memory before the ledger call; retries reuse its exact
/// amount, memo and created_at_time. A returned block is never paid again and
/// is handed to the backend's exact ICRC-3 verifier.
async fn resume_pending_payment(config: &BotConfig) -> bool {
    let Some(mut journal) = state::read_state(|s| s.pending_payments.values().next().cloned())
    else {
        return false;
    };
    if config.backend_principal != journal.backend_principal
        || config.ckusdc_ledger != journal.ledger_principal
    {
        log!(crate::INFO, "STUCK: payment journal for vault #{} is bound to different backend/ledger config; operator reconciliation required", journal.vault_id);
        return true;
    }

    if journal.status == state::BotPaymentStatus::NoEffect {
        log!(crate::INFO, "STUCK: payment for vault #{} has a definitive no-effect response; operator must correct the cause before allocating a new exact tuple", journal.vault_id);
        return true;
    }
    if journal.shortfall_receipt_observed {
        log!(crate::INFO, "STUCK: short payment receipt for vault #{} remains held; cumulative claim-generation proof support is not available", journal.vault_id);
        return true;
    }

    if journal.receipt.is_none() {
        const SAFE_RETRY_WINDOW_NS: u64 = 23 * 60 * 60 * 1_000_000_000;
        if ic_cdk::api::time().saturating_sub(journal.created_at_time) >= SAFE_RETRY_WINDOW_NS {
            log!(crate::INFO, "STUCK: payment intent for vault #{} exceeded the safe dedup retry window; do not re-dispatch, reconcile by exact ICRC-3 history", journal.vault_id);
            return true;
        }
        match swap::transfer_ckusdc_to_backend(
            config,
            journal.amount_e6,
            journal.memo.clone(),
            journal.created_at_time,
            journal.fee_e6,
        )
        .await
        {
            Ok(receipt) => {
                journal.status = state::BotPaymentStatus::ReceiptObserved;
                journal.receipt = Some(receipt);
            }
            Err(swap::TransferAttemptError::NoEffect(message)) => {
                journal.status = state::BotPaymentStatus::NoEffect;
                log!(crate::INFO, "Payment retry had definitive no-effect for vault #{}: {}", journal.vault_id, message);
                state::mutate_state(|s| { s.pending_payments.insert(journal.vault_id, journal.clone()); });
                state::save_config_to_stable();
                return true;
            }
            Err(swap::TransferAttemptError::Ambiguous(message)) => {
                journal.status = state::BotPaymentStatus::Ambiguous;
                log!(crate::INFO, "Payment retry outcome remains ambiguous for vault #{}: {}", journal.vault_id, message);
                state::mutate_state(|s| { s.pending_payments.insert(journal.vault_id, journal.clone()); });
                state::save_config_to_stable();
                return true;
            }
        }
        state::mutate_state(|s| { s.pending_payments.insert(journal.vault_id, journal.clone()); });
        state::save_config_to_stable();
    }

    let Some(receipt) = journal.receipt.as_ref() else { return true };
    if payment_receipt_is_short(&journal, receipt) {
        journal.shortfall_receipt_observed = true;
        state::mutate_state(|s| { s.pending_payments.insert(journal.vault_id, journal.clone()); });
        state::save_config_to_stable();
        log!(crate::INFO, "STUCK: exact ckUSDC block {} for vault #{} is short ({} received, {} required); claim remains held pending cumulative-payment recovery support", receipt.block_index, journal.vault_id, receipt.amount, required_ckusdc_net(journal.debt_covered_e8s));
        return true;
    }
    let proof = BotPaymentProof {
        vault_id: journal.vault_id,
        claim_generation: journal.claim_generation,
        ledger_principal: journal.ledger_principal,
        block_index: receipt.block_index,
        amount_e6s: receipt.amount,
        created_at_time: receipt.created_at_time,
    };
    match call_bot_confirm_liquidation_with_proof(config, proof).await {
        Ok(()) => {
            journal.status = state::BotPaymentStatus::Confirmed;
            state::mutate_state(|s| {
                apply_confirmed_payment_totals_once(s, &journal, receipt.amount);
            });
            state::save_config_to_stable();
            log!(crate::INFO, "Recovered and confirmed payment for vault #{} from exact ckUSDC block {}; any unswapped ICP remains held for operator reconciliation", journal.vault_id, receipt.block_index);
        }
        Err(error) => {
            journal.status = state::BotPaymentStatus::ReceiptObserved;
            state::mutate_state(|s| { s.pending_payments.insert(journal.vault_id, journal.clone()); });
            state::save_config_to_stable();
            log!(crate::INFO, "Payment block is durable but backend proof confirmation remains pending for vault #{}: {}", journal.vault_id, error);
        }
    }
    true
}

/// Resume a claim-bound collateral return with the exact ledger dedup tuple.
/// Legacy journals without an intent, and intents outside the dedup window,
/// remain operator-held for exact ledger history reconciliation.
async fn resume_pending_return(config: &BotConfig) -> bool {
    let Some(mut claim) = state::read_state(|s| {
        s.pending_claims.values()
            .find(|claim| claim.status == state::BotClaimJournalStatus::ReturnPending)
            .cloned()
    }) else { return false };
    let Some(mut intent) = claim.collateral_return.clone() else {
        log!(crate::INFO, "STUCK: return for vault #{} lacks a persisted transfer tuple; operator reconciliation required", claim.vault_id);
        return true;
    };
    if config.backend_principal != intent.backend_principal || config.icp_ledger != intent.ledger_principal {
        log!(crate::INFO, "STUCK: return tuple for vault #{} is bound to a different backend or ledger", claim.vault_id);
        return true;
    }
    if intent.status == state::BotReturnTransferStatus::NoEffect {
        log!(crate::INFO, "STUCK: collateral return for vault #{} had a definitive no-effect response; explicit operator repair is required", claim.vault_id);
        return true;
    }
    if intent.receipt.is_none() {
        const SAFE_RETRY_WINDOW_NS: u64 = 23 * 60 * 60 * 1_000_000_000;
        if ic_cdk::api::time().saturating_sub(intent.created_at_time) >= SAFE_RETRY_WINDOW_NS {
            log!(crate::INFO, "STUCK: return tuple for vault #{} exceeded the safe dedup window; exact ledger reconciliation required", claim.vault_id);
            return true;
        }
        match swap::return_collateral_to_backend(
            config,
            intent.amount_e8s.saturating_add(intent.fee_e8s),
            intent.ledger_principal,
            intent.memo.clone(),
            intent.amount_e8s,
            intent.created_at_time,
        ).await {
            Ok(receipt) => {
                intent.receipt = Some(receipt);
                intent.status = state::BotReturnTransferStatus::ReceiptObserved;
                claim.collateral_return = Some(intent.clone());
                state::mutate_state(|s| { s.pending_claims.insert(claim.vault_id, claim.clone()); });
                state::save_config_to_stable();
            }
            Err(swap::TransferAttemptError::Ambiguous(error)) => {
                intent.status = state::BotReturnTransferStatus::Ambiguous;
                claim.collateral_return = Some(intent);
                state::mutate_state(|s| { s.pending_claims.insert(claim.vault_id, claim.clone()); });
                state::save_config_to_stable();
                log!(crate::INFO, "Return outcome remains ambiguous for vault #{}: {}", claim.vault_id, error);
                return true;
            }
            Err(swap::TransferAttemptError::NoEffect(error)) => {
                intent.status = state::BotReturnTransferStatus::NoEffect;
                claim.collateral_return = Some(intent);
                state::mutate_state(|s| { s.pending_claims.insert(claim.vault_id, claim.clone()); });
                state::save_config_to_stable();
                log!(crate::INFO, "Return had a definitive no-effect response for vault #{}; operator action required: {}", claim.vault_id, error);
                return true;
            }
        }
    }
    let Some(receipt) = intent.receipt.as_ref() else { return true };
    let proof = BotCollateralReturnProofArg {
        vault_id: claim.vault_id,
        claim_generation: claim.claim_generation,
        block_index: receipt.block_index,
        amount: receipt.amount,
        created_at_time: receipt.created_at_time,
    };
    if let Err(error) = call_bot_record_collateral_return_proof(config, proof).await {
        log!(crate::INFO, "Return receipt remains durable but backend proof recording failed for vault #{}: {}", claim.vault_id, error);
        return true;
    }
    match call_bot_cancel_liquidation(config, claim.vault_id).await {
        Ok(()) => {
            state::mutate_state(|s| { s.pending_claims.remove(&claim.vault_id); });
            state::save_config_to_stable();
        }
        Err(error) => log!(crate::INFO, "Return proof recorded but claim cancellation remains pending for vault #{}: {}", claim.vault_id, error),
    }
    true
}

pub async fn admin_retry_no_effect_payment(
    config: &BotConfig,
    vault_id: u64,
) -> Result<(), String> {
    let Some(mut journal) = state::read_state(|s| s.pending_payments.get(&vault_id).cloned()) else {
        return Err("no durable payment journal for this vault".into());
    };
    if journal.status != state::BotPaymentStatus::NoEffect || journal.receipt.is_some() {
        return Err("only a definitive no-effect payment can be manually retried".into());
    }
    if config.backend_principal != journal.backend_principal
        || config.ckusdc_ledger != journal.ledger_principal
    {
        return Err("configured backend or ckUSDC ledger differs from the journal".into());
    }
    let fee = config.ckusdc_fee_e6.unwrap_or(10_000);
    if journal.gross_amount_e6 < required_ckusdc_gross(journal.debt_covered_e8s, fee) {
        return Err("available claim proceeds remain short after the current ledger fee; claim is operator-held".into());
    }
    let balance = swap::balance_of_self_ckusdc(config).await?;
    if balance < journal.gross_amount_e6 {
        return Err("bot ckUSDC balance no longer covers the persisted claim reservation".into());
    }
    // The prior ICRC error proves no transfer occurred. Allocate a fresh
    // created_at_time for the new attempt while preserving the claim-bound
    // memo and amount; then persist before the next external call.
        journal.created_at_time = ic_cdk::api::time();
        journal.fee_e6 = fee;
        journal.amount_e6 = journal.gross_amount_e6.saturating_sub(fee);
    journal.status = state::BotPaymentStatus::Prepared;
    state::mutate_state(|s| { s.pending_payments.insert(vault_id, journal); });
    state::save_config_to_stable();
    resume_pending_payment(config).await;
    if state::read_state(|s| s.pending_payments.contains_key(&vault_id)) {
        Err("payment retry remains pending or ambiguous; inspect the durable journal".into())
    } else {
        Ok(())
    }
}

/// Operator supplies a candidate ledger block from ICRC-3 history. The backend
/// independently verifies every tuple field against the claim before debt
/// changes, so this endpoint cannot authorize a forged block.
pub async fn admin_reconcile_payment_block(
    config: &BotConfig,
    vault_id: u64,
    block_index: u64,
) -> Result<(), String> {
    let mut journal = state::read_state(|s| s.pending_payments.get(&vault_id).cloned())
        .ok_or_else(|| "no pending payment journal for this vault".to_string())?;
    if journal.receipt.is_some() {
        return Err("payment journal already has a recorded block".into());
    }
    journal.receipt = Some(swap::TransferReceipt {
        block_index,
        amount: journal.amount_e6,
        created_at_time: journal.created_at_time,
    });
    journal.status = state::BotPaymentStatus::ReceiptObserved;
    state::mutate_state(|s| { s.pending_payments.insert(vault_id, journal); });
    state::save_config_to_stable();
    resume_pending_payment(config).await;
    if state::read_state(|s| s.pending_payments.contains_key(&vault_id)) {
        Err("candidate block was not accepted or confirmation remains pending; inspect the durable journal".into())
    } else {
        Ok(())
    }
}

/// Operator supplies a candidate collateral-return block. The backend checks
/// exact sender, receiver, amount, memo, generation and timestamp before
/// recording the proof or allowing cancellation.
pub async fn admin_reconcile_return_block(
    config: &BotConfig,
    vault_id: u64,
    block_index: u64,
) -> Result<(), String> {
    let mut claim = state::read_state(|s| s.pending_claims.get(&vault_id).cloned())
        .ok_or_else(|| "no pending claim journal for this vault".to_string())?;
    let intent = claim.collateral_return.as_mut()
        .ok_or_else(|| "claim has no durable return tuple to reconcile".to_string())?;
    if intent.receipt.is_some() {
        return Err("return journal already has a recorded block".into());
    }
    intent.receipt = Some(swap::TransferReceipt {
        block_index,
        amount: intent.amount_e8s,
        created_at_time: intent.created_at_time,
    });
    intent.status = state::BotReturnTransferStatus::ReceiptObserved;
    state::mutate_state(|s| { s.pending_claims.insert(vault_id, claim); });
    state::save_config_to_stable();
    resume_pending_return(config).await;
    if state::read_state(|s| s.pending_claims.contains_key(&vault_id)) {
        Err("candidate block was not accepted or cancellation remains pending; inspect the durable journal".into())
    } else {
        Ok(())
    }
}

async fn call_bot_claim_liquidation(
    config: &BotConfig,
    vault_id: u64,
) -> Result<BotLiquidationResult, String> {
    let result: Result<(BackendResult<BotLiquidationResult>,), _> =
        ic_cdk::call(config.backend_principal, "bot_claim_liquidation", (vault_id,)).await;

    match result {
        Ok((BackendResult::Ok(r),)) => Ok(r),
        Ok((BackendResult::Err(e),)) => Err(format!("{}", e)),
        Err((code, msg)) => Err(format!("{:?}: {}", code, msg)),
    }
}

async fn call_bot_confirm_liquidation_with_proof(
    config: &BotConfig,
    proof: BotPaymentProof,
) -> Result<(), String> {
    let result: Result<(BackendResult<()>,), _> = ic_cdk::call(
        config.backend_principal,
        "bot_confirm_liquidation_with_proof",
        (proof,),
    )
    .await;
    match result {
        Ok((BackendResult::Ok(()),)) => Ok(()),
        Ok((BackendResult::Err(e),)) => Err(format!("{}", e)),
        Err((code, msg)) => Err(format!("{:?}: {}", code, msg)),
    }
}

async fn call_bot_record_collateral_return_proof(
    config: &BotConfig,
    proof: BotCollateralReturnProofArg,
) -> Result<(), String> {
    let result: Result<(BackendResult<()>,), _> = ic_cdk::call(
        config.backend_principal,
        "bot_record_collateral_return_proof",
        (proof,),
    )
    .await;
    match result {
        Ok((BackendResult::Ok(()),)) => Ok(()),
        Ok((BackendResult::Err(e),)) => Err(format!("{}", e)),
        Err((code, msg)) => Err(format!("{:?}: {}", code, msg)),
    }
}

async fn call_bot_cancel_liquidation(
    config: &BotConfig,
    vault_id: u64,
) -> Result<(), String> {
    let result: Result<(BackendResult<()>,), _> =
        ic_cdk::call(config.backend_principal, "bot_cancel_liquidation", (vault_id,)).await;

    match result {
        Ok((BackendResult::Ok(()),)) => Ok(()),
        Ok((BackendResult::Err(e),)) => Err(format!("{}", e)),
        Err((code, msg)) => Err(format!("{:?}: {}", code, msg)),
    }
}

pub fn calculate_swap_amount(collateral_e8s: u64, debt_e8s: u64, collateral_price_e8s: u64) -> u64 {
    if collateral_price_e8s == 0 {
        return collateral_e8s;
    }
    let icp_needed = (debt_e8s as u128 * 100_000_000 / collateral_price_e8s as u128) as u64;
    let with_buffer = icp_needed.saturating_mul(105) / 100;
    with_buffer.min(collateral_e8s)
}

fn calculate_slippage(effective_price_e8s: u64, oracle_price_e8s: u64) -> i32 {
    if oracle_price_e8s == 0 || effective_price_e8s == 0 {
        return 0;
    }
    let diff = oracle_price_e8s as i64 - effective_price_e8s as i64;
    (diff * 10_000 / oracle_price_e8s as i64) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    const SWAP_ERR: &str = "Quote returned zero output";
    const RETURN_ERR: &str = "Transfer error: BadFee";
    const CANCEL_ERR: &str = "GenericError(\"Cannot cancel claim for vault #7: protocol collateral balance 0 < required 99990000\")";

    #[test]
    fn short_payment_gross_threshold_covers_rounding_and_fee() {
        assert_eq!(required_ckusdc_gross(100_000_000, 10_000), 1_010_000);
        assert_eq!(required_ckusdc_gross(100_000_001, 10_000), 1_010_001);
        assert_eq!(required_ckusdc_gross(u64::MAX, u64::MAX), u64::MAX);
    }

    #[test]
    fn a_short_durable_receipt_is_held_below_the_claim_net_minimum() {
        let journal = state::BotPaymentJournal {
            vault_id: 19,
            backend_principal: candid::Principal::anonymous(),
            ledger_principal: candid::Principal::management_canister(),
            claim_generation: 4,
            debt_covered_e8s: 100_000_001,
            collateral_amount_e8s: 200_000_000,
            collateral_price_e8s: 100_000_000,
            icp_swapped_e8s: 110_000_000,
            ckusdc_received_e6: 1_010_001,
            held_surplus_e6: 0,
            gross_amount_e6: 1_010_001,
            amount_e6: 1_000_001,
            fee_e6: 10_000,
            created_at_time: 123,
            memo: b"claim-19-4".to_vec(),
            status: state::BotPaymentStatus::ReceiptObserved,
            receipt: None,
            shortfall_receipt_observed: false,
        };
        let short = crate::swap::TransferReceipt {
            block_index: 8,
            amount: 1_000_000,
            created_at_time: 123,
        };
        let exact = crate::swap::TransferReceipt {
            amount: 1_000_001,
            ..short.clone()
        };

        assert!(payment_receipt_is_short(&journal, &short));
        assert!(!payment_receipt_is_short(&journal, &exact));
        assert_eq!(required_ckusdc_net(u64::MAX), u64::MAX / 100 + 1);
    }

    #[test]
    fn confirmed_payment_totals_and_surplus_are_applied_once() {
        let journal = state::BotPaymentJournal {
            vault_id: 19,
            backend_principal: candid::Principal::anonymous(),
            ledger_principal: candid::Principal::management_canister(),
            claim_generation: 4,
            debt_covered_e8s: 100_000_000,
            collateral_amount_e8s: 200_000_000,
            collateral_price_e8s: 100_000_000,
            icp_swapped_e8s: 110_000_000,
            ckusdc_received_e6: 1_050_000,
            held_surplus_e6: 40_000,
            gross_amount_e6: 1_010_000,
            amount_e6: 1_000_000,
            fee_e6: 10_000,
            created_at_time: 123,
            memo: b"claim-19-4".to_vec(),
            status: state::BotPaymentStatus::Confirmed,
            receipt: Some(crate::swap::TransferReceipt {
                block_index: 8,
                amount: 1_000_000,
                created_at_time: 123,
            }),
            shortfall_receipt_observed: false,
        };
        let mut bot_state = state::BotState::default();
        bot_state.pending_payments.insert(journal.vault_id, journal.clone());

        assert!(apply_confirmed_payment_totals_once(&mut bot_state, &journal, 1_000_000));
        assert!(!apply_confirmed_payment_totals_once(&mut bot_state, &journal, 1_000_000));
        assert_eq!(bot_state.stats.total_debt_covered_e8s, 100_000_000);
        assert_eq!(bot_state.stats.total_ckusdc_deposited_e6, 1_000_000);
        assert_eq!(bot_state.stats.total_ckusdc_surplus_held_e6, 40_000);
        assert_eq!(bot_state.stats.total_collateral_received_e8s, 200_000_000);
        assert_eq!(bot_state.stats.events_count, 1);
        assert!(!bot_state.pending_payments.contains_key(&journal.vault_id));
    }

    #[test]
    fn swap_failure_clean_cleanup_records_swap_failed() {
        let outcome = decide_swap_failure_outcome(7, SWAP_ERR, None, None);
        assert_eq!(outcome.status, LiquidationStatus::SwapFailed);
        assert_eq!(outcome.error_message, SWAP_ERR);
        assert!(outcome.stuck_log.is_none(), "happy cleanup must not log STUCK");
    }

    #[test]
    fn swap_failure_with_failed_return_records_transfer_failed_and_logs_stuck() {
        let outcome = decide_swap_failure_outcome(7, SWAP_ERR, Some(RETURN_ERR), None);
        assert_eq!(outcome.status, LiquidationStatus::TransferFailed);
        assert!(outcome.error_message.contains("swap: "));
        assert!(outcome.error_message.contains(SWAP_ERR));
        assert!(outcome.error_message.contains("return: "));
        assert!(outcome.error_message.contains(RETURN_ERR));
        let log = outcome.stuck_log.expect("must surface STUCK log");
        assert!(log.contains("STUCK"));
        assert!(log.contains("vault #7"));
        assert!(log.contains("ICP return failed"));
    }

    #[test]
    fn swap_failure_with_stuck_cancel_records_confirm_failed_and_logs_stuck() {
        let outcome = decide_swap_failure_outcome(7, SWAP_ERR, None, Some((3, CANCEL_ERR)));
        assert_eq!(outcome.status, LiquidationStatus::ConfirmFailed);
        assert!(outcome.error_message.contains("swap: "));
        assert!(outcome.error_message.contains(SWAP_ERR));
        assert!(outcome.error_message.contains("cancel after 3 retries: "));
        assert!(outcome.error_message.contains(CANCEL_ERR));
        let log = outcome.stuck_log.expect("must surface STUCK log");
        assert!(log.contains("STUCK"));
        assert!(log.contains("vault #7"));
        assert!(log.contains("3 attempts"));
    }

    #[test]
    fn claim_retry_first_failure_schedules_retry() {
        // First failure (count=0) on a 3-attempt limit must reschedule, not drop.
        let action = next_claim_retry_action(0, 3);
        assert_eq!(action, ClaimRetryAction::Retry { new_count: 1 });
    }

    #[test]
    fn claim_retry_penultimate_failure_still_retries() {
        // Second failure (count=1) leaves one more shot before the limit.
        let action = next_claim_retry_action(1, 3);
        assert_eq!(action, ClaimRetryAction::Retry { new_count: 2 });
    }

    #[test]
    fn claim_retry_final_failure_gives_up() {
        // Third failure (count=2) hits the limit, so the bot must stop trying
        // and let the 300s cascade escalate to the stability pool.
        let action = next_claim_retry_action(2, 3);
        assert_eq!(action, ClaimRetryAction::GiveUp);
    }

    #[test]
    fn claim_retry_handles_counter_overflow_safely() {
        // Defensive: if state somehow got a u8::MAX counter (e.g. legacy data
        // corruption), saturating_add prevents wraparound to 0, and the
        // attempted >= max check still pushes us into GiveUp.
        let action = next_claim_retry_action(u8::MAX, 3);
        assert_eq!(action, ClaimRetryAction::GiveUp);
    }

    // ── compute_swap_reservation ────────────────────────────────────────

    #[test]
    fn reservation_clean_router_matches_delta() {
        // Normal case: pool delivered exactly what the router promised.
        // No discrepancy, transfer the router-reported amount.
        let r = compute_swap_reservation(7, 4_220_000, 100, 4_220_100);
        assert_eq!(r.to_transfer_e6, 4_220_000);
        assert_eq!(r.recorded_received_e6, 4_220_000);
        assert!(r.discrepancy_note.is_none(), "exact-match must not flag a discrepancy");
    }

    #[test]
    fn reservation_pool_underdelivers_uses_delta() {
        // Reproduces the 2026-05-18 vault #69 incident: router reported
        // 4_219_759 ckUSDC for the swap, but the wallet only saw 0 of it
        // arrive (balance went from 373_400 → 373_400). With the old
        // accounting the bot tried to transfer 4_219_759 and the ledger
        // rejected with InsufficientFunds. With reservation, the bot only
        // tries to transfer the delta (0) and the discrepancy is recorded.
        let r = compute_swap_reservation(69, 4_219_759, 373_400, 373_400);
        assert_eq!(r.to_transfer_e6, 0, "must not attempt to transfer router-reported amount when wallet didn't gain it");
        assert_eq!(r.recorded_received_e6, 0);
        let note = r.discrepancy_note.expect("under-delivery must produce a note");
        assert!(note.contains("vault #69"));
        assert!(note.contains("router reported 4219759"));
        assert!(note.contains("wallet delta only 0"));
        assert!(note.contains("gap 4219759"));
    }

    #[test]
    fn reservation_partial_underdelivery_uses_delta() {
        // Hybrid case: pool delivered some but not all. Use what landed.
        let r = compute_swap_reservation(42, 4_000_000, 1_000_000, 2_500_000);
        assert_eq!(r.to_transfer_e6, 1_500_000);
        assert_eq!(r.recorded_received_e6, 1_500_000);
        assert!(r.discrepancy_note.is_some());
        assert!(r.discrepancy_note.unwrap().contains("gap 2500000"));
    }

    #[test]
    fn reservation_overdelivery_does_not_spend_surplus() {
        // Wallet gained more than the router claimed (delayed prior swap
        // arrived, unrelated deposit, whatever). The surplus is NOT this
        // claim's to spend — cap transfer at what the router said.
        let r = compute_swap_reservation(7, 4_000_000, 100, 9_999_999);
        assert_eq!(r.to_transfer_e6, 4_000_000, "must not dip into surplus that wasn't earmarked");
        assert_eq!(r.recorded_received_e6, 4_000_000);
        assert!(r.discrepancy_note.is_none(), "over-delivery is not a stuck-claim situation");
    }

    #[test]
    fn favorable_swap_overage_is_held_and_not_reused_by_next_claim() {
        let first = compute_swap_reservation(7, 1_050_000, 2_000_000, 3_050_000);
        let first_payment = allocate_claim_payment(&first, 1_010_000);
        assert_eq!(first_payment.gross_to_transfer_e6, 1_010_000);
        assert_eq!(first_payment.held_surplus_e6, 40_000);

        // The old 40_000 remains in the shared bot balance. A later claim's
        // before/after delta excludes it, so it cannot help satisfy the new
        // claim's independent reservation.
        let second = compute_swap_reservation(8, 800_000, 3_090_000, 3_890_000);
        let second_payment = allocate_claim_payment(&second, 810_000);
        assert_eq!(second.to_transfer_e6, 800_000);
        assert_eq!(second_payment.gross_to_transfer_e6, 800_000);
        assert_eq!(second_payment.held_surplus_e6, 0);
    }

    #[test]
    fn reservation_before_balance_unreadable_falls_back_to_router() {
        // process_pending uses u64::MAX as a sentinel when the pre-swap
        // balance read fails. saturating_sub then yields 0, but the
        // post-swap read sees the post-swap balance — actual_delta
        // saturates to 0, triggering the under-delivery branch (use 0).
        // This is the conservative behavior we want when balance reads
        // are unreliable: do nothing rather than spend blindly.
        let r = compute_swap_reservation(99, 1_000_000, u64::MAX, 500_000);
        assert_eq!(r.to_transfer_e6, 0);
        assert_eq!(r.recorded_received_e6, 0);
        assert!(r.discrepancy_note.is_some());
    }

    #[test]
    fn return_error_takes_priority_over_cancel_error() {
        // Defensive: integration code should never hand both, but the helper
        // must still pick deterministically. Return failure dominates because
        // the cancel never actually happened in that branch.
        let outcome = decide_swap_failure_outcome(
            42,
            SWAP_ERR,
            Some(RETURN_ERR),
            Some((3, CANCEL_ERR)),
        );
        assert_eq!(outcome.status, LiquidationStatus::TransferFailed);
        assert!(outcome.error_message.contains("return: "));
        assert!(!outcome.error_message.contains("cancel after"));
    }
}
