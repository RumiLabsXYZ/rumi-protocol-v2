//! Tests of 49d: the burn lane (`RC -> BU -> PR`), the binding of a release to its return and its
//! burn, custody, the holder-burn seam and the stray refunds with their per-transfer ids.

use crate::accounting::Transition;
use crate::env::{self, test_env};
use crate::flows::burn_saga::{self, Effect};
use crate::flows::release::{self, BurnedReturn, OpenError};
use crate::flows::{deposit, ledger_ops};
use crate::ids::{self, TransferLocator};
use crate::monitor;
use crate::observer::{ObserverEvent, OBSERVER_IDLE_INTERVAL_NS};
use crate::records::{OpKind, OpPayload, OpState, Sig};
use crate::saga49c_tests::{
    ata, credited, good_meta, land_valid_return, memo, observe, observer_world, profile, return_wire, run_monitors, set_supply, strays, HOLDER, SRC,
};
use crate::saga_tests::{self, assert_covered, buckets, burned_with, stored_burn, E8, SENDER};
use crate::sol_rpc::{LandedTx, TokenBalanceEntry, TxMeta};
use crate::state::{self, ClaimState, CustodyStatus, ReleaseState, ReturnState};
use crate::testkit::*;
use crate::tx::builders::RefundDestination;
use crate::tx::{assemble_wire_tx, compile_message, decode_message, derive_ata, memo_instruction, split_wire_transaction, transfer_checked_instruction, Message, MessageVersion, Pubkey, Signature};
use candid::Principal;

fn process_burns(fx: &Fixture) {
    block_on(burn_saga::process_burns(&fx.chain, &fx.signer, env::now_ns()));
}

fn lane_tick(fx: &Fixture) {
    fx.tick(env::now_ns() + secs(60));
}

fn decoded(wire: &[u8]) -> Message {
    decode_message(split_wire_transaction(wire).expect("wire").1).expect("message")
}

/// The metadata the RPC reports for a transaction: `(account, owner, pre, post)` per token account of
/// the mint, by the account's index in `msg`.
fn meta_for(msg: &Message, entries: &[(Pubkey, Option<Pubkey>, u64, u64)]) -> TxMeta {
    let row = |pick_post: bool| -> Vec<TokenBalanceEntry> {
        entries
            .iter()
            .map(|(account, owner, pre, post)| TokenBalanceEntry {
                account_index: msg.account_keys.iter().position(|k| k == account).expect("account in message") as u8,
                mint: MINT,
                owner: *owner,
                amount: if pick_post { *post } else { *pre },
                decimals: 8,
            })
            .collect()
    };
    TxMeta { inner_instruction_groups: Some(0), pre_token_balances: Some(row(false)), post_token_balances: Some(row(true)) }
}

/// The same for one of the bridge's own transactions, given as wire bytes.
fn own_meta(wire: &[u8], entries: &[(Pubkey, Option<Pubkey>, u64, u64)]) -> TxMeta {
    meta_for(&decoded(wire), entries)
}

/// Move the books into `RC`, `BU` and `stray` the way the real transitions do (so C1 keeps holding).
fn set_books(rc: u64, bu: u64, stray: u64) {
    let mut ts = vec![];
    if rc + bu > 0 {
        ts.push(Transition::ReturnCustodied { amount: rc + bu });
    }
    if bu > 0 {
        ts.push(Transition::BurnSigned { amount: bu });
    }
    if stray > 0 {
        ts.push(Transition::StrayObserved { amount: stray });
    }
    state::apply_transitions(&ts, crate::state::AuditCtx::note("test books")).unwrap();
}

fn first_signature(wire: &[u8]) -> Signature {
    split_wire_transaction(wire).expect("wire").0[0]
}

/// A custodied return in a fresh world: the observer credited it and the burn lane built its burn.
fn burning_return(amount: u64) -> (Fixture, state::ReturnRecord, u64) {
    let fx = observer_world();
    land_valid_return(&fx, 1, amount);
    observe(&fx);
    process_burns(&fx);
    let r = state::list_returns(None, 1).pop().expect("a return");
    let ReturnState::Burning { burn_op } = r.state else { panic!("{:?}", r.state) };
    (fx, r, burn_op)
}

// ---------------------------------------------------------------------------
// RC -> BU
// ---------------------------------------------------------------------------

#[test]
fn a_custodied_return_gets_exactly_one_burn_keyed_by_its_return_id() {
    let fx = observer_world();
    land_valid_return(&fx, 1, 5 * E8);
    observe(&fx);
    assert_eq!(buckets().return_custodied, (5 * E8) as u128);
    process_burns(&fx);
    let r = state::list_returns(None, 1).pop().unwrap();
    let ReturnState::Burning { burn_op } = r.state else { panic!("{:?}", r.state) };
    let (genesis, salt) = state::read_core(|c| (c.config.genesis_hash, c.instance_salt));
    let op = state::get_op(burn_op).unwrap();
    assert_eq!((op.kind, op.amount), (OpKind::Burn, 5 * E8));
    assert_eq!(op.external_id, ids::burn_id(&genesis, &MINT, &salt, &[r.return_id]).unwrap(), "the burn of THIS return, by burn_id([return_id])");
    assert!(matches!(op.payload, OpPayload::Burn));
    let b = buckets();
    assert_eq!((b.return_custodied, b.burn_unknown), (0, (5 * E8) as u128), "RC -> BU");
    // Idempotent: another pass creates nothing more.
    let next = state::read_core(|c| c.next_op_id);
    process_burns(&fx);
    process_burns(&fx);
    assert_eq!(state::read_core(|c| c.next_op_id), next);
    assert_eq!(buckets(), b);
}

#[test]
fn the_burn_is_not_built_while_halted_or_the_burn_lane_is_halted_and_waits_for_a_slot() {
    let fx = observer_world();
    land_valid_return(&fx, 1, E8);
    observe(&fx);
    state::halt_lane(crate::records::Lane::Burn, "test");
    process_burns(&fx);
    assert_eq!(state::list_returns(None, 1)[0].state, ReturnState::Custodied);
    state::clear_lane_halt(crate::records::Lane::Burn);
    state::halt("test halt");
    process_burns(&fx);
    assert_eq!(state::list_returns(None, 1)[0].state, ReturnState::Custodied);
    state::mutate_core(|c| c.halted = None);
    process_burns(&fx);
    assert!(matches!(state::list_returns(None, 1)[0].state, ReturnState::Burning { .. }));
}

// ---------------------------------------------------------------------------
// BU -> PR: no payout before a final burn proven by its exact bytes and its balance effect
// ---------------------------------------------------------------------------

