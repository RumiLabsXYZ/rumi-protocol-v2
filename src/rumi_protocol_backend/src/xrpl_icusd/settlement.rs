//! The issuer sequencer: build, sign, persist, submit and reconcile XRPL transactions
//! from the lane's threshold-Ed25519 issuer account.
//!
//! Safety rules, all in the pure functions (the async drivers only inject
//! observations):
//!
//! * Single flight: one unresolved issuer transaction at a time (the lease holder is
//!   the operation id, or `Setup`). A lease is released ONLY by a validated outcome
//!   (success, or a `tec*` that consumed the sequence) or by a complete-history
//!   non-inclusion proof after expiry.
//! * Persist before submit: the unsigned bytes and sequence are saved before signing;
//!   the signature, signed blob and hash are saved before any submit. A signing or
//!   submission ambiguity KEEPS the lease; there is no administrator "mark failed".
//! * Expiry: the validated ledger must be past `LastLedgerSequence`, providers'
//!   `complete_ledgers` must cover `[prepared_ledger, LLS + 1]`, and the transaction
//!   must be not-found according to those same providers.
//! * Setup requires an explicit issuer policy; it cannot change once setup started;
//!   clawback is refused unless the issuer owns nothing; the final account flags are
//!   verified before the issuer is `Ready`.

use super::state::XrplIcusdState;
use super::types::*;
use crate::chains::xrp::address::account_id_from_classic_address;
use crate::chains::xrp::issued_amount::{ExactIssuedAmount, IssuedAsset};
use crate::chains::xrp::issued_codec::{
    serialize_account_set_signed, serialize_account_set_unsigned, serialize_issued_payment_signed,
    serialize_issued_payment_unsigned, AccountSet, IssuedPayment, ASF_DEFAULT_RIPPLE,
    ASF_REQUIRE_DEST, TRANSFER_RATE_PAR,
};
use crate::chains::xrp::issued_rpc::{ReceiptLookup, VerifiedIssuedReceipt, CreditEligibility};
use crate::chains::xrp::issuer_rpc::{
    ExpiryEvidence, IssuerAccountInfo, TxOutcome, LSF_ALLOW_TRUSTLINE_CLAWBACK, LSF_DEFAULT_RIPPLE,
    LSF_NO_FREEZE, LSF_REQUIRE_DEST_TAG,
};
use crate::chains::xrp::network::XrplNetwork;
use crate::chains::xrp::sign::{ed25519_signing_pubkey, signing_message, tx_hash};

/// `tfFullyCanonicalSig`.
pub const TF_FULLY_CANONICAL: u32 = 0x8000_0000;
pub const ASF_NO_FREEZE: u32 = 6;
pub const ASF_ALLOW_TRUSTLINE_CLAWBACK: u32 = 16;
/// Ledgers (~3-4s each) an issuer transaction stays valid for.
pub const LAST_LEDGER_WINDOW: u32 = 40;
/// Identical-blob resubmissions before only reconciliation continues.
pub const MAX_SUBMITS: u32 = 8;
pub const RESUBMIT_AFTER_NS: u64 = 20_000_000_000;

// ----------------------------------------------------------------------------------
// Building
// ----------------------------------------------------------------------------------

/// `AccountSet` parameters for one setup step.
pub fn account_set_params(step: IssuerSetupStep) -> (Option<u32>, Option<u32>) {
    match step {
        IssuerSetupStep::DefaultRipple => (Some(ASF_DEFAULT_RIPPLE), None),
        IssuerSetupStep::RequireDest => (Some(ASF_REQUIRE_DEST), None),
        IssuerSetupStep::TransferRate => (None, Some(TRANSFER_RATE_PAR)),
        IssuerSetupStep::AllowClawback => (Some(ASF_ALLOW_TRUSTLINE_CLAWBACK), None),
        IssuerSetupStep::NoFreeze => (Some(ASF_NO_FREEZE), None),
    }
}

pub struct PrepareInput<'a> {
    pub purpose: AttemptPurpose,
    pub tx: AttemptTx,
    pub issuer_address: &'a str,
    pub issuer_pubkey: [u8; 32],
    pub sequence: u32,
    pub prepared_ledger: u32,
    pub fee_drops: u64,
    pub max_fee_drops: u64,
    pub now_ns: u64,
}

enum Built {
    Payment(IssuedPayment),
    AccountSet(AccountSet),
}

fn build_tx(a: &IssuerTxAttempt, pubkey32: &[u8; 32]) -> Result<Built, XrplIcusdError> {
    let account = account_id_from_classic_address(&a.account)
        .map_err(|e| XrplIcusdError::Internal(format!("issuer address: {e}")))?;
    let pk33 = ed25519_signing_pubkey(pubkey32);
    match &a.tx {
        AttemptTx::AccountSet {
            set_flag,
            transfer_rate,
        } => Ok(Built::AccountSet(AccountSet {
            account,
            fee_drops: a.fee_drops,
            sequence: a.sequence,
            last_ledger_sequence: a.last_ledger_sequence,
            flags: Some(TF_FULLY_CANONICAL),
            set_flag: *set_flag,
            clear_flag: None,
            transfer_rate: *transfer_rate,
            signing_pub_key: pk33,
        })),
        AttemptTx::Payment {
            destination,
            amount,
            destination_tag,
        } => {
            let asset = IssuedAsset::icusd(&a.account)
                .map_err(|e| XrplIcusdError::Internal(format!("issuer asset: {e}")))?;
            Ok(Built::Payment(IssuedPayment {
                account,
                destination: account_id_from_classic_address(destination)
                    .map_err(|e| XrplIcusdError::InvalidArgument(format!("destination: {e}")))?,
                amount: *amount,
                asset,
                fee_drops: a.fee_drops,
                sequence: a.sequence,
                last_ledger_sequence: a.last_ledger_sequence,
                destination_tag: *destination_tag,
                flags: Some(TF_FULLY_CANONICAL),
                signing_pub_key: pk33,
            }))
        }
    }
}

