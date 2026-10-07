use candid::{CandidType, Deserialize, Principal};
use ic_stable_structures::{StableBTreeMap, StableCell, Storable};
use icrc_ledger_types::icrc1::{account::Account, transfer::TransferArg};
use serde::Serialize;
use std::borrow::Cow;
use std::cell::RefCell;

use crate::memory;

/// Durable state for the ICP treasury-bonus leg. `Prepared` means the exact
/// ICRC-1 request was committed to stable history before its first call.
/// `Quarantined` is terminal for automatic retry and does not mean paid.
#[derive(CandidType, Clone, Debug, Deserialize, Serialize, PartialEq)]
pub enum IcpTreasuryBonusState {
    Prepared,
    Paid,
    Quarantined,
}

/// Exact transfer identity needed to safely retry an ambiguous ICRC-1 call.
#[derive(CandidType, Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct IcpTreasuryBonusTransfer {
    pub ledger: Principal,
    /// The current code uses the bot canister's default account as sender.
    /// Persist it explicitly so retries can reject a mismatched identity.
    pub from: Account,
    pub args: TransferArg,
    pub block_index: Option<u64>,
}

/// Exact, claim-generation-bound ICP transfer used to return collateral after
/// a liquidation swap fails. The full ledger tuple is durable before dispatch
/// so an admin retry can replay the same transfer identity after reply loss.
#[derive(CandidType, Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct BotClaimReturnTransfer {
    pub ledger: Principal,
    pub from: Account,
    pub collateral_amount_e8s: u64,
    pub ledger_fee_e8s: u64,
    pub claim_timestamp: u64,
    pub args: TransferArg,
    pub block_index: Option<u64>,
    /// Calls armed for this exact tuple. Legacy `None` stays replay-only and
    /// can never authorize tuple replacement.
    #[serde(default)]
    pub dispatch_attempt_count: Option<u32>,
    /// Typed first-attempt no-effect tuples retained across fee replacement.
    #[serde(default)]
    pub prior_no_effects: Option<Vec<BotCkUsdcPaymentNoEffect>>,
    /// Last typed result for this tuple. Ambiguous outcomes permanently
    /// disable repricing and require exact-tuple reconciliation.
    #[serde(default)]
    pub dispatch_observation: Option<BotCkUsdcPaymentDispatchObservation>,
}

/// Exact ckUSDC top-up request for a short but verified liquidation payment.
/// Every ICRC-1 duplicate-detection field is persisted before dispatch.
#[derive(CandidType, Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct BotCkUsdcTopUpTransfer {
    pub ledger: Principal,
    pub args: TransferArg,
    pub block_index: Option<u64>,
    /// Number of calls durably armed for this exact tuple. Legacy `None`
    /// provenance can never authorize replacing a tuple after rejection.
    #[serde(default)]
    pub dispatch_attempt_count: Option<u32>,
    /// Typed no-effect rejections for replaced tuples, retained as tombstones.
    #[serde(default)]
    pub prior_no_effects: Option<Vec<BotCkUsdcPaymentNoEffect>>,
    /// Typed observation of dispatching this exact top-up tuple. Missing on
    /// legacy snapshots; transport failures remain Ambiguous.
    #[serde(default)]
    pub dispatch_observation: Option<BotCkUsdcPaymentDispatchObservation>,
    /// Bounded fixed-snapshot positive-evidence scan after an uncertain or
    /// TooOld reply. Its presence permanently disables another dispatch.
    #[serde(default)]
    pub history_scan: Option<BotCkUsdcPaymentHistoryScan>,
}

/// Exact original ckUSDC payment tuple. It is persisted before dispatch so an
/// ambiguous ledger reply can be reconciled with the same duplicate-detection
/// identity instead of constructing a fresh transfer.
#[derive(CandidType, Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct BotCkUsdcPaymentTransfer {
    pub ledger: Principal,
    pub args: TransferArg,
    pub block_index: Option<u64>,
    /// Number of calls durably armed for this exact tuple. A replacement tuple
    /// is permitted only after a typed no-effect response on its first call.
    #[serde(default)]
    /// `None` marks a pre-upgrade tuple whose earlier dispatch history is
    /// unknown. It may replay only its exact tuple and can never prove that a
    /// typed rejection was its first attempt.
    pub dispatch_attempt_count: Option<u32>,
    /// Exact prior tuples rejected with typed no-effect evidence. Kept as
    /// tombstones when a first-attempt BadFee/InsufficientFunds is repriced.
    #[serde(default)]
    pub prior_no_effects: Option<Vec<BotCkUsdcPaymentNoEffect>>,
    /// Durable typed result of dispatching this exact tuple. BadFee and
    /// InsufficientFunds authorize replacement only when attempt_count is one;
    /// TooOld and every other failure remain history-reconciliation only.
    /// Missing on legacy snapshots.
    #[serde(default)]
    pub dispatch_observation: Option<BotCkUsdcPaymentDispatchObservation>,
    /// Fixed-snapshot, bounded ICRC-3 reconciliation after an ambiguous or
    /// TooOld dispatch. `None` means no history scan has started.
    #[serde(default)]
    pub history_scan: Option<BotCkUsdcPaymentHistoryScan>,
}

#[derive(CandidType, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum BotCkUsdcPaymentDispatchObservation {
    TooOld,
    BadFee { expected_fee: u64 },
    InsufficientFunds { balance: u64 },
    Ambiguous,
}

#[derive(CandidType, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BotCkUsdcPaymentNoEffect {
    pub ledger: Principal,
    pub args: TransferArg,
    pub observation: BotCkUsdcPaymentDispatchObservation,
}

#[derive(CandidType, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BotCkUsdcPaymentHistoryScan {
    pub snapshot_log_length: u64,
    pub next_index: u64,
    /// An exact candidate is persisted before it is promoted to block_index.
    pub candidate_block_index: Option<u64>,
    /// Reserved fail-closed marker for conflicting candidate evidence.
    pub multiple_candidates: bool,
}

/// Durable evidence that a completed swap stopped before any ckUSDC payment
/// because its measured reservation could not meet this claim's minimum.
/// This marker only authorizes operator review; it does not prove a payment.
#[derive(CandidType, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BotCkUsdcShortfallEligibility {
    pub ledger: Principal,
    pub measured_reserved_output_e6: u64,
    pub minimum_payment_e6: u64,
}

pub const CKUSDC_PAYMENT_HISTORY_PAGE_SIZE: u64 = 8;

#[derive(CandidType, Clone, Debug, Deserialize, Serialize, PartialEq)]
pub enum LiquidationStatus {
    Completed,
    SwapFailed,
    TransferFailed,
    ConfirmFailed,
    ClaimFailed,
    AdminResolved,
}

#[derive(CandidType, Clone, Debug, Deserialize, Serialize)]
pub struct LiquidationRecordV1 {
    pub id: u64,
    pub vault_id: u64,
    pub timestamp: u64,
    pub status: LiquidationStatus,

    pub collateral_claimed_e8s: u64,
    pub debt_to_cover_e8s: u64,
    pub icp_swapped_e8s: u64,
    pub ckusdc_received_e6: u64,
    pub ckusdc_transferred_e6: u64,
    /// Gross ICP bonus obligation. `TransferFailed` records retain the unpaid
    /// obligation here; only `Completed` means its ledger transfer succeeded.
    pub icp_to_treasury_e8s: u64,

    pub oracle_price_e8s: u64,
    pub effective_price_e8s: u64,
    pub slippage_bps: i32,

    pub error_message: Option<String>,
    pub confirm_retry_count: u8,
    /// SAT-001 proof binding for backend confirmation. Legacy rows omit these
    /// optional fields and cannot authorize a new confirmation retry.
    pub claim_timestamp: Option<u64>,
    pub payment_memo: Option<Vec<u8>>,
    pub ckusdc_payment_block_index: Option<u64>,
    pub ckusdc_payment_amount_e6: Option<u64>,
    /// Missing on legacy records written before exact transfer journaling.
    pub icp_treasury_transfer: Option<IcpTreasuryBonusTransfer>,
    /// Also identifies a quarantined legacy obligation that has no transfer
    /// identity to replay safely. Missing means historical/unknown.
    pub icp_treasury_bonus_state: Option<IcpTreasuryBonusState>,
}

#[derive(CandidType, Clone, Debug, Deserialize, Serialize)]
pub enum LiquidationRecordVersioned {
    V1(LiquidationRecordV1),
}

impl Storable for LiquidationRecordVersioned {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Owned(candid::encode_one(self).expect("Failed to encode record"))
    }
    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        candid::decode_one(&bytes).expect("Failed to decode record")
    }
    const BOUND: ic_stable_structures::storable::Bound =
        ic_stable_structures::storable::Bound::Unbounded;
}

thread_local! {
    static HISTORY: RefCell<Option<StableBTreeMap<u64, LiquidationRecordVersioned, memory::Mem>>> =
        RefCell::new(None);

    static NEXT_ID: RefCell<Option<StableCell<u64, memory::Mem>>> =
        RefCell::new(None);
    static CLAIM_INTENTS: RefCell<Option<StableBTreeMap<u64, BotClaimIntent, memory::Mem>>> =
        RefCell::new(None);
}

/// Backend receipt recovered for one durable claim request. Keeping the
/// complete result lets a resumed bot verify the exact same generation.
#[derive(CandidType, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BotClaimReceipt {
    pub collateral_amount: u64,
    pub debt_covered: u64,
    pub collateral_price_e8s: u64,
    pub claim_timestamp: u64,
    pub payment_memo: Vec<u8>,
    /// Backend-authenticated exact ledger receipt for collateral entering the
    /// bot. Missing on legacy snapshots and never sufficient for return.
    #[serde(default)]
    pub claim_transfer: Option<BotClaimTransferReceipt>,
}

#[derive(CandidType, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BotClaimTransferReceipt {
    pub ledger: Principal,
    pub block_index: u64,
    pub from: Account,
    pub to: Account,
    pub amount_e8s: u64,
    pub fee_e8s: u64,
    pub memo: Vec<u8>,
    pub created_at_time: u64,
    pub return_account: Account,
}

/// Durable per-vault identity for a backend claim call. `ClaimRequested` may
/// replay the same request ID after a lost reply. A verified `Claimed` result
/// can enter the swap once; `SwapStarted` is held after an upgrade or lost
/// reply because the external swap may already have completed.
#[derive(CandidType, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum BotClaimIntentPhase {
    /// Legacy phase name retained so previously persisted snapshots decode.
    AwaitingReceipt,
    ClaimRequested,
    Claimed(BotClaimReceipt),
    SwapStarted(BotClaimReceipt),
    /// Legacy acquired phase had no durable distinction between pre-swap and
    /// post-dispatch state. It remains held and is never replayed as a swap.
    Acquired { claim_timestamp: u64, payment_memo: Vec<u8> },
    /// The backend proved this exact request had no effect. Keep its digest
    /// durable until the backend confirms the bounded tombstone ACK; retries
    /// in this phase may ACK only and must never dispatch a claim again.
    NoEffectAcknowledgementPending { transfer_digest: Vec<u8> },
}

#[derive(CandidType, Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct BotClaimIntent {
    pub vault_id: u64,
    pub request_id: u64,
    /// Calls started with this ID. A retry-side NotFound cannot prove that an
    /// earlier reply-lost call had no effect.
    #[serde(default)]
    pub claim_call_count: Option<u32>,
    /// Backend that owns this request identity. `None` is legacy/unknown and
    /// must never be rebound to the current configuration.
    #[serde(default)]
    pub backend_principal: Option<Principal>,
    /// Missing on pre-recovery Acquired intents. Those intents stay held and
    /// must not synthesize a new transfer identity during recovery.
    #[serde(default)]
    pub return_transfer: Option<BotClaimReturnTransfer>,
    /// Exact retry identity for the ckUSDC recovery leg, if one was prepared.
    #[serde(default)]
    pub ckusdc_top_up_transfer: Option<BotCkUsdcTopUpTransfer>,
    /// Original ckUSDC payment tuple, saved before its first ledger call.
    #[serde(default)]
    pub ckusdc_payment_transfer: Option<BotCkUsdcPaymentTransfer>,
    /// Set only for a receipt-bound completed swap held before any ckUSDC
    /// payment dispatch. Immutable once recorded. Missing on legacy snapshots.
    #[serde(default)]
    pub shortfall_eligibility: Option<BotCkUsdcShortfallEligibility>,
    /// ICP ledger fee pinned for the ICPSwap pool input in this claim generation.
    /// Missing on legacy snapshots; once set it is immutable.
    #[serde(default)]
    pub icp_pool_input_fee_e8s: Option<u64>,
    pub phase: BotClaimIntentPhase,
}

impl Storable for BotClaimIntent {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Owned(candid::encode_one(self).expect("Failed to encode bot claim intent"))
    }
    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        candid::decode_one(&bytes).expect("Failed to decode bot claim intent")
    }
    const BOUND: ic_stable_structures::storable::Bound =
        ic_stable_structures::storable::Bound::Unbounded;
}

/// Initialize history storage. Must be called after memory::init_memory_manager().
pub fn init_history() {
    HISTORY.with(|h| {
        *h.borrow_mut() = Some(StableBTreeMap::init(
            memory::get_memory(memory::MEM_ID_HISTORY),
        ));
    });
    NEXT_ID.with(|c| {
        *c.borrow_mut() = Some(
            StableCell::init(memory::get_memory(memory::MEM_ID_NEXT_ID), 0u64)
                .expect("Failed to init NEXT_ID cell"),
        );
    });
    CLAIM_INTENTS.with(|m| {
        *m.borrow_mut() = Some(StableBTreeMap::init(
            memory::get_memory(memory::MEM_ID_CLAIM_INTENTS),
        ));
    });
}

pub fn get_claim_intent(vault_id: u64) -> Option<BotClaimIntent> {
    CLAIM_INTENTS.with(|m| m.borrow().as_ref().expect("Claim intent map not initialized").get(&vault_id))
}

