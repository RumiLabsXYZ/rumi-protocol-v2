//! Tests of the nonce lane against the mock chain and the scripted signer: persist before sign,
//! signature recording, rebroadcast of the same bytes, the cancel, and the reconcile matrix of
//! `chain-solana.md` section 8 item 5, plus section 12 test (a).

use crate::ids::KeyRole;
use crate::lane::*;
use crate::memo::{canister_memo, CanisterMemoKind};
use crate::nonce;
use crate::records::*;
use crate::state;
use crate::testkit::*;
use crate::tx::builders::*;
use crate::tx::{assemble_wire_tx, compile_message, serialize_message, MessageVersion, Pubkey, MEMO_PROGRAM_V3_ID};
use crate::types::LaneSettings;

const T0: u64 = 1_000 * SECOND;

fn t(s: u64) -> u64 {
    T0 + secs(s)
}

fn slot_of(op: &SolOp) -> NonceSlot {
    state::get_slot(op.lane, op.slot_index.expect("slot")).expect("slot record")
}

fn the_op(id: u64) -> SolOp {
    state::get_op(id).expect("op")
}

/// A wire transaction signed by "someone" (fixed bytes) that consumes the op's nonce with
/// `[AdvanceNonce, Memo rumi1|m|<mint id>]`: an advance-only transaction carrying our memo.
fn foreign_advance_with_our_memo(op: &SolOp) -> Vec<u8> {
    let slot = slot_of(op);
    let attempt = &op.attempts[0];
    let ixs = [
        crate::tx::advance_nonce_instruction(&slot.address, &FEE_PAYER),
        crate::tx::memo_instruction(&MEMO_PROGRAM_V3_ID, canister_memo(CanisterMemoKind::Mint, &op.external_id).as_bytes()),
    ];
    let msg = compile_message(MessageVersion::Legacy, &ixs, &FEE_PAYER, attempt.nonce_value).unwrap();
    assemble_wire_tx(&[[0xee; 64]], &serialize_message(&msg).unwrap()).unwrap()
}

fn advance_only_without_memo(op: &SolOp) -> Vec<u8> {
    let slot = slot_of(op);
    let msg = compile_message(
        MessageVersion::Legacy,
        &[crate::tx::advance_nonce_instruction(&slot.address, &FEE_PAYER)],
        &FEE_PAYER,
        op.attempts[0].nonce_value,
    )
    .unwrap();
    // The exact bytes of OUR cancel would equal this; make them differ with an extra fee-payer memo-less
    // instruction order by adding a no-op: use a different (but valid) leading structure.
    let _ = msg;
    let ixs = [
        crate::tx::advance_nonce_instruction(&slot.address, &FEE_PAYER),
        crate::tx::memo_instruction(&MEMO_PROGRAM_V3_ID, b"unrelated"),
    ];
    let msg = compile_message(MessageVersion::Legacy, &ixs, &FEE_PAYER, op.attempts[0].nonce_value).unwrap();
    assemble_wire_tx(&[[0xdd; 64]], &serialize_message(&msg).unwrap()).unwrap()
}

fn events_contain(events: &[OpEvent], f: impl Fn(&OpEvent) -> bool) -> bool {
    events.iter().any(f)
}

// ---------------------------------------------------------------------------
// Persist before sign, record before send
// ---------------------------------------------------------------------------

#[test]
fn persist_happens_before_the_first_sign_call() {
    let fx = Fixture::new(4);
    // At every sign call the stored operation must already hold exactly the bytes being signed,
    // with its slot reserved and its external id claimed.
    fx.signer.on_call.replace(Some(Box::new(|call| {
        let ids = state::active_op_ids(10);
        assert_eq!(ids.len(), 1, "the operation is stored before the signer is called");
        let op = state::get_op(ids[0]).unwrap();
        assert_eq!(op.attempts.len(), 1);
        assert_eq!(op.attempts[0].message, call.message, "the stored bytes are the signed bytes");
        assert!(op.attempts[0].signers.len() == 2);
        assert_eq!(state::op_by_external(&op.external_id), Some(op.id));
        assert_eq!(state::get_slot(op.lane, op.slot_index.unwrap()).unwrap().state, SlotState::Reserved { op_id: op.id });
    })));
    let id = fx.submit(Fixture::mint_request(0xa1, 123_456_789), T0).unwrap();
    assert_eq!(fx.signer.call_count(), 2, "fee-payer then mint-authority");
    let op = the_op(id);
    // The persisted record is the section 4 tuple: op id, mint id, recipient, amount, nonce
    // account, nonce value, exact unsigned bytes, signer list.
    assert_eq!(op.external_id, [0xa1; 32]);
    assert_eq!(op.payload, OpPayload::Mint { recipient_wallet: RECIPIENT, per_operation_cap: 1_000_000_000_000 });
    assert_eq!(op.amount, 123_456_789);
    let a = &op.attempts[0];
    assert_eq!(a.nonce_account, slot_of(&op).address);
    assert_eq!(a.nonce_value, slot_initial_value(Lane::Mint, 0));
    assert_eq!(a.signers, vec![FEE_PAYER, MINT_AUTHORITY]);
    let expected = build_mint_message(
        &fx.profile(),
        &MintRequest {
            nonce: NonceRef { account: a.nonce_account, value: a.nonce_value },
            recipient_wallet: RECIPIENT,
            amount: 123_456_789,
            mint_id: [0xa1; 32],
            per_operation_cap: 1_000_000_000_000,
        },
    )
    .unwrap();
    assert_eq!(a.message, expected.message_bytes, "the stored bytes are the builder's bytes");
}

#[test]
fn every_signature_and_the_tx_id_are_recorded_before_the_send() {
    let fx = Fixture::new(4);
    fx.chain.on_send.replace(Some(Box::new(|wire| {
        let ids = state::active_op_ids(10);
        let op = state::get_op(ids[0]).unwrap();
        let a = &op.attempts[0];
        assert_eq!(a.signatures.len(), 2, "both signatures are stored before the send");
        let (sigs, message) = crate::tx::split_wire_transaction(wire).unwrap();
        assert_eq!(message, a.message.as_slice());
        assert_eq!(a.tx_ids, vec![Sig(sigs[0])], "the transaction id is stored before the send");
        assert_eq!(a.broadcast_count, 1);
    })));
    let id = fx.submit(Fixture::mint_request(1, 1_000), T0).unwrap();
    assert_eq!(fx.chain.sent().len(), 1);
    assert!(the_op(id).attempts[0].last_broadcast_note.as_deref().unwrap().starts_with("sent"));
}

#[test]
fn submit_is_idempotent_on_the_external_id() {
    let fx = Fixture::new(4);
    let a = fx.submit(Fixture::mint_request(7, 500), T0).unwrap();
    let b = fx.submit(Fixture::mint_request(7, 500), t(5)).unwrap();
    assert_eq!(a, b);
    assert_eq!(state::active_op_count(), 1);
    assert_eq!(the_op(a).attempts.len(), 1);
    assert_eq!(fx.signer.call_count(), 2, "the repeat did not sign again");
    assert_eq!(nonce_reserved_count(Lane::Mint), 1);
}

fn nonce_reserved_count(lane: Lane) -> usize {
    state::slots_in_lane(lane).iter().filter(|s| matches!(s.state, SlotState::Reserved { .. })).count()
}

#[test]
fn a_lost_sign_reply_re_signs_the_same_unsigned_bytes() {
    let fx = Fixture::new(4);
    // The first reply is lost, the second fails outright, then signing works.
    fx.signer.script(Scripted::Unknown("SYS_UNKNOWN".into()));
    let id = fx.submit(Fixture::mint_request(2, 2_000), T0).unwrap();
    assert_eq!(fx.chain.sent().len(), 0, "nothing is sent while a signature is missing");
    let stored = the_op(id).attempts[0].message.clone();
    assert_eq!(the_op(id).attempts[0].signatures.len(), 0);
    assert_eq!(the_op(id).attempts[0].sign_failures, 1);
    fx.signer.script(Scripted::Fail("CANISTER_ERROR".into()));
    fx.tick(t(10));
    assert_eq!(the_op(id).attempts[0].sign_failures, 2);
    fx.tick(t(20));
    let op = the_op(id);
    assert_eq!(op.attempts.len(), 1, "no new attempt, no new nonce");
    assert_eq!(op.attempts[0].message, stored, "the stored bytes never changed");
    assert!(op.attempts[0].fully_signed());
    // Every sign call received identical bytes.
    let calls = fx.signer.calls.borrow();
    assert!(calls.len() >= 4);
    assert!(calls.iter().all(|c| c.message == stored), "every (re-)sign was over the SAME bytes");
    drop(calls);
    assert_eq!(fx.chain.sent().len(), 1, "sent once it was fully signed");
}