#[test]
fn nothing_is_paid_until_the_burn_is_final_and_its_exact_bytes_and_balance_effect_are_proven() {
    let amount = 5 * E8;
    let (fx, r, burn_op) = burning_return(amount);
    let wire = fx.wire_of(burn_op, 0);
    // A burn that is only signed and sent: no PR, no release, no ICP leaves the escrow.
    block_on(crate::timer::ledger_pass(&fx.ledger));
    process_burns(&fx);
    assert_eq!(buckets().pending_release, 0);
    assert_eq!(state::active_release_count(), 0);
    assert!(fx.ledger.sent.borrow().iter().all(|o| o.kind != crate::state::LedgerOpKind::Release));

    // The burn lands. The lane adopts its exact bytes (Delivered), but the RPC reports no balance
    // effect: the burn is not yet proven to have destroyed the tokens, so nothing is applied.
    assert!(fx.chain.land(&wire, false));
    lane_tick(&fx);
    assert!(matches!(state::get_op(burn_op).unwrap().state, OpState::Delivered { .. }));
    process_burns(&fx);
    assert_eq!((buckets().burn_unknown, buckets().pending_release), (amount as u128, 0));
    assert!(matches!(state::get_return(&r.return_id).unwrap().state, ReturnState::Burning { .. }));
    assert_eq!(state::active_release_count(), 0);

    // The balance effect arrives: the return account fell by exactly the amount.
    fx.chain.set_meta(&first_signature(&wire), own_meta(&wire, &[(ata(), Some(RETURN_OWNER), amount, 0)]));
    process_burns(&fx);
    let b = buckets();
    assert_eq!((b.burn_unknown, b.pending_release), (0, amount as u128), "BU -> PR");
    assert_eq!(state::get_return(&r.return_id).unwrap().state, ReturnState::Burned { burn_op });
    let release_rec = state::get_release(&r.return_id).expect("the release is open");
    assert_eq!((release_rec.burn_op_id, release_rec.amount_e8s, release_rec.destination), (burn_op, amount, r.destination));
    assert!(state::get_op(burn_op).unwrap().outcome_applied);
    // Applied once.
    process_burns(&fx);
    assert_eq!(buckets(), b);

    // Only now can ICP be paid, and the release completes through the ledger saga.
    block_on(crate::flows::release::process_releases(&fx.ledger));
    assert_eq!(state::get_release(&r.return_id).unwrap().state, ReleaseState::Released);
    assert_eq!(buckets().pending_release, 0);
    assert_covered(&fx);
}

#[test]
fn a_final_burn_whose_balance_effect_is_wrong_halts_and_pays_nothing() {
    let amount = 5 * E8;
    for case in 0..3 {
        let (fx, r, burn_op) = burning_return(amount);
        let wire = fx.wire_of(burn_op, 0);
        assert!(fx.chain.land(&wire, false));
        let payer = profile().fee_payer;
        let (name, entries) = match case {
            0 => ("the return account fell by less", vec![(ata(), Some(RETURN_OWNER), amount, 1)]),
            1 => ("another token account changed", vec![(ata(), Some(RETURN_OWNER), amount, 0), (payer, Some(payer), 5, 4)]),
            _ => ("the return account did not change", vec![(ata(), Some(RETURN_OWNER), amount, amount)]),
        };
        fx.chain.set_meta(&first_signature(&wire), own_meta(&wire, &entries));
        lane_tick(&fx);
        process_burns(&fx);
        assert!(state::is_halted(), "{name}");
        assert_eq!(buckets().pending_release, 0, "{name}");
        assert!(matches!(state::get_return(&r.return_id).unwrap().state, ReturnState::Burning { .. }), "{name}");
        assert_eq!(state::active_release_count(), 0, "{name}");
    }
}

#[test]
fn verify_effect_demands_exactly_the_expected_accounts_and_amounts() {
    let _fx = observer_world();
    let (wire, msg) = return_wire(&HOLDER, &SRC, E8, memo(1), 1);
    let tx = |meta: TxMeta| LandedTx { slot: 1, wire: wire.clone(), failed: false, meta };
    let ata = ata();
    // A burn: the return account fell by exactly the amount and nothing else moved.
    let burn = meta_for(&msg, &[(ata, Some(RETURN_OWNER), 3 * E8, 2 * E8)]);
    assert_eq!(burn_saga::verify_effect(&tx(burn.clone()), &MINT, &ata, None, E8), Effect::Verified);
    assert!(matches!(burn_saga::verify_effect(&tx(burn.clone()), &MINT, &ata, None, E8 + 1), Effect::Mismatch(_)), "a different amount");
    assert!(matches!(burn_saga::verify_effect(&tx(burn), &MINT, &ata, Some(&SRC), E8), Effect::Mismatch(_)), "a burn credits no account");
    // The return's own effect is the mirror image (the return account ROSE): not a burn.
    let own_return = meta_for(&msg, &[(SRC, Some(HOLDER), 5 * E8, 4 * E8), (ata, Some(RETURN_OWNER), 0, E8)]);
    assert!(matches!(burn_saga::verify_effect(&tx(own_return), &MINT, &ata, None, E8), Effect::Mismatch(_)));
    // A refund: the return account fell and exactly the named destination rose by the same amount.
    let refund = meta_for(&msg, &[(ata, Some(RETURN_OWNER), 3 * E8, 2 * E8), (SRC, Some(HOLDER), 0, E8)]);
    assert_eq!(burn_saga::verify_effect(&tx(refund.clone()), &MINT, &ata, Some(&SRC), E8), Effect::Verified);
    assert!(matches!(burn_saga::verify_effect(&tx(refund.clone()), &MINT, &ata, None, E8), Effect::Mismatch(_)), "a burn expects NO other changed account");
    assert!(matches!(burn_saga::verify_effect(&tx(refund.clone()), &MINT, &ata, Some(&SRC), E8 + 1), Effect::Mismatch(_)));
    assert!(matches!(burn_saga::verify_effect(&tx(refund), &MINT, &ata, Some(&[0x5a; 32]), E8), Effect::Mismatch(_)), "another destination");
    // Unreported balances are unavailable, not a verdict; a failed transaction never verifies.
    assert!(matches!(burn_saga::verify_effect(&tx(TxMeta::default()), &MINT, &ata, None, E8), Effect::Unavailable(_)));
    let ok = meta_for(&msg, &[(ata, Some(RETURN_OWNER), 3 * E8, 2 * E8)]);
    assert!(matches!(burn_saga::verify_effect(&LandedTx { failed: true, ..tx(ok) }, &MINT, &ata, None, E8), Effect::Mismatch(_)));
}

