//! Durable ordinary stability-pool liquidation rail.
//!
//! The legacy `stability_pool_liquidate` endpoint is intentionally retained
//! for compatibility. This module provides the bounded request-ID rail used
//! by new SP clients: quote and reserve first, persist the exact ICRC-2 tuple,
//! then execute and retain both payment and collateral receipts.

use candid::{CandidType, Deserialize, Nat, Principal};
use ic_cdk::api::call::RejectionCode;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::Memo;
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
use num_traits::ToPrimitive;
use rust_decimal::prelude::{FromPrimitive, ToPrimitive as DecimalToPrimitive};

use crate::management;
use crate::numeric::{ICUSD, UsdIcp};
use crate::icrc3_proof::DecodedBlock;
use crate::state::{
    mutate_state, read_state, SpLiquidationJournal, SpLiquidationPayment,
    SpLiquidationPaymentTuple, SpLiquidationPhase,
};
use crate::{ProtocolError, StableTokenType};

const REQUEST_ID_LEN: usize = 32;
/// A prepared ordinary liquidation may wait briefly for the SP to approve its
/// exact pull. Before the first payment attempt we revalidate the quote and
/// current price; once the durable attempt fence is set, the frozen tuple is
/// never changed by a freshness check or a retry.
pub const SP_LIQUIDATION_QUOTE_TTL_NS: u64 = 5 * 60 * 1_000_000_000;
const SP_LIQUIDATION_PRICE_MAX_AGE_NS: u64 = 10 * 60 * 1_000_000_000;
/// Receipt/tombstone rows are permanent so replay identities are never
/// recycled. Admission is bounded and fails closed when this cap is reached.
pub const MAX_SP_LIQUIDATION_JOURNALS: usize = 2_048;

#[derive(CandidType, Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct SpLiquidationRequest {
    pub request_id: Vec<u8>,
    pub vault_id: u64,
    pub payment: SpLiquidationPayment,
    pub max_debt_e8s: u64,
    pub max_gross_raw: u64,
}

#[derive(CandidType, Clone, Debug, Deserialize, PartialEq, Eq)]
pub enum SpLiquidationPhaseView {
    Prepared,
    PaymentSubmitted,
    PaymentConfirmed,
    DebtCommitted,
    PayoutSubmitted,
    PayoutSettled,
    PaymentNoEffect,
    PayoutNoEffect,
    Cancelled,
}

#[derive(CandidType, Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct SpLiquidationReceipt {
    pub request_id: Vec<u8>,
    pub vault_id: u64,
    pub payment: SpLiquidationPayment,
    pub payment_tuple: SpLiquidationPaymentTuple,
    pub phase: SpLiquidationPhaseView,
    pub debt_e8s: u64,
    pub principal_raw: u64,
    pub surcharge_raw: u64,
    pub pull_fee_raw: u64,
    pub gross_payment_raw: u64,
    pub collateral_ledger: Principal,
    pub collateral_price_e8s: u64,
    /// Serialized rust_decimal price bytes used by the frozen quote. The e8s
    /// projection remains for clients that only need the existing display unit.
    pub collateral_price_decimal: Vec<u8>,
    pub collateral_gross_raw: u64,
    pub collateral_net_raw: u64,
    pub collateral_payout_net_raw: u64,
    pub payout_operation_id: Option<u128>,
    pub payout_tuple: Option<SpLiquidationPayoutTuple>,
    pub payment_block_index: Option<u64>,
    pub payout_block_index: Option<u64>,
    pub payout_net_raw: Option<u64>,
}

/// Exact outgoing collateral transfer tuple. The SP can use this tuple with
/// ICRC-3 history to prove the linked payout independently of the backend's
/// accounting result, including the configured fee and dedup identity.
#[derive(CandidType, Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct SpLiquidationPayoutTuple {
    pub ledger: Principal,
    pub source: Account,
    pub destination: Account,
    pub amount_raw: u64,
    pub gross_raw: u64,
    pub ledger_fee_raw: u64,
    pub memo: Vec<u8>,
    pub created_at_time_ns: u64,
    pub op_nonce: u128,
}

/// Internal quote retained only until it is copied into the stable journal.
pub(crate) struct SpLiquidationQuote {
    pub vault_id: u64,
    pub collateral_type: Principal,
    pub collateral_ledger: Principal,
    pub collateral_decimals: u8,
    pub collateral_price: rust_decimal::Decimal,
    pub collateral_price_e8s: u64,
    pub collateral_price_usd: UsdIcp,
    pub collateral_ledger_fee_raw: u64,
    pub debt_e8s: u64,
    pub collateral_gross_raw: u64,
    pub collateral_net_raw: u64,
    pub protocol_cut_raw: u64,
    pub interest_share_e8s: u64,
}

pub(crate) struct SpLiquidationApplied {
    pub debt_e8s: u64,
    pub interest_share_e8s: u64,
    pub collateral_gross_raw: u64,
    pub collateral_net_raw: u64,
    pub protocol_cut_raw: u64,
    pub payout: Option<(u128, Principal, u64, u64, u64, u128)>,
    pub xrp_claim_id: Option<u64>,
    pub drained: bool,
    pub payment_block_index: u64,
}

fn request_id(request: &[u8]) -> Result<[u8; REQUEST_ID_LEN], ProtocolError> {
    let bytes: [u8; REQUEST_ID_LEN] = request.try_into().map_err(|_| {
        ProtocolError::GenericError("SP liquidation request_id must be exactly 32 bytes".into())
    })?;
    if bytes.iter().all(|byte| *byte == 0) {
        return Err(ProtocolError::GenericError(
            "SP liquidation request_id must be nonzero".into(),
        ));
    }
    Ok(bytes)
}

fn require_sp(caller: Principal) -> Result<(), ProtocolError> {
    if caller == Principal::anonymous() {
        return Err(ProtocolError::AnonymousCallerNotAllowed);
    }
    if read_state(|state| state.stability_pool_canister) != Some(caller) {
        return Err(ProtocolError::GenericError(
            "Caller is not the registered stability pool canister".into(),
        ));
    }
    Ok(())
}

fn request_binding_matches(
    journal_caller: Principal,
    journal_vault_id: u64,
    journal_payment: SpLiquidationPayment,
    journal_max_debt_e8s: u64,
    journal_max_gross_raw: u64,
    caller: Principal,
    request: &SpLiquidationRequest,
) -> bool {
    journal_caller == caller
        && journal_vault_id == request.vault_id
        && journal_payment == request.payment
        && journal_max_debt_e8s == request.max_debt_e8s
        && journal_max_gross_raw == request.max_gross_raw
}

/// A cancellation submitted after a lost prepare reply needs a durable row of
/// its own. The row is deliberately represented by the existing permanent
/// journal map: it carries the complete immutable request binding, but has no
/// executable payment or payout tuple and is terminally `Cancelled`. A late
/// original prepare therefore authenticates to this tombstone and can never
/// create a new reservation after the vault has moved on.
fn cancellation_tombstone(
    key: [u8; REQUEST_ID_LEN],
    caller: Principal,
    request: &SpLiquidationRequest,
    payment_ledger: Principal,
    now: u64,
) -> SpLiquidationJournal {
    SpLiquidationJournal {
        request_id: key,
        caller,
        vault_id: request.vault_id,
        payment: request.payment,
        max_debt_e8s: request.max_debt_e8s,
        max_gross_raw: request.max_gross_raw,
        collateral_type: Principal::anonymous(),
        collateral_ledger: Principal::anonymous(),
        quote_collateral_decimals: 0,
        quote_debt_e8s: 0,
        quote_collateral_gross_raw: 0,
        quote_collateral_net_raw: 0,
        quote_collateral_payout_net_raw: 0,
        quote_protocol_cut_raw: 0,
        quote_interest_share_e8s: 0,
        quote_price_e8s: 0,
        quote_price_usd_bits: 0,
        quote_price_decimal: [0; 16],
        quote_payout_fee_raw: 0,
        quote_payout_fee_known: false,
        principal_raw: 0,
        surcharge_raw: 0,
        gross_payment_raw: 0,
        payment_tuple: SpLiquidationPaymentTuple {
            // The SP owns this immutable value.  Keeping it in the tombstone
            // makes an absent-prepare cancellation authenticate to the same
            // payment identity as a normal request; an anonymous sentinel
            // would be rejected by the SP receipt matcher.
            ledger: payment_ledger,
            source: Account { owner: caller, subaccount: None },
            destination: Account { owner: Principal::anonymous(), subaccount: None },
            amount_raw: 0,
            ledger_fee_raw: 0,
            memo: [0; 16],
            created_at_time_ns: 0,
            op_nonce: 0,
        },
        phase: SpLiquidationPhase::Cancelled,
        payment_block_index: None,
        payout_operation_id: None,
        payout_op_nonce: None,
        payout_ledger: None,
        payout_gross_raw: None,
        payout_net_raw: None,
        payout_fee_raw: None,
        payout_memo: [0; 16],
        payout_created_at_time_ns: 0,
        payout_block_index: None,
        payment_ambiguous: false,
        payment_attempted: false,
        protocol_fee_treasury: None,
        protocol_fee_op_nonce: None,
        protocol_fee_ledger_fee_raw: None,
        protocol_fee_attempted: false,
        protocol_fee_block_index: None,
        interest_dispatch_started: false,
        post_accounting_done: false,
        applied_interest_share_e8s: None,
        created_at_ns: now,
        updated_at_ns: now,
    }
}

fn cancel_bound_in_state(
    state: &mut crate::state::State,
    caller: Principal,
    request: &SpLiquidationRequest,
    payment_ledger: Principal,
    key: [u8; REQUEST_ID_LEN],
    now: u64,
) -> Result<SpLiquidationJournal, ProtocolError> {
    if let Some(current) = state.sp_liquidation_journals.get(&key).cloned() {
        if current.request_id != key
            || !request_binding_matches(
                current.caller,
                current.vault_id,
                current.payment,
                current.max_debt_e8s,
                current.max_gross_raw,
                caller,
                request,
            )
            || current.payment_tuple.ledger != payment_ledger
        {
            return Err(ProtocolError::GenericError(
                "SP liquidation request_id is already bound to different parameters".into(),
            ));
        }
        if current.phase == SpLiquidationPhase::Cancelled {
            return Ok(current);
        }
        if !can_cancel_phase(&current.phase) {
            return Err(ProtocolError::TemporarilyUnavailable(
                "Only an unsubmitted SP liquidation can be cancelled".into(),
            ));
        }
        let mut next = current;
        next.phase = SpLiquidationPhase::Cancelled;
        next.updated_at_ns = now;
        state.sp_liquidation_journals.insert(key, next.clone());
        return Ok(next);
    }
    if state.sp_liquidation_journals.len() >= MAX_SP_LIQUIDATION_JOURNALS {
        return Err(ProtocolError::TemporarilyUnavailable(
            "SP liquidation receipt journal capacity is exhausted".into(),
        ));
    }
    let tombstone = cancellation_tombstone(key, caller, request, payment_ledger, now);
    state.sp_liquidation_journals.insert(key, tombstone.clone());
    Ok(tombstone)
}

/// Look up only an already durable prepare row. This helper performs no
/// admission, quote, fee, oracle, or guard work, so callers can safely use it
/// before those gates to replay an accepted request after a lost reply. A
/// missing row returns `None`; the caller must continue through normal
/// admission. A present row must match every immutable request field.
fn lookup_existing_prepare(
    caller: Principal,
    request: &SpLiquidationRequest,
    key: [u8; REQUEST_ID_LEN],
) -> Result<Option<SpLiquidationReceipt>, ProtocolError> {
    let Some(existing) = read_state(|state| state.sp_liquidation_journals.get(&key).cloned()) else {
        return Ok(None);
    };
    if existing.request_id != key
        || !request_binding_matches(
            existing.caller,
            existing.vault_id,
            existing.payment,
            existing.max_debt_e8s,
            existing.max_gross_raw,
            caller,
            request,
        )
    {
        return Err(ProtocolError::GenericError(
            "SP liquidation request_id is already bound to different parameters".into(),
        ));
    }
    Ok(Some(receipt(&existing)))
}

/// Authenticate and replay an already accepted request before mutable
/// admission gates run. Invalid or unknown IDs return `None` so new requests
/// still pass the ordinary wrapper and module gates in their normal order.
pub fn replay_existing_prepare(
    request: &SpLiquidationRequest,
) -> Result<Option<SpLiquidationReceipt>, ProtocolError> {
    let caller = ic_cdk::api::caller();
    require_sp(caller)?;
    let Ok(key) = request_id(&request.request_id) else {
        return Ok(None);
    };
    lookup_existing_prepare(caller, request, key)
}

fn checked_payment_totals(
    principal_raw: u64,
    surcharge_raw: u64,
    pull_fee_raw: u64,
    max_gross_raw: u64,
) -> Result<(u64, u64), ProtocolError> {
    let pull_amount_raw = principal_raw.checked_add(surcharge_raw).ok_or_else(|| {
        ProtocolError::GenericError("SP liquidation pull exceeds u64".into())
    })?;
    let gross_payment_raw = pull_amount_raw.checked_add(pull_fee_raw).ok_or_else(|| {
        ProtocolError::GenericError("SP liquidation payment exceeds u64".into())
    })?;
    if gross_payment_raw > max_gross_raw {
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: gross_payment_raw,
        });
    }
    Ok((pull_amount_raw, gross_payment_raw))
}

