//! The 49c fix pin (Miles' review of dba814d6): a mint operation is admitted only for the deposit it
//! was built from, in the same synchronous step that stores it, and a looked-up transaction is bound
//! to the signature it was looked up by before anything is recorded from it.

use crate::env::{self, test_env};
use crate::flows::{deposit, mint_saga};
use crate::observer;
use crate::saga49c_tests::{ata, credited, good_meta, memo, observe, observer_world, return_wire, strays, HOLDER, SRC};
use crate::saga_tests::{self, buckets, deposit_of, escrowed, user, world, E8};
use crate::state::{self, DepositState};
use crate::testkit::*;
use crate::tx::{assemble_wire_tx, split_wire_transaction};

// ---------------------------------------------------------------------------
// 1. A delayed mint preparation can no longer outlive a refundable deposit
// ---------------------------------------------------------------------------

/// The interleaving of the review, with one real deposit: an old pass starts a valid mint and is
/// suspended in its nonce-account read; an admin lowers `max_deposit_e8s`; a later pass tries the same
/// deposit at the new cap and holds it (no operation exists); the owner requests the refund of the
/// held deposit, which pays the ICP back; the old read returns.
fn run_the_interleaving(fx: &Fixture, id: u64) -> (bool, Option<u64>) {
    let mint_id = deposit_of(id).mint_id.expect("a user deposit has its mint id");
    let now = env::now_ns();

    // The old pass: valid at the old cap; suspended inside the nonce read (the reply is already taken).
    fx.chain.defer_next_account_read.set(true);
    let mut old = Box::pin(mint_saga::submit_mints(&fx.chain, &fx.signer, now));
    assert!(poll_once(old.as_mut()).is_pending(), "the old pass must be suspended in its read");
    assert!(deposit_of(id).mint_op.is_none() && state::op_by_external(&mint_id).is_none(), "nothing is stored yet");

    // The admin lowers the cap below this deposit.
    test_env::set_caller(saga_tests::admin());
    let mut limits = state::read_core(|c| c.config.limits.clone());
    limits.max_deposit_e8s = limits.min_deposit_e8s;
    assert!(deposit_of(id).amount_e8s > limits.max_deposit_e8s);
    crate::admin::set_limits(limits).expect("the real limits setter");

    // A later pass (the timer reclaimed a stale guard) tries the same deposit at the new cap.
    block_on(mint_saga::submit_mints(&fx.chain, &fx.signer, now + 1));
    assert!(matches!(deposit_of(id).state, DepositState::MintHeld { .. }), "{:?}", deposit_of(id).state);
    assert!(state::op_by_external(&mint_id).is_none(), "the later pass built no operation");

    // The owner refunds the held deposit: no operation exists, so the ICP backing goes back.
    test_env::set_caller(user(0x21));
    block_on(deposit::request_refund(&fx.ledger, id)).expect("the refund of a held deposit with no operation");
    block_on(deposit::process_refunds(&fx.ledger));
    assert_eq!(deposit_of(id).state, DepositState::Refunded);
    assert_eq!(buckets().pending_mint, 0, "the backing left the escrow");

    // The old read returns.
    block_on(old);
    let refunded = deposit_of(id).state == DepositState::Refunded;
    (refunded, state::op_by_external(&mint_id))
}

