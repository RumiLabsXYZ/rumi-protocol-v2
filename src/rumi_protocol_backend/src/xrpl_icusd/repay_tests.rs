//! Task 5 tests: XRPL repayment, receipt ingestion, held claims and the observer,
//! against a FULL `State` (installed as the thread-local state so the legacy accrual,
//! guards, event log and replay are the real ones). XRPL and ledger outcomes are
//! injected exactly as in `tests.rs` / `borrow_tests.rs`.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};

use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use super::borrow_tests::{debt_of, world, xrp, E8, NOW};
use super::ledger::*;
use super::observer::{self, ObserverTransport, PassOutcome};
use super::repay::{self, IntentRequest, RepayAdvance, ReturnOutcome, SettleOutcome};
use super::settlement::*;
use super::state::CoverageInput;
use super::tests::{backend, holder_addr, issuer_addr, owner, TESTNET};
use super::types::*;
use crate::chains::xrp::issued_amount::{ExactIssuedAmount, IssuedAsset};
use crate::chains::xrp::issued_rpc::{
    IssuedRpcError, IssuerHistoryCursor, IssuerHistoryPage, ParsedTx, RawAmount, ReceiptError,
    ReceiptLookup, TrustLineBalanceChange, VerifiedIssuedReceipt, TF_PARTIAL_PAYMENT,
};
use crate::chains::xrp::network::XrplNetwork;
use crate::numeric::{Ratio, ICUSD};
use crate::state::{mutate_state, read_state, replace_state, State};

const YEAR_NS: u64 = 365 * 24 * 3_600 * 1_000_000_000;

fn payer() -> String {
    holder_addr(5)
}
fn other() -> String {
    holder_addr(6)
}

// ---- fixtures -----------------------------------------------------------------------

fn set_vault(vault: u64, debt: u64, accrued: u64) {
    mutate_state(|s| {
        let v = s.vault_id_to_vaults.get_mut(&vault).unwrap();
        v.borrowed_icusd_amount = ICUSD::new(debt);
        v.accrued_interest = ICUSD::new(accrued);
        v.last_accrual_time = NOW;
    });
}

/// A full state with vault 1 carrying `debt`/`accrued`, `outstanding` XRPL icUSD in
/// circulation, and an XRP interest rate of `apr`.
fn rworld(debt: u64, accrued: u64, outstanding: u64, apr: Decimal) -> State {
    let mut w = world();
    w.xrpl_icusd.issued_outstanding = WideAmount::from_e8s(outstanding);
    if let Some(c) = w.collateral_configs.get_mut(&xrp()) {
        c.interest_rate_apr = Ratio::from(apr);
    }
    {
        let v = w.vault_id_to_vaults.get_mut(&1).unwrap();
        v.borrowed_icusd_amount = ICUSD::new(debt);
        v.accrued_interest = ICUSD::new(accrued);
        v.last_accrual_time = NOW;
    }
    w
}

fn intent_req(vault: u64, max: u64, payer_addr: &str, nonce: u8, full: bool, top_up: Option<u64>) -> IntentRequest {
    IntentRequest {
        owner: owner(1),
        vault_id: vault,
        max_e8s: max,
        payer: payer_addr.to_string(),
        network: TESTNET,
        request_nonce: [nonce; 32],
        full_close: full,
        top_up,
    }
}

fn try_intent(req: &IntentRequest) -> Result<OperationId, XrplIcusdError> {
    mutate_state(|s| repay::create_intent(s, req, NOW)).map(|(id, _)| id)
}

fn intent(vault: u64, max: u64, nonce: u8, full: bool) -> OperationId {
    try_intent(&intent_req(vault, max, &payer(), nonce, full, None)).expect("intent")
}

fn tag_of(id: OperationId) -> u32 {
    read_state(|s| match &s.xrpl_icusd.op(id).unwrap().kind {
        OperationKind::Repayment { destination_tag, .. } => *destination_tag,
        k => panic!("{k:?}"),
    })
}

fn hash_of(n: u8) -> String {
    format!("{n:02X}").repeat(32)
}

/// A validated, direct, non-partial return of `amount` icUSD from `sender` to the issuer.
fn ret(n: u8, sender: &str, tag: Option<u32>, amount: &str) -> VerifiedIssuedReceipt {
    let a = ExactIssuedAmount::parse(amount).unwrap();
    let before = ExactIssuedAmount::parse("1000").unwrap();
    VerifiedIssuedReceipt {
        hash: hash_of(n),
        ledger_index: 1_500 + u32::from(n),
        ledger_hash: "B".repeat(64),
        validated: true,
        engine_result: "tesSUCCESS".into(),
        account: sender.to_string(),
        destination: issuer_addr(),
        destination_tag: tag,
        flags: 0,
        nominal_amount: Some(a),
        delivered_amount: a,
        has_paths: false,
        has_send_max: false,
        balance_changes: vec![TrustLineBalanceChange {
            low_account: sender.to_string(),
            high_account: issuer_addr(),
            previous_low_balance: before,
            final_low_balance: before.checked_sub(&a).unwrap(),
        }],
        provider_evidence: vec![],
    }
}

fn apply_ret(r: &VerifiedIssuedReceipt) -> ReturnOutcome {
    mutate_state(|s| repay::apply_verified_return(&mut s.xrpl_icusd, TESTNET, &issuer_addr(), r, NOW))
        .expect("apply_verified_return")
}

fn settle_at(id: OperationId, now: u64) -> SettleOutcome {
    mutate_state(|s| repay::settle_return_against_debt(s, id, 100, backend(), now)).expect("settle")
}

fn settle(id: OperationId) -> SettleOutcome {
    settle_at(id, NOW)
}

fn op_phase(id: OperationId) -> Phase {
    read_state(|s| s.xrpl_icusd.op(id).unwrap().phase)
}

fn held_of(id: OperationId) -> WideAmount {
    read_state(|s| match &s.xrpl_icusd.op(id).unwrap().kind {
        OperationKind::Repayment { held, .. } => *held,
        OperationKind::Refund { amount, .. } => WideAmount::from_exact(amount).unwrap(),
        k => panic!("{k:?}"),
    })
}

fn applied_of(id: OperationId) -> u64 {
    read_state(|s| match &s.xrpl_icusd.op(id).unwrap().kind {
        OperationKind::Repayment { applied_e8s, .. } => *applied_e8s,
        k => panic!("{k:?}"),
    })
}

fn burn_idx(id: OperationId) -> usize {
    read_state(|s| {
        s.xrpl_icusd
            .op(id)
            .unwrap()
            .ic_jobs
            .iter()
            .rposition(|j| j.purpose == IcJobPurpose::EscrowBurn)
            .expect("an escrow burn job")
    })
}

fn no_burn(id: OperationId) {
    read_state(|s| {
        assert!(
            !s.xrpl_icusd
                .op(id)
                .unwrap()
                .ic_jobs
                .iter()
                .any(|j| j.purpose == IcJobPurpose::EscrowBurn),
            "no burn job may exist"
        )
    });
}

fn prove_burn(id: OperationId, block: u64) {
    let idx = burn_idx(id);
    mutate_state(|s| {
        s.xrpl_icusd.begin_job_call(id, idx).unwrap();
        s.xrpl_icusd
            .finish_job_call(id, idx, LedgerCallResult::Ok(block), NOW + 1)
            .unwrap();
        s.xrpl_icusd.mark_proof_verified(id, idx).unwrap();
    });
}

fn fail_burn(id: OperationId) {
    let idx = burn_idx(id);
    mutate_state(|s| {
        s.xrpl_icusd.begin_job_call(id, idx).unwrap();
        s.xrpl_icusd
            .finish_job_call(id, idx, LedgerCallResult::Rejected("insufficient funds".into()), NOW + 1)
            .unwrap();
    });
}

fn unknown_burn(id: OperationId) {
    let idx = burn_idx(id);
    mutate_state(|s| {
        s.xrpl_icusd.begin_job_call(id, idx).unwrap();
        s.xrpl_icusd
            .finish_job_call(id, idx, LedgerCallResult::Unknown("transport".into()), NOW + 1)
            .unwrap();
    });
}

fn advance_at(id: OperationId, now: u64) -> RepayAdvance {
    mutate_state(|s| repay::advance_repayment(s, id, now)).expect("advance")
}

fn vault_of(v: u64) -> (u64, u64, u64) {
    read_state(|s| {
        let x = &s.vault_id_to_vaults[&v];
        (
            x.borrowed_icusd_amount.to_u64(),
            x.accrued_interest.to_u64(),
            x.last_accrual_time,
        )
    })
}

/// `O + D + R + H` in whole e8s (ceil), the disjoint liability sum.
fn liabilities() -> u128 {
    read_state(|s| {
        let b = s.xrpl_icusd.buckets().unwrap();
        let mut sum = b.o;
        for v in [&b.d, &b.r, &b.h] {
            sum = sum.checked_add(v).unwrap();
        }
        sum.ceil_e8s()
    })
}

fn upgrade_roundtrip() {
    let bytes = read_state(|s| {
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(s, &mut bytes).unwrap();
        bytes
    });
    let restored: State = ciborium::de::from_reader(bytes.as_slice()).unwrap();
    replace_state(restored);
    mutate_state(|s| s.xrpl_icusd.recover_after_upgrade());
}

fn count_events(f: impl Fn(&XrplIcusdEvent) -> bool) -> usize {
    crate::storage::events()
        .filter(|e| matches!(e, crate::event::Event::XrplIcusd(x) if f(x)))
        .count()
}

/// A repayment op injected directly in `ReturnVerified` (for vaults the intent
/// endpoint would refuse, e.g. the replay test on the ICP vault).
fn inject_verified(vault: u64, held_e8s: u64, authorized: u64, full: bool) -> OperationId {
    mutate_state(|s| {
        let st = &mut s.xrpl_icusd;
        let id = st.allocate_operation_id();
        let tag = st.allocate_destination_tag().unwrap();
        let op = Operation {
            id,
            owner: owner(2),
            created_at_ns: NOW,
            updated_at_ns: NOW,
            request_nonce: [id as u8 + 90; 32],
            network: TESTNET,
            kind: OperationKind::Repayment {
                vault_id: vault,
                payer: payer(),
                destination_tag: tag,
                max_e8s: authorized,
                full_close: full,
                parent: None,
                receipts: vec![VerifiedReceiptRef {
                    tx_hash: hash_of(200),
                    ledger_index: 1_600,
                    delivered: ExactIssuedAmount::try_from_e8s(held_e8s).unwrap(),
                    applied_e8s: 0,
                    authorized_e8s: authorized,
                }],
                cutoff: None,
                applied_e8s: 0,
                held: WideAmount::from_e8s(held_e8s),
                children: vec![],
                pending_interest: None,
            },
            phase: Phase::ReturnVerified,
            ic_jobs: vec![],
            xrpl_attempts: vec![],
            notes: vec![],
            resolution: None,
            attempt_budget_base: 0,
            delivery_halted: false,
        };
        st.insert_operation(op).unwrap();
        st.tag_index.insert(tag, id);
        id
    })
}

// ---- injected XRPL transport --------------------------------------------------------

#[derive(Default)]
struct Mock {
    tips: RefCell<VecDeque<Result<u32, IssuedRpcError>>>,
    pages: RefCell<VecDeque<Result<IssuerHistoryPage, IssuedRpcError>>>,
    receipts: RefCell<BTreeMap<String, Result<ReceiptLookup, IssuedRpcError>>>,
    tip_calls: Cell<u32>,
    history_calls: Cell<u32>,
    receipt_calls: Cell<u32>,
    last_limit: Cell<u32>,
    last_cursor: RefCell<Option<IssuerHistoryCursor>>,
}

impl Mock {
    fn total_calls(&self) -> u32 {
        self.tip_calls.get() + self.history_calls.get() + self.receipt_calls.get()
    }
    fn with_receipt(self, r: &VerifiedIssuedReceipt) -> Self {
        self.receipts
            .borrow_mut()
            .insert(r.hash.clone(), Ok(ReceiptLookup::Verified(Box::new(r.clone()))));
        self
    }
    fn with_tip(self, tip: u32) -> Self {
        self.tips.borrow_mut().push_back(Ok(tip));
        self
    }
    fn with_page(self, p: IssuerHistoryPage) -> Self {
        self.pages.borrow_mut().push_back(Ok(p));
        self
    }
}

impl ObserverTransport for Mock {
    async fn validated_tip(&self, _n: XrplNetwork, _i: &str) -> Result<u32, IssuedRpcError> {
        self.tip_calls.set(self.tip_calls.get() + 1);
        self.tips
            .borrow_mut()
            .pop_front()
            .unwrap_or(Err(IssuedRpcError::Transport("no tip scripted".into())))
    }
    async fn history(
        &self,
        _n: XrplNetwork,
        _i: &str,
        cursor: &IssuerHistoryCursor,
        limit: u32,
    ) -> Result<IssuerHistoryPage, IssuedRpcError> {
        self.history_calls.set(self.history_calls.get() + 1);
        self.last_limit.set(limit);
        *self.last_cursor.borrow_mut() = Some(cursor.clone());
        self.pages
            .borrow_mut()
            .pop_front()
            .unwrap_or(Err(IssuedRpcError::Transport("no page scripted".into())))
    }
    async fn receipt(
        &self,
        _n: XrplNetwork,
        tx_hash: &str,
        _a: &IssuedAsset,
    ) -> Result<ReceiptLookup, IssuedRpcError> {
        self.receipt_calls.set(self.receipt_calls.get() + 1);
        self.receipts
            .borrow()
            .get(&tx_hash.to_ascii_uppercase())
            .cloned()
            .unwrap_or(Ok(ReceiptLookup::NotFound))
    }
}

fn parsed(r: &VerifiedIssuedReceipt) -> ParsedTx {
    let issued = |a: &ExactIssuedAmount| RawAmount::Issued {
        currency: crate::chains::xrp::issued_amount::ICUSD_CURRENCY_HEX.to_string(),
        issuer: issuer_addr(),
        value: a.to_decimal_string(),
    };
    ParsedTx {
        hash: r.hash.clone(),
        ledger_index: r.ledger_index,
        ledger_hash: Some(r.ledger_hash.clone()),
        validated: true,
        tx_type: "Payment".into(),
        account: r.account.clone(),
        destination: Some(r.destination.clone()),
        destination_tag: r.destination_tag,
        flags: r.flags,
        engine_result: "tesSUCCESS".into(),
        amount: issued(&r.nominal_amount.unwrap_or(r.delivered_amount)),
        delivered: issued(&r.delivered_amount),
        has_paths: false,
        has_send_max: false,
        line_changes: vec![],
    }
}

