//! Review follow-ups for the ICP icUSD <-> Base icUSD escrow bridge (draft PR #591).
//!
//! # Follow-up 1: the SUCCESSFUL `admin_clear_halt` path, end to end
//!
//! Until now the suites only asserted that `admin_clear_halt` is REFUSED while the cause of a
//! halt remains, and host unit tests cannot reach a successful ledger read. This suite runs the
//! success path on the real wasm, the real ICRC ledger and a real anvil, and proves the part
//! that is easiest to lose in a refactor: after the halt is cleared the canister must re-arm its
//! OWN timer, so work that the halt held back (an escrowed mint) moves within seconds instead of
//! waiting out the idle interval.
//!
//! # Evidence labels (same classes as `bridge_faults_pic.rs`)
//!
//! * REAL (anvil): the halt (a genuine supply violation found by the canister's own
//!   reconcile reads), the refusal while the excess exists, the successful clear (fresh
//!   ledger and Base reads), the canister's own timer tick, the mint it signs with its tECDSA
//!   key and its execution on anvil, and the anvil snapshot/revert that restores the chain.
//! * MOCKED: only the optional fail-closed step, where `FaultAction::Reject` withholds every
//!   provider answer so the fresh Base read cannot happen.
//! * SIMULATED ACTORS: anvil impersonates the bridge's admin address to
//!   `grantRole(MINTER_ROLE, attacker)` (a compromised admin key) and the attacker is an anvil
//!   unlocked account that mints directly. Restoring the chain with `evm_revert` models "the
//!   cause of the halt is gone" (for example the bad blocks were reorganized away); it is a
//!   REAL reorg on the local node, but production Base would not normally reorg a finalized
//!   block, so this is the cheapest genuine way to make a fresh check pass again.
//! * PACED, NOT MOCKED: one request (the `totalMinted()` read of the admin call) is left
//!   parked by the harness while IC time moves; once released, anvil answers it for real.
//!
//! # Follow-up 2: refund and fee-sweep reply loss re-arms the timer
//!
//! `request_deposit_refund` and `admin_sweep_fees` send one inline ledger transfer. When the
//! ledger executes it but the reply is lost, the operation is unresolved. Both entry points
//! now hand it to the background tick right away (`send_inline_attempt` then
//! `rearm_if_unresolved`, which arms the timer for about a second) instead of waiting out the
//! idle interval, which an admin may set as high as a day. The host unit tests cover
//! `request_refund`, but `sweep_fees` always awaits the ledger fee first, so only PocketIC can
//! run that call site. These tests run both on the real wasm with the `flaky_ledger` test
//! canister and let ONLY the canister's own timer reconcile (never `admin_run_tick`).
//!
//! Evidence labels for follow-up 2:
//!
//! * REAL: the bridge wasm, its refund/sweep state machine and buckets, the canister's own timer
//!   tick, the held deposit (a paused contract on anvil makes the mint preflight hold it), the
//!   delivered deposit that accrues the fee, Base execution on anvil.
//! * MOCKED: the ledger. `flaky_ledger` is a test canister, not the production ledger wasm.
//!   `set_phantom_icrc1_failures(1)` makes it COMMIT the next ICRC-1 transfer (the refund or the
//!   sweep leaves the escrow account for real, inside the ledger) and then return a generic
//!   error, which is how a lost reply looks. Its dedup (same caller, accounts, amount, fee, memo,
//!   `created_at_time` gives `Duplicate`) is a fixture approximation of the production ledger's.
//! * SIMULATED: anvil impersonates the bridge's admin address to `pauseMint()` on the contract
//!   (an operator pausing the contract), which is what makes the first deposit a held one.
//!
//! Both tests raise the observer idle interval to 3600 s and the active interval to 600 s,
//! let the timer fire once so it is armed far away, prove that no tick runs on its own for 30 s,
//! and only then trigger the operation. A refund or sweep that is still unresolved after the few
//! seconds of the observation window means the re-arm is missing.
//!
//! Build once, then run (serially):
//!
//! ```text
//! cargo build --offline --release --target wasm32-unknown-unknown \
//!     -p rumi_icusd_bridge -p evm_rpc_relay
//! POCKET_IC_BIN=$PWD/pocket-ic cargo test --offline -p rumi_icusd_bridge \
//!     --test bridge_followups_pic -- --test-threads=1
//! ```
//!
//! `BRIDGE_WASM=<path>` runs the same suite against another build of the bridge (used to
//! check that the timer assertions fail when `schedule_soon()` is removed from
//! `timer::clear_halt_after_checks`).

