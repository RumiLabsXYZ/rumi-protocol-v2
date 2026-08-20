//! PocketIC integration test for the de-scaffold pass's new XRC-sourced
//! chains price feed (`xrc::fetch_chains_prices`, registered as a 300s timer
//! in `setup_timers`).
//!
//! Two scenarios:
//!
//!   1. `no_chain_configured_price_stays_unset_and_canister_stays_healthy`:
//!      boots with NO chain registered and `xrc_principal` pointed at the
//!      management canister (so any accidental outbound XRC call would
//!      surface as a rejected/failed call rather than silently succeeding).
//!      Advances time across several 300s timer firings and asserts the
//!      canister stays alive and no manual price ever appears for chain
//!      1030/"CFX". This is the PocketIC-level companion to
//!      `xrc::chains_price_feed_tests::empty_on_default_state_zero_xrc_calls_on_prod_today`,
//!      which pins the same "no configured chains => no XRC calls" property
//!      at the pure-function level.
//!
//!   2. `registered_and_configured_chain_gets_price_via_timer`: registers
//!      chain 1030 (Conflux mainnet; native_symbol "CFX" per
//!      `chains::evm::evm_chain_config`) and stages a liquidation config row
//!      (kept `enabled: false` so the test doesn't need a working EVM RPC /
//!      DEX factory mock; the price feed counts a disabled-but-staged row,
//!      see `chains_needing_price_feed`'s doc comment). With a mock XRC
//!      canister serving CFX/USD, advances past the 300s cadence and asserts
//!      `get_manual_collateral_price(1030, "CFX")` picks up the fetched
//!      price with a fresh, non-zero `set_at_ns`.

use candid::{encode_args, encode_one, CandidType, Decode, Deserialize, Principal};
use pocket_ic::{PocketIc, WasmResult};
use std::collections::HashMap;
use std::time::Duration;

// ─── Init types (mirror lib.rs exactly) ──────────────────────────────────────

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

// ─── Chain admin wire types (mirror chains/config.rs and
//     chains/liquidation_config.rs exactly) ─────────────────────────────────

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
struct ChainId(u32);

#[derive(CandidType, Deserialize, Clone, Debug)]
enum GasStrategy {
    EvmEip1559 {
        max_priority_fee_gwei: u64,
        max_fee_gwei_ceiling: u64,
    },
    EvmLegacy {
        gas_price_gwei_ceiling: u64,
    },
    SolanaPriorityFee {
        lamports_per_cu_ceiling: u64,
    },
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

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq)]
struct ManualPriceInfo {
    price_e8: u64,
    set_at_ns: u64,
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

// ─── Mock XRC canister (local copy; each PocketIC test file in this crate
//     carries its own since `tests/*.rs` files are independent binaries) ────

#[derive(CandidType, Deserialize, Debug, Clone, Default)]
struct MockXRC {
    rates: HashMap<String, u64>,
}

impl MockXRC {
    fn set_rate(&mut self, base: &str, quote: &str, rate_e8s: u64) {
        self.rates
            .insert(format!("{}/{}", base.to_uppercase(), quote.to_uppercase()), rate_e8s);
    }
}

fn prepare_mock_xrc_with_cfx_rate(rate_e8s: u64) -> Vec<u8> {
    let mut mock = MockXRC::default();
    mock.set_rate("CFX", "USD", rate_e8s);
    encode_one(mock).expect("encode mock xrc init")
}

fn xrc_wasm() -> Vec<u8> {
    include_bytes!("../../xrc_demo/xrc/xrc.wasm").to_vec()
}

fn backend_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm")
        .to_vec()
}

const CFX_MAINNET: ChainId = ChainId(1030);

fn developer() -> Principal {
    Principal::from_slice(&[3; 29])
}

