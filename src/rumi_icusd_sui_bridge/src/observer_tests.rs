//! Tests of the 53c return observer, the monitors and the audit report against the simulated chain
//! (`chain_sim`): a Sui `bridge_out` burn becomes an ICP release only through the Gate's durable record,
//! events and hints discover only, and the C1 to C4 monitors hold on gaps and foreign mints. Nothing here
//! touches a network.

use crate::chain_sim::*;
use crate::env::{self, test_env};
use crate::flows::observer;
use crate::flows::release;
use crate::lane_tests::{lane, Lane};
use crate::saga_tests::{acct, assert_covered, buckets, dest, deposit_of, escrow, escrowed, user, E8, FEE};
use crate::state::{self, CheckOutcome, DepositState, ReleaseState};
use crate::sui::format_digest_base58;
use crate::types::*;
use icrc_ledger_types::icrc1::account::Account;

const HOLDER: [u8; 32] = [0x91; 32];

fn principal(n: u8) -> Vec<u8> {
    user(n).as_slice().to_vec()
}

/// Mint `amount` to Sui through the real lane and wait for it to be delivered.
fn minted(l: &Lane, amount: u64, req: u8) {
    let id = escrowed(&l.fx, 0x21, amount, req);
    until(l, 12, || deposit_of(id).state == DepositState::Delivered);
}

fn until(l: &Lane, max: usize, pred: impl Fn() -> bool) {
    for _ in 0..max {
        if pred() {
            return;
        }
        l.tick();
    }
    assert!(pred(), "the condition did not hold after {max} passes");
}

fn all_providers(l: &Lane, p: Provider) {
    for id in ["alpha", "beta", "gamma"] {
        l.sim.set_provider(id, p.clone());
    }
}

fn released(tid: &[u8; 32]) -> bool {
    state::get_release(tid).is_some_and(|r| r.state == ReleaseState::Released)
}

fn problems() -> Vec<String> {
    state::read_runtime(|c| c.observer.problems.iter().map(|p| p.note.clone()).collect())
}

fn check(name: &str) -> CheckOutcome {
    state::read_runtime(|c| c.monitor.clone()).expect("the monitors have run").checks.into_iter().find(|c| c.name.starts_with(name)).expect("check").outcome
}

// ---------------------------------------------------------------------------
// The round trip
// ---------------------------------------------------------------------------

#[test]
fn a_burn_on_sui_is_opened_from_the_durable_record_and_released_to_icp_exactly_once() {
    let l = lane();
    minted(&l, 5 * E8, 1);
    assert_eq!(l.sim.gate.borrow().total_supply, 5 * E8);
    let (tx, tid) = l.sim.bridge_out(HOLDER, 1, 2 * E8, principal(0x41), [0u8; 32]);
    until(&l, 40, || released(&tid));
    let r = state::get_release(&tid).unwrap();
    assert_eq!((r.gate_sequence, r.sui_sender, r.client_nonce, r.amount_e8s, r.destination, r.tx_digest), (1, HOLDER, 1, 2 * E8, dest(), tx));
    assert_eq!(r.return_id, crate::ids::transfer_id(&crate::ids::TESTNET_CHAIN_IDENTIFIER, &SIM_GATE, &HOLDER, 1), "the id the canister derived itself");
    assert!(r.checkpoint >= l.sim.tx(&tx).unwrap().checkpoint, "verified at a checkpoint at or after the burn");
    // The exact ICP amount: the burn, less the ledger fee (no bridge fee configured), once.
    assert_eq!(l.fx.ledger.balance(&dest()), (2 * E8 - FEE) as u128);
    let b = buckets();
    assert_eq!((b.outstanding, b.pending_release, b.pending_mint), ((3 * E8) as u128, 0, 0));
    assert_covered(&l.fx);
    // The observer is caught up and the monitors agree with the chain: supply equals outstanding (C3).
    let o = state::read_runtime(|c| c.observer.clone());
    assert_eq!((o.verified_through, o.caught_up, o.gap.clone(), o.candidates.len()), (1, true, None, 0));
    assert_eq!(state::read_runtime(|c| c.returned_e8s), (2 * E8) as u128);
    assert_eq!(l.sim.gate.borrow().total_supply, 3 * E8);
    for name in ["C1", "C2", "C3", "C4 mint", "C4 return", "returns complete"] {
        assert_eq!(check(name), CheckOutcome::Ok, "{name}");
    }
    assert!(!state::is_halted() && !state::is_mint_lane_halted());
    // Later passes change nothing and pay nothing more.
    l.ticks(15);
    assert_eq!(l.fx.ledger.balance(&dest()), (2 * E8 - FEE) as u128);
    assert_eq!(state::active_release_count(), 0);
}

