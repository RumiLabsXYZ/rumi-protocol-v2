//! Tests of 49c: the strict finalized return observer, the mint saga with re-mints, and the C1 and
//! C2 monitors, against the simulated chain (`MockChain`), signer and icUSD ledger.

use crate::accounting::Transition;
use crate::env::{self, test_env};
use crate::flows::mint_saga;
use crate::flows::release;
use crate::memo::ReturnMemo;
use crate::monitor;
use crate::observer::{self, ObserverEvent, MAX_TX_PER_PASS, OBSERVER_IDLE_INTERVAL_NS, OBSERVER_PAGE};
use crate::records::{OpState, Sig};
use crate::saga_tests::{self, acct, assert_covered, buckets, deposit_of, escrowed, user, world, E8};
use crate::sol_rpc::{TokenBalanceEntry, TxMeta};
use crate::state::{self, AuditCtx, DepositKind, DepositState, ReleaseState, ReturnState};
use crate::testkit::*;
use crate::tx::builders::{build_return_message, ReturnRequest, TxProfile};
use crate::tx::{assemble_wire_tx, Message, Pubkey, Signature};
use candid::Principal;

pub(crate) const HOLDER: Pubkey = [0xa0; 32];
pub(crate) const SRC: Pubkey = [0xa1; 32];

pub(crate) fn profile() -> TxProfile {
    state::read_core(|c| c.tx_profile()).expect("bound profile")
}

pub(crate) fn ata() -> Pubkey {
    profile().return_ata().expect("return ata")
}

pub(crate) fn memo(nonce: u64) -> ReturnMemo {
    ReturnMemo { destination: Principal::from_slice(&[0x41; 10]), subaccount: None, client_nonce: nonce }
}

/// A holder's return transaction with a made-up signature `[tag; 64]` (the mock never verifies it).
pub(crate) fn return_wire(holder: &Pubkey, source: &Pubkey, amount: u64, memo: ReturnMemo, tag: u8) -> (Vec<u8>, Message) {
    let unsigned = build_return_message(
        &profile(),
        &ReturnRequest { holder: *holder, source_token_account: *source, amount, memo, recent_blockhash: [tag; 32], compute_budget: vec![] },
    )
    .expect("return message");
    let wire = assemble_wire_tx(&[[tag; 64]], &unsigned.message_bytes).expect("wire");
    (wire, unsigned.message)
}

pub(crate) fn entry(msg: &Message, account: &Pubkey, owner: Option<Pubkey>, amount: u64) -> TokenBalanceEntry {
    let account_index = msg.account_keys.iter().position(|k| k == account).expect("account in message") as u8;
    TokenBalanceEntry { account_index, mint: MINT, owner, amount, decimals: 8 }
}

/// What the RPC reports for a clean return: the source down by `amount`, the return account up.
pub(crate) fn good_meta(msg: &Message, holder: &Pubkey, source: &Pubkey, amount: u64, src_pre: u64, ata_pre: u64) -> TxMeta {
    TxMeta {
        inner_instruction_groups: Some(0),
        pre_token_balances: Some(vec![entry(msg, source, Some(*holder), src_pre), entry(msg, &ata(), Some(RETURN_OWNER), ata_pre)]),
        post_token_balances: Some(vec![entry(msg, source, Some(*holder), src_pre - amount), entry(msg, &ata(), Some(RETURN_OWNER), ata_pre + amount)]),
    }
}

pub(crate) fn land_valid_return(fx: &Fixture, tag: u8, amount: u64) -> Signature {
    let (wire, msg) = return_wire(&HOLDER, &SRC, amount, memo(tag as u64), tag);
    fx.chain.land_with_meta(&wire, false, good_meta(&msg, &HOLDER, &SRC, amount, 10 * amount, 0), &[ata()])
}

/// A fixture with a bound mint and outstanding supply for returns to draw on.
pub(crate) fn observer_world() -> Fixture {
    let fx = world();
    // Outstanding supply for returns to draw on, with the escrow that backs it.
    state::mutate_core(|c| c.buckets.outstanding = (1_000 * E8) as u128);
    fx.ledger.credit(&saga_tests::escrow(), (1_000 * E8) as u128);
    test_env::set_now_ns(secs(10_000));
    fx
}

pub(crate) fn observe(fx: &Fixture) -> Vec<ObserverEvent> {
    block_on(observer::observe(&fx.chain, env::now_ns()))
}

pub(crate) fn credited(events: &[ObserverEvent]) -> usize {
    events.iter().filter(|e| matches!(e, ObserverEvent::Credited { .. })).count()
}

pub(crate) fn strays(events: &[ObserverEvent]) -> Vec<String> {
    events.iter().filter_map(|e| if let ObserverEvent::Stray { reason, .. } = e { Some(reason.clone()) } else { None }).collect()
}

// ---------------------------------------------------------------------------
// The strict return check
// ---------------------------------------------------------------------------