fn exact_payout_net(gross_raw: u64, ledger_fee_raw: u64) -> Result<u64, ProtocolError> {
    gross_raw.checked_sub(ledger_fee_raw).ok_or_else(|| {
        ProtocolError::GenericError(
            "Liquidation collateral cannot cover the exact payout ledger fee".into(),
        )
    })
}

/// Split the seized collateral into the SP leg and the protocol-fee leg. Both
/// legs use the ledger fee charged from their own gross amount, so
/// `(sp_net + sp_fee) + (treasury_net + treasury_fee) == collateral_gross`.
/// A nonzero protocol fee that cannot pay its own ledger fee is rejected while
/// the quote is still fresh; it must never borrow the shared payout balance.
fn collateral_payout_split(
    collateral_gross_raw: u64,
    protocol_cut_raw: u64,
    ledger_fee_raw: u64,
) -> Result<(u64, u64, u64), ProtocolError> {
    let collateral_net_raw = collateral_gross_raw
        .checked_sub(protocol_cut_raw)
        .ok_or_else(|| ProtocolError::GenericError("Protocol collateral cut exceeds seized collateral".into()))?;
    if collateral_net_raw <= ledger_fee_raw {
        return Err(ProtocolError::GenericError(
            "Liquidation collateral cannot cover the exact SP payout ledger fee".into(),
        ));
    }
    let sp_net_raw = exact_payout_net(collateral_net_raw, ledger_fee_raw)?;
    let treasury_net_raw = if protocol_cut_raw == 0 {
        0
    } else {
        if protocol_cut_raw <= ledger_fee_raw {
            return Err(ProtocolError::GenericError(
                "Protocol collateral fee cannot cover its exact payout ledger fee".into(),
            ));
        }
        exact_payout_net(protocol_cut_raw, ledger_fee_raw)?
    };
    Ok((collateral_net_raw, sp_net_raw, treasury_net_raw))
}

fn payment_no_effect_is_terminal(payment_ambiguous: bool, payment_attempted: bool) -> bool {
    !payment_ambiguous && !payment_attempted
}

fn should_dispatch_interest(interest_share_e8s: u64, dispatch_started: bool) -> bool {
    interest_share_e8s > 0 && !dispatch_started
}

fn can_cancel_phase(phase: &SpLiquidationPhase) -> bool {
    matches!(phase, SpLiquidationPhase::Prepared | SpLiquidationPhase::PaymentNoEffect(_))
}

fn effective_payment_fee(payment: SpLiquidationPayment, ledger_fee_raw: u64) -> u64 {
    // Burning icUSD into the backend's configured minting account is the
    // ledger's special ICRC-2 burn path. It always pins an explicit zero fee;
    // the normal `icrc1_fee` is not a valid fee argument for this tuple.
    match payment {
        SpLiquidationPayment::Icusd => 0,
        SpLiquidationPayment::Ckusdc | SpLiquidationPayment::Ckusdt => ledger_fee_raw,
    }
}

/// A candidate block is only a pointer into the configured ledger's history.
/// Once a durable block index has been adopted, a different candidate is a
/// conflict and may never replace the original proof. The equal-index path is
/// the idempotent retry used after an upgrade or a lost reconciliation reply.
fn reconcile_existing_block(
    existing: Option<u64>,
    candidate: u64,
    leg: &str,
) -> Result<bool, ProtocolError> {
    match existing {
        None => Ok(false),
        Some(index) if index == candidate => Ok(true),
        Some(index) => Err(ProtocolError::GenericError(format!(
            "SP liquidation {leg} receipt is already bound to block {index}"
        ))),
    }
}

fn payment_reconciliation_phase_allowed(
    phase: &SpLiquidationPhase,
    attempted: bool,
    block_index: Option<u64>,
) -> bool {
    attempted
        && block_index.is_none()
        && matches!(phase, SpLiquidationPhase::PaymentSubmitted | SpLiquidationPhase::PaymentNoEffect(_))
}

fn post_accounting_reconciliation_phase_allowed(
    phase: &SpLiquidationPhase,
    attempted: bool,
    block_index: Option<u64>,
) -> bool {
    attempted
        && block_index.is_none()
        && matches!(
            phase,
            SpLiquidationPhase::DebtCommitted
                | SpLiquidationPhase::PayoutSubmitted
                | SpLiquidationPhase::PayoutNoEffect(_)
        )
}

fn payout_reconciliation_phase_allowed(
    phase: &SpLiquidationPhase,
    block_index: Option<u64>,
) -> bool {
    block_index.is_none()
        && matches!(phase, SpLiquidationPhase::PayoutSubmitted | SpLiquidationPhase::PayoutNoEffect(_))
}

fn payout_first_dispatch_required(phase: &SpLiquidationPhase) -> bool {
    matches!(phase, SpLiquidationPhase::DebtCommitted)
}

fn validate_exact_payment_block(
    block: &DecodedBlock,
    payment: SpLiquidationPayment,
    tuple: &SpLiquidationPaymentTuple,
) -> Result<(), String> {
    match payment {
        SpLiquidationPayment::Icusd => {
            // The configured icUSD minting account is the ledger's burn
            // destination. Its canonical ICRC-3 record is a transfer-from
            // burn: 2burn/burn, source=SP, spender=backend, no `to`, and a
            // fee that is either omitted or explicitly zero.
            if block.btype.as_deref() != Some("2burn") || block.op != "burn" {
                return Err("SP icUSD proof is not the canonical ICRC-2 burn block".into());
            }
            if block.to.is_some() {
                return Err("SP icUSD burn proof unexpectedly contains a destination".into());
            }
            if block.fee.is_some_and(|fee| fee != 0) {
                return Err("SP icUSD burn proof records a nonzero fee".into());
            }
        }
        SpLiquidationPayment::Ckusdc | SpLiquidationPayment::Ckusdt => {
            if block.btype.as_deref() != Some("2xfer") || block.op != "xfer" {
                return Err("SP stable payment proof is not an ICRC-2 transfer-from block".into());
            }
            if block.to != Some(tuple.destination.clone()) {
                return Err("SP payment proof recipient does not match the frozen destination".into());
            }
            if block.fee != Some(effective_payment_fee(payment, tuple.ledger_fee_raw)) {
                return Err("SP payment proof fee does not match the frozen fee".into());
            }
        }
    }
    if block.from != Some(tuple.source.clone()) {
        return Err("SP payment proof sender does not match the frozen source".into());
    }
    if block.spender != Some(tuple.destination.clone()) {
        return Err("SP payment proof spender does not match the backend".into());
    }
    if block.amount != u128::from(tuple.amount_raw) {
        return Err("SP payment proof amount does not match the frozen pull".into());
    }
    if block.memo.as_deref() != Some(tuple.memo.as_slice()) {
        return Err("SP payment proof memo does not match the frozen nonce".into());
    }
    if block.created_at_time != Some(tuple.created_at_time_ns) {
        return Err("SP payment proof timestamp does not match the frozen nonce".into());
    }
    Ok(())
}

fn validate_exact_ledger_transfer_block(
    block: &DecodedBlock,
    from: Account,
    to: Account,
    amount_raw: u64,
    fee_raw: u64,
    memo: &[u8],
    created_at_time_ns: u64,
) -> Result<(), String> {
    if block.btype.as_deref() != Some("1xfer") {
        return Err("SP collateral proof is not an ICRC-1 transfer block".into());
    }
    crate::icrc3_proof::validate_icrc3_transfer_block(
        block,
        Some(from),
        to,
        amount_raw,
        Some(memo),
        Some(created_at_time_ns),
    )?;
    if block.fee != Some(fee_raw) {
        return Err("SP transfer proof fee does not match the frozen fee".into());
    }
    Ok(())
}

fn validate_native_icp_transfer_block(
    block: &crate::native_icp_proof::Block,
    from: Account,
    to: Account,
    amount_raw: u64,
    fee_raw: u64,
    memo: &[u8],
    created_at_time_ns: u64,
) -> Result<(), String> {
    if from.subaccount.is_some() || to.subaccount.is_some() {
        return Err("native ICP exact collateral proof requires default accounts".into());
    }
    let observed_fee = crate::native_icp_proof::verify_transfer_block(
        block,
        from.owner,
        to.owner,
        amount_raw,
        memo,
        created_at_time_ns,
    )?;
    if observed_fee != fee_raw {
        return Err("native ICP transfer proof fee does not match the frozen fee".into());
    }
    Ok(())
}

/// Verify an exact outbound collateral transfer receipt using the proof
/// protocol exposed by the configured ledger. The canonical ICP ledger does
/// not expose ICRC-3, so it must be read through its query_blocks path and
/// checked with the native transfer verifier. All other collateral ledgers
/// retain the bounded ICRC-3 proof path.
async fn validate_exact_collateral_transfer(
    ledger: Principal,
    block_index: u64,
    from: Account,
    to: Account,
    amount_raw: u64,
    fee_raw: u64,
    memo: &[u8],
    created_at_time_ns: u64,
) -> Result<(), String> {
    if crate::native_icp_proof::is_native_icp_ledger(ledger) {
        let block = crate::native_icp_proof::query_block(ledger, block_index).await?;
        return validate_native_icp_transfer_block(
            &block,
            from,
            to,
            amount_raw,
            fee_raw,
            memo,
            created_at_time_ns,
        );
    }

    let block_value = crate::icrc3_proof::fetch_icrc3_block_value(ledger, block_index).await?;
    let block = crate::icrc3_proof::decode_block(&block_value)
        .map_err(|error| format!("malformed collateral ICRC-3 transfer block: {error}"))?;
    validate_exact_ledger_transfer_block(
        &block,
        from,
        to,
        amount_raw,
        fee_raw,
        memo,
        created_at_time_ns,
    )
}

fn reconciliation_caller_allowed(caller: Principal) -> bool {
    caller != Principal::anonymous()
        && read_state(|state| {
            state_reconciliation_caller_allowed(state, caller)
        })
}

fn state_reconciliation_caller_allowed(
    state: &crate::state::State,
    caller: Principal,
) -> bool {
    caller != Principal::anonymous()
        && (state.stability_pool_canister == Some(caller)
            || state.developer_principal == caller)
}

fn require_reconciliation_caller(caller: Principal) -> Result<(), ProtocolError> {
    if caller == Principal::anonymous() {
        return Err(ProtocolError::AnonymousCallerNotAllowed);
    }
    if !reconciliation_caller_allowed(caller) {
        return Err(ProtocolError::GenericError(
            "Only the registered stability pool or developer may reconcile this exact liquidation journal".into(),
        ));
    }
    Ok(())
}

fn payment_gross_for_debt(
    state: &crate::state::State,
    payment: SpLiquidationPayment,
    debt_e8s: u64,
    pull_fee_raw: u64,
) -> Result<u64, ProtocolError> {
    let (_, principal_raw, surcharge_raw) = payment_ledger_and_surcharge(state, payment, debt_e8s)?;
    let pull_fee_raw = effective_payment_fee(payment, pull_fee_raw);
    checked_payment_totals(principal_raw, surcharge_raw, pull_fee_raw, u64::MAX)
        .map(|(_, gross)| gross)
}

/// Find the largest principal debt whose rounded stable pull and explicit
/// ledger fee fit the SP's gross budget. This inversion runs before the
/// economic liquidation quote, so a rounded stable amount can never produce a
/// quote that exceeds the caller's bound after admission.
fn max_affordable_debt(
    state: &crate::state::State,
    payment: SpLiquidationPayment,
    max_debt_e8s: u64,
    max_gross_raw: u64,
    pull_fee_raw: u64,
) -> Result<u64, ProtocolError> {
    let mut low = 0u64;
    let mut high = max_debt_e8s;
    while low < high {
        let candidate = low + (high - low) / 2 + 1;
        if payment_gross_for_debt(state, payment, candidate, pull_fee_raw)? <= max_gross_raw {
            low = candidate;
        } else {
            high = candidate - 1;
        }
    }
    if low == 0 {
        let minimum_amount = payment_gross_for_debt(state, payment, 1, pull_fee_raw)?;
        return Err(ProtocolError::AmountTooLow { minimum_amount });
    }
    Ok(low)
}

/// Authenticate the registered Stability Pool before any wrapper performs an
/// await or spends canister resources. The operation methods repeat this
/// check after their durable journal lookup because caller identity remains a
/// request invariant across every retry.
pub fn validate_caller() -> Result<(), ProtocolError> {
    require_sp(ic_cdk::api::caller())
}

fn phase_view(phase: &SpLiquidationPhase) -> SpLiquidationPhaseView {
    match phase {
        SpLiquidationPhase::Prepared => SpLiquidationPhaseView::Prepared,
        SpLiquidationPhase::PaymentSubmitted => SpLiquidationPhaseView::PaymentSubmitted,
        SpLiquidationPhase::PaymentConfirmed => SpLiquidationPhaseView::PaymentConfirmed,
        SpLiquidationPhase::DebtCommitted => SpLiquidationPhaseView::DebtCommitted,
        SpLiquidationPhase::PayoutSubmitted => SpLiquidationPhaseView::PayoutSubmitted,
        SpLiquidationPhase::PayoutSettled => SpLiquidationPhaseView::PayoutSettled,
        SpLiquidationPhase::PaymentNoEffect(_) => SpLiquidationPhaseView::PaymentNoEffect,
        SpLiquidationPhase::PayoutNoEffect(_) => SpLiquidationPhaseView::PayoutNoEffect,
        SpLiquidationPhase::Cancelled => SpLiquidationPhaseView::Cancelled,
    }
}

