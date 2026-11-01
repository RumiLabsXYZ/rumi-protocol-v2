use super::*;
use std::cell::RefCell;
use std::collections::VecDeque;

pub(super) mod mock {
    use super::*;
    #[derive(Default)]
    pub struct Env {
        pub pending: Option<&'static str>,
        pub balance: Option<u128>,
        pub funds: Vec<TransferFromArgs>,
        pub mints: Vec<Principal>,
        pub refunds: Vec<TransferArg>,
        pub funding_results: VecDeque<Result<Result<Nat, TransferFromError>, String>>,
        pub mint_results: VecDeque<(DepositOutcome, u128)>,
        pub refund_results: VecDeque<Result<Result<Nat, TransferError>, String>>,
    }
    thread_local! { pub static ENV: RefCell<Option<Env>> = const { RefCell::new(None) }; }
    pub fn pending(stage: &str) -> bool {
        ENV.with(|e| e.borrow().as_ref().unwrap().pending == Some(stage))
    }
    pub fn enabled() -> bool {
        ENV.with(|e| e.borrow().is_some())
    }
    pub fn time() -> Option<u64> {
        enabled().then_some(1_000_000_000_000)
    }
    pub fn balance() -> Option<u128> {
        ENV.with(|e| {
            e.borrow()
                .as_ref()
                .map(|e| e.balance.unwrap_or(100_000_000_000_000))
        })
    }
    pub fn fund(a: TransferFromArgs) -> Result<Result<Nat, TransferFromError>, String> {
        ENV.with(|e| {
            let mut e = e.borrow_mut();
            let e = e.as_mut().unwrap();
            e.funds.push(a);
            e.funding_results.pop_front().expect("unexpected funding")
        })
    }
    pub fn mint(a: Principal) -> (DepositOutcome, u128) {
        ENV.with(|e| {
            let mut e = e.borrow_mut();
            let e = e.as_mut().unwrap();
            e.mints.push(a);
            e.mint_results.pop_front().expect("unexpected mint")
        })
    }
    pub fn refund(a: TransferArg) -> Result<Result<Nat, TransferError>, String> {
        ENV.with(|e| {
            let mut e = e.borrow_mut();
            let e = e.as_mut().unwrap();
            e.refunds.push(a);
            e.refund_results.pop_front().expect("unexpected refund")
        })
    }
}
fn setup(env: mock::Env) -> Principal {
    let owner = Principal::from_slice(&[7]);
    let mut state = State::default();
    state.cksol_icp_reserve = ATTACHED_CYCLES;
    crate::state::replace_state(state);
    mock::ENV.with(|e| *e.borrow_mut() = Some(env));
    owner
}
fn begin(owner: Principal) -> Result<CkSolIcpPayment, String> {
    futures::executor::block_on(start(
        owner,
        1,
        deposit_cap(50_000, 10_000).unwrap(),
        50_000,
        1000,
        10_000,
    ))
}
fn again(owner: Principal) -> Result<CkSolIcpPayment, String> {
    futures::executor::block_on(resume(owner, 1))
}
#[test]
fn paid_mint_refund_and_idempotent_replay() {
    let mut env = mock::Env::default();
    env.funding_results.push_back(Ok(Ok(Nat::from(12u64))));
    env.mint_results
        .push_back((DepositOutcome::Queued(42), 950_000_000_000));
    env.refund_results.push_back(Ok(Ok(Nat::from(13u64))));
    let owner = setup(env);
    let p = begin(owner).unwrap();
    assert_eq!(p.phase, CkSolPaymentPhase::Complete);
    assert_eq!(p.deposit_id, Some(42));
    assert_eq!(p.net_cycles, 50_000_000_000);
    assert_eq!(p.service_fee_cycles, 2_500_000_000);
    assert_eq!(p.refund_block, Some(Nat::from(13u64)));
    assert_eq!(begin(owner).unwrap(), p);
    mock::ENV.with(|e| {
        let e = e.borrow();
        let e = e.as_ref().unwrap();
        assert_eq!((e.funds.len(), e.mints.len(), e.refunds.len()), (1, 1, 1));
        assert_eq!(e.funds[0].from.owner, owner);
        assert_eq!(e.refunds[0].to.owner, owner);
    });
}
#[test]
fn lost_funding_reply_never_debits_or_dispatches_again() {
    let mut env = mock::Env::default();
    env.funding_results.push_back(Err("lost".into()));
    let owner = setup(env);
    assert_eq!(
        begin(owner).unwrap().phase,
        CkSolPaymentPhase::FundingUnknown
    );
    assert_eq!(
        again(owner).unwrap().phase,
        CkSolPaymentPhase::FundingUnknown
    );
    mock::ENV.with(|e| {
        let e = e.borrow();
        let e = e.as_ref().unwrap();
        assert_eq!((e.funds.len(), e.mints.len(), e.refunds.len()), (1, 0, 0));
    });
}
#[test]
fn unknown_mint_and_lost_refund_resume_only_exact_refund() {
    let mut env = mock::Env::default();
    env.funding_results.push_back(Ok(Ok(Nat::from(12u64))));
    env.mint_results
        .push_back((DepositOutcome::Unknown("lost".into()), 950_000_000_000));
    env.refund_results.push_back(Err("lost".into()));
    env.refund_results
        .push_back(Ok(Err(TransferError::Duplicate {
            duplicate_of: Nat::from(13u64),
        })));
    let owner = setup(env);
    assert_eq!(
        begin(owner).unwrap().phase,
        CkSolPaymentPhase::RefundUnknown
    );
    let mut bytes = vec![];
    read_state(|s| ciborium::ser::into_writer(s, &mut bytes).unwrap());
    crate::state::replace_state(crate::storage::decode_state_body(&bytes).unwrap());
    assert_eq!(again(owner).unwrap().phase, CkSolPaymentPhase::Complete);
    mock::ENV.with(|e| {
        let e = e.borrow();
        let e = e.as_ref().unwrap();
        assert_eq!((e.funds.len(), e.mints.len(), e.refunds.len()), (1, 1, 2));
        assert_eq!(e.refunds[0], e.refunds[1]);
    });
    assert_eq!(
        crate::cksol::attempt(owner).unwrap().status,
        CkSolRelayStatus::SubmittedOrUnknown
    );
}

