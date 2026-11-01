//! Stable and wire types for the XRPL icUSD lane.
//!
//! * State types are `serde` (ciborium snapshot). Anything holding an exact XRPL
//!   amount (`ExactIssuedAmount`) is serde-only; Candid views carry decimal text.
//! * Every field added after the first release must be `#[serde(default)]`; the
//!   whole lane hangs off `State.xrpl_icusd`, which is itself `#[serde(default)]`,
//!   so an old snapshot decodes to a DISABLED lane.
//! * All money is `u64` e8s or `ExactIssuedAmount` / [`WideAmount`]; never f64.

use candid::{CandidType, Principal};
use icrc_ledger_types::icrc1::account::Account;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::chains::xrp::issued_amount::{AmountError, ExactIssuedAmount};
use crate::chains::xrp::issued_rpc::{IssuerHistoryCursor, ObligationTotal};
use crate::chains::xrp::network::XrplNetwork;

pub type OperationId = u64;

/// 10 active operations per owner, 10,000 operations globally, 50 rows per page.
pub const MAX_ACTIVE_OPS_PER_OWNER: usize = 10;
/// Cap on USER-created operations (system refund records never count against it, so
/// an attacker who pushes refund records can never make user admissions fail).
pub const MAX_OPERATIONS_GLOBAL: usize = 10_000;
pub const MAX_PAGE: u32 = 50;
pub const MAX_NOTES: usize = 16;
pub const MAX_NOTE_LEN: usize = 240;
/// Unsuccessful (proven non-executed) issuer attempts per delivery budget before the
/// claim stops auto-retrying and waits for the owner (`ReissueXrpl`/`ReceiveCanonical`).
pub const MAX_ATTEMPTS_PER_BUDGET: u32 = 5;

/// Escrow subaccount: `sha256("rumi:xrpl-icusd:escrow:v1")`. Never the minting
/// account (the default account of the backend).
pub const ESCROW_DOMAIN: &[u8] = b"rumi:xrpl-icusd:escrow:v1";

pub fn escrow_subaccount() -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(ESCROW_DOMAIN);
    h.finalize().into()
}

/// The backend's escrow account on the canonical icUSD ledger.
pub fn escrow_account(backend: Principal) -> Account {
    Account {
        owner: backend,
        subaccount: Some(escrow_subaccount()),
    }
}

/// The backend's default account: the icUSD ledger's minting account.
pub fn backend_default_account(backend: Principal) -> Account {
    Account {
        owner: backend,
        subaccount: None,
    }
}

/// Issuer key derivation path: `[b"rumi", b"xrpl-icusd", b"v1", network tag]`.
pub fn issuer_derivation_path(network: XrplNetwork) -> Vec<Vec<u8>> {
    vec![
        b"rumi".to_vec(),
        b"xrpl-icusd".to_vec(),
        b"v1".to_vec(),
        network.name().as_bytes().to_vec(),
    ]
}

// ----------------------------------------------------------------------------------
// Errors
// ----------------------------------------------------------------------------------

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum XrplIcusdError {
    /// The lane is not configured/enabled/ready.
    Disabled(String),
    /// New issuance is paused.
    Paused(String),
    AnonymousCaller,
    /// Not the caller's operation (indistinguishable from "does not exist").
    NotFound,
    /// Controller-only method.
    Unauthorized,
    InvalidArgument(String),
    LimitExceeded(String),
    /// The same request nonce was already used for DIFFERENT parameters.
    NonceConflict,
    WrongNetwork(String),
    TrustLineMissing(String),
    /// Backing/obligation coverage does not currently permit this.
    CoverageUnavailable(String),
    /// A dependency (ledger, XRPL providers) is unavailable; retry later.
    Unavailable(String),
    /// A signed issuance for this claim is unresolved; resolution is refused.
    Ambiguous(String),
    WrongPhase(String),
    /// Issuer policy not chosen / mismatched / locked.
    Policy(String),
    MintingAccount(String),
    NotApplicable(String),
    Internal(String),
}

impl std::fmt::Display for XrplIcusdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

// ----------------------------------------------------------------------------------
// Wide exact accumulator
// ----------------------------------------------------------------------------------

/// Sub-e8s fractions are tracked in units of 1e-40 icUSD (1 e8s == 1e32 units),
/// matching `ObligationTotal`. Sums are exact and never round; a value finer than
/// 1e-40 is REFUSED (callers hold it) rather than approximated.
pub const UNITS_PER_E8S: u128 = 100_000_000_000_000_000_000_000_000_000_000;

#[derive(
    CandidType, Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord,
)]
pub struct WideAmount {
    pub whole_e8s: u128,
    /// `< UNITS_PER_E8S`.
    pub frac_units: u128,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WideError {
    Overflow,
    Underflow,
    /// Finer than 1e-40 or negative.
    Unrepresentable,
    Amount(AmountError),
}

impl WideAmount {
    pub const ZERO: WideAmount = WideAmount {
        whole_e8s: 0,
        frac_units: 0,
    };

    pub fn from_e8s(e8s: u64) -> Self {
        Self {
            whole_e8s: u128::from(e8s),
            frac_units: 0,
        }
    }

    pub fn is_zero(&self) -> bool {
        self.whole_e8s == 0 && self.frac_units == 0
    }