fn page(txs: Vec<ParsedTx>, marker: Option<&str>, min: u32, max: u32) -> IssuerHistoryPage {
    IssuerHistoryPage {
        txs,
        next_marker: marker.map(str::to_string),
        interval_complete: marker.is_none(),
        fully_consumed_through: if marker.is_none() { Some(max) } else { None },
        ledger_min: min,
        ledger_max: max,
        provider_evidence: vec![],
    }
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    futures::executor::block_on(f)
}

fn pass(m: &Mock) -> PassOutcome {
    block_on(observer::observer_pass_with(m, NOW + 5))
}

fn hint(m: &Mock, caller: Principal, id: OperationId, n: u8) -> Result<OperationView, XrplIcusdError> {
    block_on(observer::observe_with(
        m,
        caller,
        &ObserveRepaymentArg {
            operation_id: id,
            tx_hash: hash_of(n),
        },
        NOW + 5,
    ))
    .map(|(v, _)| v)
}

fn processed_count() -> usize {
    read_state(|s| s.xrpl_icusd.processed_returns.len())
}

/// The aggregate of unattributed value from `sender` (zero when none).
fn agg_total(sender: &str) -> WideAmount {
    read_state(|s| {
        s.xrpl_icusd
            .unattributed
            .get(&super::retention::aggregate_key(TESTNET, sender))
            .map(|u| u.total)
            .unwrap_or(WideAmount::ZERO)
    })
}

/// Cut refund operations from aggregates (what the tick does).
fn drain() -> Vec<OperationId> {
    mutate_state(|s| s.xrpl_icusd.drain_refunds(NOW))
}

fn refund_ops() -> Vec<OperationId> {
    read_state(|s| {
        s.xrpl_icusd
            .operations
            .values()
            .filter(|o| matches!(o.kind, OperationKind::Refund { .. }))
            .map(|o| o.id)
            .collect()
    })
}

// ---- intent -----------------------------------------------------------------------

#[test]
fn intent_is_owner_bound_with_exact_payment_and_unique_persisted_tags() {
    replace_state(rworld(100 * E8, 0, 0, Decimal::ZERO));
    // Another principal cannot prepare against the caller's vault.
    let mut foreign = intent_req(1, 50 * E8, &payer(), 1, false, None);
    foreign.owner = owner(2);
    assert_eq!(try_intent(&foreign), Err(XrplIcusdError::Unauthorized));
    let a = intent(1, 50 * E8, 1, false);
    let b = intent(1, 60 * E8, 2, false);
    let (ta, tb) = (tag_of(a), tag_of(b));
    assert!(ta >= 1 && tb == ta + 1, "monotonic, never zero");
    // The same nonce is the same operation and never a second tag.
    assert_eq!(intent(1, 50 * E8, 1, false), a);
    assert_eq!(read_state(|s| s.xrpl_icusd.next_destination_tag), tb + 1);
    // A different body under the same nonce conflicts.
    assert_eq!(
        try_intent(&intent_req(1, 51 * E8, &payer(), 1, false, None)),
        Err(XrplIcusdError::NonceConflict)
    );
    // The wallet payload is exactly an issued Payment to the issuer with the tag.
    let v = read_state(|s| repay::intent_view(&s.xrpl_icusd, a, NOW)).unwrap();
    assert_eq!(v.issuer, issuer_addr());
    assert_eq!(v.currency, "6963555344000000000000000000000000000000");
    assert_eq!((v.destination_tag, v.max_e8s, v.payer.clone()), (ta, 50 * E8, payer()));
    let j: serde_json::Value = serde_json::from_str(&v.payment_json).unwrap();
    assert_eq!(j["TransactionType"], "Payment");
    assert_eq!(j["Account"], payer());
    assert_eq!(j["Destination"], issuer_addr());
    assert_eq!(j["DestinationTag"], ta);
    assert_eq!(j["Amount"]["value"], "50");
    assert_eq!(j["Amount"]["issuer"], issuer_addr());
    assert_eq!(j["Amount"]["currency"], "6963555344000000000000000000000000000000");
    assert!(j.get("Paths").is_none() && j.get("SendMax").is_none() && j.get("Flags").is_none());
    // Tags survive an upgrade and are never reassigned.
    upgrade_roundtrip();
    let c = intent(1, 70 * E8, 3, false);
    assert_eq!(tag_of(c), tb + 1);
    read_state(|s| assert_eq!(s.xrpl_icusd.tag_index.get(&ta), Some(&a)));
    // 10 active operations per owner.
    for n in 10..17u8 {
        intent(1, E8, n, false);
    }
    assert!(matches!(
        try_intent(&intent_req(1, E8, &payer(), 99, false, None)),
        Err(XrplIcusdError::LimitExceeded(_))
    ));
}

#[test]
fn intent_validation_rejects_bad_requests() {
    replace_state(rworld(100 * E8, 0, 0, Decimal::ZERO));
    let bad = |r: IntentRequest| try_intent(&r);
    assert!(matches!(bad(intent_req(1, 0, &payer(), 1, false, None)), Err(XrplIcusdError::InvalidArgument(_))));
    assert!(matches!(bad(intent_req(1, E8, "not-an-address", 2, false, None)), Err(XrplIcusdError::InvalidArgument(_))));
    assert!(matches!(bad(intent_req(1, E8, &issuer_addr(), 3, false, None)), Err(XrplIcusdError::InvalidArgument(_))));
    assert_eq!(bad(intent_req(77, E8, &payer(), 4, false, None)), Err(XrplIcusdError::NotFound));
    // Repayment is limited to native-XRP vaults in v1, like borrowing.
    let mut icp = intent_req(2, E8, &payer(), 5, false, None);
    icp.owner = owner(2);
    assert!(matches!(bad(icp), Err(XrplIcusdError::NotApplicable(_))));
    let mut wrong_net = intent_req(1, E8, &payer(), 6, false, None);
    wrong_net.network = XrplNetwork::Mainnet;
    assert!(matches!(bad(wrong_net), Err(XrplIcusdError::WrongNetwork(_))));
    // No debt, nothing to repay.
    set_vault(1, 0, 0);
    assert!(matches!(bad(intent_req(1, E8, &payer(), 7, false, None)), Err(XrplIcusdError::InvalidArgument(_))));
}

// ---- hint + scan ------------------------------------------------------------------

#[test]
fn same_return_found_by_hint_and_scan_applies_once() {
    replace_state(rworld(100 * E8, 0, 200 * E8, Decimal::ZERO));
    let id = intent(1, 100 * E8, 1, false);
    let r = ret(7, &payer(), Some(tag_of(id)), "100");
    let m = Mock::default()
        .with_receipt(&r)
        .with_tip(2_000)
        .with_page(page(vec![parsed(&r)], None, 1, 2_000));
    // The wallet hint reaches the same consumer first.
    let v = hint(&m, owner(1), id, 7).unwrap();
    assert_eq!(v.phase, Phase::ReturnVerified);
    assert_eq!(m.receipt_calls.get(), 1);
    // The scan finds the same transaction: consumed already, no second receipt read.
    let out = pass(&m);
    assert!(matches!(out, PassOutcome::Idle), "the intent is no longer open: {out:?}");
    // Re-open an intent so the scan runs, and re-offer the SAME tx in a fresh page.
    let id2 = intent(1, 5 * E8, 2, false);
    let m2 = Mock::default()
        .with_tip(2_000)
        .with_page(page(vec![parsed(&r)], None, 1, 2_000));
    let out = pass(&m2);
    assert!(matches!(out, PassOutcome::Progressed { .. }), "{out:?}");
    assert_eq!(m2.receipt_calls.get(), 0, "an already-consumed return is not re-read");
    assert_eq!(processed_count(), 1);
    read_state(|s| {
        assert_eq!(s.xrpl_icusd.issued_outstanding, WideAmount::from_e8s(100 * E8), "O reduced once");
        match &s.xrpl_icusd.op(id).unwrap().kind {
            OperationKind::Repayment { receipts, held, .. } => {
                assert_eq!(receipts.len(), 1);
                assert_eq!(*held, WideAmount::from_e8s(100 * E8));
            }
            k => panic!("{k:?}"),
        }
        assert_eq!(s.xrpl_icusd.op(id2).unwrap().phase, Phase::IntentPrepared);
    });
    // Scan first, hint second, in the other order: still once.
    let r2 = ret(8, &payer(), Some(tag_of(id2)), "5");
    let m3 = Mock::default()
        .with_receipt(&r2)
        .with_tip(2_100)
        .with_page(page(vec![parsed(&r2)], None, 2_001, 2_100));
    let id3 = intent(1, 6 * E8, 3, false);
    assert!(matches!(pass(&m3), PassOutcome::Progressed { .. }));
    let v = hint(&m3, owner(1), id2, 8).unwrap();
    assert_eq!(v.phase, Phase::ReturnVerified);
    assert_eq!(m3.receipt_calls.get(), 1, "the hint did not re-read a consumed return");
    assert_eq!(processed_count(), 2);
    assert_eq!(held_of(id2), WideAmount::from_e8s(5 * E8));
    assert_eq!(op_phase(id3), Phase::IntentPrepared);
}

#[test]
fn hint_is_owner_only_and_throttled() {
    replace_state(rworld(100 * E8, 0, 100 * E8, Decimal::ZERO));
    let id = intent(1, 10 * E8, 1, false);
    let r = ret(9, &payer(), Some(tag_of(id)), "10");
    let m = Mock::default().with_receipt(&r);
    assert_eq!(hint(&m, owner(2), id, 9).unwrap_err(), XrplIcusdError::NotFound);
    assert_eq!(hint(&m, Principal::anonymous(), id, 9).unwrap_err(), XrplIcusdError::AnonymousCaller);
    assert_eq!(m.receipt_calls.get(), 0, "no outcall for a stranger");
    // An unknown hash is only a lookup key; the backend finds nothing and credits nothing.
    let v = hint(&m, owner(1), id, 55).unwrap();
    assert_eq!(v.phase, Phase::IntentPrepared);
    assert_eq!(m.receipt_calls.get(), 1);
    // A second hint inside the gap costs no outcall.
    let _ = hint(&m, owner(1), id, 9).unwrap();
    assert_eq!(m.receipt_calls.get(), 1, "throttled");
}

// ---- verification -------------------------------------------------------------------

#[test]
fn wrong_currency_issuer_tag_payer_not_applied() {
    replace_state(rworld(100 * E8, 0, 300 * E8, Decimal::ZERO));
    let id = intent(1, 100 * E8, 1, false);
    let tag = tag_of(id);
    let asset = IssuedAsset::icusd(&issuer_addr()).unwrap();
    // Wrong currency / wrong issuer: discovered by the scan, never credited, evidence kept.
    let good = ret(1, &payer(), Some(tag), "50");
    let mut wrong_currency = parsed(&good);
    wrong_currency.hash = hash_of(2);
    wrong_currency.delivered = RawAmount::Issued {
        currency: "USD".into(),
        issuer: issuer_addr(),
        value: "50".into(),
    };
    let mut wrong_issuer = parsed(&good);
    wrong_issuer.hash = hash_of(3);
    wrong_issuer.delivered = RawAmount::Issued {
        currency: crate::chains::xrp::issued_amount::ICUSD_CURRENCY_HEX.to_string(),
        issuer: holder_addr(77),
        value: "50".into(),
    };
    let pg = page(vec![wrong_currency, wrong_issuer], None, 1, 100);
    let plan = read_state(|s| observer::plan_page(&s.xrpl_icusd, TESTNET, &issuer_addr(), &asset, &pg));
    assert!(plan.to_verify.is_empty(), "no receipt is read for a foreign asset");
    assert_eq!(plan.foreign_asset.len(), 2);
    let m = Mock::default().with_tip(100).with_page(pg);
    assert!(matches!(pass(&m), PassOutcome::Progressed { .. }));
    read_state(|s| {
        assert_eq!(s.xrpl_icusd.issued_outstanding, WideAmount::from_e8s(300 * E8), "O untouched");
        assert!(s.xrpl_icusd.processed_returns.is_empty());
        assert_eq!(s.xrpl_icusd.quarantine_log.len(), 2, "evidence recorded");
        assert_eq!(s.xrpl_icusd.op(id).unwrap().phase, Phase::IntentPrepared);
    });
    // The hint path: a receipt for another asset is a typed error, quarantined.
    let m2 = Mock::default();
    m2.receipts.borrow_mut().insert(
        hash_of(4),
        Err(IssuedRpcError::Receipt(ReceiptError::WrongAsset("USD".into()))),
    );
    let v = hint(&m2, owner(1), id, 4).unwrap();
    assert_eq!(v.phase, Phase::IntentPrepared);
    assert_eq!(read_state(|s| s.xrpl_icusd.quarantine_log.len()), 3);
    // Wrong tag: a valid return nobody can claim. O falls (the token left circulation),
    // the intent gets NO credit and the value is a refund for the verified sender.
    let unknown = ret(5, &payer(), Some(tag + 1000), "10");
    assert_eq!(apply_ret(&unknown), ReturnOutcome::Unattributed);
    assert_eq!(agg_total(&payer()), WideAmount::from_e8s(10 * E8), "aggregated for the verified sender");
    // Wrong payer: the tag is right but the SENDER is not the intent's payer.
    let wrong_payer = ret(6, &other(), Some(tag), "20");
    assert_eq!(apply_ret(&wrong_payer), ReturnOutcome::Unattributed);
    assert_eq!(agg_total(&other()), WideAmount::from_e8s(20 * E8));
    // No tag at all.
    assert_eq!(apply_ret(&ret(10, &payer(), None, "1")), ReturnOutcome::Unattributed);
    read_state(|s| {
        assert_eq!(s.xrpl_icusd.op(id).unwrap().phase, Phase::IntentPrepared, "intent untouched");
        assert_eq!(s.xrpl_icusd.issued_outstanding, WideAmount::from_e8s((300 - 10 - 20 - 1) * E8));
    });
    assert_eq!(settle(id), SettleOutcome::NotApplicable);
    assert_eq!(debt_of(1), 100 * E8, "no debt credit");
    // Bad evidence. A receipt that was NOT a value-bearing return (failed, wrong
    // destination) has no effect at all. One that DID shrink the issuer's obligations
    // but whose sender line did not move by the delivered amount (e.g. a path payment)
    // keeps the books in step with the ledger: O falls, the value is HELD, nobody is
    // credited and nothing is refunded.
    let o_before = read_state(|s| s.xrpl_icusd.issued_outstanding);
    let mut forged = ret(11, &payer(), Some(tag), "30");
    forged.balance_changes[0].final_low_balance = forged.balance_changes[0].previous_low_balance;
    assert!(matches!(apply_ret(&forged), ReturnOutcome::Quarantined(_)));
    let mut no_meta = ret(12, &payer(), Some(tag), "30");
    no_meta.balance_changes.clear();
    assert!(matches!(apply_ret(&no_meta), ReturnOutcome::Quarantined(_)));
    let mut failed = ret(13, &payer(), Some(tag), "30");
    failed.engine_result = "tecPATH_DRY".into();
    assert!(matches!(apply_ret(&failed), ReturnOutcome::Quarantined(_)));
    let mut not_issuer = ret(14, &payer(), Some(tag), "30");
    not_issuer.destination = other();
    assert!(matches!(apply_ret(&not_issuer), ReturnOutcome::Quarantined(_)));
    assert_eq!(processed_count(), 3 + 2, "only the two value-bearing quarantines are consumed");
    read_state(|s| {
        assert_eq!(
            s.xrpl_icusd.issued_outstanding,
            o_before.checked_sub(&WideAmount::from_e8s(60 * E8)).unwrap(),
            "O follows the ledger for the two value-bearing quarantines"
        );
        assert_eq!(s.xrpl_icusd.quarantined_receipts.len(), 2);
        assert_eq!(s.xrpl_icusd.op(id).unwrap().phase, Phase::IntentPrepared, "no credit");
    });
    assert_eq!(debt_of(1), 100 * E8, "quarantine never reduces debt");
}

