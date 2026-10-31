//! Tests of the 49a bootstrap driver and the timer pass against the mock chain: keys, pool, mint,
//! return account and bind, with the persist-before-sign discipline on every creation transaction.

use crate::bootstrap::*;
use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;
use crate::lane::{ensure_keys, SubmitRequest};
use crate::mint::fixtures::*;
use crate::nonce;
use crate::records::*;
use crate::state;
use crate::testkit::*;
use crate::timer::{run_once, RunReport};
use crate::tx::TokenProgram;

const T0: u64 = 1_000 * SECOND;

fn t(s: u64) -> u64 {
    T0 + secs(s)
}

fn run(fx: &Fixture, now: u64) -> RunReport {
    block_on(run_once(&fx.chain, &fx.signer, &fx.ledger, now))
}

fn all_targets(pool: u8) -> Vec<BootstrapTarget> {
    let mut v = vec![BootstrapTarget::Mint, BootstrapTarget::ReturnAta];
    for lane in Lane::ALL {
        for index in 0..pool {
            v.push(BootstrapTarget::PoolSlot { lane, index });
        }
    }
    v
}

fn stored_txs(pool: u8) -> Vec<BootstrapTx> {
    all_targets(pool).iter().filter_map(|t| state::get_bootstrap_tx(t.key())).collect()
}

#[test]
fn a_fresh_instance_bootstraps_keys_pool_mint_and_return_account_and_binds() {
    let fx = Fixture::unbootstrapped(2);
    // Persist before sign, record before send: for every creation transaction.
    fx.signer.on_call.replace(Some(Box::new(|call| {
        let found = stored_txs(2).iter().any(|tx| tx.message == call.message);
        assert!(found, "a creation transaction was signed before its exact bytes were stored");
    })));
    fx.chain.on_send.replace(Some(Box::new(|wire| {
        let (sigs, message) = crate::tx::split_wire_transaction(wire).unwrap();
        let tx = stored_txs(2).into_iter().find(|tx| tx.message == message).expect("the sent bytes are stored");
        assert_eq!(tx.signatures.len(), tx.signers.len(), "every signature is recorded before the send");
        assert!(tx.tx_ids.contains(&Sig(sigs[0])), "the transaction id is recorded before the send");
    })));

    // Pass 1: keys derived, the pool registered, every missing account's creation sent.
    let r = run(&fx, T0);
    assert!(r.bootstrap.contains(&BootstrapEvent::KeysDerived));
    assert_eq!(r.bootstrap.iter().filter(|e| matches!(e, BootstrapEvent::SlotRegistered { .. })).count(), 4);
    let sent: Vec<_> = r.bootstrap.iter().filter_map(|e| if let BootstrapEvent::Sent { target, fresh } = e { Some((*target, *fresh)) } else { None }).collect();
    assert_eq!(sent.len(), 5, "four pool accounts and the mint: {:?}", r.bootstrap);
    assert!(sent.iter().all(|(_, fresh)| *fresh));
    assert!(sent.iter().any(|(t, _)| *t == BootstrapTarget::Mint));
    assert_eq!(state::read_core(|c| c.mint_state), MintState::None);
    let keys = state::read_core(|c| c.keys.clone()).unwrap();
    assert_eq!((keys.fee_payer, keys.mint_account), (FEE_PAYER, Some(MINT)));
    // The mint creation is a two-signer transaction (fee payer and the one-time mint-account key).
    let mint_tx = state::get_bootstrap_tx(BootstrapTarget::Mint.key()).unwrap();
    assert_eq!(mint_tx.signers, vec![FEE_PAYER, MINT]);
    assert_eq!(mint_tx.signatures.len(), 2);

    // Pass 2: the pool accounts exist and verify; the mint exists, the return account is created.
    let r = run(&fx, t(40));
    assert_eq!(r.bootstrap.iter().filter(|e| matches!(e, BootstrapEvent::SlotVerified { .. })).count(), 4);
    assert!(r.bootstrap.contains(&BootstrapEvent::Sent { target: BootstrapTarget::ReturnAta, fresh: true }), "{:?}", r.bootstrap);
    assert_eq!(state::read_core(|c| c.mint_state), MintState::DeployedUnbound);
    assert!(state::slots_in_lane(Lane::Mint).iter().all(|s| s.state == SlotState::Free));
    assert_eq!(state::bootstrap_tx_count(), 1, "the completed creations were dropped; only the return account's is in flight");

    // Pass 3: bound.
    let r = run(&fx, t(80));
    assert!(r.bootstrap.contains(&BootstrapEvent::MintBound), "{:?}", r.bootstrap);
    assert_eq!(state::read_core(|c| (c.mint_state, c.mint)), (MintState::Bound, Some(MINT)));
    assert_eq!(state::bootstrap_tx_count(), 0);
    assert!(!needs_bootstrap());

    // Once bound and verified, a pass makes no calls at all.
    let (reads, sent, signs) = (fx.chain.call_count("get_account"), fx.chain.sent().len(), fx.signer.call_count());
    let r = run(&fx, t(120));
    assert!(r.bootstrap.is_empty(), "{:?}", r.bootstrap);
    assert_eq!((fx.chain.call_count("get_account"), fx.chain.sent().len(), fx.signer.call_count()), (reads, sent, signs));
    assert_eq!(fx.chain.call_count("get_slot"), 2, "one blockhash walk per creating pass (pass 1 for five accounts, pass 2 for the return account)");
}

