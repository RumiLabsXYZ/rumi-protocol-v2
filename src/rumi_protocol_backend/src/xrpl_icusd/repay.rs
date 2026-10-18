//! Repay XRPL icUSD debt with XRPL icUSD (Task 5).
//!
//! Lifecycle of a repayment operation:
//! `IntentPrepared -> ReturnVerified -> EscrowBurnPending -> DebtApplied -> Complete`,
//! with `Held` (a backed, owned payer claim) when the value cannot be applied.
//!
//! * **Intent.** [`create_intent`] binds an owner, vault, payer, network, ceiling and a
//!   never-reused destination tag. The wallet signs an exact issued `Payment` to the
//!   issuer ([`payment_json`]).
//! * **Return.** [`apply_verified_return`] is the ONE entry that consumes a verified
//!   XRPL payment (found by a wallet hint or by the paged issuer-history scan). It is
//!   deduplicated on `(network, issuer, tx hash)`, credits only `delivered_amount`,
//!   moves the value `O -> held`, and either attaches it to the intent or turns it
//!   into a refund for the verified sender.
//! * **Settlement.** [`settle_return_against_debt`] runs in ONE synchronous block: it
//!   re-reads the vault, accrues to `now`, applies the existing min-debt / full-close
//!   policy to the funds ACTUALLY received (never above them), and either holds the
//!   whole receipt (burn nothing) or admits a burn: it snapshots debt and accrued
//!   interest, computes the proportional interest share with the legacy formula,
//!   records `RepayCutoffBegun` (vault exclusion + accrual freeze) and persists the
//!   `EscrowBurn` job.
//! * **Proof.** A proven burn records `RepaySettled`, applied by
//!   [`State::apply_xrpl_repay_settlement`] (live and replay alike): the saved
//!   allocation is applied once at the cutoff and only the unpaid remainder catches
//!   up. A proven NEVER-executed burn records `RepayCancelled`; an ambiguous burn can
//!   never be cancelled.

use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;
use sha2::{Digest, Sha256};

use super::borrow::record_lane_event;
use super::ledger;
use super::state::{Admission, XrplIcusdState};
use super::types::*;
use crate::chains::xrp::address::is_valid_classic_address;
use crate::chains::xrp::issued_amount::{ExactIssuedAmount, ICUSD_CURRENCY_HEX};
use crate::chains::xrp::issued_rpc::{CreditEligibility, VerifiedIssuedReceipt};
use crate::chains::xrp::network::XrplNetwork;
use crate::numeric::ICUSD;
use crate::state::{mutate_state, read_state, State};

// ----------------------------------------------------------------------------------
// Intent
// ----------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct IntentRequest {
    pub owner: Principal,
    pub vault_id: u64,
    pub max_e8s: u64,
    pub payer: String,
    pub network: XrplNetwork,
    pub request_nonce: [u8; 32],
    pub full_close: bool,
    pub top_up: Option<OperationId>,
}

fn request_kind(req: &IntentRequest) -> OperationKind {
    // Only the owner-authorized parameters take part in the idempotency comparison.
    OperationKind::Repayment {
        vault_id: req.vault_id,
        payer: req.payer.clone(),
        destination_tag: 0,
        max_e8s: req.max_e8s,
        full_close: req.full_close,
        parent: req.top_up,
        receipts: vec![],
        cutoff: None,
        applied_e8s: 0,
        held: WideAmount::ZERO,
        children: vec![],
        pending_interest: None,
    }
}

/// The exact `Payment` the wallet must sign: an issued-currency payment to the issuer
/// with the intent's `DestinationTag` and `Amount = max_e8s` exactly. No `Paths`, no
/// `SendMax`, no `Flags` (so no `tfPartialPayment`).
pub fn payment_json(
    issuer: &str,
    payer: &str,
    destination_tag: u32,
    max_e8s: u64,
) -> Result<String, XrplIcusdError> {
    let amount = ExactIssuedAmount::try_from_e8s(max_e8s)
        .map_err(|e| XrplIcusdError::InvalidArgument(format!("amount: {e}")))?;
    let v = serde_json::json!({
        "TransactionType": "Payment",
        "Account": payer,
        "Destination": issuer,
        "DestinationTag": destination_tag,
        "Amount": {
            "currency": ICUSD_CURRENCY_HEX,
            "issuer": issuer,
            "value": amount.to_decimal_string(),
        },
    });
    serde_json::to_string(&v).map_err(|e| XrplIcusdError::Internal(e.to_string()))
}

pub fn intent_view(
    st: &XrplIcusdState,
    op_id: OperationId,
    now_ns: u64,
) -> Result<RepaymentIntentView, XrplIcusdError> {
    let op = st.op(op_id).ok_or(XrplIcusdError::NotFound)?;
    let OperationKind::Repayment {
        payer,
        destination_tag,
        max_e8s,
        ..
    } = &op.kind
    else {
        return Err(XrplIcusdError::NotApplicable("not a repayment".into()));
    };
    let issuer = st
        .config
        .issuer_address
        .clone()
        .ok_or_else(|| XrplIcusdError::Disabled("issuer address not derived".into()))?;
    Ok(RepaymentIntentView {
        operation_id: op_id,
        payment_json: payment_json(&issuer, payer, *destination_tag, *max_e8s)?,
        issuer,
        currency: ICUSD_CURRENCY_HEX.to_string(),
        payer: payer.clone(),
        destination_tag: *destination_tag,
        max_e8s: *max_e8s,
        network: op.network,
        quote_time_ns: now_ns,
    })
}