#[test]
fn a_valid_finalized_return_is_credited_once_and_recorded_with_its_verified_sender() {
    let fx = observer_world();
    let amount = 5 * E8;
    let sig = land_valid_return(&fx, 1, amount);
    let before = buckets();
    let events = observe(&fx);
    assert_eq!(credited(&events), 1, "{events:?}");
    let b = buckets();
    assert_eq!((b.outstanding, b.return_custodied), (before.outstanding - amount as u128, amount as u128), "O -> RC");
    assert_eq!(b.stray, 0);
    let return_id = crate::ids::return_id(&state::read_core(|c| c.config.genesis_hash), &sig);
    let r = state::get_return(&return_id).expect("recorded");
    assert_eq!((r.signature, r.sender, r.source_account, r.amount), (Sig(sig), HOLDER, SRC, amount));
    assert_eq!(r.destination.owner, Principal::from_slice(&[0x41; 10]));
    assert_eq!((r.client_nonce, r.state.clone(), r.seq), (1, ReturnState::Custodied, 0));
    // The cursor moved last, to the newest signature; nothing is credited again.
    assert_eq!(state::read_core(|c| c.observer.cursor), Some(Sig(sig)));
    assert!(state::read_core(|c| c.observer.caught_up));
    test_env::advance_secs(OBSERVER_IDLE_INTERVAL_NS / 1_000_000_000 + 1);
    assert_eq!(credited(&observe(&fx)), 0);
    assert_eq!(buckets(), b);
}

#[test]
fn listing_again_after_a_lost_cursor_credits_nothing_twice() {
    let fx = observer_world();
    land_valid_return(&fx, 1, E8);
    observe(&fx);
    let b = buckets();
    // A cursor lost (or the provider's history re-listed): the same signature comes back.
    state::mutate_core(|c| c.observer.cursor = None);
    test_env::advance_secs(OBSERVER_IDLE_INTERVAL_NS / 1_000_000_000 + 1);
    let events = observe(&fx);
    assert!(events.iter().any(|e| matches!(e, ObserverEvent::Ignored { why: "already recorded", .. })), "{events:?}");
    assert_eq!(buckets(), b);
    assert_eq!(state::active_return_count(), 1);
}

#[test]
fn a_transfer_that_fails_any_check_is_a_stray_never_credited() {
    let amount = E8;
    type Tweak = fn(&Message, &mut TxMeta);
    let cases: Vec<(&str, Tweak)> = vec![
        ("the owner is not reported", |_m, meta| {
            for e in meta.pre_token_balances.as_mut().unwrap() {
                e.owner = None;
            }
        }),
        ("a delegate signs: the authority is not the owner", |_m, meta| {
            for e in meta.pre_token_balances.as_mut().unwrap() {
                if e.owner == Some(HOLDER) {
                    e.owner = Some([0xee; 32]);
                }
            }
        }),
        ("inner instructions are reported", |_m, meta| meta.inner_instruction_groups = Some(1)),
        ("inner instructions are not reported", |_m, meta| meta.inner_instruction_groups = None),
        ("a third account of the mint changed", |m, meta| {
            let extra = TokenBalanceEntry { account_index: m.account_keys.len() as u8 - 1, mint: MINT, owner: None, amount: 7, decimals: 8 };
            meta.pre_token_balances.as_mut().unwrap().push(extra.clone());
            meta.post_token_balances.as_mut().unwrap().push(TokenBalanceEntry { amount: 9, ..extra });
        }),
        ("the source fell by less than the amount", |_m, meta| {
            for e in meta.post_token_balances.as_mut().unwrap() {
                if e.owner == Some(HOLDER) {
                    e.amount += 1;
                }
            }
        }),
    ];
    for (name, tweak) in cases {
        let fx = observer_world();
        let (wire, msg) = return_wire(&HOLDER, &SRC, amount, memo(1), 7);
        let mut meta = good_meta(&msg, &HOLDER, &SRC, amount, 10 * amount, 0);
        tweak(&msg, &mut meta);
        fx.chain.land_with_meta(&wire, false, meta, &[ata()]);
        let o_before = buckets().outstanding;
        let events = observe(&fx);
        assert_eq!(credited(&events), 0, "{name}: {events:?}");
        assert_eq!(strays(&events).len(), 1, "{name}: {events:?}");
        let b = buckets();
        assert_eq!((b.return_custodied, b.stray, b.outstanding), (0, amount as u128, o_before), "{name}: a stray stays in O");
        assert_eq!(state::active_return_count(), 0, "{name}");
        assert_eq!(state::stray_count(), 1, "{name}");
    }
}

#[test]
fn a_memo_that_does_not_parse_or_names_the_bridge_is_a_stray() {
    let fx = observer_world();
    // A memo naming the bridge itself fails the shape check (a return credited to the bridge).
    let bridge_memo = ReturnMemo { destination: env::canister_id(), subaccount: None, client_nonce: 1 };
    let (wire, msg) = return_wire(&HOLDER, &SRC, E8, bridge_memo, 9);
    fx.chain.land_with_meta(&wire, false, good_meta(&msg, &HOLDER, &SRC, E8, 10 * E8, 0), &[ata()]);
    let events = observe(&fx);
    assert_eq!(credited(&events), 0);
    assert!(strays(&events)[0].contains("exact shape"), "{events:?}");
    assert_eq!(buckets().stray, E8 as u128);
}

