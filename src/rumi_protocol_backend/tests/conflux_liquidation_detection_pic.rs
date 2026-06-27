//! Conflux eSpace testnet (chain 71) Increment-2 Tier-1 LIQUIDATION DETECTION
//! end-to-end integration test against the scripted mock EVM RPC canister.
//!
//! This is the integration PROOF that the Increment-2 detection scan +
//! endpoints work end-to-end:
//!
//!   - The per-chain observer tick (`run_observer`) runs a synchronous
//!     liquidation-detection scan. For an `Open` chain vault whose
//!     interest-aware collateral ratio falls below the chain's
//!     `liquidation_threshold_e4` (133% == 13_300 for Conflux chain 71), it calls
//!     `begin_liquidation_in_state`, which sets a `pending_liquidation` marker
//!     (tier `Bot`), reserves (decrements) collateral, and enqueues an INERT
//!     `LiquidationSwap` settlement op. Debt + `chain_supplies` are UNCHANGED at
//!     trigger (Design B: no burn, no reserve shift in Inc 2).
//!   - Detection is gated behind a per-chain `ChainLiquidationConfigV1` with
//!     `enabled = true` (master switch; default false).
//!   - New endpoints: `liquidate_chain_vault` (dev-gated #[update]),
//!     `get_chain_liquidatable_vaults` (#[query]), `set_chain_liquidation_config`
//!     (dev-gated; the config struct GAINED `max_swap_value_e8s` +
//!     `max_price_age_ns`).
//!   - A stale manual price (older than `max_price_age_ns`) makes detection defer
//!     for the whole chain (no marker set; the query also fails closed).
//!
//! Harness (boot, register_chain, deposit->mint->Open sequence, ECDSA-gating,
//! supply invariant) is copied from `conflux_espace_happy_path_pic.rs`. As in
//! that test, the flow PROBES ECDSA via `get_chain_settlement_address`; if ECDSA
//! is unavailable in this PocketIC build it runs a GATED subset (which still
//! proves the new config fields decode + the new query is callable) and returns
//! early.

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

// ─── Liquidation mirrors (Increment 2) ───────────────────────────────────────

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
/// field for the native collateral amount is `collateral_amount_e18` (a
/// `#[serde(rename)]` on the backend keeps the candid name legacy-stable).
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

/// Mirror of `chains::liquidation_config::ChainLiquidationConfigV1`, INCLUDING
/// the two Increment-2 fields (`max_swap_value_e8s`, `max_price_age_ns`).
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
    // Increment 3 fields (the backend struct gained these; the mirror MUST match
    // or set_chain_liquidation_config fails to decode).
    max_dex_oracle_divergence_bps: u32,
    fee_bps: u16,
    settle_stable_decimals: u8,
    deadline_secs: u64,
}