mod support;

use candid::utils::ArgumentEncoder;
use candid::{decode_one, encode_args, Nat, Principal};
use icrc_ledger_types::icrc::generic_value::ICRC3Value;
use icrc_ledger_types::icrc1::account::Account;
use pocket_ic::common::rest::CanisterHttpRequest;
use pocket_ic::{PocketIc, WasmResult};
use serde_json::Value;
use support::bridge::*;
use support::contract::{bytes32_from_u64, word_addr};
use support::ledger;
use support::outcalls::{await_with_outcalls, serve_pending_except, submit, FaultAction};

fn e8(n: u64) -> u64 {
    n * E8
}

// ---------------------------------------------------------------------------
// Contract helpers (same shape as bridge_faults_pic.rs)
// ---------------------------------------------------------------------------

fn minter_role() -> [u8; 32] {
    keccak(b"MINTER_ROLE")
}

/// `grantRole(bytes32,address)` calldata.
fn grant_role_calldata(role: [u8; 32], account: &str) -> Vec<u8> {
    let mut d = keccak(b"grantRole(bytes32,address)")[..4].to_vec();
    d.extend_from_slice(&role);
    d.extend_from_slice(&word_addr(account));
    d
}

/// Whether a parked outcall body is a `totalMinted()` read (the LAST read of the supply check
/// that `admin_clear_halt` runs; the background tick never makes it when no burn is pending).
fn is_total_minted_read(body: &[u8]) -> bool {
    let want = selector_hex("totalMinted()");
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let one = |r: &Value| {
        r["method"] == "eth_call"
            && r["params"][0]["data"].as_str().map(|d| d.to_lowercase().starts_with(&want)).unwrap_or(false)
    };
    match &v {
        Value::Array(items) => items.iter().any(one),
        other => one(other),
    }
}

fn notes_containing(w: &BridgeWorld, needle: &str) -> Vec<String> {
    w.audit_log_all().into_iter().map(|e| e.note).filter(|n| n.contains(needle)).collect()
}

fn clear_halt(w: &mut BridgeWorld) -> BridgeResult<SupplyAudit> {
    w.admin_call("admin_clear_halt", ())
}

/// Seconds of IC time that passed between two `PocketIc::get_time` readings.
fn secs_between(a: std::time::SystemTime, b: std::time::SystemTime) -> u64 {
    b.duration_since(a).expect("IC time moves forward").as_secs()
}

// ---------------------------------------------------------------------------
// 1. admin_clear_halt succeeds on fresh checks and re-arms the timer
// ---------------------------------------------------------------------------

