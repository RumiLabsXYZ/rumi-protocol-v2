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

fn is_registered(pic: &pocket_ic::PocketIc, rp: Principal, who: Principal) -> bool {
    let res = pic
        .query_call(rp, Principal::anonymous(), "is_registered", Encode!(&who).unwrap())
        .expect("is_registered call failed");
    match res {
        WasmResult::Reply(b) => Decode!(&b, bool).unwrap(),
        WasmResult::Reject(m) => panic!("is_registered rejected: {m}"),
    }
}