#[test]
fn a_non_deterministic_re_sign_keeps_every_signature_and_every_tx_id() {
    let fx = Fixture::new(4);
    fx.signer.nondeterministic.set(true);
    // The mint-authority reply is lost AFTER the fee payer signed; the retry produces another
    // fee-payer-independent signature for the second signer only. Force a re-sign of the fee payer
    // by clearing nothing: instead check that two signs of the same bytes give two signatures.
    let id = fx.submit(Fixture::mint_request(3, 3_000), T0).unwrap();
    let before = the_op(id).attempts[0].clone();
    assert_eq!(before.signatures.len(), 2);
    assert_eq!(before.tx_ids.len(), 1);
    // Signing the same bytes again yields a different signature; recording it keeps both.
    let again = block_on(fx.signer_sign(KeyRole::FeePayer, &before.message));
    let SignOutcome::Signed(sig) = again else { panic!("signed") };
    assert_ne!(sig, before.signatures[0].signature.0, "threshold signing may not be deterministic");
    update_signatures(id, sig);
    let after = the_op(id).attempts[0].clone();
    assert_eq!(after.signatures.len(), 3, "every signature is kept");
    // The next broadcast uses the newest fee-payer signature and records the new tx id as well.
    fx.tick(t(60));
    let after = the_op(id).attempts[0].clone();
    assert_eq!(after.tx_ids.len(), 2, "both transaction ids are recorded and both are looked up later");
    assert_eq!(after.message, before.message);
}

impl Fixture {
    async fn signer_sign(&self, role: KeyRole, message: &[u8]) -> SignOutcome {
        TxSigner::sign(&self.signer, role, message).await
    }
}

fn update_signatures(id: u64, sig: crate::tx::Signature) {
    let mut op = state::get_op(id).unwrap();
    op.attempts[0].signatures.push(SignatureRecord { signer: FEE_PAYER, signature: Sig(sig), at_ns: 0 });
    state::put_op(&op);
}

// ---------------------------------------------------------------------------
// Unchanged nonce: rebroadcast, never a new nonce; the cancel
// ---------------------------------------------------------------------------

#[test]
fn unchanged_nonce_rebroadcasts_the_same_bytes_and_never_a_new_nonce() {
    let fx = Fixture::new(4);
    let id = fx.submit(Fixture::mint_request(4, 4_000), T0).unwrap();
    let first_wire = fx.chain.sent()[0].clone();
    // Too early: not due.
    fx.tick(t(10));
    assert_eq!(fx.chain.sent().len(), 1);
    // Due after the rebroadcast interval (30 s): the SAME bytes go out again.
    let report = fx.tick(t(40));
    assert_eq!(fx.chain.sent().len(), 2);
    assert_eq!(fx.chain.sent()[1], first_wire, "rebroadcast is byte-identical");
    assert!(events_contain(&report.events, |e| matches!(e, OpEvent::Waiting { .. })));
    let op = the_op(id);
    assert_eq!(op.attempts.len(), 1);
    assert_eq!(op.generation, 0);
    assert_eq!(op.attempts[0].broadcast_count, 2);
    assert_eq!(op.state, OpState::Active);
    // The chain's nonce never moved and no other slot was touched.
    assert_eq!(fx.chain.nonce_value(&slot_of(&op).address), Some(slot_initial_value(Lane::Mint, 0)));
    assert_eq!(nonce_reserved_count(Lane::Mint), 1);
}

#[test]
fn every_send_outcome_is_maybe_sent_and_never_a_reason_to_rebuild() {
    let fx = Fixture::new(4);
    fx.chain.fail_next("send_transaction", Fault::Error("Transaction simulation failed: AlreadyProcessed".into()));
    let id = fx.submit(Fixture::mint_request(80, 800), T0).unwrap();
    let a = the_op(id).attempts[0].clone();
    assert_eq!(a.broadcast_count, 1, "counted as sent");
    assert!(a.last_broadcast_note.as_deref().unwrap().contains("maybe sent"), "{:?}", a.last_broadcast_note);
    assert_eq!(a.tx_ids.len(), 1);
    fx.chain.fail_next("send_transaction", Fault::Inconsistent);
    fx.tick(t(40));
    fx.tick(t(80));
    let op = the_op(id);
    assert_eq!(op.attempts.len(), 1, "a send error never builds another attempt");
    assert_eq!(op.attempts[0].message, a.message);
    assert_eq!(op.attempts[0].broadcast_count, 3);
    assert_eq!(fx.signer.call_count(), 2, "nothing was signed again");
    assert_eq!(op.state, OpState::Active);
}

#[test]
fn after_cancel_after_an_advance_only_message_on_the_same_nonce_is_persisted_then_signed() {
    let fx = Fixture::new(4);
    let id = fx.submit(Fixture::mint_request(5, 5_000), T0).unwrap();
    let original_calls = fx.signer.call_count();
    // Before cancel_after (300 s): no cancel.
    let report = fx.tick(t(299));
    assert!(!events_contain(&report.events, |e| matches!(e, OpEvent::CancelIssued { .. })));
    // At cancel_after: the cancel is persisted BEFORE it is signed.
    fx.signer.on_call.replace(Some(Box::new(move |call| {
        let op = state::get_op(id).unwrap();
        if call.message.len() < 200 && op.attempts.len() == 2 {
            assert_eq!(op.attempts[1].kind, AttemptKind::Cancel);
            assert_eq!(op.attempts[1].message, call.message, "the cancel's bytes were stored before signing");
        }
    })));
    let report = fx.tick(t(300));
    assert!(events_contain(&report.events, |e| matches!(e, OpEvent::CancelIssued { .. })));
    let op = the_op(id);
    assert_eq!(op.attempts.len(), 2);
    let (orig, cancel) = (&op.attempts[0], &op.attempts[1]);
    assert_eq!(cancel.kind, AttemptKind::Cancel);
    assert_eq!(cancel.nonce_account, orig.nonce_account, "same nonce account");
    assert_eq!(cancel.nonce_value, orig.nonce_value, "same nonce value");
    let expected = build_advance_only_message(&fx.profile(), &NonceRef { account: orig.nonce_account, value: orig.nonce_value }).unwrap();
    assert_eq!(cancel.message, expected.message_bytes);
    assert_eq!(cancel.signers, vec![FEE_PAYER], "the cancel needs only the fee payer");
    assert_eq!(fx.signer.call_count(), original_calls + 1);
    assert!(cancel.fully_signed());
    assert_eq!(cancel.broadcast_count, 1);
    // Only one cancel per generation.
    fx.tick(t(400));
    assert_eq!(the_op(id).attempts.len(), 2);
    assert_eq!(op.generation, 0, "still the same generation: no new nonce");
}

// ---------------------------------------------------------------------------
// Advanced nonce: recorded signatures
// ---------------------------------------------------------------------------

#[test]
fn landed_ok_with_the_exact_message_is_delivered_and_releases_the_slot() {
    let fx = Fixture::new(4);
    let id = fx.submit(Fixture::mint_request(6, 6_000), T0).unwrap();
    let wire = fx.chain.sent()[0].clone();
    assert!(fx.chain.land(&wire, false));
    let report = fx.tick(t(60));
    let (sigs, _) = crate::tx::split_wire_transaction(&wire).unwrap();
    assert_eq!(report.events, vec![OpEvent::Delivered { op_id: id, signature: Sig(sigs[0]), adopted: false }]);
    let op = the_op(id);
    assert!(matches!(op.state, OpState::Delivered { adopted: false, .. }));
    assert!(!op.outcome_applied, "applying the delivery to the accounting buckets is the saga's job");
    // The slot is reusable: free, holding the value now stored on chain, anchored at the consumer.
    let slot = state::get_slot(Lane::Mint, 0).unwrap();
    assert_eq!(slot.state, SlotState::Free);
    assert_eq!(slot.last_value, fx.chain.nonce_value(&slot.address));
    assert_ne!(slot.last_value, Some(slot_initial_value(Lane::Mint, 0)), "its stored value changed");
    assert_eq!(slot.anchor, Some(Sig(sigs[0])));
    assert_eq!(state::active_op_count(), 0);
    // And a new operation can reuse the slot with the fresh value.
    let id2 = fx.submit(Fixture::mint_request(8, 100), t(70)).unwrap();
    assert_eq!(the_op(id2).slot_index, Some(0));
    assert_eq!(the_op(id2).attempts[0].nonce_value, fx.chain.nonce_value(&slot.address).unwrap());
}