#[test]
fn a_suspended_mint_preparation_cannot_create_a_mint_for_a_deposit_that_was_refunded() {
    let fx = world();
    let id = escrowed(&fx, 0x21, 5 * E8, 1);
    let (refunded, op) = run_the_interleaving(&fx, id);
    assert!(refunded);

    if let Some(op_id) = op {
        // The unsafe result: show that the original really lands, on a deposit whose ICP was refunded.
        let wire = fx.wire_of(op_id, 0);
        assert!(fx.chain.land(&wire, false));
        fx.tick(env::now_ns() + secs(60));
        let landed = state::get_op(op_id).expect("op").state;
        panic!("REFUND PLUS MINT: deposit {id} is {:?} (ICP refunded) while mint operation {op_id} was admitted and is now {landed:?}", deposit_of(id).state);
    }

    // The safe result: no operation, no reservation, no signature, the deposit stays refunded.
    assert_eq!(deposit_of(id).mint_op, None);
    assert!(fx.signer.calls.borrow().is_empty(), "nothing was signed for it");
    assert_eq!(buckets().pending_mint, 0);
    assert_eq!(state::read_core(|c| c.next_op_id), 0, "no operation id was consumed");
    // The slot the old pass would have reserved is still free: the next deposit mints normally.
    test_env::set_caller(saga_tests::admin());
    let mut limits = state::read_core(|c| c.config.limits.clone());
    limits.max_deposit_e8s = 1_000 * E8;
    crate::admin::set_limits(limits).unwrap();
    let id2 = escrowed(&fx, 0x22, 2 * E8, 2);
    block_on(mint_saga::submit_mints(&fx.chain, &fx.signer, env::now_ns() + 2));
    assert!(matches!(deposit_of(id2).state, DepositState::MintInFlight));
    saga_tests::assert_covered(&fx);
}

#[test]
fn a_deposit_held_and_refunded_with_unchanged_terms_is_not_minted_by_the_old_request() {
    // The same race without any change of terms: the request the suspended pass holds is exactly the
    // deposit's own, so only the deposit's STATE at the commit can refuse it.
    let fx = world();
    let id = escrowed(&fx, 0x21, 5 * E8, 1);
    let mint_id = deposit_of(id).mint_id.unwrap();
    fx.chain.defer_next_account_read.set(true);
    let mut old = Box::pin(mint_saga::submit_mints(&fx.chain, &fx.signer, env::now_ns()));
    assert!(poll_once(old.as_mut()).is_pending());
    // Held by the preflight hold the saga itself applies, then refunded by its owner.
    deposit::on_mint_preflight_hold(id, "the mint cannot be built".into());
    test_env::set_caller(user(0x21));
    block_on(deposit::request_refund(&fx.ledger, id)).expect("refund of a held deposit");
    block_on(deposit::process_refunds(&fx.ledger));
    assert_eq!(deposit_of(id).state, DepositState::Refunded);
    assert_eq!(block_on(old), 0);
    assert!(state::op_by_external(&mint_id).is_none(), "no mint operation for a refunded deposit");
    assert!(fx.signer.calls.borrow().is_empty());
    assert_eq!(buckets().pending_mint, 0);
}

#[test]
fn admission_is_bound_to_the_deposits_current_request_pause_and_halt() {
    // The same deposit and the same mint id, but the request that was built for it is stale at the
    // commit: the cap (and so the payload) changed while the nonce was being read. Refused, nothing
    // stored, and the next pass builds it afresh with the same mint id.
    let fx = world();
    let id = escrowed(&fx, 0x21, 5 * E8, 1);
    let mint_id = deposit_of(id).mint_id.unwrap();
    let now = env::now_ns();
    fx.chain.defer_next_account_read.set(true);
    let mut old = Box::pin(mint_saga::submit_mints(&fx.chain, &fx.signer, now));
    assert!(poll_once(old.as_mut()).is_pending());
    test_env::set_caller(saga_tests::admin());
    let mut limits = state::read_core(|c| c.config.limits.clone());
    limits.max_deposit_e8s = 500 * E8;
    crate::admin::set_limits(limits).unwrap();
    assert_eq!(block_on(old), 0, "the stale request is not admitted");
    assert!(state::op_by_external(&mint_id).is_none() && deposit_of(id).mint_op.is_none());
    assert_eq!(deposit_of(id).state, DepositState::Escrowed, "still escrowed, not held: nothing was wrong with the deposit");
    assert_eq!(block_on(mint_saga::submit_mints(&fx.chain, &fx.signer, now + 1)), 1);
    let op = deposit_of(id).mint_op.expect("linked in the same step that stored it");
    assert_eq!(state::get_op(op).unwrap().external_id, mint_id, "the same mint id");

    // A pause or a halt that arrives while the read is in flight is honored at the admission.
    for case in 0..2 {
        let id = escrowed(&fx, 0x30 + case, E8, 10 + case);
        fx.chain.defer_next_account_read.set(true);
        let mut old = Box::pin(mint_saga::submit_mints(&fx.chain, &fx.signer, now + 5));
        assert!(poll_once(old.as_mut()).is_pending());
        if case == 0 {
            state::mutate_core(|c| c.config.pauses.mints = true);
        } else {
            state::halt("test halt");
        }
        assert_eq!(block_on(old), 0, "case {case}");
        assert!(deposit_of(id).mint_op.is_none(), "case {case}");
        assert_eq!(deposit_of(id).state, DepositState::Escrowed, "case {case}");
        state::mutate_core(|c| {
            c.config.pauses.mints = false;
            c.halted = None;
        });
    }
}

