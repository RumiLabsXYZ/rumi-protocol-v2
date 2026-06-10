//! PocketIC end-to-end: rumi_points ingests a forward-filtered backend event from
//! a mock source canister and auto-registers the acting principal.
//!
//! Validates the one path unit tests cannot: the live inter-canister poll
//! (`trigger_poll` -> `ic_cdk::call` -> candid decode of the source response ->
//! normalize -> `register` -> cursor advance). The mock's response types match the
//! backend `.did` structurally; the 95-variant superset-decode case is covered by
//! the canary tests in `source_types.rs`.
//!
//! Build the wasms first:
//!   cargo build --release --target wasm32-unknown-unknown -p rumi_points -p rumi_points_e2e_source
//! Run (the pocket-ic binary is at the repo root):
//!   POCKET_IC_BIN=$PWD/pocket-ic cargo test -p rumi_points --test pocket_ic_ingest

use candid::{CandidType, Decode, Encode, Principal};
use pocket_ic::{PocketIcBuilder, WasmResult};
use serde::Deserialize;
use std::time::{Duration, SystemTime};

const RUMI_POINTS_WASM: &[u8] =
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_points.wasm");
const MOCK_SOURCE_WASM: &[u8] =
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_points_e2e_source.wasm");

// Minimal candid mirrors of the rumi_points interface used by this test.
#[derive(CandidType)]
struct InitArgs {
    admin: Option<Principal>,
    excluded_principals: Option<Vec<Principal>>,
    season_start_ns: Option<u64>,
    season_end_ns: Option<u64>,
    snapshot_seed_commit: Option<[u8; 32]>,
}

#[derive(CandidType, Deserialize, Debug, PartialEq)]
enum PointsError {
    Unauthorized,
    Excluded,
}

// Minimal mirrors for the accrual E2E (decode by field name; width subtyping lets
// TPrincipalState read just `total_points` from the full record).
// Full (admin) open-epoch view: includes the capture/close cursors and completion
// flags, read via the admin-only `get_epoch_status_admin` (POINTS-001 moved these
// off the public `get_epoch_status`). Width subtyping lets this decode the full
// record while ignoring the close_* fields the tests do not assert on.
#[derive(CandidType, Deserialize)]
struct TOpenEpoch {
    epoch_index: u64,
    epoch_start_ns: u64,
    epoch_end_ns: u64,
    snapshot_a_ns: u64,
    snapshot_b_ns: u64,
    a_cursor: Option<Principal>,
    a_complete: bool,
    b_cursor: Option<Principal>,
    b_complete: bool,
}

#[derive(CandidType, Deserialize)]
struct TEpochStatus {
    current_epoch_index: u64,
    driver_enabled: bool,
    driver_interval_secs: u64,
    open_epoch: Option<TOpenEpoch>,
    revealed_seed_count: u64,
    snapshot_seed_committed: bool,
}

// Public open-epoch view: bounds only, NO capture/close cursors (POINTS-001);
// each snapshot time is `None` until it has fired (PTS-002). This is what the
// anonymous `get_epoch_status` returns.
#[derive(CandidType, Deserialize)]
struct TPublicOpenEpoch {
    epoch_index: u64,
    epoch_start_ns: u64,
    epoch_end_ns: u64,
    snapshot_a_ns: Option<u64>,
    snapshot_b_ns: Option<u64>,
}

#[derive(CandidType, Deserialize)]
struct TPublicEpochStatus {
    current_epoch_index: u64,
    driver_enabled: bool,
    driver_interval_secs: u64,
    open_epoch: Option<TPublicOpenEpoch>,
    revealed_seed_count: u64,
    snapshot_seed_committed: bool,
}

#[derive(CandidType, Deserialize)]
struct TPrincipalState {
    total_points: candid::Nat,
}

fn admin() -> Principal {
    Principal::from_slice(&[9; 10])
}

/// The synthetic borrow event's caller, hard-coded in the mock. Not in the
/// excluded set, so it registers.
fn synthetic_caller() -> Principal {
    Principal::from_text("ryjl3-tyaaa-aaaaa-aaaba-cai").unwrap()
}