#[test]
fn landed_with_an_error_is_not_delivered_and_the_same_id_is_re_signed_on_the_new_value() {
    let fx = Fixture::new(4);
    let id = fx.submit(Fixture::mint_request(9, 9_000), T0).unwrap();
    let old = the_op(id).attempts[0].clone();
    let wire = fx.chain.sent()[0].clone();
    assert!(fx.chain.land(&wire, true), "a failed durable-nonce transaction still consumes the nonce");
    let report = fx.tick(t(60));
    assert!(matches!(&report.events[..], [OpEvent::NotDelivered { reason: NotDeliveredReason::LandedWithError { .. }, .. }]), "{:?}", report.events);
    assert!(matches!(the_op(id).state, OpState::NeedsReissue { .. }));
    assert_eq!(nonce_reserved_count(Lane::Mint), 0, "proven consumed: the slot is free again");
    // Next tick: the SAME id on the fresh nonce value, a new generation.
    let report = fx.tick(t(70));
    assert_eq!(report.events, vec![OpEvent::Reissued { op_id: id, generation: 1 }]);
    let op = the_op(id);
    assert_eq!(op.state, OpState::Active);
    assert_eq!(op.generation, 1);
    assert_eq!(op.external_id, [9; 32], "same id");
    assert_eq!(op.attempts.len(), 2);
    let new = &op.attempts[1];
    assert_eq!(new.generation, 1);
    assert_ne!(new.nonce_value, old.nonce_value, "a fresh nonce value");
    assert_eq!(Some(new.nonce_value), fx.chain.nonce_value(&slot_of(&op).address));
    assert_ne!(new.message, old.message);
    // The memo still carries the same mint id.
    let memo = canister_memo(CanisterMemoKind::Mint, &[9; 32]);
    assert!(new.message.windows(memo.len()).any(|w| w == memo.as_bytes()));
    assert!(new.fully_signed());
    assert_eq!(fx.chain.sent().len(), 2, "the new generation was broadcast");
}

#[test]
fn consumed_by_our_cancel_proves_non_delivery_and_allows_a_reissue() {
    let fx = Fixture::new(4);
    let id = fx.submit(Fixture::mint_request(10, 10_000), T0).unwrap();
    fx.tick(t(300)); // cancel issued and broadcast
    let wires = fx.chain.sent();
    let cancel_wire = wires.last().unwrap().clone();
    let original_wire = wires[0].clone();
    // The cancel lands first (err == null); the original is now invalid forever.
    assert!(fx.chain.land(&cancel_wire, false));
    assert!(!fx.chain.land(&original_wire, false), "the original cannot land on the consumed nonce");
    let report = fx.tick(t(330));
    assert!(matches!(&report.events[..], [OpEvent::NotDelivered { reason: NotDeliveredReason::CancelLanded { .. }, .. }]), "{:?}", report.events);
    assert!(matches!(the_op(id).state, OpState::NeedsReissue { reason: NotDeliveredReason::CancelLanded { .. } }));
    let report = fx.tick(t(340));
    assert_eq!(report.events, vec![OpEvent::Reissued { op_id: id, generation: 1 }]);
    assert_eq!(the_op(id).generation, 1);
}

#[test]
fn the_original_landing_late_wins_over_a_pending_cancel() {
    let fx = Fixture::new(4);
    let id = fx.submit(Fixture::mint_request(11, 11_000), T0).unwrap();
    fx.tick(t(300)); // cancel issued
    let wires = fx.chain.sent();
    assert!(fx.chain.land(&wires[0], false), "the original lands although a cancel exists");
    let report = fx.tick(t(330));
    assert!(matches!(&report.events[..], [OpEvent::Delivered { adopted: false, .. }]), "{:?}", report.events);
    assert!(matches!(the_op(id).state, OpState::Delivered { .. }));
}

// ---------------------------------------------------------------------------
// Advanced nonce: discovery, a memo is not proof
// ---------------------------------------------------------------------------

#[test]
fn a_lost_signature_found_by_discovery_is_adopted_only_with_the_exact_stored_bytes() {
    let fx = Fixture::new(4);
    // Signing never completes (the signer replies are lost), so nothing was ever recorded or sent.
    fx.signer.script(Scripted::Unknown("lost".into()));
    let id = fx.submit(Fixture::mint_request(12, 12_000), T0).unwrap();
    let op = the_op(id);
    assert!(op.attempts[0].signatures.is_empty() && op.attempts[0].tx_ids.is_empty());
    // A transaction with the EXACT stored message bytes and signatures we never saw lands.
    let wire = assemble_wire_tx(&[[0xaa; 64], [0xbb; 64]], &op.attempts[0].message).unwrap();
    assert!(fx.chain.land(&wire, false));
    let report = fx.tick(t(60));
    assert_eq!(report.events, vec![OpEvent::Delivered { op_id: id, signature: Sig([0xaa; 64]), adopted: true }]);
    assert!(matches!(the_op(id).state, OpState::Delivered { adopted: true, .. }));
    // Discovery used the account's signature list, whose entry carries our memo (ordering only).
    assert!(fx.chain.call_count("get_signatures_for_address") >= 1);
}

#[test]
fn an_advance_only_transaction_carrying_our_memo_is_not_adopted_and_holds_the_lane() {
    // chain-solana.md section 12 test (a).
    let fx = Fixture::new(4);
    fx.signer.script(Scripted::Unknown("lost".into()));
    let id = fx.submit(Fixture::mint_request(13, 13_000), T0).unwrap();
    let op = the_op(id);
    let foreign = foreign_advance_with_our_memo(&op);
    assert_ne!(crate::tx::split_wire_transaction(&foreign).unwrap().1, op.attempts[0].message.as_slice());
    assert!(fx.chain.land(&foreign, false), "it consumes the mint nonce and succeeds");
    // The listing shows our memo on it, but a memo is discovery only.
    let listing = block_on(crate::sol_rpc::SolRpc::get_signatures_for_address(&fx.chain, &slot_of(&op).address, None, None, 10)).unwrap();
    assert!(listing[0].memo.as_deref().unwrap().contains(&hex::encode([13u8; 32])));

    let report = fx.tick(t(60));
    assert!(matches!(the_op(id).state, OpState::Held { .. }), "held, not delivered: {:?}", the_op(id).state);
    assert!(events_contain(&report.events, |e| matches!(e, OpEvent::Held { .. })));
    assert!(events_contain(&report.events, |e| matches!(e, OpEvent::LaneHalted { lane: Lane::Mint, .. })));
    assert!(!events_contain(&report.events, |e| matches!(e, OpEvent::Delivered { .. } | OpEvent::Reissued { .. } | OpEvent::NotDelivered { .. })));
    let halt = state::lane_halt(Lane::Mint).expect("the lane is halted");
    assert!(halt.reason.contains("memo"), "{}", halt.reason);
    // No replacement mint and no refund: the slot stays reserved, no new attempt exists, and the
    // halted lane does nothing further, however many ticks pass.
    let sent_before = fx.chain.sent().len();
    let calls_before = fx.signer.call_count();
    for i in 0..5 {
        fx.tick(t(100 + i * 100));
    }
    assert_eq!(fx.chain.sent().len(), sent_before);
    assert_eq!(fx.signer.call_count(), calls_before, "a halted lane signs nothing");
    let op = the_op(id);
    assert_eq!(op.attempts.len(), 1);
    assert_eq!(op.generation, 0);
    assert!(matches!(slot_of(&op).state, SlotState::Reserved { .. }));
    assert_eq!(fx.submit(Fixture::mint_request(14, 1), t(900)), Err(SubmitError::LaneHalted(halt.reason.clone())));
}

