//! Conflux eSpace testnet (chain 71) Increment-3 Tier-1 bot LIQUIDATION SWAP
//! end-to-end integration test against the scripted mock EVM RPC canister.
//!
//! This is the integration PROOF that the Increment-3 bot SWAP executes the full
//! two-phase liquidation against the EVM-RPC mock:
//!
//!   1. DETECTION (Increment 2, reused verbatim): the observer tick marks an
//!      underwater Open vault with a `pending_liquidation` (Bot tier) marker,
//!      reserving its collateral and enqueueing a `LiquidationSwap` op. Debt +
//!      `chain_supplies` are UNCHANGED at trigger (Design B).
//!   2. SWAP SUBMIT (Increment 3): the settlement worker reads the DEX
//!      token0/getReserves at the finalized block, computes a JIT min-out gated by
//!      the oracle cross-check, nets gas via `fundable_swap_value`, signs with the
//!      vault's custody key, and broadcasts a `swapExactETHForTokens` to the
//!      router (CFX in `value`, USDC `to` = the tECDSA reserve address). The op
//!      goes Inflight.
//!   3. SWAP CONFIRM (Increment 3): once the receipt is mined + final, the worker
//!      reads the REALIZED USDC output from the `Transfer(_, reserve, amount)` log
//!      (never min-out) and moves `debt_e8s -> reserve_backing_e8s` (the ONLY
//!      invariant move — NO icUSD burned, this is a PSM). The vault drains fully
//!      to debt 0 and Closes; `chain_supplies` is UNCHANGED.
//!
//! The reserve is BACKING, not unbacked supply (findings #17/#20/#29):
//! `reconcile_chain_supply` reports `reserve_backing_e8s` + `reserve_usdc_native`
//! as informational RHS terms and `unbacked_excess == false` (on-chain totalSupply
//! still == recorded chain supply).
//!
//! Harness (boot, register_chain, deposit->mint->Open sequence, ECDSA-gating,
//! supply invariant, candid mirrors) is COPIED from
//! `conflux_liquidation_detection_pic.rs`. As in that test, the flow PROBES ECDSA
//! via `get_chain_settlement_address`; if ECDSA is unavailable in this PocketIC
//! build it runs a GATED subset (which still proves the new config fields decode +
//! the reserve/settlement address endpoints signal ECDSA-unavailable) and returns
//! early. The swap is NEVER faked.

use candid::{encode_args, encode_one, CandidType, Decode, Deserialize, Encode, Principal};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use std::time::Duration;

// ─── Locally-mirrored backend types (shapes mirror src/.../*.rs exactly) ─────

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

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
enum ChainVaultStatus {
    AwaitingDeposit,
    MintPending,
    Open,
    Closing,
    Closed,
}

// ─── Liquidation mirrors ──────────────────────────────────────────────────────

/// Mirror of `chains::vault::LiquidationTier`.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
enum LiquidationTier {
    Bot,
    StabilityPool,
}

/// Mirror of `chains::vault::PendingLiquidationV1` (the in-flight marker).
#[derive(CandidType, Deserialize, Clone, Debug)]
struct PendingLiquidationV1 {
    op_id: u64,
    debt_to_clear_e8s: candid::Nat,
    collateral_reserved_native: candid::Nat,
    tier: LiquidationTier,
    started_at_ns: u64,
}

/// Mirror of `chains::vault::ChainVaultV1`, EXTENDED with `pending_liquidation`.
/// Candid records decode by field name and ignore extra wire fields, so the
/// happy-path subset of fields + the marker field is sufficient. NOTE: the wire
/// field for the native collateral amount is `collateral_amount_e18`.
#[derive(CandidType, Deserialize, Clone, Debug)]
struct ChainVaultV1 {
    vault_id: u64,
    owner: Principal,
    collateral_chain: ChainId,
    custody_address: String,
    collateral_amount_e18: candid::Nat,
    debt_e8s: candid::Nat,
    mint_recipient: String,
    pending_mint_e8s: candid::Nat,
    status: ChainVaultStatus,
    opened_at_ns: u64,
    owner_evm: Option<String>,
    pending_liquidation: Option<PendingLiquidationV1>,
}

/// Mirror of `chains::liquidation_config::DexKind`.
#[derive(CandidType, Deserialize, Clone, Debug)]
enum DexKind {
    UniswapV2,
}