// ---------------------------------------------------------------------------
// The binding of a release to its return and its burn
// ---------------------------------------------------------------------------

#[test]
fn an_unrelated_burn_of_an_equal_amount_cannot_authorize_another_return_or_recipient() {
    let fx = saga_tests::world();
    state::mutate_core(|c| c.buckets.pending_release = 0);
    // Two returns of the same amount to different recipients, both fully stored with their own burn.
    fx.ledger.credit(&saga_tests::escrow(), (10 * E8) as u128);
    state::apply_transitions(
        &[
            Transition::AdmitDeposit { amount: 10 * E8 },
            Transition::PullSucceeded { amount: 10 * E8 },
            Transition::MintDelivered { amount: 10 * E8, fee: 0 },
            Transition::ReturnCustodied { amount: 10 * E8 },
            Transition::BurnSigned { amount: 10 * E8 },
            Transition::BurnFinalized { amount: 10 * E8 },
        ],
        crate::state::AuditCtx::note("two burned returns"),
    )
    .unwrap();
    let alice = saga_tests::acct(Principal::from_slice(&[0x41; 10]));
    let bob = saga_tests::acct(Principal::from_slice(&[0x42; 10]));
    let a = stored_burn(1, 5 * E8, alice, SENDER);
    let b = stored_burn(2, 5 * E8, bob, [0x66; 32]);
    // Bob's burn cannot pay Alice's return, and Alice's cannot pay Bob's: the burn id names one return.
    let alice_with_bobs_burn = BurnedReturn { burn_op_id: b.burn_op_id, ..a.clone() };
    assert!(matches!(release::open_release(&alice_with_bobs_burn), Err(OpenError::NotBoundToReturn(m)) if m.contains("not the burn of return")));
    assert!(state::is_halted() || state::get_release(&a.return_id).is_none());
    state::mutate_core(|c| c.halted = None);
    // The right burn, but another recipient or sender than the stored verified return's.
    let to_bob = BurnedReturn { destination: bob, ..a.clone() };
    assert!(matches!(release::open_release(&to_bob), Err(OpenError::NotBoundToReturn(m)) if m.contains("differ from the stored")));
    let other_sender = BurnedReturn { solana_sender: [0x99; 32], ..a.clone() };
    assert!(matches!(release::open_release(&other_sender), Err(OpenError::NotBoundToReturn(_))));
    let other_tx = BurnedReturn { return_signature: Sig([0x77; 64]), ..a.clone() };
    assert!(matches!(release::open_release(&other_tx), Err(OpenError::NotBoundToReturn(_))));
    // A return id with no stored return, or one that is not waiting for this burn.
    let unknown = BurnedReturn { return_id: [0x55; 32], ..a.clone() };
    assert!(release::open_release(&unknown).is_err());
    let mut custodied = state::get_return(&a.return_id).unwrap();
    custodied.state = ReturnState::Custodied;
    state::put_return(&custodied);
    assert!(matches!(release::open_release(&a), Err(OpenError::NotBoundToReturn(m)) if m.contains("not waiting")));
    custodied.state = ReturnState::Burning { burn_op: a.burn_op_id };
    state::put_return(&custodied);
    // Nothing was created by any refusal; the genuine hand-overs open.
    assert_eq!(state::active_release_count(), 0);
    assert!(release::open_release(&a).is_ok());
    assert!(release::open_release(&b).is_ok());
    assert_eq!(state::get_release(&a.return_id).unwrap().destination, alice);
    assert_eq!(state::get_release(&b.return_id).unwrap().destination, bob);
    let _ = (fx, burned_with);
}

// ---------------------------------------------------------------------------
// Custody
// ---------------------------------------------------------------------------

fn set_ata_on(chain: &MockChain, amount: u64) {
    let tp = state::read_core(|c| c.config.token_program);
    let base = crate::mint::fixtures::token_account_data(&MINT, &RETURN_OWNER, amount, None, 1, None);
    chain.set_account(ata(), tp.id(), crate::mint::fixtures::extended_token_account(base, &[(crate::mint::EXT_IMMUTABLE_OWNER, vec![])]));
}

fn set_ata_balance(fx: &Fixture, amount: u64) {
    set_ata_on(&fx.chain, amount);
}

fn caught_up() {
    state::mutate_core(|c| {
        c.observer.caught_up = true;
        c.observer.walk_active = false;
    });
}

#[test]
fn custody_matches_returns_burns_and_strays_and_a_deficit_inside_an_unresolved_burn_is_held() {
    let fx = observer_world();
    caught_up();
    set_supply(&fx, 1_000 * E8);
    set_books(2 * E8, 3 * E8, E8);
    set_ata_balance(&fx, 6 * E8);
    let r = run_monitors(&fx);
    assert_eq!(r.custody, Some(CustodyStatus::Ok), "{r:?}");
    // The burn landed (3 icUSD left the return account) but the books still say BU: reconciling.
    set_ata_balance(&fx, 3 * E8);
    for _ in 0..4 {
        let r = run_monitors(&fx);
        assert_eq!(r.custody, Some(CustodyStatus::Reconciling { deficit: (3 * E8) as u128 }), "{r:?}");
    }
    assert!(!state::is_halted(), "a deficit an unresolved own burn explains halts nothing");
    assert!(state::read_core(|c| c.observer.credits_halted.is_none()));
    // Not OK: the snapshot says so.
    assert!(matches!(state::read_core(|c| c.monitor.custody.clone()).unwrap().status, CustodyStatus::Reconciling { .. }));
}

#[test]
fn an_unexplained_deficit_is_never_ok_and_halts_on_the_confirming_reading() {
    let fx = observer_world();
    caught_up();
    set_supply(&fx, 1_000 * E8);
    set_books(5 * E8, 0, 0);
    // 2 icUSD of the custodied return are gone from the return account and no burn or refund is in
    // flight: nothing of ours explains it.
    set_ata_balance(&fx, 3 * E8);
    let r = run_monitors(&fx);
    assert_eq!(r.custody, Some(CustodyStatus::Deficit { deficit: (2 * E8) as u128 }), "{r:?}");
    assert!(!r.halted, "the first reading only warns");
    assert_eq!(state::read_core(|c| c.monitor.custody_strikes), 1);
    let r = run_monitors(&fx);
    assert!(r.halted && state::is_halted(), "{r:?}");
    // And a deficit larger than what the unresolved burn can explain is still unexplained.
    let fx = observer_world();
    caught_up();
    set_supply(&fx, 1_000 * E8);
    set_books(4 * E8, E8, 0);
    set_ata_balance(&fx, 3 * E8);
    // Books expect 5, 3 are there: the deficit is 2 but the unresolved burn explains only 1.
    assert_eq!(run_monitors(&fx).custody, Some(CustodyStatus::Deficit { deficit: (2 * E8) as u128 }));
}