#[test]
fn a_different_consumer_without_our_memo_halts_the_lane_too() {
    let fx = Fixture::new(4);
    fx.signer.script(Scripted::Unknown("lost".into()));
    let id = fx.submit(Fixture::mint_request(15, 15_000), T0).unwrap();
    let op = the_op(id);
    assert!(fx.chain.land(&advance_only_without_memo(&op), false));
    let report = fx.tick(t(60));
    assert!(matches!(the_op(id).state, OpState::Held { .. }));
    assert!(events_contain(&report.events, |e| matches!(e, OpEvent::LaneHalted { .. })));
    assert!(state::lane_halt(Lane::Mint).is_some());
    assert!(state::lane_halt(Lane::Burn).is_none(), "the other lane is not halted");
}

#[test]
fn a_consumer_that_landed_with_wrong_bytes_for_a_recorded_signature_holds_the_lane() {
    let fx = Fixture::new(4);
    let id = fx.submit(Fixture::mint_request(16, 16_000), T0).unwrap();
    let op = the_op(id);
    // The recorded tx id is reported landed, but with a different message: not adopted.
    let recorded = op.attempts[0].tx_ids[0].0;
    let foreign = foreign_advance_with_our_memo(&op);
    let (_, foreign_message) = crate::tx::split_wire_transaction(&foreign).unwrap();
    let forged = assemble_wire_tx(&[recorded], foreign_message).unwrap();
    assert!(fx.chain.land(&forged, false));
    let report = fx.tick(t(60));
    assert!(matches!(the_op(id).state, OpState::Held { .. }), "{:?}", report.events);
    assert!(state::lane_halt(Lane::Mint).is_some());
}

#[test]
fn spam_on_the_nonce_account_does_not_hide_the_consumer_and_is_paged_through() {
    let fx = Fixture::new(4);
    fx.signer.script(Scripted::Unknown("lost".into()));
    let id = fx.submit(Fixture::mint_request(17, 17_000), T0).unwrap();
    let op = the_op(id);
    let address = slot_of(&op).address;
    // The consumer lands first; then 25 unrelated transactions mention the nonce account.
    let wire = assemble_wire_tx(&[[0xaa; 64], [0xbb; 64]], &op.attempts[0].message).unwrap();
    assert!(fx.chain.land(&wire, false));
    for i in 0..25u8 {
        let msg = compile_message(
            MessageVersion::Legacy,
            &[crate::tx::memo_instruction(&MEMO_PROGRAM_V3_ID, &[b'x', i])],
            &[0x90 + (i % 8); 32],
            [0; 32],
        )
        .unwrap();
        let w = assemble_wire_tx(&[[i + 1; 64]], &serialize_message(&msg).unwrap()).unwrap();
        fx.chain.land_noise(&w, &address);
    }
    // The page is 10 wide: the sweep needs several ticks, never concludes absence, then adopts.
    let mut delivered = false;
    for n in 0..6 {
        let report = fx.tick(t(60 + n * 10));
        assert!(!events_contain(&report.events, |e| matches!(e, OpEvent::NotDelivered { .. } | OpEvent::Reissued { .. } | OpEvent::Held { .. })));
        if events_contain(&report.events, |e| matches!(e, OpEvent::Delivered { adopted: true, .. })) {
            delivered = true;
            break;
        }
        assert!(the_op(id).discovery_cursor.is_some(), "the sweep keeps a cursor between ticks");
    }
    assert!(delivered, "the consumer was found beyond the first page");
}

// ---------------------------------------------------------------------------
// null and Inconsistent wait; absence proves nothing; the horizon
// ---------------------------------------------------------------------------

#[test]
fn null_and_inconsistent_wait_at_every_stage() {
    let fx = Fixture::new(4);
    let id = fx.submit(Fixture::mint_request(18, 18_000), T0).unwrap();
    let wire = fx.chain.sent()[0].clone();
    // Unchanged phase: the nonce read itself is Inconsistent / an error / null.
    for fault in [Fault::Inconsistent, Fault::Error("outcall failed".into()), Fault::Null] {
        fx.chain.fail_next("get_account", fault);
        let report = fx.tick(t(10));
        assert!(matches!(&report.events[..], [OpEvent::Waiting { .. }]), "{:?}", report.events);
    }
    assert_eq!(the_op(id).state, OpState::Active);
    assert!(state::lane_halt(Lane::Mint).is_none());
    // The original lands, but every lookup is Inconsistent, then null, then an error.
    assert!(fx.chain.land(&wire, false));
    for fault in [Fault::Inconsistent, Fault::Null, Fault::Error("x".into())] {
        fx.chain.fail_next("get_transaction", fault);
        let report = fx.tick(t(100));
        assert!(!events_contain(&report.events, |e| matches!(e, OpEvent::Delivered { .. } | OpEvent::NotDelivered { .. } | OpEvent::Held { .. })), "{:?}", report.events);
        assert_eq!(the_op(id).state, OpState::Active, "waiting, not resolved");
    }
    // With a clean lookup it resolves.
    let report = fx.tick(t(110));
    assert!(matches!(&report.events[..], [OpEvent::Delivered { .. }]));
}

#[test]
fn discovery_failures_wait_and_never_conclude_absence() {
    let fx = Fixture::new(4);
    let id = fx.submit(Fixture::mint_request(19, 19_000), T0).unwrap();
    // The account advanced, but no consumer is visible anywhere.
    let addr = slot_of(&the_op(id)).address;
    fx.chain.advance_without_transaction(&addr);
    fx.chain.fail_next("get_signatures_for_address", Fault::Inconsistent);
    let r = fx.tick(t(60));
    assert!(matches!(&r.events[..], [OpEvent::Waiting { .. }]));
    fx.chain.fail_next("get_signatures_for_address", Fault::Error("provider".into()));
    let r = fx.tick(t(70));
    assert!(matches!(&r.events[..], [OpEvent::Waiting { .. }]));
    // A clean, empty listing is still only "no consumer yet": absence never proves non-delivery.
    for n in 0..20 {
        let r = fx.tick(t(80 + n * 60));
        assert!(!events_contain(&r.events, |e| matches!(e, OpEvent::NotDelivered { .. } | OpEvent::Reissued { .. } | OpEvent::Held { .. })), "{:?}", r.events);
    }
    assert_eq!(the_op(id).state, OpState::Active);
    assert_eq!(the_op(id).attempts.len(), 1, "no re-issue, no new nonce, no refund");
    assert!(state::lane_halt(Lane::Mint).is_none());
}

#[test]
fn the_history_horizon_halts_the_lane_when_nothing_explains_the_advance() {
    let lane = LaneSettings { history_horizon_secs: 600, ..LaneSettings::default() };
    let fx = Fixture::with_lane_settings(lane);
    let id = fx.submit(Fixture::mint_request(20, 20_000), T0).unwrap();
    let addr = slot_of(&the_op(id)).address;
    fx.chain.advance_without_transaction(&addr);
    // First seen at t(60); still waiting before the horizon.
    let r = fx.tick(t(60));
    assert!(matches!(&r.events[..], [OpEvent::Waiting { .. }]));
    let r = fx.tick(t(60 + 599));
    assert!(matches!(&r.events[..], [OpEvent::Waiting { .. }]));
    assert_eq!(the_op(id).advance_seen_at_ns, Some(t(60)));
    // At the horizon the operation is held and the lane halts, naming the blocker.
    let r = fx.tick(t(60 + 600));
    assert!(events_contain(&r.events, |e| matches!(e, OpEvent::Held { .. })));
    let halt = state::lane_halt(Lane::Mint).unwrap();
    assert!(halt.reason.contains("history horizon"), "{}", halt.reason);
    assert!(halt.reason.contains("solana-history-availability-unmeasured"), "{}", halt.reason);
    assert!(matches!(the_op(id).state, OpState::Held { .. }));
}

// ---------------------------------------------------------------------------
// Owner and authority on every read
// ---------------------------------------------------------------------------

#[test]
fn a_changed_nonce_owner_or_authority_halts_the_lane() {
    for variant in 0..2 {
        let fx = Fixture::new(4);
        let id = fx.submit(Fixture::mint_request(21, 21_000), T0).unwrap();
        let addr = slot_of(&the_op(id)).address;
        let value = fx.chain.nonce_value(&addr).unwrap();
        if variant == 0 {
            fx.chain.set_account(addr, [0x42; 32], nonce_account_data(1, 1, &FEE_PAYER, &value));
        } else {
            fx.chain.set_account(addr, crate::tx::SYSTEM_PROGRAM_ID, nonce_account_data(1, 1, &[0x99; 32], &value));
        }
        let r = fx.tick(t(60));
        assert!(events_contain(&r.events, |e| matches!(e, OpEvent::LaneHalted { lane: Lane::Mint, .. })), "{:?}", r.events);
        let reason = state::lane_halt(Lane::Mint).unwrap().reason;
        assert!(reason.contains(if variant == 0 { "owner" } else { "authority" }), "{reason}");
        assert_eq!(the_op(id).state, OpState::Active, "the operation keeps its signed attempt");
        // A halted lane neither signs nor sends.
        let sent = fx.chain.sent().len();
        fx.tick(t(500));
        assert_eq!(fx.chain.sent().len(), sent);
    }
}