fn receipt(journal: &SpLiquidationJournal) -> SpLiquidationReceipt {
    SpLiquidationReceipt {
        request_id: journal.request_id.to_vec(),
        vault_id: journal.vault_id,
        payment: journal.payment,
        payment_tuple: journal.payment_tuple.clone(),
        phase: phase_view(&journal.phase),
        debt_e8s: journal.quote_debt_e8s,
        principal_raw: journal.principal_raw,
        surcharge_raw: journal.surcharge_raw,
        pull_fee_raw: journal.payment_tuple.ledger_fee_raw,
        gross_payment_raw: journal.gross_payment_raw,
        collateral_ledger: journal.collateral_ledger,
        collateral_price_e8s: journal.quote_price_e8s,
        collateral_price_decimal: journal.quote_price_decimal.to_vec(),
        collateral_gross_raw: journal.quote_collateral_gross_raw,
        collateral_net_raw: journal.quote_collateral_net_raw,
        collateral_payout_net_raw: if journal.quote_collateral_payout_net_raw > 0 {
            journal.quote_collateral_payout_net_raw
        } else {
            journal
                .quote_collateral_net_raw
                .saturating_sub(journal.quote_payout_fee_raw)
        },
        payout_operation_id: journal.payout_operation_id,
        payout_tuple: journal.payout_operation_id.and_then(|_operation_id| {
            let (ledger, gross_raw, amount_raw, ledger_fee_raw, op_nonce) = (
                journal.payout_ledger?,
                journal.payout_gross_raw?,
                journal.payout_net_raw?,
                journal.payout_fee_raw?,
                journal.payout_op_nonce?,
            );
            Some(SpLiquidationPayoutTuple {
                ledger,
                source: Account { owner: ic_cdk::id(), subaccount: None },
                destination: Account { owner: journal.caller, subaccount: None },
                amount_raw,
                gross_raw,
                ledger_fee_raw,
                memo: if journal.payout_memo == [0; 16] {
                    management::nonce_to_memo(op_nonce).0.to_vec()
                } else {
                    journal.payout_memo.to_vec()
                },
                created_at_time_ns: if journal.payout_created_at_time_ns == 0 {
                    management::nonce_to_created_at_time(op_nonce)
                } else {
                    journal.payout_created_at_time_ns
                },
                op_nonce,
            })
        }),
        payment_block_index: journal.payment_block_index,
        payout_block_index: journal.payout_block_index,
        payout_net_raw: journal.payout_net_raw,
    }
}

fn payment_ledger_and_surcharge(
    state: &crate::state::State,
    payment: SpLiquidationPayment,
    debt_e8s: u64,
) -> Result<(Principal, u64, u64), ProtocolError> {
    match payment {
        SpLiquidationPayment::Icusd => {
            if state.icusd_ledger_principal == Principal::anonymous() {
                return Err(ProtocolError::GenericError("icUSD ledger is not configured".into()));
            }
            Ok((state.icusd_ledger_principal, debt_e8s, 0))
        }
        SpLiquidationPayment::Ckusdc => {
            if !state.ckusdc_enabled {
                return Err(ProtocolError::GenericError("ckUSDC liquidation rail is disabled".into()));
            }
            let ledger = state.ckusdc_ledger_principal.ok_or_else(|| {
                ProtocolError::GenericError("ckUSDC ledger is not configured".into())
            })?;
            let (base, surcharge, _total) = crate::vault::stable_repay_pull_e6s(
                ICUSD::new(debt_e8s),
                state.ckstable_repay_fee,
            )?;
            Ok((ledger, base, surcharge))
        }
        SpLiquidationPayment::Ckusdt => {
            if !state.ckusdt_enabled {
                return Err(ProtocolError::GenericError("ckUSDT liquidation rail is disabled".into()));
            }
            let ledger = state.ckusdt_ledger_principal.ok_or_else(|| {
                ProtocolError::GenericError("ckUSDT ledger is not configured".into())
            })?;
            let (base, surcharge, _total) = crate::vault::stable_repay_pull_e6s(
                ICUSD::new(debt_e8s),
                state.ckstable_repay_fee,
            )?;
            Ok((ledger, base, surcharge))
        }
    }
}

fn quote_from_journal(
    state: &crate::state::State,
    journal: &SpLiquidationJournal,
) -> Result<SpLiquidationQuote, ProtocolError> {
    let vault = state.vault_id_to_vaults.get(&journal.vault_id).ok_or_else(|| {
        ProtocolError::TemporarilyUnavailable("Reserved vault is no longer present".into())
    })?;
    let config = state.get_collateral_config(&journal.collateral_type).ok_or_else(|| {
        ProtocolError::TemporarilyUnavailable("Reserved collateral configuration is missing".into())
    })?;
    if vault.collateral_type != journal.collateral_type {
        return Err(ProtocolError::TemporarilyUnavailable(
            "Reserved vault collateral changed".into(),
        ));
    }
    if journal.collateral_ledger != Principal::anonymous()
        && config.ledger_canister_id != journal.collateral_ledger
    {
        return Err(ProtocolError::TemporarilyUnavailable(
            "Reserved collateral ledger changed".into(),
        ));
    }
    let price_decimal = if journal.quote_price_decimal != [0; 16] {
        rust_decimal::Decimal::deserialize(journal.quote_price_decimal)
    } else {
        let price = f64::from_bits(journal.quote_price_usd_bits);
        rust_decimal::Decimal::from_f64_retain(price).ok_or_else(|| {
            ProtocolError::TemporarilyUnavailable("Frozen collateral price cannot be reconstructed".into())
        })?
    };
    Ok(SpLiquidationQuote {
        vault_id: journal.vault_id,
        collateral_type: journal.collateral_type,
        collateral_ledger: if journal.collateral_ledger == Principal::anonymous() {
            config.ledger_canister_id
        } else {
            journal.collateral_ledger
        },
        collateral_decimals: if journal.quote_collateral_decimals == 0 {
            config.decimals
        } else {
            journal.quote_collateral_decimals
        },
        collateral_price: price_decimal,
        collateral_price_e8s: journal.quote_price_e8s,
        collateral_price_usd: UsdIcp::from(price_decimal),
        collateral_ledger_fee_raw: if journal.quote_payout_fee_known {
            journal.quote_payout_fee_raw
        } else {
            config.ledger_fee
        },
        debt_e8s: journal.quote_debt_e8s,
        collateral_gross_raw: journal.quote_collateral_gross_raw,
        collateral_net_raw: journal.quote_collateral_net_raw,
        protocol_cut_raw: journal.quote_protocol_cut_raw,
        interest_share_e8s: journal.quote_interest_share_e8s,
    })
}

/// Persist a quote and exact incoming payment tuple before any irreversible
/// ledger operation.
pub async fn prepare(request: SpLiquidationRequest) -> Result<SpLiquidationReceipt, ProtocolError> {
    let caller = ic_cdk::api::caller();
    require_sp(caller)?;
    let key = request_id(&request.request_id)?;
    if request.max_debt_e8s == 0 || request.max_gross_raw == 0 {
        return Err(ProtocolError::GenericError("SP liquidation limits must be nonzero".into()));
    }
    if let Some(existing) = lookup_existing_prepare(caller, &request, key)? {
        return Ok(existing);
    }
    if read_state(|state| state.frozen || state.liquidation_frozen) {
        return Err(ProtocolError::TemporarilyUnavailable(
            "Liquidations are currently frozen".into(),
        ));
    }
    // Stable rails retain the ordinary liquidation depeg gate. It is part of
    // preparation, before the quote or payment tuple can become executable,
    // so a stale or out-of-band ckStable price never gets reserved.
    match request.payment {
        SpLiquidationPayment::Ckusdc => {
            crate::xrc::ensure_stable_not_depegged(&StableTokenType::CKUSDC).await?
        }
        SpLiquidationPayment::Ckusdt => {
            crate::xrc::ensure_stable_not_depegged(&StableTokenType::CKUSDT).await?
        }
        SpLiquidationPayment::Icusd => {}
    }
    let _guard = crate::guard::VaultLiquidationGuard::new(request.vault_id)?;
    let (initial_ledger, _, _) = read_state(|state| {
        payment_ledger_and_surcharge(state, request.payment, request.max_debt_e8s)
    })?;
    let normal_pull_fee_raw = management::get_ledger_fee(initial_ledger)
        .await
        .map_err(|error| ProtocolError::TemporarilyUnavailable(format!("payment fee unavailable: {error}")))?;
    let pull_fee_raw = effective_payment_fee(request.payment, normal_pull_fee_raw);
    let affordable_debt = read_state(|state| {
        max_affordable_debt(
            state,
            request.payment,
            request.max_debt_e8s,
            request.max_gross_raw,
            pull_fee_raw,
        )
    })?;
    let quote = read_state(|state| {
        crate::vault::quote_sp_liquidation(
            state,
            request.vault_id,
            ICUSD::new(affordable_debt),
        )
    })
    .map_err(ProtocolError::GenericError)?;
    let actual_payout_fee_raw = management::get_ledger_fee(quote.collateral_ledger)
        .await
        .map_err(|error| ProtocolError::TemporarilyUnavailable(format!("collateral payout fee unavailable: {error}")))?;
    if actual_payout_fee_raw != quote.collateral_ledger_fee_raw {
        return Err(ProtocolError::TemporarilyUnavailable(
            "Collateral ledger fee changed while preparing the SP liquidation".into(),
        ));
    }
    let (quoted_collateral_net_raw, collateral_payout_net_raw, _protocol_fee_net_raw) =
        collateral_payout_split(
            quote.collateral_gross_raw,
            quote.protocol_cut_raw,
            actual_payout_fee_raw,
        )?;
    if quoted_collateral_net_raw != quote.collateral_net_raw {
        return Err(ProtocolError::TemporarilyUnavailable(
            "Liquidation collateral split changed while preparing the SP liquidation".into(),
        ));
    }
    let (ledger, principal_raw, surcharge_raw) = read_state(|state| {
        payment_ledger_and_surcharge(state, request.payment, quote.debt_e8s)
    })?;
    if ledger != initial_ledger {
        return Err(ProtocolError::GenericError(
            "Payment ledger changed while preparing the SP liquidation".into(),
        ));
    }
    let (pull_amount_raw, gross_payment_raw) = checked_payment_totals(
        principal_raw,
        surcharge_raw,
        pull_fee_raw,
        request.max_gross_raw,
    )?;
    let protocol_fee_treasury = if quote.protocol_cut_raw > 0 {
        Some(read_state(|state| state.treasury_principal).ok_or_else(|| {
            ProtocolError::TemporarilyUnavailable(
                "Treasury is not configured for the quoted liquidation fee".into(),
            )
        })?)
    } else {
        None
    };
    // This check is intentionally repeated after the fee and quote awaits.
    // ReadOnly remains a valid distressed-liquidation mode, while either
    // explicit freeze must close admission before the durable row exists.
    if read_state(|state| state.frozen || state.liquidation_frozen) {
        return Err(ProtocolError::TemporarilyUnavailable(
            "Liquidations were frozen while preparing the SP liquidation".into(),
        ));
    }
    if read_state(|state| state.get_collateral_config(&quote.collateral_type)
        .is_some_and(|config| config.status == crate::state::CollateralStatus::Sunset)) {
        return Err(ProtocolError::GenericError(
            "Sunset collateral requires manual liquidation".into(),
        ));
    }
    // Only the incoming payment is dispatched from the prepared quote. The
    // protocol-fee leg is created later, immediately before its first ledger
    // await, so a long-held payment/recovery journal cannot age that later
    // tuple into TooOld before it is ever attempted.
    let op_nonce = mutate_state(|state| state.next_op_nonce());
    let tuple = SpLiquidationPaymentTuple {
        ledger,
        source: Account { owner: caller, subaccount: None },
        destination: Account { owner: ic_cdk::id(), subaccount: None },
        amount_raw: pull_amount_raw,
        ledger_fee_raw: pull_fee_raw,
        memo: management::nonce_to_memo(op_nonce).0.as_slice().try_into().unwrap_or([0; 16]),
        created_at_time_ns: management::nonce_to_created_at_time(op_nonce),
        op_nonce,
    };
    let now = ic_cdk::api::time();
    let journal = SpLiquidationJournal {
        request_id: key,
        caller,
        vault_id: request.vault_id,
        payment: request.payment,
        max_debt_e8s: request.max_debt_e8s,
        max_gross_raw: request.max_gross_raw,
        collateral_type: quote.collateral_type,
        collateral_ledger: quote.collateral_ledger,
        quote_collateral_decimals: quote.collateral_decimals,
        quote_debt_e8s: quote.debt_e8s,
        quote_collateral_gross_raw: quote.collateral_gross_raw,
        quote_collateral_net_raw: quote.collateral_net_raw,
        quote_collateral_payout_net_raw: collateral_payout_net_raw,
        quote_protocol_cut_raw: quote.protocol_cut_raw,
        quote_interest_share_e8s: quote.interest_share_e8s,
        quote_price_e8s: quote.collateral_price_e8s,
        quote_price_usd_bits: quote.collateral_price.to_f64().unwrap_or(0.0).to_bits(),
        quote_price_decimal: quote.collateral_price.serialize(),
        quote_payout_fee_raw: actual_payout_fee_raw,
        quote_payout_fee_known: true,
        principal_raw,
        surcharge_raw,
        gross_payment_raw,
        payment_tuple: tuple,
        phase: SpLiquidationPhase::Prepared,
        payment_block_index: None,
        payout_operation_id: None,
        payout_op_nonce: None,
        payout_ledger: None,
        payout_gross_raw: None,
        payout_net_raw: None,
        payout_fee_raw: None,
        payout_memo: [0; 16],
        payout_created_at_time_ns: 0,
        payout_block_index: None,
        payment_ambiguous: false,
        payment_attempted: false,
        protocol_fee_treasury,
        protocol_fee_op_nonce: None,
        protocol_fee_ledger_fee_raw: (quote.protocol_cut_raw > 0)
            .then_some(actual_payout_fee_raw),
        protocol_fee_attempted: false,
        protocol_fee_block_index: None,
        interest_dispatch_started: false,
        post_accounting_done: false,
        applied_interest_share_e8s: None,
        created_at_ns: now,
        updated_at_ns: now,
    };
    let inserted = mutate_state(|state| {
        if state.sp_liquidation_journals.contains_key(&key) {
            return false;
        }
        if state.sp_liquidation_journals.len() >= MAX_SP_LIQUIDATION_JOURNALS {
            return false;
        }
        state.sp_liquidation_journals.insert(key, journal.clone());
        true
    });
    if !inserted {
        return Err(ProtocolError::TemporarilyUnavailable(
            "SP liquidation receipt journal capacity is exhausted or request changed while preparing".into(),
        ));
    }
    Ok(receipt(&journal))
}