#[test]
fn a_confirmed_excess_halts_credits_not_the_bridge_and_lifts_when_explained() {
    let fx = observer_world();
    caught_up();
    set_supply(&fx, 1_000 * E8);
    // 4 icUSD sit in the return account that the books do not know of.
    set_ata_balance(&fx, 4 * E8);
    let r = run_monitors(&fx);
    assert_eq!(r.custody, Some(CustodyStatus::Excess { excess: (4 * E8) as u128 }), "{r:?}");
    assert!(state::read_core(|c| c.observer.credits_halted.is_none()), "the first reading only warns");
    run_monitors(&fx);
    assert!(state::read_core(|c| c.observer.credits_halted.is_some()));
    assert!(!state::is_halted(), "credits halt, the bridge does not");
    assert!(state::read_core(|c| c.monitor.custody_strikes) >= 2);
    // The books come to know of it (the observer found the transfer): the next reading is clean and
    // credits resume by themselves.
    state::apply_transitions(&[Transition::StrayObserved { amount: 4 * E8 }], crate::state::AuditCtx::note("explained")).unwrap();
    let r = run_monitors(&fx);
    assert_eq!(r.custody, Some(CustodyStatus::Ok), "{r:?}");
    assert!(state::read_core(|c| c.observer.credits_halted.is_none()), "no excess any more: credits resume");
    assert_eq!(state::read_core(|c| c.monitor.custody_strikes), 0);
}

#[test]
fn while_credits_are_halted_a_valid_return_waits_and_the_admin_alone_can_release_it_with_a_note() {
    let fx = observer_world();
    state::mutate_core(|c| c.observer.credits_halted = Some("4 e8s in the return account that the books cannot explain".into()));
    let sig = land_valid_return(&fx, 1, E8);
    let events = observe(&fx);
    assert_eq!(credited(&events), 0, "{events:?}");
    assert!(events.iter().any(|e| matches!(e, ObserverEvent::Waiting { why } if why.contains("credits are halted"))), "{events:?}");
    assert_eq!((buckets().return_custodied, state::active_return_count()), (0, 0));
    // Only the admin lifts it, and only with a note.
    test_env::set_caller(Principal::from_slice(&[9; 10]));
    assert!(crate::admin::clear_credits_halt("the excess is a donation").is_err());
    test_env::set_caller(saga_tests::admin());
    assert!(matches!(crate::admin::clear_credits_halt(" "), Err(crate::types::BridgeError::InvalidArgument(_))));
    assert!(crate::admin::clear_credits_halt("the excess is a donation").is_ok());
    assert!(matches!(crate::admin::clear_credits_halt("again"), Err(crate::types::BridgeError::InvalidState(_))));
    // The waiting return is credited on the next interval, once.
    test_env::advance_secs(crate::observer::OBSERVER_IDLE_INTERVAL_NS / 1_000_000_000 + 1);
    let events = observe(&fx);
    assert_eq!(credited(&events), 1, "{events:?}");
    assert_eq!(buckets().return_custodied, E8 as u128);
    let _ = sig;
}

#[test]
fn a_supply_change_between_the_two_reads_gives_no_verdict() {
    let fx = observer_world();
    caught_up();
    set_supply(&fx, 1_000 * E8);
    set_ata_balance(&fx, 4 * E8);
    // The first supply read happens, then a burn lands (supply moves) while the return account is read.
    fx.chain.on_account_read_of.borrow_mut().push((
        ata(),
        Box::new(|chain, _addr| {
            let tp = state::read_core(|c| c.config.token_program);
            chain.set_account(MINT, tp.id(), crate::mint::fixtures::extended_mint(crate::mint::fixtures::mint_data(Some(&MINT_AUTHORITY), 990 * E8, 8, true, None), &[]));
        }),
    ));
    let r = run_monitors(&fx);
    assert_eq!(r.custody, None, "{r:?}");
    assert!(r.notes.iter().any(|n| n.contains("supply changed")), "{r:?}");
    assert_eq!(state::read_core(|c| (c.monitor.custody.clone(), c.monitor.custody_strikes)), (None, 0));
    // Read again with no change in between: a valid pair, and now a verdict.
    let r = run_monitors(&fx);
    assert!(r.custody.is_some(), "{r:?}");
}

#[test]
fn custody_is_read_only_when_the_observer_is_at_the_head() {
    let fx = observer_world();
    set_supply(&fx, 1_000 * E8);
    set_ata_balance(&fx, 4 * E8);
    state::mutate_core(|c| c.observer.caught_up = false);
    let r = run_monitors(&fx);
    assert_eq!(r.custody, None);
    assert!(r.notes.iter().any(|n| n.contains("not at the head")), "{r:?}");
}

// ---------------------------------------------------------------------------
// The holder-burn seam
// ---------------------------------------------------------------------------

#[test]
fn a_holder_burn_without_a_memo_lowers_supply_credits_nothing_halts_nothing_and_is_reported() {
    let fx = observer_world();
    // 100 icUSD circulate; the holder burns 10 from their own account (BurnChecked, no memo, nothing
    // touches the return account).
    state::mutate_core(|c| {
        c.buckets.outstanding = (100 * E8) as u128;
        c.observer.caught_up = true;
    });
    fx.ledger.debit(&saga_tests::escrow(), (900 * E8) as u128);
    set_ata_balance(&fx, 0);
    set_supply(&fx, 90 * E8);
    let before = buckets();
    let events = observe(&fx);
    assert_eq!((credited(&events), strays(&events).len()), (0, 0), "the observer never sees it: {events:?}");
    process_burns(&fx);
    let r = run_monitors(&fx);
    assert_eq!((r.c1, r.c2, r.halted), (Some(true), Some(true), false), "{r:?}");
    assert_eq!(r.custody, Some(CustodyStatus::Ok));
    assert_eq!(buckets(), before, "O is unchanged and nothing is credited, swept or refunded");
    assert!(!state::is_halted());
    // It is reported, with the blocker that names the missing accounting.
    let reading = crate::views::monitor_status().c2.expect("a supply reading");
    assert_eq!(reading.unattributed_gap, Some(candid::Nat::from(10 * E8)));
    assert_eq!(reading.unattributed_gap_blocker.as_deref(), Some(crate::accounting::UNATTRIBUTED_BURN_BLOCKER));
    assert_eq!(state::active_return_count() + state::stray_count(), 0);
}