/// Validate and persist a repayment intent in ONE synchronous step. The same request
/// nonce returns the same operation and never a second destination tag.
pub fn create_intent(
    state: &mut State,
    req: &IntentRequest,
    now_ns: u64,
) -> Result<(OperationId, bool), XrplIcusdError> {
    if req.owner == Principal::anonymous() {
        return Err(XrplIcusdError::AnonymousCaller);
    }
    let kind = request_kind(req);
    match state
        .xrpl_icusd
        .admit(&req.owner, &req.request_nonce, req.network, &kind)?
    {
        Admission::Existing(id) => return Ok((id, false)),
        Admission::New => {}
    }
    // Repayment is available whenever the lane is configured; a pause of NEW issuance
    // never blocks paying debt down.
    state.xrpl_icusd.ensure_ready()?;
    if Some(req.network) != state.xrpl_icusd.config.network {
        return Err(XrplIcusdError::WrongNetwork(
            "request network differs from the configured network".into(),
        ));
    }
    let issuer = state
        .xrpl_icusd
        .config
        .issuer_address
        .clone()
        .ok_or_else(|| XrplIcusdError::Disabled("issuer address not derived".into()))?;
    if !is_valid_classic_address(&req.payer) {
        return Err(XrplIcusdError::InvalidArgument(
            "payer must be a valid classic XRPL address".into(),
        ));
    }
    if req.payer == issuer {
        return Err(XrplIcusdError::InvalidArgument(
            "the payer cannot be the issuer".into(),
        ));
    }
    if req.max_e8s == 0 {
        return Err(XrplIcusdError::InvalidArgument("amount must be > 0".into()));
    }
    if req.max_e8s > state.xrpl_icusd.config.per_operation_cap_e8s {
        return Err(XrplIcusdError::LimitExceeded(format!(
            "amount exceeds the per-operation cap of {} e8s",
            state.xrpl_icusd.config.per_operation_cap_e8s
        )));
    }
    // v1: the caller's own native-XRP vault with debt.
    {
        let vault = state
            .vault_id_to_vaults
            .get(&req.vault_id)
            .ok_or(XrplIcusdError::NotFound)?;
        if vault.owner != req.owner {
            return Err(XrplIcusdError::Unauthorized);
        }
        if !state
            .get_collateral_config(&vault.collateral_type)
            .is_some_and(|c| c.is_native_xrp())
        {
            return Err(XrplIcusdError::NotApplicable(
                "XRPL icUSD repayment is limited to native-XRP vaults in this version".into(),
            ));
        }
        if state
            .get_collateral_status(&vault.collateral_type)
            .is_some_and(|s| !s.allows_repay())
        {
            return Err(XrplIcusdError::Policy(
                "repayment is not allowed for this collateral type".into(),
            ));
        }
        if vault.borrowed_icusd_amount.0 == 0 {
            return Err(XrplIcusdError::InvalidArgument(
                "the vault has no debt to repay".into(),
            ));
        }
    }
    // Top-up binding: same owner, vault, payer, network and asset, and a HELD parent.
    if let Some(pid) = req.top_up {
        let parent = state
            .xrpl_icusd
            .op_for_owner(&req.owner, pid)
            .map_err(|_| XrplIcusdError::NotFound)?;
        let OperationKind::Repayment {
            vault_id,
            payer,
            parent: grandparent,
            ..
        } = &parent.kind
        else {
            return Err(XrplIcusdError::NotApplicable(
                "a top-up must target a repayment".into(),
            ));
        };
        if *vault_id != req.vault_id {
            return Err(XrplIcusdError::InvalidArgument(
                "a top-up must target the parent's vault".into(),
            ));
        }
        if *payer != req.payer {
            return Err(XrplIcusdError::InvalidArgument(
                "a top-up must come from the parent's payer".into(),
            ));
        }
        if parent.network != req.network {
            return Err(XrplIcusdError::WrongNetwork(
                "a top-up must use the parent's network".into(),
            ));
        }
        if grandparent.is_some() {
            return Err(XrplIcusdError::NotApplicable(
                "a top-up cannot itself be topped up; target the held parent".into(),
            ));
        }
        let burned = matches!(&parent.kind, OperationKind::Repayment { applied_e8s, .. } if *applied_e8s > 0);
        if parent.phase != Phase::Held || parent.resolution.is_some() || burned {
            return Err(XrplIcusdError::WrongPhase(
                "only a held repayment that has not burned and has no resolution in progress \
                 can be topped up (the excess of a settled repayment is a claim, not a deposit)"
                    .into(),
            ));
        }
    }
    let id = state.xrpl_icusd.allocate_operation_id();
    let tag = state.xrpl_icusd.allocate_destination_tag()?;
    let op = Operation {
        id,
        owner: req.owner,
        created_at_ns: now_ns,
        updated_at_ns: now_ns,
        request_nonce: req.request_nonce,
        network: req.network,
        kind: OperationKind::Repayment {
            vault_id: req.vault_id,
            payer: req.payer.clone(),
            destination_tag: tag,
            max_e8s: req.max_e8s,
            full_close: req.full_close,
            parent: req.top_up,
            receipts: vec![],
            cutoff: None,
            applied_e8s: 0,
            held: WideAmount::ZERO,
            children: vec![],
            pending_interest: None,
        },
        phase: Phase::IntentPrepared,
        ic_jobs: vec![],
        xrpl_attempts: vec![],
        notes: vec![],
        resolution: None,
        attempt_budget_base: 0,
        delivery_halted: false,
    };
    state.xrpl_icusd.insert_operation(op)?;
    state.xrpl_icusd.tag_index.insert(tag, id);
    if let Some(pid) = req.top_up {
        if let Ok(parent) = state.xrpl_icusd.op_mut(pid) {
            if let OperationKind::Repayment { children, .. } = &mut parent.kind {
                children.push(id);
            }
        }
    }
    Ok((id, true))
}

/// An unused intent expires; its tag is NEVER reassigned, so a late payment to it is
/// handled as an unattributed return (refunded to the verified sender).
pub fn expire_stale_intents(st: &mut XrplIcusdState, now_ns: u64) -> usize {
    let stale: Vec<OperationId> = st
        .operations
        .values()
        .filter(|o| {
            matches!(o.kind, OperationKind::Repayment { .. })
                && o.phase == Phase::IntentPrepared
                && now_ns.saturating_sub(o.created_at_ns) > INTENT_TTL_NS
        })
        .map(|o| o.id)
        .collect();
    for id in &stale {
        if let Ok(op) = st.op_mut(*id) {
            op.phase = Phase::Cancelled;
            op.updated_at_ns = now_ns;
            op.push_note("intent expired unused");
        }
    }
    stale.len()
}

// ----------------------------------------------------------------------------------
// Verified returns
// ----------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReturnOutcome {
    /// Already consumed: nothing changes.
    Duplicate,
    Attributed { op_id: OperationId },
    Merged { child: OperationId, parent: OperationId },
    /// Value-bearing but not attributable: a refund operation for the verified sender.
    Refund { op_id: OperationId },
    /// No financial effect (bad evidence); recorded in the quarantine log.
    Quarantined(String),
}

pub fn processed_key(network: XrplNetwork, issuer: &str, tx_hash: &str) -> (String, String, String) {
    (
        network.name().to_string(),
        issuer.to_string(),
        tx_hash.to_ascii_uppercase(),
    )
}

/// Deterministic request nonce of the refund for one return.
pub fn refund_nonce(network: XrplNetwork, issuer: &str, tx_hash: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"rumi:xrpl-icusd:refund:v1");
    h.update(network.name().as_bytes());
    h.update(issuer.as_bytes());
    h.update(tx_hash.to_ascii_uppercase().as_bytes());
    h.finalize().into()
}