/// Atomically enter SwapStarted and pin the exact ICP fee passed to the pool.
/// The request and claimed receipt must still be the active generation.
pub fn mark_claim_swap_started(
    vault_id: u64,
    request_id: u64,
    expected_receipt: &BotClaimReceipt,
    fee_e8s: u64,
) -> bool {
    CLAIM_INTENTS.with(|m| {
        let mut borrow = m.borrow_mut();
        let map = borrow.as_mut().expect("Claim intent map not initialized");
        let Some(mut intent) = map.get(&vault_id) else { return false; };
        if intent.vault_id != vault_id || intent.request_id != request_id {
            return false;
        }
        match &intent.phase {
            BotClaimIntentPhase::Claimed(receipt) if receipt == expected_receipt => {
                intent.phase = BotClaimIntentPhase::SwapStarted(receipt.clone());
            }
            BotClaimIntentPhase::SwapStarted(receipt)
                if receipt == expected_receipt && intent.icp_pool_input_fee_e8s == Some(fee_e8s) =>
            {
                return true;
            }
            _ => return false,
        }
        if intent.icp_pool_input_fee_e8s.is_some_and(|saved| saved != fee_e8s) {
            return false;
        }
        intent.icp_pool_input_fee_e8s = Some(fee_e8s);
        map.insert(vault_id, intent);
        true
    })
}

pub fn put_claim_intent(intent: BotClaimIntent) {
    CLAIM_INTENTS.with(|m| {
        m.borrow_mut().as_mut().expect("Claim intent map not initialized")
            .insert(intent.vault_id, intent);
    });
}

/// Persist the next same-ID backend request before dispatch. Returns the
/// number of earlier calls so the caller can treat a later NotFound as
/// unresolved rather than as negative transfer proof.
pub fn begin_claim_request(vault_id: u64, request_id: u64) -> Option<Option<u32>> {
    CLAIM_INTENTS.with(|m| {
        let mut borrow = m.borrow_mut();
        let map = borrow.as_mut().expect("Claim intent map not initialized");
        let mut intent = map.get(&vault_id)?;
        if intent.vault_id != vault_id
            || intent.request_id != request_id
            || !matches!(
                intent.phase,
                BotClaimIntentPhase::AwaitingReceipt | BotClaimIntentPhase::ClaimRequested
            )
            || intent.claim_call_count == Some(u32::MAX)
        {
            return None;
        }
        let prior_count = intent.claim_call_count;
        intent.claim_call_count = Some(prior_count.unwrap_or(0) + 1);
        map.insert(vault_id, intent);
        Some(prior_count)
    })
}

pub fn remove_claim_intent(vault_id: u64) {
    CLAIM_INTENTS.with(|m| {
        m.borrow_mut().as_mut().expect("Claim intent map not initialized").remove(&vault_id);
    });
}

/// Persist only a backend-authenticated proof for the exact request before
/// ACK. Any changed request, phase, backend, call count, digest, or return
/// journal keeps the intent held.
pub fn mark_no_effect_acknowledgement_pending(
    vault_id: u64,
    request_id: u64,
    backend_principal: Principal,
    transfer_digest: Vec<u8>,
) -> bool {
    if transfer_digest.len() != 32 {
        return false;
    }
    CLAIM_INTENTS.with(|m| {
        let mut borrow = m.borrow_mut();
        let map = borrow.as_mut().expect("Claim intent map not initialized");
        let Some(intent) = map.get(&vault_id) else {
            return false;
        };
        if intent.vault_id != vault_id
            || intent.request_id != request_id
            || intent.backend_principal != Some(backend_principal)
            || intent.claim_call_count.map_or(true, |count| count == 0)
            || intent.return_transfer.is_some()
        {
            return false;
        }
        let mut intent = intent;
        match &intent.phase {
            BotClaimIntentPhase::ClaimRequested | BotClaimIntentPhase::AwaitingReceipt => {
                intent.phase = BotClaimIntentPhase::NoEffectAcknowledgementPending {
                    transfer_digest,
                };
            }
            BotClaimIntentPhase::NoEffectAcknowledgementPending {
                transfer_digest: existing,
            } if existing == &transfer_digest => return true,
            _ => return false,
        }
        map.insert(vault_id, intent);
        true
    })
}

/// Remove an intent only after ACK succeeded for its exact persisted proof.
/// If the ACK reply was lost, the phase remains and an absent-tombstone ACK is
/// retried idempotently before the request can be released.
pub fn clear_acknowledged_no_effect_claim_intent(
    vault_id: u64,
    request_id: u64,
    backend_principal: Principal,
    transfer_digest: &[u8],
) -> bool {
    CLAIM_INTENTS.with(|m| {
        let mut borrow = m.borrow_mut();
        let map = borrow.as_mut().expect("Claim intent map not initialized");
        let Some(intent) = map.get(&vault_id) else {
            return false;
        };
        if intent.vault_id != vault_id
            || intent.request_id != request_id
            || intent.backend_principal != Some(backend_principal)
            || !matches!(
                &intent.phase,
                BotClaimIntentPhase::NoEffectAcknowledgementPending { transfer_digest: saved }
                    if saved.as_slice() == transfer_digest
            )
        {
            return false;
        }
        map.remove(&vault_id);
        true
    })
}

pub fn update_claim_return_transfer(
    vault_id: u64,
    transfer: BotClaimReturnTransfer,
) -> bool {
    CLAIM_INTENTS.with(|m| {
        let mut borrow = m.borrow_mut();
        let map = borrow.as_mut().expect("Claim intent map not initialized");
        let Some(mut intent) = map.get(&vault_id) else {
            return false;
        };
        if intent.vault_id != vault_id
            || !matches!(
                intent.phase,
                BotClaimIntentPhase::Acquired { .. } | BotClaimIntentPhase::Claimed(_)
            )
        {
            return false;
        }
        if let Some(existing) = &intent.return_transfer {
            if existing.ledger != transfer.ledger
                || existing.from != transfer.from
                || existing.collateral_amount_e8s != transfer.collateral_amount_e8s
                || existing.ledger_fee_e8s != transfer.ledger_fee_e8s
                || existing.claim_timestamp != transfer.claim_timestamp
                || existing.args != transfer.args
            {
                return false;
            }
            if existing.prior_no_effects != transfer.prior_no_effects {
                return false;
            }
            if existing.dispatch_attempt_count != transfer.dispatch_attempt_count {
                if existing.dispatch_attempt_count.and_then(|count| count.checked_add(1)) != transfer.dispatch_attempt_count
                    || existing.block_index.is_some()
                    || transfer.block_index.is_some()
                    || existing.dispatch_observation != transfer.dispatch_observation
                {
                    return false;
                }
            }
            if existing.block_index.is_some() && existing.block_index != transfer.block_index {
                return false;
            }
            if existing.dispatch_observation.is_some()
                && existing.dispatch_observation != transfer.dispatch_observation
            {
                return false;
            }
            if existing.dispatch_observation.is_none()
                && transfer.dispatch_observation.is_some()
                && (existing.dispatch_attempt_count != transfer.dispatch_attempt_count
                    || existing.block_index.is_some())
            {
                return false;
            }
        }
        intent.return_transfer = Some(transfer);
        map.insert(vault_id, intent);
        true
    })
}

/// Replace a return tuple only after its first dispatch was rejected by a
/// typed BadFee response. All other outcomes, including ambiguity, are held.
pub fn replace_claim_return_after_first_bad_fee(
    vault_id: u64,
    expected: &BotClaimReturnTransfer,
    mut replacement: BotClaimReturnTransfer,
) -> bool {
    CLAIM_INTENTS.with(|m| {
        let mut borrow = m.borrow_mut();
        let map = borrow.as_mut().expect("Claim intent map not initialized");
        let Some(mut intent) = map.get(&vault_id) else { return false; };
        let expected_fee = match expected.dispatch_observation.as_ref() {
            Some(BotCkUsdcPaymentDispatchObservation::BadFee { expected_fee }) => *expected_fee,
            _ => return false,
        };
        let expected_replacement_amount = expected.collateral_amount_e8s.checked_add(expected_fee);
        if intent.vault_id != vault_id
            || !matches!(intent.phase, BotClaimIntentPhase::Claimed(_))
            || intent.return_transfer.as_ref() != Some(expected)
            || expected.dispatch_attempt_count != Some(1)
            || expected.block_index.is_some()
            || expected.prior_no_effects.is_none()
            || expected.prior_no_effects.as_ref().map_or(0, Vec::len) >= 16
            || !matches!(expected.dispatch_observation.as_ref(), Some(BotCkUsdcPaymentDispatchObservation::BadFee { .. }))
            || replacement.dispatch_attempt_count != Some(0)
            || replacement.block_index.is_some()
            || replacement.dispatch_observation.is_some()
            || replacement.ledger != expected.ledger
            || replacement.from != expected.from
            || replacement.collateral_amount_e8s != expected.collateral_amount_e8s
            || replacement.claim_timestamp != expected.claim_timestamp
            || replacement.args.from_subaccount != expected.args.from_subaccount
            || replacement.args.to != expected.args.to
            || replacement.args.memo != expected.args.memo
            || replacement.args.amount.0.to_string().parse::<u64>().ok() != expected_replacement_amount
            || replacement.prior_no_effects != expected.prior_no_effects
        {
            return false;
        }
        let Some(old_created) = expected.args.created_at_time else { return false; };
        let Some(new_created) = replacement.args.created_at_time else { return false; };
        if new_created <= old_created { return false; }
        let replacement_fee = replacement.args.fee.as_ref()
            .and_then(|fee| fee.0.to_string().parse::<u64>().ok());
        if replacement_fee != Some(expected_fee) { return false; }
        let Some(prior) = expected.prior_no_effects.clone() else { return false; };
        let mut prior = prior;
        prior.push(BotCkUsdcPaymentNoEffect {
            ledger: expected.ledger,
            args: expected.args.clone(),
            observation: expected.dispatch_observation.clone().expect("checked BadFee"),
        });
        replacement.prior_no_effects = Some(prior);
        intent.return_transfer = Some(replacement);
        map.insert(vault_id, intent);
        true
    })
}

/// Store a top-up tuple once. A retry may only add the returned block index;
/// it cannot change ledger, account, amount, fee, memo, or created_at_time.
pub fn update_ckusdc_top_up_transfer(
    vault_id: u64,
    transfer: BotCkUsdcTopUpTransfer,
) -> bool {
    CLAIM_INTENTS.with(|m| {
        let mut borrow = m.borrow_mut();
        let map = borrow.as_mut().expect("Claim intent map not initialized");
        let Some(mut intent) = map.get(&vault_id) else {
            return false;
        };
        if intent.vault_id != vault_id
            || !matches!(
                intent.phase,
                BotClaimIntentPhase::Claimed(_) | BotClaimIntentPhase::SwapStarted(_)
            )
        {
            return false;
        }
        if let Some(existing) = &intent.ckusdc_top_up_transfer {
            if !same_ckusdc_top_up_identity(existing, &transfer) {
                return false;
            }
            if existing.prior_no_effects != transfer.prior_no_effects {
                return false;
            }
            if existing.dispatch_attempt_count != transfer.dispatch_attempt_count {
                if existing.dispatch_attempt_count.and_then(|count| count.checked_add(1)) != transfer.dispatch_attempt_count
                    || existing.block_index.is_some()
                    || existing.dispatch_observation.is_some()
                    || existing.history_scan.is_some()
                    || transfer.dispatch_observation.is_some()
                    || transfer.history_scan.is_some()
                    || transfer.block_index.is_some()
                {
                    return false;
                }
            }
            if existing.block_index.is_some() && existing.block_index != transfer.block_index {
                return false;
            }
            if existing.dispatch_observation.is_some()
                && existing.dispatch_observation != transfer.dispatch_observation
            {
                return false;
            }
            if existing.dispatch_observation.is_none()
                && transfer.dispatch_observation.is_some()
                && (existing.dispatch_attempt_count != transfer.dispatch_attempt_count
                    || existing.block_index.is_some())
            {
                return false;
            }
            match (&existing.history_scan, &transfer.history_scan) {
                (Some(previous), Some(next)) if payment_history_scan_advances(previous, next) => {}
                (Some(previous), None)
                    if transfer.block_index.is_some()
                        && payment_history_scan_found_exactly_one(previous, transfer.block_index.unwrap()) => {}
                (None, Some(next))
                    if existing.block_index.is_none()
                        && transfer.dispatch_observation.is_some()
                        && next.next_index == next.snapshot_log_length
                        && next.candidate_block_index.is_none()
                        && !next.multiple_candidates => {}
                (None, None) => {}
                _ => return false,
            }
        }
        intent.ckusdc_top_up_transfer = Some(transfer);
        map.insert(vault_id, intent);
        true
    })
}