#[test]
fn creation_transactions_have_the_documented_shape() {
    let fx = Fixture::unbootstrapped(1);
    run(&fx, T0);
    let profile = state::read_core(|c| c.creation_profile()).unwrap();
    // The mint creation equals the builder's bytes for the blockhash the walk found.
    let mint_tx = state::get_bootstrap_tx(BootstrapTarget::Mint.key()).unwrap();
    let expected = crate::tx::builders::build_mint_creation_message(&profile, crate::tx::MINT_RENT_LAMPORTS, mint_tx.recent_blockhash).unwrap();
    assert_eq!(mint_tx.message, expected.message_bytes);
    assert_eq!(mint_tx.recent_blockhash, [0x5b; 32], "a real block's hash, not a nonce");
    // The pool creation is a single-signature CreateAccountWithSeed + InitializeNonceAccount.
    let slot = state::get_slot(Lane::Mint, 0).unwrap();
    let pool_tx = state::get_bootstrap_tx(BootstrapTarget::PoolSlot { lane: Lane::Mint, index: 0 }.key()).unwrap();
    assert_eq!(pool_tx.signers, vec![FEE_PAYER]);
    let (expected, address) =
        crate::tx::builders::build_nonce_pool_account_message(&profile, &slot.seed, crate::tx::NONCE_ACCOUNT_RENT_LAMPORTS, pool_tx.recent_blockhash).unwrap();
    assert_eq!((pool_tx.message.clone(), address), (expected.message_bytes, slot.address));
}

#[test]
fn nothing_is_created_for_accounts_that_already_exist_and_one_pass_binds() {
    let fx = Fixture::unbootstrapped(2);
    block_on(ensure_keys(&fx.signer)).unwrap();
    for lane in Lane::ALL {
        for i in 0..2u8 {
            let (_, address) = nonce::slot_address(&FEE_PAYER, lane, i).unwrap();
            fx.chain.add_nonce_account(address, &FEE_PAYER, &slot_initial_value(lane, i));
        }
    }
    fx.chain.set_account(MINT, TokenProgram::Token2022.id(), mint_data(Some(&MINT_AUTHORITY), 0, 8, true, None));
    let profile = state::read_core(|c| c.creation_profile()).unwrap();
    let ata = profile.return_ata().unwrap();
    fx.chain.set_account(ata, TokenProgram::Token2022.id(), extended_token_account(token_account_data(&MINT, &RETURN_OWNER, 0, None, 1, None), &[(crate::mint::EXT_IMMUTABLE_OWNER, vec![])]));
    let r = run(&fx, T0);
    assert!(r.bootstrap.contains(&BootstrapEvent::MintBound), "{:?}", r.bootstrap);
    assert!(!r.bootstrap.iter().any(|e| matches!(e, BootstrapEvent::Sent { .. })));
    assert_eq!((fx.chain.sent().len(), fx.signer.call_count(), fx.chain.call_count("get_slot")), (0, 0, 0), "no signing, no sending, no blockhash walk");
    assert_eq!(state::read_core(|c| c.mint_state), MintState::Bound);
}

