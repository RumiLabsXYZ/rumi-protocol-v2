//! Durable evidence for outbound 3pool payout/refund attempts.
//!
//! The append-only journal is authoritative. `PAYOUT_CURRENT` is a lookup
//! projection used by recovery and claim queries; it is never used to invent a
//! new transfer identity for an old claim.
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
    pub amp: u128,
    pub precision_muls: [u128; 3],
    pub token_in: u8,
    pub token_out: u8,
    pub amount_in: u128,
    pub gross_output: u128,
    pub pool_fee: u128,
    pub admin_fee_bps: u16,
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
    pub settled: bool,
    pub attempts: Vec<PayoutAttempt>,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PayoutJournalEventKind {
    Prepared { attempt: PayoutAttempt },
    Submitted { attempt_number: u32, replay_count: u8 },
    Confirmed { attempt_number: u32, block: candid::Nat },
    RejectedNoTransfer { attempt_number: u32, reason: String },
    Unresolved { attempt_number: u32, reason: String },
    Settled,
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
            attempts: vec![attempt],
        };
        let bytes = candid::encode_one(&entitlement).unwrap();
        let decoded: PayoutEntitlement = candid::decode_one(&bytes).unwrap();
        assert_eq!(decoded.attempts[0].transfer, transfer);
        assert_eq!(decoded.attempts[0].outcome, PayoutOutcome::Prepared);
    }
}