/// Replace a top-up only after its first armed attempt received a typed
/// BadFee/InsufficientFunds response. The transfer amount and destination stay
/// fixed; only the fee and duplicate-detection time may be refreshed.
pub fn replace_ckusdc_top_up_after_first_no_effect(
    vault_id: u64,
    expected: &BotCkUsdcTopUpTransfer,
    mut replacement: BotCkUsdcTopUpTransfer,
) -> bool {
    CLAIM_INTENTS.with(|m| {
        let mut borrow = m.borrow_mut();
        let map = borrow.as_mut().expect("Claim intent map not initialized");
        let Some(mut intent) = map.get(&vault_id) else { return false; };
        if intent.vault_id != vault_id
            || !matches!(intent.phase, BotClaimIntentPhase::Claimed(_) | BotClaimIntentPhase::SwapStarted(_))
            || intent.ckusdc_top_up_transfer.as_ref() != Some(expected)
            || expected.dispatch_attempt_count != Some(1)
            || expected.block_index.is_some()
            || expected.history_scan.is_some()
            || expected.prior_no_effects.is_none()
            || !matches!(expected.dispatch_observation,
                Some(BotCkUsdcPaymentDispatchObservation::BadFee { .. }
                    | BotCkUsdcPaymentDispatchObservation::InsufficientFunds { .. }))
            || expected.prior_no_effects.as_ref().map_or(0, Vec::len) >= 16
            || replacement.dispatch_attempt_count != Some(0)
            || replacement.block_index.is_some()
            || replacement.dispatch_observation.is_some()
            || replacement.history_scan.is_some()
            || replacement.ledger != expected.ledger
            || replacement.args.from_subaccount != expected.args.from_subaccount
            || replacement.args.to != expected.args.to
            || replacement.args.memo != expected.args.memo
            || replacement.args.amount != expected.args.amount
            || replacement.args.fee.is_none()
            || replacement.prior_no_effects != expected.prior_no_effects
        {
            return false;
        }
        let Some(old_created_at_time) = expected.args.created_at_time else { return false; };
        let Some(new_created_at_time) = replacement.args.created_at_time else { return false; };
        if new_created_at_time <= old_created_at_time {
            return false;
        }
        let prior_fee = expected.args.fee.as_ref().and_then(|fee| fee.0.to_string().parse::<u64>().ok());
        let next_fee = replacement.args.fee.as_ref().and_then(|fee| fee.0.to_string().parse::<u64>().ok());
        if prior_fee.is_none() || next_fee.is_none() {
            return false;
        }
        let mut prior_no_effects = expected.prior_no_effects.clone().unwrap_or_default();
        if prior_no_effects.iter().any(|prior| {
            prior.ledger == replacement.ledger && prior.args == replacement.args
        }) {
            return false;
        }
        prior_no_effects.push(BotCkUsdcPaymentNoEffect {
            ledger: expected.ledger,
            args: expected.args.clone(),
            observation: expected.dispatch_observation.clone().expect("checked typed no-effect"),
        });
        replacement.prior_no_effects = Some(prior_no_effects);
        intent.ckusdc_top_up_transfer = Some(replacement);
        map.insert(vault_id, intent);
        true
    })
}

/// Store the original payment tuple once. A retry may only add its returned
/// block index; it cannot change ledger or any ICRC-1 argument.
pub fn update_ckusdc_payment_transfer(
    vault_id: u64,
    transfer: BotCkUsdcPaymentTransfer,
) -> bool {
    CLAIM_INTENTS.with(|m| {
        let mut borrow = m.borrow_mut();
        let map = borrow.as_mut().expect("Claim intent map not initialized");
        let Some(mut intent) = map.get(&vault_id) else {
            return false;
        };
        if intent.vault_id != vault_id
            || !matches!(intent.phase, BotClaimIntentPhase::Claimed(_) | BotClaimIntentPhase::SwapStarted(_))
        {
            return false;
        }
        if let Some(existing) = &intent.ckusdc_payment_transfer {
            if existing.ledger != transfer.ledger || existing.args != transfer.args {
                return false;
            }
            if existing.prior_no_effects != transfer.prior_no_effects {
                return false;
            }
            if existing.dispatch_attempt_count != transfer.dispatch_attempt_count {
                if existing.dispatch_attempt_count.and_then(|count| count.checked_add(1)) != transfer.dispatch_attempt_count
                    || existing.block_index.is_some()
                    || existing.dispatch_observation.is_some()
                    || existing.history_scan.is_some()
                    || transfer.dispatch_observation.is_some()
                    || transfer.history_scan.is_some()
                    || transfer.block_index.is_some()
                {
                    return false;
                }
            }
            if existing.block_index.is_some() && existing.block_index != transfer.block_index {
                return false;
            }
            // Once dispatch outcome has been observed, callbacks may advance
            // reconciliation but cannot erase or rewrite that evidence.
            if existing.dispatch_observation.is_some()
                && existing.dispatch_observation != transfer.dispatch_observation {
                return false;
            }
            if existing.dispatch_observation.is_none()
                && transfer.dispatch_observation.is_some()
                && (existing.dispatch_attempt_count != transfer.dispatch_attempt_count
                    || existing.block_index.is_some())
            {
                return false;
            }
            match (&existing.history_scan, &transfer.history_scan) {
                (Some(previous), Some(next)) if payment_history_scan_advances(previous, next) => {}
                (Some(previous), None)
                    if transfer.block_index.is_some()
                        && payment_history_scan_found_exactly_one(previous, transfer.block_index.unwrap()) => {}
                (None, Some(next))
                    if existing.block_index.is_none()
                        && transfer.dispatch_observation.is_some()
                        && next.next_index == next.snapshot_log_length
                        && next.candidate_block_index.is_none()
                        && !next.multiple_candidates => {}
                (None, None) => {}
                _ => return false,
            }
        }
        intent.ckusdc_payment_transfer = Some(transfer);
        map.insert(vault_id, intent);
        true
    })
}

/// Replace only a tuple whose very first armed dispatch received a typed
/// no-effect BadFee or InsufficientFunds response. The prior identity and
/// rejection remain in the replacement as a durable tombstone.
pub fn replace_ckusdc_payment_after_first_no_effect(
    vault_id: u64,
    expected: &BotCkUsdcPaymentTransfer,
    mut replacement: BotCkUsdcPaymentTransfer,
) -> bool {
    CLAIM_INTENTS.with(|m| {
        let mut borrow = m.borrow_mut();
        let map = borrow.as_mut().expect("Claim intent map not initialized");
        let Some(mut intent) = map.get(&vault_id) else {
            return false;
        };
        if intent.vault_id != vault_id
            || !matches!(intent.phase, BotClaimIntentPhase::Claimed(_) | BotClaimIntentPhase::SwapStarted(_))
            || intent.ckusdc_payment_transfer.as_ref() != Some(expected)
            || expected.dispatch_attempt_count != Some(1)
            || expected.block_index.is_some()
            || expected.history_scan.is_some()
            || !matches!(expected.dispatch_observation,
                Some(BotCkUsdcPaymentDispatchObservation::BadFee { .. }
                    | BotCkUsdcPaymentDispatchObservation::InsufficientFunds { .. }))
            || expected.prior_no_effects.as_ref().map_or(0, Vec::len) >= 16
            || replacement.dispatch_attempt_count != Some(0)
            || replacement.block_index.is_some()
            || replacement.dispatch_observation.is_some()
            || replacement.history_scan.is_some()
            || replacement.ledger != expected.ledger
            || replacement.args.from_subaccount != expected.args.from_subaccount
            || replacement.args.to != expected.args.to
            || replacement.args.memo != expected.args.memo
            || replacement.args.created_at_time.is_none()
            || replacement.args.created_at_time == expected.args.created_at_time
        {
            return false;
        }
        let old_amount = expected.args.amount.0.to_string().parse::<u64>().ok();
        let old_fee = expected.args.fee.as_ref().and_then(|fee| fee.0.to_string().parse::<u64>().ok());
        let new_amount = replacement.args.amount.0.to_string().parse::<u64>().ok();
        let new_fee = replacement.args.fee.as_ref().and_then(|fee| fee.0.to_string().parse::<u64>().ok());
        if old_amount != new_amount || old_fee.is_none() || new_fee.is_none() {
            return false;
        }
        let mut prior_no_effects = expected.prior_no_effects.clone().unwrap_or_default();
        prior_no_effects.push(BotCkUsdcPaymentNoEffect {
            ledger: expected.ledger,
            args: expected.args.clone(),
            observation: expected.dispatch_observation.clone().expect("checked typed no-effect"),
        });
        replacement.prior_no_effects = Some(prior_no_effects);
        intent.ckusdc_payment_transfer = Some(replacement);
        map.insert(vault_id, intent);
        true
    })
}

/// Record shortfall eligibility only against the exact active swap receipt,
/// before any payment, top-up, or collateral-return tuple has been prepared.
/// Repeated identical writes are idempotent; the marker cannot be rewritten.
pub fn mark_ckusdc_shortfall_eligibility(
    vault_id: u64,
    receipt: &BotClaimReceipt,
    marker: BotCkUsdcShortfallEligibility,
) -> bool {
    if marker.measured_reserved_output_e6 == 0 || marker.minimum_payment_e6 == 0 {
        return false;
    }
    CLAIM_INTENTS.with(|m| {
        let mut borrow = m.borrow_mut();
        let map = borrow.as_mut().expect("Claim intent map not initialized");
        let Some(mut intent) = map.get(&vault_id) else {
            return false;
        };
        let receipt_matches = matches!(
            &intent.phase,
            BotClaimIntentPhase::SwapStarted(saved) if saved == receipt
        );
        if intent.vault_id != vault_id
            || !receipt_matches
            || intent.return_transfer.is_some()
            || intent.ckusdc_top_up_transfer.is_some()
            || intent.ckusdc_payment_transfer.is_some()
        {
            return false;
        }
        if let Some(existing) = &intent.shortfall_eligibility {
            return existing == &marker;
        }
        intent.shortfall_eligibility = Some(marker);
        map.insert(vault_id, intent);
        true
    })
}

fn payment_history_scan_advances(
    previous: &BotCkUsdcPaymentHistoryScan,
    next: &BotCkUsdcPaymentHistoryScan,
) -> bool {
    if previous.snapshot_log_length != next.snapshot_log_length {
        return previous.next_index == 0
            && previous.candidate_block_index.is_none()
            && !previous.multiple_candidates
            && next.snapshot_log_length >= previous.snapshot_log_length
            && next.next_index == next.snapshot_log_length
            && next.candidate_block_index.is_none()
            && !next.multiple_candidates;
    }
    let scanned = previous.next_index.saturating_sub(next.next_index);
    if previous.next_index == 0
        || scanned == 0
        || scanned > CKUSDC_PAYMENT_HISTORY_PAGE_SIZE
        || next.next_index > next.snapshot_log_length
        || (previous.multiple_candidates && !next.multiple_candidates)
    {
        return false;
    }
    match previous.candidate_block_index {
        Some(_) => false,
        None => next.candidate_block_index.map_or(true, |candidate| {
            candidate >= next.next_index && candidate < previous.next_index
        }),
    }
}

fn payment_history_scan_found_exactly_one(
    scan: &BotCkUsdcPaymentHistoryScan,
    block_index: u64,
) -> bool {
    !scan.multiple_candidates
        && scan.candidate_block_index == Some(block_index)
}

fn same_ckusdc_top_up_identity(
    existing: &BotCkUsdcTopUpTransfer,
    proposed: &BotCkUsdcTopUpTransfer,
) -> bool {
    existing.ledger == proposed.ledger && existing.args == proposed.args
}

pub fn has_claim_intents() -> bool {
    CLAIM_INTENTS.with(|m| {
        claim_intent_map_has_any(
            m.borrow()
                .as_ref()
                .expect("Claim intent map not initialized"),
        )
    })
}

fn claim_intent_map_has_any(
    map: &StableBTreeMap<u64, BotClaimIntent, memory::Mem>,
) -> bool {
    map.iter().next().is_some()
}

#[cfg(test)]
mod claim_intent_tests {
    use super::*;
    use ic_stable_structures::{memory_manager::MemoryManager, DefaultMemoryImpl};

    #[test]
    fn new_claim_phases_round_trip_receipt_and_swap_start_marker() {
        let receipt = BotClaimReceipt {
            collateral_amount: 400,
            debt_covered: 300,
            collateral_price_e8s: 25_000,
            claim_timestamp: 123_456,
            payment_memo: b"claim-proof".to_vec(),
            claim_transfer: None,
        };
        for phase in [
            BotClaimIntentPhase::ClaimRequested,
            BotClaimIntentPhase::Claimed(receipt.clone()),
            BotClaimIntentPhase::SwapStarted(receipt.clone()),
        ] {
            let bytes = candid::encode_one(&phase).unwrap();
            let restored: BotClaimIntentPhase = candid::decode_one(&bytes).unwrap();
            assert_eq!(restored, phase);
        }
    }

    #[test]
    fn shortfall_eligibility_is_generation_bound_immutable_and_pre_dispatch_only() {
        memory::init_memory_manager();
        init_history();
        let receipt = BotClaimReceipt {
            collateral_amount: 400,
            debt_covered: 300,
            collateral_price_e8s: 25_000,
            claim_timestamp: 123_456,
            payment_memo: b"claim-proof".to_vec(),
            claim_transfer: None,
        };
        let marker = BotCkUsdcShortfallEligibility {
            ledger: Principal::from_slice(&[0x44]),
            measured_reserved_output_e6: 297,
            minimum_payment_e6: 300,
        };
        put_claim_intent(BotClaimIntent {
            vault_id: 81,
            request_id: 910,
            claim_call_count: Some(1),
            backend_principal: Some(Principal::from_slice(&[0x45])),
            return_transfer: None,
            ckusdc_top_up_transfer: None,
            ckusdc_payment_transfer: None,
            shortfall_eligibility: None,
            icp_pool_input_fee_e8s: None,
            phase: BotClaimIntentPhase::SwapStarted(receipt.clone()),
        });

        assert!(mark_ckusdc_shortfall_eligibility(81, &receipt, marker.clone()));
        assert!(mark_ckusdc_shortfall_eligibility(81, &receipt, marker.clone()));
        let mut rewritten = marker.clone();
        rewritten.measured_reserved_output_e6 = 298;
        assert!(!mark_ckusdc_shortfall_eligibility(81, &receipt, rewritten));
        let mut stale_receipt = receipt.clone();
        stale_receipt.claim_timestamp += 1;
        assert!(!mark_ckusdc_shortfall_eligibility(81, &stale_receipt, marker.clone()));
        assert_eq!(get_claim_intent(81).unwrap().shortfall_eligibility, Some(marker.clone()));
        let mut zero_output = marker.clone();
        zero_output.measured_reserved_output_e6 = 0;
        assert!(!mark_ckusdc_shortfall_eligibility(81, &receipt, zero_output));

        let mut paid = get_claim_intent(81).unwrap();
        paid.shortfall_eligibility = None;
        paid.ckusdc_payment_transfer = Some(BotCkUsdcPaymentTransfer {
            ledger: marker.ledger.clone(),
            args: TransferArg {
                from_subaccount: None,
                to: Account { owner: Principal::from_slice(&[0x45]), subaccount: None },
                amount: candid::Nat::from(300u64),
                fee: None,
                memo: None,
                created_at_time: Some(1),
            },
            block_index: None,
            dispatch_attempt_count: Some(0),
            prior_no_effects: Some(Vec::new()),
            dispatch_observation: None,
            history_scan: None,
        });
        put_claim_intent(paid);
        assert!(!mark_ckusdc_shortfall_eligibility(81, &receipt, marker));
    }

