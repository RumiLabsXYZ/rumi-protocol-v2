//! Unit tests for the XRPL icUSD lane foundation. Everything here is pure or uses the
//! injected-outcome seams (no canister, no network): the side-effecting code is
//! structured so its decisions are testable without PocketIC (Task 7 covers e2e).

use candid::{Nat, Principal};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::TransferError;
use icrc_ledger_types::icrc2::transfer_from::TransferFromError;
use icrc_ledger_types::icrc::generic_value::ICRC3Value;

use super::issuance::{self, ImportRequest, ResolutionPlan};
use super::ledger::*;
use super::settlement::*;
use super::state::*;
use super::types::*;
use super::*;
use crate::chains::xrp::address::classic_address_from_ed25519_pubkey;
use crate::chains::xrp::issued_amount::ExactIssuedAmount;
use crate::chains::xrp::issued_rpc::{CreditEligibility, VerifiedIssuedReceipt};
use crate::chains::xrp::issuer_rpc::{
    ExpiryEvidence, IssuerAccountInfo, LSF_ALLOW_TRUSTLINE_CLAWBACK, LSF_DEFAULT_RIPPLE,
    LSF_NO_FREEZE, LSF_REQUIRE_DEST_TAG,
};
use crate::chains::xrp::network::XrplNetwork;
use crate::icrc3_proof::DecodedBlock;

// ---- fixtures ------------------------------------------------------------------------

const TESTNET: XrplNetwork = XrplNetwork::Testnet;

fn issuer_pk() -> [u8; 32] {
    [7u8; 32]
}
fn issuer_addr() -> String {
    classic_address_from_ed25519_pubkey(&issuer_pk())
}
fn holder_addr(n: u8) -> String {
    classic_address_from_ed25519_pubkey(&[n; 32])
}
fn owner(n: u8) -> Principal {
    Principal::from_slice(&[n; 10])
}
fn backend() -> Principal {
    Principal::from_slice(&[200; 10])
}
fn nonce(n: u64) -> u128 {
    ((1_000_000_000u128 + u128::from(n)) << 64) | u128::from(n)
}

fn ready_state() -> XrplIcusdState {
    let mut s = XrplIcusdState::default();
    let c = &mut s.config;
    c.enabled = true;
    c.issuance_paused = false;
    c.network = Some(TESTNET);
    c.issuer_address = Some(issuer_addr());
    c.issuer_public_key = Some(issuer_pk().to_vec());
    c.policy = Some(IssuerPolicy {
        allow_clawback: false,
        no_freeze: false,
    });
    c.issuer_setup = IssuerSetupState::Ready { ledger_index: 1 };
    c.outstanding_cap_e8s = 1_000_000 * 100_000_000;
    c.per_operation_cap_e8s = 100_000 * 100_000_000;
    c.rounding_buffer_e8s = 0;
    c.max_fee_drops = 1_000;
    c.verified_minting_account = Some(backend_default_account(backend()));
    s
}

fn import_req(owner_p: Principal, byte: u8, amount: u64) -> ImportRequest {
    ImportRequest {
        owner: owner_p,
        recipient: holder_addr(9),
        amount_e8s: amount,
        network: TESTNET,
        request_nonce: [byte; 32],
    }
}

fn new_import(s: &mut XrplIcusdState, owner_p: Principal, byte: u8, amount: u64) -> OperationId {
    issuance::create_import(
        s,
        &import_req(owner_p, byte, amount),
        nonce(u64::from(byte)),
        100,
        backend(),
        1_000,
    )
    .unwrap()
    .0
}

/// Drive job 0 of an import to a PROVEN Done and advance to a delivery claim.
fn fund_import(s: &mut XrplIcusdState, id: OperationId) {
    s.begin_job_call(id, 0).unwrap();
    s.finish_job_call(id, 0, LedgerCallResult::Ok(500), 2_000).unwrap();
    s.mark_proof_verified(id, 0).unwrap();
    assert_eq!(
        issuance::advance_import(s, id, 2_000).unwrap(),
        issuance::ImportAdvance::Funded
    );
}

fn borrow_reservation(s: &mut XrplIcusdState, id: u64, vault: u64, gross: u64, fee: u64) {
    s.apply_reservation_begun(
        id,
        owner(1),
        &[id as u8; 32],
        TESTNET,
        vault,
        Principal::from_slice(&[0xAA]),
        gross,
        fee,
        gross - fee,
        &holder_addr(9),
        1,
        10,
    );
}

fn prepared_payment(s: &XrplIcusdState, id: OperationId, seq: u32) -> IssuerTxAttempt {
    let op = s.op(id).unwrap();
    let amount = ExactIssuedAmount::try_from_e8s(op.kind.claim_e8s().unwrap()).unwrap();
    prepare_attempt(&PrepareInput {
        purpose: AttemptPurpose::Delivery,
        tx: AttemptTx::Payment {
            destination: op.kind.recipient().unwrap().to_string(),
            amount,
            destination_tag: None,
        },
        issuer_address: &issuer_addr(),
        issuer_pubkey: issuer_pk(),
        sequence: seq,
        prepared_ledger: 1_000,
        fee_drops: 12,
        max_fee_drops: 1_000,
        now_ns: 3_000,
    })
    .unwrap()
}

fn sign_and_submit(s: &mut XrplIcusdState, id: OperationId) {
    let h = LeaseHolder::Operation(id);
    s.record_signature(h, &[9u8; 64], &issuer_pk(), 3_100).unwrap();
    s.record_submit(h, 3_200, None).unwrap();
}

fn receipt_for(a: &IssuerTxAttempt) -> VerifiedIssuedReceipt {
    let AttemptTx::Payment {
        destination, amount, ..
    } = &a.tx
    else {
        panic!("not a payment")
    };
    VerifiedIssuedReceipt {
        hash: a.tx_hash.clone().unwrap(),
        ledger_index: 1_010,
        ledger_hash: "A".repeat(64),
        validated: true,
        engine_result: "tesSUCCESS".into(),
        account: a.account.clone(),
        destination: destination.clone(),
        destination_tag: None,
        flags: TF_FULLY_CANONICAL,
        nominal_amount: Some(*amount),
        delivered_amount: *amount,
        has_paths: false,
        has_send_max: false,
        balance_changes: vec![],
        provider_evidence: vec![],
    }
}

fn succeeded(a: &IssuerTxAttempt) -> Observation {
    let r = receipt_for(a);
    Observation {
        tx: TxView::Succeeded {
            ledger_index: r.ledger_index,
            tx_type: Some("Payment".into()),
            account: Some(r.account.clone()),
            payment: Some(Box::new(r)),
        },
        expiry: None,
    }
}

/// Take a funded import all the way to Delivered; returns nothing, asserts effects.
fn deliver(s: &mut XrplIcusdState, id: OperationId, seq: u32) {
    let a = prepared_payment(s, id, seq);
    s.begin_attempt(LeaseHolder::Operation(id), a).unwrap();
    sign_and_submit(s, id);
    let att = s.lease_attempt(LeaseHolder::Operation(id)).unwrap().clone();
    let v = reconcile(&att, &succeeded(&att));
    assert!(matches!(v, Reconciled::Validated { .. }), "{v:?}");
    assert_eq!(
        s.apply_reconcile(LeaseHolder::Operation(id), v, 4_000).unwrap(),
        ReconcileEffect::Delivered
    );
}

fn plain_block(op: &str, amount: u64, memo: &[u8], cat: u64) -> DecodedBlock {
    DecodedBlock {
        btype: None,
        op: op.into(),
        from: None,
        to: None,
        spender: None,
        amount: u128::from(amount),
        fee: None,
        created_at_time: Some(cat),
        memo: Some(memo.to_vec()),
    }
}

fn test_init_state() -> crate::state::State {
    crate::state::State::from(crate::InitArg {
        xrc_principal: Principal::anonymous(),
        icusd_ledger_principal: Principal::anonymous(),
        icp_ledger_principal: Principal::anonymous(),
        fee_e8s: 0,
        developer_principal: Principal::anonymous(),
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    })
}

// ---- WideAmount -----------------------------------------------------------------------

#[test]
fn wide_amount_is_exact_and_carries() {
    let a = WideAmount::from_exact(&ExactIssuedAmount::parse("1.00000000").unwrap()).unwrap();
    assert_eq!(a, WideAmount::from_e8s(100_000_000));
    // 1e-8 icUSD = 1 e8s; a sub-e8s dust of 1e-20 is exact.
    let dust = WideAmount::from_exact(&ExactIssuedAmount::parse("0.0000000000000000001").unwrap()).unwrap();
    assert!(dust.whole_e8s == 0 && dust.frac_units > 0);
    let sum = a.checked_add(&dust).unwrap();
    assert_eq!(sum.ceil_e8s(), 100_000_001);
    assert_eq!(sum.checked_sub(&dust).unwrap(), a);
    // borrow across the e8s boundary
    let half = WideAmount {
        whole_e8s: 0,
        frac_units: UNITS_PER_E8S / 2,
    };
    let two_half = half.checked_add(&half).unwrap();
    assert_eq!(two_half, WideAmount::from_e8s(1));
    assert_eq!(
        WideAmount::from_e8s(1).checked_sub(&half).unwrap(),
        half
    );
    assert_eq!(a.checked_sub(&sum), Err(WideError::Underflow));
    // finer than 1e-40 is refused, never rounded
    let fine = ExactIssuedAmount::parse("1e-50").unwrap();
    assert!(WideAmount::from_exact(&fine).is_err());
    assert_eq!(WideAmount::from_e8s(150_000_000).to_decimal_string(), "1.5");
    assert_eq!(WideAmount::ZERO.to_decimal_string(), "0");
}

// ---- snapshot / event compatibility ------------------------------------------------------

#[test]
fn old_snapshot_defaults_lane_disabled() {
    use ciborium::value::Value;
    let state = test_init_state();
    // Serialize a current State, then DROP the lane's key: this is exactly what a
    // pre-change snapshot looks like to the decoder.
    let mut v = Value::serialized(&state).expect("state to cbor value");
    let Value::Map(entries) = &mut v else {
        panic!("state is a map")
    };
    let before = entries.len();
    entries.retain(|(k, _)| k.as_text() != Some("xrpl_icusd"));
    assert_eq!(entries.len() + 1, before, "the lane key existed and was removed");
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&v, &mut bytes).unwrap();
    let decoded: crate::state::State = ciborium::de::from_reader(bytes.as_slice()).expect("old snapshot decodes");
    let lane = &decoded.xrpl_icusd;
    assert!(!lane.config.enabled);
    assert!(lane.config.issuance_paused);
    assert!(lane.config.policy.is_none());
    assert_eq!(lane.config.issuer_setup, IssuerSetupState::NotStarted);
    assert!(lane.operations.is_empty());
    assert!(lane.ensure_ready().is_err());
    assert!(!lane.has_work());
    assert_eq!(lane.durable_reserved_total(), 0);
}

#[test]
fn lane_state_survives_a_cbor_roundtrip() {
    let mut s = ready_state();
    let id = new_import(&mut s, owner(1), 1, 5_000);
    fund_import(&mut s, id);
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&s, &mut bytes).unwrap();
    let back: XrplIcusdState = ciborium::de::from_reader(bytes.as_slice()).unwrap();
    assert_eq!(s, back);
}