/// Mirror of `main::ChainLiquidatableVault` (the discovery query row).
#[derive(CandidType, Deserialize, Clone, Debug)]
struct ChainLiquidatableVault {
    vault_id: u64,
    chain_id: ChainId,
    debt_e8s: candid::Nat,
    effective_debt_e8s: candid::Nat,
    collateral_native: candid::Nat,
    cr_e4: u64,
    liquidation_threshold_e4: u64,
    sized_repay_e8s: candid::Nat,
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
/// the `ChainAdmin(text)` variant. Any other variant fails the Candid decode
/// loudly (the correct signal that something unexpected happened).
#[derive(CandidType, Deserialize, Clone, Debug)]
enum ProtocolError {
    ChainAdmin(String),
    EvmAuth(String),
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
/// UniswapV2 factory `getPair(address,address)` selector.
const GET_PAIR_SELECTOR: &str = "0xe6a43905";

/// Conflux register arg's `finality_depth` (mirrors conflux_testnet_register_arg()).
const CONFLUX_FINALITY_DEPTH: u64 = 100;
/// Observer block-scan window (`MAX_BLOCK_SCAN_WINDOW`).
const SCAN_WINDOW: u64 = 1024;
/// Conflux chain-71 liquidation threshold (133.00% in e4) — must match
/// `collateral_config.rs`.
const CONFLUX_LIQ_THRESHOLD_E4: u64 = 13_300;
/// Valid EVM address used for every config wiring slot (the engine validates
/// router/pair/settle_stable_token via `is_valid_evm_address` before enqueue).
const VALID_EVM_ADDR: &str = "0x1111111111111111111111111111111111111111";

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

// ─── Supply-invariant assertion (Design B: marker does NOT move supply) ──────

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

// ─── 32-byte ABI word encoding for log topics / data ─────────────────────────

fn word_u128(v: u128) -> String {
    format!("0x{:064x}", v)
}

fn word_addr(addr: &str) -> String {
    let raw = addr.strip_prefix("0x").or_else(|| addr.strip_prefix("0X")).unwrap_or(addr);
    format!("0x{:0>64}", raw.to_lowercase())
}

fn script_factory_pair_sanity(pic: &PocketIc, mock: Principal, pair: &str) {
    let _ = update_any(
        pic,
        mock,
        "set_eth_call_response",
        Encode!(&GET_PAIR_SELECTOR.to_string(), &word_addr(pair)).unwrap(),
    );
}

// ─── liquidation config builders ─────────────────────────────────────────────

/// An ENABLED Conflux liquidation config with valid wiring. slippage 250 bps
/// (< the 1200-bps penalty cushion), restore target 155%, depth cap $2k,
/// staleness ceiling 30 min.
fn enabled_liq_config() -> ChainLiquidationConfigV1 {
    ChainLiquidationConfigV1 {
        dex: DexKind::UniswapV2,
        router: VALID_EVM_ADDR.to_string(),
        factory: VALID_EVM_ADDR.to_string(),
        pair: VALID_EVM_ADDR.to_string(),
        collateral_token: VALID_EVM_ADDR.to_string(),
        settle_stable_token: VALID_EVM_ADDR.to_string(),
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
fn conflux_liquidation_detection_marks_and_endpoints() {
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
                eprintln!("[liquidation-detection] ECDSA UNAVAILABLE (get_chain_settlement_address returned Err: {e:?}); running GATED subset");
                None
            }
            Err(decode_err) => {
                eprintln!("[liquidation-detection] get_chain_settlement_address decode error ({decode_err}); running GATED subset");
                None
            }
        },
        WasmResult::Reject(msg) => {
            eprintln!("[liquidation-detection] ECDSA UNAVAILABLE (get_chain_settlement_address rejected: {msg}); running GATED subset");
            None
        }
    };

    let settlement_addr = match settlement_addr {
        Some(addr) => {
            eprintln!("[liquidation-detection] ECDSA AVAILABLE; settlement address = {addr}; running FULL detection path");
            addr
        }
        None => {
            // ── GATED subset: the new config fields decode + the new query is
            // callable. No Open vaults exist yet, so the query is empty.
            script_factory_pair_sanity(&pic, mock, VALID_EVM_ADDR);
            decode_result(
                update_dev(
                    &pic,
                    backend,
                    "set_chain_liquidation_config",
                    Encode!(&ChainId(CONFLUX_CHAIN_ID), &enabled_liq_config()).unwrap(),
                ),
                "set_chain_liquidation_config",
            )
            .expect("gated: set_chain_liquidation_config (proves new fields decode)");

            let cfg = get_liq_config(&pic, backend).expect("gated: config round-trips");
            assert_eq!(
                cfg.max_swap_value_e8s,
                candid::Nat::from(2_000u128 * E8),
                "gated: max_swap_value_e8s round-trips"
            );
            assert_eq!(cfg.max_price_age_ns, 1_800_000_000_000, "gated: max_price_age_ns round-trips");
            assert!(cfg.enabled, "gated: enabled round-trips");

            let liq = get_liquidatable(&pic, backend);
            assert!(liq.is_empty(), "gated: no Open vaults yet => empty liquidatable list");

            assert_supply(&pic, backend, 0, "gated: ECDSA unavailable, invariant at 0");
            eprintln!("[liquidation-detection] ECDSA unavailable; ran gated subset");
            return;
        }
    };

    // ════════════════════════════════════════════════════════════════════════
    // FULL DETECTION PATH (ECDSA available)
    // ════════════════════════════════════════════════════════════════════════

    // Fund the settlement (hot-wallet) address so the submit-path gas gate passes.
    update_any(
        &pic,
        mock,
        "set_balance",
        Encode!(&settlement_addr, &candid::Nat::from(1_000_000u128 * E18)).unwrap(),
    );

    // ── Open a vault: 1400 CFX @ $0.15 = $210 backing 100 icUSD => 210% CR ────
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