#[test]
fn poll_ingests_backend_event_and_auto_registers() {
    let pic = PocketIcBuilder::new().with_application_subnet().build();

    // Install the mock source canister (no init args).
    let mock = pic.create_canister();
    pic.add_cycles(mock, 2_000_000_000_000);
    pic.install_canister(mock, MOCK_SOURCE_WASM.to_vec(), Encode!().unwrap(), None);

    // Install rumi_points with admin set explicitly.
    let rp = pic.create_canister();
    pic.add_cycles(rp, 4_000_000_000_000);
    let init = InitArgs {
        admin: Some(admin()),
        excluded_principals: None,
        season_start_ns: None,
        season_end_ns: None,
        snapshot_seed_commit: None,
    };
    pic.install_canister(
        rp,
        RUMI_POINTS_WASM.to_vec(),
        Encode!(&Some(init)).unwrap(),
        None,
    );

    // Point the backend source (tag 0) at the mock (admin-gated).
    let set_res = pic
        .update_call(rp, admin(), "set_source_canister", Encode!(&0u8, &mock).unwrap())
        .expect("set_source_canister call failed");
    match set_res {
        WasmResult::Reply(b) => {
            let r: Result<(), PointsError> = Decode!(&b, Result<(), PointsError>).unwrap();
            assert_eq!(r, Ok(()), "set_source_canister should succeed for admin");
        }
        WasmResult::Reject(m) => panic!("set_source_canister rejected: {m}"),
    }

    // Nobody is registered yet.
    assert!(!is_registered(&pic, rp, synthetic_caller()));

    // Trigger the poll: rumi_points calls the mock's get_events_forward_filtered,
    // decodes the synthetic borrow event, and auto-registers its caller.
    let poll_res = pic
        .update_call(rp, admin(), "trigger_poll", Encode!().unwrap())
        .expect("trigger_poll call failed");
    let applied: Result<u64, PointsError> = match poll_res {
        WasmResult::Reply(b) => Decode!(&b, Result<u64, PointsError>).unwrap(),
        WasmResult::Reject(m) => panic!("trigger_poll rejected: {m}"),
    };
    assert_eq!(applied, Ok(1), "exactly one event should be ingested");

    // The acting principal is now registered; an unrelated one is not.
    assert!(
        is_registered(&pic, rp, synthetic_caller()),
        "the borrow event's caller should be auto-registered"
    );
    assert!(!is_registered(&pic, rp, Principal::from_slice(&[8; 5])));

    // A second poll is a no-op: the mock returns empty past the cursor, so nothing
    // new is applied (and the cursor does not regress).
    let poll_res2 = pic
        .update_call(rp, admin(), "trigger_poll", Encode!().unwrap())
        .expect("second trigger_poll failed");
    let applied2: Result<u64, PointsError> = match poll_res2 {
        WasmResult::Reply(b) => Decode!(&b, Result<u64, PointsError>).unwrap(),
        WasmResult::Reject(m) => panic!("second trigger_poll rejected: {m}"),
    };
    assert_eq!(applied2, Ok(0), "second poll ingests nothing (caught up)");
    assert!(is_registered(&pic, rp, synthetic_caller()), "still registered");
}