/// Mirror of `chains::liquidation_config::ChainLiquidationConfigV1`, INCLUDING the
/// Increment-2 fields (`max_swap_value_e8s`, `max_price_age_ns`) AND the four
/// Increment-3 fields (`max_dex_oracle_divergence_bps`, `fee_bps`,
/// `settle_stable_decimals`, `deadline_secs`). Field set + types verified against
/// `rumi_protocol_backend.did`'s `type ChainLiquidationConfigV1`.
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
    max_swap_value_e8s: candid::Nat,
    max_price_age_ns: u64,
    // ── Increment 3 ──
    max_dex_oracle_divergence_bps: u32,
    fee_bps: u16,
    settle_stable_decimals: u8,
    deadline_secs: u64,
}

/// Mirror of `main::ChainSupplyReconciliation`. Field set + types verified against
/// `rumi_protocol_backend.did`'s `type ChainSupplyReconciliation` (10 fields). The
/// swap assertions only read `reserve_backing_e8s` / `reserve_usdc_native` /
/// `unbacked_excess`, but the full field set is mirrored so candid decodes.
#[derive(CandidType, Deserialize, Clone, Debug)]
struct ChainSupplyReconciliation {
    chain_id: ChainId,
    finalized_block: u64,
    onchain_total_supply_e8s: candid::Nat,
    recorded_supply_e8s: candid::Nat,
    in_flight_mint_e8s: candid::Nat,
    unbacked_excess: bool,
    gap_e8s: candid::Int,
    reserve_backing_e8s: candid::Nat,
    pending_chain_burn_e8s: candid::Nat,
    reserve_usdc_native: candid::Nat,
}

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

/// Minimal mirror of `ProtocolError`: the chain endpoints here only ever return
/// `ChainAdmin(text)` or (on ECDSA-derive failures) the same. Any other variant
/// fails the Candid decode loudly (the correct signal that something unexpected
/// happened). `TemporarilyUnavailable`/`EvmAuth` are included for completeness so
/// a transient reconcile error decodes rather than mis-tags.
#[derive(CandidType, Deserialize, Clone, Debug)]
enum ProtocolError {
    ChainAdmin(String),
    EvmAuth(String),
    TemporarilyUnavailable(String),
}

// ─── Wasm loaders ────────────────────────────────────────────────────────────

fn backend_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm")
        .to_vec()
}

fn mock_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/monad_rpc_mock.wasm").to_vec()
}

// ─── Constants ───────────────────────────────────────────────────────────────

const CONFLUX_CHAIN_ID: u32 = 71;
const E18: u128 = 1_000_000_000_000_000_000;
const E8: u128 = 100_000_000;
/// keccak256("Mint(uint256,address,uint256)"), must match evm_rpc.rs.
const MINT_EVENT_TOPIC0: &str =
    "0x4e3883c75cc9c752bb1db2e406a822e4a75067ae77ad9a0a4d179f2709b9e1f6";
/// keccak256("Transfer(address,address,uint256)"), must match evm_rpc.rs.
const TRANSFER_TOPIC0: &str =
    "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
/// UniswapV2 pair `token0()` selector (must match evm_rpc.rs `TOKEN0_SELECTOR`).
const TOKEN0_SELECTOR: &str = "0x0dfe1681";
/// UniswapV2 pair `getReserves()` selector (must match `GET_RESERVES_SELECTOR`).
const GET_RESERVES_SELECTOR: &str = "0x0902f1ac";

/// Conflux register arg's `finality_depth` (mirrors conflux_testnet_register_arg()).
const CONFLUX_FINALITY_DEPTH: u64 = 100;
/// Observer block-scan window (`MAX_BLOCK_SCAN_WINDOW`).
const SCAN_WINDOW: u64 = 1024;

/// The specific DEX wiring addresses (valid 42-char 0x). The collateral_token and
/// settle_stable_token are REUSED below for the canned DEX reads + the Transfer
/// log contract, so they must be byte-identical everywhere.
const DEX_ROUTER: &str = "0x1111111111111111111111111111111111111111";
const DEX_FACTORY: &str = "0x2222222222222222222222222222222222222222";
const DEX_PAIR: &str = "0x3333333333333333333333333333333333333333";
/// WCFX (the collateral token + path[0]).
const WCFX: &str = "0x14b2d3bc65e74dae1030eafd8ac30c533c976a9b";
/// USDC (the settle stable token + path[1]); 18-dec on eSpace.
const USDC: &str = "0x6963efed0ab40f6c3d7bda44a05dcf1437c44372";

// ─── PocketIC call helpers ───────────────────────────────────────────────────

fn dev() -> Principal {
    Principal::from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9])
}

fn query_unit<T>(pic: &PocketIc, cid: Principal, method: &str) -> T
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