#[derive(Debug)]
enum PullOutcome {
    Confirmed(u64),
    NoEffect(String),
    Ambiguous(String),
}

async fn dispatch_exact_pull(
    payment: SpLiquidationPayment,
    tuple: &SpLiquidationPaymentTuple,
) -> PullOutcome {
    let args = TransferFromArgs {
        spender_subaccount: None,
        from: tuple.source.clone(),
        to: tuple.destination.clone(),
        amount: Nat::from(tuple.amount_raw),
        fee: Some(Nat::from(effective_payment_fee(payment, tuple.ledger_fee_raw))),
        created_at_time: Some(tuple.created_at_time_ns),
        memo: Some(Memo::from(tuple.memo.to_vec())),
    };
    let result: Result<(Result<Nat, TransferFromError>,), (RejectionCode, String)> =
        ic_cdk::call(tuple.ledger, "icrc2_transfer_from", (args,)).await;
    match result {
        Err((code, message)) => PullOutcome::Ambiguous(format!("ledger rejected ({code:?}): {message}")),
        Ok((Ok(block),)) => block.0.to_u64().map(PullOutcome::Confirmed).unwrap_or_else(|| {
            PullOutcome::Ambiguous("payment block index exceeds u64".into())
        }),
        Ok((Err(TransferFromError::Duplicate { duplicate_of }),)) => duplicate_of
            .0
            .to_u64()
            .map(PullOutcome::Confirmed)
            .unwrap_or_else(|| PullOutcome::Ambiguous("duplicate block index exceeds u64".into())),
        Ok((Err(error),)) if matches!(
            error,
            TransferFromError::BadFee { .. }
                | TransferFromError::BadBurn { .. }
                | TransferFromError::InsufficientFunds { .. }
                | TransferFromError::InsufficientAllowance { .. }
                | TransferFromError::CreatedInFuture { .. }
        ) => PullOutcome::NoEffect(format!("payment had no effect: {error:?}")),
        Ok((Err(error),)) => PullOutcome::Ambiguous(format!("payment outcome is ambiguous: {error:?}")),
    }
}

fn mark_payment_submitted(key: [u8; 32], caller: Principal) -> Result<SpLiquidationJournal, ProtocolError> {
    mutate_state(|state| {
        let current = state.sp_liquidation_journals.get(&key).cloned().ok_or_else(|| {
            ProtocolError::GenericError("Unknown SP liquidation request_id".into())
        })?;
        if current.caller != caller {
            return Err(ProtocolError::GenericError("SP liquidation request owner mismatch".into()));
        }
        if matches!(current.phase, SpLiquidationPhase::Prepared) {
            let mut next = current.clone();
            next.phase = SpLiquidationPhase::PaymentSubmitted;
            next.updated_at_ns = ic_cdk::api::time();
            state.sp_liquidation_journals.insert(key, next.clone());
            Ok(next)
        } else {
            Ok(current)
        }
    })
}

fn mark_payment_attempted(
    key: [u8; 32],
    caller: Principal,
) -> Result<SpLiquidationJournal, ProtocolError> {
    mutate_state(|state| {
        let current = state.sp_liquidation_journals.get(&key).cloned().ok_or_else(|| {
            ProtocolError::GenericError("Unknown SP liquidation request_id".into())
        })?;
        if current.caller != caller {
            return Err(ProtocolError::GenericError("SP liquidation request owner mismatch".into()));
        }
        if current.payment_attempted {
            return Ok(current);
        }
        let mut next = current;
        next.payment_attempted = true;
        next.updated_at_ns = ic_cdk::api::time();
        state.sp_liquidation_journals.insert(key, next.clone());
        Ok(next)
    })
}

/// Revalidate a prepared quote immediately before its first external payment
/// call. A changed price, a healthy vault, an explicit freeze, or a bounded
/// TTL expiry terminalizes the unsubmitted request as no-effect. Once
/// `payment_attempted` is true this helper is never called, so an unknown
/// payment outcome always resumes with the exact frozen tuple.
fn validate_predispatch_quote(journal: &SpLiquidationJournal) -> Result<(), String> {
    let now = ic_cdk::api::time();
    read_state(|state| validate_predispatch_quote_at(state, journal, now))
}

fn validate_predispatch_quote_at(state: &crate::state::State, journal: &SpLiquidationJournal, now: u64) -> Result<(), String> {
    if now < journal.created_at_ns
        || now.saturating_sub(journal.created_at_ns) > SP_LIQUIDATION_QUOTE_TTL_NS
    {
        return Err("SP liquidation quote expired before payment dispatch".into());
    }
    if state.frozen || state.liquidation_frozen {
        return Err("liquidations are frozen before payment dispatch".into());
    }
    let config = state
        .get_collateral_config(&journal.collateral_type)
        .ok_or_else(|| "reserved collateral configuration is missing".to_string())?;
    if config.status == crate::state::CollateralStatus::Sunset {
        return Err("Sunset collateral requires manual liquidation".into());
    }
    let timestamp = config
        .last_price_timestamp
        .ok_or_else(|| "reserved collateral price has no freshness timestamp".to_string())?;
    if timestamp > now || now.saturating_sub(timestamp) > SP_LIQUIDATION_PRICE_MAX_AGE_NS {
        return Err("reserved collateral price is stale before payment dispatch".into());
    }
    let current = crate::vault::quote_sp_liquidation(
        state,
        journal.vault_id,
        ICUSD::new(journal.quote_debt_e8s),
    )?;
    if current.collateral_type != journal.collateral_type
        || current.collateral_ledger != journal.collateral_ledger
        || current.collateral_price != {
            if journal.quote_price_decimal != [0; 16] {
                rust_decimal::Decimal::deserialize(journal.quote_price_decimal)
            } else {
                rust_decimal::Decimal::from_f64_retain(f64::from_bits(journal.quote_price_usd_bits))
                    .ok_or_else(|| "frozen collateral price cannot be reconstructed".to_string())?
            }
        }
        || current.debt_e8s != journal.quote_debt_e8s
        || current.collateral_gross_raw != journal.quote_collateral_gross_raw
        || current.collateral_net_raw != journal.quote_collateral_net_raw
        || current.protocol_cut_raw != journal.quote_protocol_cut_raw
    {
        return Err("SP liquidation quote changed before payment dispatch".into());
    }
    Ok(())
}

fn cancel_predispatch_quote(key: [u8; 32], caller: Principal, reason: String) -> bool {
    mutate_state(|state| {
        let Some(current) = state.sp_liquidation_journals.get_mut(&key) else {
            return false;
        };
        if current.caller != caller
            || current.payment_attempted
            || current.payment_block_index.is_some()
            || !matches!(current.phase, SpLiquidationPhase::Prepared | SpLiquidationPhase::PaymentSubmitted)
        {
            return false;
        }
        current.phase = SpLiquidationPhase::PaymentNoEffect(reason);
        current.updated_at_ns = ic_cdk::api::time();
        true
    })
}

async fn settle_post_accounting(
    key: [u8; 32],
    journal: &SpLiquidationJournal,
) -> Result<(), ProtocolError> {
    if journal.post_accounting_done {
        return Ok(());
    }
    let applied_interest_share_e8s = journal.applied_interest_share_e8s.unwrap_or(0);
    let dispatch_interest = should_dispatch_interest(
        applied_interest_share_e8s,
        journal.interest_dispatch_started,
    );
    if dispatch_interest {
        // The treasury helpers admit their per-leg journals synchronously
        // before their first await. This fence prevents a resumed execute
        // from allocating a second set of interest identities; admitted rows
        // remain available to the treasury retry worker after an upgrade.
        mutate_state(|state| {
            if let Some(current) = state.sp_liquidation_journals.get_mut(&key) {
                current.interest_dispatch_started = true;
                current.updated_at_ns = ic_cdk::api::time();
            }
        });
        match journal.payment {
            SpLiquidationPayment::Icusd => {
                let remaining = crate::treasury::distribute_interest(
                    ICUSD::new(applied_interest_share_e8s),
                    journal.collateral_type,
                )
                .await;
                if remaining.to_u64() > 0 {
                    mutate_state(|state| {
                        state.restore_pending_interest_for_pool(
                            journal.collateral_type,
                            remaining.to_u64(),
                        )
                    });
                }
            }
            SpLiquidationPayment::Ckusdc => {
                crate::treasury::distribute_stablecoin_interest(
                    applied_interest_share_e8s,
                    journal.collateral_type,
                    StableTokenType::CKUSDC,
                    journal.payment_block_index.unwrap_or_default(),
                    journal.payment_tuple.ledger,
                )
                .await;
            }
            SpLiquidationPayment::Ckusdt => {
                crate::treasury::distribute_stablecoin_interest(
                    applied_interest_share_e8s,
                    journal.collateral_type,
                    StableTokenType::CKUSDT,
                    journal.payment_block_index.unwrap_or_default(),
                    journal.payment_tuple.ledger,
                )
                .await;
            }
        }
    }
    if journal.quote_protocol_cut_raw > 0 {
        // The protocol-fee tuple is created immediately before its first
        // ledger await, after any interest-distribution await. Once the
        // attempted fence is set, every retry reuses that exact nonce and
        // timestamp; a pre-dispatch quote can never age into a TooOld tuple.
        let tuple = mutate_state(|state| -> Result<Option<(Principal, u64, u128)>, ProtocolError> {
            let snapshot = state.sp_liquidation_journals.get(&key).cloned().ok_or_else(|| {
                ProtocolError::TemporarilyUnavailable("SP liquidation journal disappeared".into())
            })?;
            if snapshot.protocol_fee_block_index.is_some() {
                return Ok(None);
            }
            let treasury = snapshot.protocol_fee_treasury.ok_or_else(|| {
                ProtocolError::TemporarilyUnavailable(
                    "SP liquidation treasury destination is missing".into(),
                )
            })?;
            let fee = snapshot.protocol_fee_ledger_fee_raw.ok_or_else(|| {
                ProtocolError::TemporarilyUnavailable(
                    "SP liquidation treasury fee tuple is missing".into(),
                )
            })?;
            let op_nonce = if snapshot.protocol_fee_attempted {
                snapshot.protocol_fee_op_nonce.ok_or_else(|| {
                    ProtocolError::TemporarilyUnavailable(
                        "SP liquidation treasury nonce is missing after dispatch".into(),
                    )
                })?
            } else {
                let nonce = state.next_op_nonce();
                let current = state.sp_liquidation_journals.get_mut(&key).ok_or_else(|| {
                    ProtocolError::TemporarilyUnavailable("SP liquidation journal disappeared".into())
                })?;
                current.protocol_fee_op_nonce = Some(nonce);
                current.protocol_fee_attempted = true;
                current.updated_at_ns = ic_cdk::api::time();
                nonce
            };
            Ok(Some((treasury, fee, op_nonce)))
        })?;
        if let Some((treasury, fee, op_nonce)) = tuple {
            let protocol_fee_net_raw = exact_payout_net(journal.quote_protocol_cut_raw, fee)?;
            match crate::management::transfer_collateral_with_nonce_and_fee_status(
                protocol_fee_net_raw,
                Some(fee),
                treasury,
                journal.collateral_ledger,
                op_nonce,
            ).await {
            Ok(block_index) => {
                let should_notify = mutate_state(|state| {
                    if let Some(current) = state.sp_liquidation_journals.get_mut(&key) {
                        // A concurrent positive-proof reconciliation may have
                        // committed this same tuple while the transfer reply
                        // was in flight. Never overwrite a different durable
                        // proof and never clear a proof on a retry.
                        let should_notify = current.protocol_fee_block_index.is_none();
                        if should_notify {
                            current.protocol_fee_block_index = Some(block_index);
                        }
                        current.updated_at_ns = ic_cdk::api::time();
                        should_notify
                    } else {
                        false
                    }
                });
                // Preserve the existing treasury bookkeeping callback. Its
                // failure is non-critical; the exact ledger receipt above is
                // the durable proof of the fee transfer.
                if should_notify {
                    let _ = crate::treasury::notify_treasury_deposit(
                        treasury,
                        crate::treasury::DepositType::LiquidationFee,
                        crate::treasury::collateral_to_asset_type(&journal.collateral_type),
                        protocol_fee_net_raw,
                        block_index,
                    ).await;
                }
            }
            Err(error) => {
                return Err(ProtocolError::TemporarilyUnavailable(format!(
                    "SP treasury fee remains unresolved; retry the same request: {error:?}"
                )));
            }
        }
        }
    }
    mutate_state(|state| {
        let Some(current) = state.sp_liquidation_journals.get_mut(&key) else { return false };
        if current.caller != journal.caller || current.phase != SpLiquidationPhase::DebtCommitted {
            return false;
        }
        current.post_accounting_done = true;
        current.updated_at_ns = ic_cdk::api::time();
        true
    });
    Ok(())
}