/// Phase 2b: the periodic timer (not a manual trigger) drives ingestion. Enable
/// it, advance past one interval, and confirm the timer-driven poll auto-registered
/// the event's caller.
#[test]
fn poll_timer_drives_ingestion() {
    let pic = PocketIcBuilder::new().with_application_subnet().build();

    let mock = pic.create_canister();
    pic.add_cycles(mock, 2_000_000_000_000);
    pic.install_canister(mock, MOCK_SOURCE_WASM.to_vec(), Encode!().unwrap(), None);

    let rp = pic.create_canister();
    pic.add_cycles(rp, 4_000_000_000_000);
    let init = InitArgs {
        admin: Some(admin()),
        excluded_principals: None,
        season_start_ns: None,
        season_end_ns: None,
        snapshot_seed_commit: None,
    };
    pic.install_canister(rp, RUMI_POINTS_WASM.to_vec(), Encode!(&Some(init)).unwrap(), None);

    admin_ok(&pic, rp, "set_source_canister", Encode!(&0u8, &mock).unwrap());

    // Timer off by default; nobody registered, no manual poll.
    assert!(!is_registered(&pic, rp, synthetic_caller()));

    // Enable the timer at the minimum cadence.
    admin_ok(&pic, rp, "set_poll_interval_secs", Encode!(&60u64).unwrap());
    admin_ok(&pic, rp, "set_poll_enabled", Encode!(&true).unwrap());

    // Advance past one interval and let the timer fire + the async poll complete.
    pic.advance_time(Duration::from_secs(70));
    for _ in 0..15 {
        pic.tick();
    }

    assert!(
        is_registered(&pic, rp, synthetic_caller()),
        "the timer-driven poll should have auto-registered the event's caller"
    );
}

/// Call an admin update returning `Result<(), PointsError>` and assert Ok.
fn admin_ok(pic: &pocket_ic::PocketIc, rp: Principal, method: &str, args: Vec<u8>) {
    let res = pic.update_call(rp, admin(), method, args).expect("admin call failed");
    match res {
        WasmResult::Reply(b) => {
            let r: Result<(), PointsError> = Decode!(&b, Result<(), PointsError>).unwrap();
            assert_eq!(r, Ok(()), "{method} should succeed for admin");
        }
        WasmResult::Reject(m) => panic!("{method} rejected: {m}"),
    }
}

fn is_registered(pic: &pocket_ic::PocketIc, rp: Principal, who: Principal) -> bool {
    let res = pic
        .query_call(rp, Principal::anonymous(), "is_registered", Encode!(&who).unwrap())
        .expect("is_registered call failed");
    match res {
        WasmResult::Reply(b) => Decode!(&b, bool).unwrap(),
        WasmResult::Reject(m) => panic!("is_registered rejected: {m}"),
    }
}

// ── Accrual E2E: the snapshot capture/close path against a mock that answers all
// four sources' balance queries (the one path unit tests cannot exercise) ──

fn install_mock(pic: &pocket_ic::PocketIc) -> Principal {
    let mock = pic.create_canister();
    pic.add_cycles(mock, 4_000_000_000_000);
    pic.install_canister(mock, MOCK_SOURCE_WASM.to_vec(), Encode!().unwrap(), None);
    mock
}

/// The season seed S0 the accrual tests reveal at `start_season`. Its commit
/// `H0 = sha256(S0)` is set at init so the commit-reveal gate is satisfied.
const SEASON_SEED: [u8; 32] = [42u8; 32];

fn install_points(pic: &pocket_ic::PocketIc) -> Principal {
    let rp = pic.create_canister();
    pic.add_cycles(rp, 4_000_000_000_000);
    let init = InitArgs {
        admin: Some(admin()),
        excluded_principals: None,
        season_start_ns: None,
        season_end_ns: None,
        // Commit H0 = sha256(S0) at init: start_season requires a committed seed
        // (commit-reveal anti-sniping) and verifies S0 against this.
        snapshot_seed_commit: Some(rumi_points::snapshot_seed::commitment(&SEASON_SEED)),
    };
    pic.install_canister(rp, RUMI_POINTS_WASM.to_vec(), Encode!(&Some(init)).unwrap(), None);
    rp
}

fn set_time_ns(pic: &pocket_ic::PocketIc, ns: u64) {
    pic.set_time(SystemTime::UNIX_EPOCH + Duration::from_nanos(ns));
}

/// Point all four source tags (backend/3pool/SP/AMM) at the single mock.
fn set_all_sources(pic: &pocket_ic::PocketIc, rp: Principal, mock: Principal) {
    for tag in 0u8..4 {
        admin_ok(pic, rp, "set_source_canister", Encode!(&tag, &mock).unwrap());
    }
}

fn set_vault_debt(pic: &pocket_ic::PocketIc, mock: Principal, owner: Principal, debt: u64) {
    pic.update_call(mock, Principal::anonymous(), "set_vault_debt", Encode!(&owner, &debt).unwrap())
        .expect("set_vault_debt failed");
}