#[test]
fn event_replay_preserves_operation_owner_and_proofs() {
    let vault = 42u64;
    let events = vec![
        crate::event::Event::Init(crate::InitArg {
            xrc_principal: Principal::anonymous(),
            icusd_ledger_principal: Principal::anonymous(),
            icp_ledger_principal: Principal::anonymous(),
            fee_e8s: 0,
            developer_principal: Principal::anonymous(),
            treasury_principal: None,
            stability_pool_principal: None,
            ckusdt_ledger_principal: None,
            ckusdc_ledger_principal: None,
        }),
        crate::event::Event::XrplIcusd(XrplIcusdEvent::ReservationBegun {
            op_id: 3,
            owner: owner(5),
            request_nonce: vec![0x33; 32],
            network: TESTNET,
            vault_id: vault,
            collateral_type: Principal::from_slice(&[0xAA]),
            gross_e8s: 1_000,
            fee_e8s: 10,
            net_e8s: 990,
            recipient: holder_addr(9),
            policy_version: 4,
            timestamp: 111,
        }),
    ];
    let mid = crate::event::replay_with_nonce_time(events.clone().into_iter(), || 7).unwrap();
    let lane = &mid.xrpl_icusd;
    let op = lane.op(3).expect("reservation rebuilt from the log");
    assert_eq!(op.owner, owner(5), "owner preserved");
    assert_eq!(op.request_nonce, [0x33; 32], "request nonce preserved");
    assert_eq!(op.phase, Phase::Prepared);
    assert_eq!(lane.request_nonce_index.get(&(owner(5), [0x33; 32])), Some(&3));
    assert_eq!(lane.durable_reserved_total(), 1_000);
    assert!(xrpl_vault_excluded(&mid, vault));
    assert!(lane.next_operation_id >= 4);

    // Released with the debt committed: exclusion gone, operation handed to delivery.
    let mut with_release = events.clone();
    with_release.push(crate::event::Event::XrplIcusd(XrplIcusdEvent::ReservationReleased {
        op_id: 3,
        vault_id: vault,
        reason: ReleaseReason::DebtCommitted,
        timestamp: 112,
    }));
    let s2 = crate::event::replay_with_nonce_time(with_release.into_iter(), || 7).unwrap();
    assert_eq!(s2.xrpl_icusd.op(3).unwrap().phase, Phase::DebtCommitted);
    assert!(!xrpl_vault_excluded(&s2, vault));
    assert_eq!(s2.xrpl_icusd.durable_reserved_total(), 0);

    // A definite mint failure cancels with no debt.
    let mut with_fail = events;
    with_fail.push(crate::event::Event::XrplIcusd(XrplIcusdEvent::ReservationReleased {
        op_id: 3,
        vault_id: vault,
        reason: ReleaseReason::MintFailed,
        timestamp: 112,
    }));
    let s3 = crate::event::replay_with_nonce_time(with_fail.into_iter(), || 7).unwrap();
    assert_eq!(s3.xrpl_icusd.op(3).unwrap().phase, Phase::Cancelled);
    assert!(!xrpl_vault_excluded(&s3, vault));
}

#[test]
fn replay_rebuilds_freeze_before_accrual_event() {
    let vault = 8u64;
    let init = crate::event::Event::Init(crate::InitArg {
        xrc_principal: Principal::anonymous(),
        icusd_ledger_principal: Principal::anonymous(),
        icp_ledger_principal: Principal::anonymous(),
        fee_e8s: 0,
        developer_principal: Principal::anonymous(),
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    });
    let cutoff = crate::event::Event::XrplIcusd(XrplIcusdEvent::RepayCutoffBegun {
        op_id: 1,
        owner: owner(2),
        request_nonce: vec![1; 32],
        network: TESTNET,
        vault_id: vault,
        payer: holder_addr(9),
        destination_tag: 77,
        max_e8s: 500,
        full_close: false,
        cutoff_ns: 1_000,
        snapshot_debt_e8s: 900,
        snapshot_accrued_e8s: 40,
        repay_e8s: 500,
        interest_share_e8s: 20,
        last_accrual_time: 900,
    });
    let accrue = crate::event::Event::AccrueInterest { timestamp: 2_000 };
    // Unresolved burn: the freeze/exclusion are in place when AccrueInterest replays.
    let s = crate::event::replay_with_nonce_time(
        vec![init.clone(), cutoff.clone(), accrue.clone()].into_iter(),
        || 7,
    )
    .unwrap();
    assert!(xrpl_accrual_frozen(&s, vault));
    assert!(xrpl_vault_excluded(&s, vault));
    let op = s.xrpl_icusd.op(1).unwrap();
    assert_eq!(op.owner, owner(2));
    match &op.kind {
        OperationKind::Repayment { cutoff, .. } => {
            let c = cutoff.as_ref().unwrap();
            assert_eq!((c.snapshot_debt_e8s, c.repay_e8s, c.interest_share_e8s), (900, 500, 20));
        }
        k => panic!("{k:?}"),
    }
    // Settled: both lifted, proof (burn block) recorded in the event itself.
    let settled = crate::event::Event::XrplIcusd(XrplIcusdEvent::RepaySettled {
        op_id: 1,
        vault_id: vault,
        burn_block: 4_321,
        settled_at: 3_000,
    });
    let s2 = crate::event::replay_with_nonce_time(
        vec![init.clone(), cutoff.clone(), accrue.clone(), settled].into_iter(),
        || 7,
    )
    .unwrap();
    assert!(!xrpl_accrual_frozen(&s2, vault) && !xrpl_vault_excluded(&s2, vault));
    assert_eq!(s2.xrpl_icusd.op(1).unwrap().phase, Phase::DebtApplied);
    // Cancelled burn: lifted and held.
    let cancelled = crate::event::Event::XrplIcusd(XrplIcusdEvent::RepayCancelled {
        op_id: 1,
        vault_id: vault,
        at: 3_000,
    });
    let s3 = crate::event::replay_with_nonce_time(vec![init, cutoff, accrue, cancelled].into_iter(), || 7).unwrap();
    assert!(!xrpl_accrual_frozen(&s3, vault) && !xrpl_vault_excluded(&s3, vault));
    assert_eq!(s3.xrpl_icusd.op(1).unwrap().phase, Phase::Held);
}

#[test]
fn lane_events_roundtrip_through_cbor_and_candid() {
    let ev = crate::event::Event::XrplIcusd(XrplIcusdEvent::ReservationBegun {
        op_id: 1,
        owner: owner(1),
        request_nonce: vec![5; 32],
        network: TESTNET,
        vault_id: 2,
        collateral_type: Principal::from_slice(&[0xAA]),
        gross_e8s: 3,
        fee_e8s: 1,
        net_e8s: 2,
        recipient: holder_addr(9),
        policy_version: 1,
        timestamp: 9,
    });
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&ev, &mut bytes).unwrap();
    let back: crate::event::Event = ciborium::de::from_reader(bytes.as_slice()).unwrap();
    assert_eq!(ev, back);
    let enc = candid::encode_one(&ev).unwrap();
    let dec: crate::event::Event = candid::decode_one(&enc).unwrap();
    assert_eq!(ev, dec);
    assert!(ev.is_vault_related(&2) && !ev.is_vault_related(&3));
    assert_eq!(ev.timestamp_ns(), Some(9));
    assert!(ev.involves_principal(&owner(1)));
    assert!(!ev.is_accrue_interest());
}

// ---- escrow identity ----------------------------------------------------------------------

#[test]
fn escrow_is_not_minting_account() {
    use sha2::{Digest, Sha256};
    let expected: [u8; 32] = Sha256::digest(b"rumi:xrpl-icusd:escrow:v1").into();
    assert_eq!(escrow_subaccount(), expected);
    assert_ne!(escrow_subaccount(), [0u8; 32]);
    let b = backend();
    assert_ne!(escrow_account(b), backend_default_account(b));
    // The ledger's minting account must be the backend default account...
    assert_eq!(
        check_minting_account(Some(backend_default_account(b)), b).unwrap(),
        backend_default_account(b)
    );
    // ...never escrow, never somebody else's, never missing.
    assert!(matches!(
        check_minting_account(Some(escrow_account(b)), b),
        Err(XrplIcusdError::MintingAccount(m)) if m.contains("escrow")
    ));
    assert!(check_minting_account(None, b).is_err());
    assert!(check_minting_account(
        Some(Account {
            owner: owner(3),
            subaccount: None
        }),
        b
    )
    .is_err());
    // A subaccount of all zeroes IS the default account.
    assert!(check_minting_account(
        Some(Account {
            owner: b,
            subaccount: Some([0; 32])
        }),
        b
    )
    .is_ok());
}

// ---- ledger jobs ------------------------------------------------------------------------------

fn borrow_with_mint_job(s: &mut XrplIcusdState) -> (OperationId, usize) {
    borrow_reservation(s, 1, 10, 1_000, 10);
    let idx = s
        .add_job(1, mint_job_spec(backend(), 990, 100), nonce(77), 50)
        .unwrap();
    (1, idx)
}

#[test]
fn mint_reply_lost_reuses_nonce() {
    let mut s = ready_state();
    let (id, idx) = borrow_with_mint_job(&mut s);
    let first = s.begin_job_call(id, idx).unwrap();
    // The reply is lost: transport failure -> Unknown, never Failed.
    let e = s
        .finish_job_call(
            id,
            idx,
            classify_transfer(Err((4, "canister did not reply".into()))),
            60,
        )
        .unwrap();
    assert!(matches!(e, JobEffect::Unknown(_)));
    assert!(matches!(s.job(id, idx).unwrap().status, IcJobStatus::Unknown { .. }));
    // The retry re-sends the IDENTICAL persisted tuple.
    let second = s.begin_job_call(id, idx).unwrap();
    assert_eq!(first.op_nonce, second.op_nonce);
    assert_eq!(first.created_at_time, second.created_at_time);
    assert_eq!(first.memo, second.memo);
    assert_eq!((first.to, first.amount_e8s), (second.to, second.amount_e8s));
    assert_eq!(first.from_subaccount, second.from_subaccount);
    assert_eq!(second.sends, 2);
    assert_eq!(s.op(id).unwrap().ic_jobs.len(), 1, "no second job");
    // The ledger's dedup answers Duplicate -> Done at the original block.
    let dup = classify_transfer(Ok(Err(TransferError::Duplicate {
        duplicate_of: Nat::from(321u64),
    })));
    assert_eq!(dup, LedgerCallResult::Duplicate(321));
    assert_eq!(
        s.finish_job_call(id, idx, dup, 70).unwrap(),
        JobEffect::Done(321)
    );
    assert_eq!(s.job(id, idx).unwrap().done_block(), Some(321));
    // The nonce is timestamp<<64|counter: created_at_time derives from it.
    assert_eq!(second.created_at_time, (nonce(77) >> 64) as u64);
    // A mint is from the default account to escrow.
    assert_eq!(second.from_subaccount, None);
    assert_eq!(second.to, escrow_account(backend()));
}

#[test]
fn burn_reply_lost_no_second_burn() {
    let mut s = ready_state();
    borrow_reservation(&mut s, 1, 10, 1_000, 10);
    let idx = s
        .add_job(1, burn_job_spec(backend(), 500, 100), nonce(88), 50)
        .unwrap();
    let a = s.begin_job_call(1, idx).unwrap();
    s.finish_job_call(1, idx, LedgerCallResult::Unknown("lost".into()), 60).unwrap();
    let b = s.begin_job_call(1, idx).unwrap();
    assert_eq!((a.op_nonce, a.created_at_time, a.memo.clone()), (b.op_nonce, b.created_at_time, b.memo.clone()));
    s.finish_job_call(1, idx, LedgerCallResult::Duplicate(900), 70).unwrap();
    // Resolved: it can never be sent again, so there is no second burn.
    assert!(s.begin_job_call(1, idx).is_err());
    assert_eq!(s.op(1).unwrap().ic_jobs.len(), 1);
    // The burn block is recognised by its exact tuple (escrow -> minting, memo, time).
    let mut blk = plain_block("burn", 500, &b.memo, b.created_at_time);
    blk.from = Some(Account {
        owner: backend(),
        subaccount: Some(escrow_subaccount()),
    });
    assert_eq!(job_matches_block(&b, backend(), &blk), BlockMatch::Exact);
    // A burn from the DEFAULT account (not escrow) is not this burn.
    let mut wrong = blk.clone();
    wrong.from = Some(backend_default_account(backend()));
    assert!(matches!(job_matches_block(&b, backend(), &wrong), BlockMatch::Partial(_)));
}

#[test]
fn definite_rejections_fail_and_transient_retry() {
    let rej = classify_transfer(Ok(Err(TransferError::InsufficientFunds {
        balance: Nat::from(1u64),
    })));
    assert!(matches!(rej, LedgerCallResult::Rejected(_)));
    assert!(matches!(
        classify_transfer(Ok(Err(TransferError::CreatedInFuture { ledger_time: 1 }))),
        LedgerCallResult::Retryable(_)
    ));
    assert!(matches!(
        classify_transfer(Ok(Err(TransferError::TemporarilyUnavailable))),
        LedgerCallResult::Retryable(_)
    ));
    assert_eq!(classify_transfer(Ok(Err(TransferError::TooOld))), LedgerCallResult::TooOld);
    assert!(matches!(
        classify_transfer_from(Ok(Err(TransferFromError::InsufficientAllowance {
            allowance: Nat::from(0u64)
        }))),
        LedgerCallResult::Rejected(_)
    ));
    let mut s = ready_state();
    let (id, idx) = borrow_with_mint_job(&mut s);
    s.begin_job_call(id, idx).unwrap();
    s.finish_job_call(id, idx, rej, 5).unwrap();
    assert!(matches!(s.job(id, idx).unwrap().status, IcJobStatus::Failed { .. }));
    // Retryable keeps it Pending and sendable with the same tuple.
    let mut s = ready_state();
    let (id, idx) = borrow_with_mint_job(&mut s);
    let a = s.begin_job_call(id, idx).unwrap();
    s.finish_job_call(id, idx, LedgerCallResult::Retryable("x".into()), 5).unwrap();
    assert_eq!(s.job(id, idx).unwrap().status, IcJobStatus::Pending);
    assert_eq!(s.begin_job_call(id, idx).unwrap().op_nonce, a.op_nonce);
}