/// Deterministic request nonce of an owner-requested reissue of a held claim.
pub fn reissue_nonce(parent: OperationId, ordinal: usize) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"rumi:xrpl-icusd:reissue:v1");
    h.update(parent.to_be_bytes());
    h.update((ordinal as u64).to_be_bytes());
    h.finalize().into()
}

/// Receipt evidence that must hold before ANY value moves. `None` means sound.
fn evidence_problem(r: &VerifiedIssuedReceipt, issuer: &str) -> Option<String> {
    if !r.validated || r.engine_result != "tesSUCCESS" {
        return Some("not a validated tesSUCCESS payment".into());
    }
    if r.destination != issuer {
        return Some("destination is not the issuer".into());
    }
    if r.account == issuer {
        return Some("the issuer's own payment is not a return".into());
    }
    if r.delivered_amount.is_zero() || r.delivered_amount.is_negative() {
        return Some("zero or negative delivered amount".into());
    }
    // The trust line between the sender and the issuer must show exactly the
    // delivered amount leaving the sender: the tokens really came back.
    let moved = r.balance_changes.iter().find_map(|c| {
        let pair = (c.low_account == r.account && c.high_account == issuer)
            || (c.high_account == r.account && c.low_account == issuer);
        pair.then(|| c.delta_for(&r.account)).flatten()
    });
    match moved {
        Some(d) if d == r.delivered_amount.negated() => None,
        Some(_) => Some("sender trust-line change differs from the delivered amount".into()),
        None => Some("no sender trust-line balance change in the receipt metadata".into()),
    }
}

/// Consume ONE verified XRPL payment to the issuer, exactly once per
/// `(network, issuer, tx hash)`, whether it was found by a hint or by the history scan.
///
/// Only `delivered_amount` is credited, never the nominal `Amount`. The value moves
/// `O -> held` (the escrow backing stays put). Attribution requires the destination
/// tag to map to an open intent AND the sender to be that intent's payer AND an
/// eligible (non-partial, non-path) payment; anything else becomes a refund for the
/// verified sender and grants no debt credit.
pub fn apply_verified_return(
    st: &mut XrplIcusdState,
    network: XrplNetwork,
    issuer: &str,
    r: &VerifiedIssuedReceipt,
    now_ns: u64,
) -> Result<ReturnOutcome, XrplIcusdError> {
    let key = processed_key(network, issuer, &r.hash);
    if st.processed_returns.contains_key(&key) {
        return Ok(ReturnOutcome::Duplicate);
    }
    if let Some(problem) = evidence_problem(r, issuer) {
        st.log_quarantine(QuarantineRecord {
            tx_hash: r.hash.to_ascii_uppercase(),
            ledger_index: Some(r.ledger_index),
            sender: Some(r.account.clone()),
            destination_tag: r.destination_tag,
            reason: problem.clone(),
            at_ns: now_ns,
        });
        return Ok(ReturnOutcome::Quarantined(problem));
    }
    let delivered = match WideAmount::from_exact(&r.delivered_amount) {
        Ok(w) => w,
        Err(_) => {
            let problem = "delivered amount is finer than the exact accounting range".to_string();
            st.log_quarantine(QuarantineRecord {
                tx_hash: r.hash.to_ascii_uppercase(),
                ledger_index: Some(r.ledger_index),
                sender: Some(r.account.clone()),
                destination_tag: r.destination_tag,
                reason: problem.clone(),
                at_ns: now_ns,
            });
            return Ok(ReturnOutcome::Quarantined(problem));
        }
    };
    if st.processed_returns.len() >= MAX_PROCESSED_RETURNS {
        return Err(XrplIcusdError::LimitExceeded(
            "processed-return capacity reached".into(),
        ));
    }
    // The tokens left circulation: O must cover them. A return observed before the
    // delivery that created it was reconciled is retried later, never half-applied.
    let new_o = st.issued_outstanding.checked_sub(&delivered).map_err(|_| {
        XrplIcusdError::Unavailable(
            "returned value exceeds recorded outstanding issuance; reconcile deliveries first"
                .into(),
        )
    })?;

    let eligible = r.credit_eligibility() == CreditEligibility::Eligible;
    let intent = r
        .destination_tag
        .and_then(|t| st.tag_index.get(&t).copied())
        .and_then(|id| st.operations.get(&id))
        .filter(|op| {
            op.phase == Phase::IntentPrepared
                && op.network == network
                && matches!(&op.kind, OperationKind::Repayment { payer, .. } if *payer == r.account)
        })
        .map(|op| op.id);

    let (disposition, outcome) = match (intent, eligible) {
        (Some(op_id), true) => attach_to_intent(st, op_id, r, &delivered, now_ns)?,
        _ => {
            let reason = if !eligible {
                "anomalous payment (partial/path/mismatched amount)"
            } else if r.destination_tag.is_none() {
                "no destination tag"
            } else if r
                .destination_tag
                .and_then(|t| st.tag_index.get(&t))
                .is_none()
            {
                "unknown destination tag"
            } else {
                "tag is not an open intent of this sender"
            };
            let op_id = create_refund(st, network, issuer, r, reason, now_ns)?;
            (
                ReturnDisposition::Refund {
                    op_id,
                    reason: reason.to_string(),
                },
                ReturnOutcome::Refund { op_id },
            )
        }
    };
    st.issued_outstanding = new_o;
    st.processed_returns.insert(
        key,
        ProcessedReturn {
            disposition,
            ledger_index: r.ledger_index,
            sender: r.account.clone(),
            destination_tag: r.destination_tag,
            delivered: r.delivered_amount.to_decimal_string(),
            recorded_at_ns: now_ns,
        },
    );
    Ok(outcome)
}