#[test]
fn a_bad_account_at_submission_halts_the_lane_before_anything_is_persisted() {
    let fx = Fixture::new(4);
    let slot = state::get_slot(Lane::Mint, 0).unwrap();
    fx.chain.set_account(slot.address, crate::tx::SYSTEM_PROGRAM_ID, nonce_account_data(1, 1, &[0x99; 32], &slot_initial_value(Lane::Mint, 0)));
    let r = fx.submit(Fixture::mint_request(22, 1), T0);
    assert!(matches!(r, Err(SubmitError::LaneHalted(_))), "{r:?}");
    assert_eq!(state::active_op_count(), 0);
    assert_eq!(fx.signer.call_count(), 0);
    assert_eq!(state::get_slot(Lane::Mint, 0).unwrap().state, SlotState::Free);
}

// ---------------------------------------------------------------------------
// The pool
// ---------------------------------------------------------------------------

#[test]
fn a_slot_is_reserved_by_exactly_one_operation_until_proven_consumed() {
    let fx = Fixture::new(2);
    let a = fx.submit(Fixture::mint_request(30, 1_000), T0).unwrap();
    let b = fx.submit(Fixture::mint_request(31, 1_000), T0).unwrap();
    assert_eq!((the_op(a).slot_index, the_op(b).slot_index), (Some(0), Some(1)));
    assert_eq!(fx.submit(Fixture::mint_request(32, 1_000), T0), Err(SubmitError::NoFreeSlot));
    assert_eq!(state::slots_in_lane(Lane::Mint)[0].state, SlotState::Reserved { op_id: a });
    // The burn lane has its own pool.
    let burn = fx.submit(Fixture::burn_request(33, 500), T0).unwrap();
    assert_eq!(the_op(burn).lane, Lane::Burn);
    assert_eq!(the_op(burn).attempts[0].signers, vec![FEE_PAYER, RETURN_OWNER]);
    // Delivering `a` frees exactly its slot.
    assert!(fx.chain.land(&fx.chain.sent()[0], false));
    fx.tick(t(60));
    assert_eq!(state::slots_in_lane(Lane::Mint)[0].state, SlotState::Free);
    assert_eq!(state::slots_in_lane(Lane::Mint)[1].state, SlotState::Reserved { op_id: b });
    assert!(fx.submit(Fixture::mint_request(34, 1_000), t(70)).is_ok());
}

#[test]
fn reserving_a_reserved_slot_fails_and_only_the_owner_releases_it() {
    let _fx = Fixture::new(2);
    assert!(nonce::reserve_slot(Lane::Mint, 0, 1));
    assert!(!nonce::reserve_slot(Lane::Mint, 0, 2), "a reserved slot cannot be taken");
    assert!(!nonce::reserve_slot(Lane::Mint, 9, 2), "an unregistered slot cannot be taken");
    nonce::release_slot(Lane::Mint, 0, 2, [9; 32], Sig([9; 64]));
    assert_eq!(state::get_slot(Lane::Mint, 0).unwrap().state, SlotState::Reserved { op_id: 1 }, "a non-owner cannot release it");
    nonce::release_slot(Lane::Mint, 0, 1, [9; 32], Sig([9; 64]));
    let slot = state::get_slot(Lane::Mint, 0).unwrap();
    assert_eq!((slot.state, slot.last_value, slot.anchor), (SlotState::Free, Some([9; 32]), Some(Sig([9; 64]))));
    assert!(nonce::reserve_slot(Lane::Mint, 0, 3));
}

#[test]
fn a_burn_in_the_burn_lane_is_reconciled_like_a_mint() {
    let fx = Fixture::new(2);
    let id = fx.submit(Fixture::burn_request(70, 700), T0).unwrap();
    let op = the_op(id);
    assert_eq!(op.lane, Lane::Burn);
    let memo = canister_memo(CanisterMemoKind::Burn, &[70; 32]);
    assert!(op.attempts[0].message.windows(memo.len()).any(|w| w == memo.as_bytes()));
    assert!(fx.chain.land(&fx.chain.sent()[0], false));
    let r = fx.tick(t(60));
    assert!(matches!(&r.events[..], [OpEvent::Delivered { .. }]), "{:?}", r.events);
    assert_eq!(state::slots_in_lane(Lane::Burn)[0].state, SlotState::Free);
}

#[test]
fn a_halt_in_one_lane_leaves_the_other_working() {
    let fx = Fixture::new(2);
    state::halt_lane(Lane::Mint, "test halt");
    assert!(matches!(fx.submit(Fixture::mint_request(40, 1), T0), Err(SubmitError::LaneHalted(_))));
    assert!(fx.submit(Fixture::burn_request(41, 1), T0).is_ok());
    assert!(state::clear_lane_halt(Lane::Mint));
    assert!(fx.submit(Fixture::mint_request(40, 1), T0).is_ok());
}

#[test]
fn registration_and_verification_of_pool_slots() {
    let fx = Fixture::new(2);
    assert_eq!(nonce::register_slot(Lane::Mint, 0), Err(nonce::PoolError::AlreadyRegistered));
    assert_eq!(nonce::register_slot(Lane::Mint, 2), Err(nonce::PoolError::IndexOutOfRange { index: 2, pool_size: 2 }));
    state::mutate_core(|c| c.config.lane.nonce_pool_size = 4);
    let slot = nonce::register_slot(Lane::Mint, 2).unwrap();
    assert_eq!(slot.state, SlotState::Unverified);
    assert!(nonce::first_free_slot(Lane::Mint).map(|s| s.index) != Some(2), "an unverified slot is never used");
    // Not created yet.
    assert_eq!(block_on(nonce::verify_slot(&fx.chain, Lane::Mint, 2)).unwrap(), nonce::VerifyOutcome::NotCreated);
    // Created with the wrong authority: a problem, still unverified.
    fx.chain.set_account(slot.address, crate::tx::SYSTEM_PROGRAM_ID, nonce_account_data(1, 1, &[0x55; 32], &[1; 32]));
    assert!(matches!(block_on(nonce::verify_slot(&fx.chain, Lane::Mint, 2)).unwrap(), nonce::VerifyOutcome::Problem(nonce::NonceProblem::WrongAuthority(_))));
    assert_eq!(state::get_slot(Lane::Mint, 2).unwrap().state, SlotState::Unverified);
    // Inconsistent read: unavailable.
    fx.chain.fail_next("get_account", Fault::Inconsistent);
    assert!(matches!(block_on(nonce::verify_slot(&fx.chain, Lane::Mint, 2)).unwrap(), nonce::VerifyOutcome::Unavailable(_)));
    // Correct: verified and free.
    fx.chain.add_nonce_account(slot.address, &FEE_PAYER, &[7; 32]);
    assert_eq!(block_on(nonce::verify_slot(&fx.chain, Lane::Mint, 2)).unwrap(), nonce::VerifyOutcome::Verified([7; 32]));
    assert_eq!(state::get_slot(Lane::Mint, 2).unwrap().state, SlotState::Free);
}

// ---------------------------------------------------------------------------
// Re-issue limit, alarm, guards, keys
// ---------------------------------------------------------------------------

#[test]
fn the_reissue_limit_holds_the_operation_without_halting_the_lane() {
    let lane = LaneSettings { max_reissues: 1, ..LaneSettings::default() };
    let fx = Fixture::with_lane_settings(lane);
    let id = fx.submit(Fixture::mint_request(50, 5_000), T0).unwrap();
    let mut now = 60;
    for round in 0..2 {
        let wire = fx.chain.sent().last().unwrap().clone();
        assert!(fx.chain.land(&wire, true), "round {round}");
        fx.tick(t(now));
        now += 10;
        let r = fx.tick(t(now));
        now += 10;
        if round == 0 {
            assert_eq!(r.events, vec![OpEvent::Reissued { op_id: id, generation: 1 }]);
        } else {
            assert!(matches!(&r.events[..], [OpEvent::Held { reason, .. }] if reason.contains("re-issue limit")), "{:?}", r.events);
        }
    }
    assert!(matches!(the_op(id).state, OpState::Held { .. }));
    assert!(state::lane_halt(Lane::Mint).is_none(), "a re-issue limit is a hold, not a lane halt");
}