#[test]
fn wrong_existing_accounts_are_reported_and_never_overwritten() {
    let fx = Fixture::unbootstrapped(1);
    block_on(ensure_keys(&fx.signer)).unwrap();
    let (_, bad_pool) = nonce::slot_address(&FEE_PAYER, Lane::Mint, 0).unwrap();
    fx.chain.add_nonce_account(bad_pool, &[0x99; 32], &[1; 32]); // authority is not the fee payer
    fx.chain.set_account(MINT, TokenProgram::Token2022.id(), mint_data(Some(&[0x98; 32]), 0, 8, true, None)); // not our mint authority
    let r = run(&fx, T0);
    assert!(r.bootstrap.iter().any(|e| matches!(e, BootstrapEvent::SlotProblem { lane: Lane::Mint, index: 0, problem } if problem.contains("authority"))), "{:?}", r.bootstrap);
    assert!(r.bootstrap.iter().any(|e| matches!(e, BootstrapEvent::MintProblem(p) if p.contains("MintAuthority"))), "{:?}", r.bootstrap);
    // Only the Burn lane's missing pool account was created; the existing wrong accounts were left alone.
    let sent_targets: Vec<_> = r.bootstrap.iter().filter_map(|e| if let BootstrapEvent::Sent { target, .. } = e { Some(*target) } else { None }).collect();
    assert_eq!(sent_targets, vec![BootstrapTarget::PoolSlot { lane: Lane::Burn, index: 0 }]);
    assert_eq!(state::read_core(|c| c.mint_state), MintState::None);
    assert_eq!(state::get_slot(Lane::Mint, 0).unwrap().state, SlotState::Unverified);
    assert!(state::get_bootstrap_tx(BootstrapTarget::Mint.key()).is_none());
}

#[test]
fn a_lost_sign_reply_re_signs_the_same_creation_bytes_and_a_stale_transaction_is_replaced() {
    let fx = Fixture::unbootstrapped(1);
    fx.chain.auto_apply_creation.set(false);
    // The first sign reply is lost (the pool account's, the first signature of the pass).
    fx.signer.script(Scripted::Unknown("SYS_UNKNOWN".into()));
    run(&fx, T0);
    let key = BootstrapTarget::PoolSlot { lane: Lane::Mint, index: 0 }.key();
    let first = state::get_bootstrap_tx(key).unwrap();
    assert_eq!(first.signatures.len(), 0);
    assert_eq!(first.sign_failures, 1);
    assert!(fx.chain.sent().len() < 5, "the unsigned one was not sent");
    // Next pass inside the TTL: the SAME bytes are signed and sent.
    run(&fx, t(40));
    let second = state::get_bootstrap_tx(key).unwrap();
    assert_eq!(second.message, first.message, "the same bytes");
    assert_eq!(second.created_at_ns, first.created_at_ns);
    assert!(second.fully_signed());
    assert!(fx.signer.calls.borrow().iter().filter(|c| c.message == first.message).count() >= 2, "re-signed over identical bytes");
    // The transaction lands nowhere. Inside the TTL it is rebroadcast byte for byte.
    let sent_before = fx.chain.sent().len();
    run(&fx, t(80));
    assert!(fx.chain.sent().len() > sent_before);
    assert_eq!(state::get_bootstrap_tx(key).unwrap().message, first.message);
    // Past the TTL its blockhash is dead: a fresh transaction replaces it.
    fx.chain.blocks.borrow_mut().insert(2_999, [0x6c; 32]);
    fx.chain.tip_slot.set(3_000);
    let r = run(&fx, t(200));
    let replaced = state::get_bootstrap_tx(key).unwrap();
    assert_ne!(replaced.message, first.message);
    assert_eq!(replaced.recent_blockhash, [0x6c; 32]);
    assert!(r.bootstrap.iter().any(|e| matches!(e, BootstrapEvent::Sent { target: BootstrapTarget::PoolSlot { lane: Lane::Mint, index: 0 }, fresh: true })), "{:?}", r.bootstrap);
}

#[test]
fn send_failures_from_an_unfunded_fee_payer_are_recorded_and_retried() {
    let fx = Fixture::unbootstrapped(1);
    fx.chain.auto_apply_creation.set(false);
    fx.chain.fail_always("send_transaction", Fault::Error("Transaction simulation failed: Attempt to debit an account but found no record of a prior credit".into()));
    run(&fx, T0);
    let key = BootstrapTarget::Mint.key();
    let tx = state::get_bootstrap_tx(key).unwrap();
    assert!(tx.last_broadcast_note.as_deref().unwrap().contains("maybe sent"), "{:?}", tx.last_broadcast_note);
    assert_eq!(tx.broadcast_count, 1);
    // Funded later: the same bytes are sent again after the interval and the creation applies.
    fx.chain.clear_faults();
    fx.chain.auto_apply_creation.set(true);
    run(&fx, t(40));
    assert_eq!(state::get_bootstrap_tx(key).unwrap().message, tx.message, "the same bytes");
    assert!(fx.chain.get_mint_for_test().is_some(), "the creation applied once it could be sent");
}