#[test]
fn simulated_interruption_and_state_codec_preserve_holds_and_refund_tuple() {
    use futures::{task::noop_waker, Future};
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };
    for stage in ["fund", "mint", "refund"] {
        let mut env = mock::Env::default();
        env.pending = Some(stage);
        env.funding_results.push_back(Ok(Ok(Nat::from(12u64))));
        env.mint_results
            .push_back((DepositOutcome::Queued(42), 950_000_000_000));
        env.refund_results.push_back(Ok(Ok(Nat::from(13u64))));
        if stage == "refund" {
            env.refund_results
                .push_back(Ok(Err(TransferError::Duplicate {
                    duplicate_of: Nat::from(13u64),
                })));
        }
        let owner = setup(env);
        let mut future = Box::pin(start(
            owner,
            1,
            deposit_cap(50_000, 10_000).unwrap(),
            50_000,
            1000,
            10_000,
        ));
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        assert!(matches!(
            Pin::as_mut(&mut future).poll(&mut cx),
            Poll::Pending
        ));
        let expected = match stage {
            "fund" => CkSolPaymentPhase::FundingPending,
            "mint" => CkSolPaymentPhase::DepositPending,
            _ => CkSolPaymentPhase::RefundPending,
        };
        assert_eq!(payment(owner).unwrap().phase, expected);
        let mut bytes = vec![];
        read_state(|s| ciborium::ser::into_writer(s, &mut bytes).unwrap());
        drop(future);
        crate::state::replace_state(crate::storage::decode_state_body(&bytes).unwrap());
        mock::ENV.with(|e| e.borrow_mut().as_mut().unwrap().pending = None);
        let resumed = again(owner).unwrap();
        assert_eq!(
            resumed.phase,
            if stage == "refund" {
                CkSolPaymentPhase::Complete
            } else {
                expected
            }
        );
        mock::ENV.with(|e| {
            let e = e.borrow();
            let e = e.as_ref().unwrap();
            assert_eq!(e.funds.len(), 1);
            assert_eq!(e.mints.len(), if stage == "fund" { 0 } else { 1 });
            if stage == "refund" {
                assert_eq!(e.refunds.len(), 2);
                assert_eq!(e.refunds[0], e.refunds[1]);
            }
        });
    }
}