#[test]
fn the_release_pays_the_exact_eight_decimal_amount_net_of_both_fees() {
    let l = lane();
    state::mutate_runtime(|r| r.config.fees.release_fee_e8s = 1_234_567);
    minted(&l, 5 * E8 + 3, 1);
    let burned = 123_456_789u64;
    let (_tx, tid) = l.sim.bridge_out(HOLDER, 1, burned, principal(0x41), [7u8; 32]);
    until(&l, 40, || released(&tid));
    let r = state::get_release(&tid).unwrap();
    assert_eq!((r.amount_e8s, r.release_fee_e8s), (burned, 1_234_567));
    assert_eq!(r.destination, Account { owner: user(0x41), subaccount: Some([7u8; 32]) });
    let paid = (burned - 1_234_567 - FEE) as u128;
    assert_eq!(l.fx.ledger.balance(&Account { owner: user(0x41), subaccount: Some([7u8; 32]) }), paid, "the destination subaccount the Gate saved");
    assert_eq!(buckets().fees, 1_234_567, "the bridge fee is kept apart in the fee bucket");
    assert_eq!(buckets().outstanding, (5 * E8 + 3 - burned) as u128);
    assert_eq!(l.sim.gate.borrow().total_supply, 5 * E8 + 3 - burned);
    assert_covered(&l.fx);
}

#[test]
fn the_release_comes_from_the_record_not_from_what_an_event_says() {
    // The event lies about the amount (and the sequence); the durable record is true. Nothing is opened, the
    // disagreement is recorded, and the unaccounted return stays a visible gap.
    let l = lane();
    minted(&l, 5 * E8, 1);
    let (_tx, tid) = l.sim.bridge_out_with_false_event(HOLDER, 1, 2 * E8, principal(0x41), [0u8; 32], (4 * E8, 1));
    l.ticks(14);
    assert!(state::get_release(&tid).is_none(), "an event never pays");
    assert_eq!(l.fx.ledger.balance(&dest()), 0);
    assert!(problems().iter().any(|p| p.contains("disagree about the amount, sequence or destination")), "{:?}", problems());
    let o = state::read_runtime(|c| c.observer.clone());
    assert!(o.candidates.is_empty(), "the candidate was dropped");
    assert!(o.gap.as_deref().unwrap_or("").contains("sequence 1"), "{:?}", o.gap);
    assert_eq!(o.verified_through, 0);
    assert!(matches!(check("returns complete"), CheckOutcome::NotEvaluated { reason } if reason.contains("hold")));
}

#[test]
fn a_lookalike_event_of_another_package_is_never_a_candidate_not_by_scan_and_not_by_hint() {
    let l = lane();
    minted(&l, 5 * E8, 1);
    let tx = l.sim.emit_lookalike_bridge_out([0x71; 32], HOLDER, 1, 3 * E8);
    l.ticks(14);
    assert_eq!(state::read_runtime(|c| c.observer.candidates.len()), 0);
    assert_eq!(state::active_release_count(), 0);
    assert_eq!(l.fx.ledger.balance(&dest()), 0);
    // A hint naming the lookalike transaction finds no BridgeOut event of the configured package.
    crate::notify_sui_return(format_digest_base58(&tx)).unwrap();
    l.ticks(4);
    assert_eq!(state::active_release_count(), 0);
    assert!(problems().iter().any(|p| p.contains("no BridgeOut event of the configured Gate package")), "{:?}", problems());
    assert!(state::read_runtime(|c| c.hints.is_empty()), "the hint was consumed");
    assert!(!state::is_halted(), "a lookalike changes no counter and halts nothing");
}

