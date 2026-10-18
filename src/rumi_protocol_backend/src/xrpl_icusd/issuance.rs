//! Backed delivery claims: canonical-to-XRPL issuance and owner claim resolution.
//!
//! `issue_xrpl_from_canonical` is the only entry point of this task that creates a
//! claim, and it creates NO debt and charges NO borrowing fee: the user's own
//! canonical icUSD, pulled with an explicit ICRC-2 allowance, becomes escrow backing
//! (`D`), and delivery then follows the shared pipeline (issuer Payment, `D -> O`
//! at validated delivery). The same pipeline carries Borrow claims in Task 4.

use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;

use super::state::{Admission, JobSpec, XrplIcusdState};
use super::types::*;
use crate::chains::xrp::address::is_valid_classic_address;
use crate::chains::xrp::network::XrplNetwork;

pub fn parse_request_nonce(v: &[u8]) -> Result<[u8; 32], XrplIcusdError> {
    <[u8; 32]>::try_from(v)
        .map_err(|_| XrplIcusdError::InvalidArgument("request_nonce must be exactly 32 bytes".into()))
}

/// Recipient: a CLASSIC address only (an X-address would carry a tag that must not be
/// silently dropped) and never the issuer itself.
pub fn validate_recipient(recipient: &str, issuer: &str) -> Result<(), XrplIcusdError> {
    if !is_valid_classic_address(recipient) {
        return Err(XrplIcusdError::InvalidArgument(
            "recipient must be a valid classic XRPL address".into(),
        ));
    }
    if recipient == issuer {
        return Err(XrplIcusdError::InvalidArgument(
            "recipient cannot be the issuer".into(),
        ));
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct ImportRequest {
    pub owner: Principal,
    pub recipient: String,
    pub amount_e8s: u64,
    pub network: XrplNetwork,
    pub request_nonce: [u8; 32],
}

/// Create the Import operation and its persisted ICRC-2 pull job in ONE synchronous
/// step (after every await of the caller). The same request nonce returns the same
/// operation and never a second pull.
pub fn create_import(
    st: &mut XrplIcusdState,
    req: &ImportRequest,
    op_nonce: u128,
    ledger_tip: u64,
    backend: Principal,
    now_ns: u64,
) -> Result<(OperationId, bool), XrplIcusdError> {
    st.ensure_open_for_issuance()?;
    if Some(req.network) != st.config.network {
        return Err(XrplIcusdError::WrongNetwork(
            "request network differs from the configured network".into(),
        ));
    }
    if req.owner == Principal::anonymous() {
        return Err(XrplIcusdError::AnonymousCaller);
    }
    let kind = OperationKind::Import {
        amount_e8s: req.amount_e8s,
        recipient: req.recipient.clone(),
    };
    match st.admit(&req.owner, &req.request_nonce, req.network, &kind)? {
        Admission::Existing(id) => return Ok((id, false)),
        Admission::New => {}
    }
    st.check_caps(req.amount_e8s)?;
    let id = st.allocate_operation_id();
    st.insert_operation(Operation {
        id,
        owner: req.owner,
        created_at_ns: now_ns,
        updated_at_ns: now_ns,
        request_nonce: req.request_nonce,
        network: req.network,
        kind,
        phase: Phase::EscrowPullPending,
        ic_jobs: vec![],
        xrpl_attempts: vec![],
        notes: vec![],
        resolution: None,
        attempt_budget_base: 0,
        delivery_halted: false,
    })?;
    st.add_job(
        id,
        JobSpec {
            purpose: IcJobPurpose::ImportPull,
            from_subaccount: None,
            from_owner: Some(Account {
                owner: req.owner,
                subaccount: None,
            }),
            to: escrow_account(backend),
            amount_e8s: req.amount_e8s,
            reserved_fee_e8s: 0,
            ledger_tip_before: Some(ledger_tip),
        },
        op_nonce,
        now_ns,
    )?;
    Ok((id, true))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportAdvance {
    /// Backing is proven in escrow: the delivery claim exists.
    Funded,
    /// The pull definitively failed: nothing was funded, nothing is owed.
    Cancelled,
    Waiting,
}

/// Turn a proven pull into a delivery claim. A claim exists ONLY after the pull block
/// was fetched back and matched the persisted tuple.
pub fn advance_import(
    st: &mut XrplIcusdState,
    op_id: OperationId,
    now_ns: u64,
) -> Result<ImportAdvance, XrplIcusdError> {
    let op = st.op_mut(op_id)?;
    if !matches!(op.kind, OperationKind::Import { .. }) || op.phase != Phase::EscrowPullPending {
        return Ok(ImportAdvance::Waiting);
    }
    let Some(job) = op.ic_jobs.iter().find(|j| j.purpose == IcJobPurpose::ImportPull) else {
        return Ok(ImportAdvance::Waiting);
    };
    if job.is_proven_done() {
        op.phase = Phase::IssuePrepared;
        op.updated_at_ns = now_ns;
        return Ok(ImportAdvance::Funded);
    }
    if let IcJobStatus::Failed { reason } = &job.status {
        let reason = reason.clone();
        op.phase = Phase::Cancelled;
        op.updated_at_ns = now_ns;
        op.push_note(format!("canonical pull failed: {reason}"));
        return Ok(ImportAdvance::Cancelled);
    }
    Ok(ImportAdvance::Waiting)
}

// ----------------------------------------------------------------------------------
// Owner claim resolution
// ----------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolutionPlan {
    Reissue,
    ReceiveCanonical { owner: Principal, amount_e8s: u64 },
}

/// Validate an owner's resolution request. Refused while any signed issuance for the
/// claim is ambiguous (the token may still land), or any issuer attempt is unresolved.
pub fn plan_resolution(
    st: &XrplIcusdState,
    caller: &Principal,
    arg: &ResolveClaimArg,
) -> Result<ResolutionPlan, XrplIcusdError> {
    let op = st.op_for_owner(caller, arg.operation_id)?;
    let Some(amount) = op.kind.claim_e8s() else {
        return Err(XrplIcusdError::NotApplicable(
            "only delivery claims can be resolved here".into(),
        ));
    };
    if arg.action == ClaimAction::CancelUnexecutedRepayment {
        return Err(XrplIcusdError::NotApplicable(
            "repayment claims are resolved by the repayment flow".into(),
        ));
    }
    // Ambiguity first: a signed issuance may still land, whatever phase it is in.
    if op.has_signed_ambiguity() {
        return Err(XrplIcusdError::Ambiguous(
            "a signed issuance for this claim is unresolved; it may still be delivered".into(),
        ));
    }
    if op.unresolved_attempt().is_some() {
        return Err(XrplIcusdError::Ambiguous(
            "an issuer attempt for this claim is in progress".into(),
        ));
    }
    if !matches!(op.phase, Phase::DebtCommitted | Phase::IssuePrepared) {
        return Err(XrplIcusdError::WrongPhase(format!(
            "operation in phase {:?} is not a resolvable claim",
            op.phase
        )));
    }
    if op.resolution.is_some() {
        return Err(XrplIcusdError::WrongPhase(
            "a resolution is already in progress".into(),
        ));
    }
    match arg.action {
        ClaimAction::ReissueXrpl => {
            if !op.delivery_halted {
                return Err(XrplIcusdError::WrongPhase(
                    "delivery is not halted; background delivery will continue on its own".into(),
                ));
            }
            Ok(ResolutionPlan::Reissue)
        }
        ClaimAction::ReceiveCanonical => Ok(ResolutionPlan::ReceiveCanonical {
            owner: op.owner,
            amount_e8s: amount,
        }),
        ClaimAction::CancelUnexecutedRepayment => unreachable!(),
    }
}

impl XrplIcusdState {
    /// Backing the escrow must hold for every liability, the buffer, pending IC fees
    /// and `extra_fee_e8s` of a new canonical return.
    pub fn required_backing_e8s(&self, extra_fee_e8s: u64) -> Result<u128, XrplIcusdError> {
        let b = self.buckets()?;
        let mut sum = b.o;
        for v in [&b.d, &b.r, &b.h] {
            sum = sum
                .checked_add(v)
                .map_err(|e| XrplIcusdError::Internal(format!("{e:?}")))?;
        }
        Ok(sum.ceil_e8s()
            + u128::from(self.config.rounding_buffer_e8s)
            + b.ic_fee_reserve_e8s
            + u128::from(extra_fee_e8s))
    }
}

pub fn apply_reissue(
    st: &mut XrplIcusdState,
    op_id: OperationId,
    now_ns: u64,
) -> Result<(), XrplIcusdError> {
    let op = st.op_mut(op_id)?;
    op.delivery_halted = false;
    op.attempt_budget_base = op.xrpl_attempts.len() as u32;
    op.updated_at_ns = now_ns;
    op.push_note("owner requested reissue: fresh delivery budget");
    Ok(())
}

/// Create the persisted escrow -> owner return job. The IC ledger fee is reserved
/// SEPARATELY from holder backing: the escrow must cover the claim, the buffer and
/// this fee.
#[allow(clippy::too_many_arguments)]
pub fn begin_receive_canonical(
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
        let amount = op
            .kind
            .claim_e8s()
            .ok_or_else(|| XrplIcusdError::NotApplicable("not a delivery claim".into()))?;
        if op.resolution.is_some() || op.has_signed_ambiguity() || op.unresolved_attempt().is_some() {
            return Err(XrplIcusdError::Ambiguous(
                "the claim cannot be returned while a resolution or issuer attempt is unresolved"
                    .into(),
            ));
        }
        (op.owner, amount)
    };
    let required = st.required_backing_e8s(fee_e8s)?;
    if escrow_balance_e8s < required {
        return Err(XrplIcusdError::CoverageUnavailable(format!(
            "escrow holds {escrow_balance_e8s} e8s but {required} e8s are required including the \
             ledger fee reserve"
        )));
    }
    let idx = st.add_job(
        op_id,
        JobSpec {
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
    op.push_note("owner chose the canonical return");
    Ok(idx)
}

/// Complete or unwind a `ReceiveCanonical` resolution once its job resolves.
pub fn advance_resolution(
    st: &mut XrplIcusdState,
    op_id: OperationId,
    now_ns: u64,
) -> Result<bool, XrplIcusdError> {
    let op = st.op_mut(op_id)?;
    if op.resolution != Some(ClaimAction::ReceiveCanonical) {
        return Ok(false);
    }
    let Some(job) = op
        .ic_jobs
        .iter()
        .rfind(|j| j.purpose == IcJobPurpose::CanonicalReturn)
    else {
        return Ok(false);
    };
    if job.is_proven_done() {
        op.phase = Phase::ClaimResolved;
        op.updated_at_ns = now_ns;
        return Ok(true);
    }
    if let IcJobStatus::Failed { reason } = &job.status {
        let reason = reason.clone();
        // Proven not executed: the claim is untouched and may be resolved again.
        op.resolution = None;
        op.push_note(format!("canonical return failed: {reason}"));
    }
    Ok(false)
}