#[test]
fn a_stray_keeps_the_single_source_and_the_signing_owner_for_a_later_refund() {
    let fx = observer_world();
    let (wire, msg) = return_wire(&HOLDER, &SRC, E8, ReturnMemo { destination: env::canister_id(), subaccount: None, client_nonce: 1 }, 3);
    fx.chain.land_with_meta(&wire, false, good_meta(&msg, &HOLDER, &SRC, E8, 5 * E8, 0), &[ata()]);
    observe(&fx);
    let s = state::list_strays(None, 10).pop().expect("a stray");
    assert_eq!((s.amount, s.source_account, s.verified_sender), (E8, Some(SRC), Some(HOLDER)), "{s:?}");
    // An owner the RPC did not report is no verified sender.
    let fx = observer_world();
    let (wire, msg) = return_wire(&HOLDER, &SRC, E8, ReturnMemo { destination: env::canister_id(), subaccount: None, client_nonce: 1 }, 4);
    let mut meta = good_meta(&msg, &HOLDER, &SRC, E8, 5 * E8, 0);
    meta.pre_token_balances.as_mut().unwrap()[0].owner = None;
    fx.chain.land_with_meta(&wire, false, meta, &[ata()]);
    observe(&fx);
    let s = state::list_strays(None, 10).pop().expect("a stray");
    assert_eq!((s.source_account, s.verified_sender), (Some(SRC), None), "{s:?}");
}

#[test]
fn a_return_above_the_per_operation_cap_is_a_stray() {
    let fx = observer_world();
    state::mutate_core(|c| c.config.limits.max_deposit_e8s = 2 * E8);
    land_valid_return(&fx, 1, 3 * E8);
    let events = observe(&fx);
    assert_eq!(credited(&events), 0);
    assert!(strays(&events)[0].contains("outside"), "{events:?}");
}

#[test]
fn unmeasurable_transactions_are_tracked_without_moving_any_bucket() {
    let fx = observer_world();
    let (wire, _msg) = return_wire(&HOLDER, &SRC, E8, memo(1), 5);
    // The RPC recorded no token balances: whatever it moved cannot be measured. It is tracked at
    // amount zero and any real change shows up as custody the books cannot explain (49d).
    fx.chain.land_with_meta(&wire, false, TxMeta::default(), &[ata()]);
    let before = buckets();
    let events = observe(&fx);
    assert_eq!(strays(&events).len(), 1, "{events:?}");
    assert_eq!(buckets(), before);
    assert_eq!(state::list_strays(None, 10)[0].amount, 0);
}

#[test]
fn failed_and_outgoing_and_unchanged_transactions_credit_nothing() {
    let fx = observer_world();
    // Failed: never queued.
    let (wire, msg) = return_wire(&HOLDER, &SRC, E8, memo(1), 1);
    fx.chain.land_with_meta(&wire, true, good_meta(&msg, &HOLDER, &SRC, E8, 5 * E8, 0), &[ata()]);
    // The bridge's own burn or refund: the return account fell.
    let (wire, msg) = return_wire(&HOLDER, &SRC, E8, memo(2), 2);
    let mut out = good_meta(&msg, &HOLDER, &SRC, E8, 5 * E8, 3 * E8);
    for e in out.pre_token_balances.as_mut().unwrap() {
        if e.owner == Some(RETURN_OWNER) {
            e.amount = 3 * E8;
        }
    }
    for e in out.post_token_balances.as_mut().unwrap() {
        if e.owner == Some(RETURN_OWNER) {
            e.amount = E8;
        }
    }
    fx.chain.land_with_meta(&wire, false, out, &[ata()]);
    // Unchanged (the creation of the account, a memo-only mention).
    let (wire, msg) = return_wire(&HOLDER, &SRC, E8, memo(3), 3);
    let mut same = good_meta(&msg, &HOLDER, &SRC, E8, 5 * E8, 0);
    same.post_token_balances = same.pre_token_balances.clone();
    fx.chain.land_with_meta(&wire, false, same, &[ata()]);
    let before = buckets();
    let events = observe(&fx);
    assert_eq!((credited(&events), strays(&events).len()), (0, 0), "{events:?}");
    assert_eq!(buckets(), before);
    assert_eq!((state::active_return_count(), state::stray_count()), (0, 0));
}

// ---------------------------------------------------------------------------
// Discovery: order, paging, failures, hints, cadence
// ---------------------------------------------------------------------------

#[test]
fn many_returns_are_credited_oldest_first_in_bounded_passes() {
    let fx = observer_world();
    let n = 130usize;
    let sigs: Vec<Signature> = (1..=n as u8).map(|t| land_valid_return(&fx, t, E8)).collect();
    let mut passes = 0;
    let mut max_fetches = 0;
    while !state::read_core(|c| c.observer.caught_up) || passes == 0 {
        passes += 1;
        assert!(passes < 40, "the observer does not converge");
        let (tx_before, list_before) = (fx.chain.call_count("get_transaction"), fx.chain.call_count("get_signatures_for_address"));
        observe(&fx);
        max_fetches = max_fetches.max(fx.chain.call_count("get_transaction") - tx_before);
        assert!(fx.chain.call_count("get_signatures_for_address") - list_before <= observer::MAX_PAGES_PER_PASS as u32);
    }
    assert!(max_fetches as usize <= MAX_TX_PER_PASS, "a pass fetched {max_fetches} transactions");
    assert_eq!(state::active_return_count(), n as u64);
    // Credited in landing order: the sequence follows the slots.
    let mut all = state::list_returns(None, 100);
    all.extend(state::list_returns(Some(all.last().unwrap().seq), 100));
    all.sort_by_key(|r| r.seq);
    assert_eq!(all.iter().map(|r| r.signature).collect::<Vec<_>>(), sigs.iter().map(|s| Sig(*s)).collect::<Vec<_>>());
    assert_eq!(all.iter().map(|r| r.seq).collect::<Vec<_>>(), (0..n as u64).collect::<Vec<_>>());
    assert_eq!(state::read_core(|c| c.observer.cursor), Some(Sig(*sigs.last().unwrap())));
    assert_eq!(buckets().return_custodied, (n as u64 * E8) as u128);
    assert!(n as u32 > 2 * OBSERVER_PAGE, "the walk needed more than one pass of pages");
}