fn unsigned_of(b: &Built) -> Result<Vec<u8>, XrplIcusdError> {
    match b {
        Built::Payment(p) => serialize_issued_payment_unsigned(p),
        Built::AccountSet(a) => serialize_account_set_unsigned(a),
    }
    .map_err(|e| XrplIcusdError::Internal(format!("serialize: {e}")))
}

fn signed_of(b: &Built, sig: &[u8]) -> Result<Vec<u8>, XrplIcusdError> {
    match b {
        Built::Payment(p) => serialize_issued_payment_signed(p, sig),
        Built::AccountSet(a) => serialize_account_set_signed(a, sig),
    }
    .map_err(|e| XrplIcusdError::Internal(format!("serialize signed: {e}")))
}

/// Build the unsigned attempt (status `Prepared`). The fee is bounded by
/// `max_fee_drops`; `LastLedgerSequence` is `prepared_ledger + LAST_LEDGER_WINDOW`.
pub fn prepare_attempt(inp: &PrepareInput) -> Result<IssuerTxAttempt, XrplIcusdError> {
    if inp.max_fee_drops == 0 || inp.fee_drops == 0 || inp.fee_drops > inp.max_fee_drops {
        return Err(XrplIcusdError::Unavailable(format!(
            "fee {} drops is outside (0, max_fee_drops = {}]",
            inp.fee_drops, inp.max_fee_drops
        )));
    }
    let lls = inp
        .prepared_ledger
        .checked_add(LAST_LEDGER_WINDOW)
        .ok_or_else(|| XrplIcusdError::Internal("ledger sequence overflow".into()))?;
    let mut a = IssuerTxAttempt {
        purpose: inp.purpose,
        tx: inp.tx.clone(),
        account: inp.issuer_address.to_string(),
        sequence: inp.sequence,
        last_ledger_sequence: lls,
        prepared_ledger: inp.prepared_ledger,
        fee_drops: inp.fee_drops,
        unsigned_hex: String::new(),
        signed_hex: None,
        tx_hash: None,
        status: AttemptStatus::Prepared,
        created_at_ns: inp.now_ns,
        submit_count: 0,
        last_submit_ns: 0,
        note: None,
    };
    let unsigned = unsigned_of(&build_tx(&a, &inp.issuer_pubkey)?)?;
    a.unsigned_hex = hex::encode_upper(unsigned);
    Ok(a)
}

/// The exact bytes the threshold signer must sign (`STX\0 || unsigned`).
pub fn message_to_sign(a: &IssuerTxAttempt) -> Result<Vec<u8>, XrplIcusdError> {
    let unsigned = hex::decode(&a.unsigned_hex)
        .map_err(|e| XrplIcusdError::Internal(format!("persisted unsigned blob: {e}")))?;
    Ok(signing_message(&unsigned))
}

/// Attach a signature: re-derive the unsigned bytes from the persisted fields and
/// require them to equal the persisted blob (the signature is only valid for it).
pub fn finalize_signature(
    a: &mut IssuerTxAttempt,
    sig: &[u8],
    pubkey32: &[u8; 32],
) -> Result<String, XrplIcusdError> {
    if a.status != AttemptStatus::Prepared {
        return Err(XrplIcusdError::WrongPhase("attempt is not awaiting a signature".into()));
    }
    let built = build_tx(a, pubkey32)?;
    if hex::encode_upper(unsigned_of(&built)?) != a.unsigned_hex {
        return Err(XrplIcusdError::Internal(
            "persisted unsigned blob does not match the attempt fields".into(),
        ));
    }
    let signed = signed_of(&built, sig)?;
    let hash = hex::encode_upper(tx_hash(&signed));
    a.signed_hex = Some(hex::encode_upper(&signed));
    a.tx_hash = Some(hash.clone());
    a.status = AttemptStatus::Signed;
    Ok(hash)
}

// ----------------------------------------------------------------------------------
// Reconciliation (pure)
// ----------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum TxView {
    NotFound,
    NotValidated,
    Failed(String),
    Succeeded {
        ledger_index: u32,
        tx_type: Option<String>,
        account: Option<String>,
        payment: Option<Box<VerifiedIssuedReceipt>>,
    },
}

