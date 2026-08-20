//! PocketIC integration test for the F2 XRC-managed pricing invariant
//! (`xrc::pair_is_xrc_managed`, read by `set_manual_collateral_price`,
//! main.rs).
//!
//! Once a chain is registered AND carries a `chain_liquidation_configs` row,
//! the XRC price timer is the SOLE writer of its native-symbol price FOR THE
//! PRICE-PUSHER: a pusher write there is rejected, closing the last-writer-
//! wins race between an unattended automated pusher and the timer. The
//! developer principal is exempt (a single trusted operator, who also stages
//! and removes the config rows in the first place, cannot race itself; the
//! exemption also keeps existing manual price-drop test patterns in
//! conflux_liquidation_swap_pic / conflux_liquidation_detection_pic working
//! without needing an XRC mock).
//!
//! Scenarios:
//!   1. `pusher_rejected_developer_accepted_for_xrc_managed_pair`
//!   2. `pusher_accepted_again_after_disable_chain` (the documented stop
//!      procedure: disable_chain makes the chain no longer XRC-managed)
//!   3. `pusher_unaffected_for_a_pair_with_no_liquidation_config_row`
//!      (sanity: the gate is scoped to XRC-managed pairs only)

use candid::{encode_args, encode_one, CandidType, Decode, Deserialize, Principal};
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
enum GasStrategy {
    EvmEip1559 { max_priority_fee_gwei: u64, max_fee_gwei_ceiling: u64 },
    EvmLegacy { gas_price_gwei_ceiling: u64 },
    SolanaPriorityFee { lamports_per_cu_ceiling: u64 },
    NotApplicable,
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

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
enum DexKind {
    UniswapV2,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct ChainLiquidationConfigV1 {
    dex: DexKind,
    router: String,
    factory: String,
    pair: String,
    collateral_token: String,
    settle_stable_token: String,
    slippage_cap_bps: u16,
    restore_target_cr_e4: u64,
    enabled: bool,
    max_swap_value_e8s: u128,
    max_price_age_ns: u64,
    max_dex_oracle_divergence_bps: u32,
    fee_bps: u16,
    settle_stable_decimals: u8,
    deadline_secs: u64,
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
    Principal::from_slice(&[21; 29])
}
fn pusher() -> Principal {
    Principal::from_slice(&[22; 29])
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

fn update_as<T>(pic: &PocketIc, cid: Principal, sender: Principal, method: &str, arg: Vec<u8>) -> T
where
    T: CandidType + for<'a> Deserialize<'a>,
{
    let reply = pic
        .update_call(cid, sender, method, arg)
        .unwrap_or_else(|e| panic!("{} call: {:?}", method, e));
    match reply {
        WasmResult::Reply(b) => Decode!(&b, T).unwrap_or_else(|e| panic!("decode {}: {}", method, e)),
        WasmResult::Reject(msg) => panic!("{} rejected: {}", method, msg),
    }
}

fn register_chain(pic: &PocketIc, cid: Principal) {
    let arg = RegisterChainArg {
        chain_id: CFX_MAINNET,
        display_name: "Conflux mainnet".to_string(),
        rpc_endpoints: vec!["https://evm.confluxrpc.com".to_string()],
        finality_depth: 400,
        gas_strategy: GasStrategy::EvmEip1559 { max_priority_fee_gwei: 1, max_fee_gwei_ceiling: 100 },
        chain_native_decimals: 18,
        min_quorum_providers: Some(2),
    };
    let r: Result<(), ProtocolError> =
        update_as(pic, cid, developer(), "register_chain", encode_args((arg,)).unwrap());
    r.expect("register_chain must succeed");
}

fn stage_liquidation_config(pic: &PocketIc, cid: Principal) {
    let cfg = ChainLiquidationConfigV1 {
        dex: DexKind::UniswapV2,
        router: String::new(),
        factory: String::new(),
        pair: String::new(),
        collateral_token: String::new(),
        settle_stable_token: String::new(),
        slippage_cap_bps: 250,
        restore_target_cr_e4: 15_500,
        enabled: false,
        max_swap_value_e8s: 0,
        max_price_age_ns: 0,
        max_dex_oracle_divergence_bps: 0,
        fee_bps: 0,
        settle_stable_decimals: 0,
        deadline_secs: 0,
    };
    let r: Result<(), ProtocolError> = update_as(
        pic,
        cid,
        developer(),
        "set_chain_liquidation_config",
        encode_args((CFX_MAINNET, cfg)).unwrap(),
    );
    r.expect("set_chain_liquidation_config must succeed");
}

fn grant_pusher(pic: &PocketIc, cid: Principal) {
    let allowed = vec![(CFX_MAINNET.0, "CFX".to_string())];
    let r: Result<(), ProtocolError> = update_as(
        pic,
        cid,
        developer(),
        "set_price_pusher_principal",
        encode_args((Some(pusher()), allowed)).unwrap(),
    );
    r.expect("set_price_pusher_principal must succeed");
}

fn set_price(pic: &PocketIc, cid: Principal, sender: Principal, price_e8: u64) -> Result<(), ProtocolError> {
    update_as(
        pic,
        cid,
        sender,
        "set_manual_collateral_price",
        encode_args((CFX_MAINNET, "CFX".to_string(), price_e8)).unwrap(),
    )
}

fn disable_chain(pic: &PocketIc, cid: Principal) {
    let r: Result<(), ProtocolError> =
        update_as(pic, cid, developer(), "disable_chain", encode_one(CFX_MAINNET).unwrap());
    r.expect("disable_chain must succeed");
}

#[test]
fn pusher_rejected_developer_accepted_for_xrc_managed_pair() {
    let (pic, cid) = boot();
    register_chain(&pic, cid);
    grant_pusher(&pic, cid);
    stage_liquidation_config(&pic, cid);

    let err = set_price(&pic, cid, pusher(), 5_000_000).expect_err("pusher must be rejected");
    match err {
        ProtocolError::ChainAdmin(msg) => {
            assert!(msg.contains("XRC-managed"), "msg={msg}");
        }
        other => panic!("expected ChainAdmin, got {other:?}"),
    }

    set_price(&pic, cid, developer(), 5_000_000).expect("developer override must still succeed");
}

#[test]
fn pusher_accepted_again_after_disable_chain() {
    let (pic, cid) = boot();
    register_chain(&pic, cid);
    grant_pusher(&pic, cid);
    stage_liquidation_config(&pic, cid);

    set_price(&pic, cid, pusher(), 1).expect_err("pusher rejected while XRC-managed");

    disable_chain(&pic, cid);

    set_price(&pic, cid, pusher(), 5_000_000)
        .expect("pusher must be able to set price again after disable_chain (documented stop procedure)");
}

#[test]
fn pusher_unaffected_for_a_pair_with_no_liquidation_config_row() {
    let (pic, cid) = boot();
    register_chain(&pic, cid);
    grant_pusher(&pic, cid);
    // No set_chain_liquidation_config call: the pair is not XRC-managed.

    set_price(&pic, cid, pusher(), 5_000_000)
        .expect("pusher retains normal access when the pair is not XRC-managed");
}
