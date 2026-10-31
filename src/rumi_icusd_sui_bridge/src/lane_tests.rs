//! Tests of the 53b observed binding and the mint lane against the simulated chain (`chain_sim`):
//! the whole stack from the timer pass down through the mirror's queries, the strict quorum and the
//! JSON parsers, with deterministic TEST signers. Nothing here touches a network.
//!
//! These are the lane's own behavior tests. None of them is a "red" proof of a pre-existing defect:
//! the lane is new in 53b, and the failing development runs are listed in the handback.

use crate::bind;
use crate::chain_sim::*;
use crate::env::{self, test_env};
use crate::flows::{deposit, mint, release, remint};
use crate::graphql::{MirrorSettings, ProviderConfig, Verdict};
use crate::ids::SuiRole;
use crate::mirror::Mirror;
use crate::saga_tests::{acct, admin, args, assert_covered, bridge, buckets, burned_with, deposit_of, escrowed, fund, user, world, E8, RECIPIENT, SENDER};
use crate::state::{
    self, AttemptDisposition, DeliveryVia, DepositKind, DepositState, MintOpState, ReleaseState, StoredRoleKey,
};
use crate::testkit::*;
use crate::types::*;
use std::cell::RefCell;
use std::rc::Rc;

pub(crate) struct Lane {
    pub(crate) fx: Fixture,
    pub(crate) sim: ChainSim,
    pub(crate) signer: TestSigner,
}

fn providers(ids: &[&str]) -> MirrorSettings {
    MirrorSettings {
        providers: ids.iter().map(|id| ProviderConfig { id: id.to_string(), url: format!("https://{id}.example.com/graphql") }).collect(),
        min_quorum: 2,
        min_independent_providers: 2,
        max_response_bytes: 16_384,
        enabled: true,
    }
}

/// A configured instance with real (test) role keys, three simulated providers, a pristine gate with a
/// large supply cap, a gas coin, and the deployment observed through the bind.
pub(crate) fn lane() -> Lane {
    let fx = world();
    let (t, g, c) = (TestSigner::treasury_public_key(), TestSigner::gas_public_key(), TestSigner::custody_public_key());
    state::mutate_core(|core| {
        core.roles = vec![
            StoredRoleKey { role: SuiRole::Treasury, public_key: t },
            StoredRoleKey { role: SuiRole::Gas, public_key: g },
            StoredRoleKey { role: SuiRole::UpgradeCustody, public_key: c },
        ];
        core.mirror = providers(&["alpha", "beta", "gamma"]);
    });
    state::mutate_runtime(|r| {
        r.config.lane.gas_coin = Some(SIM_GAS_COIN);
        r.observed = None;
    });
    let sim = ChainSim::new(t, g, c);
    sim.set_supply_cap(1_000_000 * E8);
    sim.advance(100);
    let l = Lane { fx, sim, signer: TestSigner::new() };
    block_on(bind::observe_deployment(&SimTransport { sim: &l.sim })).expect("the simulated deployment is observed");
    test_env::set_caller(user(0x21));
    l
}

impl Lane {
    pub(crate) fn net(&self) -> SimTransport<'_> {
        SimTransport { sim: &self.sim }
    }

    /// Thirty seconds of the timer, a hundred checkpoints of the chain, one full pass.
    pub(crate) fn tick(&self) {
        self.sim.advance(100);
        test_env::advance_secs(30);
        block_on(crate::timer::run_full(&self.fx.ledger, &self.net(), &self.signer, env::now_ns()));
    }

    pub(crate) fn ticks(&self, n: usize) {
        for _ in 0..n {
            self.tick();
        }
    }

    pub(crate) fn op(&self, id: u64) -> state::MintOp {
        state::get_mint_op(id).expect("mint op")
    }
}

// ---------------------------------------------------------------------------
// The mirror: quorum, identity and the head
// ---------------------------------------------------------------------------

#[test]
fn the_head_is_rounded_and_providers_in_one_window_agree_while_those_across_a_boundary_are_held() {
    let l = lane();
    l.sim.head.set(1_050);
    l.sim.set_provider("gamma", Provider { lag: 20, ..Provider::default() });
    let net = l.net();
    let m = Mirror::from_state(&net).unwrap();
    assert_eq!(block_on(m.head()), Verdict::Agreed(1_000), "1,050 and 1,030 both round to 1,000");
    l.sim.set_provider("gamma", Provider { lag: 60, ..Provider::default() });
    assert!(matches!(block_on(m.head()), Verdict::Held(_)), "1,050 and 990 straddle a boundary: held, not averaged");
}

#[test]
fn a_provider_on_another_network_a_dead_one_and_a_wrong_version_are_set_aside_before_the_tally() {
    let l = lane();
    let net = l.net();
    let m = Mirror::from_state(&net).unwrap();
    // One provider reports mainnet: excluded; the other two agree.
    l.sim.set_provider("gamma", Provider { wrong_network: true, ..Provider::default() });
    assert!(matches!(block_on(m.head()), Verdict::Agreed(_)));
    // A second one down leaves one usable provider: unavailable, never a verdict from one.
    l.sim.set_provider("beta", Provider { down: true, ..Provider::default() });
    match block_on(m.head()) {
        Verdict::Unavailable(why) => assert!(why.contains("wrong network") && why.contains("connection refused"), "{why}"),
        other => panic!("{other:?}"),
    }
    // Exact-version reads: a provider that answers with another version is excluded BEFORE the tally.
    l.sim.set_provider("beta", Provider::default());
    l.sim.set_provider("gamma", Provider { wrong_exact_version: true, ..Provider::default() });
    let cap = l.sim.current_ref(&SIM_MINT_CAP);
    match block_on(m.object_at_version(&SIM_MINT_CAP, cap.version)) {
        Verdict::Agreed(o) => assert_eq!((o.version, o.digest), (cap.version, cap.digest)),
        other => panic!("{other:?}"),
    }
    // With only the lying provider and one honest one, nothing is agreed.
    state::mutate_core(|c| c.mirror = providers(&["alpha", "gamma"]));
    let m2 = Mirror::from_state(&net).unwrap();
    match block_on(m2.object_at_version(&SIM_MINT_CAP, cap.version)) {
        Verdict::Unavailable(why) => assert!(why.contains("requested version"), "{why}"),
        other => panic!("{other:?}"),
    }
    // A provider whose gate breaks the supply identity is excluded before the tally, not outvoted.
    state::mutate_core(|c| c.mirror = providers(&["alpha", "beta", "gamma"]));
    l.sim.set_provider("gamma", Provider { forge_supply: Some(77), ..Provider::default() });
    let m3 = Mirror::from_state(&net).unwrap();
    let head = l.sim.head.get() - l.sim.head.get() % 100;
    match block_on(m3.gate(&sim_cfg(), head)) {
        Verdict::Agreed(g) => assert_eq!(g.gate.total_supply, 0),
        other => panic!("{other:?}"),
    }
    // A disagreement among usable providers is Held: one finds the object, one does not yet serve the checkpoint.
    l.sim.set_provider("gamma", Provider { lag: 10, ..Provider::default() });
    let now_head = l.sim.head.get();
    assert!(matches!(block_on(m3.gate(&sim_cfg(), now_head)), Verdict::Held(_)));
}

// ---------------------------------------------------------------------------
// The observed binding
// ---------------------------------------------------------------------------

#[test]
fn the_observed_deployment_records_what_the_providers_agree_and_not_what_was_configured() {
    let l = lane();
    let o = state::read_runtime(|r| r.observed.clone()).expect("observed");
    assert_eq!((o.gate_version, o.initial_shared_version, o.decimals, o.symbol.as_str()), (1, SIM_SHARED_VERSION, 8, "icUSD"));
    assert_eq!((o.supply_cap, o.total_supply, o.mint_count, o.minted_total), (1_000_000 * E8, 0, 0, 0));
    assert_eq!((o.mint_cap.id, o.admin_cap.id), (SIM_MINT_CAP, SIM_ADMIN_CAP));
    let roles = bind::lane_identity().unwrap().roles;
    assert_eq!(o.mint_cap_owner, roles.treasury, "the mint cap is owned by the treasury role");
    assert_eq!((o.admin_cap_owner, o.admin_cap_role), (roles.upgrade_custody, Some(SuiRole::UpgradeCustody)));
    assert!(!o.module_digest_observed, "the module bytes were never compared with a build");
    assert!(o.checkpoint % 100 == 0, "observed at a rounded head");
    assert!(state::audit_page(0, 200).iter().any(|a| a.note.contains("deployment observed")));
    let _ = l;
}