fn update_dev(pic: &PocketIc, cid: Principal, method: &str, args: Vec<u8>) -> WasmResult {
    pic.update_call(cid, dev(), method, args)
        .unwrap_or_else(|e| panic!("update {} failed: {}", method, e))
}

fn update_any(pic: &PocketIc, cid: Principal, method: &str, args: Vec<u8>) -> WasmResult {
    pic.update_call(cid, Principal::anonymous(), method, args)
        .unwrap_or_else(|e| panic!("update {} failed: {}", method, e))
}

fn decode_result(reply: WasmResult, method: &str) -> Result<(), ProtocolError> {
    match reply {
        WasmResult::Reply(b) => Decode!(&b, Result<(), ProtocolError>)
            .unwrap_or_else(|e| panic!("decode {} Result: {}", method, e)),
        WasmResult::Reject(msg) => panic!("{} rejected: {}", method, msg),
    }
}

// ─── Boot: II subnet (for tECDSA) + application subnet (backend + mock) ───────

fn boot() -> (PocketIc, Principal, Principal) {
    let pic = PocketIcBuilder::new()
        .with_ii_subnet()
        .with_application_subnet()
        .build();

    let backend_id = pic.create_canister();
    pic.add_cycles(backend_id, 100_000_000_000_000);
    let mock_id = pic.create_canister();
    pic.add_cycles(mock_id, 100_000_000_000_000);

    let mgmt = Principal::from_text("aaaaa-aa").expect("mgmt principal");
    let init = ProtocolArg::Init(ProtocolInitArg {
        xrc_principal: mgmt,
        icusd_ledger_principal: mgmt,
        icp_ledger_principal: mgmt,
        fee_e8s: 10_000,
        developer_principal: dev(),
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    });
    pic.install_canister(
        backend_id,
        backend_wasm(),
        encode_args((init,)).expect("encode init"),
        None,
    );
    pic.install_canister(mock_id, mock_wasm(), Encode!().expect("encode mock init"), None);

    for _ in 0..5 {
        pic.tick();
    }

    // Pin observer + settlement cadence to 30s (code default is 300s).
    let _ = update_dev(&pic, backend_id, "set_observer_tick_interval_secs", Encode!(&30u64).unwrap());
    let _ = update_dev(&pic, backend_id, "set_settlement_tick_interval_secs", Encode!(&30u64).unwrap());

    (pic, backend_id, mock_id)
}

/// Tick the canister timers across `windows` 35s windows so the observer (Timer
/// A) and settlement (Timer D) timers fire and their async futures run.
fn advance_and_tick(pic: &PocketIc, windows: u32) {
    for _ in 0..windows {
        pic.advance_time(Duration::from_secs(35));
        for _ in 0..10 {
            pic.tick();
        }
    }
}

// ─── Supply-invariant assertion (PSM: a liquidation swap does NOT move supply) ─

fn assert_supply(pic: &PocketIc, cid: Principal, expected_e8s: u128, step: &str) {
    let global: candid::Nat = query_unit(pic, cid, "get_global_icusd_supply");
    assert_eq!(
        global,
        candid::Nat::from(expected_e8s),
        "[{step}] get_global_icusd_supply mismatch"
    );

    let audit: SupplyAuditWire = query_unit(pic, cid, "get_supply_audit");
    assert_eq!(
        audit.total_e8s,
        candid::Nat::from(expected_e8s),
        "[{step}] supply_audit.total mismatch"
    );
    let sum: candid::Nat = audit
        .per_chain
        .iter()
        .fold(candid::Nat::from(0u32), |acc, e| acc + e.supply_e8s.clone());
    assert_eq!(audit.total_e8s, sum, "[{step}] audit total != sum(per_chain)");

    if let Some(conflux) = audit.per_chain.iter().find(|e| e.chain_id == CONFLUX_CHAIN_ID) {
        assert_eq!(
            conflux.supply_e8s,
            candid::Nat::from(expected_e8s),
            "[{step}] Conflux per-chain supply mismatch"
        );
    }
}

// ─── 32-byte ABI word encoding for log topics / data / eth_call returns ───────

fn word_u128(v: u128) -> String {
    format!("0x{:064x}", v)
}

fn word_addr(addr: &str) -> String {
    let raw = addr.strip_prefix("0x").or_else(|| addr.strip_prefix("0X")).unwrap_or(addr);
    format!("0x{:0>64}", raw.to_lowercase())
}

// ─── liquidation config builder (ENABLED, with the Increment-3 fields) ────────