// ---------------------------------------------------------------------------
// Stray refunds
// ---------------------------------------------------------------------------

const ALICE: Pubkey = [0xb1; 32];
const BOB: Pubkey = [0xb2; 32];
const ALICE_SRC: Pubkey = [0xc1; 32];
const BOB_SRC: Pubkey = [0xc2; 32];

/// ONE malformed transaction with two incoming transfers from different senders (each signs its own
/// transfer): no return memo, so a stray; two attributable claims.
fn two_transfer_stray(fx: &Fixture, a1: u64, a2: u64, tag: u8) -> Signature {
    let p = profile();
    let tp = p.token_program.id();
    let instructions = vec![
        transfer_checked_instruction(&tp, &ALICE_SRC, &MINT, &ata(), &ALICE, a1),
        transfer_checked_instruction(&tp, &BOB_SRC, &MINT, &ata(), &BOB, a2),
        memo_instruction(&p.memo_program, b"thanks"),
    ];
    let msg = compile_message(MessageVersion::Legacy, &instructions, &ALICE, [tag; 32]).unwrap();
    let bytes = crate::tx::serialize_message(&msg).unwrap();
    let wire = assemble_wire_tx(&[[tag; 64], [tag.wrapping_add(1); 64]], &bytes).unwrap();
    let entry = |acct: &Pubkey, owner: Pubkey, amount: u64| TokenBalanceEntry {
        account_index: msg.account_keys.iter().position(|k| k == acct).unwrap() as u8,
        mint: MINT,
        owner: Some(owner),
        amount,
        decimals: 8,
    };
    let meta = TxMeta {
        inner_instruction_groups: Some(0),
        pre_token_balances: Some(vec![entry(&ALICE_SRC, ALICE, 10 * E8), entry(&BOB_SRC, BOB, 10 * E8), entry(&ata(), RETURN_OWNER, 0)]),
        post_token_balances: Some(vec![entry(&ALICE_SRC, ALICE, 10 * E8 - a1), entry(&BOB_SRC, BOB, 10 * E8 - a2), entry(&ata(), RETURN_OWNER, a1 + a2)]),
    };
    fx.chain.land_with_meta(&wire, false, meta, &[ata()])
}

fn stray_of(sig: &Signature) -> state::StrayRecord {
    state::get_stray(&Sig(*sig)).expect("a stray")
}

#[test]
fn two_transfers_in_one_transaction_are_two_claims_with_distinct_ids_bound_to_their_source_and_amount() {
    let fx = observer_world();
    let sig = two_transfer_stray(&fx, 2 * E8, 3 * E8, 7);
    let events = observe(&fx);
    assert_eq!(strays(&events).len(), 1, "{events:?}");
    let b = buckets();
    assert_eq!((b.stray, b.return_custodied), ((5 * E8) as u128, 0));
    let st = stray_of(&sig);
    assert_eq!(st.amount, 5 * E8);
    assert_eq!(st.claims.len(), 2, "{:?}", st.claims);
    let (genesis, salt) = state::read_core(|c| (c.config.genesis_hash, c.instance_salt));
    for (i, (src, who, amount)) in [(ALICE_SRC, ALICE, 2 * E8), (BOB_SRC, BOB, 3 * E8)].into_iter().enumerate() {
        let c = &st.claims[i];
        assert_eq!((c.source_account, c.amount, c.verified_sender), (src, amount, Some(who)));
        assert_eq!(c.locator, TransferLocator { outer: i as u16, inner: None });
        assert_eq!(c.refund_id, ids::refund_id(&genesis, &MINT, &salt, &sig, c.locator), "the id carries the position, not the amount or recipient");
        assert_eq!(c.state, ClaimState::Held);
    }
    assert_ne!(st.claims[0].refund_id, st.claims[1].refund_id);
    // Seen again (a lost cursor): the claims are not duplicated and keep their ids.
    state::mutate_core(|c| c.observer.cursor = None);
    test_env::advance_secs(crate::observer::OBSERVER_IDLE_INTERVAL_NS / 1_000_000_000 + 1);
    observe(&fx);
    assert_eq!(stray_of(&sig).claims, st.claims);
    assert_eq!(buckets(), b);
}

#[test]
fn each_claim_is_refunded_to_its_verified_sender_and_settles_stray_only_when_final_and_proven() {
    let fx = observer_world();
    let sig = two_transfer_stray(&fx, 2 * E8, 3 * E8, 7);
    observe(&fx);
    // Alice's source account still exists as her account of this mint: refunded THERE. Bob's was closed:
    // refunded to his associated account, created in the same transaction.
    let tp = state::read_core(|c| c.config.token_program);
    let base = crate::mint::fixtures::token_account_data(&MINT, &ALICE, 10 * E8 - 2 * E8, None, 1, None);
    fx.chain.set_account(ALICE_SRC, tp.id(), crate::mint::fixtures::extended_token_account(base, &[(crate::mint::EXT_IMMUTABLE_OWNER, vec![])]));
    block_on(burn_saga::refund_claims(&fx.chain, &fx.signer, env::now_ns()));
    let st = stray_of(&sig);
    let ops: Vec<u64> = st.claims.iter().map(|c| match c.state { ClaimState::Refunding { op_id } => op_id, ref s => panic!("{s:?}") }).collect();
    let (alice_op, bob_op) = (state::get_op(ops[0]).unwrap(), state::get_op(ops[1]).unwrap());
    assert_eq!((alice_op.kind, alice_op.amount, alice_op.external_id), (OpKind::Refund, 2 * E8, st.claims[0].refund_id));
    assert_eq!((bob_op.kind, bob_op.amount, bob_op.external_id), (OpKind::Refund, 3 * E8, st.claims[1].refund_id));
    assert!(matches!(&alice_op.payload, OpPayload::Refund { destination: RefundDestination::ExistingAccount { token_account } } if *token_account == ALICE_SRC));
    assert!(matches!(&bob_op.payload, OpPayload::Refund { destination: RefundDestination::CreateAta { owner } } if *owner == BOB));
    // Idempotent: another pass builds nothing more.
    let next = state::read_core(|c| c.next_op_id);
    block_on(burn_saga::refund_claims(&fx.chain, &fx.signer, env::now_ns()));
    assert_eq!(state::read_core(|c| c.next_op_id), next);
    // Not yet final: `stray` is untouched.
    assert_eq!(buckets().stray, (5 * E8) as u128);

    // Alice's refund lands with its balance effect; Bob's lands with the WRONG effect (it also moved
    // some other account): Alice settles, Bob halts and settles nothing.
    let alice_wire = fx.wire_of(ops[0], 0);
    assert!(fx.chain.land(&alice_wire, false));
    fx.chain.set_meta(&first_signature(&alice_wire), own_meta(&alice_wire, &[(ata(), Some(RETURN_OWNER), 5 * E8, 3 * E8), (ALICE_SRC, Some(ALICE), 8 * E8, 10 * E8)]));
    lane_tick(&fx);
    block_on(burn_saga::refund_claims(&fx.chain, &fx.signer, env::now_ns()));
    let st = stray_of(&sig);
    assert_eq!(st.claims[0].state, ClaimState::Refunded { op_id: ops[0] });
    assert_eq!(buckets().stray, (3 * E8) as u128, "StrayRefunded: stray fell by exactly Alice's amount, O did not move");
    assert_eq!(buckets().outstanding, (1_000 * E8) as u128);
    assert!(state::get_op(ops[0]).unwrap().outcome_applied);
    let bob_wire = fx.wire_of(ops[1], 0);
    assert!(fx.chain.land(&bob_wire, false));
    let bob_ata = derive_ata(&BOB, &tp.id(), &MINT).unwrap();
    fx.chain.set_meta(&first_signature(&bob_wire), own_meta(&bob_wire, &[(ata(), Some(RETURN_OWNER), 3 * E8, 0), (bob_ata, Some(BOB), 0, 3 * E8 - 1)]));
    lane_tick(&fx);
    block_on(burn_saga::refund_claims(&fx.chain, &fx.signer, env::now_ns()));
    assert!(state::is_halted());
    assert!(matches!(stray_of(&sig).claims[1].state, ClaimState::Refunding { .. }));
    assert_eq!(buckets().stray, (3 * E8) as u128);
}