#[test]
fn an_rpc_failure_never_advances_the_cursor_or_loses_a_transaction() {
    let fx = observer_world();
    let sig = land_valid_return(&fx, 1, E8);
    // The listing fails: nothing changes at all.
    fx.chain.fail_next("get_signatures_for_address", Fault::Error("provider down".into()));
    let events = observe(&fx);
    assert!(events.iter().any(|e| matches!(e, ObserverEvent::Waiting { .. })), "{events:?}");
    assert_eq!((state::read_core(|c| c.observer.cursor), state::observer_queue_len(), buckets().return_custodied), (None, 0, 0));
    // The transaction cannot be fetched (an error, then a consistent null): it stays queued.
    fx.chain.fail_next("get_transaction", Fault::Error("timeout".into()));
    let events = observe(&fx);
    assert!(events.iter().any(|e| matches!(e, ObserverEvent::Waiting { .. })), "{events:?}");
    assert_eq!((state::read_core(|c| c.observer.cursor), state::observer_queue_len()), (None, 1), "queued, cursor not moved");
    fx.chain.fail_next("get_transaction", Fault::Null);
    assert!(observe(&fx).iter().any(|e| matches!(e, ObserverEvent::Waiting { .. })));
    assert_eq!(state::observer_queue_len(), 1);
    // It works again: credited exactly once, and only now does the cursor move.
    let events = observe(&fx);
    assert_eq!(credited(&events), 1, "{events:?}");
    assert_eq!((state::read_core(|c| c.observer.cursor), state::observer_queue_len()), (Some(Sig(sig)), 0));
}

#[test]
fn a_return_waits_for_a_mint_not_yet_applied_and_halts_only_for_an_unexplained_excess() {
    let fx = observer_world();
    state::mutate_core(|c| c.buckets.outstanding = 0);
    land_valid_return(&fx, 1, 5 * E8);
    // The books know of no outstanding supply, but a mint of exactly that size is in flight: wait.
    let id = escrowed(&fx, 0x21, 5 * E8, 1);
    let mut d = deposit_of(id);
    d.state = DepositState::MintInFlight;
    d.mint_op = Some(0);
    state::put_deposit(&d);
    let events = observe(&fx);
    assert!(events.iter().any(|e| matches!(e, ObserverEvent::Waiting { why } if why.contains("in-flight mint"))), "{events:?}");
    assert!(!state::is_halted());
    assert_eq!(buckets().return_custodied, 0);
    // The mint is applied (the books catch up): the very same return is now credited.
    state::mutate_core(|c| {
        c.buckets.pending_mint = 0;
        c.buckets.outstanding = (5 * E8) as u128;
    });
    assert_eq!(credited(&observe(&fx)), 1);
    // A return nothing explains halts the bridge.
    let fx = observer_world();
    state::mutate_core(|c| c.buckets.outstanding = E8 as u128);
    land_valid_return(&fx, 2, 5 * E8);
    observe(&fx);
    assert!(state::is_halted(), "a return above everything the books know of is a supply anomaly");
    assert_eq!(buckets().return_custodied, 0);
}

#[test]
fn a_hint_has_the_named_transaction_processed_at_once_and_credits_it_once() {
    let fx = observer_world();
    let sig = land_valid_return(&fx, 1, E8);
    test_env::set_caller(user(0x21));
    release::record_hint(crate::tx::signature_to_base58(&sig)).unwrap();
    let events = observe(&fx);
    assert_eq!(credited(&events), 1, "{events:?}");
    assert!(events.iter().any(|e| matches!(e, ObserverEvent::Ignored { why: "already recorded", .. })), "the walk found it again: {events:?}");
    assert_eq!(buckets().return_custodied, E8 as u128);
    assert!(release::live_hints(env::now_ns()).is_empty(), "the hint served its purpose");
    // A hint for a transaction that is no return changes nothing and is dropped.
    let (wire, msg) = return_wire(&HOLDER, &SRC, E8, memo(2), 2);
    let mut same = good_meta(&msg, &HOLDER, &SRC, E8, 5 * E8, 0);
    same.post_token_balances = same.pre_token_balances.clone();
    let noise = fx.chain.land_with_meta(&wire, false, same, &[ata()]);
    release::record_hint(crate::tx::signature_to_base58(&noise)).unwrap();
    let before = buckets();
    observe(&fx);
    assert_eq!(buckets(), before);
    assert!(release::live_hints(env::now_ns()).is_empty());
}

#[test]
fn an_idle_observer_lists_the_return_account_only_every_idle_interval() {
    let fx = observer_world();
    observe(&fx);
    assert_eq!(fx.chain.call_count("get_signatures_for_address"), 1);
    // Within the idle interval: no RPC at all.
    test_env::advance_secs(60);
    assert!(observe(&fx).is_empty());
    assert_eq!(fx.chain.call_count("get_signatures_for_address"), 1);
    // After it: one listing.
    test_env::advance_secs(OBSERVER_IDLE_INTERVAL_NS / 1_000_000_000);
    observe(&fx);
    assert_eq!(fx.chain.call_count("get_signatures_for_address"), 2);
    // Paused or halted: nothing, whatever is due.
    test_env::advance_secs(OBSERVER_IDLE_INTERVAL_NS / 1_000_000_000 + 1);
    state::mutate_core(|c| c.config.pauses.observer = true);
    assert!(observe(&fx).is_empty());
    state::mutate_core(|c| c.config.pauses.observer = false);
    state::halt("test halt");
    assert!(observe(&fx).is_empty());
    assert_eq!(fx.chain.call_count("get_signatures_for_address"), 2);
}