#[test]
fn nominal_amount_partial_payment_never_credited() {
    replace_state(rworld(100 * E8, 0, 300 * E8, Decimal::ZERO));
    let id = intent(1, 100 * E8, 1, false);
    let tag = tag_of(id);
    // tfPartialPayment: nominal 100 but only 40 delivered. Only the delivered 40 left
    // circulation; nothing is credited against the debt; the 40 is refundable.
    let mut partial = ret(1, &payer(), Some(tag), "40");
    partial.flags = TF_PARTIAL_PAYMENT;
    partial.nominal_amount = Some(ExactIssuedAmount::parse("100").unwrap());
    assert_eq!(apply_ret(&partial), ReturnOutcome::Unattributed);
    assert_eq!(agg_total(&payer()), WideAmount::from_e8s(40 * E8), "exactly the delivered amount");
    read_state(|s| {
        assert_eq!(s.xrpl_icusd.issued_outstanding, WideAmount::from_e8s(260 * E8));
        assert_eq!(s.xrpl_icusd.op(id).unwrap().phase, Phase::IntentPrepared);
    });
    // Delivered != nominal without the flag is the same.
    let mut short = ret(2, &payer(), Some(tag), "10");
    short.nominal_amount = Some(ExactIssuedAmount::parse("100").unwrap());
    assert_eq!(apply_ret(&short), ReturnOutcome::Unattributed);
    // Paths / SendMax likewise.
    let mut paths = ret(3, &payer(), Some(tag), "10");
    paths.has_paths = true;
    assert_eq!(apply_ret(&paths), ReturnOutcome::Unattributed);
    assert_eq!(settle(id), SettleOutcome::NotApplicable);
    assert_eq!(debt_of(1), 100 * E8);
    // The intent can still be paid correctly afterwards.
    assert!(matches!(apply_ret(&ret(4, &payer(), Some(tag), "100")), ReturnOutcome::Attributed { .. }));
}

#[test]
fn a_tag_accepts_one_payment_and_only_while_open() {
    replace_state(rworld(100 * E8, 0, 300 * E8, Decimal::ZERO));
    let id = intent(1, 100 * E8, 1, false);
    let tag = tag_of(id);
    assert!(matches!(apply_ret(&ret(1, &payer(), Some(tag), "10")), ReturnOutcome::Attributed { .. }));
    // A second payment with the same tag is new money: a refund, not a second credit.
    assert_eq!(apply_ret(&ret(2, &payer(), Some(tag), "10")), ReturnOutcome::Unattributed);
    assert_eq!(held_of(id), WideAmount::from_e8s(10 * E8));
    // An expired intent's tag is never reassigned; a late payment is refunded.
    let id2 = intent(1, 5 * E8, 2, false);
    mutate_state(|s| repay::expire_stale_intents(&mut s.xrpl_icusd, NOW + INTENT_TTL_NS + 1));
    assert_eq!(op_phase(id2), Phase::Cancelled);
    assert_eq!(
        apply_ret(&ret(3, &payer(), Some(tag_of(id2)), "5")),
        ReturnOutcome::Unattributed
    );
    let id3 = intent(1, 5 * E8, 3, false);
    assert!(tag_of(id3) > tag_of(id2), "expired tags are not reused");
}

// ---- settlement -------------------------------------------------------------------

#[test]
fn excess_remains_backed_owned_claim() {
    replace_state(rworld(60 * E8, 0, 100 * E8, Decimal::ZERO));
    let id = intent(1, 100 * E8, 1, true);
    assert_eq!(liabilities(), 100 * E8 as u128);
    apply_ret(&ret(1, &payer(), Some(tag_of(id)), "100"));
    assert_eq!(liabilities(), 100 * E8 as u128, "O moved to held, nothing created or lost");
    assert_eq!(
        settle(id),
        SettleOutcome::BurnAdmitted {
            repay_e8s: 60 * E8,
            interest_share_e8s: 0
        }
    );
    // Admitted: R = 60 (to burn), H = 40 (excess), total unchanged.
    read_state(|s| {
        let b = s.xrpl_icusd.buckets().unwrap();
        assert_eq!(b.r, WideAmount::from_e8s(60 * E8));
        assert_eq!(b.h, WideAmount::from_e8s(40 * E8));
        assert_eq!(b.o, WideAmount::ZERO);
    });
    assert_eq!(liabilities(), 100 * E8 as u128);
    read_state(|s| {
        let j = s.xrpl_icusd.op(id).unwrap().ic_jobs.last().unwrap().clone();
        assert_eq!(j.purpose, IcJobPurpose::EscrowBurn);
        assert_eq!(j.amount_e8s, 60 * E8, "burn exactly the applied amount, not the excess");
        assert_eq!(j.from_subaccount, Some(escrow_subaccount()));
        assert_eq!(j.to, backend_default_account(backend()));
    });
    prove_burn(id, 321);
    assert!(matches!(advance_at(id, NOW + 10), RepayAdvance::Settled { .. }));
    assert_eq!(debt_of(1), 0, "full close stays zero");
    assert_eq!(op_phase(id), Phase::Held);
    assert_eq!(held_of(id), WideAmount::from_e8s(40 * E8));
    assert_eq!(liabilities(), 40 * E8 as u128, "the 60 burned left escrow; the excess stays backed");
    // The excess of a SETTLED repayment is a claim, not a deposit: it cannot be topped up
    // (that would re-spend an old payment against a later debt).
    assert!(try_intent(&intent_req(1, E8, &payer(), 50, false, Some(id))).is_err());
    // The excess is the OWNER's: only they can resolve it, and only for the whole e8s.
    let stranger = ResolveClaimArg {
        operation_id: id,
        action: ClaimAction::ReceiveCanonical,
    };
    assert_eq!(
        read_state(|s| repay::plan_repay_resolution(&s.xrpl_icusd, &owner(2), &stranger)).unwrap_err(),
        XrplIcusdError::NotFound
    );
    let plan = read_state(|s| repay::plan_repay_resolution(&s.xrpl_icusd, &owner(1), &stranger)).unwrap();
    assert_eq!(
        plan,
        repay::RepayResolutionPlan::ReceiveCanonicalHeld {
            owner: owner(1),
            amount_e8s: 40 * E8
        }
    );
    // Escrow must cover the claim plus the ledger fee reserve.
    let short = mutate_state(|s| {
        repay::begin_receive_canonical_held(&mut s.xrpl_icusd, id, 10, (40 * E8) as u128, nonce_n(1), 5, NOW + 20)
    });
    assert!(matches!(short, Err(XrplIcusdError::CoverageUnavailable(_))));
    let idx = mutate_state(|s| {
        repay::begin_receive_canonical_held(&mut s.xrpl_icusd, id, 10, (41 * E8) as u128, nonce_n(2), 5, NOW + 20)
    })
    .unwrap();
    read_state(|s| {
        let j = &s.xrpl_icusd.op(id).unwrap().ic_jobs[idx];
        assert_eq!(j.purpose, IcJobPurpose::CanonicalReturn);
        assert_eq!(j.to, Account { owner: owner(1), subaccount: None });
        assert_eq!(j.amount_e8s, 40 * E8);
        assert_eq!(j.reserved_fee_e8s, 10);
    });
    mutate_state(|s| {
        s.xrpl_icusd.begin_job_call(id, idx).unwrap();
        s.xrpl_icusd.finish_job_call(id, idx, LedgerCallResult::Ok(900), NOW + 30).unwrap();
        s.xrpl_icusd.mark_proof_verified(id, idx).unwrap();
        super::issuance::advance_resolution(&mut s.xrpl_icusd, id, NOW + 31).unwrap();
    });
    assert_eq!(op_phase(id), Phase::ClaimResolved);
    assert!(held_of(id).is_zero());
}

fn nonce_n(n: u64) -> u128 {
    super::tests::nonce(n)
}

#[test]
fn excess_can_be_reissued_on_the_xrpl_for_the_exact_amount() {
    replace_state(rworld(60 * E8, 0, 100 * E8, Decimal::ZERO));
    let id = intent(1, 100 * E8, 1, true);
    apply_ret(&ret(1, &payer(), Some(tag_of(id)), "100"));
    settle(id);
    prove_burn(id, 5);
    advance_at(id, NOW + 10);
    let arg = ResolveClaimArg {
        operation_id: id,
        action: ClaimAction::ReissueXrpl,
    };
    let plan = read_state(|s| repay::plan_repay_resolution(&s.xrpl_icusd, &owner(1), &arg)).unwrap();
    assert_eq!(
        plan,
        repay::RepayResolutionPlan::ReissueHeld {
            amount: ExactIssuedAmount::parse("40").unwrap()
        }
    );
    let child = mutate_state(|s| repay::begin_reissue_held(&mut s.xrpl_icusd, id, NOW + 20)).unwrap();
    assert_eq!(op_phase(id), Phase::ClaimResolved);
    assert!(held_of(id).is_zero(), "the value moved, it was not duplicated");
    assert_eq!(liabilities(), 40 * E8 as u128);
    // The refund pays the payer the exact amount through the SAME issuer sequencer.
    let att = mutate_state(|s| {
        let op = s.xrpl_icusd.op(child).unwrap();
        assert_eq!(op.owner, owner(1));
        assert_eq!(op.phase, Phase::RefundPending);
        let amount = op.kind.payout_amount().unwrap().unwrap();
        assert_eq!(amount, ExactIssuedAmount::parse("40").unwrap());
        let a = prepare_attempt(&PrepareInput {
            purpose: AttemptPurpose::Refund,
            tx: AttemptTx::Payment {
                destination: payer(),
                amount,
                destination_tag: None,
            },
            issuer_address: &issuer_addr(),
            issuer_pubkey: super::tests::issuer_pk(),
            sequence: 9,
            prepared_ledger: 1_000,
            fee_drops: 12,
            max_fee_drops: 1_000,
            now_ns: NOW,
        })
        .unwrap();
        s.xrpl_icusd.begin_attempt(LeaseHolder::Operation(child), a).unwrap();
        super::tests::sign_and_submit(&mut s.xrpl_icusd, child);
        // Signed refund value is in flight: D, not H, and the sum is conserved.
        let b = s.xrpl_icusd.buckets().unwrap();
        assert_eq!((b.d, b.h), (WideAmount::from_e8s(40 * E8), WideAmount::ZERO));
        s.xrpl_icusd.lease_attempt(LeaseHolder::Operation(child)).unwrap().clone()
    });
    let v = reconcile(&att, &super::tests::succeeded(&att));
    mutate_state(|s| s.xrpl_icusd.apply_reconcile(LeaseHolder::Operation(child), v, NOW + 40)).unwrap();
    assert_eq!(op_phase(child), Phase::Delivered);
    read_state(|s| assert_eq!(s.xrpl_icusd.issued_outstanding, WideAmount::from_e8s(40 * E8)));
    assert_eq!(liabilities(), 40 * E8 as u128);
}

#[test]
fn sub_e8s_return_retains_exact_liability() {
    replace_state(rworld(10 * E8, 0, 100 * E8, Decimal::ZERO));
    let id = intent(1, 10 * E8, 1, true);
    let r = ret(1, &payer(), Some(tag_of(id)), "10.000000005");
    apply_ret(&r);
    assert!(matches!(
        settle(id),
        SettleOutcome::BurnAdmitted { repay_e8s, .. } if repay_e8s == 10 * E8
    ));
    prove_burn(id, 11);
    advance_at(id, NOW + 1);
    assert_eq!(applied_of(id), 10 * E8, "applied e8s = floor");
    let held = held_of(id);
    assert_eq!(held.to_decimal_string(), "0.000000005", "the sub-e8s part is retained exactly");
    assert_eq!(op_phase(id), Phase::Held);
    assert_eq!(debt_of(1), 0);
    read_state(|s| {
        let b = s.xrpl_icusd.buckets().unwrap();
        assert_eq!(b.h, held);
        assert_eq!(b.h.ceil_e8s(), 1, "backing is required for the fraction (rounded up)");
    });
    // Less than one e8s cannot go back canonical; it can only be reissued exactly.
    let canonical = ResolveClaimArg {
        operation_id: id,
        action: ClaimAction::ReceiveCanonical,
    };
    assert!(matches!(
        read_state(|s| repay::plan_repay_resolution(&s.xrpl_icusd, &owner(1), &canonical)),
        Err(XrplIcusdError::NotApplicable(_))
    ));
    let reissue = ResolveClaimArg {
        operation_id: id,
        action: ClaimAction::ReissueXrpl,
    };
    assert_eq!(
        read_state(|s| repay::plan_repay_resolution(&s.xrpl_icusd, &owner(1), &reissue)).unwrap(),
        repay::RepayResolutionPlan::ReissueHeld {
            amount: ExactIssuedAmount::parse("0.000000005").unwrap()
        }
    );
}