    advance_and_tick(&pic, 1); // submit (Queued -> Inflight)
    update_any(
        &pic,
        mock,
        "set_receipt",
        Encode!(&"0xcfxmint1".to_string(), &true, &cursor1).unwrap(),
    );
    push_mint_log(&pic, mock, vault_id, &recipient, debt_e8s, "0xcfxmint1", cursor1);
    advance_and_tick(&pic, 4); // confirm (receipt mined + final, Mint log read)

    let v = get_vault(&pic, backend, vault_id).expect("vault after mint confirm");
    assert_eq!(v.status, ChainVaultStatus::Open, "mint confirmed => Open");
    assert_eq!(v.debt_e8s, candid::Nat::from(debt_e8s), "mint confirmed => debt 100e8");
    assert!(v.pending_liquidation.is_none(), "healthy vault has no liquidation marker");
    assert_supply(&pic, backend, 100 * E8, "after mint");

    // Inc 3: the LiquidationSwap op is now ACTIONABLE (the settlement worker would
    // submit it). This test is about DETECTION only (the swap submit/confirm is the
    // conflux_liquidation_swap_pic suite), so idle the settlement timer now (after
    // the mint confirmed) — the swap op stays Queued and the `pending_liquidation`
    // marker persists for the detection assertions below. The observer (detection)
    // timer keeps firing.
    let _ = update_dev(&pic, backend, "set_settlement_tick_interval_secs", Encode!(&31_536_000u64).unwrap());

    // ── Enable the liquidation config (master switch on) ─────────────────────
    script_factory_pair_sanity(&pic, mock, VALID_EVM_ADDR);
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

    // ── Drop the price to $0.08 => CR ~112% < 133% (liquidatable) ────────────
    decode_result(
        update_dev(
            &pic,
            backend,
            "set_manual_collateral_price",
            Encode!(&ChainId(CONFLUX_CHAIN_ID), &"CFX".to_string(), &8_000_000u64).unwrap(),
        ),
        "set_manual_collateral_price",
    )
    .expect("set_manual_collateral_price (drop)");

    // ── Pre-tick: the discovery query computes CR LIVE and lists the vault ────
    let liq_before = get_liquidatable(&pic, backend);
    let row = liq_before
        .iter()
        .find(|r| r.vault_id == vault_id)
        .expect("liquidatable list includes the underwater vault (pre-mark)");
    assert!(
        row.cr_e4 < CONFLUX_LIQ_THRESHOLD_E4,
        "pre-mark: cr_e4 {} must be below threshold {}",
        row.cr_e4,
        CONFLUX_LIQ_THRESHOLD_E4
    );
    assert_eq!(
        row.liquidation_threshold_e4, CONFLUX_LIQ_THRESHOLD_E4,
        "row carries the chain-71 threshold"
    );
    assert!(row.sized_repay_e8s > candid::Nat::from(0u32), "pre-mark: a non-zero repay is sized");

    // ── Tick the observer; detection sets the marker (Bot tier) ──────────────
    advance_and_tick(&pic, 3);

    let v = get_vault(&pic, backend, vault_id).expect("vault after detection");
    let marker = v
        .pending_liquidation
        .clone()
        .expect("detection set a pending_liquidation marker");
    assert_eq!(marker.tier, LiquidationTier::Bot, "Tier-1 bot liquidation");
    assert!(
        marker.collateral_reserved_native > candid::Nat::from(0u32),
        "marker reserved a non-zero amount of collateral"
    );
    assert!(
        marker.debt_to_clear_e8s > candid::Nat::from(0u32),
        "marker is sized to clear a non-zero debt"
    );
    // Reservation DECREMENTED the vault's remaining collateral (partial liq).
    assert!(
        v.collateral_amount_e18 < candid::Nat::from(collateral_e18),
        "remaining collateral {} must be LESS than original {} (collateral reserved)",
        v.collateral_amount_e18,
        collateral_e18
    );
    // Debt is UNCHANGED at trigger (Design B: no burn in Inc 2).
    assert_eq!(
        v.debt_e8s,
        candid::Nat::from(debt_e8s),
        "debt UNCHANGED at trigger (Design B)"
    );
    // Vault stays Open throughout (marker, not a status variant).
    assert_eq!(v.status, ChainVaultStatus::Open, "vault stays Open under the marker");

    // ── Supply UNCHANGED: Design B sets no burn / reserve shift in Inc 2 ──────
    assert_supply(&pic, backend, 100 * E8, "after detection (supply unchanged)");