    /// Exact conversion of a non-negative XRPL amount.
    pub fn from_exact(a: &ExactIssuedAmount) -> Result<Self, WideError> {
        if a.is_negative() {
            return Err(WideError::Unrepresentable);
        }
        let (floor, rem) = a.split_e8s().map_err(WideError::Amount)?;
        let mut out = WideAmount::from_e8s(floor);
        if !rem.is_zero() {
            // rem < 1e-8: mantissa in [1e15, 1e16), exponent <= -24.
            let shift = i64::from(rem.exponent()) + 40;
            let units = if shift >= 0 {
                u128::from(rem.mantissa())
                    .checked_mul(10u128.pow(shift as u32))
                    .ok_or(WideError::Overflow)?
            } else {
                let k = (-shift) as u32;
                if k > 16 {
                    return Err(WideError::Unrepresentable);
                }
                let div = 10u128.pow(k);
                let m = u128::from(rem.mantissa());
                if m % div != 0 {
                    return Err(WideError::Unrepresentable);
                }
                m / div
            };
            if units >= UNITS_PER_E8S {
                return Err(WideError::Unrepresentable);
            }
            out.frac_units = units;
        }
        Ok(out)
    }

    /// Conservative reading of an XRPL snapshot total (finer-than-1e-40 dust rounds UP).
    pub fn from_obligation(t: &ObligationTotal) -> Result<Self, WideError> {
        let frac_total = t
            .sub_e8s_units_1e40
            .checked_add(u128::from(t.finer_dust_lines > 0))
            .ok_or(WideError::Overflow)?;
        let whole = t
            .whole_e8s
            .checked_add(frac_total / UNITS_PER_E8S)
            .ok_or(WideError::Overflow)?;
        Ok(Self {
            whole_e8s: whole,
            frac_units: frac_total % UNITS_PER_E8S,
        })
    }

    pub fn checked_add(&self, o: &Self) -> Result<Self, WideError> {
        let mut frac = self.frac_units + o.frac_units; // each < 1e32: no overflow
        let mut whole = self
            .whole_e8s
            .checked_add(o.whole_e8s)
            .ok_or(WideError::Overflow)?;
        if frac >= UNITS_PER_E8S {
            frac -= UNITS_PER_E8S;
            whole = whole.checked_add(1).ok_or(WideError::Overflow)?;
        }
        Ok(Self {
            whole_e8s: whole,
            frac_units: frac,
        })
    }

    pub fn checked_sub(&self, o: &Self) -> Result<Self, WideError> {
        if self < o {
            return Err(WideError::Underflow);
        }
        let (mut whole, frac) = if self.frac_units >= o.frac_units {
            (self.whole_e8s - o.whole_e8s, self.frac_units - o.frac_units)
        } else {
            (
                self.whole_e8s - o.whole_e8s - 1,
                self.frac_units + UNITS_PER_E8S - o.frac_units,
            )
        };
        if frac >= UNITS_PER_E8S {
            whole += 1;
        }
        Ok(Self {
            whole_e8s: whole,
            frac_units: frac % UNITS_PER_E8S,
        })
    }

    /// Exact XRPL amount, or an error when it needs more than 16 significant digits
    /// (never rounded).
    pub fn to_exact_issued(&self) -> Result<ExactIssuedAmount, AmountError> {
        ExactIssuedAmount::parse(&self.to_decimal_string())
    }

    /// Whole e8s, rounded UP (the conservative backing requirement).
    pub fn ceil_e8s(&self) -> u128 {
        self.whole_e8s + u128::from(self.frac_units > 0)
    }

    pub fn to_decimal_string(&self) -> String {
        let whole = self.whole_e8s / 100_000_000;
        let e8 = self.whole_e8s % 100_000_000;
        let mut s = format!("{whole}.{e8:08}");
        if self.frac_units > 0 {
            s.push_str(&format!("{:032}", self.frac_units));
        }
        let t = s.trim_end_matches('0');
        let t = t.trim_end_matches('.');
        if t.is_empty() {
            "0".to_string()
        } else {
            t.to_string()
        }
    }
}

// ----------------------------------------------------------------------------------
// Configuration
// ----------------------------------------------------------------------------------

/// Explicit issuer policy. There is NO default: setup cannot start until both
/// values were chosen, and they cannot change once setup has started.
#[derive(CandidType, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct IssuerPolicy {
    pub allow_clawback: bool,
    pub no_freeze: bool,
}

impl IssuerPolicy {
    /// XRPL forbids enabling both (`asfAllowTrustLineClawback` requires NoFreeze off
    /// and vice versa).
    pub fn validate(&self) -> Result<(), XrplIcusdError> {
        if self.allow_clawback && self.no_freeze {
            return Err(XrplIcusdError::Policy(
                "allow_clawback and no_freeze are mutually exclusive on the XRPL".into(),
            ));
        }
        Ok(())
    }
}

#[derive(CandidType, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssuerSetupStep {
    DefaultRipple,
    RequireDest,
    TransferRate,
    AllowClawback,
    NoFreeze,
}

impl IssuerSetupStep {
    /// Ordered steps for a policy. Irreversible flags come last.
    pub fn sequence_for(policy: &IssuerPolicy) -> Vec<IssuerSetupStep> {
        let mut v = vec![
            IssuerSetupStep::DefaultRipple,
            IssuerSetupStep::RequireDest,
            IssuerSetupStep::TransferRate,
        ];
        if policy.allow_clawback {
            v.push(IssuerSetupStep::AllowClawback);
        }
        if policy.no_freeze {
            v.push(IssuerSetupStep::NoFreeze);
        }
        v
    }