// ---------------------------------------------------------------------------
// The mint saga
// ---------------------------------------------------------------------------

pub(crate) fn tick_mints(fx: &Fixture) {
    block_on(mint_saga::process_mints(&fx.chain, &fx.signer, env::now_ns()));
}

#[test]
fn an_escrowed_deposit_gets_exactly_one_mint_operation_keyed_by_its_mint_id() {
    let fx = saga_tests::world();
    let id = escrowed(&fx, 0x21, E8, 1);
    let d = deposit_of(id);
    tick_mints(&fx);
    let d2 = deposit_of(id);
    assert_eq!(d2.state, DepositState::MintInFlight);
    let op_id = d2.mint_op.expect("linked");
    let op = state::get_op(op_id).unwrap();
    assert_eq!((op.external_id, op.amount, op.kind), (d.mint_id.unwrap(), d.mint_amount_e8s, crate::records::OpKind::Mint));
    assert!(matches!(op.payload, crate::records::OpPayload::Mint { recipient_wallet, .. } if recipient_wallet == d.solana_recipient));
    // Idempotent: another pass creates nothing.
    let next = state::read_core(|c| c.next_op_id);
    tick_mints(&fx);
    assert_eq!(state::read_core(|c| c.next_op_id), next);
    assert_eq!(deposit_of(id).mint_op, Some(op_id));
}

#[test]
fn a_crash_between_creating_the_operation_and_linking_it_finds_the_same_operation() {
    let fx = saga_tests::world();
    let id = escrowed(&fx, 0x21, E8, 1);
    let d = deposit_of(id);
    // The operation was committed (as `submit_op` does before signing) and the link was lost.
    let op_id = fx
        .submit(
            crate::lane::SubmitRequest {
                kind: crate::records::OpKind::Mint,
                payload: crate::records::OpPayload::Mint { recipient_wallet: d.solana_recipient, per_operation_cap: 1_000 * E8 },
                external_id: d.mint_id.unwrap(),
                amount: d.mint_amount_e8s,
            },
            env::now_ns(),
        )
        .unwrap();
    let ops_before = state::read_core(|c| c.next_op_id);
    tick_mints(&fx);
    assert_eq!(deposit_of(id).mint_op, Some(op_id), "linked to the SAME operation");
    assert_eq!(state::read_core(|c| c.next_op_id), ops_before, "no second operation");
}

#[test]
fn a_delivered_mint_moves_pending_mint_to_outstanding_once_and_carves_the_fee() {
    let fx = saga_tests::world();
    state::mutate_core(|c| c.config.fees.deposit_fee_e8s = 1_000_000);
    test_env::set_caller(user(0x21));
    saga_tests::fund(&fx, &acct(user(0x21)), E8);
    let mut a = saga_tests::args(E8, 1);
    a.expected_deposit_fee_e8s = 1_000_000;
    let id = saga_tests::bridge(&fx, a).unwrap().id;
    tick_mints(&fx);
    let op_id = deposit_of(id).mint_op.unwrap();
    // Not delivered yet: the books do not move.
    let before = buckets();
    tick_mints(&fx);
    assert_eq!(buckets(), before);
    // The original lands; the lane adopts it; the saga applies it.
    assert!(fx.chain.land(&fx.wire_of(op_id, 0), false));
    fx.tick(env::now_ns() + secs(60));
    assert!(matches!(state::get_op(op_id).unwrap().state, OpState::Delivered { .. }));
    assert_eq!(buckets(), before, "delivery is not applied until the saga runs");
    tick_mints(&fx);
    let b = buckets();
    assert_eq!((b.pending_mint, b.outstanding, b.fees), (0, (E8 - 1_000_000) as u128, 1_000_000));
    let d = deposit_of(id);
    assert_eq!(d.state, DepositState::Delivered);
    assert!(d.delivered.is_some());
    assert!(state::get_op(op_id).unwrap().outcome_applied);
    // Applied once: further passes change nothing, and even a deposit wrongly put back in flight
    // cannot apply the same delivery again (the operation carries the mark).
    tick_mints(&fx);
    tick_mints(&fx);
    assert_eq!(buckets(), b);
    let mut again = deposit_of(id);
    again.state = DepositState::MintInFlight;
    state::put_deposit(&again);
    tick_mints(&fx);
    assert_eq!(buckets(), b);
    assert!(!state::is_halted());
    assert_covered(&fx);
}

#[test]
fn the_saga_never_builds_while_paused_halted_or_the_lane_is_halted() {
    let fx = saga_tests::world();
    let id = escrowed(&fx, 0x21, E8, 1);
    state::mutate_core(|c| c.config.pauses.mints = true);
    tick_mints(&fx);
    assert!(deposit_of(id).mint_op.is_none());
    state::mutate_core(|c| c.config.pauses.mints = false);
    state::halt_lane(crate::records::Lane::Mint, "test");
    tick_mints(&fx);
    assert!(deposit_of(id).mint_op.is_none());
    assert!(matches!(deposit_of(id).state, DepositState::Escrowed), "it simply waits");
    state::clear_lane_halt(crate::records::Lane::Mint);
    state::halt("test halt");
    tick_mints(&fx);
    assert!(deposit_of(id).mint_op.is_none());
    state::mutate_core(|c| c.halted = None);
    tick_mints(&fx);
    assert!(deposit_of(id).mint_op.is_some());
}

