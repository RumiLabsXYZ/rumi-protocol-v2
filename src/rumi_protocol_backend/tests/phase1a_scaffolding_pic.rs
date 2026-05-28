//! Phase 1a PocketIC smoke test.
//!
//! Boots `rumi_protocol_backend`, calls every new endpoint, then upgrades
//! the canister in place and re-checks every query. The upgrade round-trip
//! is the guard against the AMM-style state-wipe failure mode: if the
//! new `multi_chain` field's CBOR shape goes sideways, the second
//! `get_supply_audit()` would lose its registered chain.

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
struct ProtocolUpgradeArg {
    mode: Option<Mode>,
    description: Option<String>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum Mode {
    GeneralAvailability,
    Recovery,
    ReadOnly,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum ProtocolArg {
    Init(ProtocolInitArg),
    Upgrade(ProtocolUpgradeArg),
}

// Wire types matching the .did exactly.
// `chain_id : nat32` -> u32 (ChainId newtype serializes as its inner u32).
// `supply_e8s : nat` and `total_e8s : nat` -> candid::Nat.
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
    include_bytes!(
        "../../../target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm"
    )
    .to_vec()
}

fn boot() -> (PocketIc, Principal) {
    let pic = PocketIc::new();
    let protocol_id = pic.create_canister();
    pic.add_cycles(protocol_id, 100_000_000_000_000);

    // Phase 1a's PocketIC test does NOT install the ICP ledger, icusd
    // ledger, or XRC. We only exercise the chain-agnostic surface
    // (queries + upgrade round-trip), none of which make inter-canister
    // calls. The init principals point at the management canister so
    // any accidental outbound call traps fast.
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
    // Advance time so a subsequent upgrade_canister call clears the
    // CanisterInstallCodeRateLimited window that applies to large wasms.
    pic.advance_time(Duration::from_secs(600));
    for _ in 0..10 {
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

#[test]
fn phase1a_queries_return_empty_on_fresh_canister() {
    let (pic, cid) = boot();
    let supply: candid::Nat = query(&pic, cid, "get_global_icusd_supply");
    assert_eq!(supply, candid::Nat::from(0u32));

    let audit: SupplyAuditWire = query(&pic, cid, "get_supply_audit");
    assert_eq!(audit.total_e8s, candid::Nat::from(0u32));
    assert!(audit.per_chain.is_empty());
}

#[test]
fn phase1a_state_survives_upgrade() {
    let (pic, cid) = boot();
    let upgrade = ProtocolArg::Upgrade(ProtocolUpgradeArg {
        mode: None,
        description: Some("Phase 1a PocketIC self-check".to_string()),
    });
    pic.upgrade_canister(
        cid,
        backend_wasm(),
        encode_args((upgrade,)).expect("encode upgrade"),
        None,
    )
    .expect("upgrade");

    // After the upgrade, the multi_chain field must round-trip cleanly.
    // get_supply_audit returns the same empty shape; if the CBOR went
    // sideways, either decoding traps or the per_chain length jumps.
    let audit: SupplyAuditWire = query(&pic, cid, "get_supply_audit");
    assert_eq!(audit.total_e8s, candid::Nat::from(0u32));
    assert!(audit.per_chain.is_empty());
}