    #[test]
    fn pool_input_fee_is_pinned_with_swap_start_and_immutable() {
        memory::init_memory_manager();
        init_history();
        let receipt = BotClaimReceipt {
            collateral_amount: 400,
            debt_covered: 300,
            collateral_price_e8s: 25_000,
            claim_timestamp: 123_456,
            payment_memo: b"claim-proof".to_vec(),
            claim_transfer: None,
        };
        put_claim_intent(BotClaimIntent {
            vault_id: 82,
            request_id: 911,
            claim_call_count: Some(1),
            backend_principal: Some(Principal::from_slice(&[0x45])),
            return_transfer: None,
            ckusdc_top_up_transfer: None,
            ckusdc_payment_transfer: None,
            shortfall_eligibility: None,
            icp_pool_input_fee_e8s: None,
            phase: BotClaimIntentPhase::Claimed(receipt.clone()),
        });

        assert!(mark_claim_swap_started(82, 911, &receipt, 10_000));
        assert_eq!(get_claim_intent(82).unwrap().icp_pool_input_fee_e8s, Some(10_000));
        assert!(!mark_claim_swap_started(82, 911, &receipt, 20_000));
        assert_eq!(get_claim_intent(82).unwrap().icp_pool_input_fee_e8s, Some(10_000));
    }

    #[test]
    fn intent_snapshot_preserves_exact_request_and_acquired_payment_binding() {
        assert_ne!(memory::MEM_ID_CLAIM_INTENTS, memory::MEM_ID_CONFIG);
        assert_ne!(memory::MEM_ID_CLAIM_INTENTS, memory::MEM_ID_HISTORY);
        assert_ne!(memory::MEM_ID_CLAIM_INTENTS, memory::MEM_ID_NEXT_ID);

        let acquired = BotClaimIntent {
            vault_id: 73,
            request_id: 9001,
            claim_call_count: Some(1),
            backend_principal: Some(Principal::from_slice(&[0x41])),
            return_transfer: None,
            ckusdc_top_up_transfer: None,
            ckusdc_payment_transfer: None,
            shortfall_eligibility: None,
            icp_pool_input_fee_e8s: None,
            phase: BotClaimIntentPhase::Acquired {
                claim_timestamp: 123_456,
                payment_memo: b"claim-123456".to_vec(),
            },
        };
        let bytes = candid::encode_one(&acquired).unwrap();
        let restored: BotClaimIntent = candid::decode_one(&bytes).unwrap();
        assert_eq!(restored, acquired);
        assert!(matches!(
            restored.phase,
            BotClaimIntentPhase::Acquired { claim_timestamp: 123_456, ref payment_memo }
                if payment_memo == b"claim-123456"
        ));
    }

    #[test]
    fn claim_intent_survives_managed_memory_reinitialization() {
        let raw = DefaultMemoryImpl::default();
        let expected = BotClaimIntent {
            vault_id: 71,
            request_id: 902,
            claim_call_count: Some(0),
            backend_principal: Some(Principal::from_slice(&[0x41])),
            return_transfer: None,
            ckusdc_top_up_transfer: None,
            ckusdc_payment_transfer: None,
            shortfall_eligibility: None,
            icp_pool_input_fee_e8s: None,
            phase: BotClaimIntentPhase::AwaitingReceipt,
        };
        let return_memo = b"BOTRET02".to_vec();
        let second = BotClaimIntent {
            vault_id: 72,
            request_id: 903,
            claim_call_count: Some(1),
            backend_principal: Some(Principal::from_slice(&[0x41])),
            return_transfer: Some(BotClaimReturnTransfer {
                ledger: Principal::from_slice(&[0x42]),
                from: Account { owner: Principal::from_slice(&[0x43]), subaccount: None },
                collateral_amount_e8s: 110,
                ledger_fee_e8s: 10,
                claim_timestamp: 124,
                args: TransferArg {
                    from_subaccount: None,
                    to: Account {
                        owner: Principal::from_slice(&[0x41]),
                        subaccount: Some([7; 32]),
                    },
                    amount: candid::Nat::from(120u64),
                    fee: Some(candid::Nat::from(10u64)),
                    memo: Some(icrc_ledger_types::icrc1::transfer::Memo::from(return_memo)),
                    created_at_time: Some(555),
                },
                block_index: Some(12),
                dispatch_attempt_count: Some(1),
                prior_no_effects: Some(Vec::new()),
                dispatch_observation: None,
            }),
            ckusdc_top_up_transfer: None,
            ckusdc_payment_transfer: None,
            shortfall_eligibility: None,
            icp_pool_input_fee_e8s: None,
            phase: BotClaimIntentPhase::Acquired {
                claim_timestamp: 124,
                payment_memo: b"claim-124".to_vec(),
            },
        };
        let top_up_memo = b"RUMI-BOT-LIQ:claim-124".to_vec();
        let third = BotClaimIntent {
            vault_id: 74,
            request_id: 905,
            claim_call_count: Some(1),
            backend_principal: Some(Principal::from_slice(&[0x41])),
            return_transfer: None,
            ckusdc_top_up_transfer: Some(BotCkUsdcTopUpTransfer {
                ledger: Principal::from_slice(&[0x44]),
                args: TransferArg {
                    from_subaccount: None,
                    to: Account { owner: Principal::from_slice(&[0x45]), subaccount: None },
                    amount: candid::Nat::from(23u64),
                    fee: Some(candid::Nat::from(17u64)),
                    memo: Some(icrc_ledger_types::icrc1::transfer::Memo::from(top_up_memo)),
                    created_at_time: Some(987_654),
                },
                block_index: None,
                dispatch_attempt_count: Some(0),
                prior_no_effects: Some(Vec::new()),
                dispatch_observation: None,
                history_scan: None,
            }),
            ckusdc_payment_transfer: Some(BotCkUsdcPaymentTransfer {
                ledger: Principal::from_slice(&[0x46]),
                args: TransferArg {
                    from_subaccount: None,
                    to: Account { owner: Principal::from_slice(&[0x45]), subaccount: None },
                    amount: candid::Nat::from(13u64),
                    fee: Some(candid::Nat::from(10u64)),
                    memo: Some(icrc_ledger_types::icrc1::transfer::Memo::from(b"RUMI-BOT-LIQ:claim-124".to_vec())),
                    created_at_time: Some(987_655),
                },
                block_index: None,
                dispatch_attempt_count: Some(0),
                prior_no_effects: Some(Vec::new()),
                dispatch_observation: Some(BotCkUsdcPaymentDispatchObservation::TooOld),
                history_scan: None,
            }),
            shortfall_eligibility: None,
            icp_pool_input_fee_e8s: None,
            phase: BotClaimIntentPhase::SwapStarted(BotClaimReceipt {
                collateral_amount: 200,
                debt_covered: 123,
                collateral_price_e8s: 456,
                claim_timestamp: 124,
                payment_memo: b"RUMI-BOT-LIQ:claim-124".to_vec(),
                claim_transfer: None,
            }),
        };
        {
            let manager = MemoryManager::init(raw.clone());
            let mut map: StableBTreeMap<u64, BotClaimIntent, memory::Mem> =
                StableBTreeMap::init(manager.get(memory::MEM_ID_CLAIM_INTENTS));
            map.insert(expected.vault_id, expected.clone());
            map.insert(second.vault_id, second.clone());
            map.insert(third.vault_id, third.clone());
        }
        // A canister upgrade reconstructs the manager and stable map from the
        // same stable memory. The exact recovery identity must survive it.
        let manager = MemoryManager::init(raw);
        let mut map: StableBTreeMap<u64, BotClaimIntent, memory::Mem> =
            StableBTreeMap::init(manager.get(memory::MEM_ID_CLAIM_INTENTS));
        assert_eq!(map.get(&expected.vault_id), Some(expected));
        assert_eq!(map.get(&second.vault_id), Some(second));
        assert_eq!(map.get(&third.vault_id), Some(third.clone()));
        let restored_top_up = map.get(&74).unwrap().ckusdc_top_up_transfer.unwrap();
        assert_eq!(restored_top_up.args.created_at_time, Some(987_654));
        assert_eq!(restored_top_up.args.amount, candid::Nat::from(23u64));
        assert_eq!(restored_top_up.args.fee, Some(candid::Nat::from(17u64)));
        assert_eq!(restored_top_up.args.memo.unwrap().0.as_ref(), b"RUMI-BOT-LIQ:claim-124");
        assert_eq!(restored_top_up.block_index, None, "an unresolved transfer must remain pinned across upgrade");
        let restored_payment = map.get(&74).unwrap().ckusdc_payment_transfer.unwrap();
        assert_eq!(restored_payment.ledger, Principal::from_slice(&[0x46]));
        assert_eq!(restored_payment.args.amount, candid::Nat::from(13u64));
        assert_eq!(restored_payment.args.fee, Some(candid::Nat::from(10u64)));
        assert_eq!(restored_payment.args.created_at_time, Some(987_655));
        assert_eq!(restored_payment.args.memo.unwrap().0.as_ref(), b"RUMI-BOT-LIQ:claim-124");
        assert_eq!(restored_payment.block_index, None, "an ambiguous original transfer must keep its exact tuple across upgrade");
        assert_eq!(restored_payment.dispatch_observation, Some(BotCkUsdcPaymentDispatchObservation::TooOld), "typed TooOld evidence must survive upgrade");
        assert!(claim_intent_map_has_any(&map));
        map.remove(&71);
        assert!(claim_intent_map_has_any(&map), "one remaining intent must keep backend retargeting blocked");
        map.remove(&72);
        map.remove(&74);
        assert!(!claim_intent_map_has_any(&map), "retargeting is allowed only after all intents clear");
    }

    #[test]
    fn old_claim_intent_without_backend_identity_decodes_unbound() {
        #[derive(CandidType)]
        enum OldPhase {
            AwaitingReceipt,
            Acquired { claim_timestamp: u64, payment_memo: Vec<u8> },
        }
        #[derive(CandidType)]
        struct OldIntent {
            vault_id: u64,
            request_id: u64,
            phase: OldPhase,
        }
        for old_phase in [
            OldPhase::AwaitingReceipt,
            OldPhase::Acquired {
                claim_timestamp: 124,
                payment_memo: b"legacy-claim".to_vec(),
            },
        ] {
            let old = OldIntent {
                vault_id: 71,
                request_id: 902,
                phase: old_phase,
            };
            let bytes = candid::encode_one(old).unwrap();
            let restored: BotClaimIntent = candid::decode_one(&bytes).unwrap();
            assert_eq!(restored.backend_principal, None);
            assert_eq!(restored.return_transfer, None);
            assert_eq!(restored.ckusdc_payment_transfer, None);
            assert_eq!(restored.shortfall_eligibility, None);
            assert_eq!(restored.icp_pool_input_fee_e8s, None);
            assert_eq!(restored.claim_call_count, None);
        }
    }

    #[test]
    fn legacy_payment_tuple_migrates_with_no_dispatch_observation() {
        #[derive(CandidType)]
        struct LegacyPayment {
            ledger: Principal,
            args: TransferArg,
            block_index: Option<u64>,
            history_scan: Option<BotCkUsdcPaymentHistoryScan>,
        }
        let legacy = LegacyPayment {
            ledger: Principal::from_slice(&[0x46]),
            args: TransferArg {
                from_subaccount: None,
                to: Account { owner: Principal::from_slice(&[0x45]), subaccount: None },
                amount: candid::Nat::from(13u64),
                fee: Some(candid::Nat::from(10u64)),
                memo: Some(icrc_ledger_types::icrc1::transfer::Memo::from(b"legacy".to_vec())),
                created_at_time: Some(987_655),
            },
            block_index: None,
            history_scan: None,
        };
        let restored: BotCkUsdcPaymentTransfer = candid::decode_one(
            &candid::encode_one(legacy).unwrap(),
        ).unwrap();
        assert_eq!(restored.dispatch_observation, None);
        assert_eq!(restored.history_scan, None);
    }

    #[test]
    fn legacy_return_tuple_migrates_as_unknown_and_cannot_authorize_rotation() {
        #[derive(CandidType)]
        struct LegacyReturn {
            ledger: Principal,
            from: Account,
            collateral_amount_e8s: u64,
            ledger_fee_e8s: u64,
            claim_timestamp: u64,
            args: TransferArg,
            block_index: Option<u64>,
        }
        let legacy = LegacyReturn {
            ledger: Principal::from_slice(&[0x47]),
            from: Account { owner: Principal::from_slice(&[0x46]), subaccount: None },
            collateral_amount_e8s: 1_000,
            ledger_fee_e8s: 10,
            claim_timestamp: 500,
            args: TransferArg {
                from_subaccount: None,
                to: Account { owner: Principal::from_slice(&[0x45]), subaccount: Some([8; 32]) },
                amount: candid::Nat::from(1_010u64),
                fee: Some(candid::Nat::from(10u64)),
                memo: Some(b"legacy-return".to_vec().into()),
                created_at_time: Some(700),
            },
            block_index: None,
        };
        let restored: BotClaimReturnTransfer = candid::decode_one(
            &candid::encode_one(legacy).unwrap(),
        ).unwrap();
        assert_eq!(restored.dispatch_attempt_count, None);
        assert_eq!(restored.prior_no_effects, None);
        assert_eq!(restored.dispatch_observation, None);
    }