#[test]
fn an_unbuildable_request_holds_the_deposit_without_any_operation() {
    let fx = saga_tests::world();
    let id = escrowed(&fx, 0x21, 2 * E8, 1);
    // A deposit the cap would refuse (the limit was lowered after admission).
    let mut d = deposit_of(id);
    d.mint_amount_e8s = d.amount_e8s;
    state::put_deposit(&d);
    state::mutate_core(|c| c.config.limits.max_deposit_e8s = E8);
    // (A non-re-mint deposit's cap is the configured maximum.)
    tick_mints(&fx);
    let d = deposit_of(id);
    assert!(matches!(d.state, DepositState::MintHeld { .. }), "{:?}", d.state);
    assert_eq!(d.mint_op, None);
    assert_eq!(state::active_op_count(), 0);
}

#[test]
fn more_deposits_than_free_nonce_slots_simply_wait_for_one() {
    let fx = saga_tests::world();
    let ids: Vec<u64> = (0..3u8).map(|i| escrowed(&fx, 0x30 + i, E8, 1)).collect();
    tick_mints(&fx);
    let linked: Vec<bool> = ids.iter().map(|id| deposit_of(*id).mint_op.is_some()).collect();
    assert_eq!(linked.iter().filter(|b| **b).count(), 2, "the pool has two mint slots: {linked:?}");
    let waiting = ids.iter().find(|id| deposit_of(**id).mint_op.is_none()).copied().unwrap();
    assert!(matches!(deposit_of(waiting).state, DepositState::Escrowed), "no hold for a transient shortage");
    // One mint is delivered and its slot is freed: the waiting deposit gets its operation.
    let first = ids.iter().find(|id| deposit_of(**id).mint_op.is_some()).copied().unwrap();
    let op = deposit_of(first).mint_op.unwrap();
    assert!(fx.chain.land(&fx.wire_of(op, 0), false));
    fx.tick(env::now_ns() + secs(60));
    tick_mints(&fx);
    assert!(deposit_of(waiting).mint_op.is_some());
}

#[test]
fn a_held_or_abandoned_mint_is_never_built_again() {
    let fx = saga_tests::world();
    let id = escrowed(&fx, 0x21, E8, 1);
    tick_mints(&fx);
    let op_id = deposit_of(id).mint_op.unwrap();
    let mut op = state::get_op(op_id).unwrap();
    op.state = OpState::Held { reason: "x".into() };
    state::put_op(&op);
    let next = state::read_core(|c| c.next_op_id);
    tick_mints(&fx);
    tick_mints(&fx);
    assert_eq!(state::read_core(|c| c.next_op_id), next);
    assert_eq!(deposit_of(id).mint_op, Some(op_id));
    // An abandoned deposit is not escrowed and carries the abandoned id: not resubmitted either.
    let mut d = deposit_of(id);
    d.mint_op = None;
    d.abandoned_mint_ops = vec![op_id];
    d.state = DepositState::Escrowed;
    state::put_deposit(&d);
    tick_mints(&fx);
    assert_eq!(state::read_core(|c| c.next_op_id), next, "a deposit with an abandoned operation is refund-only");
    assert!(deposit_of(id).mint_op.is_none());
}

#[test]
fn a_held_release_is_re_minted_to_its_solana_sender_with_the_remint_id_and_settles_the_claim() {
    let fx = saga_tests::world();
    // A burned return whose destination cannot be paid (the ledger's minting account): held.
    let minting = state::read_core(|c| c.ledger_minting_account.unwrap());
    let b = saga_tests::burned_with(&fx, 5 * E8, 1, minting, HOLDER);
    release::open_release(&b).unwrap();
    let r = state::get_release(&b.return_id).unwrap();
    assert!(matches!(r.state, ReleaseState::Held { .. }));
    let held_before = buckets().held;
    assert_eq!(mint_saga::start_remints(), 1);
    let r = state::get_release(&b.return_id).unwrap();
    let ReleaseState::Reminting { deposit_id } = r.state else { panic!("{:?}", r.state) };
    let d = deposit_of(deposit_id);
    assert_eq!(d.kind, DepositKind::Remint { return_id: b.return_id });
    assert_eq!((d.solana_recipient, d.amount_e8s, d.mint_amount_e8s, d.deposit_fee_e8s), (HOLDER, 5 * E8, 5 * E8, 0));
    let (genesis, salt) = state::read_core(|c| (c.config.genesis_hash, c.instance_salt));
    assert_eq!(d.mint_id, Some(crate::ids::remint_id(&genesis, &MINT, &salt, &b.return_id)));
    let bk = buckets();
    assert_eq!((bk.held, bk.pending_mint), (held_before - (5 * E8) as u128, (5 * E8) as u128), "H -> PM");
    // Started once only.
    assert_eq!(mint_saga::start_remints(), 0);
    // It mints like any deposit, and the delivery settles the claim.
    tick_mints(&fx);
    let op_id = deposit_of(deposit_id).mint_op.expect("operation");
    assert!(fx.chain.land(&fx.wire_of(op_id, 0), false));
    fx.tick(env::now_ns() + secs(60));
    tick_mints(&fx);
    assert_eq!(deposit_of(deposit_id).state, DepositState::Delivered);
    assert_eq!(state::get_release(&b.return_id).unwrap().state, ReleaseState::Reminted { deposit_id });
    let bk = buckets();
    assert_eq!((bk.pending_mint, bk.held), (0, held_before - (5 * E8) as u128));
    assert_eq!(bk.outstanding, (5 * E8) as u128);
}