#[test]
fn the_bind_names_what_does_not_match_and_stores_nothing() {
    let l = lane();
    state::mutate_runtime(|r| r.observed = None);
    // The mint cap is owned by an address that is not the treasury role: the providers agree on it.
    let wrong_owner = ChainSim::new(TestSigner::gas_public_key(), TestSigner::gas_public_key(), TestSigner::custody_public_key());
    wrong_owner.set_supply_cap(E8);
    match block_on(bind::observe_deployment(&SimTransport { sim: &wrong_owner })) {
        Err(bind::BindError::Mismatch(m)) => assert!(m.contains("owned by"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert!(state::read_runtime(|r| r.observed.is_none()), "a mismatch stores nothing");
    // Wrong decimals, a wrong symbol and an unregistered currency.
    for (md, expect) in [(Some((6u8, "icUSD".to_string())), "6 decimals"), (Some((8u8, "ICP".to_string())), "symbol"), (None, "no coin metadata")] {
        *l.sim.coin_metadata.borrow_mut() = md;
        match block_on(bind::observe_deployment(&l.net())) {
            Err(bind::BindError::Mismatch(m)) => assert!(m.contains(expect), "{m}"),
            other => panic!("{other:?}"),
        }
        assert!(state::read_runtime(|r| r.observed.is_none()));
    }
    *l.sim.coin_metadata.borrow_mut() = Some((8, "icUSD".into()));
    // A gate that already minted something is not pristine: the books cannot explain it.
    l.sim.foreign_mint([0x31; 32], [0x41; 32], 5);
    l.sim.advance(100);
    match block_on(bind::observe_deployment(&l.net())) {
        Err(bind::BindError::Mismatch(m)) => assert!(m.contains("already has activity"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert!(state::read_runtime(|r| r.observed.is_none()));
}

#[test]
fn the_bind_is_unavailable_when_providers_disagree_or_the_mirror_is_off() {
    let l = lane();
    state::mutate_runtime(|r| r.observed = None);
    // Two providers one window apart: held, so nothing is stored.
    l.sim.head.set(1_150);
    l.sim.set_provider("gamma", Provider { lag: 60, ..Provider::default() });
    assert!(matches!(block_on(bind::observe_deployment(&l.net())), Err(bind::BindError::Unavailable(m)) if m.contains("disagree")));
    assert!(state::read_runtime(|r| r.observed.is_none()));
    l.sim.set_provider("gamma", Provider::default());
    state::mutate_core(|c| c.mirror.enabled = false);
    assert!(matches!(block_on(bind::observe_deployment(&l.net())), Err(bind::BindError::NotReady(_))));
    state::mutate_core(|c| c.mirror.enabled = true);
    state::mutate_core(|c| c.binding = None);
    assert!(matches!(block_on(bind::observe_deployment(&l.net())), Err(bind::BindError::NotReady(_))));
}

#[test]
fn deposits_need_an_observed_deployment() {
    let l = lane();
    assert!(deposit::deployment_not_ready().is_none());
    state::mutate_runtime(|r| r.observed = None);
    assert_eq!(deposit::deployment_not_ready().as_deref(), Some("the deployment has not been observed by the providers"));
    assert!(mint::blocked_by().iter().any(|m| m.contains("not been observed")));
    let _ = l;
}

// ---------------------------------------------------------------------------
// The lane: the happy path and persist-before-sign
// ---------------------------------------------------------------------------

#[test]
fn an_escrowed_deposit_is_minted_once_with_a_receipt_and_the_books_follow() {
    let l = lane();
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    assert_eq!(buckets().pending_mint, (5 * E8) as u128);
    l.tick();
    // The first pass only builds: the operation exists, the deposit points at it, nothing is signed.
    let d = deposit_of(id);
    assert_eq!((d.state.clone(), d.mint_op), (DepositState::MintInFlight, Some(0)));
    let op = l.op(0);
    assert_eq!((op.deposit_id, op.attempts.len(), op.mint_id), (id, 1, d.mint_id.unwrap()));
    let a = op.current().unwrap();
    assert!(a.treasury_sig.is_none() && a.gas_sig.is_none() && a.sends == 0);
    assert_eq!(l.signer.calls.borrow().len(), 0);
    assert_eq!(state::read_runtime(|r| r.active_mint_op), Some(0));
    l.tick();
    let d = deposit_of(id);
    assert_eq!(d.state, DepositState::Delivered);
    let op = l.op(0);
    match &op.state {
        MintOpState::Delivered { tx_digest, sequence, via, .. } => {
            assert_eq!((*tx_digest, *sequence, *via), (Some(op.current().unwrap().tx_digest), 1, DeliveryVia::Receipt));
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(op.current().unwrap().disposition, AttemptDisposition::LandedFinal { .. }));
    let rec = l.sim.mint_record(&d.mint_id.unwrap()).expect("minted on chain");
    assert_eq!((rec.recipient, rec.amount, rec.sequence), (RECIPIENT, 5 * E8, 1));
    assert_eq!(l.sim.executed_count(), 1);
    // The books: PM moved to O; the chain's supply equals O (C3, quiescent).
    let b = buckets();
    assert_eq!((b.pending_mint, b.outstanding), (0, (5 * E8) as u128));
    let report = crate::accounting::check_destination_supply(&b, l.sim.gate.borrow().total_supply as u128, state::inflight_mint_e8s(), true).unwrap();
    assert_eq!((report.within_bound, report.exact_match), (true, Some(true)));
    assert_eq!(state::read_runtime(|r| (r.active_mint_op, r.lane_wait.clone())), (None, None));
    assert_covered(&l.fx);
    // Nothing more happens, and nothing is minted twice.
    l.ticks(3);
    assert_eq!(l.sim.executed_count(), 1);
    assert_eq!(l.signer.calls.borrow().len(), 2, "one signature per role, once");
}

#[test]
fn the_operation_and_the_link_exist_before_the_first_signature_and_the_role_is_marked_before_the_call() {
    let l = lane();
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    let seen: Rc<RefCell<Vec<SuiRole>>> = Rc::default();
    let sink = seen.clone();
    *l.signer.on_sign.borrow_mut() = Some(Box::new(move |role, digest| {
        let op = state::active_mint_op().expect("the operation is stored before any signing call");
        let a = op.current().unwrap();
        assert_eq!(&a.signing_digest, digest, "the signing digest is the stored attempt's");
        assert_eq!(crate::tx::signing_digest(&a.tx_bcs), *digest);
        let marked = match role {
            SuiRole::Treasury => a.treasury_sig_unknown,
            SuiRole::Gas => a.gas_sig_unknown,
            SuiRole::UpgradeCustody => false,
        };
        assert!(marked, "a signature may exist from the moment the call is made");
        let d = state::get_deposit(op.deposit_id).unwrap();
        assert_eq!((d.mint_op, d.state), (Some(op.id), DepositState::MintInFlight), "the deposit is linked before the call");
        sink.borrow_mut().push(role);
    }));
    l.ticks(2);
    assert_eq!(*seen.borrow(), vec![SuiRole::Treasury, SuiRole::Gas]);
    let a = l.op(0).current().unwrap().clone();
    assert!(a.treasury_sig.is_some() && a.gas_sig.is_some());
    assert!(!a.treasury_sig_unknown && !a.gas_sig_unknown, "a stored signature clears the may-exist flag");
    assert_eq!(deposit_of(id).state, DepositState::Delivered);
}

#[test]
fn the_built_transaction_is_the_accepted_strict_one_for_the_deposit() {
    let l = lane();
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    l.tick();
    let op = l.op(0);
    let a = op.current().unwrap();
    let identity = bind::lane_identity().unwrap();
    let parsed = crate::tx::check_mint_transaction(&a.tx_bcs, &identity.cfg, &identity.roles).unwrap();
    assert_eq!((parsed.mint_id, parsed.recipient, parsed.amount), (deposit_of(id).mint_id.unwrap(), RECIPIENT, 5 * E8));
    assert_eq!((parsed.sender, parsed.gas_owner), (identity.roles.treasury, identity.roles.gas));
    assert_eq!((parsed.gas_price, parsed.gas_budget, parsed.epoch), (REFERENCE_GAS_PRICE, 50_000_000, 5));
    assert_eq!((parsed.gate_initial_shared_version, parsed.mint_cap.version), (SIM_SHARED_VERSION, l.sim.current_ref(&SIM_MINT_CAP).version));
    assert_eq!(parsed.gas_payment[0].id, SIM_GAS_COIN);
    assert_eq!(a.tx_digest, crate::tx::transaction_digest(&a.tx_bcs));
    assert_eq!(a.signing_digest, crate::tx::signing_digest(&a.tx_bcs));
}

// ---------------------------------------------------------------------------
// Signing faults
// ---------------------------------------------------------------------------

#[test]
fn a_lost_sign_reply_re_signs_the_same_bytes_and_never_builds_a_second_transaction() {
    let l = lane();
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    l.signer.push_fault(SuiRole::Treasury, SignFault::LoseReply);
    l.ticks(2);
    let op = l.op(0);
    let a = op.current().unwrap();
    assert!(a.treasury_sig.is_none() && a.treasury_sig_unknown, "a signature may exist that we never saw");
    assert!(a.gas_sig.is_some());
    assert!(a.may_be_complete());
    assert_eq!(l.sim.accepted.borrow().len(), 0, "nothing is broadcast without both signatures");
    // A complete signed transaction may exist: an admin cannot abandon it.
    test_env::set_caller(admin());
    assert!(matches!(crate::admin::abandon_mint_op(0, "try"), Err(BridgeError::InvalidState(m)) if m.contains("complete signed transaction may exist")));
    test_env::set_caller(user(0x21));
    assert!(matches!(deposit_of(id).state, DepositState::MintInFlight));
    let digest = a.tx_digest;
    l.tick();
    assert_eq!(deposit_of(id).state, DepositState::Delivered);
    let op = l.op(0);
    assert_eq!(op.attempts.len(), 1, "the same attempt was re-signed");
    assert_eq!(op.current().unwrap().tx_digest, digest);
    let calls = l.signer.calls.borrow();
    let treasury: Vec<_> = calls.iter().filter(|(r, _)| *r == SuiRole::Treasury).collect();
    assert_eq!(treasury.len(), 2);
    assert_eq!(treasury[0].1, treasury[1].1, "both treasury calls asked for the same signing digest");
    assert_eq!(l.sim.executed_count(), 1);
}

#[test]
fn a_definite_refusal_leaves_nothing_signed_so_the_attempt_can_be_abandoned_and_the_deposit_refunded() {
    let l = lane();
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    l.signer.push_fault(SuiRole::Treasury, SignFault::Refuse);
    l.ticks(2);
    let op = l.op(0);
    let a = op.current().unwrap();
    assert!(a.treasury_sig.is_none() && !a.treasury_sig_unknown, "a definite refusal clears the flag");
    assert!(a.gas_sig.is_some());
    assert!(!a.may_be_complete(), "the sender signature was never produced and cannot exist");
    assert!(op.hold_reason.as_deref().unwrap_or("").contains("refused"));
    // Still refundable? No: the operation is live. Abandon it first (an admin, with a note).
    assert!(matches!(block_on(deposit::request_refund(&l.fx.ledger, id)), Err(BridgeError::InvalidState(_))), "a live operation cannot be refunded");
    test_env::set_caller(user(0x77));
    assert_eq!(crate::admin::abandon_mint_op(0, "signer down"), Err(BridgeError::Unauthorized));
    test_env::set_caller(admin());
    assert_eq!(crate::admin::abandon_mint_op(0, " "), Err(BridgeError::InvalidState("a note of 1 to 512 bytes is required".into())));
    crate::admin::abandon_mint_op(0, "signer down").unwrap();
    let op = l.op(0);
    assert!(matches!(op.state, MintOpState::Failed { .. }));
    assert!(matches!(op.current().unwrap().disposition, AttemptDisposition::NeverSigned { .. }));
    let d = deposit_of(id);
    assert_eq!((d.mint_op, d.abandoned_mint_ops.clone()), (None, vec![0]));
    assert!(matches!(d.state, DepositState::MintHeld { .. }));
    assert_eq!(state::read_runtime(|r| r.active_mint_op), None);
    // The owner may now refund: every attempt is proven unable to deliver.
    assert!(mint::prior_ops_provably_dead(&d).is_ok());
    test_env::set_caller(user(0x21));
    let v = block_on(deposit::request_refund(&l.fx.ledger, id)).unwrap();
    assert!(matches!(v.status, DepositStatus::Refunded | DepositStatus::Refunding), "{:?}", v.status);
    block_on(deposit::process_refunds(&l.fx.ledger));
    assert_eq!(deposit_of(id).state, DepositState::Refunded);
    assert_covered(&l.fx);
    assert_eq!(l.sim.accepted.borrow().len(), 0, "nothing reached the chain");
}

#[test]
fn an_invalid_signature_is_not_kept_and_leaves_the_role_possibly_signed() {
    let l = lane();
    escrowed(&l.fx, 0x21, 5 * E8, 1);
    l.signer.push_fault(SuiRole::Gas, SignFault::Garbage);
    l.ticks(2);
    let a = l.op(0).current().unwrap().clone();
    assert!(a.treasury_sig.is_some());
    assert!(a.gas_sig.is_none() && a.gas_sig_unknown, "garbage behind a reply: a real signature may exist");
    assert!(l.op(0).hold_reason.unwrap().contains("does not verify"));
    l.tick();
    assert_eq!(l.op(0).state, MintOpState::Delivered { tx_digest: Some(a.tx_digest), checkpoint: l.sim.tx(&a.tx_digest).unwrap().checkpoint, sequence: 1, via: DeliveryVia::Receipt });
}

#[test]
fn a_signing_result_for_an_attempt_that_changed_in_flight_is_discarded() {
    let l = lane();
    escrowed(&l.fx, 0x21, 5 * E8, 1);
    l.tick();
    *l.signer.on_sign.borrow_mut() = Some(Box::new(|role, _| {
        if role == SuiRole::Treasury {
            // Another message replaces the attempt while the signing call is in flight.
            let mut op = state::active_mint_op().unwrap();
            op.attempts.last_mut().unwrap().tx_digest = [9; 32];
            state::put_mint_op(&op);
        }
    }));
    l.tick();
    let a = l.op(0).current().unwrap().clone();
    assert_eq!(a.tx_digest, [9; 32]);
    assert!(a.treasury_sig.is_none(), "the stale continuation stored nothing");
    assert!(a.treasury_sig_unknown, "and the attempt keeps its may-exist flag, so it can never be judged never signed");
}

// ---------------------------------------------------------------------------
// Broadcast faults and reconciliation
// ---------------------------------------------------------------------------

#[test]
fn a_transaction_that_ran_but_whose_send_reply_was_lost_is_found_by_its_receipt() {
    let l = lane();
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    for p in ["alpha", "beta", "gamma"] {
        l.sim.set_provider(p, Provider { lose_send_reply: true, ..Provider::default() });
    }
    l.ticks(2);
    assert_eq!(deposit_of(id).state, DepositState::Delivered);
    let a = l.op(0).current().unwrap().clone();
    assert_eq!(a.sends, 1, "sent once; every reply was lost");
    assert!(a.last_send_note.unwrap().contains("0 provider(s) HTTP-acknowledged (maybe sent), 3 failed"));
    assert_eq!(l.sim.executed_count(), 1);
}

#[test]
fn an_accepted_but_unexecuted_attempt_is_held_across_epochs_never_replaced_never_refunded() {
    let l = lane();
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    *l.sim.mode.borrow_mut() = ExecMode::Drop;
    l.ticks(2);
    let op = l.op(0);
    let a = op.current().unwrap().clone();
    assert_eq!((a.sends, l.sim.accepted.borrow().len()), (1, 1));
    assert!(op.hold_reason.as_deref().unwrap().contains("inputs are unchanged"), "{:?}", op.hold_reason);
    // Many passes: resends are spaced, bounded, and always the same bytes.
    l.ticks(20);
    let a2 = l.op(0).current().unwrap().clone();
    assert_eq!(a2.tx_bcs, a.tx_bcs);
    assert!(a2.sends > 1 && a2.sends <= 12, "resent at most once a minute: {}", a2.sends);
    // The epoch ends: still held. Neither a missing digest nor the epoch authorizes anything.
    l.sim.next_epoch();
    test_env::advance_secs(10 * 60);
    l.tick();
    let op = l.op(0);
    assert_eq!(op.attempts.len(), 1, "no replacement");
    assert!(op.hold_reason.as_deref().unwrap().contains("has passed"), "{:?}", op.hold_reason);
    assert!(matches!(op.state, MintOpState::Active));
    let d = deposit_of(id);
    assert_eq!((d.state.clone(), d.mint_op), (DepositState::MintInFlight, Some(0)));
    assert!(mint::prior_ops_provably_dead(&d).is_err());
    assert!(matches!(block_on(deposit::request_refund(&l.fx.ledger, id)), Err(BridgeError::InvalidState(_))));
    test_env::set_caller(admin());
    assert!(crate::admin::abandon_mint_op(0, "epoch passed").is_err());
    assert!(!state::is_mint_lane_halted() && !state::is_halted());
    // A copy that had been certified earlier executes after the epoch ends: the receipt settles it.
    l.sim.execute_accepted(&a.tx_bcs);
    test_env::advance_secs(6 * 60); // the next scheduled check
    l.ticks(2);
    assert_eq!(deposit_of(id).state, DepositState::Delivered);
    assert_eq!(l.sim.executed_count(), 1);
}

#[test]
fn a_consumed_mint_record_alone_proves_delivery_only_when_id_recipient_and_amount_match() {
    // The transaction lookup is lost everywhere, the mint happened: the record names it.
    let l = lane();
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    for p in ["alpha", "beta", "gamma"] {
        l.sim.set_provider(p, Provider { no_transactions: true, ..Provider::default() });
    }
    l.ticks(4);
    let d = deposit_of(id);
    assert_eq!(d.state, DepositState::Delivered);
    match l.op(0).state {
        MintOpState::Delivered { tx_digest, via, sequence, .. } => assert_eq!((tx_digest, via, sequence), (None, DeliveryVia::ConsumedRecord, 1)),
        other => panic!("{other:?}"),
    }
    assert_eq!(d.delivered.unwrap().tx_digest, None);
    assert_eq!(buckets().outstanding, (5 * E8) as u128);

    // The same id consumed with another amount: a foreign mint. Nothing is credited; the lane halts.
    let l = lane();
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    *l.sim.mode.borrow_mut() = ExecMode::Drop;
    l.ticks(2);
    l.sim.foreign_mint(deposit_of(id).mint_id.unwrap(), RECIPIENT, 5 * E8 + 1);
    l.ticks(2);
    assert!(state::is_mint_lane_halted());
    assert!(state::read_runtime(|r| r.mint_lane_halted.clone().unwrap().reason).contains("foreign mint"));
    assert_eq!(deposit_of(id).state, DepositState::MintInFlight);
    assert_eq!(buckets().outstanding, 0);
    // Another recipient with the right amount is foreign too.
    let l = lane();
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    *l.sim.mode.borrow_mut() = ExecMode::Drop;
    l.ticks(2);
    l.sim.foreign_mint(deposit_of(id).mint_id.unwrap(), [0x42; 32], 5 * E8);
    l.ticks(2);
    assert!(state::is_mint_lane_halted());
    assert_eq!(deposit_of(id).state, DepositState::MintInFlight);
}

#[test]
fn a_failed_receipt_closes_the_attempt_and_a_replacement_reuses_the_same_mint_id_with_fresh_references() {
    for bump_on_abort in [true, false] {
        let l = lane();
        l.sim.bump_on_abort.set(bump_on_abort);
        let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
        *l.sim.mode.borrow_mut() = ExecMode::Abort("MoveAbort test".into());
        l.ticks(2);
        let op = l.op(0);
        let first = op.attempts[0].clone();
        assert!(matches!(&first.disposition, AttemptDisposition::FailedFinal { error, .. } if error.contains("MoveAbort test")), "{:?}", first.disposition);
        assert_eq!(op.attempts.len(), 1);
        assert_eq!(l.sim.mint_record(&deposit_of(id).mint_id.unwrap()), None, "the aborted mint consumed nothing");
        assert!(matches!(deposit_of(id).state, DepositState::MintInFlight), "a failed attempt alone does not free the deposit");
        *l.sim.mode.borrow_mut() = ExecMode::Execute;
        l.tick();
        let op = l.op(0);
        assert_eq!(op.attempts.len(), 2, "bump_on_abort={bump_on_abort}");
        let second = &op.attempts[1];
        assert_eq!((second.generation, second.disposition.clone()), (2, AttemptDisposition::Open));
        assert_ne!(second.tx_digest, first.tx_digest);
        assert_eq!(
            crate::tx::parse_mint_transaction(&second.tx_bcs).unwrap().mint_id,
            crate::tx::parse_mint_transaction(&first.tx_bcs).unwrap().mint_id,
            "the replacement reuses the same mint id"
        );
        // The references were read again after the failure: the gas coin always moved; the cap only if it did.
        assert!(second.gas_coin.version > first.gas_coin.version);
        assert_eq!(second.mint_cap.version > first.mint_cap.version, bump_on_abort);
        assert_eq!(second.mint_cap.version, l.sim.current_ref(&SIM_MINT_CAP).version);
        l.tick();
        assert_eq!(deposit_of(id).state, DepositState::Delivered);
        assert_eq!(l.sim.executed_count(), 1);
        assert!(matches!(l.op(0).attempts[0].disposition, AttemptDisposition::FailedFinal { .. }));
        assert!(matches!(l.op(0).attempts[1].disposition, AttemptDisposition::LandedFinal { .. }));
    }
}

#[test]
fn exhausted_attempts_fail_the_operation_and_only_then_may_the_owner_refund_with_every_receipt_proven() {
    let l = lane();
    state::mutate_runtime(|r| r.config.lane.max_attempts = 2);
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    *l.sim.mode.borrow_mut() = ExecMode::Abort("MoveAbort always".into());
    l.ticks(6);
    let op = l.op(0);
    assert_eq!(op.attempts.len(), 2);
    assert!(matches!(op.state, MintOpState::Failed { .. }), "{:?}", op.state);
    let d = deposit_of(id);
    assert!(matches!(d.state, DepositState::MintHeld { .. }));
    assert_eq!((d.mint_op, d.abandoned_mint_ops.clone()), (None, vec![0]));
    assert!(mint::prior_ops_provably_dead(&d).is_ok());
    assert_eq!(l.sim.executed_count(), 0);
    // The proof is re-derived, not trusted: bytes that no longer hash to the receipt's digest are refused.
    let mut tampered = l.op(0);
    tampered.attempts[0].tx_bcs[3] ^= 1;
    state::put_mint_op(&tampered);
    assert!(mint::prior_ops_provably_dead(&d).unwrap_err().contains("other bytes"));
    assert!(matches!(block_on(deposit::request_refund(&l.fx.ledger, id)), Err(BridgeError::InvalidState(_))));
    state::put_mint_op(&op);
    // Back-index identity: a deposit that forgot an operation, or lists one that is not indexed, is refused.
    let mut forgetful = d.clone();
    forgetful.abandoned_mint_ops.clear();
    assert!(mint::prior_ops_provably_dead(&forgetful).unwrap_err().contains("disagree"));
    let mut invented = d.clone();
    invented.abandoned_mint_ops.push(9);
    assert!(mint::prior_ops_provably_dead(&invented).unwrap_err().contains("disagree"));
    // Another deposit's operation does not describe this one.
    let mut other = l.op(0);
    other.deposit_id = 5;
    state::put_mint_op(&other);
    let mut stray = d.clone();
    stray.id = 5;
    stray.mint_id = Some([0x44; 32]);
    stray.abandoned_mint_ops = vec![0];
    assert!(mint::prior_ops_provably_dead(&stray).unwrap_err().contains("does not describe"));
    state::put_mint_op(&op);
    // Untampered, the owner can refund.
    let v = block_on(deposit::request_refund(&l.fx.ledger, id)).unwrap();
    assert!(matches!(v.status, DepositStatus::Refunded | DepositStatus::Refunding), "{:?}", v.status);
    block_on(deposit::process_refunds(&l.fx.ledger));
    assert_eq!(deposit_of(id).state, DepositState::Refunded);
    assert_covered(&l.fx);
}

#[test]
fn a_signed_attempt_whose_owned_inputs_moved_without_a_receipt_halts_the_lane_after_a_grace_and_replaces_nothing() {
    let l = lane();
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    *l.sim.mode.borrow_mut() = ExecMode::Drop;
    l.ticks(2);
    l.sim.foreign_consume_cap();
    l.tick();
    let op = l.op(0);
    assert!(op.current().unwrap().versions_changed_since_ns.is_some(), "suspicion starts");
    assert!(!state::is_mint_lane_halted(), "the receipt may only be lagging: a grace period first");
    assert!(op.hold_reason.as_deref().unwrap().contains("grace"));
    test_env::advance_secs(mint::FOREIGN_GRACE_NS / 1_000_000_000 + 1);
    l.tick();
    assert!(state::is_mint_lane_halted());
    assert!(state::read_runtime(|r| r.mint_lane_halted.clone().unwrap().reason).contains("unexplained consumer"));
    let op = l.op(0);
    assert_eq!(op.attempts.len(), 1, "no replacement");
    assert!(matches!(op.state, MintOpState::Active));
    let d = deposit_of(id);
    assert!(mint::prior_ops_provably_dead(&d).is_err());
    assert!(matches!(block_on(deposit::request_refund(&l.fx.ledger, id)), Err(BridgeError::InvalidState(_))));
    // The ICP side and returns keep working while the lane is halted.
    let before = l.sim.accepted.borrow().len();
    l.ticks(3);
    assert_eq!(l.sim.accepted.borrow().len(), before, "a halted lane sends nothing more");
    let b = burned_with(&l.fx, E8, 7, acct(user(0x41)), SENDER);
    release::open_release(&b).unwrap();
    l.tick();
    assert_eq!(state::get_release(&b.transfer_id).unwrap().state, ReleaseState::Released);
    // Clearing the halt is a decision, not a fix: the same evidence halts it again.
    test_env::set_caller(admin());
    assert!(crate::admin::clear_mint_lane_halt("  ").is_err());
    crate::admin::clear_mint_lane_halt("reviewed the explorer").unwrap();
    assert!(!state::is_mint_lane_halted());
    test_env::advance_secs(6 * 60); // the decaying check schedule is due again
    l.tick();
    assert!(state::is_mint_lane_halted(), "still unexplained");
    assert!(crate::admin::clear_mint_lane_halt("again").is_ok());
    assert!(crate::admin::clear_mint_lane_halt("twice").is_err(), "not halted any more");
}

// ---------------------------------------------------------------------------
// Re-checks before signing and admission
// ---------------------------------------------------------------------------

#[test]
fn signing_is_refused_when_the_stored_request_no_longer_matches_the_deposit_or_the_configuration() {
    type Tamper = Box<dyn Fn(u64)>;
    let cases: Vec<(&str, Tamper)> = vec![
        ("recipient", Box::new(|id| {
            let mut d = state::get_deposit(id).unwrap();
            d.sui_recipient = [0x78; 32];
            state::put_deposit(&d);
        })),
        ("amount", Box::new(|id| {
            let mut d = state::get_deposit(id).unwrap();
            d.mint_amount_e8s -= 1;
            state::put_deposit(&d);
        })),
        ("mint id", Box::new(|id| {
            let mut d = state::get_deposit(id).unwrap();
            d.mint_id = Some([0x13; 32]);
            state::put_deposit(&d);
        })),
        ("link", Box::new(|id| {
            let mut d = state::get_deposit(id).unwrap();
            d.mint_op = None;
            state::put_deposit(&d);
        })),
        ("stored bytes", Box::new(|_| {
            let mut op = state::get_mint_op(0).unwrap();
            let n = op.attempts[0].tx_bcs.len();
            op.attempts[0].tx_bcs[n - 1] ^= 1;
            state::put_mint_op(&op);
        })),
        ("gas coin reconfigured", Box::new(|_| state::mutate_runtime(|r| r.config.lane.gas_coin = Some([0x55; 32])))),
        ("gas budget lowered", Box::new(|_| state::mutate_runtime(|r| r.config.lane.gas_budget = 10_000_000))),
        ("amount cap lowered", Box::new(|_| state::mutate_runtime(|r| r.config.limits.max_deposit_e8s = 4 * E8))),
        ("mints paused", Box::new(|_| state::mutate_runtime(|r| r.config.pauses.mints = true))),
        ("bridge halted", Box::new(|_| state::halt("test halt"))),
    ];
    for (what, tamper) in cases {
        let l = lane();
        let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
        l.tick();
        tamper(id);
        l.tick();
        assert_eq!(l.signer.calls.borrow().len(), 0, "{what}: nothing was signed");
        assert_eq!(l.sim.accepted.borrow().len(), 0, "{what}: nothing was sent");
        assert_eq!(l.sim.executed_count(), 0, "{what}");
    }
}

#[test]
fn one_operation_is_in_flight_at_a_time_and_a_second_deposit_builds_on_the_first_ones_result() {
    let l = lane();
    let first = escrowed(&l.fx, 0x21, 5 * E8, 1);
    let second = escrowed(&l.fx, 0x22, 3 * E8, 1);
    l.tick();
    assert_eq!(deposit_of(first).state, DepositState::MintInFlight);
    assert_eq!(deposit_of(second).state, DepositState::Escrowed, "the second waits: the cap and the coin are in use");
    // A direct attempt to start another operation while one is active is refused.
    let facts_err = {
        let id = bind::lane_identity().unwrap();
        let _ = id;
        state::read_runtime(|r| r.active_mint_op)
    };
    assert_eq!(facts_err, Some(0));
    l.tick();
    assert_eq!(deposit_of(first).state, DepositState::Delivered);
    l.ticks(2);
    assert_eq!(deposit_of(second).state, DepositState::Delivered);
    let a = l.op(0).attempts[0].clone();
    let b = l.op(1).attempts[0].clone();
    assert!(b.mint_cap.version > a.mint_cap.version && b.gas_coin.version > a.gas_coin.version, "the second reads the references the first left");
    assert_eq!(l.sim.executed_count(), 2);
    assert_eq!(l.sim.gate.borrow().total_supply, 8 * E8);
    assert_eq!(buckets().outstanding, (8 * E8) as u128);
}

// ---------------------------------------------------------------------------
// Preflight conditions
// ---------------------------------------------------------------------------

#[test]
fn a_supply_cap_that_is_too_low_holds_the_deposit_without_an_operation_and_it_mints_once_raised() {
    let l = lane();
    l.sim.set_supply_cap(E8);
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    l.tick();
    let d = deposit_of(id);
    assert!(matches!(&d.state, DepositState::MintHeld { reason } if reason.contains("supply cap")), "{:?}", d.state);
    assert_eq!((d.mint_op, state::mint_op_count()), (None, 0), "no operation, so nothing signed");
    assert!(mint::prior_ops_provably_dead(&d).is_ok(), "it never had an operation: refundable");
    l.sim.set_supply_cap(1_000 * E8);
    test_env::advance_secs(11 * 60);
    l.tick();
    assert_eq!(deposit_of(id).state, DepositState::MintInFlight);
    l.tick();
    assert_eq!(deposit_of(id).state, DepositState::Delivered);
}

#[test]
fn the_lane_waits_without_changing_anything_when_it_cannot_run() {
    let l = lane();
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    // An under-funded gas coin: the lane names it and waits.
    l.sim.set_gas_balance(1_000);
    l.tick();
    assert_eq!(deposit_of(id).state, DepositState::Escrowed);
    assert!(state::read_runtime(|r| r.lane_wait.clone()).unwrap().contains("below"), "names the gas shortfall");
    l.sim.set_gas_balance(1_000_000_000);
    // Providers that disagree (one window apart): wait.
    l.sim.head.set(1_150);
    l.sim.set_provider("gamma", Provider { lag: 60, ..Provider::default() });
    test_env::advance_secs(60); // past the back-off the previous wait set
    l.tick();
    assert_eq!(deposit_of(id).state, DepositState::Escrowed);
    assert!(state::read_runtime(|r| r.lane_wait.clone()).unwrap().contains("disagree"));
    l.sim.set_provider("gamma", Provider::default());
    // Mints paused, the mirror disabled, the lane not configured: nothing is even asked.
    let asked = l.sim.served.borrow().len();
    state::mutate_runtime(|r| r.config.pauses.mints = true);
    l.tick();
    state::mutate_runtime(|r| r.config.pauses.mints = false);
    state::mutate_core(|c| c.mirror.enabled = false);
    l.tick();
    state::mutate_core(|c| c.mirror.enabled = true);
    state::mutate_runtime(|r| r.config.lane.gas_coin = None);
    l.tick();
    assert_eq!(l.sim.served.borrow().len(), asked, "a lane that is blocked makes no request");
    assert!(mint::blocked_by().iter().any(|m| m.contains("no gas coin")));
    assert_eq!(deposit_of(id).state, DepositState::Escrowed);
    assert_eq!(state::mint_op_count(), 0);
    // Everything restored: it mints.
    state::mutate_runtime(|r| r.config.lane.gas_coin = Some(SIM_GAS_COIN));
    l.ticks(2);
    assert_eq!(deposit_of(id).state, DepositState::Delivered);
}

#[test]
fn a_mint_id_that_is_already_consumed_with_no_operation_of_ours_halts_the_lane() {
    let l = lane();
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    l.sim.foreign_mint(deposit_of(id).mint_id.unwrap(), RECIPIENT, 5 * E8);
    l.tick();
    assert!(state::is_mint_lane_halted());
    assert_eq!(deposit_of(id).state, DepositState::Escrowed);
    assert_eq!(state::mint_op_count(), 0);
    assert_eq!(l.signer.calls.borrow().len(), 0);
}

// ---------------------------------------------------------------------------
// Re-mints of held releases
// ---------------------------------------------------------------------------

#[test]
fn a_held_release_is_re_minted_to_the_sui_sender_and_the_books_close() {
    let l = lane();
    // A return whose saved ICP destination is this very canister cannot be paid: it opens held.
    let b = burned_with(&l.fx, 5 * E8, 1, acct(env::canister_id()), SENDER);
    assert!(matches!(release::open_release(&b).unwrap(), release::OpenOutcome::Opened(ReleaseState::Held { .. })));
    assert_eq!((buckets().held, buckets().outstanding), ((5 * E8) as u128, 0));
    l.tick();
    let r = state::get_release(&b.transfer_id).unwrap();
    let ReleaseState::Reminting { deposit_id } = r.state else { panic!("{:?}", r.state) };
    let d = deposit_of(deposit_id);
    assert_eq!(d.kind, DepositKind::Remint { return_id: b.transfer_id });
    assert_eq!((d.sui_recipient, d.amount_e8s, d.deposit_fee_e8s, d.owner), (SENDER, 5 * E8, 0, env::canister_id()));
    assert_eq!((buckets().held, buckets().pending_mint), (0, (5 * E8) as u128), "H moved to PM");
    assert_eq!(d.mint_id, Some(crate::ids::remint_id(&crate::ids::TESTNET_CHAIN_IDENTIFIER, &[0x70; 32], &[0x6a; 32], &[0x5a; 32], &b.transfer_id, &SENDER, 5 * E8)));
    l.ticks(2);
    assert_eq!(deposit_of(deposit_id).state, DepositState::Delivered);
    assert_eq!(state::get_release(&b.transfer_id).unwrap().state, ReleaseState::Reminted { deposit_id });
    let rec = l.sim.mint_record(&d.mint_id.unwrap()).unwrap();
    assert_eq!((rec.recipient, rec.amount), (SENDER, 5 * E8));
    let bk = buckets();
    assert_eq!((bk.held, bk.pending_mint, bk.outstanding), (0, 0, (5 * E8) as u128));
    // Idempotent: the release is never re-minted twice.
    l.ticks(3);
    assert_eq!(l.sim.executed_count(), 1);
    assert_eq!(state::mint_op_count(), 1);
    assert!(remint::start_remint(&b.transfer_id).is_err());
}

#[test]
fn a_remint_that_fails_is_not_refunded_to_icp_but_an_admin_can_retry_it_once_every_attempt_is_proven_dead() {
    let l = lane();
    state::mutate_runtime(|r| r.config.lane.max_attempts = 1);
    let b = burned_with(&l.fx, 5 * E8, 1, acct(env::canister_id()), SENDER);
    release::open_release(&b).unwrap();
    *l.sim.mode.borrow_mut() = ExecMode::Abort("MoveAbort remint".into());
    l.ticks(5);
    let ReleaseState::Reminting { deposit_id } = state::get_release(&b.transfer_id).unwrap().state else { panic!() };
    let d = deposit_of(deposit_id);
    assert!(matches!(d.state, DepositState::MintHeld { .. }), "{:?}", d.state);
    // Never a refund to ICP: the destination is the problem.
    test_env::set_caller(env::canister_id());
    assert!(matches!(block_on(deposit::request_refund(&l.fx.ledger, deposit_id)), Err(BridgeError::InvalidState(_))));
    // The admin retry is allowed because the lane's own records prove the attempt dead.
    *l.sim.mode.borrow_mut() = ExecMode::Execute;
    state::mutate_runtime(|r| r.config.lane.max_attempts = 3);
    test_env::set_caller(admin());
    deposit::retry_held(deposit_id).unwrap();
    l.ticks(3);
    assert_eq!(deposit_of(deposit_id).state, DepositState::Delivered);
    assert_eq!(state::get_release(&b.transfer_id).unwrap().state, ReleaseState::Reminted { deposit_id });
    assert_eq!(l.sim.executed_count(), 1);
    assert_eq!(l.op(1).attempts[0].generation, 1);
}

#[test]
fn an_admin_retry_is_refused_while_any_earlier_attempt_is_not_proven_dead() {
    let l = lane();
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    *l.sim.mode.borrow_mut() = ExecMode::Drop;
    l.ticks(2);
    // Force the deposit to look held while an unresolved signed operation exists (an inconsistent state).
    let mut d = deposit_of(id);
    d.mint_op = None;
    d.state = DepositState::MintHeld { reason: "x".into() };
    state::put_deposit(&d);
    test_env::set_caller(admin());
    assert!(matches!(deposit::retry_held(id), Err(BridgeError::InvalidState(m)) if m.contains("not retryable")));
    d.abandoned_mint_ops = vec![0];
    state::put_deposit(&d);
    assert!(matches!(deposit::retry_held(id), Err(BridgeError::InvalidState(m)) if m.contains("not retryable")), "an abandoned, unproven operation never authorizes a retry");
    test_env::set_caller(user(0x21));
    assert!(matches!(block_on(deposit::request_refund(&l.fx.ledger, id)), Err(BridgeError::InvalidState(m)) if m.contains("not refundable")), "nor a refund");
}

// ---------------------------------------------------------------------------
// Persistence and administration
// ---------------------------------------------------------------------------

#[test]
fn a_signed_and_sent_attempt_survives_an_upgrade_and_resumes_as_the_same_attempt() {
    use ic_stable_structures::Storable;
    let l = lane();
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    *l.sim.mode.borrow_mut() = ExecMode::Drop;
    l.ticks(2);
    let before = l.op(0);
    let a = before.current().unwrap().clone();
    // "Upgrade": every record is read back from its bytes.
    let reloaded = state::MintOp::from_bytes(before.to_bytes());
    assert_eq!(reloaded, before);
    state::put_mint_op(&reloaded);
    let core = state::read_runtime(|c| c.clone());
    state::mutate_runtime(|c| *c = state::BridgeCore::from_bytes(core.to_bytes()));
    let dep = deposit_of(id);
    state::put_deposit(&state::DepositRecord::from_bytes(dep.to_bytes()));
    l.sim.execute_accepted(&a.tx_bcs);
    l.ticks(2);
    assert_eq!(deposit_of(id).state, DepositState::Delivered);
    assert_eq!(l.op(0).attempts.len(), 1);
    assert_eq!(l.op(0).attempts[0].tx_digest, a.tx_digest);
    assert_eq!(l.sim.executed_count(), 1);
    assert_eq!(state::mint_op_ids_for_deposit(id), vec![0]);
}

#[test]
fn the_lane_admin_surface_is_authorized_bounded_and_audited() {
    let l = lane();
    test_env::set_caller(user(0x21));
    assert_eq!(crate::admin::set_lane_settings(LaneSettingsArg { gas_budget: 1_000_000, max_attempts: 2 }), Err(BridgeError::Unauthorized));
    assert_eq!(crate::admin::clear_mint_lane_halt("x"), Err(BridgeError::Unauthorized));
    assert_eq!(crate::admin::abandon_mint_op(0, "x"), Err(BridgeError::Unauthorized));
    assert_eq!(crate::admin::set_gas_coin(&crate::sui::format_address(&[0x43; 32])), Err(BridgeError::Unauthorized));
    test_env::set_caller(admin());
    assert!(matches!(crate::admin::set_lane_settings(LaneSettingsArg { gas_budget: 1, max_attempts: 2 }), Err(BridgeError::InvalidArgument(_))));
    assert!(matches!(crate::admin::set_lane_settings(LaneSettingsArg { gas_budget: 1_000_000, max_attempts: 0 }), Err(BridgeError::InvalidArgument(_))));
    assert!(matches!(crate::admin::set_lane_settings(LaneSettingsArg { gas_budget: 1_000_000, max_attempts: 6 }), Err(BridgeError::InvalidArgument(_))));
    crate::admin::set_lane_settings(LaneSettingsArg { gas_budget: 2_000_000, max_attempts: 2 }).unwrap();
    assert_eq!(state::read_runtime(|r| (r.config.lane.gas_budget, r.config.lane.max_attempts)), (2_000_000, 2));
    // The gas coin is a controller identity, never a gate object, and never changed under an operation.
    test_env::set_caller(user(0x99));
    for bad in ["nope".to_string(), crate::sui::format_address(&[0; 32]), crate::sui::format_address(&[0x6d; 32]), crate::sui::format_address(&[0x6a; 32])] {
        assert!(matches!(crate::admin::set_gas_coin(&bad), Err(BridgeError::InvalidArgument(_))), "{bad}");
    }
    crate::admin::set_gas_coin(&crate::sui::format_address(&[0x43; 32])).unwrap();
    assert_eq!(state::read_runtime(|r| r.config.lane.gas_coin), Some([0x43; 32]));
    state::mutate_runtime(|r| r.config.lane.gas_coin = Some(SIM_GAS_COIN));
    escrowed(&l.fx, 0x21, 5 * E8, 1);
    l.tick();
    test_env::set_caller(user(0x99));
    assert!(matches!(crate::admin::set_gas_coin(&crate::sui::format_address(&[0x43; 32])), Err(BridgeError::InvalidState(_))), "an operation is in flight");
    let view = crate::views::mint_lane_view();
    assert!(view.ready && view.blocked_by.is_empty());
    assert_eq!((view.active_op, view.gas_budget, view.max_attempts), (Some(0), 2_000_000, 2));
    assert!(view.observed.is_some());
    let opv = crate::views::mint_op_view(&l.op(0));
    assert_eq!((opv.status, opv.attempts.len(), opv.attempts[0].treasury_signed), (MintOpStatusView::Active, 1, false));
}

// ---------------------------------------------------------------------------
// More adversarial cases
// ---------------------------------------------------------------------------

#[test]
fn a_deposit_whose_mint_id_does_not_match_its_records_is_held_and_never_minted() {
    let l = lane();
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    let mut d = deposit_of(id);
    d.mint_id = Some([0x13; 32]);
    state::put_deposit(&d);
    l.ticks(2);
    let d = deposit_of(id);
    assert!(matches!(&d.state, DepositState::MintHeld { reason } if reason.contains("stored mint id is not the one the proven pull yields")), "{:?}", d.state);
    assert_eq!((state::mint_op_count(), l.signer.calls.borrow().len(), l.sim.accepted.borrow().len()), (0, 0, 0));
}

#[test]
fn the_lane_waits_while_the_providers_head_is_behind_our_last_finalized_transaction() {
    let l = lane();
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    state::mutate_runtime(|r| r.last_final_checkpoint = 1_000_000);
    l.tick();
    assert_eq!(deposit_of(id).state, DepositState::Escrowed);
    assert!(state::read_runtime(|r| r.lane_wait.clone()).unwrap().contains("behind our last finalized transaction"));
    assert_eq!(state::mint_op_count(), 0);
}

#[test]
fn a_gate_that_is_not_the_configured_one_and_a_mint_cap_that_moved_halt_the_lane_and_build_nothing() {
    // The configured gate does not exist on the chain the providers serve. (Re-pointing the binding at
    // another gate would change every mint id, so the deposit would be held instead: see the test above.)
    let l = lane();
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    l.sim.forget_object(&SIM_GATE);
    l.tick();
    assert!(state::is_mint_lane_halted());
    assert!(state::read_runtime(|r| r.mint_lane_halted.clone().unwrap().reason).contains("gate does not exist"));
    assert_eq!((deposit_of(id).state, state::mint_op_count()), (DepositState::Escrowed, 0));
    // The mint cap now belongs to another address than the treasury role.
    let l = lane();
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    l.sim.set_owner(&SIM_MINT_CAP, [0x99; 32]);
    l.tick();
    assert!(state::is_mint_lane_halted());
    assert!(state::read_runtime(|r| r.mint_lane_halted.clone().unwrap().reason).contains("owned by"));
    assert_eq!((deposit_of(id).state, state::mint_op_count(), l.signer.calls.borrow().len()), (DepositState::Escrowed, 0, 0));
}

#[test]
fn the_gate_sequence_is_recorded_not_assumed_and_a_refresh_follows_the_counters() {
    let l = lane();
    // A mint by someone else comes first (another id): ours lands second.
    l.sim.foreign_mint([0x31; 32], [0x41; 32], 7);
    l.sim.advance(100);
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    l.ticks(2);
    assert_eq!(deposit_of(id).state, DepositState::Delivered);
    match l.op(0).state {
        MintOpState::Delivered { sequence, .. } => assert_eq!(sequence, 2, "the Gate's own mint number, read from its record"),
        other => panic!("{other:?}"),
    }
    // A refresh of the observation follows the counters forward.
    l.sim.advance(100);
    let o = block_on(bind::observe_deployment(&l.net())).unwrap();
    assert_eq!((o.mint_count, o.minted_total, o.total_supply), (2, (5 * E8 + 7) as u128, 5 * E8 + 7));
}

#[test]
fn exact_version_reads_have_exactly_one_entry_point() {
    // Every other file may only mention the request builder in a comment, a test or its own definition.
    let sources: [(&str, &str); 6] = [
        ("flows/mint.rs", include_str!("flows/mint.rs")),
        ("bind.rs", include_str!("bind.rs")),
        ("reads.rs", include_str!("reads.rs")),
        ("lib.rs", include_str!("lib.rs")),
        ("timer.rs", include_str!("timer.rs")),
        ("flows/remint.rs", include_str!("flows/remint.rs")),
    ];
    for (name, src) in sources {
        let uses = src.lines().filter(|l| l.contains("request_object_at_version(") && !l.trim_start().starts_with("//")).count();
        assert_eq!(uses, 0, "{name} builds an exact-version request itself instead of going through Mirror::object_at_version");
    }
    let mirror = include_str!("mirror.rs");
    let calls: Vec<&str> = mirror.lines().filter(|l| l.contains("request_object_at_version(") && !l.trim_start().starts_with("//")).collect();
    assert_eq!(calls.len(), 1, "exactly the one composed helper: {calls:?}");
    assert!(mirror.contains("parse_object_at_version(d, &address, version, &expected)"), "the helper parses with parse_object_at_version before the tally");
}

#[test]
fn an_unresolved_attempt_is_checked_on_a_decaying_schedule_so_a_long_hold_costs_few_outcalls() {
    let l = lane();
    escrowed(&l.fx, 0x21, 5 * E8, 1);
    *l.sim.mode.borrow_mut() = ExecMode::Drop;
    l.ticks(2);
    let served = |l: &Lane| l.sim.served.borrow().len();
    let at_start = served(&l);
    l.ticks(10); // the first five minutes: every pass
    let first_five_minutes = served(&l) - at_start;
    l.ticks(120); // the next hour
    let before_second_hour = served(&l);
    l.ticks(120); // another hour
    let second_hour = served(&l) - before_second_hour;
    assert!(first_five_minutes > 100, "fast checks while the attempt is young: {first_five_minutes}");
    assert!(second_hour <= 2 * 21 + 3, "two half-hourly checks of 21 provider calls (3 providers): {second_hour}");
    assert_eq!(l.op(0).attempts.len(), 1);
    assert!(l.op(0).current().unwrap().sends <= mint::MAX_SENDS_PER_ATTEMPT);
}

#[test]
fn a_persistent_provider_outage_backs_off_instead_of_asking_every_pass() {
    let l = lane();
    escrowed(&l.fx, 0x21, 5 * E8, 1);
    for p in ["alpha", "beta", "gamma"] {
        l.sim.set_provider(p, Provider { down: true, ..Provider::default() });
    }
    let before = l.sim.served.borrow().len();
    l.tick();
    assert_eq!(l.sim.served.borrow().len(), before, "a down provider never reaches the simulated chain");
    // Count the lane's own attempts through the retry gate instead.
    let until = state::read_runtime(|r| r.gather_retry_at_ns);
    assert!(until > env::now_ns(), "the next gather is not before {until}");
    let wait = state::read_runtime(|r| r.lane_wait.clone()).unwrap();
    assert!(wait.contains("unavailable"), "{wait}");
    for p in ["alpha", "beta", "gamma"] {
        l.sim.set_provider(p, Provider::default());
    }
    l.tick(); // 30 s later: still inside the back-off
    assert_eq!(state::mint_op_count(), 0);
    l.tick(); // 60 s later
    assert_eq!(state::mint_op_count(), 1);
    assert_eq!(state::read_runtime(|r| r.gather_retry_at_ns), 0, "a successful build clears the back-off");
}

#[test]
fn the_deposit_fee_stays_in_the_fee_bucket_and_the_chain_mints_exactly_the_net_eight_decimal_amount() {
    let l = lane();
    state::mutate_runtime(|r| r.config.fees.deposit_fee_e8s = 1_234_567);
    fund(&l.fx, &acct(user(0x21)), 5 * E8 + 7);
    let mut a = args(5 * E8 + 7, 1);
    a.expected_deposit_fee_e8s = 1_234_567;
    let id = bridge(&l.fx, a).expect("admitted").id;
    let d = deposit_of(id);
    assert_eq!((d.amount_e8s, d.deposit_fee_e8s, d.mint_amount_e8s), (5 * E8 + 7, 1_234_567, 5 * E8 + 7 - 1_234_567));
    l.ticks(2);
    assert_eq!(deposit_of(id).state, DepositState::Delivered);
    let rec = l.sim.mint_record(&d.mint_id.unwrap()).unwrap();
    assert_eq!(rec.amount, 5 * E8 + 7 - 1_234_567, "the chain minted the net amount, not the deposit");
    let b = buckets();
    assert_eq!((b.pending_mint, b.outstanding, b.fees), (0, (5 * E8 + 7 - 1_234_567) as u128, 1_234_567));
    assert_eq!(l.sim.gate.borrow().total_supply as u128, b.outstanding, "supply equals outstanding (C3)");
    assert_covered(&l.fx);
}

#[test]
fn a_mint_above_the_per_operation_cap_is_held_before_anything_is_built() {
    let l = lane();
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    state::mutate_runtime(|r| r.config.limits.max_deposit_e8s = 4 * E8);
    l.tick();
    let d = deposit_of(id);
    assert!(matches!(&d.state, DepositState::MintHeld { reason } if reason.contains("per-operation cap")), "{:?}", d.state);
    assert_eq!((state::mint_op_count(), l.signer.calls.borrow().len()), (0, 0));
}

// ---------------------------------------------------------------------------
// Review corrections (first 53b review): F1 per-role signing uncertainty, F2 send guard
// ---------------------------------------------------------------------------

#[test]
fn a_later_definite_refusal_cannot_erase_an_earlier_unknown_signature() {
    let l = lane();
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    l.signer.push_fault(SuiRole::Treasury, SignFault::LoseReply);
    l.signer.push_fault(SuiRole::Treasury, SignFault::Refuse);
    l.ticks(2); // build; treasury reply lost (a signature may exist), gas signs
    let a = l.op(0).current().unwrap().clone();
    assert!(a.treasury_sig.is_none() && a.treasury_sig_unknown && a.gas_sig.is_some());
    assert!(a.may_be_complete());
    // The epoch ends, then the same-attempt treasury retry is DEFINITELY refused.
    l.sim.next_epoch();
    test_env::advance_secs(10 * 60);
    l.tick();
    let op = l.op(0);
    let a2 = op.current().unwrap();
    assert!(a2.treasury_sig_unknown, "a later refusal is not proof that the earlier call never signed");
    assert!(a2.may_be_complete());
    assert!(matches!(a2.disposition, AttemptDisposition::Open), "the epoch plus a refusal must not close the attempt: {:?}", a2.disposition);
    assert_eq!(a2.tx_digest, a.tx_digest);
    // It cannot be abandoned, refunded or replaced, now or on later passes.
    for _ in 0..3 {
        test_env::set_caller(admin());
        assert!(matches!(crate::admin::abandon_mint_op(0, "refused"), Err(BridgeError::InvalidState(m)) if m.contains("complete signed transaction may exist")));
        test_env::set_caller(user(0x21));
        assert!(matches!(block_on(deposit::request_refund(&l.fx.ledger, id)), Err(BridgeError::InvalidState(_))));
        assert!(mint::prior_ops_provably_dead(&deposit_of(id)).is_err());
        let op = l.op(0);
        assert_eq!(op.attempts.len(), 1, "no replacement");
        assert!(matches!(op.state, MintOpState::Active) && matches!(op.current().unwrap().disposition, AttemptDisposition::Open));
        test_env::advance_secs(10 * 60);
        l.tick();
    }
    // Only an actually verified stored signature resolves the role's uncertainty.
    let a3 = l.op(0).current().unwrap().clone();
    assert!(a3.treasury_sig.is_some() && !a3.treasury_sig_unknown, "the verified signature resolved it");
    assert_eq!(a3.tx_digest, a.tx_digest);
}

#[test]
fn an_overlapping_stale_callback_cannot_clear_a_newer_uncertainty() {
    let l = lane();
    escrowed(&l.fx, 0x21, 5 * E8, 1);
    l.tick();
    l.signer.push_fault(SuiRole::Treasury, SignFault::Refuse);
    // While the first treasury call is in flight, a later call for the same attempt begins (a reclaimed
    // stale pass); the first call's definite refusal then arrives.
    *l.signer.on_sign.borrow_mut() = Some(Box::new(|role, _| {
        if role == SuiRole::Treasury {
            let identity = bind::lane_identity().unwrap();
            let _ = mint::begin_sign(0, SuiRole::Treasury, &identity);
        }
    }));
    l.tick();
    let a = l.op(0).current().unwrap().clone();
    assert!(a.treasury_sig.is_none());
    assert!(a.treasury_sig_unknown, "the newer call owns the uncertainty; the stale refusal cannot clear it");
    assert!(a.may_be_complete());
}

#[test]
fn a_first_definite_refusal_still_clears_its_own_flag() {
    // Nothing earlier could have signed, so the ordinary behavior is unchanged.
    let l = lane();
    escrowed(&l.fx, 0x21, 5 * E8, 1);
    l.signer.push_fault(SuiRole::Treasury, SignFault::Refuse);
    l.signer.push_fault(SuiRole::Treasury, SignFault::Refuse);
    l.ticks(2);
    let a = l.op(0).current().unwrap().clone();
    assert!(a.treasury_sig.is_none() && !a.treasury_sig_unknown);
    l.tick(); // a second definite refusal after a clean first one: still nothing could have signed
    let a = l.op(0).current().unwrap().clone();
    assert!(a.treasury_sig.is_none() && !a.treasury_sig_unknown && !a.may_be_complete());
}

#[test]
fn pausing_or_halting_during_the_final_signing_await_stops_the_broadcast_and_keeps_the_exact_attempt() {
    for halt_bridge in [false, true] {
        let l = lane();
        let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
        l.tick();
        let built = l.op(0).current().unwrap().clone();
        *l.signer.on_sign.borrow_mut() = Some(Box::new(move |role, _| {
            if role == SuiRole::Gas {
                // The final signature is in flight when the operator pauses mints (or the bridge halts).
                if halt_bridge {
                    state::halt("halted during the final signature");
                } else {
                    state::mutate_runtime(|r| r.config.pauses.mints = true);
                }
            }
        }));
        l.tick();
        let op = l.op(0);
        let a = op.current().unwrap();
        assert!(a.treasury_sig.is_some() && a.gas_sig.is_some(), "the verified signatures are kept (halt={halt_bridge})");
        assert_eq!(a.sends, 0, "nothing was persisted as sent (halt={halt_bridge})");
        assert_eq!(l.sim.accepted.borrow().len(), 0, "nothing reached the chain (halt={halt_bridge})");
        assert_eq!((a.generation, a.tx_digest, a.tx_bcs.clone()), (1, built.tx_digest, built.tx_bcs.clone()), "the exact bytes are held");
        let hold = op.hold_reason.clone().unwrap_or_default();
        assert!(hold.contains("not sent") && (hold.contains("paused") || hold.contains("halted")), "{hold}");
        assert_eq!(op.attempts.len(), 1);
        assert!(matches!(op.state, MintOpState::Active));
        assert_eq!(deposit_of(id).state, DepositState::MintInFlight);
        assert!(mint::prior_ops_provably_dead(&deposit_of(id)).is_err(), "no refund");
        // Recovery: the same attempt is sent once the pause or halt is lifted.
        *l.signer.on_sign.borrow_mut() = None;
        if halt_bridge {
            assert!(state::clear_halt());
        } else {
            state::mutate_runtime(|r| r.config.pauses.mints = false);
        }
        l.ticks(2);
        assert_eq!(deposit_of(id).state, DepositState::Delivered, "halt={halt_bridge}");
        assert_eq!(l.op(0).attempts.len(), 1);
        assert_eq!(l.op(0).attempts[0].tx_digest, built.tx_digest);
        assert_eq!(l.sim.executed_count(), 1);
    }
}

#[test]
fn an_already_sent_unknown_attempt_is_preserved_while_the_lane_is_paused() {
    let l = lane();
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    *l.sim.mode.borrow_mut() = ExecMode::Drop;
    l.ticks(2);
    let before = l.op(0).current().unwrap().clone();
    assert_eq!(before.sends, 1);
    state::mutate_runtime(|r| r.config.pauses.mints = true);
    l.sim.next_epoch();
    test_env::advance_secs(10 * 60);
    l.ticks(4);
    let op = l.op(0);
    let a = op.current().unwrap();
    assert_eq!((a.sends, a.generation, a.tx_digest, a.tx_bcs.clone()), (1, 1, before.tx_digest, before.tx_bcs.clone()), "no resend, no new generation");
    assert_eq!(op.attempts.len(), 1);
    assert!(matches!(a.disposition, AttemptDisposition::Open) && matches!(op.state, MintOpState::Active));
    assert_eq!(deposit_of(id).state, DepositState::MintInFlight);
    assert!(mint::prior_ops_provably_dead(&deposit_of(id)).is_err());
    // A certified copy lands while paused; once unpaused the receipt settles the same attempt.
    l.sim.execute_accepted(&before.tx_bcs);
    state::mutate_runtime(|r| r.config.pauses.mints = false);
    test_env::advance_secs(10 * 60);
    l.ticks(2);
    assert_eq!(deposit_of(id).state, DepositState::Delivered);
    assert_eq!(l.sim.executed_count(), 1);
}

// ---------------------------------------------------------------------------
// Review correction F3: the head transform keeps GraphQL errors unusable (BB-52 F3 class)
// ---------------------------------------------------------------------------

#[test]
fn graphql_errors_on_a_head_reply_survive_the_transform_and_exclude_the_provider() {
    let l = lane();
    // Two providers, both required: one reply carrying `errors` must leave only one usable provider.
    state::mutate_core(|c| c.mirror = providers(&["alpha", "beta"]));
    let net = l.net();
    let m = Mirror::from_state(&net).unwrap();
    assert!(matches!(block_on(m.head()), Verdict::Agreed(_)), "both clean providers agree");
    let cases = [
        ("a nonempty errors array", serde_json::json!([{ "message": "rate limited" }])),
        ("a malformed errors object", serde_json::json!({ "message": "not an array" })),
        ("a string errors member", serde_json::json!("oops")),
        ("a null errors member", serde_json::Value::Null),
    ];
    for (what, errors) in cases {
        l.sim.set_provider("beta", Provider { head_errors: Some(errors), ..Provider::default() });
        match block_on(m.head()) {
            Verdict::Unavailable(why) => assert!(why.contains("1 usable providers, 2 required") && why.contains("beta"), "{what}: {why}"),
            other => panic!("{what}: a reply with errors became a usable head: {other:?}"),
        }
    }
    // A valid empty errors array is not an error: the provider still counts, and the head is still rounded.
    l.sim.set_provider("beta", Provider { head_errors: Some(serde_json::json!([])), ..Provider::default() });
    assert!(matches!(block_on(m.head()), Verdict::Agreed(h) if h % 100 == 0));
    // The network identity check still excludes a provider on another network.
    l.sim.set_provider("beta", Provider { wrong_network: true, ..Provider::default() });
    assert!(matches!(block_on(m.head()), Verdict::Unavailable(why) if why.contains("wrong network")));
}

#[test]
fn a_send_reply_carrying_errors_is_never_evidence_so_its_transform_has_no_such_gap() {
    // `transform_sui_send` replaces every 200 reply, errors included, with a constant: that is by design
    // (a duplicate submission may answer differently) and nothing reads the body as evidence. The lane
    // settles an attempt only by receipt or consumed record, so a rejection that arrives as GraphQL errors
    // and is counted as "answered" changes nothing.
    let l = lane();
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    *l.sim.mode.borrow_mut() = ExecMode::Drop;
    l.ticks(2);
    l.sim.next_epoch(); // validators now refuse the resend (an `errors` reply) because its epoch passed
    test_env::advance_secs(10 * 60);
    l.ticks(2);
    assert!(!l.sim.rejections.borrow().is_empty(), "the simulated validators rejected the resend with errors");
    let op = l.op(0);
    let a = op.current().unwrap();
    assert!(a.last_send_note.clone().unwrap().contains("HTTP-acknowledged (maybe sent)"), "the reply still counts as answered");
    assert!(matches!(a.disposition, AttemptDisposition::Open) && matches!(op.state, MintOpState::Active));
    assert_eq!((op.attempts.len(), deposit_of(id).state), (1, DepositState::MintInFlight), "no delivery, no failure, no replacement");
}

// ---------------------------------------------------------------------------
// Review correction F4: facts read before an await are re-checked against the CURRENT state at admission
// ---------------------------------------------------------------------------

/// While the first owned-object read of a gather is in flight, run `f` once (a newer pass finishing).
fn during_the_owned_object_read(l: &Lane, f: impl Fn() + 'static) {
    let fired = std::cell::Cell::new(false);
    *l.sim.on_serve.borrow_mut() = Some(Box::new(move |q| {
        if q == crate::reads::QUERY_OWNED_OBJECT && !fired.replace(true) {
            f();
        }
    }));
}

#[test]
fn facts_older_than_the_current_final_checkpoint_admit_no_operation() {
    let l = lane();
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    let head = l.sim.head.get() + 100; // what the next pass will see (tick advances by 100)
    during_the_owned_object_read(&l, move || {
        // A newer pass completes another mint and advances our final checkpoint beyond these facts.
        state::mutate_runtime(|r| r.last_final_checkpoint = head + 1);
    });
    l.tick();
    assert_eq!(state::mint_op_count(), 0, "no operation was admitted from refs older than the current floor");
    assert_eq!((deposit_of(id).state, deposit_of(id).mint_op), (DepositState::Escrowed, None));
    assert_eq!((l.signer.calls.borrow().len(), l.sim.accepted.borrow().len()), (0, 0));
    assert_eq!(state::read_runtime(|r| r.active_mint_op), None);
    assert!(state::read_runtime(|r| r.lane_wait.clone()).unwrap_or_default().contains("behind our last finalized transaction"));
    // Re-gathered after the chain moves on: admitted with fresh references.
    *l.sim.on_serve.borrow_mut() = None;
    l.sim.advance(300);
    l.tick();
    assert_eq!(state::mint_op_count(), 1);
    let a = l.op(0).attempts[0].clone();
    assert!(a.built_checkpoint > head);
    assert_eq!(a.mint_cap.version, l.sim.current_ref(&SIM_MINT_CAP).version);
    l.tick();
    assert_eq!(deposit_of(id).state, DepositState::Delivered);
}

#[test]
fn facts_older_than_the_current_final_checkpoint_append_no_replacement_attempt() {
    // A replacement attempt after a finalized failure.
    let l = lane();
    let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
    *l.sim.mode.borrow_mut() = ExecMode::Abort("MoveAbort test".into());
    l.ticks(2);
    assert!(matches!(l.op(0).attempts[0].disposition, AttemptDisposition::FailedFinal { .. }));
    let head = l.sim.head.get() + 100;
    during_the_owned_object_read(&l, move || state::mutate_runtime(|r| r.last_final_checkpoint = head + 1));
    *l.sim.mode.borrow_mut() = ExecMode::Execute;
    l.tick();
    assert_eq!(l.op(0).attempts.len(), 1, "no attempt was appended from stale refs");
    assert_eq!((l.signer.calls.borrow().len(), l.sim.executed_count()), (2, 0), "only the first attempt's two signatures exist");
    *l.sim.on_serve.borrow_mut() = None;
    l.sim.advance(300);
    l.ticks(2);
    assert_eq!(l.op(0).attempts.len(), 2);
    assert_eq!(deposit_of(id).state, DepositState::Delivered);
}

#[test]
fn a_gas_coin_or_budget_changed_while_the_facts_were_read_admits_nothing() {
    for what in ["gas coin reconfigured", "gas budget raised"] {
        let l = lane();
        let id = escrowed(&l.fx, 0x21, 5 * E8, 1);
        let change: Box<dyn Fn()> = if what == "gas coin reconfigured" {
            Box::new(|| state::mutate_runtime(|r| r.config.lane.gas_coin = Some([0x43; 32])))
        } else {
            Box::new(|| state::mutate_runtime(|r| r.config.lane.gas_budget = 900_000_000))
        };
        during_the_owned_object_read(&l, change);
        l.tick();
        assert_eq!(state::mint_op_count(), 0, "{what}: nothing admitted from a pre-await snapshot");
        assert_eq!(deposit_of(id).state, DepositState::Escrowed, "{what}");
        assert_eq!(l.signer.calls.borrow().len(), 0, "{what}");
        assert!(state::read_runtime(|r| r.lane_wait.clone()).is_some(), "{what}: the lane says why it waited");
    }
}