/// A genuine halt (a foreign mint the canister did not make, found by its own reconcile) is
/// refused while the excess exists, then cleared successfully once the chain is restored, and
/// the canister's OWN timer moves the escrowed deposit that the halt had been holding.
///
/// Scenario:
/// 1. One deposit is delivered; the chain is snapshotted (`evm_snapshot`).
/// 2. A foreign mint of 50 on Base (compromised admin grants MINTER_ROLE) and a second
///    deposit of 20 that is `Escrowed` and waiting to mint. Deposits cannot be admitted while
///    halted, so it is created first, and no tick runs between its creation and the halt
///    (PocketIC time only moves when the test moves it).
/// 3. `admin_reconcile` finds Base supply above what the canister accounts for and halts. The
///    escrowed deposit is NOT minted.
/// 4. `admin_clear_halt` is refused (the excess still exists), then the chain is restored
///    (`evm_revert`) and a fail-closed variant (providers unreachable) is refused too.
/// 5. The observer idle interval is raised to 3600 s and the active one to 600 s so a missing
///    re-arm is unmistakable. `admin_clear_halt` is then submitted and its LAST read
///    (`totalMinted()`) is held back while IC time moves 2 s: the canister's own timer (armed
///    for +1 s by the admin endpoint itself) fires DURING the checks, sees the halt, has no
///    pending work and re-arms for the idle interval (3600 s). This is the exact case the
///    final `schedule_soon()` in `timer::clear_halt_after_checks` exists for.
/// 6. The read is released; the clear succeeds with fresh coverage and base-supply checks, the
///    halt is gone and the audit log says so.
/// 7. WITHOUT calling `admin_run_tick`, IC time moves by one second at a time (at most 5 s in
///    total) and only the canister's own timer runs. The escrowed deposit's mint is signed,
///    broadcast and executed on anvil within that window, hundreds of seconds before either
///    configured interval could have elapsed. Without the re-arm this assertion fails.
///
/// Real: everything listed under REAL in the module docs. Mocked: the fail-closed step only
/// (`Reject` of every provider request, cleared right after). Simulated: the impersonated
/// admin `grantRole` and the attacker's mint. Paced, not mocked: the held `totalMinted()` read.
#[test]
fn admin_clear_halt_succeeds_on_fresh_checks_and_rearms_the_timer_for_the_escrowed_mint() {
    let mut w = BridgeWorld::setup(SetupOptions::default());
    let (user, user2) = (w.user, w.user2);
    let (recipient1, recipient2, attacker) = (w.holder(0), w.holder(1), w.holder(3));

    // ---- 1. A healthy bridge with one delivered deposit, snapshotted before the foreign mint.
    let first = w.deposit_and_deliver(user, e8(100), &recipient1);
    let clean = w.reconcile();
    assert_eq!(clean.base_supply_within_bound, Some(true), "{clean:?}");
    assert_eq!(clean.coverage_ok, Some(true), "{clean:?}");
    assert!(w.bridge_info().halted.is_none());
    let snap = w.anvil.snapshot();
    let latest_at_snapshot = w.anvil.block_number("latest");
    let minter_nonce_at_snapshot = w.minter_nonce();
    assert_eq!(minter_nonce_at_snapshot, 1, "only the first deposit's mint so far");

    // ---- 2. Foreign mint on Base, then a second deposit that stays Escrowed.
    let grant = w.impersonate_send(
        &w.base_addrs.admin.clone(),
        &w.token.address.clone(),
        &grant_role_calldata(minter_role(), &attacker),
    );
    assert!(w.anvil.tx_succeeded(&grant), "grantRole by the (impersonated) admin");
    let foreign = w.token.mint_from_unlocked(&w.anvil, &attacker, &attacker, e8(50) as u128, bytes32_from_u64(0xBAD));
    assert!(w.anvil.tx_succeeded(&foreign));
    w.mine_finalize();
    assert_eq!(w.base_total_supply(), e8(150) as u128, "100 delivered + 50 foreign");

    let second = w.approve_and_deposit(user2, e8(20), &recipient2);
    assert_eq!(second.status, DepositStatus::Escrowed, "{second:?}");
    assert!(second.mint_tx_hashes.is_empty(), "no mint signed yet: {second:?}");
    assert_ne!(second.id, first.id);
    let dep_id = second.id;

    // ---- 3. The canister's own reconcile finds the excess and halts.
    let audit = w.reconcile();
    assert_eq!(audit.base_supply_within_bound, Some(false), "{audit:?}");
    let reason = w.bridge_info().halted.clone().expect("the reconcile halted the bridge");
    assert!(reason.contains("exceed"), "halt reason: {reason}");
    let held = w.get_deposit(dep_id).expect("second deposit");
    assert_eq!(held.status, DepositStatus::Escrowed, "the halt holds the mint back: {held:?}");
    assert!(held.mint_tx_hashes.is_empty());
    assert_eq!(w.minter_nonce(), minter_nonce_at_snapshot, "nothing was minted by the bridge");

    // ---- 4a. Refused while the excess exists.
    match clear_halt(&mut w) {
        Err(BridgeError::Halted(r)) => assert!(r.contains("fresh data") || r.contains("exceed"), "{r}"),
        other => panic!("expected admin_clear_halt to be refused while the foreign mint exists, got {other:?}"),
    }
    assert!(w.bridge_info().halted.is_some(), "still halted");

    // ---- 4b. The cause goes away: the chain is restored to the snapshot (a real reorg).
    assert!(w.anvil.revert(&snap), "evm_revert succeeded");
    assert_eq!(w.anvil.block_number("latest"), latest_at_snapshot);
    assert_eq!(w.base_total_supply(), e8(100) as u128, "the foreign mint is gone");
    assert_eq!(w.minter_nonce(), minter_nonce_at_snapshot);

    // ---- 4c. Fail closed: with Base unreachable the fresh Base read cannot happen, so the
    // halt must stay (MOCKED: every provider answer is withheld).
    w.router.set_fault(|_, _| FaultAction::Reject);
    match clear_halt(&mut w) {
        Err(BridgeError::Halted(r)) => {
            assert!(r.contains("fresh data") && r.contains("base_ok=false"), "{r}");
            assert!(r.contains("escrow_ok=true"), "the ledger side was readable: {r}");
        }
        other => panic!("expected a fail-closed refusal with Base unreachable, got {other:?}"),
    }
    w.router.clear_fault();
    assert!(w.bridge_info().halted.is_some(), "still halted after the failed attempt");
    assert!(!notes_containing(&w, "Base supply check failed").is_empty(), "the failed read is audited");
    assert!(notes_containing(&w, "halt cleared").is_empty(), "nothing was cleared yet");

    // ---- 5. Make a missing timer re-arm unmistakable.
    let mut oc = w.bridge_info().observer_config;
    oc.idle_interval_secs = 3_600;
    oc.active_interval_secs = 600;
    let set: BridgeResult<()> = w.admin_call("admin_set_observer_config", (oc,));
    set.expect("admin_set_observer_config");

    // Submit the clear and let it run until its LAST read is parked. IC time has not moved
    // since the admin endpoint armed the canister's timer for +1 s.
    let hold = |r: &CanisterHttpRequest| is_total_minted_read(&r.body);
    let call = submit(&w.pic, w.bridge, w.admin, "admin_clear_halt", encode_args(()).unwrap()).expect("submit");
    let mut parked = 0;
    for _ in 0..600 {
        w.pic.tick();
        let (_, held_now, _) = serve_pending_except(&w.pic, &mut w.router, &hold);
        if held_now > 0 {
            parked = held_now;
            break;
        }
    }
    assert!(parked > 0, "the admin call never reached its totalMinted() read\n{}", w.dump());
    let last_run_before = w.bridge_info().observer.last_run_ns;
    let halted_mid_call = w.bridge_info().halted.clone();
    assert!(halted_mid_call.is_some(), "still halted while the checks are in flight");

    // IC time moves 2 s while the checks are in flight: the canister's own timer (due at +1 s)
    // fires, sees the halt and re-arms for the idle interval. Everything except the held read
    // is answered, until that tick has finished.
    w.advance_secs(2);
    let mut quiet = 0;
    for _ in 0..1_500 {
        w.pic.tick();
        let (_, _, answered) = serve_pending_except(&w.pic, &mut w.router, &hold);
        quiet = if answered == 0 { quiet + 1 } else { 0 };
        if quiet >= 40 && w.bridge_info().observer.last_run_ns != last_run_before {
            break;
        }
    }
    let mid = w.bridge_info();
    assert_ne!(
        mid.observer.last_run_ns, last_run_before,
        "the canister's own timer must have run a tick during the checks\n{}",
        w.dump()
    );
    assert!(mid.halted.is_some(), "the halt is still set while the checks are in flight");
    let mid_dep = w.get_deposit(dep_id).unwrap();
    assert_eq!(mid_dep.status, DepositStatus::Escrowed, "a halted tick must not mint: {mid_dep:?}");
    assert!(mid_dep.mint_tx_hashes.is_empty());

    // ---- 6. Release the read: the clear completes on fresh data.
    let bytes = await_with_outcalls(&w.pic, call, &mut w.router)
        .unwrap_or_else(|e| panic!("admin_clear_halt did not complete: {e}\n{}", w.dump()));
    let result: BridgeResult<SupplyAudit> = decode_one(&bytes).expect("decode admin_clear_halt reply");
    let audit = result.unwrap_or_else(|e| panic!("admin_clear_halt failed on fresh, healthy data: {e:?}\n{}", w.dump()));
    let cleared_at = w.pic.get_time();
    assert_eq!(audit.coverage_ok, Some(true), "{audit:?}");
    assert_eq!(audit.base_supply_within_bound, Some(true), "{audit:?}");
    assert_eq!(audit.base_total_supply_e8s.as_ref().map(nat_u128), Some(e8(100) as u128), "{audit:?}");
    assert_eq!(audit.outstanding(), e8(100) as u128);
    assert_eq!(nat_u128(&audit.pending_mint_e8s), e8(20) as u128, "the escrowed deposit is still waiting: {audit:?}");
    assert!(audit.deficit_e8s.is_none() || audit.deficit_e8s.as_ref().map(nat_u128) == Some(0), "{audit:?}");
    assert!(w.bridge_info().halted.is_none(), "get_bridge_info shows no halt");
    let cleared_notes = notes_containing(&w, "halt cleared after fresh checks");
    assert_eq!(cleared_notes.len(), 1, "exactly one clear is audited: {cleared_notes:?}");
    assert!(cleared_notes[0].contains("exceed"), "the note carries the cleared reason: {cleared_notes:?}");
    let still = w.get_deposit(dep_id).unwrap();
    assert_eq!(still.status, DepositStatus::Escrowed, "the clear itself does not mint: {still:?}");
    assert!(still.mint_tx_hashes.is_empty());

    // ---- 7. The canister's own timer moves the escrowed deposit within seconds. NO
    // admin_run_tick: IC time moves one second at a time and only outcalls are answered.
    let mut minted = false;
    for _ in 0..5 {
        w.advance_secs(1);
        for _ in 0..12 {
            w.pump_idle();
            let d = w.get_deposit(dep_id).unwrap();
            if !d.mint_tx_hashes.is_empty() && w.minter_nonce() > minter_nonce_at_snapshot {
                minted = true;
                break;
            }
        }
        if minted {
            break;
        }
    }
    let window = secs_between(cleared_at, w.pic.get_time());
    assert!(window <= 6, "the observation window was meant to be a few seconds, was {window} s");
    let dep = w.get_deposit(dep_id).unwrap();
    assert!(
        minted && matches!(dep.status, DepositStatus::MintInFlight | DepositStatus::Delivered),
        "within {window} s of the clear the canister's own timer must sign and broadcast the held mint \
         (idle interval 3600 s, active interval 600 s), but the deposit is {dep:?}\n{}",
        w.dump()
    );
    assert_eq!(dep.mint_tx_hashes.len(), 1, "exactly one signed attempt: {dep:?}");
    assert_eq!(w.minter_nonce(), minter_nonce_at_snapshot + 1, "one Base transaction from the minter lane");
    assert_eq!(w.base_balance(&recipient2), e8(20) as u128, "the mint executed on anvil");
    assert_eq!(w.base_total_supply(), e8(120) as u128);

    // ---- Finish normally: finality, delivery, and a last clean reconcile.
    w.mine_finalize();
    let done = w.wait_delivered(dep_id);
    assert_eq!(done.delivered.as_ref().map(|d| d.tx_hash.clone()), Some(dep.mint_tx_hashes[0].clone()));
    let end = w.reconcile();
    assert_eq!(end.coverage_ok, Some(true), "{end:?}");
    assert_eq!(end.base_supply_within_bound, Some(true), "{end:?}");
    assert_eq!(end.outstanding(), e8(120) as u128);
    assert!(w.bridge_info().halted.is_none());
}

