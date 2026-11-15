//! Tests of the 53a ledger saga against the simulated icUSD ledger (`testkit::MockLedger`), which
//! replays the ledger's deduplication window, fee check, block log and ICRC-3 search. They pin the
//! guarantees the Base and Solana instances give (exact tuple stored before the first send,
//! same-tuple resend inside the window, bounded archive proof afterwards, caller and allowance
//! binding, disjoint fees) and the Sui-specific ones (a refund only without a live mint operation,
//! a release only from a verified final return, the saved ICP destination immutable).
//!
//! Ported from the frozen Solana suite (`saga_tests.rs` at 4394b472), test by test where the
//! semantics fit.

use crate::accounting::{check_coverage, Transition};
use crate::env::{self, test_env};
use crate::flows::ledger_ops::{self, OpRef, SearchBasis, SentAttempt, DEDUP_SAFE_WINDOW_NS};
use crate::flows::release::{OpenError, OpenOutcome, VerifiedReturn};
use crate::flows::{deposit, release};
use crate::ids;
use crate::ledger::{CallOutcome, LedgerRejection, SearchOutcome};
use crate::state::{self, AuditCtx, DepositKind, DepositRecord, DepositState, LedgerOp, LedgerOpKind, LedgerOpStatus, ReleaseState};
use crate::sui::format_address;
use crate::testkit::*;
use crate::types::*;
use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;
use std::cell::RefCell;
use std::rc::Rc;

pub(crate) const FEE: u64 = 10_000;
pub(crate) const E8: u64 = 100_000_000;
const HOUR: u64 = 3_600;
pub(crate) const RECIPIENT: [u8; 32] = [0x77; 32];
/// The test role addresses, from the role keys the fixture stores.
const TREASURY_KEY: [u8; 32] = [0x11; 32];

pub(crate) fn user(n: u8) -> Principal {
    Principal::from_slice(&[n; 10])
}

pub(crate) fn admin() -> Principal {
    Principal::from_slice(&[3; 10])
}

pub(crate) fn acct(p: Principal) -> Account {
    Account { owner: p, subaccount: None }
}

pub(crate) fn spender() -> Account {
    acct(env::canister_id())
}

pub(crate) fn escrow() -> Account {
    state::read_runtime(|c| c.escrow_account(env::canister_id()))
}

pub(crate) fn wallet() -> String {
    format_address(&RECIPIENT)
}

/// A configured instance (gate binding, role keys, deposits enabled, ledger facts known) and a
/// funded caller.
pub(crate) fn world() -> Fixture {
    let fx = Fixture::new();
    state::mutate_runtime(|c| {
        c.config.pauses.deposits = false;
        c.ledger_fee_e8s = Some(FEE);
        c.ledger_minting_account = Some(Account { owner: Principal::from_slice(&[0x4d; 10]), subaccount: None });
    });
    fx.ledger.fee.set(FEE);
    test_env::set_caller(user(0x21));
    test_env::set_controllers(vec![user(0x99)]);
    fx
}

pub(crate) fn args(amount: u64, req: u8) -> BridgeToSuiArgs {
    BridgeToSuiArgs {
        amount_e8s: amount,
        sui_recipient: wallet(),
        from_subaccount: None,
        client_request_id: serde_bytes::ByteBuf::from(vec![req; 16]),
        expected_deposit_fee_e8s: 0,
    }
}

/// Give `who` a balance and an allowance to the bridge, enough for `amount` plus the ledger fee.
pub(crate) fn fund(fx: &Fixture, who: &Account, amount: u64) {
    fx.ledger.credit(who, (amount + FEE) as u128);
    fx.ledger.approve(who, &spender(), (amount + FEE) as u128, None);
}

pub(crate) fn bridge(fx: &Fixture, a: BridgeToSuiArgs) -> BridgeResult<DepositView> {
    block_on(deposit::bridge_to_sui(&fx.ledger, a))
}

pub(crate) fn deposit_of(id: u64) -> DepositRecord {
    state::get_deposit(id).expect("deposit")
}

pub(crate) fn buckets() -> crate::accounting::Buckets {
    state::read_runtime(|c| c.buckets)
}

/// C1 against the simulated ledger: the escrow balance covers every liability not in doubt.
pub(crate) fn assert_covered(fx: &Fixture) {
    let report = check_coverage(&buckets(), fx.ledger.balance(&escrow())).expect("coverage");
    assert!(report.ok, "escrow does not cover liabilities: {report:?}");
}

/// A deposit that was pulled into escrow (state `Escrowed`, backing in `pending_mint`).
pub(crate) fn escrowed(fx: &Fixture, who: u8, amount: u64, req: u8) -> u64 {
    test_env::set_caller(user(who));
    fund(fx, &acct(user(who)), amount);
    let v = bridge(fx, args(amount, req)).expect("admitted");
    assert_eq!(deposit_of(v.id).state, DepositState::Escrowed);
    v.id
}

/// Stand-in for the mint lane (53b): the mint was delivered.
pub(crate) fn deliver(id: u64) {
    let mut d = deposit_of(id);
    state::apply_transition(Transition::MintDelivered { amount: d.amount_e8s, fee: d.deposit_fee_e8s }, AuditCtx::deposit(id, "test delivery"))
        .expect("delivered");
    d.state = DepositState::Delivered;
    state::put_deposit(&d);
}

/// A deposit that is held with no mint operation (a preflight hold), so its owner may refund it.
pub(crate) fn held_deposit(fx: &Fixture, who: u8, amount: u64, req: u8) -> u64 {
    let id = escrowed(fx, who, amount, req);
    deposit::on_mint_preflight_hold(id, "mints are paused".into());
    assert!(matches!(deposit_of(id).state, DepositState::MintHeld { .. }));
    id
}

fn pull_tuples(fx: &Fixture) -> Vec<LedgerOp> {
    fx.ledger.sent.borrow().iter().filter(|o| o.kind == LedgerOpKind::Pull).cloned().collect()
}

// ---------------------------------------------------------------------------
// Admission, caller binding and the pull
// ---------------------------------------------------------------------------

#[test]
fn the_pull_moves_only_the_callers_own_funds_to_the_escrow() {
    let fx = world();
    let me = acct(user(0x21));
    fund(&fx, &me, E8);
    let v = bridge(&fx, args(E8, 1)).unwrap();
    assert_eq!(v.status, DepositStatus::Escrowed);
    let sent = fx.ledger.sent.borrow().clone();
    assert_eq!(sent.len(), 1);
    let op = &sent[0];
    assert_eq!(op.kind, LedgerOpKind::Pull);
    assert_eq!(op.from, me, "the source is the caller's own account");
    assert_eq!(op.spender, Some(spender()), "the spender is the bridge canister's default account");
    assert_eq!(op.to, escrow());
    assert_eq!((op.amount_e8s, op.fee_e8s), (E8, None));
    assert_eq!(op.memo, ids::pull_memo(0));
    assert_eq!(op.created_at_time, env::now_ns());
    // The ledger charged the caller amount plus its fee and spent that much allowance.
    assert_eq!(fx.ledger.balance(&me), 0);
    assert_eq!(fx.ledger.allowance_of(&me, &spender()), 0);
    assert_eq!(fx.ledger.balance(&escrow()), E8 as u128);
    assert_covered(&fx);
}

#[test]
fn a_subaccount_deposit_binds_to_that_subaccount_of_the_caller() {
    let fx = world();
    let from = Account { owner: user(0x21), subaccount: Some([5; 32]) };
    fund(&fx, &from, E8);
    let mut a = args(E8, 2);
    a.from_subaccount = Some(serde_bytes::ByteBuf::from(vec![5; 32]));
    let v = bridge(&fx, a).unwrap();
    assert_eq!(v.from, Some(from));
    assert_eq!(fx.ledger.sent.borrow()[0].from, from);
    // A wrong-length subaccount is refused before anything is stored.
    let mut bad = args(E8, 3);
    bad.from_subaccount = Some(serde_bytes::ByteBuf::from(vec![5; 31]));
    assert!(matches!(bridge(&fx, bad), Err(BridgeError::InvalidArgument(_))));
}

#[test]
fn another_principals_allowance_cannot_be_used_and_nothing_is_stored_without_funds() {
    let fx = world();
    // Alice approved the bridge; Bob did not. Bob cannot spend Alice's allowance.
    fund(&fx, &acct(user(0x31)), E8);
    test_env::set_caller(user(0x32));
    assert_eq!(
        bridge(&fx, args(E8, 1)),
        Err(BridgeError::InsufficientAllowance { allowance_e8s: 0, required_e8s: E8 + FEE })
    );
    assert_eq!(fx.ledger.sent_count(), 0, "no ledger call was made");
    assert_eq!((state::active_deposit_count(), state::read_runtime(|c| c.next_deposit_id)), (0, 0), "no record");
    assert_eq!(buckets().reserved_pulls, 0);
    // An allowance that covers the amount but not the fee is not enough; a balance gap is reported.
    let carol = acct(user(0x33));
    test_env::set_caller(user(0x33));
    fx.ledger.credit(&carol, (E8 + FEE) as u128);
    fx.ledger.approve(&carol, &spender(), E8 as u128, None);
    assert_eq!(bridge(&fx, args(E8, 1)), Err(BridgeError::InsufficientAllowance { allowance_e8s: E8, required_e8s: E8 + FEE }));
    let dan = acct(user(0x34));
    test_env::set_caller(user(0x34));
    fx.ledger.credit(&dan, E8 as u128);
    fx.ledger.approve(&dan, &spender(), (E8 + FEE) as u128, None);
    assert_eq!(bridge(&fx, args(E8, 1)), Err(BridgeError::InsufficientFunds { balance_e8s: E8, required_e8s: E8 + FEE }));
    assert_eq!(state::read_runtime(|c| c.next_deposit_id), 0);
    // An allowance about to expire counts as missing.
    let erin = acct(user(0x35));
    test_env::set_caller(user(0x35));
    fx.ledger.credit(&erin, (E8 + FEE) as u128);
    fx.ledger.approve(&erin, &spender(), (E8 + FEE) as u128, Some(env::now_ns() + 30 * 1_000_000_000));
    assert!(matches!(bridge(&fx, args(E8, 1)), Err(BridgeError::InsufficientAllowance { allowance_e8s: 0, .. })));
    // Anonymous callers are refused.
    test_env::set_caller(Principal::anonymous());
    assert_eq!(bridge(&fx, args(E8, 1)), Err(BridgeError::Anonymous));
}