    #[test]
    fn legacy_top_up_tuple_decodes_with_reconciliation_fields_empty() {
        #[derive(CandidType)]
        struct LegacyTopUp {
            ledger: Principal,
            args: TransferArg,
            block_index: Option<u64>,
        }
        let legacy = LegacyTopUp {
            ledger: Principal::from_slice(&[0x44]),
            args: TransferArg {
                from_subaccount: None,
                to: Account { owner: Principal::from_slice(&[0x45]), subaccount: None },
                amount: candid::Nat::from(23u64),
                fee: Some(candid::Nat::from(17u64)),
                memo: Some(icrc_ledger_types::icrc1::transfer::Memo::from(b"legacy-top-up".to_vec())),
                created_at_time: Some(987_654),
            },
            block_index: None,
        };
        let restored: BotCkUsdcTopUpTransfer = candid::decode_one(
            &candid::encode_one(legacy).unwrap(),
        ).unwrap();
        assert_eq!(restored.dispatch_attempt_count, None);
        assert_eq!(restored.prior_no_effects, None);
        assert_eq!(restored.dispatch_observation, None);
        assert_eq!(restored.history_scan, None);
    }

    #[test]
    fn top_up_history_cursor_advances_only_within_fixed_bounded_snapshot() {
        let initial = BotCkUsdcPaymentHistoryScan {
            snapshot_log_length: 100,
            next_index: 100,
            candidate_block_index: None,
            multiple_candidates: false,
        };
        let page = BotCkUsdcPaymentHistoryScan { next_index: 92, ..initial.clone() };
        assert!(payment_history_scan_advances(&initial, &page));
        let candidate = BotCkUsdcPaymentHistoryScan {
            next_index: 96,
            candidate_block_index: Some(96),
            ..initial.clone()
        };
        assert!(payment_history_scan_advances(&initial, &candidate));
        let skipped_too_far = BotCkUsdcPaymentHistoryScan { next_index: 91, ..initial.clone() };
        assert!(!payment_history_scan_advances(&initial, &skipped_too_far));
        let rewritten_snapshot = BotCkUsdcPaymentHistoryScan { snapshot_log_length: 101, next_index: 101, ..initial.clone() };
        assert!(!payment_history_scan_advances(&initial, &rewritten_snapshot));
    }

    #[test]
    fn observed_dispatch_classification_is_immutable_during_scan_progress() {
        memory::init_memory_manager();
        init_history();
        let backend = Principal::from_slice(&[0x45]);
        put_claim_intent(BotClaimIntent {
            vault_id: 76, request_id: 907, claim_call_count: Some(1),
            backend_principal: Some(backend), return_transfer: None,
            ckusdc_top_up_transfer: None, ckusdc_payment_transfer: None,
            shortfall_eligibility: None,
            icp_pool_input_fee_e8s: None,
            phase: BotClaimIntentPhase::SwapStarted(BotClaimReceipt {
                collateral_amount: 200, debt_covered: 100_000, collateral_price_e8s: 456,
                claim_timestamp: 124, payment_memo: b"claim-memo".to_vec(), claim_transfer: None,
            }),
        });
        let mut transfer = BotCkUsdcPaymentTransfer {
            ledger: Principal::from_slice(&[0x44]),
            args: TransferArg {
                from_subaccount: None, to: Account { owner: backend, subaccount: None },
                amount: candid::Nat::from(990u64), fee: Some(candid::Nat::from(10u64)),
                memo: Some(icrc_ledger_types::icrc1::transfer::Memo::from(b"claim-memo".to_vec())),
                created_at_time: Some(555),
            },
            block_index: None, dispatch_attempt_count: Some(0), prior_no_effects: Some(Vec::new()),
            dispatch_observation: None, history_scan: None,
        };
        assert!(update_ckusdc_payment_transfer(76, transfer.clone()));
        transfer.dispatch_attempt_count = Some(1);
        assert!(update_ckusdc_payment_transfer(76, transfer.clone()));
        transfer.dispatch_observation = Some(BotCkUsdcPaymentDispatchObservation::TooOld);
        assert!(update_ckusdc_payment_transfer(76, transfer.clone()));
        transfer.history_scan = Some(BotCkUsdcPaymentHistoryScan {
            snapshot_log_length: 10, next_index: 10, candidate_block_index: None,
            multiple_candidates: false,
        });
        assert!(update_ckusdc_payment_transfer(76, transfer.clone()));
        let mut stale = transfer.clone();
        stale.dispatch_observation = None;
        assert!(!update_ckusdc_payment_transfer(76, stale));
        let mut rewritten = transfer.clone();
        rewritten.dispatch_observation = Some(BotCkUsdcPaymentDispatchObservation::Ambiguous);
        assert!(!update_ckusdc_payment_transfer(76, rewritten));
        assert_eq!(get_claim_intent(76).unwrap().ckusdc_payment_transfer.unwrap().dispatch_observation,
            Some(BotCkUsdcPaymentDispatchObservation::TooOld));
    }

    #[test]
    fn top_up_identity_is_immutable_across_ambiguous_reply_retry() {
        let mut original = BotCkUsdcTopUpTransfer {
            ledger: Principal::from_slice(&[0x44]),
            args: TransferArg {
                from_subaccount: None,
                to: Account { owner: Principal::from_slice(&[0x45]), subaccount: None },
                amount: candid::Nat::from(23u64),
                fee: Some(candid::Nat::from(17u64)),
                memo: Some(icrc_ledger_types::icrc1::transfer::Memo::from(b"claim-memo".to_vec())),
                created_at_time: Some(987_654),
            },
            block_index: None,
            dispatch_attempt_count: None,
            prior_no_effects: None,
            dispatch_observation: None,
            history_scan: None,
        };
        // A lost response leaves this exact journal intact; a retry can only
        // change the evidence field after the ledger returns a block.
        let retry = original.clone();
        assert!(same_ckusdc_top_up_identity(&original, &retry));
        let mut altered = retry.clone();
        altered.args.created_at_time = Some(987_655);
        assert!(!same_ckusdc_top_up_identity(&original, &altered));
        altered = retry.clone();
        altered.args.memo = Some(icrc_ledger_types::icrc1::transfer::Memo::from(b"other-memo".to_vec()));
        assert!(!same_ckusdc_top_up_identity(&original, &altered));
        original.block_index = Some(456);
        assert!(same_ckusdc_top_up_identity(&original, &retry));
    }

    #[test]
    fn first_typed_top_up_no_effect_allows_only_exact_bounded_replacement() {
        memory::init_memory_manager();
        init_history();
        let vault_id = 78;
        let backend = Principal::from_slice(&[0x45]);
        let ledger = Principal::from_slice(&[0x44]);
        let memo = b"claim-memo".to_vec();
        put_claim_intent(BotClaimIntent {
            vault_id,
            request_id: 909,
            claim_call_count: Some(1),
            backend_principal: Some(backend),
            return_transfer: None,
            ckusdc_top_up_transfer: None,
            ckusdc_payment_transfer: None,
            shortfall_eligibility: None,
            icp_pool_input_fee_e8s: None,
            phase: BotClaimIntentPhase::Claimed(BotClaimReceipt {
                collateral_amount: 200,
                debt_covered: 100_000,
                collateral_price_e8s: 456,
                claim_timestamp: 124,
                payment_memo: memo.clone(),
                claim_transfer: None,
            }),
        });
        let mut first = BotCkUsdcTopUpTransfer {
            ledger,
            args: TransferArg {
                from_subaccount: Some([7; 32]),
                to: Account { owner: backend, subaccount: None },
                amount: candid::Nat::from(23u64),
                fee: Some(candid::Nat::from(17u64)),
                memo: Some(memo.clone().into()),
                created_at_time: Some(555),
            },
            block_index: None,
            dispatch_attempt_count: Some(0),
            prior_no_effects: Some(Vec::new()),
            dispatch_observation: None,
            history_scan: None,
        };
        assert!(update_ckusdc_top_up_transfer(vault_id, first.clone()));
        first.dispatch_attempt_count = Some(1);
        assert!(update_ckusdc_top_up_transfer(vault_id, first.clone()));
        first.dispatch_observation = Some(BotCkUsdcPaymentDispatchObservation::BadFee { expected_fee: 20 });
        assert!(update_ckusdc_top_up_transfer(vault_id, first.clone()));

        let mut replacement = first.clone();
        replacement.args.fee = Some(candid::Nat::from(20u64));
        replacement.args.created_at_time = Some(556);
        replacement.dispatch_attempt_count = Some(0);
        replacement.prior_no_effects = Some(Vec::new());
        replacement.dispatch_observation = None;
        replacement.history_scan = None;
        assert!(replace_ckusdc_top_up_after_first_no_effect(vault_id, &first, replacement.clone()));
        let saved = get_claim_intent(vault_id).unwrap().ckusdc_top_up_transfer.unwrap();
        assert_eq!(saved.args.amount, candid::Nat::from(23u64));
        assert_eq!(saved.args.fee, Some(candid::Nat::from(20u64)));
        assert_eq!(saved.args.created_at_time, Some(556));
        let tombstones = saved.prior_no_effects.as_ref().unwrap();
        assert_eq!(tombstones.len(), 1);
        assert_eq!(tombstones[0].ledger, ledger);
        assert_eq!(tombstones[0].args, first.args);
        assert_eq!(tombstones[0].observation,
            BotCkUsdcPaymentDispatchObservation::BadFee { expected_fee: 20 });
        assert!(!replace_ckusdc_top_up_after_first_no_effect(vault_id, &first, replacement.clone()));

        let mut legacy = first.clone();
        legacy.dispatch_attempt_count = None;
        let mut legacy_intent = get_claim_intent(vault_id).unwrap();
        legacy_intent.vault_id = vault_id + 1;
        legacy_intent.ckusdc_top_up_transfer = Some(legacy.clone());
        put_claim_intent(legacy_intent);
        assert!(!replace_ckusdc_top_up_after_first_no_effect(vault_id + 1, &legacy, replacement.clone()));

        let tombstone = BotCkUsdcPaymentNoEffect {
            ledger,
            args: first.args.clone(),
            observation: BotCkUsdcPaymentDispatchObservation::BadFee { expected_fee: 20 },
        };
        let mut saturated = first.clone();
        saturated.prior_no_effects = Some(vec![tombstone.clone(); 16]);
        let mut saturated_intent = get_claim_intent(vault_id).unwrap();
        saturated_intent.vault_id = vault_id + 2;
        saturated_intent.ckusdc_top_up_transfer = Some(saturated.clone());
        put_claim_intent(saturated_intent);
        let mut saturated_replacement = replacement.clone();
        saturated_replacement.prior_no_effects = Some(vec![tombstone; 16]);
        assert!(!replace_ckusdc_top_up_after_first_no_effect(
            vault_id + 2, &saturated, saturated_replacement,
        ));

        let mut wrong_amount = saved.clone();
        wrong_amount.args.amount = candid::Nat::from(24u64);
        assert!(!replace_ckusdc_top_up_after_first_no_effect(vault_id, &saved, wrong_amount));
        let mut ambiguous = first.clone();
        ambiguous.dispatch_observation = Some(BotCkUsdcPaymentDispatchObservation::Ambiguous);
        assert!(!replace_ckusdc_top_up_after_first_no_effect(vault_id, &ambiguous, replacement));
    }

    #[test]
    fn claim_return_bad_fee_replacement_is_durable_and_ambiguous_attempt_cannot_rotate() {
        memory::init_memory_manager();
        init_history();
        let vault_id = 188;
        let backend = Principal::from_slice(&[0x45]);
        let bot = Principal::from_slice(&[0x46]);
        let ledger = Principal::from_slice(&[0x47]);
        let subaccount = [8; 32];
        let memo: icrc_ledger_types::icrc1::transfer::Memo = b"BOTRET02-generation".to_vec().into();
        let intent = |return_transfer| BotClaimIntent {
            vault_id,
            request_id: 1908,
            claim_call_count: Some(1),
            backend_principal: Some(backend),
            return_transfer,
            ckusdc_top_up_transfer: None,
            ckusdc_payment_transfer: None,
            shortfall_eligibility: None,
            icp_pool_input_fee_e8s: None,
            phase: BotClaimIntentPhase::Claimed(BotClaimReceipt {
                collateral_amount: 1_000,
                debt_covered: 900,
                collateral_price_e8s: 20_000,
                claim_timestamp: 500,
                payment_memo: b"claim-payment".to_vec(),
                claim_transfer: None,
            }),
        };
        let tuple = |fee: u64, amount: u64, created_at_time: u64| BotClaimReturnTransfer {
            ledger,
            from: Account { owner: bot, subaccount: None },
            collateral_amount_e8s: 1_000,
            ledger_fee_e8s: fee,
            claim_timestamp: 500,
            args: TransferArg {
                from_subaccount: None,
                to: Account { owner: backend, subaccount: Some(subaccount) },
                amount: candid::Nat::from(amount),
                fee: Some(candid::Nat::from(fee)),
                memo: Some(memo.clone()),
                created_at_time: Some(created_at_time),
            },
            block_index: None,
            dispatch_attempt_count: Some(1),
            prior_no_effects: Some(Vec::new()),
            dispatch_observation: None,
        };
        let mut rejected = tuple(10, 1_010, 700);
        rejected.dispatch_observation = Some(BotCkUsdcPaymentDispatchObservation::BadFee { expected_fee: 20 });
        put_claim_intent(intent(Some(rejected.clone())));
        let mut replacement = tuple(20, 1_020, 701);
        replacement.dispatch_attempt_count = Some(0);
        replacement.dispatch_observation = None;
        assert!(replace_claim_return_after_first_bad_fee(vault_id, &rejected, replacement.clone()));
        let mut saved = get_claim_intent(vault_id).unwrap().return_transfer.unwrap();
        assert_eq!(saved.args.amount, candid::Nat::from(1_020u64));
        assert_eq!(saved.args.fee, Some(candid::Nat::from(20u64)));
        assert_eq!(saved.prior_no_effects.as_ref().unwrap().len(), 1);
        assert_eq!(saved.prior_no_effects.as_ref().unwrap()[0].args, rejected.args);
        saved.dispatch_attempt_count = Some(1);
        assert!(update_claim_return_transfer(vault_id, saved.clone()));
        saved.block_index = Some(88);
        assert!(update_claim_return_transfer(vault_id, saved));

        let mut ambiguous = tuple(10, 1_010, 800);
        ambiguous.dispatch_observation = Some(BotCkUsdcPaymentDispatchObservation::Ambiguous);
        put_claim_intent(intent(Some(ambiguous.clone())));
        assert!(!replace_claim_return_after_first_bad_fee(vault_id, &ambiguous, replacement));
        let mut exact_retry = ambiguous.clone();
        exact_retry.dispatch_attempt_count = Some(2);
        assert!(update_claim_return_transfer(vault_id, exact_retry.clone()));
        let mut ambiguity_cannot_be_overwritten = exact_retry.clone();
        ambiguity_cannot_be_overwritten.dispatch_observation = Some(
            BotCkUsdcPaymentDispatchObservation::BadFee { expected_fee: 20 },
        );
        assert!(!update_claim_return_transfer(vault_id, ambiguity_cannot_be_overwritten));
        let saved_retry = get_claim_intent(vault_id).unwrap().return_transfer.unwrap();
        assert_eq!(saved_retry.args, ambiguous.args);
        assert_eq!(saved_retry.dispatch_observation, ambiguous.dispatch_observation);
    }