// ---------------------------------------------------------------------------
// 2. Refund and sweep reply loss: the canister's own timer reconciles within seconds
// ---------------------------------------------------------------------------

fn flaky_update(pic: &PocketIc, ledger_id: Principal, method: &str, args: impl ArgumentEncoder) {
    match pic
        .update_call(ledger_id, Principal::anonymous(), method, encode_args(args).expect("encode"))
        .unwrap_or_else(|e| panic!("flaky_ledger {method} failed: {e}"))
    {
        WasmResult::Reply(_) => {}
        WasmResult::Reject(m) => panic!("flaky_ledger {method} rejected: {m}"),
    }
}

/// Install `flaky_ledger` with the harness fee and the planned balances.
fn flaky_ledger(pic: &PocketIc, plan: &LedgerPlan) -> Principal {
    let id = pic.create_canister();
    pic.add_cycles(id, 100_000_000_000_000);
    pic.install_canister(id, flaky_ledger_wasm(), encode_args(()).expect("encode init"), None);
    flaky_update(pic, id, "set_fee", (Nat::from(LEDGER_FEE),));
    for (who, balance) in &plan.balances {
        flaky_update(pic, id, "mint", (Account { owner: *who, subaccount: None }, Nat::from(*balance)));
    }
    id
}

fn flaky_world(opts: SetupOptions) -> BridgeWorld {
    BridgeWorld::setup_with_ledger(opts, flaky_ledger)
}