    // ── The discovery query now EXCLUDES the vault (it is marked) ─────────────
    let liq_after = get_liquidatable(&pic, backend);
    assert!(
        liq_after.iter().all(|r| r.vault_id != vault_id),
        "marked vault is excluded from the liquidatable list"
    );

    // ── Dev-gate: a NON-dev caller cannot trigger liquidate_chain_vault ──────
    // The endpoint is a dev-gated #[update]; the canister's `inspect_message`
    // filter rejects an anonymous caller at the INGRESS boundary (before the
    // method body), so `update_call` returns a transport-level `Err`. That is the
    // correct, strongest rejection. (If the build ever lets the call through,
    // the in-body developer check still returns `Err(ChainAdmin("not developer"))`,
    // which we also accept.)
    match pic.update_call(
        backend,
        Principal::anonymous(),
        "liquidate_chain_vault",
        Encode!(&vault_id).unwrap(),
    ) {
        Err(_) => { /* ingress-filter rejection — the expected, strongest gate */ }
        Ok(WasmResult::Reject(_)) => { /* an explicit canister reject is also fine */ }
        Ok(WasmResult::Reply(b)) => {
            let r = Decode!(&b, Result<u64, ProtocolError>).expect("decode liquidate_chain_vault");
            match r {
                Err(ProtocolError::ChainAdmin(m)) => assert!(
                    m.contains("developer"),
                    "anonymous liquidate must be rejected as non-developer, got: {m}"
                ),
                other => panic!("anonymous liquidate_chain_vault should be rejected, got Ok {other:?}"),
            }
        }
    }

    // ════════════════════════════════════════════════════════════════════════
    // STALE-PRICE sub-case: a fresh vault + a stale price defers detection
    // (no marker set), and the discovery query fails closed (empty).
    // ════════════════════════════════════════════════════════════════════════

    // Re-enable the settlement timer (it was idled above to keep vault 1's marker
    // for the detection assertions, which are now done). Vault 2 needs settlement
    // to mint. Vault 1's Queued swap will now submit + escalate (no DEX reads in
    // this detection-only test -> getReserves read fails -> Failed, not Inflight),
    // which is harmless here (vault 1's assertions already ran).
    let _ = update_dev(&pic, backend, "set_settlement_tick_interval_secs", Encode!(&30u64).unwrap());

    // Refresh the price (re-stamp set_at_ns to "now") so the second vault can OPEN
    // at a healthy CR, then later go underwater while its price ages out.
    decode_result(
        update_dev(
            &pic,
            backend,
            "set_manual_collateral_price",
            Encode!(&ChainId(CONFLUX_CHAIN_ID), &"CFX".to_string(), &15_000_000u64).unwrap(),
        ),
        "set_manual_collateral_price (refresh for vault 2 open)",
    )
    .expect("set_manual_collateral_price refresh");

    update_any(&pic, mock, "set_next_send_hash", Encode!(&"0xcfxmint2".to_string()).unwrap());
    let v2_collateral = 1_400u128 * E18;
    let v2_debt = 100u128 * E8;
    let v2_recipient = "0x000000000000000000000000000000000000c0d2".to_string();
    let vault2_id: u64 = match update_dev(
        &pic,
        backend,
        "open_chain_vault",
        Encode!(
            &ChainId(CONFLUX_CHAIN_ID),
            &candid::Nat::from(v2_collateral),
            &candid::Nat::from(v2_debt),
            &v2_recipient
        )
        .unwrap(),
    ) {
        WasmResult::Reply(b) => Decode!(&b, Result<u64, ProtocolError>)
            .expect("decode open vault2")
            .expect("open vault2 Ok"),
        WasmResult::Reject(msg) => panic!("open vault2 rejected: {msg}"),
    };
    let v2_custody = get_vault(&pic, backend, vault2_id).unwrap().custody_address;