#[test]
fn interest_growth_never_spends_above_signed_amount() {
    // 100 icUSD of debt at 10% APR; the user signed exactly 100 and the payment lands a
    // year later, when the debt has grown past it.
    replace_state(rworld(100 * E8, 0, 100 * E8, dec!(0.10)));
    let id = intent(1, 100 * E8, 1, true);
    apply_ret(&ret(1, &payer(), Some(tag_of(id)), "100"));
    let later = NOW + YEAR_NS;
    let out = settle_at(id, later);
    let (grown, _, _) = vault_of(1);
    assert!(grown > 100 * E8, "interest grew the debt to {grown}");
    match out {
        SettleOutcome::BurnAdmitted { repay_e8s, .. } => {
            assert_eq!(repay_e8s, 100 * E8, "the funds actually received are the ceiling");
        }
        o => panic!("{o:?}"),
    }
    assert_eq!(applied_of(id), 100 * E8);
    assert!(held_of(id).is_zero());
    prove_burn(id, 3);
    advance_at(id, later);
    let (left, _, _) = vault_of(1);
    assert_eq!(left, grown - 100 * E8, "exactly the signed amount was spent, no more");
    assert!(left > 0, "the growth stays as (re-payable) debt, never silently taken from the payer");
}

#[test]
fn stale_full_close_holds_without_burn_then_topup_closes() {
    // The user signed 100 for a full close, but the debt is 100.05: paying 100 would
    // leave 0.05, below the 0.1 minimum vault debt.
    replace_state(rworld(10_005_000_000, 0, 300 * E8, Decimal::ZERO));
    let parent = intent(1, 100 * E8, 1, true);
    apply_ret(&ret(1, &payer(), Some(tag_of(parent)), "100"));
    assert!(matches!(settle(parent), SettleOutcome::Held(_)));
    no_burn(parent);
    assert_eq!(op_phase(parent), Phase::Held);
    assert_eq!(held_of(parent), WideAmount::from_e8s(100 * E8), "the ENTIRE receipt is held");
    assert_eq!(debt_of(1), 10_005_000_000, "debt untouched");
    // A merely held parent is not an admitted burn: nothing is excluded or frozen, so
    // ordinary liquidation is unaffected.
    read_state(|s| {
        assert!(s.xrpl_icusd.vault_exclusions.is_empty());
        assert!(s.xrpl_icusd.accrual_frozen_vaults.is_empty());
    });
    assert!(crate::guard::VaultLiquidationGuard::new(1).is_ok());
    // The owner tops up with a child bound to the held parent.
    let child = try_intent(&intent_req(1, E8, &payer(), 2, true, Some(parent))).unwrap();
    let ReturnOutcome::Merged { child: c, parent: p } =
        apply_ret(&ret(2, &payer(), Some(tag_of(child)), "1"))
    else {
        panic!()
    };
    assert_eq!((c, p), (child, parent));
    assert_eq!(op_phase(child), Phase::Complete);
    assert_eq!(op_phase(parent), Phase::ReturnVerified);
    assert_eq!(held_of(parent), WideAmount::from_e8s(101 * E8));
    assert_eq!(
        settle(parent),
        SettleOutcome::BurnAdmitted {
            repay_e8s: 10_005_000_000,
            interest_share_e8s: 0
        }
    );
    prove_burn(parent, 40);
    advance_at(parent, NOW + 5);
    assert_eq!(debt_of(1), 0, "the aggregate closes the vault");
    assert_eq!(held_of(parent), WideAmount::from_e8s(101 * E8 - 10_005_000_000), "exact excess held");
    assert_eq!(op_phase(parent), Phase::Held);
}

#[test]
fn settled_leftover_is_a_claim_not_a_topup_parent() {
    // 100 owed; 40.000000005 returned: 40 applied, the sub-e8s part held, 60 still owed.
    replace_state(rworld(100 * E8, 0, 100 * E8, Decimal::ZERO));
    let id = intent(1, 41 * E8, 1, false);
    apply_ret(&ret(1, &payer(), Some(tag_of(id)), "40.000000005"));
    settle(id);
    prove_burn(id, 3);
    advance_at(id, NOW + 1);
    assert_eq!((debt_of(1), op_phase(id)), (60 * E8, Phase::Held));
    assert!(matches!(
        try_intent(&intent_req(1, E8, &payer(), 2, false, Some(id))),
        Err(XrplIcusdError::WrongPhase(_))
    ));
}

#[test]
fn topup_owner_payer_vault_and_once_only_binding() {
    replace_state({
        let mut w = rworld(10_005_000_000, 0, 300 * E8, Decimal::ZERO);
        w.open_vault(super::borrow_tests::xrp_vault(5, owner(1), 1_000_000_000));
        w
    });
    set_vault(5, 50 * E8, 0);
    let parent = intent(1, 100 * E8, 1, true);
    // A parent that is not HELD cannot be topped up.
    assert!(matches!(
        try_intent(&intent_req(1, E8, &payer(), 9, false, Some(parent))),
        Err(XrplIcusdError::WrongPhase(_))
    ));
    apply_ret(&ret(1, &payer(), Some(tag_of(parent)), "100"));
    assert!(matches!(settle(parent), SettleOutcome::Held(_)));
    // Different owner: the parent is simply not theirs.
    set_vault(3, 50 * E8, 0);
    let mut other_owner = intent_req(3, E8, &payer(), 10, false, Some(parent));
    other_owner.owner = owner(3);
    assert_eq!(try_intent(&other_owner), Err(XrplIcusdError::NotFound));
    // Different vault, payer, network.
    assert!(matches!(
        try_intent(&intent_req(5, E8, &payer(), 11, false, Some(parent))),
        Err(XrplIcusdError::InvalidArgument(_))
    ));
    assert!(matches!(
        try_intent(&intent_req(1, E8, &other(), 12, false, Some(parent))),
        Err(XrplIcusdError::InvalidArgument(_))
    ));
    let mut net = intent_req(1, E8, &payer(), 13, false, Some(parent));
    net.network = XrplNetwork::Mainnet;
    assert!(matches!(try_intent(&net), Err(XrplIcusdError::WrongNetwork(_))));
    // A valid child: its receipt attaches to the parent exactly once.
    let child = try_intent(&intent_req(1, E8, &payer(), 14, true, Some(parent))).unwrap();
    let r = ret(2, &payer(), Some(tag_of(child)), "1");
    assert!(matches!(apply_ret(&r), ReturnOutcome::Merged { .. }));
    assert_eq!(apply_ret(&r), ReturnOutcome::Duplicate);
    let r_again_other_hash_same_tag = ret(3, &payer(), Some(tag_of(child)), "1");
    assert_eq!(
        apply_ret(&r_again_other_hash_same_tag),
        ReturnOutcome::Unattributed,
        "the child's tag takes one payment"
    );
    read_state(|s| match &s.xrpl_icusd.op(parent).unwrap().kind {
        OperationKind::Repayment { receipts, held, children, .. } => {
            assert_eq!(receipts.len(), 2, "the original and the single top-up receipt");
            assert_eq!(*held, WideAmount::from_e8s(101 * E8));
            assert_eq!(children, &vec![child]);
        }
        k => panic!("{k:?}"),
    });
    assert_eq!(op_phase(parent), Phase::ReturnVerified);
}

#[test]
fn payment_after_liquidation_retains_payer_claim() {
    for remove in [false, true] {
        replace_state(rworld(100 * E8, 0, 100 * E8, Decimal::ZERO));
        let id = intent(1, 100 * E8, 1, false);
        apply_ret(&ret(1, &payer(), Some(tag_of(id)), "100"));
        // The vault is liquidated (debt cleared) or gone before the return is settled.
        if remove {
            mutate_state(|s| {
                s.remove_vault_and_unindex(1);
            });
        } else {
            set_vault(1, 0, 0);
        }
        assert!(matches!(settle(id), SettleOutcome::Held(_)));
        no_burn(id);
        assert_eq!(op_phase(id), Phase::Held);
        assert_eq!(held_of(id), WideAmount::from_e8s(100 * E8), "a backed payer claim, not a donation");
        assert_eq!(liabilities(), 100 * E8 as u128);
        read_state(|s| {
            assert!(s.xrpl_icusd.vault_exclusions.is_empty(), "no exclusion for a held claim");
            assert_eq!(s.xrpl_icusd.issued_outstanding, WideAmount::ZERO);
        });
    }
}

#[test]
fn settlement_defers_while_another_operation_holds_the_vault() {
    replace_state(rworld(100 * E8, 0, 100 * E8, Decimal::ZERO));
    let id = intent(1, 50 * E8, 1, false);
    apply_ret(&ret(1, &payer(), Some(tag_of(id)), "50"));
    let guard = crate::guard::VaultLiquidationGuard::new(1).unwrap();
    assert!(matches!(settle(id), SettleOutcome::Deferred(_)));
    assert_eq!(op_phase(id), Phase::ReturnVerified, "retried later, not held");
    no_burn(id);
    drop(guard);
    assert!(matches!(settle(id), SettleOutcome::BurnAdmitted { .. }));
    // Below the minimum repayment (not a full close): held, not burned.
    replace_state(rworld(100 * E8, 0, 100 * E8, Decimal::ZERO));
    let tiny = intent(1, E8 / 100, 1, false);
    apply_ret(&ret(1, &payer(), Some(tag_of(tiny)), "0.01"));
    assert!(matches!(settle(tiny), SettleOutcome::Held(_)));
}

// ---- burn lifecycle ---------------------------------------------------------------

#[test]
fn burn_unknown_blocks_conflicting_liquidation() {
    replace_state(rworld(100 * E8, 5 * E8, 100 * E8, dec!(0.10)));
    let id = intent(1, 40 * E8, 1, false);
    apply_ret(&ret(1, &payer(), Some(tag_of(id)), "40"));
    assert!(matches!(settle(id), SettleOutcome::BurnAdmitted { .. }));
    unknown_burn(id);
    assert_eq!(advance_at(id, NOW + 50), RepayAdvance::Waiting);
    assert_eq!(op_phase(id), Phase::EscrowBurnPending);
    // Every conflicting admission refuses while the burn is ambiguous.
    assert!(crate::guard::VaultLiquidationGuard::new(1).is_err());
    assert!(crate::state::has_xrpl_vault_exclusion(1));
    // An ambiguous burn can never be cancelled, by the engine or by the owner.
    let cancel = ResolveClaimArg {
        operation_id: id,
        action: ClaimAction::CancelUnexecutedRepayment,
    };
    assert!(matches!(
        read_state(|s| repay::plan_repay_resolution(&s.xrpl_icusd, &owner(1), &cancel)),
        Err(XrplIcusdError::Ambiguous(_))
    ));
    assert!(matches!(
        mutate_state(|s| repay::cancel_unexecuted(s, id, NOW + 51)),
        Err(XrplIcusdError::Ambiguous(_))
    ));
    // Time passes: the frozen vault does not accrue, the exclusion stays.
    let frozen = vault_of(1);
    mutate_state(|s| {
        s.accrue_all_vault_interest(NOW + YEAR_NS);
        s.accrue_single_vault(1, NOW + YEAR_NS);
        s.harvest_accrued_interest();
    });
    assert_eq!(vault_of(1), frozen);
    assert!(crate::guard::VaultLiquidationGuard::new(1).is_err());
    // A Done block that does not match the persisted tuple is unproven: still blocked.
    let idx = burn_idx(id);
    mutate_state(|s| {
        let j = &mut s.xrpl_icusd.op_mut(id).unwrap().ic_jobs[idx];
        j.status = IcJobStatus::Done { block: 9 };
        j.proof_verified = false;
    });
    assert_eq!(advance_at(id, NOW + 60), RepayAdvance::Waiting);
    assert!(crate::state::has_xrpl_vault_exclusion(1));
}

#[test]
fn upgrade_mid_burn_applies_debt_once() {
    replace_state(rworld(100 * E8, 0, 100 * E8, Decimal::ZERO));
    let id = intent(1, 40 * E8, 1, false);
    apply_ret(&ret(1, &payer(), Some(tag_of(id)), "40"));
    settle(id);
    let idx = burn_idx(id);
    // The burn call is outstanding when the canister upgrades.
    mutate_state(|s| {
        s.xrpl_icusd.begin_job_call(id, idx).unwrap();
    });
    upgrade_roundtrip();
    read_state(|s| {
        assert!(s.xrpl_icusd.vault_exclusions.contains(&1), "exclusion survives");
        assert!(s.xrpl_icusd.accrual_frozen_vaults.contains(&1), "freeze survives");
        assert!(matches!(
            s.xrpl_icusd.job(id, idx).unwrap().status,
            IcJobStatus::Unknown { .. }
        ));
    });
    // The identical job is resent; the ledger's dedup turns the lost reply into
    // `Duplicate { duplicate_of }`.
    let before = read_state(|s| s.xrpl_icusd.job(id, idx).unwrap().clone());
    mutate_state(|s| {
        let sent = s.xrpl_icusd.begin_job_call(id, idx).unwrap();
        assert_eq!((sent.op_nonce, sent.created_at_time, sent.amount_e8s, &sent.memo), (before.op_nonce, before.created_at_time, before.amount_e8s, &before.memo));
        s.xrpl_icusd
            .finish_job_call(id, idx, LedgerCallResult::Duplicate(77), NOW + 9)
            .unwrap();
        s.xrpl_icusd.mark_proof_verified(id, idx).unwrap();
    });
    assert!(matches!(advance_at(id, NOW + 10), RepayAdvance::Settled { .. }));
    assert_eq!(debt_of(1), 60 * E8);
    // Re-running the driver (a retried tick, a second upgrade) never applies it twice.
    assert_eq!(advance_at(id, NOW + 11), RepayAdvance::Waiting);
    upgrade_roundtrip();
    assert_eq!(advance_at(id, NOW + 12), RepayAdvance::Waiting);
    assert_eq!(debt_of(1), 60 * E8);
    assert_eq!(count_events(|e| matches!(e, XrplIcusdEvent::RepaySettled { .. })), 1);
}