#[test]
fn an_event_whose_transfer_id_is_not_what_its_sender_and_nonce_produce_is_rejected() {
    let l = lane();
    minted(&l, 5 * E8, 1);
    // A genuine return, then a right-typed event whose transfer id the Gate could not have computed for its
    // (sender, nonce). (With no genuine return the Gate's counter would show nothing new and the window would
    // not even be listed.)
    let (_tx, good) = l.sim.bridge_out(HOLDER, 1, E8, principal(0x41), [0u8; 32]);
    let cp = l.sim.head.get() + 1;
    l.sim.head.set(cp);
    let mut w = crate::bcs::Writer::new();
    w.bytes(&[0x13; 32]).raw(&[0x95; 32]).u64(E8).u64(2).bytes(&principal(0x42)).bytes(&[0u8; 32]).u64(7).u128(0).u64(0);
    l.sim.push_event(cp, [0x77; 32], [0x95; 32], format!("0x{}::rumi_icusd_gate::BridgeOut", hex::encode(SIM_PACKAGE)), w.into_bytes());
    until(&l, 40, || released(&good));
    l.ticks(6);
    assert!(problems().iter().any(|p| p.contains("transfer id is not the one its sender and nonce")), "{:?}", problems());
    // Nothing was opened for the corrupt event and the genuine return paid once. (The scan moves on only
    // because the Gate's own counter shows every return is accounted for; the event never counted for anything.)
    assert_eq!(state::read_runtime(|c| c.observer.verified_through), 1);
    assert_eq!(state::active_release_count(), 0);
    assert_eq!(l.fx.ledger.balance(&dest()), (E8 - FEE) as u128);
    assert_eq!(l.fx.ledger.balance(&acct(user(0x42))), 0);
}

#[test]
fn a_return_found_twice_by_scan_and_by_hint_makes_one_release_and_one_payment() {
    let l = lane();
    minted(&l, 5 * E8, 1);
    let (tx, tid) = l.sim.bridge_out(HOLDER, 1, 2 * E8, principal(0x41), [0u8; 32]);
    crate::notify_sui_return(format_digest_base58(&tx)).unwrap();
    crate::notify_sui_return(format_digest_base58(&tx)).unwrap(); // a repeat hint is a no-op
    assert_eq!(state::read_runtime(|c| c.hints.len()), 1);
    until(&l, 40, || released(&tid));
    // Hint it again after it paid, and let the scan run past: still one release, one payment.
    crate::notify_sui_return(format_digest_base58(&tx)).unwrap();
    l.ticks(14);
    assert_eq!(state::read_runtime(|c| c.observer.verified_through), 1);
    assert_eq!(l.fx.ledger.balance(&dest()), (2 * E8 - FEE) as u128);
    assert_eq!(state::read_runtime(|c| c.returned_e8s), (2 * E8) as u128);
    assert_eq!(l.fx.ledger.blocks.borrow().iter().filter(|b| b.kind == state::LedgerOpKind::Release).count(), 1);
    assert!(state::read_runtime(|c| c.hints.is_empty()));
}

// ---------------------------------------------------------------------------
// Hints and gaps
// ---------------------------------------------------------------------------