#[test]
fn rejected_funding_restores_reserve_without_minter_or_refund() {
    let mut env = mock::Env::default();
    env.funding_results
        .push_back(Ok(Err(TransferFromError::InsufficientFunds {
            balance: Nat::from(0u8),
        })));
    let owner = setup(env);
    assert_eq!(
        begin(owner).unwrap().phase,
        CkSolPaymentPhase::FundingRejected
    );
    assert_eq!(read_state(|s| s.cksol_icp_reserve), ATTACHED_CYCLES);
    assert!(crate::cksol::attempt(owner).is_none());
    mock::ENV.with(|e| {
        let e = e.borrow();
        let e = e.as_ref().unwrap();
        assert_eq!((e.funds.len(), e.mints.len(), e.refunds.len()), (1, 0, 0));
    });
}

#[test]
fn refund_bad_fee_and_expiry_hold_without_resubmission() {
    for error in [
        TransferError::BadFee {
            expected_fee: Nat::from(20_000u64),
        },
        TransferError::TooOld,
    ] {
        let mut env = mock::Env::default();
        env.funding_results.push_back(Ok(Ok(Nat::from(12u64))));
        env.mint_results
            .push_back((DepositOutcome::Rejected("rejected".into()), 950_000_000_000));
        env.refund_results.push_back(Ok(Err(error)));
        let owner = setup(env);
        assert_eq!(
            begin(owner).unwrap().phase,
            CkSolPaymentPhase::RefundPending
        );
        assert_eq!(
            again(owner).unwrap().phase,
            CkSolPaymentPhase::RefundPending
        );
        mock::ENV.with(|e| {
            let e = e.borrow();
            let e = e.as_ref().unwrap();
            assert_eq!((e.funds.len(), e.mints.len(), e.refunds.len()), (1, 1, 1));
        });
    }
}

#[test]
fn two_owners_progress_with_first_attachment_in_flight() {
    use futures::{task::noop_waker, Future};
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };
    let mut env = mock::Env::default();
    env.pending = Some("mint");
    env.balance = Some(CKSOL_RELAY_CANISTER_RESERVE + 2 * ATTACHED_CYCLES as u128);
    env.funding_results.push_back(Ok(Ok(Nat::from(1u8))));
    env.funding_results.push_back(Ok(Ok(Nat::from(2u8))));
    env.mint_results
        .push_back((DepositOutcome::Queued(11), 950_000_000_000));
    env.mint_results
        .push_back((DepositOutcome::Queued(12), 950_000_000_000));
    env.refund_results.push_back(Ok(Ok(Nat::from(3u8))));
    env.refund_results.push_back(Ok(Ok(Nat::from(4u8))));
    let a = setup(env);
    let b = Principal::from_slice(&[8]);
    mutate_state(|s| s.cksol_icp_reserve = 2 * ATTACHED_CYCLES);
    let mut first = Box::pin(start(
        a,
        1,
        deposit_cap(50_000, 10_000).unwrap(),
        50_000,
        1000,
        10_000,
    ));
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    assert!(matches!(
        Pin::as_mut(&mut first).poll(&mut cx),
        Poll::Pending
    ));
    assert_eq!(payment(a).unwrap().phase, CkSolPaymentPhase::DepositPending);
    mock::ENV.with(|e| {
        let mut e = e.borrow_mut();
        let e = e.as_mut().unwrap();
        e.pending = None;
        e.balance = Some(CKSOL_RELAY_CANISTER_RESERVE + ATTACHED_CYCLES as u128);
    });
    let second = futures::executor::block_on(start(
        b,
        2,
        deposit_cap(50_000, 10_000).unwrap(),
        50_000,
        1000,
        10_000,
    ))
    .unwrap();
    assert_eq!(second.phase, CkSolPaymentPhase::Complete);
    assert_eq!(second.deposit_id, Some(12));
    assert!(matches!(
        Pin::as_mut(&mut first).poll(&mut cx),
        Poll::Ready(Ok(_))
    ));
    assert_eq!(payment(a).unwrap().deposit_id, Some(11));
    mock::ENV.with(|e| {
        let e = e.borrow();
        let e = e.as_ref().unwrap();
        assert_eq!((e.funds.len(), e.mints.len(), e.refunds.len()), (2, 2, 2));
    });
}