#[derive(Clone, Debug)]
pub struct Observation {
    pub tx: TxView,
    /// Present ONLY when the lookup behind `tx` was restricted to the providers named
    /// in the evidence (so a `NotFound` really means "not in complete history").
    pub expiry: Option<ExpiryEvidence>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reconciled {
    Validated { ledger_index: u32 },
    FailedOnLedger(String),
    Expired { validated_ledger: u32 },
    Quarantine(String),
    Pending,
}

pub fn view_from_receipt(r: ReceiptLookup) -> TxView {
    match r {
        ReceiptLookup::NotFound => TxView::NotFound,
        ReceiptLookup::NotValidated => TxView::NotValidated,
        ReceiptLookup::Failed(res) => TxView::Failed(res),
        ReceiptLookup::Verified(b) => TxView::Succeeded {
            ledger_index: b.ledger_index,
            tx_type: Some("Payment".into()),
            account: Some(b.account.clone()),
            payment: Some(b),
        },
    }
}

pub fn view_from_outcome(o: TxOutcome) -> TxView {
    match o {
        TxOutcome::NotFound => TxView::NotFound,
        TxOutcome::NotValidated => TxView::NotValidated,
        TxOutcome::Failed(r) => TxView::Failed(r),
        TxOutcome::Succeeded {
            ledger_index,
            tx_type,
            account,
        } => TxView::Succeeded {
            ledger_index,
            tx_type: Some(tx_type),
            account: Some(account),
            payment: None,
        },
    }
}

/// Decide what an observation means for an unresolved attempt.
pub fn reconcile(a: &IssuerTxAttempt, obs: &Observation) -> Reconciled {
    if !a.status.holds_lease() {
        return Reconciled::Pending;
    }
    match &obs.tx {
        TxView::Failed(res) => Reconciled::FailedOnLedger(res.clone()),
        TxView::NotValidated => Reconciled::Pending,
        TxView::NotFound => match &obs.expiry {
            Some(ev)
                if ev.validated_ledger > a.last_ledger_sequence
                    && ev.history_from <= a.prepared_ledger
                    && ev.history_to > a.last_ledger_sequence =>
            {
                Reconciled::Expired {
                    validated_ledger: ev.validated_ledger,
                }
            }
            _ => Reconciled::Pending,
        },
        TxView::Succeeded {
            ledger_index,
            tx_type,
            account,
            payment,
        } => {
            if account.as_deref() != Some(a.account.as_str()) {
                return Reconciled::Quarantine("validated transaction has a different account".into());
            }
            match &a.tx {
                AttemptTx::AccountSet { .. } => {
                    if tx_type.as_deref() == Some("AccountSet") {
                        Reconciled::Validated {
                            ledger_index: *ledger_index,
                        }
                    } else {
                        Reconciled::Quarantine("validated transaction is not an AccountSet".into())
                    }
                }
                AttemptTx::Payment {
                    destination,
                    amount,
                    destination_tag,
                } => match payment {
                    None => Reconciled::Quarantine("no payment receipt for a payment attempt".into()),
                    Some(r) => match check_receipt(a, r, destination, amount, *destination_tag) {
                        Ok(()) => Reconciled::Validated {
                            ledger_index: r.ledger_index,
                        },
                        Err(m) => Reconciled::Quarantine(m),
                    },
                },
            }
        }
    }
}

fn check_receipt(
    a: &IssuerTxAttempt,
    r: &VerifiedIssuedReceipt,
    destination: &str,
    amount: &ExactIssuedAmount,
    tag: Option<u32>,
) -> Result<(), String> {
    if !r.validated || r.engine_result != "tesSUCCESS" {
        return Err("receipt is not a validated success".into());
    }
    if a.tx_hash.as_deref().map(str::to_ascii_uppercase) != Some(r.hash.to_ascii_uppercase()) {
        return Err("receipt hash differs from the persisted transaction hash".into());
    }
    if r.account != a.account {
        return Err("receipt account differs from the issuer".into());
    }
    if r.destination != destination {
        return Err("receipt destination differs from the authorized recipient".into());
    }
    if r.destination_tag != tag {
        return Err("receipt destination tag differs".into());
    }
    if r.delivered_amount != *amount {
        return Err("delivered amount differs from the authorized amount".into());
    }
    if r.credit_eligibility() != CreditEligibility::Eligible {
        return Err("receipt is a partial/path/anomalous payment".into());
    }
    Ok(())
}

// ----------------------------------------------------------------------------------
// Setup decisions (pure)
// ----------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepDecision {
    Send,
    AlreadySatisfied,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupAction {
    Start(IssuerSetupStep),
    Verify,
    Ready,
}

/// Is this step needed, allowed, or already done on the live account?
pub fn step_precondition(
    step: IssuerSetupStep,
    info: &IssuerAccountInfo,
    has_owner_objects: bool,
) -> Result<StepDecision, XrplIcusdError> {
    if !info.exists {
        return Err(XrplIcusdError::Disabled("the issuer account is not funded".into()));
    }
    let f = info.flags;
    match step {
        IssuerSetupStep::DefaultRipple => Ok(if f & LSF_DEFAULT_RIPPLE != 0 {
            StepDecision::AlreadySatisfied
        } else {
            StepDecision::Send
        }),
        IssuerSetupStep::RequireDest => Ok(if f & LSF_REQUIRE_DEST_TAG != 0 {
            StepDecision::AlreadySatisfied
        } else {
            StepDecision::Send
        }),
        IssuerSetupStep::TransferRate => Ok(if info.transfer_rate == Some(TRANSFER_RATE_PAR) {
            StepDecision::AlreadySatisfied
        } else {
            StepDecision::Send
        }),
        IssuerSetupStep::AllowClawback => {
            if f & LSF_ALLOW_TRUSTLINE_CLAWBACK != 0 {
                return Ok(StepDecision::AlreadySatisfied);
            }
            if f & LSF_NO_FREEZE != 0 {
                return Err(XrplIcusdError::Policy(
                    "NoFreeze is already set; clawback can no longer be enabled".into(),
                ));
            }
            if info.owner_count != 0 || has_owner_objects {
                return Err(XrplIcusdError::Policy(
                    "AllowTrustLineClawback requires the issuer owner directory to be empty \
                     (it is irreversible and must precede every trust line)"
                        .into(),
                ));
            }
            Ok(StepDecision::Send)
        }
        IssuerSetupStep::NoFreeze => {
            if f & LSF_NO_FREEZE != 0 {
                return Ok(StepDecision::AlreadySatisfied);
            }
            if f & LSF_ALLOW_TRUSTLINE_CLAWBACK != 0 {
                return Err(XrplIcusdError::Policy(
                    "clawback is enabled; NoFreeze cannot also be enabled".into(),
                ));
            }
            Ok(StepDecision::Send)
        }
    }
}

/// The account must match the chosen policy EXACTLY before the issuer is `Ready`.
pub fn verify_final_flags(
    info: &IssuerAccountInfo,
    policy: &IssuerPolicy,
) -> Result<(), XrplIcusdError> {
    let f = info.flags;
    let mismatch = |what: &str| XrplIcusdError::Policy(format!("issuer account mismatch: {what}"));
    if !info.exists {
        return Err(mismatch("account does not exist"));
    }
    if f & LSF_DEFAULT_RIPPLE == 0 {
        return Err(mismatch("DefaultRipple is not set"));
    }
    if f & LSF_REQUIRE_DEST_TAG == 0 {
        return Err(mismatch("RequireDest is not set"));
    }
    if !matches!(info.transfer_rate, None | Some(TRANSFER_RATE_PAR)) {
        return Err(mismatch("TransferRate is not par"));
    }
    if (f & LSF_ALLOW_TRUSTLINE_CLAWBACK != 0) != policy.allow_clawback {
        return Err(mismatch("clawback flag differs from the policy"));
    }
    if (f & LSF_NO_FREEZE != 0) != policy.no_freeze {
        return Err(mismatch("NoFreeze flag differs from the policy"));
    }
    Ok(())
}

// ----------------------------------------------------------------------------------
// State transitions
// ----------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconcileEffect {
    None,
    Delivered,
    SetupStepValidated,
    AttemptReleased,
    Halted,
    Quarantined,
}