#[test]
fn a_hint_discovers_a_return_the_event_index_does_not_show_and_a_gap_holds_later_returns_in_order() {
    let l = lane();
    minted(&l, 9 * E8, 1);
    // Two returns; the event index hides everything from the first one on.
    let (tx_a, tid_a) = l.sim.bridge_out(HOLDER, 1, 2 * E8, principal(0x41), [0u8; 32]);
    let (tx_b, tid_b) = l.sim.bridge_out([0x92; 32], 1, 3 * E8, principal(0x42), [0u8; 32]);
    all_providers(&l, Provider { hide_events_from: Some(l.sim.tx(&tx_a).unwrap().checkpoint), ..Provider::default() });
    l.ticks(12);
    // Nothing is known: the Gate counts two returns that no event accounts for.
    let o = state::read_runtime(|c| c.observer.clone());
    assert_eq!((o.verified_through, o.candidates.len()), (0, 0));
    assert!(o.gap.as_deref().unwrap_or("").contains("sequence 1"), "{:?}", o.gap);
    // The later return is hinted first. It is verified against the record but waits: sequence 1 is unaccounted.
    crate::notify_sui_return(format_digest_base58(&tx_b)).unwrap();
    l.ticks(4);
    let o = state::read_runtime(|c| c.observer.clone());
    assert_eq!(o.candidates.len(), 1);
    assert!(o.candidates[0].ready.is_some(), "verified against the durable record");
    assert!(state::get_release(&tid_b).is_none(), "but not opened ahead of sequence 1");
    assert_eq!(l.fx.ledger.balance(&acct(user(0x42))), 0);
    // The earlier one is hinted: both open, in Gate sequence order.
    crate::notify_sui_return(format_digest_base58(&tx_a)).unwrap();
    until(&l, 40, || released(&tid_a) && released(&tid_b));
    assert_eq!((state::get_release(&tid_a).unwrap().seq, state::get_release(&tid_b).unwrap().seq), (0, 1), "opened in sequence order");
    assert_eq!(l.fx.ledger.balance(&dest()), (2 * E8 - FEE) as u128);
    assert_eq!(l.fx.ledger.balance(&acct(user(0x42))), (3 * E8 - FEE) as u128);
    assert!(state::read_runtime(|c| c.observer.gap.is_none()));
}

#[test]
fn hints_that_name_failed_or_unknown_transactions_settle_nothing() {
    let l = lane();
    minted(&l, 5 * E8, 1);
    // A transaction that is not a return (the mint itself).
    let mint_tx = l.op(0).current().unwrap().tx_digest;
    crate::notify_sui_return(format_digest_base58(&mint_tx)).unwrap();
    // A transaction nobody has ever heard of stays a hint until it expires.
    let unknown = [0xee; 32];
    crate::notify_sui_return(format_digest_base58(&unknown)).unwrap();
    l.ticks(4);
    assert!(problems().iter().any(|p| p.contains("no BridgeOut event")), "{:?}", problems());
    let hints = state::read_runtime(|c| c.hints.clone());
    assert_eq!(hints.len(), 1, "the unknown one is kept");
    assert_eq!(hints[0].digest, unknown);
    assert_eq!((state::active_release_count(), l.fx.ledger.balance(&dest())), (0, 0));
    test_env::advance_secs(2 * 3_600 + 1);
    assert!(release::live_hints(env::now_ns()).is_empty(), "and it expires");
}

#[test]
fn the_unserved_event_range_is_a_hold_and_scanning_resumes_when_it_is_served() {
    let l = lane();
    minted(&l, 5 * E8, 1);
    let before = state::read_runtime(|c| c.observer.scanned_through);
    all_providers(&l, Provider { events_unserved: true, ..Provider::default() });
    let (_tx, tid) = l.sim.bridge_out(HOLDER, 1, 2 * E8, principal(0x41), [0u8; 32]);
    l.ticks(14);
    assert_eq!(state::read_runtime(|c| c.observer.scanned_through), before, "an unserved range is not an empty one");
    assert!(problems().iter().any(|p| p.contains("do not serve events")), "{:?}", problems());
    assert!(state::get_release(&tid).is_none());
    all_providers(&l, Provider::default());
    until(&l, 40, || released(&tid));
}