#[test]
fn dedup_window_expired_uses_archive_proof() {
    let mut s = ready_state();
    let (id, idx) = borrow_with_mint_job(&mut s); // ledger_tip_before = 100
    let job0 = s.begin_job_call(id, idx).unwrap();
    s.finish_job_call(id, idx, LedgerCallResult::TooOld, 60).unwrap();
    let j = s.job(id, idx).unwrap();
    assert!(j.scan_required && matches!(j.status, IcJobStatus::Unknown { .. }));
    // A TooOld job is NEVER re-sent (and never gets a new nonce).
    assert!(s.begin_job_call(id, idx).is_err());
    // The scan starts at ledger_tip_before and pins its end.
    let (from, end) = s.begin_scan(id, idx, 300).unwrap();
    assert_eq!((from, end), (100, 300));
    assert_eq!(s.begin_scan(id, idx, 999).unwrap(), (100, 300), "the pin never moves");

    // Page 1: unrelated blocks only -> keep scanning.
    let unrelated = vec![(100u64, plain_block("xfer", 5, b"zz", 1))];
    assert_eq!(scan_blocks(&job0, backend(), &unrelated), ScanVerdict::Clear);
    assert_eq!(
        s.finish_scan_page(id, idx, 164, ScanVerdict::Clear, 70).unwrap(),
        ScanEffect::Continue
    );
    // Page 2 (an ARCHIVED block in a later page) holds the exact mint -> Done.
    let mut mint = plain_block("mint", 990, &job0.memo, job0.created_at_time);
    mint.to = Some(escrow_account(backend()));
    assert_eq!(job_matches_block(&job0, backend(), &mint), BlockMatch::Exact);
    let page = vec![(150u64, plain_block("xfer", 1, b"q", 2)), (170u64, mint.clone())];
    let verdict = scan_blocks(&job0, backend(), &page);
    assert_eq!(verdict, ScanVerdict::Found(170));
    assert_eq!(
        s.finish_scan_page(id, idx, 228, verdict, 80).unwrap(),
        ScanEffect::Found(170)
    );
    let j = s.job(id, idx).unwrap();
    assert_eq!(j.done_block(), Some(170));
    assert!(j.proof_verified && !j.scan_required, "the scanned block was exact-matched");
    assert_eq!(j.op_nonce, job0.op_nonce, "no new nonce was ever allocated");
}

#[test]
fn archive_scan_proves_absence_only_over_the_whole_pinned_range() {
    let mut s = ready_state();
    let (id, idx) = borrow_with_mint_job(&mut s);
    s.begin_job_call(id, idx).unwrap();
    s.finish_job_call(id, idx, LedgerCallResult::TooOld, 60).unwrap();
    s.begin_scan(id, idx, 200).unwrap(); // [100, 200)
    assert_eq!(
        s.finish_scan_page(id, idx, 164, ScanVerdict::Clear, 70).unwrap(),
        ScanEffect::Continue
    );
    assert!(!s.job(id, idx).unwrap().is_resolved(), "half-scanned is NOT proof of absence");
    assert_eq!(
        s.finish_scan_page(id, idx, 200, ScanVerdict::Clear, 80).unwrap(),
        ScanEffect::ProvenAbsent
    );
    assert!(matches!(s.job(id, idx).unwrap().status, IcJobStatus::Failed { .. }));
    // The job (a never-executed mint) is a definite failure: the operation can unwind.
}

#[test]
fn scan_holds_on_plausible_but_inexact_evidence() {
    let mut s = ready_state();
    let (id, idx) = borrow_with_mint_job(&mut s);
    let job = s.begin_job_call(id, idx).unwrap();
    s.finish_job_call(id, idx, LedgerCallResult::TooOld, 60).unwrap();
    s.begin_scan(id, idx, 200).unwrap();
    // Same memo + time but the WRONG destination: never accepted, never ignored.
    let mut bad = plain_block("mint", 990, &job.memo, job.created_at_time);
    bad.to = Some(Account {
        owner: owner(66),
        subaccount: None,
    });
    let v = scan_blocks(&job, backend(), &[(120, bad)]);
    assert!(matches!(v, ScanVerdict::Conflict(_)));
    assert!(matches!(
        s.finish_scan_page(id, idx, 164, v, 70).unwrap(),
        ScanEffect::Held(_)
    ));
    assert!(!s.job(id, idx).unwrap().is_resolved());
    // Two exact copies are a conflict too.
    let mut ex = plain_block("mint", 990, &job.memo, job.created_at_time);
    ex.to = Some(escrow_account(backend()));
    assert!(matches!(
        scan_blocks(&job, backend(), &[(120, ex.clone()), (121, ex)]),
        ScanVerdict::Conflict(_)
    ));
}

#[test]
fn assemble_blocks_fails_closed() {
    let blk = |i: u64| (i, ICRC3Value::Nat(Nat::from(i)));
    // direct [10,12) + archive [12,14)
    let ok = assemble_blocks(10, 4, vec![blk(12), blk(10), blk(13), blk(11)]).unwrap();
    assert_eq!(ok.iter().map(|(i, _)| *i).collect::<Vec<_>>(), vec![10, 11, 12, 13]);
    assert!(assemble_blocks(10, 4, vec![blk(10), blk(11), blk(13)]).is_err(), "short");
    assert!(assemble_blocks(10, 3, vec![blk(10), blk(11), blk(13)]).is_err(), "gap");
    assert!(assemble_blocks(10, 3, vec![blk(10), blk(10), blk(11)]).is_err(), "dup");
    assert!(assemble_blocks(10, 2, vec![blk(10), blk(11), blk(12)]).is_err(), "extra");
}

#[test]
fn pull_block_must_match_user_spender_and_escrow() {
    let mut s = ready_state();
    let id = new_import(&mut s, owner(1), 1, 5_000);
    let job = s.job(id, 0).unwrap().clone();
    assert_eq!(job.purpose, IcJobPurpose::ImportPull);
    assert_eq!(job.to, escrow_account(backend()));
    assert_eq!(
        job.from_owner,
        Some(Account {
            owner: owner(1),
            subaccount: None
        })
    );
    assert_eq!(job.ledger_tip_before, Some(100));
    let mut blk = plain_block("xfer", 5_000, &job.memo, job.created_at_time);
    blk.from = job.from_owner;
    blk.to = Some(escrow_account(backend()));
    blk.spender = Some(backend_default_account(backend()));
    assert_eq!(job_matches_block(&job, backend(), &blk), BlockMatch::Exact);
    // Pulled from someone else / paid to the wrong place: not this pull.
    let mut other = blk.clone();
    other.from = Some(Account {
        owner: owner(2),
        subaccount: None,
    });
    assert!(matches!(job_matches_block(&job, backend(), &other), BlockMatch::Partial(_)));
    let mut wrong_to = blk.clone();
    wrong_to.to = Some(backend_default_account(backend()));
    assert!(matches!(job_matches_block(&job, backend(), &wrong_to), BlockMatch::Partial(_)));
}

#[test]
fn upgrade_normalises_in_flight_jobs() {
    let mut s = ready_state();
    let (id, idx) = borrow_with_mint_job(&mut s);
    s.begin_job_call(id, idx).unwrap();
    assert_eq!(s.job(id, idx).unwrap().status, IcJobStatus::InFlight);
    s.recover_after_upgrade();
    assert!(matches!(s.job(id, idx).unwrap().status, IcJobStatus::Unknown { .. }));
    // ...and is then resendable with the same tuple.
    assert!(s.begin_job_call(id, idx).is_ok());
}

// ---- operations, dedup, limits, reservations ------------------------------------------------

#[test]
fn request_nonce_dedup_limits_and_ownership() {
    let mut s = ready_state();
    let id = new_import(&mut s, owner(1), 1, 5_000);
    // Same nonce, same body: the same operation and no second job.
    let again = issuance::create_import(&mut s, &import_req(owner(1), 1, 5_000), nonce(500), 100, backend(), 2_000).unwrap();
    assert_eq!(again, (id, false));
    assert_eq!(s.operations.len(), 1);
    assert_eq!(s.op(id).unwrap().ic_jobs.len(), 1);
    // Same nonce, different body: conflict.
    assert_eq!(
        issuance::create_import(&mut s, &import_req(owner(1), 1, 6_000), nonce(501), 100, backend(), 2_000),
        Err(XrplIcusdError::NonceConflict)
    );
    // The nonce space is per owner.
    let other = new_import(&mut s, owner(2), 1, 5_000);
    assert_ne!(other, id);
    // 10 active operations per owner.
    for b in 2..=10u8 {
        new_import(&mut s, owner(1), b, 1_000);
    }
    assert_eq!(s.active_ops_for(&owner(1)), 10);
    assert!(matches!(
        issuance::create_import(&mut s, &import_req(owner(1), 11, 1_000), nonce(11), 100, backend(), 2_000),
        Err(XrplIcusdError::LimitExceeded(_))
    ));
    // Caller scoping: another principal's operation does not exist.
    assert_eq!(s.op_for_owner(&owner(2), id).err(), Some(XrplIcusdError::NotFound));
    assert!(s.op_for_owner(&owner(1), id).is_ok());
    // Paging: caller-only rows, at most 50, cursor continues.
    let p = owner_page(&s, &owner(1), None, 4);
    assert_eq!(p.items.len(), 4);
    assert!(p.items.iter().all(|v| s.op(v.operation_id).unwrap().owner == owner(1)));
    assert!(p.next_cursor.is_some());
    let p2 = owner_page(&s, &owner(1), p.next_cursor, 50);
    assert_eq!(p2.items.len(), 6);
    assert_eq!(p2.next_cursor, None);
    assert_eq!(owner_page(&s, &owner(3), None, 50).items.len(), 0);
    assert_eq!(owner_page(&s, &owner(1), None, 1_000).items.len(), 10, "limit is clamped to 50");
}

#[test]
fn global_operation_limit() {
    let mut s = ready_state();
    for i in 0..MAX_OPERATIONS_GLOBAL as u64 {
        let owner_p = Principal::from_slice(&i.to_be_bytes());
        s.operations.insert(
            i,
            Operation {
                id: i,
                owner: owner_p,
                created_at_ns: 0,
                updated_at_ns: 0,
                request_nonce: [0; 32],
                network: TESTNET,
                kind: OperationKind::Import {
                    amount_e8s: 1,
                    recipient: holder_addr(9),
                },
                phase: Phase::Delivered,
                ic_jobs: vec![],
                xrpl_attempts: vec![],
                notes: vec![],
                resolution: None,
                attempt_budget_base: 0,
                delivery_halted: false,
            },
        );
    }
    assert!(matches!(
        s.admit(&owner(1), &[1; 32], TESTNET, &OperationKind::Import { amount_e8s: 1, recipient: holder_addr(9) }),
        Err(XrplIcusdError::LimitExceeded(_))
    ));
}