    #[test]
    fn top_up_reconciliation_state_is_monotonic_and_tuple_bound() {
        memory::init_memory_manager();
        init_history();
        let backend = Principal::from_slice(&[0x45]);
        put_claim_intent(BotClaimIntent {
            vault_id: 77,
            request_id: 908,
            claim_call_count: Some(1),
            backend_principal: Some(backend),
            return_transfer: None,
            ckusdc_top_up_transfer: None,
            ckusdc_payment_transfer: None,
            shortfall_eligibility: None,
            icp_pool_input_fee_e8s: None,
            phase: BotClaimIntentPhase::Claimed(BotClaimReceipt {
                collateral_amount: 200,
                debt_covered: 100_000,
                collateral_price_e8s: 456,
                claim_timestamp: 124,
                payment_memo: b"claim-memo".to_vec(),
                claim_transfer: None,
            }),
        });
        let mut transfer = BotCkUsdcTopUpTransfer {
            ledger: Principal::from_slice(&[0x44]),
            args: TransferArg {
                from_subaccount: None,
                to: Account { owner: backend, subaccount: None },
                amount: candid::Nat::from(23u64),
                fee: Some(candid::Nat::from(17u64)),
                memo: Some(icrc_ledger_types::icrc1::transfer::Memo::from(b"claim-memo".to_vec())),
                created_at_time: Some(987_654),
            },
            block_index: None,
            dispatch_attempt_count: Some(0),
            prior_no_effects: Some(Vec::new()),
            dispatch_observation: None,
            history_scan: None,
        };
        assert!(update_ckusdc_top_up_transfer(77, transfer.clone()));
        transfer.dispatch_observation = Some(BotCkUsdcPaymentDispatchObservation::TooOld);
        assert!(update_ckusdc_top_up_transfer(77, transfer.clone()));
        transfer.history_scan = Some(BotCkUsdcPaymentHistoryScan {
            snapshot_log_length: 10,
            next_index: 10,
            candidate_block_index: None,
            multiple_candidates: false,
        });
        assert!(update_ckusdc_top_up_transfer(77, transfer.clone()));
        let mut progressed = transfer.clone();
        progressed.history_scan.as_mut().unwrap().next_index = 2;
        assert!(update_ckusdc_top_up_transfer(77, progressed.clone()));
        let mut cleared = progressed.clone();
        cleared.dispatch_observation = None;
        assert!(!update_ckusdc_top_up_transfer(77, cleared));
        let mut rewritten = progressed;
        rewritten.args.created_at_time = Some(987_655);
        assert!(!update_ckusdc_top_up_transfer(77, rewritten));
    }

    #[test]
    fn original_payment_journal_allows_only_block_index_completion() {
        memory::init_memory_manager();
        init_history();
        let ledger = Principal::from_slice(&[0x44]);
        let backend = Principal::from_slice(&[0x45]);
        let receipt = BotClaimReceipt {
            collateral_amount: 200,
            debt_covered: 100_000,
            collateral_price_e8s: 456,
            claim_timestamp: 124,
            payment_memo: b"claim-memo".to_vec(),
            claim_transfer: None,
        };
        put_claim_intent(BotClaimIntent {
            vault_id: 74,
            request_id: 905,
            claim_call_count: Some(1),
            backend_principal: Some(backend),
            return_transfer: None,
            ckusdc_top_up_transfer: None,
            ckusdc_payment_transfer: None,
            shortfall_eligibility: None,
            icp_pool_input_fee_e8s: None,
            phase: BotClaimIntentPhase::SwapStarted(receipt),
        });
        let transfer = BotCkUsdcPaymentTransfer {
            ledger,
            args: TransferArg {
                from_subaccount: None,
                to: Account { owner: backend, subaccount: None },
                amount: candid::Nat::from(990u64),
                fee: Some(candid::Nat::from(10u64)),
                memo: Some(icrc_ledger_types::icrc1::transfer::Memo::from(b"claim-memo".to_vec())),
                created_at_time: Some(555),
            },
            block_index: None,
            dispatch_attempt_count: Some(0),
            prior_no_effects: Some(Vec::new()),
            dispatch_observation: None,
            history_scan: None,
        };
        assert!(update_ckusdc_payment_transfer(74, transfer.clone()));
        let mut completed = transfer.clone();
        completed.block_index = Some(77);
        assert!(update_ckusdc_payment_transfer(74, completed.clone()));
        assert_eq!(get_claim_intent(74).unwrap().ckusdc_payment_transfer, Some(completed.clone()));

        let mut changed = completed.clone();
        changed.args.created_at_time = Some(556);
        assert!(!update_ckusdc_payment_transfer(74, changed));
        let mut changed_block = completed.clone();
        changed_block.block_index = Some(78);
        assert!(!update_ckusdc_payment_transfer(74, changed_block));
        assert_eq!(get_claim_intent(74).unwrap().ckusdc_payment_transfer, Some(completed));
    }

    #[test]
    fn original_payment_history_cursor_and_positive_candidate_are_durable() {
        memory::init_memory_manager();
        init_history();
        let ledger = Principal::from_slice(&[0x44]);
        let backend = Principal::from_slice(&[0x45]);
        put_claim_intent(BotClaimIntent {
            vault_id: 75,
            request_id: 906,
            claim_call_count: Some(1),
            backend_principal: Some(backend),
            return_transfer: None,
            ckusdc_top_up_transfer: None,
            ckusdc_payment_transfer: None,
            shortfall_eligibility: None,
            icp_pool_input_fee_e8s: None,
            phase: BotClaimIntentPhase::SwapStarted(BotClaimReceipt {
                collateral_amount: 200, debt_covered: 100_000, collateral_price_e8s: 456,
                claim_timestamp: 124, payment_memo: b"claim-memo".to_vec(), claim_transfer: None,
            }),
        });
        let transfer = BotCkUsdcPaymentTransfer {
            ledger,
            args: TransferArg {
                from_subaccount: None,
                to: Account { owner: backend, subaccount: None },
                amount: candid::Nat::from(990u64), fee: Some(candid::Nat::from(10u64)),
                memo: Some(icrc_ledger_types::icrc1::transfer::Memo::from(b"claim-memo".to_vec())),
                created_at_time: Some(555),
            },
            block_index: None,
            dispatch_attempt_count: Some(0),
            prior_no_effects: Some(Vec::new()),
            dispatch_observation: Some(BotCkUsdcPaymentDispatchObservation::TooOld),
            history_scan: Some(BotCkUsdcPaymentHistoryScan {
                snapshot_log_length: 100, next_index: 99, candidate_block_index: None,
                multiple_candidates: false,
            }),
        };
        assert!(update_ckusdc_payment_transfer(75, transfer.clone()));
        let mut cursor = transfer.clone();
        cursor.history_scan.as_mut().unwrap().next_index = 98;
        assert!(update_ckusdc_payment_transfer(75, cursor.clone()));
        while cursor.history_scan.as_ref().unwrap().next_index > 0 {
            cursor.history_scan.as_mut().unwrap().next_index -= 1;
            assert!(update_ckusdc_payment_transfer(75, cursor.clone()));
        }
        // A late ledger commit can appear after the first fixed snapshot was
        // exhausted. Reconciliation may only advance to a monotonic snapshot.
        cursor.history_scan.as_mut().unwrap().snapshot_log_length = 101;
        cursor.history_scan.as_mut().unwrap().next_index = 101;
        assert!(update_ckusdc_payment_transfer(75, cursor.clone()));
        cursor.history_scan.as_mut().unwrap().next_index = 100;
        assert!(update_ckusdc_payment_transfer(75, cursor.clone()));
        let mut candidate = cursor;
        candidate.history_scan.as_mut().unwrap().next_index = 99;
        candidate.history_scan.as_mut().unwrap().candidate_block_index = Some(99);
        assert!(update_ckusdc_payment_transfer(75, candidate.clone()));
        let mut proven = candidate;
        proven.history_scan = None;
        proven.block_index = Some(99);
        assert!(update_ckusdc_payment_transfer(75, proven.clone()));
        assert_eq!(get_claim_intent(75).unwrap().ckusdc_payment_transfer, Some(proven));
    }

    #[test]
    fn no_effect_phase_and_ack_are_bound_to_exact_request_and_digest() {
        memory::init_memory_manager();
        init_history();
        let backend = Principal::from_slice(&[0x41]);
        let digest = vec![7; 32];
        let mut intent = BotClaimIntent {
            vault_id: 71,
            request_id: 902,
            claim_call_count: Some(2),
            backend_principal: Some(backend),
            return_transfer: None,
            ckusdc_top_up_transfer: None,
            ckusdc_payment_transfer: None,
            shortfall_eligibility: None,
            icp_pool_input_fee_e8s: None,
            phase: BotClaimIntentPhase::ClaimRequested,
        };

        history_put_for_cas_test(intent.clone());
        assert!(!mark_no_effect_acknowledgement_pending(71, 903, backend, digest.clone()));
        assert_eq!(get_claim_intent(71), Some(intent.clone()));

        assert!(!mark_no_effect_acknowledgement_pending(71, 902, Principal::from_slice(&[0x42]), digest.clone()));
        assert!(!mark_no_effect_acknowledgement_pending(71, 902, backend, vec![7; 31]));
        assert_eq!(get_claim_intent(71), Some(intent.clone()));

        assert!(mark_no_effect_acknowledgement_pending(71, 902, backend, digest.clone()));
        let pending = get_claim_intent(71).unwrap();
        assert_eq!(pending.phase, BotClaimIntentPhase::NoEffectAcknowledgementPending {
            transfer_digest: digest.clone(),
        });
        assert!(!clear_acknowledged_no_effect_claim_intent(71, 903, backend, &digest));
        assert!(!clear_acknowledged_no_effect_claim_intent(71, 902, Principal::from_slice(&[0x42]), &digest));
        assert!(!clear_acknowledged_no_effect_claim_intent(71, 902, backend, &[8; 32]));
        assert_eq!(get_claim_intent(71), Some(pending.clone()));

        // The proof phase must survive the stable Candid encoding used by the
        // intent map, including an upgrade between proof persistence and ACK.
        let bytes = candid::encode_one(&pending).unwrap();
        let restored: BotClaimIntent = candid::decode_one(&bytes).unwrap();
        assert_eq!(restored, pending);

        intent.claim_call_count = Some(0);
        history_put_for_cas_test(intent.clone());
        assert!(!mark_no_effect_acknowledgement_pending(71, 902, backend, digest.clone()));
        assert_eq!(get_claim_intent(71), Some(intent.clone()));

        intent.claim_call_count = Some(2);
        intent.phase = BotClaimIntentPhase::Acquired {
            claim_timestamp: 10,
            payment_memo: b"memo".to_vec(),
        };
        history_put_for_cas_test(intent.clone());
        assert!(!mark_no_effect_acknowledgement_pending(71, 902, backend, digest.clone()));
        assert_eq!(get_claim_intent(71), Some(intent.clone()));

        intent.phase = BotClaimIntentPhase::ClaimRequested;
        intent.backend_principal = Some(Principal::from_slice(&[0x42]));
        history_put_for_cas_test(intent.clone());
        assert!(!mark_no_effect_acknowledgement_pending(71, 902, backend, digest));
        assert_eq!(get_claim_intent(71), Some(intent));

        let mut acknowledged = pending;
        acknowledged.backend_principal = Some(backend);
        history_put_for_cas_test(acknowledged);
        assert!(clear_acknowledged_no_effect_claim_intent(71, 902, backend, &[7; 32]));
        assert_eq!(get_claim_intent(71), None);
    }

