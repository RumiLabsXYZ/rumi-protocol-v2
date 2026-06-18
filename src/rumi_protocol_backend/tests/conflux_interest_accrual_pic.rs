//! Conflux eSpace (chain 71) end-to-end INTEREST-ACCRUAL integration test
//! (Task 12, Option B), against the scripted mock EVM RPC canister.
//!
//! Mirrors `conflux_espace_happy_path_pic.rs` through open -> deposit -> mint,
//! then exercises the interest path:
//!
//!   open -> deposit -> mint (debt 100e8, supply 100e8)
//!     -> advance ~60d, harvest_chain_interest  => InterestMint enqueued, vault
//!        reserves pending_interest_mint_e8s (debt/supply UNCHANGED)
//!     -> settlement mints the interest to the per-chain interest-treasury and
//!        confirms                               => debt 100e8+I, supply 100e8+I
//!     -> burn (100e8 + I) on chain              => debt 0, supply 0
//!     -> withdraw all collateral                => Closing -> Closed, supply 0
//!
//! The POINT: after the interest mint confirms, the vault's `debt_e8s` and the
//! chain supply grow by EXACTLY the same amount, so the supply invariant
//! (get_global_icusd_supply() == sum(chain vault debt)) gap stays 0. This is
//! what makes Option B correct where Option C would drive supply below debt.
//!
//! Like the happy path, this boots with an II subnet so PocketIC can provision
//! `test_key_1`, PROBES ECDSA, and runs the GATED subset (register/config +
//! invariant-at-0) on an ECDSA-less build, auto-upgrading to the full path on an
//! ECDSA-capable build.

use candid::{encode_args, encode_one, CandidType, Decode, Deserialize, Encode, Principal};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use std::time::Duration;

// ─── Locally-mirrored backend types ──────────────────────────────────────────

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

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
enum ChainVaultStatus {
    AwaitingDeposit,
    MintPending,
    Open,
    Closing,
    Closed,
}

/// Mirror of the backend `ChainVaultV1` WIRE shape, INCLUDING the two Task-12
/// interest fields (decode tolerates them because the backend now returns them).
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
    last_interest_accrual_ns: u64,
    pending_interest_mint_e8s: candid::Nat,
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

#[derive(CandidType, Deserialize, Clone, Debug)]
enum ProtocolError {
    ChainAdmin(String),
    GenericError(String),
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
const MINT_EVENT_TOPIC0: &str =
    "0x4e3883c75cc9c752bb1db2e406a822e4a75067ae77ad9a0a4d179f2709b9e1f6";
const BURN_EVENT_TOPIC0: &str =
    "0xe1b6e34006e9871307436c226f232f9c5e7690c1d2c4f4adda4f607a75a9beca";
const CONFLUX_FINALITY_DEPTH: u64 = 100;
const SCAN_WINDOW: u64 = 1024;

// ─── Call helpers ─────────────────────────────────────────────────────────────

fn dev() -> Principal {
    Principal::from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9])
}

