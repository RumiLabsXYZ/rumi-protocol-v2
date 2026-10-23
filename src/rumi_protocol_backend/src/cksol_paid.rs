//! Caller-funded SOL notification. Every external effect has a durable phase.
//! Unknown funding/deposit/refund outcomes are never retried as fresh effects.
use crate::cksol::{CKSOL_RELAY_CALL_CYCLES, CKSOL_RELAY_CANISTER_RESERVE, CKSOL_RELAY_OWNER_CAP};
use crate::state::{mutate_state, read_state, CkSolRelayAttempt, CkSolRelayStatus, State};
use async_trait::async_trait;
use candid::{CandidType, Deserialize, Nat, Principal};
use serde::Serialize;

pub const CYCLES_LEDGER: &str = "um5iw-rqaaa-aaaaq-qaaba-cai";
pub const PROCESSING_FEE: u64 = 1_000_000_000;
pub const LEDGER_FEE: u64 = 100_000_000;
// Included in PROCESSING_FEE, not an additional debit. Covers a ledger deposit
// fee up to LEDGER_FEE, so the minter's entire unused attachment returns to user.
pub const REFUND_FEE_RESERVE: u64 = LEDGER_FEE;
const FUNDING: u64 = CKSOL_RELAY_CALL_CYCLES as u64 + PROCESSING_FEE;

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub enum CkSolPaymentPhase {
    FundingPending,
    FundingUnknown,
    Funded,
    DepositPending,
    RefundReady,
    RefundPending,
    RefundUnknown,
    Complete,
    FundingRejected,
}
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct CkSolPayment {
    pub sequence: u64,
    pub created_at_time: u64,
    pub phase: CkSolPaymentPhase,
    pub funding_block: Option<Nat>,
    pub deposit_id: Option<u64>,
    pub refundable_cycles: u64,
    pub refund_block: Option<Nat>,
    pub message: Option<String>,
}
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct CkSolPaymentQuote {
    pub cycles_ledger: Principal,
    pub funding_cycles: u64,
    pub processing_fee: u64,
    pub ledger_fee: u64,
    pub approval_amount: u64,
    pub refund_fee_reserve: u64,
    pub next_sequence: u64,
}
pub fn quote() -> CkSolPaymentQuote {
    CkSolPaymentQuote {
        cycles_ledger: ledger(),
        funding_cycles: FUNDING,
        processing_fee: PROCESSING_FEE,
        ledger_fee: LEDGER_FEE,
        approval_amount: FUNDING + LEDGER_FEE,
        refund_fee_reserve: REFUND_FEE_RESERVE,
        next_sequence: read_state(|s| s.cksol_paid_sequence.saturating_add(1)),
    }
}
pub fn ledger() -> Principal {
    Principal::from_text(CYCLES_LEDGER).expect("fixed ledger principal")
}
pub fn payment(owner: Principal) -> Option<CkSolPayment> {
    read_state(|s| s.cksol_paid_payments.get(&owner).cloned())
}
fn row(owner: Principal) -> Result<CkSolPayment, String> {
    payment(owner).ok_or_else(|| "No ckSOL payment exists for this account".into())
}
fn short(message: String) -> String {
    message.chars().take(512).collect()
}
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub owner: Principal,
    pub subaccount: Option<Vec<u8>>,
}
#[derive(CandidType, Clone, Debug, PartialEq, Eq)]
pub struct WithdrawFromArgs {
    pub spender_subaccount: Option<Vec<u8>>,
    pub from: Account,
    pub to: Principal,
    pub amount: Nat,
    pub created_at_time: Option<u64>,
}
#[derive(CandidType, Deserialize, Debug)]
pub enum RejectionCode {
    NoError,
    CanisterError,
    SysTransient,
    DestinationInvalid,
    Unknown,
    SysFatal,
    CanisterReject,
}
#[derive(CandidType, Deserialize, Debug)]
pub enum WithdrawFromError {
    GenericError {
        message: String,
        error_code: Nat,
    },
    TemporarilyUnavailable,
    FailedToWithdrawFrom {
        withdraw_from_block: Option<Nat>,
        refund_block: Option<Nat>,
        approval_refund_block: Option<Nat>,
        rejection_code: RejectionCode,
        rejection_reason: String,
    },
    Duplicate {
        duplicate_of: Nat,
    },
    InvalidReceiver {
        receiver: Principal,
    },
    CreatedInFuture {
        ledger_time: u64,
    },
    TooOld,
    InsufficientFunds {
        balance: Nat,
    },
    InsufficientAllowance {
        allowance: Nat,
    },
}
fn definitely_unfunded(error: &WithdrawFromError) -> bool {
    matches!(
        error,
        WithdrawFromError::InsufficientFunds { .. }
            | WithdrawFromError::InsufficientAllowance { .. }
            | WithdrawFromError::InvalidReceiver { .. }
            | WithdrawFromError::CreatedInFuture { .. }
            | WithdrawFromError::TooOld
    )
}
#[derive(CandidType, Clone, Debug, PartialEq, Eq)]
pub struct DepositArgs {
    pub to: Account,
    pub memo: Option<Vec<u8>>,
}
#[derive(CandidType, Deserialize, Debug)]
pub struct DepositResult {
    pub balance: Nat,
    pub block_index: Nat,
}
#[derive(Clone, Debug)]
pub enum DepositOutcome {
    Queued(u64),
    Rejected(String),
    Unknown(String),
}