    #[test]
    fn first_typed_no_effect_replacement_keeps_old_tuple_tombstone_and_principal() {
        memory::init_memory_manager();
        init_history();
        let vault_id = 84;
        let backend = Principal::from_slice(&[0x45]);
        let ledger = Principal::from_slice(&[0x46]);
        let memo = b"claim-payment".to_vec();
        let receipt = BotClaimReceipt {
            collateral_amount: 200,
            debt_covered: 100_000,
            collateral_price_e8s: 456,
            claim_timestamp: 124,
            payment_memo: memo.clone(),
            claim_transfer: None,
        };
        put_claim_intent(BotClaimIntent {
            vault_id,
            request_id: 908,
            claim_call_count: Some(1),
            backend_principal: Some(backend),
            return_transfer: None,
            ckusdc_top_up_transfer: None,
            ckusdc_payment_transfer: None,
            shortfall_eligibility: None,
            icp_pool_input_fee_e8s: None,
            phase: BotClaimIntentPhase::Claimed(receipt),
        });
        let account_subaccount = {
            let mut bytes = [0; 32];
            bytes[..16].copy_from_slice(b"RUMI-CLAIM-PAY01");
            bytes[16..24].copy_from_slice(&vault_id.to_be_bytes());
            bytes[24..32].copy_from_slice(&124u64.to_be_bytes());
            bytes
        };
        let mut old = BotCkUsdcPaymentTransfer {
            ledger,
            args: TransferArg {
                from_subaccount: Some(account_subaccount),
                to: Account { owner: backend, subaccount: None },
                amount: candid::Nat::from(1_000u64),
                fee: Some(candid::Nat::from(10u64)),
                memo: Some(memo.clone().into()),
                created_at_time: Some(555),
            },
            block_index: None,
            dispatch_attempt_count: Some(0),
            prior_no_effects: Some(Vec::new()),
            dispatch_observation: None,
            history_scan: None,
        };
        assert!(update_ckusdc_payment_transfer(vault_id, old.clone()));
        old.dispatch_attempt_count = Some(1);
        assert!(update_ckusdc_payment_transfer(vault_id, old.clone()));
        let mut later_attempt = old.clone();
        later_attempt.dispatch_attempt_count = Some(2);
        assert!(!replace_ckusdc_payment_after_first_no_effect(vault_id, &later_attempt, later_attempt.clone()));
        old.dispatch_observation = Some(BotCkUsdcPaymentDispatchObservation::BadFee { expected_fee: 20 });
        assert!(update_ckusdc_payment_transfer(vault_id, old.clone()));

        let mut replacement = old.clone();
        replacement.args.amount = candid::Nat::from(1_000u64);
        replacement.args.fee = Some(candid::Nat::from(20u64));
        replacement.args.created_at_time = Some(556);
        replacement.dispatch_attempt_count = Some(0);
        replacement.prior_no_effects = Some(Vec::new());
        replacement.dispatch_observation = None;
        replacement.history_scan = None;

        let mut legacy_unknown = old.clone();
        legacy_unknown.dispatch_attempt_count = None;
        let mut legacy_intent = get_claim_intent(vault_id).unwrap();
        legacy_intent.vault_id = vault_id + 1;
        legacy_intent.ckusdc_payment_transfer = Some(legacy_unknown.clone());
        put_claim_intent(legacy_intent);
        assert!(!replace_ckusdc_payment_after_first_no_effect(
            vault_id + 1, &legacy_unknown, replacement.clone(),
        ), "legacy missing attempt provenance must never authorize a replacement");

        assert!(replace_ckusdc_payment_after_first_no_effect(vault_id, &old, replacement.clone()));
        let saved = get_claim_intent(vault_id).unwrap().ckusdc_payment_transfer.unwrap();
        assert_eq!(saved.args.amount, candid::Nat::from(1_000u64));
        assert_eq!(saved.args.fee, Some(candid::Nat::from(20u64)));
        let prior_no_effects = saved.prior_no_effects.unwrap();
        assert_eq!(prior_no_effects.len(), 1);
        assert_eq!(prior_no_effects[0].args.created_at_time, Some(555));
        assert_eq!(prior_no_effects[0].observation,
            BotCkUsdcPaymentDispatchObservation::BadFee { expected_fee: 20 });
        assert!(!replace_ckusdc_payment_after_first_no_effect(vault_id, &old, replacement));
    }

    fn history_put_for_cas_test(intent: BotClaimIntent) {
        put_claim_intent(intent);
    }

    #[test]
    fn record_count_is_the_stable_next_unused_request_id() {
        memory::init_memory_manager();
        init_history();

        assert_eq!(record_count(), 0, "zero is the first unused request ID");
        assert_eq!(next_id(), 0);
        assert_eq!(record_count(), 1);
        assert_eq!(next_id(), 1);
        assert_eq!(record_count(), 2);
    }
}

pub fn next_id() -> u64 {
    NEXT_ID.with(|c| {
        let mut borrow = c.borrow_mut();
        let cell = borrow.as_mut().expect("History not initialized");
        let id = *cell.get();
        cell.set(id + 1).expect("Failed to increment NEXT_ID");
        id
    })
}

pub fn insert_record(record: LiquidationRecordVersioned) {
    let id = match &record {
        LiquidationRecordVersioned::V1(r) => r.id,
    };
    HISTORY.with(|h| {
        h.borrow_mut()
            .as_mut()
            .expect("History not initialized")
            .insert(id, record);
    });
}

pub fn get_record(id: u64) -> Option<LiquidationRecordVersioned> {
    HISTORY.with(|h| {
        h.borrow()
            .as_ref()
            .expect("History not initialized")
            .get(&id)
    })
}

/// Return the newest persisted record for a vault. Retry decisions must use
/// the latest attempt so an old paid ConfirmFailed record cannot authorize a
/// later attempt with a different outcome.
pub fn get_latest_record_for_vault(vault_id: u64) -> Option<LiquidationRecordVersioned> {
    let count = record_count();
    for id in (0..count).rev() {
        if let Some(record) = get_record(id) {
            match &record {
                LiquidationRecordVersioned::V1(r) if r.vault_id == vault_id => {
                    return Some(record);
                }
                _ => {}
            }
        }
    }
    None
}

/// A stuck confirm is safe to retry only after this attempt recorded a
/// successful, positive ckUSDC transfer. ConfirmFailed is also used when
/// swap-failure cleanup cannot cancel a claim; those records have zero
/// transferred ckUSDC and must never be confirmed as paid liquidations.
pub fn is_paid_confirm_failure(record: &LiquidationRecordV1) -> bool {
    record.status == LiquidationStatus::ConfirmFailed
        && record.ckusdc_transferred_e6 > 0
        && record.claim_timestamp.is_some()
        && record.payment_memo.as_ref().is_some_and(|memo| !memo.is_empty())
        && record.ckusdc_payment_block_index.is_some()
        && record.ckusdc_payment_amount_e6 == Some(record.ckusdc_transferred_e6)
}

/// A retry must match the requested vault, a currently open backend claim,
/// and a persisted successful payment from the latest local attempt.
pub fn is_retriable_paid_confirm_failure(
    record: &LiquidationRecordVersioned,
    vault_id: u64,
    active_claim_ids: &[u64],
) -> bool {
    active_claim_ids.contains(&vault_id)
        && matches!(record, LiquidationRecordVersioned::V1(r)
            if r.vault_id == vault_id && is_paid_confirm_failure(r))
}

/// A treasury-bonus transfer can fail only after the backend has already been
/// paid in ckUSDC. Do not let the ckUSDC-only emergency sweep relabel that
/// still-unpaid ICP obligation as resolved.
pub fn is_unresolved_treasury_bonus(record: &LiquidationRecordVersioned) -> bool {
    matches!(record, LiquidationRecordVersioned::V1(r)
        if r.status == LiquidationStatus::TransferFailed
            && r.ckusdc_transferred_e6 > 0
            && r.icp_to_treasury_e8s > 0)
}

pub fn get_records(offset: u64, limit: u64) -> Vec<LiquidationRecordVersioned> {
    let limit = limit.min(1000);
    HISTORY.with(|h| {
        let borrow = h.borrow();
        let map = borrow.as_ref().expect("History not initialized");
        let count = record_count();
        if count == 0 || offset >= count {
            return vec![];
        }
        let start = count.saturating_sub(offset.saturating_add(limit));
        let end = count.saturating_sub(offset);
        (start..end).filter_map(|id| map.get(&id)).collect()
    })
}

pub fn record_count() -> u64 {
    NEXT_ID.with(|c| {
        *c.borrow()
            .as_ref()
            .expect("History not initialized")
            .get()
    })
}

/// Returns stuck records (TransferFailed or ConfirmFailed), scanning from newest first.
/// Capped at 100 results to avoid hitting the instruction limit on large histories.
pub fn get_stuck_records() -> Vec<LiquidationRecordVersioned> {
    const MAX_RESULTS: usize = 100;
    HISTORY.with(|h| {
        let borrow = h.borrow();
        let map = borrow.as_ref().expect("History not initialized");
        let count = record_count();
        let mut results = Vec::new();
        for id in (0..count).rev() {
            if results.len() >= MAX_RESULTS {
                break;
            }
            if let Some(record) = map.get(&id) {
                match &record {
                    LiquidationRecordVersioned::V1(r) => match r.status {
                        LiquidationStatus::TransferFailed
                        | LiquidationStatus::ConfirmFailed => results.push(record),
                        _ => {}
                    },
                }
            }
        }
        results
    })
}

pub fn update_record_status(id: u64, new_status: LiquidationStatus) {
    HISTORY.with(|h| {
        let mut borrow = h.borrow_mut();
        let map = borrow.as_mut().expect("History not initialized");
        if let Some(mut record) = map.get(&id) {
            match &mut record {
                LiquidationRecordVersioned::V1(ref mut r) => {
                    r.status = new_status;
                }
            }
            map.insert(id, record);
        }
    });
}

pub fn update_record_status_and_error(
    id: u64,
    new_status: LiquidationStatus,
    error_message: Option<String>,
) -> bool {
    HISTORY.with(|h| {
        let mut borrow = h.borrow_mut();
        let map = borrow.as_mut().expect("History not initialized");
        let Some(LiquidationRecordVersioned::V1(mut record)) = map.get(&id) else {
            return false;
        };
        record.status = new_status;
        record.error_message = error_message;
        map.insert(id, LiquidationRecordVersioned::V1(record));
        true
    })
}

/// Update the durable ICP bonus journal and terminal accounting status in one
/// synchronous state transition. `transfer == None` is valid for quarantined
/// legacy records whose original request identity was never captured.
pub fn update_icp_treasury_bonus(
    id: u64,
    transfer: Option<IcpTreasuryBonusTransfer>,
    bonus_state: IcpTreasuryBonusState,
    status: LiquidationStatus,
    error_message: Option<String>,
) -> bool {
    HISTORY.with(|h| {
        let mut borrow = h.borrow_mut();
        let map = borrow.as_mut().expect("History not initialized");
        let Some(LiquidationRecordVersioned::V1(mut record)) = map.get(&id) else {
            return false;
        };
        record.icp_treasury_transfer = transfer;
        record.icp_treasury_bonus_state = Some(bonus_state);
        record.status = status;
        record.error_message = error_message;
        map.insert(id, LiquidationRecordVersioned::V1(record));
        true
    })
}

pub fn is_pending_icp_treasury_bonus(record: &LiquidationRecordVersioned) -> bool {
    matches!(record, LiquidationRecordVersioned::V1(r)
        if r.status == LiquidationStatus::TransferFailed
            && r.ckusdc_transferred_e6 > 0
            && r.icp_to_treasury_e8s > 0
            && r.icp_treasury_bonus_state != Some(IcpTreasuryBonusState::Paid)
            && r.icp_treasury_bonus_state != Some(IcpTreasuryBonusState::Quarantined))
}

/// Retry only an ICP bonus with an exact, internally consistent persisted
/// request. Legacy records have no such identity and can only be quarantined
/// or reconciled from independent ledger evidence.
pub fn is_retryable_icp_treasury_bonus(
    record: &LiquidationRecordVersioned,
    record_id: u64,
    bot_id: Principal,
    ledger: Principal,
    treasury: Principal,
) -> bool {
    let LiquidationRecordVersioned::V1(r) = record;
    if !is_pending_icp_treasury_bonus(record)
        || r.id != record_id
        || r.icp_treasury_bonus_state != Some(IcpTreasuryBonusState::Prepared)
    {
        return false;
    }
    let Some(transfer) = &r.icp_treasury_transfer else {
        return false;
    };
    let amount = transfer.args.amount.0.to_string().parse::<u64>();
    let fee = transfer.args.fee.as_ref().and_then(|fee| fee.0.to_string().parse::<u64>().ok());
    let memo = transfer.args.memo.as_ref().map(|memo| memo.0.as_ref());
    let mut expected_memo = b"RUMI:ICP_BONUS:V1:".to_vec();
    expected_memo.extend_from_slice(&record_id.to_be_bytes());
    transfer.ledger == ledger
        && transfer.from == (Account { owner: bot_id, subaccount: None })
        && transfer.args.from_subaccount.is_none()
        && transfer.args.to == (Account { owner: treasury, subaccount: None })
        && transfer.args.created_at_time.is_some()
        && transfer.block_index.is_none()
        && fee.is_some()
        && amount.ok().zip(fee).is_some_and(|(amount, fee)| {
            amount.checked_add(fee) == Some(r.icp_to_treasury_e8s)
        })
        && memo == Some(expected_memo.as_slice())
        && memo.is_some_and(|memo| memo.len() <= 32)
}

/// Reconciliation is limited to complete, new-format journals. A quarantined
/// exact journal may still be reconciled by block; rows with no identity or a
/// block already recorded cannot be upgraded by an admin-supplied index.
pub fn is_reconcilable_icp_treasury_bonus(
    record: &LiquidationRecordVersioned,
    record_id: u64,
    bot_id: Principal,
    ledger: Principal,
    treasury: Principal,
) -> bool {
    let LiquidationRecordVersioned::V1(r) = record;
    if r.id != record_id
        || r.status != LiquidationStatus::TransferFailed
        || r.ckusdc_transferred_e6 == 0
        || r.icp_to_treasury_e8s == 0
        || r.icp_treasury_bonus_state == Some(IcpTreasuryBonusState::Paid)
    {
        return false;
    }
    let Some(transfer) = &r.icp_treasury_transfer else { return false };
    if transfer.block_index.is_some() { return false; }
    let amount = transfer.args.amount.0.to_string().parse::<u64>();
    let fee = transfer.args.fee.as_ref().and_then(|fee| fee.0.to_string().parse::<u64>().ok());
    let memo = transfer.args.memo.as_ref().map(|memo| memo.0.as_ref());
    let mut expected_memo = b"RUMI:ICP_BONUS:V1:".to_vec();
    expected_memo.extend_from_slice(&record_id.to_be_bytes());
    matches!(r.icp_treasury_bonus_state,
        Some(IcpTreasuryBonusState::Prepared | IcpTreasuryBonusState::Quarantined))
        && transfer.ledger == ledger
        && transfer.from == (Account { owner: bot_id, subaccount: None })
        && transfer.args.from_subaccount.is_none()
        && transfer.args.to == (Account { owner: treasury, subaccount: None })
        && transfer.args.created_at_time.is_some()
        && fee.is_some()
        && amount.ok().zip(fee).is_some_and(|(amount, fee)| {
            amount.checked_add(fee) == Some(r.icp_to_treasury_e8s)
        })
        && memo == Some(expected_memo.as_slice())
        && memo.is_some_and(|memo| memo.len() <= 32)
}