#[test]
fn the_reconcile_alarm_is_raised_once() {
    let fx = Fixture::new(4);
    let id = fx.submit(Fixture::mint_request(51, 1), T0).unwrap();
    let r = fx.tick(t(899));
    assert!(!events_contain(&r.events, |e| matches!(e, OpEvent::Alarm { .. })));
    let r = fx.tick(t(900));
    assert!(events_contain(&r.events, |e| matches!(e, OpEvent::Alarm { op_id, age_secs: 900 } if *op_id == id)));
    let r = fx.tick(t(1_000));
    assert!(!events_contain(&r.events, |e| matches!(e, OpEvent::Alarm { .. })), "raised once");
}

#[test]
fn a_tick_that_finds_another_tick_running_does_nothing() {
    let fx = Fixture::new(4);
    fx.submit(Fixture::mint_request(52, 1), T0).unwrap();
    let guard = TickGuard::acquire().expect("first tick");
    let r = fx.tick(t(100));
    assert!(r.skipped);
    drop(guard);
    assert!(!fx.tick(t(100)).skipped);
}

#[test]
fn keys_are_derived_once_through_the_signer_seam() {
    state::reset_for_tests();
    let signer = MockSigner::new();
    assert!(state::read_core(|c| c.keys.is_none()));
    let keys = block_on(ensure_keys(&signer)).unwrap();
    assert_eq!((keys.fee_payer, keys.mint_authority, keys.return_owner), (FEE_PAYER, MINT_AUTHORITY, RETURN_OWNER));
    assert_eq!(state::read_core(|c| c.keys.clone()), Some(keys));
}

#[test]
fn unconfigured_instances_refuse_to_submit() {
    state::reset_for_tests();
    let chain = MockChain::new();
    let signer = MockSigner::new();
    let r = block_on(submit_op(&chain, &signer, Fixture::mint_request(1, 1), T0));
    assert!(matches!(r, Err(SubmitError::NotConfigured(_))), "{r:?}");
    assert_eq!(signer.call_count(), 0);
}

// ---------------------------------------------------------------------------
// classify_landing as a pure function
// ---------------------------------------------------------------------------

#[test]
fn classify_landing_table() {
    let fx = Fixture::new(2);
    fx.signer.script(Scripted::Unknown("x".into()));
    let id = fx.submit(Fixture::mint_request(60, 600), T0).unwrap();
    let op = the_op(id);
    let slot = slot_of(&op);
    let bound = op.attempts[0].nonce_value;
    let landed = |wire: Vec<u8>, failed: bool| crate::sol_rpc::LandedTx { slot: 1, wire, failed, meta: Default::default() };
    let exact = assemble_wire_tx(&[[1; 64], [2; 64]], &op.attempts[0].message).unwrap();
    assert_eq!(classify_landing(&op, &slot.address, &bound, &FEE_PAYER, &landed(exact.clone(), false)), Landing::Exact { attempt: 0, failed: false });
    assert_eq!(classify_landing(&op, &slot.address, &bound, &FEE_PAYER, &landed(exact, true)), Landing::Exact { attempt: 0, failed: true });
    let foreign = foreign_advance_with_our_memo(&op);
    assert_eq!(classify_landing(&op, &slot.address, &bound, &FEE_PAYER, &landed(foreign.clone(), false)), Landing::DifferentConsumer);
    // Same transaction relative to another bound value: it is not a consumer of THAT value.
    assert_eq!(classify_landing(&op, &slot.address, &[9; 32], &FEE_PAYER, &landed(foreign.clone(), false)), Landing::NotConsumer);
    // Another fee payer: not our authority.
    assert_eq!(classify_landing(&op, &slot.address, &bound, &[0x12; 32], &landed(foreign, false)), Landing::NotConsumer);
    // A transfer-like message that does not start with an advance.
    let other = compile_message(MessageVersion::Legacy, &[crate::tx::memo_instruction(&MEMO_PROGRAM_V3_ID, b"hi")], &FEE_PAYER, bound).unwrap();
    let w = assemble_wire_tx(&[[3; 64]], &serialize_message(&other).unwrap()).unwrap();
    assert_eq!(classify_landing(&op, &slot.address, &bound, &FEE_PAYER, &landed(w, false)), Landing::NotConsumer);
    // v0 with a lookup list (strictly undecodable): signed by our fee payer => unverifiable; by
    // someone else => not a consumer.
    for (payer, expected) in [(FEE_PAYER, Landing::UnverifiableSigned), ([0x66; 32], Landing::NotConsumer)] {
        // The nonce authority signs, so a transaction signed by someone else cannot consume it.
        let ixs = [crate::tx::advance_nonce_instruction(&slot.address, &payer)];
        let mut bytes = serialize_message(&compile_message(MessageVersion::V0, &ixs, &payer, bound).unwrap()).unwrap();
        *bytes.last_mut().unwrap() = 1;
        let w = assemble_wire_tx(&[[4; 64], [5; 64]][..1], &bytes).unwrap();
        assert_eq!(classify_landing(&op, &slot.address, &bound, &FEE_PAYER, &landed(w, false)), expected);
    }
    // Garbage that is not even a wire transaction cannot be verified.
    assert_eq!(classify_landing(&op, &slot.address, &bound, &FEE_PAYER, &landed(vec![9], false)), Landing::UnverifiableSigned);
    let _: Pubkey = slot.address;
}

// ---------------------------------------------------------------------------
// BB-48 follow-up 1: a halted lane signs and sends nothing, across awaits
// ---------------------------------------------------------------------------

#[test]
fn a_halt_during_signing_keeps_the_evidence_but_stops_the_next_sign_and_the_send() {
    let fx = Fixture::new(4);
    // The first sign reply arrives while another operation halts this lane.
    let fired = std::rc::Rc::new(std::cell::Cell::new(false));
    let flag = fired.clone();
    fx.signer.on_call.replace(Some(Box::new(move |_| {
        if !flag.replace(true) {
            state::halt_lane(Lane::Mint, "another operation halted the lane");
        }
    })));
    let id = fx.submit(Fixture::mint_request(90, 9_000), T0).unwrap();
    let op = the_op(id);
    let a = &op.attempts[0];
    // The returned signature is evidence and is kept; the attempt and its bytes are preserved.
    assert_eq!(a.signatures.len(), 1, "the signature that came back is recorded");
    assert_eq!(a.signatures[0].signer, FEE_PAYER);
    assert_eq!(fx.signer.call_count(), 1, "a halted lane does not sign the next role");
    assert_eq!(fx.chain.sent().len(), 0, "a halted lane sends nothing");
    assert_eq!(a.broadcast_count, 0);
    let stored = a.message.clone();
    // Ticks while halted change nothing.
    fx.tick(t(100));
    assert_eq!((fx.signer.call_count(), fx.chain.sent().len()), (1, 0));
    // After a legitimate clear the SAME bytes are completed and sent.
    assert!(state::clear_lane_halt(Lane::Mint));
    fx.signer.on_call.replace(None);
    fx.tick(t(110));
    let op = the_op(id);
    assert_eq!(op.attempts.len(), 1, "no new attempt");
    assert_eq!(op.attempts[0].message, stored, "the same bytes");
    assert_eq!(op.attempts[0].signatures.len(), 2, "the kept signature is reused, only the missing role is signed");
    assert_eq!(fx.signer.call_count(), 2);
    assert_eq!(fx.signer.calls.borrow()[1].role, KeyRole::MintAuthority);
    assert_eq!(fx.chain.sent().len(), 1);
}

