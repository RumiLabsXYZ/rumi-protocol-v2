use candid::{CandidType, Deserialize};
use ic_canister_log::log;

use crate::history::{self, LiquidationRecordV1, LiquidationRecordVersioned, LiquidationStatus};
use crate::state::{self, BotConfig};
use crate::swap;

const CONFIRM_ATTEMPTS: u8 = 5;
const CANCEL_ATTEMPTS: u8 = 3;
thread_local! {
    /// In-memory fairness cursor only; pending return journals remain the
    /// durable source of truth and selection restarts from the first claim.
    static RETURN_RESUME_CURSOR: std::cell::Cell<Option<u64>> = std::cell::Cell::new(None);
}
/// Number of fee-refresh tuples permitted after the original first dispatch.
/// Every typed first-dispatch BadFee is retained before a refresh.
const MAX_AUTO_RETURN_FEE_REFRESHES: usize = 3;

fn persist_swap_intent(
    vault_id: u64,
    claim_generation: u64,
    config: &BotConfig,
    prepared: &swap::PreparedSwap,
    pre_swap_ckusdc_balance_e6: u64,
) -> Result<(), String> {
    let now = ic_cdk::api::time();
    let persisted = state::mutate_state(|s| {
        let Some(claim) = s.pending_claims.get_mut(&vault_id) else {
            return false;
        };
        if claim.claim_generation != claim_generation
            || claim.status != state::BotClaimJournalStatus::SwapMayHaveStarted
        {
            return false;
        }
        let ordinal = u32::try_from(claim.swap_intents.len())
            .ok()
            .and_then(|length| length.checked_add(1));
        let Some(attempt_ordinal) = ordinal else { return false };
        claim.swap_intents.push(state::BotSwapIntent {
            pool_principal: config.icpswap_pool,
            input_ledger_principal: config.icp_ledger,
            output_ledger_principal: config.ckusdc_ledger,
            amount_in_e8s: prepared.amount_in_e8s,
            amount_out_minimum_e6: prepared.amount_out_minimum_e6,
            zero_for_one: prepared.zero_for_one,
            input_fee_e8s: prepared.input_fee_e8s,
            output_fee_e6: prepared.output_fee_e6,
            attempt_ordinal,
            created_at_time: now,
            pre_swap_ckusdc_balance_e6,
        });
        true
    });
    if !persisted {
        return Err("claim generation changed or swap intent could not be persisted".into());
    }
    state::save_config_to_stable();
    Ok(())
}

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

fn shortfall_residual(journal: &state::BotPaymentJournal) -> Result<(u64, u64), String> {
    let receipt = journal
        .receipt
        .as_ref()
        .ok_or_else(|| "shortfall recovery requires the original payment receipt".to_string())?;
    let required = required_ckusdc_net(journal.debt_covered_e8s);
    if receipt.amount >= required {
        return Err("original payment receipt is not short".into());
    }
    let residual = required
        .checked_sub(receipt.amount)
        .ok_or_else(|| "shortfall residual underflow".to_string())?;
    if residual == 0 {
        return Err("shortfall residual is zero".into());
    }
    Ok((receipt.amount, residual))
}

/// Bind an operator-supplied ledger block to the already persisted residual
/// intent. The backend independently proves this tuple before this helper is
/// called; keeping the checks pure makes it difficult to accidentally replace
/// the original short receipt or accept a changed claim tuple.
fn reconciled_shortfall_receipt(
    journal: &state::BotPaymentJournal,
    block_index: u64,
) -> Result<swap::TransferReceipt, String> {
    if !journal.shortfall_receipt_observed {
        return Err("payment journal is not in shortfall recovery".into());
    }
    let (_, residual) = shortfall_residual(journal)?;
    let topup = journal.shortfall_topup.as_ref()
        .ok_or_else(|| "shortfall residual intent is missing".to_string())?;
    if topup.backend_principal != journal.backend_principal
        || topup.ledger_principal != journal.ledger_principal
        || topup.memo != journal.memo
        || topup.amount_e6 != residual
        || topup.funding_allocation_e6 != topup.amount_e6.checked_add(topup.fee_e6)
            .ok_or_else(|| "residual amount plus fee overflowed".to_string())?
        || topup.created_at_time <= journal.receipt.as_ref().unwrap().created_at_time
    {
        return Err("persisted residual tuple does not match the shortfall claim".into());
    }
    if let Some(receipt) = topup.receipt.as_ref() {
        if receipt.block_index != block_index
            || receipt.amount != topup.amount_e6
            || receipt.created_at_time != topup.created_at_time
        {
            return Err("residual receipt is already bound to a different block".into());
        }
        return Ok(receipt.clone());
    }
    let original = journal.receipt.as_ref().expect("shortfall_residual checked receipt");
    if block_index == original.block_index {
        return Err("residual block must differ from the original short-payment block".into());
    }
    Ok(swap::TransferReceipt {
        block_index,
        amount: topup.amount_e6,
        created_at_time: topup.created_at_time,
    })
}

fn dispatch_only_after_original_receipt_verification<T>(
    verification: Result<(), String>,
    transfer: impl FnOnce() -> T,
) -> Result<T, String> {
    verification?;
    Ok(transfer())
}

