use candid::{CandidType, Deserialize, Principal};
use ic_canister_log::log;

use crate::history::{self, BotCkUsdcPaymentDispatchObservation, LiquidationRecordV1, LiquidationRecordVersioned, LiquidationStatus};
use crate::state::{self, BotConfig, LiquidatableVaultInfo};
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
/// `to_transfer_e6` is the gross amount reserved for the payment, including
/// the outgoing ledger fee.
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

fn compute_swap_reservation_from_snapshots(
    vault_id: u64,
    router_received_e6: u64,
    balance_before_e6: Option<u64>,
    balance_after_e6: Option<u64>,
) -> Result<SwapReservation, String> {
    let before = balance_before_e6.ok_or_else(|| "pre-swap ckUSDC balance is unavailable".to_string())?;
    let after = balance_after_e6.ok_or_else(|| "post-swap ckUSDC balance is unavailable".to_string())?;
    Ok(compute_swap_reservation(vault_id, router_received_e6, before, after))
}

/// Return the remaining ckUSDC payment deficit after the configured outgoing
/// transfer fee. A short result must be held before sending anything to the
/// backend: once a positive underpayment lands, the current single-block
/// confirmation path cannot complete the claim.
fn ckusdc_payment_shortfall_e6(
    gross_swap_proceeds_e6: u64,
    transfer_fee_e6: u64,
    minimum_payment_e6: u64,
) -> Option<u64> {
    let net_payment_e6 = gross_swap_proceeds_e6.saturating_sub(transfer_fee_e6);
    (net_payment_e6 < minimum_payment_e6).then(|| minimum_payment_e6 - net_payment_e6)
}

/// The pool pulls both the swap input and its ICP ledger fee from the bot.
/// The treasury bonus is a gross debit of what remains from this claim;
/// its own outgoing fee is included in that gross amount later.
pub(crate) fn icp_treasury_bonus_gross_after_swap(
    collateral_e8s: u64,
    swap_input_e8s: u64,
    pool_input_fee_e8s: u64,
) -> u64 {
    collateral_e8s.saturating_sub(swap_input_e8s).saturating_sub(pool_input_fee_e8s)
}

/// Outcome of the swap-failure cleanup path. Pure data, produced by
/// `decide_swap_failure_outcome` and consumed by `process_pending` to write
/// the LiquidationRecord and emit the STUCK log line if applicable.
#[derive(Debug, PartialEq)]
pub(crate) struct SwapFailureOutcome {
    pub status: history::LiquidationStatus,
    pub error_message: String,
    /// Some(line) when the bot is leaving a claim active that the protocol
    /// will reconcile after the timeout, or admin via `admin_retry_claim_return`.
    /// None when both cleanup steps succeeded.
    pub stuck_log: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum ClaimIntentAction {
    Create(u64),
    Retry(u64),
    AcknowledgeNoEffect { request_id: u64, transfer_digest: Vec<u8> },
    RecoverClaimed(u64),
    HoldReturnRecovery(u64),
    QuarantineSwapStarted(u64),
    HoldLegacyAcquired(u64),
}

enum SwapAttemptError {
    NotStarted(String),
    StartedOutcomeUnknown(String),
}

fn claim_intent_action(
    intent: Option<history::BotClaimIntent>,
    fresh_request_id: u64,
) -> ClaimIntentAction {
    match intent {
        Some(intent) if intent.return_transfer.is_some()
            && matches!(intent.phase, history::BotClaimIntentPhase::Claimed(_)) =>
        {
            // Once a claim-specific collateral return tuple exists, processing
            // it as a swap retry can race a cancel whose reply was lost.
            // Recovery belongs to the explicit exact-return path.
            ClaimIntentAction::HoldReturnRecovery(intent.request_id)
        }
        Some(intent) => match intent.phase {
            history::BotClaimIntentPhase::AwaitingReceipt
            | history::BotClaimIntentPhase::ClaimRequested => {
                ClaimIntentAction::Retry(intent.request_id)
            }
            history::BotClaimIntentPhase::NoEffectAcknowledgementPending { transfer_digest } => {
                ClaimIntentAction::AcknowledgeNoEffect {
                    request_id: intent.request_id,
                    transfer_digest,
                }
            }
            history::BotClaimIntentPhase::Claimed(_) => ClaimIntentAction::RecoverClaimed(intent.request_id),
            history::BotClaimIntentPhase::SwapStarted(_) => ClaimIntentAction::QuarantineSwapStarted(intent.request_id),
            history::BotClaimIntentPhase::Acquired { .. } => ClaimIntentAction::HoldLegacyAcquired(intent.request_id),
        },
        None => ClaimIntentAction::Create(fresh_request_id),
    }
}

fn enqueue_claim_retry(vault: &crate::state::LiquidatableVaultInfo) {
    state::mutate_state(|s| {
        if !s.pending_vaults.iter().any(|queued| queued.vault_id == vault.vault_id) {
            s.pending_vaults.insert(0, vault.clone());
        }
    });
}

fn bot_claim_receipt_matches(
    vault_id: u64,
    receipt: &history::BotClaimReceipt,
    result: &BotLiquidationResult,
) -> bool {
    result.vault_id == vault_id
        && result.collateral_amount == receipt.collateral_amount
        && result.debt_covered == receipt.debt_covered
        && result.collateral_price_e8s == receipt.collateral_price_e8s
        && result.claim_timestamp == Some(receipt.claim_timestamp)
        && result.payment_memo.as_ref() == Some(&receipt.payment_memo)
        && result.claim_transfer.as_ref() == receipt.claim_transfer.as_ref()
}

pub(crate) fn claim_transfer_receipt_is_complete(
    backend: Principal,
    bot: Principal,
    icp_ledger: Principal,
    vault_id: u64,
    claim_timestamp: u64,
    collateral_amount: u64,
    proof: &history::BotClaimTransferReceipt,
) -> bool {
    proof.ledger == icp_ledger
        && proof.from.owner == backend
        && proof.to == (icrc_ledger_types::icrc1::account::Account { owner: bot, subaccount: None })
        && proof.amount_e8s == collateral_amount
        && !proof.memo.is_empty()
        && proof.created_at_time >= claim_timestamp
        && proof.return_account == (icrc_ledger_types::icrc1::account::Account {
            owner: backend,
            subaccount: Some(swap::bot_claim_return_subaccount(vault_id, claim_timestamp)),
        })
}

fn mark_claim_swap_started(vault_id: u64, request_id: u64, pool_input_fee_e8s: u64) -> bool {
    let Some(intent) = history::get_claim_intent(vault_id) else { return false; };
    let history::BotClaimIntentPhase::Claimed(receipt) = intent.phase else { return false; };
    history::mark_claim_swap_started(vault_id, request_id, &receipt, pool_input_fee_e8s)
}

/// Reconcile the original payment from a persisted exact tuple. A missing
/// block index may be dispatched only with the saved arguments; after either
/// a success or Duplicate response, the index is persisted before block proof.
pub async fn dispatch_and_verify_ckusdc_payment(
    config: &BotConfig,
    vault_id: u64,
    claim_timestamp: u64,
    payment_memo: &[u8],
    transfer: &mut history::BotCkUsdcPaymentTransfer,
) -> Result<swap::CkUsdcPaymentReceipt, String> {
    ensure_current_ckusdc_payment_generation(vault_id, claim_timestamp, payment_memo, transfer)?;
    if transfer.args.from_subaccount != Some(claim_payment_subaccount(vault_id, claim_timestamp))
        || transfer.args.to.subaccount.is_some()
        || transfer.args.fee.is_none()
        || transfer.args.memo.as_ref().map(|memo| memo.0.as_ref()) != Some(payment_memo)
        || transfer.args.created_at_time.is_none_or(|created| created < claim_timestamp)
    {
        return Err("persisted original ckUSDC tuple does not match this claim generation".into());
    }
    if transfer.dispatch_attempt_count == Some(1)
        && transfer.history_scan.is_none()
        && transfer.block_index.is_none()
        && matches!(transfer.dispatch_observation,
            Some(BotCkUsdcPaymentDispatchObservation::BadFee { .. }
                | BotCkUsdcPaymentDispatchObservation::InsufficientFunds { .. }))
    {
        return Err("the exact ckUSDC tuple has a first-attempt typed no-effect receipt; refresh isolated-account fee/balance and prepare a replacement tuple before dispatch".into());
    }

    let block_index = match transfer.block_index {
        Some(index) => index,
        None => {
            if transfer.history_scan.is_none() {
                if transfer.ledger != config.ckusdc_ledger || transfer.args.to.owner != config.backend_principal {
                    return Err("original payment ledger/backend changed before dispatch or reconciliation".into());
                }
                if transfer.dispatch_attempt_count == Some(u32::MAX) {
                    return Err("original ckUSDC dispatch attempt counter is exhausted".into());
                }
                if let Some(attempt_count) = transfer.dispatch_attempt_count {
                    transfer.dispatch_attempt_count = Some(attempt_count + 1);
                    if !history::update_ckusdc_payment_transfer(vault_id, transfer.clone()) {
                        transfer.dispatch_attempt_count = Some(attempt_count);
                        return Err("could not persist exact ckUSDC dispatch attempt before ledger call".into());
                    }
                }
                ensure_current_ckusdc_payment_generation(vault_id, claim_timestamp, payment_memo, transfer)?;
                let dispatch_result = swap::transfer_ckusdc_payment_exact_typed(transfer).await;
                ensure_current_ckusdc_payment_generation(vault_id, claim_timestamp, payment_memo, transfer)?;
                match dispatch_result {
                    Ok(index) => {
                        transfer.block_index = Some(index);
                        if !history::update_ckusdc_payment_transfer(vault_id, transfer.clone()) {
                            return Err("original ckUSDC transfer succeeded but its block index could not be persisted".into());
                        }
                        index
                    }
                    Err(dispatch_error) => {
                        // Persist the typed ledger observation against this
                        // exact tuple before even asking for history length.
                        // Strings and local clocks never classify TooOld.
                        transfer.dispatch_observation = Some(dispatch_error.observation());
                        if !history::update_ckusdc_payment_transfer(vault_id, transfer.clone()) {
                            return Err("could not persist original ckUSDC dispatch observation".into());
                        }
                        ensure_current_ckusdc_payment_generation(vault_id, claim_timestamp, payment_memo, transfer)?;
                        if transfer.dispatch_attempt_count == Some(1)
                            && matches!(transfer.dispatch_observation,
                                Some(BotCkUsdcPaymentDispatchObservation::BadFee { .. }
                                    | BotCkUsdcPaymentDispatchObservation::InsufficientFunds { .. }))
                        {
                            return Err(format!(
                                "first ckUSDC tuple was definitively rejected without effect ({:?}); refresh this claim account and fee before preparing its replacement",
                                transfer.dispatch_observation,
                            ));
                        }
                        // Any rejected/ambiguous reply becomes a fixed-snapshot
                        // positive-evidence scan. It never authorizes a fresh
                        // transfer tuple, including after TooOld.
                        let log_length = swap::ckusdc_history_log_length(transfer.ledger).await?;
                        ensure_current_ckusdc_payment_generation(vault_id, claim_timestamp, payment_memo, transfer)?;
                        transfer.history_scan = Some(history::BotCkUsdcPaymentHistoryScan {
                            snapshot_log_length: log_length,
                            next_index: log_length,
                            candidate_block_index: None,
                            multiple_candidates: false,
                        });
                        if !history::update_ckusdc_payment_transfer(vault_id, transfer.clone()) {
                            return Err("could not persist original ckUSDC history scan before lookup".into());
                        }
                        advance_ckusdc_payment_history_scan(vault_id, claim_timestamp, payment_memo, transfer).await
                            .map_err(|scan_error| format!("original ckUSDC dispatch {dispatch_error:?}; exact history reconciliation held: {scan_error}"))?
                    }
                }
            } else {
                advance_ckusdc_payment_history_scan(vault_id, claim_timestamp, payment_memo, transfer).await?
            }
        }
    };

    let paid = swap::verify_ckusdc_transfer_block(
        transfer.ledger,
        block_index,
        icrc_ledger_types::icrc1::account::Account {
            owner: ic_cdk::id(),
            subaccount: transfer.args.from_subaccount,
        },
        transfer.args.to,
        payment_memo,
    ).await?;
    ensure_current_ckusdc_payment_generation(vault_id, claim_timestamp, payment_memo, transfer)?;
    let expected_amount = transfer.args.amount.0.to_string().parse::<u64>()
        .map_err(|_| "persisted original ckUSDC amount exceeds u64")?;
    let expected_fee = transfer.args.fee.as_ref()
        .and_then(|fee| fee.0.to_string().parse::<u64>().ok())
        .ok_or("persisted original ckUSDC fee is invalid")?;
    if paid.amount_e6 != expected_amount
        || paid.fee_e6 != expected_fee
        || Some(paid.created_at_time) != transfer.args.created_at_time
    {
        return Err("original ckUSDC block does not match the exact persisted transfer tuple".into());
    }
    Ok(swap::CkUsdcPaymentReceipt { amount_e6: paid.amount_e6, block_index })
}

const CKUSDC_HISTORY_SCAN_BLOCKS_PER_CALL: u64 = history::CKUSDC_PAYMENT_HISTORY_PAGE_SIZE;
pub(crate) const AUTOMATIC_CLAIM_SWAP_ENABLED: bool = false;

/// Stable ckUSDC source account reserved for one claim generation. This must
/// stay byte-for-byte aligned with the backend's claim-payment validator.
pub(crate) fn claim_payment_subaccount(vault_id: u64, claim_timestamp: u64) -> [u8; 32] {
    let mut subaccount = [0u8; 32];
    subaccount[..16].copy_from_slice(b"RUMI-CLAIM-PAY01");
    subaccount[16..24].copy_from_slice(&vault_id.to_be_bytes());
    subaccount[24..32].copy_from_slice(&claim_timestamp.to_be_bytes());
    subaccount
}

/// Advance a persisted backwards scan over one bounded page. One exact block
/// proves a positive receipt immediately; a no-match result remains held and
/// can never authorize a replacement transfer.
async fn advance_ckusdc_payment_history_scan(
    vault_id: u64,
    claim_timestamp: u64,
    payment_memo: &[u8],
    transfer: &mut history::BotCkUsdcPaymentTransfer,
) -> Result<u64, String> {
    use swap::CkUsdcHistoryMatch;
    let mut scan = transfer.history_scan.clone().ok_or("payment history scan is not initialized")?;
    if scan.next_index > scan.snapshot_log_length {
        return Err("persisted payment history cursor exceeds its fixed snapshot".into());
    }
    if scan.multiple_candidates {
        return Err("conflicting exact ckUSDC history candidates remain held".into());
    }
    if let Some(index) = scan.candidate_block_index {
        transfer.history_scan = None;
        transfer.block_index = Some(index);
        if !history::update_ckusdc_payment_transfer(vault_id, transfer.clone()) {
            return Err("could not persist positively matched ckUSDC block index".into());
        }
        return Ok(index);
    }
    if scan.next_index == 0 {
        // A transfer that was still executing when the initial log-length
        // query ran may land after that snapshot. Refresh only after the
        // previous prefix was exhausted; this extends positive evidence
        // coverage but can never authorize a new transfer.
        let latest = swap::ckusdc_history_log_length(transfer.ledger).await?;
        ensure_current_ckusdc_payment_generation(vault_id, claim_timestamp, payment_memo, transfer)?;
        if latest > scan.snapshot_log_length {
            scan.snapshot_log_length = latest;
            scan.next_index = latest;
            transfer.history_scan = Some(scan.clone());
            if !history::update_ckusdc_payment_transfer(vault_id, transfer.clone()) {
                return Err("could not persist refreshed ckUSDC history snapshot".into());
            }
        } else {
            return Err("fixed ckUSDC history snapshot contains no exact payment; reissue remains disabled".into());
        }
    }
    if scan.next_index > 0 {
        let start = scan.next_index.saturating_sub(CKUSDC_HISTORY_SCAN_BLOCKS_PER_CALL);
        for index in (start..scan.next_index).rev() {
            let block = swap::fetch_ckusdc_history_block(transfer.ledger, index).await?;
            ensure_current_ckusdc_payment_generation(vault_id, claim_timestamp, payment_memo, transfer)?;
            if swap::classify_ckusdc_payment_block(
                &block,
                icrc_ledger_types::icrc1::account::Account {
                    owner: ic_cdk::id(),
                    subaccount: transfer.args.from_subaccount,
                },
                &transfer.args,
            )? == CkUsdcHistoryMatch::ExactMatch {
                // Persist the positive candidate first. ICRC-1 retries retain
                // this same complete tuple, so the ledger duplicate key is
                // unchanged; once scanning begins, this path never resends.
                scan.next_index = index;
                scan.candidate_block_index = Some(index);
                transfer.history_scan = Some(scan.clone());
                if !history::update_ckusdc_payment_transfer(vault_id, transfer.clone()) {
                    return Err("could not persist positively matched ckUSDC history candidate".into());
                }
                transfer.history_scan = None;
                transfer.block_index = Some(index);
                if !history::update_ckusdc_payment_transfer(vault_id, transfer.clone()) {
                    return Err("could not persist positively matched ckUSDC block index".into());
                }
                return Ok(index);
            }
        }
        scan.next_index = start;
        transfer.history_scan = Some(scan.clone());
        if !history::update_ckusdc_payment_transfer(vault_id, transfer.clone()) {
            return Err("could not persist ckUSDC history scan cursor".into());
        }
    }
    if scan.next_index != 0 {
        return Err(format!("ckUSDC history scan advanced; {} earlier blocks remain", scan.next_index));
    }
    Err("ckUSDC history scan advanced without proving payment; reissue remains disabled".into())
}

fn ensure_current_ckusdc_payment_generation(
    vault_id: u64,
    claim_timestamp: u64,
    payment_memo: &[u8],
    transfer: &history::BotCkUsdcPaymentTransfer,
) -> Result<(), String> {
    let intent = history::get_claim_intent(vault_id).ok_or("original ckUSDC claim intent disappeared during await")?;
    let receipt_matches = match intent.phase {
        history::BotClaimIntentPhase::Claimed(receipt)
        | history::BotClaimIntentPhase::SwapStarted(receipt) => {
            receipt.claim_timestamp == claim_timestamp && receipt.payment_memo == payment_memo
        }
        _ => false,
    };
    let journal_matches = intent.ckusdc_payment_transfer.as_ref().is_some_and(|saved| {
        saved.ledger == transfer.ledger
            && saved.args == transfer.args
            && saved.block_index == transfer.block_index
            && saved.dispatch_attempt_count == transfer.dispatch_attempt_count
            && saved.prior_no_effects == transfer.prior_no_effects
            && saved.dispatch_observation == transfer.dispatch_observation
            && saved.history_scan == transfer.history_scan
    });
    if intent.vault_id != vault_id || !receipt_matches || !journal_matches {
        return Err("claim generation or exact ckUSDC journal changed during reconciliation await".into());
    }
    Ok(())
}

#[cfg(test)]
mod ckusdc_dispatch_generation_tests {
    use super::{arm_ckusdc_top_up_dispatch, ensure_current_ckusdc_payment_generation, ensure_current_ckusdc_top_up_generation};
    use crate::history::{
        self, BotClaimIntent, BotClaimIntentPhase, BotClaimReceipt,
        BotCkUsdcPaymentDispatchObservation, BotCkUsdcPaymentTransfer,
        BotCkUsdcTopUpTransfer,
    };
    use candid::Principal;
    use icrc_ledger_types::icrc1::{account::Account, transfer::{Memo, TransferArg}};