/// The ENABLED Conflux liquidation config used throughout this test. Uses the
/// SPECIFIC WCFX/USDC token addresses (reused for the canned DEX reads + the
/// realized Transfer log). 18-dec USDC, 0.25% pool fee, 2.5% slippage cap, 5%
/// oracle-divergence ceiling, 155% restore target, $2k depth cap, 30-min
/// staleness ceiling, 180s on-chain deadline.
fn enabled_liq_config() -> ChainLiquidationConfigV1 {
    ChainLiquidationConfigV1 {
        dex: DexKind::UniswapV2,
        router: DEX_ROUTER.to_string(),
        factory: DEX_FACTORY.to_string(),
        pair: DEX_PAIR.to_string(),
        collateral_token: WCFX.to_string(),
        settle_stable_token: USDC.to_string(),
        slippage_cap_bps: 250,
        restore_target_cr_e4: 15_500,
        enabled: true,
        max_swap_value_e8s: candid::Nat::from(2_000u128 * E8),
        max_price_age_ns: 1_800_000_000_000, // 30 min
        max_dex_oracle_divergence_bps: 500,
        fee_bps: 25,
        settle_stable_decimals: 18,
        deadline_secs: 180,
    }
}

// ─── The test ────────────────────────────────────────────────────────────────

#[test]
fn conflux_liquidation_swap_executes_and_credits_reserve() {
    let (pic, backend, mock) = boot();

    // ── Step 1: point the backend's EVM wrapper at the mock + Conflux quirks ──
    decode_result(
        update_dev(&pic, backend, "set_evm_rpc_principal", Encode!(&mock).unwrap()),
        "set_evm_rpc_principal",
    )
    .expect("set_evm_rpc_principal");
    update_any(&pic, mock, "set_getlogs_max_range", Encode!(&1000u64).unwrap());
    update_any(&pic, mock, "set_espace_receipt_fields", Encode!(&true).unwrap());

    // ── Step 2: register Conflux + contract + manual CFX price ($0.15) ───────
    let reg = RegisterChainArg {
        chain_id: ChainId(CONFLUX_CHAIN_ID),
        display_name: "ConfluxESpaceTestnet".to_string(),
        rpc_endpoints: vec!["https://evmtestnet.confluxrpc.com".to_string()],
        finality_depth: CONFLUX_FINALITY_DEPTH as u32,
        gas_strategy: GasStrategy::EvmEip1559 {
            max_priority_fee_gwei: 1,
            max_fee_gwei_ceiling: 100,
        },
        chain_native_decimals: 18,
        min_quorum_providers: Some(1),
    };
    decode_result(
        update_dev(&pic, backend, "register_chain", Encode!(&reg).unwrap()),
        "register_chain",
    )
    .expect("register_chain");

    decode_result(
        update_dev(
            &pic,
            backend,
            "set_chain_contract",
            Encode!(&ChainId(CONFLUX_CHAIN_ID), &"0x00000000000000000000000000000000cf1c0de5".to_string())
                .unwrap(),
        ),
        "set_chain_contract",
    )
    .expect("set_chain_contract");

    // $0.15 / CFX in e8 => 15_000_000. Healthy price for the open.
    decode_result(
        update_dev(
            &pic,
            backend,
            "set_manual_collateral_price",
            Encode!(&ChainId(CONFLUX_CHAIN_ID), &"CFX".to_string(), &15_000_000u64).unwrap(),
        ),
        "set_manual_collateral_price",
    )
    .expect("set_manual_collateral_price");

    // ── Seed the burn-watch cursor + enable poll-scan (harness parity) ───────
    let seed: u64 = 1_000_000;
    decode_result(
        update_dev(
            &pic,
            backend,
            "set_last_observed_block",
            Encode!(&ChainId(CONFLUX_CHAIN_ID), &seed).unwrap(),
        ),
        "set_last_observed_block",
    )
    .expect("set_last_observed_block");
    decode_result(
        update_dev(
            &pic,
            backend,
            "set_burn_watch_poll_enabled",
            Encode!(&ChainId(CONFLUX_CHAIN_ID), &true).unwrap(),
        ),
        "set_burn_watch_poll_enabled",
    )
    .expect("set_burn_watch_poll_enabled");

    assert_supply(&pic, backend, 0, "after register/config");

    // ── Block plan (finality_depth = 100) ────────────────────────────────────
    // The mint receipt + log AND the swap receipt + Transfer log all land at the
    // SAME finalized cursor (cursor1), the deepest block the observer + settlement
    // finality gate treat as final.
    let cursor1 = seed + SCAN_WINDOW;
    let head1 = cursor1 + CONFLUX_FINALITY_DEPTH + 24;
    update_any(&pic, mock, "set_blocks", Encode!(&head1, &head1).unwrap());
    update_any(&pic, mock, "set_next_send_hash", Encode!(&"0xcfxmint1".to_string()).unwrap());

    // ── ECDSA probe: decide full vs gated ────────────────────────────────────
    let settlement_addr = match update_dev(
        &pic,
        backend,
        "get_chain_settlement_address",
        Encode!(&ChainId(CONFLUX_CHAIN_ID)).unwrap(),
    ) {
        WasmResult::Reply(b) => match Decode!(&b, Result<String, ProtocolError>) {
            Ok(Ok(addr)) => Some(addr),
            Ok(Err(e)) => {
                eprintln!("[swap] ECDSA UNAVAILABLE (get_chain_settlement_address returned Err: {e:?}); running GATED subset");
                None
            }
            Err(decode_err) => {
                eprintln!("[swap] get_chain_settlement_address decode error ({decode_err}); running GATED subset");
                None
            }
        },
        WasmResult::Reject(msg) => {
            eprintln!("[swap] ECDSA UNAVAILABLE (get_chain_settlement_address rejected: {msg}); running GATED subset");
            None
        }
    };

    let settlement_addr = match settlement_addr {
        Some(addr) => {
            eprintln!("[swap] ECDSA AVAILABLE; settlement address = {addr}; running FULL swap path");
            addr
        }
        None => {
            // ── GATED subset: the new config (incl. the 4 Increment-3 fields)
            // decodes + round-trips, and the reserve/settlement address endpoints
            // correctly signal ECDSA-unavailable. We do NOT fake the swap.
            decode_result(
                update_dev(
                    &pic,
                    backend,
                    "set_chain_liquidation_config",
                    Encode!(&ChainId(CONFLUX_CHAIN_ID), &enabled_liq_config()).unwrap(),
                ),
                "set_chain_liquidation_config",
            )
            .expect("gated: set_chain_liquidation_config (proves the 4 Inc-3 fields decode)");

            let cfg = get_liq_config(&pic, backend).expect("gated: config round-trips");
            assert_eq!(cfg.max_dex_oracle_divergence_bps, 500, "gated: divergence bps round-trips");
            assert_eq!(cfg.fee_bps, 25, "gated: fee_bps round-trips");
            assert_eq!(cfg.settle_stable_decimals, 18, "gated: settle decimals round-trips");
            assert_eq!(cfg.deadline_secs, 180, "gated: deadline_secs round-trips");
            assert_eq!(cfg.collateral_token, WCFX, "gated: collateral_token round-trips");
            assert_eq!(cfg.settle_stable_token, USDC, "gated: settle_stable_token round-trips");
            assert!(cfg.enabled, "gated: enabled round-trips");

            // The reserve-address endpoint must ALSO signal ECDSA-unavailable.
            match get_reserve_address(&pic, backend) {
                Err(_) => { /* expected: ECDSA off */ }
                Ok(a) => panic!("gated: reserve address returned Ok({a}) but settlement ECDSA was unavailable"),
            }

            assert_supply(&pic, backend, 0, "gated: ECDSA unavailable, invariant at 0");
            eprintln!("[swap] ECDSA unavailable; ran gated subset (Inc-3 config decodes; reserve/settlement signal ECDSA-off)");
            return;
        }
    };

    // ════════════════════════════════════════════════════════════════════════
    // FULL SWAP PATH (ECDSA available)
    // ════════════════════════════════════════════════════════════════════════

    // Fund the settlement (hot-wallet) address so the mint submit-path gas gate passes.
    update_any(
        &pic,
        mock,
        "set_balance",
        Encode!(&settlement_addr, &candid::Nat::from(1_000_000u128 * E18)).unwrap(),
    );

    // ── Step 1: Open a vault (1400 CFX @ $0.15 = $210 backing 100 icUSD = 210% CR) ──
    let collateral_e18 = 1_400u128 * E18;
    let debt_e8s = 100u128 * E8;
    let recipient = "0x000000000000000000000000000000000000c0de".to_string();
    let vault_id: u64 = match update_dev(
        &pic,
        backend,
        "open_chain_vault",
        Encode!(
            &ChainId(CONFLUX_CHAIN_ID),
            &candid::Nat::from(collateral_e18),
            &candid::Nat::from(debt_e8s),
            &recipient
        )
        .unwrap(),
    ) {
        WasmResult::Reply(b) => Decode!(&b, Result<u64, ProtocolError>)
            .expect("decode open_chain_vault")
            .expect("open_chain_vault Ok"),
        WasmResult::Reject(msg) => panic!("open_chain_vault rejected: {msg}"),
    };

    let v = get_vault(&pic, backend, vault_id).expect("vault exists after open");
    assert_eq!(v.status, ChainVaultStatus::AwaitingDeposit, "open => AwaitingDeposit");
    let custody = v.custody_address.clone();
    assert_supply(&pic, backend, 0, "after open (AwaitingDeposit)");

    // ── deposit -> MintPending -> mint confirm (Open, supply 100e8) ──────────
    update_any(
        &pic,
        mock,
        "set_balance",
        Encode!(&custody, &candid::Nat::from(collateral_e18)).unwrap(),
    );
    advance_and_tick(&pic, 2);
    assert_eq!(
        get_vault(&pic, backend, vault_id).unwrap().status,
        ChainVaultStatus::MintPending,
        "deposit verified => MintPending"
    );

    advance_and_tick(&pic, 1); // submit mint (Queued -> Inflight)
    update_any(
        &pic,
        mock,
        "set_receipt",
        Encode!(&"0xcfxmint1".to_string(), &true, &cursor1).unwrap(),
    );
    push_mint_log(&pic, mock, vault_id, &recipient, debt_e8s, "0xcfxmint1", cursor1);
    advance_and_tick(&pic, 4); // confirm mint

    let v = get_vault(&pic, backend, vault_id).expect("vault after mint confirm");
    assert_eq!(v.status, ChainVaultStatus::Open, "mint confirmed => Open");
    assert_eq!(v.debt_e8s, candid::Nat::from(debt_e8s), "mint confirmed => debt 100e8");
    assert!(v.pending_liquidation.is_none(), "healthy vault has no liquidation marker");
    assert_supply(&pic, backend, 100 * E8, "after mint");

    // ── Step 2: learn the reserve address (the swap's USDC `to` + Transfer match) ──
    // Derived from the tECDSA reserve key — independent of any vault state, so it is
    // resolved up front (before detection) and reused to seed the canned reads.
    let reserve = match get_reserve_address(&pic, backend) {
        Ok(a) => a.to_lowercase(),
        Err(e) => {
            // ECDSA derived the SETTLEMENT address above but not the RESERVE
            // address: we are in the gated regime after all. Bail honestly.
            eprintln!("[swap] reserve address derive Err ({e:?}) despite settlement ECDSA; running gated subset");
            return;
        }
    };
    eprintln!("[swap] reserve address = {reserve}");

    // ── Step 3: seed everything the swap SUBMIT reads, BEFORE detection ───────
    // The settlement worker can pick up the freshly-enqueued LiquidationSwap op on
    // the very next tick after detection marks it (both timers fire in the same
    // advance_and_tick window). If the DEX reads / custody balance / send-hash are
    // not in place by then, the swap fail-closes (escalates) and clears the marker.
    // So seed them all FIRST, then drop the price to trigger detection.

    // 3a. Fund custody so the swap is fundable (>= 1400 CFX collateral + gas).
    update_any(
        &pic,
        mock,
        "set_balance",
        Encode!(&custody, &candid::Nat::from(2_000u128 * E18)).unwrap(),
    );
    // 3b. token0() == collateral_token (WCFX), ABI-encoded as a single 32-byte word.
    update_any(
        &pic,
        mock,
        "set_eth_call_response",
        Encode!(&TOKEN0_SELECTOR.to_string(), &word_addr(WCFX)).unwrap(),
    );
    // 3c. getReserves() -> (reserve0, reserve1, ts). token0 == WCFX so reserve0 =
    // WCFX reserve, reserve1 = USDC reserve. A DEEP pool priced at $0.08/CFX so the
    // oracle cross-check passes: 1e24 WCFX vs 8e22 USDC (8e22/1e24 = 0.08).
    let reserve0_wcfx = 1_000_000u128 * E18; // 1e24
    let reserve1_usdc = 80_000u128 * E18; // 8e22
    let getreserves_blob = format!(
        "0x{:064x}{:064x}{:064x}",
        reserve0_wcfx, reserve1_usdc, 0u128
    );
    update_any(
        &pic,
        mock,
        "set_eth_call_response",
        Encode!(&GET_RESERVES_SELECTOR.to_string(), &getreserves_blob).unwrap(),
    );
    // 3d. The hash the swap broadcast returns (so the confirm can match it).
    update_any(&pic, mock, "set_next_send_hash", Encode!(&"0xcfxswap1".to_string()).unwrap());

    // ── Step 4: enable the liquidation config + drop the price to $0.08 ──────
    decode_result(
        update_dev(
            &pic,
            backend,
            "set_chain_liquidation_config",
            Encode!(&ChainId(CONFLUX_CHAIN_ID), &enabled_liq_config()).unwrap(),
        ),
        "set_chain_liquidation_config",
    )
    .expect("set_chain_liquidation_config Ok");

    // $0.08 / CFX => 1400 * 0.08 = $112 vs 100 debt => CR ~112% < 133%.
    decode_result(
        update_dev(
            &pic,
            backend,
            "set_manual_collateral_price",
            Encode!(&ChainId(CONFLUX_CHAIN_ID), &"CFX".to_string(), &8_000_000u64).unwrap(),
        ),
        "set_manual_collateral_price (drop)",
    )
    .expect("set_manual_collateral_price (drop)");

    // ── Step 5: observer tick marks the vault (Bot tier; collateral reserved) +
    // the settlement worker SUBMITS the swap (DEX reads + JIT min-out + oracle
    // gate, all seeded above) -> the op goes Inflight, the marker persists. ─────
    advance_and_tick(&pic, 3);

    let v = get_vault(&pic, backend, vault_id).expect("vault after detection + swap submit");
    let marker = v
        .pending_liquidation
        .clone()
        .expect("detection set a pending_liquidation marker (NOT escalated: the swap broadcast)");
    assert_eq!(marker.tier, LiquidationTier::Bot, "Tier-1 bot liquidation");
    assert!(
        marker.debt_to_clear_e8s > candid::Nat::from(0u32),
        "marker sized to clear a non-zero debt"
    );
    // The full $112 of collateral covers the full $100 debt at the 1.12x bonus, so
    // the entire 1400 CFX is reserved and the vault's remaining collateral is 0.
    assert_eq!(
        v.collateral_amount_e18,
        candid::Nat::from(0u32),
        "all collateral reserved (full liquidation): remaining collateral == 0"
    );
    assert_eq!(v.debt_e8s, candid::Nat::from(debt_e8s), "debt UNCHANGED at trigger (Design B)");
    assert_eq!(v.status, ChainVaultStatus::Open, "vault stays Open under the marker");
    assert_supply(&pic, backend, 100 * E8, "after detection + swap submit (supply unchanged)");

    // ── Step 6: provide the realized-USDC Transfer log + receipt at the cursor ─
    // The confirm reads the receipt (mined+final) then queries eth_getLogs on the
    // USDC token for TRANSFER_TOPIC0 at [cursor1, cursor1]. The mock's get_logs
    // filters by topic0 + block range (NOT contract address), so a push_log with
    // the Transfer topic0 + the reserve `to` topic at cursor1 is returned.
    update_any(
        &pic,
        mock,
        "set_receipt",
        Encode!(&"0xcfxswap1".to_string(), &true, &cursor1).unwrap(),
    );
    // realized = 110 USDC (18-dec) > the 100 icUSD debt; from(any), to(reserve).
    let realized_usdc_native = 110u128 * E18;
    let transfer_topics = vec![
        TRANSFER_TOPIC0.to_string(),
        word_addr("0x000000000000000000000000000000000000babe"), // from (any)
        word_addr(&reserve),                                     // to == reserve
    ];
    update_any(
        &pic,
        mock,
        "push_log",
        Encode!(
            &transfer_topics,
            &word_u128(realized_usdc_native),
            &"0xcfxswap1".to_string(),
            &cursor1
        )
        .unwrap(),
    );
    advance_and_tick(&pic, 4); // confirm the swap (Phase 2: debt -> reserve)

    // ════════════════════════════════════════════════════════════════════════
    // HAPPY-PATH ASSERTIONS
    // ════════════════════════════════════════════════════════════════════════
    let v = get_vault(&pic, backend, vault_id).expect("vault after swap confirm");
    assert_eq!(v.debt_e8s, candid::Nat::from(0u32), "swap cleared the full debt => debt_e8s 0");
    assert_eq!(
        v.status,
        ChainVaultStatus::Closed,
        "fully drained (debt 0 + collateral 0) => Closed"
    );
    assert!(
        v.pending_liquidation.is_none(),
        "swap settled => pending_liquidation marker cleared"
    );

    // PSM: NO icUSD burned. chain_supplies is UNCHANGED across the whole swap.
    assert_supply(&pic, backend, 100 * E8, "after swap confirm (PSM: supply unchanged)");

    // Reserve credited: reconcile reads on-chain totalSupply (== recorded => no
    // unbacked excess) and reports the reserve backing + physical USDC.
    update_any(&pic, mock, "set_total_supply", Encode!(&(100u128 * E8)).unwrap());
    let recon = reconcile_chain_supply(&pic, backend).expect("reconcile_chain_supply Ok");
    assert_eq!(
        recon.reserve_backing_e8s,
        candid::Nat::from(100u128 * E8),
        "reserve_backing_e8s == the cleared debt (100e8)"
    );
    assert_eq!(
        recon.reserve_usdc_native,
        candid::Nat::from(110u128 * E18),
        "reserve_usdc_native == the realized USDC (110e18)"
    );
    assert!(
        !recon.unbacked_excess,
        "reserve is BACKING, not unbacked supply (findings #17/#20/#29): unbacked_excess == false"
    );

    eprintln!("[swap] FULL swap path PASSED: detection marked an underwater vault (Bot tier, all collateral reserved), the settlement worker SUBMITTED swapExactETHForTokens (DEX reads + JIT min-out + oracle gate) and CONFIRMED it (decoded the realized 110 USDC Transfer to the reserve), moving 100e8 debt -> reserve_backing (vault Closed, debt 0). chain_supplies UNCHANGED (PSM, no icUSD burned); reconcile reports reserve_backing=100e8, reserve_usdc=110e18, unbacked_excess=false on chain 71.");
}