#[test]
fn a_claim_without_a_verified_sender_below_the_minimum_or_with_a_foreign_source_is_retained_not_refunded() {
    let fx = observer_world();
    // Below the minimum.
    let small = two_transfer_stray(&fx, 1_000, 3 * E8, 7);
    observe(&fx);
    block_on(burn_saga::refund_claims(&fx.chain, &fx.signer, env::now_ns()));
    let st = stray_of(&small);
    assert_eq!(st.claims[0].state, ClaimState::Held, "dust is never refunded");
    assert!(matches!(st.claims[1].state, ClaimState::Refunding { .. }));
    // A source account that exists but belongs to someone else: considered and retained for review.
    let fx = observer_world();
    let sig = two_transfer_stray(&fx, 2 * E8, 3 * E8, 9);
    observe(&fx);
    let tp = state::read_core(|c| c.config.token_program);
    let foreign = crate::mint::fixtures::token_account_data(&MINT, &[0xee; 32], 1, None, 1, None);
    fx.chain.set_account(ALICE_SRC, tp.id(), crate::mint::fixtures::extended_token_account(foreign, &[(crate::mint::EXT_IMMUTABLE_OWNER, vec![])]));
    block_on(burn_saga::refund_claims(&fx.chain, &fx.signer, env::now_ns()));
    assert_eq!(stray_of(&sig).claims[0].state, ClaimState::Retained);
    assert!(matches!(stray_of(&sig).claims[1].state, ClaimState::Refunding { .. }));
    // Retained claims are not retried: no further reads of that account.
    let reads = fx.chain.call_count("get_account");
    block_on(burn_saga::refund_claims(&fx.chain, &fx.signer, env::now_ns()));
    assert_eq!(fx.chain.call_count("get_account"), reads);
    // An owner the RPC did not report is no verified sender: no operation at all.
    let fx = observer_world();
    let (wire, msg) = return_wire(&HOLDER, &SRC, 2 * E8, crate::memo::ReturnMemo { destination: env::canister_id(), subaccount: None, client_nonce: 1 }, 5);
    let mut meta = good_meta(&msg, &HOLDER, &SRC, 2 * E8, 5 * E8, 0);
    meta.pre_token_balances.as_mut().unwrap()[0].owner = None;
    let sig = fx.chain.land_with_meta(&wire, false, meta, &[ata()]);
    observe(&fx);
    assert_eq!(stray_of(&sig).claims[0].verified_sender, None);
    let next = state::read_core(|c| c.next_op_id);
    block_on(burn_saga::refund_claims(&fx.chain, &fx.signer, env::now_ns()));
    assert_eq!(state::read_core(|c| c.next_op_id), next);
    assert_eq!(buckets().stray, (2 * E8) as u128);
}

#[test]
fn a_tampered_claim_amount_cannot_settle_a_refund() {
    let fx = observer_world();
    let sig = two_transfer_stray(&fx, 2 * E8, 3 * E8, 7);
    observe(&fx);
    block_on(burn_saga::refund_claims(&fx.chain, &fx.signer, env::now_ns()));
    let ClaimState::Refunding { op_id } = stray_of(&sig).claims[0].state else { panic!() };
    // The stored claim no longer matches the operation built for it.
    let mut st = stray_of(&sig);
    st.claims[0].amount = 2 * E8 + 1;
    state::put_stray(&mut st);
    let wire = fx.wire_of(op_id, 0);
    assert!(fx.chain.land(&wire, false));
    fx.chain.set_meta(&first_signature(&wire), own_meta(&wire, &[(ata(), Some(RETURN_OWNER), 5 * E8, 3 * E8), (derive_ata(&ALICE, &profile().token_program.id(), &MINT).unwrap(), Some(ALICE), 0, 2 * E8)]));
    lane_tick(&fx);
    let before = buckets();
    block_on(burn_saga::refund_claims(&fx.chain, &fx.signer, env::now_ns()));
    assert_eq!(buckets(), before);
    assert!(matches!(stray_of(&sig).claims[0].state, ClaimState::Refunding { .. }));
}