fn attach_to_intent(
    st: &mut XrplIcusdState,
    op_id: OperationId,
    r: &VerifiedIssuedReceipt,
    delivered: &WideAmount,
    now_ns: u64,
) -> Result<(ReturnDisposition, ReturnOutcome), XrplIcusdError> {
    let (parent_id, authorized) = {
        let op = st.op(op_id).ok_or(XrplIcusdError::NotFound)?;
        let OperationKind::Repayment {
            parent, max_e8s, ..
        } = &op.kind
        else {
            return Err(XrplIcusdError::Internal("intent is not a repayment".into()));
        };
        (*parent, *max_e8s)
    };
    let receipt = VerifiedReceiptRef {
        tx_hash: r.hash.to_ascii_uppercase(),
        ledger_index: r.ledger_index,
        delivered: r.delivered_amount,
        applied_e8s: 0,
        authorized_e8s: authorized,
    };
    // Combine with a HELD parent exactly once (the processed key guards the tx).
    if let Some(pid) = parent_id {
        let mergeable = st.op(pid).is_some_and(|p| {
            p.phase == Phase::Held
                && p.resolution.is_none()
                && matches!(&p.kind, OperationKind::Repayment { applied_e8s, .. } if *applied_e8s == 0)
        });
        if mergeable {
            let parent = st.op_mut(pid)?;
            if let OperationKind::Repayment {
                held, receipts, ..
            } = &mut parent.kind
            {
                *held = held
                    .checked_add(delivered)
                    .map_err(|e| XrplIcusdError::Internal(format!("held overflow: {e:?}")))?;
                receipts.push(receipt.clone());
            }
            parent.phase = Phase::ReturnVerified;
            parent.updated_at_ns = now_ns;
            parent.push_note(format!("top-up #{op_id} received {}", r.delivered_amount));
            let child = st.op_mut(op_id)?;
            if let OperationKind::Repayment { receipts, .. } = &mut child.kind {
                receipts.push(receipt);
            }
            child.phase = Phase::Complete;
            child.updated_at_ns = now_ns;
            child.push_note(format!("combined into held repayment #{pid}"));
            return Ok((
                ReturnDisposition::MergedIntoParent {
                    child: op_id,
                    parent: pid,
                },
                ReturnOutcome::Merged {
                    child: op_id,
                    parent: pid,
                },
            ));
        }
    }
    let op = st.op_mut(op_id)?;
    if let OperationKind::Repayment {
        held, receipts, ..
    } = &mut op.kind
    {
        *held = held
            .checked_add(delivered)
            .map_err(|e| XrplIcusdError::Internal(format!("held overflow: {e:?}")))?;
        receipts.push(receipt);
    }
    op.phase = Phase::ReturnVerified;
    op.updated_at_ns = now_ns;
    Ok((
        ReturnDisposition::Attributed { op_id },
        ReturnOutcome::Attributed { op_id },
    ))
}

/// A refund operation for the verified sender, created with a deterministic nonce
/// BEFORE anything is signed: the once-only state is bound first. System-created
/// operations bypass the user admission caps (the observer must always be able to
/// record value it has already consumed from `O`).
fn create_refund(
    st: &mut XrplIcusdState,
    network: XrplNetwork,
    issuer: &str,
    r: &VerifiedIssuedReceipt,
    reason: &str,
    now_ns: u64,
) -> Result<OperationId, XrplIcusdError> {
    let nonce = refund_nonce(network, issuer, &r.hash);
    let owner = system_owner();
    if let Some(id) = st.request_nonce_index.get(&(owner, nonce)) {
        return Ok(*id);
    }
    let whole = WideAmount::from_exact(&r.delivered_amount)
        .map_err(|e| XrplIcusdError::Internal(format!("{e:?}")))?;
    let dust = whole.whole_e8s < u128::from(MIN_AUTO_REFUND_E8S);
    let id = st.allocate_operation_id();
    let mut op = Operation {
        id,
        owner,
        created_at_ns: now_ns,
        updated_at_ns: now_ns,
        request_nonce: nonce,
        network,
        kind: OperationKind::Refund {
            source_tx: r.hash.to_ascii_uppercase(),
            recipient: r.account.clone(),
            amount: r.delivered_amount,
        },
        phase: if dust { Phase::Held } else { Phase::RefundPending },
        ic_jobs: vec![],
        xrpl_attempts: vec![],
        notes: vec![],
        resolution: None,
        attempt_budget_base: 0,
        delivery_halted: false,
    };
    op.push_note(format!(
        "unattributed return ({reason}) from {} at ledger {}",
        r.account, r.ledger_index
    ));
    if dust {
        op.push_note("below the automatic refund floor: retained exactly, not refunded");
    }
    st.request_nonce_index.insert((owner, nonce), id);
    st.next_operation_id = st.next_operation_id.max(id + 1);
    st.operations.insert(id, op);
    Ok(id)
}

// ----------------------------------------------------------------------------------
// Settlement against debt
// ----------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettleOutcome {
    /// Not a repayment in `ReturnVerified`.
    NotApplicable,
    /// A transient conflict (another vault operation in flight): retried next pass.
    Deferred(String),
    /// Nothing can be applied now: the whole receipt stays a backed payer claim.
    Held(String),
    /// A burn was admitted: the vault is excluded and frozen, the job persisted.
    BurnAdmitted { repay_e8s: u64, interest_share_e8s: u64 },
}

/// Why a vault cannot take a debt mutation right now (transient).
fn busy_reason(state: &State, vault_id: u64, now_ns: u64) -> Option<String> {
    let vault = state.vault_id_to_vaults.get(&vault_id)?;
    if crate::guard::is_vault_liquidating(vault_id) {
        return Some("another operation on the vault is in flight".into());
    }
    if vault.bot_processing || state.bot_claims.contains_key(&vault_id) {
        return Some("the vault has an unresolved bot liquidation".into());
    }
    if state.has_unresolved_push_sweep_for_vault(vault_id) {
        return Some("the vault has an unresolved push-deposit sweep".into());
    }
    if state.pending_borrow_mints.contains_key(&vault_id) {
        return Some("the vault has a pending borrow mint".into());
    }
    if state.xrpl_icusd.vault_exclusions.contains(&vault_id) {
        return Some("the vault is held by another XRPL icUSD operation".into());
    }
    if state.sp_xrp_absorb_preflights.contains_key(&vault_id) {
        let _ = now_ns;
        return Some("the vault has a pending stability-pool absorb reservation".into());
    }
    None
}

fn hold(state: &mut State, op_id: OperationId, why: &str, now_ns: u64) -> SettleOutcome {
    if let Ok(op) = state.xrpl_icusd.op_mut(op_id) {
        op.phase = Phase::Held;
        op.updated_at_ns = now_ns;
        op.push_note(format!("held, no burn: {why}"));
    }
    SettleOutcome::Held(why.to_string())
}