    pub fn next_after(&self, policy: &IssuerPolicy) -> Option<IssuerSetupStep> {
        let seq = Self::sequence_for(policy);
        let i = seq.iter().position(|s| s == self)?;
        seq.get(i + 1).copied()
    }
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub enum IssuerSetupState {
    #[default]
    NotStarted,
    /// `step` is the next (or in-flight) step; an unresolved attempt for it lives in
    /// `XrplIcusdState::setup_attempt`.
    Configuring { step: IssuerSetupStep },
    /// Every policy step is done; the account flags still have to be verified.
    Verifying,
    /// All policy steps are validated AND the final account flags were verified.
    Ready { ledger_index: u32 },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct XrplIcusdConfig {
    pub enabled: bool,
    pub issuance_paused: bool,
    pub settlement_paused: bool,
    pub network: Option<XrplNetwork>,
    pub issuer_address: Option<String>,
    pub issuer_public_key: Option<Vec<u8>>,
    pub policy: Option<IssuerPolicy>,
    pub issuer_setup: IssuerSetupState,
    pub outstanding_cap_e8s: u64,
    pub per_operation_cap_e8s: u64,
    pub rounding_buffer_e8s: u64,
    pub max_fee_drops: u64,
    /// Set only after `icrc1_minting_account` proved it is the backend default
    /// account and differs from the escrow account.
    pub verified_minting_account: Option<Account>,
    pub config_version: u64,
    /// Aggregated unattributed value below this (e8s) is retained exactly and never
    /// auto-refunded: a refund costs the issuer XRP, so it must exceed that cost.
    pub min_auto_refund_e8s: u64,
}

impl Default for XrplIcusdConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            issuance_paused: true,
            settlement_paused: false,
            network: None,
            issuer_address: None,
            issuer_public_key: None,
            policy: None,
            issuer_setup: IssuerSetupState::NotStarted,
            outstanding_cap_e8s: 0,
            per_operation_cap_e8s: 0,
            rounding_buffer_e8s: 0,
            max_fee_drops: 0,
            verified_minting_account: None,
            config_version: 0,
            min_auto_refund_e8s: MIN_AUTO_REFUND_E8S,
        }
    }
}

impl XrplIcusdConfig {
    pub fn setup_started(&self) -> bool {
        !matches!(self.issuer_setup, IssuerSetupState::NotStarted)
    }
    pub fn issuer_ready(&self) -> bool {
        matches!(self.issuer_setup, IssuerSetupState::Ready { .. })
    }
}

// ----------------------------------------------------------------------------------
// IC ledger jobs
// ----------------------------------------------------------------------------------

#[derive(CandidType, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum IcJobPurpose {
    /// minting account -> escrow (Borrow: net proceeds).
    EscrowMint,
    /// escrow -> minting account (Repayment: retire backing).
    EscrowBurn,
    /// minting account -> treasury/remainder (Borrow fee remainder).
    FeeMint,
    /// ICRC-2 pull from the user's canonical account into escrow (Import).
    ImportPull,
    /// escrow -> owner's canonical account (claim resolution).
    CanonicalReturn,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum IcJobStatus {
    /// Persisted, not yet sent (or definitively not executed and safe to resend).
    Pending,
    /// A call is outstanding (normalised to `Unknown` by an upgrade).
    InFlight,
    /// Executed at `block` (proven by the ledger reply; `proof_verified` once the
    /// block was fetched and matched).
    Done { block: u64 },
    /// Proven never executed (definite ledger rejection, or an absence proof).
    Failed { reason: String },
    /// Possibly executed. Resent with the IDENTICAL fields until it resolves.
    Unknown { detail: String },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ScanProgress {
    /// Next block to inspect.
    pub next_block: u64,
    /// Pinned exclusive end (log length read AFTER the `TooOld` reply).
    pub end_block: Option<u64>,
}

/// One idempotent IC ledger operation. Persisted BEFORE the first await; the executor
/// takes only (operation id, job index) and loads these immutable fields from state.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct IcLedgerJob {
    pub purpose: IcJobPurpose,
    pub op_nonce: u128,
    pub created_at_time: u64,
    pub from_subaccount: Option<[u8; 32]>,
    /// ICRC-2 pulls only: the owner's account being debited.
    pub from_owner: Option<Account>,
    pub to: Account,
    pub amount_e8s: u64,
    pub memo: Vec<u8>,
    /// IC ledger fee that a `CanonicalReturn` charges ON TOP of `amount_e8s`, reserved
    /// from the rounding/fee reserve, never from holder backing.
    #[serde(default)]
    pub reserved_fee_e8s: u64,
    pub ledger_tip_before: Option<u64>,
    pub status: IcJobStatus,
    #[serde(default)]
    pub proof_verified: bool,
    /// Set after a `TooOld` reply: only the archive-following scan may resolve it.
    #[serde(default)]
    pub scan_required: bool,
    #[serde(default)]
    pub scan: Option<ScanProgress>,
    #[serde(default)]
    pub sends: u32,
    /// When the job last became `InFlight` (a stale in-flight job is normalised to
    /// `Unknown`, then reconciled the normal way).
    #[serde(default)]
    pub in_flight_since_ns: Option<u64>,
}

impl IcLedgerJob {
    pub fn is_resolved(&self) -> bool {
        matches!(
            self.status,
            IcJobStatus::Done { .. } | IcJobStatus::Failed { .. }
        )
    }
    pub fn done_block(&self) -> Option<u64> {
        match self.status {
            IcJobStatus::Done { block } => Some(block),
            _ => None,
        }
    }
    /// Done AND its block was fetched back and matched the persisted tuple.
    pub fn is_proven_done(&self) -> bool {
        self.done_block().is_some() && self.proof_verified
    }
}

// ----------------------------------------------------------------------------------
// Issuer (XRPL) attempts
// ----------------------------------------------------------------------------------

