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
    if let Some(existing) = read_state(|state| state.sp_liquidation_journals.get(&key).cloned()) {
        if !request_binding_matches(
            existing.caller,
            existing.vault_id,
            existing.payment,
            existing.max_debt_e8s,
            existing.max_gross_raw,
            caller,
            &request,
        ) {
            return Err(ProtocolError::GenericError(
                "SP liquidation request_id is already bound to different parameters".into(),
            ));
        }
        return Ok(receipt(&existing));
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
    let (op_nonce, protocol_fee_op_nonce) = mutate_state(|state| {
        let payment_nonce = state.next_op_nonce();
        let protocol_nonce = if quote.protocol_cut_raw > 0 {
            Some(state.next_op_nonce())
        } else {
            None
        };
        (payment_nonce, protocol_nonce)
    });
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
        protocol_fee_op_nonce,
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
    if now < journal.created_at_ns
        || now.saturating_sub(journal.created_at_ns) > SP_LIQUIDATION_QUOTE_TTL_NS
    {
        return Err("SP liquidation quote expired before payment dispatch".into());
    }
    read_state(|state| {
        if state.frozen || state.liquidation_frozen {
            return Err("liquidations are frozen before payment dispatch".into());
        }
        let config = state
            .get_collateral_config(&journal.collateral_type)
            .ok_or_else(|| "reserved collateral configuration is missing".to_string())?;
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
    })
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
    if journal.quote_protocol_cut_raw > 0
        && journal.protocol_fee_block_index.is_none()
    {
        let (treasury, fee, op_nonce, attempted) = read_state(|state| {
            let current = state.sp_liquidation_journals.get(&key);
            current.map(|current| (
                current.protocol_fee_treasury,
                current.protocol_fee_ledger_fee_raw,
                current.protocol_fee_op_nonce,
                current.protocol_fee_attempted,
            )).unwrap_or((None, None, None, false))
        });
        let treasury = treasury.ok_or_else(|| {
            ProtocolError::TemporarilyUnavailable("SP liquidation treasury destination is missing".into())
        })?;
        let fee = fee.ok_or_else(|| {
            ProtocolError::TemporarilyUnavailable("SP liquidation treasury fee tuple is missing".into())
        })?;
        let op_nonce = op_nonce.ok_or_else(|| {
            ProtocolError::TemporarilyUnavailable("SP liquidation treasury nonce is missing".into())
        })?;
        let protocol_fee_net_raw = exact_payout_net(journal.quote_protocol_cut_raw, fee)?;
        if !attempted {
            mutate_state(|state| {
                if let Some(current) = state.sp_liquidation_journals.get_mut(&key) {
                    current.protocol_fee_attempted = true;
                    current.updated_at_ns = ic_cdk::api::time();
                }
            });
        }
        match crate::management::transfer_collateral_with_nonce_and_fee_status(
            protocol_fee_net_raw,
            Some(fee),
            treasury,
            journal.collateral_ledger,
            op_nonce,
        ).await {
            Ok(block_index) => {
                mutate_state(|state| {
                    if let Some(current) = state.sp_liquidation_journals.get_mut(&key) {
                        current.protocol_fee_block_index = Some(block_index);
                        current.updated_at_ns = ic_cdk::api::time();
                    }
                });
                // Preserve the existing treasury bookkeeping callback. Its
                // failure is non-critical; the exact ledger receipt above is
                // the durable proof of the fee transfer.
                let _ = crate::treasury::notify_treasury_deposit(
                    treasury,
                    crate::treasury::DepositType::LiquidationFee,
                    crate::treasury::collateral_to_asset_type(&journal.collateral_type),
                    protocol_fee_net_raw,
                    block_index,
                ).await;
            }
            Err(error) => {
                return Err(ProtocolError::TemporarilyUnavailable(format!(
                    "SP treasury fee remains unresolved; retry the same request: {error:?}"
                )));
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
    let Some(operation_id) = journal.payout_operation_id else {
        return Err(ProtocolError::TemporarilyUnavailable("SP payout operation is not linked yet".into()));
    };
    let Some((_, transfer)) = read_state(|state| state.get_pending_payout(operation_id)) else {
        if journal.phase == SpLiquidationPhase::PayoutSettled {
            return Ok(receipt(&journal));
        }
        return Err(ProtocolError::TemporarilyUnavailable("SP payout receipt is missing".into()));
    };
    mutate_state(|state| {
        let Some(current) = state.sp_liquidation_journals.get_mut(&key) else { return false };
        if current.caller != caller
            || current.payout_operation_id != Some(operation_id)
            || !matches!(
                current.phase,
                SpLiquidationPhase::DebtCommitted
                    | SpLiquidationPhase::PayoutSubmitted
                    | SpLiquidationPhase::PayoutNoEffect(_)
            )
        {
            return false;
        }
        current.phase = SpLiquidationPhase::PayoutSubmitted;
        current.updated_at_ns = ic_cdk::api::time();
        state.mutate_pending_payout(operation_id, |row| row.in_flight = true);
        true
    });
    let amount = transfer.transfer_amount_raw.ok_or_else(|| {
        ProtocolError::TemporarilyUnavailable("SP payout net amount is missing".into())
    })?;
    let ledger = transfer.ledger.ok_or_else(|| {
        ProtocolError::TemporarilyUnavailable("SP payout ledger is missing".into())
    })?;
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
            next.payout_op_nonce = applied.payout.as_ref().map(|row| row.5);
            next.payout_ledger = applied.payout.as_ref().map(|row| row.1);
            next.payout_gross_raw = applied.payout.as_ref().map(|row| row.2);
            next.payout_net_raw = applied.payout.as_ref().map(|row| row.3);
            next.payout_fee_raw = applied.payout.as_ref().map(|row| row.4);
            next.payout_memo = applied
                .payout
                .as_ref()
                .map(|row| {
                    management::nonce_to_memo(row.5)
                        .0
                        .as_slice()
                        .try_into()
                        .unwrap_or([0; 16])
                })
                .unwrap_or([0; 16]);
            next.payout_created_at_time_ns = applied
                .payout
                .as_ref()
                .map(|row| management::nonce_to_created_at_time(row.5))
                .unwrap_or_default();
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
}