#[test]
fn a_busy_window_is_bisected_and_a_single_checkpoint_with_more_than_a_page_is_a_hold() {
    // 25 returns over 25 checkpoints: one window holds more than a page (20), so it is bisected and all are found.
    let l = lane();
    minted(&l, 40 * E8, 1);
    let mut tids = vec![];
    for n in 0..25u64 {
        tids.push(l.sim.bridge_out([0x93; 32], n, E8, principal(0x41), [0u8; 32]).1);
    }
    until(&l, 120, || tids.iter().all(released));
    assert_eq!(state::read_runtime(|c| c.observer.verified_through), 25);
    assert_eq!(l.fx.ledger.balance(&dest()), (25 * (E8 - FEE)) as u128);
    let seqs: Vec<u64> = tids.iter().map(|t| state::get_release(t).unwrap().seq).collect();
    assert_eq!(seqs, (0..25).collect::<Vec<_>>(), "opened in Gate sequence order");

    // 25 returns in ONE checkpoint cannot be listed (cursors are not compared across providers). The
    // window shrinks until the busy checkpoint is isolated, the problem is recorded, and nothing is opened
    // from the listing.
    let l = lane();
    minted(&l, 40 * E8, 1);
    l.sim.batch_cp.set(Some(l.sim.head.get() + 3));
    let batch: Vec<_> = (0..25u64).map(|n| l.sim.bridge_out([0x93; 32], n, E8, principal(0x41), [0u8; 32])).collect();
    l.ticks(24);
    assert!(problems().iter().any(|p| p.contains("more BridgeOut events than one page")), "{:?}", problems());
    assert_eq!((state::active_release_count(), state::read_runtime(|c| c.observer.verified_through)), (0, 0));
    assert_eq!(l.fx.ledger.balance(&dest()), 0);
    let stuck_at = state::read_runtime(|c| c.observer.scanned_through);
    assert!(stuck_at < l.sim.tx(&batch[0].0).unwrap().checkpoint, "the scan holds before the busy checkpoint");
    assert!(state::read_runtime(|c| c.observer.gap.is_some() || !c.observer.caught_up));
    // Hints supply the returns the listing cannot (five callers, five hints each). Once the Gate's counter is
    // fully accounted for, the window commits past the busy checkpoint without a listing, and they open in order.
    for (i, (tx, _)) in batch.iter().enumerate() {
        test_env::set_caller(user(0x30 + (i / 5) as u8));
        crate::notify_sui_return(format_digest_base58(tx)).unwrap();
    }
    test_env::set_caller(user(0x21));
    until(&l, 120, || batch.iter().all(|(_, t)| released(t)));
    assert!(state::read_runtime(|c| c.observer.scanned_through) > stuck_at, "the scan moved past the busy checkpoint");
    assert_eq!((state::read_runtime(|c| c.observer.verified_through), state::read_runtime(|c| c.observer.gap.clone())), (25, None));
}

// ---------------------------------------------------------------------------
// Waiting for the books
// ---------------------------------------------------------------------------

#[test]
fn a_return_of_coin_whose_mint_the_books_have_not_applied_waits_and_pays_nothing() {
    let l = lane();
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    *l.sim.mode.borrow_mut() = ExecMode::Drop;
    l.ticks(2);
    // The mint lands on Sui, the holder returns part of it at once, and the mint lane is paused so the
    // canister has not yet applied the delivery to its books.
    let bytes = l.op(0).current().unwrap().tx_bcs.clone();
    l.sim.execute_accepted(&bytes);
    state::mutate_runtime(|r| r.config.pauses.mints = true);
    let (_tx, tid) = l.sim.bridge_out(HOLDER, 1, 2 * E8, principal(0x41), [0u8; 32]);
    l.ticks(14);
    let o = state::read_runtime(|c| c.observer.clone());
    assert_eq!(o.candidates.len(), 1);
    assert!(o.candidates[0].ready.is_some());
    assert!(o.candidates[0].waiting.as_deref().unwrap_or("").contains("mint it returns is not applied yet"), "{:?}", o.candidates[0].waiting);
    assert!(state::get_release(&tid).is_none());
    assert_eq!(l.fx.ledger.balance(&dest()), 0);
    assert_eq!(buckets().outstanding, 0);
    // The mint lane resumes, the delivery is applied, and the waiting return opens and pays.
    state::mutate_runtime(|r| r.config.pauses.mints = false);
    test_env::advance_secs(10 * 60);
    until(&l, 40, || released(&tid));
    assert_eq!(deposit_of(id).state, DepositState::Delivered);
    assert_eq!(l.fx.ledger.balance(&dest()), (2 * E8 - FEE) as u128);
    assert_eq!(buckets().outstanding, (3 * E8) as u128);
    assert_eq!(l.sim.gate.borrow().total_supply, 3 * E8);
}