async fn settle_payout(
    key: [u8; 32],
    caller: Principal,
) -> Result<SpLiquidationReceipt, ProtocolError> {
    let journal = read_state(|state| state.sp_liquidation_journals.get(&key).cloned())
        .ok_or_else(|| ProtocolError::GenericError("Unknown SP liquidation request_id".into()))?;
    if journal.caller != caller {
        return Err(ProtocolError::GenericError("SP liquidation request owner mismatch".into()));
    }
    if journal.phase == SpLiquidationPhase::PayoutSettled {
        return Ok(receipt(&journal));
    }
    let Some(operation_id) = journal.payout_operation_id else {
        return Err(ProtocolError::TemporarilyUnavailable("SP payout operation is not linked yet".into()));
    };
    let (journal, transfer) = mutate_state(|state| {
        let current = state.sp_liquidation_journals.get(&key).cloned().ok_or_else(|| {
            ProtocolError::GenericError("Unknown SP liquidation request_id".into())
        })?;
        if current.caller != caller
            || current.payout_operation_id != Some(operation_id)
            || !matches!(
                current.phase,
                SpLiquidationPhase::DebtCommitted
                    | SpLiquidationPhase::PayoutSubmitted
                    | SpLiquidationPhase::PayoutNoEffect(_)
            )
        {
            return Err(ProtocolError::TemporarilyUnavailable(
                "SP payout is not dispatchable in the current journal phase".into(),
            ));
        }
        if state.get_pending_payout(operation_id).is_none() {
            return Err(ProtocolError::TemporarilyUnavailable(
                "SP payout receipt is missing".into(),
            ));
        }

        let mut next = current;
        if payout_first_dispatch_required(&next.phase) {
            // Debt commit reserves the payout operation and keeps its pending
            // row in-flight, but the actual ledger tuple is frozen only here,
            // immediately before the first payout await. This avoids a stale
            // created_at_time after a long interest/proof recovery delay.
            let op_nonce = state.next_op_nonce();
            next.payout_op_nonce = Some(op_nonce);
            next.payout_memo = management::nonce_to_memo(op_nonce)
                .0
                .as_slice()
                .try_into()
                .unwrap_or([0; 16]);
            next.payout_created_at_time_ns = management::nonce_to_created_at_time(op_nonce);
        }
        next.phase = SpLiquidationPhase::PayoutSubmitted;
        next.updated_at_ns = ic_cdk::api::time();
        state.sp_liquidation_journals.insert(key, next.clone());
        state.mutate_pending_payout(operation_id, |row| {
            row.op_nonce = next.payout_op_nonce.unwrap_or(row.op_nonce);
            row.in_flight = true;
        });
        let transfer = state
            .get_pending_payout(operation_id)
            .map(|(_, transfer)| transfer)
            .ok_or_else(|| {
                ProtocolError::TemporarilyUnavailable("SP payout receipt disappeared".into())
            })?;
        Ok((next, transfer))
    })?;
    let amount = transfer.transfer_amount_raw.ok_or_else(|| {
        ProtocolError::TemporarilyUnavailable("SP payout net amount is missing".into())
    })?;
    let ledger = transfer.ledger.ok_or_else(|| {
        ProtocolError::TemporarilyUnavailable("SP payout ledger is missing".into())
    })?;
    if journal.payout_ledger != Some(ledger)
        || journal.payout_net_raw != Some(amount)
        || journal.payout_op_nonce != Some(transfer.op_nonce)
    {
        return Err(ProtocolError::TemporarilyUnavailable(
            "SP payout row does not match its frozen journal tuple".into(),
        ));
    }
    let payout = crate::management::transfer_collateral_with_nonce_and_fee_status(
        amount,
        journal.payout_fee_raw,
        transfer.owner,
        ledger,
        transfer.op_nonce,
    )
    .await;
    match payout {
        Ok(block_index) => {
            let committed = mutate_state(|state| {
                let Some(current) = state.sp_liquidation_journals.get_mut(&key) else { return false };
                if current.caller != caller || current.payout_operation_id != Some(operation_id) {
                    return false;
                }
                current.phase = SpLiquidationPhase::PayoutSettled;
                current.payout_block_index = Some(block_index);
                current.payout_net_raw = Some(amount);
                current.updated_at_ns = ic_cdk::api::time();
                crate::event::record_margin_transfer(
                    state,
                    transfer.vault_id,
                    transfer.owner,
                    operation_id,
                    transfer.payout_kind,
                    block_index,
                );
                true
            });
            if !committed {
                return Err(ProtocolError::TemporarilyUnavailable(
                    "SP payout landed but its receipt commit is still pending".into(),
                ));
            }
            Ok(read_state(|state| receipt(state.sp_liquidation_journals.get(&key).unwrap())))
        }
        Err(crate::management::DurableTransferError::LedgerNoEffect(error)) => {
            mutate_state(|state| {
                if let Some(current) = state.sp_liquidation_journals.get_mut(&key) {
                    current.phase = SpLiquidationPhase::PayoutNoEffect(format!("{error:?}"));
                    current.updated_at_ns = ic_cdk::api::time();
                }
                state.mutate_pending_payout(operation_id, |row| {
                    row.in_flight = false;
                    row.held_for_manual_retry = true;
                });
            });
            Err(ProtocolError::GenericError("SP collateral payout had no effect; exact tuple remains held".into()))
        }
        Err(error) => Err(ProtocolError::TemporarilyUnavailable(format!(
            "SP collateral payout is unresolved; retry the exact request: {error:?}"
        ))),
    }
}

/// Adopt a positive ICRC-3 proof for the exact incoming SP pull after its
/// original transfer-from reply was lost or became TooOld. The candidate is
/// checked against the immutable request tuple and the journal CAS is repeated
/// after the ledger await. No absence, balance, or new-nonce claim can resolve
/// this rail.
pub async fn reconcile_payment(
    request_id_arg: Vec<u8>,
    candidate_block_index: u64,
) -> Result<SpLiquidationReceipt, ProtocolError> {
    let caller = ic_cdk::api::caller();
    require_reconciliation_caller(caller)?;
    let key = request_id(&request_id_arg)?;
    let journal = read_state(|state| state.sp_liquidation_journals.get(&key).cloned())
        .ok_or_else(|| ProtocolError::GenericError("Unknown SP liquidation request_id".into()))?;
    if reconcile_existing_block(journal.payment_block_index, candidate_block_index, "payment")? {
        return Ok(receipt(&journal));
    }
    if !payment_reconciliation_phase_allowed(
        &journal.phase,
        journal.payment_attempted,
        journal.payment_block_index,
    ) {
        return Err(ProtocolError::TemporarilyUnavailable(
            "SP payment proof is not admissible for the current journal phase".into(),
        ));
    }
    let block = crate::icrc3_proof::fetch_icrc3_block(
        journal.payment_tuple.ledger,
        candidate_block_index,
    )
    .await
    .map_err(|error| {
        ProtocolError::TemporarilyUnavailable(format!(
            "SP payment proof could not be fetched: {error}"
        ))
    })?;
    validate_exact_payment_block(&block, journal.payment, &journal.payment_tuple).map_err(
        |error| {
            ProtocolError::TemporarilyUnavailable(format!(
                "SP payment proof does not match the frozen tuple: {error}"
            ))
        },
    )?;

    let committed = mutate_state(|state| {
        if !state_reconciliation_caller_allowed(state, caller) {
            return Err(ProtocolError::GenericError(
                "SP payment reconciliation caller is no longer authorized".into(),
            ));
        }
        let current = state.sp_liquidation_journals.get_mut(&key).ok_or_else(|| {
            ProtocolError::GenericError("Unknown SP liquidation request_id".into())
        })?;
        if let Some(existing) = current.payment_block_index {
            if existing == candidate_block_index {
                return Ok(current.clone());
            }
            return Err(ProtocolError::GenericError(
                "SP payment receipt is already bound to a different block".into(),
            ));
        }
        if current.request_id != journal.request_id
            || current.caller != journal.caller
            || current.payment != journal.payment
            || current.payment_tuple != journal.payment_tuple
            || !payment_reconciliation_phase_allowed(
                &current.phase,
                current.payment_attempted,
                current.payment_block_index,
            )
        {
            return Err(ProtocolError::TemporarilyUnavailable(
                "SP payment journal changed while its proof was being fetched".into(),
            ));
        }
        current.payment_block_index = Some(candidate_block_index);
        current.phase = SpLiquidationPhase::PaymentConfirmed;
        current.updated_at_ns = ic_cdk::api::time();
        Ok(current.clone())
    })?;
    Ok(receipt(&committed))
}

/// Adopt a positive ledger proof for the frozen protocol-fee transfer. The
/// exact treasury, gross/net amount, fee, memo, timestamp, and ledger are all
/// read from the durable journal; mutable treasury configuration is not used.
pub async fn reconcile_protocol_fee(
    request_id_arg: Vec<u8>,
    candidate_block_index: u64,
) -> Result<SpLiquidationReceipt, ProtocolError> {
    let caller = ic_cdk::api::caller();
    require_reconciliation_caller(caller)?;
    let key = request_id(&request_id_arg)?;
    let journal = read_state(|state| state.sp_liquidation_journals.get(&key).cloned())
        .ok_or_else(|| ProtocolError::GenericError("Unknown SP liquidation request_id".into()))?;
    if reconcile_existing_block(
        journal.protocol_fee_block_index,
        candidate_block_index,
        "protocol fee",
    )? {
        return Ok(receipt(&journal));
    }
    if journal.quote_protocol_cut_raw == 0
        || !post_accounting_reconciliation_phase_allowed(
            &journal.phase,
            journal.protocol_fee_attempted,
            journal.protocol_fee_block_index,
        )
    {
        return Err(ProtocolError::TemporarilyUnavailable(
            "SP protocol-fee proof is not admissible for the current journal phase".into(),
        ));
    }
    let treasury = journal.protocol_fee_treasury.ok_or_else(|| {
        ProtocolError::TemporarilyUnavailable(
            "SP protocol-fee proof has no frozen treasury destination".into(),
        )
    })?;
    let fee = journal.protocol_fee_ledger_fee_raw.ok_or_else(|| {
        ProtocolError::TemporarilyUnavailable(
            "SP protocol-fee proof has no frozen ledger fee".into(),
        )
    })?;
    let op_nonce = journal.protocol_fee_op_nonce.ok_or_else(|| {
        ProtocolError::TemporarilyUnavailable(
            "SP protocol-fee proof has no frozen operation nonce".into(),
        )
    })?;
    let amount = exact_payout_net(journal.quote_protocol_cut_raw, fee)?;
    let memo = management::nonce_to_memo(op_nonce).0.to_vec();
    let created_at_time_ns = management::nonce_to_created_at_time(op_nonce);
    validate_exact_collateral_transfer(
        journal.collateral_ledger,
        candidate_block_index,
        Account {
            owner: ic_cdk::id(),
            subaccount: None,
        },
        Account {
            owner: treasury,
            subaccount: None,
        },
        amount,
        fee,
        &memo,
        created_at_time_ns,
    )
    .await
    .map_err(|error| {
        ProtocolError::TemporarilyUnavailable(format!(
            "SP protocol-fee proof does not match the frozen tuple: {error}"
        ))
    })?;

    let committed = mutate_state(|state| {
        if !state_reconciliation_caller_allowed(state, caller) {
            return Err(ProtocolError::GenericError(
                "SP protocol-fee reconciliation caller is no longer authorized".into(),
            ));
        }
        let current = state.sp_liquidation_journals.get_mut(&key).ok_or_else(|| {
            ProtocolError::GenericError("Unknown SP liquidation request_id".into())
        })?;
        if let Some(existing) = current.protocol_fee_block_index {
            if existing == candidate_block_index {
                return Ok(current.clone());
            }
            return Err(ProtocolError::GenericError(
                "SP protocol-fee receipt is already bound to a different block".into(),
            ));
        }
        if current.request_id != journal.request_id
            || current.caller != journal.caller
            || current.protocol_fee_treasury != journal.protocol_fee_treasury
            || current.protocol_fee_op_nonce != journal.protocol_fee_op_nonce
            || current.protocol_fee_ledger_fee_raw != journal.protocol_fee_ledger_fee_raw
            || current.quote_protocol_cut_raw != journal.quote_protocol_cut_raw
            || current.collateral_ledger != journal.collateral_ledger
            || !post_accounting_reconciliation_phase_allowed(
                &current.phase,
                current.protocol_fee_attempted,
                current.protocol_fee_block_index,
            )
        {
            return Err(ProtocolError::TemporarilyUnavailable(
                "SP protocol-fee journal changed while its proof was being fetched".into(),
            ));
        }
        current.protocol_fee_block_index = Some(candidate_block_index);
        current.updated_at_ns = ic_cdk::api::time();
        Ok(current.clone())
    })?;
    Ok(receipt(&committed))
}

#[derive(Clone)]
struct FrozenPayoutTuple {
    operation_id: u128,
    ledger: Principal,
    gross_raw: u64,
    net_raw: u64,
    fee_raw: u64,
    memo: [u8; 16],
    created_at_time_ns: u64,
    op_nonce: u128,
}