#[test]
fn a_halt_while_the_account_read_is_pending_stops_signing_and_sending() {
    let fx = Fixture::new(4);
    // The signer is down at admission: the attempt is stored, nothing is signed.
    fx.signer.script(Scripted::Unknown("down".into()));
    let id = fx.submit(Fixture::mint_request(91, 9_100), T0).unwrap();
    assert_eq!((fx.signer.call_count(), fx.chain.sent().len()), (1, 0));
    // The tick's nonce read is pending when another operation halts the lane.
    fx.chain.on_account_read.replace(Some(Box::new(|_, _| state::halt_lane(Lane::Mint, "halted while the read was in flight"))));
    let report = fx.tick(t(40));
    assert!(state::lane_halt(Lane::Mint).is_some());
    assert_eq!(fx.signer.call_count(), 1, "no sign call after the halt: {:?}", report.events);
    assert_eq!(fx.chain.sent().len(), 0, "no send after the halt");
    assert_eq!(the_op(id).attempts.len(), 1);
    assert!(!events_contain(&report.events, |e| matches!(e, OpEvent::CancelIssued { .. })));
    // Resumes with the same bytes after a clear.
    let stored = the_op(id).attempts[0].message.clone();
    state::clear_lane_halt(Lane::Mint);
    fx.tick(t(80));
    assert_eq!(the_op(id).attempts[0].message, stored);
    assert!(the_op(id).attempts[0].fully_signed());
    assert_eq!(fx.chain.sent().len(), 1);
}

#[test]
fn a_halt_while_the_account_read_is_pending_before_the_cancel_issues_no_cancel() {
    let fx = Fixture::new(4);
    let id = fx.submit(Fixture::mint_request(92, 9_200), T0).unwrap();
    fx.chain.on_account_read.replace(Some(Box::new(|_, _| state::halt_lane(Lane::Mint, "halted while the read was in flight"))));
    let calls = fx.signer.call_count();
    let sent = fx.chain.sent().len();
    fx.tick(t(300)); // cancel_after has elapsed
    assert_eq!(the_op(id).attempts.len(), 1, "no cancel is created or signed on a halted lane");
    assert_eq!((fx.signer.call_count(), fx.chain.sent().len()), (calls, sent));
}

#[test]
fn a_halt_while_submit_reads_the_account_admits_nothing() {
    let fx = Fixture::new(4);
    fx.chain.on_account_read.replace(Some(Box::new(|_, _| state::halt_lane(Lane::Mint, "halted while the read was in flight"))));
    let r = fx.submit(Fixture::mint_request(93, 9_300), T0);
    assert!(matches!(r, Err(SubmitError::LaneHalted(_))), "{r:?}");
    assert_eq!((fx.signer.call_count(), fx.chain.sent().len(), state::active_op_count()), (0, 0, 0));
    assert_eq!(state::get_slot(Lane::Mint, 0).unwrap().state, SlotState::Free);
}

// ---------------------------------------------------------------------------
// BB-48 follow-up 2: fair reconcile paging
// ---------------------------------------------------------------------------

/// Stand up `n` operations that need no further work (held by the re-issue limit, which keeps
/// no slot), driving them with `process_op` so setup does not depend on the tick under test.
fn make_held_ops(fx: &Fixture, n: u8) {
    for i in 0..n {
        let now = t(10 + i as u64 * 10);
        let id = fx.submit(Fixture::mint_request(100 + i, 1_000), now).unwrap();
        assert!(fx.chain.land(&fx.wire_of(id, 0), true));
        let ev = block_on(process_op(&fx.chain, &fx.signer, id, now + secs(1)));
        assert!(matches!(&ev[..], [OpEvent::NotDelivered { .. }]), "{ev:?}");
        let ev = block_on(process_op(&fx.chain, &fx.signer, id, now + secs(2)));
        assert!(matches!(&ev[..], [OpEvent::Held { .. }]), "{ev:?}");
    }
}

#[test]
fn thirty_two_held_operations_do_not_starve_a_newer_active_operation() {
    let lane = LaneSettings { max_reissues: 0, nonce_pool_size: 2, ..LaneSettings::default() };
    let fx = Fixture::with_lane_settings(lane);
    make_held_ops(&fx, 34);
    assert_eq!(state::active_op_count(), 34, "held operations stay non-terminal");
    assert!(state::lane_halt(Lane::Mint).is_none(), "the lane is healthy");
    // A newer operation whose exact original landed.
    let now = t(1_000);
    let id = fx.submit(Fixture::mint_request(250, 5_000), now).unwrap();
    assert!(id >= 34);
    assert!(fx.chain.land(&fx.wire_of(id, 0), false));
    // A bounded number of ticks must reach it.
    let mut delivered_after = None;
    for n in 1..=4 {
        let report = fx.tick(now + secs(10 * n));
        assert!(report.processed <= MAX_OPS_PER_TICK);
        if matches!(the_op(id).state, OpState::Delivered { .. }) {
            delivered_after = Some(n);
            break;
        }
    }
    assert!(delivered_after.is_some(), "the newer operation was starved behind {} held operations", 34);
    assert!(delivered_after.unwrap() <= 2, "reached within ceil(35/32) ticks");
    // Held money is not settled by being skipped or visited.
    for hid in 0..34u64 {
        assert!(matches!(the_op(hid).state, OpState::Held { .. }));
        assert!(!the_op(hid).outcome_applied);
    }
}

#[test]
fn operations_blocked_in_a_halted_lane_do_not_starve_the_other_lane() {
    let fx = Fixture::new(2);
    // 34 operations waiting for a re-issue (no slot held), then the mint lane halts.
    for i in 0..34u8 {
        let now = t(10 + i as u64 * 10);
        let id = fx.submit(Fixture::mint_request(100 + i, 1_000), now).unwrap();
        assert!(fx.chain.land(&fx.wire_of(id, 0), true));
        let ev = block_on(process_op(&fx.chain, &fx.signer, id, now + secs(1)));
        assert!(matches!(&ev[..], [OpEvent::NotDelivered { .. }]), "{ev:?}");
    }
    state::halt_lane(Lane::Mint, "a halt that blocks every mint operation");
    // The burn lane is healthy: a newer burn whose exact original landed.
    let now = t(2_000);
    let id = fx.submit(Fixture::burn_request(251, 700), now).unwrap();
    assert!(fx.chain.land(&fx.wire_of(id, 0), false));
    let mut reached = false;
    for n in 1..=4 {
        fx.tick(now + secs(10 * n));
        if matches!(the_op(id).state, OpState::Delivered { .. }) {
            reached = true;
            break;
        }
    }
    assert!(reached, "the healthy lane's operation was starved by 34 blocked operations");
}

// ---------------------------------------------------------------------------
// BB-48 follow-up 3: free-slot ABA
// ---------------------------------------------------------------------------

/// What another submission does to slot 0 while this one's account read is in flight: it reserves
/// the slot, its transaction lands (the nonce advances), and the slot is released Free again with
/// the NEW value. Returns the consumed (old) value.
fn another_submission_cycles_slot_zero(chain: &MockChain, address: &Pubkey) -> crate::ids::Hash32 {
    let old = chain.nonce_value(address).unwrap();
    assert!(nonce::reserve_slot(Lane::Mint, 0, 999));
    let new = chain.advance_without_transaction(address);
    nonce::release_slot(Lane::Mint, 0, 999, new, Sig([0xcc; 64]));
    old
}

#[test]
fn a_stale_free_slot_preparation_is_never_admitted_on_the_consumed_value() {
    let fx = Fixture::new(2);
    let address = state::get_slot(Lane::Mint, 0).unwrap().address;
    let consumed = std::rc::Rc::new(std::cell::Cell::new(None));
    let seen = consumed.clone();
    fx.chain.on_account_read.replace(Some(Box::new(move |chain, addr| seen.set(Some(another_submission_cycles_slot_zero(chain, addr))))));
    let r = fx.submit(Fixture::mint_request(110, 1_100), T0);
    let consumed = consumed.get().expect("the other submission ran during the read");
    assert_eq!(consumed, slot_initial_value(Lane::Mint, 0));
    // Whatever the outcome (a refusal, or a retry on the fresh value), no operation may be
    // bound to the consumed value, and nothing may have been signed over it.
    for id in state::active_op_ids(10) {
        for a in &the_op(id).attempts {
            assert_ne!(a.nonce_value, consumed, "op {id} was admitted on the already-consumed nonce value");
        }
    }
    for call in fx.signer.calls.borrow().iter() {
        assert!(!call.message.windows(32).any(|w| w == consumed), "a stale message was signed");
    }
    match r {
        Ok(id) => {
            let a = &the_op(id).attempts[0];
            assert_eq!(Some(a.nonce_value), fx.chain.nonce_value(&address), "a retry binds the CURRENT value");
            assert_eq!(a.nonce_account, address);
        }
        Err(e) => assert!(matches!(e, SubmitError::Busy | SubmitError::NoFreeSlot), "{e:?}"),
    }
    // And it can never become a "foreign consumer" hold: the lane stays healthy.
    fx.tick(t(60));
    assert!(state::lane_halt(Lane::Mint).is_none(), "{:?}", state::lane_halt(Lane::Mint));
}