#[test]
fn admission_enforces_state_recipient_amount_capacity_window_and_active_limits() {
    let fx = world();
    let me = acct(user(0x21));
    fund(&fx, &me, 5 * E8);
    let check = |a: BridgeToSuiArgs| validate_only(a);
    fn validate_only(a: BridgeToSuiArgs) -> BridgeResult<()> {
        deposit::validate_admission(user(0x21), &a)
    }
    // Recipient rules: not an address, the short form, uppercase, no prefix, the zero address, the system
    // range, the bridge's own objects and role addresses.
    let own = [
        format_address(&[0x70; 32]),
        format_address(&[0x6a; 32]),
        format_address(&[0x6d; 32]),
        format_address(&[0x6e; 32]),
        format_address(&crate::sui::address_from_ed25519(&TREASURY_KEY)),
        format_address(&crate::sui::address_from_ed25519(&[0x22; 32])),
        format_address(&crate::sui::address_from_ed25519(&[0x33; 32])),
    ];
    let mut system = [0u8; 32];
    system[31] = 2;
    let mut edge = [0u8; 32];
    edge[30] = 0xff;
    edge[31] = 0xff;
    let mut first_ok = [0u8; 32];
    first_ok[29] = 1;
    let mut bad: Vec<String> = vec![
        "not an address".into(),
        "0x2".into(),
        wallet().to_uppercase(),
        wallet()[2..].to_string(),
        format_address(&[0; 32]),
        format_address(&system),
        format_address(&edge),
    ];
    bad.extend(own);
    for b in &bad {
        let mut a = args(E8, 1);
        a.sui_recipient = b.clone();
        assert!(matches!(check(a), Err(BridgeError::InvalidRecipient(_))), "{b}");
    }
    // The first address above the reserved range is a normal wallet.
    let mut ok = args(E8, 1);
    ok.sui_recipient = format_address(&first_ok);
    assert_eq!(check(ok), Ok(()));
    // Amount bounds and the fee the caller was shown.
    assert_eq!(check(args(E8 - 1, 1)), Err(BridgeError::AmountBelowMinimum { min_e8s: E8 }));
    assert_eq!(check(args(1_001 * E8, 1)), Err(BridgeError::AmountAboveMaximum { max_e8s: 1_000 * E8 }));
    state::mutate_runtime(|c| c.config.fees.deposit_fee_e8s = 1_000);
    assert_eq!(check(args(E8, 1)), Err(BridgeError::FeeChanged { current_fee_e8s: 1_000 }));
    let mut with_fee = args(E8, 1);
    with_fee.expected_deposit_fee_e8s = 1_000;
    assert_eq!(check(with_fee), Ok(()));
    state::mutate_runtime(|c| c.config.fees.deposit_fee_e8s = 0);
    // Capacity counts destination exposure, held claims and reserved pulls.
    state::mutate_runtime(|c| c.config.limits.max_outstanding_e8s = 3 * E8);
    state::mutate_runtime(|c| c.buckets.outstanding = E8 as u128);
    state::mutate_runtime(|c| c.buckets.held = E8 as u128);
    assert_eq!(check(args(E8, 1)), Ok(()));
    assert_eq!(check(args(E8 + 1, 1)), Err(BridgeError::CapacityExceeded { available_e8s: E8 }));
    state::mutate_runtime(|c| c.buckets.reserved_pulls = 1);
    assert_eq!(check(args(E8, 1)), Err(BridgeError::CapacityExceeded { available_e8s: E8 - 1 }));
    state::mutate_runtime(|c| {
        c.buckets = Default::default();
        c.config.limits.max_outstanding_e8s = 10_000 * E8;
    });
    // Window.
    state::mutate_runtime(|c| c.config.limits.mint_window_limit_e8s = 2 * E8);
    state::mutate_runtime(|c| {
        c.windows.window_start_ns = env::now_ns();
        c.windows.admitted_in_window = (E8 + 1) as u128;
    });
    assert!(matches!(check(args(E8, 1)), Err(BridgeError::RateLimited { available_e8s, .. }) if available_e8s == E8 - 1));
    state::mutate_runtime(|c| c.windows = Default::default());
    // Pauses, binding and halts.
    state::mutate_runtime(|c| c.config.pauses.deposits = true);
    assert!(matches!(check(args(E8, 1)), Err(BridgeError::Paused(_))));
    state::mutate_runtime(|c| c.config.pauses.deposits = false);
    let binding = state::read_core(|c| c.binding.clone());
    state::mutate_core(|c| c.binding = None);
    assert!(matches!(check(args(E8, 1)), Err(BridgeError::NotConfigured(_))), "no gate binding takes no deposit");
    state::mutate_core(|c| c.binding = binding);
    let roles = state::read_core(|c| c.roles.clone());
    state::mutate_core(|c| c.roles.pop());
    assert!(matches!(check(args(E8, 1)), Err(BridgeError::NotConfigured(_))), "missing role keys take no deposit");
    state::mutate_core(|c| c.roles = roles);
    state::halt("test halt");
    assert!(matches!(check(args(E8, 1)), Err(BridgeError::Halted(_))));
}

#[test]
fn active_deposit_limits_count_per_caller_and_globally() {
    let fx = world();
    state::mutate_runtime(|c| {
        c.config.limits.max_active_per_caller = 2;
        c.config.limits.max_active_global = 3;
    });
    for req in 1..=2u8 {
        escrowed(&fx, 0x21, E8, req);
    }
    test_env::set_caller(user(0x21));
    assert_eq!(deposit::validate_admission(user(0x21), &args(E8, 9)), Err(BridgeError::TooManyActiveOperations), "per caller");
    escrowed(&fx, 0x22, E8, 1);
    assert_eq!(deposit::validate_admission(user(0x23), &args(E8, 9)), Err(BridgeError::TooManyActiveOperations), "global");
    // A re-mint does not count against users.
    let mut re = state::get_deposit(0).unwrap();
    re.id = 77;
    re.kind = DepositKind::Remint { return_id: [9; 32] };
    state::put_deposit(&re);
    assert_eq!(state::active_user_deposit_count(), 3);
}

#[test]
fn a_request_id_names_one_deposit_forever() {
    let fx = world();
    let me = acct(user(0x21));
    fund(&fx, &me, E8);
    let first = bridge(&fx, args(E8, 7)).unwrap();
    let again = bridge(&fx, args(E8, 7)).unwrap();
    assert_eq!(first, again);
    assert_eq!(fx.ledger.sent_count(), 1, "the repeat made no second pull");
    // Different arguments under the same id are a conflict, not a second deposit.
    let mut other = args(E8, 7);
    other.amount_e8s = 2 * E8;
    assert_eq!(bridge(&fx, other), Err(BridgeError::RequestIdConflict));
    let mut other = args(E8, 7);
    other.sui_recipient = format_address(&[0x88; 32]);
    assert_eq!(bridge(&fx, other), Err(BridgeError::RequestIdConflict));
    // The same id from another caller is another deposit.
    test_env::set_caller(user(0x22));
    fund(&fx, &acct(user(0x22)), E8);
    assert_ne!(bridge(&fx, args(E8, 7)).unwrap().id, first.id);
}

#[test]
fn the_tuple_is_stored_before_the_first_send_and_every_send_is_that_tuple() {
    let fx = world();
    fund(&fx, &acct(user(0x21)), E8);
    let seen: Rc<RefCell<Vec<(LedgerOp, LedgerOp, u128)>>> = Rc::default();
    let sink = seen.clone();
    *fx.ledger.on_execute.borrow_mut() = Some(Box::new(move |sent| {
        let stored = ledger_ops::load_op(&OpRef::Pull(0)).expect("stored before the call");
        sink.borrow_mut().push((stored, sent.clone(), state::read_runtime(|c| c.buckets.reserved_pulls)));
    }));
    fx.ledger.push_fault(LedgerFault::ExecuteThenLoseReply);
    let v = bridge(&fx, args(E8, 1)).unwrap();
    assert_eq!(v.status, DepositStatus::PullUnknown);
    block_on(deposit::reconcile_pulls(&fx.ledger));
    let seen = seen.borrow();
    assert_eq!(seen.len(), 2);
    for (stored, sent, reserved) in seen.iter() {
        assert!(stored.attempts >= 1, "the attempt was counted before the call");
        assert_eq!(tuple_of(stored), tuple_of(sent), "what was stored is what was sent");
        assert_eq!(*reserved, E8 as u128, "capacity was reserved before the call");
    }
    assert_eq!((seen[0].0.attempts, seen[1].0.attempts), (1, 2));
    assert_eq!(tuple_of(&seen[0].1), tuple_of(&seen[1].1), "the resend is the identical tuple");
    assert_eq!(deposit_of(0).state, DepositState::Escrowed);
    assert_eq!(fx.ledger.balance(&escrow()), E8 as u128, "the funds moved exactly once");
}

/// What the ledger deduplicates on.
type Tuple = (LedgerOpKind, Account, Account, Option<Account>, u64, Option<u64>, Vec<u8>, u64);

fn tuple_of(o: &LedgerOp) -> Tuple {
    (o.kind, o.from, o.to, o.spender, o.amount_e8s, o.fee_e8s, o.memo.clone(), o.created_at_time)
}

#[test]
fn a_proven_pull_escrows_the_funds_and_derives_the_mint_id_from_the_pull_block() {
    let fx = world();
    fx.ledger.preload_blocks(7);
    fund(&fx, &acct(user(0x21)), E8);
    let v = bridge(&fx, args(E8, 1)).unwrap();
    assert_eq!(v.pull_block_index, Some(7));
    let b = buckets();
    assert_eq!((b.pending_mint, b.reserved_pulls), (E8 as u128, 0));
    let d = deposit_of(0);
    let expected = state::read_core(|c| ids::mint_id(&c.chain_identifier, &[0x70; 32], &[0x6a; 32], &c.instance_salt, 7, &RECIPIENT, E8));
    assert_eq!(d.mint_id, Some(expected), "the mint id contains the escrow-pull block index");
    assert_eq!(v.mint_id, Some(hex::encode(expected)));
    assert_covered(&fx);
}

#[test]
fn a_lost_reply_is_resolved_by_resending_the_same_tuple_and_the_ledger_deduplicates() {
    let fx = world();
    fund(&fx, &acct(user(0x21)), E8);
    fx.ledger.push_fault(LedgerFault::ExecuteThenLoseReply);
    let v = bridge(&fx, args(E8, 1)).unwrap();
    assert_eq!(v.status, DepositStatus::PullUnknown);
    assert_eq!(buckets().reserved_pulls, E8 as u128, "still reserved: the outcome is not proven");
    assert_eq!(buckets().pending_mint, 0);
    test_env::advance_secs(200);
    block_on(deposit::reconcile_pulls(&fx.ledger));
    let sent = pull_tuples(&fx);
    assert_eq!(sent.len(), 2);
    assert_eq!((sent[0].from, sent[0].amount_e8s, sent[0].memo.clone(), sent[0].created_at_time),
               (sent[1].from, sent[1].amount_e8s, sent[1].memo.clone(), sent[1].created_at_time));
    assert_eq!(deposit_of(0).state, DepositState::Escrowed);
    assert_eq!(fx.ledger.blocks.borrow().len(), 1, "one transfer happened");
    assert_eq!(fx.ledger.balance(&escrow()), E8 as u128);
    assert_covered(&fx);
}

#[test]
fn a_retryable_no_effect_reply_resends_and_a_typed_failure_fails_only_a_clean_pull() {
    let fx = world();
    fund(&fx, &acct(user(0x21)), E8);
    fx.ledger.push_fault(LedgerFault::Reject(LedgerRejection::TemporarilyUnavailable));
    let v = bridge(&fx, args(E8, 1)).unwrap();
    assert_eq!(v.status, DepositStatus::PullUnknown, "visibly not final");
    test_env::advance_secs(5);
    block_on(deposit::reconcile_pulls(&fx.ledger));
    assert_eq!(deposit_of(0).state, DepositState::Escrowed);

    // A typed insufficient-funds reply on a clean first attempt proves nothing moved: the pull
    // fails, the reservation and the window are given back.
    fund(&fx, &acct(user(0x22)), E8);
    test_env::set_caller(user(0x22));
    fx.ledger.push_fault(LedgerFault::Reject(LedgerRejection::InsufficientFunds { balance: 0 }));
    let before_window = state::read_runtime(|c| c.windows.admitted_in_window);
    let v = bridge(&fx, args(E8, 1)).unwrap();
    assert!(matches!(v.status, DepositStatus::PullFailed { .. }), "{:?}", v.status);
    assert_eq!(buckets().reserved_pulls, 0);
    assert_eq!(state::read_runtime(|c| c.windows.admitted_in_window), before_window);

    // After an ambiguous attempt the same typed reply proves nothing: the pull stays unknown.
    fund(&fx, &acct(user(0x23)), E8);
    test_env::set_caller(user(0x23));
    fx.ledger.push_fault(LedgerFault::AmbiguousNoEffect);
    let v = bridge(&fx, args(E8, 1)).unwrap();
    assert_eq!(v.status, DepositStatus::PullUnknown);
    fx.ledger.push_fault(LedgerFault::Reject(LedgerRejection::InsufficientAllowance { allowance: 0 }));
    test_env::advance_secs(200);
    block_on(deposit::reconcile_pulls(&fx.ledger));
    assert_eq!(deposit_of(v.id).state, DepositState::PullUnknown, "a typed error after an ambiguous attempt is not proof");
    assert_eq!(buckets().reserved_pulls, E8 as u128, "capacity stays reserved");
}