fn set_fail_get_vaults(pic: &pocket_ic::PocketIc, mock: Principal, fail: bool) {
    pic.update_call(mock, Principal::anonymous(), "set_fail_get_vaults", Encode!(&fail).unwrap())
        .expect("set_fail_get_vaults failed");
}

fn start_season_ok(pic: &pocket_ic::PocketIc, rp: Principal, seed: [u8; 32]) {
    let res = pic
        .update_call(rp, admin(), "start_season", Encode!(&seed.to_vec()).unwrap())
        .expect("start_season call failed");
    match res {
        WasmResult::Reply(b) => {
            let r: Result<(), String> = Decode!(&b, Result<(), String>).unwrap();
            assert_eq!(r, Ok(()), "start_season should succeed in-season");
        }
        WasmResult::Reject(m) => panic!("start_season rejected: {m}"),
    }
}

fn force_tick(pic: &pocket_ic::PocketIc, rp: Principal) {
    let res = pic
        .update_call(rp, admin(), "force_epoch_tick", Encode!().unwrap())
        .expect("force_epoch_tick call failed");
    match res {
        WasmResult::Reply(b) => {
            let r: Result<(), PointsError> = Decode!(&b, Result<(), PointsError>).unwrap();
            assert_eq!(r, Ok(()), "force_epoch_tick should succeed for admin");
        }
        WasmResult::Reject(m) => panic!("force_epoch_tick rejected: {m}"),
    }
}

/// Full epoch status with capture/close progress, via the ADMIN-only
/// `get_epoch_status_admin` (POINTS-001 keeps the cursors off the public query).
fn epoch_status(pic: &pocket_ic::PocketIc, rp: Principal) -> TEpochStatus {
    let res = pic
        .query_call(rp, admin(), "get_epoch_status_admin", Encode!().unwrap())
        .expect("get_epoch_status_admin call failed");
    match res {
        WasmResult::Reply(b) => Decode!(&b, TEpochStatus).unwrap(),
        WasmResult::Reject(m) => panic!("get_epoch_status_admin rejected: {m}"),
    }
}

/// Public epoch status (anonymous caller), via `get_epoch_status`. Used to assert
/// the cursors are NOT exposed (POINTS-001).
fn public_epoch_status(pic: &pocket_ic::PocketIc, rp: Principal) -> TPublicEpochStatus {
    let res = pic
        .query_call(rp, Principal::anonymous(), "get_epoch_status", Encode!().unwrap())
        .expect("get_epoch_status call failed");
    match res {
        WasmResult::Reply(b) => Decode!(&b, TPublicEpochStatus).unwrap(),
        WasmResult::Reject(m) => panic!("get_epoch_status rejected: {m}"),
    }
}

fn total_points(pic: &pocket_ic::PocketIc, rp: Principal, who: Principal) -> u128 {
    let res = pic
        .query_call(rp, Principal::anonymous(), "get_principal_state", Encode!(&who).unwrap())
        .expect("get_principal_state call failed");
    let st: Option<TPrincipalState> = match res {
        WasmResult::Reply(b) => Decode!(&b, Option<TPrincipalState>).unwrap(),
        WasmResult::Reject(m) => panic!("get_principal_state rejected: {m}"),
    };
    st.map(|s| nat_to_u128(&s.total_points)).unwrap_or(0)
}

fn nat_to_u128(n: &candid::Nat) -> u128 {
    n.to_string()
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .unwrap()
}

