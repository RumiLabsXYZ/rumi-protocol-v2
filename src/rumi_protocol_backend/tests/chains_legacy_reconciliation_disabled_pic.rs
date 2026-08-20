//! PocketIC integration test for the hard-disabled legacy reconciliation
//! endpoints (`settle_pending_chain_burn`, `settle_reserve_burn`; main.rs).
//!
//! These endpoints kept their exact pre-existing candid signatures (no
//! breaking removal) but now refuse unconditionally, regardless of caller
//! identity, before touching state. The unit-level proof lives in main.rs's
//! `legacy_reconciliation_hard_disabled_tests` (calls the Rust functions
//! directly, since the disabled bodies no longer read `ic_cdk::caller()` at
//! all). This file is the real-candid-boundary companion: it proves the
//! SAME refusal through an actual canister call, AS THE DEVELOPER PRINCIPAL
//! specifically (the one caller who used to be authorized), so there is no
//! doubt the disablement applies to every caller, not just unauthorized ones.

use candid::{encode_args, CandidType, Decode, Deserialize, Principal};
use pocket_ic::{PocketIc, WasmResult};

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

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
struct ChainId(u32);

#[derive(CandidType, Deserialize, Clone, Debug)]
enum ProtocolError {
    GenericError(String),
    TemporarilyUnavailable(String),
    ChainAdmin(String),
    AmountTooLow { minimum_amount: u64 },
    CallerNotOwner,
    AlreadyProcessing,
    NotLowestCR,
    SupplyInvariantHalted,
    AnonymousCallerNotAllowed,
}

const CFX_MAINNET: ChainId = ChainId(1030);

fn backend_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm")
        .to_vec()
}

fn developer() -> Principal {
    Principal::from_slice(&[13; 29])
}

fn boot() -> (PocketIc, Principal) {
    let pic = PocketIc::new();
    let cid = pic.create_canister();
    pic.add_cycles(cid, 100_000_000_000_000);

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
    for _ in 0..5 {
        pic.tick();
    }
    (pic, cid)
}

fn call_as_developer(
    pic: &PocketIc,
    cid: Principal,
    method: &str,
) -> Result<(), ProtocolError> {
    let args = encode_args((CFX_MAINNET, 100u128, "any proof".to_string())).expect("encode");
    let reply = pic
        .update_call(cid, developer(), method, args)
        .unwrap_or_else(|e| panic!("{} call: {:?}", method, e));
    match reply {
        WasmResult::Reply(b) => {
            Decode!(&b, Result<(), ProtocolError>).unwrap_or_else(|e| panic!("decode {}: {}", method, e))
        }
        WasmResult::Reject(msg) => panic!("{} rejected at the candid layer: {}", method, msg),
    }
}

#[test]
fn settle_pending_chain_burn_refuses_even_for_the_developer_principal() {
    let (pic, cid) = boot();
    let err = call_as_developer(&pic, cid, "settle_pending_chain_burn")
        .expect_err("the legacy endpoint must refuse, even for the developer principal");
    match err {
        ProtocolError::ChainAdmin(msg) => {
            assert!(msg.contains("disabled"), "msg={msg}");
            assert!(
                msg.contains("settle_pending_chain_burn_with_proof"),
                "msg should point at the receipt-verified replacement: msg={msg}"
            );
        }
        other => panic!("expected ChainAdmin, got {other:?}"),
    }
}

#[test]
fn settle_reserve_burn_refuses_even_for_the_developer_principal() {
    let (pic, cid) = boot();
    let err = call_as_developer(&pic, cid, "settle_reserve_burn")
        .expect_err("the legacy endpoint must refuse, even for the developer principal");
    match err {
        ProtocolError::ChainAdmin(msg) => {
            assert!(msg.contains("disabled"), "msg={msg}");
            assert!(
                msg.contains("settle_reserve_burn_with_proof"),
                "msg should point at the receipt-verified replacement: msg={msg}"
            );
        }
        other => panic!("expected ChainAdmin, got {other:?}"),
    }
}