#[test]
fn past_the_dedup_window_an_unknown_pull_is_searched_never_resent() {
    let fx = world();
    fund(&fx, &acct(user(0x21)), E8);
    fx.ledger.push_fault(LedgerFault::ExecuteThenLoseReply);
    bridge(&fx, args(E8, 1)).unwrap();
    assert_eq!(fx.ledger.sent_count(), 1);
    test_env::set_now_ns(deposit_of(0).created_at_ns + DEDUP_SAFE_WINDOW_NS + 1);
    block_on(deposit::reconcile_pulls(&fx.ledger));
    assert_eq!(fx.ledger.sent_count(), 1, "outside the safe window the tuple is not resent");
    assert_eq!(fx.ledger.search_calls.borrow().len(), 1);
    assert_eq!(deposit_of(0).state, DepositState::Escrowed, "the search found the executed pull");
    assert_covered(&fx);
}

#[test]
fn absence_is_proven_only_after_the_whole_window_and_the_whole_log_were_searched() {
    let fx = world();
    fx.ledger.preload_blocks(1_200);
    state::mutate_runtime(|c| c.ledger_tip_lower_bound = 0);
    fund(&fx, &acct(user(0x21)), E8);
    fx.ledger.push_fault(LedgerFault::AmbiguousNoEffect);
    bridge(&fx, args(E8, 1)).unwrap();
    let created = deposit_of(0).created_at_ns;
    // Inside the dedup window of the ledger: the scan reaches the tip but the tuple could still be
    // accepted, so nothing is proven.
    test_env::set_now_ns(created + DEDUP_SAFE_WINDOW_NS + 1);
    for _ in 0..3 {
        block_on(deposit::reconcile_pulls(&fx.ledger));
    }
    assert_eq!(deposit_of(0).state, DepositState::PullUnknown);
    assert_eq!(fx.ledger.sent_count(), 1);
    let calls = fx.ledger.search_calls.borrow().clone();
    assert_eq!(&calls[..3], &[(0, 500), (500, 500), (1_000, 500)], "bounded pages, resumed where the last stopped");
    // Once the window has closed, a complete scan proves the pull never happened.
    test_env::set_now_ns(created + 25 * HOUR * 1_000_000_000);
    block_on(deposit::reconcile_pulls(&fx.ledger));
    assert!(matches!(deposit_of(0).state, DepositState::PullFailed { .. }), "{:?}", deposit_of(0).state);
    assert_eq!(buckets().reserved_pulls, 0);
    assert_eq!(fx.ledger.sent_count(), 1, "still never resent");
}

#[test]
fn a_search_error_is_never_treated_as_absence() {
    let fx = world();
    fund(&fx, &acct(user(0x21)), E8);
    fx.ledger.push_fault(LedgerFault::AmbiguousNoEffect);
    bridge(&fx, args(E8, 1)).unwrap();
    *fx.ledger.search_error.borrow_mut() = Some("archive unavailable".into());
    test_env::set_now_ns(deposit_of(0).created_at_ns + 30 * HOUR * 1_000_000_000);
    block_on(deposit::reconcile_pulls(&fx.ledger));
    assert_eq!(deposit_of(0).state, DepositState::PullUnknown);
    assert_eq!(buckets().reserved_pulls, E8 as u128);
}

#[test]
fn a_late_reply_from_an_older_attempt_cannot_fail_the_pull() {
    let fx = world();
    fund(&fx, &acct(user(0x21)), E8);
    fx.ledger.push_fault(LedgerFault::AmbiguousNoEffect);
    bridge(&fx, args(E8, 1)).unwrap();
    let r = OpRef::Pull(0);
    // Two more attempts are started (the timer resent), then the first one's typed reply arrives.
    let mut op = ledger_ops::load_op(&r).unwrap();
    op.attempts += 1;
    ledger_ops::store_op(&r, op.clone());
    let newest = op.attempts;
    ledger_ops::apply_outcome(
        &r,
        SentAttempt { attempt: 1, outcome: CallOutcome::Rejected(LedgerRejection::InsufficientAllowance { allowance: 0 }) },
    );
    assert!(matches!(deposit_of(0).state, DepositState::PullPending | DepositState::PullUnknown));
    assert!(matches!(ledger_ops::load_op(&r).unwrap().status, LedgerOpStatus::Unknown { .. }));
    // The newest attempt proves it happened (a Duplicate of the first block).
    ledger_ops::apply_outcome(&r, SentAttempt { attempt: newest, outcome: CallOutcome::Accepted { block_index: 4 } });
    assert_eq!(deposit_of(0).state, DepositState::Escrowed);
    // And a success from an older attempt still counts.
    let fx = world();
    fund(&fx, &acct(user(0x21)), E8);
    fx.ledger.push_fault(LedgerFault::AmbiguousNoEffect);
    bridge(&fx, args(E8, 1)).unwrap();
    let mut op = ledger_ops::load_op(&r).unwrap();
    op.attempts += 1;
    ledger_ops::store_op(&r, op);
    ledger_ops::apply_outcome(&r, SentAttempt { attempt: 1, outcome: CallOutcome::Accepted { block_index: 9 } });
    assert_eq!(deposit_of(0).state, DepositState::Escrowed);
}

#[test]
fn classification_uses_the_op_as_stored_after_the_await() {
    let fx = world();
    fund(&fx, &acct(user(0x21)), E8);
    fx.ledger.push_fault(LedgerFault::Reject(LedgerRejection::TemporarilyUnavailable));
    bridge(&fx, args(E8, 1)).unwrap();
    let r = OpRef::Pull(0);
    let n = ledger_ops::load_op(&r).unwrap().attempts;
    // While the call was in flight something else marked the op ambiguous.
    let mut op = ledger_ops::load_op(&r).unwrap();
    op.ambiguous_seen = true;
    ledger_ops::store_op(&r, op);
    ledger_ops::apply_outcome(&r, SentAttempt { attempt: n, outcome: CallOutcome::Rejected(LedgerRejection::InsufficientFunds { balance: 0 }) });
    assert!(!matches!(deposit_of(0).state, DepositState::PullFailed { .. }), "a no-effect reply after ambiguity is not proof");
}

/// An old, unknown refund (so the search path applies) and the time it can be searched.
fn refunding_with_unknown_outcome(fx: &Fixture) -> (OpRef, u64) {
    let id = held_deposit(fx, 0x21, E8, 1);
    let mut d = deposit_of(id);
    state::apply_transition(Transition::RefundStarted { amount: d.amount_e8s }, AuditCtx::deposit(id, "refund")).unwrap();
    d.state = DepositState::Refunding;
    let t0 = env::now_ns();
    d.refund = Some(LedgerOp {
        kind: LedgerOpKind::Refund,
        from: escrow(),
        to: d.from.unwrap(),
        spender: None,
        amount_e8s: 90,
        fee_e8s: Some(10),
        memo: vec![7; 8],
        created_at_time: t0,
        attempts: 2,
        ambiguous_seen: true,
        status: LedgerOpStatus::Unknown { last_error: "timeout".into() },
        search_from_block: 5,
        search_next_block: Some(40),
        last_attempt_ns: Some(t0 + 1_000),
        outflow_reserved: false,
    });
    state::put_deposit(&d);
    (OpRef::Refund(id), t0 + 25 * HOUR * 1_000_000_000)
}

#[test]
fn proven_absent_rebuilds_an_unchanged_escrow_tuple_and_only_that() {
    let fx = world();
    let (r, now) = refunding_with_unknown_outcome(&fx);
    state::mutate_runtime(|c| c.ledger_tip_lower_bound = 77);
    let basis = SearchBasis::of(&ledger_ops::load_op(&r).unwrap());
    ledger_ops::apply_search_outcome(&r, &basis, SearchOutcome::ProvenAbsent, now);
    let op = ledger_ops::load_op(&r).unwrap();
    assert_eq!(op.created_at_time, now, "a fresh tuple");
    assert_eq!((op.attempts, op.ambiguous_seen, op.search_next_block), (0, false, None));
    assert_eq!(op.status, LedgerOpStatus::Pending);
    assert_eq!(op.search_from_block, 77, "the search window restarts at the known tip");
    assert_eq!((op.amount_e8s, op.fee_e8s), (90, Some(10)), "same amounts");
}

#[test]
fn an_absence_proof_is_dropped_when_the_op_changed_while_the_search_ran() {
    // A send while the search awaited.
    let fx = world();
    let (r, now) = refunding_with_unknown_outcome(&fx);
    let basis = SearchBasis::of(&ledger_ops::load_op(&r).unwrap());
    let mut op = ledger_ops::load_op(&r).unwrap();
    op.attempts += 1;
    op.last_attempt_ns = Some(now - 1);
    ledger_ops::store_op(&r, op.clone());
    ledger_ops::apply_search_outcome(&r, &basis, SearchOutcome::ProvenAbsent, now);
    assert_eq!(ledger_ops::load_op(&r).unwrap(), op, "the proof does not cover the newer send");
    // Any field of the tuple, or the search window, changing.
    for tweak in [
        (|o: &mut LedgerOp| o.created_at_time -= 5) as fn(&mut LedgerOp),
        |o: &mut LedgerOp| o.amount_e8s += 1,
        |o: &mut LedgerOp| o.fee_e8s = Some(11),
        |o: &mut LedgerOp| o.memo = vec![8; 8],
        |o: &mut LedgerOp| o.search_from_block += 1,
    ] {
        let fx = world();
        let (r, now) = refunding_with_unknown_outcome(&fx);
        let basis = SearchBasis::of(&ledger_ops::load_op(&r).unwrap());
        let mut op = ledger_ops::load_op(&r).unwrap();
        tweak(&mut op);
        ledger_ops::store_op(&r, op.clone());
        ledger_ops::apply_search_outcome(&r, &basis, SearchOutcome::ProvenAbsent, now);
        assert_eq!(ledger_ops::load_op(&r).unwrap(), op);
    }
    // The op left the searchable state (reset to unsent, or terminal).
    let fx = world();
    let (r, now) = refunding_with_unknown_outcome(&fx);
    let basis = SearchBasis::of(&ledger_ops::load_op(&r).unwrap());
    let mut op = ledger_ops::load_op(&r).unwrap();
    op.status = LedgerOpStatus::Pending;
    op.attempts = 0;
    ledger_ops::store_op(&r, op.clone());
    ledger_ops::apply_search_outcome(&r, &basis, SearchOutcome::ProvenAbsent, now);
    assert_eq!(ledger_ops::load_op(&r).unwrap(), op);
    let fx = world();
    let (r, now) = refunding_with_unknown_outcome(&fx);
    let basis = SearchBasis::of(&ledger_ops::load_op(&r).unwrap());
    let mut op = ledger_ops::load_op(&r).unwrap();
    op.status = LedgerOpStatus::Succeeded { block_index: 9 };
    ledger_ops::store_op(&r, op.clone());
    ledger_ops::apply_search_outcome(&r, &basis, SearchOutcome::ProvenAbsent, now);
    assert_eq!(ledger_ops::load_op(&r).unwrap(), op);
}

#[test]
fn search_progress_belongs_to_the_searched_tuple_and_a_found_block_always_counts() {
    let fx = world();
    let (r, now) = refunding_with_unknown_outcome(&fx);
    let basis = SearchBasis::of(&ledger_ops::load_op(&r).unwrap());
    ledger_ops::apply_search_outcome(&r, &basis, SearchOutcome::NotFoundYet { next_block: 60, tip: 100 }, now);
    assert_eq!(ledger_ops::load_op(&r).unwrap().search_next_block, Some(60));
    let mut op = ledger_ops::load_op(&r).unwrap();
    op.created_at_time -= 1;
    op.search_next_block = None;
    ledger_ops::store_op(&r, op.clone());
    ledger_ops::apply_search_outcome(&r, &basis, SearchOutcome::NotFoundYet { next_block: 90, tip: 100 }, now);
    assert_eq!(ledger_ops::load_op(&r).unwrap(), op, "the old position does not leak into a rebuilt tuple");
    // A found block is a fact about the ledger, whatever the op looks like now. The refund is
    // completed: accounting and parent move together.
    state::mutate_runtime(|c| c.buckets.unknown_outflow = 0);
    ledger_ops::apply_search_outcome(&r, &basis, SearchOutcome::Found { block_index: 12 }, now);
    assert_eq!(deposit_of(0).state, DepositState::Refunded);
    assert_eq!(buckets().pending_refund, 0);
}