impl MockChain {
    fn get_mint_for_test(&self) -> Option<Vec<u8>> {
        block_on(crate::sol_rpc::SolRpc::get_account(self, &MINT)).ok().flatten().map(|a| a.data)
    }
}

#[test]
fn a_global_halt_stops_bootstrap_signing_and_sending() {
    let fx = Fixture::unbootstrapped(1);
    state::halt("operator halt");
    let r = run(&fx, T0);
    // Public keys are public information, but nothing is signed or sent.
    assert_eq!((fx.signer.call_count(), fx.chain.sent().len()), (0, 0));
    assert!(r.bootstrap.iter().any(|e| matches!(e, BootstrapEvent::Waiting { .. })));
    assert!(stored_txs(1).iter().all(|tx| tx.signatures.is_empty()), "no creation was signed");
}

#[test]
fn a_bootstrapped_instance_runs_the_lane_from_the_same_timer_pass() {
    let fx = Fixture::unbootstrapped(2);
    for n in 0..3 {
        run(&fx, t(40 * n));
    }
    assert_eq!(state::read_core(|c| c.mint_state), MintState::Bound);
    // Admit a mint, let it land, and let the timer pass deliver it.
    let req = SubmitRequest {
        kind: OpKind::Mint,
        payload: OpPayload::Mint { recipient_wallet: RECIPIENT, per_operation_cap: 1_000_000_000 },
        external_id: [0xd1; 32],
        amount: 12_345,
    };
    let id = block_on(crate::lane::submit_op(&fx.chain, &fx.signer, req, t(200))).unwrap();
    assert!(fx.chain.land(fx.chain.sent().last().unwrap(), false));
    let r = run(&fx, t(260));
    assert!(matches!(&r.lane.events[..], [crate::lane::OpEvent::Delivered { op_id, adopted: false, .. }] if *op_id == id), "{:?}", r.lane.events);
}

#[test]
fn keys_from_a_bb48_snapshot_gain_the_mint_account_key_but_never_change() {
    let fx = Fixture::unbootstrapped(1);
    state::mutate_core(|c| {
        c.keys = Some(state::RoleKeys { fee_payer: FEE_PAYER, mint_authority: MINT_AUTHORITY, return_owner: RETURN_OWNER, mint_account: None });
    });
    let keys = block_on(ensure_keys(&fx.signer)).unwrap();
    assert_eq!(keys.mint_account, Some(MINT));
    assert_eq!(keys.fee_payer, FEE_PAYER);
    // Derived keys that disagree with the stored ones are an error, and the stored keys survive.
    state::mutate_core(|c| {
        c.keys = Some(state::RoleKeys { fee_payer: [0x12; 32], mint_authority: MINT_AUTHORITY, return_owner: RETURN_OWNER, mint_account: None });
    });
    assert!(block_on(ensure_keys(&fx.signer)).is_err());
    assert_eq!(state::read_core(|c| c.keys.clone()).unwrap().fee_payer, [0x12; 32]);
}

#[test]
fn the_status_query_shows_what_to_fund_and_where_the_bind_stands() {
    let fx = Fixture::unbootstrapped(2);
    let s = crate::get_bootstrap_status();
    assert!(!s.keys_derived && s.fee_payer.is_none() && s.mint_state == MintState::None);
    assert_eq!((s.pool_registered, s.pending_bootstrap_txs), (0, 0));
    run(&fx, T0);
    let s = crate::get_bootstrap_status();
    assert!(s.keys_derived);
    assert_eq!(s.fee_payer, Some(crate::tx::pubkey_to_base58(&FEE_PAYER)), "the address an operator must fund");
    assert_eq!(s.mint_account, Some(crate::tx::pubkey_to_base58(&MINT)));
    assert_eq!((s.pool_registered, s.pool_unverified, s.pending_bootstrap_txs), (4, 4, 5));
    run(&fx, t(40));
    run(&fx, t(80));
    let s = crate::get_bootstrap_status();
    assert_eq!((s.mint_state, s.pool_free, s.pool_unverified, s.pending_bootstrap_txs), (MintState::Bound, 4, 0, 0));
    assert!(s.return_account.is_some() && s.bind_note.is_none());
}