#[test]
fn full_close_cutoff_survives_timer_harvest_and_upgrade() {
    let mut w = rworld(100 * E8, 4 * E8, 150 * E8, dec!(0.10));
    // A control vault that DOES accrue and harvest while vault 1 is frozen.
    {
        let v = w.vault_id_to_vaults.get_mut(&3).unwrap();
        v.borrowed_icusd_amount = ICUSD::new(50 * E8);
        v.last_accrual_time = NOW;
    }
    replace_state(w);
    let id = intent(1, 120 * E8, 1, true);
    apply_ret(&ret(1, &payer(), Some(tag_of(id)), "120"));
    let out = settle(id);
    assert_eq!(
        out,
        SettleOutcome::BurnAdmitted {
            repay_e8s: 100 * E8,
            interest_share_e8s: 4 * E8
        },
        "a full close repays all debt and releases all accrued interest"
    );
    let snap = vault_of(1);
    assert_eq!(snap, (100 * E8, 4 * E8, NOW));
    let t1 = NOW + YEAR_NS;
    // Exactly the synchronous part of `xrc::interest_and_treasury_tick`.
    mutate_state(|s| {
        crate::storage::record_event_at(&crate::event::Event::AccrueInterest { timestamp: t1 }, t1);
        s.accrue_all_vault_interest(t1);
        s.harvest_accrued_interest();
        s.accrue_single_vault(1, t1);
    });
    assert_eq!(vault_of(1), snap, "timer accrual and harvest skipped the frozen vault");
    read_state(|s| {
        let v3 = &s.vault_id_to_vaults[&3];
        assert!(v3.borrowed_icusd_amount.0 > 50 * E8, "other vaults accrue normally");
        assert_eq!(
            s.pending_interest_for_pools.get(&xrp()).copied(),
            Some(v3.borrowed_icusd_amount.0 - 50 * E8),
            "only the control vault's interest was harvested, not the frozen 4"
        );
        assert_eq!(v3.accrued_interest.0, 0, "the control vault was harvested");
    });
    upgrade_roundtrip();
    mutate_state(|s| {
        s.accrue_all_vault_interest(t1 + 1);
        s.harvest_accrued_interest();
    });
    assert_eq!(vault_of(1), snap, "the cutoff survived the upgrade");
    prove_burn(id, 12);
    let RepayAdvance::Settled { interest } = advance_at(id, t1 + 2) else { panic!() };
    assert_eq!(interest.unwrap().amount_e8s, 4 * E8, "the saved allocation, once");
    let (debt, accrued, _) = vault_of(1);
    assert_eq!((debt, accrued), (0, 0), "full close stays zero");
    mutate_state(|s| s.accrue_all_vault_interest(t1 + YEAR_NS));
    assert_eq!(vault_of(1).0, 0, "a slow burn cannot manufacture dust debt");
    // The share is taken exactly once (a crash can lose it, never repeat it).
    let first = mutate_state(|s| repay::take_pending_interest(&mut s.xrpl_icusd, id, t1 + 3));
    assert_eq!(first.unwrap().amount_e8s, 4 * E8);
    assert!(mutate_state(|s| repay::take_pending_interest(&mut s.xrpl_icusd, id, t1 + 4)).is_none());
    read_state(|s| {
        assert!(s.xrpl_icusd.accrual_frozen_vaults.is_empty());
        assert!(s.xrpl_icusd.vault_exclusions.is_empty());
    });
}

#[test]
fn partial_repay_catches_up_only_unpaid_remainder() {
    replace_state(rworld(100 * E8, 10 * E8, 100 * E8, dec!(0.10)));
    let id = intent(1, 40 * E8, 1, false);
    apply_ret(&ret(1, &payer(), Some(tag_of(id)), "40"));
    assert_eq!(
        settle(id),
        SettleOutcome::BurnAdmitted {
            repay_e8s: 40 * E8,
            interest_share_e8s: 4 * E8
        },
        "the proportional interest share: 40/100 of the 10 accrued"
    );
    // The independent expectation: accrue ONLY the 60 left, from the cutoff.
    let settled_at = NOW + 90 * 24 * 3_600 * 1_000_000_000;
    let expected = {
        let mut probe = read_state(|s| s.clone_for_probe());
        {
            let v = probe.vault_id_to_vaults.get_mut(&1).unwrap();
            v.borrowed_icusd_amount = ICUSD::new(60 * E8);
            v.accrued_interest = ICUSD::new(6 * E8);
            v.last_accrual_time = NOW;
        }
        probe.xrpl_icusd.accrual_frozen_vaults.clear();
        probe.accrue_single_vault(1, settled_at);
        let v = &probe.vault_id_to_vaults[&1];
        (v.borrowed_icusd_amount.to_u64(), v.accrued_interest.to_u64())
    };
    assert!(expected.0 > 60 * E8);
    prove_burn(id, 2);
    let RepayAdvance::Settled { interest } = advance_at(id, settled_at) else { panic!() };
    assert_eq!(interest.unwrap().amount_e8s, 4 * E8);
    let (debt, accrued, last) = vault_of(1);
    assert_eq!((debt, accrued), expected, "interest accrued on the unpaid 60 only");
    assert_eq!(last, settled_at);
    read_state(|s| assert!(s.xrpl_icusd.accrual_frozen_vaults.is_empty()));
}

impl State {
    fn clone_for_probe(&self) -> State {
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(self, &mut bytes).unwrap();
        ciborium::de::from_reader(bytes.as_slice()).unwrap()
    }
}

#[test]
fn cancel_proven_unexecuted_burn_restores_full_accrual() {
    replace_state(rworld(100 * E8, 10 * E8, 100 * E8, dec!(0.10)));
    let id = intent(1, 40 * E8, 1, false);
    apply_ret(&ret(1, &payer(), Some(tag_of(id)), "40"));
    settle(id);
    fail_burn(id);
    let cancel = ResolveClaimArg {
        operation_id: id,
        action: ClaimAction::CancelUnexecutedRepayment,
    };
    assert_eq!(
        read_state(|s| repay::plan_repay_resolution(&s.xrpl_icusd, &owner(1), &cancel)).unwrap(),
        repay::RepayResolutionPlan::CancelBurn
    );
    let at = NOW + 60 * 24 * 3_600 * 1_000_000_000;
    // The independent expectation: FULL still-unpaid debt accrues from the cutoff.
    let expected = {
        let mut probe = read_state(|s| s.clone_for_probe());
        probe.xrpl_icusd.accrual_frozen_vaults.clear();
        probe.accrue_single_vault(1, at);
        let v = &probe.vault_id_to_vaults[&1];
        (v.borrowed_icusd_amount.to_u64(), v.accrued_interest.to_u64())
    };
    assert!(expected.0 > 100 * E8);
    assert_eq!(advance_at(id, at), RepayAdvance::Cancelled);
    assert_eq!(
        (vault_of(1).0, vault_of(1).1),
        expected,
        "accrual restored on the full 100, through the cancellation time"
    );
    read_state(|s| {
        assert!(s.xrpl_icusd.accrual_frozen_vaults.is_empty());
        assert!(s.xrpl_icusd.vault_exclusions.is_empty());
    });
    assert!(crate::guard::VaultLiquidationGuard::new(1).is_ok(), "exclusion released");
    // The payment becomes the payer's held claim (nothing burned, nothing lost).
    assert_eq!(op_phase(id), Phase::Held);
    assert_eq!(held_of(id), WideAmount::from_e8s(40 * E8));
    assert_eq!(applied_of(id), 0);
    assert_eq!(liabilities(), 100 * E8 as u128, "O 60 + held 40: nothing burned, nothing lost");
    assert_eq!(count_events(|e| matches!(e, XrplIcusdEvent::RepayCancelled { .. })), 1);
    // The owner's explicit action is the same transition and is idempotent: nothing to cancel now.
    assert!(matches!(
        mutate_state(|s| repay::cancel_unexecuted(s, id, at + 1)),
        Err(XrplIcusdError::WrongPhase(_))
    ));
}

#[test]
fn explicit_cancel_action_matches_automatic_cancel() {
    replace_state(rworld(100 * E8, 0, 100 * E8, Decimal::ZERO));
    let id = intent(1, 40 * E8, 1, false);
    apply_ret(&ret(1, &payer(), Some(tag_of(id)), "40"));
    settle(id);
    fail_burn(id);
    mutate_state(|s| repay::cancel_unexecuted(s, id, NOW + 5)).unwrap();
    assert_eq!(op_phase(id), Phase::Held);
    assert_eq!(held_of(id), WideAmount::from_e8s(40 * E8));
    // The re-evaluation after a top-up admits a FRESH burn cycle (new job, new nonce).
    let child = try_intent(&intent_req(1, 60 * E8, &payer(), 2, true, Some(id))).unwrap();
    apply_ret(&ret(2, &payer(), Some(tag_of(child)), "60"));
    assert!(matches!(settle_at(id, NOW + 6), SettleOutcome::BurnAdmitted { repay_e8s, .. } if repay_e8s == 100 * E8));
    read_state(|s| {
        let burns: Vec<_> = s
            .xrpl_icusd
            .op(id)
            .unwrap()
            .ic_jobs
            .iter()
            .filter(|j| j.purpose == IcJobPurpose::EscrowBurn)
            .collect();
        assert_eq!(burns.len(), 2);
        assert_ne!(burns[0].op_nonce, burns[1].op_nonce);
    });
    prove_burn(id, 4);
    advance_at(id, NOW + 7);
    assert_eq!(debt_of(1), 0);
    assert_eq!(op_phase(id), Phase::Complete);
}

// ---- refunds ----------------------------------------------------------------------

#[test]
fn unattributed_refund_only_to_verified_sender() {
    replace_state(rworld(100 * E8, 0, 100 * E8, Decimal::ZERO));
    let _id = intent(1, 100 * E8, 1, false);
    let r = ret(1, &other(), Some(424_242), "25");
    assert_eq!(apply_ret(&r), ReturnOutcome::Unattributed);
    // The once-only state exists BEFORE anything is signed, and replay finds it.
    assert_eq!(apply_ret(&r), ReturnOutcome::Duplicate);
    assert!(refund_ops().is_empty(), "value waits in the sender aggregate until drained");
    assert_eq!(liabilities(), 100 * E8 as u128, "the aggregate is held, backed value");
    let created = drain();
    assert_eq!(created.len(), 1);
    let op_id = created[0];
    assert_eq!(refund_ops(), vec![op_id]);
    assert!(agg_total(&other()).is_zero(), "the value moved into the refund operation");
    assert!(drain().is_empty(), "nothing left to cut");
    read_state(|s| {
        let op = s.xrpl_icusd.op(op_id).unwrap();
        assert_eq!(op.owner, system_owner());
        assert_eq!(op.phase, Phase::RefundPending);
        assert!(op.needs_work(), "the sequencer will pay it");
        let OperationKind::Refund { recipient, amount, source_tx } = &op.kind else { panic!() };
        assert_eq!(*recipient, other(), "the verified XRPL sender, never an arbitrary claimant");
        assert_eq!(*amount, ExactIssuedAmount::parse("25").unwrap());
        assert!(source_tx.starts_with(&format!("aggregate:{}", hash_of(1))));
        assert_eq!(s.xrpl_icusd.issued_outstanding, WideAmount::from_e8s(75 * E8));
    });
    // Nobody can drive or redirect it: it belongs to no principal.
    for p in [owner(1), owner(2), other_principal()] {
        let arg = ResolveClaimArg {
            operation_id: op_id,
            action: ClaimAction::ReissueXrpl,
        };
        assert_eq!(
            read_state(|s| repay::plan_repay_resolution(&s.xrpl_icusd, &p, &arg)).unwrap_err(),
            XrplIcusdError::NotFound
        );
        assert!(get_ops(p).iter().all(|v| v.operation_id != op_id));
    }
    // A missing trust line leaves the refund pending (it is not failed or dropped).
    let e = check_trust_line_exact(None, &ExactIssuedAmount::parse("25").unwrap()).unwrap_err();
    assert!(matches!(e, XrplIcusdError::TrustLineMissing(_)));
    assert_eq!(op_phase(op_id), Phase::RefundPending);
    // The payment pays exactly the sender, the exact amount, from the single sequencer;
    // validation moves it back into O (conservation holds throughout).
    assert_eq!(liabilities(), 100 * E8 as u128);
    let att = mutate_state(|s| {
        let op = s.xrpl_icusd.op(op_id).unwrap();
        let a = prepare_attempt(&PrepareInput {
            purpose: AttemptPurpose::Refund,
            tx: AttemptTx::Payment {
                destination: op.kind.recipient().unwrap().to_string(),
                amount: op.kind.payout_amount().unwrap().unwrap(),
                destination_tag: None,
            },
            issuer_address: &issuer_addr(),
            issuer_pubkey: super::tests::issuer_pk(),
            sequence: 3,
            prepared_ledger: 1_000,
            fee_drops: 12,
            max_fee_drops: 1_000,
            now_ns: NOW,
        })
        .unwrap();
        s.xrpl_icusd.begin_attempt(LeaseHolder::Operation(op_id), a).unwrap();
        super::tests::sign_and_submit(&mut s.xrpl_icusd, op_id);
        s.xrpl_icusd.lease_attempt(LeaseHolder::Operation(op_id)).unwrap().clone()
    });
    assert_eq!(liabilities(), 100 * E8 as u128, "in flight: D, not lost");
    let v = reconcile(&att, &super::tests::succeeded(&att));
    mutate_state(|s| s.xrpl_icusd.apply_reconcile(LeaseHolder::Operation(op_id), v, NOW + 9)).unwrap();
    assert_eq!(op_phase(op_id), Phase::Delivered);
    assert_eq!(liabilities(), 100 * E8 as u128);
    // Dust is retained exactly in the sender aggregate but never auto-refunded (a
    // refund costs the issuer XRP): no operation is created for it.
    let dust = ret(2, &other(), Some(7), "0.0000001");
    assert_eq!(apply_ret(&dust), ReturnOutcome::Unattributed);
    assert_eq!(agg_total(&other()), WideAmount::from_e8s(10));
    assert!(drain().is_empty());
    assert_eq!(refund_ops(), vec![op_id], "no new operation for dust");
}

fn other_principal() -> Principal {
    Principal::from_slice(&[0x31; 10])
}

fn get_ops(p: Principal) -> Vec<OperationView> {
    super::get_operations(p, None, 50).items
}

// ---- observer ---------------------------------------------------------------------

#[test]
fn idle_zero_outcalls() {
    replace_state(rworld(100 * E8, 0, 100 * E8, Decimal::ZERO));
    let m = Mock::default();
    // No repayment intent: nothing is scheduled and a pass returns before any outcall.
    read_state(|s| {
        assert!(!s.xrpl_icusd.has_work());
        assert!(!s.xrpl_icusd.observer_has_work());
        assert!(!super::plan_tick(&s.xrpl_icusd, 6, 0).contains(&super::TickItem::Observer));
    });
    assert_eq!(pass(&m), PassOutcome::Idle);
    assert_eq!(m.total_calls(), 0, "idle makes zero outcalls");
    assert_eq!(super::next_tick_delay(false, false, 30), None, "and the timer stops");
    // A held / settled repayment is not observer work either.
    let id = intent(1, 10 * E8, 1, false);
    apply_ret(&ret(1, &payer(), Some(tag_of(id)), "10"));
    assert_eq!(pass(&m), PassOutcome::Idle);
    assert_eq!(m.total_calls(), 0);
    // An open intent turns the observer on, with the 30 s / 5 min cadence.
    let _open = intent(1, 10 * E8, 2, false);
    read_state(|s| {
        assert!(s.xrpl_icusd.observer_has_work());
        assert!(s.xrpl_icusd.has_work());
        assert!(super::plan_tick(&s.xrpl_icusd, 6, 0).contains(&super::TickItem::Observer));
    });
    assert_eq!(super::next_tick_delay(true, true, 300), Some(30));
    assert_eq!(super::next_tick_delay(true, false, 30), Some(60));
    assert_eq!(super::next_tick_delay(true, false, 300), Some(300), "capped at 5 minutes");
    // The lane disabled: no work, no outcalls, even with an open intent.
    mutate_state(|s| s.xrpl_icusd.config.enabled = false);
    assert_eq!(pass(&m), PassOutcome::Idle);
    assert_eq!(m.total_calls(), 0);
}