/// Decide what the funds actually received do to the vault's debt, and admit the
/// escrow burn. ONE synchronous block: nothing awaits between the re-read, the
/// accrual, the snapshot, the exclusion/freeze and the persisted burn job.
pub fn settle_return_against_debt(
    state: &mut State,
    op_id: OperationId,
    ledger_tip: u64,
    backend: Principal,
    now_ns: u64,
) -> Result<SettleOutcome, XrplIcusdError> {
    let (vault_id, owner, full_close, held, authorized, nonce, network, payer, tag, max_e8s) = {
        let op = state.xrpl_icusd.op(op_id).ok_or(XrplIcusdError::NotFound)?;
        let OperationKind::Repayment {
            vault_id,
            full_close,
            held,
            receipts,
            payer,
            destination_tag,
            max_e8s,
            ..
        } = &op.kind
        else {
            return Ok(SettleOutcome::NotApplicable);
        };
        if op.phase != Phase::ReturnVerified {
            return Ok(SettleOutcome::NotApplicable);
        }
        (
            *vault_id,
            op.owner,
            *full_close,
            *held,
            receipts
                .iter()
                .fold(0u64, |a, r| a.saturating_add(r.authorized_e8s)),
            op.request_nonce,
            op.network,
            payer.clone(),
            *destination_tag,
            *max_e8s,
        )
    };
    // The vault must still exist and belong to the payer's owner; a payment that
    // arrives after a liquidation or close is a held payer claim, never a resurrection
    // of the debt.
    let (collateral_type, vault_owner) = match state.vault_id_to_vaults.get(&vault_id) {
        Some(v) => (v.collateral_type, v.owner),
        None => return Ok(hold(state, op_id, "the vault no longer exists", now_ns)),
    };
    if let Some(why) = busy_reason(state, vault_id, now_ns) {
        return Ok(SettleOutcome::Deferred(why));
    }
    if vault_owner != owner {
        return Ok(hold(state, op_id, "the vault owner changed", now_ns));
    }
    if state
        .get_collateral_status(&collateral_type)
        .is_some_and(|s| !s.allows_repay())
    {
        return Ok(hold(state, op_id, "repayment is not allowed for this collateral", now_ns));
    }
    state.accrue_single_vault(vault_id, now_ns);
    let (debt, accrued, last_accrual) = {
        let v = &state.vault_id_to_vaults[&vault_id];
        (
            v.borrowed_icusd_amount.0,
            v.accrued_interest.0,
            v.last_accrual_time,
        )
    };
    if debt == 0 {
        return Ok(hold(state, op_id, "the vault has no debt (closed or liquidated)", now_ns));
    }
    // The funds actually received are the ceiling, further capped by what the owner
    // signed; growth of the debt never spends more than that.
    let received_e8s = u64::try_from(held.whole_e8s).unwrap_or(u64::MAX);
    let ceiling = received_e8s.min(authorized);
    let applicable = ceiling.min(debt);
    if applicable == 0 {
        return Ok(hold(state, op_id, "nothing applicable to the debt", now_ns));
    }
    let remainder = debt - applicable;
    let min_vault_debt = state
        .get_collateral_config(&collateral_type)
        .map(|c| c.min_vault_debt.0)
        .unwrap_or(0);
    if remainder > 0 && remainder < min_vault_debt {
        return Ok(hold(
            state,
            op_id,
            "applying this payment would leave forbidden dust debt; top up or return it",
            now_ns,
        ));
    }
    if remainder > 0 && !full_close && applicable < state.min_icusd_amount.0 {
        return Ok(hold(state, op_id, "below the minimum repayment amount", now_ns));
    }
    let interest_share =
        crate::state::proportional_repay_interest_share(applicable, debt, accrued);
    let cutoff = CutoffSnapshot {
        cutoff_ns: now_ns,
        snapshot_debt_e8s: debt,
        snapshot_accrued_e8s: accrued,
        repay_e8s: applicable,
        interest_share_e8s: interest_share,
        last_accrual_time: last_accrual,
    };
    record_lane_event(
        state,
        XrplIcusdEvent::RepayCutoffBegun {
            op_id,
            owner,
            request_nonce: nonce.to_vec(),
            network,
            vault_id,
            payer,
            destination_tag: tag,
            max_e8s,
            full_close,
            cutoff_ns: cutoff.cutoff_ns,
            snapshot_debt_e8s: cutoff.snapshot_debt_e8s,
            snapshot_accrued_e8s: cutoff.snapshot_accrued_e8s,
            repay_e8s: cutoff.repay_e8s,
            interest_share_e8s: cutoff.interest_share_e8s,
            last_accrual_time: cutoff.last_accrual_time,
        },
    );
    let op_nonce = state.next_op_nonce_at(now_ns);
    state.xrpl_icusd.add_job(
        op_id,
        ledger::burn_job_spec(backend, applicable, ledger_tip),
        op_nonce,
        now_ns,
    )?;
    Ok(SettleOutcome::BurnAdmitted {
        repay_e8s: applicable,
        interest_share_e8s: interest_share,
    })
}

impl State {
    /// Apply a settled repayment burn to the vault. Used by BOTH the live path and the
    /// replay of `XrplIcusdEvent::RepaySettled`. Idempotent per cutoff: the saved
    /// snapshot is applied absolutely (the accrual freeze guarantees nothing moved the
    /// vault meanwhile), then the freeze and exclusion are lifted and ONLY the unpaid
    /// remainder accrues from the saved clock up to `settled_at`.
    ///
    /// Returns `(collateral_type, interest_share_e8s)` for the live path to distribute.
    pub fn apply_xrpl_repay_settlement(
        &mut self,
        op_id: u64,
        vault_id: u64,
        settled_at: u64,
    ) -> Option<(Principal, u64)> {
        let cutoff = self.xrpl_icusd.operations.get(&op_id).and_then(|op| match &op.kind {
            OperationKind::Repayment { cutoff, .. } => cutoff.clone(),
            _ => None,
        });
        let already = self
            .xrpl_icusd
            .operations
            .get(&op_id)
            .is_some_and(|op| !matches!(op.phase, Phase::EscrowBurnPending));
        let mut distribute = None;
        if let (Some(c), false) = (cutoff, already) {
            if let Some(vault) = self.vault_id_to_vaults.get_mut(&vault_id) {
                vault.borrowed_icusd_amount =
                    ICUSD::new(c.snapshot_debt_e8s.saturating_sub(c.repay_e8s));
                vault.accrued_interest =
                    ICUSD::new(c.snapshot_accrued_e8s.saturating_sub(c.interest_share_e8s));
                vault.last_accrual_time = c.last_accrual_time;
                distribute = Some((vault.collateral_type, c.interest_share_e8s));
            }
        }
        // Lift the freeze first so the remainder (and only the remainder) catches up.
        self.xrpl_icusd.accrual_frozen_vaults.remove(&vault_id);
        self.accrue_single_vault(vault_id, settled_at);
        self.xrpl_icusd.apply_repay_settled(op_id, vault_id, settled_at);
        self.reindex_vault_cr(vault_id);
        distribute
    }

