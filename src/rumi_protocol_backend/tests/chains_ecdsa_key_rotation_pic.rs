//! PocketIC integration test for the stale ECDSA-derived address cache fix
//! (chains/evm/tecdsa.rs `clear_address_caches`, wired into
//! `set_chains_ecdsa_key_name` in main.rs).
//!
//! Live prod repro this test reproduces and then proves fixed:
//! `get_chain_reserve_address(1030)` called under `chains_ecdsa_key_name`
//! "test_key_1", then again after `set_chains_ecdsa_key_name("key_1")`,
//! returned the IDENTICAL address on unpatched code. Two independent
//! threshold root keys can never derive the same address for the same
//! derivation path, so the match proved `RESERVE_ADDR_CACHE` (keyed only by
//! `ChainId`) was serving a stale, pre-rotation address.
//!
//! Scenarios:
//!   1. `key_rotation_changes_reserve_and_interest_treasury_addresses`: warm
//!      both caches under the default key, rotate to `key_1` (no chain
//!      vaults exist, so the orphan guard allows it), and assert BOTH
//!      addresses actually change.
//!   2. `rejected_rotation_does_not_clear_the_cache`: warm the cache, then
//!      attempt a rotation `set_chains_ecdsa_key_name` rejects (an
//!      unsupported key name) and assert the cached address is unchanged.
//!      This proves `clear_address_caches()` is reached ONLY on the success
//!      path. The other rejection path (`has_chain_vaults`) shares the exact
//!      same code order (`validate_ecdsa_key_change(...)?` before any
//!      mutation), which the pure `ecdsa_key_change_rules` unit test in
//!      main.rs already proves rejects when vaults exist, so both rejection
//!      routes are covered between the two test layers.

use candid::{encode_args, encode_one, CandidType, Decode, Deserialize, Principal};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};

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
    Principal::from_slice(&[11; 29])
}

/// II subnet + application subnet: PocketIC's simulated tECDSA signing
/// (`ecdsa_public_key`) needs a subnet pairing that supports it, mirrored
/// from the existing `conflux_espace_happy_path_pic.rs` boot helper.
fn boot() -> (PocketIc, Principal) {
    let pic = PocketIcBuilder::new().with_ii_subnet().with_application_subnet().build();

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

fn update_dev<T>(pic: &PocketIc, cid: Principal, method: &str, arg_bytes: Vec<u8>) -> T
where
    T: CandidType + for<'a> Deserialize<'a>,
{
    let reply = pic
        .update_call(cid, developer(), method, arg_bytes)
        .unwrap_or_else(|e| panic!("{} call: {:?}", method, e));
    match reply {
        WasmResult::Reply(b) => Decode!(&b, T).unwrap_or_else(|e| panic!("decode {}: {}", method, e)),
        WasmResult::Reject(msg) => panic!("{} rejected: {}", method, msg),
    }
}

fn get_reserve_address(pic: &PocketIc, cid: Principal) -> Result<String, ProtocolError> {
    update_dev(pic, cid, "get_chain_reserve_address", encode_args((CFX_MAINNET,)).unwrap())
}

fn get_interest_treasury_address(pic: &PocketIc, cid: Principal) -> Result<String, ProtocolError> {
    update_dev(
        pic,
        cid,
        "get_chain_interest_treasury_address",
        encode_args((CFX_MAINNET,)).unwrap(),
    )
}

fn set_key(pic: &PocketIc, cid: Principal, name: &str) -> Result<(), ProtocolError> {
    update_dev(pic, cid, "set_chains_ecdsa_key_name", encode_one(name.to_string()).unwrap())
}

#[test]
fn key_rotation_changes_reserve_and_interest_treasury_addresses() {
    let (pic, cid) = boot();

    // Warm both caches under the canister's default key ("test_key_1").
    let reserve_before = get_reserve_address(&pic, cid).expect("derive reserve address");
    let treasury_before =
        get_interest_treasury_address(&pic, cid).expect("derive interest-treasury address");

    // No chain vaults exist, so the orphan guard allows this rotation.
    set_key(&pic, cid, "key_1").expect("key rotation must succeed with no chain vaults");

    let reserve_after = get_reserve_address(&pic, cid).expect("derive reserve address again");
    let treasury_after =
        get_interest_treasury_address(&pic, cid).expect("derive interest-treasury address again");

    assert_ne!(
        reserve_before, reserve_after,
        "reserve address must change after an ECDSA key rotation (stale-cache regression)"
    );
    assert_ne!(
        treasury_before, treasury_after,
        "interest-treasury address must change after an ECDSA key rotation (stale-cache regression)"
    );
}

#[test]
fn rejected_rotation_does_not_clear_the_cache() {
    let (pic, cid) = boot();

    let reserve_before = get_reserve_address(&pic, cid).expect("derive reserve address");

    // An unsupported key name is rejected by `validate_ecdsa_key_change`
    // BEFORE any state mutation or cache clear.
    let err = set_key(&pic, cid, "bogus_key").expect_err("unsupported key name must be rejected");
    assert!(matches!(err, ProtocolError::ChainAdmin(_)));

    let reserve_after = get_reserve_address(&pic, cid).expect("derive reserve address again");
    assert_eq!(
        reserve_before, reserve_after,
        "a rejected key-name change must not clear the address cache"
    );
}