fn query_unit<T>(pic: &PocketIc, cid: Principal, method: &str) -> T
where
    T: CandidType + for<'a> Deserialize<'a>,
{
    match pic
        .query_call(cid, Principal::anonymous(), method, encode_one(()).unwrap())
        .expect("query call")
    {
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

fn u128_of(n: &candid::Nat) -> u128 {
    // candid::Nat's Display uses `_` thousands separators; strip before parse.
    n.to_string().replace('_', "").parse::<u128>().expect("nat fits u128")
}

// ─── Boot ─────────────────────────────────────────────────────────────────────

fn boot() -> (PocketIc, Principal, Principal) {
    let pic = PocketIcBuilder::new()
        .with_ii_subnet()
        .with_application_subnet()
        .build();
    let backend_id = pic.create_canister();
    pic.add_cycles(backend_id, 100_000_000_000_000);
    let mock_id = pic.create_canister();
    pic.add_cycles(mock_id, 100_000_000_000_000);

    let mgmt = Principal::from_text("aaaaa-aa").unwrap();
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
    pic.install_canister(backend_id, backend_wasm(), encode_args((init,)).unwrap(), None);
    pic.install_canister(mock_id, mock_wasm(), Encode!().unwrap(), None);
    for _ in 0..5 {
        pic.tick();
    }
    let _ = update_dev(&pic, backend_id, "set_observer_tick_interval_secs", Encode!(&30u64).unwrap());
    let _ = update_dev(&pic, backend_id, "set_settlement_tick_interval_secs", Encode!(&30u64).unwrap());
    // The mock has no XRC, so every ICP price fetch fails; 3 consecutive failures
    // trip the oracle circuit breaker into ReadOnly (which halts the settlement
    // worker). This test runs longer than the happy path, so slow the XRC fetch
    // timer to ~never — only the one immediate startup fetch fails (counter=1).
    let _ = update_dev(&pic, backend_id, "set_xrc_fetch_interval_secs", Encode!(&1_000_000_000u64).unwrap());
    (pic, backend_id, mock_id)
}

fn advance_and_tick(pic: &PocketIc, windows: u32) {
    for _ in 0..windows {
        pic.advance_time(Duration::from_secs(35));
        for _ in 0..10 {
            pic.tick();
        }
    }
}

fn assert_supply(pic: &PocketIc, cid: Principal, expected_e8s: u128, step: &str) {
    let global: candid::Nat = query_unit(pic, cid, "get_global_icusd_supply");
    assert_eq!(global, candid::Nat::from(expected_e8s), "[{step}] get_global_icusd_supply mismatch");
    let audit: SupplyAuditWire = query_unit(pic, cid, "get_supply_audit");
    assert_eq!(audit.total_e8s, candid::Nat::from(expected_e8s), "[{step}] supply_audit.total mismatch");
    let sum: candid::Nat = audit
        .per_chain
        .iter()
        .fold(candid::Nat::from(0u32), |acc, e| acc + e.supply_e8s.clone());
    assert_eq!(audit.total_e8s, sum, "[{step}] audit total != sum(per_chain)");
    if let Some(c) = audit.per_chain.iter().find(|e| e.chain_id == CONFLUX_CHAIN_ID) {
        assert_eq!(c.supply_e8s, candid::Nat::from(expected_e8s), "[{step}] Conflux per-chain supply mismatch");
    }
}

fn word_u128(v: u128) -> String {
    format!("0x{:064x}", v)
}
fn word_addr(addr: &str) -> String {
    let raw = addr.strip_prefix("0x").or_else(|| addr.strip_prefix("0X")).unwrap_or(addr);
    format!("0x{:0>64}", raw.to_lowercase())
}

fn get_vault(pic: &PocketIc, backend: Principal, vault_id: u64) -> Option<ChainVaultV1> {
    match pic
        .query_call(backend, Principal::anonymous(), "get_chain_vault", Encode!(&vault_id).unwrap())
        .expect("get_chain_vault query")
    {
        WasmResult::Reply(b) => Decode!(&b, Option<ChainVaultV1>).expect("decode get_chain_vault"),
        WasmResult::Reject(msg) => panic!("get_chain_vault rejected: {msg}"),
    }
}

fn push_mint_log(pic: &PocketIc, mock: Principal, vault_id: u64, recipient: &str, amount_e8s: u128, tx_hash: &str, block: u64) {
    let topics = vec![MINT_EVENT_TOPIC0.to_string(), word_u128(vault_id as u128), word_addr(recipient)];
    update_any(pic, mock, "push_log", Encode!(&topics, &word_u128(amount_e8s), &tx_hash.to_string(), &block).unwrap());
}
fn push_burn_log(pic: &PocketIc, mock: Principal, vault_id: u64, burner: &str, amount_e8s: u128, tx_hash: &str, block: u64) {
    let topics = vec![BURN_EVENT_TOPIC0.to_string(), word_u128(vault_id as u128), word_addr(burner)];
    update_any(pic, mock, "push_log", Encode!(&topics, &word_u128(amount_e8s), &tx_hash.to_string(), &block).unwrap());
}

// ─── The test ────────────────────────────────────────────────────────────────

#[test]
fn conflux_interest_accrual_supply_invariant() {
    let (pic, backend, mock) = boot();

    decode_result(update_dev(&pic, backend, "set_evm_rpc_principal", Encode!(&mock).unwrap()), "set_evm_rpc_principal").unwrap();
    update_any(&pic, mock, "set_getlogs_max_range", Encode!(&1000u64).unwrap());
    update_any(&pic, mock, "set_espace_receipt_fields", Encode!(&true).unwrap());

    let reg = RegisterChainArg {
        chain_id: ChainId(CONFLUX_CHAIN_ID),
        display_name: "ConfluxESpaceTestnet".to_string(),
        rpc_endpoints: vec!["https://evmtestnet.confluxrpc.com".to_string()],
        finality_depth: CONFLUX_FINALITY_DEPTH as u32,
        gas_strategy: GasStrategy::EvmEip1559 { max_priority_fee_gwei: 1, max_fee_gwei_ceiling: 100 },
        chain_native_decimals: 18,
        min_quorum_providers: Some(1),
    };
    decode_result(update_dev(&pic, backend, "register_chain", Encode!(&reg).unwrap()), "register_chain").unwrap();
    decode_result(
        update_dev(&pic, backend, "set_chain_contract", Encode!(&ChainId(CONFLUX_CHAIN_ID), &"0x00000000000000000000000000000000cf1c0de5".to_string()).unwrap()),
        "set_chain_contract",
    ).unwrap();
    decode_result(
        update_dev(&pic, backend, "set_manual_collateral_price", Encode!(&ChainId(CONFLUX_CHAIN_ID), &"CFX".to_string(), &15_000_000u64).unwrap()),
        "set_manual_collateral_price",
    ).unwrap();
    let seed: u64 = 1_000_000;
    decode_result(update_dev(&pic, backend, "set_last_observed_block", Encode!(&ChainId(CONFLUX_CHAIN_ID), &seed).unwrap()), "set_last_observed_block").unwrap();
    decode_result(update_dev(&pic, backend, "set_burn_watch_poll_enabled", Encode!(&ChainId(CONFLUX_CHAIN_ID), &true).unwrap()), "set_burn_watch_poll_enabled").unwrap();

    assert_supply(&pic, backend, 0, "after register/config");

    let cursor1 = seed + SCAN_WINDOW;
    let head1 = cursor1 + CONFLUX_FINALITY_DEPTH + 24;
    update_any(&pic, mock, "set_blocks", Encode!(&head1, &head1).unwrap());
    update_any(&pic, mock, "set_next_send_hash", Encode!(&"0xcfxmint1".to_string()).unwrap());

    // ECDSA probe (decides full vs gated).
    let settlement_addr = match update_dev(&pic, backend, "get_chain_settlement_address", Encode!(&ChainId(CONFLUX_CHAIN_ID)).unwrap()) {
        WasmResult::Reply(b) => match Decode!(&b, Result<String, ProtocolError>) {
            Ok(Ok(addr)) => Some(addr),
            _ => None,
        },
        WasmResult::Reject(_) => None,
    };
    let _settlement_addr = match settlement_addr {
        Some(addr) => addr,
        None => {
            eprintln!("[conflux interest] ECDSA UNAVAILABLE; running GATED subset");
            assert_supply(&pic, backend, 0, "gated: invariant at 0");
            return;
        }
    };
    eprintln!("[conflux interest] ECDSA AVAILABLE; running FULL interest path");

    // Fund the settlement (hot wallet) address so the mint gas gate passes.
    update_any(&pic, mock, "set_balance", Encode!(&_settlement_addr, &candid::Nat::from(1_000_000u128 * E18)).unwrap());

    // ── open -> deposit -> mint (debt 100e8, supply 100e8) ───────────────────
    let collateral_e18 = 1_400u128 * E18;
    let debt_e8s = 100u128 * E8;
    let recipient = "0x000000000000000000000000000000000000c0de".to_string();
    let vault_id: u64 = match update_dev(
        &pic, backend, "open_chain_vault",
        Encode!(&ChainId(CONFLUX_CHAIN_ID), &candid::Nat::from(collateral_e18), &candid::Nat::from(debt_e8s), &recipient).unwrap(),
    ) {
        WasmResult::Reply(b) => Decode!(&b, Result<u64, ProtocolError>).unwrap().expect("open ok"),
        WasmResult::Reject(m) => panic!("open_chain_vault rejected: {m}"),
    };
    let custody = get_vault(&pic, backend, vault_id).unwrap().custody_address;
    update_any(&pic, mock, "set_balance", Encode!(&custody, &candid::Nat::from(collateral_e18)).unwrap());
    advance_and_tick(&pic, 2); // observer -> MintPending + enqueue mint

    advance_and_tick(&pic, 1); // settlement submit
    update_any(&pic, mock, "set_receipt", Encode!(&"0xcfxmint1".to_string(), &true, &cursor1).unwrap());
    push_mint_log(&pic, mock, vault_id, &recipient, debt_e8s, "0xcfxmint1", cursor1);
    advance_and_tick(&pic, 4); // settlement confirm

    let v = get_vault(&pic, backend, vault_id).expect("vault after mint");
    assert_eq!(v.status, ChainVaultStatus::Open, "mint confirmed => Open");
    assert_eq!(u128_of(&v.debt_e8s), 100 * E8);
    assert!(v.last_interest_accrual_ns > 0, "accrual window stamped at mint-confirm");
    assert_supply(&pic, backend, 100 * E8, "after mint confirm");

    // ── Interest: harvest, mint interest to treasury, confirm ────────────────
    // Lower the dust floor to 1 e8s so the small elapsed since mint-confirm
    // produces a realizable (positive) interest WITHOUT a large clock jump (a
    // big advance_time confuses PocketIC's overdue-timer handling). The interest
    // MAGNITUDE is unit-tested in chains::interest; this test proves the
    // end-to-end supply invariant, which holds for any positive interest.
    decode_result(update_dev(&pic, backend, "set_chain_interest_min_realize_e8s", Encode!(&1u128).unwrap()), "set_chain_interest_min_realize_e8s").unwrap();
    advance_and_tick(&pic, 2); // accrue a little interest since mint-confirm

    let treasury: String = match update_dev(&pic, backend, "get_chain_interest_treasury_address", Encode!(&ChainId(CONFLUX_CHAIN_ID)).unwrap()) {
        WasmResult::Reply(b) => Decode!(&b, Result<String, ProtocolError>).unwrap().expect("treasury addr"),
        WasmResult::Reject(m) => panic!("get_chain_interest_treasury_address rejected: {m}"),
    };
    assert_ne!(treasury.to_lowercase(), _settlement_addr.to_lowercase(), "treasury != minter address");

    // Pre-arm the interest-mint broadcast hash, then harvest.
    update_any(&pic, mock, "set_next_send_hash", Encode!(&"0xcfxint1".to_string()).unwrap());
    let n: u64 = match update_dev(&pic, backend, "harvest_chain_interest", Encode!(&ChainId(CONFLUX_CHAIN_ID)).unwrap()) {
        WasmResult::Reply(b) => Decode!(&b, Result<u64, ProtocolError>).unwrap().expect("harvest ok"),
        WasmResult::Reject(m) => panic!("harvest_chain_interest rejected: {m}"),
    };
    assert_eq!(n, 1, "one eligible vault => one interest mint enqueued");

    let v = get_vault(&pic, backend, vault_id).expect("vault after harvest");
    let interest = u128_of(&v.pending_interest_mint_e8s);
    assert!(interest > 0, "accrued interest is positive: got {interest}");
    assert_eq!(u128_of(&v.debt_e8s), 100 * E8, "debt unchanged until interest mint confirms");
    assert_supply(&pic, backend, 100 * E8, "after harvest (reserved, not yet minted)");

    // Synthetic on-chain mint id = chain_vault_id_counter after harvest. One vault
    // opened (vault_id == counter == 1); the harvest allocated counter+1.
    let interest_mint_id = vault_id + 1;

    advance_and_tick(&pic, 1); // settlement submit interest mint
    update_any(&pic, mock, "set_receipt", Encode!(&"0xcfxint1".to_string(), &true, &cursor1).unwrap());
    push_mint_log(&pic, mock, interest_mint_id, &treasury, interest, "0xcfxint1", cursor1);
    advance_and_tick(&pic, 4); // settlement confirm interest mint

    let v = get_vault(&pic, backend, vault_id).expect("vault after interest confirm");
    assert_eq!(u128_of(&v.pending_interest_mint_e8s), 0, "pending interest cleared on confirm");
    let debt_with_interest = 100 * E8 + interest;
    assert_eq!(u128_of(&v.debt_e8s), debt_with_interest, "debt grew by the minted interest");
    // THE invariant: debt and supply grew together -> gap stays 0.
    assert_supply(&pic, backend, debt_with_interest, "after interest mint (debt+supply grew together)");

    // ── burn principal + interest => debt 0, supply 0 ─────────────────────────
    let cursor2 = cursor1 + SCAN_WINDOW;
    let head2 = cursor2 + CONFLUX_FINALITY_DEPTH + 24;
    update_any(&pic, mock, "set_blocks", Encode!(&head2, &head2).unwrap());
    push_burn_log(&pic, mock, vault_id, &recipient, debt_with_interest, "0xcfxburn1", cursor1 + 500);
    advance_and_tick(&pic, 3);

    let v = get_vault(&pic, backend, vault_id).expect("vault after burn");
    assert_eq!(u128_of(&v.debt_e8s), 0, "principal+interest fully burned => debt 0");
    assert_supply(&pic, backend, 0, "after burn principal+interest");

    // ── withdraw all collateral => Closing -> Closed ──────────────────────────
    update_any(&pic, mock, "set_next_send_hash", Encode!(&"0xcfxwd1".to_string()).unwrap());
    decode_result(
        update_dev(&pic, backend, "withdraw_chain_collateral", Encode!(&vault_id, &candid::Nat::from(collateral_e18), &"0x000000000000000000000000000000000000dead".to_string()).unwrap()),
        "withdraw_chain_collateral",
    ).unwrap();
    let v = get_vault(&pic, backend, vault_id).expect("vault after withdraw enqueue");
    assert_eq!(v.status, ChainVaultStatus::Closing, "withdraw all => Closing");

    advance_and_tick(&pic, 1);
    update_any(&pic, mock, "set_receipt", Encode!(&"0xcfxwd1".to_string(), &true, &cursor2).unwrap());
    advance_and_tick(&pic, 4);

    let v = get_vault(&pic, backend, vault_id).expect("vault after withdraw confirm");
    assert_eq!(v.status, ChainVaultStatus::Closed, "withdraw confirmed => Closed");
    assert_supply(&pic, backend, 0, "final (Closed)");

    eprintln!("[conflux interest] FULL interest path PASSED: supply invariant held 0 -> 100e8 -> 100e8+I -> 0 across open/mint/HARVEST/interest-mint/burn/withdraw; debt and supply grew together by I={interest} e8s");
}