/// Migrate legacy BotLiquidationEvent entries into the new stable map.
/// Capped at 500 entries to avoid trapping in post_upgrade due to instruction limit.
/// In practice the bot has far fewer legacy events than this.
pub fn migrate_legacy_events(events: &[crate::state::BotLiquidationEvent]) {
    let events = if events.len() > 500 { &events[..500] } else { events };
    for event in events {
        let id = next_id();
        let status = if event.success {
            LiquidationStatus::Completed
        } else {
            LiquidationStatus::SwapFailed
        };
        let record = LiquidationRecordV1 {
            id,
            vault_id: event.vault_id,
            timestamp: event.timestamp,
            status,
            collateral_claimed_e8s: event.collateral_received_e8s,
            debt_to_cover_e8s: event.debt_covered_e8s,
            icp_swapped_e8s: 0,
            ckusdc_received_e6: 0,
            ckusdc_transferred_e6: 0,
            icp_to_treasury_e8s: event.collateral_to_treasury_e8s,
            oracle_price_e8s: event.effective_price_e8s,
            effective_price_e8s: event.effective_price_e8s,
            slippage_bps: event.slippage_bps,
            error_message: event.error_message.clone(),
            confirm_retry_count: 0,
            claim_timestamp: None,
            payment_memo: None,
            ckusdc_payment_block_index: None,
            ckusdc_payment_amount_e6: None,
            icp_treasury_transfer: None,
            icp_treasury_bonus_state: None,
        };
        insert_record(LiquidationRecordVersioned::V1(record));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use candid::Nat;
    use icrc_ledger_types::icrc1::transfer::Memo;

    fn record(status: LiquidationStatus, transferred: u64) -> LiquidationRecordV1 {
        LiquidationRecordV1 {
            id: 0,
            vault_id: 1,
            timestamp: 0,
            status,
            collateral_claimed_e8s: 1,
            debt_to_cover_e8s: 1,
            icp_swapped_e8s: 1,
            ckusdc_received_e6: transferred,
            ckusdc_transferred_e6: transferred,
            icp_to_treasury_e8s: 0,
            oracle_price_e8s: 1,
            effective_price_e8s: 1,
            slippage_bps: 0,
            error_message: None,
            confirm_retry_count: 0,
            claim_timestamp: None,
            payment_memo: None,
            ckusdc_payment_block_index: None,
            ckusdc_payment_amount_e6: None,
            icp_treasury_transfer: None,
            icp_treasury_bonus_state: None,
        }
    }

    fn prepared_icp_bonus() -> (LiquidationRecordVersioned, Principal, Principal, Principal) {
        let bot = Principal::from_text("aaaaa-aa").unwrap();
        let ledger = Principal::from_text("ryjl3-tyaaa-aaaaa-aaaba-cai").unwrap();
        let treasury = Principal::from_text("rrkah-fqaaa-aaaaa-aaaaq-cai").unwrap();
        let id: u64 = 7;
        let mut memo = b"RUMI:ICP_BONUS:V1:".to_vec();
        memo.extend_from_slice(&id.to_be_bytes());
        let mut record = record(LiquidationStatus::TransferFailed, 30_000);
        record.id = id;
        record.icp_to_treasury_e8s = 20_000;
        record.icp_treasury_bonus_state = Some(IcpTreasuryBonusState::Prepared);
        record.icp_treasury_transfer = Some(IcpTreasuryBonusTransfer {
            ledger,
            from: Account { owner: bot, subaccount: None },
            args: TransferArg {
                from_subaccount: None,
                to: Account { owner: treasury, subaccount: None },
                amount: Nat::from(10_000u64),
                fee: Some(Nat::from(10_000u64)),
                memo: Some(Memo::from(memo)),
                created_at_time: Some(123),
            },
            block_index: None,
        });
        (LiquidationRecordVersioned::V1(record), bot, ledger, treasury)
    }

    #[test]
    fn ambiguous_icp_attempt_stays_pending_with_same_identity_for_retry() {
        let (record, bot, ledger, treasury) = prepared_icp_bonus();
        assert!(is_pending_icp_treasury_bonus(&record));
        assert!(is_retryable_icp_treasury_bonus(&record, 7, bot, ledger, treasury));
        assert!(is_reconcilable_icp_treasury_bonus(&record, 7, bot, ledger, treasury));
        let LiquidationRecordVersioned::V1(r) = record else { unreachable!() };
        assert_eq!(r.icp_treasury_bonus_state, Some(IcpTreasuryBonusState::Prepared));
        assert_eq!(r.icp_treasury_transfer.unwrap().args.created_at_time, Some(123));
    }

    #[test]
    fn icp_bonus_memo_fits_native_ledger_limit_and_binds_distinct_ids() {
        for id in [0u64, 7, u64::MAX] {
            let mut memo = b"RUMI:ICP_BONUS:V1:".to_vec();
            memo.extend_from_slice(&id.to_be_bytes());
            assert!(memo.len() <= 32);
            assert_eq!(&memo[memo.len() - 8..], &id.to_be_bytes());
        }
        let mut one = b"RUMI:ICP_BONUS:V1:".to_vec();
        one.extend_from_slice(&1u64.to_be_bytes());
        let mut two = b"RUMI:ICP_BONUS:V1:".to_vec();
        two.extend_from_slice(&2u64.to_be_bytes());
        assert_ne!(one, two);
    }

    #[test]
    fn reconciliation_requires_complete_exact_identity_and_is_idempotently_gated() {
        let (record, bot, ledger, treasury) = prepared_icp_bonus();
        assert!(is_reconcilable_icp_treasury_bonus(&record, 7, bot, ledger, treasury));
        assert!(!is_reconcilable_icp_treasury_bonus(&record, 8, bot, ledger, treasury));
        let LiquidationRecordVersioned::V1(mut legacy) = record.clone() else { unreachable!() };
        legacy.icp_treasury_transfer = None;
        legacy.icp_treasury_bonus_state = Some(IcpTreasuryBonusState::Quarantined);
        assert!(!is_reconcilable_icp_treasury_bonus(
            &LiquidationRecordVersioned::V1(legacy), 7, bot, ledger, treasury,
        ));
        let LiquidationRecordVersioned::V1(mut resolved) = record else { unreachable!() };
        resolved.icp_treasury_transfer.as_mut().unwrap().block_index = Some(99);
        assert!(!is_reconcilable_icp_treasury_bonus(
            &LiquidationRecordVersioned::V1(resolved), 7, bot, ledger, treasury,
        ));
    }

    #[test]
    fn retry_rejects_wrong_recipient_amount_and_legacy_missing_identity() {
        let (record, bot, ledger, treasury) = prepared_icp_bonus();
        let LiquidationRecordVersioned::V1(mut wrong_to) = record.clone() else { unreachable!() };
        wrong_to.icp_treasury_transfer.as_mut().unwrap().args.to.owner = bot;
        assert!(!is_retryable_icp_treasury_bonus(
            &LiquidationRecordVersioned::V1(wrong_to), 7, bot, ledger, treasury,
        ));

        let LiquidationRecordVersioned::V1(mut wrong_amount) = record.clone() else { unreachable!() };
        wrong_amount.icp_treasury_transfer.as_mut().unwrap().args.amount = Nat::from(9_999u64);
        assert!(!is_retryable_icp_treasury_bonus(
            &LiquidationRecordVersioned::V1(wrong_amount), 7, bot, ledger, treasury,
        ));

        let LiquidationRecordVersioned::V1(mut legacy) = record else { unreachable!() };
        legacy.icp_treasury_transfer = None;
        legacy.icp_treasury_bonus_state = None;
        let legacy = LiquidationRecordVersioned::V1(legacy);
        assert!(is_pending_icp_treasury_bonus(&legacy));
        assert!(!is_retryable_icp_treasury_bonus(&legacy, 7, bot, ledger, treasury));
    }

    #[test]
    fn completed_ckusdc_does_not_make_an_icp_bonus_paid() {
        let (record, ..) = prepared_icp_bonus();
        assert!(is_unresolved_treasury_bonus(&record));
        assert!(is_pending_icp_treasury_bonus(&record));
    }

    #[test]
    fn old_candid_record_decodes_without_inventing_a_retry_identity() {
        #[derive(CandidType, Deserialize)]
        struct OldRecord {
            id: u64,
            vault_id: u64,
            timestamp: u64,
            status: LiquidationStatus,
            collateral_claimed_e8s: u64,
            debt_to_cover_e8s: u64,
            icp_swapped_e8s: u64,
            ckusdc_received_e6: u64,
            ckusdc_transferred_e6: u64,
            icp_to_treasury_e8s: u64,
            oracle_price_e8s: u64,
            effective_price_e8s: u64,
            slippage_bps: i32,
            error_message: Option<String>,
            confirm_retry_count: u8,
        }
        #[derive(CandidType, Deserialize)]
        enum OldVersioned {
            V1(OldRecord),
        }

        let bytes = candid::encode_one(OldVersioned::V1(OldRecord {
            id: 4,
            vault_id: 9,
            timestamp: 11,
            status: LiquidationStatus::TransferFailed,
            collateral_claimed_e8s: 30_000,
            debt_to_cover_e8s: 1,
            icp_swapped_e8s: 10_000,
            ckusdc_received_e6: 3,
            ckusdc_transferred_e6: 3,
            icp_to_treasury_e8s: 20_000,
            oracle_price_e8s: 1,
            effective_price_e8s: 1,
            slippage_bps: 0,
            error_message: Some("old failure".into()),
            confirm_retry_count: 0,
        })).unwrap();
        let decoded: LiquidationRecordVersioned =
            candid::decode_one(&bytes).expect("additive opt fields decode old rows");
        let LiquidationRecordVersioned::V1(decoded) = decoded;
        assert_eq!(decoded.icp_to_treasury_e8s, 20_000);
        assert_eq!(decoded.icp_treasury_transfer, None);
        assert_eq!(decoded.icp_treasury_bonus_state, None);
        assert!(is_pending_icp_treasury_bonus(&LiquidationRecordVersioned::V1(decoded)));
    }

    #[test]
    fn only_paid_confirm_failed_records_are_retriable() {
        let paid = || {
            let mut r = record(LiquidationStatus::ConfirmFailed, 1);
            r.claim_timestamp = Some(10);
            r.payment_memo = Some(vec![1]);
            r.ckusdc_payment_block_index = Some(12);
            r.ckusdc_payment_amount_e6 = Some(1);
            r
        };
        for status in [
            LiquidationStatus::Completed,
            LiquidationStatus::SwapFailed,
            LiquidationStatus::TransferFailed,
            LiquidationStatus::ClaimFailed,
            LiquidationStatus::AdminResolved,
        ] {
            let mut r = paid();
            r.status = status;
            assert!(!is_paid_confirm_failure(&r));
        }
        let mut zero_transfer = paid();
        zero_transfer.ckusdc_transferred_e6 = 0;
        assert!(!is_paid_confirm_failure(&zero_transfer));
        assert!(is_paid_confirm_failure(&paid()));
    }

    #[test]
    fn retry_requires_the_requested_active_vault_claim() {
        let mut paid_record = record(LiquidationStatus::ConfirmFailed, 1);
        paid_record.claim_timestamp = Some(10);
        paid_record.payment_memo = Some(vec![1]);
        paid_record.ckusdc_payment_block_index = Some(12);
        paid_record.ckusdc_payment_amount_e6 = Some(1);
        let paid = LiquidationRecordVersioned::V1(paid_record);
        assert!(is_retriable_paid_confirm_failure(&paid, 1, &[1]));
        assert!(!is_retriable_paid_confirm_failure(&paid, 1, &[]));
        assert!(!is_retriable_paid_confirm_failure(&paid, 1, &[2]));
        assert!(!is_retriable_paid_confirm_failure(&paid, 2, &[1, 2]));
    }

    #[test]
    fn only_paid_ckusdc_transfer_failures_with_icp_obligation_block_ckusdc_resolution() {
        let pending_icp = LiquidationRecordVersioned::V1(LiquidationRecordV1 {
            icp_to_treasury_e8s: 7,
            ..record(LiquidationStatus::TransferFailed, 1)
        });
        assert!(is_unresolved_treasury_bonus(&pending_icp));

        let unpaid_ckusdc = LiquidationRecordVersioned::V1(LiquidationRecordV1 {
            icp_to_treasury_e8s: 7,
            ..record(LiquidationStatus::TransferFailed, 0)
        });
        assert!(!is_unresolved_treasury_bonus(&unpaid_ckusdc));

        let no_icp_obligation = LiquidationRecordVersioned::V1(LiquidationRecordV1 {
            icp_to_treasury_e8s: 0,
            ..record(LiquidationStatus::TransferFailed, 1)
        });
        assert!(!is_unresolved_treasury_bonus(&no_icp_obligation));

        let already_completed = LiquidationRecordVersioned::V1(LiquidationRecordV1 {
            icp_to_treasury_e8s: 7,
            ..record(LiquidationStatus::Completed, 1)
        });
        assert!(!is_unresolved_treasury_bonus(&already_completed));
    }
}