#[test]
fn a_pull_proven_absent_fails_only_if_unchanged_since_the_search_began() {
    let fx = world();
    fund(&fx, &acct(user(0x21)), E8);
    fx.ledger.push_fault(LedgerFault::AmbiguousNoEffect);
    bridge(&fx, args(E8, 1)).unwrap();
    let r = OpRef::Pull(0);
    let basis = SearchBasis::of(&ledger_ops::load_op(&r).unwrap());
    // A third send races in: the pull must not be failed on the old proof.
    let mut op = ledger_ops::load_op(&r).unwrap();
    op.attempts += 1;
    op.last_attempt_ns = Some(env::now_ns() + 1);
    ledger_ops::store_op(&r, op);
    ledger_ops::apply_search_outcome(&r, &basis, SearchOutcome::ProvenAbsent, env::now_ns());
    assert!(!ledger_ops::is_terminal(&ledger_ops::load_op(&r).unwrap()));
    let basis = SearchBasis::of(&ledger_ops::load_op(&r).unwrap());
    ledger_ops::apply_search_outcome(&r, &basis, SearchOutcome::ProvenAbsent, env::now_ns());
    assert!(matches!(deposit_of(0).state, DepositState::PullFailed { .. }));
    assert_eq!(buckets().reserved_pulls, 0);
}

// ---------------------------------------------------------------------------
// Refunds
// ---------------------------------------------------------------------------

#[test]
fn only_a_held_deposit_with_no_live_mint_operation_can_be_refunded_and_only_by_its_owner() {
    let fx = world();
    let id = held_deposit(&fx, 0x21, E8, 1);
    // Wrong caller.
    test_env::set_caller(user(0x22));
    assert_eq!(block_on(deposit::request_refund(&fx.ledger, id)), Err(BridgeError::NotOwner));
    test_env::set_caller(user(0x21));
    assert_eq!(block_on(deposit::request_refund(&fx.ledger, 999)), Err(BridgeError::NotFound));
    // Every state that is not a held deposit without an operation is refused.
    for st in [
        DepositState::Escrowed,
        DepositState::MintInFlight,
        DepositState::Delivered,
        DepositState::Refunding,
        DepositState::Refunded,
        DepositState::PullUnknown,
    ] {
        let mut d = deposit_of(id);
        d.state = st.clone();
        state::put_deposit(&d);
        assert!(matches!(block_on(deposit::request_refund(&fx.ledger, id)), Err(BridgeError::InvalidState(_))), "{st:?}");
    }
    // Held, but a mint operation is attached: never refunded (it may yet deliver).
    let mut d = deposit_of(id);
    d.state = DepositState::MintHeld { reason: "x".into() };
    d.mint_op = Some(3);
    state::put_deposit(&d);
    assert!(matches!(block_on(deposit::request_refund(&fx.ledger, id)), Err(BridgeError::InvalidState(_))));
    // An abandoned mint operation cannot be judged without the mint lane's records (53b), so a deposit
    // that ever had one is not refundable: the proof fails closed.
    d.mint_op = None;
    d.abandoned_mint_ops = vec![3];
    state::put_deposit(&d);
    assert!(matches!(block_on(deposit::request_refund(&fx.ledger, id)), Err(BridgeError::InvalidState(_))));
    d.abandoned_mint_ops = vec![];
    state::put_deposit(&d);
    // A re-mint can never be refunded to ICP.
    let mut re = deposit_of(id);
    re.kind = DepositKind::Remint { return_id: [1; 32] };
    state::put_deposit(&re);
    assert!(matches!(block_on(deposit::request_refund(&fx.ledger, id)), Err(BridgeError::InvalidState(_))));
    assert_eq!(fx.ledger.sent.borrow().iter().filter(|o| o.kind == LedgerOpKind::Refund).count(), 0, "nothing was sent");
}

#[test]
fn a_refund_persists_its_tuple_and_reserves_the_outflow_before_it_sends() {
    let fx = world();
    let id = held_deposit(&fx, 0x21, E8, 1);
    let me = acct(user(0x21));
    type Seen = (LedgerOp, u128, DepositState, u128);
    let seen: Rc<RefCell<Vec<Seen>>> = Rc::default();
    let sink = seen.clone();
    *fx.ledger.on_execute.borrow_mut() = Some(Box::new(move |sent| {
        if sent.kind == LedgerOpKind::Refund {
            let d = state::get_deposit(0).unwrap();
            sink.borrow_mut().push((
                d.refund.clone().expect("stored before the call"),
                state::read_runtime(|c| c.buckets.unknown_outflow),
                d.state,
                state::read_runtime(|c| c.buckets.pending_refund),
            ));
        }
    }));
    let v = block_on(deposit::request_refund(&fx.ledger, id)).unwrap();
    assert_eq!(v.status, DepositStatus::Refunded);
    let seen = seen.borrow();
    assert_eq!(seen.len(), 1);
    let (stored, unknown, st, rf) = &seen[0];
    assert_eq!(*st, DepositState::Refunding);
    assert_eq!((stored.amount_e8s, stored.fee_e8s), (E8 - FEE, Some(FEE)));
    assert_eq!(stored.to, me, "the refund goes back to the account the pull came from");
    assert_eq!(stored.memo, ids::refund_ledger_memo(id));
    assert_eq!(*unknown, E8 as u128, "amount plus fee were in doubt before the send");
    assert_eq!(*rf, E8 as u128);
    let b = buckets();
    assert_eq!((b.pending_mint, b.pending_refund, b.unknown_outflow), (0, 0, 0));
    assert_eq!(fx.ledger.balance(&me), (E8 - FEE) as u128, "the refund is net of the ledger fee the escrow paid");
    assert_eq!(fx.ledger.balance(&escrow()), 0);
    assert_covered(&fx);
}

#[test]
fn a_lost_refund_reply_keeps_the_outflow_in_doubt_and_resends_the_same_tuple() {
    let fx = world();
    let id = held_deposit(&fx, 0x21, E8, 1);
    fx.ledger.push_fault(LedgerFault::ExecuteThenLoseReply);
    let v = block_on(deposit::request_refund(&fx.ledger, id)).unwrap();
    assert_eq!(v.status, DepositStatus::Refunding);
    assert_eq!(buckets().unknown_outflow, E8 as u128);
    // The escrow already paid, yet coverage holds because the outflow is in doubt, not ignored.
    assert_covered(&fx);
    test_env::advance_secs(200);
    block_on(deposit::process_refunds(&fx.ledger));
    let sent: Vec<_> = fx.ledger.sent.borrow().iter().filter(|o| o.kind == LedgerOpKind::Refund).cloned().collect();
    assert_eq!(sent.len(), 2);
    assert_eq!((sent[0].to, sent[0].amount_e8s, sent[0].fee_e8s, sent[0].memo.clone(), sent[0].created_at_time),
               (sent[1].to, sent[1].amount_e8s, sent[1].fee_e8s, sent[1].memo.clone(), sent[1].created_at_time));
    assert_eq!(deposit_of(id).state, DepositState::Refunded);
    assert_eq!(buckets().unknown_outflow, 0);
    assert_eq!(fx.ledger.blocks.borrow().iter().filter(|b| b.kind == LedgerOpKind::Refund).count(), 1, "refunded exactly once");
    assert_covered(&fx);
}

#[test]
fn a_refund_cannot_start_while_the_bridge_is_halted_or_the_deposit_cannot_cover_the_fee() {
    let fx = world();
    let id = held_deposit(&fx, 0x21, E8, 1);
    state::halt("test halt");
    assert!(matches!(block_on(deposit::request_refund(&fx.ledger, id)), Err(BridgeError::Halted(_))));
    let mut d = deposit_of(id);
    d.amount_e8s = FEE;
    state::put_deposit(&d);
    state::mutate_runtime(|c| c.halted = None);
    assert!(matches!(block_on(deposit::request_refund(&fx.ledger, id)), Err(BridgeError::InvalidState(_))));
}

// ---------------------------------------------------------------------------
// Fees stay disjoint, fee sweeps
// ---------------------------------------------------------------------------

#[test]
fn the_deposit_fee_the_release_fee_and_the_ledger_fee_are_three_separate_things() {
    let fx = world();
    state::mutate_runtime(|c| c.config.fees.deposit_fee_e8s = 1_000_000);
    test_env::set_caller(user(0x21));
    fund(&fx, &acct(user(0x21)), E8);
    let mut a = args(E8, 1);
    a.expected_deposit_fee_e8s = 1_000_000;
    let v = bridge(&fx, a).unwrap();
    assert_eq!((v.amount_e8s, v.deposit_fee_e8s, v.mint_amount_e8s), (E8, 1_000_000, E8 - 1_000_000));
    // The pull is the full amount: the deposit fee is not taken from it, and the ledger fee is the
    // depositor's, on top.
    assert_eq!(fx.ledger.sent.borrow()[0].amount_e8s, E8);
    assert_eq!(fx.ledger.balance(&escrow()), E8 as u128);
    let id = v.id;
    let expected_mint_id = state::read_core(|c| ids::mint_id(&c.chain_identifier, &[0x70; 32], &[0x6a; 32], &c.instance_salt, 0, &RECIPIENT, E8 - 1_000_000));
    assert_eq!(deposit_of(id).mint_id, Some(expected_mint_id), "the mint id commits to the minted amount");
    deliver(id);
    let b = buckets();
    assert_eq!((b.outstanding, b.fees, b.pending_mint), ((E8 - 1_000_000) as u128, 1_000_000, 0));
    assert_covered(&fx);
    // A sweep takes only from the fee bucket, and the escrow keeps backing everything else.
    state::mutate_runtime(|c| c.config.fee_recipient = Some(acct(user(0x77))));
    let sweep = block_on(deposit::sweep_fees(&fx.ledger, 400_000)).unwrap();
    assert_eq!(sweep, 0);
    let b = buckets();
    assert_eq!((b.fees, b.outstanding, b.unknown_outflow), (600_000, (E8 - 1_000_000) as u128, 0));
    assert_eq!(fx.ledger.balance(&acct(user(0x77))), (400_000 - FEE) as u128, "the ledger fee comes out of the sweep");
    assert_covered(&fx);
}

#[test]
fn a_fee_sweep_is_bounded_single_flight_and_resolved_like_every_escrow_transfer() {
    let fx = world();
    let id = escrowed(&fx, 0x21, E8, 1);
    state::mutate_runtime(|c| c.config.fees.deposit_fee_e8s = 0);
    deliver(id);
    state::mutate_runtime(|c| {
        c.buckets.fees = 500_000;
        c.buckets.outstanding -= 500_000;
    });
    fx.ledger.credit(&escrow(), 0);
    // No recipient, too much, too little.
    assert!(matches!(block_on(deposit::sweep_fees(&fx.ledger, 100_000)), Err(BridgeError::NotConfigured(_))));
    state::mutate_runtime(|c| c.config.fee_recipient = Some(acct(user(0x77))));
    assert!(matches!(block_on(deposit::sweep_fees(&fx.ledger, 500_001)), Err(BridgeError::InvalidArgument(_))));
    assert!(matches!(block_on(deposit::sweep_fees(&fx.ledger, FEE)), Err(BridgeError::InvalidArgument(_))));
    // A lost reply leaves it in flight; a second sweep is refused until it resolves.
    fx.ledger.push_fault(LedgerFault::ExecuteThenLoseReply);
    let sweep = block_on(deposit::sweep_fees(&fx.ledger, 200_000)).unwrap();
    assert_eq!(buckets().unknown_outflow, 200_000);
    assert!(matches!(block_on(deposit::sweep_fees(&fx.ledger, 100_000)), Err(BridgeError::InvalidState(_))));
    test_env::advance_secs(200);
    block_on(deposit::process_sweeps(&fx.ledger));
    let sc = state::read_runtime(|c| c.sweeps.clone());
    assert!(matches!(sc[sweep as usize].op.status, LedgerOpStatus::Succeeded { .. }));
    assert_eq!((buckets().fees, buckets().unknown_outflow), (300_000, 0));
    assert_eq!(fx.ledger.balance(&acct(user(0x77))), (200_000 - FEE) as u128, "paid once");
    assert_covered(&fx);
    // Now another sweep may start.
    assert!(block_on(deposit::sweep_fees(&fx.ledger, 100_000)).is_ok());
}

