//! Isolated ic-cdk 0.12.2 async callback control: no Rumi state, ledger or funds.
//! Build the test-only Wasm with `cargo build --manifest-path
//! src/rumi_protocol_backend/tests/cksol_callback_upgrade_mock/Cargo.toml
//! --release --target wasm32-unknown-unknown --locked`. Set
//! CKSOL_CALLBACK_MOCK_WASM to the resulting Wasm and POCKET_IC_BIN to the
//! PocketIC 7.0.0 server, then run `cargo test -p rumi_protocol_backend
//! --test cksol_callback_upgrade_pic -- --ignored --nocapture --test-threads=1`.
//! All canisters and calls are local. Stopping is not attempted on any live IC.
use candid::{decode_one, encode_args, encode_one, Principal};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
fn query(p: &PocketIc, id: Principal, method: &str) -> u64 {
    match p
        .query_call(id, Principal::anonymous(), method, encode_args(()).unwrap())
        .unwrap()
    {
        WasmResult::Reply(b) => decode_one(&b).unwrap(),
        WasmResult::Reject(e) => panic!("{method}: {e}"),
    }
}
#[test]
#[ignore = "requires locally built mock Wasm and PocketIC 7.0.0"]
fn callback_control_with_and_without_upgrade() {
    let p = PocketIcBuilder::new()
        .with_nns_subnet()
        .with_application_subnet()
        .build();
    let mock =
        std::fs::read(std::env::var("CKSOL_CALLBACK_MOCK_WASM").expect("set mock Wasm path"))
            .unwrap();
    let cmc = Principal::from_text("rkp4c-7iaaa-aaaaa-aaaca-cai").unwrap();
    let minter = Principal::from_text("lh22c-kyaaa-aaaar-qb5nq-cai").unwrap();
    for id in [cmc, minter] {
        p.create_canister_with_id(None, None, id).unwrap();
        p.add_cycles(id, 5_000_000_000_000);
        p.install_canister(id, mock.clone(), encode_args(()).unwrap(), None);
    }
    let sender = Principal::self_authenticating(b"callback-upgrade-control");
    let control = p.create_canister();
    p.add_cycles(control, 5_000_000_000_000);
    p.install_canister(control, mock.clone(), encode_args(()).unwrap(), None);
    let _msg = p
        .submit_call(control, sender, "start", encode_one(cmc).unwrap())
        .unwrap();
    for _ in 0..100 {
        p.tick();
        if query(&p, control, "finished") == 1 {
            break;
        }
    }
    assert_eq!(query(&p, control, "started"), 1);
    assert_eq!(query(&p, control, "finished"), 1);
    assert_eq!(query(&p, cmc, "target_completed"), 1);
    let control_logs = p
        .fetch_canister_logs(control, Principal::anonymous())
        .unwrap();
    assert!(!control_logs
        .iter()
        .any(|r| String::from_utf8_lossy(&r.content).contains("[TRAP]")));
    let upgraded = p.create_canister();
    p.add_cycles(upgraded, 5_000_000_000_000);
    p.install_canister(upgraded, mock.clone(), encode_args(()).unwrap(), None);
    let _msg = p
        .submit_call(upgraded, sender, "start", encode_one(cmc).unwrap())
        .unwrap();
    for _ in 0..20 {
        p.tick();
        if query(&p, upgraded, "started") == 1 {
            break;
        }
    }
    assert_eq!(query(&p, upgraded, "started"), 1);
    assert_eq!(query(&p, cmc, "target_completed"), 1);
    p.upgrade_canister(upgraded, mock, encode_args(()).unwrap(), None)
        .unwrap();
    for _ in 0..100 {
        p.tick();
        if query(&p, cmc, "target_completed") == 2 {
            break;
        }
    }
    assert_eq!(
        query(&p, cmc, "target_completed"),
        2,
        "target must finish after caller upgrade"
    );
    for _ in 0..5 {
        p.tick();
    }
    assert_eq!(
        query(&p, upgraded, "finished"),
        0,
        "the old continuation must not run as a fresh completion"
    );
    println!("control completed; upgraded caller discarded late continuation");
    let logs = p
        .fetch_canister_logs(upgraded, Principal::anonymous())
        .unwrap();
    let joined = logs
        .iter()
        .map(|r| String::from_utf8_lossy(&r.content).to_string())
        .collect::<String>();
    assert!(
        joined.contains("[TRAP]: heap out of bounds"),
        "expected the late callback trap: {joined}"
    );
    assert!(
        joined.contains("ic_cdk::api::call::callback"),
        "callback trap missing: {joined}"
    );
    assert!(
        joined.contains("ic_cdk::api::call::cleanup"),
        "cleanup trap missing: {joined}"
    );
    println!("upgraded caller canister logs: {joined}");
}