#[test]
fn active_history_gap_recovery() {
    replace_state(rworld(100 * E8, 0, 100 * E8, Decimal::ZERO));
    let id = intent(1, 10 * E8, 1, false);
    let r = ret(1, &payer(), Some(tag_of(id)), "10");
    let m = Mock::default().with_receipt(&r);
    // The validated tip cannot be read: nothing moves.
    assert!(matches!(pass(&m), PassOutcome::Recoverable(_)));
    // The providers do not hold the interval: a typed gap, cursor parked, no effect.
    m.tips.borrow_mut().push_back(Ok(2_000));
    m.pages.borrow_mut().push_back(Err(IssuedRpcError::HistoryGap {
        requested_min: 1,
        requested_max: 2_000,
        detail: "complete=500-1999".into(),
    }));
    assert!(matches!(pass(&m), PassOutcome::Recoverable(_)));
    read_state(|s| {
        let o = &s.xrpl_icusd.observer;
        assert_eq!(o.cursor, Some(IssuerHistoryCursor { ledger_min: 1, ledger_max: 2_000, marker: None }));
        assert_eq!(o.next_ledger_min, None, "no advance over a gap");
        assert_eq!(o.progress, 0);
        assert_eq!(s.xrpl_icusd.op(id).unwrap().phase, Phase::IntentPrepared);
        assert_eq!(s.xrpl_icusd.issued_outstanding, WideAmount::from_e8s(100 * E8));
    });
    // Recovery: the SAME interval is retried (the cursor was kept), the page is
    // processed, the interval completes and the observer moves on.
    m.pages.borrow_mut().push_back(Ok(page(vec![parsed(&r)], None, 1, 2_000)));
    let out = pass(&m);
    assert!(matches!(out, PassOutcome::Progressed { ref affected, .. } if affected == &vec![id]), "{out:?}");
    assert_eq!(m.last_cursor.borrow().clone().unwrap().ledger_max, 2_000, "the interval did not move");
    assert_eq!(m.last_limit.get(), 50, "bounded batches");
    read_state(|s| {
        let o = &s.xrpl_icusd.observer;
        assert_eq!(o.cursor, None);
        assert_eq!(o.next_ledger_min, Some(2_001));
        assert_eq!(s.xrpl_icusd.op(id).unwrap().phase, Phase::ReturnVerified);
    });
    // Caught up and idle: the next pass stops without an outcall.
    let calls = m.total_calls();
    assert_eq!(pass(&m), PassOutcome::Idle);
    assert_eq!(m.total_calls(), calls);
    // A later intent resumes from the durable cursor, not from the start.
    let _id2 = intent(1, 5 * E8, 2, false);
    m.tips.borrow_mut().push_back(Ok(2_050));
    m.pages.borrow_mut().push_back(Ok(page(vec![], None, 2_001, 2_050)));
    assert!(matches!(pass(&m), PassOutcome::Progressed { .. }));
    let c = m.last_cursor.borrow().clone().unwrap();
    assert_eq!((c.ledger_min, c.ledger_max), (2_001, 2_050));
    // Nothing new on the ledger: caught up.
    m.tips.borrow_mut().push_back(Ok(2_050));
    assert_eq!(pass(&m), PassOutcome::CaughtUp);
}

#[test]
fn history_page_mid_ledger_preserves_cursor() {
    replace_state(rworld(100 * E8, 0, 100 * E8, Decimal::ZERO));
    let id = intent(1, 10 * E8, 1, false);
    let r = ret(1, &payer(), Some(tag_of(id)), "10");
    let marker = r#"{"ledger":1500,"seq":3}"#;
    let m = Mock::default()
        .with_receipt(&r)
        .with_tip(2_000)
        .with_page(page(vec![parsed(&r)], Some(marker), 1, 2_000));
    assert!(matches!(pass(&m), PassOutcome::Progressed { .. }));
    // Mid-ledger: the bounds are preserved and only the continuation marker moves.
    read_state(|s| {
        let o = &s.xrpl_icusd.observer;
        assert_eq!(
            o.cursor,
            Some(IssuerHistoryCursor {
                ledger_min: 1,
                ledger_max: 2_000,
                marker: Some(marker.to_string())
            })
        );
        assert_eq!(o.next_ledger_min, None, "ledger 1500 is only partly consumed");
    });
    // The intent was satisfied by the first page, so the observer rests; the cursor
    // stays parked mid-interval and resumes from the marker when work reappears.
    assert_eq!(pass(&m), PassOutcome::Idle);
    let _id2 = intent(1, 5 * E8, 2, false);
    m.pages.borrow_mut().push_back(Ok(page(vec![], None, 1, 2_000)));
    assert!(matches!(pass(&m), PassOutcome::Progressed { .. }));
    let c = m.last_cursor.borrow().clone().unwrap();
    assert_eq!(c.marker.as_deref(), Some(marker), "the page request continued from the marker");
    assert_eq!(m.tip_calls.get(), 1, "an in-progress interval does not re-read the tip");
    read_state(|s| {
        assert_eq!(s.xrpl_icusd.observer.cursor, None);
        assert_eq!(s.xrpl_icusd.observer.next_ledger_min, Some(2_001));
    });
}

#[test]
fn page_with_an_unconfirmed_return_does_not_advance_but_keeps_applied_ones() {
    replace_state(rworld(100 * E8, 0, 100 * E8, Decimal::ZERO));
    let a = intent(1, 10 * E8, 1, false);
    let b = intent(1, 20 * E8, 2, false);
    let ra = ret(1, &payer(), Some(tag_of(a)), "10");
    let rb = ret(2, &payer(), Some(tag_of(b)), "20");
    // Only `ra` can be confirmed; `rb` is unknown to the providers right now.
    let m = Mock::default()
        .with_receipt(&ra)
        .with_tip(100)
        .with_page(page(vec![parsed(&ra), parsed(&rb)], None, 1, 100));
    assert!(matches!(pass(&m), PassOutcome::Recoverable(_)));
    read_state(|s| {
        assert_eq!(s.xrpl_icusd.observer.cursor.as_ref().unwrap().marker, None);
        assert_eq!(s.xrpl_icusd.observer.next_ledger_min, None, "no advance over an unconfirmed return");
        assert_eq!(s.xrpl_icusd.op(a).unwrap().phase, Phase::ReturnVerified, "the confirmed one is applied");
        assert_eq!(s.xrpl_icusd.op(b).unwrap().phase, Phase::IntentPrepared);
    });
    // The retry re-reads only the missing one (the applied one is consumed).
    m.receipts
        .borrow_mut()
        .insert(rb.hash.clone(), Ok(ReceiptLookup::Verified(Box::new(rb.clone()))));
    m.pages.borrow_mut().push_back(Ok(page(vec![parsed(&ra), parsed(&rb)], None, 1, 100)));
    let before = m.receipt_calls.get();
    assert!(matches!(pass(&m), PassOutcome::Progressed { .. }));
    assert_eq!(m.receipt_calls.get() - before, 1);
    assert_eq!(processed_count(), 2);
    assert_eq!(read_state(|s| s.xrpl_icusd.issued_outstanding), WideAmount::from_e8s(70 * E8));
}

// ---- exclusion / gating -------------------------------------------------------------

#[test]
fn vault_has_open_operation_covers_exclusion_freeze_and_unresolved_ops() {
    replace_state(rworld(100 * E8, 0, 100 * E8, Decimal::ZERO));
    read_state(|s| assert!(!s.xrpl_icusd.vault_has_open_operation(1)));
    let id = intent(1, 40 * E8, 1, false);
    read_state(|s| assert!(s.xrpl_icusd.vault_has_open_operation(1), "an open repayment"));
    read_state(|s| assert!(!s.xrpl_icusd.vault_has_open_operation(3)));
    apply_ret(&ret(1, &payer(), Some(tag_of(id)), "40"));
    settle(id);
    read_state(|s| {
        assert!(s.xrpl_icusd.vault_exclusions.contains(&1) && s.xrpl_icusd.accrual_frozen_vaults.contains(&1));
        assert!(s.xrpl_icusd.vault_has_open_operation(1));
    });
    prove_burn(id, 1);
    advance_at(id, NOW + 1);
    read_state(|s| assert!(!s.xrpl_icusd.vault_has_open_operation(1), "settled and complete"));
}

// ---- replay ---------------------------------------------------------------------------

fn replay_init() -> crate::event::Event {
    crate::event::Event::Init(crate::InitArg {
        xrc_principal: Principal::anonymous(),
        icusd_ledger_principal: Principal::from_slice(&[0x10; 10]),
        icp_ledger_principal: Principal::from_slice(&[0x11; 10]),
        fee_e8s: 0,
        developer_principal: Principal::from_slice(&[0xdd; 10]),
        treasury_principal: Some(super::borrow_tests::treasury()),
        stability_pool_principal: Some(super::borrow_tests::sp()),
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    })
}

#[test]
fn cutoff_without_settle_replays_frozen_and_settles_to_the_live_debt() {
    use crate::event::Event;
    // Live path on the ICP vault (vault 2): 100 icUSD of debt, 10 accrued, 10% APR.
    let mut w = world();
    let icp = w.icp_collateral_type();
    w.collateral_configs.get_mut(&icp).unwrap().interest_rate_apr = Ratio::from(dec!(0.10));
    // Replay never replays price updates, so accrual runs at CR 0 there; make the live
    // side identical (the freeze/settlement math is what is under test).
    w.collateral_configs.get_mut(&icp).unwrap().last_price = None;
    w.xrpl_icusd.issued_outstanding = WideAmount::from_e8s(100 * E8);
    {
        let v = w.vault_id_to_vaults.get_mut(&2).unwrap();
        v.borrowed_icusd_amount = ICUSD::new(100 * E8);
        v.accrued_interest = ICUSD::new(10 * E8);
        v.last_accrual_time = NOW;
    }
    replace_state(w);
    let id = inject_verified(2, 40 * E8, 40 * E8, false);
    let settled_at = NOW + 30 * 24 * 3_600 * 1_000_000_000;
    assert!(matches!(settle(id), SettleOutcome::BurnAdmitted { .. }));
    let mid = NOW + 10 * 24 * 3_600 * 1_000_000_000;
    prove_burn(id, 2);
    advance_at(id, settled_at);
    let live_final = {
        let v = read_state(|s| s.vault_id_to_vaults[&2].clone());
        (v.borrowed_icusd_amount.to_u64(), v.accrued_interest.to_u64(), v.last_accrual_time)
    };
    let lane_events: Vec<Event> = crate::storage::events().collect();
    assert_eq!(lane_events.len(), 2, "Begun then Settled");
    let begun = lane_events[0].clone();
    let settled = lane_events[1].clone();
    // The replayed log: the vault opens with its pre-admission debt; the lane events
    // and a timer `AccrueInterest` interleave in log order.
    let base = || -> Vec<Event> {
        vec![
            replay_init(),
            Event::SetInterestRate {
                collateral_type: icp,
                interest_rate_apr: "0.1".into(),
            },
            Event::OpenVault {
                vault: crate::vault::Vault {
                    owner: owner(2),
                    vault_id: 2,
                    borrowed_icusd_amount: ICUSD::new(100 * E8),
                    collateral_amount: 100 * E8,
                    collateral_type: icp,
                    last_accrual_time: NOW,
                    accrued_interest: ICUSD::new(10 * E8),
                    bot_processing: false,
                },
                block_index: 0,
                timestamp: Some(NOW),
            },
        ]
    };
    let debt_of_replay = |events: Vec<Event>| -> (u64, u64, u64, bool) {
        let st = crate::event::replay_with_nonce_time(events.into_iter(), || 7).unwrap();
        let v = &st.vault_id_to_vaults[&2];
        (
            v.borrowed_icusd_amount.to_u64(),
            v.accrued_interest.to_u64(),
            v.last_accrual_time,
            st.xrpl_icusd.accrual_frozen_vaults.contains(&2),
        )
    };
    // Control: WITHOUT the cutoff event, the timer accrual moves the vault.
    let mut control = base();
    control.push(Event::AccrueInterest { timestamp: mid });
    let (control_debt, ..) = debt_of_replay(control);
    assert!(control_debt > 100 * E8, "the control vault accrues: {control_debt}");
    // Cutoff recorded, burn unresolved, then a timer AccrueInterest: frozen.
    let mut unresolved = base();
    unresolved.push(begun.clone());
    unresolved.push(Event::AccrueInterest { timestamp: mid });
    let (d, a, t, frozen) = debt_of_replay(unresolved);
    assert!(frozen, "the freeze is rebuilt from the event");
    assert_eq!((d, a, t), (100 * E8, 10 * E8, NOW), "the frozen vault did not accrue in replay");
    // Then the settlement: the same final debt as the live path.
    let mut full = base();
    full.push(begun);
    full.push(Event::AccrueInterest { timestamp: mid });
    full.push(settled);
    let (d, a, t, frozen) = debt_of_replay(full);
    assert!(!frozen);
    assert_eq!((d, a, t), live_final, "replay settles to exactly the live debt");
}

// ---- review hardening: bounded growth, quarantined value, retention -------------------

fn hash_n(i: u64) -> String {
    format!("{i:064X}")
}

/// A validated direct return with an arbitrary hash/ledger (for volume tests).
fn ret_vol(i: u64, sender: &str, tag: Option<u32>, amount: &str, ledger: u32) -> VerifiedIssuedReceipt {
    let mut r = ret(1, sender, tag, amount);
    r.hash = hash_n(i);
    r.ledger_index = ledger;
    r
}

/// Liabilities recomputed from EVERY operation (no index): the oracle the index-based
/// `buckets()` must always agree with.
fn buckets_by_full_scan(st: &super::state::XrplIcusdState) -> super::state::Buckets {
    let mut b = super::state::Buckets {
        o: st.issued_outstanding,
        ..Default::default()
    };
    for op in st.operations.values() {
        super::state::add_op_liability(&mut b, op).unwrap();
    }
    for u in st.unattributed.values() {
        b.h = b.h.checked_add(&u.total).unwrap();
    }
    for q in st.quarantined_receipts.values() {
        b.h = b.h.checked_add(&q.amount).unwrap();
    }
    b
}