// ---------------------------------------------------------------------------
// Releases: only from a verified final return
// ---------------------------------------------------------------------------

pub(crate) const SENDER: [u8; 32] = [0x55; 32];

pub(crate) fn dest() -> Account {
    acct(user(0x41))
}

/// What the return observer (53c) hands over for a return the Gate holds: the Gate's own record
/// fields, with the destination as the exact bytes the Gate saved.
pub(crate) fn verified(tag: u8, amount: u64, destination: Account, sender: [u8; 32]) -> VerifiedReturn {
    VerifiedReturn {
        transfer_id: [tag; 32],
        gate_sequence: tag as u64,
        sui_sender: sender,
        client_nonce: 1_000 + tag as u64,
        amount_e8s: amount,
        icp_principal: destination.owner.as_slice().to_vec(),
        icp_subaccount: destination.subaccount.unwrap_or([0u8; 32]).to_vec(),
        tx_digest: [0xd0 + tag; 32],
        checkpoint: 700 + tag as u64,
    }
}

/// Make `amount` outstanding on Sui (the escrow funded, the mint delivered, all in `O`) and return
/// the verified return of it, not yet opened.
pub(crate) fn burned(fx: &Fixture, amount: u64, tag: u8) -> VerifiedReturn {
    burned_with(fx, amount, tag, dest(), SENDER)
}

/// As [`burned`], for a return from `sender` whose saved ICP account is `destination`.
pub(crate) fn burned_with(fx: &Fixture, amount: u64, tag: u8, destination: Account, sender: [u8; 32]) -> VerifiedReturn {
    fx.ledger.credit(&escrow(), amount as u128);
    state::apply_transitions(
        &[Transition::AdmitDeposit { amount }, Transition::PullSucceeded { amount }, Transition::MintDelivered { amount, fee: 0 }],
        AuditCtx::note("test lifecycle"),
    )
    .expect("lifecycle");
    verified(tag, amount, destination, sender)
}

#[test]
fn no_release_exists_without_a_verified_return_of_coin_the_books_hold_as_outstanding() {
    let fx = world();
    // The return names coin whose mint the books have not applied: the funds are still in `PM`.
    let id = escrowed(&fx, 0x21, 5 * E8, 1);
    assert_eq!(buckets().pending_mint, (5 * E8) as u128);
    let premature = verified(1, 5 * E8, dest(), SENDER);
    assert_eq!(release::open_release(&premature), Err(OpenError::NotYetOutstanding { outstanding: 0, amount: 5 * E8 }));
    block_on(release::process_releases(&fx.ledger));
    block_on(crate::timer::ledger_pass(&fx.ledger));
    assert!(fx.ledger.sent.borrow().iter().all(|o| o.kind != LedgerOpKind::Release), "no release transfer exists");
    assert_eq!(state::active_release_count(), 0);
    assert!(state::get_release(&premature.transfer_id).is_none());
    assert_eq!(buckets().pending_mint, (5 * E8) as u128, "a refused return moved nothing");
    // Once the mint delivery is applied the same return opens, and only then.
    deliver(id);
    let before = buckets();
    assert_eq!(release::open_release(&premature), Ok(OpenOutcome::Opened(ReleaseState::Pending)));
    let after = buckets();
    assert_eq!((after.outstanding, after.pending_release), (before.outstanding - (5 * E8) as u128, before.pending_release + (5 * E8) as u128), "O to PR");
    // The record carries the verified fields, not the caller's say-so about anything else.
    let r = state::get_release(&premature.transfer_id).unwrap();
    assert_eq!((r.gate_sequence, r.sui_sender, r.client_nonce, r.amount_e8s, r.seq), (1, SENDER, 1_001, 5 * E8, 0));
    assert_eq!((r.tx_digest, r.checkpoint, r.destination), ([0xd1; 32], 701, dest()));
    assert_eq!((r.icp_principal.clone(), r.icp_subaccount.clone()), (dest().owner.as_slice().to_vec(), vec![0u8; 32]));
    assert_eq!(r.state, ReleaseState::Pending);
    assert_covered(&fx);
}

#[test]
fn a_return_that_breaks_the_gates_own_bounds_is_rejected_and_changes_nothing() {
    let fx = world();
    let good = burned(&fx, 5 * E8, 1);
    let before = buckets();
    let cases: Vec<(&str, VerifiedReturn)> = vec![
        ("zero amount", VerifiedReturn { amount_e8s: 0, ..good.clone() }),
        ("zero sequence", VerifiedReturn { gate_sequence: 0, ..good.clone() }),
        ("empty principal", VerifiedReturn { icp_principal: vec![], ..good.clone() }),
        ("anonymous principal bytes", VerifiedReturn { icp_principal: vec![0x04], ..good.clone() }),
        ("principal over 29 bytes", VerifiedReturn { icp_principal: vec![7; 30], ..good.clone() }),
        ("short subaccount", VerifiedReturn { icp_subaccount: vec![0; 31], ..good.clone() }),
        ("long subaccount", VerifiedReturn { icp_subaccount: vec![0; 33], ..good.clone() }),
        ("empty subaccount", VerifiedReturn { icp_subaccount: vec![], ..good.clone() }),
    ];
    for (what, v) in cases {
        assert!(matches!(release::open_release(&v), Err(OpenError::Malformed(_))), "{what}");
    }
    assert_eq!(buckets(), before, "no refusal moved the books");
    assert_eq!(state::active_release_count(), 0);
    assert!(!state::is_halted(), "a malformed record is a failed verification, not a halt");
    // The ledger facts are needed to judge the destination and the fees: without them nothing moves.
    state::mutate_runtime(|c| c.ledger_fee_e8s = None);
    assert_eq!(release::open_release(&good), Err(OpenError::LedgerFactsUnknown));
    assert_eq!(buckets(), before);
    state::mutate_runtime(|c| c.ledger_fee_e8s = Some(FEE));
    assert_eq!(release::open_release(&good), Ok(OpenOutcome::Opened(ReleaseState::Pending)));
}

#[test]
fn the_saved_destination_is_kept_verbatim_including_a_subaccount() {
    let fx = world();
    let with_sub = Account { owner: user(0x42), subaccount: Some([9; 32]) };
    let a = burned_with(&fx, 5 * E8, 1, with_sub, SENDER);
    let zero_sub = Account { owner: user(0x43), subaccount: None };
    let b = burned_with(&fx, 5 * E8, 2, zero_sub, SENDER);
    release::open_release(&a).unwrap();
    release::open_release(&b).unwrap();
    let ra = state::get_release(&a.transfer_id).unwrap();
    let rb = state::get_release(&b.transfer_id).unwrap();
    assert_eq!(ra.destination, with_sub);
    assert_eq!(ra.icp_subaccount, vec![9u8; 32]);
    assert_eq!(rb.destination, zero_sub, "an all-zero saved subaccount is the default account");
    assert_eq!(rb.icp_subaccount, vec![0u8; 32], "and the exact saved bytes are kept");
    block_on(release::process_releases(&fx.ledger));
    assert_eq!(fx.ledger.balance(&with_sub), (5 * E8 - FEE) as u128);
    assert_eq!(fx.ledger.balance(&zero_sub), (5 * E8 - FEE) as u128);
    assert_eq!(fx.ledger.balance(&acct(user(0x42))), 0, "the subaccount destination is not the default account");
}

#[test]
fn a_release_must_be_funded_by_pending_release_on_top_of_those_already_owed() {
    let fx = world();
    let first = burned(&fx, 5 * E8, 1);
    assert_eq!(release::open_release(&first), Ok(OpenOutcome::Opened(ReleaseState::Pending)));
    assert_eq!(state::pending_release_total(), (5 * E8) as u128);
    // Corrupt the books so `PR` no longer covers what is already owed: a second release must not be
    // opened on top of an unfunded first one; the bridge halts instead.
    state::mutate_runtime(|c| c.buckets.pending_release = 0);
    let second = burned(&fx, 5 * E8, 2);
    assert!(matches!(release::open_release(&second), Err(OpenError::NotFunded { owed, .. }) if owed == (5 * E8) as u128));
    assert!(state::is_halted());
    assert!(state::get_release(&second.transfer_id).is_none(), "no record without funding");
    block_on(release::process_releases(&fx.ledger));
    assert_eq!(fx.ledger.sent_count(), 0, "a halted bridge pays nothing");
}

#[test]
fn opening_is_idempotent_on_the_return_id_and_one_return_makes_one_release() {
    let fx = world();
    let b = burned(&fx, 5 * E8, 1);
    assert_eq!(release::open_release(&b), Ok(OpenOutcome::Opened(ReleaseState::Pending)));
    let after_first = buckets();
    assert_eq!(release::open_release(&b), Ok(OpenOutcome::AlreadyOpen));
    assert_eq!(buckets(), after_first, "a repeat moves nothing");
    assert_eq!(state::read_runtime(|c| c.next_release_seq), 1);
    assert!(!state::is_halted());
    // The same return id with any other content is a conflict that halts the bridge, including a
    // different destination (the saved account is immutable) and a different transaction.
    let variants = [
        VerifiedReturn { amount_e8s: b.amount_e8s + 1, ..b.clone() },
        VerifiedReturn { icp_principal: user(0x44).as_slice().to_vec(), ..b.clone() },
        VerifiedReturn { icp_subaccount: vec![1; 32], ..b.clone() },
        VerifiedReturn { sui_sender: [0x56; 32], ..b.clone() },
        VerifiedReturn { tx_digest: [0xee; 32], ..b.clone() },
        VerifiedReturn { gate_sequence: 9, ..b.clone() },
    ];
    for other in variants {
        state::mutate_runtime(|c| c.halted = None);
        assert!(matches!(release::open_release(&other), Err(OpenError::Conflict(_))), "{other:?}");
        assert!(state::is_halted());
    }
    assert_eq!(buckets(), after_first, "no conflict moved the books");
    // The same Gate sequence under another transfer id is a conflict too.
    state::mutate_runtime(|c| c.halted = None);
    let twin = VerifiedReturn { transfer_id: [0x42; 32], ..b.clone() };
    assert!(matches!(release::open_release(&twin), Err(OpenError::Conflict(_))));
    assert!(state::is_halted());
    assert!(state::get_release(&[0x42; 32]).is_none());
    assert_eq!(state::active_release_count(), 1);
}

#[test]
fn releases_are_indexed_by_owner_sender_and_gate_sequence() {
    let fx = world();
    let a = burned_with(&fx, E8, 1, acct(user(0x41)), SENDER);
    let b = burned_with(&fx, E8, 2, acct(user(0x42)), SENDER);
    let c = burned_with(&fx, E8, 3, acct(user(0x41)), [0x66; 32]);
    for v in [&a, &b, &c] {
        release::open_release(v).unwrap();
    }
    assert_eq!(state::release_by_sequence(2), Some(b.transfer_id));
    assert_eq!(state::release_by_sequence(4), None);
    let ids_of = |rs: Vec<state::ReleaseRecord>| rs.iter().map(|r| r.return_id).collect::<Vec<_>>();
    // Listings are newest first and the cursor is the last sequence seen (older ones follow).
    assert_eq!(ids_of(state::list_owner_releases(&user(0x41), None, 10)), vec![c.transfer_id, a.transfer_id]);
    assert_eq!(ids_of(state::list_owner_releases(&user(0x41), Some(2), 10)), vec![a.transfer_id], "paging past the newest");
    assert!(state::list_owner_releases(&user(0x41), Some(0), 10).is_empty(), "nothing is older than the first");
    assert_eq!(ids_of(state::list_owner_releases(&user(0x41), None, 1)), vec![c.transfer_id], "the limit bounds the page");
    assert_eq!(ids_of(state::list_sender_releases(&SENDER, None, 10)), vec![b.transfer_id, a.transfer_id]);
    assert_eq!(ids_of(state::list_sender_releases(&[0x66; 32], None, 10)), vec![c.transfer_id]);
    assert!(state::list_owner_releases(&user(0x77), None, 10).is_empty());
}