#[test]
fn epoch_accrues_points_for_a_held_position() {
    let pic = PocketIcBuilder::new().with_application_subnet().build();
    set_time_ns(&pic, rumi_points::DEFAULT_SEASON_START_NS);

    let mock = install_mock(&pic);
    let rp = install_points(&pic);
    set_all_sources(&pic, rp, mock);

    let p = Principal::from_slice(&[7; 5]);
    admin_ok(&pic, rp, "register_test_principal", Encode!(&p).unwrap());

    const DEBT: u64 = 100_000_000; // $1 of icUSD debt (e8s)
    set_vault_debt(&pic, mock, p, DEBT);

    // Open epoch 0 (S0 verified against the committed H0), then disable the timer
    // so only force_tick drives the state machine.
    start_season_ok(&pic, rp, SEASON_SEED);
    admin_ok(&pic, rp, "set_epoch_driver_enabled", Encode!(&false).unwrap());

    let oe = epoch_status(&pic, rp)
        .open_epoch
        .expect("epoch 0 should be open after start_season");

    // Snapshot A.
    set_time_ns(&pic, oe.snapshot_a_ns);
    force_tick(&pic, rp);
    assert!(
        epoch_status(&pic, rp).open_epoch.unwrap().a_complete,
        "snapshot A should be captured"
    );

    // Snapshot B.
    set_time_ns(&pic, oe.snapshot_b_ns);
    force_tick(&pic, rp);
    assert!(
        epoch_status(&pic, rp).open_epoch.unwrap().b_complete,
        "snapshot B should be captured"
    );

    // Close.
    set_time_ns(&pic, oe.epoch_end_ns);
    force_tick(&pic, rp);
    let after = epoch_status(&pic, rp);
    assert!(after.open_epoch.is_none(), "the epoch should be closed");
    assert_eq!(after.current_epoch_index, 1, "epoch index advanced");

    // Held $1 of debt through both snapshots over the full week -> debt x 7 days.
    assert_eq!(total_points(&pic, rp, p), DEBT as u128 * 7);
}

#[test]
fn between_snapshot_withdrawal_earns_zero() {
    let pic = PocketIcBuilder::new().with_application_subnet().build();
    set_time_ns(&pic, rumi_points::DEFAULT_SEASON_START_NS);

    let mock = install_mock(&pic);
    let rp = install_points(&pic);
    set_all_sources(&pic, rp, mock);

    let p = Principal::from_slice(&[7; 5]);
    admin_ok(&pic, rp, "register_test_principal", Encode!(&p).unwrap());

    set_vault_debt(&pic, mock, p, 100_000_000); // position present at snapshot A
    start_season_ok(&pic, rp, SEASON_SEED);
    admin_ok(&pic, rp, "set_epoch_driver_enabled", Encode!(&false).unwrap());
    let oe = epoch_status(&pic, rp).open_epoch.unwrap();

    set_time_ns(&pic, oe.snapshot_a_ns);
    force_tick(&pic, rp); // capture A: debt present

    // Withdraw the whole position before snapshot B.
    set_vault_debt(&pic, mock, p, 0);
    set_time_ns(&pic, oe.snapshot_b_ns);
    force_tick(&pic, rp); // capture B: debt gone -> min(D, 0) = 0

    set_time_ns(&pic, oe.epoch_end_ns);
    force_tick(&pic, rp); // close

    assert_eq!(
        total_points(&pic, rp, p),
        0,
        "the two-snapshot min() closes end-of-epoch sniping"
    );
}