fn frozen_payout_tuple(journal: &SpLiquidationJournal) -> Result<FrozenPayoutTuple, ProtocolError> {
    let operation_id = journal.payout_operation_id.ok_or_else(|| {
        ProtocolError::TemporarilyUnavailable("SP payout operation is not linked yet".into())
    })?;
    let ledger = journal.payout_ledger.ok_or_else(|| {
        ProtocolError::TemporarilyUnavailable("SP payout ledger is missing".into())
    })?;
    let gross_raw = journal.payout_gross_raw.ok_or_else(|| {
        ProtocolError::TemporarilyUnavailable("SP payout gross amount is missing".into())
    })?;
    let net_raw = journal.payout_net_raw.ok_or_else(|| {
        ProtocolError::TemporarilyUnavailable("SP payout net amount is missing".into())
    })?;
    let fee_raw = journal.payout_fee_raw.ok_or_else(|| {
        ProtocolError::TemporarilyUnavailable("SP payout ledger fee is missing".into())
    })?;
    let op_nonce = journal.payout_op_nonce.ok_or_else(|| {
        ProtocolError::TemporarilyUnavailable("SP payout operation nonce is missing".into())
    })?;
    if operation_id == 0 || op_nonce == 0 || journal.payout_memo == [0; 16]
        || journal.payout_created_at_time_ns == 0
    {
        return Err(ProtocolError::TemporarilyUnavailable(
            "SP payout exact tuple is incomplete".into(),
        ));
    }
    if gross_raw.checked_sub(fee_raw) != Some(net_raw) {
        return Err(ProtocolError::TemporarilyUnavailable(
            "SP payout gross, net, and fee tuple is inconsistent".into(),
        ));
    }
    Ok(FrozenPayoutTuple {
        operation_id,
        ledger,
        gross_raw,
        net_raw,
        fee_raw,
        memo: journal.payout_memo,
        created_at_time_ns: journal.payout_created_at_time_ns,
        op_nonce,
    })
}

fn pending_payout_matches_frozen_tuple(
    transfer: &crate::state::PendingMarginTransfer,
    journal: &SpLiquidationJournal,
    frozen: &FrozenPayoutTuple,
) -> bool {
    transfer.operation_id == frozen.operation_id
        && transfer.vault_id == journal.vault_id
        && transfer.payout_kind == crate::state::PendingPayoutKind::Margin
        && transfer.owner == journal.caller
        && transfer.ledger == Some(frozen.ledger)
        && transfer.transfer_amount_raw == Some(frozen.net_raw)
        && transfer.op_nonce == frozen.op_nonce
}

/// Adopt a positive ledger proof for the outgoing SP collateral transfer. If
/// the generic payout worker already removed the pending row, the journal's
/// immutable operation tuple is sufficient to reattach the settled receipt;
/// if the row remains, journal, event, and pending-row removal commit together.
pub async fn reconcile_payout(
    request_id_arg: Vec<u8>,
    candidate_block_index: u64,
) -> Result<SpLiquidationReceipt, ProtocolError> {
    let caller = ic_cdk::api::caller();
    require_reconciliation_caller(caller)?;
    let key = request_id(&request_id_arg)?;
    let journal = read_state(|state| state.sp_liquidation_journals.get(&key).cloned())
        .ok_or_else(|| ProtocolError::GenericError("Unknown SP liquidation request_id".into()))?;
    if reconcile_existing_block(journal.payout_block_index, candidate_block_index, "payout")? {
        return Ok(receipt(&journal));
    }
    if !journal.post_accounting_done
        || !payout_reconciliation_phase_allowed(&journal.phase, journal.payout_block_index)
    {
        return Err(ProtocolError::TemporarilyUnavailable(
            "SP payout proof is not admissible before post-accounting is committed".into(),
        ));
    }
    let frozen = frozen_payout_tuple(&journal)?;
    let pending = read_state(|state| state.get_pending_payout(frozen.operation_id));
    if let Some((_, transfer)) = &pending {
        if !pending_payout_matches_frozen_tuple(transfer, &journal, &frozen) {
            return Err(ProtocolError::TemporarilyUnavailable(
                "SP payout pending row does not match the frozen tuple".into(),
            ));
        }
    }
    validate_exact_collateral_transfer(
        frozen.ledger,
        candidate_block_index,
        Account {
            owner: ic_cdk::id(),
            subaccount: None,
        },
        Account {
            owner: journal.caller,
            subaccount: None,
        },
        frozen.net_raw,
        frozen.fee_raw,
        &frozen.memo,
        frozen.created_at_time_ns,
    )
    .await
    .map_err(|error| {
        ProtocolError::TemporarilyUnavailable(format!(
            "SP payout proof does not match the frozen tuple: {error}"
        ))
    })?;

    let committed = mutate_state(|state| {
        if !state_reconciliation_caller_allowed(state, caller) {
            return Err(ProtocolError::GenericError(
                "SP payout reconciliation caller is no longer authorized".into(),
            ));
        }
        let current_snapshot = state.sp_liquidation_journals.get(&key).cloned().ok_or_else(|| {
            ProtocolError::GenericError("Unknown SP liquidation request_id".into())
        })?;
        if let Some(existing) = current_snapshot.payout_block_index {
            if existing == candidate_block_index {
                return Ok(current_snapshot);
            }
            return Err(ProtocolError::GenericError(
                "SP payout receipt is already bound to a different block".into(),
            ));
        }
        if current_snapshot.request_id != journal.request_id
            || current_snapshot.caller != journal.caller
            || current_snapshot.post_accounting_done != journal.post_accounting_done
            || current_snapshot.payout_operation_id != Some(frozen.operation_id)
            || current_snapshot.payout_ledger != Some(frozen.ledger)
            || current_snapshot.payout_gross_raw != Some(frozen.gross_raw)
            || current_snapshot.payout_net_raw != Some(frozen.net_raw)
            || current_snapshot.payout_fee_raw != Some(frozen.fee_raw)
            || current_snapshot.payout_memo != frozen.memo
            || current_snapshot.payout_created_at_time_ns != frozen.created_at_time_ns
            || current_snapshot.payout_op_nonce != Some(frozen.op_nonce)
            || !payout_reconciliation_phase_allowed(
                &current_snapshot.phase,
                current_snapshot.payout_block_index,
            )
        {
            return Err(ProtocolError::TemporarilyUnavailable(
                "SP payout journal changed while its proof was being fetched".into(),
            ));
        }
        let pending = state.get_pending_payout(frozen.operation_id);
        let pending = pending.map(|(kind, transfer)| (kind, transfer));
        if let Some((_, transfer)) = &pending {
            if !pending_payout_matches_frozen_tuple(transfer, &current_snapshot, &frozen) {
                return Err(ProtocolError::TemporarilyUnavailable(
                    "SP payout pending row changed while its proof was being fetched".into(),
                ));
            }
        }
        let committed = {
            let current = state.sp_liquidation_journals.get_mut(&key).ok_or_else(|| {
                ProtocolError::GenericError("Unknown SP liquidation request_id".into())
            })?;
            current.phase = SpLiquidationPhase::PayoutSettled;
            current.payout_block_index = Some(candidate_block_index);
            current.payout_net_raw = Some(frozen.net_raw);
            current.updated_at_ns = ic_cdk::api::time();
            current.clone()
        };
        if let Some((kind, transfer)) = pending {
            crate::event::record_margin_transfer(
                state,
                transfer.vault_id,
                transfer.owner,
                frozen.operation_id,
                kind,
                candidate_block_index,
            );
        }
        Ok(committed)
    })?;
    Ok(receipt(&committed))
}

/// Execute or resume one exact prepared request. No fresh nonce is allocated
/// after the first dispatch fence.
pub async fn execute(request_id_arg: Vec<u8>) -> Result<SpLiquidationReceipt, ProtocolError> {
    let caller = ic_cdk::api::caller();
    require_sp(caller)?;
    let key = request_id(&request_id_arg)?;
    let journal = read_state(|state| state.sp_liquidation_journals.get(&key).cloned())
        .ok_or_else(|| ProtocolError::GenericError("Unknown SP liquidation request_id".into()))?;
    if journal.caller != caller {
        return Err(ProtocolError::GenericError("SP liquidation request owner mismatch".into()));
    }
    let _guard = crate::guard::VaultLiquidationGuard::new_for_sp_liquidation_retry(
        journal.vault_id,
        caller,
        key,
    )?;
    if matches!(journal.phase, SpLiquidationPhase::PayoutSettled | SpLiquidationPhase::Cancelled | SpLiquidationPhase::PaymentNoEffect(_)) {
        return Ok(receipt(&journal));
    }
    if matches!(journal.phase, SpLiquidationPhase::DebtCommitted | SpLiquidationPhase::PayoutSubmitted | SpLiquidationPhase::PayoutNoEffect(_)) {
        settle_post_accounting(key, &journal).await?;
        return settle_payout(key, caller).await;
    }
    if !journal.payment_attempted && journal.payment_block_index.is_none() {
        if let Err(reason) = validate_predispatch_quote(&journal) {
            let _ = cancel_predispatch_quote(key, caller, reason.clone());
            return Err(ProtocolError::GenericError(reason));
        }
    }
    let submitted = mark_payment_submitted(key, caller)?;
    let payment_block = if let Some(block_index) = submitted.payment_block_index {
        block_index
    } else {
        let had_prior_attempt = submitted.payment_attempted;
        let dispatched = mark_payment_attempted(key, caller)?;
        match dispatch_exact_pull(dispatched.payment, &dispatched.payment_tuple).await {
            PullOutcome::Confirmed(block_index) => {
                let _ = mutate_state(|state| {
                    state.sp_liquidation_journals.get_mut(&key).map(|current| {
                        current.payment_block_index = Some(block_index);
                        current.phase = SpLiquidationPhase::PaymentConfirmed;
                        current.updated_at_ns = ic_cdk::api::time();
                    })
                });
                block_index
            }
            PullOutcome::NoEffect(reason) => {
                let terminal_no_effect = payment_no_effect_is_terminal(
                    submitted.payment_ambiguous,
                    had_prior_attempt,
                );
                mutate_state(|state| {
                    if let Some(current) = state.sp_liquidation_journals.get_mut(&key) {
                        if terminal_no_effect {
                            current.phase = SpLiquidationPhase::PaymentNoEffect(reason);
                        }
                        current.updated_at_ns = ic_cdk::api::time();
                    }
                });
                if !terminal_no_effect {
                    return Err(ProtocolError::TemporarilyUnavailable(
                        "SP payment remains unresolved after a prior ambiguous dispatch; retry the same request".into(),
                    ));
                }
                return Err(ProtocolError::GenericError("SP payment had no effect; request is terminal".into()));
            }
            PullOutcome::Ambiguous(reason) => {
                mutate_state(|state| {
                    if let Some(current) = state.sp_liquidation_journals.get_mut(&key) {
                        current.payment_ambiguous = true;
                        current.updated_at_ns = ic_cdk::api::time();
                    }
                });
                return Err(ProtocolError::TemporarilyUnavailable(format!(
                    "SP payment outcome is unresolved; retry the same request: {reason}"
                )));
            }
        }
    };
    let committed = mutate_state(|state| {
        let current = state.sp_liquidation_journals.get(&key).cloned().ok_or_else(|| {
            ProtocolError::GenericError("SP liquidation journal disappeared".into())
        })?;
        let quote = quote_from_journal(state, &current)?;
        let applied = crate::vault::apply_sp_liquidation_in_state(state, &quote, caller, payment_block)
            .map_err(ProtocolError::TemporarilyUnavailable)?;

        // The stable rails pull principal plus the existing surcharge. Keep
        // the surcharge reserve keyed to the exact inbound receipt in the
        // same state transition as debt/collateral accounting. This is the
        // same source identity used by the legacy stable liquidation route;
        // a reply loss or upgrade therefore cannot lose the reserve or admit
        // it under a fresh block/nonce on retry.
        let stable_fee_asset = match current.payment {
            SpLiquidationPayment::Ckusdc => Some(crate::treasury::AssetType::CKUSDC),
            SpLiquidationPayment::Ckusdt => Some(crate::treasury::AssetType::CKUSDT),
            SpLiquidationPayment::Icusd => None,
        };
        if let Some(asset) = stable_fee_asset {
            let frozen_treasury = state.treasury_principal;
            crate::treasury::admit_treasury_stable_transfer_in_state(
                state,
                crate::state::TreasuryStableTransferSource {
                    ledger: current.payment_tuple.ledger,
                    block_index: payment_block,
                    lane: "stable_liquidation_surcharge".into(),
                    slot: 0,
                },
                current.payment_tuple.ledger,
                frozen_treasury,
                current.surcharge_raw,
                asset,
                crate::treasury::DepositType::LiquidationFee,
            );
        }
        let committed = {
            let Some(next) = state.sp_liquidation_journals.get_mut(&key) else {
                return Err(ProtocolError::TemporarilyUnavailable("SP journal disappeared during commit".into()));
            };
            next.phase = SpLiquidationPhase::DebtCommitted;
            next.payment_block_index = Some(payment_block);
            next.applied_interest_share_e8s = Some(applied.interest_share_e8s);
            next.payout_operation_id = applied.payout.as_ref().map(|row| row.0);
            next.payout_ledger = applied.payout.as_ref().map(|row| row.1);
            next.payout_gross_raw = applied.payout.as_ref().map(|row| row.2);
            next.payout_net_raw = applied.payout.as_ref().map(|row| row.3);
            next.payout_fee_raw = applied.payout.as_ref().map(|row| row.4);
            // The generic pending row is created with an operation nonce for
            // worker compatibility, but the SP rail must not expose or use
            // that tuple before its own first payout dispatch. The exact
            // nonce, memo, and timestamp are frozen by `settle_payout`
            // immediately before its first ledger await.
            next.payout_op_nonce = None;
            next.payout_memo = [0; 16];
            next.payout_created_at_time_ns = 0;
            next.updated_at_ns = ic_cdk::api::time();
            next.clone()
        };
        if let Some(operation_id) = committed.payout_operation_id {
            // Reserve the payout against the generic worker in the same
            // synchronous state transition as the debt result. The worker
            // skips `in_flight` rows, so a timer cannot spend this receipt
            // between the commit and the next await below.
            state.mutate_pending_payout(operation_id, |row| row.in_flight = true);
        }
        Ok(committed)
    })?;
    settle_post_accounting(key, &committed).await?;
    settle_payout(key, caller).await
}