#[test]
fn an_unpayable_destination_or_fee_gap_is_held_at_opening() {
    let fx = world();
    // The destination is the ledger's minting account (a transfer there would burn the funds).
    let minting = state::read_runtime(|c| c.ledger_minting_account.unwrap());
    let b = burned_with(&fx, 5 * E8, 1, minting, SENDER);
    let out = release::open_release(&b).unwrap();
    assert!(matches!(out, OpenOutcome::Opened(ReleaseState::Held { .. })), "{out:?}");
    let bk = buckets();
    assert_eq!((bk.pending_release, bk.held), (0, (5 * E8) as u128), "moved from PR to H");
    // The destination is this very canister.
    let me = burned_with(&fx, 5 * E8, 3, acct(env::canister_id()), SENDER);
    assert!(matches!(release::open_release(&me).unwrap(), OpenOutcome::Opened(ReleaseState::Held { .. })));
    // The amount does not cover the bridge fee plus the ledger fee.
    state::mutate_runtime(|c| c.config.fees.release_fee_e8s = 40_000);
    let small = burned(&fx, 50_000, 2);
    assert!(matches!(release::open_release(&small).unwrap(), OpenOutcome::Opened(ReleaseState::Held { .. })));
    assert_eq!(buckets().held, (10 * E8 + 50_000) as u128);
    assert_eq!(buckets().pending_release, 0);
    // A held release is never sent, and its destination is never replaced.
    block_on(release::process_releases(&fx.ledger));
    assert_eq!(fx.ledger.sent_count(), 0);
    assert_eq!(state::get_release(&b.transfer_id).unwrap().icp_principal, minting.owner.as_slice().to_vec());
    assert_covered(&fx);
}

#[test]
fn a_release_persists_its_tuple_first_pays_net_of_both_fees_and_completes_once() {
    let fx = world();
    state::mutate_runtime(|c| c.config.fees.release_fee_e8s = 1_000_000);
    let b = burned(&fx, 5 * E8, 1);
    release::open_release(&b).unwrap();
    let seen: Rc<RefCell<Vec<(LedgerOp, u128)>>> = Rc::default();
    let sink = seen.clone();
    *fx.ledger.on_execute.borrow_mut() = Some(Box::new(move |_| {
        let stored = ledger_ops::load_op(&OpRef::Release([1; 32])).expect("stored before the call");
        sink.borrow_mut().push((stored, state::read_runtime(|c| c.buckets.unknown_outflow)));
    }));
    block_on(release::process_releases(&fx.ledger));
    let r = state::get_release(&b.transfer_id).unwrap();
    assert_eq!(r.state, ReleaseState::Released);
    let seen = seen.borrow();
    assert_eq!(seen.len(), 1);
    let (stored, unknown) = &seen[0];
    assert_eq!(stored.to, dest());
    assert_eq!(stored.amount_e8s, 5 * E8 - 1_000_000 - FEE, "net of the release fee and the ledger fee");
    assert_eq!(stored.fee_e8s, Some(FEE));
    assert_eq!(stored.memo, b.transfer_id.to_vec(), "the memo is the Gate's transfer id");
    assert_eq!(*unknown, (5 * E8 - 1_000_000) as u128, "the outflow (amount plus ledger fee) was in doubt before the send");
    let bk = buckets();
    assert_eq!((bk.pending_release, bk.fees, bk.unknown_outflow), (0, 1_000_000, 0));
    assert_eq!(fx.ledger.balance(&dest()), (5 * E8 - 1_000_000 - FEE) as u128);
    assert_covered(&fx);
    // The pass again does nothing.
    block_on(release::process_releases(&fx.ledger));
    assert_eq!(fx.ledger.sent_count(), 1);
    assert_eq!(release::open_release(&b), Ok(OpenOutcome::AlreadyOpen), "a paid return cannot be opened again");
    block_on(release::process_releases(&fx.ledger));
    assert_eq!(fx.ledger.sent_count(), 1);
}

#[test]
fn a_lost_release_reply_resends_the_same_tuple_and_a_new_ledger_fee_rebuilds_it() {
    let fx = world();
    state::mutate_runtime(|c| c.config.fees.release_fee_e8s = 1_000_000);
    let b = burned(&fx, 5 * E8, 1);
    release::open_release(&b).unwrap();
    fx.ledger.push_fault(LedgerFault::ExecuteThenLoseReply);
    block_on(release::process_releases(&fx.ledger));
    assert_eq!(state::get_release(&b.transfer_id).unwrap().state, ReleaseState::Pending);
    assert_eq!(buckets().unknown_outflow, (5 * E8 - 1_000_000) as u128);
    assert_covered(&fx);
    test_env::advance_secs(200);
    block_on(release::process_releases(&fx.ledger));
    let sent: Vec<_> = fx.ledger.sent.borrow().iter().filter(|o| o.kind == LedgerOpKind::Release).cloned().collect();
    assert_eq!(sent.len(), 2);
    assert_eq!((sent[0].to, sent[0].amount_e8s, sent[0].memo.clone(), sent[0].created_at_time), (sent[1].to, sent[1].amount_e8s, sent[1].memo.clone(), sent[1].created_at_time));
    assert_eq!(state::get_release(&b.transfer_id).unwrap().state, ReleaseState::Released);
    assert_eq!(fx.ledger.blocks.borrow().iter().filter(|x| x.kind == LedgerOpKind::Release).count(), 1, "paid once");

    // A ledger fee that changed before the first send: a typed BadFee proves nothing moved, the
    // tuple is rebuilt with the new fee and the same escrow outflow.
    let fx = world();
    let b = burned(&fx, 5 * E8, 1);
    release::open_release(&b).unwrap();
    fx.ledger.fee.set(20_000);
    block_on(release::process_releases(&fx.ledger));
    assert_eq!(state::get_release(&b.transfer_id).unwrap().state, ReleaseState::Pending, "rebuilt, not yet paid");
    assert_eq!(state::read_runtime(|c| c.ledger_fee_e8s), Some(20_000));
    block_on(release::process_releases(&fx.ledger));
    assert_eq!(state::get_release(&b.transfer_id).unwrap().state, ReleaseState::Released);
    assert_eq!(fx.ledger.balance(&dest()), (5 * E8 - 20_000) as u128);
    assert_covered(&fx);
}

#[test]
fn a_ledger_fee_that_swallows_the_release_holds_it_for_re_mint_instead_of_paying_dust() {
    let fx = world();
    let b = burned(&fx, 15_000, 1);
    release::open_release(&b).unwrap();
    assert_eq!(state::get_release(&b.transfer_id).unwrap().state, ReleaseState::Pending);
    fx.ledger.fee.set(20_000);
    block_on(release::process_releases(&fx.ledger));
    let r = state::get_release(&b.transfer_id).unwrap();
    assert!(matches!(r.state, ReleaseState::Held { .. }), "{:?}", r.state);
    assert!(r.transfer.is_none());
    let bk = buckets();
    assert_eq!((bk.pending_release, bk.held, bk.unknown_outflow), (0, 15_000, 0));
    assert_covered(&fx);
}

#[test]
fn the_release_window_defers_a_second_release_and_never_starves_a_large_one() {
    let fx = world();
    state::mutate_runtime(|c| c.config.limits.release_window_limit_e8s = 100_000);
    let b1 = burned(&fx, 60_000, 1);
    let b2 = burned(&fx, 60_000, 2);
    release::open_release(&b1).unwrap();
    release::open_release(&b2).unwrap();
    block_on(release::process_releases(&fx.ledger));
    let states: Vec<_> = [b1.transfer_id, b2.transfer_id].iter().map(|r| state::get_release(r).unwrap().state).collect();
    assert_eq!(states, vec![ReleaseState::Released, ReleaseState::Pending], "the second waits for the window");
    assert_eq!(fx.ledger.sent_count(), 1);
    test_env::advance_secs(86_401);
    block_on(release::process_releases(&fx.ledger));
    assert_eq!(state::get_release(&b2.transfer_id).unwrap().state, ReleaseState::Released);
    // A single release above the limit goes out at the start of a fresh window.
    test_env::advance_secs(86_401);
    let big = burned(&fx, 500_000, 3);
    release::open_release(&big).unwrap();
    block_on(release::process_releases(&fx.ledger));
    assert_eq!(state::get_release(&big.transfer_id).unwrap().state, ReleaseState::Released);
}

#[test]
fn paused_releases_and_a_halt_send_nothing() {
    let fx = world();
    let b = burned(&fx, 5 * E8, 1);
    release::open_release(&b).unwrap();
    state::mutate_runtime(|c| c.config.pauses.releases = true);
    block_on(release::process_releases(&fx.ledger));
    assert_eq!(fx.ledger.sent_count(), 0);
    state::mutate_runtime(|c| c.config.pauses.releases = false);
    state::halt("test halt");
    block_on(release::process_releases(&fx.ledger));
    assert_eq!(fx.ledger.sent_count(), 0);
    // A return can still be recorded while halted (the books must follow the chain), but still pays nothing.
    let c = burned(&fx, E8, 2);
    assert_eq!(release::open_release(&c), Ok(OpenOutcome::Opened(ReleaseState::Pending)));
    block_on(release::process_releases(&fx.ledger));
    assert_eq!(fx.ledger.sent_count(), 0);
}

#[test]
fn an_unknown_release_is_never_given_a_second_tuple() {
    let fx = world();
    let b = burned(&fx, 5 * E8, 1);
    release::open_release(&b).unwrap();
    fx.ledger.push_fault(LedgerFault::AmbiguousNoEffect);
    block_on(release::process_releases(&fx.ledger));
    let first = ledger_ops::load_op(&OpRef::Release(b.transfer_id)).unwrap();
    assert!(matches!(first.status, LedgerOpStatus::Unknown { .. }));
    // Many passes: the tuple identity (memo, time, amount) never changes while it is unresolved.
    for _ in 0..3 {
        test_env::advance_secs(200);
        fx.ledger.push_fault(LedgerFault::AmbiguousNoEffect);
        block_on(release::process_releases(&fx.ledger));
    }
    let sent = fx.ledger.sent.borrow();
    assert!(sent.iter().all(|o| (o.memo.clone(), o.created_at_time, o.amount_e8s) == (first.memo.clone(), first.created_at_time, first.amount_e8s)));
    assert_eq!(buckets().unknown_outflow, 5 * E8 as u128, "in doubt the whole time");
}