#[test]
fn durable_reservations_and_exclusions() {
    let mut s = ready_state();
    let ct_a = Principal::from_slice(&[0xAA]);
    let ct_b = Principal::from_slice(&[0xBB]);
    borrow_reservation(&mut s, 1, 10, 1_000, 10);
    s.apply_reservation_begun(2, owner(2), &[2; 32], TESTNET, 11, ct_b, 400, 4, 396, &holder_addr(9), 1, 11);
    assert_eq!(s.durable_reserved_for(ct_a), 1_000);
    assert_eq!(s.durable_reserved_for(ct_b), 400);
    assert_eq!(s.durable_reserved_total(), 1_400);
    assert!(s.vault_exclusions.contains(&10) && s.vault_exclusions.contains(&11));
    // EscrowMintPending still reserves; DebtCommitted no longer does.
    s.op_mut(1).unwrap().phase = Phase::EscrowMintPending;
    assert_eq!(s.durable_reserved_total(), 1_400);
    s.apply_reservation_released(1, 10, ReleaseReason::DebtCommitted, 20);
    assert_eq!(s.durable_reserved_total(), 400);
    assert!(!s.vault_exclusions.contains(&10));
    // A definite mint failure frees headroom and cancels with no claim.
    s.apply_reservation_released(2, 11, ReleaseReason::MintFailed, 21);
    assert_eq!(s.op(2).unwrap().phase, Phase::Cancelled);
    assert_eq!(s.durable_reserved_total(), 0);
    assert!(s.vault_exclusions.is_empty());
    // Idempotent: a repeated release changes nothing.
    s.apply_reservation_released(2, 11, ReleaseReason::MintFailed, 22);
    // The helper reads the lane inside the full State.
    let mut full = test_init_state();
    full.xrpl_icusd = s;
    assert!(!xrpl_vault_excluded(&full, 10));
    full.xrpl_icusd.vault_exclusions.insert(77);
    assert!(xrpl_vault_excluded(&full, 77));
}

// ---- liabilities and coverage -----------------------------------------------------------------

fn cov(escrow: u128, snapshot: Option<(u32, WideAmount)>) -> CoverageInput {
    CoverageInput {
        escrow_balance_e8s: escrow,
        snapshot: snapshot.map(|(i, t)| SnapshotSummary { ledger_index: i, total: t }),
        unprocessed_return_allowance: WideAmount::ZERO,
    }
}

#[test]
fn pending_claims_consume_coverage() {
    let mut s = ready_state();
    let a = new_import(&mut s, owner(1), 1, 6_000);
    let b = new_import(&mut s, owner(2), 2, 4_000);
    // Unfunded pulls are not claims yet: they consume nothing.
    let r = s.check_coverage(&cov(0, Some((9, WideAmount::ZERO)))).unwrap();
    assert_eq!(r.status, CoverageStatus::Covered);
    assert_eq!(r.required_e8s, 0);
    fund_import(&mut s, a);
    fund_import(&mut s, b);
    let bk = s.buckets().unwrap();
    assert_eq!(bk.d, WideAmount::from_e8s(10_000));
    assert_eq!(bk.o, WideAmount::ZERO);
    // Escrow holds exactly the backing: covered; one e8s short: shortfall.
    let ok = s.check_coverage(&cov(10_000, Some((9, WideAmount::ZERO)))).unwrap();
    assert_eq!(ok.status, CoverageStatus::Covered);
    assert_eq!((ok.required_e8s, ok.d_e8s, ok.surplus_e8s), (10_000, 10_000, 0));
    let short = s.check_coverage(&cov(9_999, Some((9, WideAmount::ZERO)))).unwrap();
    assert_eq!(short.status, CoverageStatus::Shortfall { short_e8s: 1 });
    assert!(!short.permits_issuance());
    // The rounding buffer is required on top.
    s.config.rounding_buffer_e8s = 25;
    assert_eq!(
        s.check_coverage(&cov(10_000, Some((9, WideAmount::ZERO)))).unwrap().status,
        CoverageStatus::Shortfall { short_e8s: 25 }
    );
    // No snapshot: not allowed to issue.
    s.config.rounding_buffer_e8s = 0;
    assert_eq!(
        s.check_coverage(&cov(10_000, None)).unwrap().status,
        CoverageStatus::SnapshotMissing
    );
}

#[test]
fn snapshot_must_agree_with_recorded_outstanding() {
    let mut s = ready_state();
    let id = new_import(&mut s, owner(1), 1, 6_000);
    fund_import(&mut s, id);
    deliver(&mut s, id, 5);
    assert_eq!(s.issued_outstanding, WideAmount::from_e8s(6_000));
    assert_eq!(s.buckets().unwrap().d, WideAmount::ZERO, "D moved to O exactly once");
    let good = s.check_coverage(&cov(6_000, Some((20, WideAmount::from_e8s(6_000))))).unwrap();
    assert_eq!(good.status, CoverageStatus::Covered);
    // The XRPL shows MORE than we recorded: unexplained issuance.
    let more = s.check_coverage(&cov(6_000, Some((20, WideAmount::from_e8s(6_001))))).unwrap();
    assert!(matches!(more.status, CoverageStatus::SnapshotDisagrees { .. }));
    // LESS than recorded: unprocessed returns would explain it, nothing else does.
    let less = s.check_coverage(&cov(6_000, Some((20, WideAmount::from_e8s(5_000))))).unwrap();
    assert!(matches!(less.status, CoverageStatus::SnapshotDisagrees { .. }));
    let mut allowed = cov(6_000, Some((20, WideAmount::from_e8s(5_000))));
    allowed.unprocessed_return_allowance = WideAmount::from_e8s(1_000);
    assert_eq!(s.check_coverage(&allowed).unwrap().status, CoverageStatus::Covered);
    // An in-flight signed issuance may legitimately already show on the XRPL.
    let id2 = new_import(&mut s, owner(2), 2, 1_000);
    fund_import(&mut s, id2);
    let a = prepared_payment(&s, id2, 6);
    s.begin_attempt(LeaseHolder::Operation(id2), a).unwrap();
    sign_and_submit(&mut s, id2);
    assert_eq!(s.buckets().unwrap().in_flight, WideAmount::from_e8s(1_000));
    let inflight = s.check_coverage(&cov(7_000, Some((21, WideAmount::from_e8s(7_000))))).unwrap();
    assert_eq!(inflight.status, CoverageStatus::Covered);
    assert_eq!(inflight.in_flight_e8s, 1_000);
}

#[test]
fn held_and_return_buckets_consume_backing() {
    let mut s = ready_state();
    s.operations.insert(
        1,
        Operation {
            id: 1,
            owner: owner(1),
            created_at_ns: 0,
            updated_at_ns: 0,
            request_nonce: [1; 32],
            network: TESTNET,
            kind: OperationKind::Repayment {
                vault_id: 5,
                payer: holder_addr(9),
                destination_tag: 1,
                max_e8s: 1_000,
                full_close: false,
                parent: None,
                receipts: vec![],
                cutoff: None,
                applied_e8s: 700,
                held: ExactIssuedAmount::parse("0.000003").unwrap(),
                children: vec![],
            },
            phase: Phase::EscrowBurnPending,
            ic_jobs: vec![],
            xrpl_attempts: vec![],
            notes: vec![],
            resolution: None,
            attempt_budget_base: 0,
            delivery_halted: false,
        },
    );
    let b = s.buckets().unwrap();
    assert_eq!(b.r, WideAmount::from_e8s(700));
    assert_eq!(b.h, WideAmount::from_e8s(300));
    s.operations.insert(
        2,
        Operation {
            id: 2,
            owner: owner(1),
            created_at_ns: 0,
            updated_at_ns: 0,
            request_nonce: [2; 32],
            network: TESTNET,
            kind: OperationKind::Refund {
                source_tx: "A".repeat(64),
                recipient: holder_addr(9),
                amount: ExactIssuedAmount::parse("0.00000001").unwrap(),
            },
            phase: Phase::RefundPending,
            ic_jobs: vec![],
            xrpl_attempts: vec![],
            notes: vec![],
            resolution: None,
            attempt_budget_base: 0,
            delivery_halted: false,
        },
    );
    let b = s.buckets().unwrap();
    assert_eq!(b.h, WideAmount::from_e8s(301));
    // Required backing rounds the sum UP.
    let r = s.check_coverage(&cov(1_001, Some((1, WideAmount::ZERO)))).unwrap();
    assert_eq!(r.required_e8s, 1_001);
}

#[test]
fn borrow_fee_liquidity_can_reach_xrpl() {
    // A 100 icUSD borrow pays out 99 net (1 fee). Without the canonical import route
    // the last 1 icUSD could never exist on the XRPL ledger, so a one-borrower pilot
    // could not close its debt there.
    let e8 = 100_000_000u64;
    let mut full = test_init_state();
    let debt_before = full.total_borrowed_icusd_amount();
    let s = &mut full.xrpl_icusd;
    *s = ready_state();
    borrow_reservation(s, 1, 10, 100 * e8, e8);
    s.apply_reservation_released(1, 10, ReleaseReason::DebtCommitted, 20);
    assert_eq!(s.buckets().unwrap().d, WideAmount::from_e8s(99 * e8));
    // The holder tops up from their own canonical balance: 1 icUSD, no debt.
    let imp = new_import(s, owner(1), 5, e8);
    fund_import(s, imp);
    assert_eq!(s.buckets().unwrap().d, WideAmount::from_e8s(100 * e8));
    // Both claims deliver: the XRPL side can now hold the full 100.
    s.op_mut(1).unwrap().phase = Phase::IssuePrepared;
    deliver(s, 1, 5);
    deliver(s, imp, 6);
    assert_eq!(s.issued_outstanding, WideAmount::from_e8s(100 * e8));
    assert_eq!(s.buckets().unwrap().d, WideAmount::ZERO);
    // The import created no debt anywhere.
    assert_eq!(full.total_borrowed_icusd_amount(), debt_before);
    assert!(full.vault_id_to_vaults.is_empty());
}

// ---- canonical import --------------------------------------------------------------------------

#[test]
fn canonical_import_is_backed_not_new_debt() {
    let mut full = test_init_state();
    let debt_before = full.total_borrowed_icusd_amount();
    full.xrpl_icusd = ready_state();
    let s = &mut full.xrpl_icusd;
    let id = new_import(s, owner(1), 1, 7_000);
    // Before the pull is proven there is NO claim.
    assert_eq!(s.op(id).unwrap().phase, Phase::EscrowPullPending);
    assert_eq!(s.buckets().unwrap().d, WideAmount::ZERO);
    // Done but not yet fetched back and matched: still no claim.
    s.begin_job_call(id, 0).unwrap();
    s.finish_job_call(id, 0, LedgerCallResult::Ok(55), 10).unwrap();
    assert_eq!(
        issuance::advance_import(s, id, 11).unwrap(),
        issuance::ImportAdvance::Waiting
    );
    assert_eq!(s.buckets().unwrap().d, WideAmount::ZERO);
    // Proven: the claim exists as D with no fee and no vault.
    s.mark_proof_verified(id, 0).unwrap();
    assert_eq!(issuance::advance_import(s, id, 12).unwrap(), issuance::ImportAdvance::Funded);
    assert_eq!(s.op(id).unwrap().phase, Phase::IssuePrepared);
    assert_eq!(s.buckets().unwrap().d, WideAmount::from_e8s(7_000));
    let v = operation_view(s.op(id).unwrap());
    assert_eq!((v.gross_e8s, v.fee_e8s, v.net_e8s, v.vault_id), (7_000, 0, 7_000, None));
    assert_eq!(full.total_borrowed_icusd_amount(), debt_before, "no new debt");
    assert!(full.vault_id_to_vaults.is_empty() && full.xrpl_icusd.vault_exclusions.is_empty());
}

#[test]
fn canonical_import_reply_lost_no_second_pull() {
    let mut s = ready_state();
    let id = new_import(&mut s, owner(1), 1, 7_000);
    let first = s.begin_job_call(id, 0).unwrap();
    s.finish_job_call(id, 0, classify_transfer_from(Err((4, "timeout".into()))), 10).unwrap();
    // The user retries the endpoint with the same request nonce: same operation.
    let again = issuance::create_import(&mut s, &import_req(owner(1), 1, 7_000), nonce(999), 5_000, backend(), 20).unwrap();
    assert_eq!(again, (id, false));
    assert_eq!(s.op(id).unwrap().ic_jobs.len(), 1, "no second pull job");
    let second = s.begin_job_call(id, 0).unwrap();
    assert_eq!(first.op_nonce, second.op_nonce);
    assert_eq!(first.created_at_time, second.created_at_time);
    assert_eq!(first.memo, second.memo);
    assert_eq!(first.ledger_tip_before, second.ledger_tip_before, "the baseline is frozen");
    let dup = classify_transfer_from(Ok(Err(TransferFromError::Duplicate {
        duplicate_of: Nat::from(77u64),
    })));
    assert_eq!(s.finish_job_call(id, 0, dup, 30).unwrap(), JobEffect::Done(77));
    assert_eq!(s.job(id, 0).unwrap().done_block(), Some(77));
    // One pull, one block, nothing to resend.
    assert!(s.begin_job_call(id, 0).is_err());
}