impl XrplIcusdState {
    pub fn lease_attempt(&self, holder: LeaseHolder) -> Option<&IssuerTxAttempt> {
        match holder {
            LeaseHolder::Setup => self
                .setup_attempt
                .as_ref()
                .filter(|a| a.status.holds_lease()),
            LeaseHolder::Operation(id) => self.op(id).and_then(|o| {
                o.xrpl_attempts
                    .iter()
                    .find(|a| a.status.holds_lease())
            }),
        }
    }

    fn lease_attempt_mut(&mut self, holder: LeaseHolder) -> Option<&mut IssuerTxAttempt> {
        match holder {
            LeaseHolder::Setup => self
                .setup_attempt
                .as_mut()
                .filter(|a| a.status.holds_lease()),
            LeaseHolder::Operation(id) => self
                .operations
                .get_mut(&id)
                .and_then(|o| o.xrpl_attempts.iter_mut().find(|a| a.status.holds_lease())),
        }
    }

    pub fn next_setup_action(&self) -> Result<SetupAction, XrplIcusdError> {
        let policy = self.config.policy.as_ref().ok_or_else(|| {
            XrplIcusdError::Policy(
                "an explicit issuer policy (allow_clawback, no_freeze) must be chosen before setup"
                    .into(),
            )
        })?;
        policy.validate()?;
        if self.config.network.is_none() || self.config.issuer_address.is_none() {
            return Err(XrplIcusdError::Disabled(
                "network and issuer address must be configured first".into(),
            ));
        }
        Ok(match &self.config.issuer_setup {
            IssuerSetupState::NotStarted => {
                SetupAction::Start(IssuerSetupStep::sequence_for(policy)[0])
            }
            IssuerSetupState::Configuring { step } => SetupAction::Start(*step),
            IssuerSetupState::Verifying => SetupAction::Verify,
            IssuerSetupState::Ready { .. } => SetupAction::Ready,
        })
    }

    /// Mark a step satisfied WITHOUT a transaction (the account already has it).
    pub fn setup_step_satisfied(&mut self, step: IssuerSetupStep) -> Result<(), XrplIcusdError> {
        let policy = self.config.policy.ok_or_else(|| {
            XrplIcusdError::Policy("issuer policy must be chosen before setup".into())
        })?;
        self.config.issuer_setup = match step.next_after(&policy) {
            Some(n) => IssuerSetupState::Configuring { step: n },
            None => IssuerSetupState::Verifying,
        };
        self.config.config_version += 1;
        Ok(())
    }

    pub fn finish_setup(&mut self, ledger_index: u32) {
        self.config.issuer_setup = IssuerSetupState::Ready { ledger_index };
        self.config.config_version += 1;
    }

    /// Persist a new prepared attempt and take the single issuer lease. Nothing is
    /// signed or sent until this has been saved.
    pub fn begin_attempt(
        &mut self,
        holder: LeaseHolder,
        attempt: IssuerTxAttempt,
    ) -> Result<(), XrplIcusdError> {
        if self.config.settlement_paused {
            return Err(XrplIcusdError::Paused("settlement is paused".into()));
        }
        if self.issuer_lease.is_some() {
            return Err(XrplIcusdError::Ambiguous(
                "another issuer transaction is unresolved".into(),
            ));
        }
        match holder {
            LeaseHolder::Setup => {
                let AttemptPurpose::Setup { step } = attempt.purpose else {
                    return Err(XrplIcusdError::Internal("setup lease with a non-setup attempt".into()));
                };
                if self
                    .setup_attempt
                    .as_ref()
                    .is_some_and(|a| a.status.holds_lease())
                {
                    return Err(XrplIcusdError::Ambiguous("a setup attempt is unresolved".into()));
                }
                // Locks the policy: configure() refuses a different one from now on.
                self.config.issuer_setup = IssuerSetupState::Configuring { step };
                self.setup_attempt = Some(attempt);
            }
            LeaseHolder::Operation(id) => {
                let op = self.op_mut(id)?;
                if op.phase != Phase::IssuePrepared
                    || op.delivery_halted
                    || op.resolution.is_some()
                    || op.unresolved_attempt().is_some()
                {
                    return Err(XrplIcusdError::WrongPhase(
                        "operation is not ready for a new issuer attempt".into(),
                    ));
                }
                let used = (op.xrpl_attempts.len() as u32).saturating_sub(op.attempt_budget_base);
                if used >= MAX_ATTEMPTS_PER_BUDGET {
                    op.delivery_halted = true;
                    return Err(XrplIcusdError::WrongPhase(
                        "delivery attempt budget exhausted".into(),
                    ));
                }
                op.xrpl_attempts.push(attempt);
            }
        }
        self.issuer_lease = Some(holder);
        Ok(())
    }

    pub fn record_signature(
        &mut self,
        holder: LeaseHolder,
        sig: &[u8],
        pubkey32: &[u8; 32],
        now_ns: u64,
    ) -> Result<String, XrplIcusdError> {
        let a = self
            .lease_attempt_mut(holder)
            .ok_or_else(|| XrplIcusdError::WrongPhase("no unresolved attempt".into()))?;
        let hash = finalize_signature(a, sig, pubkey32)?;
        if let LeaseHolder::Operation(id) = holder {
            let op = self.op_mut(id)?;
            op.phase = Phase::Signed;
            op.updated_at_ns = now_ns;
        }
        Ok(hash)
    }