#[test]
fn ten_thousand_dust_returns_do_not_block_users_or_grow_state() {
    // 10k dust returns from ONE sender, 10k more from DISTINCT senders (past the
    // aggregate cap): no operation is created, user admission keeps working, the
    // exact value stays backed, and the encoded lane state stays small.
    replace_state(rworld(100 * E8, 0, 1_000 * E8, Decimal::ZERO));
    let ops_before = read_state(|s| s.xrpl_icusd.operations.len());
    let liabilities_before = liabilities();
    let dust = "0.00000001"; // 1 e8s
    for i in 0..10_000u64 {
        let r = ret_vol(i, &payer(), None, dust, 2_000);
        assert_eq!(apply_ret(&r), ReturnOutcome::Unattributed);
    }
    for i in 10_000..20_000u64 {
        let sender = format!("rDust{i:05}");
        let r = ret_vol(i, &sender, None, dust, 2_000);
        assert_eq!(apply_ret(&r), ReturnOutcome::Unattributed);
    }
    read_state(|s| {
        let st = &s.xrpl_icusd;
        assert_eq!(st.operations.len(), ops_before, "no operation per dust transaction");
        assert!(
            st.unattributed.len() <= MAX_UNATTRIBUTED_SENDERS + 1,
            "aggregates are capped (+ one overflow record)"
        );
        let overflow = st
            .unattributed
            .get(&super::retention::aggregate_key(TESTNET, OVERFLOW_SENDER))
            .expect("excess senders share the overflow aggregate");
        assert!(overflow.tx_count > 0);
        // Exactness: 20,000 e8s of dust left O and is held, nothing lost.
        assert_eq!(
            st.issued_outstanding,
            WideAmount::from_e8s(1_000 * E8 - 20_000)
        );
        assert_eq!(buckets_by_full_scan(st), st.buckets().unwrap());
        // Users are unaffected: admission still works, and the limits count only users.
        assert_eq!(st.user_op_count(), 0);
        assert!(st
            .admit(
                &owner(1),
                &[9; 32],
                TESTNET,
                &OperationKind::Import { amount_e8s: 1, recipient: payer() }
            )
            .is_ok());
    });
    assert_eq!(liabilities(), liabilities_before, "value moved O -> H, backing unchanged");
    // Dust never becomes a refund operation (a refund costs the issuer XRP).
    assert!(drain().is_empty());
    assert!(refund_ops().is_empty());
    // A user operation can still be created and works end to end.
    let id = intent(1, 10 * E8, 77, false);
    assert_eq!(op_phase(id), Phase::IntentPrepared);
    // The encoded lane state is bounded by what is retained, not by traffic.
    let bytes = read_state(|s| {
        let mut b = Vec::new();
        ciborium::ser::into_writer(&s.xrpl_icusd, &mut b).unwrap();
        b.len()
    });
    assert!(bytes < 6 * 1024 * 1024, "20k dust returns encode to {bytes} bytes");
}

#[test]
fn encoded_state_at_the_configured_caps_stays_under_budget() {
    // Documented budget: the lane state at EVERY configured cap simultaneously must
    // encode below 64 MiB. Measured at the time of writing: about 38 MB of operations
    // (10,000 user + 1,000 system records, each with two proven ledger jobs, a full
    // signed XRPL attempt and four maximum-length notes: a deliberate worst case), 7 MB
    // of dedup records, 0.8 MB of aggregates and quarantine. The snapshot is written to
    // stable memory on upgrade, so this is what an upgrade's serialization carries.
    // Terminal operations leave after `OP_RETENTION_NS`, so the operations term is a
    // ceiling, not the steady state.
    const BUDGET: usize = 64 * 1024 * 1024;
    let mut st = ready_lane();
    // Dedup records at their hard cap.
    for i in 0..MAX_PROCESSED_RETURNS as u64 {
        st.processed_returns.insert(
            repay::processed_key(TESTNET, &issuer_addr(), &hash_n(i)),
            ProcessedReturn {
                disposition: ReturnDisposition::Aggregated {
                    reason: "no destination tag".into(),
                },
                ledger_index: 3_000,
                sender: holder_addr(9),
                destination_tag: Some(1),
                delivered: "0.12345678".into(),
                recorded_at_ns: NOW,
            },
        );
    }
    // Aggregates and quarantine at their caps.
    for i in 0..MAX_UNATTRIBUTED_SENDERS as u64 {
        st.fold_unattributed(
            TESTNET,
            &format!("{}{i:04}", holder_addr(9)),
            &WideAmount::from_e8s(5),
            &hash_n(i),
            3_000,
            "unknown destination tag",
            NOW,
        )
        .unwrap();
    }
    for i in 0..MAX_QUARANTINED_RECEIPTS as u64 {
        st.hold_quarantined(QuarantinedReceipt {
            tx_hash: hash_n(i),
            ledger_index: 3_000,
            sender: holder_addr(9),
            destination_tag: Some(1),
            amount: WideAmount::from_e8s(7),
            reason: "sender trust-line change differs from the delivered amount".into(),
            at_ns: NOW,
        })
        .unwrap();
    }
    for i in 0..MAX_QUARANTINE_LOG as u64 {
        st.log_quarantine(QuarantineRecord {
            tx_hash: hash_n(i),
            ledger_index: Some(3_000),
            sender: Some(holder_addr(9)),
            destination_tag: Some(1),
            reason: "x".repeat(60),
            at_ns: NOW,
        });
    }
    // User operations at the global cap (worst case: each with a proven job, a
    // signed XRPL attempt and notes), plus system refund records at their cap.
    for i in 0..MAX_OPERATIONS_GLOBAL as u64 {
        let mut op = fat_operation(i, Principal::from_slice(&i.to_be_bytes()), false);
        op.request_nonce = [(i % 251) as u8; 32];
        st.request_nonce_index.insert((op.owner, op.request_nonce), i);
        st.operations.insert(i, op);
    }
    for i in 0..MAX_SYSTEM_REFUND_OPS as u64 {
        let id = MAX_OPERATIONS_GLOBAL as u64 + i;
        let op = fat_operation(id, system_owner(), true);
        st.request_nonce_index.insert((op.owner, [(i % 251) as u8; 32]), id);
        st.operations.insert(id, op);
    }
    st.rebuild_indexes();
    let size_of = |v: &dyn Fn(&mut Vec<u8>)| {
        let mut b = Vec::new();
        v(&mut b);
        b.len()
    };
    eprintln!(
        "ops {} processed {} agg {} quarantine {}",
        size_of(&|b| ciborium::ser::into_writer(&st.operations, b).unwrap()),
        size_of(&|b| ciborium::ser::into_writer(&st.processed_returns, b).unwrap()),
        size_of(&|b| ciborium::ser::into_writer(&st.unattributed, b).unwrap()),
        size_of(&|b| ciborium::ser::into_writer(&st.quarantined_receipts, b).unwrap()),
    );
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&st, &mut bytes).unwrap();
    eprintln!("lane state at every cap: {} bytes", bytes.len());
    assert!(
        bytes.len() < BUDGET,
        "lane state at the configured caps encodes to {} bytes (budget {BUDGET})",
        bytes.len()
    );
    // The user cap is independent of system refund records.
    assert_eq!(st.user_op_count(), MAX_OPERATIONS_GLOBAL);
    assert!(matches!(
        st.admit(&owner(1), &[9; 32], TESTNET, &OperationKind::Import { amount_e8s: 1, recipient: payer() }),
        Err(XrplIcusdError::LimitExceeded(_))
    ));
}

fn ready_lane() -> super::state::XrplIcusdState {
    super::tests::ready_state()
}

/// A worst-case-sized operation: a proven ledger job, a signed issuer attempt with a
/// full-size blob, and a few notes.
fn fat_operation(id: u64, owner_p: Principal, refund: bool) -> Operation {
    let kind = if refund {
        OperationKind::Refund {
            source_tx: format!("aggregate:{}:0", hash_n(id)),
            recipient: holder_addr(9),
            amount: ExactIssuedAmount::parse("12.5").unwrap(),
        }
    } else {
        OperationKind::Borrow {
            vault_id: id,
            collateral_type: Principal::from_slice(&[7; 10]),
            gross_e8s: 100 * E8,
            fee_e8s: E8,
            net_e8s: 99 * E8,
            recipient: holder_addr(9),
            policy_version: 1,
            fee_split: Some(FeeSplit {
                to_deficit_e8s: 1,
                to_treasury_remainder_e8s: 1,
                treasury: Some(Principal::from_slice(&[8; 10])),
            }),
        }
    };
    let job = |purpose| IcLedgerJob {
        purpose,
        op_nonce: u128::from(id) << 70,
        created_at_time: NOW,
        from_subaccount: None,
        from_owner: None,
        to: Account { owner: Principal::from_slice(&[9; 10]), subaccount: Some([3; 32]) },
        amount_e8s: 99 * E8,
        memo: vec![1; 16],
        reserved_fee_e8s: 0,
        ledger_tip_before: Some(5),
        status: IcJobStatus::Done { block: id },
        proof_verified: true,
        scan_required: false,
        scan: None,
        sends: 1,
        in_flight_since_ns: None,
    };
    Operation {
        id,
        owner: owner_p,
        created_at_ns: NOW,
        updated_at_ns: NOW,
        request_nonce: [0; 32],
        network: TESTNET,
        kind,
        phase: if refund { Phase::RefundPending } else { Phase::Delivered },
        ic_jobs: vec![job(IcJobPurpose::EscrowMint), job(IcJobPurpose::FeeMint)],
        xrpl_attempts: vec![IssuerTxAttempt {
            purpose: AttemptPurpose::Delivery,
            tx: AttemptTx::Payment {
                destination: holder_addr(9),
                amount: ExactIssuedAmount::parse("99").unwrap(),
                destination_tag: None,
            },
            account: issuer_addr(),
            sequence: 1,
            last_ledger_sequence: 2,
            prepared_ledger: 1,
            fee_drops: 12,
            unsigned_hex: "AB".repeat(180),
            signed_hex: Some("CD".repeat(260)),
            tx_hash: Some(hash_n(id)),
            status: AttemptStatus::Validated { ledger_index: 9 },
            created_at_ns: NOW,
            submit_count: 1,
            last_submit_ns: NOW,
            note: None,
        }],
        notes: vec!["n".repeat(MAX_NOTE_LEN); 4],
        resolution: None,
        attempt_budget_base: 0,
        delivery_halted: false,
    }
}

#[test]
fn refunds_are_cut_from_aggregates_with_threshold_budget_and_separate_cap() {
    replace_state(rworld(100 * E8, 0, 100_000 * E8, Decimal::ZERO));
    let threshold = read_state(|s| s.xrpl_icusd.config.min_auto_refund_e8s);
    assert!(threshold >= 1_000_000, "above the issuer XRP fee value by default");
    // Several tiny returns from one sender add up exactly; below the threshold: nothing.
    for i in 0..5u64 {
        apply_ret(&ret_vol(i, &payer(), None, "0.001", 2_000));
    }
    assert_eq!(agg_total(&payer()), WideAmount::from_e8s(500_000));
    assert!(drain().is_empty(), "below the floor: retained exactly, not refunded");
    // Crossing the threshold cuts ONE refund for the whole aggregate (sum, not N ops).
    for i in 5..15u64 {
        apply_ret(&ret_vol(i, &payer(), None, "0.001", 2_000));
    }
    let made = drain();
    assert_eq!(made.len(), 1);
    assert_eq!(held_of(made[0]), WideAmount::from_e8s(1_500_000), "exact sum of 15 returns");
    assert!(agg_total(&payer()).is_zero());
    // Per-pass budget: many senders over the threshold are refunded REFUNDS_PER_PASS at a time.
    for n in 0..20u64 {
        apply_ret(&ret_vol(100 + n, &format!("rBudget{n:03}"), None, "5", 2_000));
    }
    assert_eq!(drain().len(), REFUNDS_PER_PASS);
    assert_eq!(drain().len(), REFUNDS_PER_PASS);
    assert_eq!(drain().len(), 20 - 2 * REFUNDS_PER_PASS);
    assert!(drain().is_empty());
    // The system cap defers refunds (value stays held) and NEVER blocks a user admit.
    mutate_state(|s| {
        let st = &mut s.xrpl_icusd;
        let mut id = 1_000_000;
        while st.system_refund_ops.len() < MAX_SYSTEM_REFUND_OPS {
            id += 1;
            let op = fat_operation(id, system_owner(), true);
            st.request_nonce_index.insert((op.owner, [0; 32]), id);
            st.operations.insert(id, op);
            st.register_op(id);
        }
    });
    apply_ret(&ret_vol(500, &other(), None, "5", 2_000));
    assert!(drain().is_empty(), "system refund cap reached: deferred");
    assert_eq!(agg_total(&other()), WideAmount::from_e8s(5 * E8), "still held, still backed");
    assert!(read_state(|s| s
        .xrpl_icusd
        .admit(&owner(1), &[44; 32], TESTNET, &OperationKind::Import { amount_e8s: 1, recipient: payer() })
        .is_ok()));
    read_state(|s| assert_eq!(buckets_by_full_scan(&s.xrpl_icusd), s.xrpl_icusd.buckets().unwrap()));
}

#[test]
fn value_bearing_quarantine_keeps_o_in_step_with_the_ledger_and_restores_coverage() {
    // The issuer's real obligations shrank by 40 (a path payment whose sender line
    // delta does not match), so the XRPL snapshot is 60 while recorded O is 100.
    replace_state(rworld(100 * E8, 0, 100 * E8, Decimal::ZERO));
    let cov = |snap: u64| CoverageInput {
        escrow_balance_e8s: 1_000 * E8 as u128,
        snapshot: Some(super::state::SnapshotSummary {
            ledger_index: 3_000,
            total: WideAmount::from_e8s(snap),
        }),
        unprocessed_return_allowance: WideAmount::ZERO,
    };
    let before = read_state(|s| s.xrpl_icusd.check_coverage(&cov(60 * E8)).unwrap());
    assert!(matches!(before.status, CoverageStatus::SnapshotDisagrees { .. }));
    // Disagreement below O wakes the observer (once per coverage record).
    mutate_state(|s| {
        s.xrpl_icusd.last_coverage = Some(CoverageRecord { checked_at_ns: NOW, report: before.clone() });
    });
    read_state(|s| {
        assert!(s.xrpl_icusd.coverage_scan_wanted());
        assert!(s.xrpl_icusd.observer_has_work(), "no open intent, yet the scan runs");
        assert!(super::plan_tick(&s.xrpl_icusd, 6, 0).contains(&super::TickItem::Observer));
    });
    // The offending receipt arrives (scan or hint): value-bearing but unverifiable.
    let mut pathy = ret_vol(900, &payer(), None, "40", 2_500);
    pathy.balance_changes[0].final_low_balance = pathy.balance_changes[0].previous_low_balance;
    assert!(matches!(apply_ret(&pathy), ReturnOutcome::Quarantined(_)));
    read_state(|s| {
        let st = &s.xrpl_icusd;
        assert_eq!(st.issued_outstanding, WideAmount::from_e8s(60 * E8), "O tracks the ledger");
        assert_eq!(st.quarantined_receipts.len(), 1);
        let b = st.buckets().unwrap();
        assert_eq!(b.h, WideAmount::from_e8s(40 * E8), "the value is held, part of H");
        assert_eq!(buckets_by_full_scan(st), b);
        // Coverage now agrees with the ledger and still requires full backing.
        let after = st.check_coverage(&cov(60 * E8)).unwrap();
        assert_eq!(after.status, CoverageStatus::Covered);
        assert_eq!(after.quarantined_e8s, 40 * E8 as u128);
        assert_eq!(after.holder_backing_e8s, 100 * E8 as u128, "O + H, nothing released");
        // No auto-refund, no debt credit.
        assert!(!st.refund_drain_possible());
    });
    assert!(drain().is_empty());
    assert_eq!(debt_of(1), 100 * E8);
    // Seeing the same transaction again is a no-op.
    assert_eq!(apply_ret(&pathy), ReturnOutcome::Duplicate);
}