// ---------------------------------------------------------------------------
// A reply that arrives after its attempt was replaced (Miles' correction to 49a)
//
// `timer::RunGuard` reclaims a pass after ten minutes while an older pass may still be awaiting a
// signer or the RPC, and `ensure_tx` replaces a creation transaction after the TTL. The older pass's
// continuation must then touch nothing of the replacement: a signature belongs to the exact message
// it signed, and a failure or a send note describes the attempt it was about. A time-based test
// proves nothing here; these tests replace the stored attempt while the call is in flight.
// ---------------------------------------------------------------------------

/// The attempt a later pass would store for the mint after the TTL: another blockhash, so other
/// message bytes, no signatures yet.
fn newer_mint_attempt(created_at_ns: u64) -> BootstrapTx {
    let profile = state::read_core(|c| c.creation_profile()).unwrap();
    let unsigned = crate::tx::builders::build_mint_creation_message(&profile, crate::tx::MINT_RENT_LAMPORTS, [0x6c; 32]).unwrap();
    let signers = unsigned.required_signers().to_vec();
    BootstrapTx {
        target: BootstrapTarget::Mint,
        recent_blockhash: [0x6c; 32],
        message: unsigned.message_bytes,
        signers,
        signatures: vec![],
        tx_ids: vec![],
        created_at_ns,
        sign_failures: 0,
        last_sign_error: None,
        broadcast_count: 0,
        last_broadcast_ns: None,
        last_broadcast_note: None,
    }
}

/// A fixture whose first pass has derived the keys and created everything except the mint's
/// transaction: the pool accounts exist, so the mint is the only creation in flight.
fn fixture_with_only_the_mint_pending() -> Fixture {
    let fx = Fixture::unbootstrapped(1);
    block_on(ensure_keys(&fx.signer)).unwrap();
    for lane in Lane::ALL {
        let (_, address) = nonce::slot_address(&FEE_PAYER, lane, 0).unwrap();
        fx.chain.add_nonce_account(address, &FEE_PAYER, &slot_initial_value(lane, 0));
    }
    fx.chain.auto_apply_creation.set(false);
    fx
}

/// The signature the mock signer returns for the stored message of `tx`, per signer.
fn expected_signature(tx: &BootstrapTx, signer: &crate::tx::Pubkey) -> crate::tx::Signature {
    let role = if *signer == FEE_PAYER { crate::ids::KeyRole::FeePayer } else { crate::ids::KeyRole::MintAccount };
    MockSigner::signature_for(role, &tx.message, 0)
}

#[test]
fn a_delayed_sign_reply_never_joins_the_message_that_replaced_its_attempt() {
    let fx = fixture_with_only_the_mint_pending();
    // Pass 1 stores the mint's attempt and starts signing it; while the FIRST signature is being
    // produced, a newer pass (after the TTL) replaces the attempt with another message.
    let replaced = Rc::new(Cell::new(false));
    let flag = replaced.clone();
    fx.signer.on_call.replace(Some(Box::new(move |call| {
        let Some(old) = state::get_bootstrap_tx(BootstrapTarget::Mint.key()) else { return };
        if !flag.get() && call.message == old.message && old.message != newer_mint_attempt(0).message {
            flag.set(true);
            state::put_bootstrap_tx(&newer_mint_attempt(old.created_at_ns + secs(BOOTSTRAP_TX_RETRY_SECS + 1)));
        }
    })));
    let r = run(&fx, T0);
    assert!(replaced.get(), "the replacement happened during the sign call");
    let tx = state::get_bootstrap_tx(BootstrapTarget::Mint.key()).unwrap();
    assert_eq!(tx.message, newer_mint_attempt(0).message, "the stored attempt is the newer one");
    for record in &tx.signatures {
        assert_eq!(
            record.signature.0,
            expected_signature(&tx, &record.signer),
            "a signature of the OLD message was attached to the NEW message (signer {:?})",
            record.signer
        );
    }
    // The old pass stopped at once: it neither signed nor sent anything for the replacement.
    assert!(tx.signatures.is_empty() && tx.tx_ids.is_empty(), "{tx:?}");
    assert!(
        r.bootstrap.iter().any(|e| matches!(e, BootstrapEvent::Waiting { why } if why.contains("Mint") && why.contains("signatures"))),
        "{:?}",
        r.bootstrap
    );
    // What the late reply said is kept as evidence, with the signature itself, and touches no record.
    let old_message = state::audit_page(0, 100);
    assert!(old_message.iter().any(|a| a.note.contains("signature ") && a.note.contains("replaced meanwhile; discarded")), "{old_message:?}");
    // The newer attempt is signed and sent by the next pass with ITS bytes only.
    let r = run(&fx, T0 + secs(BOOTSTRAP_TX_RETRY_SECS + 2));
    assert!(r.bootstrap.iter().any(|e| matches!(e, BootstrapEvent::Sent { target: BootstrapTarget::Mint, .. })), "{:?}", r.bootstrap);
    let tx = state::get_bootstrap_tx(BootstrapTarget::Mint.key()).unwrap();
    assert_eq!(tx.signatures.len(), 2);
    for record in &tx.signatures {
        assert_eq!(record.signature.0, expected_signature(&tx, &record.signer));
    }
}