#[test]
fn a_waiting_verified_return_survives_an_upgrade_and_opens_once_afterwards() {
    use ic_stable_structures::Storable;
    let l = lane();
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    *l.sim.mode.borrow_mut() = ExecMode::Drop;
    l.ticks(2);
    let bytes = l.op(0).current().unwrap().tx_bcs.clone();
    l.sim.execute_accepted(&bytes);
    state::mutate_runtime(|r| r.config.pauses.mints = true);
    let (_tx, tid) = l.sim.bridge_out(HOLDER, 1, 2 * E8, principal(0x41), [0u8; 32]);
    l.ticks(14);
    assert!(state::read_runtime(|c| c.observer.candidates[0].ready.is_some()));
    // "Upgrade": the runtime record is read back from its bytes.
    let core = state::read_runtime(|c| c.clone());
    state::mutate_runtime(|c| *c = state::BridgeCore::from_bytes(core.to_bytes()));
    let c = state::read_runtime(|c| c.observer.candidates.clone());
    assert_eq!(c.len(), 1);
    assert!(c[0].ready.is_some() && c[0].waiting.is_some());
    state::mutate_runtime(|r| r.config.pauses.mints = false);
    test_env::advance_secs(10 * 60);
    until(&l, 40, || released(&tid));
    assert_eq!(l.fx.ledger.balance(&dest()), (2 * E8 - FEE) as u128);
    assert_eq!(deposit_of(id).state, DepositState::Delivered);
    assert_eq!(state::read_runtime(|c| c.observer.verified_through), 1);
}

#[test]
fn a_return_to_an_unpayable_destination_is_held_and_re_minted_to_its_sui_sender() {
    // The full cycle through the observer: the saved ICP destination is this canister, so the release opens
    // held; the re-mint driver gives the value back to the Sui sender through the mint lane.
    let l = lane();
    minted(&l, 5 * E8, 1);
    let canister = env::canister_id().as_slice().to_vec();
    let (_tx, tid) = l.sim.bridge_out(HOLDER, 1, 2 * E8, canister, [0u8; 32]);
    until(&l, 60, || matches!(state::get_release(&tid).map(|r| r.state), Some(ReleaseState::Reminted { .. })));
    let ReleaseState::Reminted { deposit_id } = state::get_release(&tid).unwrap().state else { panic!() };
    assert_eq!(deposit_of(deposit_id).state, DepositState::Delivered);
    // The chain: 5 minted, 2 burned, 2 minted back to the sender.
    assert_eq!(l.sim.gate.borrow().total_supply, 5 * E8);
    assert_eq!(buckets().outstanding, (5 * E8) as u128);
    assert_eq!((buckets().held, buckets().pending_mint, buckets().pending_release), (0, 0, 0));
    assert_eq!(l.fx.ledger.balance(&acct(env::canister_id())), 0, "nothing was paid on ICP");
    until(&l, 20, || state::read_runtime(|c| c.monitor.clone()).is_some_and(|m| m.caught_up));
    assert_eq!(check("C3"), CheckOutcome::Ok);
    assert!(!state::is_halted());
}

// ---------------------------------------------------------------------------
// Monitors
// ---------------------------------------------------------------------------

#[test]
fn a_foreign_mint_halts_the_bridge_and_the_lane_and_stops_every_release() {
    let l = lane();
    minted(&l, 5 * E8, 1);
    let (_tx, tid) = l.sim.bridge_out(HOLDER, 1, 2 * E8, principal(0x41), [0u8; 32]);
    until(&l, 40, || released(&tid));
    // Someone mints with an id this canister never made.
    l.sim.foreign_mint([0x31; 32], [0x44; 32], 7);
    state::mutate_runtime(|r| r.observer.next_run_ns = 0);
    l.ticks(14);
    assert!(state::is_halted() && state::is_mint_lane_halted());
    let reason = state::read_runtime(|c| c.halted.clone().unwrap().reason);
    assert!(reason.contains("monitor C"), "{reason}");
    assert!(matches!(check("C4 mint"), CheckOutcome::Breach { .. }));
    assert!(matches!(check("C2"), CheckOutcome::Breach { .. }), "the supply also exceeds what the books explain");
    // A return opened now is recorded (the books follow the chain) but nothing is paid while halted.
    let (_tx2, tid2) = l.sim.bridge_out(HOLDER, 2, E8, principal(0x42), [0u8; 32]);
    l.ticks(14);
    assert!(state::get_release(&tid2).is_some_and(|r| r.state == ReleaseState::Pending));
    assert_eq!(l.fx.ledger.balance(&acct(user(0x42))), 0);
    let report = crate::views::audit_report(env::now_ns());
    assert!(report.checks.iter().any(|c| matches!(c.outcome, CheckOutcomeView::Breach { .. })));
    assert!(report.pending_and_held.reasons.iter().any(|r| r.contains("bridge halted")));
}