pub fn status(request_id_arg: Vec<u8>) -> Result<SpLiquidationReceipt, ProtocolError> {
    let caller = ic_cdk::api::caller();
    require_sp(caller)?;
    let key = request_id(&request_id_arg)?;
    let journal = read_state(|state| state.sp_liquidation_journals.get(&key).cloned())
        .ok_or_else(|| ProtocolError::GenericError("Unknown SP liquidation request_id".into()))?;
    if journal.caller != caller {
        return Err(ProtocolError::GenericError("SP liquidation request owner mismatch".into()));
    }
    Ok(receipt(&journal))
}

/// Cancel an ordinary request using its complete immutable binding. If the
/// prepare row is absent (for example, its reply was lost before the backend
/// persisted it), this creates a permanent cancellation tombstone. The
/// tombstone consumes the same bounded journal capacity as a normal request,
/// so an absent-ID cancellation cannot grow unbounded state or be replayed as
/// a fresh admission later.
pub fn cancel_bound(
    request: SpLiquidationRequest,
    payment_ledger: Principal,
) -> Result<SpLiquidationReceipt, ProtocolError> {
    let caller = ic_cdk::api::caller();
    require_sp(caller)?;
    let key = request_id(&request.request_id)?;
    let journal = mutate_state(|state| {
        cancel_bound_in_state(
            state,
            caller,
            &request,
            payment_ledger,
            key,
            ic_cdk::api::time(),
        )
    })?;
    Ok(receipt(&journal))
}