#[test]
fn a_delayed_sign_failure_does_not_mark_the_replacement_attempt() {
    let fx = fixture_with_only_the_mint_pending();
    let old_message: Rc<RefCell<Option<Vec<u8>>>> = Rc::default();
    let seen = old_message.clone();
    let failing = old_message.clone();
    // The sign call for the mint's first message fails, and while it is in flight the attempt is
    // replaced.
    let replaced = Rc::new(Cell::new(false));
    let flag = replaced.clone();
    fx.signer.override_call.replace(Some(Box::new(move |call| {
        (failing.borrow().as_ref() == Some(&call.message)).then(|| Scripted::Fail("signer unavailable".into()))
    })));
    fx.signer.on_call.replace(Some(Box::new(move |call| {
        let Some(old) = state::get_bootstrap_tx(BootstrapTarget::Mint.key()) else { return };
        if !flag.get() && call.message == old.message && old.message != newer_mint_attempt(0).message {
            flag.set(true);
            *seen.borrow_mut() = Some(old.message.clone());
            state::put_bootstrap_tx(&newer_mint_attempt(old.created_at_ns + secs(BOOTSTRAP_TX_RETRY_SECS + 1)));
        }
    })));
    run(&fx, T0);
    assert!(replaced.get());
    let tx = state::get_bootstrap_tx(BootstrapTarget::Mint.key()).unwrap();
    assert_ne!(Some(tx.message.clone()), old_message.borrow().clone());
    assert_eq!(tx.sign_failures, 0, "the old attempt's failure was charged to the new one");
    assert_eq!(tx.last_sign_error, None);
}

#[test]
fn a_delayed_send_note_does_not_overwrite_the_replacement_attempt() {
    let fx = fixture_with_only_the_mint_pending();
    // The mint's transaction is signed and sent; while the send is in flight the attempt is replaced.
    let replaced = Rc::new(Cell::new(false));
    let flag = replaced.clone();
    fx.chain.on_send.replace(Some(Box::new(move |wire| {
        let Ok((_, message)) = crate::tx::split_wire_transaction(wire) else { return };
        let Some(old) = state::get_bootstrap_tx(BootstrapTarget::Mint.key()) else { return };
        if !flag.get() && message == old.message && old.message != newer_mint_attempt(0).message {
            flag.set(true);
            state::put_bootstrap_tx(&newer_mint_attempt(old.created_at_ns + secs(BOOTSTRAP_TX_RETRY_SECS + 1)));
        }
    })));
    run(&fx, T0);
    assert!(replaced.get(), "the replacement happened during the send");
    let tx = state::get_bootstrap_tx(BootstrapTarget::Mint.key()).unwrap();
    assert_eq!(tx.message, newer_mint_attempt(0).message);
    assert_eq!(tx.last_broadcast_note, None, "the old send's note was written onto the new attempt");
    assert_eq!((tx.broadcast_count, tx.last_broadcast_ns), (0, None));
    assert!(tx.signatures.is_empty() && tx.tx_ids.is_empty(), "nothing of the old attempt joined the new one");
    assert!(
        state::audit_page(0, 100).iter().any(|a| a.note.contains("send result") && a.note.contains("replaced meanwhile; discarded")),
        "the old send's result is kept as evidence"
    );
}