    /// Apply a proven never-executed burn: restore normal accrual on the FULL
    /// still-unpaid debt through `at`, release the exclusion, and move the value back
    /// to the payer's held claim.
    pub fn apply_xrpl_repay_cancellation(&mut self, op_id: u64, vault_id: u64, at: u64) {
        let was_pending = self
            .xrpl_icusd
            .operations
            .get(&op_id)
            .is_some_and(|op| op.phase == Phase::EscrowBurnPending);
        self.xrpl_icusd.accrual_frozen_vaults.remove(&vault_id);
        if was_pending {
            self.accrue_single_vault(vault_id, at);
        }
        self.xrpl_icusd.apply_repay_cancelled(op_id, vault_id, at);
        self.reindex_vault_cr(vault_id);
    }
}

// ----------------------------------------------------------------------------------
// Burn proof -> settled / cancelled
// ----------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepayAdvance {
    Waiting,
    Settled { interest: Option<PendingInterest> },
    Cancelled,
}

fn latest_burn(op: &Operation) -> Option<&IcLedgerJob> {
    op.ic_jobs
        .iter()
        .rfind(|j| j.purpose == IcJobPurpose::EscrowBurn)
}

/// Settle or cancel an admitted burn once its outcome is PROVEN. A burn that is only
/// possibly executed (`Unknown`, or `Done` with an unmatched block) waits: the vault
/// stays excluded and frozen, and nothing can cancel it.
pub fn advance_repayment(
    state: &mut State,
    op_id: OperationId,
    now_ns: u64,
) -> Result<RepayAdvance, XrplIcusdError> {
    let (vault_id, burn) = {
        let op = state.xrpl_icusd.op(op_id).ok_or(XrplIcusdError::NotFound)?;
        let OperationKind::Repayment { vault_id, .. } = &op.kind else {
            return Ok(RepayAdvance::Waiting);
        };
        if op.phase != Phase::EscrowBurnPending {
            return Ok(RepayAdvance::Waiting);
        }
        (*vault_id, latest_burn(op).cloned())
    };
    let Some(burn) = burn else {
        return Ok(RepayAdvance::Waiting);
    };
    if burn.is_proven_done() {
        record_lane_event(
            state,
            XrplIcusdEvent::RepaySettled {
                op_id,
                vault_id,
                burn_block: burn.done_block().unwrap_or_default(),
                settled_at: now_ns,
            },
        );
        // Live-only: queue the interest share for distribution (replay never does).
        let share = state.xrpl_icusd.op(op_id).and_then(|op| match &op.kind {
            OperationKind::Repayment { cutoff, .. } => cutoff.as_ref().map(|c| c.interest_share_e8s),
            _ => None,
        });
        let ct = state
            .vault_id_to_vaults
            .get(&vault_id)
            .map(|v| v.collateral_type);
        let mut interest = None;
        if let (Some(share), Some(ct)) = (share, ct) {
            if share > 0 {
                let pi = PendingInterest {
                    collateral_type: ct,
                    amount_e8s: share,
                };
                let op = state.xrpl_icusd.op_mut(op_id)?;
                if let OperationKind::Repayment {
                    pending_interest, ..
                } = &mut op.kind
                {
                    *pending_interest = Some(pi);
                }
                op.phase = Phase::DebtApplied;
                interest = Some(pi);
            }
        }
        return Ok(RepayAdvance::Settled { interest });
    }
    if let IcJobStatus::Failed { reason } = &burn.status {
        let reason = reason.clone();
        return cancel_unexecuted(state, op_id, now_ns).map(|_| {
            if let Ok(op) = state.xrpl_icusd.op_mut(op_id) {
                op.push_note(format!("burn proven never executed: {reason}; returned to a held claim"));
            }
            RepayAdvance::Cancelled
        });
    }
    Ok(RepayAdvance::Waiting)
}

/// Cancel an admitted burn that is PROVEN never executed. Refuses while the outcome is
/// ambiguous.
pub fn cancel_unexecuted(
    state: &mut State,
    op_id: OperationId,
    now_ns: u64,
) -> Result<(), XrplIcusdError> {
    let (vault_id, burn) = {
        let op = state.xrpl_icusd.op(op_id).ok_or(XrplIcusdError::NotFound)?;
        let OperationKind::Repayment { vault_id, .. } = &op.kind else {
            return Err(XrplIcusdError::NotApplicable("not a repayment".into()));
        };
        if op.phase != Phase::EscrowBurnPending {
            return Err(XrplIcusdError::WrongPhase(format!(
                "operation in phase {:?} has no admitted burn to cancel",
                op.phase
            )));
        }
        (*vault_id, latest_burn(op).cloned())
    };
    match burn.map(|j| j.status) {
        Some(IcJobStatus::Failed { .. }) => {}
        Some(_) | None => {
            return Err(XrplIcusdError::Ambiguous(
                "the escrow burn is not proven unexecuted; it may still have happened".into(),
            ))
        }
    }
    record_lane_event(
        state,
        XrplIcusdEvent::RepayCancelled {
            op_id,
            vault_id,
            at: now_ns,
        },
    );
    Ok(())
}

/// Take the queued interest share: the phase moves on and the field is cleared BEFORE
/// the distribution's await, so a crash can lose a distribution (as the legacy repay
/// can) but never repeat one.
pub fn take_pending_interest(
    st: &mut XrplIcusdState,
    op_id: OperationId,
    now_ns: u64,
) -> Option<PendingInterest> {
    let op = st.operations.get_mut(&op_id)?;
    let leftover = matches!(&op.kind, OperationKind::Repayment { held, .. } if !held.is_zero());
    let OperationKind::Repayment {
        pending_interest, ..
    } = &mut op.kind
    else {
        return None;
    };
    let taken = pending_interest.take()?;
    if op.phase == Phase::DebtApplied {
        op.phase = if leftover { Phase::Held } else { Phase::Complete };
    }
    op.updated_at_ns = now_ns;
    Some(taken)
}

// ----------------------------------------------------------------------------------
// Owner resolution of repayment claims
// ----------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepayResolutionPlan {
    /// Pay the entire held amount back to the payer on the XRPL (exact).
    ReissueHeld { amount: ExactIssuedAmount },
    /// Whole e8s of the held amount to the owner's canonical account; sub-e8s stays.
    ReceiveCanonicalHeld { owner: Principal, amount_e8s: u64 },
    /// Cancel an admitted burn proven never executed.
    CancelBurn,
    /// A halted refund gets a fresh delivery budget.
    RegrantRefund,
}