#[async_trait(?Send)]
pub trait Transport {
    fn now(&self) -> u64;
    fn balance(&self) -> u128;
    fn canister(&self) -> Principal;
    async fn fund(&self, args: WithdrawFromArgs) -> Result<Result<Nat, WithdrawFromError>, String>;
    async fn submit(&self, owner: Principal) -> (DepositOutcome, u128);
    async fn refund(
        &self,
        args: DepositArgs,
        cycles: u128,
    ) -> (Result<DepositResult, String>, u128);
}
pub struct IcTransport;
#[async_trait(?Send)]
impl Transport for IcTransport {
    fn now(&self) -> u64 {
        ic_cdk::api::time()
    }
    fn balance(&self) -> u128 {
        ic_cdk::api::canister_balance128()
    }
    fn canister(&self) -> Principal {
        ic_cdk::id()
    }
    async fn fund(&self, args: WithdrawFromArgs) -> Result<Result<Nat, WithdrawFromError>, String> {
        let result: Result<(Result<Nat, WithdrawFromError>,), _> =
            ic_cdk::call(ledger(), "withdraw_from", (args,)).await;
        result
            .map(|x| x.0)
            .map_err(|e| format!("Funding response unavailable: {e:?}"))
    }
    async fn submit(&self, owner: Principal) -> (DepositOutcome, u128) {
        crate::cksol::call_paid_minter(owner).await
    }
    async fn refund(
        &self,
        args: DepositArgs,
        cycles: u128,
    ) -> (Result<DepositResult, String>, u128) {
        let result: Result<(DepositResult,), _> =
            ic_cdk::api::call::call_with_payment128(ledger(), "deposit", (args,), cycles).await;
        // Read this callback's refund before any other asynchronous call.
        let refunded = ic_cdk::api::call::msg_cycles_refunded128();
        (
            result
                .map(|x| x.0)
                .map_err(|e| format!("Refund response unavailable: {e:?}")),
            refunded,
        )
    }
}
struct Guard(Principal);
impl Guard {
    fn acquire(owner: Principal) -> Result<Self, String> {
        if owner == Principal::anonymous() {
            return Err("Connect an ICP wallet first".into());
        }
        mutate_state(|s| {
            if s.cksol_relay_in_flight.contains(&owner) {
                return Err("ckSOL account already processing".into());
            }
            if s.cksol_relay_in_flight.len() >= CKSOL_RELAY_OWNER_CAP {
                return Err("ckSOL relay capacity reached".into());
            }
            s.cksol_relay_in_flight.insert(owner);
            Ok(Guard(owner))
        })
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        mutate_state(|s| {
            s.cksol_relay_in_flight.remove(&self.0);
        });
    }
}
fn liabilities(s: &State) -> u128 {
    s.cksol_paid_payments
        .values()
        .map(|r| r.refundable_cycles as u128)
        .sum()
}
fn has_backing(s: &State, balance: u128) -> bool {
    balance >= CKSOL_RELAY_CANISTER_RESERVE.saturating_add(liabilities(s))
}
fn prepare(s: &mut State, owner: Principal, sequence: u64, now: u64) -> Result<bool, String> {
    if let Some(existing) = s.cksol_paid_payments.get(&owner) {
        if sequence == existing.sequence {
            return Ok(false);
        }
        if !matches!(
            existing.phase,
            CkSolPaymentPhase::Complete | CkSolPaymentPhase::FundingRejected
        ) || existing.refundable_cycles != 0
        {
            return Err("Resolve the previous ckSOL payment before starting another".into());
        }
        if now <= existing.created_at_time {
            return Err("Wait for a fresh funding timestamp".into());
        }
    }
    // A global high-watermark makes settled receipt eviction safe: an old
    // sequence can never authorize another withdrawal, even after eviction.
    if s.cksol_paid_sequence.checked_add(1) != Some(sequence) {
        return Err("Refresh the ckSOL payment sequence".into());
    }
    if s.cksol_relay_attempts.contains_key(&owner) {
        return Err("Reconcile and acknowledge the prior ckSOL deposit first".into());
    }
    crate::cksol::reclaim_expired_rejection(s, now);
    if s.cksol_relay_attempts.len() >= CKSOL_RELAY_OWNER_CAP {
        return Err("ckSOL deposit owner capacity reached".into());
    }
    if !s.cksol_paid_payments.contains_key(&owner)
        && s.cksol_paid_payments.len() >= CKSOL_RELAY_OWNER_CAP
    {
        let settled = s
            .cksol_paid_payments
            .iter()
            .filter(|(_, r)| {
                r.refundable_cycles == 0
                    && matches!(
                        r.phase,
                        CkSolPaymentPhase::Complete | CkSolPaymentPhase::FundingRejected
                    )
            })
            .min_by_key(|(_, r)| r.sequence)
            .map(|(owner, _)| *owner);
        match settled {
            Some(owner) => {
                s.cksol_paid_payments.remove(&owner);
            }
            None => return Err("ckSOL payment owner capacity reached".into()),
        }
    }
    s.cksol_paid_sequence = sequence;
    s.cksol_paid_payments.insert(
        owner,
        CkSolPayment {
            sequence,
            created_at_time: now,
            phase: CkSolPaymentPhase::FundingPending,
            funding_block: None,
            deposit_id: None,
            refundable_cycles: 0,
            refund_block: None,
            message: None,
        },
    );
    Ok(true)
}
fn funding_result(
    s: &mut State,
    owner: Principal,
    result: Result<Result<Nat, WithdrawFromError>, String>,
) {
    let r = s
        .cksol_paid_payments
        .get_mut(&owner)
        .expect("journal reserved before funding");
    match result {
        Ok(Ok(block)) => {
            r.funding_block = Some(block);
            r.phase = CkSolPaymentPhase::Funded;
            r.refundable_cycles = CKSOL_RELAY_CALL_CYCLES as u64 + REFUND_FEE_RESERVE;
        }
        Ok(Err(error)) => {
            // A duplicate withdrawal block alone is NOT delivery proof. Never
            // use it to spend Rumi's cycles while delivery/refund is uncertain.
            r.phase = if definitely_unfunded(&error) {
                CkSolPaymentPhase::FundingRejected
            } else {
                CkSolPaymentPhase::FundingUnknown
            };
            r.message = Some(short(format!("Funding not confirmed: {error:?}")));
        }
        Err(message) => {
            r.phase = CkSolPaymentPhase::FundingUnknown;
            r.message = Some(short(message));
        }
    }
}
fn finish_deposit(s: &mut State, owner: Principal, outcome: DepositOutcome, refunded: u128) {
    let r = s
        .cksol_paid_payments
        .get_mut(&owner)
        .expect("paid deposit journal exists");
    if refunded > CKSOL_RELAY_CALL_CYCLES {
        r.phase = CkSolPaymentPhase::RefundUnknown;
        r.message = Some("Invalid minter refund; preserve this payment for recovery".into());
        return;
    }
    r.refundable_cycles = if refunded == 0 {
        0
    } else {
        refunded as u64 + REFUND_FEE_RESERVE
    };
    r.phase = if r.refundable_cycles == 0 {
        CkSolPaymentPhase::Complete
    } else {
        CkSolPaymentPhase::RefundReady
    };
    let status = match outcome {
        DepositOutcome::Queued(id) => {
            r.deposit_id = Some(id);
            CkSolRelayStatus::Queued(id)
        }
        DepositOutcome::Rejected(message) => {
            r.message = Some(short(message.clone()));
            CkSolRelayStatus::Rejected(short(message))
        }
        DepositOutcome::Unknown(message) => {
            r.message = Some(short(message));
            CkSolRelayStatus::SubmittedOrUnknown
        }
    };
    s.cksol_relay_attempts
        .get_mut(&owner)
        .expect("deposit reservation exists")
        .status = status;
}
fn refund_args(owner: Principal, sequence: u64, canister: Principal) -> DepositArgs {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(b"rumi-cksol-refund-v1");
    digest.update(canister.as_slice());
    digest.update(owner.as_slice());
    digest.update(sequence.to_be_bytes());
    DepositArgs {
        to: Account {
            owner,
            subaccount: None,
        },
        memo: Some(digest.finalize().to_vec()),
    }
}
async fn refund_with<T: Transport>(owner: Principal, t: &T) -> Result<CkSolPayment, String> {
    let before = row(owner)?;
    if before.phase != CkSolPaymentPhase::RefundReady {
        return Ok(before);
    }
    if !read_state(|s| t.balance() >= liabilities(s)) {
        return Err(
            "Refund retained: backend cannot currently cover all user cycle credits".into(),
        );
    }
    mutate_state(|s| {
        s.cksol_paid_payments.get_mut(&owner).unwrap().phase = CkSolPaymentPhase::RefundPending;
    });
    let amount = before.refundable_cycles;
    let (result, returned) = t
        .refund(
            refund_args(owner, before.sequence, t.canister()),
            amount as u128,
        )
        .await;
    mutate_state(|s| {
        let r = s.cksol_paid_payments.get_mut(&owner).unwrap();
        match result {
            Ok(receipt) if returned == 0 => {
                r.refund_block = Some(receipt.block_index);
                r.refundable_cycles = 0;
                r.phase = CkSolPaymentPhase::Complete;
            }
            Ok(_) => {
                r.phase = CkSolPaymentPhase::RefundUnknown;
                r.message = Some(
                    "Ledger accepted only part of the refund; reconcile before retrying".into(),
                );
            }
            Err(message) => {
                // deposit has no created_at_time/dedup: retry ONLY when this
                // callback proves the complete attachment was returned.
                r.phase = if returned == amount as u128 {
                    CkSolPaymentPhase::RefundReady
                } else {
                    CkSolPaymentPhase::RefundUnknown
                };
                r.message = Some(short(message));
            }
        }
    });
    row(owner)
}
pub async fn start_with<T: Transport>(
    owner: Principal,
    sequence: u64,
    max_payment: u64,
    t: &T,
) -> Result<CkSolPayment, String> {
    let _guard = Guard::acquire(owner)?;
    if max_payment != FUNDING {
        return Err("Payment terms changed; refresh the quote".into());
    }
    let fresh = mutate_state(|s| prepare(s, owner, sequence, t.now()))?;
    if fresh {
        let p = row(owner)?;
        let args = WithdrawFromArgs {
            spender_subaccount: None,
            from: Account {
                owner,
                subaccount: None,
            },
            to: t.canister(),
            amount: Nat::from(FUNDING),
            created_at_time: Some(p.created_at_time),
        };
        let result = t.fund(args).await;
        mutate_state(|s| funding_result(s, owner, result));
    }
    if row(owner)?.phase != CkSolPaymentPhase::Funded {
        return row(owner);
    }
    if !read_state(|s| has_backing(s, t.balance())) {
        return Err("Payment received; processing waits for the protected backend reserve. You may reclaim the payment.".into());
    }
    mutate_state(|s| -> Result<(), String> {
        // Other callers can fill the attempt map while funding is awaiting.
        // Reserve capacity atomically with dispatch, preserving a funded user's
        // credit for refund or resume when no slot remains.
        crate::cksol::reclaim_expired_rejection(s, t.now());
        if s.cksol_relay_attempts.len() >= CKSOL_RELAY_OWNER_CAP
            || s.cksol_relay_attempts.contains_key(&owner)
        {
            return Err(
                "Payment received; deposit capacity is full. Reclaim the payment or resume later."
                    .into(),
            );
        }
        let p = s.cksol_paid_payments.get_mut(&owner).unwrap();
        p.phase = CkSolPaymentPhase::DepositPending;
        s.cksol_relay_attempts.insert(
            owner,
            CkSolRelayAttempt {
                started_at_ns: t.now(),
                status: CkSolRelayStatus::SubmittedOrUnknown,
            },
        );
        Ok(())
    })?;
    let (outcome, refunded) = t.submit(owner).await;
    mutate_state(|s| finish_deposit(s, owner, outcome, refunded));
    refund_with(owner, t).await
}
pub async fn claim_with<T: Transport>(
    owner: Principal,
    sequence: u64,
    t: &T,
) -> Result<CkSolPayment, String> {
    let _guard = Guard::acquire(owner)?;
    let p = row(owner)?;
    if p.sequence != sequence {
        return Err("Refund sequence differs from current payment".into());
    }
    if p.phase == CkSolPaymentPhase::Funded {
        mutate_state(|s| {
            s.cksol_paid_payments.get_mut(&owner).unwrap().phase = CkSolPaymentPhase::RefundReady;
        });
    }
    refund_with(owner, t).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use std::cell::{Cell, RefCell};
    type FundingResult = Result<Result<Nat, WithdrawFromError>, String>;
    struct Mock {
        now: Cell<u64>,
        fill_capacity_during_funding: Cell<bool>,
        funds: RefCell<Vec<WithdrawFromArgs>>,
        sends: Cell<usize>,
        refunds: RefCell<Vec<(DepositArgs, u128)>>,
        funding: RefCell<Option<FundingResult>>,
        outcome: RefCell<Option<DepositOutcome>>,
        minter_refund: u128,
        refund_ok: Cell<bool>,
        refund_returned: Cell<u128>,
        balance: Cell<u128>,
    }
    impl Mock {
        fn new() -> Self {
            Self {
                now: Cell::new(100),
                fill_capacity_during_funding: Cell::new(false),
                funds: RefCell::new(vec![]),
                sends: Cell::new(0),
                refunds: RefCell::new(vec![]),
                funding: RefCell::new(Some(Ok(Ok(Nat::from(3u64))))),
                outcome: RefCell::new(Some(DepositOutcome::Queued(17))),
                minter_refund: 955_000_000_000,
                refund_ok: Cell::new(true),
                refund_returned: Cell::new(0),
                balance: Cell::new(100_000_000_000_000),
            }
        }
    }
    #[async_trait(?Send)]
    impl Transport for Mock {
        fn now(&self) -> u64 {
            self.now.get()
        }
        fn balance(&self) -> u128 {
            self.balance.get()
        }
        fn canister(&self) -> Principal {
            Principal::from_slice(&[9])
        }
        async fn fund(&self, a: WithdrawFromArgs) -> FundingResult {
            self.funds.borrow_mut().push(a);
            if self.fill_capacity_during_funding.get() {
                mutate_state(|s| {
                    s.cksol_relay_attempts.insert(
                        Principal::self_authenticating(b"last concurrent slot"),
                        CkSolRelayAttempt {
                            started_at_ns: 100,
                            status: CkSolRelayStatus::SubmittedOrUnknown,
                        },
                    );
                });
            }
            self.funding
                .borrow_mut()
                .take()
                .expect("only one funding effect")
        }
        async fn submit(&self, _: Principal) -> (DepositOutcome, u128) {
            self.sends.set(self.sends.get() + 1);
            (
                self.outcome.borrow_mut().take().expect("only one submit"),
                self.minter_refund,
            )
        }
        async fn refund(&self, a: DepositArgs, c: u128) -> (Result<DepositResult, String>, u128) {
            self.refunds.borrow_mut().push((a, c));
            (
                if self.refund_ok.get() {
                    Ok(DepositResult {
                        balance: Nat::from(c),
                        block_index: Nat::from(4u64),
                    })
                } else {
                    Err("reply unavailable".into())
                },
                self.refund_returned.get(),
            )
        }
    }
    fn owner() -> Principal {
        Principal::from_slice(&[7])
    }
    fn reset() {
        crate::state::replace_state(State::default());
    }
    #[test]
    fn paid_success_returns_unused_cycles_and_replay_never_charges_twice() {
        reset();
        let t = Mock::new();
        let p = block_on(start_with(owner(), 1, FUNDING, &t)).unwrap();
        assert_eq!(p.phase, CkSolPaymentPhase::Complete);
        assert_eq!(p.deposit_id, Some(17));
        assert_eq!(p.refund_block, Some(Nat::from(4u64)));
        assert_eq!(p.refundable_cycles, 0);
        let fund = &t.funds.borrow()[0];
        assert_eq!(fund.from.owner, owner());
        assert_eq!(fund.from.subaccount, None);
        assert_eq!(fund.to, t.canister());
        assert_eq!(fund.amount, Nat::from(FUNDING));
        assert_eq!(fund.created_at_time, Some(100));
        let refund = &t.refunds.borrow()[0];
        assert_eq!(refund.0.to.owner, owner());
        assert_eq!(refund.1, t.minter_refund + REFUND_FEE_RESERVE as u128);
        assert_eq!(read_state(|s| s.cksol_relay_budget), 0);
        assert_eq!(block_on(start_with(owner(), 1, FUNDING, &t)).unwrap(), p);
        assert_eq!(t.sends.get(), 1);
        assert_eq!(t.funds.borrow().len(), 1);
    }
    #[test]
    fn uncertain_funding_never_dispatches_or_recharges() {
        reset();
        let t = Mock::new();
        *t.funding.borrow_mut() = Some(Err("lost response".into()));
        assert_eq!(
            block_on(start_with(owner(), 1, FUNDING, &t)).unwrap().phase,
            CkSolPaymentPhase::FundingUnknown
        );
        block_on(start_with(owner(), 1, FUNDING, &t)).unwrap();
        assert!(block_on(start_with(owner(), 2, FUNDING, &t)).is_err());
        assert_eq!(t.sends.get(), 0);
        assert_eq!(t.funds.borrow().len(), 1);
        assert!(t.refunds.borrow().is_empty());
    }
    #[test]
    fn duplicate_withdrawal_is_not_proof_of_funding_delivery() {
        reset();
        let t = Mock::new();
        *t.funding.borrow_mut() = Some(Ok(Err(WithdrawFromError::Duplicate {
            duplicate_of: Nat::from(99u64),
        })));
        assert_eq!(
            block_on(start_with(owner(), 1, FUNDING, &t)).unwrap().phase,
            CkSolPaymentPhase::FundingUnknown
        );
        assert_eq!(t.sends.get(), 0);
    }
    #[test]
    fn insufficient_allowance_never_uses_rumi_cycles() {
        reset();
        let t = Mock::new();
        *t.funding.borrow_mut() = Some(Ok(Err(WithdrawFromError::InsufficientAllowance {
            allowance: Nat::from(0u64),
        })));
        assert_eq!(
            block_on(start_with(owner(), 1, FUNDING, &t)).unwrap().phase,
            CkSolPaymentPhase::FundingRejected
        );
        assert_eq!(t.sends.get(), 0);
    }
    #[test]
    fn lost_refund_with_no_returned_cycles_cannot_be_retried() {
        reset();
        let t = Mock::new();
        t.refund_ok.set(false);
        assert_eq!(
            block_on(start_with(owner(), 1, FUNDING, &t)).unwrap().phase,
            CkSolPaymentPhase::RefundUnknown
        );
        assert_eq!(
            block_on(claim_with(owner(), 1, &t)).unwrap().phase,
            CkSolPaymentPhase::RefundUnknown
        );
        assert_eq!(t.refunds.borrow().len(), 1);
        assert!(block_on(start_with(owner(), 2, FUNDING, &t)).is_err());
    }
    #[test]
    fn definite_full_refund_rejection_can_retry_without_charging_or_minting_again() {
        reset();
        let t = Mock::new();
        t.refund_ok.set(false);
        t.refund_returned
            .set(t.minter_refund + REFUND_FEE_RESERVE as u128);
        assert_eq!(
            block_on(start_with(owner(), 1, FUNDING, &t)).unwrap().phase,
            CkSolPaymentPhase::RefundReady
        );
        t.refund_ok.set(true);
        t.refund_returned.set(0);
        assert_eq!(
            block_on(claim_with(owner(), 1, &t)).unwrap().phase,
            CkSolPaymentPhase::Complete
        );
        assert_eq!(t.sends.get(), 1);
        assert_eq!(t.funds.borrow().len(), 1);
        assert_eq!(t.refunds.borrow().len(), 2);
        assert_eq!(t.refunds.borrow()[0], t.refunds.borrow()[1]);
    }
    #[test]
    fn unknown_minter_stays_locked_even_after_unused_cycles_return() {
        reset();
        let t = Mock::new();
        *t.outcome.borrow_mut() = Some(DepositOutcome::Unknown("minter reply lost".into()));
        let p = block_on(start_with(owner(), 1, FUNDING, &t)).unwrap();
        assert_eq!(p.phase, CkSolPaymentPhase::Complete);
        assert_eq!(p.deposit_id, None);
        assert_eq!(
            read_state(|s| s.cksol_relay_attempts[&owner()].status.clone()),
            CkSolRelayStatus::SubmittedOrUnknown
        );
        assert!(block_on(start_with(owner(), 2, FUNDING, &t)).is_err());
        assert_eq!(t.sends.get(), 1);
    }
    #[test]
    fn funded_user_can_reclaim_before_dispatch_if_reserve_is_low() {
        reset();
        let t = Mock::new();
        t.balance.set(1);
        assert!(block_on(start_with(owner(), 1, FUNDING, &t)).is_err());
        assert_eq!(row(owner()).unwrap().phase, CkSolPaymentPhase::Funded);
        assert_eq!(t.sends.get(), 0);
        t.balance
            .set(CKSOL_RELAY_CALL_CYCLES + REFUND_FEE_RESERVE as u128);
        assert_eq!(
            block_on(claim_with(owner(), 1, &t)).unwrap().phase,
            CkSolPaymentPhase::Complete
        );
        assert_eq!(
            t.refunds.borrow()[0].1,
            CKSOL_RELAY_CALL_CYCLES + REFUND_FEE_RESERVE as u128
        );
        assert_eq!(t.funds.borrow().len(), 1);
    }
    #[test]
    fn other_users_refundable_credit_is_reserved() {
        reset();
        let t = Mock::new();
        let mut p = CkSolPayment {
            sequence: 1,
            created_at_time: 1,
            phase: CkSolPaymentPhase::RefundReady,
            funding_block: None,
            deposit_id: None,
            refundable_cycles: 5_000_000_000_000,
            refund_block: None,
            message: None,
        };
        mutate_state(|s| {
            s.cksol_paid_payments
                .insert(Principal::from_slice(&[8]), p.clone());
        });
        t.balance.set(
            CKSOL_RELAY_CANISTER_RESERVE + CKSOL_RELAY_CALL_CYCLES + REFUND_FEE_RESERVE as u128,
        );
        assert!(block_on(start_with(owner(), 1, FUNDING, &t)).is_err());
        assert_eq!(t.sends.get(), 0);
        p.refundable_cycles = 0;
        assert!(!read_state(|s| has_backing(s, t.balance())));
    }
    #[test]
    fn anonymous_wrong_sequence_changed_terms_and_cross_owner_refund_are_rejected() {
        reset();
        let t = Mock::new();
        assert!(block_on(start_with(Principal::anonymous(), 1, FUNDING, &t)).is_err());
        assert!(block_on(start_with(owner(), 0, FUNDING, &t)).is_err());
        assert!(block_on(start_with(owner(), 1, FUNDING + 1, &t)).is_err());
        block_on(start_with(owner(), 1, FUNDING, &t)).unwrap();
        assert!(block_on(claim_with(Principal::from_slice(&[8]), 1, &t)).is_err());
        assert!(block_on(claim_with(owner(), 2, &t)).is_err());
    }
    #[test]
    fn upgrade_roundtrip_retains_journals_and_defaults_old_state() {
        reset();
        let t = Mock::new();
        t.refund_ok.set(false);
        block_on(start_with(owner(), 1, FUNDING, &t)).unwrap();
        mutate_state(|s| {
            s.cksol_relay_in_flight.insert(owner());
        });
        let mut bytes = vec![];
        read_state(|s| ciborium::ser::into_writer(s, &mut bytes)).unwrap();
        let s: State = ciborium::de::from_reader(bytes.as_slice()).unwrap();
        assert_eq!(
            s.cksol_paid_payments[&owner()].phase,
            CkSolPaymentPhase::RefundUnknown
        );
        assert!(s.cksol_relay_in_flight.is_empty());
        assert_eq!(s.cksol_paid_sequence, 1);
        let empty = std::collections::BTreeMap::<String, u8>::new();
        let mut bytes = vec![];
        ciborium::ser::into_writer(&empty, &mut bytes).unwrap();
        let s: State = ciborium::de::from_reader(bytes.as_slice()).unwrap();
        assert!(s.cksol_paid_payments.is_empty());
        assert_eq!(s.cksol_paid_sequence, 0);
    }
    #[test]
    fn unfunded_owners_do_not_exhaust_capacity_and_evicted_sequence_cannot_charge() {
        reset();
        let first = Principal::self_authenticating(&0u64.to_be_bytes());
        for n in 0..CKSOL_RELAY_OWNER_CAP + 5 {
            let t = Mock::new();
            *t.funding.borrow_mut() = Some(Ok(Err(WithdrawFromError::InsufficientFunds {
                balance: Nat::from(0u64),
            })));
            let user = Principal::self_authenticating(&(n as u64).to_be_bytes());
            let sequence = quote().next_sequence;
            assert_eq!(
                block_on(start_with(user, sequence, FUNDING, &t))
                    .unwrap()
                    .phase,
                CkSolPaymentPhase::FundingRejected
            );
        }
        assert!(payment(first).is_none());
        assert_eq!(
            read_state(|s| s.cksol_paid_payments.len()),
            CKSOL_RELAY_OWNER_CAP
        );
        let t = Mock::new();
        assert_eq!(
            block_on(start_with(first, 1, FUNDING, &t)).unwrap_err(),
            "Refresh the ckSOL payment sequence"
        );
        assert!(t.funds.borrow().is_empty());
        let sequence = quote().next_sequence;
        assert_eq!(
            block_on(start_with(owner(), sequence, FUNDING, &t))
                .unwrap()
                .phase,
            CkSolPaymentPhase::Complete
        );
        assert_eq!(t.funds.borrow().len(), 1);
    }
    #[test]
    fn settled_eviction_preserves_unknown_and_refundable_credits() {
        let mut s = State::default();
        for n in 0..CKSOL_RELAY_OWNER_CAP {
            let user = Principal::self_authenticating(&(n as u64).to_be_bytes());
            prepare(&mut s, user, n as u64 + 1, 100).unwrap();
            let r = s.cksol_paid_payments.get_mut(&user).unwrap();
            r.phase = if n == 0 {
                CkSolPaymentPhase::FundingUnknown
            } else if n == 1 {
                CkSolPaymentPhase::RefundReady
            } else {
                CkSolPaymentPhase::Complete
            };
            if n == 1 {
                r.refundable_cycles = 100;
            }
        }
        prepare(&mut s, owner(), CKSOL_RELAY_OWNER_CAP as u64 + 1, 101).unwrap();
        for n in 0..2u64 {
            assert!(s
                .cksol_paid_payments
                .contains_key(&Principal::self_authenticating(&n.to_be_bytes())));
        }
        assert!(!s
            .cksol_paid_payments
            .contains_key(&Principal::self_authenticating(&2u64.to_be_bytes())));
        assert_eq!(liabilities(&s), 100);
        crate::state::replace_state(s);
        assert_eq!(quote().next_sequence, CKSOL_RELAY_OWNER_CAP as u64 + 2);
    }
    #[test]
    fn expired_minter_rejections_reclaim_capacity_without_removing_paid_credit_or_unknowns() {
        reset();
        let t = Mock::new();
        let rejected = Principal::self_authenticating(&0u64.to_be_bytes());
        let unknown = Principal::self_authenticating(&1u64.to_be_bytes());
        let queued = Principal::self_authenticating(&2u64.to_be_bytes());
        mutate_state(|s| {
            for n in 0..CKSOL_RELAY_OWNER_CAP {
                s.cksol_relay_attempts.insert(
                    Principal::self_authenticating(&(n as u64).to_be_bytes()),
                    CkSolRelayAttempt {
                        started_at_ns: if n == 0 { 0 } else { 1 },
                        status: if n == 1 {
                            CkSolRelayStatus::SubmittedOrUnknown
                        } else if n == 2 {
                            CkSolRelayStatus::Queued(9)
                        } else {
                            CkSolRelayStatus::Rejected("typed rejection".into())
                        },
                    },
                );
            }
            let mut receipt = CkSolPayment {
                sequence: 1,
                created_at_time: 0,
                phase: CkSolPaymentPhase::RefundUnknown,
                funding_block: Some(Nat::from(3u64)),
                deposit_id: None,
                refundable_cycles: 123,
                refund_block: None,
                message: None,
            };
            s.cksol_paid_payments.insert(rejected, receipt.clone());
            receipt.phase = CkSolPaymentPhase::RefundReady;
            receipt.sequence = 2;
            s.cksol_paid_sequence = 2;
            s.cksol_paid_payments.insert(unknown, receipt);
        });
        // Before cooldown expires a fresh caller is rejected without funding.
        assert_eq!(
            block_on(start_with(owner(), 3, FUNDING, &t)).unwrap_err(),
            "ckSOL deposit owner capacity reached"
        );
        assert!(t.funds.borrow().is_empty());
        t.now.set(crate::cksol::CKSOL_REJECTED_RETRY_COOLDOWN_NS);
        let paid = block_on(start_with(owner(), 3, FUNDING, &t)).unwrap();
        assert!(!read_state(|s| s
            .cksol_relay_attempts
            .contains_key(&rejected)));
        assert!(read_state(|s| s
            .cksol_relay_attempts
            .contains_key(&unknown)));
        assert!(read_state(|s| s.cksol_relay_attempts.contains_key(&queued)));
        assert_eq!(read_state(liabilities), 246);
        assert_eq!(
            row(rejected).unwrap().phase,
            CkSolPaymentPhase::RefundUnknown
        );
        assert_eq!(row(unknown).unwrap().phase, CkSolPaymentPhase::RefundReady);
        assert_eq!(paid.phase, CkSolPaymentPhase::Complete);
        assert_eq!(t.sends.get(), 1);
        assert_eq!(t.funds.borrow().len(), 1);
    }
    #[test]
    fn capacity_filled_during_funding_retains_credit_without_dispatch_or_recharge() {
        reset();
        let t = Mock::new();
        mutate_state(|s| {
            for n in 0..CKSOL_RELAY_OWNER_CAP - 1 {
                s.cksol_relay_attempts.insert(
                    Principal::self_authenticating(&(n as u64).to_be_bytes()),
                    CkSolRelayAttempt {
                        started_at_ns: 100,
                        status: CkSolRelayStatus::Queued(n as u64),
                    },
                );
            }
        });
        t.fill_capacity_during_funding.set(true);
        assert!(block_on(start_with(owner(), 1, FUNDING, &t))
            .unwrap_err()
            .contains("capacity is full"));
        assert_eq!(row(owner()).unwrap().phase, CkSolPaymentPhase::Funded);
        assert_eq!(
            read_state(|s| s.cksol_relay_attempts.len()),
            CKSOL_RELAY_OWNER_CAP
        );
        assert_eq!(t.sends.get(), 0);
        assert!(block_on(start_with(owner(), 1, FUNDING, &t)).is_err());
        assert_eq!(t.funds.borrow().len(), 1);
        assert_eq!(
            block_on(claim_with(owner(), 1, &t)).unwrap().phase,
            CkSolPaymentPhase::Complete
        );
        assert_eq!(
            t.refunds.borrow()[0].1,
            CKSOL_RELAY_CALL_CYCLES + REFUND_FEE_RESERVE as u128
        );
        assert_eq!(
            read_state(|s| s.cksol_relay_attempts.len()),
            CKSOL_RELAY_OWNER_CAP
        );
    }
    #[test]
    fn single_flight_and_owner_cap_preserve_existing_receipts() {
        reset();
        let t = Mock::new();
        let guard = Guard::acquire(owner()).unwrap();
        assert!(block_on(start_with(owner(), 1, FUNDING, &t)).is_err());
        drop(guard);
        mutate_state(|s| {
            for n in 0..CKSOL_RELAY_OWNER_CAP {
                let p = Principal::self_authenticating(&(n as u64).to_be_bytes());
                prepare(s, p, n as u64 + 1, 100).unwrap();
            }
        });
        assert_eq!(
            block_on(start_with(owner(), quote().next_sequence, FUNDING, &t)).unwrap_err(),
            "ckSOL payment owner capacity reached"
        );
        assert_eq!(t.funds.borrow().len(), 0);
    }
}