#[test]
fn the_deposit_is_linked_in_the_same_step_that_stores_the_operation() {
    let fx = world();
    let id = escrowed(&fx, 0x21, 5 * E8, 1);
    // Observe the stored state at the moment the first signature is requested: the signer is called
    // only after the commit, and the commit already linked the deposit.
    let seen = std::rc::Rc::new(std::cell::RefCell::new(None));
    let sink = seen.clone();
    *fx.signer.on_call.borrow_mut() = Some(Box::new(move |_call| {
        let d = state::get_deposit(id).unwrap();
        *sink.borrow_mut() = Some((d.state, d.mint_op));
    }));
    assert_eq!(block_on(mint_saga::submit_mints(&fx.chain, &fx.signer, env::now_ns())), 1);
    let (st, op) = seen.borrow().clone().expect("the signer was called");
    assert_eq!(st, DepositState::MintInFlight);
    assert!(op.is_some());
}

// ---------------------------------------------------------------------------
// 2. A looked-up transaction is bound to the signature it was looked up by
// ---------------------------------------------------------------------------

/// A valid finalized return with its balance effect, as the RPC would report it.
fn valid_return(amount: u64, tag: u8) -> (Vec<u8>, crate::sol_rpc::TxMeta, crate::tx::Signature) {
    let (wire, msg) = return_wire(&HOLDER, &SRC, amount, memo(tag as u64), tag);
    let meta = good_meta(&msg, &HOLDER, &SRC, amount, 10 * amount, 0);
    let (sigs, _) = split_wire_transaction(&wire).unwrap();
    (wire, meta, sigs[0])
}

#[test]
fn the_same_wire_returned_under_other_signatures_changes_nothing_and_the_real_one_credits_once() {
    let fx = observer_world();
    let (wire, meta, s) = valid_return(5 * E8, 1);
    let (h1, h2) = ([0xe1; 64], [0xe2; 64]);
    assert!(h1 != s && h2 != s);
    fx.chain.land_under(h1, &wire, false, meta.clone(), &[ata()]);
    fx.chain.land_under(h2, &wire, false, meta.clone(), &[ata()]);
    let before = buckets();
    let events = observe(&fx);
    assert_eq!(credited(&events), 0, "a response for the wrong key was credited: {events:?}");
    assert!(strays(&events).is_empty(), "{events:?}");
    assert_eq!(buckets(), before, "O and RC are untouched");
    assert!(state::list_returns(None, 10).is_empty() && state::stray_count() == 0);
    assert!(events.iter().any(|e| matches!(e, observer::ObserverEvent::Waiting { why } if why.contains("signature"))), "{events:?}");

    // The actual signature, in a world with no wrong answers: credited once, and seen again it dedups.
    let fx = observer_world();
    let (wire, meta, s) = valid_return(5 * E8, 1);
    assert_eq!(fx.chain.land_with_meta(&wire, false, meta, &[ata()]), s);
    let events = observe(&fx);
    assert_eq!(credited(&events), 1, "{events:?}");
    let after = buckets();
    assert_eq!((after.return_custodied, after.outstanding), ((5 * E8) as u128, ((1_000 - 5) * E8) as u128));
    state::mutate_core(|c| c.observer.cursor = None);
    test_env::advance_secs(observer::OBSERVER_IDLE_INTERVAL_NS / 1_000_000_000 + 1);
    let events = observe(&fx);
    assert_eq!(credited(&events), 0, "{events:?}");
    assert_eq!(buckets(), after);
}