#[test]
fn an_escrow_shortfall_read_from_the_ledger_halts_the_bridge() {
    let l = lane();
    minted(&l, 5 * E8, 1);
    l.fx.ledger.debit(&escrow(), (E8) as u128);
    state::mutate_runtime(|r| r.observer.next_run_ns = 0);
    l.ticks(3);
    assert!(matches!(check("C1"), CheckOutcome::Breach { scope: state::BreachScope::Bridge, .. }));
    assert!(state::is_halted());
    assert!(state::read_runtime(|c| c.halted.clone().unwrap().reason).contains("C1"));
}

#[test]
fn books_ahead_of_the_chain_halt_only_the_mint_lane() {
    let l = lane();
    minted(&l, 5 * E8, 1);
    state::mutate_runtime(|r| {
        r.minted_count += 1;
        r.minted_e8s += 1;
        r.observer.next_run_ns = 0;
    });
    l.ticks(3);
    assert!(matches!(check("C4 mint"), CheckOutcome::Breach { scope: state::BreachScope::MintLane, .. }));
    assert!(state::is_mint_lane_halted() && !state::is_halted());
}

#[test]
fn the_observer_makes_no_request_while_paused_unobserved_or_unconfigured() {
    let l = lane();
    minted(&l, 5 * E8, 1);
    let asked = l.sim.served.borrow().len();
    state::mutate_runtime(|r| {
        r.config.pauses.observer = true;
        r.observer.next_run_ns = 0;
    });
    l.ticks(3);
    assert_eq!(l.sim.served.borrow().len(), asked, "paused");
    state::mutate_runtime(|r| {
        r.config.pauses.observer = false;
        r.observed = None;
    });
    l.ticks(3);
    assert_eq!(l.sim.served.borrow().len(), asked, "no observed deployment");
}

#[test]
fn an_idle_observer_costs_a_handful_of_reads_every_five_minutes() {
    let l = lane();
    minted(&l, 5 * E8, 1);
    until(&l, 20, || state::read_runtime(|c| c.observer.caught_up));
    l.ticks(1);
    let before = l.sim.served.borrow().len();
    l.ticks(120); // an hour of 30 s passes
    let used = l.sim.served.borrow().len() - before;
    // Twelve wake-ups, each a head, a checkpoint and a gate from three providers (no event listing: the
    // Gate's own counter shows nothing is new).
    assert!(used <= 12 * 3 * 3 + 12, "an idle hour made {used} requests");
    assert!(!l.sim.served.borrow().iter().skip(before).any(|q| q.starts_with("query($f:EventFilter")), "an idle observer lists no events");
    assert!(used >= 12 * 3, "and it did wake up: {used}");
}

// ---------------------------------------------------------------------------
// The audit report
// ---------------------------------------------------------------------------