#[test]
fn a_transfer_by_cpi_is_not_decoded_and_stays_an_unattributed_stray() {
    let fx = observer_world();
    // The ATA rose by 5 but the only outer transfer accounts for 2: the other 3 is not attributable.
    let (wire, msg) = {
        let p = profile();
        let tp = p.token_program.id();
        let ixs = vec![transfer_checked_instruction(&tp, &ALICE_SRC, &MINT, &ata(), &ALICE, 2 * E8), memo_instruction(&p.memo_program, b"x")];
        let msg = compile_message(MessageVersion::Legacy, &ixs, &ALICE, [4; 32]).unwrap();
        let bytes = crate::tx::serialize_message(&msg).unwrap();
        (assemble_wire_tx(&[[4; 64]], &bytes).unwrap(), msg)
    };
    let e = |acct: &Pubkey, owner: Pubkey, amount: u64| TokenBalanceEntry {
        account_index: msg.account_keys.iter().position(|k| k == acct).unwrap() as u8,
        mint: MINT,
        owner: Some(owner),
        amount,
        decimals: 8,
    };
    let meta = TxMeta {
        inner_instruction_groups: Some(1),
        pre_token_balances: Some(vec![e(&ALICE_SRC, ALICE, 10 * E8), e(&ata(), RETURN_OWNER, 0)]),
        post_token_balances: Some(vec![e(&ALICE_SRC, ALICE, 8 * E8), e(&ata(), RETURN_OWNER, 5 * E8)]),
    };
    let sig = fx.chain.land_with_meta(&wire, false, meta, &[ata()]);
    observe(&fx);
    let st = stray_of(&sig);
    assert_eq!(st.amount, 5 * E8);
    assert!(st.claims.is_empty() || st.claims.iter().map(|c| c.amount).sum::<u64>() <= 2 * E8, "{:?}", st.claims);
    assert_eq!(buckets().stray, (5 * E8) as u128, "all of it is tracked, only the attributable part can be claimed");
}

// ---------------------------------------------------------------------------
// The whole round trip
// ---------------------------------------------------------------------------

#[test]
fn a_return_reaches_icp_only_after_its_burn_is_final_through_the_timer_pass_sequence() {
    let fx = saga_tests::world();
    fx.chain.tip_slot.set(2_000);
    set_supply(&fx, 0);
    let id = saga_tests::escrowed(&fx, 0x21, 5 * E8, 1);
    let run = |at: u64| block_on(crate::timer::run_once(&fx.chain, &fx.signer, &fx.ledger, at));
    let now = env::now_ns();
    run(now + secs(1));
    let op = saga_tests::deposit_of(id).mint_op.expect("mint operation");
    assert!(fx.chain.land(&fx.wire_of(op, 0), false));
    set_supply(&fx, 5 * E8);
    run(now + secs(60));
    assert_eq!(saga_tests::deposit_of(id).state, crate::state::DepositState::Delivered);
    // The holder returns 2 icUSD; the next observer interval credits it and the same pass builds the burn.
    let dest_before = fx.ledger.balance(&saga_tests::acct(Principal::from_slice(&[0x41; 10])));
    let sig = land_valid_return(&fx, 1, 2 * E8);
    let report = run(now + secs(400));
    assert_eq!(credited(&report.observer), 1, "{:?}", report.observer);
    let return_id = ids::return_id(&state::read_core(|c| c.config.genesis_hash), &sig);
    let ReturnState::Burning { burn_op } = state::get_return(&return_id).unwrap().state else { panic!() };
    assert_eq!(fx.ledger.balance(&saga_tests::acct(Principal::from_slice(&[0x41; 10]))), dest_before, "no ICP before the burn is final");
    // The burn lands with its effect; the supply falls with it.
    let wire = fx.wire_of(burn_op, 0);
    assert!(fx.chain.land(&wire, false));
    fx.chain.set_meta(&first_signature(&wire), own_meta(&wire, &[(ata(), Some(RETURN_OWNER), 2 * E8, 0)]));
    set_supply(&fx, 3 * E8);
    run(now + secs(460));
    run(now + secs(520));
    assert_eq!(state::get_return(&return_id).unwrap().state, ReturnState::Burned { burn_op });
    assert_eq!(state::get_release(&return_id).unwrap().state, ReleaseState::Released);
    assert!(fx.ledger.balance(&saga_tests::acct(Principal::from_slice(&[0x41; 10]))) > dest_before, "paid on ICP after the final burn");
    let b = buckets();
    assert_eq!((b.outstanding, b.return_custodied, b.burn_unknown, b.pending_release), ((3 * E8) as u128, 0, 0, 0));
    // The monitors agree at the end.
    let m = run(now + secs(520) + monitor::MONITOR_INTERVAL_NS + 1).monitor;
    assert_eq!((m.c1, m.c2, m.halted), (Some(true), Some(true), false), "{m:?}");
    let _ = (deposit::MAX_REFUNDS_PER_TICK, ledger_ops::SEARCH_BLOCKS_PER_TICK);
}

// ---------------------------------------------------------------------------
// Review of ea5be6ba: the custody read is fenced to unchanged books
// ---------------------------------------------------------------------------

/// While the monitor's read of the return account is in flight (its reply is the snapshot from before
/// the transfer), a legitimate return lands, the REAL observer verifies and books it (`O -> RC`, no
/// burn settles), and the on-chain account rises. Supply does not move: a transfer leaves it unchanged.
fn a_return_lands_during_the_ata_read(fx: &Fixture, tag: u8, amount: u64, balance_after: u64) {
    let now = env::now_ns();
    fx.chain.on_account_read_of.borrow_mut().push((
        ata(),
        Box::new(move |chain, _| {
            let (wire, msg) = return_wire(&HOLDER, &SRC, amount, memo(tag as u64), tag);
            chain.land_with_meta(&wire, false, good_meta(&msg, &HOLDER, &SRC, amount, 10 * amount, 0), &[ata()]);
            let events = block_on(crate::observer::observe(chain, now));
            assert_eq!(credited(&events), 1, "the observer must book the legitimate return: {events:?}");
            set_ata_on(chain, balance_after);
        }),
    ));
}

#[test]
fn a_return_booked_during_the_custody_read_defers_the_reading_instead_of_judging_a_deficit() {
    let fx = observer_world();
    caught_up();
    set_supply(&fx, 1_000 * E8);
    set_ata_balance(&fx, 0);
    // A fully backed bridge with empty custody: the first reading is clean.
    assert_eq!(run_monitors(&fx).custody, Some(CustodyStatus::Ok));

    // Reading 1: a return of 5 icUSD is booked while the account read returns its old snapshot (0).
    a_return_lands_during_the_ata_read(&fx, 1, 5 * E8, 5 * E8);
    let r = run_monitors(&fx);
    assert_eq!(buckets().return_custodied, (5 * E8) as u128, "the legitimate return is booked");
    assert_eq!(r.custody, None, "a reading whose books changed under it must not be judged: {r:?}");
    assert!(r.notes.iter().any(|n| n.contains("changed during the reading")), "{r:?}");
    assert_eq!(state::read_core(|c| c.monitor.custody_strikes), 0);
    assert!(!r.halted && !state::is_halted());

    // Reading 2 repeats it with a second return (the observer's idle interval has passed).
    test_env::advance_secs(OBSERVER_IDLE_INTERVAL_NS / 1_000_000_000 + 1);
    a_return_lands_during_the_ata_read(&fx, 2, 3 * E8, 8 * E8);
    let r = run_monitors(&fx);
    assert_eq!(buckets().return_custodied, (8 * E8) as u128);
    assert_eq!(r.custody, None, "{r:?}");
    assert_eq!(state::read_core(|c| c.monitor.custody_strikes), 0, "no deficit strikes accumulated");
    assert!(!r.halted && !state::is_halted(), "a fully backed bridge must not halt: {r:?}");
    assert!(state::read_core(|c| c.observer.credits_halted.is_none()));

    // Nothing moves during the next reading: the books and the account agree.
    let r = run_monitors(&fx);
    assert_eq!(r.custody, Some(CustodyStatus::Ok), "{r:?}");
    assert_eq!(state::read_core(|c| c.monitor.custody_strikes), 0);
    assert_covered(&fx);
}