    #[test]
    fn too_old_evidence_is_bound_to_the_saved_claim_generation_and_tuple() {
        crate::memory::init_memory_manager();
        history::init_history();
        let backend = Principal::from_slice(&[0x45]);
        let memo = b"claim-124".to_vec();
        let receipt = BotClaimReceipt {
            collateral_amount: 200, debt_covered: 100, collateral_price_e8s: 456,
            claim_timestamp: 124, payment_memo: memo.clone(),
            claim_transfer: None,
        };
        let transfer = BotCkUsdcPaymentTransfer {
            ledger: Principal::from_slice(&[0x44]),
            args: TransferArg {
                from_subaccount: None, to: Account { owner: backend, subaccount: None },
                amount: candid::Nat::from(990u64), fee: Some(candid::Nat::from(10u64)),
                memo: Some(Memo::from(memo.clone())), created_at_time: Some(555),
            },
            block_index: None,
            dispatch_attempt_count: Some(0),
            prior_no_effects: Some(Vec::new()),
            dispatch_observation: Some(BotCkUsdcPaymentDispatchObservation::TooOld),
            history_scan: None,
        };
        history::put_claim_intent(BotClaimIntent {
            vault_id: 76, request_id: 907, claim_call_count: Some(1),
            backend_principal: Some(backend), return_transfer: None,
            ckusdc_top_up_transfer: None, ckusdc_payment_transfer: Some(transfer.clone()),
            shortfall_eligibility: None,
            icp_pool_input_fee_e8s: None,
            phase: BotClaimIntentPhase::SwapStarted(receipt),
        });
        assert!(ensure_current_ckusdc_payment_generation(76, 124, &memo, &transfer).is_ok());
        assert!(ensure_current_ckusdc_payment_generation(76, 125, &memo, &transfer).is_err());
        assert!(ensure_current_ckusdc_payment_generation(76, 124, b"claim-125", &transfer).is_err());
        let mut different_tuple = transfer;
        different_tuple.args.created_at_time = Some(556);
        assert!(ensure_current_ckusdc_payment_generation(76, 124, &memo, &different_tuple).is_err());
    }

    #[test]
    fn replaced_top_up_reloads_tombstone_before_it_is_armed_for_dispatch() {
        crate::memory::init_memory_manager();
        history::init_history();
        let vault_id = 79;
        let claim_timestamp = 124;
        let backend = Principal::from_slice(&[0x45]);
        let ledger = Principal::from_slice(&[0x44]);
        let memo = b"claim-124".to_vec();
        let receipt = BotClaimReceipt {
            collateral_amount: 200, debt_covered: 100, collateral_price_e8s: 456,
            claim_timestamp, payment_memo: memo.clone(), claim_transfer: None,
        };
        let rejected = BotCkUsdcTopUpTransfer {
            ledger,
            args: TransferArg {
                from_subaccount: Some([7; 32]),
                to: Account { owner: backend, subaccount: None },
                amount: candid::Nat::from(23u64), fee: Some(candid::Nat::from(17u64)),
                memo: Some(Memo::from(memo.clone())), created_at_time: Some(555),
            },
            block_index: None,
            dispatch_attempt_count: Some(1),
            prior_no_effects: Some(Vec::new()),
            dispatch_observation: Some(BotCkUsdcPaymentDispatchObservation::BadFee { expected_fee: 20 }),
            history_scan: None,
        };
        history::put_claim_intent(BotClaimIntent {
            vault_id, request_id: 910, claim_call_count: Some(1), backend_principal: Some(backend),
            return_transfer: None, ckusdc_top_up_transfer: Some(rejected.clone()),
            ckusdc_payment_transfer: None, shortfall_eligibility: None,
            icp_pool_input_fee_e8s: None, phase: BotClaimIntentPhase::Claimed(receipt),
        });
        let mut replacement = rejected.clone();
        replacement.args.fee = Some(candid::Nat::from(23u64));
        replacement.args.created_at_time = Some(556);
        replacement.dispatch_attempt_count = Some(0);
        replacement.dispatch_observation = None;
        assert!(history::replace_ckusdc_top_up_after_first_no_effect(vault_id, &rejected, replacement));

        let mut persisted = history::get_claim_intent(vault_id).unwrap().ckusdc_top_up_transfer.unwrap();
        assert_eq!(persisted.prior_no_effects.as_ref().unwrap().len(), 1);
        assert!(ensure_current_ckusdc_top_up_generation(vault_id, claim_timestamp, &memo, &persisted).is_ok());
        arm_ckusdc_top_up_dispatch(vault_id, &mut persisted).unwrap();
        assert_eq!(persisted.dispatch_attempt_count, Some(1));
        assert!(ensure_current_ckusdc_top_up_generation(vault_id, claim_timestamp, &memo, &persisted).is_ok());
        let stored = history::get_claim_intent(vault_id).unwrap().ckusdc_top_up_transfer.unwrap();
        assert_eq!(stored, persisted, "the exact replacement with its tombstone is durably armed before ledger dispatch");
    }
}

/// Wave 13 (BOT-002): decide which `LiquidationStatus` and `error_message`
/// to record after a swap failure, given the outcomes of the return-collateral
/// transfer and the cancel-claim retry loop.
///
/// Before Wave 13 the bot ignored both call results with `let _ = ...` and
/// always wrote `SwapFailed`, even when the protocol's claim was still active
/// (budget unrestored, vault still flagged). With the Wave-12 BOT-001b balance
/// gate a failed return guarantees the cancel rejects, so the bot record must
/// reflect that the claim remains active pending exact return recovery.
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
                "STUCK: ICP return failed after swap failure for vault #{}; claim remains active for exact return recovery via admin_retry_claim_return.",
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
                "STUCK: cancel failed after {} attempts for vault #{}; ICP returned but claim remains active for admin_retry_claim_return.",
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
    #[serde(default)]
    pub claim_timestamp: Option<u64>,
    #[serde(default)]
    pub payment_memo: Option<Vec<u8>>,
    /// Exact collateral ledger receipt, absent for legacy/synthetic results.
    #[serde(default)]
    pub claim_transfer: Option<history::BotClaimTransferReceipt>,
}

#[derive(CandidType, Deserialize, Debug)]
pub enum BackendResult<T> {
    #[serde(rename = "Ok")]
    Ok(T),
    #[serde(rename = "Err")]
    Err(BackendError),
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
struct BotPartialPaymentLockEvidence {
    ledger: Principal,
    vault_id: u64,
    claim_timestamp: u64,
    payment_memo: Vec<u8>,
    payment_block_index: u64,
    total_amount_e6: u64,
    aggregate_complete: bool,
}

fn partial_payment_lock_evidence_matches(
    evidence: &BotPartialPaymentLockEvidence,
    ledger: Principal,
    vault_id: u64,
    claim_timestamp: u64,
    payment_memo: &[u8],
    payment_block_index: u64,
    total_amount_e6: u64,
) -> bool {
    evidence.ledger == ledger
        && evidence.vault_id == vault_id
        && evidence.claim_timestamp == claim_timestamp
        && evidence.payment_memo == payment_memo
        && evidence.payment_block_index == payment_block_index
        && evidence.total_amount_e6 == total_amount_e6
        && !evidence.aggregate_complete
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

/// A typed `Err` response is different from a transport rejection: only the
/// former proves that the backend returned a protocol error value. A
/// `TemporarilyUnavailable` response can still describe an unresolved ledger
/// transfer, so it remains attached to the same durable request ID.
#[derive(Debug)]
enum BotClaimCallError {
    BackendRejected(BackendError),
    TransportUnknown(String),
}

impl std::fmt::Display for BotClaimCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BackendRejected(error) => write!(f, "typed backend rejection: {}", error),
            Self::TransportUnknown(error) => write!(f, "backend call outcome unknown: {}", error),
        }
    }
}

impl BotClaimCallError {
    fn is_unresolved(&self) -> bool {
        match self {
            Self::TransportUnknown(_) => true,
            Self::BackendRejected(BackendError::TemporarilyUnavailable(_)) => true,
            Self::BackendRejected(BackendError::GenericError(message)) => {
                message.contains("outcome is unresolved")
                    || message.contains("exact retry remains available")
                    || message.contains("retry the same claim")
            }
            Self::BackendRejected(_) => false,
        }
    }
}

fn claim_call_is_unresolved(prior_calls: Option<u32>, error: &BotClaimCallError) -> bool {
    prior_calls.map_or(true, |count| count > 0) || error.is_unresolved()
}

#[derive(CandidType, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct BotClaimNoEffectProof {
    pub vault_id: u64,
    pub request_id: u64,
    pub transfer_digest: Vec<u8>,
}

fn bot_no_effect_proof_matches(
    vault_id: u64,
    request_id: u64,
    proof: &BotClaimNoEffectProof,
) -> bool {
    proof.vault_id == vault_id && proof.request_id == request_id && proof.transfer_digest.len() == 32
}