#[test]
fn definite_pull_failure_cancels_without_a_claim() {
    let mut s = ready_state();
    let id = new_import(&mut s, owner(1), 1, 7_000);
    s.begin_job_call(id, 0).unwrap();
    s.finish_job_call(
        id,
        0,
        classify_transfer_from(Ok(Err(TransferFromError::InsufficientAllowance { allowance: Nat::from(0u64) }))),
        10,
    )
    .unwrap();
    assert_eq!(issuance::advance_import(&mut s, id, 11).unwrap(), issuance::ImportAdvance::Cancelled);
    assert_eq!(s.op(id).unwrap().phase, Phase::Cancelled);
    assert_eq!(s.buckets().unwrap().d, WideAmount::ZERO);
    assert_eq!(s.active_ops_for(&owner(1)), 0, "cancelled operations stop counting");
}

#[test]
fn import_admission_rules() {
    let mut s = ready_state();
    // paused / disabled / wrong network / caps / anonymous
    s.config.issuance_paused = true;
    assert!(matches!(
        issuance::create_import(&mut s, &import_req(owner(1), 1, 100), nonce(1), 1, backend(), 1),
        Err(XrplIcusdError::Paused(_))
    ));
    s.config.issuance_paused = false;
    let mut wrong = import_req(owner(1), 1, 100);
    wrong.network = XrplNetwork::Mainnet;
    assert!(matches!(
        issuance::create_import(&mut s, &wrong, nonce(1), 1, backend(), 1),
        Err(XrplIcusdError::WrongNetwork(_))
    ));
    assert!(matches!(
        issuance::create_import(&mut s, &import_req(Principal::anonymous(), 1, 100), nonce(1), 1, backend(), 1),
        Err(XrplIcusdError::AnonymousCaller)
    ));
    s.config.per_operation_cap_e8s = 1_000;
    assert!(matches!(
        issuance::create_import(&mut s, &import_req(owner(1), 1, 1_001), nonce(1), 1, backend(), 1),
        Err(XrplIcusdError::LimitExceeded(_))
    ));
    s.config.outstanding_cap_e8s = 1_500;
    let a = new_import(&mut s, owner(1), 2, 1_000);
    fund_import(&mut s, a);
    assert!(matches!(
        issuance::create_import(&mut s, &import_req(owner(1), 3, 600), nonce(3), 1, backend(), 1),
        Err(XrplIcusdError::LimitExceeded(_))
    ));
    assert!(issuance::create_import(&mut s, &import_req(owner(1), 4, 500), nonce(4), 1, backend(), 1).is_ok());
    assert!(issuance::validate_recipient(&holder_addr(9), &issuer_addr()).is_ok());
    assert!(issuance::validate_recipient(&issuer_addr(), &issuer_addr()).is_err());
    assert!(issuance::validate_recipient("not-an-address", &issuer_addr()).is_err());
    assert!(issuance::parse_request_nonce(&[0u8; 31]).is_err());
    assert!(issuance::parse_request_nonce(&[0u8; 32]).is_ok());
    // disabled by default
    assert!(XrplIcusdState::default().ensure_ready().is_err());
}

// ---- issuer sequencer ------------------------------------------------------------------------

fn dalek_sign(msg: &[u8]) -> Vec<u8> {
    use ed25519_dalek::{Signer, SigningKey};
    // The issuer test key's 32-byte public half is `issuer_pk()`; sign with a key whose
    // public half differs is fine for structure tests, so just sign with any key.
    SigningKey::from_bytes(&[3u8; 32]).sign(msg).to_bytes().to_vec()
}

#[test]
fn prepare_and_sign_signs_the_persisted_unsigned_bytes() {
    use ed25519_dalek::{Signature, Signer, SigningKey, Verifier};
    let sk = SigningKey::from_bytes(&[3u8; 32]);
    let pk = sk.verifying_key().to_bytes();
    let addr = classic_address_from_ed25519_pubkey(&pk);
    let mut a = prepare_attempt(&PrepareInput {
        purpose: AttemptPurpose::Delivery,
        tx: AttemptTx::Payment {
            destination: holder_addr(9),
            amount: ExactIssuedAmount::parse("12.5").unwrap(),
            destination_tag: None,
        },
        issuer_address: &addr,
        issuer_pubkey: pk,
        sequence: 77,
        prepared_ledger: 5_000,
        fee_drops: 15,
        max_fee_drops: 100,
        now_ns: 1,
    })
    .unwrap();
    assert_eq!(a.last_ledger_sequence, 5_000 + LAST_LEDGER_WINDOW);
    assert_eq!(a.status, AttemptStatus::Prepared);
    assert!(a.signed_hex.is_none() && a.tx_hash.is_none());
    let msg = message_to_sign(&a).unwrap();
    assert_eq!(&msg[..4], &[0x53, 0x54, 0x58, 0x00], "STX\\0 prefix");
    let sig = sk.sign(&msg);
    assert!(sk.verifying_key().verify(&msg, &Signature::from_bytes(&sig.to_bytes())).is_ok());
    let hash = finalize_signature(&mut a, &sig.to_bytes(), &pk).unwrap();
    assert_eq!(a.status, AttemptStatus::Signed);
    let signed = hex::decode(a.signed_hex.as_ref().unwrap()).unwrap();
    assert_eq!(hex::encode_upper(crate::chains::xrp::sign::tx_hash(&signed)), hash);
    assert_eq!(a.tx_hash.as_deref(), Some(hash.as_str()));
    // Signing twice is refused; a wrong-length signature is refused.
    assert!(finalize_signature(&mut a, &sig.to_bytes(), &pk).is_err());
    let mut b = a.clone();
    b.status = AttemptStatus::Prepared;
    assert!(finalize_signature(&mut b, &[1u8; 10], &pk).is_err());
    // A tampered persisted blob is detected before the signature is attached.
    let mut c = a.clone();
    c.status = AttemptStatus::Prepared;
    c.sequence += 1;
    assert!(finalize_signature(&mut c, &sig.to_bytes(), &pk).is_err());
    let _ = dalek_sign(b"x");
}

#[test]
fn fee_is_bounded_by_max_fee_drops() {
    let mk = |fee: u64, max: u64| {
        prepare_attempt(&PrepareInput {
            purpose: AttemptPurpose::Delivery,
            tx: AttemptTx::Payment {
                destination: holder_addr(9),
                amount: ExactIssuedAmount::parse("1").unwrap(),
                destination_tag: None,
            },
            issuer_address: &issuer_addr(),
            issuer_pubkey: issuer_pk(),
            sequence: 1,
            prepared_ledger: 1,
            fee_drops: fee,
            max_fee_drops: max,
            now_ns: 1,
        })
    };
    assert!(mk(10, 100).is_ok());
    assert!(mk(100, 100).is_ok());
    assert!(mk(101, 100).is_err());
    assert!(mk(0, 100).is_err());
    assert!(mk(10, 0).is_err(), "an unset max fee never signs");
}

#[test]
fn sign_or_submit_unknown_keeps_sequence_lease() {
    let mut s = ready_state();
    let a_id = new_import(&mut s, owner(1), 1, 3_000);
    let b_id = new_import(&mut s, owner(2), 2, 4_000);
    fund_import(&mut s, a_id);
    fund_import(&mut s, b_id);
    let h = LeaseHolder::Operation(a_id);
    let att = prepared_payment(&s, a_id, 5);
    s.begin_attempt(h, att.clone()).unwrap();
    assert_eq!(s.issuer_lease, Some(h));
    // The sequence is single-flight: nobody else may prepare while it is unresolved.
    let other = prepared_payment(&s, b_id, 5);
    assert!(matches!(
        s.begin_attempt(LeaseHolder::Operation(b_id), other.clone()),
        Err(XrplIcusdError::Ambiguous(_))
    ));
    // A SIGNING failure leaves the lease and the unsigned attempt in place.
    s.note_attempt(h, "signing failed: timeout".into());
    assert_eq!(s.lease_attempt(h).unwrap().status, AttemptStatus::Prepared);
    assert_eq!(s.issuer_lease, Some(h));
    // Signed and submitted, then every kind of non-answer keeps the lease.
    sign_and_submit(&mut s, a_id);
    let att = s.lease_attempt(h).unwrap().clone();
    assert_eq!(att.status, AttemptStatus::Submitted);
    for obs in [
        Observation { tx: TxView::NotValidated, expiry: None },
        Observation { tx: TxView::NotFound, expiry: None },
        // not past LastLedgerSequence yet
        Observation {
            tx: TxView::NotFound,
            expiry: Some(ExpiryEvidence {
                validated_ledger: att.last_ledger_sequence,
                history_from: att.prepared_ledger,
                history_to: att.last_ledger_sequence + 1,
                providers: vec!["a".into(), "b".into()],
            }),
        },
        // history does not reach back to where the sequence was read
        Observation {
            tx: TxView::NotFound,
            expiry: Some(ExpiryEvidence {
                validated_ledger: att.last_ledger_sequence + 5,
                history_from: att.prepared_ledger + 1,
                history_to: att.last_ledger_sequence + 1,
                providers: vec!["a".into(), "b".into()],
            }),
        },
        // history does not reach LLS + 1
        Observation {
            tx: TxView::NotFound,
            expiry: Some(ExpiryEvidence {
                validated_ledger: att.last_ledger_sequence + 5,
                history_from: att.prepared_ledger,
                history_to: att.last_ledger_sequence,
                providers: vec!["a".into(), "b".into()],
            }),
        },
    ] {
        let v = reconcile(&att, &obs);
        assert_eq!(v, Reconciled::Pending, "{obs:?}");
        assert_eq!(s.apply_reconcile(h, v, 5_000).unwrap(), ReconcileEffect::None);
        assert_eq!(s.issuer_lease, Some(h), "ambiguity keeps the lease");
        assert!(s.op(a_id).unwrap().has_signed_ambiguity());
    }
    // A claim backed by an ambiguous signed issuance cannot be resolved by its owner.
    let r = issuance::plan_resolution(
        &s,
        &owner(1),
        &ResolveClaimArg { operation_id: a_id, action: ClaimAction::ReceiveCanonical },
    );
    assert!(matches!(r, Err(XrplIcusdError::Ambiguous(_))), "{r:?}");
    // ONLY a complete-history non-inclusion proof releases it.
    let proof = Observation {
        tx: TxView::NotFound,
        expiry: Some(ExpiryEvidence {
            validated_ledger: att.last_ledger_sequence + 1,
            history_from: att.prepared_ledger,
            history_to: att.last_ledger_sequence + 1,
            providers: vec!["a".into(), "b".into()],
        }),
    };
    let v = reconcile(&att, &proof);
    assert!(matches!(v, Reconciled::Expired { .. }));
    assert_eq!(s.apply_reconcile(h, v, 6_000).unwrap(), ReconcileEffect::AttemptReleased);
    assert_eq!(s.issuer_lease, None);
    assert_eq!(s.op(a_id).unwrap().phase, Phase::IssuePrepared, "the claim is intact");
    assert!(!s.op(a_id).unwrap().has_signed_ambiguity());
    // A successor attempt under the same claim, with a fresh sequence, is allowed.
    s.begin_attempt(h, prepared_payment(&s, a_id, 9)).unwrap();
    assert_eq!(s.op(a_id).unwrap().xrpl_attempts.len(), 2);
    assert_eq!(s.op(a_id).unwrap().xrpl_attempts[1].sequence, 9);
}

#[test]
fn a_validated_failure_releases_the_lease_and_a_success_credits_once() {
    let mut s = ready_state();
    let id = new_import(&mut s, owner(1), 1, 3_000);
    fund_import(&mut s, id);
    let h = LeaseHolder::Operation(id);
    s.begin_attempt(h, prepared_payment(&s, id, 5)).unwrap();
    sign_and_submit(&mut s, id);
    let att = s.lease_attempt(h).unwrap().clone();
    // tec*: consumed the sequence, delivered nothing.
    let v = reconcile(&att, &Observation { tx: TxView::Failed("tecPATH_DRY".into()), expiry: None });
    assert_eq!(s.apply_reconcile(h, v, 7_000).unwrap(), ReconcileEffect::AttemptReleased);
    assert_eq!(s.issuer_lease, None);
    assert_eq!(s.issued_outstanding, WideAmount::ZERO);
    // Successor delivers; repeating the verdict never double-credits.
    s.begin_attempt(h, prepared_payment(&s, id, 6)).unwrap();
    sign_and_submit(&mut s, id);
    let att2 = s.lease_attempt(h).unwrap().clone();
    let v = reconcile(&att2, &succeeded(&att2));
    assert_eq!(s.apply_reconcile(h, v.clone(), 8_000).unwrap(), ReconcileEffect::Delivered);
    assert_eq!(s.apply_reconcile(h, v, 8_001).unwrap(), ReconcileEffect::None);
    assert_eq!(s.issued_outstanding, WideAmount::from_e8s(3_000));
    assert_eq!(s.op(id).unwrap().phase, Phase::Delivered);
    assert!(matches!(
        s.op(id).unwrap().xrpl_attempts[1].status,
        AttemptStatus::Validated { .. }
    ));
}