/// The next ICRC-1 transfer COMMITS on the flaky ledger and then answers with an error (a lost
/// reply). ICRC-2 pulls and approvals are not affected.
fn lose_next_icrc1_reply(w: &BridgeWorld) {
    flaky_update(&w.pic, w.ledger, "set_phantom_icrc1_failures", (1u32,));
}

/// Number of `1xfer` (icrc1_transfer) blocks in the flaky ledger's ICRC-3 log whose receiver is
/// exactly `to`. Refunds and fee sweeps are the only ICRC-1 transfers in these worlds, so this
/// counts the transfers that REALLY executed on the ledger, whatever the canister believes.
fn xfer_blocks_to(w: &BridgeWorld, to: &Account) -> usize {
    let res = ledger::icrc3_get_blocks(&w.pic, w.ledger, 0, 200);
    res.blocks
        .iter()
        .filter(|b| {
            let ICRC3Value::Map(m) = &b.block else { return false };
            if !matches!(m.get("btype"), Some(ICRC3Value::Text(t)) if t == "1xfer") {
                return false;
            }
            let Some(ICRC3Value::Map(tx)) = m.get("tx") else { return false };
            let Some(ICRC3Value::Array(parts)) = tx.get("to") else { return false };
            let owner = matches!(parts.first(), Some(ICRC3Value::Blob(o)) if &o[..] == to.owner.as_slice());
            let sub = match (&to.subaccount, parts.get(1)) {
                (None, None) => true,
                (Some(s), Some(ICRC3Value::Blob(b))) => &b[..] == s.as_slice(),
                _ => false,
            };
            owner && sub
        })
        .count()
}

