//! Escrow-backed Stability Pool to 3pool conversion saga.
//!
//! The old `deposit_as_3usd` path pulled into the pool's default account and
//! then performed an unjournaled approval and add-liquidity call.  This module
//! keeps the conversion's value in a caller/sequence-specific escrow account
//! until the 3pool receipt is terminal.  Every value-moving tuple is written to
//! the state journal before its await and is reused verbatim after an
//! ambiguous reply.

use candid::{CandidType, Nat, Principal};
use ic_cdk::call;
use icrc_ledger_types::icrc::generic_value::ICRC3Value;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
use icrc_ledger_types::icrc2::allowance::{Allowance, AllowanceArgs};
use icrc_ledger_types::icrc2::approve::{ApproveArgs, ApproveError};
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
use num_traits::ToPrimitive;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::BTreeSet;

use crate::state::{mutate_state, read_state};
use crate::types::{PoolEventType, StabilityPoolError, ThreePoolErrorRemote};

/// Candid-only mirrors of the 3pool receipt wire contract.  The stability pool
/// must not link the 3pool canister crate into its production Wasm: that crate
/// contains the other canister's state and exports.  Keep this mirror byte for
/// byte aligned with `rumi_3pool::liquidity_receipts` and update both sides as
/// one wire change.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct AddLiquidityRequestV1 {
    pub intent_id: Vec<u8>,
    pub amounts: Vec<u128>,
    pub min_lp: u128,
    pub source_subaccount: Option<[u8; 32]>,
    pub max_transfer_fee: Option<u128>,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct LiquidityInputTransferV1 {
    pub ledger: Principal,
    pub from: Account,
    pub to: Account,
    pub amount: u128,
    #[serde(default)]
    pub fee: Option<u128>,
    #[serde(default)]
    pub memo: Option<Vec<u8>>,
    pub created_at_time: u64,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct LiquidityRefundTransferV1 {
    pub ledger: Principal,
    pub from: Account,
    pub to: Account,
    pub gross: u128,
    pub net: u128,
    pub fee: u128,
    pub memo: Vec<u8>,
    pub created_at_time: u64,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum LiquidityReceiptStatusV1 {
    Prepared,
    Executing,
    RecoveryPending,
    Completed,
    Refunded,
    Rejected,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum LiquidityLegStatusV1 {
    Prepared,
    Submitted,
    Confirmed { block: Nat },
    RejectedNoTransfer { reason: String },
    Unresolved { reason: String },
    Refunded,
    Skipped,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct LiquidityReceiptV1 {
    pub version: u16,
    pub owner: Principal,
    pub request: AddLiquidityRequestV1,
    pub status: LiquidityReceiptStatusV1,
    pub input_ids: [Option<u64>; 3],
    pub refund_ids: [Option<u64>; 3],
    pub input_status: [Option<LiquidityLegStatusV1>; 3],
    pub refund_status: [Option<LiquidityLegStatusV1>; 3],
    pub input_transfers: [Option<LiquidityInputTransferV1>; 3],
    pub refund_transfers: [Option<LiquidityRefundTransferV1>; 3],
    pub old_balances: Option<[u128; 3]>,
    pub old_lp_supply: Option<u128>,
    pub old_caller_lp: Option<u128>,
    pub balances_after: Option<[u128; 3]>,
    pub lp_supply_after: Option<u128>,
    pub caller_lp_after: Option<u128>,
    pub lp_minted: Option<u128>,
    pub fee_bps: Option<u16>,
    pub imbalance_before: Option<u64>,
    pub imbalance_after: Option<u64>,
    pub is_rebalancing: Option<bool>,
    pub mint_event_id: Option<u64>,
    pub compensation_started: bool,
    pub error: Option<String>,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum LiquidityReceiptErrorV1 {
    InvalidIntentId,
    IntentConflict,
    CapacityExceeded,
    Unauthorized,
    InvalidRequest,
    NotFound,
    Concurrent,
    PoolLocked,
    RecoveryRequired,
}

/// Permanent intent capacity.  Rows are never evicted because a terminal row
/// is also the tombstone for its caller/sequence and escrow subaccount.
pub const MAX_CONVERSION_INTENTS: usize = 10_000;
const ESCROW_DOMAIN: &[u8] = b"RUMI-SP-CONVERSION-ESCROW-V1";
const INTENT_DOMAIN: &[u8] = b"RUMI-SP-3POOL-INTENT-V1";
const MEMO_DOMAIN: &[u8] = b"RUMI-SP-CONVERSION-MEMO-V1";
const APPROVAL_EXPIRY_NS: u64 = 300_000_000_000;
/// ICRC-2 ledgers reject a created_at_time older than their 24-hour
/// deduplication window.  A journal row can survive longer while paused or
/// waiting for an aged proof, so every still-undispatched tuple gets a fresh
/// timestamp before its first value-moving await.
const CREATED_AT_MAX_AGE_NS: u64 = 24 * 60 * 60 * 1_000_000_000;
/// Admission reserves enough caller budget for approval, the 3pool pull,
/// compensation, allowance revocation, and the SP escrow refund.  The 3pool
/// receipt is still the authority for whether a leg actually consumed value.
const MAX_FEE_LEGS: u64 = 5;

thread_local! {
    static LIVE_CONVERSIONS: RefCell<BTreeSet<(Principal, u64)>> =
        const { RefCell::new(BTreeSet::new()) };
    /// A fresh intent keeps this guard while all read-only admission calls
    /// are in flight.  It closes the gap before a durable row exists, where
    /// repeated updates could otherwise admit many unfunded sequences from
    /// the same balance/allowance snapshot.
    static LIVE_CONVERSION_OWNERS: RefCell<BTreeSet<Principal>> =
        const { RefCell::new(BTreeSet::new()) };
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversionRequest {
    pub intent_seq: u64,
    pub token_ledger: Principal,
    pub principal_raw: u64,
    pub max_fee_budget_raw: u64,
    pub min_lp: u128,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConversionPhase {
    Reserved,
    Pulling,
    Approving,
    PoolCall,
    Revoking,
    Refunding,
    CreditPending,
    Completed,
    Refunded,
    Rejected,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConversionStatus {
    Pending {
        intent_seq: u64,
        phase: ConversionPhase,
        reason: Option<String>,
    },
    Completed {
        intent_seq: u64,
        lp_minted: u128,
        lp_credited: bool,
        uncredited_lp: Option<u128>,
        retained_dust_raw: u64,
    },
    Refunded {
        intent_seq: u64,
        refunded_raw: u64,
        retained_dust_raw: u64,
    },
    Rejected {
        intent_seq: u64,
        reason: String,
    },
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversionQuote {
    pub token_ledger: Principal,
    pub lp_ledger: Principal,
    pub pool_canister: Principal,
    pub pool_coin_index: u8,
    pub principal_raw: u64,
    pub quoted_lp: u128,
    pub live_transfer_fee_raw: u64,
    pub minimum_fee_budget_raw: u64,
}

/// Public caller-scoped view used by clients to resume an existing request
/// without reconstructing any private escrow or pool identity.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversionView {
    pub request: ConversionRequest,
    pub status: ConversionStatus,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConversionError {
    Unauthorized,
    InvalidRequest { reason: String },
    IntentConflict,
    ActiveIntent { intent_seq: u64 },
    SequenceMismatch { expected_seq: u64 },
    SequenceExhausted,
    CapacityExceeded,
    TokenNotAccepted { ledger: Principal },
    TokenNotActive { ledger: Principal },
    LpTokenUnavailable,
    AmountTooLow { minimum_e8s: u64 },
    EmergencyPaused,
    SystemBusy,
    NonZeroFeeBudgetRequired { fee_raw: u64, minimum_budget_raw: u64 },
    NotFound,
    CannotCancel,
}

/// All fields needed to reconstruct every external call are retained here.
/// The struct lives in the conversion module so adding this journal does not
/// change the existing public `types.rs` wire records.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversionIntent {
    pub owner: Principal,
    pub request: ConversionRequest,
    pub phase: ConversionPhase,
    pub status: ConversionStatus,
    pub lp_ledger: Principal,
    pub pool_canister: Principal,
    pub pool_coin_index: Option<u8>,
    pub quoted_lp: Option<u128>,
    pub pool_amounts: Vec<u128>,
    pub pool_intent_id: [u8; 32],
    pub escrow_subaccount: [u8; 32],
    pub escrow_account: Account,

    pub pull_from: Account,
    pub pull_to: Account,
    pub pull_amount_raw: u64,
    /// The optional explicit fee on the caller-funded ingress pull.  Legacy
    /// rows keep this as None; a BadFee retry may pin the ledger's value.
    #[serde(default)]
    pub pull_call_fee_raw: Option<u64>,
    /// The fee charged by the 3pool's ICRC-2 input pull from escrow. The
    /// caller's initial ingress `transfer_from` fee is paid separately and is
    /// intentionally not deducted from this escrow journal.
    pub pull_fee_raw: Option<u64>,
    pub pull_memo: Vec<u8>,
    pub pull_created_at_ns: u64,
    pub pull_dispatched: bool,
    pub pull_block: Option<u64>,
    #[serde(default)]
    pub pull_ambiguous: bool,
    #[serde(default)]
    pub pull_generation: u64,

    pub escrow_funded_raw: Option<u64>,
    pub escrow_remaining_raw: Option<u64>,

    pub approve_from: Account,
    pub approve_spender: Account,
    pub approve_amount_raw: u64,
    pub approve_expected_allowance_raw: u64,
    pub approve_expires_at_ns: u64,
    pub approve_fee_raw: u64,
    pub approve_memo: Vec<u8>,
    pub approve_created_at_ns: u64,
    pub approve_dispatched: bool,
    pub approve_confirmed: bool,
    pub approve_block: Option<u64>,
    #[serde(default)]
    pub approve_ambiguous: bool,
    #[serde(default)]
    pub approve_generation: u64,

    pub pool_dispatched: bool,
    pub pool_input_confirmed: Option<bool>,
    pub pool_refund_confirmed: Option<bool>,
    pub pool_lp_minted: Option<u128>,
    pub pool_last_status: Option<String>,
    #[serde(default)]
    pub pool_input_transfers: [Option<LiquidityInputTransferV1>; 3],
    #[serde(default)]
    pub pool_refund_transfers: [Option<LiquidityRefundTransferV1>; 3],

    pub revoke_from: Account,
    pub revoke_spender: Account,
    pub revoke_amount_raw: u64,
    pub revoke_expected_allowance_raw: u64,
    pub revoke_fee_raw: u64,
    pub revoke_memo: Vec<u8>,
    pub revoke_created_at_ns: u64,
    pub revoke_dispatched: bool,
    pub revoke_confirmed: bool,
    pub revoke_block: Option<u64>,
    #[serde(default)]
    pub revoke_ambiguous: bool,
    #[serde(default)]
    pub revoke_generation: u64,

    pub refund_from: Account,
    pub refund_to: Account,
    pub refund_amount_raw: Option<u64>,
    pub refund_fee_raw: u64,
    pub refund_memo: Vec<u8>,
    pub refund_created_at_ns: u64,
    pub refund_dispatched: bool,
    pub refund_block: Option<u64>,
    pub refunded_raw: u64,
    pub retained_dust_raw: u64,
    #[serde(default)]
    pub refund_ambiguous: bool,
    #[serde(default)]
    pub refund_generation: u64,

    pub lp_credited: bool,
    pub uncredited_lp: Option<u128>,
    pub last_error: Option<String>,
}

struct LiveGuard {
    key: (Principal, u64),
}

impl Drop for LiveGuard {
    fn drop(&mut self) {
        LIVE_CONVERSIONS.with(|active| {
            active.borrow_mut().remove(&self.key);
        });
    }
}

fn acquire_live(owner: Principal, intent_seq: u64) -> Result<LiveGuard, ConversionError> {
    LIVE_CONVERSIONS.with(|active| {
        let mut active = active.borrow_mut();
        if !active.insert((owner, intent_seq)) {
            return Err(ConversionError::SystemBusy);
        }
        Ok(LiveGuard {
            key: (owner, intent_seq),
        })
    })
}

struct OwnerAdmissionGuard {
    owner: Principal,
}

impl Drop for OwnerAdmissionGuard {
    fn drop(&mut self) {
        LIVE_CONVERSION_OWNERS.with(|active| {
            active.borrow_mut().remove(&self.owner);
        });
    }
}

fn acquire_owner_admission(owner: Principal) -> Result<OwnerAdmissionGuard, ConversionError> {
    LIVE_CONVERSION_OWNERS.with(|active| {
        let mut active = active.borrow_mut();
        if !active.insert(owner) {
            return Err(ConversionError::SystemBusy);
        }
        Ok(OwnerAdmissionGuard { owner })
    })
}

fn digest(domain: &[u8], owner: Principal, ledger: Principal, intent_seq: u64) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    let pool = ic_cdk::api::id();
    hasher.update((pool.as_slice().len() as u8).to_be_bytes());
    hasher.update(pool.as_slice());
    hasher.update((owner.as_slice().len() as u8).to_be_bytes());
    hasher.update(owner.as_slice());
    hasher.update((ledger.as_slice().len() as u8).to_be_bytes());
    hasher.update(ledger.as_slice());
    hasher.update(intent_seq.to_be_bytes());
    hasher.finalize().into()
}

fn intent_id(owner: Principal, ledger: Principal, intent_seq: u64) -> [u8; 32] {
    digest(INTENT_DOMAIN, owner, ledger, intent_seq)
}

fn escrow_subaccount(owner: Principal, ledger: Principal, intent_seq: u64) -> [u8; 32] {
    digest(ESCROW_DOMAIN, owner, ledger, intent_seq)
}

fn memo(tag: &[u8], owner: Principal, ledger: Principal, intent_seq: u64) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(tag);
    hasher.update(MEMO_DOMAIN);
    let pool = ic_cdk::api::id();
    hasher.update((pool.as_slice().len() as u8).to_be_bytes());
    hasher.update(pool.as_slice());
    hasher.update((owner.as_slice().len() as u8).to_be_bytes());
    hasher.update(owner.as_slice());
    hasher.update((ledger.as_slice().len() as u8).to_be_bytes());
    hasher.update(ledger.as_slice());
    hasher.update(intent_seq.to_be_bytes());
    hasher.finalize().to_vec()
}

fn retry_memo(
    tag: &[u8],
    owner: Principal,
    ledger: Principal,
    intent_seq: u64,
    generation: u64,
) -> Vec<u8> {
    let mut value = memo(tag, owner, ledger, intent_seq);
    value.extend_from_slice(&generation.to_be_bytes());
    Sha256::digest(value).to_vec()
}

fn fee_cap(request: &ConversionRequest) -> u64 {
    request.max_fee_budget_raw / MAX_FEE_LEGS
}

fn next_attempt_time(previous: u64) -> u64 {
    ic_cdk::api::time().max(previous.saturating_add(1))
}

fn created_at_needs_refresh(created_at_ns: u64, now_ns: u64) -> bool {
    now_ns >= created_at_ns.saturating_add(CREATED_AT_MAX_AGE_NS)
}

/// Refresh only a tuple that has never been dispatched and has no ambiguous
/// reply.  Once a call was dispatched, its timestamp, memo, and generation
/// are immutable evidence and must never be rebound by a resume or upgrade.
fn refresh_first_dispatch_tuple_if_stale(
    created_at_ns: &mut u64,
    generation: &mut u64,
    dispatched: bool,
    ambiguous: bool,
    now_ns: u64,
) -> bool {
    if dispatched || ambiguous || !created_at_needs_refresh(*created_at_ns, now_ns) {
        return false;
    }
    *generation = (*generation).saturating_add(1);
    *created_at_ns = now_ns.max((*created_at_ns).saturating_add(1));
    true
}

fn block_index(value: Nat) -> Result<u64, ConversionError> {
    value
        .0
        .try_into()
        .map_err(|_| ConversionError::SystemBusy)
}

fn error_text(reason: impl std::fmt::Display) -> String {
    reason.to_string().chars().take(512).collect()
}

fn is_terminal(status: &ConversionStatus) -> bool {
    matches!(
        status,
        ConversionStatus::Completed { .. }
            | ConversionStatus::Refunded { .. }
            | ConversionStatus::Rejected { .. }
    )
}

fn pending(intent: &ConversionIntent, reason: Option<String>) -> ConversionStatus {
    ConversionStatus::Pending {
        intent_seq: intent.request.intent_seq,
        phase: intent.phase.clone(),
        reason,
    }
}

fn get_intent(owner: Principal, intent_seq: u64) -> Option<ConversionIntent> {
    read_state(|state| {
        state
            .conversion_intents
            .as_ref()
            .and_then(|rows| rows.get(&(owner, intent_seq)).cloned())
    })
}

/// Return the owner's durable nonterminal sequence, ignoring terminal
/// tombstones.  This is deliberately pure so the same rule can be exercised
/// independently of stable-state mutation and the reserve path can enforce it
/// atomically with the high-watermark update.
fn active_owner_sequence<'a, I>(owner: Principal, intents: I) -> Option<u64>
where
    I: Iterator<Item = &'a ConversionIntent>,
{
    intents
        .filter(|intent| intent.owner == owner && !is_terminal(&intent.status))
        .map(|intent| intent.request.intent_seq)
        .min()
}

fn owner_admission_is_free(active_sequence: Option<u64>) -> bool {
    active_sequence.is_none()
}

fn save_intent(intent: ConversionIntent) -> bool {
    mutate_state(|state| {
        let rows = state.conversion_intents.get_or_insert_with(Default::default);
        let key = (intent.owner, intent.request.intent_seq);
        let Some(existing) = rows.get(&key) else {
            return false;
        };
        // Request and all immutable routing identities are never replaceable.
        if existing.request != intent.request
            || existing.owner != intent.owner
            || existing.escrow_subaccount != intent.escrow_subaccount
            || existing.pool_intent_id != intent.pool_intent_id
        {
            return false;
        }
        rows.insert(key, intent);
        true
    })
}

fn reserve_intent(
    owner: Principal,
    request: ConversionRequest,
    lp_ledger: Principal,
    pool_canister: Principal,
) -> Result<ConversionIntent, ConversionError> {
    mutate_state(|state| {
        let key = (owner, request.intent_seq);
        if let Some(existing) = state
            .conversion_intents
            .as_ref()
            .and_then(|rows| rows.get(&key))
        {
            if existing.request != request {
                return Err(ConversionError::IntentConflict);
            }
            return Ok(existing.clone());
        }
        if let Some(active_seq) = state
            .conversion_intents
            .as_ref()
            .and_then(|rows| active_owner_sequence(owner, rows.values()))
        {
            // The exact existing key was checked above.  Any other
            // nonterminal sequence is a durable owner-level admission fence;
            // a caller must resume it rather than reserve another row.
            return Err(ConversionError::ActiveIntent {
                intent_seq: active_seq,
            });
        }
        let expected = state
            .conversion_high_watermarks
            .as_ref()
            .and_then(|watermarks| watermarks.get(&owner).copied())
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(ConversionError::SequenceExhausted)?;
        if request.intent_seq != expected {
            return Err(ConversionError::SequenceMismatch {
                expected_seq: expected,
            });
        }
        let current_capacity = state
            .conversion_intents
            .as_ref()
            .map_or(0, |rows| rows.len());
        if current_capacity >= MAX_CONVERSION_INTENTS {
            return Err(ConversionError::CapacityExceeded);
        }
        state
            .conversion_high_watermarks
            .get_or_insert_with(Default::default)
            .insert(owner, request.intent_seq);

        let escrow = escrow_subaccount(owner, request.token_ledger, request.intent_seq);
        let now = ic_cdk::api::time();
        let pull_amount = request
            .principal_raw
            .checked_add(request.max_fee_budget_raw)
            .ok_or_else(|| ConversionError::InvalidRequest {
                reason: "principal plus fee budget overflows u64".into(),
            })?;
        let approve_amount = request
            .principal_raw
            .checked_add(0)
            .ok_or(ConversionError::SequenceExhausted)?;
        let approve_from = Account {
            owner: ic_cdk::api::id(),
            subaccount: Some(escrow),
        };
        let approve_spender = Account {
            owner: pool_canister,
            subaccount: None,
        };
        let revoke_memo = memo(b"revoke", owner, request.token_ledger, request.intent_seq);
        let intent = ConversionIntent {
            owner,
            request: request.clone(),
            phase: ConversionPhase::Reserved,
            status: ConversionStatus::Pending {
                intent_seq: request.intent_seq,
                phase: ConversionPhase::Reserved,
                reason: None,
            },
            lp_ledger,
            pool_canister,
            pool_coin_index: None,
            quoted_lp: None,
            pool_amounts: vec![request.principal_raw as u128, 0, 0],
            pool_intent_id: intent_id(owner, request.token_ledger, request.intent_seq),
            escrow_subaccount: escrow,
            escrow_account: approve_from,
            pull_from: Account {
                owner,
                subaccount: None,
            },
            pull_to: approve_from,
            pull_amount_raw: pull_amount,
            pull_call_fee_raw: None,
            pull_fee_raw: None,
            pull_memo: memo(b"pull", owner, request.token_ledger, request.intent_seq),
            pull_created_at_ns: now,
            pull_dispatched: false,
            pull_block: None,
            pull_ambiguous: false,
            pull_generation: 0,
            escrow_funded_raw: None,
            escrow_remaining_raw: None,
            approve_from,
            approve_spender,
            approve_amount_raw: approve_amount,
            approve_expected_allowance_raw: 0,
            approve_expires_at_ns: now.saturating_add(APPROVAL_EXPIRY_NS),
            approve_fee_raw: 0,
            approve_memo: memo(b"approve", owner, request.token_ledger, request.intent_seq),
            approve_created_at_ns: now.saturating_add(1),
            approve_dispatched: false,
            approve_confirmed: false,
            approve_block: None,
            approve_ambiguous: false,
            approve_generation: 0,
            pool_dispatched: false,
            pool_input_confirmed: None,
            pool_refund_confirmed: None,
            pool_lp_minted: None,
            pool_last_status: None,
            pool_input_transfers: std::array::from_fn(|_| None),
            pool_refund_transfers: std::array::from_fn(|_| None),
            revoke_from: approve_from,
            revoke_spender: approve_spender,
            revoke_amount_raw: 0,
            revoke_expected_allowance_raw: 0,
            revoke_fee_raw: 0,
            revoke_memo,
            revoke_created_at_ns: now.saturating_add(2),
            revoke_dispatched: false,
            revoke_confirmed: false,
            revoke_block: None,
            revoke_ambiguous: false,
            revoke_generation: 0,
            refund_from: approve_from,
            refund_to: Account {
                owner,
                subaccount: None,
            },
            refund_amount_raw: None,
            refund_fee_raw: 0,
            refund_memo: memo(b"refund", owner, request.token_ledger, request.intent_seq),
            refund_created_at_ns: now.saturating_add(3),
            refund_dispatched: false,
            refund_block: None,
            refunded_raw: 0,
            retained_dust_raw: 0,
            refund_ambiguous: false,
            refund_generation: 0,
            lp_credited: false,
            uncredited_lp: None,
            last_error: None,
        };
        state
            .conversion_intents
            .get_or_insert_with(Default::default)
            .insert(key, intent.clone());
        Ok(intent)
    })
}

fn set_pending(mut intent: ConversionIntent, reason: impl Into<String>) -> ConversionIntent {
    let reason = reason.into();
    intent.last_error = Some(reason.clone());
    intent.status = pending(&intent, Some(reason));
    save_intent(intent.clone());
    intent
}

fn set_rejected(mut intent: ConversionIntent, reason: impl Into<String>) -> ConversionIntent {
    let reason = error_text(reason.into());
    intent.phase = ConversionPhase::Rejected;
    intent.last_error = Some(reason.clone());
    intent.status = ConversionStatus::Rejected {
        intent_seq: intent.request.intent_seq,
        reason,
    };
    save_intent(intent.clone());
    intent
}

fn ensure_request(request: &ConversionRequest) -> Result<(), ConversionError> {
    if request.intent_seq == 0 {
        return Err(ConversionError::InvalidRequest {
            reason: "intent_seq must start at 1".into(),
        });
    }
    if request.principal_raw == 0 {
        return Err(ConversionError::InvalidRequest {
            reason: "principal_raw must be nonzero".into(),
        });
    }
    request
        .principal_raw
        .checked_add(request.max_fee_budget_raw)
        .ok_or_else(|| ConversionError::InvalidRequest {
            reason: "principal plus fee budget overflows u64".into(),
        })?;
    Ok(())
}

fn synchronous_route(
    token_ledger: Principal,
) -> Result<(Principal, Principal, u64), ConversionError> {
    read_state(|state| {
        let input = state
            .stablecoin_registry
            .get(&token_ledger)
            .ok_or(ConversionError::TokenNotAccepted {
                ledger: token_ledger,
            })?;
        if input.is_lp_token.unwrap_or(false) {
            return Err(ConversionError::TokenNotAccepted {
                ledger: token_ledger,
            });
        }
        if !input.is_active {
            return Err(ConversionError::TokenNotActive {
                ledger: token_ledger,
            });
        }
        let lp = state
            .stablecoin_registry
            .iter()
            .find(|(_, config)| {
                config.is_active
                    && config.is_lp_token.unwrap_or(false)
                    && config.underlying_pool.is_some()
            })
            .map(|(ledger, config)| (*ledger, config.underlying_pool.unwrap()))
            .ok_or(ConversionError::LpTokenUnavailable)?;
        Ok((lp.0, lp.1, input.decimals as u64))
    })
}

fn ensure_admission(
    token_ledger: Principal,
    principal_raw: u64,
    decimals: u64,
) -> Result<(), ConversionError> {
    let amount_e8s = crate::types::normalize_to_e8s(principal_raw, decimals as u8);
    let minimum = read_state(|state| state.configuration.min_deposit_e8s);
    if amount_e8s < minimum {
        return Err(ConversionError::AmountTooLow {
            minimum_e8s: minimum,
        });
    }
    if read_state(|state| state.configuration.emergency_pause) {
        return Err(ConversionError::EmergencyPaused);
    }
    crate::ensure_pool_token_balance_mutation_allowed(&[token_ledger]).map_err(|error| match error {
        StabilityPoolError::EmergencyPaused => ConversionError::EmergencyPaused,
        _ => ConversionError::SystemBusy,
    })
}

fn fee_budget(fee: u64) -> Result<u64, ConversionError> {
    fee.checked_mul(MAX_FEE_LEGS)
        .ok_or_else(|| ConversionError::InvalidRequest {
            reason: "fee budget calculation overflow".into(),
        })
}

fn refund_plan(remaining: u64, fee: u64) -> (Option<u64>, u64) {
    if remaining <= fee {
        (None, remaining)
    } else {
        (Some(remaining - fee), 0)
    }
}

fn legacy_error(error: ConversionError) -> StabilityPoolError {
    match error {
        ConversionError::Unauthorized => StabilityPoolError::Unauthorized,
        ConversionError::AmountTooLow { minimum_e8s } => {
            StabilityPoolError::AmountTooLow { minimum_e8s }
        }
        ConversionError::TokenNotAccepted { ledger } => {
            StabilityPoolError::TokenNotAccepted { ledger }
        }
        ConversionError::TokenNotActive { ledger } => StabilityPoolError::TokenNotActive { ledger },
        ConversionError::EmergencyPaused => StabilityPoolError::EmergencyPaused,
        ConversionError::SystemBusy => StabilityPoolError::SystemBusy,
        other => StabilityPoolError::LedgerTransferFailed {
            reason: format!(
                "legacy deposit_as_3usd cannot complete safely; use start_3usd_conversion with an explicit fee budget ({other:?})"
            ),
        },
    }
}

fn legacy_status(status: ConversionStatus) -> Result<u64, StabilityPoolError> {
    match status {
        ConversionStatus::Completed { lp_minted, .. } => {
            u64::try_from(lp_minted).map_err(|_| StabilityPoolError::LedgerTransferFailed {
                reason: "completed LP amount exceeds the legacy u64 return type".into(),
            })
        }
        ConversionStatus::Pending {
            intent_seq, reason, ..
        } => Err(StabilityPoolError::LedgerTransferFailed {
            reason: format!(
                "legacy conversion intent {intent_seq} remains pending{}; resume_3usd_conversion or use the explicit conversion API",
                reason.map(|value| format!(": {value}")).unwrap_or_default()
            ),
        }),
        ConversionStatus::Refunded {
            intent_seq,
            refunded_raw,
            retained_dust_raw,
        } => Err(StabilityPoolError::LedgerTransferFailed {
            reason: format!(
                "legacy conversion intent {intent_seq} was refunded ({refunded_raw} raw) with retained caller dust {retained_dust_raw}; use the explicit conversion API"
            ),
        }),
        ConversionStatus::Rejected { intent_seq, reason } => {
            Err(StabilityPoolError::LedgerTransferFailed {
                reason: format!(
                    "legacy conversion intent {intent_seq} was rejected: {reason}; use the explicit conversion API"
                ),
            })
        }
    }
}

fn matching_legacy_intent(
    owner: Principal,
    token_ledger: Principal,
    principal_raw: u64,
) -> Option<ConversionIntent> {
    read_state(|state| {
        state
            .conversion_intents
            .as_ref()?
            .values()
            .filter(|intent| {
                intent.owner == owner
                    && intent.request.token_ledger == token_ledger
                    && intent.request.principal_raw == principal_raw
            })
            .max_by_key(|intent| intent.request.intent_seq)
            .cloned()
    })
}

/// Compatibility wrapper for the old two-argument endpoint.  It can only
/// create a fresh intent when the ledger proves a zero transfer fee.  A
/// matching durable row always wins, including a nonterminal row after a lost
/// reply, so the wrapper never allocates a replacement sequence or escrow.
pub async fn legacy_deposit_as_3usd(
    token_ledger: Principal,
    amount: u64,
) -> Result<u64, StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if caller == Principal::anonymous() {
        return Err(StabilityPoolError::Unauthorized);
    }
    if amount == 0 {
        return Err(StabilityPoolError::LedgerTransferFailed {
            reason: "legacy conversion amount must be nonzero".into(),
        });
    }

    if let Some(existing) = matching_legacy_intent(caller, token_ledger, amount) {
        if is_terminal(&existing.status) {
            return legacy_status(existing.status);
        }
        let _live = acquire_live(caller, existing.request.intent_seq).map_err(legacy_error)?;
        return legacy_status(run(existing).await);
    }

    let live_fee = crate::deposits::current_ledger_transfer_fee(token_ledger).await?;
    if live_fee != 0 {
        return Err(StabilityPoolError::LedgerTransferFailed {
            reason: format!(
                "legacy deposit_as_3usd cannot admit a nonzero ledger fee ({live_fee}); call start_3usd_conversion with max_fee_budget_raw"
            ),
        });
    }

    // Quote before reserving the durable sequence.  The quote is read-only and
    // supplies the slippage floor for the zero-fee compatibility request.
    let quote = quote_conversion(token_ledger, amount, 0).await.map_err(legacy_error)?;
    let intent_seq = next_sequence().map_err(legacy_error)?;
    let request = ConversionRequest {
        intent_seq,
        token_ledger,
        principal_raw: amount,
        max_fee_budget_raw: 0,
        min_lp: quote.quoted_lp,
    };
    let status = start_conversion(request).await.map_err(legacy_error)?;
    legacy_status(status)
}

async fn pool_route_and_quote(
    token_ledger: Principal,
    pool_canister: Principal,
    principal_raw: u64,
    min_lp: u128,
) -> Result<(u8, u128), String> {
    let (pool_status,): (crate::types::ThreePoolStatus,) = call(pool_canister, "get_pool_status", ())
        .await
        .map_err(|(_, message)| format!("get_pool_status failed: {message}"))?;
    let coin_index = pool_status
        .tokens
        .iter()
        .position(|token| token.ledger_id == token_ledger)
        .ok_or_else(|| "input token is not registered in the 3pool".to_string())?;
    if coin_index >= 3 {
        return Err("3pool returned an unsupported coin index".into());
    }
    let mut amounts = vec![0u128; 3];
    amounts[coin_index] = principal_raw as u128;
    let (quoted,): (Result<u128, ThreePoolErrorRemote>,) =
        call(pool_canister, "calc_add_liquidity_query", (amounts, min_lp))
            .await
            .map_err(|(_, message)| format!("calc_add_liquidity_query failed: {message}"))?;
    let quoted = quoted.map_err(|error| format!("3pool quote rejected: {error:?}"))?;
    // The 3pool query treats the minimum as an input hint and may return a
    // quote below it.  Enforce the caller's floor here, before this shared
    // helper can be reached by either the public quote path or the funded
    // conversion path.
    ensure_quote_meets_min_lp(quoted, min_lp)?;
    Ok((coin_index as u8, quoted))
}

fn ensure_quote_meets_min_lp(quoted_lp: u128, min_lp: u128) -> Result<(), String> {
    if quoted_lp < min_lp {
        return Err(format!(
            "3pool quote {quoted_lp} is below requested minimum LP {min_lp}"
        ));
    }
    Ok(())
}

fn preflight_pull_amount(
    principal_raw: u64,
    max_fee_budget_raw: u64,
    pull_fee_raw: u64,
) -> Result<u64, ConversionError> {
    principal_raw
        .checked_add(max_fee_budget_raw)
        .and_then(|amount| amount.checked_add(pull_fee_raw))
        .ok_or_else(|| ConversionError::InvalidRequest {
            reason: "principal, fee budget, and ingress fee overflow".into(),
        })
}

fn wallet_preflight_sufficient(
    balance_raw: u128,
    allowance_raw: u128,
    required_raw: u128,
) -> bool {
    balance_raw >= required_raw && allowance_raw >= required_raw
}

/// Complete all read-only admission checks before the durable reservation.
/// The owner guard held by the caller spans every await in this function,
/// while the final route/admission check runs immediately before journaling.
async fn preflight_new_intent(
    owner: Principal,
    request: &ConversionRequest,
    route: (Principal, Principal, u64),
) -> Result<(), ConversionError> {
    let (lp_ledger, pool_canister, decimals) = route;
    ensure_admission(request.token_ledger, request.principal_raw, decimals)?;

    let live_fee = crate::deposits::current_ledger_transfer_fee(request.token_ledger)
        .await
        .map_err(|_| ConversionError::SystemBusy)?;
    let minimum_budget = fee_budget(live_fee)?;
    if request.max_fee_budget_raw < minimum_budget {
        return Err(ConversionError::NonZeroFeeBudgetRequired {
            fee_raw: live_fee,
            minimum_budget_raw: minimum_budget,
        });
    }
    let required_raw = preflight_pull_amount(
        request.principal_raw,
        request.max_fee_budget_raw,
        live_fee,
    )?;

    pool_route_and_quote(
        request.token_ledger,
        pool_canister,
        request.principal_raw,
        request.min_lp,
    )
    .await
    .map_err(|reason| ConversionError::InvalidRequest { reason })?;

    let owner_account = Account {
        owner,
        subaccount: None,
    };
    let balance_result: Result<(Nat,), _> =
        call(request.token_ledger, "icrc1_balance_of", (owner_account.clone(),)).await;
    let (balance,) = balance_result.map_err(|_| ConversionError::SystemBusy)?;
    let balance_raw = balance.0.to_u128().unwrap_or(u128::MAX);
    if balance_raw < required_raw as u128 {
        return Err(ConversionError::InvalidRequest {
            reason: format!(
                "wallet balance {balance_raw} is below required ingress amount {required_raw}"
            ),
        });
    }

    let allowance_result: Result<(Allowance,), _> = call(
        request.token_ledger,
        "icrc2_allowance",
        (AllowanceArgs {
            account: owner_account,
            spender: Account {
                owner: ic_cdk::api::id(),
                subaccount: None,
            },
        },),
    )
    .await;
    let (allowance,) = allowance_result.map_err(|_| ConversionError::SystemBusy)?;
    if allowance
        .expires_at
        .is_some_and(|expires_at| expires_at <= ic_cdk::api::time())
    {
        return Err(ConversionError::InvalidRequest {
            reason: "SP allowance is expired".into(),
        });
    }
    let allowance_raw = allowance.allowance.0.to_u128().unwrap_or(u128::MAX);
    if !wallet_preflight_sufficient(balance_raw, allowance_raw, required_raw as u128) {
        return Err(ConversionError::InvalidRequest {
            reason: format!(
                "SP allowance {allowance_raw} is below required ingress amount {required_raw}"
            ),
        });
    }

    // Treat a fee change during the read-only window as a stale admission
    // snapshot.  Re-querying here avoids reserving a durable row whose
    // already-checked wallet and allowance no longer cover the ingress tuple.
    let final_fee = crate::deposits::current_ledger_transfer_fee(request.token_ledger)
        .await
        .map_err(|_| ConversionError::SystemBusy)?;
    if final_fee != live_fee {
        return Err(ConversionError::SystemBusy);
    }

    // The route and its configured decimal scale are immutable admission
    // inputs. If another update changed them while the quote or ledger
    // queries were suspended, fail before reserving rather than binding a
    // request to a stale route.
    if synchronous_route(request.token_ledger)? != (lp_ledger, pool_canister, decimals) {
        return Err(ConversionError::SystemBusy);
    }
    ensure_admission(request.token_ledger, request.principal_raw, decimals)?;
    Ok(())
}

pub async fn quote_conversion(
    token_ledger: Principal,
    principal_raw: u64,
    min_lp: u128,
) -> Result<ConversionQuote, ConversionError> {
    if principal_raw == 0 {
        return Err(ConversionError::InvalidRequest {
            reason: "principal_raw must be nonzero".into(),
        });
    }
    let (lp_ledger, pool_canister, decimals) = synchronous_route(token_ledger)?;
    ensure_admission(token_ledger, principal_raw, decimals)?;
    let (pool_coin_index, quoted_lp) = pool_route_and_quote(
        token_ledger,
        pool_canister,
        principal_raw,
        min_lp,
    )
    .await
    .map_err(|reason| ConversionError::InvalidRequest { reason })?;
    let live_fee = crate::deposits::current_ledger_transfer_fee(token_ledger)
        .await
        .map_err(|_| ConversionError::SystemBusy)?;
    Ok(ConversionQuote {
        token_ledger,
        lp_ledger,
        pool_canister,
        pool_coin_index,
        principal_raw,
        quoted_lp,
        live_transfer_fee_raw: live_fee,
        minimum_fee_budget_raw: fee_budget(live_fee)?,
    })
}

fn status_of(owner: Principal, intent_seq: u64) -> Result<ConversionStatus, ConversionError> {
    get_intent(owner, intent_seq)
        .map(|intent| intent.status)
        .ok_or(ConversionError::NotFound)
}

pub fn get_status(intent_seq: u64) -> Result<ConversionStatus, ConversionError> {
    let caller = ic_cdk::api::caller();
    if caller == Principal::anonymous() {
        return Err(ConversionError::Unauthorized);
    }
    status_of(caller, intent_seq)
}

pub fn get_active_status() -> Option<ConversionStatus> {
    let caller = ic_cdk::api::caller();
    if caller == Principal::anonymous() {
        return None;
    }
    read_state(|state| {
        state
            .conversion_intents
            .as_ref()?
            .values()
            .filter(|intent| intent.owner == caller && !is_terminal(&intent.status))
            .max_by_key(|intent| intent.request.intent_seq)
            .map(|intent| intent.status.clone())
    })
}

pub fn get_active_view() -> Option<ConversionView> {
    let caller = ic_cdk::api::caller();
    if caller == Principal::anonymous() {
        return None;
    }
    read_state(|state| {
        state
            .conversion_intents
            .as_ref()?
            .values()
            .filter(|intent| intent.owner == caller && !is_terminal(&intent.status))
            .max_by_key(|intent| intent.request.intent_seq)
            .map(|intent| ConversionView {
                request: intent.request.clone(),
                status: intent.status.clone(),
            })
    })
}

pub fn next_sequence() -> Result<u64, ConversionError> {
    let caller = ic_cdk::api::caller();
    if caller == Principal::anonymous() {
        return Err(ConversionError::Unauthorized);
    }
    read_state(|state| {
        state
            .conversion_high_watermarks
            .as_ref()
            .and_then(|watermarks| watermarks.get(&caller).copied())
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(ConversionError::SequenceExhausted)
    })
}

fn update_phase(intent: &mut ConversionIntent, phase: ConversionPhase) {
    intent.phase = phase.clone();
    intent.status = ConversionStatus::Pending {
        intent_seq: intent.request.intent_seq,
        phase,
        reason: intent.last_error.clone(),
    };
}

fn approve_args(intent: &ConversionIntent) -> ApproveArgs {
    ApproveArgs {
        from_subaccount: intent.escrow_account.subaccount,
        spender: intent.approve_spender,
        amount: Nat::from(intent.approve_amount_raw),
        expected_allowance: Some(Nat::from(intent.approve_expected_allowance_raw)),
        expires_at: Some(intent.approve_expires_at_ns),
        fee: Some(Nat::from(intent.approve_fee_raw)),
        memo: Some(intent.approve_memo.clone().into()),
        created_at_time: Some(intent.approve_created_at_ns),
    }
}

fn revoke_args(intent: &ConversionIntent) -> ApproveArgs {
    ApproveArgs {
        from_subaccount: intent.revoke_from.subaccount,
        spender: intent.revoke_spender,
        amount: Nat::from(intent.revoke_amount_raw),
        expected_allowance: Some(Nat::from(intent.revoke_expected_allowance_raw)),
        // No expiry is the ICRC-2 wire representation of the revoke's expiry
        // zero sentinel.  `Some(0)` is rejected as already expired by
        // standard ledgers, so the exact tuple records absence rather than a
        // short-lived timestamp.
        expires_at: None,
        fee: Some(Nat::from(intent.revoke_fee_raw)),
        memo: Some(intent.revoke_memo.clone().into()),
        created_at_time: Some(intent.revoke_created_at_ns),
    }
}

fn pull_args(intent: &ConversionIntent) -> TransferFromArgs {
    TransferFromArgs {
        spender_subaccount: None,
        from: intent.pull_from,
        to: intent.pull_to,
        amount: Nat::from(intent.pull_amount_raw),
        // The wallet/user pays this ingress fee separately from the explicit
        // downstream fee budget held in escrow.  A fee is pinned only after a
        // ledger has returned BadFee for a no-effect attempt.
        fee: intent.pull_call_fee_raw.map(Nat::from),
        memo: Some(intent.pull_memo.clone().into()),
        created_at_time: Some(intent.pull_created_at_ns),
    }
}

fn save_phase(mut intent: ConversionIntent, phase: ConversionPhase) -> ConversionIntent {
    update_phase(&mut intent, phase);
    save_intent(intent.clone());
    intent
}

async fn prepare_pool(mut intent: ConversionIntent) -> ConversionIntent {
    if intent.pool_coin_index.is_none() {
        if read_state(|state| state.configuration.emergency_pause) {
            return set_pending(intent, "pool conversion is paused before quote");
        }
        let route = pool_route_and_quote(
            intent.request.token_ledger,
            intent.pool_canister,
            intent.request.principal_raw,
            intent.request.min_lp,
        )
        .await;
        let (pool_coin_index, quoted_lp) = match route {
            Ok(route) => route,
            Err(reason) => return set_pending(intent, reason),
        };
        intent.pool_coin_index = Some(pool_coin_index);
        intent.quoted_lp = Some(quoted_lp);
        intent.pool_amounts = vec![0, 0, 0];
        intent.pool_amounts[pool_coin_index as usize] = intent.request.principal_raw as u128;
        if !save_intent(intent.clone()) {
            return set_pending(intent, "conversion identity changed while quoting");
        }
        intent = get_intent(intent.owner, intent.request.intent_seq).unwrap();
    }
    if intent.quoted_lp.is_none() {
        return set_pending(intent, "conversion quote is missing");
    }

    let fee = match crate::deposits::current_ledger_transfer_fee(intent.request.token_ledger).await {
        Ok(fee) => fee,
        Err(_) => return set_pending(intent, "live token fee unavailable before pull"),
    };
    let minimum_budget = match fee_budget(fee) {
        Ok(value) => value,
        Err(error) => return set_rejected(intent, format!("{error:?}")),
    };
    let per_leg_cap = fee_cap(&intent.request);
    if intent.request.max_fee_budget_raw < minimum_budget || fee > per_leg_cap {
        let reason = format!(
            "max_fee_budget_raw {} cannot cap each fee at {} (required total {})",
            intent.request.max_fee_budget_raw, fee, minimum_budget
        );
        return set_rejected(intent, reason);
    }
    intent.approve_fee_raw = fee;
    intent.revoke_fee_raw = fee;
    intent.refund_fee_raw = fee;
    intent.pull_fee_raw = Some(fee);
    intent.approve_amount_raw = match intent.request.principal_raw.checked_add(fee) {
        Some(amount) => amount,
        None => return set_rejected(intent, "approval amount overflow"),
    };
    // Revoke only the allowance granted by this exact conversion.  The
    // expected allowance is the CAS fence that prevents clearing a later or
    // independently granted allowance on the same escrow account.
    intent.revoke_expected_allowance_raw = intent.approve_amount_raw;
    intent.last_error = None;
    update_phase(&mut intent, ConversionPhase::Pulling);
    if !save_intent(intent.clone()) {
        return set_pending(intent, "conversion identity changed before pull");
    }
    intent
}

async fn pull(mut intent: ConversionIntent) -> ConversionIntent {
    if intent.pull_block.is_some() {
        return save_phase(intent, ConversionPhase::Approving);
    }
    if read_state(|state| state.configuration.emergency_pause) {
        return set_pending(intent, "conversion paused before user pull");
    }
    if let Err(reason) = crate::ensure_pool_token_balance_mutation_allowed(&[intent.request.token_ledger]) {
        return set_pending(intent, format!("conversion waits for pool mutation guard: {reason:?}"));
    }
    let now = ic_cdk::api::time();
    if refresh_first_dispatch_tuple_if_stale(
        &mut intent.pull_created_at_ns,
        &mut intent.pull_generation,
        intent.pull_dispatched,
        intent.pull_ambiguous,
        now,
    ) {
        intent.pull_memo = retry_memo(
            b"pull-retry",
            intent.owner,
            intent.request.token_ledger,
            intent.request.intent_seq,
            intent.pull_generation,
        );
    }
    intent.pull_dispatched = true;
    update_phase(&mut intent, ConversionPhase::Pulling);
    if !save_intent(intent.clone()) {
        return set_pending(intent, "conversion identity changed before pull dispatch");
    }
    let result: Result<(Result<Nat, TransferFromError>,), _> =
        call(intent.request.token_ledger, "icrc2_transfer_from", (pull_args(&intent),)).await;
    match result {
        Ok((Ok(block),)) => {
            let Ok(block) = block_index(block) else {
                return set_pending(intent, "pull block index does not fit u64");
            };
            intent.pull_ambiguous = false;
            intent.pull_block = Some(block);
            intent.escrow_funded_raw = Some(intent.pull_amount_raw);
            intent.escrow_remaining_raw = Some(intent.pull_amount_raw);
            intent.last_error = None;
            save_phase(intent, ConversionPhase::Approving)
        }
        Ok((Err(TransferFromError::Duplicate { duplicate_of }),)) => {
            let Ok(block) = block_index(duplicate_of) else {
                return set_pending(intent, "duplicate pull block index does not fit u64");
            };
            intent.pull_ambiguous = false;
            intent.pull_block = Some(block);
            intent.escrow_funded_raw = Some(intent.pull_amount_raw);
            intent.escrow_remaining_raw = Some(intent.pull_amount_raw);
            intent.last_error = None;
            save_phase(intent, ConversionPhase::Approving)
        }
        Ok((Err(error),)) => {
            if intent.pull_ambiguous {
                return set_pending(
                    intent,
                    format!("pull retry returned after an ambiguous attempt; exact original tuple is retained: {error:?}"),
                );
            }
            match error {
                TransferFromError::GenericError { .. }
                | TransferFromError::TemporarilyUnavailable => {
                    intent.pull_ambiguous = true;
                    set_pending(intent, format!("pull remains unresolved: {error:?}"))
                }
                TransferFromError::BadFee { expected_fee } => {
                    let Ok(expected_fee) = block_index(expected_fee) else {
                        return set_pending(intent, "user pull BadFee does not fit u64");
                    };
                    intent.pull_call_fee_raw = Some(expected_fee);
                    intent.pull_generation = intent.pull_generation.saturating_add(1);
                    intent.pull_memo = retry_memo(
                        b"pull-retry",
                        intent.owner,
                        intent.request.token_ledger,
                        intent.request.intent_seq,
                        intent.pull_generation,
                    );
                    intent.pull_created_at_ns = next_attempt_time(intent.pull_created_at_ns);
                    intent.pull_dispatched = false;
                    set_pending(intent, "ledger rejected the user pull for BadFee; retrying with a new exact tuple")
                }
                _ => set_rejected(
                    intent,
                    format!("pull rejected without authenticated success: {error:?}"),
                ),
            }
        }
        Err((code, message)) => {
            intent.pull_ambiguous = true;
            set_pending(intent, format!("pull call unresolved: {code:?} {message}"))
        }
    }
}

fn approval_funded_no_effect(
    mut intent: ConversionIntent,
    reason: impl Into<String>,
) -> ConversionIntent {
    let reason = error_text(reason.into());
    // A definite ledger no-effect response can enter the refund path only
    // when no earlier approval call was ambiguous.  After an unknown reply,
    // the same tuple may already have committed and must remain held for a
    // positive approval proof before any refund or revoke decision.
    if intent.approve_ambiguous {
        return set_pending(intent, reason);
    }
    intent.last_error = Some(reason);
    save_phase(intent, ConversionPhase::Refunding)
}

fn approval_unknown(
    intent: ConversionIntent,
    reason: impl Into<String>,
) -> ConversionIntent {
    let mut intent = intent;
    intent.approve_ambiguous = true;
    set_pending(intent, reason)
}

fn approval_error_is_authenticated_no_effect(error: &ApproveError) -> bool {
    matches!(
        error,
        ApproveError::InsufficientFunds { .. }
            | ApproveError::AllowanceChanged { .. }
            | ApproveError::Expired { .. }
            | ApproveError::TooOld
            | ApproveError::CreatedInFuture { .. }
            | ApproveError::BadFee { .. }
    )
}

fn approval_no_effect_can_refund(ambiguous: bool, error: &ApproveError) -> bool {
    !ambiguous && approval_error_is_authenticated_no_effect(error)
}

fn approval_bad_fee_can_retry(ambiguous: bool, expected_fee: u64, cap: u64) -> bool {
    !ambiguous && expected_fee <= cap
}

async fn approve(mut intent: ConversionIntent) -> ConversionIntent {
    if intent.approve_confirmed {
        return save_phase(intent, ConversionPhase::PoolCall);
    }
    if intent.approve_fee_raw > fee_cap(&intent.request) {
        return approval_funded_no_effect(intent, "approval fee exceeds caller fee budget cap");
    }
    if read_state(|state| state.configuration.emergency_pause) {
        return set_pending(intent, "conversion paused before approval");
    }
    if let Err(reason) = crate::ensure_pool_token_balance_mutation_allowed(&[intent.request.token_ledger]) {
        return set_pending(intent, format!("approval waits for pool mutation guard: {reason:?}"));
    }
    let now = ic_cdk::api::time();
    if refresh_first_dispatch_tuple_if_stale(
        &mut intent.approve_created_at_ns,
        &mut intent.approve_generation,
        intent.approve_dispatched,
        intent.approve_ambiguous,
        now,
    ) {
        intent.approve_memo = retry_memo(
            b"approve-retry",
            intent.owner,
            intent.request.token_ledger,
            intent.request.intent_seq,
            intent.approve_generation,
        );
        intent.approve_expires_at_ns = intent
            .approve_created_at_ns
            .saturating_add(APPROVAL_EXPIRY_NS);
    }
    intent.approve_dispatched = true;
    update_phase(&mut intent, ConversionPhase::Approving);
    if !save_intent(intent.clone()) {
        return set_pending(intent, "conversion identity changed before approval dispatch");
    }
    let result: Result<(Result<Nat, ApproveError>,), _> =
        call(intent.request.token_ledger, "icrc2_approve", (approve_args(&intent),)).await;
    match result {
        Ok((Ok(block),)) => {
            let Ok(block) = block_index(block) else {
                return set_pending(intent, "approval block index does not fit u64");
            };
            intent.approve_confirmed = true;
            intent.approve_ambiguous = false;
            intent.approve_block = Some(block);
            intent.last_error = None;
            save_phase(intent, ConversionPhase::PoolCall)
        }
        Ok((Err(ApproveError::Duplicate { duplicate_of }),)) => {
            let Ok(block) = block_index(duplicate_of) else {
                return set_pending(intent, "duplicate approval block index does not fit u64");
            };
            intent.approve_confirmed = true;
            intent.approve_ambiguous = false;
            intent.approve_block = Some(block);
            intent.last_error = None;
            save_phase(intent, ConversionPhase::PoolCall)
        }
        Ok((Err(ApproveError::BadFee { expected_fee }),)) => {
            let Ok(expected_fee) = block_index(expected_fee) else {
                return set_pending(intent, "approval fee does not fit u64");
            };
            if !approval_bad_fee_can_retry(
                intent.approve_ambiguous,
                expected_fee,
                fee_cap(&intent.request),
            ) {
                if intent.approve_ambiguous {
                    return set_pending(
                        intent,
                        "approval returned BadFee after an ambiguous attempt; exact original tuple is retained",
                    );
                }
                return approval_funded_no_effect(
                    intent,
                    format!("approval fee {expected_fee} exceeds caller fee budget"),
                );
            }
            intent.approve_fee_raw = expected_fee;
            intent.approve_amount_raw = match intent
                .request
                .principal_raw
                .checked_add(expected_fee)
            {
                Some(amount) => amount,
                None => {
                    return approval_funded_no_effect(
                        intent,
                        "approval amount overflow after BadFee",
                    )
                }
            };
            intent.revoke_expected_allowance_raw = intent.approve_amount_raw;
            intent.approve_generation = intent.approve_generation.saturating_add(1);
            intent.approve_memo = retry_memo(
                b"approve-retry",
                intent.owner,
                intent.request.token_ledger,
                intent.request.intent_seq,
                intent.approve_generation,
            );
            intent.approve_created_at_ns = next_attempt_time(intent.approve_created_at_ns);
            intent.approve_expires_at_ns = intent
                .approve_created_at_ns
                .saturating_add(APPROVAL_EXPIRY_NS);
            intent.approve_dispatched = false;
            let reason = format!("ledger requires approval fee {expected_fee}");
            intent.last_error = Some(reason.clone());
            set_pending(intent, reason)
        }
        Ok((Err(error),)) => {
            if approval_no_effect_can_refund(intent.approve_ambiguous, &error) {
                approval_funded_no_effect(
                    intent,
                    format!("approval had an authenticated no-effect response: {error:?}"),
                )
            } else {
                // Duplicate is handled above and is positive evidence for
                // the exact persisted approval tuple.  Every other result
                // after an unknown attempt remains held for exact proof.
                approval_unknown(intent, format!("approval remains unresolved: {error:?}"))
            }
        }
        Err((code, message)) => {
            intent.approve_ambiguous = true;
            set_pending(intent, format!("approval call unresolved: {code:?} {message}"))
        }
    }
}

fn receipt_inputs_confirmed(receipt: &LiquidityReceiptV1) -> bool {
    receipt.input_status.iter().any(|status| {
        matches!(
            status,
            Some(LiquidityLegStatusV1::Confirmed { .. })
        )
    })
}

fn receipt_refund_confirmed(receipt: &LiquidityReceiptV1) -> bool {
    receipt
        .input_status
        .iter()
        .zip(receipt.refund_status.iter())
        .any(|(input, refund)| refund_leg_is_final(input, refund))
}

fn refund_leg_is_final(
    input_status: &Option<LiquidityLegStatusV1>,
    refund_status: &Option<LiquidityLegStatusV1>,
) -> bool {
    matches!(
        input_status,
        Some(LiquidityLegStatusV1::Confirmed { .. })
    ) && matches!(
        refund_status,
        Some(LiquidityLegStatusV1::Confirmed { .. } | LiquidityLegStatusV1::Refunded)
    )
}

fn explicit_no_dispatch_rejection(
    status: &LiquidityReceiptStatusV1,
    input_ids: &[Option<u64>; 3],
    refund_ids: &[Option<u64>; 3],
    input_status: &[Option<LiquidityLegStatusV1>; 3],
    refund_status: &[Option<LiquidityLegStatusV1>; 3],
    input_transfers: &[Option<LiquidityInputTransferV1>; 3],
    refund_transfers: &[Option<LiquidityRefundTransferV1>; 3],
) -> bool {
    matches!(status, LiquidityReceiptStatusV1::Rejected)
        && input_ids.iter().all(Option::is_none)
        && refund_ids.iter().all(Option::is_none)
        && input_transfers.iter().all(Option::is_none)
        && refund_transfers.iter().all(Option::is_none)
        // A terminal per-leg no-effect marker is safe.  An absent marker is
        // also accepted only because the receipt itself is an explicit
        // terminal Rejected result with no identity ever allocated.
        && input_status.iter().all(|leg| {
            matches!(
                leg,
                None
                    | Some(
                        LiquidityLegStatusV1::RejectedNoTransfer { .. }
                            | LiquidityLegStatusV1::Skipped
                    )
            )
        })
        && refund_status.iter().all(|leg| {
            matches!(leg, None | Some(LiquidityLegStatusV1::Skipped))
        })
}

fn exact_input_tuple_matches(
    ledger: Principal,
    escrow: Account,
    pool_default: Account,
    amount: u128,
    fee_cap_raw: u128,
    input: &LiquidityInputTransferV1,
) -> bool {
    input.ledger == ledger
        && input.from == escrow
        && input.to == pool_default
        && input.amount == amount
        && input.fee.is_some_and(|fee| fee <= fee_cap_raw)
}

fn exact_refund_tuple_matches(
    ledger: Principal,
    escrow: Account,
    pool_default: Account,
    amount: u128,
    fee_cap_raw: u128,
    refund: &LiquidityRefundTransferV1,
) -> bool {
    refund.ledger == ledger
        && refund.from == pool_default
        && refund.to == escrow
        && refund.gross == amount
        && refund.fee <= fee_cap_raw
        && refund.net.checked_add(refund.fee) == Some(refund.gross)
}

/// A terminal rejection can retain the prepared input/refund tuples even
/// though no value-moving ledger call was dispatched.  The explicit
/// `RejectedNoTransfer`/`Skipped` markers are the proof of no effect; any
/// retained tuples still have to bind to this conversion's escrow and pool
/// accounts before they are accepted.
fn rejected_no_effect_with_exact_tuples(
    intent: &ConversionIntent,
    receipt: &LiquidityReceiptV1,
) -> Result<bool, String> {
    if !matches!(&receipt.status, LiquidityReceiptStatusV1::Rejected) {
        return Ok(false);
    }
    let pool_default = Account {
        owner: intent.pool_canister,
        subaccount: None,
    };
    let fee_cap_raw = u128::from(fee_cap(&intent.request));
    let mut saw_marked_leg = false;
    for index in 0..3 {
        let amount = intent.pool_amounts[index];
        if amount == 0 {
            if receipt.input_ids[index].is_some()
                || receipt.refund_ids[index].is_some()
                || receipt.input_status[index].is_some()
                || receipt.refund_status[index].is_some()
                || receipt.input_transfers[index].is_some()
                || receipt.refund_transfers[index].is_some()
            {
                return Err("rejected pool receipt has an identity for a zero amount leg".into());
            }
            continue;
        }
        let marked_no_effect = matches!(
            receipt.input_status[index].as_ref(),
            Some(LiquidityLegStatusV1::RejectedNoTransfer { .. }
                | LiquidityLegStatusV1::Skipped)
        );
        if marked_no_effect {
            saw_marked_leg = true;
            let input = receipt.input_transfers[index].as_ref().ok_or_else(|| {
                "rejected no-transfer leg omitted its exact prepared input tuple".to_string()
            })?;
            if !exact_input_tuple_matches(
                intent.request.token_ledger,
                intent.escrow_account.clone(),
                pool_default.clone(),
                amount,
                fee_cap_raw,
                input,
            ) {
                return Err("rejected no-transfer input tuple is not bound to the conversion escrow".into());
            }
            if !matches!(
                receipt.refund_status[index].as_ref(),
                None | Some(LiquidityLegStatusV1::Skipped)
            ) {
                return Err("rejected no-transfer leg has a value-moving refund status".into());
            }
            if let Some(refund) = receipt.refund_transfers[index].as_ref() {
                if !matches!(
                    receipt.refund_status[index].as_ref(),
                    Some(LiquidityLegStatusV1::Skipped)
                ) {
                    return Err(
                        "rejected no-transfer refund tuple lacks an explicit skipped marker"
                            .into(),
                    );
                }
                if !exact_refund_tuple_matches(
                    intent.request.token_ledger,
                    intent.escrow_account.clone(),
                    pool_default.clone(),
                    amount,
                    fee_cap_raw,
                    refund,
                ) {
                    return Err("rejected no-transfer refund tuple is not bound to escrow".into());
                }
            }
            continue;
        }

        // A completely absent identity is handled by the older explicit
        // no-dispatch rejection path.  Once a positive leg has a retained
        // identity, however, an unmarked leg is not safe to infer as unused.
        if receipt.input_ids[index].is_none()
            && receipt.refund_ids[index].is_none()
            && receipt.input_status[index].is_none()
            && receipt.refund_status[index].is_none()
            && receipt.input_transfers[index].is_none()
            && receipt.refund_transfers[index].is_none()
        {
            continue;
        }
        return Ok(false);
    }
    Ok(saw_marked_leg)
}

fn validate_receipt_tuples(
    intent: &ConversionIntent,
    receipt: &LiquidityReceiptV1,
) -> Result<(), String> {
    if receipt.request.max_transfer_fee != Some(u128::from(fee_cap(&intent.request)))
        || receipt.request.amounts != intent.pool_amounts
        || receipt.request.min_lp != intent.request.min_lp
    {
        return Err("3pool receipt fee cap or amount plan does not match conversion journal".into());
    }
    match rejected_no_effect_with_exact_tuples(intent, receipt) {
        Ok(true) => return Ok(()),
        Ok(false) => {}
        Err(reason) => return Err(reason),
    }
    if explicit_no_dispatch_rejection(
        &receipt.status,
        &receipt.input_ids,
        &receipt.refund_ids,
        &receipt.input_status,
        &receipt.refund_status,
        &receipt.input_transfers,
        &receipt.refund_transfers,
    ) {
        return Ok(());
    }
    let pool_default = Account {
        owner: intent.pool_canister,
        subaccount: None,
    };
    for index in 0..3 {
        let amount = intent.pool_amounts[index];
        if amount == 0 {
            if receipt.input_transfers[index].is_some()
                || receipt.refund_transfers[index].is_some()
            {
                return Err("3pool supplied a value-moving tuple for a zero amount leg".into());
            }
            continue;
        }
        let input = receipt.input_transfers[index]
            .as_ref()
            .ok_or_else(|| "3pool receipt omitted its exact input tuple".to_string())?;
        if !exact_input_tuple_matches(
            intent.request.token_ledger,
            intent.escrow_account.clone(),
            pool_default.clone(),
            amount,
            u128::from(fee_cap(&intent.request)),
            input,
        ) {
            return Err("3pool input tuple is not bound to the conversion escrow".into());
        }
        let refund = receipt.refund_transfers[index]
            .as_ref()
            .ok_or_else(|| "3pool receipt omitted its exact refund tuple".to_string())?;
        if !exact_refund_tuple_matches(
            intent.request.token_ledger,
            intent.escrow_account,
            pool_default,
            amount,
            u128::from(fee_cap(&intent.request)),
            refund,
        ) {
            return Err("3pool refund tuple is not an exact net compensation to escrow".into());
        }
    }
    Ok(())
}

fn receipt_requires_recovery(status: &LiquidityReceiptStatusV1) -> bool {
    matches!(
        status,
        LiquidityReceiptStatusV1::Prepared
            | LiquidityReceiptStatusV1::Executing
            | LiquidityReceiptStatusV1::RecoveryPending
    )
}

fn receipt_terminal_action(status: &LiquidityReceiptStatusV1) -> Option<bool> {
    match status {
        LiquidityReceiptStatusV1::Completed => Some(true),
        LiquidityReceiptStatusV1::Refunded | LiquidityReceiptStatusV1::Rejected => Some(false),
        LiquidityReceiptStatusV1::Prepared
        | LiquidityReceiptStatusV1::Executing
        | LiquidityReceiptStatusV1::RecoveryPending => None,
    }
}

fn pool_receipt_is_terminal(status: &LiquidityReceiptStatusV1) -> bool {
    matches!(
        status,
        LiquidityReceiptStatusV1::Completed
            | LiquidityReceiptStatusV1::Refunded
            | LiquidityReceiptStatusV1::Rejected
    )
}

fn revoke_allowance_is_frozen(intent: &ConversionIntent) -> bool {
    let terminal = matches!(
        intent.pool_last_status.as_deref(),
        Some("Completed") | Some("Refunded") | Some("Rejected")
    );
    terminal
        && (intent.pool_input_confirmed == Some(true)
            // `RejectedNoTransfer` is a terminal no-effect proof even when
            // the receipt retains the prepared input tuple for auditability.
            || intent.pool_last_status.as_deref() == Some("Rejected"))
}

fn effective_approval_allowance(
    approved: u64,
    expires_at_ns: u64,
    now_ns: u64,
) -> u64 {
    if now_ns >= expires_at_ns {
        0
    } else {
        approved
    }
}

fn terminal_leg_is_known_no_effect(
    receipt_status: &LiquidityReceiptStatusV1,
    input_status: &Option<LiquidityLegStatusV1>,
) -> bool {
    matches!(receipt_status, LiquidityReceiptStatusV1::Rejected)
        && matches!(
            input_status,
            Some(
                LiquidityLegStatusV1::RejectedNoTransfer { .. }
                    | LiquidityLegStatusV1::Skipped
            )
        )
}

/// Freeze the allowance CAS value from the exact terminal pool receipt.  The
/// original approval covers principal plus the pool input fee; once the pool
/// has positively recorded that input, only the unused allowance remains to
/// be revoked.  This is called before the first revoke dispatch and is never
/// recomputed from a nonterminal or ambiguous response.
fn terminal_revoke_expected_allowance(
    intent: &ConversionIntent,
    receipt: &LiquidityReceiptV1,
) -> Result<u64, String> {
    if !pool_receipt_is_terminal(&receipt.status) {
        return Err("allowance CAS cannot be frozen from a nonterminal pool receipt".into());
    }
    let pool_default = Account {
        owner: intent.pool_canister,
        subaccount: None,
    };
    let fee_cap_raw = u128::from(fee_cap(&intent.request));
    // Once the fixed approval expiry has elapsed, the official ledger's
    // effective allowance is zero even if the pool receipt was a no-effect
    // rejection.  Freeze that value before the first revoke dispatch.  The
    // equality boundary is intentional and covered by a pure regression test.
    let mut remaining = effective_approval_allowance(
        intent.approve_amount_raw,
        intent.approve_expires_at_ns,
        ic_cdk::api::time(),
    );
    for index in 0..3 {
        let amount = intent.pool_amounts[index];
        if amount == 0 {
            continue;
        }
        let explicit_absent_no_effect = matches!(&receipt.status, LiquidityReceiptStatusV1::Rejected)
            && receipt.input_status[index].is_none()
            && receipt.input_transfers[index].is_none();
        if terminal_leg_is_known_no_effect(&receipt.status, &receipt.input_status[index])
            || explicit_absent_no_effect
        {
            // A known no-effect leg did not consume the approval.  If a
            // tuple is retained, validate it again here so this accounting
            // helper cannot be called with a differently bound receipt.
            if let Some(input) = receipt.input_transfers[index].as_ref() {
                if !exact_input_tuple_matches(
                    intent.request.token_ledger,
                    intent.escrow_account.clone(),
                    pool_default.clone(),
                    amount,
                    fee_cap_raw,
                    input,
                ) {
                    return Err("rejected no-transfer input tuple does not match the conversion escrow".into());
                }
            }
            if let Some(refund) = receipt.refund_transfers[index].as_ref() {
                if !exact_refund_tuple_matches(
                    intent.request.token_ledger,
                    intent.escrow_account.clone(),
                    pool_default.clone(),
                    amount,
                    fee_cap_raw,
                    refund,
                ) {
                    return Err("rejected no-transfer refund tuple does not match the conversion escrow".into());
                }
            }
            continue;
        }
        let input_confirmed = matches!(
            receipt.input_status[index].as_ref(),
            Some(LiquidityLegStatusV1::Confirmed { .. })
        );
        if !input_confirmed {
            if matches!(&receipt.status, LiquidityReceiptStatusV1::Rejected)
                && receipt.input_transfers[index].is_none()
            {
                continue;
            }
            return Err("terminal pool receipt lacks a confirmed input for allowance accounting".into());
        }
        let input = receipt.input_transfers[index]
            .as_ref()
            .ok_or_else(|| "confirmed pool input is missing its exact tuple".to_string())?;
        if !exact_input_tuple_matches(
            intent.request.token_ledger,
            intent.escrow_account.clone(),
            pool_default.clone(),
            amount,
            fee_cap_raw,
            input,
        ) {
            return Err("confirmed pool input does not match the allowance accounting tuple".into());
        }
        let input_fee = input
            .fee
            .ok_or_else(|| "confirmed pool input is missing its actual fee".to_string())?;
        let input_fee = u64::try_from(input_fee)
            .map_err(|_| "confirmed pool input fee exceeds u64".to_string())?;
        remaining = allowance_after_pool_input(remaining, amount, u128::from(input_fee))?;
    }
    Ok(remaining)
}

fn allowance_after_pool_input(
    approved: u64,
    input_amount: u128,
    input_fee: u128,
) -> Result<u64, String> {
    let consumed = input_amount
        .checked_add(input_fee)
        .ok_or_else(|| "pool input allowance consumption overflows u128".to_string())?;
    let consumed = u64::try_from(consumed)
        .map_err(|_| "pool input allowance consumption exceeds u64".to_string())?;
    approved
        .checked_sub(consumed)
        .ok_or_else(|| "pool input consumed more allowance than approved".to_string())
}

fn apply_pool_receipt(mut intent: ConversionIntent, receipt: LiquidityReceiptV1) -> ConversionIntent {
    if receipt.owner != ic_cdk::api::id()
        || receipt.request.intent_id != intent.pool_intent_id.to_vec()
        || receipt.request.source_subaccount != Some(intent.escrow_subaccount)
    {
        return set_pending(intent, "3pool receipt identity does not match conversion escrow");
    }
    if let Err(reason) = validate_receipt_tuples(&intent, &receipt) {
        return set_pending(intent, reason);
    }
    if pool_receipt_is_terminal(&receipt.status) {
        match terminal_revoke_expected_allowance(&intent, &receipt) {
            Ok(expected) => intent.revoke_expected_allowance_raw = expected,
            Err(reason) => return set_pending(intent, reason),
        }
    }
    intent.pool_input_transfers = receipt.input_transfers.clone();
    intent.pool_refund_transfers = receipt.refund_transfers.clone();
    intent.pool_last_status = Some(format!("{:?}", receipt.status));
    intent.pool_input_confirmed = Some(receipt_inputs_confirmed(&receipt));
    intent.pool_refund_confirmed = Some(receipt_refund_confirmed(&receipt));
    intent.last_error = None;
    match receipt_terminal_action(&receipt.status) {
        Some(true) => {
            let Some(lp_minted) = receipt.lp_minted else {
                return set_pending(intent, "3pool completed receipt omitted lp_minted");
            };
            if intent.lp_credited && intent.pool_lp_minted != Some(lp_minted) {
                return set_pending(intent, "3pool completed receipt conflicts with credited LP");
            }
            intent.pool_lp_minted = Some(lp_minted);
            save_phase(intent, ConversionPhase::Revoking)
        }
        Some(false) => {
            // A terminal compensation/rejection is the no-mint branch.  Do
            // not carry a stale LP amount into the refund path, where it
            // could make a later retry credit or report a mint that never
            // happened.
            if intent.lp_credited {
                return set_pending(intent, "3pool refund arrived after LP credit");
            }
            intent.pool_lp_minted = None;
            save_phase(intent, ConversionPhase::Revoking)
        }
        None => {
            debug_assert!(receipt_requires_recovery(&receipt.status));
            set_pending(intent, "3pool receipt remains nonterminal")
        }
    }
}

fn pool_request(intent: &ConversionIntent) -> AddLiquidityRequestV1 {
    AddLiquidityRequestV1 {
        intent_id: intent.pool_intent_id.to_vec(),
        amounts: intent.pool_amounts.clone(),
        min_lp: intent.request.min_lp,
        source_subaccount: Some(intent.escrow_subaccount),
        max_transfer_fee: Some(u128::from(fee_cap(&intent.request))),
    }
}

fn pool_recovery_error(error: LiquidityReceiptErrorV1) -> String {
    match error {
        LiquidityReceiptErrorV1::Concurrent | LiquidityReceiptErrorV1::PoolLocked => {
            "3pool receipt recovery guard is busy; exact receipt identity remains pending".into()
        }
        other => format!("3pool receipt recovery remains pending: {other:?}"),
    }
}

fn recovery_requires_same_pool_resubmission(error: &LiquidityReceiptErrorV1) -> bool {
    matches!(error, LiquidityReceiptErrorV1::NotFound)
}

async fn submit_same_pool_request(mut intent: ConversionIntent) -> ConversionIntent {
    if read_state(|state| state.configuration.emergency_pause) {
        return set_pending(intent, "conversion paused before same-intent 3pool resubmission");
    }
    if let Err(reason) = crate::ensure_pool_token_balance_mutation_allowed(&[intent.request.token_ledger]) {
        return set_pending(intent, format!("same-intent 3pool resubmission waits for pool mutation guard: {reason:?}"));
    }
    if intent.request.max_fee_budget_raw < fee_cap(&intent.request) * MAX_FEE_LEGS {
        return set_pending(intent, "caller fee budget is below the per-leg cap product");
    }
    // This is deliberately the existing durable pool intent, not a new
    // intent ID or a fresh escrow.  3pool reserves before its first await and
    // deduplicates a delayed original against this exact request.
    intent.pool_dispatched = true;
    update_phase(&mut intent, ConversionPhase::PoolCall);
    if !save_intent(intent.clone()) {
        return set_pending(intent, "conversion identity changed before same-intent 3pool resubmission");
    }
    let request = pool_request(&intent);
    let result: Result<(Result<LiquidityReceiptV1, LiquidityReceiptErrorV1>,), _> =
        call(intent.pool_canister, "add_liquidity_with_receipt_v1", (request,)).await;
    match result {
        Ok((Ok(receipt),)) => apply_pool_receipt(intent, receipt),
        Ok((Err(error),)) => set_pending(intent, format!("same-intent 3pool request unresolved: {error:?}")),
        Err((code, message)) => set_pending(
            intent,
            format!("same-intent 3pool call unresolved: {code:?} {message}"),
        ),
    }
}

async fn recover_pool_receipt(mut intent: ConversionIntent) -> ConversionIntent {
    if let Err(reason) = crate::ensure_pool_token_balance_mutation_allowed(&[intent.request.token_ledger]) {
        return set_pending(intent, format!("3pool receipt recovery waits for pool mutation guard: {reason:?}"));
    }
    // The original add-liquidity call has already been dispatched.  Recovery
    // is keyed by the same durable 32-byte intent and can only reconcile that
    // receipt; it must never allocate a second pool request.
    update_phase(&mut intent, ConversionPhase::PoolCall);
    if !save_intent(intent.clone()) {
        return set_pending(intent, "conversion identity changed before 3pool receipt recovery");
    }
    let result: Result<(Result<LiquidityReceiptV1, LiquidityReceiptErrorV1>,), _> =
        call(
            intent.pool_canister,
            "recover_liquidity_receipt_v1",
            (intent.pool_intent_id.to_vec(),),
        )
        .await;
    match result {
        Ok((Ok(receipt),)) => apply_pool_receipt(intent, receipt),
        Ok((Err(error),)) if recovery_requires_same_pool_resubmission(&error) => {
            // The original call may have been lost before the 3pool canister
            // admitted the receipt.  Re-submit the immutable request with the
            // same 32-byte ID; a delayed original then deduplicates to one
            // receipt and one possible LP mint.
            submit_same_pool_request(intent).await
        }
        Ok((Err(error),)) => set_pending(intent, pool_recovery_error(error)),
        Err((code, message)) => set_pending(
            intent,
            format!("3pool receipt recovery call unresolved: {code:?} {message}"),
        ),
    }
}

async fn pool_call(mut intent: ConversionIntent) -> ConversionIntent {
    if !intent.approve_confirmed {
        return save_phase(intent, ConversionPhase::Refunding);
    }
    if intent.pool_dispatched {
        // A nonterminal receipt (or an ambiguous add-liquidity callback) is
        // resumed through the 3pool recovery rail.  The durable pool intent
        // is the only identity accepted here; a fresh add request would be a
        // second spend attempt against the same escrow.
        return recover_pool_receipt(intent).await;
    }
    if read_state(|state| state.configuration.emergency_pause) {
        return set_pending(intent, "conversion paused before 3pool request");
    }
    if intent.request.max_fee_budget_raw < fee_cap(&intent.request) * MAX_FEE_LEGS {
        return set_pending(
            intent,
            "caller fee budget is below the per-leg cap product; funded escrow remains recoverable",
        );
    }
    submit_same_pool_request(intent).await
}

async fn revoke(mut intent: ConversionIntent) -> ConversionIntent {
    if intent.revoke_confirmed {
        return if intent.pool_lp_minted.is_some() {
            save_phase(intent, ConversionPhase::CreditPending)
        } else {
            save_phase(intent, ConversionPhase::Refunding)
        };
    }
    if !intent.approve_confirmed {
        return if intent.pool_lp_minted.is_some() {
            save_phase(intent, ConversionPhase::CreditPending)
        } else {
            save_phase(intent, ConversionPhase::Refunding)
        };
    }
    if !intent.revoke_dispatched && !revoke_allowance_is_frozen(&intent) {
        return set_pending(
            intent,
            "allowance revoke waits for an exact terminal pool input or rejection proof",
        );
    }
    if intent.revoke_fee_raw > fee_cap(&intent.request) {
        return set_pending(intent, "allowance revoke fee exceeds caller fee budget cap");
    }
    if let Err(reason) = crate::ensure_pool_token_balance_mutation_allowed(&[intent.request.token_ledger]) {
        return set_pending(intent, format!("allowance revoke waits for pool mutation guard: {reason:?}"));
    }
    let now = ic_cdk::api::time();
    if !intent.revoke_dispatched && now >= intent.approve_expires_at_ns {
        // The approval's effective allowance is now zero.  Freeze that CAS
        // value immediately before the first revoke dispatch; once dispatched
        // (or ambiguous) this tuple is immutable and cannot be rebound.
        intent.revoke_expected_allowance_raw = 0;
    }
    if refresh_first_dispatch_tuple_if_stale(
        &mut intent.revoke_created_at_ns,
        &mut intent.revoke_generation,
        intent.revoke_dispatched,
        intent.revoke_ambiguous,
        now,
    ) {
        intent.revoke_memo = retry_memo(
            b"revoke-retry",
            intent.owner,
            intent.request.token_ledger,
            intent.request.intent_seq,
            intent.revoke_generation,
        );
    }
    intent.revoke_dispatched = true;
    update_phase(&mut intent, ConversionPhase::Revoking);
    if !save_intent(intent.clone()) {
        return set_pending(intent, "conversion identity changed before allowance revoke");
    }
    let result: Result<(Result<Nat, ApproveError>,), _> =
        call(intent.request.token_ledger, "icrc2_approve", (revoke_args(&intent),)).await;
    match result {
        Ok((Ok(block),)) => {
            let Ok(block) = block_index(block) else {
                return set_pending(intent, "revoke block index does not fit u64");
            };
            intent.revoke_confirmed = true;
            intent.revoke_ambiguous = false;
            intent.revoke_block = Some(block);
            intent.last_error = None;
            if intent.pool_lp_minted.is_some() {
                save_phase(intent, ConversionPhase::CreditPending)
            } else {
                save_phase(intent, ConversionPhase::Refunding)
            }
        }
        Ok((Err(ApproveError::Duplicate { duplicate_of }),)) => {
            let Ok(block) = block_index(duplicate_of) else {
                return set_pending(intent, "duplicate revoke block index does not fit u64");
            };
            intent.revoke_confirmed = true;
            intent.revoke_ambiguous = false;
            intent.revoke_block = Some(block);
            intent.last_error = None;
            if intent.pool_lp_minted.is_some() {
                save_phase(intent, ConversionPhase::CreditPending)
            } else {
                save_phase(intent, ConversionPhase::Refunding)
            }
        }
        Ok((Err(ApproveError::BadFee { expected_fee }),)) => {
            let Ok(expected_fee) = block_index(expected_fee) else {
                return set_pending(intent, "revoke fee does not fit u64");
            };
            if intent.revoke_ambiguous {
                return set_pending(
                    intent,
                    "revoke returned BadFee after an ambiguous attempt; exact original tuple is retained",
                );
            }
            if expected_fee > fee_cap(&intent.request) {
                return set_pending(intent, "revoke fee exceeds caller fee budget");
            }
            intent.revoke_fee_raw = expected_fee;
            intent.revoke_generation = intent.revoke_generation.saturating_add(1);
            intent.revoke_memo = retry_memo(
                b"revoke-retry",
                intent.owner,
                intent.request.token_ledger,
                intent.request.intent_seq,
                intent.revoke_generation,
            );
            intent.revoke_created_at_ns = next_attempt_time(intent.revoke_created_at_ns);
            intent.revoke_dispatched = false;
            set_pending(intent, format!("ledger requires revoke fee {expected_fee}"))
        }
        Ok((Err(error),)) => {
            intent.revoke_ambiguous = true;
            set_pending(intent, format!("allowance revoke unresolved: {error:?}"))
        }
        Err((code, message)) => {
            intent.revoke_ambiguous = true;
            set_pending(intent, format!("allowance revoke call unresolved: {code:?} {message}"))
        }
    }
}

fn refundable_escrow(intent: &ConversionIntent) -> Result<u64, String> {
    let mut remaining = intent
        .escrow_funded_raw
        .ok_or_else(|| "escrow funding receipt is missing".to_string())?;
    if intent.approve_confirmed {
        remaining = remaining
            .checked_sub(intent.approve_fee_raw)
            .ok_or_else(|| "approval fee exceeds escrow funding".to_string())?;
    }
    if intent.pool_input_confirmed == Some(true) {
        let coin_index = intent
            .pool_coin_index
            .ok_or_else(|| "confirmed pool input is missing its coin index".to_string())?
            as usize;
        let input = intent
            .pool_input_transfers
            .get(coin_index)
            .and_then(Option::as_ref)
            .ok_or_else(|| "confirmed pool input is missing its exact receipt tuple".to_string())?;
        if input.ledger != intent.request.token_ledger
            || input.from != intent.escrow_account
            || input.to
                != (Account {
                    owner: intent.pool_canister,
                    subaccount: None,
                })
            || input.amount != u128::from(intent.request.principal_raw)
        {
            return Err("confirmed pool input tuple no longer matches escrow binding".into());
        }
        let input_fee = input
            .fee
            .ok_or_else(|| "confirmed pool input has no actual fee".to_string())?;
        if input_fee > u128::from(fee_cap(&intent.request)) {
            return Err("confirmed pool input fee exceeds caller budget cap".into());
        }
        let consumed = intent
            .request
            .principal_raw
            .checked_add(u64::try_from(input_fee).map_err(|_| "pool input fee exceeds u64".to_string())?)
            .ok_or_else(|| "pool pull amount overflow".to_string())?;
        remaining = remaining
            .checked_sub(consumed)
            .ok_or_else(|| "pool pull exceeds escrow funding".to_string())?;
        if intent.pool_refund_confirmed == Some(true) {
            let refund = intent
                .pool_refund_transfers
                .get(coin_index)
                .and_then(Option::as_ref)
                .ok_or_else(|| "confirmed pool refund has no exact receipt tuple".to_string())?;
            let pool_default = Account {
                owner: intent.pool_canister,
                subaccount: None,
            };
            if refund.ledger != intent.request.token_ledger
                || refund.from != pool_default
                || refund.to != intent.escrow_account
                || refund.gross != u128::from(intent.request.principal_raw)
                || refund.fee > u128::from(fee_cap(&intent.request))
                || refund.net.checked_add(refund.fee) != Some(refund.gross)
            {
                return Err("confirmed pool refund tuple no longer matches escrow binding".into());
            }
            let compensation = u64::try_from(refund.net)
                .map_err(|_| "pool refund net exceeds u64".to_string())?;
            remaining = remaining
                .checked_add(compensation)
                .ok_or_else(|| "escrow compensation overflow".to_string())?;
        }
    }
    if intent.revoke_confirmed {
        remaining = remaining
            .checked_sub(intent.revoke_fee_raw)
            .ok_or_else(|| "revoke fee exceeds escrow funding".to_string())?;
    }
    Ok(remaining)
}

async fn refund(mut intent: ConversionIntent) -> ConversionIntent {
    if let Err(reason) = crate::ensure_pool_token_balance_mutation_allowed(&[intent.request.token_ledger]) {
        return set_pending(intent, format!("refund waits for pool mutation guard: {reason:?}"));
    }
    if intent.refund_fee_raw > fee_cap(&intent.request) {
        return set_pending(intent, "escrow refund fee exceeds caller fee budget cap");
    }
    let remaining = match refundable_escrow(&intent) {
        Ok(value) => value,
        Err(reason) => return set_pending(intent, reason),
    };
    intent.escrow_remaining_raw = Some(remaining);
    let after_mint = intent.lp_credited && intent.pool_lp_minted.is_some();
    let (transfer_amount, retained_dust) = refund_plan(remaining, intent.refund_fee_raw);
    if remaining == 0 {
        intent.retained_dust_raw = 0;
        intent.phase = if after_mint {
            ConversionPhase::Completed
        } else {
            ConversionPhase::Refunded
        };
        intent.status = if after_mint {
            ConversionStatus::Completed {
                intent_seq: intent.request.intent_seq,
                lp_minted: intent.pool_lp_minted.unwrap_or(0),
                lp_credited: true,
                uncredited_lp: None,
                retained_dust_raw: 0,
            }
        } else {
            ConversionStatus::Refunded {
                intent_seq: intent.request.intent_seq,
                refunded_raw: intent.refunded_raw,
                retained_dust_raw: 0,
            }
        };
        intent.last_error = None;
        save_intent(intent.clone());
        return intent;
    }
    let Some(transfer_amount) = transfer_amount else {
        intent.retained_dust_raw = retained_dust;
        if after_mint {
            intent.phase = ConversionPhase::Completed;
            intent.status = ConversionStatus::Completed {
                intent_seq: intent.request.intent_seq,
                lp_minted: intent.pool_lp_minted.unwrap_or(0),
                lp_credited: true,
                uncredited_lp: None,
                retained_dust_raw: retained_dust,
            };
            intent.last_error = None;
            save_intent(intent.clone());
            return intent;
        }
        intent.last_error = Some(format!(
            "caller-owned escrow dust {remaining} is below refund fee {}",
            intent.refund_fee_raw
        ));
        update_phase(&mut intent, ConversionPhase::Refunding);
        save_intent(intent.clone());
        return intent;
    };
    intent.refund_amount_raw = Some(transfer_amount);
    let now = ic_cdk::api::time();
    if refresh_first_dispatch_tuple_if_stale(
        &mut intent.refund_created_at_ns,
        &mut intent.refund_generation,
        intent.refund_dispatched,
        intent.refund_ambiguous,
        now,
    ) {
        intent.refund_memo = retry_memo(
            b"refund-retry",
            intent.owner,
            intent.request.token_ledger,
            intent.request.intent_seq,
            intent.refund_generation,
        );
    }
    intent.refund_dispatched = true;
    update_phase(&mut intent, ConversionPhase::Refunding);
    if !save_intent(intent.clone()) {
        return set_pending(intent, "conversion identity changed before escrow refund");
    }
    let args = TransferArg {
        from_subaccount: intent.refund_from.subaccount,
        to: intent.refund_to,
        amount: Nat::from(transfer_amount),
        fee: Some(Nat::from(intent.refund_fee_raw)),
        memo: Some(intent.refund_memo.clone().into()),
        created_at_time: Some(intent.refund_created_at_ns),
    };
    let result: Result<(Result<Nat, TransferError>,), _> =
        call(intent.request.token_ledger, "icrc1_transfer", (args,)).await;
    match result {
        Ok((Ok(block),)) => {
            let Ok(block) = block_index(block) else {
                return set_pending(intent, "refund block index does not fit u64");
            };
            intent.refund_block = Some(block);
            intent.refund_ambiguous = false;
            intent.refunded_raw = transfer_amount;
            intent.retained_dust_raw = 0;
            intent.escrow_remaining_raw = Some(0);
            intent.phase = if after_mint {
                ConversionPhase::Completed
            } else {
                ConversionPhase::Refunded
            };
            intent.status = if after_mint {
                ConversionStatus::Completed {
                    intent_seq: intent.request.intent_seq,
                    lp_minted: intent.pool_lp_minted.unwrap_or(0),
                    lp_credited: true,
                    uncredited_lp: None,
                    retained_dust_raw: 0,
                }
            } else {
                ConversionStatus::Refunded {
                    intent_seq: intent.request.intent_seq,
                    refunded_raw: transfer_amount,
                    retained_dust_raw: 0,
                }
            };
            intent.last_error = None;
            save_intent(intent.clone());
            intent
        }
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            let Ok(block) = block_index(duplicate_of) else {
                return set_pending(intent, "duplicate refund block index does not fit u64");
            };
            intent.refund_block = Some(block);
            intent.refund_ambiguous = false;
            intent.refunded_raw = transfer_amount;
            intent.retained_dust_raw = 0;
            intent.escrow_remaining_raw = Some(0);
            intent.phase = if after_mint {
                ConversionPhase::Completed
            } else {
                ConversionPhase::Refunded
            };
            intent.status = if after_mint {
                ConversionStatus::Completed {
                    intent_seq: intent.request.intent_seq,
                    lp_minted: intent.pool_lp_minted.unwrap_or(0),
                    lp_credited: true,
                    uncredited_lp: None,
                    retained_dust_raw: 0,
                }
            } else {
                ConversionStatus::Refunded {
                    intent_seq: intent.request.intent_seq,
                    refunded_raw: transfer_amount,
                    retained_dust_raw: 0,
                }
            };
            intent.last_error = None;
            save_intent(intent.clone());
            intent
        }
        Ok((Err(TransferError::BadFee { expected_fee }),)) => {
            let Ok(expected_fee) = block_index(expected_fee) else {
                return set_pending(intent, "refund fee does not fit u64");
            };
            if intent.refund_ambiguous {
                return set_pending(
                    intent,
                    "refund returned BadFee after an ambiguous attempt; exact original tuple is retained",
                );
            }
            if expected_fee > fee_cap(&intent.request) {
                return set_pending(intent, "refund fee exceeds caller fee budget");
            }
            intent.refund_fee_raw = expected_fee;
            intent.refund_generation = intent.refund_generation.saturating_add(1);
            intent.refund_memo = retry_memo(
                b"refund-retry",
                intent.owner,
                intent.request.token_ledger,
                intent.request.intent_seq,
                intent.refund_generation,
            );
            intent.refund_created_at_ns = next_attempt_time(intent.refund_created_at_ns);
            intent.refund_dispatched = false;
            set_pending(intent, format!("ledger requires refund fee {expected_fee}"))
        }
        Ok((Err(error),)) => {
            intent.refund_ambiguous = true;
            set_pending(intent, format!("refund remains unresolved: {error:?}"))
        }
        Err((code, message)) => {
            intent.refund_ambiguous = true;
            set_pending(intent, format!("refund call unresolved: {code:?} {message}"))
        }
    }
}

fn credit_lp(mut intent: ConversionIntent) -> ConversionIntent {
    let Some(lp_minted) = intent.pool_lp_minted else {
        return set_pending(intent, "credit requested without a completed pool mint");
    };
    if !intent.revoke_confirmed {
        return set_pending(intent, "LP credit waits for exact allowance revocation");
    }
    if intent.lp_credited {
        update_phase(&mut intent, ConversionPhase::Refunding);
        save_intent(intent.clone());
        return intent;
    }
    if lp_minted > u64::MAX as u128 {
        intent.uncredited_lp = Some(lp_minted);
        return set_pending(intent, "LP amount exceeds Stability Pool u64 balance representation");
    }
    if read_state(|state| state.configuration.emergency_pause) {
        intent.uncredited_lp = Some(lp_minted);
        return set_pending(intent, "LP credit waits for emergency pause to clear");
    }
    if let Err(error) = crate::ensure_pool_token_balance_mutation_allowed(&[
        intent.request.token_ledger,
        intent.lp_ledger,
    ]) {
        intent.uncredited_lp = Some(lp_minted);
        return set_pending(intent, format!("LP credit waits for pool mutation guard: {error:?}"));
    }
    let intent_key = (intent.owner, intent.request.intent_seq);
    let credit = mutate_state(|state| {
        {
            let rows = state
                .conversion_intents
                .as_ref()
                .ok_or_else(|| "conversion journal is unavailable".to_string())?;
            let stored = rows
                .get(&intent_key)
                .ok_or_else(|| "conversion journal row disappeared".to_string())?;
            if stored.lp_credited {
                return Err("conversion LP credit was already committed".to_string());
            }
        }
        let current_user = state
            .deposits
            .get(&intent.owner)
            .and_then(|position| position.stablecoin_balances.get(&intent.lp_ledger).copied())
            .unwrap_or(0);
        let current_total = state
            .total_stablecoin_balances
            .get(&intent.lp_ledger)
            .copied()
            .unwrap_or(0);
        let amount = lp_minted as u64;
        if current_user.checked_add(amount).is_none() || current_total.checked_add(amount).is_none() {
            return Err("LP balance would overflow".to_string());
        }
        state.add_deposit_at(intent.owner, intent.lp_ledger, amount, ic_cdk::api::time());
        state.push_event_at(
            intent.owner,
            PoolEventType::DepositAs3USD {
                token_ledger: intent.request.token_ledger,
                amount_in: intent.request.principal_raw,
                lp_minted: amount,
            },
            ic_cdk::api::time(),
        );
        let rows = state
            .conversion_intents
            .as_mut()
            .ok_or_else(|| "conversion journal is unavailable".to_string())?;
        let stored = rows
            .get_mut(&intent_key)
            .ok_or_else(|| "conversion journal row disappeared".to_string())?;
        stored.lp_credited = true;
        stored.uncredited_lp = None;
        stored.last_error = None;
        stored.phase = ConversionPhase::Refunding;
        stored.status = ConversionStatus::Pending {
            intent_seq: stored.request.intent_seq,
            phase: ConversionPhase::Refunding,
            reason: None,
        };
        Ok(())
    });
    if let Err(reason) = credit {
        intent.uncredited_lp = Some(lp_minted);
        return set_pending(intent, reason);
    }
    get_intent(intent.owner, intent.request.intent_seq).unwrap_or_else(|| {
        intent.lp_credited = true;
        intent.uncredited_lp = None;
        update_phase(&mut intent, ConversionPhase::Refunding);
        intent.last_error = None;
        intent
    })
}

async fn run(mut intent: ConversionIntent) -> ConversionStatus {
    // Every loop iteration re-reads the durable row.  A pause or a reentrant
    // state change after an await therefore cannot authorize the next leg from
    // an old in-memory snapshot.
    for _ in 0..12 {
        if is_terminal(&intent.status) {
            return intent.status;
        }
        intent = match intent.phase.clone() {
            ConversionPhase::Reserved => prepare_pool(intent).await,
            ConversionPhase::Pulling => pull(intent).await,
            ConversionPhase::Approving => approve(intent).await,
            ConversionPhase::PoolCall => pool_call(intent).await,
            ConversionPhase::Revoking => revoke(intent).await,
            ConversionPhase::Refunding => refund(intent).await,
            ConversionPhase::CreditPending => credit_lp(intent),
            ConversionPhase::Completed | ConversionPhase::Refunded | ConversionPhase::Rejected => {
                return intent.status;
            }
        };
        // A different update can run while the awaited ledger or 3pool call
        // is suspended.  The durable row is the CAS winner; never authorize
        // the next value-moving leg from a stale in-memory continuation.
        if let Some(durable) = get_intent(intent.owner, intent.request.intent_seq) {
            intent = durable;
        } else {
            return intent.status;
        }
        if is_terminal(&intent.status) {
            return intent.status;
        }
        // A pending status is a durable handoff point for the caller.  The
        // next explicit resume uses the same identities and phase.
        if matches!(intent.status, ConversionStatus::Pending { .. })
            && intent.last_error.is_some()
        {
            return intent.status;
        }
    }
    intent.status
}

pub async fn start_conversion(request: ConversionRequest) -> Result<ConversionStatus, ConversionError> {
    let owner = ic_cdk::api::caller();
    if owner == Principal::anonymous() {
        return Err(ConversionError::Unauthorized);
    }
    ensure_request(&request)?;
    // An existing row is authoritative. Return a cached terminal result or
    // resume its exact identities before consulting mutable route/admission
    // state, so pause or registry changes cannot force a fresh identity.
    if let Some(existing) = get_intent(owner, request.intent_seq) {
        if existing.request != request {
            return Err(ConversionError::IntentConflict);
        }
        if is_terminal(&existing.status) {
            return Ok(existing.status);
        }
        let _live = acquire_live(owner, request.intent_seq)?;
        return Ok(run(existing).await);
    }
    // Hold a transient owner fence across every read-only await.  The durable
    // reserve below is the second fence, so a reentrant caller cannot fill
    // permanent capacity with unfunded rows while this preflight is pending.
    let owner_guard = acquire_owner_admission(owner)?;
    let active_sequence = read_state(|state| {
        state
            .conversion_intents
            .as_ref()
            .and_then(|rows| active_owner_sequence(owner, rows.values()))
    });
    if let Some(active_seq) = active_sequence {
        if !owner_admission_is_free(Some(active_seq)) {
            return Err(ConversionError::ActiveIntent {
                intent_seq: active_seq,
            });
        }
    }
    let route = synchronous_route(request.token_ledger)?;
    preflight_new_intent(owner, &request, route).await?;
    let (lp_ledger, pool_canister, _) = route;
    let intent = reserve_intent(owner, request.clone(), lp_ledger, pool_canister)?;
    if intent.request != request {
        return Err(ConversionError::IntentConflict);
    }
    if is_terminal(&intent.status) {
        return Ok(intent.status);
    }
    drop(owner_guard);
    let _live = acquire_live(owner, request.intent_seq)?;
    Ok(run(intent).await)
}

pub async fn resume_conversion(intent_seq: u64) -> Result<ConversionStatus, ConversionError> {
    let owner = ic_cdk::api::caller();
    if owner == Principal::anonymous() {
        return Err(ConversionError::Unauthorized);
    }
    let intent = get_intent(owner, intent_seq).ok_or(ConversionError::NotFound)?;
    if matches!(
        intent.status,
        ConversionStatus::Completed {
            retained_dust_raw: dust,
            ..
        } if dust > 0
    ) {
        // LP minting is already terminal, but caller-owned dust remains an
        // escrow liability.  Reopen only the refund phase so a later fee drop
        // can recover it without ever retrying the pool mint.
        let _live = acquire_live(owner, intent_seq)?;
        let mut retry = intent;
        retry.phase = ConversionPhase::Refunding;
        retry.last_error = Some("retrying retained caller-owned escrow dust".into());
        retry.status = ConversionStatus::Pending {
            intent_seq,
            phase: ConversionPhase::Refunding,
            reason: retry.last_error.clone(),
        };
        save_intent(retry.clone());
        return Ok(run(retry).await);
    }
    if is_terminal(&intent.status) {
        return Ok(intent.status);
    }
    let _live = acquire_live(owner, intent_seq)?;
    Ok(run(intent).await)
}

fn validate_pool_reconcile_leg(
    pool_dispatched: bool,
    phase: &ConversionPhase,
    pool_amounts: &[u128],
    leg_index: u8,
) -> Result<usize, ConversionError> {
    if !pool_dispatched || !matches!(phase, ConversionPhase::PoolCall) {
        return Err(ConversionError::InvalidRequest {
            reason: "pool input reconciliation requires a dispatched PoolCall intent".into(),
        });
    }
    let index = usize::from(leg_index);
    if index >= pool_amounts.len() || index >= 3 || pool_amounts[index] == 0 {
        return Err(ConversionError::InvalidRequest {
            reason: "pool input reconciliation leg is not a positive planned leg".into(),
        });
    }
    Ok(index)
}

/// Proxy the 3pool's positive input proof through the Stability Pool.  The
/// caller can select only its own durable conversion row; the pool canister,
/// receipt intent ID, escrow subaccount, planned amounts, and request phase
/// all come from that row.  The 3pool canister therefore sees the SP as its
/// authenticated caller while the user retains a caller-scoped recovery API.
pub async fn reconcile_pool_input(
    intent_seq: u64,
    leg_index: u8,
    candidate_block: u64,
) -> Result<ConversionStatus, ConversionError> {
    let owner = ic_cdk::api::caller();
    if owner == Principal::anonymous() {
        return Err(ConversionError::Unauthorized);
    }
    let intent = get_intent(owner, intent_seq).ok_or(ConversionError::NotFound)?;
    if is_terminal(&intent.status) {
        return Ok(intent.status);
    }
    validate_pool_reconcile_leg(
        intent.pool_dispatched,
        &intent.phase,
        &intent.pool_amounts,
        leg_index,
    )?;
    let _live = acquire_live(owner, intent_seq)?;

    // Persist the exact proof selector and durable pool identity before the
    // await.  A transport failure leaves the same conversion pending; it
    // cannot silently switch to another leg, receipt, or escrow.
    let mut attempt = intent.clone();
    let reason = format!(
        "reconciling exact 3pool input proof for leg {leg_index}, candidate block {candidate_block}"
    );
    attempt.last_error = Some(reason.clone());
    attempt.status = ConversionStatus::Pending {
        intent_seq,
        phase: ConversionPhase::PoolCall,
        reason: Some(reason),
    };
    if !save_intent(attempt.clone()) {
        return Err(ConversionError::SystemBusy);
    }

    let result: Result<(Result<LiquidityReceiptV1, LiquidityReceiptErrorV1>,), _> = call(
        attempt.pool_canister,
        "reconcile_liquidity_receipt_input_v1",
        (attempt.pool_intent_id.to_vec(), leg_index, candidate_block),
    )
    .await;
    match result {
        Ok((Ok(receipt),)) => {
            let updated = apply_pool_receipt(attempt, receipt);
            let durable = get_intent(owner, intent_seq).unwrap_or(updated);
            if is_terminal(&durable.status) {
                return Ok(durable.status);
            }
            Ok(run(durable).await)
        }
        Ok((Err(error),)) => {
            let current = get_intent(owner, intent_seq).unwrap_or(attempt);
            Ok(set_pending(current, pool_recovery_error(error)).status)
        }
        Err((code, message)) => {
            let current = get_intent(owner, intent_seq).unwrap_or(attempt);
            Ok(set_pending(
                current,
                format!("3pool input proof call unresolved: {code:?} {message}"),
            )
            .status)
        }
    }
}

async fn fetch_exact_conversion_block(
    ledger: Principal,
    candidate_block: u64,
) -> Result<ICRC3Value, String> {
    // The backend helper proves the exact global block whether the ledger
    // serves it directly or through one validated archive callback.  Keep
    // this wrapper local so every conversion proof still feeds the same field
    // matchers below, without duplicating archive/range handling here.
    rumi_protocol_backend::icrc3_proof::fetch_icrc3_block_value(ledger, candidate_block).await
}

fn conversion_wire_expiry(value: &ICRC3Value) -> Result<Option<u64>, String> {
    let ICRC3Value::Map(block) = value else {
        return Err("conversion proof block is not a map".into());
    };
    let Some(ICRC3Value::Map(tx)) = block.get("tx") else {
        return Err("conversion proof block has no transaction map".into());
    };
    match tx.get("expires_at") {
        Some(ICRC3Value::Nat(expiry)) => expiry
            .0
            .to_u64()
            .map(Some)
            .ok_or_else(|| "conversion proof expiry exceeds u64".to_string()),
        Some(_) => Err("conversion proof expiry is not a Nat".into()),
        None => Ok(None),
    }
}

fn conversion_wire_expected_allowance(value: &ICRC3Value) -> Result<Option<u64>, String> {
    let ICRC3Value::Map(block) = value else {
        return Err("conversion proof block is not a map".into());
    };
    let Some(ICRC3Value::Map(tx)) = block.get("tx") else {
        return Err("conversion proof block has no transaction map".into());
    };
    match tx.get("expected_allowance") {
        Some(ICRC3Value::Nat(expected)) => expected
            .0
            .to_u64()
            .map(Some)
            .ok_or_else(|| "conversion proof expected allowance exceeds u64".to_string()),
        Some(_) => Err("conversion proof expected allowance is not a Nat".into()),
        None => Ok(None),
    }
}

fn conversion_approval_fields_match(
    block: &rumi_protocol_backend::icrc3_proof::DecodedBlock,
    from: &Account,
    spender: &Account,
    amount: u64,
    fee: u64,
    memo: &[u8],
    created_at_ns: u64,
) -> bool {
    block.op == "approve"
        && block.btype.as_deref().is_none_or(|kind| kind == "2approve")
        && block.from.as_ref() == Some(from)
        && block.to.is_none()
        && block.spender.as_ref() == Some(spender)
        && block.amount == u128::from(amount)
        && block.fee == Some(fee)
        && block.memo.as_deref() == Some(memo)
        && block.created_at_time == Some(created_at_ns)
}

fn verify_conversion_approval_value(
    intent: &ConversionIntent,
    value: &ICRC3Value,
    from: Account,
    spender: Account,
    amount: u64,
    expected_allowance: u64,
    fee: u64,
    memo: &[u8],
    created_at_ns: u64,
    expiry: Option<u64>,
) -> Result<(), String> {
    let block = rumi_protocol_backend::icrc3_proof::decode_block(value)
        .map_err(|error| format!("malformed conversion approval proof: {error}"))?;
    if !conversion_approval_fields_match(
        &block,
        &from,
        &spender,
        amount,
        fee,
        memo,
        created_at_ns,
    )
        || conversion_wire_expected_allowance(value)? != Some(expected_allowance)
        || conversion_wire_expiry(value)? != expiry
    {
        return Err(format!(
            "candidate approval proof does not match the exact persisted tuple for conversion {}",
            intent.request.intent_seq
        ));
    }
    Ok(())
}

fn conversion_refund_fields_match(
    block: &rumi_protocol_backend::icrc3_proof::DecodedBlock,
    from: &Account,
    to: &Account,
    amount: u64,
    fee: u64,
    memo: &[u8],
    created_at_ns: u64,
) -> bool {
    (block.op == "xfer" || block.op == "transfer")
        && block.btype.as_deref().is_none_or(|kind| kind == "1xfer")
        && block.from.as_ref() == Some(from)
        && block.to.as_ref() == Some(to)
        && block.spender.is_none()
        && block.amount == u128::from(amount)
        && block.fee == Some(fee)
        && block.memo.as_deref() == Some(memo)
        && block.created_at_time == Some(created_at_ns)
}

fn verify_conversion_refund_value(
    intent: &ConversionIntent,
    value: &ICRC3Value,
    amount: u64,
) -> Result<(), String> {
    let block = rumi_protocol_backend::icrc3_proof::decode_block(value)
        .map_err(|error| format!("malformed conversion refund proof: {error}"))?;
    if !conversion_refund_fields_match(
        &block,
        &intent.refund_from,
        &intent.refund_to,
        amount,
        intent.refund_fee_raw,
        &intent.refund_memo,
        intent.refund_created_at_ns,
    ) {
        return Err(format!(
            "candidate refund proof does not match the exact persisted tuple for conversion {}",
            intent.request.intent_seq
        ));
    }
    Ok(())
}

fn proof_pending(
    owner: Principal,
    intent_seq: u64,
    reason: impl Into<String>,
) -> Result<ConversionStatus, ConversionError> {
    let current = get_intent(owner, intent_seq).ok_or(ConversionError::NotFound)?;
    if is_terminal(&current.status) {
        return Ok(current.status);
    }
    Ok(set_pending(current, reason).status)
}

/// Reconcile an aged approval only from an exact ICRC-3 approval block. A
/// candidate block never changes the journal unless the complete persisted
/// tuple and phase still match after verification.
pub async fn reconcile_approval(
    intent_seq: u64,
    candidate_block: u64,
) -> Result<ConversionStatus, ConversionError> {
    let owner = ic_cdk::api::caller();
    if owner == Principal::anonymous() {
        return Err(ConversionError::Unauthorized);
    }
    let intent = get_intent(owner, intent_seq).ok_or(ConversionError::NotFound)?;
    if is_terminal(&intent.status) {
        return Ok(intent.status);
    }
    if intent.approve_confirmed {
        return resume_conversion(intent_seq).await;
    }
    if !matches!(&intent.phase, ConversionPhase::Approving)
        || !intent.approve_dispatched
        || intent.approve_block.is_some()
    {
        return Err(ConversionError::InvalidRequest {
            reason: "approval reconciliation requires a dispatched unconfirmed approval".into(),
        });
    }
    let _live = acquire_live(owner, intent_seq)?;
    let value = match fetch_exact_conversion_block(intent.request.token_ledger, candidate_block).await {
        Ok(value) => value,
        Err(reason) => return proof_pending(owner, intent_seq, format!("exact approval proof remains pending: {reason}")),
    };
    if let Err(reason) = verify_conversion_approval_value(
        &intent,
        &value,
        intent.approve_from.clone(),
        intent.approve_spender.clone(),
        intent.approve_amount_raw,
        intent.approve_expected_allowance_raw,
        intent.approve_fee_raw,
        &intent.approve_memo,
        intent.approve_created_at_ns,
        Some(intent.approve_expires_at_ns),
    ) {
        return proof_pending(owner, intent_seq, format!("exact approval proof remains pending: {reason}"));
    }
    let committed = mutate_state(|state| {
        let Some(rows) = state.conversion_intents.as_mut() else {
            return false;
        };
        let Some(stored) = rows.get_mut(&(owner, intent_seq)) else {
            return false;
        };
        if stored.request != intent.request
            || !matches!(&stored.phase, ConversionPhase::Approving)
            || !stored.approve_dispatched
            || stored.approve_confirmed
            || stored.approve_block.is_some()
            || stored.approve_generation != intent.approve_generation
            || stored.approve_amount_raw != intent.approve_amount_raw
            || stored.approve_expected_allowance_raw != intent.approve_expected_allowance_raw
            || stored.approve_fee_raw != intent.approve_fee_raw
            || stored.approve_memo != intent.approve_memo
            || stored.approve_created_at_ns != intent.approve_created_at_ns
        {
            return false;
        }
        stored.approve_confirmed = true;
        stored.approve_ambiguous = false;
        stored.approve_block = Some(candidate_block);
        stored.last_error = None;
        stored.phase = ConversionPhase::PoolCall;
        stored.status = ConversionStatus::Pending {
            intent_seq,
            phase: ConversionPhase::PoolCall,
            reason: None,
        };
        true
    });
    if !committed {
        return Err(ConversionError::SystemBusy);
    }
    let durable = get_intent(owner, intent_seq).ok_or(ConversionError::NotFound)?;
    Ok(run(durable).await)
}

/// Reconcile an aged allowance revoke from its exact ICRC-3 approval block.
pub async fn reconcile_revoke(
    intent_seq: u64,
    candidate_block: u64,
) -> Result<ConversionStatus, ConversionError> {
    let owner = ic_cdk::api::caller();
    if owner == Principal::anonymous() {
        return Err(ConversionError::Unauthorized);
    }
    let intent = get_intent(owner, intent_seq).ok_or(ConversionError::NotFound)?;
    if is_terminal(&intent.status) {
        return Ok(intent.status);
    }
    if intent.revoke_confirmed {
        return resume_conversion(intent_seq).await;
    }
    if !matches!(&intent.phase, ConversionPhase::Revoking)
        || !intent.revoke_dispatched
        || intent.revoke_block.is_some()
    {
        return Err(ConversionError::InvalidRequest {
            reason: "revoke reconciliation requires a dispatched unconfirmed revoke".into(),
        });
    }
    let _live = acquire_live(owner, intent_seq)?;
    let value = match fetch_exact_conversion_block(intent.request.token_ledger, candidate_block).await {
        Ok(value) => value,
        Err(reason) => return proof_pending(owner, intent_seq, format!("exact revoke proof remains pending: {reason}")),
    };
    if let Err(reason) = verify_conversion_approval_value(
        &intent,
        &value,
        intent.revoke_from.clone(),
        intent.revoke_spender.clone(),
        intent.revoke_amount_raw,
        intent.revoke_expected_allowance_raw,
        intent.revoke_fee_raw,
        &intent.revoke_memo,
        intent.revoke_created_at_ns,
        None,
    ) {
        return proof_pending(owner, intent_seq, format!("exact revoke proof remains pending: {reason}"));
    }
    let committed = mutate_state(|state| {
        let Some(rows) = state.conversion_intents.as_mut() else {
            return false;
        };
        let Some(stored) = rows.get_mut(&(owner, intent_seq)) else {
            return false;
        };
        if stored.request != intent.request
            || !matches!(&stored.phase, ConversionPhase::Revoking)
            || !stored.revoke_dispatched
            || stored.revoke_confirmed
            || stored.revoke_block.is_some()
            || stored.revoke_generation != intent.revoke_generation
            || stored.revoke_expected_allowance_raw != intent.revoke_expected_allowance_raw
            || stored.revoke_amount_raw != intent.revoke_amount_raw
            || stored.revoke_fee_raw != intent.revoke_fee_raw
            || stored.revoke_memo != intent.revoke_memo
            || stored.revoke_created_at_ns != intent.revoke_created_at_ns
        {
            return false;
        }
        stored.revoke_confirmed = true;
        stored.revoke_ambiguous = false;
        stored.revoke_block = Some(candidate_block);
        stored.last_error = None;
        let next_phase = if stored.pool_lp_minted.is_some() {
            ConversionPhase::CreditPending
        } else {
            ConversionPhase::Refunding
        };
        stored.phase = next_phase.clone();
        stored.status = ConversionStatus::Pending {
            intent_seq,
            phase: next_phase,
            reason: None,
        };
        true
    });
    if !committed {
        return Err(ConversionError::SystemBusy);
    }
    let durable = get_intent(owner, intent_seq).ok_or(ConversionError::NotFound)?;
    Ok(run(durable).await)
}

/// Reconcile an aged escrow refund from its exact ICRC-3 transfer block.
pub async fn reconcile_refund(
    intent_seq: u64,
    candidate_block: u64,
) -> Result<ConversionStatus, ConversionError> {
    let owner = ic_cdk::api::caller();
    if owner == Principal::anonymous() {
        return Err(ConversionError::Unauthorized);
    }
    let intent = get_intent(owner, intent_seq).ok_or(ConversionError::NotFound)?;
    if is_terminal(&intent.status) {
        return Ok(intent.status);
    }
    if intent.refund_block.is_some() {
        return resume_conversion(intent_seq).await;
    }
    let Some(refund_amount) = intent.refund_amount_raw else {
        return Err(ConversionError::InvalidRequest {
            reason: "refund reconciliation requires a persisted refund amount".into(),
        });
    };
    if !matches!(&intent.phase, ConversionPhase::Refunding)
        || !intent.refund_dispatched
        || intent.pool_lp_minted.is_some() && !intent.lp_credited
    {
        return Err(ConversionError::InvalidRequest {
            reason: "refund reconciliation requires a dispatched recoverable refund".into(),
        });
    }
    let _live = acquire_live(owner, intent_seq)?;
    let value = match fetch_exact_conversion_block(intent.request.token_ledger, candidate_block).await {
        Ok(value) => value,
        Err(reason) => return proof_pending(owner, intent_seq, format!("exact refund proof remains pending: {reason}")),
    };
    if let Err(reason) = verify_conversion_refund_value(&intent, &value, refund_amount) {
        return proof_pending(owner, intent_seq, format!("exact refund proof remains pending: {reason}"));
    }
    let committed = mutate_state(|state| {
        let Some(rows) = state.conversion_intents.as_mut() else {
            return false;
        };
        let Some(stored) = rows.get_mut(&(owner, intent_seq)) else {
            return false;
        };
        if stored.request != intent.request
            || !matches!(&stored.phase, ConversionPhase::Refunding)
            || !stored.refund_dispatched
            || stored.refund_block.is_some()
            || stored.refund_generation != intent.refund_generation
            || stored.refund_amount_raw != Some(refund_amount)
            || stored.refund_fee_raw != intent.refund_fee_raw
            || stored.refund_memo != intent.refund_memo
            || stored.refund_created_at_ns != intent.refund_created_at_ns
            || (stored.pool_lp_minted.is_some() && !stored.lp_credited)
        {
            return false;
        }
        stored.refund_block = Some(candidate_block);
        stored.refund_ambiguous = false;
        stored.refunded_raw = refund_amount;
        stored.retained_dust_raw = 0;
        stored.escrow_remaining_raw = Some(0);
        stored.phase = if stored.lp_credited {
            ConversionPhase::Completed
        } else {
            ConversionPhase::Refunded
        };
        stored.status = if stored.lp_credited {
            ConversionStatus::Completed {
                intent_seq,
                lp_minted: stored.pool_lp_minted.unwrap_or(0),
                lp_credited: true,
                uncredited_lp: None,
                retained_dust_raw: 0,
            }
        } else {
            ConversionStatus::Refunded {
                intent_seq,
                refunded_raw: refund_amount,
                retained_dust_raw: 0,
            }
        };
        stored.last_error = None;
        true
    });
    if !committed {
        return Err(ConversionError::SystemBusy);
    }
    get_intent(owner, intent_seq)
        .map(|intent| intent.status)
        .ok_or(ConversionError::NotFound)
}

/// Reconcile an aged ambiguous user pull with a caller-supplied exact ICRC-3
/// block. The persisted tuple is the only authority; a failed proof leaves the
/// same intent pending and never allocates a replacement sequence or escrow.
pub async fn reconcile_pull(
    intent_seq: u64,
    candidate_block: u64,
) -> Result<ConversionStatus, ConversionError> {
    let owner = ic_cdk::api::caller();
    if owner == Principal::anonymous() {
        return Err(ConversionError::Unauthorized);
    }
    let intent = get_intent(owner, intent_seq).ok_or(ConversionError::NotFound)?;
    if is_terminal(&intent.status) {
        return Ok(intent.status);
    }
    if intent.pull_block.is_some() {
        return resume_conversion(intent_seq).await;
    }
    if !intent.pull_dispatched {
        return Err(ConversionError::InvalidRequest {
            reason: "conversion pull has not been dispatched".into(),
        });
    }
    let _live = acquire_live(owner, intent_seq)?;
    if let Err(reason) = crate::deposits::verify_exact_conversion_pull_block(
        intent.request.token_ledger,
        candidate_block,
        &intent.pull_from,
        &intent.pull_to,
        intent.pull_amount_raw,
        intent.pull_call_fee_raw,
        &intent.pull_memo,
        intent.pull_created_at_ns,
    )
    .await
    {
        let current = get_intent(owner, intent_seq).unwrap_or(intent);
        if is_terminal(&current.status) {
            return Ok(current.status);
        }
        return Ok(
            set_pending(current, format!("exact pull proof remains pending: {reason}")).status,
        );
    }
    let committed = mutate_state(|state| {
        let rows = state
            .conversion_intents
            .as_mut()
            .ok_or(ConversionError::SystemBusy)?;
        let stored = rows
            .get_mut(&(owner, intent_seq))
            .ok_or(ConversionError::NotFound)?;
        if stored.request != intent.request
            || stored.pull_block.is_some()
            || !stored.pull_dispatched
        {
            return Err(ConversionError::SystemBusy);
        }
        stored.pull_block = Some(candidate_block);
        stored.pull_ambiguous = false;
        stored.escrow_funded_raw = Some(stored.pull_amount_raw);
        stored.escrow_remaining_raw = Some(stored.pull_amount_raw);
        stored.last_error = None;
        stored.phase = ConversionPhase::Approving;
        stored.status = ConversionStatus::Pending {
            intent_seq,
            phase: ConversionPhase::Approving,
            reason: None,
        };
        Ok(())
    });
    committed?;
    let durable = get_intent(owner, intent_seq).ok_or(ConversionError::NotFound)?;
    Ok(run(durable).await)
}

pub fn cancel_conversion(intent_seq: u64) -> Result<ConversionStatus, ConversionError> {
    let owner = ic_cdk::api::caller();
    if owner == Principal::anonymous() {
        return Err(ConversionError::Unauthorized);
    }
    let Some(mut intent) = get_intent(owner, intent_seq) else {
        return Err(ConversionError::NotFound);
    };
    if is_terminal(&intent.status) {
        return Ok(intent.status);
    }
    // Cancellation participates in the same per-intent live guard as start and
    // resume. A reentrant cancel must not turn an in-flight quote or ledger
    // continuation into a stale writer.
    let _live = acquire_live(owner, intent_seq)?;
    if intent.pull_dispatched || intent.pull_block.is_some() || intent.approve_dispatched {
        return Err(ConversionError::CannotCancel);
    }
    intent = set_rejected(intent, "cancelled before user pull");
    Ok(intent.status)
}

/// Upgrade hook: transient guards are intentionally not persisted.  Any row
/// that was in-flight before the upgrade stays in its exact phase and tuple,
/// with a visible reconciliation reason; no escrow, memo, timestamp, or
/// sequence is reallocated.
pub fn reconcile_after_upgrade() -> usize {
    mutate_state(|state| {
        let Some(rows) = state.conversion_intents.as_mut() else {
            state.conversion_intents = Some(Default::default());
            state.conversion_high_watermarks = Some(Default::default());
            return 0;
        };
        let mut count = 0;
        for intent in rows.values_mut() {
            if is_terminal(&intent.status) {
                continue;
            }
            count += 1;
            if intent.pull_dispatched && intent.pull_block.is_none() {
                intent.pull_ambiguous = true;
            }
            if intent.approve_dispatched && !intent.approve_confirmed {
                intent.approve_ambiguous = true;
            }
            if intent.revoke_dispatched && !intent.revoke_confirmed {
                intent.revoke_ambiguous = true;
            }
            if intent.refund_dispatched && intent.refund_block.is_none() {
                intent.refund_ambiguous = true;
            }
            let reason = "upgrade interrupted conversion; resume the exact durable tuple".to_string();
            intent.last_error = Some(reason.clone());
            intent.status = ConversionStatus::Pending {
                intent_seq: intent.request.intent_seq,
                phase: intent.phase.clone(),
                reason: Some(reason),
            };
        }
        state.conversion_high_watermarks.get_or_insert_with(Default::default);
        count
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fee_budget_requires_all_reserved_legs_without_overflow() {
        assert_eq!(fee_budget(0).unwrap(), 0);
        assert_eq!(fee_budget(7).unwrap(), 35);
        assert!(fee_budget(u64::MAX).is_err());
    }

    #[test]
    fn admission_requires_principal_budget_and_ingress_fee_without_overflow() {
        assert_eq!(preflight_pull_amount(100, 20, 3).unwrap(), 123);
        assert!(preflight_pull_amount(u64::MAX, 1, 0).is_err());
        assert!(preflight_pull_amount(u64::MAX - 1, 0, 1).is_ok());
    }

    #[test]
    fn admission_requires_both_wallet_balance_and_sp_allowance() {
        assert!(wallet_preflight_sufficient(123, 123, 123));
        assert!(!wallet_preflight_sufficient(122, 123, 123));
        assert!(!wallet_preflight_sufficient(123, 122, 123));
        assert!(!wallet_preflight_sufficient(u128::MAX, 122, 123));
    }

    #[test]
    fn owner_admission_guard_rejects_only_nonterminal_owner_work() {
        assert!(owner_admission_is_free(None));
        assert!(!owner_admission_is_free(Some(7)));
    }

    #[test]
    fn quote_minimum_equality_is_accepted() {
        assert!(ensure_quote_meets_min_lp(1_999_602_769, 1_999_602_769).is_ok());
    }

    #[test]
    fn quote_above_minimum_is_accepted() {
        assert!(ensure_quote_meets_min_lp(1_999_602_770, 1_999_602_769).is_ok());
    }

    #[test]
    fn quote_below_minimum_is_rejected() {
        assert!(ensure_quote_meets_min_lp(1_999_602_768, 1_999_602_769).is_err());
    }

    #[test]
    fn refund_plan_never_sends_fee_or_more_from_caller_escrow() {
        assert_eq!(refund_plan(0, 10), (None, 0));
        assert_eq!(refund_plan(10, 10), (None, 10));
        assert_eq!(refund_plan(11, 10), (Some(1), 0));
        assert_eq!(refund_plan(u64::MAX, 1), (Some(u64::MAX - 1), 0));
    }

    #[test]
    fn terminal_statuses_are_the_only_statuses_that_stop_retries() {
        let pending = ConversionStatus::Pending {
            intent_seq: 1,
            phase: ConversionPhase::Refunding,
            reason: Some("ambiguous".into()),
        };
        assert!(!is_terminal(&pending));
        assert!(is_terminal(&ConversionStatus::Completed {
            intent_seq: 1,
            lp_minted: 2,
            lp_credited: true,
            uncredited_lp: None,
            retained_dust_raw: 0,
        }));
        assert!(is_terminal(&ConversionStatus::Refunded {
            intent_seq: 1,
            refunded_raw: 2,
            retained_dust_raw: 0,
        }));
        assert!(is_terminal(&ConversionStatus::Rejected {
            intent_seq: 1,
            reason: "known no-effect".into(),
        }));
    }

    #[test]
    fn legacy_nonzero_fee_is_not_admitted_by_zero_budget() {
        let required = fee_budget(3).unwrap();
        assert_eq!(required, 15);
        assert!(0 < required);
    }

    #[test]
    fn fee_cap_reserves_a_bound_for_each_value_moving_leg() {
        let request = ConversionRequest {
            intent_seq: 1,
            token_ledger: Principal::anonymous(),
            principal_raw: 100,
            max_fee_budget_raw: 17,
            min_lp: 1,
        };
        assert_eq!(fee_cap(&request), 3);
        assert!(fee_cap(&request) * MAX_FEE_LEGS <= request.max_fee_budget_raw);
        assert!(request.max_fee_budget_raw < (fee_cap(&request) + 1) * MAX_FEE_LEGS);
    }

    #[test]
    fn funded_approval_no_effect_is_not_refundable_after_unknown() {
        let error = ApproveError::TooOld;
        assert!(approval_no_effect_can_refund(false, &error));
        assert!(!approval_no_effect_can_refund(true, &error));
        assert!(approval_bad_fee_can_retry(false, 3, 3));
        assert!(!approval_bad_fee_can_retry(false, 4, 3));
        assert!(!approval_bad_fee_can_retry(true, 3, 3));
        assert!(!approval_no_effect_can_refund(
            false,
            &ApproveError::GenericError {
                error_code: Nat::from(9u64),
                message: "unknown".into(),
            }
        ));
    }

    #[test]
    fn revoke_expected_allowance_uses_exact_pool_input_consumption() {
        assert_eq!(allowance_after_pool_input(105, 100, 2).unwrap(), 3);
        assert!(allowance_after_pool_input(101, 100, 2).is_err());
        assert!(allowance_after_pool_input(u64::MAX, u128::from(u64::MAX), 1).is_err());
    }

    #[test]
    fn approval_expiry_boundary_freezes_effective_allowance_to_zero() {
        assert_eq!(effective_approval_allowance(105, 500, 499), 105);
        assert_eq!(effective_approval_allowance(105, 500, 500), 0);
        assert_eq!(effective_approval_allowance(105, 500, 501), 0);
    }

    #[test]
    fn stale_undispatched_legs_refresh_once_before_first_dispatch() {
        let old = 100;
        let now = old + CREATED_AT_MAX_AGE_NS + 1;
        let mut prior_phase_legs = [(old, 0u64), (old, 4u64), (old, 8u64), (old, 12u64)];
        for (created_at, generation) in &mut prior_phase_legs {
            assert!(refresh_first_dispatch_tuple_if_stale(
                created_at,
                generation,
                false,
                false,
                now,
            ));
            assert_eq!(*created_at, now);
            assert!(*generation > 0);
        }

        let mut dispatched_created = old;
        let mut dispatched_generation = 2;
        assert!(!refresh_first_dispatch_tuple_if_stale(
            &mut dispatched_created,
            &mut dispatched_generation,
            true,
            false,
            now,
        ));
        assert_eq!(dispatched_created, old);
        assert_eq!(dispatched_generation, 2);

        let mut ambiguous_created = old;
        let mut ambiguous_generation = 3;
        assert!(!refresh_first_dispatch_tuple_if_stale(
            &mut ambiguous_created,
            &mut ambiguous_generation,
            false,
            true,
            now,
        ));
        assert_eq!(ambiguous_created, old);
        assert_eq!(ambiguous_generation, 3);

        let mut boundary_created = old;
        let mut boundary_generation = 0;
        assert!(refresh_first_dispatch_tuple_if_stale(
            &mut boundary_created,
            &mut boundary_generation,
            false,
            false,
            old + CREATED_AT_MAX_AGE_NS,
        ));
        assert_eq!(boundary_created, old + CREATED_AT_MAX_AGE_NS);
        assert_eq!(boundary_generation, 1);
    }

    #[test]
    fn rejected_no_transfer_exact_input_is_known_no_effect() {
        let ledger = Principal::self_authenticating(b"rejected-ledger");
        let escrow = Account {
            owner: Principal::self_authenticating(b"rejected-sp"),
            subaccount: Some([7; 32]),
        };
        let pool_default = Account {
            owner: Principal::self_authenticating(b"rejected-pool"),
            subaccount: None,
        };
        let input = LiquidityInputTransferV1 {
            ledger,
            from: escrow.clone(),
            to: pool_default.clone(),
            amount: 100,
            fee: Some(2),
            memo: Some(vec![1, 2, 3]),
            created_at_time: 44,
        };
        assert!(exact_input_tuple_matches(
            ledger,
            escrow,
            pool_default,
            100,
            2,
            &input,
        ));
        assert!(terminal_leg_is_known_no_effect(
            &LiquidityReceiptStatusV1::Rejected,
            &Some(LiquidityLegStatusV1::RejectedNoTransfer {
                reason: "paused before dispatch".into(),
            }),
        ));
        assert!(!terminal_leg_is_known_no_effect(
            &LiquidityReceiptStatusV1::Completed,
            &Some(LiquidityLegStatusV1::RejectedNoTransfer {
                reason: "wrong terminal status".into(),
            }),
        ));
        // The no-effect marker makes the terminal allowance branch retain
        // the approved value; only a Confirmed input calls
        // `allowance_after_pool_input`.
        assert_eq!(effective_approval_allowance(102, 500, 499), 102);
    }

    #[test]
    fn aged_approval_and_refund_proofs_reject_wrong_tuple_fields() {
        let from = Account {
            owner: Principal::self_authenticating(b"proof-sp"),
            subaccount: Some([4; 32]),
        };
        let spender = Account {
            owner: Principal::self_authenticating(b"proof-pool"),
            subaccount: None,
        };
        let to = Account {
            owner: Principal::self_authenticating(b"proof-user"),
            subaccount: None,
        };
        let memo = b"proof-memo".to_vec();
        let approval = rumi_protocol_backend::icrc3_proof::DecodedBlock {
            btype: Some("2approve".into()),
            op: "approve".into(),
            from: Some(from.clone()),
            to: None,
            spender: Some(spender.clone()),
            amount: 102,
            fee: Some(2),
            created_at_time: Some(8),
            memo: Some(memo.clone()),
        };
        assert!(conversion_approval_fields_match(
            &approval,
            &from,
            &spender,
            102,
            2,
            &memo,
            8,
        ));
        assert!(!conversion_approval_fields_match(
            &approval,
            &from,
            &Account {
                owner: Principal::self_authenticating(b"wrong-spender"),
                subaccount: None,
            },
            102,
            2,
            &memo,
            8,
        ));
        assert!(!conversion_approval_fields_match(
            &approval,
            &from,
            &spender,
            103,
            2,
            &memo,
            8,
        ));

        let refund = rumi_protocol_backend::icrc3_proof::DecodedBlock {
            btype: Some("1xfer".into()),
            op: "xfer".into(),
            from: Some(from.clone()),
            to: Some(to.clone()),
            spender: None,
            amount: 100,
            fee: Some(2),
            created_at_time: Some(9),
            memo: Some(memo.clone()),
        };
        assert!(conversion_refund_fields_match(
            &refund, &from, &to, 100, 2, &memo, 9
        ));
        assert!(!conversion_refund_fields_match(
            &refund, &from, &to, 99, 2, &memo, 9
        ));
    }

    #[test]
    fn approval_proof_expected_allowance_is_required_and_exact() {
        let mut tx = std::collections::BTreeMap::new();
        tx.insert(
            "expected_allowance".into(),
            ICRC3Value::Nat(Nat::from(7u64)),
        );
        let mut block = std::collections::BTreeMap::new();
        block.insert("tx".into(), ICRC3Value::Map(tx.clone()));
        let value = ICRC3Value::Map(block);
        assert_eq!(conversion_wire_expected_allowance(&value).unwrap(), Some(7));
        assert_ne!(conversion_wire_expected_allowance(&value).unwrap(), Some(8));

        tx.remove("expected_allowance");
        let mut missing_block = std::collections::BTreeMap::new();
        missing_block.insert("tx".into(), ICRC3Value::Map(tx));
        assert_eq!(
            conversion_wire_expected_allowance(&ICRC3Value::Map(missing_block)).unwrap(),
            None
        );
    }

    #[test]
    fn allowance_freeze_is_terminal_only_and_nonterminal_stays_held() {
        assert!(pool_receipt_is_terminal(&LiquidityReceiptStatusV1::Completed));
        assert!(pool_receipt_is_terminal(&LiquidityReceiptStatusV1::Refunded));
        assert!(pool_receipt_is_terminal(&LiquidityReceiptStatusV1::Rejected));
        assert!(!pool_receipt_is_terminal(&LiquidityReceiptStatusV1::Executing));
        assert!(!pool_receipt_is_terminal(&LiquidityReceiptStatusV1::RecoveryPending));
    }

    #[test]
    fn refunded_compensation_leg_is_final_evidence() {
        let input = Some(LiquidityLegStatusV1::Confirmed {
            block: Nat::from(17u64),
        });
        assert!(refund_leg_is_final(
            &input,
            &Some(LiquidityLegStatusV1::Refunded)
        ));
        assert!(refund_leg_is_final(
            &input,
            &Some(LiquidityLegStatusV1::Confirmed {
                block: Nat::from(18u64),
            })
        ));
        assert!(!refund_leg_is_final(
            &input,
            &Some(LiquidityLegStatusV1::Submitted)
        ));
        assert!(!refund_leg_is_final(
            &Some(LiquidityLegStatusV1::Submitted),
            &Some(LiquidityLegStatusV1::Refunded)
        ));
    }

    #[test]
    fn nonterminal_pool_receipt_recovers_to_refund_without_lp_credit() {
        for status in [
            LiquidityReceiptStatusV1::Prepared,
            LiquidityReceiptStatusV1::Executing,
            LiquidityReceiptStatusV1::RecoveryPending,
        ] {
            assert!(receipt_requires_recovery(&status));
            assert_eq!(receipt_terminal_action(&status), None);
        }

        let confirmed_input = Some(LiquidityLegStatusV1::Confirmed {
            block: Nat::from(41u64),
        });
        let refunded_leg = Some(LiquidityLegStatusV1::Refunded);
        assert!(refund_leg_is_final(&confirmed_input, &refunded_leg));

        // A recovered Refunded receipt must enter the SP refund path.  It can
        // never select the LP-credit branch, and a later duplicate response
        // therefore cannot credit LP a second time.
        assert_eq!(
            receipt_terminal_action(&LiquidityReceiptStatusV1::Refunded),
            Some(false)
        );
        assert_ne!(
            receipt_terminal_action(&LiquidityReceiptStatusV1::Refunded),
            receipt_terminal_action(&LiquidityReceiptStatusV1::Completed)
        );
        assert_eq!(refund_plan(101, 1), (Some(100), 0));
    }

    #[test]
    fn missing_pool_receipt_retries_same_id_but_guard_errors_stay_held() {
        assert!(recovery_requires_same_pool_resubmission(
            &LiquidityReceiptErrorV1::NotFound
        ));
        assert!(!recovery_requires_same_pool_resubmission(
            &LiquidityReceiptErrorV1::Concurrent
        ));
        assert!(!recovery_requires_same_pool_resubmission(
            &LiquidityReceiptErrorV1::PoolLocked
        ));
        assert!(pool_recovery_error(LiquidityReceiptErrorV1::Concurrent).contains("pending"));
        assert!(pool_recovery_error(LiquidityReceiptErrorV1::PoolLocked).contains("pending"));
    }

    #[test]
    fn pool_input_proxy_rejects_wrong_leg_and_wrong_escrow_tuple() {
        let ledger = Principal::self_authenticating(b"conversion-ledger");
        let owner = Principal::self_authenticating(b"conversion-owner");
        let pool = Principal::self_authenticating(b"conversion-pool");
        let escrow = Account {
            owner: Principal::self_authenticating(b"stability-pool"),
            subaccount: Some([7; 32]),
        };
        let pool_default = Account {
            owner: pool,
            subaccount: None,
        };
        let input = LiquidityInputTransferV1 {
            ledger,
            from: escrow,
            to: pool_default,
            amount: 100,
            fee: Some(2),
            memo: Some(vec![1, 2, 3]),
            created_at_time: 44,
        };
        assert!(exact_input_tuple_matches(
            ledger,
            escrow,
            pool_default,
            100,
            2,
            &input,
        ));
        let mut wrong_input = input.clone();
        wrong_input.from = Account {
            owner,
            subaccount: None,
        };
        assert!(!exact_input_tuple_matches(
            ledger,
            escrow,
            pool_default,
            100,
            2,
            &wrong_input,
        ));

        let refund = LiquidityRefundTransferV1 {
            ledger,
            from: pool_default,
            to: escrow,
            gross: 100,
            net: 98,
            fee: 2,
            memo: vec![4, 5, 6],
            created_at_time: 45,
        };
        assert!(exact_refund_tuple_matches(
            ledger,
            escrow,
            pool_default,
            100,
            2,
            &refund,
        ));
        let mut wrong_refund = refund.clone();
        wrong_refund.to = Account {
            owner,
            subaccount: None,
        };
        assert!(!exact_refund_tuple_matches(
            ledger,
            escrow,
            pool_default,
            100,
            2,
            &wrong_refund,
        ));

        assert!(validate_pool_reconcile_leg(
            true,
            &ConversionPhase::PoolCall,
            &[100, 0, 0],
            0,
        )
        .is_ok());
        assert!(validate_pool_reconcile_leg(
            true,
            &ConversionPhase::PoolCall,
            &[100, 0, 0],
            1,
        )
        .is_err());
        assert!(validate_pool_reconcile_leg(
            false,
            &ConversionPhase::PoolCall,
            &[100, 0, 0],
            0,
        )
        .is_err());
    }

    #[test]
    fn explicit_rejected_receipt_without_any_identity_is_safe() {
        let ids = [None; 3];
        let statuses = [None, None, None];
        let input_transfers: [Option<LiquidityInputTransferV1>; 3] = [None, None, None];
        let refund_transfers: [Option<LiquidityRefundTransferV1>; 3] = [None, None, None];
        assert!(explicit_no_dispatch_rejection(
            &LiquidityReceiptStatusV1::Rejected,
            &ids,
            &ids,
            &statuses,
            &statuses,
            &input_transfers,
            &refund_transfers,
        ));

        let mut unknown_status = [None, None, None];
        unknown_status[0] = Some(LiquidityLegStatusV1::Unresolved {
            reason: "unknown".into(),
        });
        assert!(!explicit_no_dispatch_rejection(
            &LiquidityReceiptStatusV1::Rejected,
            &ids,
            &ids,
            &unknown_status,
            &statuses,
            &input_transfers,
            &refund_transfers,
        ));
    }
}