#[test]
fn at_most_two_re_mints_are_active_and_an_unmintable_sender_stays_held() {
    let fx = saga_tests::world();
    let mut held = vec![];
    for tag in 1..=4u8 {
        let minting = state::read_core(|c| c.ledger_minting_account.unwrap());
        let b = saga_tests::burned_with(&fx, E8, tag, minting, if tag == 1 { MINT } else { [0xc0 + tag; 32] });
        release::open_release(&b).unwrap();
        held.push(b);
    }
    // The first sender is the mint itself: not a wallet the bridge can mint to.
    assert_eq!(mint_saga::start_remints(), 2);
    assert!(matches!(state::get_release(&held[0].return_id).unwrap().state, ReleaseState::Held { .. }), "stays held for a human");
    assert_eq!(state::active_remint_count(), 2);
    assert_eq!(mint_saga::start_remints(), 0, "the cap holds until one settles");
}

// ---------------------------------------------------------------------------
// The monitors
// ---------------------------------------------------------------------------

pub(crate) fn run_monitors(fx: &Fixture) -> monitor::MonitorReport {
    block_on(monitor::run(&fx.chain, &fx.ledger, env::now_ns()))
}

pub(crate) fn set_supply(fx: &Fixture, supply: u64) {
    let tp = state::read_core(|c| c.config.token_program);
    fx.chain.set_account(MINT, tp.id(), crate::mint::fixtures::extended_mint(crate::mint::fixtures::mint_data(Some(&MINT_AUTHORITY), supply, 8, true, None), &[]));
}

#[test]
fn c1_passes_when_the_escrow_covers_what_is_owed_and_halts_only_on_a_confirmed_deficit() {
    let fx = saga_tests::world();
    let id = escrowed(&fx, 0x21, E8, 1);
    set_supply(&fx, 0);
    let r = run_monitors(&fx);
    assert_eq!((r.c1, r.c2, r.halted), (Some(true), Some(true), false), "{r:?}");
    assert_covered(&fx);
    // A deficit: funds vanish from the escrow. The first reading only warns.
    let escrow = saga_tests::escrow();
    let balance = fx.ledger.balance(&escrow);
    fx.ledger.debit(&escrow, balance / 2);
    let r = run_monitors(&fx);
    assert_eq!((r.c1, r.halted), (Some(false), false), "{r:?}");
    assert_eq!(state::read_core(|c| c.monitor.c1_strikes), 1);
    assert!(!state::is_halted());
    // It recovers: the strike is cleared.
    fx.ledger.credit(&escrow, balance / 2);
    assert_eq!(run_monitors(&fx).c1, Some(true));
    assert_eq!(state::read_core(|c| c.monitor.c1_strikes), 0);
    // Two consecutive violating readings halt the bridge.
    fx.ledger.debit(&escrow, balance / 2);
    run_monitors(&fx);
    let r = run_monitors(&fx);
    assert!(r.halted && state::is_halted(), "{r:?}");
    let _ = id;
}

#[test]
fn an_escrow_outflow_in_doubt_is_not_a_deficit() {
    let fx = saga_tests::world();
    let id = saga_tests::held_deposit(&fx, 0x21, E8, 1);
    set_supply(&fx, 0);
    fx.ledger.push_fault(LedgerFault::ExecuteThenLoseReply);
    test_env::set_caller(user(0x21));
    let v = crate::testkit::block_on(crate::flows::deposit::request_refund(&fx.ledger, id)).unwrap();
    assert_eq!(v.status, crate::types::DepositStatus::Refunding);
    // The escrow already paid; the outflow is reserved as unknown, so coverage still holds.
    let r = run_monitors(&fx);
    assert_eq!(r.c1, Some(true), "{r:?}");
}

#[test]
fn c2_bounds_the_supply_by_outstanding_in_flight_custodied_and_burn_unknown() {
    let fx = observer_world();
    set_supply(&fx, 900 * E8);
    let r = run_monitors(&fx);
    assert_eq!(r.c2, Some(true), "{r:?}");
    // Supply above everything the books can explain: the first reading warns, the second halts.
    set_supply(&fx, 1_001 * E8);
    let r = run_monitors(&fx);
    assert_eq!((r.c2, r.halted), (Some(false), false), "{r:?}");
    assert!(run_monitors(&fx).halted);
    assert!(state::is_halted());
}

#[test]
fn c2_counts_an_operation_in_flight_and_custodied_returns() {
    let fx = saga_tests::world();
    let id = escrowed(&fx, 0x21, E8, 1);
    tick_mints(&fx);
    assert!(deposit_of(id).mint_op.is_some());
    // The mint landed on chain but the books have not applied it: supply E8 is explained by the
    // in-flight operation.
    set_supply(&fx, E8);
    assert_eq!(run_monitors(&fx).c2, Some(true));
    // Tokens in custody (RC) and burn-unknown (BU) still count: they are supply until the burn is final.
    let fx = observer_world();
    state::mutate_core(|c| {
        c.buckets.outstanding = 0;
        c.buckets.return_custodied = (3 * E8) as u128;
        c.buckets.burn_unknown = (2 * E8) as u128;
    });
    set_supply(&fx, 5 * E8);
    assert_eq!(run_monitors(&fx).c2, Some(true));
    set_supply(&fx, 5 * E8 + 1);
    assert_eq!(run_monitors(&fx).c2, Some(false));
}