    pub fn note_attempt(&mut self, holder: LeaseHolder, note: String) {
        if let Some(a) = self.lease_attempt_mut(holder) {
            let mut n = note;
            n.truncate(MAX_NOTE_LEN);
            a.note = Some(n);
        }
    }

    /// A submit was ATTEMPTED (whatever came back): the blob may be on the network.
    pub fn record_submit(
        &mut self,
        holder: LeaseHolder,
        now_ns: u64,
        note: Option<String>,
    ) -> Result<(), XrplIcusdError> {
        let a = self
            .lease_attempt_mut(holder)
            .ok_or_else(|| XrplIcusdError::WrongPhase("no unresolved attempt".into()))?;
        if !matches!(a.status, AttemptStatus::Signed | AttemptStatus::Submitted) {
            return Err(XrplIcusdError::WrongPhase("attempt is not signed".into()));
        }
        a.status = AttemptStatus::Submitted;
        a.submit_count += 1;
        a.last_submit_ns = now_ns;
        if let Some(n) = note {
            let mut n = n;
            n.truncate(MAX_NOTE_LEN);
            a.note = Some(n);
        }
        if let LeaseHolder::Operation(id) = holder {
            let op = self.op_mut(id)?;
            op.phase = Phase::Submitted;
            op.updated_at_ns = now_ns;
        }
        Ok(())
    }

    /// Apply a reconciliation verdict. Idempotent: only an attempt that still holds
    /// the lease changes, so a repeated verdict can never double-credit `O`.
    pub fn apply_reconcile(
        &mut self,
        holder: LeaseHolder,
        verdict: Reconciled,
        now_ns: u64,
    ) -> Result<ReconcileEffect, XrplIcusdError> {
        let Some(att) = self.lease_attempt(holder) else {
            return Ok(ReconcileEffect::None);
        };
        let tx = att.tx.clone();
        let purpose = att.purpose;
        let release = |st: &mut XrplIcusdState| {
            if st.issuer_lease == Some(holder) {
                st.issuer_lease = None;
            }
        };
        match verdict {
            Reconciled::Pending => Ok(ReconcileEffect::None),
            Reconciled::Quarantine(reason) => {
                let a = self.lease_attempt_mut(holder).expect("checked above");
                a.status = AttemptStatus::Quarantined { reason: reason.clone() };
                if let LeaseHolder::Operation(id) = holder {
                    self.op_mut(id)?.push_note(format!("issuer transaction quarantined: {reason}"));
                }
                Ok(ReconcileEffect::Quarantined)
            }
            Reconciled::Validated { ledger_index } => {
                let a = self.lease_attempt_mut(holder).expect("checked above");
                a.status = AttemptStatus::Validated { ledger_index };
                release(self);
                match (holder, purpose, tx) {
                    (LeaseHolder::Setup, AttemptPurpose::Setup { step }, _) => {
                        self.setup_step_satisfied(step)?;
                        Ok(ReconcileEffect::SetupStepValidated)
                    }
                    (LeaseHolder::Operation(id), _, AttemptTx::Payment { amount, .. }) => {
                        let add = WideAmount::from_exact(&amount).map_err(|e| {
                            XrplIcusdError::Internal(format!("delivered amount: {e:?}"))
                        })?;
                        self.issued_outstanding = self
                            .issued_outstanding
                            .checked_add(&add)
                            .map_err(|e| XrplIcusdError::Internal(format!("O overflow: {e:?}")))?;
                        let op = self.op_mut(id)?;
                        // D -> O exactly once, in the same step as the validation.
                        op.phase = Phase::Delivered;
                        op.updated_at_ns = now_ns;
                        Ok(ReconcileEffect::Delivered)
                    }
                    _ => Err(XrplIcusdError::Internal("validated attempt/holder mismatch".into())),
                }
            }
            Reconciled::FailedOnLedger(_) | Reconciled::Expired { .. } => {
                let a = self.lease_attempt_mut(holder).expect("checked above");
                a.status = match &verdict {
                    Reconciled::FailedOnLedger(r) => AttemptStatus::FailedOnLedger { result: r.clone() },
                    Reconciled::Expired { validated_ledger } => AttemptStatus::Expired {
                        validated_ledger: *validated_ledger,
                    },
                    _ => unreachable!(),
                };
                release(self);
                if let LeaseHolder::Operation(id) = holder {
                    let op = self.op_mut(id)?;
                    // The claim is intact; a successor attempt (fresh sequence read)
                    // may follow, within the budget.
                    op.phase = Phase::IssuePrepared;
                    op.updated_at_ns = now_ns;
                    let used = (op.xrpl_attempts.len() as u32).saturating_sub(op.attempt_budget_base);
                    if used >= MAX_ATTEMPTS_PER_BUDGET {
                        op.delivery_halted = true;
                        op.push_note("delivery halted: attempt budget exhausted; the owner may reissue or take the canonical claim");
                        return Ok(ReconcileEffect::Halted);
                    }
                }
                Ok(ReconcileEffect::AttemptReleased)
            }
        }
    }
}

/// Is a resubmission of the identical blob due?
pub fn resubmit_due(a: &IssuerTxAttempt, now_ns: u64) -> bool {
    a.status == AttemptStatus::Submitted
        && a.submit_count < MAX_SUBMITS
        && now_ns.saturating_sub(a.last_submit_ns) >= RESUBMIT_AFTER_NS
}

/// Network tag/identity context loaded from config for one driver pass.
#[derive(Clone, Debug)]
pub struct IssuerCtx {
    pub network: XrplNetwork,
    pub issuer_address: String,
    pub issuer_pubkey: [u8; 32],
    pub max_fee_drops: u64,
    pub settlement_paused: bool,
}

impl XrplIcusdState {
    pub fn issuer_ctx(&self) -> Result<IssuerCtx, XrplIcusdError> {
        let c = &self.config;
        let pk: [u8; 32] = c
            .issuer_public_key
            .as_deref()
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| XrplIcusdError::Disabled("issuer key not derived".into()))?;
        Ok(IssuerCtx {
            network: c
                .network
                .ok_or_else(|| XrplIcusdError::Disabled("network not configured".into()))?,
            issuer_address: c
                .issuer_address
                .clone()
                .ok_or_else(|| XrplIcusdError::Disabled("issuer address not derived".into()))?,
            issuer_pubkey: pk,
            max_fee_drops: c.max_fee_drops,
            settlement_paused: c.settlement_paused,
        })
    }
}