#[cfg(test)]
mod policy_tests {
    use super::*;
    #[test]
    fn legacy_sponsorship_cannot_be_reenabled() {
        let owner = Principal::from_slice(&[7]);
        let mut state = State::default();
        state.developer_principal = owner;
        state.cksol_relay_budget = 10_000_000_000_000;
        crate::state::replace_state(state);
        assert_eq!(crate::cksol::budget(), 0);
        assert!(crate::cksol::set_budget(owner, 1).is_err());
        crate::cksol::set_budget(owner, 0).unwrap();
        assert!(futures::executor::block_on(crate::cksol::start(owner)).is_err());
        assert!(read_state(|s| s.cksol_relay_attempts.is_empty()));
    }
    #[test]
    fn every_pending_effect_survives_the_actual_upgrade_codec() {
        for phase in [
            CkSolPaymentPhase::FundingPending,
            CkSolPaymentPhase::FundingUnknown,
            CkSolPaymentPhase::Funded,
            CkSolPaymentPhase::DepositPending,
            CkSolPaymentPhase::RefundPending,
            CkSolPaymentPhase::RefundUnknown,
        ] {
            let owner = Principal::from_slice(&[7]);
            let mut s = State::default();
            let row = CkSolPayment {
                sequence: 99,
                created_at_time: 123,
                phase,
                funding_block: Some(Nat::from(11u64)),
                deposit_id: Some(17),
                refundable_cycles: 1234,
                refund_block: None,
                message: Some("retained".into()),
            };
            s.cksol_paid_payments.insert(owner, row.clone());
            s.cksol_relay_in_flight.insert(owner);
            let mut b = vec![];
            ciborium::ser::into_writer(&s, &mut b).unwrap();
            let decoded = crate::storage::decode_state_body(&b).unwrap();
            assert_eq!(decoded.cksol_paid_payments[&owner], row);
            assert!(decoded.cksol_relay_in_flight.is_empty());
        }
    }
}