#[test]
fn a_release_in_doubt_survives_an_upgrade_and_resumes_with_the_stored_tuple() {
    use ic_stable_structures::Storable;
    let fx = world();
    let b = burned(&fx, 5 * E8, 1);
    release::open_release(&b).unwrap();
    fx.ledger.push_fault(LedgerFault::ExecuteThenLoseReply);
    block_on(release::process_releases(&fx.ledger));
    let stored = state::get_release(&b.transfer_id).unwrap();
    assert!(matches!(stored.transfer.as_ref().unwrap().status, LedgerOpStatus::Unknown { .. }));
    // "Upgrade": every record is read back from its bytes; nothing lives only on the heap.
    let reloaded = state::ReleaseRecord::from_bytes(stored.to_bytes());
    assert_eq!(reloaded, stored);
    state::put_release(&reloaded);
    let core = state::read_runtime(|c| c.clone());
    state::mutate_runtime(|c| *c = state::BridgeCore::from_bytes(core.to_bytes()));
    assert_eq!(state::get_release(&b.transfer_id).unwrap().transfer, stored.transfer, "the exact tuple is intact");
    assert_eq!(state::release_by_sequence(1), Some(b.transfer_id));
    test_env::advance_secs(200);
    block_on(release::process_releases(&fx.ledger));
    let sent: Vec<_> = fx.ledger.sent.borrow().iter().filter(|o| o.kind == LedgerOpKind::Release).cloned().collect();
    assert_eq!(sent.len(), 2);
    assert_eq!((sent[0].memo.clone(), sent[0].created_at_time, sent[0].amount_e8s), (sent[1].memo.clone(), sent[1].created_at_time, sent[1].amount_e8s));
    assert_eq!(state::get_release(&b.transfer_id).unwrap().state, ReleaseState::Released);
    assert_eq!(fx.ledger.blocks.borrow().iter().filter(|x| x.kind == LedgerOpKind::Release).count(), 1, "paid once");
    assert_eq!(release::open_release(&b), Ok(OpenOutcome::AlreadyOpen), "the return stays one release after the upgrade");
}

// ---------------------------------------------------------------------------
// Hints
// ---------------------------------------------------------------------------

fn digest_text(b: u8) -> String {
    bs58::encode([b; 32]).into_string()
}

#[test]
fn a_hint_is_stored_bounded_deduplicated_and_expires_and_settles_nothing() {
    let fx = world();
    let before = buckets();
    test_env::set_caller(Principal::anonymous());
    assert_eq!(release::record_hint(digest_text(1)), Err(BridgeError::Anonymous));
    test_env::set_caller(user(0x21));
    assert!(matches!(release::record_hint("not a digest".into()), Err(BridgeError::InvalidArgument(_))));
    assert!(matches!(release::record_hint("1111".into()), Err(BridgeError::InvalidArgument(_))), "wrong length");
    assert!(release::record_hint(digest_text(1)).is_ok());
    assert!(release::record_hint(digest_text(1)).is_ok(), "a repeat is a no-op");
    assert_eq!(state::read_runtime(|c| c.hints.len()), 1);
    for b in 2..=5u8 {
        assert!(release::record_hint(digest_text(b)).is_ok());
    }
    assert_eq!(release::record_hint(digest_text(6)), Err(BridgeError::TooManyActiveOperations), "five per caller");
    test_env::set_caller(user(0x22));
    assert!(release::record_hint(digest_text(7)).is_ok());
    assert_eq!(release::live_hints(env::now_ns()).len(), 6);
    // Hints expire.
    test_env::advance_secs(2 * HOUR + 1);
    assert!(release::live_hints(env::now_ns()).is_empty());
    assert!(release::record_hint(digest_text(8)).is_ok());
    assert_eq!(state::read_runtime(|c| c.hints.len()), 1, "expired hints were dropped");
    assert_eq!(buckets(), before, "a hint settles nothing");
    assert_eq!(fx.ledger.sent_count(), 0);
    // The global bound evicts the oldest.
    state::mutate_runtime(|c| c.hints.clear());
    for i in 0..(state::MAX_HINTS + 3) {
        test_env::set_caller(Principal::from_slice(&[(i % 250) as u8 + 1, (i / 250) as u8, 9]));
        let mut raw = [0u8; 32];
        raw[..8].copy_from_slice(&(i as u64 + 1).to_be_bytes());
        release::record_hint(bs58::encode(raw).into_string()).unwrap();
    }
    assert_eq!(state::read_runtime(|c| c.hints.len()), state::MAX_HINTS);
}

#[test]
fn opening_a_release_drops_the_hint_for_its_return() {
    let fx = world();
    let b = burned(&fx, 5 * E8, 1);
    test_env::set_caller(user(0x21));
    release::record_hint(bs58::encode(b.tx_digest).into_string()).unwrap();
    release::record_hint(digest_text(9)).unwrap();
    assert_eq!(release::live_hints(env::now_ns()).len(), 2);
    release::open_release(&b).unwrap();
    let live = release::live_hints(env::now_ns());
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].digest, [9; 32], "only the hint for the opened return is gone");
}

// ---------------------------------------------------------------------------
// Admin setters
// ---------------------------------------------------------------------------

#[test]
fn the_admin_setters_are_admin_only_and_bounded() {
    let fx = world();
    test_env::set_caller(user(0x21));
    assert_eq!(crate::admin::set_pauses(Pauses::default()), Err(BridgeError::Unauthorized));
    assert_eq!(crate::admin::set_limits(state::default_limits()), Err(BridgeError::Unauthorized));
    assert_eq!(crate::admin::set_fees(BridgeFees::default()), Err(BridgeError::Unauthorized));
    assert_eq!(crate::admin::clear_halt("x"), Err(BridgeError::Unauthorized));
    test_env::set_caller(admin());
    // Limits and fees stay within their invariants.
    let mut l = state::default_limits();
    l.min_deposit_e8s = 0;
    assert!(matches!(crate::admin::set_limits(l), Err(BridgeError::InvalidArgument(_))));
    let mut l = state::default_limits();
    l.max_deposit_e8s = l.max_outstanding_e8s + 1;
    assert!(matches!(crate::admin::set_limits(l), Err(BridgeError::InvalidArgument(_))));
    let mut l = state::default_limits();
    l.max_active_global = 10_001;
    assert!(matches!(crate::admin::set_limits(l), Err(BridgeError::InvalidArgument(_))));
    assert!(crate::admin::set_limits(state::default_limits()).is_ok());
    assert!(matches!(
        crate::admin::set_fees(BridgeFees { deposit_fee_e8s: state::default_limits().min_deposit_e8s, release_fee_e8s: 0 }),
        Err(BridgeError::InvalidArgument(_))
    ));
    assert!(matches!(
        crate::admin::set_fees(BridgeFees { deposit_fee_e8s: 0, release_fee_e8s: state::MAX_RELEASE_FEE_E8S + 1 }),
        Err(BridgeError::InvalidArgument(_))
    ));
    assert!(crate::admin::set_fees(BridgeFees { deposit_fee_e8s: 1_000, release_fee_e8s: 2_000 }).is_ok());
    // A raised minimum cannot leave the deposit fee at or above it.
    let mut l = state::default_limits();
    l.min_deposit_e8s = 1_000;
    assert!(matches!(crate::admin::set_limits(l), Err(BridgeError::InvalidArgument(_))));
    // Deposits cannot be enabled before the gate is bound, nor unpaused during a halt.
    state::mutate_core(|c| c.binding = None);
    state::mutate_runtime(|c| c.config.pauses.deposits = true);
    assert!(matches!(crate::admin::set_pauses(Pauses { deposits: false, ..Pauses::default() }), Err(BridgeError::NotConfigured(_))));
    assert!(crate::admin::set_pauses(Pauses { deposits: true, ..Pauses::default() }).is_ok(), "pausing is always allowed");
    state::mutate_core(|c| c.binding = Some(state::StoredBinding { package: [0x70; 32], gate: [0x6a; 32], mint_cap: [0x6d; 32], admin_cap: [0x6e; 32] }));
    state::halt("test halt");
    assert!(matches!(crate::admin::set_pauses(Pauses { deposits: false, ..Pauses::default() }), Err(BridgeError::Halted(_))));
    assert!(crate::admin::set_pauses(Pauses { deposits: true, releases: true, ..Pauses::default() }).is_ok());
    drop(fx);
}

#[test]
fn a_halt_is_cleared_only_with_a_note_and_only_when_halted_and_the_decision_is_audited() {
    let _fx = world();
    test_env::set_caller(admin());
    assert!(matches!(crate::admin::clear_halt("all clear"), Err(BridgeError::InvalidState(_))), "not halted");
    state::halt("a release was opened twice");
    assert!(matches!(crate::admin::clear_halt(" "), Err(BridgeError::InvalidArgument(_))));
    assert!(state::is_halted(), "an empty note clears nothing");
    assert!(crate::admin::clear_halt("reviewed on the explorer").is_ok());
    assert!(!state::is_halted());
    let log = state::audit_page(0, 100);
    assert!(log.iter().any(|a| a.note.contains("reviewed on the explorer")), "the decision is in the audit log");
    assert!(log.iter().any(|a| a.note.contains("a release was opened twice")), "and so is the halt");
}

// ---------------------------------------------------------------------------
// The timer, persistence and the audit trail
// ---------------------------------------------------------------------------

#[test]
fn the_timer_pass_drives_pulls_refunds_releases_and_sweeps_to_a_proven_end() {
    let fx = world();
    // A pull whose reply was lost, a held deposit awaiting its refund and a release.
    fund(&fx, &acct(user(0x21)), E8);
    fx.ledger.push_fault(LedgerFault::ExecuteThenLoseReply);
    bridge(&fx, args(E8, 1)).unwrap();
    let held_id = held_deposit(&fx, 0x22, E8, 1);
    fx.ledger.push_fault(LedgerFault::ExecuteThenLoseReply);
    block_on(deposit::request_refund(&fx.ledger, held_id)).ok();
    test_env::set_caller(user(0x21));
    let b = burned(&fx, 5 * E8, 1);
    release::open_release(&b).unwrap();
    fx.ledger.push_fault(LedgerFault::ExecuteThenLoseReply);
    block_on(release::process_releases(&fx.ledger));
    assert_eq!(deposit_of(0).state, DepositState::PullUnknown);
    assert_eq!(deposit_of(held_id).state, DepositState::Refunding);
    assert_eq!(state::get_release(&b.transfer_id).unwrap().state, ReleaseState::Pending);
    test_env::advance_secs(200);
    let now = env::now_ns();
    let report = block_on(crate::timer::run_once(&fx.ledger, now));
    assert!(!report.skipped);
    assert_eq!(deposit_of(0).state, DepositState::Escrowed);
    assert_eq!(deposit_of(held_id).state, DepositState::Refunded);
    assert_eq!(state::get_release(&b.transfer_id).unwrap().state, ReleaseState::Released);
    assert_eq!(buckets().unknown_outflow, 0);
    assert_covered(&fx);
}

#[test]
fn an_idle_pass_makes_only_the_three_cheap_ledger_reads_and_no_transfer() {
    let fx = world();
    state::mutate_runtime(|c| {
        c.ledger_minting_account = None;
        c.ledger_fee_e8s = None;
    });
    fx.ledger.preload_blocks(30);
    let report = block_on(crate::timer::run_once(&fx.ledger, env::now_ns()));
    assert!(!report.skipped);
    assert_eq!(fx.ledger.sent_count(), 0);
    assert_eq!(state::read_runtime(|c| (c.ledger_fee_e8s, c.ledger_tip_lower_bound)), (Some(FEE), 30));
    assert!(state::read_runtime(|c| c.ledger_minting_account).is_some());
    // The next idle pass trusts the facts for a few minutes: no ledger call at all.
    let calls = fx.ledger.fee_calls.get();
    test_env::advance_secs(30);
    block_on(crate::timer::run_once(&fx.ledger, env::now_ns()));
    assert_eq!(fx.ledger.fee_calls.get(), calls, "an idle bridge does not poll the ledger every pass");
    // With ledger work pending the facts are read every pass.
    fund(&fx, &acct(user(0x21)), E8);
    fx.ledger.push_fault(LedgerFault::AmbiguousNoEffect);
    bridge(&fx, args(E8, 1)).unwrap();
    let calls = fx.ledger.fee_calls.get();
    test_env::advance_secs(30);
    block_on(crate::timer::run_once(&fx.ledger, env::now_ns()));
    assert!(fx.ledger.fee_calls.get() > calls);
    // And they are read again once stale.
    state::mutate_runtime(|c| c.buckets = Default::default());
    state::put_deposit(&{
        let mut d = deposit_of(0);
        d.state = DepositState::Delivered;
        d
    });
    test_env::advance_secs(6 * 60);
    let calls = fx.ledger.fee_calls.get();
    block_on(crate::timer::run_once(&fx.ledger, env::now_ns()));
    assert!(fx.ledger.fee_calls.get() > calls, "stale facts are refreshed");
}

