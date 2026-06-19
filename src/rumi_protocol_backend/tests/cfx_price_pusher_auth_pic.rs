//! PocketIC integration test for the audit F-01 price-pusher auth path.
//!
//! Proves end-to-end, through the real candid boundary, that:
//!   * a stranger CANNOT call `set_manual_collateral_price`,
//!   * the developer can grant a narrowly-scoped price-pusher principal,
//!   * the pusher CAN set the price (and ONLY the price),
//!   * `get_manual_collateral_price` returns the value + a non-zero set timestamp,
//!   * the developer can still set the price,
//!   * the price + pusher survive a real upgrade (V5 multi-chain migration + the
//!     new `price_pusher_principal` State field round-trip without a state wipe).

use candid::{encode_args, encode_one, CandidType, Decode, Deserialize, Principal};
use pocket_ic::{PocketIc, WasmResult};
use std::time::Duration;

// ─── Init/upgrade types (mirror lib.rs exactly) ──────────────────────────────

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

// ─── Wire types matching the .did exactly ────────────────────────────────────

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq)]
struct ManualPriceInfo {
    price_e8: u64,
    set_at_ns: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum TransferErrorWire {
    GenericError { message: String, error_code: candid::Nat },
    TemporarilyUnavailable,
    BadBurn { min_burn_amount: candid::Nat },
    Duplicate { duplicate_of: candid::Nat },
    BadFee { expected_fee: candid::Nat },
    CreatedInFuture { ledger_time: u64 },
    TooOld,
    InsufficientFunds { balance: candid::Nat },
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum TransferFromErrorWire {
    GenericError { message: String, error_code: candid::Nat },
    TemporarilyUnavailable,
    InsufficientAllowance { allowance: candid::Nat },
    BadBurn { min_burn_amount: candid::Nat },
    Duplicate { duplicate_of: candid::Nat },
    BadFee { expected_fee: candid::Nat },
    CreatedInFuture { ledger_time: u64 },
    TooOld,
    InsufficientFunds { balance: candid::Nat },
}

// Full ProtocolError mirror so `Decode!` is unambiguous regardless of which
// variant the canister returns.
#[derive(CandidType, Deserialize, Clone, Debug)]
enum ProtocolError {
    GenericError(String),
    TemporarilyUnavailable(String),
    TransferError(TransferErrorWire),
    AlreadyProcessing,
    NotLowestCR,
    SupplyInvariantHalted,
    AnonymousCallerNotAllowed,
    ChainAdmin(String),
    AmountTooLow { minimum_amount: u64 },
    TransferFromError(TransferFromErrorWire, u64),
    CallerNotOwner,
}

const CFX_CHAIN: u32 = 1030;

fn backend_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm").to_vec()
}

fn developer() -> Principal {
    Principal::from_slice(&[7; 29])
}
fn pusher() -> Principal {
    Principal::from_slice(&[5; 29])
}
fn stranger() -> Principal {
    Principal::from_slice(&[9; 29])
}

fn boot() -> (PocketIc, Principal) {
    let pic = PocketIc::new();
    let cid = pic.create_canister();
    pic.add_cycles(cid, 100_000_000_000_000);

    // No ledgers/XRC needed — the price-setter surface makes no inter-canister
    // calls. Mgmt principal for the unused ledger fields traps any stray call.
    let mgmt = Principal::from_text("aaaaa-aa").expect("mgmt");
    let init = ProtocolArg::Init(ProtocolInitArg {
        xrc_principal: mgmt,
        icusd_ledger_principal: mgmt,
        icp_ledger_principal: mgmt,
        fee_e8s: 10_000,
        developer_principal: developer(),
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    });
    pic.install_canister(cid, backend_wasm(), encode_args((init,)).expect("encode init"), None);
    pic.advance_time(Duration::from_secs(600));
    for _ in 0..10 {
        pic.tick();
    }
    (pic, cid)
}

// ─── call helpers ────────────────────────────────────────────────────────────

fn set_price(pic: &PocketIc, cid: Principal, sender: Principal, price_e8: u64) -> Result<(), ProtocolError> {
    let args = encode_args((CFX_CHAIN, "CFX".to_string(), price_e8)).expect("encode");
    let reply = pic
        .update_call(cid, sender, "set_manual_collateral_price", args)
        .expect("update call");
    match reply {
        WasmResult::Reply(b) => Decode!(&b, Result<(), ProtocolError>).expect("decode Result"),
        WasmResult::Reject(msg) => panic!("set_manual_collateral_price rejected: {msg}"),
    }
}

fn set_pusher(pic: &PocketIc, cid: Principal, sender: Principal, p: Option<Principal>) -> Result<(), ProtocolError> {
    let reply = pic
        .update_call(cid, sender, "set_price_pusher_principal", encode_one(p).expect("encode"))
        .expect("update call");
    match reply {
        WasmResult::Reply(b) => Decode!(&b, Result<(), ProtocolError>).expect("decode Result"),
        WasmResult::Reject(msg) => panic!("set_price_pusher_principal rejected: {msg}"),
    }
}

fn get_pusher(pic: &PocketIc, cid: Principal) -> Option<Principal> {
    let reply = pic
        .query_call(cid, Principal::anonymous(), "get_price_pusher_principal", encode_one(()).expect("encode"))
        .expect("query");
    match reply {
        WasmResult::Reply(b) => Decode!(&b, Option<Principal>).expect("decode"),
        WasmResult::Reject(msg) => panic!("get_price_pusher_principal rejected: {msg}"),
    }
}

fn get_price(pic: &PocketIc, cid: Principal) -> Option<ManualPriceInfo> {
    let args = encode_args((CFX_CHAIN, "CFX".to_string())).expect("encode");
    let reply = pic
        .query_call(cid, Principal::anonymous(), "get_manual_collateral_price", args)
        .expect("query");
    match reply {
        WasmResult::Reply(b) => Decode!(&b, Option<ManualPriceInfo>).expect("decode"),
        WasmResult::Reject(msg) => panic!("get_manual_collateral_price rejected: {msg}"),
    }
}

fn assert_chain_admin_err(r: Result<(), ProtocolError>) {
    match r {
        Err(ProtocolError::ChainAdmin(_)) => {}
        other => panic!("expected ChainAdmin error, got {other:?}"),
    }
}

// ─── tests ───────────────────────────────────────────────────────────────────

#[test]
fn stranger_cannot_set_price_until_granted() {
    let (pic, cid) = boot();

    // No pusher registered yet: a stranger is rejected, the developer is allowed.
    assert_chain_admin_err(set_price(&pic, cid, stranger(), 5_000_000));
    assert!(get_price(&pic, cid).is_none(), "no price should be set after a rejected call");
    set_price(&pic, cid, developer(), 5_000_000).expect("developer may set price");

    // Developer grants the scoped pusher.
    set_pusher(&pic, cid, developer(), Some(pusher())).expect("developer grants pusher");
    assert_eq!(get_pusher(&pic, cid), Some(pusher()));

    // A stranger granting a pusher is rejected (setter is developer-gated).
    assert_chain_admin_err(set_pusher(&pic, cid, stranger(), Some(stranger())));

    // The pusher can now set the price; a stranger still cannot.
    set_price(&pic, cid, pusher(), 4_915_384).expect("pusher may set price");
    assert_chain_admin_err(set_price(&pic, cid, stranger(), 1));

    let info = get_price(&pic, cid).expect("price is set");
    assert_eq!(info.price_e8, 4_915_384);
    assert!(info.set_at_ns > 0, "set timestamp must be stamped");
}

#[test]
fn zero_price_is_rejected_even_for_authorized_callers() {
    let (pic, cid) = boot();
    set_pusher(&pic, cid, developer(), Some(pusher())).expect("grant");
    assert_chain_admin_err(set_price(&pic, cid, pusher(), 0));
    assert_chain_admin_err(set_price(&pic, cid, developer(), 0));
}

#[test]
fn price_and_pusher_survive_upgrade() {
    let (pic, cid) = boot();
    set_pusher(&pic, cid, developer(), Some(pusher())).expect("grant");
    set_price(&pic, cid, pusher(), 4_915_384).expect("set");
    let before = get_price(&pic, cid).expect("price set");

    let upgrade = ProtocolArg::Upgrade(ProtocolUpgradeArg {
        mode: None,
        description: Some("CFX price-pusher auth PocketIC self-check".to_string()),
    });
    pic.upgrade_canister(cid, backend_wasm(), encode_args((upgrade,)).expect("encode upgrade"), None)
        .expect("upgrade");

    // State-wipe guard: the V5 multi_chain migration + the new State field must
    // round-trip the price, its timestamp, and the registered pusher.
    assert_eq!(get_price(&pic, cid), Some(before), "price + timestamp survive upgrade");
    assert_eq!(get_pusher(&pic, cid), Some(pusher()), "pusher survives upgrade");
    // And the pusher is still authorized after the upgrade.
    set_price(&pic, cid, pusher(), 5_000_000).expect("pusher still authorized post-upgrade");
}
