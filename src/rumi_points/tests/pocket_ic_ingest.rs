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

fn install_points(pic: &pocket_ic::PocketIc) -> Principal {
    let rp = pic.create_canister();
    pic.add_cycles(rp, 4_000_000_000_000);
    let init = InitArgs {
        admin: Some(admin()),
        excluded_principals: None,
        season_start_ns: None,
        season_end_ns: None,
        snapshot_seed_commit: None, // uncommitted: start_season accepts any seed
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

fn epoch_status(pic: &pocket_ic::PocketIc, rp: Principal) -> TEpochStatus {
    let res = pic
        .query_call(rp, Principal::anonymous(), "get_epoch_status", Encode!().unwrap())
        .expect("get_epoch_status call failed");
    match res {
        WasmResult::Reply(b) => Decode!(&b, TEpochStatus).unwrap(),
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

    // Open epoch 0 (uncommitted singleton accepts the seed without verification),
    // then disable the timer so only force_tick drives the state machine.
    start_season_ok(&pic, rp, [42u8; 32]);
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
    start_season_ok(&pic, rp, [42u8; 32]);
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