/// Observer idle interval 3600 s and active interval 600 s: far above the few seconds the
/// observation windows use, so only an explicit re-arm can make a tick run in time.
fn set_slow_timer_intervals(w: &mut BridgeWorld) {
    let mut oc = w.bridge_info().observer_config;
    oc.idle_interval_secs = 3_600;
    oc.active_interval_secs = 600;
    let r: BridgeResult<()> = w.admin_call("admin_set_observer_config", (oc,));
    r.expect("admin_set_observer_config");
}

/// Let the timer that the last `admin_run_tick` armed for +1 s fire (it re-arms for the long idle
/// interval), then prove that nothing runs by itself for 30 more seconds. Without this the
/// operation under test could be reconciled by a timer that was already due.
fn settle_timer_far_away(w: &mut BridgeWorld) {
    w.advance_secs(2);
    for _ in 0..12 {
        w.pump_idle();
    }
    let before = w.bridge_info().observer.last_run_ns;
    w.advance_secs(30);
    for _ in 0..12 {
        w.pump_idle();
    }
    assert_eq!(
        w.bridge_info().observer.last_run_ns,
        before,
        "no tick may run on its own in the 30 s before the operation (the timer is armed for the idle interval)\n{}",
        w.dump()
    );
}

/// Move IC time one second at a time (at most `max_secs` in total) and answer outcalls, so that
/// ONLY the canister's own timer can run; `admin_run_tick` is never called. Returns the seconds
/// that passed when `done` held, `None` if it never did.
fn let_own_timer_run(w: &mut BridgeWorld, max_secs: u64, mut done: impl FnMut(&BridgeWorld) -> bool) -> Option<u64> {
    for secs in 1..=max_secs {
        w.advance_secs(1);
        for _ in 0..12 {
            w.pump_idle();
            if done(w) {
                return Some(secs);
            }
        }
    }
    None
}

fn count_transitions(w: &BridgeWorld, want: &Transition) -> usize {
    w.audit_log_all().iter().filter(|e| e.transition.as_ref() == Some(want)).count()
}