#[test]
fn mismatched_validated_payments_are_quarantined_not_credited() {
    let mut s = ready_state();
    let id = new_import(&mut s, owner(1), 1, 3_000);
    fund_import(&mut s, id);
    let h = LeaseHolder::Operation(id);
    s.begin_attempt(h, prepared_payment(&s, id, 5)).unwrap();
    sign_and_submit(&mut s, id);
    let att = s.lease_attempt(h).unwrap().clone();
    let mutate = |f: &dyn Fn(&mut VerifiedIssuedReceipt)| {
        let mut r = receipt_for(&att);
        f(&mut r);
        Observation {
            tx: TxView::Succeeded {
                ledger_index: r.ledger_index,
                tx_type: Some("Payment".into()),
                account: Some(r.account.clone()),
                payment: Some(Box::new(r)),
            },
            expiry: None,
        }
    };
    for (name, obs) in [
        ("amount", mutate(&|r| r.delivered_amount = ExactIssuedAmount::parse("0.00002999").unwrap())),
        ("dest", mutate(&|r| r.destination = holder_addr(33))),
        ("tag", mutate(&|r| r.destination_tag = Some(5))),
        ("hash", mutate(&|r| r.hash = "B".repeat(64))),
        ("partial", mutate(&|r| r.flags |= crate::chains::xrp::issued_rpc::TF_PARTIAL_PAYMENT)),
        ("paths", mutate(&|r| r.has_paths = true)),
        ("account", mutate(&|r| r.account = holder_addr(44))),
    ] {
        let v = reconcile(&att, &obs);
        assert!(matches!(v, Reconciled::Quarantine(_)), "{name}: {v:?}");
    }
    let v = reconcile(&att, &mutate(&|r| r.delivered_amount = ExactIssuedAmount::parse("0.00002999").unwrap()));
    assert_eq!(s.apply_reconcile(h, v, 9_000).unwrap(), ReconcileEffect::Quarantined);
    assert_eq!(s.issuer_lease, Some(h), "quarantine keeps the lease");
    assert_eq!(s.issued_outstanding, WideAmount::ZERO);
    let view = operation_view(s.op(id).unwrap());
    assert_eq!(view.next_action, NextAction::ContactOperator);
    assert_eq!(CreditEligibility::Eligible, receipt_for(&att).credit_eligibility());
}

#[test]
fn upgrade_recovers_signed_blob() {
    let mut s = ready_state();
    let id = new_import(&mut s, owner(1), 1, 3_000);
    fund_import(&mut s, id);
    let h = LeaseHolder::Operation(id);
    s.begin_attempt(h, prepared_payment(&s, id, 5)).unwrap();
    sign_and_submit(&mut s, id);
    // an in-flight ledger call at upgrade time
    let id2 = new_import(&mut s, owner(2), 2, 10);
    s.begin_job_call(id2, 0).unwrap();
    let before = s.lease_attempt(h).unwrap().clone();
    assert!(before.signed_hex.is_some() && before.tx_hash.is_some());
    // pre_upgrade -> stable memory -> post_upgrade
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&s, &mut bytes).unwrap();
    let mut restored: XrplIcusdState = ciborium::de::from_reader(bytes.as_slice()).unwrap();
    restored.recover_after_upgrade();
    let after = restored.lease_attempt(h).unwrap();
    assert_eq!(after.signed_hex, before.signed_hex, "the exact signed blob survives");
    assert_eq!(after.tx_hash, before.tx_hash);
    assert_eq!(after.sequence, before.sequence);
    assert_eq!(restored.issuer_lease, Some(h), "the lease survives");
    assert_eq!(after.status, AttemptStatus::Submitted);
    // The identical blob can be resubmitted and reconciled by its persisted hash.
    assert!(resubmit_due(after, after.last_submit_ns + RESUBMIT_AFTER_NS));
    assert!(!resubmit_due(after, after.last_submit_ns + 1));
    assert!(matches!(restored.job(id2, 0).unwrap().status, IcJobStatus::Unknown { .. }));
    // Work remains, so the tick re-arms after the upgrade.
    assert!(restored.has_work());
    assert!(plan_tick(&restored, 6, 0).contains(&TickItem::Op(id)));
}

#[test]
fn pause_keeps_reconciliation_live() {
    let mut s = ready_state();
    let id = new_import(&mut s, owner(1), 1, 3_000);
    fund_import(&mut s, id);
    let h = LeaseHolder::Operation(id);
    s.begin_attempt(h, prepared_payment(&s, id, 5)).unwrap();
    sign_and_submit(&mut s, id);
    let att = s.lease_attempt(h).unwrap().clone();
    // Both pauses on.
    s.set_pauses(Some(true), Some(true));
    // New operations are refused...
    assert!(matches!(
        issuance::create_import(&mut s, &import_req(owner(2), 2, 10), nonce(2), 1, backend(), 1),
        Err(XrplIcusdError::Paused(_))
    ));
    // ...and so is a NEW issuer attempt...
    let other = new_import_unchecked(&mut s, owner(3), 3, 500);
    let r = s.begin_attempt(LeaseHolder::Operation(other), prepared_payment(&s, other, 6));
    assert!(matches!(r, Err(XrplIcusdError::Paused(_))));
    // ...but the existing signed transaction still reconciles to Delivered.
    assert!(plan_tick(&s, 6, 0).contains(&TickItem::Op(id)), "the tick keeps reconciling");
    let v = reconcile(&att, &succeeded(&att));
    assert_eq!(s.apply_reconcile(h, v, 10_000).unwrap(), ReconcileEffect::Delivered);
    assert_eq!(s.issued_outstanding, WideAmount::from_e8s(3_000));
    assert_eq!(s.issuer_lease, None);
    // Reads stay available.
    assert!(s.ensure_ready().is_ok());
    assert!(s.ensure_open_for_issuance().is_err());
}

/// An import already funded (bypassing the issuance gate) for pause tests.
fn new_import_unchecked(s: &mut XrplIcusdState, o: Principal, byte: u8, amount: u64) -> OperationId {
    let was = s.config.issuance_paused;
    s.config.issuance_paused = false;
    let id = new_import(s, o, byte, amount);
    s.config.issuance_paused = was;
    fund_import(s, id);
    id
}

#[test]
fn attempt_budget_halts_delivery_and_reissue_restores_it() {
    let mut s = ready_state();
    let id = new_import(&mut s, owner(1), 1, 3_000);
    fund_import(&mut s, id);
    let h = LeaseHolder::Operation(id);
    for n in 0..MAX_ATTEMPTS_PER_BUDGET {
        s.begin_attempt(h, prepared_payment(&s, id, 10 + n)).unwrap();
        sign_and_submit(&mut s, id);
        let att = s.lease_attempt(h).unwrap().clone();
        let v = reconcile(&att, &Observation { tx: TxView::Failed("tecNO_LINE".into()), expiry: None });
        let e = s.apply_reconcile(h, v, 11_000).unwrap();
        if n + 1 == MAX_ATTEMPTS_PER_BUDGET {
            assert_eq!(e, ReconcileEffect::Halted);
        }
    }
    let op = s.op(id).unwrap();
    assert!(op.delivery_halted);
    assert_eq!(operation_view(op).next_action, NextAction::ResolveClaim);
    assert!(s.begin_attempt(h, prepared_payment(&s, id, 99)).is_err());
    assert!(!s.has_work() || s.operations.values().all(|o| !o.needs_work() || o.id != id));
    // The owner can reissue: a fresh budget.
    let plan = issuance::plan_resolution(&s, &owner(1), &ResolveClaimArg { operation_id: id, action: ClaimAction::ReissueXrpl }).unwrap();
    assert_eq!(plan, ResolutionPlan::Reissue);
    issuance::apply_reissue(&mut s, id, 12_000).unwrap();
    assert!(!s.op(id).unwrap().delivery_halted);
    assert!(s.begin_attempt(h, prepared_payment(&s, id, 100)).is_ok());
}

// ---- owner claim resolution ----------------------------------------------------------------------

#[test]
fn resolve_claim_receive_canonical_reserves_the_ic_fee_separately() {
    let mut s = ready_state();
    s.config.rounding_buffer_e8s = 10;
    let id = new_import(&mut s, owner(1), 1, 5_000);
    fund_import(&mut s, id);
    // Wrong owner / anonymous-equivalent / unsupported action
    for who in [owner(2), Principal::anonymous()] {
        assert_eq!(
            issuance::plan_resolution(&s, &who, &ResolveClaimArg { operation_id: id, action: ClaimAction::ReceiveCanonical }),
            Err(XrplIcusdError::NotFound)
        );
    }
    assert!(matches!(
        issuance::plan_resolution(&s, &owner(1), &ResolveClaimArg { operation_id: id, action: ClaimAction::CancelUnexecutedRepayment }),
        Err(XrplIcusdError::NotApplicable(_))
    ));
    // Reissue is only meaningful once delivery is halted.
    assert!(matches!(
        issuance::plan_resolution(&s, &owner(1), &ResolveClaimArg { operation_id: id, action: ClaimAction::ReissueXrpl }),
        Err(XrplIcusdError::WrongPhase(_))
    ));
    let plan = issuance::plan_resolution(&s, &owner(1), &ResolveClaimArg { operation_id: id, action: ClaimAction::ReceiveCanonical }).unwrap();
    assert_eq!(plan, ResolutionPlan::ReceiveCanonical { owner: owner(1), amount_e8s: 5_000 });
    // Escrow must cover the claim + buffer + THIS fee (10_000 e8s ledger fee here).
    let fee = 10_000u64;
    assert_eq!(s.required_backing_e8s(fee).unwrap(), 5_000 + 10 + 10_000);
    assert!(matches!(
        issuance::begin_receive_canonical(&mut s, id, fee, 5_009 + 10_000, nonce(60), 100, 1),
        Err(XrplIcusdError::CoverageUnavailable(_))
    ));
    let idx = issuance::begin_receive_canonical(&mut s, id, fee, 5_010 + 10_000, nonce(60), 100, 1).unwrap();
    let job = s.job(id, idx).unwrap().clone();
    assert_eq!(job.purpose, IcJobPurpose::CanonicalReturn);
    assert_eq!(job.from_subaccount, Some(escrow_subaccount()));
    assert_eq!(job.to, Account { owner: owner(1), subaccount: None });
    assert_eq!((job.amount_e8s, job.reserved_fee_e8s), (5_000, fee));
    // The fee reserve is a SEPARATE coverage requirement, not holder backing.
    let b = s.buckets().unwrap();
    assert_eq!(b.d, WideAmount::from_e8s(5_000));
    assert_eq!(b.ic_fee_reserve_e8s, u128::from(fee));
    assert_eq!(s.required_backing_e8s(0).unwrap(), 5_000 + 10 + 10_000);
    // A second resolution while one is pending is refused.
    assert!(issuance::plan_resolution(&s, &owner(1), &ResolveClaimArg { operation_id: id, action: ClaimAction::ReceiveCanonical }).is_err());
    // Not done yet: the phase does not move.
    assert!(!issuance::advance_resolution(&mut s, id, 5).unwrap());
    s.begin_job_call(id, idx).unwrap();
    s.finish_job_call(id, idx, LedgerCallResult::Ok(900), 6).unwrap();
    assert!(!issuance::advance_resolution(&mut s, id, 7).unwrap(), "unproven Done does not resolve");
    s.mark_proof_verified(id, idx).unwrap();
    assert!(issuance::advance_resolution(&mut s, id, 8).unwrap());
    assert_eq!(s.op(id).unwrap().phase, Phase::ClaimResolved);
    assert_eq!(s.buckets().unwrap().d, WideAmount::ZERO);
    assert_eq!(s.buckets().unwrap().ic_fee_reserve_e8s, 0);
}