/// Only a positive typed proof can release a failed request ID. Persist the
/// exact proof before ACK so upgrades and lost ACK replies resume ACK-only.
async fn resolve_no_effect_receipt(
    config: &BotConfig,
    vault_id: u64,
    request_id: u64,
) -> Result<bool, String> {
    let Some(proof) = call_bot_claim_no_effect_status(config, vault_id, request_id).await? else {
        return Ok(false);
    };
    if !bot_no_effect_proof_matches(vault_id, request_id, &proof) {
        return Err("Backend returned a mismatched or malformed no-effect proof".into());
    }
    if !history::mark_no_effect_acknowledgement_pending(
        vault_id,
        request_id,
        config.backend_principal,
        proof.transfer_digest.clone(),
    ) {
        return Err("Could not persist the exact backend no-effect proof before ACK".into());
    }
    call_bot_acknowledge_no_effect(
        config,
        vault_id,
        request_id,
        proof.transfer_digest.clone(),
    )
    .await?;
    if !history::clear_acknowledged_no_effect_claim_intent(
        vault_id,
        request_id,
        config.backend_principal,
        &proof.transfer_digest,
    ) {
        return Err("Backend ACK succeeded but the exact durable bot intent could not be cleared".into());
    }
    Ok(true)
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

pub async fn process_pending() {
    // Do not acquire new collateral claims while their payment output cannot
    // be attributed to a claim generation. Leave the full-set notification in
    // the queue for later activation; the 30-second timer returns cheaply and
    // never spins a claim retry. Explicit admin recovery enters through
    // `process_specific_vault` and remains available for existing journals.
    if !AUTOMATIC_CLAIM_SWAP_ENABLED
        && !state::read_state(|s| {
            s.pending_vaults.iter().any(|vault| {
                matches!(
                    history::get_claim_intent(vault.vault_id).map(|intent| intent.phase),
                    Some(history::BotClaimIntentPhase::NoEffectAcknowledgementPending { .. })
                )
            })
        })
    {
        return;
    }

    let guard = match crate::ProcessingGuard::acquire() {
        Ok(g) => g,
        Err(_) => return, // Another liquidation is already in flight
    };

    let vault = state::mutate_state(|s| {
        take_pending_vault_for_automatic_processing(
            &mut s.pending_vaults,
            AUTOMATIC_CLAIM_SWAP_ENABLED,
        )
    });
    let Some(vault) = vault else { return };

    process_specific_vault(vault, guard).await;
}

fn take_pending_vault_for_automatic_processing(
    pending_vaults: &mut Vec<crate::state::LiquidatableVaultInfo>,
    automatic_claim_swap_enabled: bool,
) -> Option<crate::state::LiquidatableVaultInfo> {
    if automatic_claim_swap_enabled {
        pending_vaults.pop()
    } else {
        // A typed no-effect ACK only clears the exact old request receipt; it
        // cannot acquire collateral. Leave every other pending candidate in
        // place so timer ticks do not drop it or spin through claim retries.
        let index = pending_vaults.iter().rposition(|vault| {
            matches!(
                history::get_claim_intent(vault.vault_id).map(|intent| intent.phase),
                Some(history::BotClaimIntentPhase::NoEffectAcknowledgementPending { .. })
            )
        })?;
        Some(pending_vaults[index].clone())
    }
}

fn remove_pending_vault(
    pending_vaults: &mut Vec<crate::state::LiquidatableVaultInfo>,
    vault_id: u64,
) {
    pending_vaults.retain(|queued| queued.vault_id != vault_id);
}

/// Process a specific vault while owning the already-acquired global guard.
/// Admin receipt requeue passes the selected vault directly so full-set
/// notifications cannot erase the request before work begins.
pub(crate) async fn process_specific_vault(
    vault: LiquidatableVaultInfo,
    guard: crate::ProcessingGuard,
) {
    let _guard = guard;
    crate::ProcessingGuard::set_vault_id(vault.vault_id);
    // A full-set notification may have copied this vault into the queue while
    // an earlier await was open. The stable intent, not that stale queue copy,
    // controls whether recovery or a fresh claim is allowed.
    let retain_for_paused_ack = !AUTOMATIC_CLAIM_SWAP_ENABLED
        && matches!(
            history::get_claim_intent(vault.vault_id).map(|intent| intent.phase),
            Some(history::BotClaimIntentPhase::NoEffectAcknowledgementPending { .. })
        );
    if !retain_for_paused_ack {
        state::mutate_state(|s| remove_pending_vault(&mut s.pending_vaults, vault.vault_id));
    }

    let mut config = match state::read_state(|s| s.config.clone()) {
        Some(c) => c,
        None => {
            log!(crate::INFO, "Bot not configured, skipping vault #{}", vault.vault_id);
            return;
        }
    };

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
    // The stable phase decides whether this is a same-ID receipt recovery, a
    // one-time resume before dispatch, or a quarantine after swap dispatch.
    let prior_intent = history::get_claim_intent(vault.vault_id);
    let recovering_claimed = matches!(
        prior_intent.as_ref().map(|intent| &intent.phase),
        Some(history::BotClaimIntentPhase::Claimed(_))
    );
    let fresh_id = prior_intent.is_none().then(history::next_id).unwrap_or(0);
    let request_id = match claim_intent_action(prior_intent.clone(), fresh_id) {
        ClaimIntentAction::Retry(request_id) => {
            if prior_intent.as_ref().and_then(|intent| intent.backend_principal.as_ref())
                != Some(&config.backend_principal)
            {
                log!(crate::INFO, "Holding claim receipt for vault #{}: persisted backend identity is missing or differs from current configuration", vault.vault_id);
                return;
            }
            request_id
        }
        ClaimIntentAction::RecoverClaimed(request_id) => {
            if prior_intent.as_ref().and_then(|intent| intent.backend_principal.as_ref())
                != Some(&config.backend_principal)
            {
                log!(crate::INFO, "Holding claimed receipt for vault #{}: persisted backend identity is missing or differs from current configuration", vault.vault_id);
                return;
            }
            request_id
        }
        ClaimIntentAction::AcknowledgeNoEffect { request_id, transfer_digest } => {
            if prior_intent.as_ref().and_then(|intent| intent.backend_principal.as_ref())
                != Some(&config.backend_principal)
            {
                log!(crate::INFO, "Holding no-effect proof for vault #{}: persisted backend identity is missing or differs from current configuration", vault.vault_id);
                return;
            }
            match call_bot_acknowledge_no_effect(
                &config,
                vault.vault_id,
                request_id,
                transfer_digest.clone(),
            ).await {
                Ok(()) => {
                    if !history::clear_acknowledged_no_effect_claim_intent(
                        vault.vault_id,
                        request_id,
                        config.backend_principal,
                        &transfer_digest,
                    ) {
                        log!(crate::INFO, "Holding vault #{}: backend ACK succeeded but the persisted no-effect proof changed", vault.vault_id);
                        return;
                    }
                    // Keep the queue entry across the ACK await so an
                    // interruption can retry reconciliation. Remove it only
                    // after the exact ACK and durable journal clear succeed.
                    state::mutate_state(|s| remove_pending_vault(&mut s.pending_vaults, vault.vault_id));
                    if matches!(next_claim_retry_action(prior_retry_count, CLAIM_RETRY_LIMIT), ClaimRetryAction::Retry { .. }) {
                        enqueue_claim_retry(&vault);
                    } else {
                        state::mutate_state(|s| { s.claim_retry_counts.remove(&vault.vault_id); });
                    }
                }
                Err(error) => {
                    log!(crate::INFO, "No-effect ACK remains pending for vault #{} request {}: {}", vault.vault_id, request_id, error);
                    // ACK retries are protocol reconciliation, not new claim
                    // attempts, so keep retrying this exact ACK even at the
                    // claim retry limit. The durable phase prevents re-claim.
                    enqueue_claim_retry(&vault);
                }
            }
            return;
        }
        ClaimIntentAction::HoldReturnRecovery(request_id) => {
            log!(crate::INFO, "Holding vault #{} claim {} because an exact collateral-return tuple is already journaled; resume through admin return recovery", vault.vault_id, request_id);
            return;
        }
        ClaimIntentAction::QuarantineSwapStarted(request_id) => {
            log!(crate::INFO, "Quarantining vault #{} claim {} because its durable phase says the swap may already have started; no automatic replay", vault.vault_id, request_id);
            return;
        }
        ClaimIntentAction::HoldLegacyAcquired(request_id) => {
            log!(crate::INFO, "Holding legacy acquired claim for vault #{} (request {}); its pre-swap versus post-swap phase is unknown", vault.vault_id, request_id);
            return;
        }
        ClaimIntentAction::Create(request_id) => {
            history::put_claim_intent(history::BotClaimIntent {
                vault_id: vault.vault_id,
                request_id,
                claim_call_count: Some(0),
                backend_principal: Some(config.backend_principal.clone()),
                return_transfer: None,
                ckusdc_top_up_transfer: None,
                ckusdc_payment_transfer: None,
                shortfall_eligibility: None,
                icp_pool_input_fee_e8s: None,
                phase: history::BotClaimIntentPhase::ClaimRequested,
            });
            request_id
        }
    };
    let record_id = request_id;
    let timestamp = ic_cdk::api::time();

    // -- Phase 1: CLAIM --
    let prior_request_calls = if recovering_claimed {
        Some(0)
    } else {
        match history::begin_claim_request(vault.vault_id, request_id) {
            Some(prior_calls) => prior_calls,
            None => {
                log!(crate::INFO, "Holding vault #{}: durable same-ID claim request could not be advanced before backend dispatch", vault.vault_id);
                return;
            }
        }
    };
    let liq_result = call_bot_claim_liquidation(&config, vault.vault_id, request_id).await;
    let (collateral_amount, debt_covered, collateral_price, claim_timestamp, payment_memo) = match liq_result {
        Ok(r) => {
            if r.vault_id != vault.vault_id {
                log!(crate::INFO, "Holding mismatched backend claim response for vault #{}: response named vault #{}", vault.vault_id, r.vault_id);
                return;
            }
            if recovering_claimed {
                let expected = prior_intent.as_ref().and_then(|intent| match &intent.phase {
                    history::BotClaimIntentPhase::Claimed(receipt) => Some(receipt),
                    _ => None,
                });
                if !expected.is_some_and(|receipt| bot_claim_receipt_matches(vault.vault_id, receipt, &r)) {
                    log!(crate::INFO, "Holding backend claim recovery for vault #{} because its receipt differs from the durable claimed phase", vault.vault_id);
                    return;
                }
            }
            state::mutate_state(|s| {
                s.claim_retry_counts.remove(&vault.vault_id);
            });
            let (Some(claim_timestamp), Some(payment_memo)) = (r.claim_timestamp, r.payment_memo) else {
                let msg = "backend claim response omitted SAT-001 confirmation binding".to_string();
                // A generation-bound return subaccount cannot be derived
                // without the timestamp. Keep the collateral quarantined and
                // the claim untouched for operator recovery instead of
                // returning to an unbound shared account or cancelling it.
                write_record(LiquidationRecordV1 {
                    id: record_id, vault_id: vault.vault_id, timestamp,
                    status: LiquidationStatus::TransferFailed,
                    collateral_claimed_e8s: r.collateral_amount, debt_to_cover_e8s: r.debt_covered,
                    icp_swapped_e8s: 0, ckusdc_received_e6: 0, ckusdc_transferred_e6: 0,
                    icp_to_treasury_e8s: 0, oracle_price_e8s: r.collateral_price_e8s,
                    effective_price_e8s: 0, slippage_bps: 0,
                    error_message: Some(format!("{}; collateral remains at bot and claim remains open for recovery", msg)),
                    confirm_retry_count: 0, claim_timestamp: None, payment_memo: None,
                    ckusdc_payment_block_index: None, ckusdc_payment_amount_e6: None,
                    icp_treasury_transfer: None, icp_treasury_bonus_state: None,
                });
                log!(crate::INFO, "STUCK: {} for vault #{}; claim retained for operator recovery", msg, vault.vault_id);
                return;
            };
            if payment_memo.is_empty() {
                ic_cdk::trap("backend returned an empty SAT-001 claim memo");
            }
            let claim_transfer = r.claim_transfer.clone();
            history::put_claim_intent(history::BotClaimIntent {
                vault_id: vault.vault_id,
                request_id,
                claim_call_count: history::get_claim_intent(vault.vault_id)
                    .and_then(|intent| intent.claim_call_count)
                    .or(Some(1)),
                backend_principal: Some(config.backend_principal.clone()),
                return_transfer: prior_intent.as_ref().and_then(|intent| intent.return_transfer.clone()),
                ckusdc_top_up_transfer: prior_intent.as_ref().and_then(|intent| intent.ckusdc_top_up_transfer.clone()),
                ckusdc_payment_transfer: prior_intent.as_ref().and_then(|intent| intent.ckusdc_payment_transfer.clone()),
                shortfall_eligibility: prior_intent.as_ref().and_then(|intent| intent.shortfall_eligibility.clone()),
                icp_pool_input_fee_e8s: prior_intent.as_ref().and_then(|intent| intent.icp_pool_input_fee_e8s),
                phase: history::BotClaimIntentPhase::Claimed(history::BotClaimReceipt {
                    collateral_amount: r.collateral_amount,
                    debt_covered: r.debt_covered,
                    collateral_price_e8s: r.collateral_price_e8s,
                    claim_timestamp,
                    payment_memo: payment_memo.clone(),
                    claim_transfer: claim_transfer.clone(),
                }),
            });
            if !claim_transfer.as_ref().is_some_and(|proof| claim_transfer_receipt_is_complete(
                config.backend_principal,
                ic_cdk::id(),
                config.icp_ledger,
                vault.vault_id,
                claim_timestamp,
                r.collateral_amount,
                proof,
            )) {
                let message = "backend claim lacks complete exact collateral-transfer provenance; claim remains held and no return or payment is authorized";
                write_record(LiquidationRecordV1 {
                    id: record_id, vault_id: vault.vault_id, timestamp,
                    status: LiquidationStatus::TransferFailed,
                    collateral_claimed_e8s: r.collateral_amount, debt_to_cover_e8s: r.debt_covered,
                    icp_swapped_e8s: 0, ckusdc_received_e6: 0, ckusdc_transferred_e6: 0,
                    icp_to_treasury_e8s: 0, oracle_price_e8s: r.collateral_price_e8s,
                    effective_price_e8s: 0, slippage_bps: 0,
                    error_message: Some(message.into()), confirm_retry_count: 0,
                    claim_timestamp: Some(claim_timestamp), payment_memo: Some(payment_memo.clone()),
                    ckusdc_payment_block_index: None, ckusdc_payment_amount_e6: None,
                    icp_treasury_transfer: None, icp_treasury_bonus_state: None,
                });
                log!(crate::INFO, "STUCK: {} for vault #{} generation {}", message, vault.vault_id, claim_timestamp);
                return;
            }
            (r.collateral_amount, r.debt_covered, r.collateral_price_e8s, claim_timestamp, payment_memo)
        }
        Err(e) => {
            if recovering_claimed {
                // A recovery lookup failure, including NotFound, is not proof
                // that the original transfer or claim did not happen. Keep
                // the positive durable receipt and its history untouched.
                match call_bot_claim_no_effect_status(&config, vault.vault_id, request_id).await {
                    Ok(None) => {}
                    Ok(Some(proof)) if bot_no_effect_proof_matches(vault.vault_id, request_id, &proof) => {
                        log!(crate::INFO, "Holding previously claimed vault #{}: backend returned a contradictory no-effect proof for an already claimed request {}", vault.vault_id, request_id);
                    }
                    Ok(Some(_)) => {
                        log!(crate::INFO, "Holding previously claimed vault #{}: backend returned a mismatched no-effect proof", vault.vault_id);
                    }
                    Err(status_error) => {
                        log!(crate::INFO, "Holding previously claimed vault #{} after no-effect status check failed: {}", vault.vault_id, status_error);
                    }
                }
                log!(crate::INFO, "Holding previously claimed vault #{} after exact-ID receipt recovery failed: {}", vault.vault_id, e);
                return;
            }
            let action = next_claim_retry_action(prior_retry_count, CLAIM_RETRY_LIMIT);
            let (request_id_released, proof_recovery_failed) =
                match resolve_no_effect_receipt(&config, vault.vault_id, request_id).await {
                    Ok(released) => (released, false),
                    Err(recovery_error) => {
                        log!(crate::INFO, "No-effect status/ACK reconciliation is incomplete for vault #{} request {}: {}", vault.vault_id, request_id, recovery_error);
                        (false, true)
                    }
                };
            let no_effect_ack_pending = history::get_claim_intent(vault.vault_id).is_some_and(|intent| {
                intent.request_id == request_id
                    && matches!(intent.phase, history::BotClaimIntentPhase::NoEffectAcknowledgementPending { .. })
            });
            let unresolved = claim_call_is_unresolved(prior_request_calls, &e)
                || proof_recovery_failed
                || no_effect_ack_pending;
            let status = if unresolved {
                LiquidationStatus::TransferFailed
            } else {
                LiquidationStatus::ClaimFailed
            };
            let outcome = if no_effect_ack_pending {
                format!("exact no-effect proof is durable and its backend ACK is pending; no claim will be replayed; {}", e)
            } else if request_id_released {
                format!("exact backend no-effect proof was acknowledged; request ID {} was released, so a later attempt can use a fresh ID; {}", request_id, e)
            } else if proof_recovery_failed {
                format!("backend no-effect proof could not be verified or acknowledged; same request ID {} remains held; {}", request_id, e)
            } else if unresolved {
                format!("same request ID {} remains durably held for recovery; {}", request_id, e)
            } else {
                format!("backend returned a typed rejection; same request ID {} is retained for retry; {}", request_id, e)
            };
            let (status, message) = match &action {
                ClaimRetryAction::Retry { new_count } => (
                    status.clone(),
                    format!("Attempt {}/{}: {}", new_count, CLAIM_RETRY_LIMIT, outcome),
                ),
                ClaimRetryAction::GiveUp => (
                    status,
                    if unresolved {
                        format!("Final attempt {}/{} (automatic retries stopped; {})", CLAIM_RETRY_LIMIT, CLAIM_RETRY_LIMIT, outcome)
                    } else {
                        format!("Final attempt {}/{} (automatic retries stopped; {}; safe to continue normal cascade handling)", CLAIM_RETRY_LIMIT, CLAIM_RETRY_LIMIT, outcome)
                    },
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
                claim_timestamp: None, payment_memo: None,
                ckusdc_payment_block_index: None, ckusdc_payment_amount_e6: None,
                icp_treasury_transfer: None, icp_treasury_bonus_state: None,
            });
            match action {
                ClaimRetryAction::Retry { new_count } => {
                    state::mutate_state(|s| {
                        s.claim_retry_counts.insert(vault.vault_id, new_count);
                    });
                    enqueue_claim_retry(&vault);
                }
                ClaimRetryAction::GiveUp => {
                    if no_effect_ack_pending {
                        // This is not another claim attempt. Retain the limit
                        // marker, and keep the vault queued only to finish the
                        // exact ACK; the ACK phase can never issue a claim.
                        enqueue_claim_retry(&vault);
                    } else {
                        state::mutate_state(|s| {
                            s.claim_retry_counts.remove(&vault.vault_id);
                        });
                    }
                }
            }
            return;
        }
    };

    // ICPSwap's current deposit-and-swap API pays the bot's shared default
    // account and provides no claim-specific destination or independently
    // attributable output receipt. Keep the seized collateral and generation
    // journal held until an operator funds this claim's dedicated payment
    // account and submits its exact transfer receipt.
    write_record(LiquidationRecordV1 {
        id: record_id,
        vault_id: vault.vault_id,
        timestamp,
        status: LiquidationStatus::SwapFailed,
        collateral_claimed_e8s: collateral_amount,
        debt_to_cover_e8s: debt_covered,
        icp_swapped_e8s: 0,
        ckusdc_received_e6: 0,
        ckusdc_transferred_e6: 0,
        icp_to_treasury_e8s: 0,
        oracle_price_e8s: collateral_price,
        effective_price_e8s: 0,
        slippage_bps: 0,
        error_message: Some(
            "automatic swap paused: pool output lacks claim-specific attribution; use explicit claim-bound ckUSDC funding and payment recovery".into(),
        ),
        confirm_retry_count: 0,
        claim_timestamp: Some(claim_timestamp),
        payment_memo: Some(payment_memo.clone()),
        ckusdc_payment_block_index: None,
        ckusdc_payment_amount_e6: None,
        icp_treasury_transfer: None,
        icp_treasury_bonus_state: None,
    });
    if !AUTOMATIC_CLAIM_SWAP_ENABLED {
        log!(crate::INFO, "STUCK: vault #{} claim held; automatic swap/payment disabled until pool output is claim-attributable", vault.vault_id);
        return;
    }

    #[cfg(any())]
    {
    // -- Phase 2: SWAP ICP -> ckUSDC --
    let swap_amount = calculate_swap_amount(collateral_amount, debt_covered, collateral_price);

    // Per-claim reservation: bracket the swap with wallet balance reads so
    // we know the EXACT ckUSDC this claim earned, independent of any leftover
    // balance or what the swap router claims. The transfer in Phase 3 spends
    // only this delta (see compute_swap_reservation for rationale).
    //
    // Without this first snapshot, the swap output cannot be attributed to
    // this claim. Treat failure as a pre-dispatch error and return collateral.
    let bal_before_swap = swap::balance_of_self_ckusdc(&config).await.map_err(|e| {
        log!(crate::INFO, "Pre-swap balance read failed for vault #{}: {} (swap will not be dispatched)", vault.vault_id, e);
        e
    }).ok();

    // Fetch both live ledger fees before the irreversible pool call. The ICP
    // fee is the pool's transfer_from input fee and is pinned with SwapStarted;
    // the ckUSDC fee covers the pool output and debt-covering payment floor.
    let pre_swap_quote = if bal_before_swap.is_none() {
        Err("Pre-swap ckUSDC balance is unreadable; no swap was dispatched".to_string())
    } else {
        match swap::fetch_ledger_fee(config.icp_ledger).await {
            Ok(icp_fee) => {
                config.icp_fee_e8s = Some(icp_fee);
                match swap::fetch_ledger_fee(config.ckusdc_ledger).await {
                    Ok(ckusdc_fee) => {
                        config.ckusdc_fee_e6 = Some(ckusdc_fee);
                        let required_payment = (debt_covered / 100)
                            .saturating_add(u64::from(debt_covered % 100 != 0))
                            .saturating_add(ckusdc_fee);
                        swap::quote_icp_for_ckusdc_with_floor(&config, swap_amount, required_payment).await
                    }
                    Err(error) => Err(format!("Could not fetch live ckUSDC fee before swap: {error}")),
                }
            }
            Err(error) => Err(format!("Could not fetch live ICP input fee before swap: {error}")),
        }
    };
    let swap_result = match pre_swap_quote {
        Err(error) => Err(SwapAttemptError::NotStarted(error)),
        Ok((quoted_output, min_output)) => {
            let pool_input_fee_e8s = config.icp_fee_e8s.unwrap_or(swap::FALLBACK_LEDGER_FEE);
            if !mark_claim_swap_started(vault.vault_id, request_id, pool_input_fee_e8s) {
                log!(crate::INFO, "Holding claim for vault #{}: could not persist SwapStarted before ICPSwap dispatch", vault.vault_id);
                return;
            }
            config.icp_fee_e8s = Some(pool_input_fee_e8s);
            swap::execute_quoted_icp_for_ckusdc(
                &config,
                swap_amount,
                min_output,
                quoted_output,
            )
            .await
            .map_err(SwapAttemptError::StartedOutcomeUnknown)
        }
    };

    let (router_received, effective_price) = match swap_result {
        Ok(r) => (r.ckusdc_received_e6, r.effective_price_e8s),
        Err(SwapAttemptError::StartedOutcomeUnknown(swap_err)) => {
            let message = format!(
                "ICPSwap was dispatched with durable SwapStarted evidence but returned no proven result: {}; claim and collateral remain quarantined for independent output reconciliation",
                swap_err
            );
            log!(crate::INFO, "STUCK: {} for vault #{}; no second swap, collateral return, or cancellation will be attempted", message, vault.vault_id);
            write_record(LiquidationRecordV1 {
                id: record_id, vault_id: vault.vault_id, timestamp,
                status: LiquidationStatus::TransferFailed,
                collateral_claimed_e8s: collateral_amount, debt_to_cover_e8s: debt_covered,
                icp_swapped_e8s: 0, ckusdc_received_e6: 0, ckusdc_transferred_e6: 0,
                icp_to_treasury_e8s: 0, oracle_price_e8s: collateral_price,
                effective_price_e8s: 0, slippage_bps: 0,
                error_message: Some(message), confirm_retry_count: 0,
                claim_timestamp: Some(claim_timestamp), payment_memo: Some(payment_memo.clone()),
                ckusdc_payment_block_index: None, ckusdc_payment_amount_e6: None,
                icp_treasury_transfer: None, icp_treasury_bonus_state: None,
            });
            return;
        }
        Err(SwapAttemptError::NotStarted(swap_err)) => {
            log!(crate::INFO, "Swap failed for vault #{}: {}. Returning ICP.", vault.vault_id, swap_err);

            // Step 1: return seized ICP to the backend.
            let return_result = dispatch_claim_collateral_return(
                &config,
                vault.vault_id,
                collateral_amount,
                claim_timestamp,
            )
            .await;

            // Step 2: cancel the protocol-side claim, only if the return succeeded.
            // The Wave-12 BOT-001b balance gate rejects cancel until the protocol's
            // collateral balance is back to (>=) `claim.collateral_amount - fee`,
            // so attempting cancel after a failed return is pointless and would
            // just produce noisy `[BOT-001b] cancel rejected` log lines.
            let cancel_err = if return_result.is_ok() {
                let return_block_index = return_result.as_ref().copied().expect("checked successful return");
                let mut last_err = String::new();
                let mut succeeded = false;
                let mut attempts: u8 = 0;
                for attempt in 0..CANCEL_ATTEMPTS {
                    attempts = attempt + 1;
                    match call_bot_cancel_liquidation(&config, vault.vault_id, claim_timestamp, return_block_index).await {
                        Ok(()) => match call_bot_acknowledge_claim_cancellation(
                            &config,
                            vault.vault_id,
                            claim_timestamp,
                        ).await {
                            Ok(()) => {
                                succeeded = true;
                                break;
                            }
                            Err(e) => last_err = format!("cancellation completed but generation ACK failed: {e}"),
                        },
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
                return_result.as_ref().err().map(|s| s.as_str()),
                cancel_err.as_ref().map(|(n, e)| (*n, e.as_str())),
            );
            let claim_cancelled = outcome.status == LiquidationStatus::SwapFailed;

            if let Some(line) = &outcome.stuck_log {
                log!(crate::INFO, "{}", line);
            }

            write_record(LiquidationRecordV1 {
                id: record_id, vault_id: vault.vault_id, timestamp,
                status: outcome.status,
                collateral_claimed_e8s: collateral_amount, debt_to_cover_e8s: debt_covered,
                icp_swapped_e8s: swap_amount, ckusdc_received_e6: 0, ckusdc_transferred_e6: 0,
                icp_to_treasury_e8s: 0, oracle_price_e8s: collateral_price,
                effective_price_e8s: 0, slippage_bps: 0,
                error_message: Some(outcome.error_message), confirm_retry_count: 0,
                claim_timestamp: Some(claim_timestamp), payment_memo: Some(payment_memo.clone()),
                ckusdc_payment_block_index: None, ckusdc_payment_amount_e6: None,
                icp_treasury_transfer: None, icp_treasury_bonus_state: None,
            });
            if claim_cancelled {
                history::remove_claim_intent(vault.vault_id);
            }
            return;
        }
    };

    let slippage_bps = calculate_slippage(effective_price, collateral_price);

    // Per-claim reservation requires both successful reads. The router's
    // returned amount alone cannot establish claim-specific wallet credit.
    let bal_after_swap = swap::balance_of_self_ckusdc(&config).await.map_err(|e| {
        log!(crate::INFO, "Post-swap balance read failed for vault #{}: {} (claim-specific output cannot be proven)", vault.vault_id, e);
        e
    }).ok();
    let reservation = match compute_swap_reservation_from_snapshots(
        vault.vault_id,
        router_received,
        bal_before_swap,
        bal_after_swap,
    ) {
        Ok(reservation) => reservation,
        Err(error) => {
            let message = format!("Swap completed but claim-specific ckUSDC wallet delta is unproven; no ckUSDC payment was attempted and the claim remains held for operator reconciliation: {}", error);
            log!(crate::INFO, "STUCK: {} for vault #{}", message, vault.vault_id);
            write_record(LiquidationRecordV1 {
                id: record_id, vault_id: vault.vault_id, timestamp,
                status: LiquidationStatus::TransferFailed,
                collateral_claimed_e8s: collateral_amount, debt_to_cover_e8s: debt_covered,
                icp_swapped_e8s: swap_amount, ckusdc_received_e6: 0,
                ckusdc_transferred_e6: 0, icp_to_treasury_e8s: 0,
                oracle_price_e8s: collateral_price, effective_price_e8s: effective_price,
                slippage_bps, error_message: Some(message), confirm_retry_count: 0,
                claim_timestamp: Some(claim_timestamp), payment_memo: Some(payment_memo.clone()),
                ckusdc_payment_block_index: None, ckusdc_payment_amount_e6: None,
                icp_treasury_transfer: None, icp_treasury_bonus_state: None,
            });
            return;
        }
    };
    if let Some(note) = &reservation.discrepancy_note {
        log!(crate::INFO, "[per-claim-reservation] {}", note);
    }
    let ckusdc_received = reservation.recorded_received_e6;

    // The quote's amount_out_minimum protects the pool call, while the wallet
    // delta above protects against pool/router reporting mismatches. Keep both
    // checks: an anomalously short delta must not become an accepted but
    // permanently unconfirmable payment block. The swap may already have
    // consumed collateral, so retain SwapStarted and the backend claim for
    // operator reconciliation rather than returning collateral or canceling.
    // Pin the live ledger fee before checking the recipient-credit floor or
    // building the immutable transfer tuple. A cached configuration value can
    // understate a fee change and produce an unconfirmable short payment.
    let ckusdc_fee_e6 = match swap::fetch_ledger_fee(config.ckusdc_ledger).await {
        Ok(fee) => fee,
        Err(error) => {
            let message = format!("Could not fetch live ckUSDC transfer fee; no payment was attempted and the claim remains held: {}", error);
            write_record(LiquidationRecordV1 {
                id: record_id, vault_id: vault.vault_id, timestamp,
                status: LiquidationStatus::TransferFailed,
                collateral_claimed_e8s: collateral_amount, debt_to_cover_e8s: debt_covered,
                icp_swapped_e8s: swap_amount, ckusdc_received_e6: ckusdc_received,
                ckusdc_transferred_e6: 0, icp_to_treasury_e8s: 0,
                oracle_price_e8s: collateral_price, effective_price_e8s: effective_price,
                slippage_bps, error_message: Some(message), confirm_retry_count: 0,
                claim_timestamp: Some(claim_timestamp), payment_memo: Some(payment_memo.clone()),
                ckusdc_payment_block_index: None, ckusdc_payment_amount_e6: None,
                icp_treasury_transfer: None, icp_treasury_bonus_state: None,
            });
            return;
        }
    };
    let minimum_payment_e6 = (debt_covered / 100) + u64::from(debt_covered % 100 != 0);
    if let Some(shortfall_e6) = ckusdc_payment_shortfall_e6(
        reservation.to_transfer_e6,
        ckusdc_fee_e6,
        minimum_payment_e6,
    ) {
        let net_payment_e6 = reservation.to_transfer_e6.checked_sub(ckusdc_fee_e6).unwrap_or(0);
        // Persist eligibility synchronously before recording the held result.
        // The history CAS validates the exact SwapStarted receipt and refuses
        // any claim that already has a payment, top-up, or return tuple.
        let marker_saved = history::mark_ckusdc_shortfall_eligibility(
            vault.vault_id,
            &history::BotClaimReceipt {
                collateral_amount,
                debt_covered,
                collateral_price_e8s: collateral_price,
                claim_timestamp,
                payment_memo: payment_memo.clone(),
                claim_transfer: history::get_claim_intent(vault.vault_id).and_then(|intent| match intent.phase {
                    history::BotClaimIntentPhase::Claimed(receipt) | history::BotClaimIntentPhase::SwapStarted(receipt) => receipt.claim_transfer,
                    _ => None,
                }),
            },
            history::BotCkUsdcShortfallEligibility {
                ledger: config.ckusdc_ledger.clone(),
                measured_reserved_output_e6: reservation.to_transfer_e6,
                minimum_payment_e6,
            },
        );
        let marker_note = if marker_saved {
            "durable shortfall eligibility recorded".to_string()
        } else {
            "durable shortfall eligibility could not be recorded because the exact generation or no-prior-transfer invariant failed".to_string()
        };
        let message = format!(
            "Swap output shortfall: wallet delta can pay only {} ckUSDC e6 net, below the {} e6 claim minimum by {} e6 after the configured {} e6 transfer fee. No ckUSDC payment or backend confirmation was attempted; the swap may have consumed collateral, so the claim and durable SwapStarted evidence remain held for operator reconciliation. {}.",
            net_payment_e6,
            minimum_payment_e6,
            shortfall_e6,
            ckusdc_fee_e6,
            marker_note,
        );
        log!(crate::INFO, "STUCK: {} for vault #{}", message, vault.vault_id);
        write_record(LiquidationRecordV1 {
            id: record_id, vault_id: vault.vault_id, timestamp,
            status: LiquidationStatus::TransferFailed,
            collateral_claimed_e8s: collateral_amount, debt_to_cover_e8s: debt_covered,
            icp_swapped_e8s: swap_amount, ckusdc_received_e6: ckusdc_received,
            ckusdc_transferred_e6: 0, icp_to_treasury_e8s: 0,
            oracle_price_e8s: collateral_price, effective_price_e8s: effective_price,
            slippage_bps, error_message: Some(message), confirm_retry_count: 0,
            claim_timestamp: Some(claim_timestamp), payment_memo: Some(payment_memo.clone()),
            ckusdc_payment_block_index: None, ckusdc_payment_amount_e6: None,
            icp_treasury_transfer: None, icp_treasury_bonus_state: None,
        });
        return;
    }

    // Persist the exact ledger tuple before dispatch. If the call reply is
    // ambiguous, recovery can only replay this identical fee/memo/time/amount.
    let mut payment_transfer = match swap::prepare_ckusdc_payment_transfer(
        &config,
        reservation.to_transfer_e6,
        &payment_memo,
        ckusdc_fee_e6,
        // The backend claim can be stamped a nanosecond later than this
        // callback's time. The payment identity must not predate its claim.
        ic_cdk::api::time().max(claim_timestamp),
    ) {
        Ok(transfer) => transfer,
        Err(error) => {
            write_record(LiquidationRecordV1 {
                id: record_id, vault_id: vault.vault_id, timestamp,
                status: LiquidationStatus::TransferFailed,
                collateral_claimed_e8s: collateral_amount, debt_to_cover_e8s: debt_covered,
                icp_swapped_e8s: swap_amount, ckusdc_received_e6: ckusdc_received,
                ckusdc_transferred_e6: 0, icp_to_treasury_e8s: 0,
                oracle_price_e8s: collateral_price, effective_price_e8s: effective_price,
                slippage_bps, error_message: Some(error), confirm_retry_count: 0,
                claim_timestamp: Some(claim_timestamp), payment_memo: Some(payment_memo.clone()),
                ckusdc_payment_block_index: None, ckusdc_payment_amount_e6: None,
                icp_treasury_transfer: None, icp_treasury_bonus_state: None,
            });
            return;
        }
    };
    if !history::update_ckusdc_payment_transfer(vault.vault_id, payment_transfer.clone()) {
        log!(crate::INFO, "STUCK: could not persist original ckUSDC transfer identity for vault #{}; no ledger call made", vault.vault_id);
        write_record(LiquidationRecordV1 {
            id: record_id, vault_id: vault.vault_id, timestamp,
            status: LiquidationStatus::TransferFailed,
            collateral_claimed_e8s: collateral_amount, debt_to_cover_e8s: debt_covered,
            icp_swapped_e8s: swap_amount, ckusdc_received_e6: ckusdc_received,
            ckusdc_transferred_e6: 0, icp_to_treasury_e8s: 0,
            oracle_price_e8s: collateral_price, effective_price_e8s: effective_price,
            slippage_bps, error_message: Some("original ckUSDC transfer journal persistence failed; no payment was attempted".into()), confirm_retry_count: 0,
            claim_timestamp: Some(claim_timestamp), payment_memo: Some(payment_memo.clone()),
            ckusdc_payment_block_index: None, ckusdc_payment_amount_e6: None,
            icp_treasury_transfer: None, icp_treasury_bonus_state: None,
        });
        return;
    }

    // -- Phase 3: TRANSFER ckUSDC to backend. --
    let transfer_result = dispatch_and_verify_ckusdc_payment(
        &config, vault.vault_id, claim_timestamp, &payment_memo, &mut payment_transfer,
    ).await;

    let payment_receipt = match transfer_result {
        Ok(receipt) => receipt,
        Err(e) => {
            let stuck_msg = match &reservation.discrepancy_note {
                Some(note) => format!("{} | transfer: {}", note, e),
                None => e,
            };
            log!(crate::INFO,
                "STUCK: ckUSDC transfer failed for vault #{}. Bot holding {} ckUSDC e6 (router said {}). Error: {}. Needs admin resolution.",
                vault.vault_id, reservation.to_transfer_e6, router_received, stuck_msg);
            write_record(LiquidationRecordV1 {
                id: record_id, vault_id: vault.vault_id, timestamp,
                status: LiquidationStatus::TransferFailed,
                collateral_claimed_e8s: collateral_amount, debt_to_cover_e8s: debt_covered,
                icp_swapped_e8s: swap_amount, ckusdc_received_e6: ckusdc_received,
                ckusdc_transferred_e6: 0, icp_to_treasury_e8s: 0,
                oracle_price_e8s: collateral_price, effective_price_e8s: effective_price,
                slippage_bps, error_message: Some(stuck_msg), confirm_retry_count: 0,
                claim_timestamp: Some(claim_timestamp), payment_memo: Some(payment_memo.clone()),
                ckusdc_payment_block_index: None, ckusdc_payment_amount_e6: None,
                icp_treasury_transfer: None, icp_treasury_bonus_state: None,
            });
            return;
        }
    };
    let ckusdc_transferred = payment_receipt.amount_e6;

    // Persist the accepted ckUSDC block and claim generation before the first
    // backend confirmation call. If its reply is lost, SAT-001 admin recovery
    // can replay this exact proof rather than inferring payment from status.
    write_record(LiquidationRecordV1 {
        id: record_id, vault_id: vault.vault_id, timestamp,
        status: LiquidationStatus::ConfirmFailed,
        collateral_claimed_e8s: collateral_amount, debt_to_cover_e8s: debt_covered,
        icp_swapped_e8s: swap_amount, ckusdc_received_e6: ckusdc_received,
        ckusdc_transferred_e6: ckusdc_transferred, icp_to_treasury_e8s: 0,
        oracle_price_e8s: collateral_price, effective_price_e8s: effective_price,
        slippage_bps, error_message: Some("ckUSDC payment accepted; backend confirmation pending".into()),
        confirm_retry_count: 0, claim_timestamp: Some(claim_timestamp),
        payment_memo: Some(payment_memo.clone()),
        ckusdc_payment_block_index: Some(payment_receipt.block_index),
        ckusdc_payment_amount_e6: Some(payment_receipt.amount_e6),
        icp_treasury_transfer: None, icp_treasury_bonus_state: None,
    });

    // -- Phase 4: CONFIRM (with retry, idempotent) --
    let mut confirm_ok = false;
    let mut confirm_retries: u8 = 0;
    let mut last_confirm_err = String::new();

    for attempt in 0..CONFIRM_ATTEMPTS {
        match call_bot_confirm_liquidation(
            &config, vault.vault_id, claim_timestamp, payment_receipt.block_index,
        ).await {
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
            "STUCK: Confirm failed after {} attempts for vault #{}. ckUSDC is in backend but debt not written down. Error: {}. Needs admin resolution.",
            CONFIRM_ATTEMPTS, vault.vault_id, last_confirm_err);
        write_record(LiquidationRecordV1 {
            id: record_id, vault_id: vault.vault_id, timestamp,
            status: LiquidationStatus::ConfirmFailed,
            collateral_claimed_e8s: collateral_amount, debt_to_cover_e8s: debt_covered,
            icp_swapped_e8s: swap_amount, ckusdc_received_e6: ckusdc_received,
            ckusdc_transferred_e6: ckusdc_transferred, icp_to_treasury_e8s: 0,
            oracle_price_e8s: collateral_price, effective_price_e8s: effective_price,
            slippage_bps, error_message: Some(last_confirm_err), confirm_retry_count: confirm_retries,
            claim_timestamp: Some(claim_timestamp), payment_memo: Some(payment_memo.clone()),
            ckusdc_payment_block_index: Some(payment_receipt.block_index),
            ckusdc_payment_amount_e6: Some(payment_receipt.amount_e6),
            icp_treasury_transfer: None, icp_treasury_bonus_state: None,
        });
        return;
    }

    // -- Phase 5: TREASURY (liquidation bonus) --
    let icp_to_treasury = icp_treasury_bonus_gross_after_swap(
        collateral_amount,
        swap_amount,
        config.icp_fee_e8s.unwrap_or(swap::FALLBACK_LEDGER_FEE),
    );
    let (mut treasury_transfer, preparation_error) = if icp_to_treasury > 0 {
        match swap::prepare_icp_treasury_bonus_transfer(&config, icp_to_treasury, record_id) {
            Ok(transfer) => (Some(transfer), None),
            Err(error) => (None, Some(error)),
        }
    } else {
        (None, None)
    };

    // Commit the exact transfer identity before the first external call. A
    // trap or lost reply after this point leaves a retryable, durable record.
    write_record(LiquidationRecordV1 {
        id: record_id, vault_id: vault.vault_id, timestamp,
        status: if icp_to_treasury == 0 { LiquidationStatus::Completed } else { LiquidationStatus::TransferFailed },
        collateral_claimed_e8s: collateral_amount, debt_to_cover_e8s: debt_covered,
        icp_swapped_e8s: swap_amount, ckusdc_received_e6: ckusdc_received,
        ckusdc_transferred_e6: ckusdc_transferred,
        icp_to_treasury_e8s: icp_to_treasury,
        oracle_price_e8s: collateral_price, effective_price_e8s: effective_price,
        slippage_bps,
        error_message: preparation_error.clone().or_else(|| {
            (icp_to_treasury > 0).then(|| "ICP treasury bonus transfer prepared; outcome pending".to_string())
        }),
        confirm_retry_count: confirm_retries,
        claim_timestamp: Some(claim_timestamp),
        payment_memo: Some(payment_memo),
        ckusdc_payment_block_index: Some(payment_receipt.block_index),
        ckusdc_payment_amount_e6: Some(payment_receipt.amount_e6),
        icp_treasury_transfer: treasury_transfer.clone(),
        icp_treasury_bonus_state: treasury_transfer.as_ref().map(|_| history::IcpTreasuryBonusState::Prepared),
    });

    // Settle the liquidation-side metrics once. Treasury-payout metrics move
    // only after the ledger returns a block index for this exact request.
    state::mutate_state(|s| {
        s.stats.total_debt_covered_e8s += debt_covered;
        s.stats.total_collateral_received_e8s += collateral_amount;
        s.stats.events_count += 1;
    });

    if icp_to_treasury == 0 {
        log!(crate::INFO,
            "Liquidation record #{} for vault #{} completed: debt={} e8s, ckUSDC={} e6, no ICP treasury bonus",
            record_id, vault.vault_id, debt_covered, ckusdc_received);
        history::remove_claim_intent(vault.vault_id);
        return;
    }

    let Some(mut transfer) = treasury_transfer.take() else {
        let error = preparation_error.unwrap_or_else(|| "unable to prepare ICP treasury transfer".to_string());
        history::update_icp_treasury_bonus(
            record_id, None, history::IcpTreasuryBonusState::Quarantined,
            LiquidationStatus::TransferFailed, Some(error.clone()),
        );
        log!(crate::INFO,
            "STUCK: liquidation record #{} for vault #{} settled, but ICP treasury bonus is quarantined (gross obligation={} e8s): {}",
            record_id, vault.vault_id, icp_to_treasury, error);
        return;
    };

    match swap::transfer_icp_treasury_bonus_exact(&transfer).await {
        Ok(block_index) => {
            transfer.block_index = Some(block_index);
            if !history::update_icp_treasury_bonus(
                record_id, Some(transfer), history::IcpTreasuryBonusState::Paid,
                LiquidationStatus::Completed, None,
            ) {
                ic_cdk::trap("ICP treasury transfer accepted but its history record disappeared");
            }
            state::mutate_state(|s| {
                s.stats.total_collateral_to_treasury_e8s += icp_to_treasury;
            });
            log!(crate::INFO,
                "Liquidation record #{} completed with ICP treasury block {} (gross debit={} e8s)",
                record_id, block_index, icp_to_treasury);
            history::remove_claim_intent(vault.vault_id);
        }
        Err(error) => {
            history::update_icp_treasury_bonus(
                record_id, Some(transfer), history::IcpTreasuryBonusState::Prepared,
                LiquidationStatus::TransferFailed, Some(format!(
                    "ICP treasury bonus remains pending (gross obligation {} e8s): {}",
                    icp_to_treasury, error
                )),
            );
            log!(crate::INFO,
                "STUCK: liquidation record #{} for vault #{} settled, but ICP treasury bonus remains pending (gross obligation={} e8s): {}",
                record_id, vault.vault_id, icp_to_treasury, error);
        }
    }
    }
}

/// Persist the exact return tuple before the first ledger call, then record a
/// positive ledger receipt before attempting the backend's proof-gated cancel.
async fn dispatch_claim_collateral_return(
    config: &crate::state::BotConfig,
    vault_id: u64,
    collateral_amount_e8s: u64,
    claim_timestamp: u64,
) -> Result<u64, String> {
    let mut intent = history::get_claim_intent(vault_id)
        .ok_or_else(|| "Durable claim intent disappeared before collateral return".to_string())?;
    let phase_matches = match &intent.phase {
        history::BotClaimIntentPhase::Claimed(receipt) => {
            receipt.claim_timestamp == claim_timestamp
                && receipt.collateral_amount == collateral_amount_e8s
                && receipt.claim_transfer.as_ref().is_some_and(|proof| claim_transfer_receipt_is_complete(
                    config.backend_principal,
                    ic_cdk::id(),
                    config.icp_ledger,
                    vault_id,
                    claim_timestamp,
                    collateral_amount_e8s,
                    proof,
                ))
        }
        _ => false,
    };
    if intent.vault_id != vault_id
        || intent.backend_principal.as_ref() != Some(&config.backend_principal)
        || !phase_matches
    {
        return Err("Durable claim identity does not match collateral return".to_string());
    }
    let pinned_return_fee = match &intent.phase {
        history::BotClaimIntentPhase::Claimed(receipt) => receipt.claim_transfer.as_ref()
            .map(|proof| proof.fee_e8s)
            .ok_or("claim has no exact outbound fee to fund its return-buffer obligation")?,
        _ => return Err("claim return has no exact claimed receipt".into()),
    };
    let mut return_config = config.clone();
    return_config.icp_fee_e8s = Some(pinned_return_fee);

    let transfer = match intent.return_transfer.take() {
        Some(transfer) => {
            if !swap::claim_return_transfer_matches(
                &transfer,
                ic_cdk::id(),
                config,
                vault_id,
                claim_timestamp,
            ) || transfer.collateral_amount_e8s != collateral_amount_e8s
                || !claim_return_fee_matches_claim_proof(&transfer, pinned_return_fee)
            {
                return Err("Persisted collateral return identity does not match this claim".to_string());
            }
            transfer
        }
        None => {
            let transfer = swap::prepare_claim_return_transfer(
                ic_cdk::id(),
                &return_config,
                collateral_amount_e8s,
                vault_id,
                claim_timestamp,
                ic_cdk::api::time(),
            )?;
            intent.return_transfer = Some(transfer.clone());
            if !history::update_claim_return_transfer(vault_id, transfer.clone()) {
                return Err("Could not persist collateral return identity before dispatch".to_string());
            }
            transfer
        }
    };

    if let Some(block_index) = transfer.block_index {
        return Ok(block_index);
    }

    dispatch_prepared_claim_return(config, vault_id, transfer).await
}

fn ensure_current_claim_return_generation(
    vault_id: u64,
    transfer: &history::BotClaimReturnTransfer,
) -> Result<(), String> {
    let intent = history::get_claim_intent(vault_id).ok_or("claim return intent disappeared during await")?;
    let claim_matches = match &intent.phase {
        history::BotClaimIntentPhase::Claimed(receipt) => {
            receipt.claim_timestamp == transfer.claim_timestamp
                && receipt.collateral_amount == transfer.collateral_amount_e8s
        }
        _ => false,
    };
    if intent.vault_id != vault_id
        || !claim_matches
        || intent.return_transfer.as_ref() != Some(transfer)
    {
        return Err("claim generation or exact return tuple changed during ledger await".into());
    }
    Ok(())
}

fn claim_return_fee_matches_claim_proof(
    transfer: &history::BotClaimReturnTransfer,
    claim_fee: u64,
) -> bool {
    let prior = transfer.prior_no_effects.as_deref().unwrap_or_default();
    let mut expected_fee = claim_fee;
    let mut previous_created = None;
    for rejected in prior {
        let Some(created) = rejected.args.created_at_time else { return false; };
        let rejected_fee = rejected.args.fee.as_ref()
            .and_then(|fee| fee.0.to_string().parse::<u64>().ok());
        let rejected_amount = rejected.args.amount.0.to_string().parse::<u64>().ok();
        let next_fee = match &rejected.observation {
            history::BotCkUsdcPaymentDispatchObservation::BadFee { expected_fee } => *expected_fee,
            _ => return false,
        };
        if rejected.ledger != transfer.ledger
            || rejected.args.from_subaccount != transfer.args.from_subaccount
            || rejected.args.to != transfer.args.to
            || rejected.args.memo != transfer.args.memo
            || rejected_fee != Some(expected_fee)
            || rejected_amount != transfer.collateral_amount_e8s.checked_add(expected_fee)
            || next_fee == expected_fee
            || previous_created.is_some_and(|previous| created <= previous)
        {
            return false;
        }
        previous_created = Some(created);
        expected_fee = next_fee;
    }
    let current_created = transfer.args.created_at_time;
    let current_fee = transfer.args.fee.as_ref()
        .and_then(|fee| fee.0.to_string().parse::<u64>().ok());
    current_fee == Some(expected_fee)
        && transfer.args.amount.0.to_string().parse::<u64>().ok()
            == transfer.collateral_amount_e8s.checked_add(expected_fee)
        && current_created.is_some_and(|created| previous_created.map_or(true, |previous| created > previous))
}

fn claim_return_gross_debit(collateral: u64, fee: u64) -> Option<u64> {
    collateral.checked_add(fee.checked_mul(2)?)
}

fn arm_claim_return_dispatch(
    vault_id: u64,
    transfer: &mut history::BotClaimReturnTransfer,
) -> Result<(), String> {
    if let Some(count) = transfer.dispatch_attempt_count {
        let next = count.checked_add(1).ok_or("claim return dispatch attempt counter exhausted")?;
        transfer.dispatch_attempt_count = Some(next);
        if !history::update_claim_return_transfer(vault_id, transfer.clone()) {
            transfer.dispatch_attempt_count = Some(count);
            return Err("could not persist exact collateral return dispatch attempt before ledger call".into());
        }
    }
    Ok(())
}

async fn dispatch_prepared_claim_return(
    config: &BotConfig,
    vault_id: u64,
    mut transfer: history::BotClaimReturnTransfer,
) -> Result<u64, String> {
    if let Some(index) = transfer.block_index { return Ok(index); }
    ensure_current_claim_return_generation(vault_id, &transfer)?;
    arm_claim_return_dispatch(vault_id, &mut transfer)?;
    ensure_current_claim_return_generation(vault_id, &transfer)?;
    match swap::return_collateral_to_backend_typed(config, &transfer).await {
        Ok(block_index) => {
            ensure_current_claim_return_generation(vault_id, &transfer)?;
            transfer.block_index = Some(block_index);
            if !history::update_claim_return_transfer(vault_id, transfer) {
                return Err("Return landed but its receipt could not be persisted; exact retry remains required".into());
            }
            Ok(block_index)
        }
        Err(error) => {
            ensure_current_claim_return_generation(vault_id, &transfer)?;
            if transfer.dispatch_observation.is_none() {
                transfer.dispatch_observation = Some(error.observation());
                if !history::update_claim_return_transfer(vault_id, transfer) {
                    return Err("could not persist collateral return dispatch observation".into());
                }
            }
            Err(format!("Collateral return dispatch failed: {error:?}"))
        }
    }
}

/// Admin recovery for a return transfer whose first outcome was failed or
/// ambiguous. Legacy Acquired intents without a persisted return tuple remain
/// quarantined; this function never allocates a replacement timestamp or memo.
pub(crate) async fn retry_claim_collateral_return(
    config: &crate::state::BotConfig,
    vault_id: u64,
) -> Result<(), String> {
    let mut intent = history::get_claim_intent(vault_id)
        .ok_or_else(|| "No durable claim intent for this vault".to_string())?;
    if intent.vault_id != vault_id
        || intent.backend_principal.as_ref() != Some(&config.backend_principal)
    {
        return Err("Claim intent is not bound to the configured backend".to_string());
    }
    let (claim_timestamp, payment_memo, pinned_return_fee) = match &intent.phase {
        history::BotClaimIntentPhase::Claimed(receipt) => {
            let proof = receipt.claim_transfer.as_ref()
                .ok_or("Claim transfer provenance is missing; collateral return remains held")?;
            if !claim_transfer_receipt_is_complete(
                config.backend_principal,
                ic_cdk::id(),
                config.icp_ledger,
                vault_id,
                receipt.claim_timestamp,
                receipt.collateral_amount,
                proof,
            ) {
                return Err("Claim transfer provenance is missing or invalid; collateral return remains held".into());
            }
            (receipt.claim_timestamp, receipt.payment_memo.clone(), proof.fee_e8s)
        }
        history::BotClaimIntentPhase::Acquired { .. } =>
            return Err("Legacy acquired claim has no exact claim-transfer provenance; return remains held".into()),
        history::BotClaimIntentPhase::SwapStarted(_) =>
            return Err("Swap outcome may be ambiguous; collateral return is held until independent output proof".into()),
        history::BotClaimIntentPhase::AwaitingReceipt
        | history::BotClaimIntentPhase::ClaimRequested
        | history::BotClaimIntentPhase::NoEffectAcknowledgementPending { .. } =>
            return Err("Claim receipt is not acquired; use exact-ID claim receipt recovery or no-effect ACK recovery".into()),
    };
    let mut transfer = intent.return_transfer.take()
        .ok_or_else(|| "Legacy or unprepared claim return is held; no exact transfer identity is available".to_string())?;
    if !swap::claim_return_transfer_matches(
        &transfer,
        ic_cdk::id(),
        config,
        vault_id,
        claim_timestamp,
    ) {
        return Err("Persisted collateral return identity failed validation".to_string());
    }
    // A return fee may differ from the original outbound fee only when this
    // exact return tuple has a durable first-attempt BadFee no-effect receipt.
    // The replacement keeps C+F in the return amount and proves the bot can
    // fund the full sender debit C+2F before it is atomically journaled.
    if transfer.dispatch_attempt_count == Some(1)
        && matches!(transfer.dispatch_observation.as_ref(), Some(
            history::BotCkUsdcPaymentDispatchObservation::BadFee { .. }
        ))
    {
        transfer = reprice_first_bad_fee_claim_return(config, vault_id, transfer).await?;
    } else if !claim_return_fee_matches_claim_proof(&transfer, pinned_return_fee) {
        return Err("Legacy return tuple fee differs from its claim fee and has no typed no-effect receipt; return remains held".into());
    }

    let record = history::get_latest_record_for_vault(vault_id)
        .ok_or_else(|| "No liquidation history for this claim".to_string())?;
    let record_id = match &record {
        LiquidationRecordVersioned::V1(r)
            if r.id == intent.request_id
                && r.vault_id == vault_id
                && r.collateral_claimed_e8s == transfer.collateral_amount_e8s
                && r.ckusdc_transferred_e6 == 0
                && matches!(r.status, LiquidationStatus::TransferFailed | LiquidationStatus::ConfirmFailed)
                && r.claim_timestamp.map_or(true, |ts| ts == claim_timestamp)
                && r.payment_memo.as_ref().map_or(true, |memo| memo == &payment_memo) => r.id,
        _ => return Err("Latest history is not an unpaid failed record for this exact claim".into()),
    };

    let expected_receipt = match &intent.phase {
        history::BotClaimIntentPhase::Claimed(receipt) => receipt,
        _ => return Err("Claim return recovery lost its exact claimed receipt".into()),
    };
    let active_claims = get_active_bot_claims(config).await?;
    if !active_claims.contains(&vault_id) {
        if !claim_return_completion_is_verified(false, transfer.block_index) {
            return Err("Backend claim is no longer active and no positive return receipt is recorded; keeping intent held".into());
        }
        let return_block_index = transfer.block_index.ok_or("verified backend cancellation has no durable bot return block")?;
        call_bot_cancel_liquidation(
            config,
            vault_id,
            expected_receipt.claim_timestamp,
            return_block_index,
        ).await?;
        call_bot_acknowledge_claim_cancellation(config, vault_id, expected_receipt.claim_timestamp).await?;
        finish_recovered_claim_return(vault_id, record_id)?;
        return Ok(());
    }

    let claim = call_bot_claim_liquidation(config, vault_id, intent.request_id)
        .await
        .map_err(|error| error.to_string())?;
    if !bot_claim_receipt_matches(vault_id, expected_receipt, &claim)
        || claim.collateral_amount != transfer.collateral_amount_e8s
    {
        return Err("Backend claim receipt differs from the persisted return generation".into());
    }

    if transfer.block_index.is_none() {
        let block_index = dispatch_prepared_claim_return(config, vault_id, transfer.clone()).await?;
        transfer.block_index = Some(block_index);
    }

    let return_block_index = transfer.block_index.ok_or("claim return tuple has no confirmed ledger block")?;
    match call_bot_cancel_liquidation(config, vault_id, expected_receipt.claim_timestamp, return_block_index).await {
        Ok(()) => {
            call_bot_acknowledge_claim_cancellation(config, vault_id, expected_receipt.claim_timestamp).await?;
            finish_recovered_claim_return(vault_id, record_id)
        }
        Err(cancel_error) => {
            // The backend can commit cancellation and lose only its reply.
            // With a positive return receipt already durable, an absent active
            // claim is sufficient to reconcile that exact cancel attempt.
            let active_after = get_active_bot_claims(config).await?;
            if active_after.contains(&vault_id) {
                return Err(format!(
                    "Returned collateral receipt is block {:?}, but backend cancel remains pending: {}",
                    transfer.block_index, cancel_error
                ));
            }
            call_bot_acknowledge_claim_cancellation(
                config,
                vault_id,
                expected_receipt.claim_timestamp,
            ).await?;
            finish_recovered_claim_return(vault_id, record_id)
        }
    }
}

async fn reprice_first_bad_fee_claim_return(
    config: &BotConfig,
    vault_id: u64,
    rejected: history::BotClaimReturnTransfer,
) -> Result<history::BotClaimReturnTransfer, String> {
    if rejected.dispatch_attempt_count != Some(1)
        || rejected.block_index.is_some()
        || rejected.prior_no_effects.as_ref().map_or(0, Vec::len) >= 16
    {
        return Err("claim return has no bounded first-attempt BadFee replacement authority".into());
    }
    let expected_fee = match rejected.dispatch_observation.as_ref() {
        Some(history::BotCkUsdcPaymentDispatchObservation::BadFee { expected_fee }) => *expected_fee,
        _ => return Err("claim return is not backed by an exact typed BadFee receipt".into()),
    };
    ensure_current_claim_return_generation(vault_id, &rejected)?;
    let live_fee = swap::fetch_ledger_fee(rejected.ledger).await?;
    ensure_current_claim_return_generation(vault_id, &rejected)?;
    if live_fee != expected_fee || live_fee == rejected.ledger_fee_e8s {
        return Err(format!(
            "live collateral ledger fee {live_fee} does not match the first-attempt BadFee replacement fee {expected_fee}; claim remains held"
        ));
    }
    let balance = swap::balance_of_ckusdc_account(rejected.ledger, rejected.from.clone()).await?;
    ensure_current_claim_return_generation(vault_id, &rejected)?;
    let required = claim_return_gross_debit(rejected.collateral_amount_e8s, live_fee)
        .ok_or("claim return gross funding requirement overflows")?;
    if balance < required {
        return Err(format!(
            "bot collateral account balance {balance} is below fee-buffered return gross debit {required}; claim remains held"
        ));
    }
    let old_created = rejected.args.created_at_time.ok_or("rejected return tuple has no created_at_time")?;
    let fresh_created = old_created.checked_add(1)
        .ok_or("rejected return created_at_time is exhausted")?
        .max(ic_cdk::api::time());
    let mut replacement_config = config.clone();
    replacement_config.icp_fee_e8s = Some(live_fee);
    let mut replacement = swap::prepare_claim_return_transfer(
        ic_cdk::id(),
        &replacement_config,
        rejected.collateral_amount_e8s,
        vault_id,
        rejected.claim_timestamp,
        fresh_created,
    )?;
    replacement.prior_no_effects = rejected.prior_no_effects.clone();
    ensure_current_claim_return_generation(vault_id, &rejected)?;
    if !history::replace_claim_return_after_first_bad_fee(vault_id, &rejected, replacement.clone()) {
        return Err("could not atomically replace the first-attempt BadFee collateral return tuple".into());
    }
    ensure_current_claim_return_generation(vault_id, &replacement)?;
    Ok(replacement)
}

async fn get_active_bot_claims(config: &crate::state::BotConfig) -> Result<Vec<u64>, String> {
    let result: Result<(Vec<u64>,), _> =
        ic_cdk::call(config.backend_principal, "get_bot_claim_vault_ids", ()).await;
    result.map(|(ids,)| ids)
        .map_err(|(code, message)| format!("Unable to verify active backend claims: {:?}: {}", code, message))
}

fn finish_recovered_claim_return(vault_id: u64, record_id: u64) -> Result<(), String> {
    if !history::update_record_status_and_error(
        record_id,
        LiquidationStatus::SwapFailed,
        Some("Claim-specific collateral return and backend cancellation confirmed by admin recovery".into()),
    ) {
        return Err("Backend cancellation completed but the liquidation record disappeared".into());
    }
    history::remove_claim_intent(vault_id);
    log!(crate::INFO, "Recovered exact claim collateral return for vault #{} in record #{}", vault_id, record_id);
    Ok(())
}

fn claim_return_completion_is_verified(claim_still_active: bool, return_block: Option<u64>) -> bool {
    !claim_still_active && return_block.is_some()
}

// -- Helpers --

fn write_record(record: LiquidationRecordV1) {
    history::insert_record(LiquidationRecordVersioned::V1(record));
}

async fn call_bot_claim_liquidation(
    config: &BotConfig,
    vault_id: u64,
    request_id: u64,
) -> Result<BotLiquidationResult, BotClaimCallError> {
    let result: Result<(BackendResult<BotLiquidationResult>,), _> =
        ic_cdk::call(config.backend_principal, "bot_claim_liquidation_with_request_id", (vault_id, request_id)).await;

    match result {
        Ok((BackendResult::Ok(r),)) => Ok(r),
        Ok((BackendResult::Err(e),)) => Err(BotClaimCallError::BackendRejected(e)),
        Err((code, msg)) => Err(BotClaimCallError::TransportUnknown(format!("{:?}: {}", code, msg))),
    }
}

/// Query typed backend state after every failed or reply-lost claim call.
/// An absent result is not proof that this request had no effect.
async fn call_bot_claim_no_effect_status(
    config: &BotConfig,
    vault_id: u64,
    request_id: u64,
) -> Result<Option<BotClaimNoEffectProof>, String> {
    let result: Result<(BackendResult<Option<BotClaimNoEffectProof>>,), _> = ic_cdk::call(
        config.backend_principal,
        "bot_claim_request_no_effect",
        (vault_id, request_id),
    )
    .await;
    match result {
        Ok((BackendResult::Ok(proof),)) => Ok(proof),
        Ok((BackendResult::Err(error),)) => Err(format!("typed backend status error: {}", error)),
        Err((code, message)) => Err(format!("backend status outcome unknown: {:?}: {}", code, message)),
    }
}

/// ACK only a locally persisted exact proof. Repeating this call is safe if
/// the backend committed the ACK but its reply was lost.
async fn call_bot_acknowledge_no_effect(
    config: &BotConfig,
    vault_id: u64,
    request_id: u64,
    transfer_digest: Vec<u8>,
) -> Result<(), String> {
    let result: Result<(BackendResult<()>,), _> = ic_cdk::call(
        config.backend_principal,
        "acknowledge_bot_claim_no_effect",
        (vault_id, request_id, transfer_digest),
    )
    .await;
    match result {
        Ok((BackendResult::Ok(()),)) => Ok(()),
        Ok((BackendResult::Err(error),)) => Err(format!("typed backend ACK error: {}", error)),
        Err((code, message)) => Err(format!("backend ACK outcome unknown: {:?}: {}", code, message)),
    }
}

pub async fn call_bot_confirm_liquidation(
    config: &BotConfig,
    vault_id: u64,
    claim_timestamp: u64,
    payment_block_index: u64,
) -> Result<(), String> {
    let result: Result<(BackendResult<()>,), _> =
        ic_cdk::call(
            config.backend_principal,
            "bot_confirm_liquidation_with_payment",
            (vault_id, claim_timestamp, payment_block_index),
        ).await;

    match result {
        Ok((BackendResult::Ok(()),)) => Ok(()),
        Ok((BackendResult::Err(e),)) => Err(format!("{}", e)),
        Err((code, msg)) => Err(format!("{:?}: {}", code, msg)),
    }
}

/// Repair only a short, memo-bound payment whose original block has already
/// been authenticated against the configured ledger. Returns false when the
/// verified payment already meets the claim minimum so the caller can retain
/// the normal single-block confirmation path.
pub async fn recover_short_ckusdc_payment(
    config: &BotConfig,
    record: &LiquidationRecordV1,
    original: &swap::VerifiedCkUsdcPayment,
) -> Result<bool, String> {
    let vault_id = record.vault_id;
    let claim_timestamp = record.claim_timestamp.ok_or("claim timestamp is missing")?;
    let memo = record.payment_memo.as_deref().filter(|memo| !memo.is_empty())
        .ok_or("legacy claim has no payment memo; top-up recovery is disabled")?;
    if record.status != LiquidationStatus::ConfirmFailed
        || record.ckusdc_payment_block_index.is_none()
        || record.ckusdc_payment_amount_e6 != Some(original.amount_e6)
        || record.ckusdc_transferred_e6 != original.amount_e6
        || original.created_at_time < claim_timestamp
    {
        return Err("original ckUSDC payment does not match the active memo-bound claim record".into());
    }
    let minimum = ckusdc_minimum_payment_e6(record.debt_to_cover_e8s);
    let Some(shortfall) = minimum.checked_sub(original.amount_e6) else {
        return Ok(false);
    };
    if shortfall == 0 { return Ok(false); }

    let intent = history::get_claim_intent(vault_id)
        .ok_or("durable claim intent is missing; top-up recovery is disabled")?;
    if intent.vault_id != vault_id
        || intent.backend_principal != Some(config.backend_principal)
        || intent.return_transfer.is_some()
        || !matches!(&intent.phase,
            history::BotClaimIntentPhase::Claimed(receipt) | history::BotClaimIntentPhase::SwapStarted(receipt)
                if receipt.claim_timestamp == claim_timestamp
                    && receipt.payment_memo.as_slice() == memo
                    && receipt.debt_covered == record.debt_to_cover_e8s)
    {
        return Err("durable claim intent does not bind this exact active claim generation".into());
    }

    let mut transfer = match intent.ckusdc_top_up_transfer.clone() {
        Some(transfer) => {
            if !ckusdc_top_up_tuple_matches(
                &transfer, config.ckusdc_ledger, config.backend_principal,
                vault_id, claim_timestamp, memo, shortfall,
            ) {
                return Err("persisted ckUSDC top-up tuple does not match this claim's exact shortfall".into());
            }
            Some(transfer)
        }
        None => None,
    };

    let original_index = record.ckusdc_payment_block_index.ok_or("original payment block index is missing")?;
    // A durable top-up block may already have completed the claim even when
    // the backend reply was lost. Verify its exact saved ledger tuple, then
    // let the backend's exact-set settled replay resolve that ambiguity; the
    // incomplete-lock evidence endpoint is no longer applicable after settle.
    if let Some(index) = transfer.as_ref().and_then(|saved| saved.block_index) {
        let saved = transfer.as_ref().expect("block index came from saved transfer");
        verify_ckusdc_top_up_tuple(saved, index, config.backend_principal).await?;
        call_bot_confirm_liquidation_with_payments(
            config, vault_id, claim_timestamp, vec![original_index, index],
        ).await?;
        return Ok(true);
    }

    // The singleton response can be lost after the backend commits. Its text
    // never authorizes a top-up; only the exact persisted block proof below
    // can establish the incomplete lock.
    let _confirm_result = call_bot_confirm_liquidation(
        config, vault_id, claim_timestamp, original_index,
    ).await;
    require_exact_partial_payment_lock(
        config, vault_id, claim_timestamp, memo, original_index, original.amount_e6,
    ).await?;

    if transfer.is_none() {
        // A new tuple is permitted only after the backend has locked this
        // exact short payment to the active generation. Read both the current
        // fee and this claim's isolated account balance; pooled balance never
        // funds recovery.
        let fee_e6 = swap::fetch_ledger_fee(config.ckusdc_ledger).await?;
        ensure_ckusdc_top_up_preparation_generation(
            vault_id, claim_timestamp, memo, &intent,
        )?;
        let gross_required = ckusdc_top_up_gross_e6(shortfall, fee_e6)
            .ok_or("ckUSDC top-up plus live fee overflows")?;
        let source_subaccount = claim_payment_subaccount(vault_id, claim_timestamp);
        let balance_e6 = swap::balance_of_ckusdc_account(
            config.ckusdc_ledger,
            icrc_ledger_types::icrc1::account::Account {
                owner: ic_cdk::id(),
                subaccount: Some(source_subaccount),
            },
        ).await?;
        ensure_ckusdc_top_up_preparation_generation(
            vault_id, claim_timestamp, memo, &intent,
        )?;
        if balance_e6 < gross_required {
            return Err(format!(
                "claim-specific ckUSDC account has {balance_e6} e6; exact shortfall {shortfall} plus live fee {fee_e6} requires {gross_required} e6"
            ));
        }
        // Reconfirm the backend lock after the ledger awaits and immediately
        // before preparing the immutable tuple.
        require_exact_partial_payment_lock(
            config, vault_id, claim_timestamp, memo, original_index, original.amount_e6,
        ).await.map_err(|_| "exact backend partial-payment block lock disappeared during top-up preflight")?;
        ensure_ckusdc_top_up_preparation_generation(
            vault_id, claim_timestamp, memo, &intent,
        )?;
        let prepared = prepare_ckusdc_top_up_transfer(
            config,
            vault_id,
            claim_timestamp,
            memo,
            shortfall,
            fee_e6,
            ic_cdk::api::time().max(claim_timestamp),
        )?;
        if !ckusdc_top_up_tuple_matches(
            &prepared, config.ckusdc_ledger, config.backend_principal,
            vault_id, claim_timestamp, memo, shortfall,
        ) {
            return Err("prepared ckUSDC top-up tuple is not bound to the exact claim generation".into());
        }
        if !history::update_ckusdc_top_up_transfer(vault_id, prepared.clone()) {
            return Err("could not persist exact claim-specific ckUSDC top-up tuple; no transfer attempted".into());
        }
        transfer = Some(prepared);
    }
    let mut transfer = transfer.expect("top-up tuple exists after preparation or recovery");

    // A typed first-attempt no-effect result is the sole case where a new
    // top-up tuple may be prepared. Re-read live fee and this claim's isolated
    // account after awaits, then revalidate the backend lock and CAS against
    // the exact rejected tuple before persisting the replacement.
    if matches!(transfer.dispatch_attempt_count, Some(1))
        && transfer.history_scan.is_none()
        && transfer.block_index.is_none()
        && matches!(transfer.dispatch_observation,
            Some(history::BotCkUsdcPaymentDispatchObservation::BadFee { .. }
                | history::BotCkUsdcPaymentDispatchObservation::InsufficientFunds { .. }))
    {
        if transfer.prior_no_effects.as_ref().map_or(0, Vec::len) >= 16 {
            return Err("ckUSDC top-up replacement tombstone limit reached; manual review required".into());
        }
        let rejected = transfer.clone();
        let fee_e6 = swap::fetch_ledger_fee(rejected.ledger).await?;
        ensure_current_ckusdc_top_up_generation(vault_id, claim_timestamp, memo, &rejected)?;
        let amount_e6 = rejected.args.amount.0.to_string().parse::<u64>()
            .map_err(|_| "persisted ckUSDC top-up amount exceeds u64")?;
        let required_balance = amount_e6.checked_add(fee_e6)
            .ok_or("ckUSDC replacement amount plus live fee overflows")?;
        let source = icrc_ledger_types::icrc1::account::Account {
            owner: ic_cdk::id(),
            subaccount: rejected.args.from_subaccount,
        };
        if source.subaccount != Some(claim_payment_subaccount(vault_id, claim_timestamp)) {
            return Err("persisted ckUSDC top-up is not bound to this claim's isolated source account".into());
        }
        let balance_e6 = swap::balance_of_ckusdc_account(rejected.ledger, source).await?;
        ensure_current_ckusdc_top_up_generation(vault_id, claim_timestamp, memo, &rejected)?;
        if balance_e6 < required_balance {
            return Err(format!("claim-specific ckUSDC account has {balance_e6} e6; the unchanged replacement amount plus live fee requires {required_balance} e6"));
        }
        require_exact_partial_payment_lock(
            config, vault_id, claim_timestamp, memo, original_index, original.amount_e6,
        ).await.map_err(|_| "exact backend partial-payment block lock disappeared during top-up replacement preflight")?;
        ensure_current_ckusdc_top_up_generation(vault_id, claim_timestamp, memo, &rejected)?;
        let old_created = rejected.args.created_at_time.ok_or("rejected top-up has no created_at_time")?;
        let fresh_created = old_created.checked_add(1)
            .ok_or("rejected top-up created_at_time is exhausted")?
            .max(ic_cdk::api::time());
        let mut replacement = prepare_ckusdc_top_up_transfer(
            config, vault_id, claim_timestamp, memo, shortfall, fee_e6, fresh_created,
        )?;
        require_exact_partial_payment_lock(
            config, vault_id, claim_timestamp, memo, original_index, original.amount_e6,
        ).await.map_err(|_| "exact backend partial-payment block lock disappeared before top-up replacement")?;
        ensure_current_ckusdc_top_up_generation(vault_id, claim_timestamp, memo, &rejected)?;
        if replacement.args.amount != rejected.args.amount {
            return Err("refreshed ckUSDC top-up changed the shortfall amount; replacement remains held".into());
        }
        replacement.dispatch_attempt_count = Some(0);
        replacement.prior_no_effects = rejected.prior_no_effects.clone();
        if !history::replace_ckusdc_top_up_after_first_no_effect(vault_id, &rejected, replacement.clone()) {
            return Err("could not atomically replace the exact first-attempt ckUSDC top-up rejection".into());
        }
        transfer = history::get_claim_intent(vault_id)
            .and_then(|intent| intent.ckusdc_top_up_transfer)
            .ok_or("replacement succeeded but the durable ckUSDC top-up tuple disappeared")?;
        if transfer.ledger != replacement.ledger
            || transfer.args != replacement.args
            || transfer.dispatch_attempt_count != Some(0)
            || transfer.dispatch_observation.is_some()
            || transfer.history_scan.is_some()
        {
            return Err("replacement CAS stored a different ckUSDC top-up tuple than the prepared one".into());
        }
    }

    // Dispatch exactly once into durable positive-history reconciliation.
    // Once observation/scan state exists, this path never resends the tuple.
    let block_index = if let Some(index) = transfer.block_index {
        index
    } else if transfer.history_scan.is_some() {
        advance_ckusdc_top_up_history_scan(vault_id, claim_timestamp, memo, &mut transfer).await?
    } else if transfer.dispatch_observation.is_some() {
        initialize_ckusdc_top_up_history_scan(vault_id, claim_timestamp, memo, &mut transfer).await?
    } else {
        arm_ckusdc_top_up_dispatch(vault_id, &mut transfer)?;
        ensure_current_ckusdc_top_up_generation(vault_id, claim_timestamp, memo, &transfer)?;
        let dispatch_result = swap::transfer_ckusdc_top_up_exact_typed(
            transfer.ledger, transfer.args.clone(),
        ).await;
        ensure_current_ckusdc_top_up_generation(vault_id, claim_timestamp, memo, &transfer)?;
        match dispatch_result {
            Ok(index) => {
                transfer.block_index = Some(index);
                if !history::update_ckusdc_top_up_transfer(vault_id, transfer.clone()) {
                    return Err("ckUSDC top-up succeeded but its block index could not be persisted".into());
                }
                index
            }
            Err(dispatch_error) => {
                transfer.dispatch_observation = Some(dispatch_error.observation());
                if !history::update_ckusdc_top_up_transfer(vault_id, transfer.clone()) {
                    return Err("could not persist ckUSDC top-up dispatch observation".into());
                }
                if transfer.dispatch_attempt_count == Some(1)
                    && matches!(transfer.dispatch_observation,
                        Some(history::BotCkUsdcPaymentDispatchObservation::BadFee { .. }
                            | history::BotCkUsdcPaymentDispatchObservation::InsufficientFunds { .. }))
                {
                    return Err(format!("first ckUSDC top-up tuple was definitively rejected without effect ({:?}); refresh isolated-account balance and fee before preparing its replacement", transfer.dispatch_observation));
                }
                initialize_ckusdc_top_up_history_scan(vault_id, claim_timestamp, memo, &mut transfer)
                    .await.map_err(|scan_error| format!("ckUSDC top-up dispatch {dispatch_error:?}; exact history reconciliation held: {scan_error}"))?
            }
        }
    };
    transfer.block_index = Some(block_index);
    verify_ckusdc_top_up_tuple(&transfer, block_index, config.backend_principal).await?;

    let top_up_index = transfer.block_index.ok_or("top-up payment block index is missing")?;
    call_bot_confirm_liquidation_with_payments(
        config, vault_id, claim_timestamp, vec![original_index, top_up_index],
    ).await?;
    Ok(true)
}

fn ckusdc_top_up_tuple_matches(
    transfer: &history::BotCkUsdcTopUpTransfer,
    ledger: candid::Principal,
    backend: candid::Principal,
    vault_id: u64,
    claim_timestamp: u64,
    memo: &[u8],
    shortfall: u64,
) -> bool {
    transfer.ledger == ledger
        && transfer.args.to == (icrc_ledger_types::icrc1::account::Account {
            owner: backend,
            subaccount: None,
        })
        && transfer.args.from_subaccount == Some(claim_payment_subaccount(vault_id, claim_timestamp))
        && transfer.args.amount.0.to_string().parse::<u64>().ok() == Some(shortfall)
        && transfer.args.fee.is_some()
        && transfer.args.memo.as_ref().map(|m| m.0.as_ref()) == Some(memo)
        && transfer.args.created_at_time.is_some_and(|time| time >= claim_timestamp)
}

fn ensure_ckusdc_top_up_preparation_generation(
    vault_id: u64,
    claim_timestamp: u64,
    payment_memo: &[u8],
    expected_intent: &history::BotClaimIntent,
) -> Result<(), String> {
    let current = history::get_claim_intent(vault_id)
        .ok_or("top-up claim intent disappeared during preflight")?;
    let matches_generation = current == *expected_intent
        && current.vault_id == vault_id
        && current.ckusdc_top_up_transfer.is_none()
        && current.backend_principal == expected_intent.backend_principal
        && matches!(&current.phase,
            history::BotClaimIntentPhase::Claimed(receipt)
                | history::BotClaimIntentPhase::SwapStarted(receipt)
                if receipt.claim_timestamp == claim_timestamp
                    && receipt.payment_memo.as_slice() == payment_memo);
    if !matches_generation {
        return Err("ckUSDC top-up claim generation changed during preparation".into());
    }
    Ok(())
}

fn prepare_ckusdc_top_up_transfer(
    config: &BotConfig,
    vault_id: u64,
    claim_timestamp: u64,
    payment_memo: &[u8],
    shortfall_e6: u64,
    fee_e6: u64,
    created_at_time: u64,
) -> Result<history::BotCkUsdcTopUpTransfer, String> {
    let gross = ckusdc_top_up_gross_e6(shortfall_e6, fee_e6)
        .ok_or("ckUSDC top-up plus fee overflows")?;
    let prepared = swap::prepare_ckusdc_payment_transfer_from_subaccount(
        config,
        gross,
        payment_memo,
        fee_e6,
        created_at_time.max(claim_timestamp),
        Some(claim_payment_subaccount(vault_id, claim_timestamp)),
    )?;
    Ok(history::BotCkUsdcTopUpTransfer {
        ledger: prepared.ledger,
        args: prepared.args,
        block_index: None,
        dispatch_attempt_count: Some(0),
        prior_no_effects: Some(Vec::new()),
        dispatch_observation: None,
        history_scan: None,
    })
}

const CKUSDC_TOP_UP_HISTORY_SCAN_BLOCKS_PER_CALL: u64 = history::CKUSDC_PAYMENT_HISTORY_PAGE_SIZE;

async fn verify_ckusdc_top_up_tuple(
    transfer: &history::BotCkUsdcTopUpTransfer,
    block_index: u64,
    backend: candid::Principal,
) -> Result<(), String> {
    let source = icrc_ledger_types::icrc1::account::Account {
        owner: ic_cdk::id(),
        subaccount: transfer.args.from_subaccount,
    };
    if source.subaccount.is_none() {
        return Err("ckUSDC top-up has no isolated claim source account".into());
    }
    let block = swap::fetch_ckusdc_history_block(transfer.ledger, block_index).await?;
    if swap::classify_ckusdc_payment_block(
        &block,
        source,
        &transfer.args,
    )? != swap::CkUsdcHistoryMatch::ExactMatch
    {
        return Err("ckUSDC top-up block does not prove the exact persisted account and transfer tuple".into());
    }
    let paid = swap::verify_ckusdc_transfer_block(
        transfer.ledger,
        block_index,
        source,
        icrc_ledger_types::icrc1::account::Account { owner: backend, subaccount: None },
        transfer.args.memo.as_ref().map(|memo| memo.0.as_ref()).unwrap_or_default(),
    ).await?;
    let expected_amount = transfer.args.amount.0.to_string().parse::<u64>()
        .map_err(|_| "persisted ckUSDC top-up amount exceeds u64")?;
    let expected_fee = transfer.args.fee.as_ref().and_then(|fee| fee.0.to_string().parse::<u64>().ok())
        .ok_or("persisted ckUSDC top-up fee is invalid")?;
    if paid.amount_e6 != expected_amount
        || paid.fee_e6 != expected_fee
        || Some(paid.created_at_time) != transfer.args.created_at_time
    {
        return Err("ckUSDC top-up block does not match the exact persisted amount, fee, and time".into());
    }
    Ok(())
}

async fn initialize_ckusdc_top_up_history_scan(
    vault_id: u64,
    claim_timestamp: u64,
    payment_memo: &[u8],
    transfer: &mut history::BotCkUsdcTopUpTransfer,
) -> Result<u64, String> {
    ensure_current_ckusdc_top_up_generation(vault_id, claim_timestamp, payment_memo, transfer)?;
    let log_length = swap::ckusdc_history_log_length(transfer.ledger).await?;
    ensure_current_ckusdc_top_up_generation(vault_id, claim_timestamp, payment_memo, transfer)?;
    transfer.history_scan = Some(history::BotCkUsdcPaymentHistoryScan {
        snapshot_log_length: log_length,
        next_index: log_length,
        candidate_block_index: None,
        multiple_candidates: false,
    });
    if !history::update_ckusdc_top_up_transfer(vault_id, transfer.clone()) {
        return Err("could not persist ckUSDC top-up history snapshot before lookup".into());
    }
    advance_ckusdc_top_up_history_scan(vault_id, claim_timestamp, payment_memo, transfer).await
}

/// Search one bounded page of a persisted fixed snapshot for the exact
/// top-up tuple. Any missing or malformed history stays held and cannot
/// authorize a second transfer.
async fn advance_ckusdc_top_up_history_scan(
    vault_id: u64,
    claim_timestamp: u64,
    payment_memo: &[u8],
    transfer: &mut history::BotCkUsdcTopUpTransfer,
) -> Result<u64, String> {
    use swap::CkUsdcHistoryMatch;
    let mut scan = transfer.history_scan.clone().ok_or("top-up history scan is not initialized")?;
    if scan.next_index > scan.snapshot_log_length || scan.multiple_candidates {
        return Err("persisted top-up history cursor is invalid or conflicting candidates remain held".into());
    }
    if let Some(index) = scan.candidate_block_index {
        transfer.block_index = Some(index);
        transfer.history_scan = None;
        if !history::update_ckusdc_top_up_transfer(vault_id, transfer.clone()) {
            return Err("could not persist positively matched ckUSDC top-up block index".into());
        }
        return Ok(index);
    }
    if scan.next_index == 0 {
        let latest = swap::ckusdc_history_log_length(transfer.ledger).await?;
        ensure_current_ckusdc_top_up_generation(vault_id, claim_timestamp, payment_memo, transfer)?;
        if latest <= scan.snapshot_log_length {
            return Err("fixed ckUSDC top-up history snapshot contains no exact transfer; reissue remains disabled".into());
        }
        scan.snapshot_log_length = latest;
        scan.next_index = latest;
        transfer.history_scan = Some(scan.clone());
        if !history::update_ckusdc_top_up_transfer(vault_id, transfer.clone()) {
            return Err("could not persist refreshed ckUSDC top-up history snapshot".into());
        }
    }
    let start = scan.next_index.saturating_sub(CKUSDC_TOP_UP_HISTORY_SCAN_BLOCKS_PER_CALL);
    for index in (start..scan.next_index).rev() {
        let block = swap::fetch_ckusdc_history_block(transfer.ledger, index).await?;
        ensure_current_ckusdc_top_up_generation(vault_id, claim_timestamp, payment_memo, transfer)?;
        if swap::classify_ckusdc_payment_block(
            &block,
            icrc_ledger_types::icrc1::account::Account {
                owner: ic_cdk::id(),
                subaccount: Some(claim_payment_subaccount(vault_id, claim_timestamp)),
            },
            &transfer.args,
        )? == CkUsdcHistoryMatch::ExactMatch {
            scan.next_index = index;
            scan.candidate_block_index = Some(index);
            transfer.history_scan = Some(scan.clone());
            if !history::update_ckusdc_top_up_transfer(vault_id, transfer.clone()) {
                return Err("could not persist positively matched ckUSDC top-up history candidate".into());
            }
            transfer.block_index = Some(index);
            transfer.history_scan = None;
            if !history::update_ckusdc_top_up_transfer(vault_id, transfer.clone()) {
                return Err("could not promote positively matched ckUSDC top-up history candidate".into());
            }
            return Ok(index);
        }
    }
    scan.next_index = start;
    transfer.history_scan = Some(scan.clone());
    if !history::update_ckusdc_top_up_transfer(vault_id, transfer.clone()) {
        return Err("could not persist ckUSDC top-up history scan cursor".into());
    }
    if scan.next_index > 0 {
        return Err(format!("ckUSDC top-up history scan advanced; {} earlier blocks remain", scan.next_index));
    }
    Err("ckUSDC top-up history scan contains no exact transfer; reissue remains disabled".into())
}

fn ensure_current_ckusdc_top_up_generation(
    vault_id: u64,
    claim_timestamp: u64,
    payment_memo: &[u8],
    transfer: &history::BotCkUsdcTopUpTransfer,
) -> Result<(), String> {
    let intent = history::get_claim_intent(vault_id).ok_or("top-up claim intent disappeared during await")?;
    let receipt_matches = match intent.phase {
        history::BotClaimIntentPhase::Claimed(receipt) | history::BotClaimIntentPhase::SwapStarted(receipt) => {
            receipt.claim_timestamp == claim_timestamp && receipt.payment_memo == payment_memo
        }
        _ => false,
    };
    let journal_matches = intent.ckusdc_top_up_transfer.as_ref().is_some_and(|saved| saved == transfer);
    if intent.vault_id != vault_id || !receipt_matches || !journal_matches {
        return Err("ckUSDC top-up claim generation changed during history reconciliation".into());
    }
    Ok(())
}

fn arm_ckusdc_top_up_dispatch(
    vault_id: u64,
    transfer: &mut history::BotCkUsdcTopUpTransfer,
) -> Result<(), String> {
    if transfer.dispatch_attempt_count == Some(u32::MAX) {
        return Err("ckUSDC top-up dispatch attempt counter is exhausted".into());
    }
    if let Some(attempt_count) = transfer.dispatch_attempt_count {
        transfer.dispatch_attempt_count = Some(attempt_count + 1);
        if !history::update_ckusdc_top_up_transfer(vault_id, transfer.clone()) {
            transfer.dispatch_attempt_count = Some(attempt_count);
            return Err("could not persist exact ckUSDC top-up dispatch attempt before ledger call".into());
        }
    }
    Ok(())
}

pub fn ckusdc_minimum_payment_e6(debt_e8s: u64) -> u64 {
    debt_e8s / 100 + u64::from(debt_e8s % 100 != 0)
}

fn ckusdc_top_up_gross_e6(shortfall_e6: u64, fee_e6: u64) -> Option<u64> {
    shortfall_e6.checked_add(fee_e6)
}

pub async fn call_bot_confirm_liquidation_with_payments(
    config: &BotConfig,
    vault_id: u64,
    claim_timestamp: u64,
    payment_block_indices: Vec<u64>,
) -> Result<(), String> {
    let result: Result<(BackendResult<()>,), _> = ic_cdk::call(
        config.backend_principal,
        "bot_confirm_liquidation_with_payments",
        (vault_id, claim_timestamp, payment_block_indices),
    ).await;
    match result {
        Ok((BackendResult::Ok(()),)) => Ok(()),
        Ok((BackendResult::Err(error),)) => Err(format!("{error:?}")),
        Err((code, message)) => Err(format!("backend multi-payment confirmation outcome unknown: {code:?}: {message}")),
    }
}

async fn call_bot_claim_partial_payment_block_evidence(
    config: &BotConfig,
    vault_id: u64,
    claim_timestamp: u64,
    payment_block_index: u64,
) -> Result<Option<BotPartialPaymentLockEvidence>, String> {
    let result: Result<(BackendResult<Option<BotPartialPaymentLockEvidence>>,), _> = ic_cdk::call(
        config.backend_principal,
        "bot_claim_partial_payment_block_evidence",
        (vault_id, claim_timestamp, payment_block_index),
    ).await;
    match result {
        Ok((BackendResult::Ok(evidence),)) => Ok(evidence),
        Ok((BackendResult::Err(error),)) => Err(format!("backend partial-payment block evidence error: {error:?}")),
        Err((code, message)) => Err(format!("backend partial-payment block evidence outcome unknown: {code:?}: {message}")),
    }
}

async fn require_exact_partial_payment_lock(
    config: &BotConfig,
    vault_id: u64,
    claim_timestamp: u64,
    payment_memo: &[u8],
    payment_block_index: u64,
    total_amount_e6: u64,
) -> Result<(), String> {
    let evidence = call_bot_claim_partial_payment_block_evidence(
        config, vault_id, claim_timestamp, payment_block_index,
    ).await?.ok_or("backend has no exact incomplete partial-payment block lock")?;
    if !partial_payment_lock_evidence_matches(
        &evidence,
        config.ckusdc_ledger,
        vault_id,
        claim_timestamp,
        payment_memo,
        payment_block_index,
        total_amount_e6,
    ) {
        return Err("backend partial-payment evidence does not match the exact incomplete claim lock".into());
    }
    Ok(())
}

async fn call_bot_cancel_liquidation(
    config: &BotConfig,
    vault_id: u64,
    claim_timestamp: u64,
    return_block_index: u64,
) -> Result<(), String> {
    let transfer = history::get_claim_intent(vault_id)
        .and_then(|intent| intent.return_transfer)
        .ok_or("claim return has no durable exact transfer tuple")?;
    let intent = history::get_claim_intent(vault_id)
        .ok_or("claim return intent is missing")?;
    let pinned_fee = match &intent.phase {
        history::BotClaimIntentPhase::Claimed(receipt) => receipt.claim_transfer.as_ref()
            .map(|proof| proof.fee_e8s).ok_or("claim has no exact outbound transfer fee")?,
        _ => return Err("claim return has no exact claimed receipt".into()),
    };
    if transfer.block_index != Some(return_block_index)
        || transfer.claim_timestamp != claim_timestamp
        || !claim_return_fee_matches_claim_proof(&transfer, pinned_fee)
        || !swap::claim_return_transfer_matches(&transfer, ic_cdk::id(), config, vault_id, claim_timestamp)
    {
        return Err("claim cancellation return block differs from the durable generation-bound transfer".into());
    }
    let return_created_at_time = transfer.args.created_at_time
        .ok_or("claim return tuple has no created_at_time")?;
    let result: Result<(BackendResult<()>,), _> =
        ic_cdk::call(
            config.backend_principal,
            "bot_cancel_liquidation_with_generation",
            (vault_id, claim_timestamp, return_block_index, return_created_at_time, transfer.ledger_fee_e8s),
        ).await;

    match result {
        Ok((BackendResult::Ok(()),)) => Ok(()),
        Ok((BackendResult::Err(e),)) => Err(format!("{}", e)),
        Err((code, msg)) => Err(format!("{:?}: {}", code, msg)),
    }
}

async fn call_bot_acknowledge_claim_cancellation(
    config: &BotConfig,
    vault_id: u64,
    claim_timestamp: u64,
) -> Result<(), String> {
    let result: Result<(BackendResult<()>,), _> = ic_cdk::call(
        config.backend_principal,
        "bot_acknowledge_claim_cancellation",
        (vault_id, claim_timestamp),
    ).await;
    match result {
        Ok((BackendResult::Ok(()),)) => Ok(()),
        Ok((BackendResult::Err(error),)) => Err(format!("{error}")),
        Err((code, message)) => Err(format!("{code:?}: {message}")),
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

    #[test]
    fn fee_buffered_return_requires_collateral_plus_both_transfer_fees() {
        let collateral = 1_000;
        let fee = 20;
        let required = claim_return_gross_debit(collateral, fee).unwrap();
        assert_eq!(required, 1_040);
        assert!(1_039 < required);
        assert!(1_040 >= required);
        let escrow_credit = collateral + fee;
        let backend_credit = escrow_credit - fee;
        assert_eq!(backend_credit, collateral);
    }

    #[test]
    fn returned_collateral_requires_generation_ack_before_recovery_completion() {
        assert!(!claim_return_completion_is_verified(true, Some(9)));
        assert!(!claim_return_completion_is_verified(false, None));
        assert!(claim_return_completion_is_verified(false, Some(9)));
    }

    #[test]
    fn claim_transfer_receipt_is_generation_and_account_bound() {
        let backend = Principal::from_slice(&[0x41]);
        let bot = Principal::from_slice(&[0x42]);
        let ledger = Principal::from_slice(&[0x43]);
        let vault_id = 7;
        let claim_timestamp = 55;
        let proof = history::BotClaimTransferReceipt {
            ledger,
            block_index: 9,
            from: icrc_ledger_types::icrc1::account::Account { owner: backend, subaccount: None },
            to: icrc_ledger_types::icrc1::account::Account { owner: bot, subaccount: None },
            amount_e8s: 100,
            fee_e8s: 10,
            memo: b"transfer-55".to_vec(),
            created_at_time: 56,
            return_account: icrc_ledger_types::icrc1::account::Account {
                owner: backend,
                subaccount: Some(swap::bot_claim_return_subaccount(vault_id, claim_timestamp)),
            },
        };
        assert!(claim_transfer_receipt_is_complete(
            backend, bot, ledger, vault_id, claim_timestamp, 100, &proof,
        ));
        assert!(!claim_transfer_receipt_is_complete(
            backend, bot, ledger, vault_id + 1, claim_timestamp, 100, &proof,
        ));
        assert!(!claim_transfer_receipt_is_complete(
            backend, bot, ledger, vault_id, claim_timestamp, 101, &proof,
        ));
        assert!(!claim_transfer_receipt_is_complete(
            backend, bot, Principal::anonymous(), vault_id, claim_timestamp, 100, &proof,
        ));
    }

    #[test]
    fn partial_payment_lock_requires_exact_incomplete_block_evidence() {
        let ledger = Principal::from_slice(&[0x43]);
        let memo = b"claim-payment-55";
        let evidence = BotPartialPaymentLockEvidence {
            ledger,
            vault_id: 7,
            claim_timestamp: 55,
            payment_memo: memo.to_vec(),
            payment_block_index: 9,
            total_amount_e6: 123,
            aggregate_complete: false,
        };
        let matches = |evidence: &BotPartialPaymentLockEvidence| {
            partial_payment_lock_evidence_matches(
                evidence, ledger, 7, 55, memo, 9, 123,
            )
        };

        assert!(matches(&evidence));
        let mut wrong_ledger = evidence.clone();
        wrong_ledger.ledger = Principal::from_slice(&[0x44]);
        assert!(!matches(&wrong_ledger));
        let mut wrong_vault = evidence.clone();
        wrong_vault.vault_id += 1;
        assert!(!matches(&wrong_vault));
        let mut wrong_generation = evidence.clone();
        wrong_generation.claim_timestamp += 1;
        assert!(!matches(&wrong_generation));
        let mut wrong_memo = evidence.clone();
        wrong_memo.payment_memo.push(0);
        assert!(!matches(&wrong_memo));
        let mut wrong_block = evidence.clone();
        wrong_block.payment_block_index += 1;
        assert!(!matches(&wrong_block));
        let mut wrong_amount = evidence.clone();
        wrong_amount.total_amount_e6 += 1;
        assert!(!matches(&wrong_amount));
        let mut complete = evidence;
        complete.aggregate_complete = true;
        assert!(!matches(&complete));
    }

    #[test]
    fn automatic_claim_swap_remains_disabled_before_router_dispatch() {
        assert!(!AUTOMATIC_CLAIM_SWAP_ENABLED);
    }

    #[test]
    fn disabled_automatic_claim_path_preserves_pending_vault_without_dispatch() {
        init_claim_history();
        let vault = LiquidatableVaultInfo {
            vault_id: 7,
            collateral_type: Principal::from_slice(&[0x41]),
            debt_amount: 90,
            collateral_amount: 100,
            recommended_liquidation_amount: 90,
            collateral_price_e8s: 1_000,
        };
        let mut pending = vec![vault.clone()];

        assert!(take_pending_vault_for_automatic_processing(
            &mut pending,
            AUTOMATIC_CLAIM_SWAP_ENABLED,
        ).is_none());
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].vault_id, vault.vault_id);
    }

    const SWAP_ERR: &str = "Quote returned zero output";
    const RETURN_ERR: &str = "Transfer error: BadFee";
    const CANCEL_ERR: &str = "GenericError(\"Cannot cancel claim for vault #7: protocol collateral balance 0 < required 99990000\")";

    fn requested_intent(vault_id: u64, request_id: u64, backend: candid::Principal) -> history::BotClaimIntent {
        history::BotClaimIntent {
            vault_id,
            request_id,
            claim_call_count: Some(1),
            backend_principal: Some(backend),
            return_transfer: None,
            ckusdc_top_up_transfer: None,
            ckusdc_payment_transfer: None,
            shortfall_eligibility: None,
            icp_pool_input_fee_e8s: None,
            phase: history::BotClaimIntentPhase::ClaimRequested,
        }
    }

    fn init_claim_history() {
        crate::memory::init_memory_manager();
        history::init_history();
    }

    #[test]
    fn paused_timer_does_not_pop_vault_or_create_claim_intent() {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};

        init_claim_history();
        let vault = LiquidatableVaultInfo {
            vault_id: 707,
            collateral_type: Principal::from_slice(&[0x42]),
            debt_amount: 90,
            collateral_amount: 100,
            recommended_liquidation_amount: 90,
            collateral_price_e8s: 1_000,
        };
        let mut bot = crate::state::BotState::default();
        bot.pending_vaults.push(vault.clone());
        state::init_state(bot);

        // This is the real timer entry point. With no ACK-only work it must
        // complete before any backend call, queue pop, or claim-intent write.
        let mut future = std::pin::pin!(process_pending());
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(Future::poll(future.as_mut(), &mut context), Poll::Ready(())));
        assert_eq!(
            state::read_state(|s| s.pending_vaults.iter().map(|queued| queued.vault_id).collect::<Vec<_>>()),
            vec![vault.vault_id],
        );
        assert!(history::get_claim_intent(707).is_none());
    }

    #[test]
    fn paused_automatic_queue_processes_only_no_effect_ack_and_keeps_other_vaults_queued() {
        init_claim_history();
        let backend = Principal::from_slice(&[0x41]);
        let mut ack_intent = requested_intent(8, 45, backend);
        ack_intent.phase = history::BotClaimIntentPhase::NoEffectAcknowledgementPending {
            transfer_digest: vec![0x55; 32],
        };
        history::put_claim_intent(ack_intent);

        let vault = |vault_id| LiquidatableVaultInfo {
            vault_id,
            collateral_type: Principal::from_slice(&[0x42]),
            debt_amount: 90,
            collateral_amount: 100,
            recommended_liquidation_amount: 90,
            collateral_price_e8s: 1_000,
        };
        let mut pending = vec![vault(7), vault(8), vault(9)];

        let selected = take_pending_vault_for_automatic_processing(&mut pending, false)
            .expect("safe no-effect ACK should remain reconcilable while claims are paused");
        assert_eq!(selected.vault_id, 8);
        assert_eq!(pending.iter().map(|vault| vault.vault_id).collect::<Vec<_>>(), vec![7, 8, 9]);

        // Failed/mismatched ACK handling performs no queue removal, so the
        // next timer can retry the same durable ACK without creating a claim.
        let selected_again = take_pending_vault_for_automatic_processing(&mut pending, false)
            .expect("unresolved ACK remains queued for the next timer");
        assert_eq!(selected_again.vault_id, 8);
        assert_eq!(pending.iter().map(|vault| vault.vault_id).collect::<Vec<_>>(), vec![7, 8, 9]);

        // The success path removes the queue item only after the ACK and
        // durable intent clear have completed.
        assert!(history::clear_acknowledged_no_effect_claim_intent(
            8,
            45,
            backend,
            &[0x55; 32],
        ));
        remove_pending_vault(&mut pending, 8);
        assert!(take_pending_vault_for_automatic_processing(&mut pending, false).is_none());
        assert_eq!(pending.iter().map(|vault| vault.vault_id).collect::<Vec<_>>(), vec![7, 9]);
    }

    #[test]
    fn lost_reply_retries_same_request_and_swap_started_claim_is_quarantined() {
        let pending = history::BotClaimIntent {
            vault_id: 7,
            request_id: 44,
            claim_call_count: Some(1),
            backend_principal: Some(candid::Principal::from_slice(&[0x41])),
            return_transfer: None,
            ckusdc_top_up_transfer: None,
            ckusdc_payment_transfer: None,
            shortfall_eligibility: None,
            icp_pool_input_fee_e8s: None,
            phase: history::BotClaimIntentPhase::ClaimRequested,
        };
        assert_eq!(claim_intent_action(Some(pending), 99), ClaimIntentAction::Retry(44));
        let claimed = history::BotClaimIntent {
            vault_id: 7,
            request_id: 44,
            claim_call_count: Some(1),
            backend_principal: Some(candid::Principal::from_slice(&[0x41])),
            return_transfer: None,
            ckusdc_top_up_transfer: None,
            ckusdc_payment_transfer: None,
            shortfall_eligibility: None,
            icp_pool_input_fee_e8s: None,
            phase: history::BotClaimIntentPhase::Claimed(history::BotClaimReceipt {
                collateral_amount: 100,
                debt_covered: 90,
                collateral_price_e8s: 1_000,
                claim_timestamp: 55,
                payment_memo: b"receipt".to_vec(),
                claim_transfer: None,
            }),
        };
        assert_eq!(claim_intent_action(Some(claimed.clone()), 99), ClaimIntentAction::RecoverClaimed(44));
        let mut returning = claimed.clone();
        returning.return_transfer = Some(history::BotClaimReturnTransfer {
            ledger: candid::Principal::from_slice(&[0x42]),
            from: icrc_ledger_types::icrc1::account::Account {
                owner: candid::Principal::from_slice(&[0x43]),
                subaccount: None,
            },
            collateral_amount_e8s: 100,
            ledger_fee_e8s: 1,
            claim_timestamp: 55,
            args: icrc_ledger_types::icrc1::transfer::TransferArg {
                from_subaccount: None,
                to: icrc_ledger_types::icrc1::account::Account {
                    owner: candid::Principal::from_slice(&[0x44]),
                    subaccount: None,
                },
                amount: candid::Nat::from(101u64),
                fee: Some(candid::Nat::from(1u64)),
                memo: Some(icrc_ledger_types::icrc1::transfer::Memo::from(b"return".to_vec())),
                created_at_time: Some(56),
            },
            block_index: None,
            dispatch_attempt_count: Some(0),
            prior_no_effects: Some(Vec::new()),
            dispatch_observation: None,
        });
        assert_eq!(claim_intent_action(Some(returning), 99), ClaimIntentAction::HoldReturnRecovery(44));
        let receipt = match claimed.phase {
            history::BotClaimIntentPhase::Claimed(receipt) => receipt,
            _ => unreachable!(),
        };
        let swap_started = history::BotClaimIntent {
            vault_id: claimed.vault_id,
            request_id: claimed.request_id,
            claim_call_count: claimed.claim_call_count,
            backend_principal: claimed.backend_principal,
            return_transfer: claimed.return_transfer,
            ckusdc_top_up_transfer: claimed.ckusdc_top_up_transfer,
            ckusdc_payment_transfer: claimed.ckusdc_payment_transfer,
            shortfall_eligibility: claimed.shortfall_eligibility,
            icp_pool_input_fee_e8s: claimed.icp_pool_input_fee_e8s,
            phase: history::BotClaimIntentPhase::SwapStarted(receipt),
        };
        assert_eq!(claim_intent_action(Some(swap_started), 99), ClaimIntentAction::QuarantineSwapStarted(44));
        let acquired = history::BotClaimIntent {
            vault_id: 7,
            request_id: 44,
            claim_call_count: Some(1),
            backend_principal: Some(candid::Principal::from_slice(&[0x41])),
            return_transfer: None,
            ckusdc_top_up_transfer: None,
            ckusdc_payment_transfer: None,
            shortfall_eligibility: None,
            icp_pool_input_fee_e8s: None,
            phase: history::BotClaimIntentPhase::Acquired {
                claim_timestamp: 55,
                payment_memo: b"receipt".to_vec(),
            },
        };
        assert_eq!(claim_intent_action(Some(acquired), 99), ClaimIntentAction::HoldLegacyAcquired(44));
        assert_eq!(claim_intent_action(None, 99), ClaimIntentAction::Create(99));
    }

    #[test]
    fn bot_claim_reply_classifies_transport_loss_separately_from_typed_rejection() {
        assert!(BotClaimCallError::TransportUnknown("reply lost".into()).is_unresolved());
        let not_found = BotClaimCallError::BackendRejected(BackendError::VaultNotFound { vault_id: 7 });
        assert!(!claim_call_is_unresolved(Some(0), &not_found));
        assert!(claim_call_is_unresolved(Some(1), &not_found));
        assert!(claim_call_is_unresolved(None, &not_found), "legacy state has no call-count proof");
        assert!(BotClaimCallError::BackendRejected(BackendError::TemporarilyUnavailable(
            "pending transfer unresolved".into(),
        )).is_unresolved());
    }

    #[test]
    fn lost_claim_reply_recovers_typed_no_effect_then_retries_lost_ack_without_reclaiming() {
        init_claim_history();
        let backend = candid::Principal::from_slice(&[0x41]);
        let mut intent = requested_intent(7, 44, backend);
        intent.claim_call_count = Some(2); // First reply was lost; same ID was retried.
        history::put_claim_intent(intent);
        let lost_claim_reply = BotClaimCallError::TransportUnknown("reply lost after ledger rejection".into());
        assert!(claim_call_is_unresolved(Some(1), &lost_claim_reply));
        let proof = BotClaimNoEffectProof {
            vault_id: 7,
            request_id: 44,
            transfer_digest: vec![0x55; 32],
        };
        assert!(bot_no_effect_proof_matches(7, 44, &proof));
        assert!(history::mark_no_effect_acknowledgement_pending(
            7, 44, backend, proof.transfer_digest.clone(),
        ));
        assert_eq!(claim_intent_action(history::get_claim_intent(7), 45),
            ClaimIntentAction::AcknowledgeNoEffect { request_id: 44, transfer_digest: proof.transfer_digest.clone() });
        // Simulate the backend committing ACK while the bot loses its reply:
        // the stable phase remains ACK-only and cannot redispatch a claim.
        assert_eq!(claim_intent_action(history::get_claim_intent(7), 45),
            ClaimIntentAction::AcknowledgeNoEffect { request_id: 44, transfer_digest: proof.transfer_digest.clone() });
        assert!(history::clear_acknowledged_no_effect_claim_intent(
            7, 44, backend, &proof.transfer_digest,
        ));
        assert_eq!(history::get_claim_intent(7), None);
        assert_eq!(claim_intent_action(None, 45), ClaimIntentAction::Create(45));
    }

    #[test]
    fn absent_or_mismatched_typed_proof_never_releases_the_request_id() {
        init_claim_history();
        let backend = candid::Principal::from_slice(&[0x41]);
        let intent = requested_intent(7, 44, backend);
        history::put_claim_intent(intent.clone());
        let pre_reservation = BotClaimCallError::BackendRejected(BackendError::GenericError(
            "Vault is no longer liquidatable".into(),
        ));
        assert!(!claim_call_is_unresolved(Some(0), &pre_reservation));
        // The status endpoint yields None here, so the durable intent and ID
        // remain unchanged even though the typed validation error was definite.
        assert_eq!(history::get_claim_intent(7), Some(intent.clone()));
        let transport_unknown = BotClaimCallError::TransportUnknown("reply lost".into());
        assert!(claim_call_is_unresolved(None, &transport_unknown));
        assert!(claim_call_is_unresolved(Some(2), &transport_unknown));
        let wrong_vault = BotClaimNoEffectProof { vault_id: 8, request_id: 44, transfer_digest: vec![0x55; 32] };
        let wrong_id = BotClaimNoEffectProof { vault_id: 7, request_id: 45, transfer_digest: vec![0x55; 32] };
        let malformed_digest = BotClaimNoEffectProof { vault_id: 7, request_id: 44, transfer_digest: vec![0x55; 31] };
        assert!(!bot_no_effect_proof_matches(7, 44, &wrong_vault));
        assert!(!bot_no_effect_proof_matches(7, 44, &wrong_id));
        assert!(!bot_no_effect_proof_matches(7, 44, &malformed_digest));
        assert_eq!(claim_intent_action(Some(intent), 45), ClaimIntentAction::Retry(44));
    }

    #[test]
    fn give_up_limit_still_retries_ack_only_until_backend_receipt_is_cleared() {
        init_claim_history();
        let backend = candid::Principal::from_slice(&[0x41]);
        assert_eq!(next_claim_retry_action(2, CLAIM_RETRY_LIMIT), ClaimRetryAction::GiveUp);
        let proof = BotClaimNoEffectProof { vault_id: 8, request_id: 45, transfer_digest: vec![9; 32] };
        history::put_claim_intent(requested_intent(8, 45, backend));
        assert!(history::mark_no_effect_acknowledgement_pending(
            8, 45, backend, proof.transfer_digest.clone(),
        ));
        assert_eq!(claim_intent_action(history::get_claim_intent(8), 46),
            ClaimIntentAction::AcknowledgeNoEffect { request_id: 45, transfer_digest: proof.transfer_digest.clone() });
        assert!(history::get_claim_intent(8).is_some(), "GiveUp must not clear an unacknowledged backend receipt");
        assert!(history::clear_acknowledged_no_effect_claim_intent(8, 45, backend, &proof.transfer_digest));
        assert_eq!(history::get_claim_intent(8), None);
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
    fn short_wallet_delta_is_held_before_payment_but_exact_net_floor_passes() {
        // The pool quote may satisfy its minimum while the independently read
        // wallet delta is anomalously lower. Do not send a positive short
        // payment that the backend can never use to confirm this claim.
        assert_eq!(ckusdc_payment_shortfall_e6(1_010, 10, 1_000), None);
        assert_eq!(ckusdc_payment_shortfall_e6(1_011, 10, 1_000), None,
            "an above-minimum swap must not evaluate an underflowing shortfall");
        assert_eq!(ckusdc_payment_shortfall_e6(1_008, 10, 1_000), Some(2));
        assert_eq!(ckusdc_payment_shortfall_e6(9, 10, 1), Some(1));
    }

    #[test]
    fn treasury_bonus_gross_excludes_the_pool_input_fee_once() {
        assert_eq!(icp_treasury_bonus_gross_after_swap(1_000_000, 700_000, 10_000), 290_000);
        assert_eq!(icp_treasury_bonus_gross_after_swap(700_000, 700_000, 10_000), 0);
    }

    #[test]
    fn top_up_credit_and_fee_cover_only_the_verified_debt_shortfall() {
        let minimum = ckusdc_minimum_payment_e6(100_001);
        assert_eq!(minimum, 1_001);
        let verified_original = 999;
        let top_up_credit = minimum.checked_sub(verified_original).unwrap();
        assert_eq!(top_up_credit, 2);
        assert_eq!(verified_original + top_up_credit, minimum);
        assert_eq!(ckusdc_top_up_gross_e6(top_up_credit, 10_000), Some(10_002));
        assert_eq!(ckusdc_top_up_gross_e6(u64::MAX, 1), None);
    }

    #[test]
    fn prepared_top_up_is_exact_shortfall_from_claim_account() {
        let backend = Principal::from_slice(&[0x41]);
        let ledger = Principal::from_slice(&[0x42]);
        let config = BotConfig {
            backend_principal: backend,
            treasury_principal: Principal::from_slice(&[0x44]),
            admin: Principal::from_slice(&[0x45]),
            max_slippage_bps: 100,
            icp_ledger: Principal::from_slice(&[0x46]),
            ckusdc_ledger: ledger,
            icpswap_pool: Principal::from_slice(&[0x47]),
            icpswap_zero_for_one: None,
            icp_fee_e8s: None,
            ckusdc_fee_e6: None,
            three_pool_principal: None,
            kong_swap_principal: None,
            ckusdt_ledger: None,
            icusd_ledger: None,
        };
        let vault_id = 77;
        let claim_timestamp = 124;
        let memo = b"claim-shortfall";
        let transfer = prepare_ckusdc_top_up_transfer(
            &config, vault_id, claim_timestamp, memo, 2, 10_000, 100,
        ).unwrap();

        assert_eq!(transfer.ledger, ledger);
        assert_eq!(transfer.args.from_subaccount, Some(claim_payment_subaccount(vault_id, claim_timestamp)));
        assert_eq!(transfer.args.to, icrc_ledger_types::icrc1::account::Account {
            owner: backend,
            subaccount: None,
        });
        assert_eq!(transfer.args.amount, candid::Nat::from(2u64));
        assert_eq!(transfer.args.fee, Some(candid::Nat::from(10_000u64)));
        assert_eq!(transfer.args.memo.as_ref().map(|memo| memo.0.as_ref()), Some(memo.as_slice()));
        assert_eq!(transfer.args.created_at_time, Some(claim_timestamp));
        assert!(ckusdc_top_up_tuple_matches(
            &transfer, ledger, backend, vault_id, claim_timestamp, memo, 2,
        ));

        let mut pooled = transfer.clone();
        pooled.args.from_subaccount = None;
        assert!(!ckusdc_top_up_tuple_matches(
            &pooled, ledger, backend, vault_id, claim_timestamp, memo, 2,
        ));
        assert!(prepare_ckusdc_top_up_transfer(
            &config, vault_id, claim_timestamp, memo, u64::MAX, 1, 100,
        ).is_err());
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
    fn missing_balance_snapshot_never_uses_router_output_as_payment_credit() {
        assert!(compute_swap_reservation_from_snapshots(99, 1_000_000, None, Some(500_000)).is_err());
        assert!(compute_swap_reservation_from_snapshots(99, 1_000_000, Some(100), None).is_err());
        let complete = compute_swap_reservation_from_snapshots(99, 1_000_000, Some(100), Some(500_100)).unwrap();
        assert_eq!(complete.to_transfer_e6, 500_000);
        assert_eq!(complete.recorded_received_e6, 500_000);
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