#[test]
fn a_deferred_custody_reading_does_not_clear_a_credits_halt() {
    let fx = observer_world();
    caught_up();
    set_supply(&fx, 1_000 * E8);
    set_ata_balance(&fx, 0);
    state::mutate_core(|c| c.observer.credits_halted = Some("held for review".into()));
    // A burn settling in the books during the read would also make the pair stale; here a stray is booked.
    fx.chain.on_account_read_of.borrow_mut().push((
        ata(),
        Box::new(|_chain, _| {
            state::apply_transitions(&[Transition::StrayObserved { amount: E8 }], crate::state::AuditCtx::note("booked during the read")).unwrap();
        }),
    ));
    let r = run_monitors(&fx);
    assert_eq!(r.custody, None, "{r:?}");
    assert!(state::read_core(|c| c.observer.credits_halted.is_some()), "a reading that was not judged cannot lift the hold");
}

// ---------------------------------------------------------------------------
// Review of ea5be6ba: the secondary lookup of a burn or a refund is bound to the finalized operation
// ---------------------------------------------------------------------------

/// The wrong answers for an operation's signature, each carrying the perfect balance effect: the stored
/// message under another first signature, the right signatures over a message the operation never
/// stored, and the stored message with fewer signatures than its header requires.
fn wrong_answers(wire: &[u8]) -> Vec<(&'static str, Vec<u8>)> {
    let (sigs, message_bytes) = split_wire_transaction(wire).unwrap();
    assert!(sigs.len() >= 2, "the bridge's own transactions carry two signatures");
    let mut other_first = sigs.clone();
    other_first[0] = [0x99; 64];
    let mut other = decoded(wire);
    other.recent_blockhash = [0xaa; 32];
    vec![
        ("the stored message under another first signature", assemble_wire_tx(&other_first, message_bytes).unwrap()),
        ("the right signatures over a message the operation never stored", assemble_wire_tx(&sigs, &crate::tx::serialize_message(&other).unwrap()).unwrap()),
        ("fewer signatures than the header requires", assemble_wire_tx(&sigs[..1], message_bytes).unwrap()),
    ]
}

#[test]
fn a_burn_is_applied_only_from_the_transaction_that_was_asked_for_and_stored() {
    let amount = 5 * E8;
    for case in 0..3 {
        let (fx, r, burn_op) = burning_return(amount);
        let wire = fx.wire_of(burn_op, 0);
        let sig = first_signature(&wire);
        assert!(fx.chain.land(&wire, false));
        lane_tick(&fx);
        assert!(matches!(state::get_op(burn_op).unwrap().state, OpState::Delivered { .. }));
        let meta = own_meta(&wire, &[(ata(), Some(RETURN_OWNER), amount, 0)]);
        let (name, wrong) = wrong_answers(&wire).remove(case);
        fx.chain.land_under(sig, &wrong, false, meta.clone(), &[]);
        process_burns(&fx);
        assert!(matches!(state::get_return(&r.return_id).unwrap().state, ReturnState::Burning { .. }), "{name}: applied from a wrong answer");
        assert_eq!((buckets().pending_release, state::active_release_count()), (0, 0), "{name}");
        assert!(!state::is_halted(), "{name}: a wrong answer waits, it does not halt");
        // The genuine answer arrives: applied.
        fx.chain.land_under(sig, &wire, false, meta, &[]);
        process_burns(&fx);
        assert!(matches!(state::get_return(&r.return_id).unwrap().state, ReturnState::Burned { .. }), "{name}");
    }
}

#[test]
fn a_refund_is_settled_only_from_the_transaction_that_was_asked_for_and_stored() {
    for case in 0..3 {
        let fx = observer_world();
        let sig_stray = two_transfer_stray(&fx, 2 * E8, 3 * E8, 7);
        observe(&fx);
        let tp = state::read_core(|c| c.config.token_program);
        let base = crate::mint::fixtures::token_account_data(&MINT, &ALICE, 8 * E8, None, 1, None);
        fx.chain.set_account(ALICE_SRC, tp.id(), crate::mint::fixtures::extended_token_account(base, &[(crate::mint::EXT_IMMUTABLE_OWNER, vec![])]));
        block_on(burn_saga::refund_claims(&fx.chain, &fx.signer, env::now_ns()));
        let ClaimState::Refunding { op_id } = stray_of(&sig_stray).claims[0].state else { panic!("claim 0 must be refunding") };
        let wire = fx.wire_of(op_id, 0);
        let sig = first_signature(&wire);
        assert!(fx.chain.land(&wire, false));
        lane_tick(&fx);
        let meta = own_meta(&wire, &[(ata(), Some(RETURN_OWNER), 5 * E8, 3 * E8), (ALICE_SRC, Some(ALICE), 8 * E8, 10 * E8)]);
        let (name, wrong) = wrong_answers(&wire).remove(case);
        fx.chain.land_under(sig, &wrong, false, meta.clone(), &[]);
        let before = buckets();
        block_on(burn_saga::refund_claims(&fx.chain, &fx.signer, env::now_ns()));
        assert!(matches!(stray_of(&sig_stray).claims[0].state, ClaimState::Refunding { .. }), "{name}: settled from a wrong answer");
        assert_eq!(buckets(), before, "{name}");
        assert!(!state::is_halted(), "{name}");
        fx.chain.land_under(sig, &wire, false, meta, &[]);
        block_on(burn_saga::refund_claims(&fx.chain, &fx.signer, env::now_ns()));
        assert_eq!(stray_of(&sig_stray).claims[0].state, ClaimState::Refunded { op_id }, "{name}");
    }
}