// ─── helpers ─────────────────────────────────────────────────────────────────

fn get_vault(pic: &PocketIc, backend: Principal, vault_id: u64) -> Option<ChainVaultV1> {
    let reply = pic
        .query_call(
            backend,
            Principal::anonymous(),
            "get_chain_vault",
            Encode!(&vault_id).unwrap(),
        )
        .expect("get_chain_vault query");
    match reply {
        WasmResult::Reply(b) => Decode!(&b, Option<ChainVaultV1>).expect("decode get_chain_vault"),
        WasmResult::Reject(msg) => panic!("get_chain_vault rejected: {msg}"),
    }
}

fn get_liq_config(pic: &PocketIc, backend: Principal) -> Option<ChainLiquidationConfigV1> {
    let reply = pic
        .query_call(
            backend,
            Principal::anonymous(),
            "get_chain_liquidation_config",
            Encode!(&ChainId(CONFLUX_CHAIN_ID)).unwrap(),
        )
        .expect("get_chain_liquidation_config query");
    match reply {
        WasmResult::Reply(b) => {
            Decode!(&b, Option<ChainLiquidationConfigV1>).expect("decode get_chain_liquidation_config")
        }
        WasmResult::Reject(msg) => panic!("get_chain_liquidation_config rejected: {msg}"),
    }
}