/// A held deposit is refunded; the ledger EXECUTES the refund transfer but the reply is lost, so
/// `request_deposit_refund` returns with the refund unresolved. With no `admin_run_tick` and only
/// a few seconds of IC time (the idle interval is 3600 s), the canister's own timer resends the
/// identical tuple, the ledger answers `Duplicate`, and the deposit reaches `Refunded`.
///
/// Real: the bridge wasm, the held deposit (contract paused on anvil), the refund state machine,
/// buckets and escrow accounting, the canister's own timer tick.
/// Mocked: the ledger (`flaky_ledger`, see the module docs): it commits the refund and returns an
/// error once (`set_phantom_icrc1_failures(1)`), and deduplicates the resend.
/// Simulated: `pauseMint()` by anvil impersonating the bridge's admin address.
#[test]
fn refund_with_lost_ledger_reply_is_reconciled_by_the_own_timer_within_seconds() {
    let mut w = flaky_world(SetupOptions::default());
    set_slow_timer_intervals(&mut w);
    let (user, recipient) = (w.user, w.holder(0));
    let a = e8(30);

    // A held deposit: the paused contract makes the mint preflight revert, so no tx is ever signed.
    w.contract_pause_mint_as_admin();
    assert!(w.contract_mint_paused());
    let dep = w.approve_and_deposit(user, a, &recipient);
    assert_eq!(dep.status, DepositStatus::Escrowed, "{dep:?}");
    let id = dep.id;
    let held = w.wait_deposit(id, "MintHeld", |d| matches!(d.status, DepositStatus::MintHeld { .. }));
    assert!(held.mint_tx_hashes.is_empty() && held.mint_stage.is_none(), "no tx was signed: {held:?}");
    assert_eq!(w.escrow_balance(), a as u128, "the backing is in escrow");
    let held_audit = w.supply_audit();
    assert_eq!(nat_u128(&held_audit.pending_mint_e8s), a as u128, "{held_audit:?}");

    settle_timer_far_away(&mut w);
    assert_eq!(w.get_deposit(id).unwrap().status, held.status, "still held, nothing moved by itself");

    // The refund transfer executes on the ledger, the reply is lost.
    let user_before = w.user_balance(user);
    lose_next_icrc1_reply(&w);
    let started = w.request_refund(user, id).expect("request_deposit_refund returns even though the reply was lost");
    assert_eq!(started.status, DepositStatus::Refunding, "unresolved, not Refunded: {started:?}");
    assert!(started.refund_block_index.is_none(), "{started:?}");
    let at_request = w.pic.get_time();
    // What really happened inside the ledger, and what the canister has reserved meanwhile.
    assert_eq!(w.user_balance(user) - user_before, (a - LEDGER_FEE) as u128, "the ledger executed the refund");
    assert_eq!(w.escrow_balance(), 0, "the escrow paid the refund");
    assert_eq!(xfer_blocks_to(&w, &ledger::account(user)), 1, "one refund transfer on the ledger");
    let mid = w.supply_audit();
    assert_eq!(nat_u128(&mid.pending_refund_e8s), a as u128, "{mid:?}");
    assert_eq!(nat_u128(&mid.unknown_outflow_e8s), a as u128, "the outflow stays reserved while unknown: {mid:?}");
    assert_eq!(nat_u128(&mid.pending_mint_e8s), 0, "{mid:?}");
    assert_eq!(count_transitions(&w, &Transition::RefundStarted { amount: a }), 1);
    assert_eq!(count_transitions(&w, &Transition::RefundCompleted { amount: a }), 0);

    // ONLY the canister's own timer runs, for a few seconds.
    let took = let_own_timer_run(&mut w, 5, |w| {
        w.get_deposit(id).map(|d| d.status == DepositStatus::Refunded).unwrap_or(false)
    });
    let window = secs_between(at_request, w.pic.get_time());
    let dep = w.get_deposit(id).unwrap();
    assert!(
        took.is_some(),
        "within {window} s of the lost reply the canister's own timer must reconcile the refund \
         (idle interval 3600 s, active interval 600 s), but the deposit is {dep:?}\n{}",
        w.dump()
    );
    assert!(window <= 6, "the observation window was meant to be a few seconds, was {window} s");

    // Exactly one refund, no duplicate, and everything consistent.
    assert_eq!(dep.status, DepositStatus::Refunded, "{dep:?}");
    assert_eq!(dep.refund_amount_e8s, Some(a - LEDGER_FEE));
    assert!(dep.refund_block_index.is_some(), "the duplicate_of block is the refund block: {dep:?}");
    assert_eq!(w.user_balance(user) - user_before, (a - LEDGER_FEE) as u128, "credited exactly once");
    assert_eq!(xfer_blocks_to(&w, &ledger::account(user)), 1, "still a single refund block on the ledger");
    assert_eq!(w.escrow_balance(), 0);
    assert_eq!(count_transitions(&w, &Transition::RefundStarted { amount: a }), 1);
    assert_eq!(count_transitions(&w, &Transition::RefundCompleted { amount: a }), 1, "completed exactly once");
    let after = w.supply_audit();
    assert_eq!(after.buckets(), [0; 7], "{after:?}");
    assert_eq!(nat_u128(&after.unknown_outflow_e8s), 0, "{after:?}");
    assert!(w.bridge_info().halted.is_none());

    // Later ticks change nothing: no second transfer, no second credit.
    w.advance_secs(120);
    w.ticks(3);
    assert_eq!(xfer_blocks_to(&w, &ledger::account(user)), 1);
    assert_eq!(w.user_balance(user) - user_before, (a - LEDGER_FEE) as u128);
    let end = w.reconcile();
    assert_eq!(end.coverage_ok, Some(true), "{end:?}");
    assert_eq!(end.escrow(), Some(0), "{end:?}");
    assert_eq!(end.buckets(), [0; 7], "{end:?}");
}