// ----------------------------------------------------------------------------------
// Async drivers
// ----------------------------------------------------------------------------------

use crate::chains::xrp::issued_rpc::{fetch_issuer_obligation_snapshot, fetch_trust_line, IssuedRpcError};
use crate::chains::xrp::issuer_rpc as rpc;
use crate::chains::xrp::ted25519;

fn now() -> u64 {
    ic_cdk::api::time()
}

fn unavailable(e: IssuedRpcError) -> XrplIcusdError {
    XrplIcusdError::Unavailable(e.to_string())
}

/// Look the attempt's transaction up. A `NotFound` is only trusted for expiry when it
/// comes from providers proven to hold the whole interval.
pub async fn gather_observation(
    ctx: &IssuerCtx,
    a: &IssuerTxAttempt,
) -> Result<Observation, IssuedRpcError> {
    let hash = a
        .tx_hash
        .clone()
        .ok_or_else(|| IssuedRpcError::InvalidInput("attempt has no hash yet".into()))?;
    let is_payment = matches!(a.tx, AttemptTx::Payment { .. });
    let asset = IssuedAsset::icusd(&ctx.issuer_address)
        .map_err(IssuedRpcError::Asset)?;
    let lookup = |only: Option<Vec<String>>| {
        let hash = hash.clone();
        async move {
            if is_payment {
                rpc::fetch_receipt_via(ctx.network, &hash, &asset, only.as_deref())
                    .await
                    .map(view_from_receipt)
            } else {
                rpc::fetch_tx_outcome(ctx.network, &hash, only.as_deref())
                    .await
                    .map(view_from_outcome)
            }
        }
    };
    let first = lookup(None).await?;
    if !matches!(first, TxView::NotFound) {
        return Ok(Observation {
            tx: first,
            expiry: None,
        });
    }
    // Not found: can it be proven never included?
    match rpc::fetch_expiry_evidence(ctx.network, a.prepared_ledger, a.last_ledger_sequence).await {
        Ok(ev) => {
            let restricted = lookup(Some(ev.providers.clone())).await?;
            Ok(Observation {
                expiry: matches!(restricted, TxView::NotFound).then_some(ev),
                tx: restricted,
            })
        }
        // History not (yet) complete or the ledger not past LLS: keep waiting.
        Err(IssuedRpcError::HistoryGap { .. }) => Ok(Observation {
            tx: TxView::NotFound,
            expiry: None,
        }),
        Err(e) => Err(e),
    }
}

/// Advance the lease holder's unresolved attempt as far as it can go in this call:
/// sign -> submit -> reconcile (and resubmit). Every step persists first.
pub async fn drive_attempt(holder: LeaseHolder) -> Result<ReconcileEffect, XrplIcusdError> {
    let mut effect = ReconcileEffect::None;
    for _ in 0..4 {
        let (ctx, att) = crate::state::read_state(|s| {
            let ctx = s.xrpl_icusd.issuer_ctx();
            let att = s.xrpl_icusd.lease_attempt(holder).cloned();
            (ctx, att)
        });
        let ctx = ctx?;
        let Some(att) = att else {
            return Ok(effect);
        };
        match att.status {
            AttemptStatus::Prepared => {
                if ctx.settlement_paused {
                    return Ok(effect);
                }
                let msg = message_to_sign(&att)?;
                let path = issuer_derivation_path(ctx.network);
                match ted25519::sign_message(msg, path).await {
                    Ok(sig) => {
                        crate::state::mutate_state(|s| {
                            s.xrpl_icusd
                                .record_signature(holder, &sig, &ctx.issuer_pubkey, now())
                        })?;
                    }
                    Err(e) => {
                        // Nothing was persisted as signed; the lease and the sequence
                        // stay, and the same unsigned attempt is re-signed later.
                        crate::state::mutate_state(|s| {
                            s.xrpl_icusd.note_attempt(holder, format!("signing failed: {e}"))
                        });
                        return Ok(effect);
                    }
                }
            }
            AttemptStatus::Signed => {
                if ctx.settlement_paused {
                    return Ok(effect);
                }
                submit(&ctx, holder, &att).await?;
            }
            AttemptStatus::Submitted => {
                match gather_observation(&ctx, &att).await {
                    Ok(obs) => {
                        let verdict = reconcile(&att, &obs);
                        let e = crate::state::mutate_state(|s| {
                            s.xrpl_icusd.apply_reconcile(holder, verdict, now())
                        })?;
                        if e != ReconcileEffect::None {
                            return Ok(e);
                        }
                        effect = e;
                    }
                    Err(e) => {
                        crate::state::mutate_state(|s| {
                            s.xrpl_icusd
                                .note_attempt(holder, format!("reconcile deferred: {e}"))
                        });
                        return Ok(effect);
                    }
                }
                if !ctx.settlement_paused && resubmit_due(&att, now()) {
                    submit(&ctx, holder, &att).await?;
                }
                return Ok(effect);
            }
            AttemptStatus::Quarantined { .. } => return Ok(ReconcileEffect::Quarantined),
            _ => return Ok(effect),
        }
    }
    Ok(effect)
}

async fn submit(
    ctx: &IssuerCtx,
    holder: LeaseHolder,
    att: &IssuerTxAttempt,
) -> Result<(), XrplIcusdError> {
    let blob = att
        .signed_hex
        .clone()
        .ok_or_else(|| XrplIcusdError::Internal("signed attempt without a blob".into()))?;
    // Persist "submit attempted" BEFORE the call: from here on the blob may exist on
    // the network, whatever the call returns.
    crate::state::mutate_state(|s| s.xrpl_icusd.record_submit(holder, now(), None))?;
    let outcome = rpc::submit_signed_blob(ctx.network, &blob).await;
    crate::state::mutate_state(|s| {
        s.xrpl_icusd
            .note_attempt(holder, format!("submit: {outcome:?}"))
    });
    Ok(())
}