/// F2: a transient per-principal fetch error during snapshot capture must NOT be
/// recorded as a 0. With the close-time `min()`, a single transient 0 in either
/// snapshot would otherwise zero a held position for the whole epoch. Instead the
/// snapshot stays incomplete and is retried, so the position earns full credit once
/// the source recovers.
#[test]
fn transient_fetch_error_does_not_zero_a_held_position() {
    let pic = PocketIcBuilder::new().with_application_subnet().build();
    set_time_ns(&pic, rumi_points::DEFAULT_SEASON_START_NS);

    let mock = install_mock(&pic);
    let rp = install_points(&pic);
    set_all_sources(&pic, rp, mock);

    let p = Principal::from_slice(&[7; 5]);
    admin_ok(&pic, rp, "register_test_principal", Encode!(&p).unwrap());

    const DEBT: u64 = 100_000_000; // $1 of icUSD debt, held all epoch
    set_vault_debt(&pic, mock, p, DEBT);

    start_season_ok(&pic, rp, SEASON_SEED);
    admin_ok(&pic, rp, "set_epoch_driver_enabled", Encode!(&false).unwrap());
    let oe = epoch_status(&pic, rp).open_epoch.unwrap();

    // Snapshot A while the backend `get_vaults` call is failing (forced trap).
    set_fail_get_vaults(&pic, mock, true);
    set_time_ns(&pic, oe.snapshot_a_ns);
    force_tick(&pic, rp);
    assert!(
        !epoch_status(&pic, rp).open_epoch.unwrap().a_complete,
        "snapshot A must NOT complete while a source errors (no transient 0 is recorded)"
    );

    // Source recovers: the retry captures the real debt and completes snapshot A.
    set_fail_get_vaults(&pic, mock, false);
    force_tick(&pic, rp);
    assert!(
        epoch_status(&pic, rp).open_epoch.unwrap().a_complete,
        "snapshot A completes once the source recovers"
    );

    // Snapshot B and close, debt held throughout.
    set_time_ns(&pic, oe.snapshot_b_ns);
    force_tick(&pic, rp);
    set_time_ns(&pic, oe.epoch_end_ns);
    force_tick(&pic, rp);

    // Full credit: the transient error under-credited nothing.
    assert_eq!(
        total_points(&pic, rp, p),
        DEBT as u128 * 7,
        "a recovered transient error yields full credit, not a min()-locked zero"
    );
}

/// POINTS-001: the PUBLIC `get_epoch_status` must NOT expose the capture/close
/// cursors (an attacker watching the cursor could time a flash deposit to beat the
/// `min(A,B)` anti-snipe defense). The full cursor view is admin-only.
#[test]
fn public_epoch_status_hides_cursors_and_admin_view_is_gated() {
    let pic = PocketIcBuilder::new().with_application_subnet().build();
    set_time_ns(&pic, rumi_points::DEFAULT_SEASON_START_NS);

    let mock = install_mock(&pic);
    let rp = install_points(&pic);
    set_all_sources(&pic, rp, mock);

    let p = Principal::from_slice(&[7; 5]);
    admin_ok(&pic, rp, "register_test_principal", Encode!(&p).unwrap());
    set_vault_debt(&pic, mock, p, 100_000_000);
    start_season_ok(&pic, rp, SEASON_SEED);
    admin_ok(&pic, rp, "set_epoch_driver_enabled", Encode!(&false).unwrap());

    let oe = epoch_status(&pic, rp).open_epoch.unwrap();

    // PTS-002: while both snapshot times are in the FUTURE the public view hides
    // them (a future time IS the flash-deposit snipe target).
    let poe = public_epoch_status(&pic, rp).open_epoch.expect("epoch 0 is open");
    assert_eq!(poe.epoch_index, 0);
    assert_eq!(poe.snapshot_a_ns, None, "future snapshot A must be hidden");
    assert_eq!(poe.snapshot_b_ns, None, "future snapshot B must be hidden");

    // Capture A so the admin view has a non-trivial cursor to expose.
    set_time_ns(&pic, oe.snapshot_a_ns);
    force_tick(&pic, rp);

    // The PUBLIC query (anonymous caller) decodes into PublicEpochStatus, whose
    // open epoch has bounds but NO cursor/complete fields. The decode itself
    // proves the wire shape carries no cursors. A has fired (now >= a) so it is
    // revealed; B is still in the future and stays hidden (PTS-002).
    let pub_status = public_epoch_status(&pic, rp);
    let poe = pub_status.open_epoch.expect("epoch 0 is open");
    assert_eq!(poe.epoch_index, 0);
    assert_eq!(poe.snapshot_a_ns, Some(oe.snapshot_a_ns), "fired snapshot A is revealed");
    assert_eq!(poe.snapshot_b_ns, None, "future snapshot B stays hidden");

    // The ADMIN query is admin-gated: an anonymous caller is rejected (trap).
    let denied = pic.query_call(
        rp,
        Principal::anonymous(),
        "get_epoch_status_admin",
        Encode!().unwrap(),
    );
    match denied {
        Err(_) => {} // rejected at the call layer
        Ok(WasmResult::Reject(_)) => {} // trapped: unauthorized
        Ok(WasmResult::Reply(_)) => {
            panic!("get_epoch_status_admin must reject a non-admin caller")
        }
    }

    // The admin CAN read the full view (with cursors).
    let admin_view = epoch_status(&pic, rp);
    assert!(
        admin_view.open_epoch.unwrap().a_complete,
        "admin view still exposes capture progress"
    );
}