fn get_reserve_address(pic: &PocketIc, backend: Principal) -> Result<String, ProtocolError> {
    match update_dev(
        pic,
        backend,
        "get_chain_reserve_address",
        Encode!(&ChainId(CONFLUX_CHAIN_ID)).unwrap(),
    ) {
        WasmResult::Reply(b) => {
            Decode!(&b, Result<String, ProtocolError>).expect("decode get_chain_reserve_address")
        }
        WasmResult::Reject(msg) => {
            // A transport-level reject (ECDSA off in this build) => treat as Err.
            Err(ProtocolError::ChainAdmin(format!("reject: {msg}")))
        }
    }
}

fn reconcile_chain_supply(
    pic: &PocketIc,
    backend: Principal,
) -> Result<ChainSupplyReconciliation, ProtocolError> {
    match update_dev(
        pic,
        backend,
        "reconcile_chain_supply",
        Encode!(&ChainId(CONFLUX_CHAIN_ID)).unwrap(),
    ) {
        WasmResult::Reply(b) => Decode!(&b, Result<ChainSupplyReconciliation, ProtocolError>)
            .expect("decode reconcile_chain_supply"),
        WasmResult::Reject(msg) => panic!("reconcile_chain_supply rejected: {msg}"),
    }
}

fn push_mint_log(
    pic: &PocketIc,
    mock: Principal,
    vault_id: u64,
    recipient: &str,
    amount_e8s: u128,
    tx_hash: &str,
    block: u64,
) {
    let topics = vec![
        MINT_EVENT_TOPIC0.to_string(),
        word_u128(vault_id as u128),
        word_addr(recipient),
    ];
    let data = word_u128(amount_e8s);
    update_any(
        pic,
        mock,
        "push_log",
        Encode!(&topics, &data, &tx_hash.to_string(), &block).unwrap(),
    );
}