/// Coverage against the real escrow balance and a fixed-ledger issuer snapshot.
pub async fn refresh_coverage() -> Result<CoverageReport, XrplIcusdError> {
    use super::state::{CoverageInput, SnapshotSummary};
    let ctx = crate::state::read_state(|s| s.xrpl_icusd.issuer_ctx())?;
    let asset = IssuedAsset::icusd(&ctx.issuer_address)
        .map_err(|e| XrplIcusdError::Internal(e.to_string()))?;
    let balance = super::ledger::escrow_balance_e8s()
        .await
        .map_err(XrplIcusdError::Unavailable)?;
    let snapshot = match fetch_issuer_obligation_snapshot(ctx.network, &asset).await {
        Ok(s) => Some(SnapshotSummary {
            ledger_index: s.ledger_index,
            total: WideAmount::from_obligation(&s.total)
                .map_err(|e| XrplIcusdError::Internal(format!("snapshot: {e:?}")))?,
        }),
        Err(e) if e.is_recoverable() => None,
        Err(IssuedRpcError::SnapshotTooLarge { .. }) => None,
        Err(e) => return Err(unavailable(e)),
    };
    crate::state::mutate_state(|s| {
        let report = s.xrpl_icusd.check_coverage(&CoverageInput {
            escrow_balance_e8s: balance,
            snapshot,
            unprocessed_return_allowance: WideAmount::ZERO,
        })?;
        s.xrpl_icusd.last_coverage = Some(CoverageRecord {
            checked_at_ns: now(),
            report: report.clone(),
        });
        Ok(report)
    })
}

/// A coverage record young enough to authorize starting a new issuer attempt.
pub const COVERAGE_FRESH_NS: u64 = 15 * 60 * 1_000_000_000;

pub fn coverage_permits(st: &XrplIcusdState, now_ns: u64) -> bool {
    st.last_coverage.as_ref().is_some_and(|c| {
        c.report.permits_issuance() && now_ns.saturating_sub(c.checked_at_ns) <= COVERAGE_FRESH_NS
    })
}

/// Start (if needed) and advance the delivery of one backed claim.
pub async fn drive_delivery(op_id: OperationId) -> Result<ReconcileEffect, XrplIcusdError> {
    let holder = LeaseHolder::Operation(op_id);
    let (ctx, kind_ok, unresolved, start_ok, recipient, amount_e8s) = crate::state::read_state(|s| {
        let st = &s.xrpl_icusd;
        let ctx = st.issuer_ctx();
        let Some(op) = st.op(op_id) else {
            return (ctx, false, false, false, String::new(), 0);
        };
        let claim = op.kind.claim_e8s();
        let kind_ok = claim.is_some() && !op.phase.is_terminal();
        let unresolved = op.unresolved_attempt().is_some();
        let start_ok = op.phase == Phase::IssuePrepared
            && !op.delivery_halted
            && op.resolution.is_none()
            && st.issuer_lease.is_none();
        (
            ctx,
            kind_ok,
            unresolved,
            start_ok,
            op.kind.recipient().unwrap_or_default().to_string(),
            claim.unwrap_or(0),
        )
    });
    let ctx = ctx?;
    if !kind_ok {
        return Ok(ReconcileEffect::None);
    }
    if unresolved {
        return drive_attempt(holder).await;
    }
    if !start_ok || ctx.settlement_paused {
        return Ok(ReconcileEffect::None);
    }
    // ---- start a new attempt ----
    let _issuer = super::IssuerGuard::acquire()?;
    if !crate::state::read_state(|s| coverage_permits(&s.xrpl_icusd, now())) {
        let report = refresh_coverage().await?;
        if !report.permits_issuance() {
            crate::state::mutate_state(|s| {
                if let Ok(op) = s.xrpl_icusd.op_mut(op_id) {
                    op.push_note(format!("issuance waiting on coverage: {:?}", report.status));
                }
            });
            return Ok(ReconcileEffect::None);
        }
    }
    let asset = IssuedAsset::icusd(&ctx.issuer_address)
        .map_err(|e| XrplIcusdError::Internal(e.to_string()))?;
    let proof = fetch_trust_line(ctx.network, &recipient, &asset)
        .await
        .map_err(unavailable)?;
    if let Err(e) = check_trust_line(proof.line.as_ref(), amount_e8s) {
        crate::state::mutate_state(|s| {
            if let Ok(op) = s.xrpl_icusd.op_mut(op_id) {
                op.push_note(format!("delivery waiting on trust line: {e}"));
            }
        });
        return Ok(ReconcileEffect::None);
    }
    let info = rpc::fetch_issuer_account(ctx.network, &ctx.issuer_address)
        .await
        .map_err(unavailable)?;
    if !info.exists {
        return Err(XrplIcusdError::Disabled("the issuer account is not funded".into()));
    }
    let base_fee = rpc::fetch_base_fee(ctx.network).await.map_err(unavailable)?;
    let reserve = rpc::fetch_reserve_base(ctx.network).await.map_err(unavailable)?;
    let attempts_so_far = crate::state::read_state(|s| {
        s.xrpl_icusd
            .op(op_id)
            .map(|o| o.xrpl_attempts.len() as u32)
            .unwrap_or(0)
    });
    // Deterministic fee escalation per successor attempt, bounded by max_fee_drops.
    let fee = base_fee
        .saturating_mul(1u64 << attempts_so_far.min(6))
        .min(ctx.max_fee_drops);
    // Never sign a transaction the issuer cannot pay for above its reserve.
    if let Err(m) = rpc::issuer_can_pay(&info, fee, reserve) {
        crate::state::mutate_state(|s| {
            if let Ok(op) = s.xrpl_icusd.op_mut(op_id) {
                op.push_note(format!("delivery waiting on issuer funding: {m}"));
            }
        });
        return Ok(ReconcileEffect::None);
    }
    let amount = ExactIssuedAmount::try_from_e8s(amount_e8s)
        .map_err(|e| XrplIcusdError::InvalidArgument(format!("amount: {e}")))?;
    let attempt = prepare_attempt(&PrepareInput {
        purpose: AttemptPurpose::Delivery,
        tx: AttemptTx::Payment {
            destination: recipient,
            amount,
            destination_tag: None,
        },
        issuer_address: &ctx.issuer_address,
        issuer_pubkey: ctx.issuer_pubkey,
        sequence: info.sequence,
        prepared_ledger: info.ledger_index,
        fee_drops: fee,
        max_fee_drops: ctx.max_fee_drops,
        now_ns: now(),
    })?;
    crate::state::mutate_state(|s| s.xrpl_icusd.begin_attempt(holder, attempt))?;
    drive_attempt(holder).await
}