#[test]
fn the_audit_report_keeps_observations_books_pending_liabilities_and_the_checkpoint_apart() {
    let l = lane();
    minted(&l, 9 * E8, 1);
    let (_tx, tid) = l.sim.bridge_out(HOLDER, 1, 2 * E8, principal(0x41), [0u8; 32]);
    until(&l, 40, || released(&tid));
    until(&l, 20, || state::read_runtime(|c| c.monitor.clone()).is_some_and(|m| m.caught_up));
    // A pending liability on the books: a second deposit, escrowed and not yet minted.
    let pending = escrowed(&l.fx, 0x22, 3 * E8, 1);
    let r = crate::views::audit_report(env::now_ns());
    // Independent: the chain at the checkpoint, the ledger's escrow balance, the observed deployment.
    let m = state::read_runtime(|c| c.monitor.clone()).unwrap();
    assert_eq!(r.independent.checkpoint, Some(m.checkpoint));
    assert_eq!(r.independent.gate.as_ref().map(|g| (g.total_supply, g.minted_total, g.burned_total, g.mint_count, g.bridge_out_count)), Some((7 * E8, (9 * E8) as u128, (2 * E8) as u128, 1, 1)));
    assert_eq!(r.independent.escrow_balance_e8s, m.escrow_balance);
    assert!(r.independent.deployment.is_some());
    // Self-reported: the books as they stand now, including the new pending mint the checks never saw.
    assert_eq!((r.self_reported.buckets.outstanding, r.self_reported.minted_count, r.self_reported.minted_e8s), (candid::Nat::from(7 * E8), 1, (9 * E8) as u128));
    assert_eq!((r.self_reported.returns_verified_through, r.self_reported.returned_e8s), (1, (2 * E8) as u128));
    assert_eq!(r.pending_and_held.pending_mint_e8s, (3 * E8) as u128);
    assert_eq!(r.books_at_last_check.as_ref().map(|b| b.pending_mint.clone()), Some(candid::Nat::from(0u64)), "the books as the checks saw them");
    assert_ne!(Some(r.self_reported.buckets.pending_mint.clone()), r.books_at_last_check.as_ref().map(|b| b.pending_mint.clone()));
    // The checkpoint of the last check and its age.
    assert!(!r.last_checked.stale && r.last_checked.caught_up);
    assert_eq!(r.last_checked.checkpoint, Some(m.checkpoint));
    assert_eq!(r.last_checked.age_ns, Some(env::now_ns() - m.at_ns));
    assert!(r.checks.iter().all(|c| matches!(c.outcome, CheckOutcomeView::Ok)), "{:?}", r.checks);
    // It goes stale; the disclosures stay.
    test_env::advance_secs(20 * 60);
    let later = crate::views::audit_report(env::now_ns());
    assert!(later.last_checked.stale);
    assert!(later.disclosures.iter().any(|d| d.contains("D6")) && later.disclosures.iter().any(|d| d.contains("not compared with a build")));
    let _ = pending;
    // The observer view agrees.
    let ov = crate::views::observer_view();
    assert_eq!((ov.returns_verified_through, ov.next_sequence, ov.caught_up, ov.candidates.len()), (1, 2, true, 0));
    let _ = observer::IDLE_INTERVAL_NS;
}

#[test]
fn open_release_is_reached_only_from_the_observer_and_the_verified_return_is_built_only_there() {
    // The only production caller of `open_release` is the observer's `open_ready_returns`, and the only
    // place a `VerifiedReturn` is constructed outside tests is the observer (events and hints never pay).
    let production: [(&str, &str); 9] = [
        ("flows/mint.rs", include_str!("flows/mint.rs")),
        ("flows/remint.rs", include_str!("flows/remint.rs")),
        ("flows/deposit.rs", include_str!("flows/deposit.rs")),
        ("flows/ledger_ops.rs", include_str!("flows/ledger_ops.rs")),
        ("lib.rs", include_str!("lib.rs")),
        ("timer.rs", include_str!("timer.rs")),
        ("admin.rs", include_str!("admin.rs")),
        ("bind.rs", include_str!("bind.rs")),
        ("monitor.rs", include_str!("monitor.rs")),
    ];
    for (name, src) in production {
        let code: String = src.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        assert!(!code.contains("open_release("), "{name} opens a release");
        assert!(!code.contains("VerifiedReturn {"), "{name} builds a verified return");
    }
    let obs = include_str!("flows/observer.rs");
    assert_eq!(obs.matches("release::open_release(&v)").count(), 1);
    assert_eq!(obs.matches("VerifiedReturn {").count(), 1);
    // And that one construction reads the stored, record-verified `ReadyReturn`, never an event.
    assert!(obs.contains("let r = cand.ready.clone().expect(\"position() found a ready candidate\");"));
}