pub fn cancel(request_id_arg: Vec<u8>) -> Result<SpLiquidationReceipt, ProtocolError> {
    let caller = ic_cdk::api::caller();
    require_sp(caller)?;
    let key = request_id(&request_id_arg)?;
    let journal = mutate_state(|state| {
        let current = state.sp_liquidation_journals.get(&key).cloned().ok_or_else(|| {
            ProtocolError::GenericError("Unknown SP liquidation request_id".into())
        })?;
        if current.caller != caller {
            return Err(ProtocolError::GenericError("SP liquidation request owner mismatch".into()));
        }
        if current.phase == SpLiquidationPhase::Cancelled {
            return Ok(current);
        }
        if !can_cancel_phase(&current.phase) {
            return Err(ProtocolError::TemporarilyUnavailable(
                "Only an unsubmitted SP liquidation can be cancelled".into(),
            ));
        }
        let mut next = current;
        next.phase = SpLiquidationPhase::Cancelled;
        next.updated_at_ns = ic_cdk::api::time();
        state.sp_liquidation_journals.insert(key, next.clone());
        Ok(next)
    })?;
    Ok(receipt(&journal))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(payment: SpLiquidationPayment) -> SpLiquidationRequest {
        SpLiquidationRequest {
            request_id: vec![7; REQUEST_ID_LEN],
            vault_id: 41,
            payment,
            max_debt_e8s: 12_345,
            max_gross_raw: 50_000,
        }
    }

    #[test]
    fn request_id_is_immutable_to_owner_and_quote_parameters() {
        let owner = Principal::from_slice(&[1, 2, 3]);
        let other = Principal::from_slice(&[4, 5, 6]);
        let original = request(SpLiquidationPayment::Ckusdc);

        assert!(request_binding_matches(
            owner,
            original.vault_id,
            original.payment,
            original.max_debt_e8s,
            original.max_gross_raw,
            owner,
            &original,
        ));
        assert!(!request_binding_matches(
            owner,
            original.vault_id,
            original.payment,
            original.max_debt_e8s,
            original.max_gross_raw,
            other,
            &original,
        ));

        let mut changed_payment = original.clone();
        changed_payment.payment = SpLiquidationPayment::Icusd;
        assert!(!request_binding_matches(
            owner,
            original.vault_id,
            original.payment,
            original.max_debt_e8s,
            original.max_gross_raw,
            owner,
            &changed_payment,
        ));

        let mut changed_budget = original.clone();
        changed_budget.max_gross_raw += 1;
        assert!(!request_binding_matches(
            owner,
            original.vault_id,
            original.payment,
            original.max_debt_e8s,
            original.max_gross_raw,
            owner,
            &changed_budget,
        ));
    }

    #[test]
    fn exact_existing_request_replay_is_independent_of_freeze_gate() {
        let owner = Principal::from_slice(&[1, 2, 3]);
        let original = request(SpLiquidationPayment::Ckusdc);
        let mut changed = original.clone();
        changed.max_debt_e8s += 1;

        // The wrapper's pre-gate lookup uses only this immutable binding. A
        // frozen protocol must replay the exact row, while a mismatch remains
        // a conflict and never becomes an admission or a new journal.
        for _frozen in [false, true] {
            assert!(request_binding_matches(
                owner,
                original.vault_id,
                original.payment,
                original.max_debt_e8s,
                original.max_gross_raw,
                owner,
                &original,
            ));
            assert!(!request_binding_matches(
                owner,
                original.vault_id,
                original.payment,
                original.max_debt_e8s,
                original.max_gross_raw,
                owner,
                &changed,
            ));
        }
    }

    #[test]
    fn absent_bound_cancel_is_a_terminal_request_bound_tombstone() {
        let caller = Principal::from_slice(&[12, 13, 14]);
        let original = request(SpLiquidationPayment::Ckusdc);
        let key = request_id(&original.request_id).unwrap();
        let mut state = crate::state::State::default();

        let payment_ledger = Principal::from_slice(&[75]);
        let tombstone = cancel_bound_in_state(
            &mut state,
            caller,
            &original,
            payment_ledger,
            key,
            77,
        )
            .expect("an absent request can be durably cancelled by the registered SP");
        assert_eq!(tombstone.request_id, key);
        assert_eq!(tombstone.caller, caller);
        assert_eq!(tombstone.vault_id, original.vault_id);
        assert_eq!(tombstone.payment, original.payment);
        assert_eq!(tombstone.max_debt_e8s, original.max_debt_e8s);
        assert_eq!(tombstone.max_gross_raw, original.max_gross_raw);
        assert_eq!(tombstone.payment_tuple.ledger, payment_ledger);
        assert_eq!(tombstone.phase, SpLiquidationPhase::Cancelled);
        assert_eq!(state.sp_liquidation_journals.len(), 1);

        let mut conflicting = original.clone();
        conflicting.max_gross_raw += 1;
        let conflict = cancel_bound_in_state(
            &mut state,
            caller,
            &conflicting,
            payment_ledger,
            key,
            78,
        );
        assert!(conflict.is_err(), "a changed immutable tuple must conflict");
        let ledger_conflict = cancel_bound_in_state(
            &mut state,
            caller,
            &original,
            Principal::from_slice(&[77]),
            key,
            79,
        );
        assert!(ledger_conflict.is_err(), "a changed frozen payment ledger must conflict");

        // A delayed original prepare sees the durable cancellation before any
        // quote, guard, oracle, or mutable admission work and cannot create a
        // replacement row.
        crate::state::replace_state(state);
        let replay = lookup_existing_prepare(caller, &original, key)
            .unwrap()
            .expect("the exact delayed prepare must replay the tombstone");
        assert_eq!(replay.phase, SpLiquidationPhaseView::Cancelled);
    }

    #[test]
    fn absent_bound_cancel_is_idempotent_and_capacity_bounded() {
        let caller = Principal::from_slice(&[15]);
        let original = request(SpLiquidationPayment::Icusd);
        let key = request_id(&original.request_id).unwrap();
        let mut state = crate::state::State::default();
        let payment_ledger = Principal::from_slice(&[76]);
        let first = cancel_bound_in_state(
            &mut state,
            caller,
            &original,
            payment_ledger,
            key,
            100,
        )
        .unwrap();
        let second = cancel_bound_in_state(
            &mut state,
            caller,
            &original,
            payment_ledger,
            key,
            101,
        )
        .unwrap();
        assert_eq!(first, second, "repeated cancellation must not mutate the tombstone");

        for index in 1..MAX_SP_LIQUIDATION_JOURNALS {
            let mut request = original.clone();
            let mut request_key = [0; REQUEST_ID_LEN];
            request_key[..8].copy_from_slice(&(index as u64).to_be_bytes());
            request.request_id = request_key.to_vec();
            let key = request_id(&request.request_id).unwrap();
            cancel_bound_in_state(
                &mut state,
                caller,
                &request,
                payment_ledger,
                key,
                102,
            )
            .unwrap();
        }
        assert_eq!(state.sp_liquidation_journals.len(), MAX_SP_LIQUIDATION_JOURNALS);

        let mut exhausted = original;
        exhausted.request_id = vec![0xaa; REQUEST_ID_LEN];
        let exhausted_key = request_id(&exhausted.request_id).unwrap();
        let error = cancel_bound_in_state(
            &mut state,
            caller,
            &exhausted,
            payment_ledger,
            exhausted_key,
            103,
        )
            .expect_err("bounded cancellation tombstones must fail closed at capacity");
        assert!(matches!(error, ProtocolError::TemporarilyUnavailable(_)));
        assert_eq!(state.sp_liquidation_journals.len(), MAX_SP_LIQUIDATION_JOURNALS);
    }

    #[test]
    fn stable_payment_totals_include_surcharge_and_explicit_pull_fee_once() {
        let (pull_amount, gross) = checked_payment_totals(1_000_000, 25_000, 10, 1_025_010)
            .expect("the exact stable payment tuple should fit the budget");
        assert_eq!(pull_amount, 1_025_000);
        assert_eq!(gross, 1_025_010);

        assert!(matches!(
            checked_payment_totals(1_000, 25, 10, 1_034),
            Err(ProtocolError::AmountTooLow { minimum_amount: 1_035 })
        ));
        assert!(matches!(
            checked_payment_totals(u64::MAX, 1, 0, u64::MAX),
            Err(ProtocolError::GenericError(_))
        ));
    }

    #[test]
    fn payment_no_effect_after_ambiguity_retries_the_same_tuple() {
        assert!(payment_no_effect_is_terminal(false, false));
        assert!(!payment_no_effect_is_terminal(true, false));
        assert!(!payment_no_effect_is_terminal(false, true));
        assert!(!payment_no_effect_is_terminal(true, true));
    }

    #[test]
    fn payout_net_requires_room_for_the_pinned_ledger_fee() {
        assert_eq!(exact_payout_net(1_000, 10).unwrap(), 990);
        assert!(matches!(
            exact_payout_net(10, 11),
            Err(ProtocolError::GenericError(_))
        ));
    }

    #[test]
    fn collateral_split_conserves_gross_with_two_explicit_ledger_fees() {
        let (sp_gross, sp_net, treasury_net) =
            collateral_payout_split(1_000, 250, 10).unwrap();
        assert_eq!(sp_gross, 750);
        assert_eq!(sp_net, 740);
        assert_eq!(treasury_net, 240);
        assert_eq!(sp_net + 10 + treasury_net + 10, 1_000);
        assert!(collateral_payout_split(1_000, 10, 10).is_err());
    }

    #[test]
    fn icusd_burn_tuple_uses_zero_fee_even_when_normal_ledger_fee_is_nonzero() {
        assert_eq!(effective_payment_fee(SpLiquidationPayment::Icusd, 100_000), 0);
        assert_eq!(effective_payment_fee(SpLiquidationPayment::Ckusdc, 10), 10);
    }

    fn native_transfer_block(
        from: Principal,
        to: Principal,
        amount_raw: u64,
        fee_raw: u64,
        memo: &[u8],
        created_at_time_ns: u64,
    ) -> crate::native_icp_proof::Block {
        crate::native_icp_proof::Block {
            parent_hash: None,
            transaction: crate::native_icp_proof::Transaction {
                memo: 0,
                icrc1_memo: Some(memo.to_vec()),
                operation: Some(crate::native_icp_proof::Operation::Transfer {
                    from: crate::native_icp_proof::account_identifier(from).to_vec(),
                    to: crate::native_icp_proof::account_identifier(to).to_vec(),
                    spender: None,
                    amount: crate::native_icp_proof::Tokens { e8s: amount_raw },
                    fee: crate::native_icp_proof::Tokens { e8s: fee_raw },
                }),
                created_at_time: crate::native_icp_proof::Timestamp {
                    timestamp_nanos: created_at_time_ns,
                },
            },
            timestamp: crate::native_icp_proof::Timestamp {
                timestamp_nanos: created_at_time_ns,
            },
        }
    }

    #[test]
    fn native_collateral_proof_accepts_backend_treasury_and_sp_exact_tuples() {
        let backend = Principal::from_slice(&[31]);
        let treasury = Principal::from_slice(&[32]);
        let stability_pool = Principal::from_slice(&[33]);
        let memo = [0x71; 16];

        let treasury_block = native_transfer_block(backend, treasury, 900, 10, &memo, 701);
        assert!(validate_native_icp_transfer_block(
            &treasury_block,
            Account {
                owner: backend,
                subaccount: None,
            },
            Account {
                owner: treasury,
                subaccount: None,
            },
            900,
            10,
            &memo,
            701,
        )
        .is_ok());

        let stability_pool_block =
            native_transfer_block(backend, stability_pool, 1_900, 10, &memo, 702);
        assert!(validate_native_icp_transfer_block(
            &stability_pool_block,
            Account {
                owner: backend,
                subaccount: None,
            },
            Account {
                owner: stability_pool,
                subaccount: None,
            },
            1_900,
            10,
            &memo,
            702,
        )
        .is_ok());
    }

    #[test]
    fn native_collateral_proof_rejects_wrong_frozen_tuple_fields() {
        let backend = Principal::from_slice(&[41]);
        let treasury = Principal::from_slice(&[42]);
        let other = Principal::from_slice(&[43]);
        let memo = [0x72; 16];
        let block = native_transfer_block(backend, treasury, 900, 10, &memo, 701);
        let from = Account {
            owner: backend,
            subaccount: None,
        };
        let to = Account {
            owner: treasury,
            subaccount: None,
        };

        assert!(validate_native_icp_transfer_block(
            &block,
            from.clone(),
            to.clone(),
            899,
            10,
            &memo,
            701,
        )
        .is_err());
        assert!(validate_native_icp_transfer_block(
            &block,
            from.clone(),
            Account {
                owner: other,
                subaccount: None,
            },
            900,
            10,
            &memo,
            701,
        )
        .is_err());
        assert!(validate_native_icp_transfer_block(
            &block,
            Account {
                owner: other,
                subaccount: None,
            },
            to.clone(),
            900,
            10,
            &memo,
            701,
        )
        .is_err());
        assert!(validate_native_icp_transfer_block(
            &block,
            from.clone(),
            to.clone(),
            900,
            11,
            &memo,
            701,
        )
        .is_err());
        assert!(validate_native_icp_transfer_block(
            &block,
            from.clone(),
            to.clone(),
            900,
            10,
            &[0x73; 16],
            701,
        )
        .is_err());
        assert!(validate_native_icp_transfer_block(
            &block, from, to, 900, 10, &memo, 702
        )
        .is_err());
    }

    #[test]
    fn prepared_quote_rejects_collateral_sunset_before_first_payment() {
        let mut state = crate::state::State::from(crate::InitArg {
            xrc_principal: Principal::anonymous(),
            icusd_ledger_principal: Principal::from_slice(&[10]),
            icp_ledger_principal: Principal::from_slice(&[11]),
            fee_e8s: 0,
            developer_principal: Principal::from_slice(&[12]),
            treasury_principal: None,
            stability_pool_principal: None,
            ckusdt_ledger_principal: None,
            ckusdc_ledger_principal: None,
        });
        let collateral = state.icp_collateral_type();
        let config = state.collateral_configs.get_mut(&collateral).unwrap();
        config.status = crate::state::CollateralStatus::Sunset;
        let mut journal = cancellation_tombstone([7; REQUEST_ID_LEN], Principal::from_slice(&[1]), &request(SpLiquidationPayment::Icusd), Principal::from_slice(&[10]), 10);
        journal.collateral_type = collateral;
        let err = validate_predispatch_quote_at(&state, &journal, 11).unwrap_err();
        assert!(err.contains("manual liquidation"));
        state.collateral_configs.get_mut(&collateral).unwrap().status = crate::state::CollateralStatus::Active;
        let err = validate_predispatch_quote_at(&state, &journal, 11).unwrap_err();
        assert!(!err.contains("manual liquidation"));
    }

    #[test]
    fn quote_freshness_is_bounded_before_first_attempt() {
        assert_eq!(SP_LIQUIDATION_QUOTE_TTL_NS, 5 * 60 * 1_000_000_000);
        assert_eq!(SP_LIQUIDATION_PRICE_MAX_AGE_NS, 10 * 60 * 1_000_000_000);
    }

    #[test]
    fn lifecycle_view_preserves_each_durable_phase() {
        assert_eq!(phase_view(&SpLiquidationPhase::Prepared), SpLiquidationPhaseView::Prepared);
        assert_eq!(
            phase_view(&SpLiquidationPhase::PaymentSubmitted),
            SpLiquidationPhaseView::PaymentSubmitted
        );
        assert_eq!(
            phase_view(&SpLiquidationPhase::PaymentConfirmed),
            SpLiquidationPhaseView::PaymentConfirmed
        );
        assert_eq!(phase_view(&SpLiquidationPhase::DebtCommitted), SpLiquidationPhaseView::DebtCommitted);
        assert_eq!(phase_view(&SpLiquidationPhase::PayoutSubmitted), SpLiquidationPhaseView::PayoutSubmitted);
        assert_eq!(phase_view(&SpLiquidationPhase::PayoutSettled), SpLiquidationPhaseView::PayoutSettled);
        assert_eq!(
            phase_view(&SpLiquidationPhase::PaymentNoEffect("typed".into())),
            SpLiquidationPhaseView::PaymentNoEffect
        );
        assert_eq!(
            phase_view(&SpLiquidationPhase::PayoutNoEffect("typed".into())),
            SpLiquidationPhaseView::PayoutNoEffect
        );
        assert_eq!(phase_view(&SpLiquidationPhase::Cancelled), SpLiquidationPhaseView::Cancelled);
    }

    #[test]
    fn cancel_cas_allows_only_pre_dispatch_or_definitive_no_effect() {
        assert!(can_cancel_phase(&SpLiquidationPhase::Prepared));
        assert!(can_cancel_phase(&SpLiquidationPhase::PaymentNoEffect("no effect".into())));
        assert!(!can_cancel_phase(&SpLiquidationPhase::PaymentSubmitted));
        assert!(!can_cancel_phase(&SpLiquidationPhase::PaymentConfirmed));
        assert!(!can_cancel_phase(&SpLiquidationPhase::DebtCommitted));
        assert!(!can_cancel_phase(&SpLiquidationPhase::PayoutSettled));
    }

    #[test]
    fn interest_dispatch_fence_is_one_shot_across_reply_loss_and_upgrade() {
        // The treasury helpers synchronously admit every routed leg (or the
        // supported fallback accumulator) before their first await. The
        // backend fence is committed immediately before that call, so the
        // resumed execute path has no second dispatch opportunity.
        assert!(should_dispatch_interest(123, false));
        assert!(!should_dispatch_interest(123, true));
        assert!(!should_dispatch_interest(0, false));
        assert!(!should_dispatch_interest(0, true));
    }

    fn proof_payment_tuple() -> SpLiquidationPaymentTuple {
        SpLiquidationPaymentTuple {
            ledger: Principal::from_slice(&[9]),
            source: Account {
                owner: Principal::from_slice(&[1]),
                subaccount: None,
            },
            destination: Account {
                owner: Principal::from_slice(&[2]),
                subaccount: None,
            },
            amount_raw: 123,
            ledger_fee_raw: 17,
            memo: [8; 16],
            created_at_time_ns: 55,
            op_nonce: 7,
        }
    }

    #[test]
    fn positive_payment_proof_requires_every_icrc2_tuple_field() {
        let tuple = proof_payment_tuple();
        let block = DecodedBlock {
            btype: Some("2xfer".into()),
            op: "xfer".into(),
            from: Some(tuple.source.clone()),
            to: Some(tuple.destination.clone()),
            spender: Some(tuple.destination.clone()),
            amount: tuple.amount_raw.into(),
            fee: Some(tuple.ledger_fee_raw),
            created_at_time: Some(tuple.created_at_time_ns),
            memo: Some(tuple.memo.to_vec()),
        };
        assert!(validate_exact_payment_block(
            &block,
            SpLiquidationPayment::Ckusdc,
            &tuple
        )
        .is_ok());

        let mut wrong = block.clone();
        wrong.spender = Some(tuple.source.clone());
        assert!(validate_exact_payment_block(
            &wrong,
            SpLiquidationPayment::Ckusdc,
            &tuple
        )
        .is_err());
        let mut wrong_fee = block.clone();
        wrong_fee.fee = Some(18);
        assert!(validate_exact_payment_block(
            &wrong_fee,
            SpLiquidationPayment::Ckusdc,
            &tuple
        )
        .is_err());
        let mut wrong_memo = block;
        wrong_memo.memo = Some(vec![7; 16]);
        assert!(validate_exact_payment_block(
            &wrong_memo,
            SpLiquidationPayment::Ckusdc,
            &tuple
        )
        .is_err());
    }

    #[test]
    fn icusd_payment_proof_uses_canonical_transfer_from_burn_shape() {
        let mut tuple = proof_payment_tuple();
        tuple.ledger_fee_raw = 100_000;
        let backend = tuple.destination.clone();
        let block = DecodedBlock {
            btype: Some("2burn".into()),
            op: "burn".into(),
            from: Some(tuple.source.clone()),
            to: None,
            spender: Some(backend.clone()),
            amount: tuple.amount_raw.into(),
            fee: None,
            created_at_time: Some(tuple.created_at_time_ns),
            memo: Some(tuple.memo.to_vec()),
        };
        assert!(validate_exact_payment_block(
            &block,
            SpLiquidationPayment::Icusd,
            &tuple,
        )
        .is_ok());

        let mut explicit_zero = block.clone();
        explicit_zero.fee = Some(0);
        assert!(validate_exact_payment_block(
            &explicit_zero,
            SpLiquidationPayment::Icusd,
            &tuple,
        )
        .is_ok());

        let mut unexpected_destination = block.clone();
        unexpected_destination.to = Some(backend.clone());
        assert!(validate_exact_payment_block(
            &unexpected_destination,
            SpLiquidationPayment::Icusd,
            &tuple,
        )
        .is_err());

        let mut wrong_spender = block.clone();
        wrong_spender.spender = Some(tuple.source.clone());
        assert!(validate_exact_payment_block(
            &wrong_spender,
            SpLiquidationPayment::Icusd,
            &tuple,
        )
        .is_err());

        let mut plain_unbound_burn = block;
        plain_unbound_burn.btype = Some("1burn".into());
        plain_unbound_burn.spender = None;
        assert!(validate_exact_payment_block(
            &plain_unbound_burn,
            SpLiquidationPayment::Icusd,
            &tuple,
        )
        .is_err());
    }

    #[test]
    fn positive_collateral_proof_requires_exact_fee_and_accounts() {
        let from = Account {
            owner: Principal::from_slice(&[3]),
            subaccount: None,
        };
        let to = Account {
            owner: Principal::from_slice(&[4]),
            subaccount: None,
        };
        let block = DecodedBlock {
            btype: Some("1xfer".into()),
            op: "xfer".into(),
            from: Some(from.clone()),
            to: Some(to.clone()),
            spender: None,
            amount: 90,
            fee: Some(10),
            created_at_time: Some(77),
            memo: Some(vec![6; 16]),
        };
        assert!(validate_exact_ledger_transfer_block(
            &block,
            from.clone(),
            to.clone(),
            90,
            10,
            &[6; 16],
            77,
        )
        .is_ok());
        let mut wrong_fee = block.clone();
        wrong_fee.fee = Some(9);
        assert!(validate_exact_ledger_transfer_block(
            &wrong_fee,
            from.clone(),
            to.clone(),
            90,
            10,
            &[6; 16],
            77,
        )
        .is_err());
        let mut wrong_to = block;
        wrong_to.to = Some(Account {
            owner: Principal::from_slice(&[5]),
            subaccount: None,
        });
        assert!(validate_exact_ledger_transfer_block(
            &wrong_to,
            from,
            to,
            90,
            10,
            &[6; 16],
            77,
        )
        .is_err());
    }

    #[test]
    fn positive_proof_wrong_candidate_does_not_pin_and_equal_retry_is_idempotent() {
        assert!(!reconcile_existing_block(None, 12, "payment").unwrap());
        assert!(reconcile_existing_block(Some(12), 12, "payment").unwrap());
        assert!(reconcile_existing_block(Some(12), 13, "payment").is_err());

        assert!(payment_reconciliation_phase_allowed(
            &SpLiquidationPhase::PaymentNoEffect("TooOld".into()),
            true,
            None,
        ));
        assert!(!payment_reconciliation_phase_allowed(
            &SpLiquidationPhase::Prepared,
            false,
            None,
        ));
        assert!(!payment_reconciliation_phase_allowed(
            &SpLiquidationPhase::PaymentSubmitted,
            true,
            Some(12),
        ));
        assert!(payout_first_dispatch_required(&SpLiquidationPhase::DebtCommitted));
        assert!(!payout_first_dispatch_required(&SpLiquidationPhase::PayoutSubmitted));
        assert!(!payout_first_dispatch_required(&SpLiquidationPhase::PayoutNoEffect(
            "typed".into(),
        )));
        assert!(!payout_reconciliation_phase_allowed(
            &SpLiquidationPhase::DebtCommitted,
            None,
        ));
        assert!(payout_reconciliation_phase_allowed(
            &SpLiquidationPhase::PayoutSubmitted,
            None,
        ));
        assert!(payout_reconciliation_phase_allowed(
            &SpLiquidationPhase::PayoutNoEffect("TooOld".into()),
            None,
        ));
        assert!(!payout_reconciliation_phase_allowed(
            &SpLiquidationPhase::PayoutSubmitted,
            Some(19),
        ));
    }
}