#[derive(CandidType, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttemptPurpose {
    Setup { step: IssuerSetupStep },
    /// Pay the claim's recipient (Borrow / Import).
    Delivery,
    /// Return held value (Task 5).
    Refund,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum AttemptTx {
    AccountSet {
        set_flag: Option<u32>,
        transfer_rate: Option<u32>,
    },
    Payment {
        destination: String,
        amount: ExactIssuedAmount,
        destination_tag: Option<u32>,
    },
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum AttemptStatus {
    /// Unsigned bytes + sequence persisted. Holds the lease.
    Prepared,
    /// Signature persisted, not yet submitted. Holds the lease.
    Signed,
    /// Broadcast attempted; the outcome is unknown until reconciled. Holds the lease.
    Submitted,
    Validated { ledger_index: u32 },
    /// Validated with a non-success result: the sequence was consumed, nothing
    /// delivered.
    FailedOnLedger { result: String },
    /// Proven never included: validated ledger past LastLedgerSequence with complete
    /// history coverage and the transaction not found.
    Expired { validated_ledger: u32 },
    /// Validated but not matching what was authorized. Holds the lease; needs a human.
    Quarantined { reason: String },
}

impl AttemptStatus {
    /// An unresolved attempt owns the single issuer sequence.
    pub fn holds_lease(&self) -> bool {
        matches!(
            self,
            AttemptStatus::Prepared
                | AttemptStatus::Signed
                | AttemptStatus::Submitted
                | AttemptStatus::Quarantined { .. }
        )
    }
    /// A signed blob may exist on the network.
    pub fn is_signed_ambiguous(&self) -> bool {
        matches!(
            self,
            AttemptStatus::Signed | AttemptStatus::Submitted | AttemptStatus::Quarantined { .. }
        )
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct IssuerTxAttempt {
    pub purpose: AttemptPurpose,
    pub tx: AttemptTx,
    /// Issuer classic address that signs (binding).
    pub account: String,
    pub sequence: u32,
    pub last_ledger_sequence: u32,
    /// Validated ledger at which the sequence was read; history coverage for the
    /// non-inclusion proof must reach back to here.
    pub prepared_ledger: u32,
    pub fee_drops: u64,
    pub unsigned_hex: String,
    pub signed_hex: Option<String>,
    pub tx_hash: Option<String>,
    pub status: AttemptStatus,
    pub created_at_ns: u64,
    #[serde(default)]
    pub submit_count: u32,
    #[serde(default)]
    pub last_submit_ns: u64,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseHolder {
    Setup,
    Operation(OperationId),
}

// ----------------------------------------------------------------------------------
// Operations
// ----------------------------------------------------------------------------------

#[derive(CandidType, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    // Borrow
    Prepared,
    EscrowMintPending,
    DebtCommitted,
    IssuePrepared,
    Signed,
    Submitted,
    Delivered,
    // Import only: the ICRC-2 pull into escrow is outstanding
    EscrowPullPending,
    // Repayment
    IntentPrepared,
    ReturnObserved,
    ReturnVerified,
    EscrowBurnPending,
    DebtApplied,
    Complete,
    // Shared
    Held,
    RefundPending,
    ClaimResolved,
    Cancelled,
}

impl Phase {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Phase::Delivered | Phase::Complete | Phase::ClaimResolved | Phase::Cancelled
        )
    }
    /// Borrow phases that hold a durable headroom reservation.
    pub fn is_reservation(&self) -> bool {
        matches!(self, Phase::Prepared | Phase::EscrowMintPending)
    }
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct FeeSplit {
    /// Part of the borrowing fee that repaid the protocol deficit (recorded once by
    /// `plan_fee_routing_at`; a retry never re-runs the allocation).
    pub to_deficit_e8s: u64,
    /// Remainder of the fee to mint to the treasury (the FeeMint job's amount).
    pub to_treasury_remainder_e8s: u64,
    /// Treasury principal captured at the commit; `None` while the remainder is
    /// held (no FeeMint job exists, mirroring the legacy `TreasuryMissing` hold).
    pub treasury: Option<Principal>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct VerifiedReceiptRef {
    pub tx_hash: String,
    pub ledger_index: u32,
    pub delivered: ExactIssuedAmount,
    pub applied_e8s: u64,
    /// The signed ceiling (`max_e8s`) of the intent this receipt was attached under:
    /// debt application never exceeds the sum of these, whatever was delivered.
    #[serde(default)]
    pub authorized_e8s: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct CutoffSnapshot {
    pub cutoff_ns: u64,
    pub snapshot_debt_e8s: u64,
    pub snapshot_accrued_e8s: u64,
    pub repay_e8s: u64,
    pub interest_share_e8s: u64,
    pub last_accrual_time: u64,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingInterest {
    pub collateral_type: Principal,
    pub amount_e8s: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum OperationKind {
    Borrow {
        vault_id: u64,
        collateral_type: Principal,
        gross_e8s: u64,
        fee_e8s: u64,
        net_e8s: u64,
        recipient: String,
        policy_version: u64,
        fee_split: Option<FeeSplit>,
    },
    Import {
        amount_e8s: u64,
        recipient: String,
    },
    Repayment {
        vault_id: u64,
        payer: String,
        destination_tag: u32,
        max_e8s: u64,
        full_close: bool,
        parent: Option<OperationId>,
        receipts: Vec<VerifiedReceiptRef>,
        cutoff: Option<CutoffSnapshot>,
        applied_e8s: u64,
        /// Exact value received and NOT (or not yet) part of the admitted burn: the
        /// payer's backed, owned claim. Wide exact accumulator (never `Inexact`).
        held: WideAmount,
        children: Vec<OperationId>,
        /// Interest share of a settled burn still to be distributed (live only;
        /// replay never sets it, so a rebuilt state cannot re-mint).
        #[serde(default)]
        pending_interest: Option<PendingInterest>,
    },
    /// Unattributed or held value returned on the XRPL.
    Refund {
        source_tx: String,
        recipient: String,
        amount: ExactIssuedAmount,
    },
}

impl OperationKind {
    pub fn vault_id(&self) -> Option<u64> {
        match self {
            OperationKind::Borrow { vault_id, .. } | OperationKind::Repayment { vault_id, .. } => {
                Some(*vault_id)
            }
            _ => None,
        }
    }
    pub fn tag(&self) -> OperationKindTag {
        match self {
            OperationKind::Borrow { .. } => OperationKindTag::Borrow,
            OperationKind::Import { .. } => OperationKindTag::Import,
            OperationKind::Repayment { .. } => OperationKindTag::Repayment,
            OperationKind::Refund { .. } => OperationKindTag::Refund,
        }
    }
    /// e8s backing a deliverable claim, when this kind carries one.
    pub fn claim_e8s(&self) -> Option<u64> {
        match self {
            OperationKind::Borrow { net_e8s, .. } => Some(*net_e8s),
            OperationKind::Import { amount_e8s, .. } => Some(*amount_e8s),
            _ => None,
        }
    }
    pub fn recipient(&self) -> Option<&str> {
        match self {
            OperationKind::Borrow { recipient, .. } | OperationKind::Import { recipient, .. } => {
                Some(recipient)
            }
            OperationKind::Refund { recipient, .. } => Some(recipient),
            OperationKind::Repayment { payer, .. } => Some(payer),
        }
    }
    /// The exact token amount the issuer pays out for this operation, when it is a
    /// payout operation (delivery claims and refunds).
    pub fn payout_amount(&self) -> Option<Result<ExactIssuedAmount, AmountError>> {
        match self {
            OperationKind::Borrow { net_e8s, .. } => Some(ExactIssuedAmount::try_from_e8s(*net_e8s)),
            OperationKind::Import { amount_e8s, .. } => {
                Some(ExactIssuedAmount::try_from_e8s(*amount_e8s))
            }
            OperationKind::Refund { amount, .. } => Some(Ok(*amount)),
            OperationKind::Repayment { .. } => None,
        }
    }
    /// The phase in which a payout operation waits to start (or restart) an issuer
    /// attempt.
    pub fn ready_phase(&self) -> Phase {
        match self {
            OperationKind::Refund { .. } => Phase::RefundPending,
            _ => Phase::IssuePrepared,
        }
    }
}

#[derive(CandidType, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationKindTag {
    Borrow,
    Import,
    Repayment,
    Refund,
}

#[derive(CandidType, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimAction {
    ReceiveCanonical,
    ReissueXrpl,
    /// Task 5.
    CancelUnexecutedRepayment,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Operation {
    pub id: OperationId,
    pub owner: Principal,
    pub created_at_ns: u64,
    pub updated_at_ns: u64,
    pub request_nonce: [u8; 32],
    pub network: XrplNetwork,
    pub kind: OperationKind,
    pub phase: Phase,
    pub ic_jobs: Vec<IcLedgerJob>,
    pub xrpl_attempts: Vec<IssuerTxAttempt>,
    pub notes: Vec<String>,
    /// Owner-chosen claim resolution in progress (sticky until it completes or its
    /// job is proven failed).
    #[serde(default)]
    pub resolution: Option<ClaimAction>,
    /// Attempts already consumed when the current delivery budget was granted.
    #[serde(default)]
    pub attempt_budget_base: u32,
    /// Proven-failed attempts exhausted the budget: waits for the owner.
    #[serde(default)]
    pub delivery_halted: bool,
}

impl Operation {
    pub fn push_note(&mut self, note: impl Into<String>) {
        let mut n: String = note.into();
        n.truncate(MAX_NOTE_LEN);
        if self.notes.len() >= MAX_NOTES {
            self.notes.remove(0);
        }
        self.notes.push(n);
    }
    /// Any attempt that owns the issuer sequence.
    pub fn unresolved_attempt(&self) -> Option<usize> {
        self.xrpl_attempts
            .iter()
            .position(|a| a.status.holds_lease())
    }
    /// A signed blob may exist on the network for this claim.
    pub fn has_signed_ambiguity(&self) -> bool {
        self.xrpl_attempts
            .iter()
            .any(|a| a.status.is_signed_ambiguous())
    }
}

// ----------------------------------------------------------------------------------
// Coverage
// ----------------------------------------------------------------------------------

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum CoverageStatus {
    Covered,
    Shortfall { short_e8s: u128 },
    SnapshotMissing,
    SnapshotDisagrees { detail: String },
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct CoverageReport {
    pub status: CoverageStatus,
    pub escrow_balance_e8s: u128,
    pub required_e8s: u128,
    pub d_e8s: u128,
    pub o_e8s: u128,
    pub r_e8s: u128,
    pub h_e8s: u128,
    pub buffer_e8s: u128,
    pub ic_fee_reserve_e8s: u128,
    pub surplus_e8s: u128,
    pub snapshot_e8s: Option<u128>,
    pub snapshot_ledger_index: Option<u32>,
    pub in_flight_e8s: u128,
    /// Holder backing alone: `ceil(O + D + R + H)`.
    pub holder_backing_e8s: u128,
    /// The operator-funded surplus the escrow must hold ABOVE holder backing: the
    /// rounding buffer plus pending IC ledger fees. Canonical returns charge the IC
    /// ledger fee from this surplus, never from holder backing; if it is not funded a
    /// canonical return is refused with `CoverageUnavailable("ledger-fee surplus not
    /// funded ...")`. Launch action: fund the escrow with at least this amount (plus
    /// one ledger fee per expected return).
    pub fee_surplus_required_e8s: u128,
    /// Value moved out of `O` into the quarantined-receipt bucket (part of `H`).
    pub quarantined_e8s: u128,
}

impl CoverageReport {
    pub fn permits_issuance(&self) -> bool {
        self.status == CoverageStatus::Covered
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct CoverageRecord {
    pub checked_at_ns: u64,
    pub report: CoverageReport,
}

// ----------------------------------------------------------------------------------
// Candid views and arguments
// ----------------------------------------------------------------------------------

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct PublicXrplIcusdConfig {
    pub enabled: bool,
    pub issuance_paused: bool,
    pub settlement_paused: bool,
    pub ready: bool,
    pub network: Option<XrplNetwork>,
    pub issuer_address: Option<String>,
    pub currency_hex: String,
    pub policy: Option<IssuerPolicy>,
    pub issuer_setup: IssuerSetupState,
    pub outstanding_cap_e8s: u64,
    pub per_operation_cap_e8s: u64,
    pub max_fee_drops: u64,
    pub verified_minting_account: Option<Account>,
    pub escrow_account: Account,
    pub config_version: u64,
    pub min_auto_refund_e8s: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ConfigureXrplIcusdArg {
    pub network: Option<XrplNetwork>,
    pub policy: Option<IssuerPolicy>,
    pub enabled: Option<bool>,
    pub outstanding_cap_e8s: Option<u64>,
    pub per_operation_cap_e8s: Option<u64>,
    pub rounding_buffer_e8s: Option<u64>,
    pub max_fee_drops: Option<u64>,
    pub min_auto_refund_e8s: Option<u64>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct SetPausesArg {
    pub issuance_paused: Option<bool>,
    pub settlement_paused: Option<bool>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct IssueFromCanonicalArg {
    pub amount_e8s: u64,
    pub recipient: String,
    pub network: XrplNetwork,
    pub request_nonce: Vec<u8>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct BorrowToXrplArg {
    pub vault_id: u64,
    pub gross_e8s: u64,
    pub recipient: String,
    pub network: XrplNetwork,
    pub request_nonce: Vec<u8>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ResolveClaimArg {
    pub operation_id: OperationId,
    pub action: ClaimAction,
}

/// Request to prepare a repayment intent: the user will return `max_e8s` of XRPL icUSD
/// from `payer` to the issuer with the returned destination tag.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct PrepareRepaymentArg {
    pub vault_id: u64,
    pub max_e8s: u64,
    pub payer: String,
    pub network: XrplNetwork,
    pub request_nonce: Vec<u8>,
    pub full_close: bool,
    /// Bind this intent to a HELD parent repayment (same owner, vault, payer, network
    /// and asset): its verified receipt is combined with the parent exactly once.
    pub top_up_operation: Option<OperationId>,
}

/// What the wallet must sign. `payment_json` is the exact XRPL `Payment` (issued
/// currency, to the issuer, with `DestinationTag`, no `Paths`, no `tfPartialPayment`).
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct RepaymentIntentView {
    pub operation_id: OperationId,
    pub issuer: String,
    /// 160-bit currency hex.
    pub currency: String,
    pub payer: String,
    pub destination_tag: u32,
    pub max_e8s: u64,
    pub network: XrplNetwork,
    pub payment_json: String,
    pub quote_time_ns: u64,
}

/// A wallet-reported transaction hash: a LOOKUP KEY only. The backend re-reads and
/// verifies the transaction itself.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ObserveRepaymentArg {
    pub operation_id: OperationId,
    pub tx_hash: String,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum NextAction {
    None,
    /// Background work will advance this on its own.
    Wait,
    /// The owner may resolve the claim.
    ResolveClaim,
    /// Needs the operator (quarantine).
    ContactOperator,
    /// A replay-rebuilt borrow whose escrow mint outcome is unknown: the operator
    /// resolves it only with ledger proof (`reconcile_xrpl_borrow_mint`).
    ReconcileMint,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct OperationView {
    pub operation_id: OperationId,
    pub kind: OperationKindTag,
    pub vault_id: Option<u64>,
    pub phase: Phase,
    pub network: XrplNetwork,
    pub recipient: Option<String>,
    pub gross_e8s: u64,
    pub fee_e8s: u64,
    pub net_e8s: u64,
    pub observed_exact_amount: Option<String>,
    pub debt_applied_e8s: u64,
    pub held_exact_amount: Option<String>,
    pub settlement_cutoff_ns: Option<u64>,
    pub ic_ledger_blocks: Vec<u64>,
    pub xrpl_tx_hashes: Vec<String>,
    pub next_action: NextAction,
    pub resolution: Option<ClaimAction>,
    pub notes: Vec<String>,
    pub created_at_ns: u64,
    pub updated_at_ns: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct OperationPage {
    pub items: Vec<OperationView>,
    pub next_cursor: Option<u64>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct IssuerSetupStatus {
    pub setup: IssuerSetupState,
    pub issuer_address: Option<String>,
    pub attempt_status: Option<AttemptStatus>,
    pub note: Option<String>,
}

// ----------------------------------------------------------------------------------
// Returns, quarantine and the issuer-history observer (Task 5)
// ----------------------------------------------------------------------------------

/// Owner of operations the backend creates by itself (unattributed refunds). It can
/// never be a caller, so no principal can drive or resolve such an operation.
pub fn system_owner() -> Principal {
    Principal::management_canister()
}

fn anonymous_principal() -> Principal {
    Principal::anonymous()
}

/// What happened to a value-bearing return (one that reduced `O`). Keyed by
/// `(network, issuer, tx hash)`: a return is consumed exactly once.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ReturnDisposition {
    /// Attached to a repayment intent.
    Attributed { op_id: OperationId },
    /// A top-up child's receipt combined with its held parent.
    MergedIntoParent { child: OperationId, parent: OperationId },
    /// Not attributable (unknown/consumed tag, payer mismatch, anomalous payment):
    /// the value joined the verified sender's aggregate (and is refunded in chunks).
    Refund { op_id: OperationId, reason: String },
    /// Unattributed value folded into the per-(network, sender) aggregate: no
    /// per-transaction operation exists.
    Aggregated { reason: String },
    /// A value-bearing receipt that failed the strict evidence checks: the value left
    /// `O` into the quarantined bucket (held; never debt credit).
    Quarantined { reason: String },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ProcessedReturn {
    pub disposition: ReturnDisposition,
    pub ledger_index: u32,
    pub sender: String,
    pub destination_tag: Option<u32>,
    /// Exact decimal text of the delivered amount.
    pub delivered: String,
    pub recorded_at_ns: u64,
}

/// Evidence for a payment that had NO financial effect (wrong asset, bad receipt).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct QuarantineRecord {
    pub tx_hash: String,
    pub ledger_index: Option<u32>,
    pub sender: Option<String>,
    pub destination_tag: Option<u32>,
    pub reason: String,
    pub at_ns: u64,
}

/// Candid view of a held quarantined receipt.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct QuarantinedReceiptView {
    pub tx_hash: String,
    pub ledger_index: u32,
    pub sender: String,
    pub destination_tag: Option<u32>,
    /// Exact decimal amount.
    pub amount: String,
    pub reason: String,
    pub at_ns: u64,
}

impl From<&QuarantinedReceipt> for QuarantinedReceiptView {
    fn from(q: &QuarantinedReceipt) -> Self {
        Self {
            tx_hash: q.tx_hash.clone(),
            ledger_index: q.ledger_index,
            sender: q.sender.clone(),
            destination_tag: q.destination_tag,
            amount: q.amount.to_decimal_string(),
            reason: q.reason.clone(),
            at_ns: q.at_ns,
        }
    }
}

pub const MAX_QUARANTINE_LOG: usize = 200;
/// Hard cap on dedup records. Records below the scan frontier are compacted (see
/// `XrplIcusdState::compact_processed_returns`), so the cap only binds while the
/// observer lags.
pub const MAX_PROCESSED_RETURNS: usize = 25_000;
/// Records this many ledgers behind the fully-scanned frontier are dropped (about a
/// day of XRPL ledgers). Dedup stays correct: see `dedup_floor_ledger`.
pub const PROCESSED_RETAIN_LEDGERS: u32 = 20_000;
/// Aggregated unattributed value below this (e8s) is retained exactly but never
/// auto-refunded: a refund costs the issuer XRP (up to `max_fee_drops`), so the
/// default (0.01 icUSD) exceeds the fee value at any plausible XRP price. Configurable.
pub const MIN_AUTO_REFUND_E8S: u64 = 1_000_000;
/// Per-pass budget of refund operations the system may create from aggregates.
pub const REFUNDS_PER_PASS: usize = 8;
/// Cap on live system refund records. Separate from user admissions: reaching it only
/// defers further refunds (value stays in its aggregate), it never blocks a user.
pub const MAX_SYSTEM_REFUND_OPS: usize = 1_000;
/// Cap on per-(network, sender) aggregates; beyond it value joins ONE overflow
/// aggregate that is never auto-refunded (operator-visible).
pub const MAX_UNATTRIBUTED_SENDERS: usize = 2_000;
/// Cap on quarantined value-bearing receipts held individually; beyond it value joins
/// the overflow quarantine entry.
pub const MAX_QUARANTINED_RECEIPTS: usize = 500;
/// Sender key of the overflow aggregate / quarantine entry.
pub const OVERFLOW_SENDER: &str = "*overflow*";
/// Cap on per-transaction evidence kept for value that joined an overflow bucket (only
/// contributions of at least `min_auto_refund_e8s` are recorded). Past it, a larger
/// contribution replaces the smallest recorded one.
pub const MAX_OVERFLOW_EVIDENCE: usize = 500;
/// Terminal operations are pruned after this retention window.
pub const OP_RETENTION_NS: u64 = 30 * 24 * 3_600 * 1_000_000_000;
/// Terminal operations pruned per tick pass.
pub const PRUNE_PER_PASS: usize = 200;
/// A ledger job stuck `InFlight` longer than this is treated as `Unknown`.
pub const STALE_IN_FLIGHT_NS: u64 = 10 * 60 * 1_000_000_000;
/// Fresh-nonce retries of a proven-failed borrow fee-remainder mint before it is held
/// for the operator.
pub const MAX_FEE_MINT_AUTO_RETRIES: usize = 3;

/// Unattributed (or dust) value from one sender, folded into ONE record instead of
/// one operation per transaction. Exact: sums use [`WideAmount`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct UnattributedReturns {
    pub network: XrplNetwork,
    pub sender: String,
    /// Value received and not yet moved into a refund operation (part of `H`).
    pub total: WideAmount,
    pub tx_count: u64,
    pub first_ledger: u32,
    pub last_ledger: u32,
    pub last_tx: String,
    pub last_reason: String,
    pub updated_at_ns: u64,
    /// Refund operations already cut from this aggregate (feeds the deterministic
    /// refund nonce).
    pub refunds_issued: u32,
}

/// Which overflow bucket a contribution joined.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverflowKind {
    Unattributed,
    Quarantine,
}

/// Per-transaction evidence for a contribution that joined an overflow bucket (the
/// bucket itself is only a sum). It is what lets a controller re-verify THAT receipt
/// through the provider quorum and move exactly its amount to the verified sender's
/// attributed aggregate, once (the entry is consumed by the move).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct OverflowEvidence {
    pub network: XrplNetwork,
    pub kind: OverflowKind,
    pub tx_hash: String,
    pub sender: String,
    pub ledger_index: u32,
    pub destination_tag: Option<u32>,
    pub amount: WideAmount,
    pub at_ns: u64,
}

/// A value-bearing receipt that failed strict evidence checks. Its value left `O`
/// (the ledger reduced issuer obligations by it) and is held here until a controller
/// re-verifies and books it. NEVER debt credit; NEVER auto-refunded.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct QuarantinedReceipt {
    pub tx_hash: String,
    pub ledger_index: u32,
    pub sender: String,
    pub destination_tag: Option<u32>,
    pub amount: WideAmount,
    pub reason: String,
    pub at_ns: u64,
}
/// An unused repayment intent expires (the destination tag is never reassigned).
pub const INTENT_TTL_NS: u64 = 7 * 24 * 3_600 * 1_000_000_000;

/// Durable issuer-history position. `next_ledger_min` is the first ledger not yet
/// fully processed; `cursor` is an in-progress `[min, max]` interval with rippled's
/// continuation marker. Neither moves over unprocessed history.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(default)]
pub struct ObserverState {
    pub next_ledger_min: Option<u32>,
    pub cursor: Option<IssuerHistoryCursor>,
    /// Bumped whenever a pass made durable progress (drives the tick back-off).
    pub progress: u64,
    pub last_pass_ns: u64,
    pub last_note: Option<String>,
    /// `checked_at_ns` of the coverage record whose snapshot-below-`O` condition the
    /// observer has already caught up on (so it does not poll again for that record).
    pub settled_coverage_ns: u64,
}

// ----------------------------------------------------------------------------------
// Events (wrapped in `Event::XrplIcusd`)
// ----------------------------------------------------------------------------------

#[derive(CandidType, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseReason {
    /// A definite mint failure: no debt, the operation is cancelled.
    MintFailed,
    /// The mint is proven and the debt event was committed in the same step.
    DebtCommitted,
}

/// Sub-events sufficient to rebuild durable reservations, vault exclusions and
/// interest-accrual freezes during event replay (snapshot-only flags are not enough
/// for the fallback replay path, which re-runs `AccrueInterest`).
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum XrplIcusdEvent {
    ReservationBegun {
        op_id: u64,
        owner: Principal,
        request_nonce: Vec<u8>,
        network: XrplNetwork,
        vault_id: u64,
        collateral_type: Principal,
        gross_e8s: u64,
        fee_e8s: u64,
        net_e8s: u64,
        recipient: String,
        policy_version: u64,
        timestamp: u64,
    },
    ReservationReleased {
        op_id: u64,
        vault_id: u64,
        reason: ReleaseReason,
        timestamp: u64,
    },
    RepayCutoffBegun {
        op_id: u64,
        owner: Principal,
        request_nonce: Vec<u8>,
        network: XrplNetwork,
        vault_id: u64,
        payer: String,
        destination_tag: u32,
        max_e8s: u64,
        full_close: bool,
        cutoff_ns: u64,
        snapshot_debt_e8s: u64,
        snapshot_accrued_e8s: u64,
        repay_e8s: u64,
        interest_share_e8s: u64,
        last_accrual_time: u64,
    },
    /// Carries everything an analytics/points/explorer consumer needs to index an
    /// XRPL repayment WITHOUT a separate `RepayToVault` event (which replay would
    /// apply a second time): the vault, the owner, the debt actually applied and the
    /// interest share.
    RepaySettled {
        op_id: u64,
        vault_id: u64,
        burn_block: u64,
        settled_at: u64,
        #[serde(default = "anonymous_principal")]
        owner: Principal,
        #[serde(default)]
        applied_e8s: u64,
        #[serde(default)]
        interest_share_e8s: u64,
    },
    RepayCancelled {
        op_id: u64,
        vault_id: u64,
        at: u64,
    },
}

impl XrplIcusdEvent {
    pub fn vault_id(&self) -> u64 {
        match self {
            XrplIcusdEvent::ReservationBegun { vault_id, .. }
            | XrplIcusdEvent::ReservationReleased { vault_id, .. }
            | XrplIcusdEvent::RepayCutoffBegun { vault_id, .. }
            | XrplIcusdEvent::RepaySettled { vault_id, .. }
            | XrplIcusdEvent::RepayCancelled { vault_id, .. } => *vault_id,
        }
    }
    pub fn owner(&self) -> Option<Principal> {
        match self {
            XrplIcusdEvent::ReservationBegun { owner, .. }
            | XrplIcusdEvent::RepayCutoffBegun { owner, .. }
            | XrplIcusdEvent::RepaySettled { owner, .. } => Some(*owner),
            _ => None,
        }
    }
    pub fn timestamp(&self) -> u64 {
        match self {
            XrplIcusdEvent::ReservationBegun { timestamp, .. }
            | XrplIcusdEvent::ReservationReleased { timestamp, .. } => *timestamp,
            XrplIcusdEvent::RepayCutoffBegun { cutoff_ns, .. } => *cutoff_ns,
            XrplIcusdEvent::RepaySettled { settled_at, .. } => *settled_at,
            XrplIcusdEvent::RepayCancelled { at, .. } => *at,
        }
    }
    pub fn type_name(&self) -> &'static str {
        match self {
            XrplIcusdEvent::ReservationBegun { .. } => "XrplReservationBegun",
            XrplIcusdEvent::ReservationReleased { .. } => "XrplReservationReleased",
            XrplIcusdEvent::RepayCutoffBegun { .. } => "XrplRepayCutoffBegun",
            XrplIcusdEvent::RepaySettled { .. } => "XrplRepaySettled",
            XrplIcusdEvent::RepayCancelled { .. } => "XrplRepayCancelled",
        }
    }
}
