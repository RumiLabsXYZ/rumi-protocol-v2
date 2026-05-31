//! Phase 1b Task 15 PocketIC smoke test: NO-CHAIN TIMER SAFETY.
//!
//! Task 15 wires the Monad inbound observer (`observer_tick`) and the outbound
//! settlement worker (`settlement_tick`, Timer D) onto live `set_timer_interval`
//! timers (default 30s) in `setup_timers`. Both tick fns fan out over the set of
//! REGISTERED+ENABLED chains; with no chain configured that set is empty and
//! each tick is a pure no-op (it never makes an inter-canister / EVM RPC call).
//!
//! This test boots the backend with NO chains registered, then advances time
//! well past several 30s timer firings (120s across repeated ticks) and asserts
//! the canister stays ALIVE: queries still answer and still return the empty /
//! zero shape. If either tick trapped or panicked with no chain configured (e.g.
//! an unguarded state borrow, an `unwrap` on a missing chain, or a 0s busy-loop
//! that froze the canister), a post-advance query would reject and this test
//! would fail.
//!
//! This is the NO-CHAIN safety test ONLY. The full happy-path (register Monad,
//! deposit -> mint -> confirm via mocked RPC) is Task 17. The init principals
//! point at the management canister so any accidental outbound call traps fast,
//! which is exactly what we want: it would surface a tick that wrongly tried to
//! do work with no chain.

use candid::{encode_args, encode_one, CandidType, Decode, Deserialize, Principal};
use pocket_ic::{PocketIc, WasmResult};
use std::time::Duration;

// Locally-mirrored types so this test does not pull in the crate's macros.
// Field shapes must mirror `src/rumi_protocol_backend/src/lib.rs` exactly.

#[derive(CandidType, Deserialize, Clone, Debug)]
struct ProtocolInitArg {
    xrc_principal: Principal,
    icusd_ledger_principal: Principal,
    icp_ledger_principal: Principal,
    fee_e8s: u64,
    developer_principal: Principal,
    treasury_principal: Option<Principal>,
    stability_pool_principal: Option<Principal>,
    ckusdt_ledger_principal: Option<Principal>,
    ckusdc_ledger_principal: Option<Principal>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum ProtocolArg {
    Init(ProtocolInitArg),
}

// Wire type matching the `get_supply_audit` reply in the .did.
#[derive(CandidType, Deserialize, Clone, Debug)]
struct SupplyAuditEntryWire {
    chain_id: u32,
    display_name: String,
    supply_e8s: candid::Nat,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct SupplyAuditWire {
    total_e8s: candid::Nat,
    per_chain: Vec<SupplyAuditEntryWire>,
}

fn backend_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm")
        .to_vec()
}

/// Boot the backend with NO chains registered. Init principals point at the
/// management canister so any accidental outbound call traps fast.
fn boot() -> (PocketIc, Principal) {
    let pic = PocketIc::new();
    let protocol_id = pic.create_canister();
    pic.add_cycles(protocol_id, 100_000_000_000_000);

    let mgmt = Principal::from_text("aaaaa-aa").expect("mgmt principal");
    let developer = Principal::from_text("aaaaa-aa").expect("dev principal");

    let init = ProtocolArg::Init(ProtocolInitArg {
        xrc_principal: mgmt,
        icusd_ledger_principal: mgmt,
        icp_ledger_principal: mgmt,
        fee_e8s: 10_000,
        developer_principal: developer,
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    });
    pic.install_canister(
        protocol_id,
        backend_wasm(),
        encode_args((init,)).expect("encode init"),
        None,
    );
    // Let the install settle (timers registered in `setup_timers`).
    for _ in 0..5 {
        pic.tick();
    }
    (pic, protocol_id)
}

fn query<T>(pic: &PocketIc, cid: Principal, method: &str) -> T
where
    T: CandidType + for<'a> Deserialize<'a>,
{
    let reply = pic
        .query_call(
            cid,
            Principal::anonymous(),
            method,
            encode_one(()).expect("encode unit"),
        )
        .expect("query call");
    match reply {
        WasmResult::Reply(b) => Decode!(&b, T).expect("decode reply"),
        WasmResult::Reject(msg) => panic!("query {} rejected: {}", method, msg),
    }
}

/// The core no-chain safety assertion: fire the observer + settlement timers
/// several times (advance in steps past the 300s default interval, ticking
/// between each) and prove the canister is still alive and still empty afterward.
#[test]
fn phase1b_timers_safe_with_no_chain_configured() {
    let (pic, cid) = boot();

    // Sanity: empty/zero before any timer fires.
    let supply: u128 = query(&pic, cid, "get_global_icusd_supply");
    assert_eq!(supply, 0u128, "fresh canister should report zero global supply");
    let audit: SupplyAuditWire = query(&pic, cid, "get_supply_audit");
    assert_eq!(audit.total_e8s, candid::Nat::from(0u32));
    assert!(audit.per_chain.is_empty(), "no chains registered -> empty audit");

    // Advance time past several timer firings (steps > the 300s default interval).
    // With no chain registered, every observer_tick / settlement_tick is a no-op
    // fan-out over an empty chain list. If either trapped, the queries below would
    // reject.
    for _ in 0..4 {
        pic.advance_time(Duration::from_secs(305));
        // Several ticks per step so the spawned async tick futures get scheduled
        // and run to completion (a no-op return) within the step.
        for _ in 0..5 {
            pic.tick();
        }
    }

    // Canister still alive: queries still answer and still return empty/zero.
    let supply_after: u128 = query(&pic, cid, "get_global_icusd_supply");
    assert_eq!(
        supply_after, 0u128,
        "global supply must remain zero after observer/settlement timers fired with no chain"
    );
    let audit_after: SupplyAuditWire = query(&pic, cid, "get_supply_audit");
    assert_eq!(
        audit_after.total_e8s,
        candid::Nat::from(0u32),
        "supply audit total must remain zero after timers fired"
    );
    assert!(
        audit_after.per_chain.is_empty(),
        "no chains should appear after timers fired with none registered"
    );
}