#[test]
#[ignore = "requires locally built mock Wasm and PocketIC 7.0.0"]
fn stopping_drains_callback_before_upgrade() {
    use pocket_ic::common::rest::RawEffectivePrincipal;
    use pocket_ic::management_canister::{CanisterIdRecord, CanisterStatusResultStatus};
    let p = PocketIcBuilder::new()
        .with_nns_subnet()
        .with_application_subnet()
        .build();
    let mock = std::fs::read(std::env::var("CKSOL_CALLBACK_MOCK_WASM").unwrap()).unwrap();
    let cmc = Principal::from_text("rkp4c-7iaaa-aaaaa-aaaca-cai").unwrap();
    let minter = Principal::from_text("lh22c-kyaaa-aaaar-qb5nq-cai").unwrap();
    for id in [cmc, minter] {
        p.create_canister_with_id(None, None, id).unwrap();
        p.add_cycles(id, 5_000_000_000_000);
        p.install_canister(id, mock.clone(), encode_args(()).unwrap(), None);
    }
    let sender = Principal::self_authenticating(b"callback-drain-control");
    let caller = p.create_canister();
    p.add_cycles(caller, 5_000_000_000_000);
    p.install_canister(caller, mock.clone(), encode_args(()).unwrap(), None);
    let original = p
        .submit_call(caller, sender, "start", encode_one(cmc).unwrap())
        .unwrap();
    for _ in 0..20 {
        p.tick();
        if query(&p, caller, "started") == 1 {
            break;
        }
    }
    assert_eq!(query(&p, caller, "started"), 1);
    assert_eq!(
        query(&p, cmc, "target_completed"),
        0,
        "callback must be in flight"
    );
    let stop = p
        .submit_call_with_effective_principal(
            Principal::management_canister(),
            RawEffectivePrincipal::CanisterId(caller.as_slice().to_vec()),
            Principal::anonymous(),
            "stop_canister",
            encode_one(CanisterIdRecord {
                canister_id: caller,
            })
            .unwrap(),
        )
        .unwrap();
    p.tick();
    let state = p.canister_status(caller, None).unwrap();
    println!("status before draining: {:?}", state.status);
    assert!(
        matches!(state.status, CanisterStatusResultStatus::Stopping),
        "the delayed call must keep stop pending"
    );
    // Never issue the upgrade while stopping; continue until the outstanding
    // ingress has completed and the stop has actually resolved.
    let original_result = p.await_call(original).expect("drained ingress call");
    assert!(matches!(original_result, WasmResult::Reply(_)));
    let stop_result = p.await_call(stop).expect("stop completes after draining");
    assert!(matches!(stop_result, WasmResult::Reply(_)));
    assert_eq!(query(&p, cmc, "target_completed"), 1);
    let stopped = p.canister_status(caller, None).unwrap();
    assert!(matches!(
        stopped.status,
        CanisterStatusResultStatus::Stopped
    ));
    p.upgrade_canister(caller, mock, encode_args(()).unwrap(), None)
        .unwrap();
    p.start_canister(caller, None).unwrap();
    assert_eq!(query(&p, caller, "finished"), 0, "heap resets on upgrade");
    let logs = p
        .fetch_canister_logs(caller, Principal::anonymous())
        .unwrap();
    let joined = logs
        .iter()
        .map(|r| String::from_utf8_lossy(&r.content).to_string())
        .collect::<String>();
    assert!(
        !joined.contains("[TRAP]"),
        "drained caller must not trap: {joined}"
    );
    println!("stop waited for callback; drained upgrade/start completed with no trap");
}

#[test]
#[ignore = "requires locally built mock Wasm and PocketIC 7.0.0"]
fn stopping_can_be_cancelled_while_callback_is_outstanding() {
    use pocket_ic::common::rest::RawEffectivePrincipal;
    use pocket_ic::management_canister::{CanisterIdRecord, CanisterStatusResultStatus};
    let p = PocketIcBuilder::new()
        .with_nns_subnet()
        .with_application_subnet()
        .build();
    let mock = std::fs::read(std::env::var("CKSOL_CALLBACK_MOCK_WASM").unwrap()).unwrap();
    let cmc = Principal::from_text("rkp4c-7iaaa-aaaaa-aaaca-cai").unwrap();
    let minter = Principal::from_text("lh22c-kyaaa-aaaar-qb5nq-cai").unwrap();
    for id in [cmc, minter] {
        p.create_canister_with_id(None, None, id).unwrap();
        p.add_cycles(id, 5_000_000_000_000);
        p.install_canister(id, mock.clone(), encode_args(()).unwrap(), None);
    }
    let sender = Principal::self_authenticating(b"callback-stop-cancel-control");
    let caller = p.create_canister();
    p.add_cycles(caller, 5_000_000_000_000);
    p.install_canister(caller, mock, encode_args(()).unwrap(), None);
    let original = p
        .submit_call(caller, sender, "start", encode_one(cmc).unwrap())
        .unwrap();
    for _ in 0..20 {
        p.tick();
        if query(&p, caller, "started") == 1 {
            break;
        }
    }
    assert_eq!(query(&p, caller, "started"), 1);
    assert_eq!(query(&p, cmc, "target_completed"), 0);
    let stop = p
        .submit_call_with_effective_principal(
            Principal::management_canister(),
            RawEffectivePrincipal::CanisterId(caller.as_slice().to_vec()),
            Principal::anonymous(),
            "stop_canister",
            encode_one(CanisterIdRecord {
                canister_id: caller,
            })
            .unwrap(),
        )
        .unwrap();
    p.tick();
    assert!(matches!(
        p.canister_status(caller, None).unwrap().status,
        CanisterStatusResultStatus::Stopping
    ));
    p.start_canister(caller, None).unwrap();
    assert!(matches!(
        p.canister_status(caller, None).unwrap().status,
        CanisterStatusResultStatus::Running
    ));
    let stop_result = p.await_call(stop);
    assert!(
        !matches!(stop_result, Ok(WasmResult::Reply(_))),
        "cancelled stop must not report success"
    );
    assert!(matches!(p.await_call(original), Ok(WasmResult::Reply(_))));
    assert_eq!(query(&p, caller, "finished"), 1);
    let logs = p
        .fetch_canister_logs(caller, Principal::anonymous())
        .unwrap();
    let joined = logs
        .iter()
        .map(|r| String::from_utf8_lossy(&r.content).to_string())
        .collect::<String>();
    assert!(
        !joined.contains("[TRAP]"),
        "cancelled stop must not trap: {joined}"
    );
    println!("start cancelled the pending stop and original callback completed");
}