    // Mint vault2 to Open. Its mint receipt+log must land at the current observer
    // cursor (still cursor1; the cursor only advances when the chain head allows).
    update_any(
        &pic,
        mock,
        "set_balance",
        Encode!(&v2_custody, &candid::Nat::from(v2_collateral)).unwrap(),
    );
    advance_and_tick(&pic, 2); // deposit -> MintPending
    advance_and_tick(&pic, 1); // submit
    update_any(
        &pic,
        mock,
        "set_receipt",
        Encode!(&"0xcfxmint2".to_string(), &true, &cursor1).unwrap(),
    );
    push_mint_log(&pic, mock, vault2_id, &v2_recipient, v2_debt, "0xcfxmint2", cursor1);
    advance_and_tick(&pic, 4); // confirm
    assert_eq!(
        get_vault(&pic, backend, vault2_id).unwrap().status,
        ChainVaultStatus::Open,
        "vault2 minted to Open"
    );
    assert!(
        get_vault(&pic, backend, vault2_id).unwrap().pending_liquidation.is_none(),
        "vault2 unmarked after mint"
    );
    // supply is now 200e8 (two 100e8 vaults).
    assert_supply(&pic, backend, 200 * E8, "after vault2 mint");

    // Drop the price so vault2 is underwater, then AGE the price past
    // max_price_age_ns (30 min) WITHOUT re-stamping it. set_manual stamps the
    // wall-clock at write time, so advancing time alone makes it stale.
    decode_result(
        update_dev(
            &pic,
            backend,
            "set_manual_collateral_price",
            Encode!(&ChainId(CONFLUX_CHAIN_ID), &"CFX".to_string(), &8_000_000u64).unwrap(),
        ),
        "set_manual_collateral_price (drop for vault2)",
    )
    .expect("set_manual_collateral_price drop2");

    // Confirm vault2 IS liquidatable on a FRESH price (sanity: the only thing that
    // will keep it un-marked below is staleness, not a healthy CR).
    let fresh_liq = get_liquidatable(&pic, backend);
    assert!(
        fresh_liq.iter().any(|r| r.vault_id == vault2_id),
        "vault2 is liquidatable while the price is fresh"
    );

    // Age the price WAY past the 30-min staleness ceiling (without re-stamping).
    pic.advance_time(Duration::from_secs(3_600)); // 1 hour > 30 min ceiling
    for _ in 0..10 {
        pic.tick();
    }

    // The discovery query fails closed on a stale price -> empty for the chain.
    let stale_liq = get_liquidatable(&pic, backend);
    assert!(
        stale_liq.is_empty(),
        "stale price => get_chain_liquidatable_vaults fails closed (empty), got {} rows",
        stale_liq.len()
    );

    // Tick the observer: detection must DEFER on the stale price (no new marker).
    advance_and_tick(&pic, 3);
    assert!(
        get_vault(&pic, backend, vault2_id).unwrap().pending_liquidation.is_none(),
        "stale price => detection deferred; vault2 NOT marked"
    );
    // Inc 3: once settlement was re-enabled (for vault 2's mint), vault 1's Queued
    // swap submitted and ESCALATED (this detection-only test sets no DEX reads, so
    // getReserves fails -> Failed, collateral restored, marker cleared, sp_attempted).
    // Detection does NOT re-mark it (sp_attempted exclusion, finding #10). The full
    // swap-success path is the conflux_liquidation_swap_pic suite.
    assert!(
        get_vault(&pic, backend, vault_id).unwrap().pending_liquidation.is_none(),
        "vault 1 escalated (no DEX reads) and is not re-marked"
    );
    // Supply is UNCHANGED throughout: escalation restores collateral, never touches
    // debt/supply; detection never moves supply.
    assert_supply(&pic, backend, 200 * E8, "after stale-price tick (supply unchanged)");

    eprintln!("[liquidation-detection] FULL detection path PASSED: enabled-config-gated observer scan marked an underwater vault (Bot tier, collateral reserved, debt+supply unchanged), the discovery query lists pre-mark / excludes post-mark, the dev gate rejects an anonymous trigger, and a stale price defers detection (no marker, empty query) on chain 71.");
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

fn get_liquidatable(pic: &PocketIc, backend: Principal) -> Vec<ChainLiquidatableVault> {
    let reply = pic
        .query_call(
            backend,
            Principal::anonymous(),
            "get_chain_liquidatable_vaults",
            Encode!(&ChainId(CONFLUX_CHAIN_ID)).unwrap(),
        )
        .expect("get_chain_liquidatable_vaults query");
    match reply {
        WasmResult::Reply(b) => {
            Decode!(&b, Vec<ChainLiquidatableVault>).expect("decode get_chain_liquidatable_vaults")
        }
        WasmResult::Reject(msg) => panic!("get_chain_liquidatable_vaults rejected: {msg}"),
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