fn boot_with_xrc(xrc_principal: Principal) -> (PocketIc, Principal) {
    let pic = PocketIc::new();
    let cid = pic.create_canister();
    pic.add_cycles(cid, 100_000_000_000_000);

    let mgmt = Principal::from_text("aaaaa-aa").expect("mgmt");
    let init = ProtocolArg::Init(ProtocolInitArg {
        xrc_principal,
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

fn register_chain(pic: &PocketIc, cid: Principal) {
    let arg = RegisterChainArg {
        chain_id: CFX_MAINNET,
        display_name: "Conflux mainnet".to_string(),
        rpc_endpoints: vec!["https://evm.confluxrpc.com".to_string()],
        finality_depth: 400,
        gas_strategy: GasStrategy::EvmEip1559 {
            max_priority_fee_gwei: 1,
            max_fee_gwei_ceiling: 100,
        },
        chain_native_decimals: 18,
        min_quorum_providers: Some(2),
    };
    let args = encode_args((arg,)).expect("encode register_chain arg");
    let reply = pic
        .update_call(cid, developer(), "register_chain", args)
        .expect("register_chain call");
    match reply {
        WasmResult::Reply(b) => {
            Decode!(&b, Result<(), ProtocolError>)
                .expect("decode register_chain result")
                .expect("register_chain must succeed");
        }
        WasmResult::Reject(msg) => panic!("register_chain rejected: {msg}"),
    }
}

/// Stages a liquidation config row with `enabled: false` (no EVM RPC / DEX
/// factory validation needed) purely so the price-feed gate sees a config
/// row present, per `chains_needing_price_feed`'s "staged but disabled
/// still counts" rule.
fn stage_disabled_liquidation_config(pic: &PocketIc, cid: Principal) {
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
    let args = encode_args((CFX_MAINNET, cfg)).expect("encode set_chain_liquidation_config arg");
    let reply = pic
        .update_call(cid, developer(), "set_chain_liquidation_config", args)
        .expect("set_chain_liquidation_config call");
    match reply {
        WasmResult::Reply(b) => {
            Decode!(&b, Result<(), ProtocolError>)
                .expect("decode set_chain_liquidation_config result")
                .expect("set_chain_liquidation_config must succeed");
        }
        WasmResult::Reject(msg) => panic!("set_chain_liquidation_config rejected: {msg}"),
    }
}

fn get_manual_price(pic: &PocketIc, cid: Principal) -> Option<ManualPriceInfo> {
    let args = encode_args((CFX_MAINNET, "CFX".to_string())).expect("encode");
    let reply = pic
        .query_call(cid, Principal::anonymous(), "get_manual_collateral_price", args)
        .expect("query");
    match reply {
        WasmResult::Reply(b) => Decode!(&b, Option<ManualPriceInfo>).expect("decode"),
        WasmResult::Reject(msg) => panic!("get_manual_collateral_price rejected: {msg}"),
    }
}

fn advance_past_several_timer_firings(pic: &PocketIc) {
    for _ in 0..4 {
        pic.advance_time(Duration::from_secs(305));
        for _ in 0..5 {
            pic.tick();
        }
    }
}

/// Scenario 1: no chain configured at all. `xrc_principal` points at the
/// management canister, so IF `fetch_chains_prices` ever wrongly attempted a
/// call it would hit an unknown-method reject on "aaaaa-aa" (caught and
/// logged by the best-effort error handling, not a canister trap); the
/// stronger, structural guarantee is `chains_needing_price_feed` returning
/// empty before any `await`, proven directly at the unit level. This test's
/// job is the PocketIC-level companion: prove the timer firing repeatedly
/// with nothing configured leaves the canister healthy and never produces a
/// price out of nowhere.
#[test]
fn no_chain_configured_price_stays_unset_and_canister_stays_healthy() {
    let mgmt = Principal::from_text("aaaaa-aa").expect("mgmt");
    let (pic, cid) = boot_with_xrc(mgmt);

    assert_eq!(get_manual_price(&pic, cid), None, "no price before any timer firing");

    advance_past_several_timer_firings(&pic);

    assert_eq!(
        get_manual_price(&pic, cid),
        None,
        "chains price timer must not produce a price with no chain registered/configured"
    );
}

/// Scenario 2: the happy path. A registered + (disabled-but-staged)
/// configured chain gets a real price written by the 300s timer.
#[test]
fn registered_and_configured_chain_gets_price_via_timer() {
    // The mock XRC canister and the backend must live on the SAME PocketIc
    // instance (inter-canister calls only work within one PocketIc world),
    // so this scenario cannot reuse `boot_with_xrc` (which creates its own).
    let pic = PocketIc::new();
    let xrc_id = pic.create_canister();
    pic.add_cycles(xrc_id, 1_000_000_000_000);
    pic.install_canister(xrc_id, xrc_wasm(), prepare_mock_xrc_with_cfx_rate(1_234_000_000), None);

    let cid = pic.create_canister();
    pic.add_cycles(cid, 100_000_000_000_000);
    let init = ProtocolArg::Init(ProtocolInitArg {
        xrc_principal: xrc_id,
        icusd_ledger_principal: Principal::from_text("aaaaa-aa").unwrap(),
        icp_ledger_principal: Principal::from_text("aaaaa-aa").unwrap(),
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

    register_chain(&pic, cid);
    stage_disabled_liquidation_config(&pic, cid);

    assert_eq!(
        get_manual_price(&pic, cid),
        None,
        "no price yet: the 300s timer has not fired since registration"
    );

    advance_past_several_timer_firings(&pic);

    let info = get_manual_price(&pic, cid).expect(
        "a registered + config-staged chain must get a price from the 300s XRC timer",
    );
    assert_eq!(info.price_e8, 1_234_000_000, "price must match the mock XRC CFX/USD rate");
    assert!(info.set_at_ns > 0, "set timestamp must be stamped");
}