#[test]
fn pending_ledger_work_survives_an_upgrade_and_resumes_with_the_stored_tuple() {
    use ic_stable_structures::Storable;
    let fx = world();
    fund(&fx, &acct(user(0x21)), E8);
    fx.ledger.push_fault(LedgerFault::ExecuteThenLoseReply);
    bridge(&fx, args(E8, 1)).unwrap();
    // "Upgrade": the stored records are read back from their bytes; the saga keeps no heap state.
    let reloaded = DepositRecord::from_bytes(deposit_of(0).to_bytes());
    assert_eq!(reloaded, deposit_of(0));
    state::put_deposit(&reloaded);
    let core = state::read_runtime(|c| c.clone());
    state::mutate_runtime(|c| *c = state::BridgeCore::from_bytes(core.to_bytes()));
    assert!(!state::migrate_runtime(), "already current");
    test_env::advance_secs(200);
    block_on(deposit::reconcile_pulls(&fx.ledger));
    let sent = pull_tuples(&fx);
    assert_eq!(
        (sent[0].from, sent[0].memo.clone(), sent[0].created_at_time, sent[0].amount_e8s),
        (sent[1].from, sent[1].memo.clone(), sent[1].created_at_time, sent[1].amount_e8s)
    );
    assert_eq!(deposit_of(0).state, DepositState::Escrowed);
    assert_eq!(fx.ledger.blocks.borrow().len(), 1);
}

#[test]
fn the_audit_log_names_the_deposit_and_return_it_is_about() {
    let fx = world();
    let id = escrowed(&fx, 0x21, E8, 1);
    let log = state::audit_page(0, 100);
    assert!(log.iter().any(|a| a.deposit_id == Some(id) && a.note == "admitted"));
    assert!(log.iter().any(|a| a.deposit_id == Some(id) && a.transition == Some(Transition::PullSucceeded { amount: E8 })));
    let b = burned(&fx, 5 * E8, 1);
    release::open_release(&b).unwrap();
    let log = state::audit_page(0, 100);
    assert!(log.iter().any(|a| a.return_id == Some(b.transfer_id) && a.transition == Some(Transition::ReturnFinalized { amount: 5 * E8 })));
    let v = crate::views::audit_entry(&state::audit_page(0, 1)[0]);
    assert_eq!(v.seq, 0);
}

#[test]
fn views_carry_the_whole_record_in_text_forms() {
    let fx = world();
    let id = escrowed(&fx, 0x21, E8, 1);
    let v = crate::views::deposit_view(&deposit_of(id));
    assert_eq!((v.sui_recipient.clone(), v.status.clone(), v.pull_block_index), (wallet(), DepositStatus::Escrowed, Some(0)));
    assert_eq!(v.mint_id.as_ref().map(|s| s.len()), Some(64));
    let b = burned(&fx, 5 * E8, 1);
    release::open_release(&b).unwrap();
    let r = crate::views::release_view(&state::get_release(&b.transfer_id).unwrap());
    assert_eq!(
        (r.status.clone(), r.sui_sender.clone(), r.return_id.len(), r.destination, r.gate_sequence, r.checkpoint),
        (ReleaseStatus::Pending, format_address(&SENDER), 64, dest(), 1, 701)
    );
    assert_eq!(r.tx_digest, bs58::encode(b.tx_digest).into_string());
    // An unknown transfer shows as such.
    fx.ledger.push_fault(LedgerFault::AmbiguousNoEffect);
    block_on(release::process_releases(&fx.ledger));
    let r = crate::views::release_view(&state::get_release(&b.transfer_id).unwrap());
    assert_eq!(r.status, ReleaseStatus::TransferUnknown);
    // A paid one shows its block.
    test_env::advance_secs(200);
    block_on(release::process_releases(&fx.ledger));
    let r = crate::views::release_view(&state::get_release(&b.transfer_id).unwrap());
    assert_eq!((r.status, r.release_block_index.is_some()), (ReleaseStatus::Released, true));
}

// ---------------------------------------------------------------------------
// Fair paging of the background queues
//
// A pass walks one bounded window of the active index after a cursor, so a bounded number of passes
// reaches every entry, and looking at an entry settles, resets or rebuilds nothing. Old entries whose
// effect stays unresolved (an unavailable archive, a transfer that keeps failing ambiguously) cannot
// starve newer ones.
// ---------------------------------------------------------------------------

#[test]
fn old_pulls_that_cannot_be_proven_do_not_starve_a_newer_resolvable_pull() {
    let fx = world();
    // Twelve pulls (more than one batch of ten) whose reply was lost.
    let mut older = vec![];
    for who in 0x40..0x4c {
        test_env::set_caller(user(who));
        fund(&fx, &acct(user(who)), E8);
        fx.ledger.push_fault(LedgerFault::ExecuteThenLoseReply);
        older.push(bridge(&fx, args(E8, 1)).unwrap().id);
    }
    // A day later they can only be searched, and the archive is unavailable: they stay unresolved.
    *fx.ledger.search_error.borrow_mut() = Some("archive unavailable".into());
    test_env::set_now_ns(env::now_ns() + DEDUP_SAFE_WINDOW_NS + 1_000_000_000);
    // A newer pull, inside its window, whose reply was lost: resending the identical tuple resolves it.
    test_env::set_caller(user(0x60));
    fund(&fx, &acct(user(0x60)), E8);
    fx.ledger.push_fault(LedgerFault::ExecuteThenLoseReply);
    let newer = bridge(&fx, args(E8, 1)).unwrap().id;
    assert_eq!(deposit_of(newer).state, DepositState::PullUnknown);
    let before: Vec<_> = older
        .iter()
        .map(|id| {
            let p = deposit_of(*id).pull.unwrap();
            (p.created_at_time, p.attempts, p.memo)
        })
        .collect();
    let sent_before = fx.ledger.sent_count();
    // A bounded number of passes reaches it.
    let mut resolved_after = None;
    for n in 1..=3 {
        test_env::advance_secs(5);
        block_on(deposit::reconcile_pulls(&fx.ledger));
        if deposit_of(newer).state == DepositState::Escrowed {
            resolved_after = Some(n);
            break;
        }
    }
    assert!(resolved_after.is_some(), "the newer resolvable pull was starved by older unresolved ones");
    // The older ones are exactly as they were: not failed, not settled, the same tuples, never resent.
    for (id, was) in older.iter().zip(before) {
        let d = deposit_of(*id);
        assert_eq!(d.state, DepositState::PullUnknown);
        let p = d.pull.unwrap();
        assert_eq!((p.created_at_time, p.attempts, p.memo), was, "deposit {id}");
    }
    assert_eq!(fx.ledger.sent_count(), sent_before + 1, "only the newer pull was resent");
    assert_eq!(buckets().reserved_pulls, (12 * E8) as u128, "the older capacity stays reserved");
}

#[test]
fn ten_unresolved_releases_do_not_starve_a_newer_pending_release() {
    let fx = world();
    let mut all = vec![];
    for tag in 1..=11u8 {
        let b = burned(&fx, 200_000, tag);
        release::open_release(&b).unwrap();
        all.push(b);
    }
    // The transfers of the first ten can never be resolved: every call about them fails ambiguously.
    for b in &all[..10] {
        fx.ledger.ambiguous_memos.borrow_mut().push(ids::release_memo(&b.transfer_id));
    }
    let mut resolved_after = None;
    for n in 1..=3 {
        test_env::advance_secs(200);
        block_on(release::process_releases(&fx.ledger));
        if state::get_release(&all[10].transfer_id).unwrap().state == ReleaseState::Released {
            resolved_after = Some(n);
            break;
        }
    }
    assert!(resolved_after.is_some(), "the newer pending release was starved by older unresolved ones");
    // The older ones stay pending with their own tuple and their outflow still in doubt.
    for b in &all[..10] {
        let r = state::get_release(&b.transfer_id).unwrap();
        assert_eq!(r.state, ReleaseState::Pending);
        let t = r.transfer.unwrap();
        assert!(matches!(t.status, LedgerOpStatus::Unknown { .. }));
        assert_eq!(t.memo, b.transfer_id.to_vec());
    }
    assert_eq!(buckets().unknown_outflow, 10 * 200_000);
    assert_covered(&fx);
}

#[test]
fn five_unresolved_refunds_do_not_starve_a_newer_one() {
    let fx = world();
    let deposits: Vec<u64> = (0..6u8).map(|i| held_deposit(&fx, 0x50 + i, E8, 1)).collect();
    for id in &deposits[..5] {
        fx.ledger.ambiguous_memos.borrow_mut().push(ids::refund_ledger_memo(*id));
    }
    for (i, id) in deposits.iter().enumerate() {
        test_env::set_caller(user(0x50 + i as u8));
        if i == 5 {
            // The newer refund executes and its reply is lost: resending the identical tuple resolves it.
            fx.ledger.push_fault(LedgerFault::ExecuteThenLoseReply);
        }
        let v = block_on(deposit::request_refund(&fx.ledger, *id)).unwrap();
        assert_eq!(v.status, DepositStatus::Refunding, "refund {i}");
    }
    let before: Vec<_> = deposits[..5]
        .iter()
        .map(|id| {
            let r = deposit_of(*id).refund.unwrap();
            (r.created_at_time, r.memo)
        })
        .collect();
    let mut resolved_after = None;
    for n in 1..=3 {
        test_env::advance_secs(200);
        block_on(deposit::process_refunds(&fx.ledger));
        if deposit_of(deposits[5]).state == DepositState::Refunded {
            resolved_after = Some(n);
            break;
        }
    }
    assert!(resolved_after.is_some(), "the newer refund was starved by older unresolved ones");
    for (id, was) in deposits[..5].iter().zip(before) {
        let d = deposit_of(*id);
        assert_eq!(d.state, DepositState::Refunding);
        let r = d.refund.unwrap();
        assert_eq!((r.created_at_time, r.memo), was, "deposit {id}");
        assert!(r.outflow_reserved, "the held funds stay in doubt, not released");
    }
    assert_eq!(buckets().unknown_outflow, 5 * E8 as u128);
    assert_covered(&fx);
}

/// An active deposit that no queue has any business with (it waits for its mint).
fn filler_deposit(id: u64) -> DepositRecord {
    DepositRecord {
        id,
        kind: DepositKind::UserDeposit,
        owner: user(0x77),
        from: None,
        client_request_id: vec![1; 16],
        sui_recipient: RECIPIENT,
        amount_e8s: E8,
        deposit_fee_e8s: 0,
        mint_amount_e8s: E8,
        mint_id: None,
        state: DepositState::Escrowed,
        pull: None,
        refund: None,
        mint_op: None,
        abandoned_mint_ops: vec![],
        delivered: None,
        hold_retry_at_ns: None,
        created_at_ns: 0,
        updated_at_ns: 0,
    }
}

#[test]
fn a_pass_examines_a_bounded_window_of_the_active_index_whatever_is_active() {
    let fx = world();
    // 200 deposits waiting for their mint, then one real pull whose reply was lost: it is the 201st
    // active entry, so it is reached exactly when the window has walked the whole index.
    state::mutate_runtime(|c| c.next_deposit_id = 200);
    for id in 0..200 {
        state::put_deposit(&filler_deposit(id));
    }
    test_env::set_caller(user(0x21));
    fund(&fx, &acct(user(0x21)), E8);
    fx.ledger.push_fault(LedgerFault::ExecuteThenLoseReply);
    let id = bridge(&fx, args(E8, 1)).unwrap().id;
    assert_eq!(id, 200);
    let needed = 201usize.div_ceil(state::QUEUE_SCAN);
    for n in 1..=needed {
        block_on(deposit::reconcile_pulls(&fx.ledger));
        let done = deposit_of(id).state == DepositState::Escrowed;
        assert_eq!(done, n == needed, "pass {n} of {needed}");
    }
}