fn prepare_shortfall_topup_intent(
    journal: &state::BotPaymentJournal,
    fee_e6: u64,
    created_at_time: u64,
    funding_allocation_e6: u64,
) -> Result<state::BotPaymentTopUpJournal, String> {
    let (_, residual) = shortfall_residual(journal)?;
    let exact_allocation = residual
        .checked_add(fee_e6)
        .ok_or_else(|| "residual amount plus ledger fee overflowed".to_string())?;
    if funding_allocation_e6 != exact_allocation {
        return Err(format!(
            "administrator allocation must equal the exact residual plus fee: {} e6",
            exact_allocation
        ));
    }
    let original = journal.receipt.as_ref().expect("shortfall_residual checked receipt");
    if created_at_time <= original.created_at_time {
        return Err("clock has not advanced beyond the original payment tuple".into());
    }
    Ok(state::BotPaymentTopUpJournal {
        backend_principal: journal.backend_principal,
        ledger_principal: journal.ledger_principal,
        amount_e6: residual,
        fee_e6,
        created_at_time,
        memo: journal.memo.clone(),
        funding_allocation_e6,
        status: state::BotPaymentStatus::Prepared,
        receipt: None,
    })
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

const VAULT158_ID: u64 = 158;
const VAULT158_RECORD_ID: u64 = 229;
const VAULT158_GENERATION: u64 = 1;
const VAULT158_CLAIM_GROSS_E8S: u64 = 236_783_835;
const VAULT158_CLAIM_NET_E8S: u64 = 236_773_835;
const VAULT158_DEBT_E8S: u64 = 513_841_815;
const VAULT158_SWAP_INPUT_BLOCK: u64 = 39_835_236;
const VAULT158_SWAP_INPUT_E8S: u64 = 226_020_933;
const VAULT158_SWAP_INPUT_FEE_E8S: u64 = 10_000;
const VAULT158_SWAP_INPUT_TIME: u64 = 1_793_306_981_079_625_901;
const VAULT158_SWAP_OUTPUT_BLOCK: u64 = 806_697;
const VAULT158_SWAP_OUTPUT_E6: u64 = 5_545_309;
const VAULT158_CKUSDC_BEFORE_SWAP_E6: u64 = 26_190_995;
const VAULT158_CKUSDC_FEE_E6: u64 = 10_000;
const VAULT158_POOL_MEMO_BYTES: &[u8] = &[0, 0, 0, 0, 0, 0x11, 0xbe, 0x56];

fn vault158_principal(text: &str) -> candid::Principal {
    candid::Principal::from_text(text).expect("pinned vault-158 principal is valid")
}

fn vault158_identity_matches(
    config: &BotConfig,
    claim: &state::BotClaimJournal,
    record: &LiquidationRecordV1,
) -> Result<(), String> {
    vault158_identity_matches_for_bot(config, claim, record, ic_cdk::id())
}

fn vault158_identity_matches_for_bot(
    config: &BotConfig,
    claim: &state::BotClaimJournal,
    record: &LiquidationRecordV1,
    bot_id: candid::Principal,
) -> Result<(), String> {
    if bot_id != vault158_principal("nygob-3qaaa-aaaap-qttcq-cai")
        || config.backend_principal != vault158_principal("tfesu-vyaaa-aaaap-qrd7a-cai")
        || config.icpswap_pool != vault158_principal("mohjv-bqaaa-aaaag-qjyia-cai")
        || config.icp_ledger != vault158_principal("ryjl3-tyaaa-aaaaa-aaaba-cai")
        || config.ckusdc_ledger != vault158_principal("xevnm-gaaaa-aaaar-qafnq-cai")
    {
        return Err("configured canister identities do not match the pinned vault-158 recovery".into());
    }
    if claim.vault_id != VAULT158_ID
        || claim.claim_generation != VAULT158_GENERATION
        || claim.status != state::BotClaimJournalStatus::PaymentShortfall
        || claim.collateral_amount_e8s != VAULT158_CLAIM_GROSS_E8S
        || claim.collateral_received_amount_e8s != Some(VAULT158_CLAIM_NET_E8S)
        || claim.collateral_outbound_fee_e8s != Some(10_000)
        || claim.debt_covered_e8s != VAULT158_DEBT_E8S
        || claim.collateral_return.is_some()
        || !claim.failed_return_attempts.is_empty()
        || claim.payment_memo != payment_memo(VAULT158_ID, VAULT158_GENERATION)
        || claim.swap_intents.len() != 1
    {
        return Err("active claim is not the pinned vault-158 generation-1 PaymentShortfall".into());
    }
    let intent = &claim.swap_intents[0];
    if intent.pool_principal != config.icpswap_pool
        || intent.input_ledger_principal != config.icp_ledger
        || intent.output_ledger_principal != config.ckusdc_ledger
        || intent.amount_in_e8s != VAULT158_SWAP_INPUT_E8S
        || intent.input_fee_e8s != VAULT158_SWAP_INPUT_FEE_E8S
        || intent.pre_swap_ckusdc_balance_e6 != VAULT158_CKUSDC_BEFORE_SWAP_E6
        || intent.attempt_ordinal != 1
    {
        return Err("saved swap intent does not match the single pinned vault-158 swap".into());
    }
    if record.id != VAULT158_RECORD_ID
        || record.vault_id != VAULT158_ID
        || record.status != LiquidationStatus::TransferFailed
        || record.collateral_claimed_e8s != VAULT158_CLAIM_GROSS_E8S
        || record.debt_to_cover_e8s != VAULT158_DEBT_E8S
        || record.icp_swapped_e8s != VAULT158_SWAP_INPUT_E8S
        || record.ckusdc_received_e6 != 0
        || record.ckusdc_transferred_e6 != 0
    {
        return Err("history record 229 does not match the pinned unrecovered shortfall".into());
    }
    Ok(())
}

fn vault158_exact_evidence(
    evidence: &state::Vault158RecoveryEvidence,
) -> bool {
    evidence.record_id == VAULT158_RECORD_ID
        && evidence.vault_id == VAULT158_ID
        && evidence.claim_generation == VAULT158_GENERATION
        && evidence.backend_principal == vault158_principal("tfesu-vyaaa-aaaap-qrd7a-cai")
        && evidence.pool_principal == vault158_principal("mohjv-bqaaa-aaaag-qjyia-cai")
        && evidence.icp_ledger_principal == vault158_principal("ryjl3-tyaaa-aaaaa-aaaba-cai")
        && evidence.ckusdc_ledger_principal == vault158_principal("xevnm-gaaaa-aaaar-qafnq-cai")
        && evidence.collateral_amount_e8s == VAULT158_CLAIM_GROSS_E8S
        && evidence.collateral_received_e8s == VAULT158_CLAIM_NET_E8S
        && evidence.debt_covered_e8s == VAULT158_DEBT_E8S
        && evidence.swap_input_amount_e8s == VAULT158_SWAP_INPUT_E8S
        && evidence.swap_input_fee_e8s == VAULT158_SWAP_INPUT_FEE_E8S
        && evidence.swap_output_amount_e6 == VAULT158_SWAP_OUTPUT_E6
        && evidence.swap_input_receipt.block_index == VAULT158_SWAP_INPUT_BLOCK
        && evidence.swap_input_receipt.amount == VAULT158_SWAP_INPUT_E8S
        && evidence.swap_input_receipt.created_at_time == VAULT158_SWAP_INPUT_TIME
        && evidence.swap_output_receipt.block_index == VAULT158_SWAP_OUTPUT_BLOCK
        && evidence.swap_output_receipt.amount == VAULT158_SWAP_OUTPUT_E6
        && evidence.swap_output_receipt.created_at_time == 0
        && evidence.pre_swap_ckusdc_balance_e6 == VAULT158_CKUSDC_BEFORE_SWAP_E6
        && evidence.held_surplus_e6 == 396_890
        && evidence.payment_gross_e6 == 5_148_419
        && evidence.payment_amount_e6 == 5_138_419
        && evidence.payment_fee_e6 == VAULT158_CKUSDC_FEE_E6
        && evidence.payment_memo == payment_memo(VAULT158_ID, VAULT158_GENERATION)
}

fn payment_memo(vault_id: u64, generation: u64) -> Vec<u8> {
    let mut memo = b"RUMI-BPAY-V2:".to_vec();
    memo.extend_from_slice(&vault_id.to_be_bytes());
    memo.extend_from_slice(&generation.to_be_bytes());
    memo
}

fn claim_icp_residual_after_swap(
    collateral_received_e8s: u64,
    swapped_e8s: u64,
    swap_input_fee_e8s: u64,
) -> u64 {
    collateral_received_e8s
        .saturating_sub(swapped_e8s)
        .saturating_sub(swap_input_fee_e8s)
}

/// Claim-attributable ICP still unspent: net collateral minus swapped inputs
/// and the input fees ICPSwap charged separately for them.
fn claim_icp_remaining(net_e8s: u64, swapped_e8s: u64, swap_input_fees_e8s: u64) -> u64 {
    net_e8s
        .saturating_sub(swapped_e8s)
        .saturating_sub(swap_input_fees_e8s)
}

/// Largest swap input whose amount plus its own input fee fits in `available`.
fn max_swap_input_for_claim(available_e8s: u64, input_fee_e8s: u64) -> u64 {
    available_e8s.saturating_sub(input_fee_e8s)
}

/// A swap may dispatch only if input plus fee never draws on shared float.
fn swap_fits_claim(available_e8s: u64, amount_e8s: u64, input_fee_e8s: u64) -> bool {
    amount_e8s > 0 && amount_e8s.checked_add(input_fee_e8s).is_some_and(|total| total <= available_e8s)
}

fn vault158_payment_journal_matches_evidence(
    journal: &state::BotPaymentJournal,
    evidence: &state::Vault158RecoveryEvidence,
) -> bool {
    journal.record_id == Some(evidence.record_id)
        && journal.vault_id == evidence.vault_id
        && journal.claim_generation == evidence.claim_generation
        && journal.ledger_principal == evidence.ckusdc_ledger_principal
        && journal.backend_principal == evidence.backend_principal
        && journal.amount_e6 == evidence.payment_amount_e6
        && journal.gross_amount_e6 == evidence.payment_gross_e6
        && journal.fee_e6 == evidence.payment_fee_e6
        && Some(journal.created_at_time) == evidence.payment_created_at_time
        && journal.memo == evidence.payment_memo
        && journal.swap_input_fee_e8s == evidence.swap_input_fee_e8s
        && match (&journal.receipt, &evidence.payment_receipt) {
            (None, None) => true,
            (Some(journal_receipt), Some(evidence_receipt)) => {
                journal_receipt == evidence_receipt
                    && journal_receipt.amount == journal.amount_e6
                    && journal_receipt.created_at_time == journal.created_at_time
            }
            _ => false,
        }
}

fn vault158_balance_matches_exact_allocation(
    balance_e6: u64,
    evidence: &state::Vault158RecoveryEvidence,
) -> bool {
    evidence.pre_swap_ckusdc_balance_e6
        .checked_add(evidence.swap_output_amount_e6)
        == Some(balance_e6)
}

fn select_pending_payment(
    bot_state: &state::BotState,
    only_vault_id: Option<u64>,
) -> Option<state::BotPaymentJournal> {
    bot_state.pending_payments.iter()
        .find(|(vault_id, _)| only_vault_id.map_or(true, |expected| **vault_id == expected))
        .map(|(_, journal)| journal.clone())
}

fn vault158_claim_matches_evidence(
    claim: &state::BotClaimJournal,
    evidence: &state::Vault158RecoveryEvidence,
) -> bool {
    claim.vault_id == evidence.vault_id
        && claim.claim_generation == evidence.claim_generation
        && claim.status == state::BotClaimJournalStatus::PaymentShortfall
        && claim.collateral_amount_e8s == evidence.collateral_amount_e8s
        && claim.collateral_received_amount_e8s == Some(evidence.collateral_received_e8s)
        && claim.debt_covered_e8s == evidence.debt_covered_e8s
        && claim.payment_memo == evidence.payment_memo
        && claim.collateral_return.is_none()
        && claim.failed_return_attempts.is_empty()
}

/// Promote the exact held claim into its payment journal in one durable state
/// mutation. The claim is removed only after its generation and no-return
/// evidence still match the retained receipt record.
fn commit_vault158_payment_preparation_once(
    bot_state: &mut state::BotState,
    evidence: &state::Vault158RecoveryEvidence,
    journal: state::BotPaymentJournal,
) -> bool {
    if !vault158_exact_evidence(evidence)
        || evidence.status != state::Vault158RecoveryStatus::PaymentPrepared
        || !vault158_payment_journal_matches_evidence(&journal, evidence)
        || !bot_state.processing_paused
        || !bot_state.pending_payments.is_empty()
        || !bot_state.pending_treasury.is_empty()
        || bot_state.vault158_recovery.as_ref().is_some_and(|saved| {
            saved.status != state::Vault158RecoveryStatus::EvidencePrepared
                || !vault158_exact_evidence(saved)
        })
        || !bot_state.vault158_recovery.as_ref().is_some_and(vault158_exact_evidence)
    {
        return false;
    }
    let Some(claim) = bot_state.pending_claims.get(&evidence.vault_id) else {
        return false;
    };
    if !vault158_claim_matches_evidence(claim, evidence) {
        return false;
    }
    bot_state.pending_claims.remove(&evidence.vault_id);
    bot_state.vault158_recovery = Some(evidence.clone());
    bot_state.pending_payments.insert(evidence.vault_id, journal);
    true
}

/// Verify and persist the two already-completed swap receipts for this one
/// historical claim. This endpoint is preparation-only: it creates no payment
/// tuple, does not call the backend, and never dispatches a transfer.
pub async fn admin_prepare_vault158_recovery(config: &BotConfig) -> Result<(), String> {
    if let Some(existing) = state::read_state(|s| s.vault158_recovery.clone()) {
        return if vault158_exact_evidence(&existing) {
            Err("vault-158 swap receipts were already consumed by a durable recovery record".into())
        } else {
            Err("a different vault-158 recovery tombstone already exists".into())
        };
    }
    let claim = state::read_state(|s| s.pending_claims.get(&VAULT158_ID).cloned())
        .ok_or_else(|| "no active vault-158 claim journal".to_string())?;
    let record = match history::get_record(VAULT158_RECORD_ID) {
        Some(crate::history::LiquidationRecordVersioned::V1(record)) => record,
        None => return Err("history record 229 is absent".into()),
    };
    vault158_identity_matches(config, &claim, &record)?;
    let has_conflicting_work = state::read_state(|s| {
        !s.pending_payments.is_empty()
            || s.pending_claims.keys().any(|id| *id != VAULT158_ID)
            || s.pending_treasury.values().any(|j| j.vault_id == VAULT158_ID)
    });
    if has_conflicting_work {
        return Err("another payment or claim recovery would make vault-158 allocation ambiguous".into());
    }

    swap::verify_vault158_swap_input_receipt(
        config.icp_ledger,
        ic_cdk::id(),
        config.icpswap_pool,
        VAULT158_SWAP_INPUT_BLOCK,
        VAULT158_SWAP_INPUT_E8S,
        VAULT158_SWAP_INPUT_FEE_E8S,
        VAULT158_POOL_MEMO_BYTES,
        VAULT158_SWAP_INPUT_TIME,
    ).await.map_err(|error| format!("pinned ICP swap-input receipt failed verification: {error}"))?;
    verify_vault158_swap_output_receipt(config, ic_cdk::id()).await?;

    let balance = swap::balance_of_self_ckusdc(config).await
        .map_err(|error| format!("ckUSDC balance read failed: {error}"))?;
    let required_balance = VAULT158_CKUSDC_BEFORE_SWAP_E6
        .checked_add(VAULT158_SWAP_OUTPUT_E6)
        .ok_or_else(|| "pinned ckUSDC allocation overflowed".to_string())?;
    if balance != required_balance {
        return Err("current ckUSDC balance differs from the exact pre-swap balance plus verified output; aggregate balance cannot authorize recovery".into());
    }
    let current_claim = state::read_state(|s| s.pending_claims.get(&VAULT158_ID).cloned());
    let current_config = state::read_state(|s| s.config.clone());
    let current_record = match history::get_record(VAULT158_RECORD_ID) {
        Some(crate::history::LiquidationRecordVersioned::V1(record)) => record,
        None => return Err("history record 229 disappeared during receipt verification".into()),
    };
    let current_claim = current_claim.ok_or_else(|| "active vault-158 claim disappeared during receipt verification".to_string())?;
    let current_config = current_config.ok_or_else(|| "bot configuration disappeared during receipt verification".to_string())?;
    vault158_identity_matches(&current_config, &current_claim, &current_record)?;
    if state::read_state(|s| s.vault158_recovery.is_some() || !s.pending_payments.is_empty()) {
        return Err("vault-158 receipt evidence was consumed by another recovery call".into());
    }
    let minimum_net = required_ckusdc_net(VAULT158_DEBT_E8S);
    let gross = minimum_net.checked_add(VAULT158_CKUSDC_FEE_E6)
        .ok_or_else(|| "ckUSDC payment amount overflowed".to_string())?;
    let held_surplus = VAULT158_SWAP_OUTPUT_E6.checked_sub(gross)
        .ok_or_else(|| "verified swap output cannot cover exact debt payment".to_string())?;
    let evidence = state::Vault158RecoveryEvidence {
        record_id: VAULT158_RECORD_ID,
        vault_id: VAULT158_ID,
        claim_generation: VAULT158_GENERATION,
        backend_principal: config.backend_principal,
        pool_principal: config.icpswap_pool,
        icp_ledger_principal: config.icp_ledger,
        ckusdc_ledger_principal: config.ckusdc_ledger,
        collateral_amount_e8s: VAULT158_CLAIM_GROSS_E8S,
        collateral_received_e8s: VAULT158_CLAIM_NET_E8S,
        debt_covered_e8s: VAULT158_DEBT_E8S,
        swap_input_amount_e8s: VAULT158_SWAP_INPUT_E8S,
        swap_input_fee_e8s: VAULT158_SWAP_INPUT_FEE_E8S,
        swap_output_amount_e6: VAULT158_SWAP_OUTPUT_E6,
        swap_input_receipt: swap::TransferReceipt {
            block_index: VAULT158_SWAP_INPUT_BLOCK,
            amount: VAULT158_SWAP_INPUT_E8S,
            created_at_time: VAULT158_SWAP_INPUT_TIME,
        },
        swap_output_receipt: swap::TransferReceipt {
            block_index: VAULT158_SWAP_OUTPUT_BLOCK,
            amount: VAULT158_SWAP_OUTPUT_E6,
            created_at_time: 0,
        },
        pre_swap_ckusdc_balance_e6: VAULT158_CKUSDC_BEFORE_SWAP_E6,
        held_surplus_e6: held_surplus,
        payment_gross_e6: gross,
        payment_amount_e6: minimum_net,
        payment_fee_e6: VAULT158_CKUSDC_FEE_E6,
        payment_created_at_time: None,
        payment_memo: claim.payment_memo,
        payment_receipt: None,
        status: state::Vault158RecoveryStatus::EvidencePrepared,
    };
    if !vault158_exact_evidence(&evidence) {
        return Err("prepared evidence did not satisfy the exact vault-158 recovery tuple".into());
    }
    state::mutate_state(|s| {
        if s.vault158_recovery.is_none() {
            s.vault158_recovery = Some(evidence);
        }
    });
    state::save_config_to_stable();
    Ok(())
}

/// Execute the explicitly prepared recovery. The first outgoing tuple is
/// created only here, after re-verifying both swap blocks and checking the
/// current ckUSDC fee and claim-attributable balance floor.
pub async fn admin_execute_vault158_recovery(config: &BotConfig) -> Result<(), String> {
    if !state::read_state(|s| s.processing_paused) {
        return Err("pause bot processing before explicitly executing vault-158 recovery".into());
    }
    let Some(mut evidence) = state::read_state(|s| s.vault158_recovery.clone()) else {
        return Err("vault-158 receipt recovery has not been prepared".into());
    };
    if !vault158_exact_evidence(&evidence) {
        return Err("durable vault-158 evidence does not match its pinned receipt tuple".into());
    }
    if evidence.status == state::Vault158RecoveryStatus::PaymentConfirmed {
        return continue_vault158_treasury(config, &evidence).await;
    }
    if evidence.status == state::Vault158RecoveryStatus::TreasurySettled {
        return continue_vault158_treasury(config, &evidence).await;
    }
    if evidence.status == state::Vault158RecoveryStatus::PaymentPrepared {
        let (journal, pending_count) = state::read_state(|s| (
            s.pending_payments.get(&VAULT158_ID).cloned(),
            s.pending_payments.len(),
        ));
        if pending_count != 1 {
            return Err("vault-158 execution is held because another payment journal exists".into());
        }
        let journal = journal
            .ok_or_else(|| "executed vault-158 tuple is missing its pending payment journal".to_string())?;
        if !vault158_payment_journal_matches_evidence(&journal, &evidence) {
            return Err("pending vault-158 payment journal differs from its retained exact tuple".into());
        }
        let _ = resume_pending_payment_for_vault(config, Some(VAULT158_ID)).await;
        let Some(updated) = state::read_state(|s| s.vault158_recovery.clone()) else {
            return Err("vault-158 recovery tombstone disappeared during payment reconciliation".into());
        };
        return if updated.status == state::Vault158RecoveryStatus::PaymentConfirmed {
            continue_vault158_treasury(config, &updated).await
        } else {
            Err("vault-158 exact payment tuple remains pending; inspect the durable payment journal".into())
        };
    }
    if evidence.status != state::Vault158RecoveryStatus::EvidencePrepared {
        return Err("vault-158 recovery evidence has an unsupported lifecycle status".into());
    }
    let claim = state::read_state(|s| s.pending_claims.get(&VAULT158_ID).cloned())
        .ok_or_else(|| "active vault-158 claim journal is absent".to_string())?;
    let record = match history::get_record(VAULT158_RECORD_ID) {
        Some(crate::history::LiquidationRecordVersioned::V1(record)) => record,
        None => return Err("history record 229 is absent".into()),
    };
    vault158_identity_matches(config, &claim, &record)?;
    if !state::read_state(|s| s.pending_payments.is_empty() && s.pending_treasury.is_empty()) {
        return Err("another payment or treasury journal exists; recovery will not allocate or continue a tuple".into());
    }
    swap::verify_vault158_swap_input_receipt(
        evidence.icp_ledger_principal,
        ic_cdk::id(),
        evidence.pool_principal,
        evidence.swap_input_receipt.block_index,
        evidence.swap_input_amount_e8s,
        evidence.swap_input_fee_e8s,
        VAULT158_POOL_MEMO_BYTES,
        evidence.swap_input_receipt.created_at_time,
    ).await?;
    verify_vault158_swap_output_receipt(config, ic_cdk::id()).await?;
    let fee = swap::fetch_ledger_fee(evidence.ckusdc_ledger_principal).await?;
    if fee != evidence.payment_fee_e6 || config.ckusdc_fee_e6.unwrap_or(VAULT158_CKUSDC_FEE_E6) != fee {
        return Err("current ckUSDC transfer fee does not match the pinned 10,000 e6 payment fee".into());
    }
    let balance = swap::balance_of_self_ckusdc(config).await
        .map_err(|error| format!("ckUSDC balance read failed: {error}"))?;
    if !vault158_balance_matches_exact_allocation(balance, &evidence) {
        return Err("current ckUSDC balance differs from the exact pre-swap balance plus verified output; aggregate balance cannot authorize recovery".into());
    }
    let current_claim = state::read_state(|s| s.pending_claims.get(&VAULT158_ID).cloned());
    let current_config = state::read_state(|s| s.config.clone());
    let current_record = match history::get_record(VAULT158_RECORD_ID) {
        Some(crate::history::LiquidationRecordVersioned::V1(record)) => record,
        None => return Err("history record 229 disappeared during receipt verification".into()),
    };
    let current_claim = current_claim.ok_or_else(|| "active vault-158 claim disappeared during receipt verification".to_string())?;
    let current_config = current_config.ok_or_else(|| "bot configuration disappeared during receipt verification".to_string())?;
    vault158_identity_matches(&current_config, &current_claim, &current_record)?;
    if current_claim.collateral_return.is_some()
        || !current_claim.failed_return_attempts.is_empty()
        || current_claim.payment_memo != evidence.payment_memo
        || !state::read_state(|s| s.processing_paused)
        || !state::read_state(|s| s.pending_payments.is_empty() && s.pending_treasury.is_empty())
    {
        return Err("claim or payment state changed while receipts were being reverified".into());
    }
    let created_at_time = ic_cdk::api::time();
    if created_at_time == 0 {
        return Err("current time is unavailable for a unique ckUSDC transfer tuple".into());
    }
    evidence.payment_created_at_time = Some(created_at_time);
    evidence.status = state::Vault158RecoveryStatus::PaymentPrepared;
    let journal = state::BotPaymentJournal {
        record_id: Some(VAULT158_RECORD_ID),
        vault_id: VAULT158_ID,
        backend_principal: evidence.backend_principal,
        ledger_principal: evidence.ckusdc_ledger_principal,
        claim_generation: VAULT158_GENERATION,
        debt_covered_e8s: evidence.debt_covered_e8s,
        collateral_amount_e8s: evidence.collateral_amount_e8s,
        collateral_received_amount_e8s: Some(evidence.collateral_received_e8s),
        collateral_price_e8s: claim.collateral_price_e8s,
        icp_swapped_e8s: evidence.swap_input_amount_e8s,
        swap_input_fee_e8s: evidence.swap_input_fee_e8s,
        ckusdc_received_e6: evidence.swap_output_amount_e6,
        held_surplus_e6: evidence.held_surplus_e6,
        gross_amount_e6: evidence.payment_gross_e6,
        amount_e6: evidence.payment_amount_e6,
        fee_e6: evidence.payment_fee_e6,
        created_at_time,
        memo: evidence.payment_memo.clone(),
        status: state::BotPaymentStatus::Prepared,
        receipt: None,
        shortfall_receipt_observed: false,
        shortfall_topup: None,
    };
    let committed = state::mutate_state(|s| {
        commit_vault158_payment_preparation_once(s, &evidence, journal)
    });
    if !committed {
        return Err("vault-158 recovery state changed while the verified receipts were being rechecked".into());
    }
    state::save_config_to_stable();
    let _ = resume_pending_payment_for_vault(config, Some(VAULT158_ID)).await;
    let Some(updated) = state::read_state(|s| s.vault158_recovery.clone()) else {
        return Err("vault-158 recovery tombstone disappeared during payment reconciliation".into());
    };
    if updated.status != state::Vault158RecoveryStatus::PaymentConfirmed {
        return Err("vault-158 payment or backend proof remains pending; inspect the durable payment journal".into());
    }
    continue_vault158_treasury(config, &updated).await
}

/// Continue only history record 229's already authorized treasury tuple. The
/// pause stays in force, so resolving this tuple cannot resume unrelated work.
async fn continue_vault158_treasury(
    config: &BotConfig,
    evidence: &state::Vault158RecoveryEvidence,
) -> Result<(), String> {
    if !state::read_state(|s| s.processing_paused) || !vault158_exact_evidence(evidence)
        || (evidence.status != state::Vault158RecoveryStatus::PaymentConfirmed
            && evidence.status != state::Vault158RecoveryStatus::TreasurySettled)
    {
        return Err("vault-158 treasury continuation requires the confirmed pinned recovery while processing remains paused".into());
    }
    if evidence.status == state::Vault158RecoveryStatus::TreasurySettled {
        return match history::get_record(VAULT158_RECORD_ID) {
            Some(crate::history::LiquidationRecordVersioned::V1(record))
                if record.vault_id == VAULT158_ID
                    && record.status == LiquidationStatus::Completed
                    && record.icp_to_treasury_e8s == 10_742_902 => Ok(()),
            _ => Err("vault-158 treasury tombstone does not match completed record 229".into()),
        };
    }
    let pending = state::read_state(|s| {
        s.pending_treasury.get(&VAULT158_RECORD_ID).cloned()
            .map(|journal| (journal, s.pending_treasury.len()))
    });
    let Some((journal, pending_count)) = pending else {
        return match history::get_record(VAULT158_RECORD_ID) {
            Some(crate::history::LiquidationRecordVersioned::V1(record))
                if record.vault_id == VAULT158_ID
                    && record.status == LiquidationStatus::Completed
                    && record.icp_to_treasury_e8s == 10_742_902 => {
                        state::mutate_state(|s| {
                            if let Some(evidence) = s.vault158_recovery.as_mut() {
                                if evidence.status == state::Vault158RecoveryStatus::PaymentConfirmed {
                                    evidence.status = state::Vault158RecoveryStatus::TreasurySettled;
                                }
                            }
                        });
                        state::save_config_to_stable();
                        Ok(())
                    }
            _ => Err("vault-158 payment is confirmed; pinned treasury record 229 remains pending reconciliation".into()),
        };
    };
    if pending_count != 1
        || journal.record_id != VAULT158_RECORD_ID
        || journal.vault_id != VAULT158_ID
        || journal.claim_generation != VAULT158_GENERATION
        || journal.sender_principal != ic_cdk::id()
        || journal.ledger_principal != config.icp_ledger
        || journal.treasury_principal != config.treasury_principal
        || journal.gross_amount_e8s != 10_742_902
        || journal.fee_e8s != config.icp_fee_e8s.unwrap_or(10_000)
        || journal.amount_e8s.checked_add(journal.fee_e8s) != Some(journal.gross_amount_e8s)
        || journal.memo != swap::icp_treasury_memo(VAULT158_RECORD_ID, VAULT158_GENERATION)
        || journal.created_at_time == 0
        || journal.record.id != VAULT158_RECORD_ID
        || journal.record.vault_id != VAULT158_ID
        || journal.record.collateral_claimed_e8s != VAULT158_CLAIM_GROSS_E8S
        || journal.record.debt_to_cover_e8s != VAULT158_DEBT_E8S
        || journal.record.icp_swapped_e8s != VAULT158_SWAP_INPUT_E8S
        || journal.record.ckusdc_transferred_e6 != evidence.payment_amount_e6
    {
        return Err("pinned treasury obligation differs from vault-158 history record 229".into());
    }
    let _ = resume_pending_treasury_for_record(config, Some(VAULT158_RECORD_ID)).await;
    if state::read_state(|s| s.pending_treasury.contains_key(&VAULT158_RECORD_ID)) {
        return Err("pinned treasury tuple remains pending; inspect record 229 and its durable journal".into());
    }
    match history::get_record(VAULT158_RECORD_ID) {
        Some(crate::history::LiquidationRecordVersioned::V1(record))
            if record.vault_id == VAULT158_ID
                && record.status == LiquidationStatus::Completed
                && record.icp_to_treasury_e8s == 10_742_902 => {
                    state::mutate_state(|s| {
                        if let Some(evidence) = s.vault158_recovery.as_mut() {
                            if evidence.status == state::Vault158RecoveryStatus::PaymentConfirmed {
                                evidence.status = state::Vault158RecoveryStatus::TreasurySettled;
                            }
                        }
                    });
                    state::save_config_to_stable();
                    Ok(())
                }
        _ => Err("treasury journal was consumed without a completed record-229 settlement".into()),
    }
}

async fn verify_vault158_swap_output_receipt(
    config: &BotConfig,
    bot: candid::Principal,
) -> Result<(), String> {
    let block = rumi_protocol_backend::icrc3_proof::fetch_icrc3_block(
        config.ckusdc_ledger, VAULT158_SWAP_OUTPUT_BLOCK,
    ).await.map_err(|error| format!("ckUSDC output block fetch failed: {error}"))?;
    validate_vault158_swap_output_block(&block, config.icpswap_pool, bot)
}

fn validate_vault158_swap_output_block(
    block: &rumi_protocol_backend::icrc3_proof::DecodedBlock,
    pool: candid::Principal,
    bot: candid::Principal,
) -> Result<(), String> {
    use icrc_ledger_types::icrc1::account::Account;
    rumi_protocol_backend::icrc3_proof::validate_icrc3_transfer_block(
        &block,
        Some(Account { owner: pool, subaccount: None }),
        Account { owner: bot, subaccount: None },
        VAULT158_SWAP_OUTPUT_E6,
        Some(VAULT158_POOL_MEMO_BYTES),
        None,
    ).map_err(|error| format!("ckUSDC swap-output block did not match the pinned transfer: {error}"))?;
    if block.fee != Some(VAULT158_CKUSDC_FEE_E6) {
        return Err("ckUSDC swap-output block does not show the pool-paid 10,000 e6 ledger fee".into());
    }
    Ok(())
}

/// Record an ambiguous DEX dispatch without changing the claim's
/// `SwapMayHaveStarted` status or attempting any financial cleanup. Amounts
/// in the history row are only the amounts observed before the ambiguous call.
fn write_ambiguous_swap_recovery(
    id: u64,
    vault_id: u64,
    timestamp: u64,
    collateral_amount: u64,
    debt_covered: u64,
    observed_icp_swapped: u64,
    observed_ckusdc_received: u64,
    collateral_price: u64,
    effective_price: u64,
    slippage_bps: i32,
    message: &str,
) {
    write_record(LiquidationRecordV1 {
        id,
        vault_id,
        timestamp,
        status: LiquidationStatus::ConfirmFailed,
        collateral_claimed_e8s: collateral_amount,
        debt_to_cover_e8s: debt_covered,
        icp_swapped_e8s: observed_icp_swapped,
        ckusdc_received_e6: observed_ckusdc_received,
        ckusdc_transferred_e6: 0,
        icp_to_treasury_e8s: 0,
        oracle_price_e8s: collateral_price,
        effective_price_e8s: effective_price,
        slippage_bps,
        error_message: Some(message.to_string()),
        confirm_retry_count: 0,
    });
}

/// Record a successful router response whose output could not be verified
/// against the bot's wallet. Keep the existing `SwapMayHaveStarted` claim
/// journal untouched so the claim generation remains held for reconciliation.
fn write_unverified_swap_recovery(
    id: u64,
    vault_id: u64,
    timestamp: u64,
    collateral_amount: u64,
    debt_covered: u64,
    swap_amount: u64,
    collateral_price: u64,
    effective_price: u64,
    slippage_bps: i32,
    router_received: u64,
    error: &str,
) {
    write_record(LiquidationRecordV1 {
        id,
        vault_id,
        timestamp,
        status: LiquidationStatus::ConfirmFailed,
        collateral_claimed_e8s: collateral_amount,
        debt_to_cover_e8s: debt_covered,
        icp_swapped_e8s: swap_amount,
        // No wallet balance was read, so the router's reported output is not
        // recorded as received ckUSDC.
        ckusdc_received_e6: 0,
        ckusdc_transferred_e6: 0,
        icp_to_treasury_e8s: 0,
        oracle_price_e8s: collateral_price,
        effective_price_e8s: effective_price,
        slippage_bps,
        error_message: Some(format!(
            "post-swap ckUSDC balance unavailable; router reported {router_received} e6 but receipt is unverified; claim generation remains held and no payment, collateral return, or cancellation was attempted: {error}"
        )),
        confirm_retry_count: 0,
    });
}

/// Authorize the existing exact collateral-return path after a failure that
/// happened before ICPSwap dispatch. The generation/status checks ensure this
/// transition cannot overwrite a later claim phase or an existing return.
fn arm_pre_swap_failure_return(claim: &mut state::BotClaimJournal, claim_generation: u64) -> bool {
    if claim.claim_generation != claim_generation
        || claim.status != state::BotClaimJournalStatus::SwapMayHaveStarted
        || claim.collateral_return.is_some()
    {
        return false;
    }
    claim.status = state::BotClaimJournalStatus::ReturnFeeQueryPending;
    true
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

/// Result of the post-swap wallet read. An unavailable balance has no
/// reservation, so it cannot flow into the Phase 3 payment path.
#[derive(Debug, PartialEq)]
enum PostSwapReservation {
    Measured(SwapReservation),
    Unavailable(String),
}

fn reserve_measured_post_swap_delta(
    vault_id: u64,
    router_received_e6: u64,
    balance_before_e6: u64,
    balance_after: Result<u64, String>,
) -> PostSwapReservation {
    match balance_after {
        Ok(balance_after_e6) => PostSwapReservation::Measured(compute_swap_reservation(
            vault_id,
            router_received_e6,
            balance_before_e6,
            balance_after_e6,
        )),
        Err(error) => PostSwapReservation::Unavailable(error),
    }
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
    if !pending_payment_matches(bot_state, journal) {
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
    if let Some(evidence) = bot_state.vault158_recovery.as_mut().filter(|evidence| {
        evidence.vault_id == journal.vault_id
            && evidence.claim_generation == journal.claim_generation
            && Some(evidence.record_id) == journal.record_id
            && evidence.payment_created_at_time == Some(journal.created_at_time)
            && evidence.payment_memo == journal.memo
    }) {
        evidence.payment_receipt = journal.receipt.clone();
        evidence.status = state::Vault158RecoveryStatus::PaymentConfirmed;
    }
    bot_state.pending_payments.remove(&journal.vault_id);
    true
}

fn record_vault158_payment_receipt(bot_state: &mut state::BotState, journal: &state::BotPaymentJournal) {
    if let Some(evidence) = bot_state.vault158_recovery.as_mut().filter(|evidence| {
        evidence.vault_id == journal.vault_id
            && evidence.claim_generation == journal.claim_generation
            && Some(evidence.record_id) == journal.record_id
            && evidence.payment_created_at_time == Some(journal.created_at_time)
            && evidence.payment_memo == journal.memo
    }) {
        evidence.payment_receipt = journal.receipt.clone();
    }
}

fn pending_payment_matches(
    bot_state: &state::BotState,
    journal: &state::BotPaymentJournal,
) -> bool {
    bot_state
        .pending_payments
        .get(&journal.vault_id)
        .is_some_and(|pending| {
            pending.claim_generation == journal.claim_generation
                && pending.ledger_principal == journal.ledger_principal
                && pending.created_at_time == journal.created_at_time
                && pending.memo == journal.memo
        })
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
    /// Net ICP received by the bot after the backend-paid outbound fee.
    /// Optional only for rolling Candid compatibility; absence is held fail-closed.
    pub collateral_received_amount: Option<u64>,
    /// Exact outbound ledger fee paid by the backend.
    /// Optional only for rolling Candid compatibility; absence is held fail-closed.
    pub collateral_outbound_fee: Option<u64>,
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

    let treasury_work_attempted = resume_pending_treasury(&config).await;
    if should_stop_after_treasury_attempt(treasury_work_attempted) {
        return;
    }
    if state::read_state(|s| s.processing_paused) {
        return;
    }
    let Some(refreshed_config) = state::read_state(|s| s.config.clone()) else { return };
    config = refreshed_config;
    if resume_pending_payment(&config).await {
        return;
    }
    if resume_pending_return(&config, None).await {
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
    let (
        collateral_amount,
        collateral_received_amount,
        collateral_outbound_fee,
        debt_covered,
        collateral_price,
        claim_generation,
        payment_memo,
    ) = match liq_result {
        Ok(r) => {
            state::mutate_state(|s| {
                s.claim_retry_counts.remove(&vault.vault_id);
                s.pending_claims.insert(vault.vault_id, state::BotClaimJournal {
                    vault_id: r.vault_id,
                    claim_generation: r.claim_generation,
                    debt_covered_e8s: r.debt_covered,
                    collateral_amount_e8s: r.collateral_amount,
                    collateral_received_amount_e8s: r.collateral_received_amount,
                    collateral_outbound_fee_e8s: r.collateral_outbound_fee,
                    collateral_price_e8s: r.collateral_price_e8s,
                    payment_memo: r.payment_memo.clone(),
                    collateral_return_memo: r.collateral_return_memo.clone(),
                    failed_return_attempts: Vec::new(),
                    collateral_return: None,
                    swap_intents: Vec::new(),
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
            (
                r.collateral_amount,
                r.collateral_received_amount,
                r.collateral_outbound_fee,
                r.debt_covered,
                r.collateral_price_e8s,
                r.claim_generation,
                r.payment_memo,
            )
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

    let collateral_received_amount = match validated_net_claim(
        collateral_amount,
        collateral_received_amount,
        collateral_outbound_fee,
    ) {
        Some(net) => net,
        None => {
            let error = "claim result lacks a valid gross/net/outbound-fee tuple; exact backend return accounting is required";
            log!(
                crate::INFO,
                "STUCK: {} for vault #{}; claim remains active and no swap will run",
                error,
                vault.vault_id
            );
            write_record(LiquidationRecordV1 {
                id: record_id,
                vault_id: vault.vault_id,
                timestamp,
                status: LiquidationStatus::TransferFailed,
                collateral_claimed_e8s: collateral_amount,
                debt_to_cover_e8s: debt_covered,
                icp_swapped_e8s: 0,
                ckusdc_received_e6: 0,
                ckusdc_transferred_e6: 0,
                icp_to_treasury_e8s: 0,
                oracle_price_e8s: collateral_price,
                effective_price_e8s: 0,
                slippage_bps: 0,
                error_message: Some(error.to_string()),
                confirm_retry_count: 0,
            });
            return;
        }
    };

    // -- Phase 2: SWAP ICP -> ckUSDC --
    // ICPSwap debits the input fee on top of the input amount, so size the
    // swap such that amount + fee stays within this claim's net collateral.
    let mut swap_amount = calculate_swap_amount(collateral_received_amount, debt_covered, collateral_price)
        .min(max_swap_input_for_claim(collateral_received_amount, swap::swap_input_fee_e8s(&config)));
    let mut initial_swap_fee_e8s = 0u64;

    // Per-claim reservation: bracket the swap with wallet balance reads so
    // we know the EXACT ckUSDC this claim earned, independent of any leftover
    // balance or what the swap router claims. The transfer in Phase 3 spends
    // only this delta (see compute_swap_reservation for rationale).
    //
    // Do not dispatch a swap unless its per-claim ckUSDC baseline is known.
    // Because this failure is before depositFromAndSwap, the exact collateral
    // return is safe and can be resumed through the normal proof-backed path.
    let bal_before_swap = match swap::balance_of_self_ckusdc(&config).await {
        Ok(balance) => balance,
        Err(error) => {
            let armed = state::mutate_state(|s| {
                s.pending_claims
                    .get_mut(&vault.vault_id)
                    .is_some_and(|claim| arm_pre_swap_failure_return(claim, claim_generation))
            });
            if armed {
                state::save_config_to_stable();
                log!(
                    crate::INFO,
                    "Pre-swap ckUSDC balance read failed for vault #{}: {}; no swap was dispatched, returning collateral",
                    vault.vault_id,
                    error
                );
                // Reuses the generation-bound return journal/proof/cancel
                // pipeline. If a query or fee check still fails, the claim
                // remains in ReturnFeeQueryPending for the next timer tick.
                resume_pending_return(&config, Some(vault.vault_id)).await;
            } else {
                log!(
                    crate::INFO,
                    "STUCK: pre-swap ckUSDC balance read failed for vault #{} and claim state could not be moved to safe return; no swap was dispatched",
                    vault.vault_id
                );
            }
            let message = if armed {
                format!(
                    "pre-swap ckUSDC balance read failed; no swap dispatched and claim moved to proof-backed collateral return: {error}"
                )
            } else {
                format!(
                    "pre-swap ckUSDC balance read failed; no swap dispatched, but claim state could not be moved to safe return: {error}"
                )
            };
            write_record(LiquidationRecordV1 {
                id: record_id,
                vault_id: vault.vault_id,
                timestamp,
                status: LiquidationStatus::TransferFailed,
                collateral_claimed_e8s: collateral_amount,
                debt_to_cover_e8s: debt_covered,
                icp_swapped_e8s: 0,
                ckusdc_received_e6: 0,
                ckusdc_transferred_e6: 0,
                icp_to_treasury_e8s: 0,
                oracle_price_e8s: collateral_price,
                effective_price_e8s: 0,
                slippage_bps: 0,
                error_message: Some(message),
                confirm_retry_count: 0,
            });
            return;
        }
    };

    let swap_result = match swap::prepare_icp_for_ckusdc(&config, swap_amount).await {
        Err(error) => Err(error),
        Ok(prepared) if !swap_fits_claim(collateral_received_amount, prepared.amount_in_e8s, prepared.input_fee_e8s) => {
            Err(swap::SwapAttemptError::NoEffect(format!(
                "swap input {} + fee {} e8s exceeds claim net collateral {}; not dispatched",
                prepared.amount_in_e8s, prepared.input_fee_e8s, collateral_received_amount
            )))
        }
        Ok(prepared) => match persist_swap_intent(
            vault.vault_id,
            claim_generation,
            &config,
            &prepared,
            bal_before_swap,
        ) {
            Ok(()) => {
                initial_swap_fee_e8s = prepared.input_fee_e8s;
                swap::dispatch_prepared_swap(&config, &prepared).await
            }
            Err(error) => Err(swap::SwapAttemptError::NoEffect(error)),
        },
    };

    if let Err(swap_err) = &swap_result {
        if !swap::swap_error_allows_return(swap_err) {
            let message = format!(
                "ICPSwap depositFromAndSwap outcome for requested {} ICP e8s is ambiguous: {}; no collateral return, claim cancellation, or ckUSDC payment was attempted. The claim remains held in SwapMayHaveStarted for operator reconciliation; swap output was not observed.",
                swap_amount, swap_err
            );
            log!(crate::INFO, "STUCK: {} for vault #{}", message, vault.vault_id);
            write_record(LiquidationRecordV1 {
                id: record_id,
                vault_id: vault.vault_id,
                timestamp,
                status: LiquidationStatus::ConfirmFailed,
                collateral_claimed_e8s: collateral_amount,
                debt_to_cover_e8s: debt_covered,
                icp_swapped_e8s: 0,
                ckusdc_received_e6: 0,
                ckusdc_transferred_e6: 0,
                icp_to_treasury_e8s: 0,
                oracle_price_e8s: collateral_price,
                effective_price_e8s: 0,
                slippage_bps: 0,
                error_message: Some(message),
                confirm_retry_count: 0,
            });
            return;
        }
    }

    let (router_received, effective_price) = match swap_result {
        Ok(r) => (r.ckusdc_received_e6, r.effective_price_e8s),
        Err(swap_err) => {
            let swap_err = swap_err.to_string();
            log!(crate::INFO, "Swap failed before deposit dispatch for vault #{}: {}. Returning ICP.", vault.vault_id, swap_err);

            state::mutate_state(|s| {
                if let Some(journal) = s.pending_claims.get_mut(&vault.vault_id) {
                    journal.status = state::BotClaimJournalStatus::ReturnFeeQueryPending;
                }
            });
            state::save_config_to_stable();

            // A new return gets a fresh ledger fee before its exact transfer
            // tuple is persisted. The helper saves that tuple before dispatch.
            let claim = state::read_state(|s| s.pending_claims.get(&vault.vault_id).cloned());
            let return_result = match claim {
                Some(claim) => create_and_send_return_intent(&config, &claim).await,
                None => Err(swap::TransferAttemptError::NoEffect(
                    "durable bot claim journal disappeared before ICP return".into(),
                )),
            };

            // Step 2: cancel the protocol-side claim, only if the return succeeded.
            // The backend requires an exact full-gross return proof before
            // cancellation. Do not attempt cancel after a failed or partial
            // return, because no debt or claim state may be cleared then.
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
                    match &e {
                        swap::TransferAttemptError::BadFee { expected_fee } => {
                            let refresh_scheduled = record_first_dispatch_bad_fee(vault.vault_id);
                            log!(crate::INFO, "ICP return for vault #{} received typed first-dispatch BadFee (expected {}). The no-effect tuple was retained; fee refresh scheduled: {}", vault.vault_id, expected_fee, refresh_scheduled == Some(true));
                        }
                        _ => {
                            state::mutate_state(|s| {
                                if let Some(intent) = s.pending_claims.get_mut(&vault.vault_id)
                                    .and_then(|claim| claim.collateral_return.as_mut()) {
                                    intent.status = match &e {
                                        swap::TransferAttemptError::NoEffect(_) => state::BotReturnTransferStatus::NoEffect,
                                        swap::TransferAttemptError::BadFee { .. } => unreachable!(),
                                        swap::TransferAttemptError::Ambiguous(_) => state::BotReturnTransferStatus::Ambiguous,
                                    };
                                }
                            });
                            state::save_config_to_stable();
                        }
                    }
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

    // Reaching here means the initial swap confirmed, so its fee was charged.
    let mut swap_input_fees_e8s = initial_swap_fee_e8s;
    let slippage_bps = calculate_slippage(effective_price, collateral_price);

    // The router's report is advisory. A failed wallet read cannot produce a
    // measured delta, so retain the generation-bound claim for reconciliation
    // and stop before any Phase 3 payment or post-swap cleanup.
    let mut reservation = match reserve_measured_post_swap_delta(
        vault.vault_id,
        router_received,
        bal_before_swap,
        swap::balance_of_self_ckusdc(&config).await,
    ) {
        PostSwapReservation::Measured(reservation) => reservation,
        PostSwapReservation::Unavailable(error) => {
            let message = format!(
                "post-swap ckUSDC balance read failed for vault #{}: {}",
                vault.vault_id, error
            );
            log!(crate::INFO, "STUCK: {}; no payment, collateral return, or cancellation attempted", message);
            write_unverified_swap_recovery(
                record_id,
                vault.vault_id,
                timestamp,
                collateral_amount,
                debt_covered,
                swap_amount,
                collateral_price,
                effective_price,
                slippage_bps,
                router_received,
                &error,
            );
            return;
        }
    };
    if let Some(note) = &reservation.discrepancy_note {
        log!(crate::INFO, "[per-claim-reservation] {}", note);
    }
    let ckusdc_fee = config.ckusdc_fee_e6.unwrap_or(10_000);
    let required_gross = required_ckusdc_gross(debt_covered, ckusdc_fee);
    if reservation.to_transfer_e6 < required_gross {
        let shortfall = required_gross - reservation.to_transfer_e6;
        let remaining_icp = claim_icp_remaining(collateral_received_amount, swap_amount, swap_input_fees_e8s);
        let max_topup_icp = max_swap_input_for_claim(remaining_icp, swap::swap_input_fee_e8s(&config));
        let mut topup_fee_e8s = 0u64;
        match choose_bounded_topup_amount(&config, max_topup_icp, shortfall).await {
            Ok(Some(topup_icp)) => {
                let before = match swap::balance_of_self_ckusdc(&config).await {
                    Ok(balance) => balance,
                    Err(error) => {
                        log!(crate::INFO, "STUCK: pre-top-up ckUSDC balance unavailable for vault #{}: {}", vault.vault_id, error);
                        write_short_payment_recovery(record_id, vault.vault_id, timestamp, collateral_amount, debt_covered, swap_amount, reservation.recorded_received_e6, collateral_price, effective_price, slippage_bps, "pre-top-up balance unavailable");
                        return;
                    }
                };
                let topup_result = match swap::prepare_icp_for_ckusdc(&config, topup_icp).await {
                    Err(error) => Err(error),
                    Ok(prepared) if !swap_fits_claim(remaining_icp, prepared.amount_in_e8s, prepared.input_fee_e8s) => {
                        Err(swap::SwapAttemptError::NoEffect(format!(
                            "top-up input {} + fee {} e8s exceeds claim remaining {}; not dispatched",
                            prepared.amount_in_e8s, prepared.input_fee_e8s, remaining_icp
                        )))
                    }
                    Ok(prepared) => match persist_swap_intent(
                        vault.vault_id,
                        claim_generation,
                        &config,
                        &prepared,
                        before,
                    ) {
                        Ok(()) => {
                            topup_fee_e8s = prepared.input_fee_e8s;
                            swap::dispatch_prepared_swap(&config, &prepared).await
                        }
                        Err(error) => Err(swap::SwapAttemptError::NoEffect(error)),
                    },
                };
                match topup_result {
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
                        swap_input_fees_e8s = swap_input_fees_e8s.saturating_add(topup_fee_e8s);
                    }
                    Err(swap::SwapAttemptError::Ambiguous(error)) => {
                        let message = format!(
                            "bounded top-up depositFromAndSwap outcome for requested {} ICP e8s is ambiguous: {}; no payment, collateral return, or cancellation was attempted. The claim remains held in SwapMayHaveStarted. History contains only the earlier confirmed swap output; top-up output was not observed.",
                            topup_icp, error
                        );
                        log!(crate::INFO, "STUCK: {} for vault #{}", message, vault.vault_id);
                        write_ambiguous_swap_recovery(
                            record_id,
                            vault.vault_id,
                            timestamp,
                            collateral_amount,
                            debt_covered,
                            swap_amount,
                            reservation.recorded_received_e6,
                            collateral_price,
                            effective_price,
                            slippage_bps,
                            &message,
                        );
                        return;
                    }
                    Err(swap::SwapAttemptError::NoEffect(error)) => {
                        log!(crate::INFO, "STUCK: bounded short-payment top-up failed before deposit dispatch for vault #{}: {}", vault.vault_id, error);
                        write_short_payment_recovery(record_id, vault.vault_id, timestamp, collateral_amount, debt_covered, swap_amount, reservation.recorded_received_e6, collateral_price, effective_price, slippage_bps, &format!("bounded top-up failed before deposit dispatch: {error}"));
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
        record_id: Some(record_id),
        vault_id: vault.vault_id,
        backend_principal: config.backend_principal,
        ledger_principal: config.ckusdc_ledger,
        claim_generation,
        debt_covered_e8s: debt_covered,
        collateral_amount_e8s: collateral_amount,
        collateral_received_amount_e8s: Some(collateral_received_amount),
        collateral_price_e8s: collateral_price,
        icp_swapped_e8s: swap_amount,
        swap_input_fee_e8s: swap_input_fees_e8s,
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
        shortfall_topup: None,
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
                swap::TransferAttemptError::BadFee { expected_fee } => (
                    state::BotPaymentStatus::NoEffect,
                    format!("ledger rejected transfer fee; expected {}", expected_fee),
                ),
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
            "exact ckUSDC receipt is short (received {}, required {}); claim and receipt held pending explicit admin allocation of the exact residual plus fee",
            ckusdc_transferred.amount, minimum
        );
        log!(crate::INFO, "STUCK: {} for vault #{}; original receipt is preserved and no second transfer will occur without admin authorization", message, vault.vault_id);
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
    let gross_bonus_e8s = claim_icp_residual_after_swap(
        collateral_received_amount, swap_amount, payment_journal.swap_input_fee_e8s,
    );
    let record = LiquidationRecordV1 {
        id: record_id, vault_id: vault.vault_id, timestamp,
        status: if gross_bonus_e8s == 0 { LiquidationStatus::Completed } else { LiquidationStatus::TransferFailed },
        collateral_claimed_e8s: collateral_amount, debt_to_cover_e8s: debt_covered,
        icp_swapped_e8s: swap_amount, ckusdc_received_e6: ckusdc_received,
        ckusdc_transferred_e6: ckusdc_transferred.amount,
        // A held record carries an unpaid obligation; only verified settlement
        // moves it to Completed and the paid-ICP statistic.
        icp_to_treasury_e8s: 0,
        oracle_price_e8s: collateral_price, effective_price_e8s: effective_price,
        slippage_bps, error_message: None, confirm_retry_count: confirm_retries,
    };
    if gross_bonus_e8s == 0 {
        write_record(record);
    } else {
        let mut treasury = swap::prepare_icp_treasury_transfer(
            &config, record_id, vault.vault_id, claim_generation, gross_bonus_e8s,
            ic_cdk::api::time(),
        ).expect("treasury obligation construction is infallible");
        treasury.record = record.clone();
        if treasury.status == state::BotTreasuryStatus::NeedsPreparation {
            treasury.record.error_message = Some(format!("ICP treasury bonus obligation pending preparation: {} e8s gross", gross_bonus_e8s));
        }
        let initial_record = treasury.record.clone();
        state::mutate_state(|s| { s.pending_treasury.insert(record_id, treasury); });
        write_record(initial_record);
        state::mutate_state(|s| { apply_confirmed_payment_totals_once(s, &payment_journal, ckusdc_transferred.amount); });
        state::save_config_to_stable();
        let _ = resume_pending_treasury(&config).await;
        return;
    }

    log!(crate::INFO, "Vault #{} liquidated: debt={} e8s, ckUSDC={} e6, treasury=0 e8s ICP",
        vault.vault_id, debt_covered, ckusdc_received);
    state::mutate_state(|s| { apply_confirmed_payment_totals_once(s, &payment_journal, ckusdc_transferred.amount); });
    state::save_config_to_stable();
}

/// Treasury delivery is a best-effort follow-up after ckUSDC settlement. The
/// current claim's swaps are bounded by its measured collateral, so a held
/// bonus must not block unrelated claim/payment progress.
fn should_stop_after_treasury_attempt(_attempted: bool) -> bool {
    false
}

// -- Helpers --

fn write_record(record: LiquidationRecordV1) {
    history::insert_record(LiquidationRecordVersioned::V1(record));
}

/// Commit an already ledger-verified receipt and its paid metric in one
/// BotState mutation. A stale generation or mismatched receipt cannot settle.
fn apply_verified_treasury_receipt_once(
    bot_state: &mut state::BotState,
    journal: &state::BotTreasuryJournal,
) -> bool {
    let Some(receipt) = journal.receipt.as_ref() else { return false };
    if receipt.amount != journal.amount_e8s
        || receipt.created_at_time != journal.created_at_time
        || journal.amount_e8s.checked_add(journal.fee_e8s) != Some(journal.gross_amount_e8s)
    { return false; }
    let Some(current) = bot_state.pending_treasury.get_mut(&journal.record_id) else { return false };
    if !treasury_intent_matches(current, journal)
        || current.status != state::BotTreasuryStatus::ReceiptObserved
        || current.receipt.as_ref() != Some(receipt)
    { return false; }
    current.status = state::BotTreasuryStatus::Paid;
    if !current.paid_total_applied {
        bot_state.stats.total_collateral_to_treasury_e8s = bot_state.stats.total_collateral_to_treasury_e8s
            .saturating_add(current.gross_amount_e8s);
        current.paid_total_applied = true;
    }
    true
}

fn treasury_intent_matches(
    current: &state::BotTreasuryJournal,
    candidate: &state::BotTreasuryJournal,
) -> bool {
    current.record_id == candidate.record_id
        && current.vault_id == candidate.vault_id
        && current.claim_generation == candidate.claim_generation
        && current.ledger_principal == candidate.ledger_principal
        && current.sender_principal == candidate.sender_principal
        && current.treasury_principal == candidate.treasury_principal
        && current.gross_amount_e8s == candidate.gross_amount_e8s
        && current.amount_e8s == candidate.amount_e8s
        && current.fee_e8s == candidate.fee_e8s
        && current.memo == candidate.memo
        && current.created_at_time == candidate.created_at_time
}

/// Commit a candidate that was already verified against the saved tuple by the
/// configured ledger's direct/archive parser. This function performs no ledger
/// calls and never initiates a transfer.
fn apply_verified_treasury_candidate_once(
    bot_state: &mut state::BotState,
    journal: &state::BotTreasuryJournal,
    receipt: &swap::TransferReceipt,
    expected_sender: candid::Principal,
) -> bool {
    if journal.sender_principal != expected_sender
        || journal.amount_e8s == 0
        || journal.amount_e8s.checked_add(journal.fee_e8s) != Some(journal.gross_amount_e8s)
        || receipt.amount != journal.amount_e8s
        || receipt.created_at_time != journal.created_at_time
    {
        return false;
    }
    let mut observed = journal.clone();
    observed.status = state::BotTreasuryStatus::ReceiptObserved;
    observed.receipt = Some(receipt.clone());
    let Some(current) = bot_state.pending_treasury.get_mut(&journal.record_id) else {
        return false;
    };
    if !treasury_intent_matches(current, journal)
        || current.status == state::BotTreasuryStatus::Paid
    {
        return false;
    }
    current.status = state::BotTreasuryStatus::ReceiptObserved;
    current.receipt = Some(receipt.clone());
    apply_verified_treasury_receipt_once(bot_state, &observed)
}

/// Reconcile an ambiguous treasury delivery from an exact saved ICP ledger
/// block. The retry-window cutoff applies only to redispatch; a verified block
/// remains admissible for an older unresolved intent.
pub async fn admin_reconcile_treasury_block(
    record_id: u64,
    block_index: u64,
) -> Result<(), String> {
    let journal = state::read_state(|s| s.pending_treasury.get(&record_id).cloned())
        .ok_or_else(|| "no pending treasury intent for this record ID".to_string())?;
    if journal.status == state::BotTreasuryStatus::Paid {
        return Err("treasury intent is already settled".into());
    }
    let receipt = swap::TransferReceipt {
        block_index,
        amount: journal.amount_e8s,
        created_at_time: journal.created_at_time,
    };
    swap::verify_icp_treasury_receipt(&journal, &receipt)
        .await
        .map_err(|error| format!(
            "candidate block did not prove the exact saved treasury transfer; intent remains pending: {error}"
        ))?;

    let committed = state::mutate_state(|s| {
        apply_verified_treasury_candidate_once(s, &journal, &receipt, ic_cdk::id())
    });
    if !committed {
        return Err("treasury intent changed or was already settled while its candidate block was verified".into());
    }
    state::save_config_to_stable();

    let mut record = journal.record.clone();
    record.status = LiquidationStatus::Completed;
    record.icp_to_treasury_e8s = journal.gross_amount_e8s;
    record.error_message = None;
    write_record(record);
    state::mutate_state(|s| {
        if s.pending_treasury.get(&record_id).is_some_and(|current| {
            treasury_intent_matches(current, &journal)
                && current.status == state::BotTreasuryStatus::Paid
        }) {
            s.pending_treasury.remove(&record_id);
        }
    });
    state::save_config_to_stable();
    Ok(())
}

/// Resume one claim-bound bonus transfer. Ambiguous outcomes replay the exact
/// tuple; metrics move only after the ledger block matches that tuple.
async fn resume_pending_treasury(config: &BotConfig) -> bool {
    resume_pending_treasury_for_record(config, None).await
}

async fn resume_pending_treasury_for_record(
    config: &BotConfig,
    only_record_id: Option<u64>,
) -> bool {
    const SAFE_RETRY_WINDOW_NS: u64 = 23 * 60 * 60 * 1_000_000_000;
    let now = ic_cdk::api::time();
    let selected = state::read_state(|s| {
        let eligible = |j: &state::BotTreasuryJournal| {
            if j.status == state::BotTreasuryStatus::Paid { return true; }
            if j.status == state::BotTreasuryStatus::ReceiptObserved { return true; }
            if config.icp_ledger != j.ledger_principal || config.treasury_principal != j.treasury_principal { return false; }
            match j.status {
                state::BotTreasuryStatus::ReceiptObserved => true,
                state::BotTreasuryStatus::NeedsPreparation => swap::icp_treasury_transfer_amount(
                    j.gross_amount_e8s, config.icp_fee_e8s.unwrap_or(10_000),
                ).is_ok(),
                state::BotTreasuryStatus::Prepared | state::BotTreasuryStatus::Ambiguous | state::BotTreasuryStatus::NoEffect =>
                    now.saturating_sub(j.created_at_time) < SAFE_RETRY_WINDOW_NS,
                state::BotTreasuryStatus::Paid => false,
            }
        };
        let cursor = s.treasury_resume_cursor;
        s.pending_treasury.iter()
            .filter(|(id, _)| only_record_id.map_or(true, |expected| **id == expected))
            .find(|(id, j)| cursor.is_some_and(|cursor| **id > cursor) && eligible(j))
            .or_else(|| s.pending_treasury.iter()
                .filter(|(id, _)| only_record_id.map_or(true, |expected| **id == expected))
                .find(|(_, j)| eligible(j)))
            .map(|(id, journal)| (*id, journal.clone()))
    });
    let Some((selected_id, mut journal)) = selected else { return false };
    state::mutate_state(|s| { s.treasury_resume_cursor = Some(selected_id); });
    state::save_config_to_stable();
    if !matches!(journal.status, state::BotTreasuryStatus::Paid | state::BotTreasuryStatus::ReceiptObserved)
        && (config.icp_ledger != journal.ledger_principal
        || config.treasury_principal != journal.treasury_principal
        )
    {
        log!(crate::INFO, "STUCK: ICP treasury intent for vault #{} is bound to different ledger/recipient configuration", journal.vault_id);
        return true;
    }
    if journal.status == state::BotTreasuryStatus::Paid {
        let mut record = journal.record.clone();
        record.status = LiquidationStatus::Completed;
        record.icp_to_treasury_e8s = journal.gross_amount_e8s;
        record.error_message = None;
        write_record(record);
        state::mutate_state(|s| {
            if s.pending_treasury.get(&journal.record_id).is_some_and(|current| current.claim_generation == journal.claim_generation && current.status == state::BotTreasuryStatus::Paid) {
                s.pending_treasury.remove(&journal.record_id);
            }
        });
        state::save_config_to_stable();
        return true;
    }
    if journal.status == state::BotTreasuryStatus::NeedsPreparation {
        let mut refreshed = swap::prepare_icp_treasury_transfer(
            config, journal.record_id, journal.vault_id, journal.claim_generation,
            journal.gross_amount_e8s, journal.created_at_time,
        ).expect("treasury obligation construction is infallible");
        refreshed.record = journal.record.clone();
        journal = refreshed;
        state::mutate_state(|s| { s.pending_treasury.insert(journal.record_id, journal.clone()); });
        state::save_config_to_stable();
        if journal.status == state::BotTreasuryStatus::NeedsPreparation {
            return true;
        }
    }
    if journal.status != state::BotTreasuryStatus::ReceiptObserved {
        if ic_cdk::api::time().saturating_sub(journal.created_at_time) >= SAFE_RETRY_WINDOW_NS {
            log!(crate::INFO, "STUCK: ICP treasury intent for claim generation {} exceeded safe dedup window; no new tuple was created", journal.claim_generation);
            return true;
        }
        match swap::transfer_icp_to_treasury(&journal).await {
            Ok(receipt) => {
                journal.status = state::BotTreasuryStatus::ReceiptObserved;
                journal.receipt = Some(receipt);
            }
            Err(swap::TransferAttemptError::NoEffect(error)) => {
                journal.status = state::BotTreasuryStatus::NoEffect;
                log!(crate::INFO, "ICP treasury transfer had a typed no-effect result for claim {}: {}", journal.claim_generation, error);
            }
            Err(swap::TransferAttemptError::BadFee { expected_fee }) => {
                journal.status = if journal.status == state::BotTreasuryStatus::Ambiguous {
                    state::BotTreasuryStatus::Ambiguous
                } else {
                    state::BotTreasuryStatus::NeedsPreparation
                };
                log!(crate::INFO, "ICP treasury transfer returned BadFee ({expected_fee}) for claim {}", journal.claim_generation);
            }
            Err(swap::TransferAttemptError::Ambiguous(error)) => {
                journal.status = state::BotTreasuryStatus::Ambiguous;
                log!(crate::INFO, "ICP treasury transfer outcome is ambiguous for claim {}: {}", journal.claim_generation, error);
            }
        }
        state::mutate_state(|s| { s.pending_treasury.insert(journal.record_id, journal.clone()); });
        state::save_config_to_stable();
    }
    let Some(receipt) = journal.receipt.as_ref() else {
        let mut record = journal.record.clone();
        record.status = LiquidationStatus::TransferFailed;
        record.error_message = Some(match journal.status {
            state::BotTreasuryStatus::NoEffect => "ICP treasury transfer had a definitive no-effect response; retry remains bound to its exact tuple".into(),
            _ => "ICP treasury transfer outcome is unresolved; retry remains bound to its exact tuple".into(),
        });
        write_record(record);
        return true;
    };
    if let Err(error) = swap::verify_icp_treasury_receipt(&journal, receipt).await {
        journal.record.status = LiquidationStatus::TransferFailed;
        journal.record.icp_to_treasury_e8s = 0;
        journal.record.error_message = Some(format!("ICP treasury receipt block {} is not yet proven: {}", receipt.block_index, error));
        state::mutate_state(|s| { s.pending_treasury.insert(journal.record_id, journal.clone()); });
        state::save_config_to_stable();
        write_record(journal.record.clone());
        log!(crate::INFO, "STUCK: ICP treasury receipt block {} could not be verified for claim {}: {}", receipt.block_index, journal.claim_generation, error);
        return true;
    }
    let paid_now = state::mutate_state(|s| apply_verified_treasury_receipt_once(s, &journal));
    state::save_config_to_stable();
    if paid_now {
        let mut record = journal.record.clone();
        record.status = LiquidationStatus::Completed;
        record.icp_to_treasury_e8s = journal.gross_amount_e8s;
        record.error_message = None;
        write_record(record);
        state::mutate_state(|s| {
            if s.pending_treasury.get(&journal.record_id).is_some_and(|current| current.claim_generation == journal.claim_generation && current.status == state::BotTreasuryStatus::Paid) {
                s.pending_treasury.remove(&journal.record_id);
            }
        });
        state::save_config_to_stable();
    }
    true
}

/// Reconcile a payment intent before processing another vault. The intent is
/// written to stable memory before the ledger call; retries reuse its exact
/// amount, memo and created_at_time. A returned block is never paid again and
/// is handed to the backend's exact ICRC-3 verifier.
async fn resume_pending_payment(config: &BotConfig) -> bool {
    resume_pending_payment_for_vault(config, None).await
}

/// Resume one already persisted payment, optionally restricting selection to
/// the pinned recovery vault instead of the general pending queue.
async fn resume_pending_payment_for_vault(
    config: &BotConfig,
    only_vault_id: Option<u64>,
) -> bool {
    let Some(mut journal) = state::read_state(|s| select_pending_payment(s, only_vault_id)) else {
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
        return resume_shortfall_payment(config, journal).await;
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
                state::mutate_state(|s| record_vault158_payment_receipt(s, &journal));
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
            Err(swap::TransferAttemptError::BadFee { expected_fee }) => {
                journal.status = state::BotPaymentStatus::Ambiguous;
                log!(crate::INFO, "STUCK: replayed ckUSDC payment for vault #{} returned BadFee (expected {}); prior submission may have committed, so exact ledger history reconciliation is required", journal.vault_id, expected_fee);
                state::mutate_state(|s| { s.pending_payments.insert(journal.vault_id, journal.clone()); });
                state::save_config_to_stable();
                return true;
            }
        }
        state::mutate_state(|s| { s.pending_payments.insert(journal.vault_id, journal.clone()); });
        state::mutate_state(|s| record_vault158_payment_receipt(s, &journal));
        state::save_config_to_stable();
    }

    let Some(receipt) = journal.receipt.as_ref() else { return true };
    if payment_receipt_is_short(&journal, receipt) {
        journal.shortfall_receipt_observed = true;
        state::mutate_state(|s| { s.pending_payments.insert(journal.vault_id, journal.clone()); });
        state::save_config_to_stable();
        log!(crate::INFO, "STUCK: exact ckUSDC block {} for vault #{} is short ({} received, {} required); original receipt remains held pending explicit admin allocation of the exact residual plus fee", receipt.block_index, journal.vault_id, receipt.amount, required_ckusdc_net(journal.debt_covered_e8s));
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
            // Legacy payment journals predate the history link. Allocate and
            // persist one before reconstructing any unpaid treasury phase.
            if journal.record_id.is_none() {
                journal.record_id = Some(history::next_id());
                state::mutate_state(|s| { s.pending_payments.insert(journal.vault_id, journal.clone()); });
                state::save_config_to_stable();
            }
            // If an upgrade happened after backend confirmation but before
            // Phase 5, reconstruct and persist the exact treasury intent from
            // this payment journal before consuming it.
            if let (Some(record_id), Some(collateral_received)) =
                (journal.record_id, journal.collateral_received_amount_e8s)
            {
                let gross_bonus = claim_icp_residual_after_swap(
                    collateral_received, journal.icp_swapped_e8s, journal.swap_input_fee_e8s,
                );
                let mut record = LiquidationRecordV1 {
                    id: record_id,
                    vault_id: journal.vault_id,
                    timestamp: journal.created_at_time,
                    status: if gross_bonus == 0 { LiquidationStatus::Completed } else { LiquidationStatus::TransferFailed },
                    collateral_claimed_e8s: journal.collateral_amount_e8s,
                    debt_to_cover_e8s: journal.debt_covered_e8s,
                    icp_swapped_e8s: journal.icp_swapped_e8s,
                    ckusdc_received_e6: journal.ckusdc_received_e6,
                    ckusdc_transferred_e6: receipt.amount,
                    icp_to_treasury_e8s: 0,
                    oracle_price_e8s: journal.collateral_price_e8s,
                    effective_price_e8s: journal.collateral_price_e8s,
                    slippage_bps: 0,
                    error_message: if gross_bonus == 0 { None } else { Some("recovered ckUSDC confirmation; treasury bonus pending".into()) },
                    confirm_retry_count: 0,
                };
                if gross_bonus > 0 {
                    if let Some(existing) = state::read_state(|s| s.pending_treasury.get(&record_id).cloned()) {
                        record = existing.record;
                    } else {
                        let mut treasury = swap::prepare_icp_treasury_transfer(
                            config, record_id, journal.vault_id, journal.claim_generation,
                            gross_bonus, ic_cdk::api::time(),
                        ).expect("treasury obligation construction is infallible");
                        treasury.record = record.clone();
                        state::mutate_state(|s| { s.pending_treasury.insert(record_id, treasury); });
                    }
                }
                write_record(record);
            }
            state::mutate_state(|s| {
                record_vault158_payment_receipt(s, &journal);
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

/// Resume only an administrator-authorized residual ckUSDC transfer. The
/// original short receipt is immutable; the persisted top-up tuple is replayed
/// exactly after an ambiguous reply, then both blocks are submitted together.
async fn resume_shortfall_payment(
    config: &BotConfig,
    mut journal: state::BotPaymentJournal,
) -> bool {
    let Ok((original_amount, residual)) = shortfall_residual(&journal) else {
        log!(crate::INFO, "STUCK: short payment journal for vault #{} lacks a valid original receipt", journal.vault_id);
        return true;
    };
    let Some(mut topup) = journal.shortfall_topup.clone() else {
        log!(crate::INFO, "STUCK: short payment for vault #{} needs explicit admin funding allocation of exactly {} ckUSDC e6 plus the ledger fee", journal.vault_id, residual);
        return true;
    };
    if config.backend_principal != journal.backend_principal
        || config.ckusdc_ledger != journal.ledger_principal
        || topup.backend_principal != journal.backend_principal
        || topup.ledger_principal != journal.ledger_principal
        || topup.memo != journal.memo
        || topup.amount_e6 != residual
        || topup.funding_allocation_e6
            != topup.amount_e6.saturating_add(topup.fee_e6)
    {
        log!(crate::INFO, "STUCK: residual payment tuple for vault #{} does not match the persisted claim allocation", journal.vault_id);
        return true;
    }

    if topup.receipt.is_none() {
        // Re-verify the original receipt on every dispatch attempt. This also
        // protects journals persisted by older code where an operator-supplied
        // block index may have been stored without proof.
        let Some(original_receipt) = journal.receipt.as_ref() else {
            return true;
        };
        let original_receipt_verification = call_bot_verify_liquidation_payment_proof(
            config,
            BotPaymentProof {
                vault_id: journal.vault_id,
                claim_generation: journal.claim_generation,
                ledger_principal: journal.ledger_principal,
                block_index: original_receipt.block_index,
                amount_e6s: original_receipt.amount,
                created_at_time: original_receipt.created_at_time,
            },
        )
        .await;
        if topup.status == state::BotPaymentStatus::NoEffect {
            log!(crate::INFO, "STUCK: residual payment for vault #{} had a definitive no-effect response; explicit repair is required", journal.vault_id);
            return true;
        }
        const SAFE_RETRY_WINDOW_NS: u64 = 23 * 60 * 60 * 1_000_000_000;
        if ic_cdk::api::time().saturating_sub(topup.created_at_time) >= SAFE_RETRY_WINDOW_NS {
            log!(crate::INFO, "STUCK: residual payment tuple for vault #{} exceeded the safe dedup window; exact ICRC-3 history reconciliation is required", journal.vault_id);
            return true;
        }
        let transfer = match dispatch_only_after_original_receipt_verification(
            original_receipt_verification,
            || {
                swap::transfer_ckusdc_to_backend(
                    config,
                    topup.amount_e6,
                    topup.memo.clone(),
                    topup.created_at_time,
                    topup.fee_e6,
                )
            },
        ) {
            Ok(transfer) => transfer,
            Err(error) => {
                log!(crate::INFO, "STUCK: original short-payment block for vault #{} is not independently verified; residual transfer remains undispatched: {}", journal.vault_id, error);
                return true;
            }
        };
        match transfer.await {
            Ok(receipt) => {
                topup.status = state::BotPaymentStatus::ReceiptObserved;
                topup.receipt = Some(receipt);
            }
            Err(swap::TransferAttemptError::Ambiguous(error)) => {
                topup.status = state::BotPaymentStatus::Ambiguous;
                journal.shortfall_topup = Some(topup);
                state::mutate_state(|s| { s.pending_payments.insert(journal.vault_id, journal.clone()); });
                state::save_config_to_stable();
                log!(crate::INFO, "Residual payment outcome remains ambiguous for vault #{}: {}", journal.vault_id, error);
                return true;
            }
            Err(swap::TransferAttemptError::NoEffect(error)) => {
                topup.status = state::BotPaymentStatus::NoEffect;
                journal.shortfall_topup = Some(topup);
                state::mutate_state(|s| { s.pending_payments.insert(journal.vault_id, journal.clone()); });
                state::save_config_to_stable();
                log!(crate::INFO, "Residual payment had no effect for vault #{}: {}", journal.vault_id, error);
                return true;
            }
            Err(swap::TransferAttemptError::BadFee { expected_fee }) => {
                // This tuple was rejected without effect. Keep its exact fee
                // identity and require operator reconciliation before a new
                // tuple is authorized.
                topup.status = state::BotPaymentStatus::NoEffect;
                journal.shortfall_topup = Some(topup);
                state::mutate_state(|s| { s.pending_payments.insert(journal.vault_id, journal.clone()); });
                state::save_config_to_stable();
                log!(crate::INFO, "Residual payment fee was rejected for vault #{} (expected {}); tuple remains held", journal.vault_id, expected_fee);
                return true;
            }
        }
        journal.shortfall_topup = Some(topup.clone());
        state::mutate_state(|s| { s.pending_payments.insert(journal.vault_id, journal.clone()); });
        state::save_config_to_stable();
    }

    let Some(original_receipt) = journal.receipt.as_ref() else { return true };
    let Some(topup_receipt) = topup.receipt.as_ref() else { return true };
    let Some(total) = original_amount.checked_add(topup_receipt.amount) else {
        log!(crate::INFO, "STUCK: cumulative payment amount overflow for vault #{}", journal.vault_id);
        return true;
    };
    if original_receipt.block_index == topup_receipt.block_index
        || topup_receipt.amount != residual
        || topup_receipt.created_at_time != topup.created_at_time
        || total != required_ckusdc_net(journal.debt_covered_e8s)
    {
        log!(crate::INFO, "STUCK: original plus residual receipts do not form the exact claim payment for vault #{}", journal.vault_id);
        return true;
    }
    let proofs = vec![
        BotPaymentProof {
            vault_id: journal.vault_id,
            claim_generation: journal.claim_generation,
            ledger_principal: journal.ledger_principal,
            block_index: original_receipt.block_index,
            amount_e6s: original_receipt.amount,
            created_at_time: original_receipt.created_at_time,
        },
        BotPaymentProof {
            vault_id: journal.vault_id,
            claim_generation: journal.claim_generation,
            ledger_principal: journal.ledger_principal,
            block_index: topup_receipt.block_index,
            amount_e6s: topup_receipt.amount,
            created_at_time: topup_receipt.created_at_time,
        },
    ];
    match call_bot_confirm_liquidation_with_proofs(config, proofs).await {
        Ok(()) => {
            let applied = state::mutate_state(|s| {
                apply_confirmed_payment_totals_once(s, &journal, total)
            });
            if applied {
                state::save_config_to_stable();
                log!(crate::INFO, "Recovered exact cumulative payment for vault #{} from blocks {} and {}; unswapped ICP remains held for reconciliation", journal.vault_id, original_receipt.block_index, topup_receipt.block_index);
            } else {
                log!(crate::INFO, "Cumulative payment proof was accepted for vault #{}, but its local journal identity changed; operator reconciliation is required", journal.vault_id);
            }
        }
        Err(error) => log!(crate::INFO, "Cumulative payment proof remains pending for vault #{}: {}", journal.vault_id, error),
    }
    true
}

/// Query the fee and persist a new claim-bound return tuple before its first
/// dispatch. This is only valid before any return intent exists.
async fn create_and_send_return_intent(
    config: &BotConfig,
    claim: &state::BotClaimJournal,
) -> Result<swap::TransferReceipt, swap::TransferAttemptError> {
    if claim.status != state::BotClaimJournalStatus::ReturnFeeQueryPending
        || claim.collateral_return.is_some()
        || claim.failed_return_attempts.len() > MAX_AUTO_RETURN_FEE_REFRESHES
    {
        return Err(swap::TransferAttemptError::NoEffect(
            "new ICP return is not in the fee-query phase".into(),
        ));
    }

    let (Some(net_received), Some(outbound_fee)) = (
        claim.collateral_received_amount_e8s,
        claim.collateral_outbound_fee_e8s,
    ) else {
        return Err(swap::TransferAttemptError::NoEffect(
            "legacy claim journal lacks exact net collateral and outbound fee; return is held".into(),
        ));
    };
    if net_received == 0
        || net_received.checked_add(outbound_fee) != Some(claim.collateral_amount_e8s)
    {
        return Err(swap::TransferAttemptError::NoEffect(
            "claim journal gross collateral does not equal net receipt plus exact outbound fee"
                .into(),
        ));
    }

    let fee_e8s = swap::fetch_ledger_fee(config.icp_ledger)
        .await
        .map_err(|error| {
            swap::TransferAttemptError::NoEffect(format!(
                "failed to query current ICP ledger fee: {error}"
            ))
        })?;
    let amount_e8s = claim.collateral_amount_e8s;
    let available_balance_e8s = swap::balance_of_self_icp(config)
        .await
        .map_err(swap::TransferAttemptError::NoEffect)?;
    if !return_balance_covers(amount_e8s, fee_e8s, available_balance_e8s) {
        let required_balance_e8s = required_return_balance(amount_e8s, fee_e8s)
            .map(|required| required.to_string())
            .unwrap_or_else(|| "an amount above u64::MAX".into());
        return Err(swap::TransferAttemptError::NoEffect(format!(
            "ICP fee float is insufficient for gross return: have {}, need {}",
            available_balance_e8s, required_balance_e8s
        )));
    }
    let created_at_time = next_return_created_at_time(
        ic_cdk::api::time(),
        &claim.failed_return_attempts,
    )
    .ok_or_else(|| {
        swap::TransferAttemptError::NoEffect(
            "cannot create a unique ICP return timestamp after the prior BadFee attempt".into(),
        )
    })?;
    let intent = state::BotReturnTransferJournal {
        ledger_principal: config.icp_ledger,
        backend_principal: config.backend_principal,
        amount_e8s,
        fee_e8s,
        transfer_fee_e8s: Some(fee_e8s),
        memo: claim.collateral_return_memo.clone(),
        created_at_time,
        receipt: None,
        status: state::BotReturnTransferStatus::Prepared,
    };
    let persisted = state::mutate_state(|s| {
        let Some(active) = s.pending_claims.get_mut(&claim.vault_id) else {
            return false;
        };
        if active.claim_generation != claim.claim_generation
            || active.status != state::BotClaimJournalStatus::ReturnFeeQueryPending
            || active.collateral_return.is_some()
        {
            return false;
        }
        active.collateral_return = Some(intent.clone());
        active.status = state::BotClaimJournalStatus::ReturnPending;
        true
    });
    if !persisted {
        return Err(swap::TransferAttemptError::NoEffect(
            "claim changed before ICP return tuple could be persisted".into(),
        ));
    }
    state::save_config_to_stable();

    swap::return_collateral_to_backend(
        config,
        intent.ledger_principal,
        intent.memo.clone(),
        intent.amount_e8s,
        intent.fee_e8s,
        intent.transfer_fee_e8s,
        intent.created_at_time,
    )
    .await
}

fn validated_net_claim(gross: u64, net: Option<u64>, outbound_fee: Option<u64>) -> Option<u64> {
    let (Some(net), Some(outbound_fee)) = (net, outbound_fee) else {
        return None;
    };
    (net > 0 && net.checked_add(outbound_fee) == Some(gross)).then_some(net)
}

fn required_return_balance(gross: u64, return_fee: u64) -> Option<u64> {
    gross.checked_add(return_fee)
}

fn return_balance_covers(gross: u64, return_fee: u64, available: u64) -> bool {
    required_return_balance(gross, return_fee).is_some_and(|required| available >= required)
}

fn bad_fee_return_status(prior_dispatch_may_have_committed: bool) -> state::BotReturnTransferStatus {
    if prior_dispatch_may_have_committed {
        state::BotReturnTransferStatus::FeeMismatchAmbiguous
    } else {
        state::BotReturnTransferStatus::NoEffect
    }
}

/// Retain a tuple only when it was dispatched for the first time and the
/// ledger returned typed BadFee, which proves that tuple had no effect.
/// ReturnFeeQueryPending authorizes another fee query while the bounded retry
/// budget remains; exhaustion leaves the complete history operator-held.
fn archive_first_dispatch_bad_fee(
    claim: &mut state::BotClaimJournal,
) -> Option<bool> {
    if claim.status != state::BotClaimJournalStatus::ReturnPending {
        return None;
    }
    let intent = claim.collateral_return.as_ref()?;
    if intent.status != state::BotReturnTransferStatus::Prepared
        || intent.receipt.is_some()
        || intent.transfer_fee_e8s.is_none()
    {
        return None;
    }

    let mut failed = claim.collateral_return.take()?;
    failed.status = state::BotReturnTransferStatus::NoEffect;
    claim.failed_return_attempts.push(failed);
    let refresh_scheduled = claim.failed_return_attempts.len() <= MAX_AUTO_RETURN_FEE_REFRESHES;
    claim.status = if refresh_scheduled {
        state::BotClaimJournalStatus::ReturnFeeQueryPending
    } else {
        state::BotClaimJournalStatus::ReturnFeeRefreshExhausted
    };
    Some(refresh_scheduled)
}

fn record_first_dispatch_bad_fee(vault_id: u64) -> Option<bool> {
    let result = state::mutate_state(|s| {
        s.pending_claims
            .get_mut(&vault_id)
            .and_then(archive_first_dispatch_bad_fee)
    });
    if result.is_some() {
        state::save_config_to_stable();
    }
    result
}

fn next_return_created_at_time(
    now: u64,
    failed_attempts: &[state::BotReturnTransferJournal],
) -> Option<u64> {
    match failed_attempts.last() {
        Some(previous) => previous.created_at_time.checked_add(1).map(|next| now.max(next)),
        None => Some(now),
    }
}

/// Whether this claim has an automatic return action left to perform.
/// Definitive no-effect and expired ambiguous tuples require reconciliation;
/// they must not block another claim's resumable return.
fn return_claim_is_actionable(claim: &state::BotClaimJournal, now: u64) -> bool {
    match &claim.status {
        state::BotClaimJournalStatus::ReturnFeeQueryPending => {
            claim.collateral_return.is_none()
                && claim.failed_return_attempts.len() <= MAX_AUTO_RETURN_FEE_REFRESHES
        }
        state::BotClaimJournalStatus::ReturnPending => {
            let Some(intent) = claim.collateral_return.as_ref() else {
                return false;
            };
            if matches!(
                &intent.status,
                state::BotReturnTransferStatus::NoEffect
                    | state::BotReturnTransferStatus::FeeMismatchAmbiguous
            ) {
                return false;
            }
            if intent.receipt.is_some() {
                return true;
            }
            const SAFE_RETRY_WINDOW_NS: u64 = 23 * 60 * 60 * 1_000_000_000;
            now.saturating_sub(intent.created_at_time) < SAFE_RETRY_WINDOW_NS
        }
        _ => false,
    }
}

fn return_claim_matches(
    claim: &state::BotClaimJournal,
    only_vault_id: Option<u64>,
    now: u64,
) -> bool {
    only_vault_id.is_none_or(|vault_id| claim.vault_id == vault_id)
        && return_claim_is_actionable(claim, now)
}

fn select_return_claim_id(
    claims: &std::collections::BTreeMap<u64, state::BotClaimJournal>,
    only_vault_id: Option<u64>,
    cursor: Option<u64>,
    now: u64,
) -> Option<u64> {
    if let Some(vault_id) = only_vault_id {
        return claims
            .get(&vault_id)
            .filter(|claim| return_claim_matches(claim, Some(vault_id), now))
            .map(|_| vault_id);
    }

    let next = claims
        .iter()
        .find(|(vault_id, claim)| {
            cursor.is_none_or(|last| **vault_id > last)
                && return_claim_is_actionable(claim, now)
        })
        .map(|(vault_id, _)| *vault_id);
    next.or_else(|| {
        claims.iter()
            .find(|(_, claim)| return_claim_is_actionable(claim, now))
            .map(|(vault_id, _)| *vault_id)
    })
}

/// Resume a claim-bound collateral return with the exact ledger dedup tuple.
/// When a vault id is supplied, process only that claim; otherwise select the
/// first claim that can still be retried automatically.
async fn resume_pending_return(config: &BotConfig, only_vault_id: Option<u64>) -> bool {
    let cursor = RETURN_RESUME_CURSOR.with(std::cell::Cell::get);
    let Some(vault_id) = state::read_state(|s| {
        select_return_claim_id(&s.pending_claims, only_vault_id, cursor, ic_cdk::api::time())
    }) else {
        return false;
    };
    // Advance before any await so a claim that remains pending after a
    // transient failure cannot monopolize the next timer tick. An explicit
    // admin or immediate-recovery target does not change general ordering.
    if only_vault_id.is_none() {
        RETURN_RESUME_CURSOR.with(|cursor| cursor.set(Some(vault_id)));
    }
    let Some(mut claim) = state::read_state(|s| s.pending_claims.get(&vault_id).cloned()) else {
        return false;
    };

    if claim.status == state::BotClaimJournalStatus::ReturnFeeQueryPending {
        if claim.collateral_return.is_some() {
            log!(crate::INFO, "STUCK: fee-query phase for vault #{} unexpectedly already has a return tuple", claim.vault_id);
            return true;
        }
        if claim.failed_return_attempts.len() > MAX_AUTO_RETURN_FEE_REFRESHES {
            log!(crate::INFO, "STUCK: automatic ICP return fee refresh budget is exhausted for vault #{}", claim.vault_id);
            return true;
        }
        match create_and_send_return_intent(config, &claim).await {
            Ok(receipt) => {
                claim = state::read_state(|s| {
                    s.pending_claims.get(&claim.vault_id).cloned().unwrap_or(claim.clone())
                });
                if let Some(intent) = claim.collateral_return.as_mut() {
                    intent.receipt = Some(receipt);
                    intent.status = state::BotReturnTransferStatus::ReceiptObserved;
                }
                claim.status = state::BotClaimJournalStatus::ReturnPending;
                state::mutate_state(|s| { s.pending_claims.insert(claim.vault_id, claim.clone()); });
                state::save_config_to_stable();
            }
            Err(swap::TransferAttemptError::BadFee { expected_fee }) => {
                let refresh_scheduled = record_first_dispatch_bad_fee(claim.vault_id);
                log!(crate::INFO, "First-dispatch ICP return for vault #{} received typed BadFee (expected {}). The no-effect tuple was retained; fee refresh scheduled: {}", claim.vault_id, expected_fee, refresh_scheduled == Some(true));
                return true;
            }
            Err(swap::TransferAttemptError::Ambiguous(error)) => {
                claim = state::read_state(|s| {
                    s.pending_claims.get(&claim.vault_id).cloned().unwrap_or(claim.clone())
                });
                if let Some(intent) = claim.collateral_return.as_mut() {
                    intent.status = state::BotReturnTransferStatus::Ambiguous;
                }
                claim.status = state::BotClaimJournalStatus::ReturnPending;
                state::mutate_state(|s| { s.pending_claims.insert(claim.vault_id, claim.clone()); });
                state::save_config_to_stable();
                log!(crate::INFO, "Return outcome remains ambiguous for vault #{}: {}", claim.vault_id, error);
                return true;
            }
            Err(swap::TransferAttemptError::NoEffect(error)) => {
                let persisted = state::read_state(|s| {
                    s.pending_claims.get(&claim.vault_id).cloned()
                });
                if let Some(persisted) = persisted {
                    claim = persisted;
                    if let Some(intent) = claim.collateral_return.as_mut() {
                        intent.status = state::BotReturnTransferStatus::NoEffect;
                        claim.status = state::BotClaimJournalStatus::ReturnPending;
                        state::mutate_state(|s| { s.pending_claims.insert(claim.vault_id, claim.clone()); });
                        state::save_config_to_stable();
                    }
                }
                log!(crate::INFO, "No ICP return tuple was dispatched for vault #{}: {}", claim.vault_id, error);
                return true;
            }
        }
    }

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
    if intent.status == state::BotReturnTransferStatus::FeeMismatchAmbiguous {
        log!(crate::INFO, "STUCK: ICP return for vault #{} has an ambiguous prior submission after BadFee; exact ledger history reconciliation is required", claim.vault_id);
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
            intent.ledger_principal,
            intent.memo.clone(),
            intent.amount_e8s,
            intent.fee_e8s,
            intent.transfer_fee_e8s,
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
            Err(swap::TransferAttemptError::BadFee { expected_fee }) => {
                // The current attempt had no effect, but a prior dispatch may
                // have committed before a lost reply. Preserve the original
                // tuple and stop automatic retries until exact block history
                // is reconciled.
                intent.status = bad_fee_return_status(true);
                claim.collateral_return = Some(intent);
                state::mutate_state(|s| { s.pending_claims.insert(claim.vault_id, claim.clone()); });
                state::save_config_to_stable();
                log!(crate::INFO, "STUCK: replayed ICP return for vault #{} received BadFee (expected {}); prior submission may have committed, so its original tuple is held for exact ledger reconciliation", claim.vault_id, expected_fee);
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
    if vault_id == VAULT158_ID
        && state::read_state(|s| s.vault158_recovery.as_ref().is_some_and(vault158_exact_evidence))
    {
        return Err("vault-158 recovery keeps its original payment tuple immutable; reconcile its exact journal and receipt before any further action".into());
    }
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

/// Persist and dispatch one exact residual transfer after an administrator
/// explicitly allocates the required ckUSDC from the bot's mixed account.
/// The amount sent is always the claim's exact shortfall; the allocation also
/// covers its separately charged ledger fee.
pub async fn admin_authorize_shortfall_topup(
    config: &BotConfig,
    vault_id: u64,
    funding_allocation_e6: u64,
) -> Result<(), String> {
    let mut journal = state::read_state(|s| s.pending_payments.get(&vault_id).cloned())
        .ok_or_else(|| "no pending payment journal for this vault".to_string())?;
    if config.backend_principal != journal.backend_principal
        || config.ckusdc_ledger != journal.ledger_principal
    {
        return Err("configured backend or ckUSDC ledger differs from the payment journal".into());
    }
    shortfall_residual(&journal)?;
    let receipt = journal
        .receipt
        .as_ref()
        .expect("shortfall_residual checked receipt");
    call_bot_verify_liquidation_payment_proof(
        config,
        BotPaymentProof {
            vault_id: journal.vault_id,
            claim_generation: journal.claim_generation,
            ledger_principal: journal.ledger_principal,
            block_index: receipt.block_index,
            amount_e6s: receipt.amount,
            created_at_time: receipt.created_at_time,
        },
    )
    .await
    .map_err(|error| {
        format!(
            "original short-payment block is not independently verified; no residual tuple was authorized: {error}"
        )
    })?;
    if !journal.shortfall_receipt_observed {
        journal.shortfall_receipt_observed = true;
    }
    if let Some(existing) = journal.shortfall_topup.as_ref() {
        if existing.funding_allocation_e6 != funding_allocation_e6 {
            return Err("a residual payment tuple already exists with a different funding allocation".into());
        }
    } else {
        let fee = swap::fetch_ledger_fee(journal.ledger_principal).await?;
        let balance = swap::balance_of_self_ckusdc(config).await?;
        if balance < funding_allocation_e6 {
            return Err(format!(
                "bot ckUSDC balance {} is below the explicitly allocated residual debit {}",
                balance, funding_allocation_e6
            ));
        }
        journal.shortfall_topup = Some(prepare_shortfall_topup_intent(
            &journal,
            fee,
            ic_cdk::api::time(),
            funding_allocation_e6,
        )?);
    }
    state::mutate_state(|s| { s.pending_payments.insert(vault_id, journal.clone()); });
    state::save_config_to_stable();
    resume_shortfall_payment(config, journal).await;
    if state::read_state(|s| s.pending_payments.contains_key(&vault_id)) {
        Err("shortfall top-up or cumulative backend proof remains pending; inspect the durable payment journal".into())
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
    call_bot_verify_liquidation_payment_proof(
        config,
        BotPaymentProof {
            vault_id: journal.vault_id,
            claim_generation: journal.claim_generation,
            ledger_principal: journal.ledger_principal,
            block_index,
            amount_e6s: journal.amount_e6,
            created_at_time: journal.created_at_time,
        },
    )
    .await
    .map_err(|error| {
        format!(
            "candidate original payment block did not prove the exact claim-bound transfer; journal remains unchanged: {error}"
        )
    })?;
    journal.receipt = Some(swap::TransferReceipt {
        block_index,
        amount: journal.amount_e6,
        created_at_time: journal.created_at_time,
    });
    journal.status = state::BotPaymentStatus::ReceiptObserved;
    state::mutate_state(|s| {
        record_vault158_payment_receipt(s, &journal);
        s.pending_payments.insert(vault_id, journal);
    });
    state::save_config_to_stable();
    resume_pending_payment_for_vault(config, Some(vault_id)).await;
    if state::read_state(|s| s.pending_payments.contains_key(&vault_id)) {
        Err("candidate block was not accepted or confirmation remains pending; inspect the durable journal".into())
    } else {
        Ok(())
    }
}

/// Reconcile an ambiguous residual transfer from ICRC-3 history after its
/// deduplication window has expired. This only records a block for the
/// already persisted exact residual tuple; it never dispatches a transfer.
pub async fn admin_reconcile_shortfall_topup_block(
    config: &BotConfig,
    vault_id: u64,
    block_index: u64,
) -> Result<(), String> {
    let journal = state::read_state(|s| s.pending_payments.get(&vault_id).cloned())
        .ok_or_else(|| "no pending payment journal for this vault".to_string())?;
    reconciled_shortfall_receipt(&journal, block_index)?;
    let topup = journal.shortfall_topup.as_ref().expect("receipt helper checked topup");
    if config.backend_principal != journal.backend_principal
        || config.ckusdc_ledger != journal.ledger_principal
        || topup.backend_principal != journal.backend_principal
        || topup.ledger_principal != journal.ledger_principal
    {
        return Err("configured backend or ckUSDC ledger differs from the persisted residual tuple".into());
    }

    // The trusted backend checks sender, receiver, ledger, amount, memo,
    // timestamp, claim generation, and replay status against the active claim.
    // A stored receipt came either from the ledger reply or from an earlier
    // reconciliation. In that case cumulative confirmation itself is the
    // idempotent retry boundary: the backend accepts the complete consumed
    // proof batch after a lost confirmation reply. Its single-proof verifier
    // intentionally rejects already-consumed blocks.
    if topup.receipt.is_none() {
        call_bot_verify_liquidation_payment_proof(
            config,
            BotPaymentProof {
                vault_id: journal.vault_id,
                claim_generation: journal.claim_generation,
                ledger_principal: journal.ledger_principal,
                block_index,
                amount_e6s: topup.amount_e6,
                created_at_time: topup.created_at_time,
            },
        ).await.map_err(|error| format!(
            "candidate residual block did not prove the exact persisted claim-bound transfer; journal unchanged: {error}"
        ))?;
    }

    // Re-read after the inter-canister await. Only write if the original
    // receipt and entire persisted residual tuple are still the same.
    let mut current = state::read_state(|s| s.pending_payments.get(&vault_id).cloned())
        .ok_or_else(|| "payment journal disappeared during residual proof verification".to_string())?;
    if current.claim_generation != journal.claim_generation
        || current.receipt != journal.receipt
        || current.shortfall_topup != journal.shortfall_topup
        || current.backend_principal != journal.backend_principal
        || current.ledger_principal != journal.ledger_principal
        || current.memo != journal.memo
    {
        return Err("payment journal changed during residual proof verification".into());
    }
    let observed = reconciled_shortfall_receipt(&current, block_index)?;
    let topup = current.shortfall_topup.as_mut().expect("receipt helper checked topup");
    if topup.receipt.is_none() {
        topup.receipt = Some(observed);
        topup.status = state::BotPaymentStatus::ReceiptObserved;
        state::mutate_state(|s| { s.pending_payments.insert(vault_id, current.clone()); });
        state::save_config_to_stable();
    }

    // Confirmation rechecks both receipts and the cumulative exact debt at
    // the backend. Reply loss is safe: the stored block is reused and backend
    // confirmation is idempotent for the exact proof batch.
    resume_shortfall_payment(config, current).await;
    if state::read_state(|s| s.pending_payments.contains_key(&vault_id)) {
        Err("residual block was recorded; cumulative confirmation remains pending; inspect the durable journal".into())
    } else {
        Ok(())
    }
}

async fn call_bot_verify_liquidation_payment_proof(
    config: &BotConfig,
    proof: BotPaymentProof,
) -> Result<(), String> {
    let result: Result<(BackendResult<()>,), _> = ic_cdk::call(
        config.backend_principal,
        "bot_verify_liquidation_payment_proof",
        (proof,),
    )
    .await;
    match result {
        Ok((BackendResult::Ok(()),)) => Ok(()),
        Ok((BackendResult::Err(error),)) => Err(format!("{}", error)),
        Err((code, message)) => Err(format!("{:?}: {}", code, message)),
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
    resume_pending_return(config, Some(vault_id)).await;
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

async fn call_bot_confirm_liquidation_with_proofs(
    config: &BotConfig,
    proofs: Vec<BotPaymentProof>,
) -> Result<(), String> {
    let result: Result<(BackendResult<()>,), _> = ic_cdk::call(
        config.backend_principal,
        "bot_confirm_liquidation_with_proofs",
        (proofs,),
    )
    .await;
    match result {
        Ok((BackendResult::Ok(()),)) => Ok(()),
        Ok((BackendResult::Err(error),)) => Err(format!("{}", error)),
        Err((code, message)) => Err(format!("{:?}: {}", code, message)),
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
    use candid::Decode;

    fn unhex(input: &str) -> Vec<u8> {
        input.trim().as_bytes().chunks_exact(2).map(|pair| {
            let digit = |c: u8| match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                _ => panic!("invalid fixture hex"),
            };
            digit(pair[0]) * 16 + digit(pair[1])
        }).collect()
    }

    #[test]
    fn captured_vault158_ckusdc_output_block_requires_exact_pool_receipt_and_fee() {
        let bytes = unhex(include_str!("../tests/fixtures/vault158-ckusdc-output-block-806697.hex"));
        let result: icrc_ledger_types::icrc3::blocks::GetBlocksResult =
            candid::Decode!(&bytes, icrc_ledger_types::icrc3::blocks::GetBlocksResult)
                .expect("captured ICRC-3 response decodes");
        assert_eq!(result.blocks.len(), 1);
        assert_eq!(result.blocks[0].id.0.to_string(), "806697");
        let block = rumi_protocol_backend::icrc3_proof::decode_block(&result.blocks[0].block)
            .expect("captured ckUSDC block decodes");
        let pool = candid::Principal::from_text("mohjv-bqaaa-aaaag-qjyia-cai").unwrap();
        let bot = candid::Principal::from_text("nygob-3qaaa-aaaap-qttcq-cai").unwrap();
        assert!(validate_vault158_swap_output_block(&block, pool, bot).is_ok());
        assert!(validate_vault158_swap_output_block(&block, candid::Principal::anonymous(), bot).is_err());
        assert!(validate_vault158_swap_output_block(&block, pool, candid::Principal::anonymous()).is_err());
        assert!(validate_vault158_swap_output_block(&block, pool, candid::Principal::from_slice(&[7])).is_err());
        let mut wrong_fee = block.clone();
        wrong_fee.fee = Some(9_999);
        assert!(validate_vault158_swap_output_block(&wrong_fee, pool, bot).is_err());
        let mut wrong_memo = block.clone();
        wrong_memo.memo = Some(vec![1]);
        assert!(validate_vault158_swap_output_block(&wrong_memo, pool, bot).is_err());
        let mut wrong_amount = block.clone();
        wrong_amount.amount -= 1;
        assert!(validate_vault158_swap_output_block(&wrong_amount, pool, bot).is_err());
    }

    fn vault158_evidence_fixture() -> state::Vault158RecoveryEvidence {
        state::Vault158RecoveryEvidence {
            record_id: 229,
            vault_id: 158,
            claim_generation: 1,
            backend_principal: candid::Principal::from_text("tfesu-vyaaa-aaaap-qrd7a-cai").unwrap(),
            pool_principal: candid::Principal::from_text("mohjv-bqaaa-aaaag-qjyia-cai").unwrap(),
            icp_ledger_principal: candid::Principal::from_text("ryjl3-tyaaa-aaaaa-aaaba-cai").unwrap(),
            ckusdc_ledger_principal: candid::Principal::from_text("xevnm-gaaaa-aaaar-qafnq-cai").unwrap(),
            collateral_amount_e8s: 236_783_835,
            collateral_received_e8s: 236_773_835,
            debt_covered_e8s: 513_841_815,
            swap_input_amount_e8s: 226_020_933,
            swap_input_fee_e8s: 10_000,
            swap_output_amount_e6: 5_545_309,
            swap_input_receipt: swap::TransferReceipt {
                block_index: 39_835_236, amount: 226_020_933,
                created_at_time: 1_793_306_981_079_625_901,
            },
            swap_output_receipt: swap::TransferReceipt {
                block_index: 806_697, amount: 5_545_309, created_at_time: 0,
            },
            pre_swap_ckusdc_balance_e6: 26_190_995,
            held_surplus_e6: 396_890,
            payment_gross_e6: 5_148_419,
            payment_amount_e6: 5_138_419,
            payment_fee_e6: 10_000,
            payment_created_at_time: None,
            payment_memo: payment_memo(158, 1),
            payment_receipt: None,
            status: state::Vault158RecoveryStatus::EvidencePrepared,
        }
    }

    fn vault158_claim_history_fixture() -> (BotConfig, state::BotClaimJournal, LiquidationRecordV1, candid::Principal) {
        let bot = candid::Principal::from_text("nygob-3qaaa-aaaap-qttcq-cai").unwrap();
        let backend = candid::Principal::from_text("tfesu-vyaaa-aaaap-qrd7a-cai").unwrap();
        let pool = candid::Principal::from_text("mohjv-bqaaa-aaaag-qjyia-cai").unwrap();
        let icp = candid::Principal::from_text("ryjl3-tyaaa-aaaaa-aaaba-cai").unwrap();
        let ckusdc = candid::Principal::from_text("xevnm-gaaaa-aaaar-qafnq-cai").unwrap();
        let config = BotConfig {
            backend_principal: backend,
            treasury_principal: candid::Principal::anonymous(),
            admin: candid::Principal::anonymous(),
            max_slippage_bps: 100,
            icp_ledger: icp,
            ckusdc_ledger: ckusdc,
            icpswap_pool: pool,
            icpswap_zero_for_one: Some(true),
            icp_fee_e8s: Some(10_000),
            ckusdc_fee_e6: Some(10_000),
            three_pool_principal: None,
            kong_swap_principal: None,
            ckusdt_ledger: None,
            icusd_ledger: None,
        };
        let claim = state::BotClaimJournal {
            vault_id: 158,
            claim_generation: 1,
            debt_covered_e8s: 513_841_815,
            collateral_amount_e8s: 236_783_835,
            collateral_received_amount_e8s: Some(236_773_835),
            collateral_outbound_fee_e8s: Some(10_000),
            collateral_price_e8s: 100_000_000,
            payment_memo: payment_memo(158, 1),
            collateral_return_memo: b"return".to_vec(),
            failed_return_attempts: vec![],
            collateral_return: None,
            swap_intents: vec![state::BotSwapIntent {
                pool_principal: pool,
                input_ledger_principal: icp,
                output_ledger_principal: ckusdc,
                amount_in_e8s: 226_020_933,
                amount_out_minimum_e6: 5_447_073,
                zero_for_one: true,
                input_fee_e8s: 10_000,
                output_fee_e6: 10_000,
                attempt_ordinal: 1,
                created_at_time: 1_793_306_975_110_675_351,
                pre_swap_ckusdc_balance_e6: 26_190_995,
            }],
            status: state::BotClaimJournalStatus::PaymentShortfall,
        };
        let record = LiquidationRecordV1 {
            id: 229,
            vault_id: 158,
            timestamp: 1,
            status: LiquidationStatus::TransferFailed,
            collateral_claimed_e8s: 236_783_835,
            debt_to_cover_e8s: 513_841_815,
            icp_swapped_e8s: 226_020_933,
            ckusdc_received_e6: 0,
            ckusdc_transferred_e6: 0,
            icp_to_treasury_e8s: 0,
            oracle_price_e8s: 100_000_000,
            effective_price_e8s: 0,
            slippage_bps: 0,
            error_message: Some("short-payment recovery".into()),
            confirm_retry_count: 0,
        };
        (config, claim, record, bot)
    }

    #[test]
    fn live_shaped_vault158_claim_and_history_229_are_accepted_exactly() {
        let (config, claim, record, bot) = vault158_claim_history_fixture();
        assert!(vault158_identity_matches_for_bot(&config, &claim, &record, bot).is_ok());
        let mut wrong_generation = claim.clone();
        wrong_generation.claim_generation = 2;
        assert!(vault158_identity_matches_for_bot(&config, &wrong_generation, &record, bot).is_err());
        let mut wrong_output_record = record.clone();
        wrong_output_record.ckusdc_received_e6 = 5_545_309;
        assert!(vault158_identity_matches_for_bot(&config, &claim, &wrong_output_record, bot).is_err());
        let mut returned_claim = claim.clone();
        returned_claim.collateral_return = Some(state::BotReturnTransferJournal {
            ledger_principal: config.icp_ledger,
            backend_principal: config.backend_principal,
            amount_e8s: 1,
            fee_e8s: 10_000,
            transfer_fee_e8s: Some(10_000),
            memo: b"return".to_vec(),
            created_at_time: 1,
            receipt: None,
            status: state::BotReturnTransferStatus::Prepared,
        });
        assert!(vault158_identity_matches_for_bot(&config, &returned_claim, &record, bot).is_err());
    }

    const FEE: u64 = 10_000;

    #[test]
    fn ordinary_single_swap_fee_is_excluded_from_treasury_residual() {
        let net = 100_000_000u64;
        let swap = calculate_swap_amount(net, 50_000_000_000, 1_000_000_000_000);
        assert!(swap_fits_claim(net, swap, FEE));
        // Treasury gets only genuine residual; the fee is not drawn from float.
        assert_eq!(claim_icp_residual_after_swap(net, swap, FEE), net - swap - FEE);
        // The pre-fix journal value (0) over-allocates by exactly one fee.
        assert_eq!(
            claim_icp_residual_after_swap(net, swap, 0) - claim_icp_residual_after_swap(net, swap, FEE),
            FEE
        );
    }

    #[test]
    fn ordinary_initial_plus_topup_charge_both_fees() {
        let net = 100_000_000u64;
        let (initial, topup) = (60_000_000u64, 20_000_000u64);
        assert!(swap_fits_claim(net, initial, FEE));
        let remaining = claim_icp_remaining(net, initial, FEE);
        assert_eq!(remaining, net - initial - FEE);
        assert!(swap_fits_claim(remaining, topup, FEE));
        let fees = FEE + FEE;
        assert_eq!(claim_icp_remaining(net, initial + topup, fees), net - initial - topup - fees);
        assert_eq!(claim_icp_residual_after_swap(net, initial + topup, fees), 20_000_000 - 2 * FEE);
        // A top-up bounded by remaining-minus-fee fits exactly and leaves no residual.
        let max_topup = max_swap_input_for_claim(remaining, FEE);
        assert!(swap_fits_claim(remaining, max_topup, FEE));
        assert!(!swap_fits_claim(remaining, max_topup + 1, FEE));
        assert_eq!(claim_icp_residual_after_swap(net, initial + max_topup, fees), 0);
    }

    #[test]
    fn swap_that_cannot_cover_its_fee_from_the_claim_is_refused() {
        // Full-collateral swap (e.g. zero oracle price) must be shrunk by the fee.
        let net = 5_000_000u64;
        assert!(!swap_fits_claim(net, net, FEE));
        assert!(swap_fits_claim(net, max_swap_input_for_claim(net, FEE), FEE));
        // Remaining no larger than the fee: nothing can be swapped.
        assert_eq!(max_swap_input_for_claim(FEE, FEE), 0);
        assert!(!swap_fits_claim(FEE, 0, FEE));
        assert!(!swap_fits_claim(FEE - 1, 1, FEE));
        assert!(!swap_fits_claim(u64::MAX, u64::MAX, FEE), "overflow must not pass");
        // Fees already charged shrink what the top-up may use, never below zero.
        assert_eq!(claim_icp_remaining(net, net, FEE), 0);
    }

    #[test]
    fn legacy_zero_fee_journal_residual_semantics_are_unchanged() {
        // Old payment journals decode with swap_input_fee_e8s == 0 and must not
        // have a fee guessed for them.
        assert_eq!(claim_icp_residual_after_swap(100, 60, 0), 40);
        assert_eq!(claim_icp_residual_after_swap(60, 60, 0), 0);
        assert_eq!(claim_icp_residual_after_swap(50, 60, 0), 0);
    }

    #[test]
    fn vault158_evidence_is_exact_generation_and_receipts_are_single_use() {
        let evidence = vault158_evidence_fixture();
        assert!(vault158_exact_evidence(&evidence));
        let mut wrong_generation = evidence.clone();
        wrong_generation.claim_generation = 2;
        assert!(!vault158_exact_evidence(&wrong_generation));
        let mut wrong_input = evidence.clone();
        wrong_input.swap_input_receipt.block_index += 1;
        assert!(!vault158_exact_evidence(&wrong_input));
        let mut wrong_output = evidence.clone();
        wrong_output.swap_output_receipt.amount -= 1;
        assert!(!vault158_exact_evidence(&wrong_output));
        assert_eq!(
            claim_icp_residual_after_swap(
                evidence.collateral_received_e8s,
                evidence.swap_input_amount_e8s,
                evidence.swap_input_fee_e8s,
            ),
            10_742_902,
            "claim residual excludes the separately paid swap input fee"
        );
        assert_eq!(
            evidence.pre_swap_ckusdc_balance_e6 + evidence.swap_output_amount_e6
                - evidence.payment_gross_e6,
            26_587_885,
            "old balance and 396,890 held surplus remain outside the payment allocation"
        );
        let exact_balance = evidence.pre_swap_ckusdc_balance_e6 + evidence.swap_output_amount_e6;
        assert!(vault158_balance_matches_exact_allocation(exact_balance, &evidence));
        assert!(!vault158_balance_matches_exact_allocation(exact_balance + 1, &evidence),
            "unrelated deposits cannot mask a changed aggregate balance");
        assert!(!vault158_balance_matches_exact_allocation(exact_balance - 1, &evidence),
            "spent claim proceeds cannot be covered by other funds");
    }

    #[test]
    fn vault158_execution_promotes_only_the_exact_claim_once() {
        let (_, claim, _, _) = vault158_claim_history_fixture();
        let mut evidence = vault158_evidence_fixture();
        evidence.payment_created_at_time = Some(99);
        evidence.status = state::Vault158RecoveryStatus::PaymentPrepared;
        let journal = state::BotPaymentJournal {
            record_id: Some(VAULT158_RECORD_ID),
            vault_id: VAULT158_ID,
            backend_principal: evidence.backend_principal,
            ledger_principal: evidence.ckusdc_ledger_principal,
            claim_generation: VAULT158_GENERATION,
            debt_covered_e8s: evidence.debt_covered_e8s,
            collateral_amount_e8s: evidence.collateral_amount_e8s,
            collateral_received_amount_e8s: Some(evidence.collateral_received_e8s),
            collateral_price_e8s: claim.collateral_price_e8s,
            icp_swapped_e8s: evidence.swap_input_amount_e8s,
            swap_input_fee_e8s: evidence.swap_input_fee_e8s,
            ckusdc_received_e6: evidence.swap_output_amount_e6,
            held_surplus_e6: evidence.held_surplus_e6,
            gross_amount_e6: evidence.payment_gross_e6,
            amount_e6: evidence.payment_amount_e6,
            fee_e6: evidence.payment_fee_e6,
            created_at_time: 99,
            memo: evidence.payment_memo.clone(),
            status: state::BotPaymentStatus::Prepared,
            receipt: None,
            shortfall_receipt_observed: false,
            shortfall_topup: None,
        };
        let mut bot_state = state::BotState::default();
        bot_state.vault158_recovery = Some({
            let mut prepared = evidence.clone();
            prepared.status = state::Vault158RecoveryStatus::EvidencePrepared;
            prepared
        });
        bot_state.processing_paused = true;
        bot_state.pending_claims.insert(VAULT158_ID, claim.clone());
        assert!(commit_vault158_payment_preparation_once(&mut bot_state, &evidence, journal.clone()));
        assert!(!bot_state.pending_claims.contains_key(&VAULT158_ID),
            "exact claim is atomically promoted when its payment journal is committed");
        assert!(vault158_payment_journal_matches_evidence(
            bot_state.pending_payments.get(&VAULT158_ID).unwrap(), &evidence));
        let mut unrelated_payment = bot_state.pending_payments.get(&VAULT158_ID).unwrap().clone();
        unrelated_payment.vault_id = 1;
        bot_state.pending_payments.insert(1, unrelated_payment);
        assert_eq!(select_pending_payment(&bot_state, Some(VAULT158_ID)).unwrap().vault_id, VAULT158_ID,
            "recovery selection ignores lower-ID unrelated payment journals");
        bot_state.pending_payments.remove(&1);
        assert_eq!(bot_state.vault158_recovery.as_ref().unwrap().status,
            state::Vault158RecoveryStatus::PaymentPrepared);
        assert!(!commit_vault158_payment_preparation_once(&mut bot_state, &evidence, journal),
            "replay cannot create a second tuple after the claim is consumed");

        let mut unrelated_state = state::BotState::default();
        unrelated_state.vault158_recovery = Some({
            let mut prepared = evidence.clone();
            prepared.status = state::Vault158RecoveryStatus::EvidencePrepared;
            prepared
        });
        unrelated_state.processing_paused = true;
        let mut wrong_claim = claim;
        wrong_claim.claim_generation = 2;
        unrelated_state.pending_claims.insert(VAULT158_ID, wrong_claim);
        let wrong_journal = bot_state.pending_payments.get(&VAULT158_ID).unwrap().clone();
        assert!(!commit_vault158_payment_preparation_once(
            &mut unrelated_state, &evidence, wrong_journal));
        assert!(unrelated_state.pending_payments.is_empty());
        assert!(unrelated_state.pending_claims.contains_key(&VAULT158_ID));
    }

    #[test]
    fn confirmed_vault158_tombstone_survives_journal_consumption_and_replay() {
        let mut evidence = vault158_evidence_fixture();
        evidence.payment_created_at_time = Some(99);
        evidence.status = state::Vault158RecoveryStatus::PaymentPrepared;
        let receipt = swap::TransferReceipt { block_index: 900, amount: 5_138_419, created_at_time: 99 };
        evidence.payment_receipt = Some(receipt.clone());
        let journal = state::BotPaymentJournal {
            record_id: Some(229), vault_id: 158,
            backend_principal: evidence.backend_principal,
            ledger_principal: evidence.ckusdc_ledger_principal,
            claim_generation: 1, debt_covered_e8s: 513_841_815,
            collateral_amount_e8s: 236_783_835,
            collateral_received_amount_e8s: Some(236_773_835),
            collateral_price_e8s: 1,
            icp_swapped_e8s: 226_020_933,
            swap_input_fee_e8s: 10_000,
            ckusdc_received_e6: 5_545_309, held_surplus_e6: 396_890,
            gross_amount_e6: 5_148_419, amount_e6: 5_138_419, fee_e6: 10_000,
            created_at_time: 99, memo: payment_memo(158, 1),
            status: state::BotPaymentStatus::ReceiptObserved,
            receipt: Some(receipt), shortfall_receipt_observed: false, shortfall_topup: None,
        };
        assert!(vault158_payment_journal_matches_evidence(&journal, &evidence));
        let mut ambiguous = journal.clone();
        ambiguous.status = state::BotPaymentStatus::Ambiguous;
        assert!(vault158_payment_journal_matches_evidence(&ambiguous, &evidence),
            "restart/ambiguous state keeps the same outgoing tuple");
        let mut regenerated = journal.clone();
        regenerated.created_at_time += 1;
        assert!(!vault158_payment_journal_matches_evidence(&regenerated, &evidence),
            "a retry cannot allocate a new timestamp");
        let mut wrong_receipt = journal.clone();
        wrong_receipt.receipt.as_mut().unwrap().block_index += 1;
        assert!(!vault158_payment_journal_matches_evidence(&wrong_receipt, &evidence),
            "a retry cannot consume a different receipt");
        let mut bot_state = state::BotState::default();
        bot_state.vault158_recovery = Some(evidence);
        bot_state.pending_payments.insert(158, journal.clone());
        assert!(apply_confirmed_payment_totals_once(&mut bot_state, &journal, 5_138_419));
        assert!(!bot_state.pending_payments.contains_key(&158));
        let tombstone = bot_state.vault158_recovery.as_ref().unwrap();
        assert_eq!(tombstone.status, state::Vault158RecoveryStatus::PaymentConfirmed);
        assert_eq!(tombstone.payment_receipt, journal.receipt);
        assert!(!apply_confirmed_payment_totals_once(&mut bot_state, &journal, 5_138_419));
        assert_eq!(bot_state.stats.events_count, 1);
        assert!(bot_state.vault158_recovery.is_some(), "receipt tombstone survives journal deletion");
    }

    #[derive(candid::CandidType)]
    #[allow(dead_code)]
    struct LegacyBotLiquidationResult {
        vault_id: u64,
        collateral_amount: u64,
        debt_covered: u64,
        collateral_price_e8s: u64,
        claim_generation: u64,
        payment_memo: Vec<u8>,
        collateral_return_memo: Vec<u8>,
        payment_ledger_principal: Option<candid::Principal>,
    }

    const SWAP_ERR: &str = "Quote returned zero output";

    #[test]
    fn held_treasury_attempt_does_not_starve_later_bot_work() {
        assert!(!should_stop_after_treasury_attempt(true));
        assert!(!should_stop_after_treasury_attempt(false));
    }

    fn treasury_test_journal() -> state::BotTreasuryJournal {
        state::BotTreasuryJournal {
            record_id: 77, vault_id: 8, claim_generation: 12,
            ledger_principal: candid::Principal::management_canister(),
            sender_principal: candid::Principal::anonymous(),
            treasury_principal: candid::Principal::anonymous(),
            gross_amount_e8s: 25_000, amount_e8s: 15_000, fee_e8s: 10_000,
            memo: b"RUMI:TB1:77:12".to_vec(), created_at_time: 100,
            status: state::BotTreasuryStatus::ReceiptObserved,
            receipt: Some(swap::TransferReceipt { block_index: 4, amount: 15_000, created_at_time: 100 }),
            paid_total_applied: false,
            record: LiquidationRecordV1 {
                id: 77, vault_id: 8, timestamp: 90,
                status: LiquidationStatus::TransferFailed,
                collateral_claimed_e8s: 30_000, debt_to_cover_e8s: 10,
                icp_swapped_e8s: 5_000, ckusdc_received_e6: 100,
                ckusdc_transferred_e6: 100, icp_to_treasury_e8s: 0,
                oracle_price_e8s: 1, effective_price_e8s: 1, slippage_bps: 0,
                error_message: None, confirm_retry_count: 1,
            },
        }
    }

    #[test]
    fn treasury_paid_totals_require_matching_receipt_and_apply_once_per_generation() {
        let journal = treasury_test_journal();
        let mut bot_state = state::BotState::default();
        bot_state.pending_treasury.insert(journal.record_id, journal.clone());
        let mut wrong_generation = journal.clone();
        wrong_generation.claim_generation += 1;
        assert!(!apply_verified_treasury_receipt_once(&mut bot_state, &wrong_generation));
        assert_eq!(bot_state.stats.total_collateral_to_treasury_e8s, 0);
        assert!(apply_verified_treasury_receipt_once(&mut bot_state, &journal));
        assert_eq!(bot_state.stats.total_collateral_to_treasury_e8s, 25_000);
        assert_eq!(bot_state.pending_treasury[&journal.record_id].status, state::BotTreasuryStatus::Paid);
        assert!(!apply_verified_treasury_receipt_once(&mut bot_state, &journal));
        assert_eq!(bot_state.stats.total_collateral_to_treasury_e8s, 25_000);
        let mut wrong_receipt = journal;
        wrong_receipt.receipt.as_mut().unwrap().amount += 1;
        assert!(!apply_verified_treasury_receipt_once(&mut bot_state, &wrong_receipt));
        assert_eq!(bot_state.stats.total_collateral_to_treasury_e8s, 25_000);
    }

    #[test]
    fn expired_ambiguous_treasury_intent_accepts_exact_verified_candidate_once() {
        let mut journal = treasury_test_journal();
        journal.status = state::BotTreasuryStatus::Ambiguous;
        journal.receipt = None;
        // The created-at time is far beyond the 23-hour redispatch window. A
        // ledger-proven candidate remains reconcilable without another send.
        let mut bot_state = state::BotState::default();
        bot_state.pending_treasury.insert(journal.record_id, journal.clone());
        let receipt = swap::TransferReceipt {
            block_index: 9001,
            amount: journal.amount_e8s,
            created_at_time: journal.created_at_time,
        };

        assert!(apply_verified_treasury_candidate_once(&mut bot_state, &journal, &receipt, journal.sender_principal));
        assert_eq!(bot_state.pending_treasury[&journal.record_id].status, state::BotTreasuryStatus::Paid);
        assert_eq!(bot_state.stats.total_collateral_to_treasury_e8s, journal.gross_amount_e8s);
        assert!(!apply_verified_treasury_candidate_once(&mut bot_state, &journal, &receipt, journal.sender_principal));
        assert_eq!(bot_state.stats.total_collateral_to_treasury_e8s, journal.gross_amount_e8s);
    }

    #[test]
    fn treasury_candidate_with_wrong_tuple_metadata_leaves_pending_intent_unchanged() {
        let mut journal = treasury_test_journal();
        journal.status = state::BotTreasuryStatus::Ambiguous;
        journal.receipt = None;
        let mut bot_state = state::BotState::default();
        bot_state.pending_treasury.insert(journal.record_id, journal.clone());
        let wrong_receipt = swap::TransferReceipt {
            block_index: 9002,
            amount: journal.amount_e8s + 1,
            created_at_time: journal.created_at_time,
        };

        assert!(!apply_verified_treasury_candidate_once(&mut bot_state, &journal, &wrong_receipt, journal.sender_principal));
        assert_eq!(bot_state.pending_treasury[&journal.record_id].status, state::BotTreasuryStatus::Ambiguous);
        assert_eq!(bot_state.stats.total_collateral_to_treasury_e8s, 0);
    }
    const RETURN_ERR: &str = "Transfer error: BadFee";
    const CANCEL_ERR: &str = "GenericError(\"Cannot cancel claim for vault #7: protocol collateral balance 0 < required 99990000\")";

    #[test]
    fn pre_swap_balance_failure_arms_only_current_unstarted_claim_for_return() {
        let mut claim = state::BotClaimJournal {
            vault_id: 19,
            claim_generation: 42,
            debt_covered_e8s: 100,
            collateral_amount_e8s: 50,
            collateral_received_amount_e8s: Some(49),
            collateral_outbound_fee_e8s: Some(1),
            collateral_price_e8s: 200,
            payment_memo: b"payment".to_vec(),
            collateral_return_memo: b"claim-19-42-return".to_vec(),
            failed_return_attempts: Vec::new(),
            collateral_return: None,
            swap_intents: Vec::new(),
            status: state::BotClaimJournalStatus::SwapMayHaveStarted,
        };

        assert!(arm_pre_swap_failure_return(&mut claim, 42));
        assert_eq!(
            claim.status,
            state::BotClaimJournalStatus::ReturnFeeQueryPending
        );
        assert!(claim.collateral_return.is_none());

        // A stale completion or a phase that may already have dispatched must
        // not gain authority to create a new return intent.
        assert!(!arm_pre_swap_failure_return(&mut claim, 41));
        assert!(!arm_pre_swap_failure_return(&mut claim, 42));
    }

    #[test]
    fn held_return_does_not_starve_a_later_resumable_claim() {
        let mut held = state::BotClaimJournal {
            vault_id: 1,
            claim_generation: 3,
            debt_covered_e8s: 100,
            collateral_amount_e8s: 50,
            collateral_received_amount_e8s: Some(49),
            collateral_outbound_fee_e8s: Some(1),
            collateral_price_e8s: 200,
            payment_memo: b"payment-1".to_vec(),
            collateral_return_memo: b"return-1".to_vec(),
            failed_return_attempts: Vec::new(),
            collateral_return: None,
            swap_intents: Vec::new(),
            status: state::BotClaimJournalStatus::ReturnPending,
        };
        held.collateral_return = Some(state::BotReturnTransferJournal {
            ledger_principal: candid::Principal::anonymous(),
            backend_principal: candid::Principal::anonymous(),
            amount_e8s: 50,
            fee_e8s: 10,
            transfer_fee_e8s: Some(10),
            memo: held.collateral_return_memo.clone(),
            created_at_time: 100,
            receipt: None,
            status: state::BotReturnTransferStatus::NoEffect,
        });
        let resumable = state::BotClaimJournal {
            vault_id: 2,
            claim_generation: 4,
            debt_covered_e8s: 100,
            collateral_amount_e8s: 50,
            collateral_received_amount_e8s: Some(49),
            collateral_outbound_fee_e8s: Some(1),
            collateral_price_e8s: 200,
            payment_memo: b"payment-2".to_vec(),
            collateral_return_memo: b"return-2".to_vec(),
            failed_return_attempts: Vec::new(),
            collateral_return: None,
            swap_intents: Vec::new(),
            status: state::BotClaimJournalStatus::ReturnFeeQueryPending,
        };
        let later_resumable = state::BotClaimJournal {
            vault_id: 3,
            claim_generation: 5,
            debt_covered_e8s: 100,
            collateral_amount_e8s: 50,
            collateral_received_amount_e8s: Some(49),
            collateral_outbound_fee_e8s: Some(1),
            collateral_price_e8s: 200,
            payment_memo: b"payment-3".to_vec(),
            collateral_return_memo: b"return-3".to_vec(),
            failed_return_attempts: Vec::new(),
            collateral_return: None,
            swap_intents: Vec::new(),
            status: state::BotClaimJournalStatus::ReturnFeeQueryPending,
        };

        assert!(!return_claim_is_actionable(&held, 200));
        assert!(return_claim_is_actionable(&resumable, 200));
        let claims = std::collections::BTreeMap::from([
            (held.vault_id, held),
            (resumable.vault_id, resumable),
            (later_resumable.vault_id, later_resumable),
        ]);
        let first = select_return_claim_id(&claims, None, None, 200);
        assert_eq!(first, Some(2));
        // Simulate the first claim remaining actionable after a failed query;
        // the cursor advances before that attempt, so the next pass picks 3.
        assert_eq!(select_return_claim_id(&claims, None, first, 200), Some(3));
        assert_eq!(select_return_claim_id(&claims, None, Some(3), 200), Some(2));
        // Explicit-vault reconciliation ignores cursor order and stays exact.
        assert_eq!(select_return_claim_id(&claims, Some(2), Some(3), 200), Some(2));
    }

    #[test]
    fn short_payment_gross_threshold_covers_rounding_and_fee() {
        assert_eq!(required_ckusdc_gross(100_000_000, 10_000), 1_010_000);
        assert_eq!(required_ckusdc_gross(100_000_001, 10_000), 1_010_001);
        assert_eq!(required_ckusdc_gross(u64::MAX, u64::MAX), u64::MAX);
    }

    #[test]
    fn net_claim_and_return_fee_float_are_checked_fail_closed() {
        assert_eq!(
            validated_net_claim(50_000_000, Some(49_980_000), Some(20_000)),
            Some(49_980_000)
        );
        assert_eq!(validated_net_claim(50_000_000, None, Some(20_000)), None);
        assert_eq!(validated_net_claim(50_000_000, Some(49_980_000), None), None);
        assert_eq!(
            validated_net_claim(50_000_000, Some(49_980_000), Some(10_000)),
            None
        );
        assert_eq!(required_return_balance(50_000_000, 10_000), Some(50_010_000));
        assert_eq!(required_return_balance(u64::MAX, 1), None);
        assert!(return_balance_covers(50_000_000, 10_000, 50_010_000));
        assert!(!return_balance_covers(50_000_000, 10_000, 50_009_999));
        assert!(!return_balance_covers(u64::MAX, 1, u64::MAX));
        assert_eq!(
            bad_fee_return_status(false),
            state::BotReturnTransferStatus::NoEffect
        );
        assert_eq!(
            bad_fee_return_status(true),
            state::BotReturnTransferStatus::FeeMismatchAmbiguous
        );
    }

    #[test]
    fn typed_first_dispatch_bad_fee_refreshes_are_lossless_unique_and_bounded() {
        let ledger = candid::Principal::from_text("ryjl3-tyaaa-aaaaa-aaaba-cai").unwrap();
        let backend = candid::Principal::from_text("tfesu-vyaaa-aaaap-qrd7a-cai").unwrap();
        let mut claim = state::BotClaimJournal {
            vault_id: 19,
            claim_generation: 2,
            debt_covered_e8s: 100,
            collateral_amount_e8s: 50,
            collateral_received_amount_e8s: Some(49),
            collateral_outbound_fee_e8s: Some(1),
            collateral_price_e8s: 200,
            payment_memo: b"payment".to_vec(),
            collateral_return_memo: b"generation-bound-return-memo".to_vec(),
            failed_return_attempts: Vec::new(),
            collateral_return: Some(state::BotReturnTransferJournal {
                ledger_principal: ledger,
                backend_principal: backend,
                amount_e8s: 50,
                fee_e8s: 10,
                transfer_fee_e8s: Some(10),
                memo: b"generation-bound-return-memo".to_vec(),
                created_at_time: 100,
                receipt: None,
                status: state::BotReturnTransferStatus::Prepared,
            }),
            swap_intents: Vec::new(),
            status: state::BotClaimJournalStatus::ReturnPending,
        };

        for prior_bad_fee_count in 1..=MAX_AUTO_RETURN_FEE_REFRESHES + 1 {
            let failed = claim.collateral_return.as_ref().unwrap().clone();
            let refresh_scheduled = archive_first_dispatch_bad_fee(&mut claim).unwrap();
            assert_eq!(
                refresh_scheduled,
                prior_bad_fee_count <= MAX_AUTO_RETURN_FEE_REFRESHES
            );
            let archived = claim.failed_return_attempts.last().unwrap();
            assert_eq!(archived.ledger_principal, failed.ledger_principal);
            assert_eq!(archived.backend_principal, failed.backend_principal);
            assert_eq!(archived.amount_e8s, failed.amount_e8s);
            assert_eq!(archived.fee_e8s, failed.fee_e8s);
            assert_eq!(archived.transfer_fee_e8s, failed.transfer_fee_e8s);
            assert_eq!(archived.memo, failed.memo);
            assert_eq!(archived.created_at_time, failed.created_at_time);
            assert_eq!(archived.status, state::BotReturnTransferStatus::NoEffect);
            assert!(archived.receipt.is_none());

            if refresh_scheduled {
                assert_eq!(claim.status, state::BotClaimJournalStatus::ReturnFeeQueryPending);
                assert!(claim.collateral_return.is_none());
                let fresh_time = next_return_created_at_time(100, &claim.failed_return_attempts)
                    .unwrap();
                assert!(fresh_time > failed.created_at_time);
                claim.collateral_return = Some(state::BotReturnTransferJournal {
                    ledger_principal: ledger,
                    backend_principal: backend,
                    amount_e8s: 50,
                    fee_e8s: 11 + prior_bad_fee_count as u64,
                    transfer_fee_e8s: Some(11 + prior_bad_fee_count as u64),
                    memo: claim.collateral_return_memo.clone(),
                    created_at_time: fresh_time,
                    receipt: None,
                    status: state::BotReturnTransferStatus::Prepared,
                });
                claim.status = state::BotClaimJournalStatus::ReturnPending;
            }
        }

        assert_eq!(claim.failed_return_attempts.len(), 4);
        assert!(claim.collateral_return.is_none());
        assert_eq!(claim.status, state::BotClaimJournalStatus::ReturnFeeRefreshExhausted);
        let timestamps = claim
            .failed_return_attempts
            .iter()
            .map(|attempt| attempt.created_at_time)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(timestamps.len(), 4, "each attempted tuple must have a distinct dedup timestamp");
        assert!(claim.failed_return_attempts.iter().all(|attempt| {
            attempt.memo == claim.collateral_return_memo
                && attempt.status == state::BotReturnTransferStatus::NoEffect
        }));
    }

    #[test]
    fn ambiguous_or_legacy_return_attempts_cannot_be_fee_refreshed() {
        let mut claim = state::BotClaimJournal {
            vault_id: 19,
            claim_generation: 2,
            debt_covered_e8s: 100,
            collateral_amount_e8s: 50,
            collateral_received_amount_e8s: Some(49),
            collateral_outbound_fee_e8s: Some(1),
            collateral_price_e8s: 200,
            payment_memo: b"payment".to_vec(),
            collateral_return_memo: b"generation-bound-return-memo".to_vec(),
            failed_return_attempts: Vec::new(),
            collateral_return: Some(state::BotReturnTransferJournal {
                ledger_principal: candid::Principal::anonymous(),
                backend_principal: candid::Principal::anonymous(),
                amount_e8s: 50,
                fee_e8s: 10,
                transfer_fee_e8s: Some(10),
                memo: b"generation-bound-return-memo".to_vec(),
                created_at_time: 100,
                receipt: None,
                status: state::BotReturnTransferStatus::Ambiguous,
            }),
            swap_intents: Vec::new(),
            status: state::BotClaimJournalStatus::ReturnPending,
        };
        assert_eq!(archive_first_dispatch_bad_fee(&mut claim), None);
        assert_eq!(claim.failed_return_attempts.len(), 0);
        assert!(claim.collateral_return.is_some());

        claim.collateral_return.as_mut().unwrap().status = state::BotReturnTransferStatus::Prepared;
        claim.collateral_return.as_mut().unwrap().transfer_fee_e8s = None;
        assert_eq!(archive_first_dispatch_bad_fee(&mut claim), None);
        assert_eq!(claim.failed_return_attempts.len(), 0);
        assert!(claim.collateral_return.is_some());
    }

    #[test]
    fn old_backend_result_decodes_with_missing_net_fields_as_held() {
        let old_response = LegacyBotLiquidationResult {
            vault_id: 19,
            collateral_amount: 50_000_000,
            debt_covered: 100_000_000,
            collateral_price_e8s: 200_000_000,
            claim_generation: 2,
            payment_memo: b"payment".to_vec(),
            collateral_return_memo: b"return".to_vec(),
            payment_ledger_principal: Some(candid::Principal::anonymous()),
        };
        let bytes = candid::encode_one(old_response).unwrap();
        let decoded: BotLiquidationResult = candid::decode_one(&bytes).unwrap();
        assert_eq!(decoded.collateral_received_amount, None);
        assert_eq!(decoded.collateral_outbound_fee, None);
        assert_eq!(
            validated_net_claim(
                decoded.collateral_amount,
                decoded.collateral_received_amount,
                decoded.collateral_outbound_fee,
            ),
            None
        );
    }

    #[test]
    fn a_short_durable_receipt_is_held_below_the_claim_net_minimum() {
        let journal = state::BotPaymentJournal {
            record_id: None,
            vault_id: 19,
            backend_principal: candid::Principal::anonymous(),
            ledger_principal: candid::Principal::management_canister(),
            claim_generation: 4,
            debt_covered_e8s: 100_000_001,
            collateral_amount_e8s: 200_000_000,
            collateral_received_amount_e8s: None,
            collateral_price_e8s: 100_000_000,
            icp_swapped_e8s: 110_000_000,
            swap_input_fee_e8s: 0,
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
            shortfall_topup: None,
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
        assert_eq!(shortfall_residual(&state::BotPaymentJournal {
            receipt: Some(short.clone()),
            ..journal.clone()
        }).unwrap(), (1_000_000, 1));
        assert!(shortfall_residual(&state::BotPaymentJournal {
            receipt: Some(exact.clone()),
            ..journal.clone()
        }).is_err(), "an exact receipt must never authorize a second transfer");
        assert!(shortfall_residual(&state::BotPaymentJournal {
            receipt: None,
            ..journal.clone()
        }).is_err(), "recovery requires the original durable receipt");
        let with_receipt = state::BotPaymentJournal {
            receipt: Some(short.clone()),
            ..journal.clone()
        };
        let intent = prepare_shortfall_topup_intent(&with_receipt, 10_000, 124, 10_001).unwrap();
        assert_eq!(intent.amount_e6, 1);
        assert_eq!(intent.fee_e6, 10_000);
        assert_eq!(intent.created_at_time, 124);
        assert_eq!(intent.memo, with_receipt.memo);
        assert_eq!(intent.funding_allocation_e6, 10_001);
        assert_eq!(intent.status, state::BotPaymentStatus::Prepared);
        assert!(intent.receipt.is_none());
        assert!(prepare_shortfall_topup_intent(&with_receipt, 10_000, 124, 10_000).is_err());
        assert!(prepare_shortfall_topup_intent(&with_receipt, 10_000, 123, 10_001).is_err());
        assert_eq!(required_ckusdc_net(u64::MAX), u64::MAX / 100 + 1);
    }

    #[test]
    fn reconciled_residual_block_uses_exact_persisted_tuple_and_preserves_original() {
        let original = crate::swap::TransferReceipt {
            block_index: 8, amount: 999_999, created_at_time: 123,
        };
        let journal = state::BotPaymentJournal {
            record_id: None, vault_id: 19,
            backend_principal: candid::Principal::anonymous(),
            ledger_principal: candid::Principal::management_canister(),
            claim_generation: 4, debt_covered_e8s: 100_000_000,
            collateral_amount_e8s: 200_000_000, collateral_received_amount_e8s: None,
            collateral_price_e8s: 100_000_000, icp_swapped_e8s: 110_000_000,
            swap_input_fee_e8s: 0,
            ckusdc_received_e6: 1_010_000, held_surplus_e6: 0,
            gross_amount_e6: 1_010_000, amount_e6: 1_000_000, fee_e6: 10_000,
            created_at_time: 123, memo: b"claim-19-4".to_vec(),
            status: state::BotPaymentStatus::Ambiguous,
            receipt: Some(original.clone()), shortfall_receipt_observed: true,
            shortfall_topup: Some(state::BotPaymentTopUpJournal {
                backend_principal: candid::Principal::anonymous(),
                ledger_principal: candid::Principal::management_canister(),
                amount_e6: 1, fee_e6: 10_000, created_at_time: 124,
                memo: b"claim-19-4".to_vec(), funding_allocation_e6: 10_001,
                status: state::BotPaymentStatus::Ambiguous, receipt: None,
            }),
        };
        // The API constructs this proof from the saved claim generation and
        // tuple; these checks reject an original-block replay or tuple drift.
        assert_eq!(reconciled_shortfall_receipt(&journal, 9).unwrap(), crate::swap::TransferReceipt {
            block_index: 9, amount: 1, created_at_time: 124,
        });
        assert_eq!(journal.receipt.as_ref(), Some(&original));
        assert!(reconciled_shortfall_receipt(&journal, 8).is_err());
        assert!(reconciled_shortfall_receipt(&state::BotPaymentJournal {
            shortfall_topup: None, ..journal.clone()
        }, 9).is_err());
        assert!(reconciled_shortfall_receipt(&state::BotPaymentJournal {
            shortfall_topup: Some(state::BotPaymentTopUpJournal {
                amount_e6: 2, ..journal.shortfall_topup.clone().unwrap()
            }), ..journal.clone()
        }, 9).is_err());
        let recorded = crate::swap::TransferReceipt { block_index: 9, amount: 1, created_at_time: 124 };
        let recorded_journal = state::BotPaymentJournal {
            shortfall_topup: Some(state::BotPaymentTopUpJournal {
                receipt: Some(recorded.clone()), ..journal.shortfall_topup.clone().unwrap()
            }), ..journal.clone()
        };
        assert_eq!(reconciled_shortfall_receipt(&recorded_journal, 9).unwrap(), recorded,
            "retry after a lost reply reuses the exact durable residual receipt");
        assert!(reconciled_shortfall_receipt(&recorded_journal, 10).is_err());
    }

    #[test]
    fn false_original_payment_proof_never_invokes_residual_transfer() {
        let mut dispatched = false;
        let result = dispatch_only_after_original_receipt_verification(
            Err("candidate block does not match the original transfer".into()),
            || {
                dispatched = true;
            },
        );

        assert!(result.is_err());
        assert!(!dispatched, "failed verification must leave transfer closure uncalled");
    }

    #[test]
    fn cumulative_shortfall_confirmation_applies_totals_and_removes_journal_once() {
        let journal = state::BotPaymentJournal {
            record_id: None,
            vault_id: 19,
            backend_principal: candid::Principal::anonymous(),
            ledger_principal: candid::Principal::management_canister(),
            claim_generation: 4,
            debt_covered_e8s: 100_000_000,
            collateral_amount_e8s: 200_000_000,
            collateral_received_amount_e8s: Some(199_980_000),
            collateral_price_e8s: 100_000_000,
            icp_swapped_e8s: 110_000_000,
            swap_input_fee_e8s: 0,
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
                amount: 999_999,
                created_at_time: 123,
            }),
            shortfall_receipt_observed: true,
            shortfall_topup: Some(state::BotPaymentTopUpJournal {
                backend_principal: candid::Principal::anonymous(),
                ledger_principal: candid::Principal::management_canister(),
                amount_e6: 1,
                fee_e6: 10_000,
                created_at_time: 124,
                memo: b"claim-19-4".to_vec(),
                funding_allocation_e6: 10_001,
                status: state::BotPaymentStatus::ReceiptObserved,
                receipt: Some(crate::swap::TransferReceipt {
                    block_index: 9,
                    amount: 1,
                    created_at_time: 124,
                }),
            }),
        };
        let topup = journal.shortfall_topup.as_ref().unwrap();
        let original_receipt = journal.receipt.as_ref().unwrap();
        let topup_receipt = topup.receipt.as_ref().unwrap();
        let total = original_receipt.amount + topup_receipt.amount;
        assert_eq!(total, required_ckusdc_net(journal.debt_covered_e8s));
        assert_ne!(original_receipt.block_index, topup_receipt.block_index);
        let mut bot_state = state::BotState::default();
        bot_state.pending_payments.insert(journal.vault_id, journal.clone());

        // Models successful batch proof confirmation followed by a replay of
        // the same local completion; accounting and journal removal are once-only.
        assert!(apply_confirmed_payment_totals_once(&mut bot_state, &journal, total));
        assert!(!apply_confirmed_payment_totals_once(&mut bot_state, &journal, total));
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
    fn post_swap_balance_read_failure_cannot_create_phase_three_reservation() {
        crate::memory::init_memory_manager();
        history::init_history();

        let decision = reserve_measured_post_swap_delta(
            99,
            1_000_000,
            500_000,
            Err("ledger query unavailable".to_string()),
        );

        assert_eq!(
            decision,
            PostSwapReservation::Unavailable("ledger query unavailable".to_string()),
            "without a measured post-swap balance there must be no reservation to pay"
        );

        let claim = state::BotClaimJournal {
            vault_id: 99,
            claim_generation: 42,
            debt_covered_e8s: 100,
            collateral_amount_e8s: 50,
            collateral_received_amount_e8s: Some(49),
            collateral_outbound_fee_e8s: Some(1),
            collateral_price_e8s: 200,
            payment_memo: b"payment".to_vec(),
            collateral_return_memo: b"claim-99-42-return".to_vec(),
            failed_return_attempts: Vec::new(),
            collateral_return: None,
            swap_intents: Vec::new(),
            status: state::BotClaimJournalStatus::SwapMayHaveStarted,
        };
        let mut bot_state = state::BotState::default();
        bot_state.pending_claims.insert(99, claim);
        state::init_state(bot_state);

        // This is the recovery action taken by the unavailable branch. It
        // writes a diagnostic while leaving the pre-swap generation-bound
        // journal available for reconciliation.
        write_unverified_swap_recovery(
            123,
            99,
            456,
            50,
            100,
            10,
            200,
            205,
            250,
            1_000_000,
            "ledger query unavailable",
        );

        state::read_state(|s| {
            let held = s.pending_claims.get(&99).expect("claim remains held");
            assert_eq!(held.claim_generation, 42);
            assert_eq!(
                held.status,
                state::BotClaimJournalStatus::SwapMayHaveStarted
            );
            assert!(held.collateral_return.is_none());
            assert!(!s.pending_payments.contains_key(&99));
        });
        match history::get_record(123).expect("recovery diagnostic is recorded") {
            LiquidationRecordVersioned::V1(record) => {
                assert_eq!(record.status, LiquidationStatus::ConfirmFailed);
                assert_eq!(record.ckusdc_received_e6, 0);
                assert_eq!(record.ckusdc_transferred_e6, 0);
                let message = record.error_message.expect("diagnostic explains the hold");
                assert!(message.contains("router reported 1000000 e6"));
                assert!(message.contains("receipt is unverified"));
                assert!(message.contains("ledger query unavailable"));
            }
        }
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