/// Pure trust-line admission: the line must exist, not be frozen by the issuer, and
/// have room for the amount.
pub fn check_trust_line(
    line: Option<&crate::chains::xrp::issued_rpc::TrustLine>,
    amount_e8s: u64,
) -> Result<(), XrplIcusdError> {
    let line = line.ok_or_else(|| {
        XrplIcusdError::TrustLineMissing("the recipient has no trust line for icUSD".into())
    })?;
    if line.freeze_peer {
        return Err(XrplIcusdError::TrustLineMissing(
            "the issuer has frozen this trust line".into(),
        ));
    }
    let amount = ExactIssuedAmount::try_from_e8s(amount_e8s)
        .map_err(|e| XrplIcusdError::InvalidArgument(format!("amount: {e}")))?;
    let room = line.limit.checked_sub(&line.balance).map_err(|_| {
        XrplIcusdError::TrustLineMissing("trust-line limit/balance cannot be evaluated exactly".into())
    })?;
    let after = room.checked_sub(&amount).map_err(|_| {
        XrplIcusdError::TrustLineMissing("trust-line room cannot be evaluated exactly".into())
    })?;
    if after.is_negative() {
        return Err(XrplIcusdError::TrustLineMissing(
            "the trust-line limit is too low for this amount".into(),
        ));
    }
    Ok(())
}

/// One controller-driven setup step (or reconciliation of the in-flight one).
pub async fn run_setup_step() -> Result<IssuerSetupStatus, XrplIcusdError> {
    let _issuer = super::IssuerGuard::acquire()?;
    let status = |note: Option<String>| {
        crate::state::read_state(|s| IssuerSetupStatus {
            setup: s.xrpl_icusd.config.issuer_setup.clone(),
            issuer_address: s.xrpl_icusd.config.issuer_address.clone(),
            attempt_status: s.xrpl_icusd.setup_attempt.as_ref().map(|a| a.status.clone()),
            note,
        })
    };
    // An unresolved attempt is reconciled first.
    if crate::state::read_state(|s| s.xrpl_icusd.lease_attempt(LeaseHolder::Setup).is_some()) {
        drive_attempt(LeaseHolder::Setup).await?;
        return Ok(status(None));
    }
    let (action, ctx, policy) = crate::state::read_state(|s| {
        let st = &s.xrpl_icusd;
        (st.next_setup_action(), st.issuer_ctx(), st.config.policy)
    });
    let action = action?;
    let ctx = ctx?;
    let policy = policy.ok_or_else(|| XrplIcusdError::Policy("policy required".into()))?;
    match action {
        SetupAction::Ready => Ok(status(Some("issuer already ready".into()))),
        SetupAction::Verify => {
            let info = rpc::fetch_issuer_account(ctx.network, &ctx.issuer_address)
                .await
                .map_err(unavailable)?;
            verify_final_flags(&info, &policy)?;
            crate::state::mutate_state(|s| s.xrpl_icusd.finish_setup(info.ledger_index));
            Ok(status(Some("issuer flags verified".into())))
        }
        SetupAction::Start(step) => {
            let info = rpc::fetch_issuer_account(ctx.network, &ctx.issuer_address)
                .await
                .map_err(unavailable)?;
            let has_objects = if step == IssuerSetupStep::AllowClawback {
                rpc::fetch_has_owner_objects(ctx.network, &ctx.issuer_address)
                    .await
                    .map_err(unavailable)?
            } else {
                false
            };
            match step_precondition(step, &info, has_objects)? {
                StepDecision::AlreadySatisfied => {
                    crate::state::mutate_state(|s| s.xrpl_icusd.setup_step_satisfied(step))?;
                    Ok(status(Some(format!("{step:?} already satisfied"))))
                }
                StepDecision::Send => {
                    let base_fee = rpc::fetch_base_fee(ctx.network).await.map_err(unavailable)?;
                    let reserve = rpc::fetch_reserve_base(ctx.network).await.map_err(unavailable)?;
                    rpc::issuer_can_pay(&info, base_fee.min(ctx.max_fee_drops), reserve)
                        .map_err(XrplIcusdError::Unavailable)?;
                    let (set_flag, transfer_rate) = account_set_params(step);
                    let attempt = prepare_attempt(&PrepareInput {
                        purpose: AttemptPurpose::Setup { step },
                        tx: AttemptTx::AccountSet {
                            set_flag,
                            transfer_rate,
                        },
                        issuer_address: &ctx.issuer_address,
                        issuer_pubkey: ctx.issuer_pubkey,
                        sequence: info.sequence,
                        prepared_ledger: info.ledger_index,
                        fee_drops: base_fee.min(ctx.max_fee_drops),
                        max_fee_drops: ctx.max_fee_drops,
                        now_ns: now(),
                    })?;
                    crate::state::mutate_state(|s| {
                        s.xrpl_icusd.begin_attempt(LeaseHolder::Setup, attempt)
                    })?;
                    drive_attempt(LeaseHolder::Setup).await?;
                    Ok(status(None))
                }
            }
        }
    }
}