pub fn plan_repay_resolution(
    st: &XrplIcusdState,
    caller: &Principal,
    arg: &ResolveClaimArg,
) -> Result<RepayResolutionPlan, XrplIcusdError> {
    let op = st.op_for_owner(caller, arg.operation_id)?;
    match (&op.kind, arg.action) {
        (OperationKind::Refund { .. }, ClaimAction::ReissueXrpl) => {
            if op.phase == Phase::RefundPending && op.delivery_halted && op.unresolved_attempt().is_none() {
                Ok(RepayResolutionPlan::RegrantRefund)
            } else {
                Err(XrplIcusdError::WrongPhase(
                    "the refund is not halted; background delivery continues on its own".into(),
                ))
            }
        }
        (OperationKind::Refund { .. }, _) => Err(XrplIcusdError::NotApplicable(
            "a refund is resolved by the issuer sequencer".into(),
        )),
        (OperationKind::Repayment { .. }, ClaimAction::CancelUnexecutedRepayment) => {
            match latest_burn(op).map(|j| &j.status) {
                Some(IcJobStatus::Failed { .. }) if op.phase == Phase::EscrowBurnPending => {
                    Ok(RepayResolutionPlan::CancelBurn)
                }
                _ => Err(XrplIcusdError::Ambiguous(
                    "the escrow burn is not proven unexecuted; it cannot be cancelled".into(),
                )),
            }
        }
        (OperationKind::Repayment { held, .. }, action) => {
            if op.phase != Phase::Held {
                return Err(XrplIcusdError::WrongPhase(format!(
                    "operation in phase {:?} has no held claim to resolve",
                    op.phase
                )));
            }
            if op.resolution.is_some() || op.unresolved_attempt().is_some() {
                return Err(XrplIcusdError::WrongPhase(
                    "a resolution is already in progress".into(),
                ));
            }
            if op.ic_jobs.iter().any(|j| !j.is_resolved()) {
                return Err(XrplIcusdError::Ambiguous(
                    "a ledger job for this repayment is unresolved".into(),
                ));
            }
            if held.is_zero() {
                return Err(XrplIcusdError::WrongPhase("nothing is held".into()));
            }
            match action {
                ClaimAction::ReissueXrpl => {
                    let amount = held.to_exact_issued().map_err(|_| {
                        XrplIcusdError::NotApplicable(
                            "the held amount needs more than 16 significant digits and cannot \
                             be reissued in one payment; use the canonical return for the whole \
                             e8s part"
                                .into(),
                        )
                    })?;
                    Ok(RepayResolutionPlan::ReissueHeld { amount })
                }
                ClaimAction::ReceiveCanonical => {
                    let e8s = u64::try_from(held.whole_e8s).map_err(|_| {
                        XrplIcusdError::Internal("held amount exceeds u64 e8s".into())
                    })?;
                    if e8s == 0 {
                        return Err(XrplIcusdError::NotApplicable(
                            "less than one e8s is held; it stays held exactly".into(),
                        ));
                    }
                    Ok(RepayResolutionPlan::ReceiveCanonicalHeld {
                        owner: op.owner,
                        amount_e8s: e8s,
                    })
                }
                ClaimAction::CancelUnexecutedRepayment => unreachable!(),
            }
        }
        _ => Err(XrplIcusdError::NotApplicable(
            "not a repayment claim".into(),
        )),
    }
}

/// Move a repayment's whole held value into an owner-owned refund operation paying
/// the payer on the XRPL (same issuer sequencer, exact amount). Value moves in one
/// step: `held` of the parent becomes the refund's `amount`.
pub fn begin_reissue_held(
    st: &mut XrplIcusdState,
    parent_id: OperationId,
    now_ns: u64,
) -> Result<OperationId, XrplIcusdError> {
    let (owner, network, payer, amount, ordinal, vault) = {
        let op = st.op(parent_id).ok_or(XrplIcusdError::NotFound)?;
        let OperationKind::Repayment {
            payer,
            held,
            children,
            vault_id,
            ..
        } = &op.kind
        else {
            return Err(XrplIcusdError::NotApplicable("not a repayment".into()));
        };
        if op.phase != Phase::Held || op.resolution.is_some() {
            return Err(XrplIcusdError::WrongPhase("the claim is not resolvable".into()));
        }
        let amount = held.to_exact_issued().map_err(|_| {
            XrplIcusdError::NotApplicable("held amount is not exactly representable".into())
        })?;
        (op.owner, op.network, payer.clone(), amount, children.len(), *vault_id)
    };
    let _ = vault;
    let nonce = reissue_nonce(parent_id, ordinal);
    let id = st.allocate_operation_id();
    let mut child = Operation {
        id,
        owner,
        created_at_ns: now_ns,
        updated_at_ns: now_ns,
        request_nonce: nonce,
        network,
        kind: OperationKind::Refund {
            source_tx: format!("repayment:{parent_id}"),
            recipient: payer,
            amount,
        },
        phase: Phase::RefundPending,
        ic_jobs: vec![],
        xrpl_attempts: vec![],
        notes: vec![],
        resolution: None,
        attempt_budget_base: 0,
        delivery_halted: false,
    };
    child.push_note(format!("owner reissued the held claim of repayment #{parent_id}"));
    st.insert_operation(child)?;
    let parent = st.op_mut(parent_id)?;
    if let OperationKind::Repayment { held, children, .. } = &mut parent.kind {
        *held = WideAmount::ZERO;
        children.push(id);
    }
    parent.phase = Phase::ClaimResolved;
    parent.updated_at_ns = now_ns;
    parent.push_note(format!("held value moved to refund #{id}"));
    Ok(id)
}

/// Persist the escrow -> owner return of the whole-e8s part of a held claim.
#[allow(clippy::too_many_arguments)]
pub fn begin_receive_canonical_held(
    st: &mut XrplIcusdState,
    op_id: OperationId,
    fee_e8s: u64,
    escrow_balance_e8s: u128,
    op_nonce: u128,
    ledger_tip: u64,
    now_ns: u64,
) -> Result<usize, XrplIcusdError> {
    let (owner, amount) = {
        let op = st.op(op_id).ok_or(XrplIcusdError::NotFound)?;
        let OperationKind::Repayment { held, .. } = &op.kind else {
            return Err(XrplIcusdError::NotApplicable("not a repayment".into()));
        };
        if op.phase != Phase::Held || op.resolution.is_some() {
            return Err(XrplIcusdError::WrongPhase("the claim is not resolvable".into()));
        }
        let e8s = u64::try_from(held.whole_e8s)
            .map_err(|_| XrplIcusdError::Internal("held exceeds u64".into()))?;
        (op.owner, e8s)
    };
    if amount == 0 {
        return Err(XrplIcusdError::NotApplicable("less than one e8s is held".into()));
    }
    let required = st.required_backing_e8s(fee_e8s)?;
    if escrow_balance_e8s < required {
        return Err(XrplIcusdError::CoverageUnavailable(format!(
            "escrow holds {escrow_balance_e8s} e8s but {required} e8s are required including the \
             ledger fee reserve"
        )));
    }
    let idx = st.add_job(
        op_id,
        super::state::JobSpec {
            purpose: IcJobPurpose::CanonicalReturn,
            from_subaccount: Some(escrow_subaccount()),
            from_owner: None,
            to: Account {
                owner,
                subaccount: None,
            },
            amount_e8s: amount,
            reserved_fee_e8s: fee_e8s,
            ledger_tip_before: Some(ledger_tip),
        },
        op_nonce,
        now_ns,
    )?;
    let op = st.op_mut(op_id)?;
    op.resolution = Some(ClaimAction::ReceiveCanonical);
    op.push_note("owner chose the canonical return of the held claim");
    Ok(idx)
}

