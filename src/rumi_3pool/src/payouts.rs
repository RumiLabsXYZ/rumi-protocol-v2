//! Durable evidence for outbound 3pool payout/refund attempts.
//!
//! `PAYOUT_CURRENT` stores the exact pre-call identity and is the recovery
//! source of truth. The append-only log stores materialized confirmation,
//! ambiguity, settlement, and compensation evidence. Definitive no-effect
//! reservations are retired, leaving monotonic ID gaps rather than history.
use crate::storage;
use candid::{CandidType, Principal};
use icrc_ledger_types::icrc1::account::Account;
use serde::{Deserialize, Serialize};

#[derive(CandidType, Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PayoutKind {
    SwapOutput,
    SwapInputRefund,
    AddLiquidityRefund,
    RemoveLiquidity,
    RemoveOneCoin,
    AdminFeeWithdrawal,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayoutTransfer {
    pub ledger: Principal,
    pub from: Account,
    pub to: Account,
    /// Gross entitlement debited from the pool, including the ledger fee.
    pub gross: u128,
    /// Recipient credit. The ledger fee is explicit and is also recorded.
    pub net: u128,
    pub fee: u128,
    pub memo: Vec<u8>,
    pub created_at_time: u64,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayoutInputTransfer {
    pub ledger: Principal,
    pub from: Account,
    pub to: Account,
    pub amount: u128,
    /// Unique deduplication salt derived from this stable entitlement ID.
    /// Optional only for forward compatibility with early, unreleased saga data.
    #[serde(default)]
    pub memo: Option<Vec<u8>>,
    pub created_at_time: u64,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PayoutInputOutcome {
    Prepared,
    Submitted,
    Confirmed { block: candid::Nat },
    RejectedNoTransfer { reason: String },
    Unresolved { reason: String },
}

#[derive(CandidType, Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PayoutInputAction {
    AddLiquidity,
    Swap,
    Donation,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PayoutOutcome {
    Prepared,
    Submitted,
    Confirmed { block: candid::Nat },
    RejectedNoTransfer { reason: String },
    Unresolved { reason: String },
    HeldLegacyUnbound,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayoutSwapContext {
    pub balances_before: [u128; 3],
    pub admin_fees_before: [u128; 3],
    pub amp: u64,
    pub precision_muls: [u64; 3],
    pub token_in: u8,
    pub token_out: u8,
    pub amount_in: u128,
    pub gross_output: u128,
    pub pool_fee: u128,
    pub admin_fee_bps: u64,
    pub fee_bps: u16,
    pub imbalance_before: u64,
    pub imbalance_after: u64,
    pub is_rebalancing: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayoutFailure {
    pub id: u64,
    pub reason: String,
    pub ambiguous: bool,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayoutAttempt {
    pub number: u32,
    pub replay_count: u8,
    pub transfer: PayoutTransfer,
    pub outcome: PayoutOutcome,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayoutEntitlement {
    pub id: u64,
    pub owner: Principal,
    pub token_index: u8,
    pub ledger: Principal,
    pub symbol: String,
    pub gross: u128,
    pub kind: PayoutKind,
    pub swap_context: Option<PayoutSwapContext>,
    /// Exactly one durable input-refund entitlement may compensate a proven
    /// no-effect swap output. Bound before that refund's ledger call.
    pub compensation_id: Option<u64>,
    /// Set on the refund entitlement so recovery can close the output leg.
    pub compensation_for: Option<u64>,
    /// False while a multi-leg operation has reserved an outbound tuple but
    /// has not yet established that this leg is owed. Missing legacy values
    /// are treated as ready because older entitlements were dispatched
    /// immediately after creation.
    pub dispatch_ready: Option<bool>,
    pub input_transfer: Option<PayoutInputTransfer>,
    pub input_outcome: Option<PayoutInputOutcome>,
    pub input_action: Option<PayoutInputAction>,
    pub settled: bool,
    pub attempts: Vec<PayoutAttempt>,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PayoutJournalEventKind {
    // Retained for decoding journal entries created by early saga builds.
    // New Prepared/Submitted/no-effect transitions live only in PAYOUT_CURRENT
    // so permissionless no-effect operations do not grow the append-only log.
    Prepared { attempt: PayoutAttempt },
    Submitted { attempt_number: u32, replay_count: u8 },
    Confirmed { attempt_number: u32, block: candid::Nat },
    RejectedNoTransfer { attempt_number: u32, reason: String },
    Unresolved { attempt_number: u32, reason: String },
    Settled,
    CompensationBound { compensation_id: u64 },
    InputSubmitted,
    InputConfirmed { block: candid::Nat },
    InputRejectedNoTransfer { reason: String },
    InputUnresolved { reason: String },
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayoutJournalEvent {
    pub sequence: u64,
    pub entitlement_id: u64,
    pub kind: PayoutJournalEventKind,
}

pub fn append(entitlement_id: u64, kind: PayoutJournalEventKind) {
    let sequence = storage::payouts::next_event_id();
    storage::payouts::append_event(PayoutJournalEvent {
        sequence,
        entitlement_id,
        kind,
    });
}

pub fn get(id: u64) -> Option<PayoutEntitlement> {
    storage::payouts::get(id)
}

pub fn list(offset: u64, limit: u64) -> Vec<PayoutEntitlement> {
    storage::payouts::list(offset, limit)
}

pub fn list_for_owner(owner: Principal, offset: u64, limit: u64) -> Vec<PayoutEntitlement> {
    storage::payouts::list_for_owner(owner, offset, limit)
}

pub fn save(value: PayoutEntitlement) {
    storage::payouts::insert(value);
}

/// Retire a definitively no-effect reservation. IDs remain monotonic and are
/// never reused, so discarded tuples cannot become a later operation identity.
pub fn remove_no_effect(value: &PayoutEntitlement) -> bool {
    storage::payouts::remove_no_effect(value.id, value.owner)
}

/// Close an entitlement once its ledger transfer and corresponding pool-side
/// accounting have both been committed.
pub fn mark_settled(id: u64) -> bool {
    let Some(mut value) = get(id) else { return false };
    if value.settled {
        return true;
    }
    value.settled = true;
    save(value.clone());
    append(id, PayoutJournalEventKind::Settled);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_transfer_tuple_and_failure_class_are_stable() {
        let owner = Principal::self_authenticating(b"payout owner");
        let ledger = Principal::self_authenticating(b"ledger");
        let transfer = PayoutTransfer {
            ledger,
            from: Account { owner: Principal::management_canister(), subaccount: None },
            to: Account { owner, subaccount: None },
            gross: 12_000,
            net: 10_000,
            fee: 2_000,
            memo: vec![1; 32],
            created_at_time: 123,
        };
        let attempt = PayoutAttempt {
            number: 0,
            replay_count: 0,
            transfer: transfer.clone(),
            outcome: PayoutOutcome::Prepared,
        };
        let entitlement = PayoutEntitlement {
            id: 3,
            owner,
            token_index: 1,
            ledger,
            symbol: "TKN".into(),
            gross: 12_000,
            kind: PayoutKind::RemoveLiquidity,
            swap_context: None,
            compensation_id: None,
            compensation_for: None,
            dispatch_ready: Some(true),
            input_transfer: None,
            input_outcome: None,
            input_action: None,
            settled: false,
            attempts: vec![attempt],
        };
        let bytes = candid::encode_one(&entitlement).unwrap();
        let decoded: PayoutEntitlement = candid::decode_one(&bytes).unwrap();
        assert_eq!(decoded.attempts[0].transfer, transfer);
        assert_eq!(decoded.attempts[0].outcome, PayoutOutcome::Prepared);
    }
}