#[test]
fn a_wrong_key_response_creates_no_stray_claim_either() {
    let fx = observer_world();
    // A malformed transaction (no return memo) with a measurable incoming transfer, returned under a key
    // that is not its first signature.
    let (wire, msg) = return_wire(&HOLDER, &SRC, 3 * E8, memo(9), 9);
    let mut meta = good_meta(&msg, &HOLDER, &SRC, 3 * E8, 10 * E8, 0);
    // Make it not a return: the source is not the signing owner.
    meta.pre_token_balances.as_mut().unwrap()[0].owner = Some([0x77; 32]);
    fx.chain.land_under([0xe3; 64], &wire, false, meta, &[ata()]);
    let before = buckets();
    let events = observe(&fx);
    assert!(strays(&events).is_empty() && credited(&events) == 0, "{events:?}");
    assert_eq!(buckets(), before);
    assert_eq!(state::stray_count(), 0);
}

#[test]
fn a_zero_or_mismatched_signature_count_is_rejected_by_the_existing_decoder() {
    for case in 0..3 {
        let fx = observer_world();
        let (wire, meta, s) = valid_return(5 * E8, 1);
        let (sigs, message_bytes) = split_wire_transaction(&wire).unwrap();
        let message_bytes = message_bytes.to_vec();
        let (bad, name) = match case {
            // No signature at all.
            0 => (assemble_wire_tx(&[], &message_bytes).unwrap(), "zero signatures"),
            // One signature too many for the header (the first one is still the right key).
            1 => (assemble_wire_tx(&[sigs[0], [0x99; 64]], &message_bytes).unwrap(), "more signatures than the header"),
            // A wire cut inside its signature list.
            _ => (wire[..40].to_vec(), "truncated wire"),
        };
        // Looked up under the real signature's key.
        fx.chain.land_under(s, &bad, false, meta, &[ata()]);
        let before = buckets();
        let events = observe(&fx);
        assert_eq!(credited(&events), 0, "{name}: {events:?}");
        assert!(strays(&events).is_empty(), "{name}: {events:?}");
        assert_eq!(buckets(), before, "{name}");
        assert!(state::list_returns(None, 10).is_empty() && state::stray_count() == 0, "{name}");
        assert!(events.iter().any(|e| matches!(e, observer::ObserverEvent::Waiting { .. })), "{name}: {events:?}");
    }
}
#[test]
fn the_binding_check_itself() {
    use crate::sol_rpc::{check_bound, LandedTx, TxMeta};
    let _fx = observer_world();
    let (wire, _meta, s) = valid_return(E8, 1);
    let tx = |wire: Vec<u8>| LandedTx { slot: 1, wire, failed: false, meta: TxMeta::default() };
    assert_eq!(check_bound(&s, &tx(wire.clone())), Ok(()));
    assert!(check_bound(&[0x02; 64], &tx(wire.clone())).is_err());
    assert!(check_bound(&s, &tx(vec![0])).is_err(), "zero signatures");
    assert!(check_bound(&s, &tx(vec![])).is_err(), "empty");
    let (_, message_bytes) = split_wire_transaction(&wire).unwrap();
    // Whatever the message is, the first signature of the wire must still be the requested one.
    let garbage = assemble_wire_tx(&[s], &[0xff, 0xff, 0xff]).unwrap();
    assert_eq!(check_bound(&s, &tx(garbage)), Ok(()));
    let two = assemble_wire_tx(&[s, [3; 64]], message_bytes).unwrap();
    assert!(check_bound(&s, &tx(two)).is_err(), "header and wire disagree");
}