/// A fee sweep: the ledger EXECUTES the sweep transfer but the reply is lost, so
/// `admin_sweep_fees` returns with the sweep unresolved. With no `admin_run_tick` and only a few
/// seconds of IC time, the canister's own timer resends the identical tuple, the ledger answers
/// `Duplicate`, and the sweep completes: the fee bucket is spent exactly once.
///
/// Real: the bridge wasm, a deposit delivered on anvil (it accrues the deposit fee), the sweep
/// state machine, buckets and escrow accounting, the canister's own timer tick.
/// Mocked: the ledger (`flaky_ledger`, see the module docs): it commits the sweep and returns an
/// error once (`set_phantom_icrc1_failures(1)`), and deduplicates the resend.
/// Simulated: nothing.
#[test]
fn fee_sweep_with_lost_ledger_reply_is_reconciled_by_the_own_timer_within_seconds() {
    let dep_fee = 50_000_000u64; // 0.5 icUSD
    let mut opts = SetupOptions::default();
    opts.fees = BridgeFees { deposit_fee_e8s: dep_fee, release_fee_e8s: 0 };
    let mut w = flaky_world(opts);
    set_slow_timer_intervals(&mut w);
    let (user, recipient) = (w.user, w.holder(0));
    let a = e8(10);
    let rec = w.fee_recipient;

    // A delivered deposit accrues the fee in escrow.
    let delivered = w.deposit_and_deliver(user, a, &recipient);
    assert_eq!(delivered.deposit_fee_e8s, dep_fee);
    let accrued = w.supply_audit();
    assert_eq!(accrued.fees(), dep_fee as u128, "{accrued:?}");
    assert_eq!(accrued.outstanding(), (a - dep_fee) as u128, "{accrued:?}");
    assert_eq!(w.escrow_balance(), a as u128);
    assert_eq!(w.ledger_balance(&rec), 0);

    settle_timer_far_away(&mut w);

    // The sweep transfer executes on the ledger, the reply is lost.
    lose_next_icrc1_reply(&w);
    let swept = dep_fee;
    let sweep_id = w.sweep_fees(swept).expect("admin_sweep_fees returns even though the reply was lost");
    let at_request = w.pic.get_time();
    assert_eq!(w.ledger_balance(&rec), (swept - LEDGER_FEE) as u128, "the ledger executed the sweep");
    assert_eq!(w.escrow_balance(), (a - swept) as u128, "the escrow paid the sweep");
    assert_eq!(xfer_blocks_to(&w, &rec), 1, "one sweep transfer on the ledger");
    let mid = w.supply_audit();
    assert_eq!(mid.fees(), dep_fee as u128, "the fee bucket is only reserved, not yet spent: {mid:?}");
    assert_eq!(nat_u128(&mid.unknown_outflow_e8s), swept as u128, "{mid:?}");
    assert_eq!(count_transitions(&w, &Transition::SweepCompleted { amount: swept }), 0);

    // ONLY the canister's own timer runs, for a few seconds.
    let took = let_own_timer_run(&mut w, 5, |w| {
        let s = w.supply_audit();
        s.fees() == 0 && nat_u128(&s.unknown_outflow_e8s) == 0
    });
    let window = secs_between(at_request, w.pic.get_time());
    assert!(
        took.is_some(),
        "within {window} s of the lost reply the canister's own timer must reconcile sweep {sweep_id} \
         (idle interval 3600 s, active interval 600 s), but the audit is {:?}\n{}",
        w.supply_audit(),
        w.dump()
    );
    assert!(window <= 6, "the observation window was meant to be a few seconds, was {window} s");

    // Exactly one sweep, the bucket spent once, everything consistent.
    assert_eq!(w.ledger_balance(&rec), (swept - LEDGER_FEE) as u128, "credited exactly once");
    assert_eq!(xfer_blocks_to(&w, &rec), 1, "still a single sweep block on the ledger");
    assert_eq!(w.escrow_balance(), (a - swept) as u128);
    assert_eq!(count_transitions(&w, &Transition::SweepCompleted { amount: swept }), 1, "completed exactly once");
    let after = w.supply_audit();
    assert_eq!(after.buckets(), [(a - dep_fee) as u128, 0, 0, 0, 0, 0, 0], "{after:?}");
    assert_eq!(nat_u128(&after.unknown_outflow_e8s), 0, "{after:?}");
    assert!(w.bridge_info().halted.is_none());

    // Later ticks change nothing; a second sweep of the now empty bucket is refused.
    w.advance_secs(120);
    w.ticks(3);
    assert_eq!(xfer_blocks_to(&w, &rec), 1);
    assert_eq!(w.ledger_balance(&rec), (swept - LEDGER_FEE) as u128);
    assert!(matches!(w.sweep_fees(swept), Err(BridgeError::InvalidArgument(_))), "nothing left to sweep");
    let end = w.reconcile();
    assert_eq!(end.coverage_ok, Some(true), "{end:?}");
    assert_eq!(end.escrow(), Some((a - swept) as u128), "{end:?}");
}