#[test]
fn a_legitimate_supply_decrease_is_reported_and_never_acted_on() {
    let fx = observer_world();
    // Quiescent, caught up, and supply below O: a holder burned. Reported as unattributed.
    state::mutate_core(|c| {
        c.observer.caught_up = true;
        c.buckets.outstanding = (100 * E8) as u128;
    });
    set_supply(&fx, 90 * E8);
    let r = run_monitors(&fx);
    assert_eq!((r.c2, r.halted), (Some(true), false), "{r:?}");
    let reading = state::read_core(|c| c.monitor.supply.clone()).unwrap();
    assert_eq!((reading.quiescent, reading.unattributed_gap), (true, Some((10 * E8) as u128)));
    let before = buckets();
    run_monitors(&fx);
    assert_eq!(buckets(), before, "nothing credits, sweeps or refunds on a gap");
    assert!(!state::is_halted());
}

#[test]
fn a_burn_applied_while_the_supply_was_read_is_not_a_violation() {
    let fx = observer_world();
    // Supply 100 = O 40 + RC 0 + BU 60 before; the burn becomes final and is applied while the read is
    // in flight (BU -> PR). The read value is above the bound AFTER, within the bound BEFORE.
    state::mutate_core(|c| {
        c.buckets.outstanding = (40 * E8) as u128;
        c.buckets.burn_unknown = (60 * E8) as u128;
    });
    set_supply(&fx, 100 * E8);
    fx.chain.on_account_read.replace(Some(Box::new(|_chain, _addr| {
        state::apply_transition(Transition::BurnFinalized { amount: 60 * E8 }, AuditCtx::note("applied during the read")).unwrap();
    })));
    let r = run_monitors(&fx);
    assert_eq!((r.c2, r.halted), (Some(true), false), "{r:?}");
    assert_eq!(state::read_core(|c| c.monitor.c2_strikes), 0);
}

#[test]
fn the_monitors_run_on_their_interval_and_only_with_a_bound_mint() {
    let fx = saga_tests::world();
    set_supply(&fx, 0);
    let first = block_on(monitor::run_if_due(&fx.chain, &fx.ledger, env::now_ns()));
    assert_eq!(first.c1, Some(true));
    let reads = fx.chain.call_count("get_account");
    let again = block_on(monitor::run_if_due(&fx.chain, &fx.ledger, env::now_ns() + 1_000));
    assert_eq!(again, monitor::MonitorReport::default());
    assert_eq!(fx.chain.call_count("get_account"), reads, "not due: no read");
    let later = block_on(monitor::run_if_due(&fx.chain, &fx.ledger, env::now_ns() + monitor::MONITOR_INTERVAL_NS + 1));
    assert_eq!(later.c1, Some(true));
    // An unbound mint is not monitored.
    state::mutate_core(|c| c.mint_state = crate::records::MintState::DeployedUnbound);
    let r = block_on(monitor::run(&fx.chain, &fx.ledger, env::now_ns()));
    assert_eq!((r.c1, r.c2), (None, None));
}

// ---------------------------------------------------------------------------
// The whole pass
// ---------------------------------------------------------------------------

#[test]
fn deposit_mint_return_in_one_timer_pass_sequence() {
    // ICP -> Solana: pull, mint operation, delivery. Solana -> ICP: the holder's return, observed,
    // custodied. Every step runs through the timer pass, in its order.
    let fx = saga_tests::world();
    fx.chain.tip_slot.set(2_000);
    set_supply(&fx, 0);
    let id = escrowed(&fx, 0x21, 5 * E8, 1);
    let run = |at: u64| block_on(crate::timer::run_once(&fx.chain, &fx.signer, &fx.ledger, at));
    let now = env::now_ns();
    run(now + secs(1));
    let op_id = deposit_of(id).mint_op.expect("the pass built the mint operation");
    assert!(fx.chain.land(&fx.wire_of(op_id, 0), false));
    set_supply(&fx, 5 * E8);
    run(now + secs(60));
    assert_eq!(deposit_of(id).state, DepositState::Delivered);
    assert_eq!(buckets().outstanding, (5 * E8) as u128);
    // The holder returns 2 icUSD.
    let (wire, msg) = return_wire(&HOLDER, &SRC, 2 * E8, memo(1), 1);
    let sig = fx.chain.land_with_meta(&wire, false, good_meta(&msg, &HOLDER, &SRC, 2 * E8, 5 * E8, 0), &[ata()]);
    set_supply(&fx, 5 * E8);
    // The idle observer looks again after its interval.
    let report = run(now + secs(400));
    assert_eq!(credited(&report.observer), 1, "{:?}", report.observer);
    // Credited, and the burn lane (the same pass) has already built the burn: RC -> BU.
    let b = buckets();
    assert_eq!((b.outstanding, b.return_custodied + b.burn_unknown), ((3 * E8) as u128, (2 * E8) as u128));
    let return_id = crate::ids::return_id(&state::read_core(|c| c.config.genesis_hash), &sig);
    assert!(state::get_return(&return_id).is_some());
    // The monitors read in the same pass family and agree.
    let m = run(now + secs(400) + monitor::MONITOR_INTERVAL_NS + 1).monitor;
    assert_eq!((m.c1, m.c2, m.halted), (Some(true), Some(true), false), "{m:?}");
    assert_covered(&fx);
}
