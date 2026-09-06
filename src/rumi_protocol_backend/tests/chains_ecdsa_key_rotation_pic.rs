//! PocketIC boundary proof for the install-time-only EVM threshold-key rule.
//! The production key may be selected on a truly fresh rail, but any derived
//! known-EVM address or even a Disabled/staged chain makes the key immutable.
//! This prevents existing config/proofs from silently referring to addresses
//! under a different threshold root.

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
enum GasStrategy {
    EvmEip1559 {
        max_priority_fee_gwei: u64,
        max_fee_gwei_ceiling: u64,
    },
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct RegisterChainArg {
    chain_id: ChainId,
    display_name: String,
    rpc_endpoints: Vec<String>,
    finality_depth: u32,
    gas_strategy: GasStrategy,
    chain_native_decimals: u8,
    min_quorum_providers: Option<u32>,
}

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
    let pic = PocketIcBuilder::new()
        .with_ii_subnet()
        .with_application_subnet()
        .build();

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
    pic.install_canister(
        cid,
        backend_wasm(),
        encode_args((init,)).expect("encode init"),
        None,
    );
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
        WasmResult::Reply(b) => {
            Decode!(&b, T).unwrap_or_else(|e| panic!("decode {}: {}", method, e))
        }
        WasmResult::Reject(msg) => panic!("{} rejected: {}", method, msg),
    }
}

fn get_reserve_address(pic: &PocketIc, cid: Principal) -> Result<String, ProtocolError> {
    update_dev(
        pic,
        cid,
        "get_chain_reserve_address",
        encode_args((CFX_MAINNET,)).unwrap(),
    )
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
    update_dev(
        pic,
        cid,
        "set_chains_ecdsa_key_name",
        encode_one(name.to_string()).unwrap(),
    )
}

fn get_key(pic: &PocketIc, cid: Principal) -> String {
    let reply = pic
        .query_call(
            cid,
            Principal::anonymous(),
            "get_chains_ecdsa_key_name",
            encode_args(()).unwrap(),
        )
        .expect("get key query");
    match reply {
        WasmResult::Reply(bytes) => Decode!(&bytes, String).expect("decode key"),
        WasmResult::Reject(message) => panic!("get key rejected: {message}"),
    }
}

fn stage_disabled_mainnet(pic: &PocketIc, cid: Principal) {
    let arg = RegisterChainArg {
        chain_id: CFX_MAINNET,
        display_name: "Conflux mainnet staged".into(),
        rpc_endpoints: vec!["https://a.invalid".into(), "https://b.invalid".into()],
        finality_depth: 400,
        gas_strategy: GasStrategy::EvmEip1559 {
            max_priority_fee_gwei: 1,
            max_fee_gwei_ceiling: 200,
        },
        chain_native_decimals: 18,
        min_quorum_providers: Some(2),
    };
    update_dev::<Result<(), ProtocolError>>(
        pic,
        cid,
        "register_chain",
        encode_args((arg,)).unwrap(),
    )
    .expect("register staged chain");
    update_dev::<Result<(), ProtocolError>>(
        pic,
        cid,
        "disable_chain",
        encode_args((CFX_MAINNET,)).unwrap(),
    )
    .expect("disable staged chain");
}

#[test]
fn fresh_key_selection_succeeds_before_any_evm_setup() {
    let (pic, cid) = boot();
    set_key(&pic, cid, "key_1").expect("fresh rail may select production key");
    assert_eq!(get_key(&pic, cid), "key_1");
    get_reserve_address(&pic, cid).expect("derive reserve under selected key");
    get_interest_treasury_address(&pic, cid).expect("derive treasury under selected key");
}

#[test]
fn derived_address_makes_key_immutable_without_a_vault() {
    let (pic, cid) = boot();
    let reserve_before = get_reserve_address(&pic, cid).expect("derive reserve under default key");
    let error =
        set_key(&pic, cid, "key_1").expect_err("cached known-EVM address must block rotation");
    assert!(format!("{error:?}").contains("any EVM rail setup"));
    assert_eq!(get_key(&pic, cid), "test_key_1");
    assert_eq!(
        get_reserve_address(&pic, cid).expect("cached reserve remains available"),
        reserve_before
    );
}

#[test]
fn disabled_staged_chain_without_vaults_rejects_key_change() {
    let (pic, cid) = boot();
    stage_disabled_mainnet(&pic, cid);
    let error = set_key(&pic, cid, "key_1").expect_err("staged chain must block rotation");
    assert!(format!("{error:?}").contains("any EVM rail setup"));
    assert_eq!(get_key(&pic, cid), "test_key_1");
}