// ----------------------------------------------------------------------------------
// Async drivers
// ----------------------------------------------------------------------------------

/// Drive one repayment: settle a verified return against the debt, prove/settle/cancel
/// the burn, and distribute the interest share. Every step loads its inputs from state
/// and is idempotent.
pub async fn drive_repayment(op_id: OperationId) {
    let phase = read_state(|s| s.xrpl_icusd.op(op_id).map(|o| o.phase));
    if phase == Some(Phase::ReturnVerified) {
        let Ok(tip) = ledger::ledger_tip().await else {
            return;
        };
        let backend = ic_cdk::id();
        let outcome = mutate_state(|s| {
            settle_return_against_debt(s, op_id, tip, backend, ic_cdk::api::time())
        });
        if matches!(outcome, Ok(SettleOutcome::BurnAdmitted { .. })) {
            super::drive_ic_jobs(op_id).await;
        }
    }
    let advance = mutate_state(|s| advance_repayment(s, op_id, ic_cdk::api::time()));
    if matches!(advance, Ok(RepayAdvance::Settled { .. })) || pending_interest_exists(op_id) {
        distribute_interest_once(op_id).await;
    }
}

fn pending_interest_exists(op_id: OperationId) -> bool {
    read_state(|s| {
        s.xrpl_icusd.op(op_id).is_some_and(|o| {
            matches!(&o.kind, OperationKind::Repayment { pending_interest: Some(_), .. })
        })
    })
}

async fn distribute_interest_once(op_id: OperationId) {
    let taken = mutate_state(|s| take_pending_interest(&mut s.xrpl_icusd, op_id, ic_cdk::api::time()));
    let Some(pi) = taken else {
        return;
    };
    let unminted =
        crate::treasury::distribute_interest(ICUSD::new(pi.amount_e8s), pi.collateral_type).await;
    if unminted.to_u64() > 0 {
        mutate_state(|s| {
            s.restore_pending_interest_for_pool(pi.collateral_type, unminted.to_u64())
        });
    }
}

/// `prepare_xrpl_repayment`: owner-only, no awaits.
pub fn prepare_xrpl_repayment(
    caller: Principal,
    arg: PrepareRepaymentArg,
) -> Result<RepaymentIntentView, XrplIcusdError> {
    if caller == Principal::anonymous() {
        return Err(XrplIcusdError::AnonymousCaller);
    }
    let nonce = super::issuance::parse_request_nonce(&arg.request_nonce)?;
    let now = super::now();
    let req = IntentRequest {
        owner: caller,
        vault_id: arg.vault_id,
        max_e8s: arg.max_e8s,
        payer: arg.payer,
        network: arg.network,
        request_nonce: nonce,
        full_close: arg.full_close,
        top_up: arg.top_up_operation,
    };
    // An accepted request is idempotent; a NEW one is bounded per caller.
    let existing = read_state(|s| s.xrpl_icusd.request_nonce_index.contains_key(&(caller, nonce)));
    if !existing
        && !super::throttle_ok(
            super::THROTTLE_KIND_NEW,
            caller.as_slice(),
            now,
            super::NEW_REQUEST_GAP_NS,
        )
    {
        return Err(XrplIcusdError::LimitExceeded(
            "too many requests; retry in a few seconds".into(),
        ));
    }
    let (id, _) = mutate_state(|s| create_intent(s, &req, now))?;
    super::ensure_xrpl_tick_scheduled();
    read_state(|s| intent_view(&s.xrpl_icusd, id, now))
}

/// Owner resolution of a repayment/refund claim (`ResolveClaimArg`).
pub async fn resolve_repayment_claim(
    caller: Principal,
    arg: ResolveClaimArg,
) -> Result<OperationView, XrplIcusdError> {
    let plan = read_state(|s| plan_repay_resolution(&s.xrpl_icusd, &caller, &arg))?;
    match plan {
        RepayResolutionPlan::CancelBurn => {
            mutate_state(|s| cancel_unexecuted(s, arg.operation_id, super::now()))?;
        }
        RepayResolutionPlan::RegrantRefund => {
            mutate_state(|s| super::issuance::apply_reissue(&mut s.xrpl_icusd, arg.operation_id, super::now()))?;
            super::ensure_xrpl_tick_scheduled();
        }
        RepayResolutionPlan::ReissueHeld { .. } => {
            let child = mutate_state(|s| {
                // Re-plan after no awaits: the same synchronous block.
                plan_repay_resolution(&s.xrpl_icusd, &caller, &arg)?;
                begin_reissue_held(&mut s.xrpl_icusd, arg.operation_id, super::now())
            })?;
            super::ensure_xrpl_tick_scheduled();
            let _ = super::run_op_step(child).await;
        }
        RepayResolutionPlan::ReceiveCanonicalHeld { .. } => {
            let ledger_id = read_state(|s| s.icusd_ledger_principal);
            let fee = crate::management::get_or_refresh_fee(ledger_id)
                .await
                .map_err(XrplIcusdError::Unavailable)?;
            let balance = ledger::escrow_balance_e8s()
                .await
                .map_err(XrplIcusdError::Unavailable)?;
            let tip = ledger::ledger_tip()
                .await
                .map_err(|e| XrplIcusdError::Unavailable(format!("ledger tip: {e}")))?;
            let idx = mutate_state(|s| {
                plan_repay_resolution(&s.xrpl_icusd, &caller, &arg)?;
                let op_nonce = s.next_op_nonce();
                begin_receive_canonical_held(
                    &mut s.xrpl_icusd,
                    arg.operation_id,
                    fee,
                    balance,
                    op_nonce,
                    tip,
                    super::now(),
                )
            })?;
            let _ = ledger::execute_job(arg.operation_id, idx).await;
            let _ = ledger::verify_done_job(arg.operation_id, idx).await;
            mutate_state(|s| {
                let _ = super::issuance::advance_resolution(&mut s.xrpl_icusd, arg.operation_id, super::now());
            });
            super::ensure_xrpl_tick_scheduled();
        }
    }
    super::view_for(&caller, arg.operation_id)
}