#[test]
fn upgrade_interrupted_mint_refunds_once_and_ignores_late_callback() {
    use futures::{task::noop_waker, Future};
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };
    let mut env = mock::Env::default();
    env.pending = Some("mint");
    env.funding_results.push_back(Ok(Ok(Nat::from(12u64))));
    env.mint_results
        .push_back((DepositOutcome::Queued(42), 950_000_000_000));
    env.refund_results.push_back(Ok(Ok(Nat::from(13u64))));
    let owner = setup(env);
    let deposit = deposit_cap(50_000, 10_000).unwrap();
    let mut first = Box::pin(start(owner, 1, deposit, 50_000, 1000, 10_000));
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    assert!(matches!(
        Pin::as_mut(&mut first).poll(&mut cx),
        Poll::Pending
    ));
    assert_eq!(
        again(owner).unwrap_err(),
        "ckSOL account processing or capacity reached"
    );
    mutate_state(|s| {
        mark_interrupted_minter_calls(s, transport::time());
        s.cksol_relay_in_flight.clear();
    });
    let recovered = again(owner).unwrap();
    assert_eq!(recovered.phase, CkSolPaymentPhase::Complete);
    assert_eq!(recovered.refundable_e8s, deposit - 10_000);
    assert_eq!(
        (
            recovered.charged_e8s,
            recovered.service_fee_cycles,
            recovered.operating_fee_cycles
        ),
        (0, 0, 0)
    );
    assert_eq!(read_state(|s| s.cksol_icp_reserve), 0);
    assert_eq!(read_state(reserved), ATTACHED_CYCLES as u128);
    mock::ENV.with(|e| e.borrow_mut().as_mut().unwrap().pending = None);
    assert!(matches!(
        Pin::as_mut(&mut first).poll(&mut cx),
        Poll::Ready(Ok(_))
    ));
    assert_eq!(payment(owner).unwrap(), recovered);
    assert!(
        futures::executor::block_on(start(owner, 2, deposit, 50_000, 1000, 10_000))
            .unwrap_err()
            .contains("held for reconciliation")
    );
    mutate_state(|s| {
        let orphan = s.cksol_icp_payments[&owner].clone();
        s.cksol_icp_payments.clear();
        for index in 0..CKSOL_RELAY_OWNER_CAP {
            let principal = Principal::self_authenticating(&(index as u64).to_be_bytes());
            s.cksol_icp_payments.insert(principal, orphan.clone());
        }
        s.cksol_icp_reserve = ATTACHED_CYCLES;
    });
    assert!(
        !terms().available,
        "orphaned completed journals are not evictable capacity"
    );
    assert_eq!(
        crate::cksol::attempt(owner).unwrap().status,
        CkSolRelayStatus::SubmittedOrUnknown
    );
    mock::ENV.with(|e| {
        let e = e.borrow();
        let e = e.as_ref().unwrap();
        assert_eq!((e.funds.len(), e.mints.len(), e.refunds.len()), (1, 1, 1));
    });
}