#[test]
fn controller_reconcile_re_verifies_then_books_to_the_verified_sender() {
    replace_state(rworld(100 * E8, 0, 100 * E8, Decimal::ZERO));
    let mut pathy = ret_vol(901, &payer(), None, "40", 2_500);
    // Metadata: the sender's line did not move, but another holder's line (rippled
    // through) shrank by exactly the delivered amount: obligations fell by 40.
    pathy.balance_changes[0].final_low_balance = pathy.balance_changes[0].previous_low_balance;
    let before = ExactIssuedAmount::parse("500").unwrap();
    pathy.balance_changes.push(TrustLineBalanceChange {
        low_account: other(),
        high_account: issuer_addr(),
        previous_low_balance: before,
        final_low_balance: before.checked_sub(&ExactIssuedAmount::parse("40").unwrap()).unwrap(),
    });
    apply_ret(&pathy);
    let h = hash_n(901);
    // Wrong evidence is refused with no change.
    let mut bad = pathy.clone();
    bad.delivered_amount = ExactIssuedAmount::parse("39").unwrap();
    let e = block_on(observer::reconcile_quarantined_with(
        &Mock::default().with_receipt(&bad),
        &h,
        NOW + 9,
    ))
    .unwrap_err();
    assert!(matches!(e, XrplIcusdError::InvalidArgument(_)), "{e:?}");
    let e = block_on(observer::reconcile_quarantined_with(&Mock::default(), &h, NOW + 9)).unwrap_err();
    assert!(matches!(e, XrplIcusdError::Unavailable(_)), "unverifiable receipts book nothing");
    assert_eq!(read_state(|s| s.xrpl_icusd.quarantined_receipts.len()), 1);
    // A receipt whose metadata does NOT show the issuer's obligations shrinking by the
    // delivered amount stays quarantined (no assertion-only adjustment).
    let mut vague = pathy.clone();
    vague.balance_changes.pop();
    let e = block_on(observer::reconcile_quarantined_with(
        &Mock::default().with_receipt(&vague),
        &h,
        NOW + 9,
    ))
    .unwrap_err();
    assert!(matches!(e, XrplIcusdError::InvalidArgument(_)), "{e:?}");
    // The genuine re-verified receipt books the value to the verified sender.
    let view = block_on(observer::reconcile_quarantined_with(
        &Mock::default().with_receipt(&pathy),
        &h,
        NOW + 9,
    ))
    .expect("booked");
    assert_eq!(view.amount, "40");
    read_state(|s| {
        assert!(s.xrpl_icusd.quarantined_receipts.is_empty());
        assert_eq!(s.xrpl_icusd.issued_outstanding, WideAmount::from_e8s(60 * E8), "O unchanged by booking");
    });
    assert_eq!(agg_total(&payer()), WideAmount::from_e8s(40 * E8));
    assert_eq!(liabilities(), 100 * E8 as u128, "the liability sum never moved");
    assert_eq!(debt_of(1), 100 * E8, "still no debt credit");
    // Once only.
    let e = block_on(observer::reconcile_quarantined_with(
        &Mock::default().with_receipt(&pathy),
        &h,
        NOW + 10,
    ))
    .unwrap_err();
    assert_eq!(e, XrplIcusdError::NotFound);
}

#[test]
fn observer_stops_polling_after_catching_up_on_a_coverage_record() {
    replace_state(rworld(100 * E8, 0, 100 * E8, Decimal::ZERO));
    let report = read_state(|s| {
        s.xrpl_icusd
            .check_coverage(&CoverageInput {
                escrow_balance_e8s: 1_000 * E8 as u128,
                snapshot: Some(super::state::SnapshotSummary {
                    ledger_index: 3_000,
                    total: WideAmount::from_e8s(60 * E8),
                }),
                unprocessed_return_allowance: WideAmount::ZERO,
            })
            .unwrap()
    });
    mutate_state(|s| s.xrpl_icusd.last_coverage = Some(CoverageRecord { checked_at_ns: NOW, report }));
    // Caught up (cursor at the tip, nothing new): the pass records it and goes quiet.
    mutate_state(|s| s.xrpl_icusd.observer.next_ledger_min = Some(5_000));
    let m = Mock::default().with_tip(4_000);
    assert_eq!(pass(&m), PassOutcome::CaughtUp);
    read_state(|s| {
        assert!(!s.xrpl_icusd.coverage_scan_wanted());
        assert!(!s.xrpl_icusd.observer_has_work(), "no polling until the next coverage record");
    });
}

#[test]
fn terminal_operations_are_pruned_but_dedup_and_nonces_survive() {
    replace_state(rworld(100 * E8, 0, 300 * E8, Decimal::ZERO));
    // A repayment intent that expires and an old delivered import.
    let id = intent(1, 10 * E8, 1, false);
    let tag = tag_of(id);
    mutate_state(|s| {
        repay::expire_stale_intents(&mut s.xrpl_icusd, NOW + INTENT_TTL_NS + 1);
    });
    assert_eq!(op_phase(id), Phase::Cancelled);
    let imp = mutate_state(|s| {
        let st = &mut s.xrpl_icusd;
        let mut op = fat_operation(50, owner(2), false);
        op.kind = OperationKind::Import { amount_e8s: E8, recipient: payer() };
        op.request_nonce = [50; 32];
        op.ic_jobs.truncate(1);
        st.request_nonce_index.insert((op.owner, op.request_nonce), 50);
        st.operations.insert(50, op);
        st.register_op(50);
        50u64
    });
    // A consumed return referencing the intent stays in the dedup map.
    let r = ret_vol(7, &payer(), Some(tag), "1", 2_000);
    apply_ret(&r);
    let later = NOW + INTENT_TTL_NS + OP_RETENTION_NS + 10;
    // Not before the retention window.
    assert_eq!(mutate_state(|s| s.xrpl_icusd.maintain(NOW + 1)), 0);
    let pruned = mutate_state(|s| s.xrpl_icusd.maintain(later));
    assert_eq!(pruned, 2, "the expired intent and the delivered import");
    read_state(|s| {
        let st = &s.xrpl_icusd;
        assert!(st.op(id).is_none() && st.op(imp).is_none());
        assert!(!st.tag_index.contains_key(&tag), "the tag mapping is released (never reused)");
        assert!(!st.live_ops.contains(&id) && !st.vault_ops.contains_key(&1)
            || st.vault_ops.get(&1).is_none_or(|v| !v.contains(&id)));
        assert_eq!(buckets_by_full_scan(st), st.buckets().unwrap());
        assert!(st.processed_returns.contains_key(&repay::processed_key(TESTNET, &issuer_addr(), &r.hash)));
    });
    // Dedup survives: the same return again changes nothing.
    assert_eq!(apply_ret(&r), ReturnOutcome::Duplicate);
    // The import's nonce stays spent: a replayed request cannot create a second one.
    let again = read_state(|s| {
        s.xrpl_icusd.admit(
            &owner(2),
            &[50; 32],
            TESTNET,
            &OperationKind::Import { amount_e8s: E8, recipient: payer() },
        )
    });
    assert_eq!(again, Err(XrplIcusdError::NonceConflict));
    // An unknown (pruned) tag is simply an unattributed return for the sender.
    assert_eq!(
        apply_ret(&ret_vol(8, &payer(), Some(tag), "1", 2_001)),
        ReturnOutcome::Unattributed
    );
}

#[test]
fn compaction_drops_old_dedup_records_without_ever_double_counting() {
    replace_state(rworld(100 * E8, 0, 100 * E8, Decimal::ZERO));
    let old = ret_vol(1, &payer(), None, "1", 1_600);
    let recent = ret_vol(2, &payer(), None, "1", 2_000);
    let ahead = ret_vol(3, &payer(), None, "1", 2_100); // hinted ahead of the scan
    for r in [&old, &recent, &ahead] {
        assert_eq!(apply_ret(r), ReturnOutcome::Unattributed);
    }
    assert_eq!(processed_count(), 3);
    // No fully-scanned frontier yet: nothing may be compacted.
    mutate_state(|s| s.xrpl_icusd.compact_processed_returns());
    assert_eq!(processed_count(), 3);
    // The scan has fully covered [.., 2_000): everything behind the retention window
    // goes, a window of PROCESSED_RETAIN_LEDGERS is kept, anything ahead is never touched.
    mutate_state(|s| s.xrpl_icusd.observer.next_ledger_min = Some(2_000 + PROCESSED_RETAIN_LEDGERS));
    mutate_state(|s| s.xrpl_icusd.compact_processed_returns());
    assert_eq!(processed_count(), 2, "only the record below frontier - window is dropped");
    let o_after = read_state(|s| s.xrpl_icusd.issued_outstanding);
    // The dropped record can neither be re-applied by a hint nor by the scan.
    assert_eq!(apply_ret(&old), ReturnOutcome::Duplicate, "below the dedup floor");
    assert_eq!(read_state(|s| s.xrpl_icusd.issued_outstanding), o_after, "no double credit");
    assert_eq!(apply_ret(&recent), ReturnOutcome::Duplicate);
    assert_eq!(apply_ret(&ahead), ReturnOutcome::Duplicate);
    // Near the hard cap the compaction is aggressive (everything behind the frontier).
    mutate_state(|s| {
        let floor = s.xrpl_icusd.dedup_floor_ledger;
        assert!(floor > 1_600 && floor <= 2_000 + PROCESSED_RETAIN_LEDGERS);
        s.xrpl_icusd.observer.next_ledger_min = Some(2_050);
        for i in 0..(MAX_PROCESSED_RETURNS as u64 / 10 * 9) {
            s.xrpl_icusd.processed_returns.insert(
                repay::processed_key(TESTNET, &issuer_addr(), &hash_n(10_000 + i)),
                ProcessedReturn {
                    disposition: ReturnDisposition::Aggregated { reason: "x".into() },
                    ledger_index: 2_040,
                    sender: payer(),
                    destination_tag: None,
                    delivered: "1".into(),
                    recorded_at_ns: NOW,
                },
            );
        }
    });
    let n = processed_count();
    assert!(n >= MAX_PROCESSED_RETURNS / 10 * 9);
    mutate_state(|s| s.xrpl_icusd.compact_processed_returns());
    assert!(processed_count() < n, "pressure compaction relieves the cap");
    // Frontier never moves the floor backwards or past the frontier.
    read_state(|s| assert!(s.xrpl_icusd.dedup_floor_ledger <= 2_050.max(2_000 + PROCESSED_RETAIN_LEDGERS)));
}

#[test]
fn indexes_agree_with_a_full_scan_and_survive_upgrade_and_replay() {
    replace_state(rworld(100 * E8, 0, 300 * E8, Decimal::ZERO));
    let id = intent(1, 40 * E8, 1, false);
    apply_ret(&ret(1, &payer(), Some(tag_of(id)), "40"));
    assert!(matches!(settle(id), SettleOutcome::BurnAdmitted { .. }));
    read_state(|s| {
        let st = &s.xrpl_icusd;
        assert!(st.vault_has_open_operation(1));
        assert!(!st.vault_has_open_operation(3));
        assert_eq!(buckets_by_full_scan(st), st.buckets().unwrap());
    });
    upgrade_roundtrip();
    read_state(|s| {
        let st = &s.xrpl_icusd;
        assert!(st.live_ops.contains(&id));
        assert!(st.vault_has_open_operation(1));
        assert_eq!(buckets_by_full_scan(st), st.buckets().unwrap());
    });
    // Settled terminal operations drop out of the hot set but remain queryable.
    prove_burn(id, 5);
    advance_at(id, NOW + 50);
    mutate_state(|s| s.xrpl_icusd.maintain(NOW + 60));
    read_state(|s| {
        let st = &s.xrpl_icusd;
        assert!(st.op(id).is_some());
        assert!(!st.vault_has_open_operation(1), "terminal repayment no longer holds the vault");
        assert_eq!(buckets_by_full_scan(st), st.buckets().unwrap());
    });
}

#[test]
fn repay_settled_event_carries_what_consumers_need() {
    replace_state(rworld(100 * E8, 0, 100 * E8, Decimal::ZERO));
    set_vault(2, 100 * E8, 0);
    let id = inject_verified(2, 40 * E8, 40 * E8, false);
    assert!(matches!(settle(id), SettleOutcome::BurnAdmitted { .. }));
    prove_burn(id, 321);
    advance_at(id, NOW + 100);
    let settled: Vec<XrplIcusdEvent> = crate::storage::events()
        .filter_map(|e| match e {
            crate::event::Event::XrplIcusd(x @ XrplIcusdEvent::RepaySettled { .. }) => Some(x),
            _ => None,
        })
        .collect();
    assert_eq!(settled.len(), 1);
    match &settled[0] {
        XrplIcusdEvent::RepaySettled {
            vault_id,
            owner: o,
            applied_e8s,
            interest_share_e8s,
            burn_block,
            ..
        } => {
            assert_eq!(*vault_id, 2);
            assert_eq!(*o, owner(2));
            assert_eq!(*applied_e8s, 40 * E8);
            assert_eq!(*interest_share_e8s, 0);
            assert_eq!(*burn_block, 321);
        }
        _ => unreachable!(),
    }
    // No RepayToVault event exists for it (replay would apply the debt a second time).
    assert_eq!(
        crate::storage::events()
            .filter(|e| matches!(e, crate::event::Event::RepayToVault { .. }))
            .count(),
        0
    );
}