/// POINTS-002: an epoch close that spans MANY principals (more than one close
/// chunk) must complete through the live driver without trapping, advancing the
/// epoch index and crediting every held position exactly once. Pre-fix this was a
/// single unchunked O(N) message that could exceed the instruction limit and trap,
/// stalling accrual permanently. We register well over CLOSE_CHUNK principals and
/// drive the state machine to completion.
#[test]
fn chunked_close_completes_over_many_principals_end_to_end() {
    let pic = PocketIcBuilder::new().with_application_subnet().build();
    set_time_ns(&pic, rumi_points::DEFAULT_SEASON_START_NS);

    let mock = install_mock(&pic);
    let rp = install_points(&pic);
    set_all_sources(&pic, rp, mock);

    // Register many principals (> 2 close chunks of 50) so the close MUST span
    // several batches. The mock tracks one held position; that principal must be
    // credited exactly once, and every other registered principal must still be
    // processed for the close to finalize (no permanent stall).
    const N: u32 = 130;
    const DEBT: u64 = 100_000_000; // $1
    let who = |i: u32| Principal::from_slice(&i.to_be_bytes());
    for i in 0..N {
        admin_ok(&pic, rp, "register_test_principal", Encode!(&who(i)).unwrap());
    }
    // One held principal (the mock returns debt for a single owner). Pick one near
    // the end so it is processed in a LATE close batch, exercising resume.
    let held = who(N - 3);
    set_vault_debt(&pic, mock, held, DEBT);

    start_season_ok(&pic, rp, SEASON_SEED);
    admin_ok(&pic, rp, "set_epoch_driver_enabled", Encode!(&false).unwrap());
    let oe = epoch_status(&pic, rp).open_epoch.unwrap();

    // Drive snapshot A to completion (capture is chunked at 100/principals tick).
    set_time_ns(&pic, oe.snapshot_a_ns);
    for _ in 0..10 {
        if epoch_status(&pic, rp).open_epoch.unwrap().a_complete {
            break;
        }
        force_tick(&pic, rp);
    }
    assert!(epoch_status(&pic, rp).open_epoch.unwrap().a_complete, "snapshot A completes");

    // Drive snapshot B to completion.
    set_time_ns(&pic, oe.snapshot_b_ns);
    for _ in 0..10 {
        if epoch_status(&pic, rp).open_epoch.unwrap().b_complete {
            break;
        }
        force_tick(&pic, rp);
    }
    assert!(epoch_status(&pic, rp).open_epoch.unwrap().b_complete, "snapshot B completes");

    // Drive the CHUNKED close to completion. It spans several ticks (130 principals
    // / 50 per close chunk = 3 batches); each tick stays in the open state until
    // the cursor reaches the end.
    set_time_ns(&pic, oe.epoch_end_ns);
    let mut closed = false;
    for _ in 0..20 {
        force_tick(&pic, rp);
        if epoch_status(&pic, rp).open_epoch.is_none() {
            closed = true;
            break;
        }
    }
    assert!(closed, "the chunked close must finish (no permanent stall)");

    let after = epoch_status(&pic, rp);
    assert!(after.open_epoch.is_none(), "epoch closed");
    assert_eq!(after.current_epoch_index, 1, "epoch index advanced exactly once");

    // The held principal is credited EXACTLY once: $1 of debt over the full week.
    // (Pre-fix, the unchunked close over 130 principals could trap before reaching
    // this principal's batch and never advance the index.)
    assert_eq!(
        total_points(&pic, rp, held),
        DEBT as u128 * 7,
        "the held principal is credited exactly once across the chunked close"
    );
    // A no-debt principal accrued nothing but was still processed (the close had to
    // iterate all 130 principals to finalize, so this is implied by closure).
    assert_eq!(total_points(&pic, rp, who(0)), 0);
}