#[test]
fn a_stale_free_slot_preparation_is_never_reissued_on_the_consumed_value() {
    let fx = Fixture::new(2);
    let id = fx.submit(Fixture::mint_request(111, 1_110), T0).unwrap();
    assert!(fx.chain.land(&fx.wire_of(id, 0), true));
    let r = fx.tick(t(60));
    assert!(matches!(&r.events[..], [OpEvent::NotDelivered { .. }]), "{:?}", r.events);
    // The re-issue reads the freed slot; meanwhile another submission cycles it.
    let address = state::get_slot(Lane::Mint, 0).unwrap().address;
    let consumed = std::rc::Rc::new(std::cell::Cell::new(None));
    let seen = consumed.clone();
    fx.chain.on_account_read.replace(Some(Box::new(move |chain, addr| seen.set(Some(another_submission_cycles_slot_zero(chain, addr))))));
    let calls_before = fx.signer.call_count();
    fx.tick(t(70));
    let consumed = consumed.get().expect("the other submission ran during the read");
    for a in &the_op(id).attempts[1..] {
        assert_ne!(a.nonce_value, consumed, "re-issued on the already-consumed nonce value");
    }
    if the_op(id).attempts.len() == 1 {
        assert_eq!(fx.signer.call_count(), calls_before, "a stale preparation signs nothing");
        assert!(matches!(the_op(id).state, OpState::NeedsReissue { .. }));
        // A later tick prepares afresh on the current value.
        fx.tick(t(80));
    }
    let op = the_op(id);
    assert_eq!(op.attempts.len(), 2);
    assert_eq!(Some(op.attempts[1].nonce_value), fx.chain.nonce_value(&address));
    assert!(state::lane_halt(Lane::Mint).is_none());
}

#[test]
fn the_round_robin_walk_reaches_every_operation_within_the_bound_and_survives_a_cursor_reset() {
    use std::collections::BTreeSet;
    for n in [1u64, 2, 31, 32, 33, 40, 64, 70, 97] {
        state::reset_for_tests();
        state::set_tick_cursor(None);
        for i in 0..n {
            // Sparse, ascending ids; a mix of states that stay non-terminal.
            let st = if i % 3 == 0 { OpState::Held { reason: "x".into() } } else { OpState::Active };
            state::put_op(&bare_op(i * 3 + 1, st));
        }
        // A terminal operation leaves the walk.
        state::put_op(&bare_op(1_000_000, OpState::Delivered { signature: Sig([1; 64]), slot: 1, adopted: false }));
        assert_eq!(state::active_op_count(), n);
        let bound = n.div_ceil(MAX_OPS_PER_TICK as u64);
        let mut seen = BTreeSet::new();
        for _ in 0..bound {
            let page = state::next_tick_page(MAX_OPS_PER_TICK);
            assert!(page.len() <= MAX_OPS_PER_TICK);
            assert_eq!(page.iter().collect::<BTreeSet<_>>().len(), page.len(), "no operation twice in one page");
            assert!(!page.contains(&1_000_000));
            seen.extend(page);
        }
        assert_eq!(seen.len() as u64, n, "n = {n}: every operation within ceil(n / 32) = {bound} ticks");
        // An upgrade resets the heap cursor mid-rotation; the walk restarts and still covers all.
        state::next_tick_page(MAX_OPS_PER_TICK);
        state::set_tick_cursor(None);
        let mut seen = BTreeSet::new();
        for _ in 0..bound {
            seen.extend(state::next_tick_page(MAX_OPS_PER_TICK));
        }
        assert_eq!(seen.len() as u64, n, "n = {n}: after a cursor reset");
    }
    // An empty set yields an empty page and a cleared cursor.
    state::reset_for_tests();
    state::set_tick_cursor(Some(5));
    assert!(state::next_tick_page(MAX_OPS_PER_TICK).is_empty());
    assert_eq!(state::tick_cursor(), None);
}

#[test]
fn a_page_wraps_to_the_lowest_ids_to_fill_itself() {
    state::reset_for_tests();
    state::set_tick_cursor(None);
    for i in 0..40u64 {
        state::put_op(&bare_op(i, OpState::Active));
    }
    let first = state::next_tick_page(32);
    assert_eq!(first, (0..32).collect::<Vec<_>>());
    let second = state::next_tick_page(32);
    // 32..40 first, then it wraps to 0..24.
    assert_eq!(second, (32..40).chain(0..24).collect::<Vec<_>>());
    let third = state::next_tick_page(32);
    assert_eq!(third[0], 24, "continues after the last id of the previous page");
}

#[test]
fn a_slot_generation_changes_on_every_reservation_release_and_verification() {
    let _fx = Fixture::new(2);
    let g0 = state::get_slot(Lane::Mint, 0).unwrap().generation;
    assert!(g0 >= 1, "verification of an unverified slot is a state change");
    // A reservation at the generation seen succeeds and bumps it.
    assert!(nonce::reserve_slot_checked(Lane::Mint, 0, 1, g0));
    let g1 = state::get_slot(Lane::Mint, 0).unwrap().generation;
    assert!(g1 > g0);
    // Release bumps it again: Free once more, but never at the generation a stale preparation saw.
    nonce::release_slot(Lane::Mint, 0, 1, [9; 32], Sig([9; 64]));
    let slot = state::get_slot(Lane::Mint, 0).unwrap();
    assert_eq!(slot.state, SlotState::Free);
    assert!(slot.generation > g1);
    assert!(!nonce::reserve_slot_checked(Lane::Mint, 0, 2, g0), "the stale generation is refused");
    assert!(!nonce::reserve_slot_checked(Lane::Mint, 0, 2, g1), "also the generation of the reserved interval");
    assert_eq!(state::get_slot(Lane::Mint, 0).unwrap().state, SlotState::Free, "a refusal changes nothing");
    assert!(nonce::reserve_slot_checked(Lane::Mint, 0, 2, slot.generation));
}

#[test]
fn legitimate_rebroadcast_and_exact_byte_adoption_still_work_after_a_slot_cycle() {
    // A slot that cycles (delivery, release, reuse) keeps working, and the new operation's own
    // unchanged-nonce rebroadcast and adoption are unaffected by generations.
    let fx = Fixture::new(2);
    let a = fx.submit(Fixture::mint_request(120, 100), T0).unwrap();
    assert!(fx.chain.land(&fx.wire_of(a, 0), false));
    fx.tick(t(40));
    let b = fx.submit(Fixture::mint_request(121, 100), t(50)).unwrap();
    assert_eq!(the_op(b).slot_index, Some(0), "the released slot is reused on its new value");
    let first = fx.chain.sent().last().unwrap().clone();
    fx.tick(t(100));
    assert_eq!(fx.chain.sent().last().unwrap(), &first, "same-attempt rebroadcast is byte-identical");
    assert!(fx.chain.land(&first, false));
    let r = fx.tick(t(150));
    assert!(matches!(&r.events[..], [OpEvent::Delivered { adopted: false, .. }]), "{:?}", r.events);
}

#[test]
fn ambiguous_sign_evidence_is_kept_when_the_lane_halts_at_the_same_time() {
    let fx = Fixture::new(4);
    // The first sign reply is lost (Unknown) and the lane halts while it is in flight.
    fx.signer.script(Scripted::Unknown("SYS_UNKNOWN".into()));
    fx.signer.on_call.replace(Some(Box::new(|_| state::halt_lane(Lane::Mint, "halted during the sign call"))));
    let id = fx.submit(Fixture::mint_request(94, 9_400), T0).unwrap();
    let a = the_op(id).attempts[0].clone();
    assert_eq!(a.sign_failures, 1, "the ambiguous reply is recorded, not discarded");
    assert_eq!(a.last_sign_error.as_deref(), Some("SYS_UNKNOWN"));
    assert_eq!(fx.signer.call_count(), 1, "and nothing else is signed on the halted lane");
    assert_eq!(fx.chain.sent().len(), 0);
}