#[test]
fn stale_minter_continuation_cannot_launch_a_second_pending_refund() {
    use futures::{task::noop_waker, Future};
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };
    let mut env = mock::Env::default();
    env.pending = Some("mint");
    env.funding_results.push_back(Ok(Ok(Nat::from(12u64))));
    env.mint_results
        .push_back((DepositOutcome::Queued(42), 950_000_000_000));
    env.refund_results.push_back(Ok(Ok(Nat::from(13u64))));
    let owner = setup(env);
    let mut original = Box::pin(start(
        owner,
        1,
        deposit_cap(50_000, 10_000).unwrap(),
        50_000,
        1000,
        10_000,
    ));
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    assert!(matches!(
        Pin::as_mut(&mut original).poll(&mut cx),
        Poll::Pending
    ));
    mutate_state(|s| {
        mark_interrupted_minter_calls(s, transport::time());
        s.cksol_relay_in_flight.clear();
    });
    mock::ENV.with(|e| e.borrow_mut().as_mut().unwrap().pending = Some("refund"));
    let mut recovery = Box::pin(resume(owner, 1));
    assert!(matches!(
        Pin::as_mut(&mut recovery).poll(&mut cx),
        Poll::Pending
    ));
    mock::ENV.with(|e| e.borrow_mut().as_mut().unwrap().pending = None);
    assert!(matches!(
        Pin::as_mut(&mut original).poll(&mut cx),
        Poll::Ready(Ok(_))
    ));
    mock::ENV.with(|e| assert_eq!(e.borrow().as_ref().unwrap().refunds.len(), 1));
    assert!(
        again(owner).unwrap_err().contains("processing or capacity"),
        "stale guard must not clear the recovery lock"
    );
    assert!(matches!(
        Pin::as_mut(&mut recovery).poll(&mut cx),
        Poll::Ready(Ok(_))
    ));
    assert_eq!(payment(owner).unwrap().phase, CkSolPaymentPhase::Complete);
}

#[test]
fn stale_funding_callback_after_recovery_cannot_mint_or_overwrite_receipt() {
    use futures::{task::noop_waker, Future};
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };
    let mut env = mock::Env::default();
    env.pending = Some("fund");
    env.funding_results.push_back(Ok(Ok(Nat::from(12u64))));
    let owner = setup(env);
    let mut original = Box::pin(start(
        owner,
        1,
        deposit_cap(50_000, 10_000).unwrap(),
        50_000,
        1000,
        10_000,
    ));
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    assert!(matches!(
        Pin::as_mut(&mut original).poll(&mut cx),
        Poll::Pending
    ));
    mutate_state(|s| {
        mark_interrupted_minter_calls(s, transport::time());
        s.cksol_relay_in_flight.clear();
        let j = s.cksol_icp_payments.get_mut(&owner).unwrap();
        j.payment.phase = CkSolPaymentPhase::RefundReady;
        j.payment.funding_block = Some(Nat::from(12u64));
        j.payment.refundable_e8s = j.payment.deposit_e8s - j.payment.ledger_fee_e8s;
    });
    let recovered = payment(owner).unwrap();
    mock::ENV.with(|e| e.borrow_mut().as_mut().unwrap().pending = None);
    assert!(matches!(
        Pin::as_mut(&mut original).poll(&mut cx),
        Poll::Ready(Ok(_))
    ));
    assert_eq!(payment(owner).unwrap(), recovered);
    mock::ENV.with(|e| {
        let e = e.borrow();
        let e = e.as_ref().unwrap();
        assert_eq!((e.funds.len(), e.mints.len(), e.refunds.len()), (1, 0, 0));
    });
}