#[test]
fn resolve_claim_failure_unwinds_and_unresolved_attempts_block_it() {
    let mut s = ready_state();
    let id = new_import(&mut s, owner(1), 1, 5_000);
    fund_import(&mut s, id);
    let idx = issuance::begin_receive_canonical(&mut s, id, 10, 1_000_000, nonce(61), 100, 1).unwrap();
    s.begin_job_call(id, idx).unwrap();
    s.finish_job_call(id, idx, LedgerCallResult::Rejected("insufficient".into()), 2).unwrap();
    issuance::advance_resolution(&mut s, id, 3).unwrap();
    assert_eq!(s.op(id).unwrap().resolution, None, "proven failure: the claim is untouched");
    assert_eq!(s.op(id).unwrap().phase, Phase::IssuePrepared);
    // An in-progress issuer attempt blocks resolution even before it is signed.
    s.begin_attempt(LeaseHolder::Operation(id), prepared_payment(&s, id, 5)).unwrap();
    assert!(matches!(
        issuance::plan_resolution(&s, &owner(1), &ResolveClaimArg { operation_id: id, action: ClaimAction::ReceiveCanonical }),
        Err(XrplIcusdError::Ambiguous(_))
    ));
    // Delivered claims are not resolvable.
    let mut s2 = ready_state();
    let d = new_import(&mut s2, owner(1), 1, 100);
    fund_import(&mut s2, d);
    deliver(&mut s2, d, 5);
    assert!(matches!(
        issuance::plan_resolution(&s2, &owner(1), &ResolveClaimArg { operation_id: d, action: ClaimAction::ReceiveCanonical }),
        Err(XrplIcusdError::WrongPhase(_))
    ));
}

// ---- issuer policy and setup ------------------------------------------------------------------------

#[test]
fn policy_required_before_setup() {
    let mut s = XrplIcusdState::default();
    s.config.network = Some(TESTNET);
    s.config.issuer_address = Some(issuer_addr());
    // No policy chosen: setup cannot start, and no default is inferred.
    assert!(matches!(s.next_setup_action(), Err(XrplIcusdError::Policy(m)) if m.contains("explicit")));
    assert!(s.config.policy.is_none());
    // A mutually exclusive policy is invalid.
    assert!(IssuerPolicy { allow_clawback: true, no_freeze: true }.validate().is_err());
    let mut u = ConfigUpdate::default();
    u.policy = Some(IssuerPolicy { allow_clawback: true, no_freeze: true });
    assert!(s.apply_config_update(u).is_err());
    // Explicit policy: the first step is DefaultRipple.
    let mut u = ConfigUpdate::default();
    u.policy = Some(IssuerPolicy { allow_clawback: false, no_freeze: false });
    s.apply_config_update(u).unwrap();
    assert_eq!(s.next_setup_action().unwrap(), SetupAction::Start(IssuerSetupStep::DefaultRipple));
    // Steps and order per policy.
    assert_eq!(
        IssuerSetupStep::sequence_for(&IssuerPolicy { allow_clawback: false, no_freeze: false }),
        vec![IssuerSetupStep::DefaultRipple, IssuerSetupStep::RequireDest, IssuerSetupStep::TransferRate]
    );
    assert_eq!(
        IssuerSetupStep::sequence_for(&IssuerPolicy { allow_clawback: true, no_freeze: false }).last(),
        Some(&IssuerSetupStep::AllowClawback)
    );
    assert_eq!(
        IssuerSetupStep::sequence_for(&IssuerPolicy { allow_clawback: false, no_freeze: true }).last(),
        Some(&IssuerSetupStep::NoFreeze)
    );
    // Step parameters.
    assert_eq!(account_set_params(IssuerSetupStep::TransferRate), (None, Some(1_000_000_000)));
    assert_eq!(account_set_params(IssuerSetupStep::DefaultRipple), (Some(8), None));
    assert_eq!(account_set_params(IssuerSetupStep::RequireDest), (Some(1), None));
    assert_eq!(account_set_params(IssuerSetupStep::NoFreeze), (Some(6), None));
    assert_eq!(account_set_params(IssuerSetupStep::AllowClawback), (Some(16), None));
}

fn issuer_info(flags: u32, owner_count: u32, rate: Option<u32>) -> IssuerAccountInfo {
    IssuerAccountInfo {
        exists: true,
        sequence: 3,
        owner_count,
        flags,
        transfer_rate: rate,
        balance_drops: 50_000_000,
        ledger_index: 500,
    }
}

#[test]
fn clawback_requires_empty_owner_directory() {
    let clean = issuer_info(0, 0, None);
    assert_eq!(
        step_precondition(IssuerSetupStep::AllowClawback, &clean, false).unwrap(),
        StepDecision::Send
    );
    // Owns an object (counted)...
    assert!(matches!(
        step_precondition(IssuerSetupStep::AllowClawback, &issuer_info(0, 1, None), false),
        Err(XrplIcusdError::Policy(m)) if m.contains("empty")
    ));
    // ...or in the directory without being counted in OwnerCount.
    assert!(step_precondition(IssuerSetupStep::AllowClawback, &clean, true).is_err());
    // NoFreeze already on: clawback can no longer be enabled, and vice versa.
    assert!(step_precondition(IssuerSetupStep::AllowClawback, &issuer_info(LSF_NO_FREEZE, 0, None), false).is_err());
    assert!(step_precondition(IssuerSetupStep::NoFreeze, &issuer_info(LSF_ALLOW_TRUSTLINE_CLAWBACK, 0, None), false).is_err());
    // Already satisfied steps are skipped, not re-sent.
    assert_eq!(
        step_precondition(IssuerSetupStep::AllowClawback, &issuer_info(LSF_ALLOW_TRUSTLINE_CLAWBACK, 3, None), false).unwrap(),
        StepDecision::AlreadySatisfied
    );
    assert_eq!(
        step_precondition(IssuerSetupStep::DefaultRipple, &issuer_info(LSF_DEFAULT_RIPPLE, 0, None), false).unwrap(),
        StepDecision::AlreadySatisfied
    );
    assert_eq!(
        step_precondition(IssuerSetupStep::TransferRate, &issuer_info(0, 0, Some(1_000_000_000)), false).unwrap(),
        StepDecision::AlreadySatisfied
    );
    assert_eq!(
        step_precondition(IssuerSetupStep::TransferRate, &issuer_info(0, 0, Some(1_100_000_000)), false).unwrap(),
        StepDecision::Send
    );
    // An unfunded issuer cannot be set up.
    let mut unfunded = clean.clone();
    unfunded.exists = false;
    assert!(step_precondition(IssuerSetupStep::DefaultRipple, &unfunded, false).is_err());
}

#[test]
fn final_flags_must_match_the_policy_exactly() {
    let base = LSF_DEFAULT_RIPPLE | LSF_REQUIRE_DEST_TAG;
    let plain = IssuerPolicy { allow_clawback: false, no_freeze: false };
    assert!(verify_final_flags(&issuer_info(base, 0, None), &plain).is_ok());
    assert!(verify_final_flags(&issuer_info(base, 0, Some(1_000_000_000)), &plain).is_ok());
    assert!(verify_final_flags(&issuer_info(base, 0, Some(1_010_000_000)), &plain).is_err());
    assert!(verify_final_flags(&issuer_info(LSF_DEFAULT_RIPPLE, 0, None), &plain).is_err());
    assert!(verify_final_flags(&issuer_info(LSF_REQUIRE_DEST_TAG, 0, None), &plain).is_err());
    // policy says no clawback but the flag is on / policy says clawback but it is off
    assert!(verify_final_flags(&issuer_info(base | LSF_ALLOW_TRUSTLINE_CLAWBACK, 0, None), &plain).is_err());
    let claw = IssuerPolicy { allow_clawback: true, no_freeze: false };
    assert!(verify_final_flags(&issuer_info(base, 0, None), &claw).is_err());
    assert!(verify_final_flags(&issuer_info(base | LSF_ALLOW_TRUSTLINE_CLAWBACK, 0, None), &claw).is_ok());
    let nf = IssuerPolicy { allow_clawback: false, no_freeze: true };
    assert!(verify_final_flags(&issuer_info(base | LSF_NO_FREEZE, 0, None), &nf).is_ok());
    assert!(verify_final_flags(&issuer_info(base, 0, None), &nf).is_err());
}

#[test]
fn policy_change_after_setup_refused() {
    let mut s = XrplIcusdState::default();
    let mut u = ConfigUpdate::default();
    u.network = Some(TESTNET);
    u.derived = Some((issuer_addr(), issuer_pk().to_vec()));
    u.policy = Some(IssuerPolicy { allow_clawback: false, no_freeze: false });
    s.apply_config_update(u).unwrap();
    // Before setup starts the policy may still be corrected.
    let mut u = ConfigUpdate::default();
    u.policy = Some(IssuerPolicy { allow_clawback: false, no_freeze: true });
    s.apply_config_update(u).unwrap();
    let mut u = ConfigUpdate::default();
    u.policy = Some(IssuerPolicy { allow_clawback: false, no_freeze: false });
    s.apply_config_update(u).unwrap();
    // The first AccountSet attempt starts setup and locks policy + network + issuer.
    let step = IssuerSetupStep::DefaultRipple;
    let (set_flag, transfer_rate) = account_set_params(step);
    s.config.max_fee_drops = 100;
    let a = prepare_attempt(&PrepareInput {
        purpose: AttemptPurpose::Setup { step },
        tx: AttemptTx::AccountSet { set_flag, transfer_rate },
        issuer_address: &issuer_addr(),
        issuer_pubkey: issuer_pk(),
        sequence: 1,
        prepared_ledger: 10,
        fee_drops: 12,
        max_fee_drops: 100,
        now_ns: 1,
    })
    .unwrap();
    s.begin_attempt(LeaseHolder::Setup, a).unwrap();
    assert!(s.config.setup_started());
    assert_eq!(s.config.issuer_setup, IssuerSetupState::Configuring { step });
    let mut u = ConfigUpdate::default();
    u.policy = Some(IssuerPolicy { allow_clawback: true, no_freeze: false });
    assert!(matches!(s.apply_config_update(u), Err(XrplIcusdError::Policy(m)) if m.contains("cannot change")));
    let mut u = ConfigUpdate::default();
    u.network = Some(XrplNetwork::Mainnet);
    assert!(s.apply_config_update(u).is_err());
    let mut u = ConfigUpdate::default();
    u.derived = Some((holder_addr(1), vec![1; 32]));
    assert!(s.apply_config_update(u).is_err());
    // Re-asserting the identical policy is harmless; caps stay adjustable.
    let mut u = ConfigUpdate::default();
    u.policy = Some(IssuerPolicy { allow_clawback: false, no_freeze: false });
    u.outstanding_cap_e8s = Some(5);
    s.apply_config_update(u).unwrap();
    assert_eq!(s.config.outstanding_cap_e8s, 5);
    // Validating the first AccountSet moves to the next step, not to Ready.
    let att = s.lease_attempt(LeaseHolder::Setup).unwrap().clone();
    s.record_signature(LeaseHolder::Setup, &[1u8; 64], &issuer_pk(), 2).unwrap();
    s.record_submit(LeaseHolder::Setup, 3, None).unwrap();
    let obs = Observation {
        tx: TxView::Succeeded {
            ledger_index: 20,
            tx_type: Some("AccountSet".into()),
            account: Some(issuer_addr()),
            payment: None,
        },
        expiry: None,
    };
    let signed_att = s.lease_attempt(LeaseHolder::Setup).unwrap().clone();
    let v = reconcile(&signed_att, &obs);
    assert_eq!(s.apply_reconcile(LeaseHolder::Setup, v, 4).unwrap(), ReconcileEffect::SetupStepValidated);
    assert_eq!(s.config.issuer_setup, IssuerSetupState::Configuring { step: IssuerSetupStep::RequireDest });
    assert_eq!(s.issuer_lease, None);
    assert_eq!(att.purpose, AttemptPurpose::Setup { step });
    // Last step -> Verifying -> Ready only through the verified-flags path.
    s.setup_step_satisfied(IssuerSetupStep::RequireDest).unwrap();
    s.setup_step_satisfied(IssuerSetupStep::TransferRate).unwrap();
    assert_eq!(s.config.issuer_setup, IssuerSetupState::Verifying);
    assert_eq!(s.next_setup_action().unwrap(), SetupAction::Verify);
    s.finish_setup(55);
    assert_eq!(s.next_setup_action().unwrap(), SetupAction::Ready);
}

#[test]
fn setup_attempt_requires_the_wrong_account_to_be_rejected() {
    // An AccountSet validated for a different account is quarantined.
    let mut s = XrplIcusdState::default();
    let step = IssuerSetupStep::RequireDest;
    let (set_flag, transfer_rate) = account_set_params(step);
    let a = prepare_attempt(&PrepareInput {
        purpose: AttemptPurpose::Setup { step },
        tx: AttemptTx::AccountSet { set_flag, transfer_rate },
        issuer_address: &issuer_addr(),
        issuer_pubkey: issuer_pk(),
        sequence: 1,
        prepared_ledger: 10,
        fee_drops: 12,
        max_fee_drops: 100,
        now_ns: 1,
    })
    .unwrap();
    s.config.max_fee_drops = 100;
    s.begin_attempt(LeaseHolder::Setup, a.clone()).unwrap();
    let obs = Observation {
        tx: TxView::Succeeded {
            ledger_index: 20,
            tx_type: Some("AccountSet".into()),
            account: Some(holder_addr(5)),
            payment: None,
        },
        expiry: None,
    };
    assert!(matches!(reconcile(&a, &obs), Reconciled::Quarantine(_)));
    let obs = Observation {
        tx: TxView::Succeeded {
            ledger_index: 20,
            tx_type: Some("Payment".into()),
            account: Some(issuer_addr()),
            payment: None,
        },
        expiry: None,
    };
    assert!(matches!(reconcile(&a, &obs), Reconciled::Quarantine(_)));
}

// ---- trust line admission --------------------------------------------------------------------------

#[test]
fn trust_line_admission() {
    use crate::chains::xrp::issued_rpc::TrustLine;
    let line = |limit: &str, bal: &str, frozen: bool| TrustLine {
        holder: holder_addr(9),
        issuer: issuer_addr(),
        balance: ExactIssuedAmount::parse(bal).unwrap(),
        limit: ExactIssuedAmount::parse(limit).unwrap(),
        limit_peer: ExactIssuedAmount::ZERO,
        no_ripple: false,
        no_ripple_peer: false,
        freeze: false,
        freeze_peer: frozen,
        authorized: false,
        peer_authorized: false,
    };
    assert!(matches!(check_trust_line(None, 100), Err(XrplIcusdError::TrustLineMissing(_))));
    assert!(check_trust_line(Some(&line("1000", "0", false)), 100_000_000).is_ok());
    assert!(check_trust_line(Some(&line("1", "0", false)), 100_000_000).is_ok());
    assert!(check_trust_line(Some(&line("1", "0.5", false)), 100_000_000).is_err());
    assert!(check_trust_line(Some(&line("1000", "0", true)), 100).is_err());
    assert!(check_trust_line(Some(&line("0", "0", false)), 1).is_err());
}

// ---- tick ----------------------------------------------------------------------------------------------

#[test]
fn tick_is_idle_free_and_backs_off() {
    // No work: no timer.
    assert_eq!(next_tick_delay(false, true, 30), None);
    assert_eq!(next_tick_delay(false, false, 300), None);
    // Progress: back to the active cadence.
    assert_eq!(next_tick_delay(true, true, 300), Some(30));
    // No progress: exponential back-off capped at 5 minutes.
    assert_eq!(next_tick_delay(true, false, 30), Some(60));
    assert_eq!(next_tick_delay(true, false, 60), Some(120));
    assert_eq!(next_tick_delay(true, false, 120), Some(240));
    assert_eq!(next_tick_delay(true, false, 240), Some(300));
    assert_eq!(next_tick_delay(true, false, 300), Some(300));
    // An empty or all-terminal lane has no work.
    let mut s = ready_state();
    assert!(!s.has_work());
    let id = new_import(&mut s, owner(1), 1, 100);
    assert!(s.has_work(), "an unresolved pull is work");
    fund_import(&mut s, id);
    assert!(s.has_work(), "a funded claim awaits delivery");
    deliver(&mut s, id, 5);
    assert!(!s.has_work(), "delivered: the timer stops");
    // Cancelled and resolved operations are idle too.
    let c = new_import(&mut s, owner(1), 2, 100);
    s.begin_job_call(c, 0).unwrap();
    s.finish_job_call(c, 0, LedgerCallResult::Rejected("x".into()), 1).unwrap();
    assert!(s.has_work(), "a failed pull still needs its phase advance");
    issuance::advance_import(&mut s, c, 2).unwrap();
    assert!(!s.has_work());
}

#[test]
fn tick_plan_is_bounded_and_round_robin() {
    let mut s = ready_state();
    for b in 1..=8u8 {
        new_import(&mut s, owner(b), b, 100);
    }
    let first = plan_tick(&s, 3, 0);
    assert_eq!(first.len(), 3);
    let after = match first.last() {
        Some(TickItem::Op(id)) => *id,
        other => panic!("{other:?}"),
    };
    let second = plan_tick(&s, 3, after);
    assert_eq!(second.len(), 3);
    assert!(second.iter().all(|i| !first.contains(i)), "the cursor advances");
    // Wraps around.
    let wrap = plan_tick(&s, 8, 5);
    assert_eq!(wrap.len(), 8);
    // Operations with nothing to do are skipped.
    let mut s2 = ready_state();
    let a = new_import(&mut s2, owner(1), 1, 100);
    deliver_import_fully(&mut s2, a);
    assert!(plan_tick(&s2, 6, 0).is_empty());
}

fn deliver_import_fully(s: &mut XrplIcusdState, id: OperationId) {
    fund_import(s, id);
    deliver(s, id, 5);
}

#[test]
fn tick_guard_is_exclusive() {
    let g1 = TickGuard::acquire();
    assert!(g1.is_some());
    assert!(TickGuard::acquire().is_none(), "no overlapping passes");
    drop(g1);
    assert!(TickGuard::acquire().is_some());
    let o1 = OpGuard::acquire(5).unwrap();
    assert!(OpGuard::acquire(5).is_err());
    assert!(OpGuard::acquire(6).is_ok());
    drop(o1);
    assert!(OpGuard::acquire(5).is_ok());
    let i1 = IssuerGuard::acquire().unwrap();
    assert!(IssuerGuard::acquire().is_err());
    drop(i1);
    assert!(IssuerGuard::acquire().is_ok());
}

// ---- caller isolation at the entry points ---------------------------------------------------------------------

#[test]
fn anonymous_and_wrong_owner_are_refused() {
    use futures::executor::block_on;
    let anon = Principal::anonymous();
    // Each of these returns BEFORE touching state, the IC, or any await.
    assert_eq!(
        block_on(issue_from_canonical(
            anon,
            IssueFromCanonicalArg {
                amount_e8s: 1,
                recipient: holder_addr(9),
                network: TESTNET,
                request_nonce: vec![0; 32],
            }
        ))
        .unwrap_err(),
        XrplIcusdError::AnonymousCaller
    );
    assert_eq!(
        block_on(resume_operation(anon, 1)).unwrap_err(),
        XrplIcusdError::AnonymousCaller
    );
    assert_eq!(
        block_on(resolve_claim(
            anon,
            ResolveClaimArg {
                operation_id: 1,
                action: ClaimAction::ReceiveCanonical
            }
        ))
        .unwrap_err(),
        XrplIcusdError::AnonymousCaller
    );
    assert!(get_operations(anon, None, 50).items.is_empty());
    // Wrong owner: indistinguishable from a missing operation (state lookups).
    let mut s = ready_state();
    let id = new_import(&mut s, owner(1), 1, 100);
    assert_eq!(s.op_for_owner(&owner(2), id).err(), Some(XrplIcusdError::NotFound));
    assert_eq!(s.op_for_owner(&owner(1), 999).err(), Some(XrplIcusdError::NotFound));
    assert_eq!(
        issuance::plan_resolution(&s, &owner(2), &ResolveClaimArg { operation_id: id, action: ClaimAction::ReceiveCanonical }),
        Err(XrplIcusdError::NotFound)
    );
    // The page and view of one principal never contain another's rows.
    assert!(owner_page(&s, &owner(2), None, 50).items.is_empty());
}

#[test]
fn wrong_owner_cannot_resume_or_resolve_through_the_entry_points() {
    use futures::executor::block_on;
    crate::state::replace_state(test_init_state());
    crate::state::mutate_state(|st| {
        st.xrpl_icusd = ready_state();
        new_import(&mut st.xrpl_icusd, owner(1), 1, 100);
    });
    // Another principal gets NotFound BEFORE any ledger or XRPL call is attempted.
    assert_eq!(block_on(resume_operation(owner(2), 0)).unwrap_err(), XrplIcusdError::NotFound);
    assert_eq!(
        block_on(resolve_claim(
            owner(2),
            ResolveClaimArg {
                operation_id: 0,
                action: ClaimAction::ReissueXrpl
            }
        ))
        .unwrap_err(),
        XrplIcusdError::NotFound
    );
    assert_eq!(get_operations(owner(2), None, 50).items.len(), 0);
    assert_eq!(get_operations(owner(1), None, 50).items.len(), 1);
    // A malformed nonce is refused before anything else.
    assert!(matches!(
        block_on(issue_from_canonical(
            owner(1),
            IssueFromCanonicalArg {
                amount_e8s: 1,
                recipient: holder_addr(9),
                network: TESTNET,
                request_nonce: vec![0; 5],
            }
        )),
        Err(XrplIcusdError::InvalidArgument(_))
    ));
}

#[test]
fn public_config_is_key_free_and_disabled_by_default() {
    let d = XrplIcusdState::default();
    let cfg = public_config(&d, backend());
    assert!(!cfg.enabled && cfg.issuance_paused && !cfg.ready);
    assert!(cfg.policy.is_none() && cfg.issuer_address.is_none());
    assert_eq!(cfg.currency_hex, "6963555344000000000000000000000000000000");
    assert_eq!(cfg.escrow_account, escrow_account(backend()));
    let r = public_config(&ready_state(), backend());
    assert!(r.ready);
    // network <-> key pairing for the token lane
    assert!(check_network_for_key(XrplNetwork::Mainnet, "key_1").is_ok() == !crate::chains::xrp::network::FIXTURE_BUILD
        || crate::chains::xrp::network::FIXTURE_BUILD);
}

#[test]
fn issuer_derivation_path_is_domain_separated() {
    let p = issuer_derivation_path(XrplNetwork::Testnet);
    assert_eq!(p, vec![b"rumi".to_vec(), b"xrpl-icusd".to_vec(), b"v1".to_vec(), b"testnet".to_vec()]);
    assert_ne!(p, issuer_derivation_path(XrplNetwork::Mainnet));
    let custody = crate::chains::xrp::ted25519::custody_derivation_path(
        crate::chains::xrp::config::XRP_CHAIN_ID,
        owner(1),
        0,
    );
    let settlement = crate::chains::xrp::ted25519::settlement_derivation_path(crate::chains::xrp::config::XRP_CHAIN_ID);
    assert_ne!(p, custody);
    assert_ne!(p, settlement);
}

#[test]
fn user_triggered_outcalls_are_throttled() {
    // First call is accepted, an immediate repeat is refused, after the gap it passes.
    assert!(throttle_ok(9, b"a", 1_000, 100));
    assert!(!throttle_ok(9, b"a", 1_050, 100));
    assert!(throttle_ok(9, b"a", 1_100, 100));
    // Independent per key and per kind.
    assert!(throttle_ok(9, b"b", 1_101, 100));
    assert!(throttle_ok(8, b"a", 1_101, 100));
}

#[test]
fn job_specs_pin_who_pays_whom() {
    let b = backend();
    let mint = mint_job_spec(b, 5, 9);
    assert_eq!((mint.from_subaccount, mint.to), (None, escrow_account(b)));
    assert_eq!(mint.purpose, IcJobPurpose::EscrowMint);
    let burn = burn_job_spec(b, 5, 9);
    assert_eq!(burn.from_subaccount, Some(escrow_subaccount()));
    assert_eq!(burn.to, backend_default_account(b));
    let fee = fee_mint_job_spec(Account { owner: owner(4), subaccount: None }, 5, 9);
    assert_eq!(fee.purpose, IcJobPurpose::FeeMint);
    assert_ne!(fee.to, escrow_account(b), "fee remainder is never minted into escrow");
    assert_eq!(mint.ledger_tip_before, Some(9));
}
