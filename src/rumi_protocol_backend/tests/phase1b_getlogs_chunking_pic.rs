//! Phase 1b — Gate-4 regression: `eth_getLogs` 100-block range cap + chunking.
//!
//! ## The bug this guards
//!
//! The Monad testnet RPC caps `eth_getLogs` at a 100-block RANGE: `toBlock -
//! fromBlock` must be <= 100; a difference of 101 returns HTTP 413 with JSON-RPC
//! code -32614 ("eth_getLogs is limited to a 100 range"). The burn-watch loop in
//! `chains/monad/deposit_watch.rs::run_observer` scans
//! `get_logs(BURN_TOPIC, from = last_observed + 1, to = finalized)`, a range as
//! wide as `MAX_BLOCK_SCAN_WINDOW` (>= 256). On staging EVERY such scan 413-ed,
//! so the burn-watch cursor never advanced and burns were never observed —
//! tracked icUSD supply could not decrement to match an on-chain burn.
//!
//! ## What this test proves
//!
//! The `monad_rpc_mock` now enforces the SAME 100-block cap (see its `request`
//! handler). A burn is placed near the FAR end of a `SCAN_WINDOW`-wide scan range
//! (> 100 blocks past `from_block`), so it can only be observed if `get_logs`
//! pages the range into <= 100-block sub-queries and aggregates them.
//!
//! - **ECDSA unavailable (gated subset, needs no signing):** a poison Burn citing
//!   a non-existent vault sits at the far end of a > 100-block window. Pre-fix the
//!   wide `get_logs` 413s on every tick and the cursor STALLS at the seed; post-fix
//!   the chunked scan reaches the poison, classifies it `InvalidBurn` (skippable),
//!   and the cursor advances past the window. Assertion: cursor advanced.
//! - **ECDSA available (full guard):** stand up an Open vault with debt 100e8, then
//!   place a GOOD 40e8 Burn at the far end of a > 100-block window. Pre-fix the burn
//!   is never seen (debt stays 100e8); post-fix the chunked scan applies it exactly
//!   once → debt 60e8 and `get_global_icusd_supply == sum(vault debt) == 60e8`.
//!
//! Both modes FAIL against the capped mock with an un-chunked `get_logs` and PASS
//! once it chunks.

use candid::{encode_args, encode_one, CandidType, Decode, Deserialize, Encode, Principal};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use std::time::Duration;

// ─── Scan-window knob ──────────────────────────────────────────────────────
//
// Must equal the backend's `MAX_BLOCK_SCAN_WINDOW` (chains/monad/evm_rpc.rs):
// one observer tick advances the burn-watch cursor by exactly this many blocks
// (the consensus-safe probe jumps `last_observed + SCAN_WINDOW`). It is > 100,
// so a single-call `get_logs` over one window exceeds the provider's 100-block
// cap and MUST be chunked.
const SCAN_WINDOW: u64 = 1024;

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

const MONAD_CHAIN_ID: u32 = 10143;
const E18: u128 = 1_000_000_000_000_000_000;
const E8: u128 = 100_000_000;
const MINT_EVENT_TOPIC0: &str =
    "0x4e3883c75cc9c752bb1db2e406a822e4a75067ae77ad9a0a4d179f2709b9e1f6";
const BURN_EVENT_TOPIC0: &str =
    "0xe1b6e34006e9871307436c226f232f9c5e7690c1d2c4f4adda4f607a75a9beca";

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

fn cursor(pic: &PocketIc, backend: Principal) -> u64 {
    let reply = pic
        .query_call(
            backend,
            Principal::anonymous(),
            "get_last_observed_block",
            Encode!(&ChainId(MONAD_CHAIN_ID)).unwrap(),
        )
        .expect("get_last_observed_block query");
    match reply {
        WasmResult::Reply(b) => Decode!(&b, u64).expect("decode get_last_observed_block"),
        WasmResult::Reject(msg) => panic!("get_last_observed_block rejected: {}", msg),
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
        WasmResult::Reject(msg) => panic!("get_chain_vault rejected: {}", msg),
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

    // Pin observer + settlement cadence to 30s so the 35s advance_and_tick windows
    // fire one tick each. The code DEFAULT is now 300s (cycle-burn hardening), so
    // tests declare the cadence they exercise instead of depending on the default.
    let _ = update_dev(&pic, backend_id, "set_observer_tick_interval_secs", Encode!(&30u64).unwrap());
    let _ = update_dev(&pic, backend_id, "set_settlement_tick_interval_secs", Encode!(&30u64).unwrap());

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

// ─── 32-byte ABI word encoding for log topics / data ─────────────────────────

fn word_u128(v: u128) -> String {
    format!("0x{:064x}", v)
}

fn word_addr(addr: &str) -> String {
    let raw = addr
        .strip_prefix("0x")
        .or_else(|| addr.strip_prefix("0X"))
        .unwrap_or(addr);
    format!("0x{:0>64}", raw.to_lowercase())
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

fn push_burn_log(
    pic: &PocketIc,
    mock: Principal,
    vault_id: u64,
    burner: &str,
    amount_e8s: u128,
    tx_hash: &str,
    block: u64,
) {
    let topics = vec![
        BURN_EVENT_TOPIC0.to_string(),
        word_u128(vault_id as u128),
        word_addr(burner),
    ];
    let data = word_u128(amount_e8s);
    update_any(
        pic,
        mock,
        "push_log",
        Encode!(&topics, &data, &tx_hash.to_string(), &block).unwrap(),
    );
}

/// Register Monad + contract + manual MON price + seed the burn-watch cursor.
/// Returns nothing; leaves the chain ready for the observer to run.
fn configure_chain(pic: &PocketIc, backend: Principal, mock: Principal, seed: u64) {
    decode_result(
        update_dev(pic, backend, "set_evm_rpc_principal", Encode!(&mock).unwrap()),
        "set_evm_rpc_principal",
    )
    .expect("set_evm_rpc_principal");

    let reg = RegisterChainArg {
        chain_id: ChainId(MONAD_CHAIN_ID),
        display_name: "MonadTestnet".to_string(),
        rpc_endpoints: vec!["https://mock-monad-rpc.invalid".to_string()],
        finality_depth: 1,
        gas_strategy: GasStrategy::EvmEip1559 {
            max_priority_fee_gwei: 2,
            max_fee_gwei_ceiling: 500,
        },
        chain_native_decimals: 18,
        // One mock RPC provider: relax the per-chain quorum floor (default 3)
        // to 1 so the mock-backed financial reads satisfy quorum.
        min_quorum_providers: Some(1),
    };
    decode_result(
        update_dev(pic, backend, "register_chain", Encode!(&reg).unwrap()),
        "register_chain",
    )
    .expect("register_chain");

    decode_result(
        update_dev(
            pic,
            backend,
            "set_chain_contract",
            Encode!(
                &ChainId(MONAD_CHAIN_ID),
                &"0x00000000000000000000000000000000deadbeef".to_string()
            )
            .unwrap(),
        ),
        "set_chain_contract",
    )
    .expect("set_chain_contract");

    decode_result(
        update_dev(
            pic,
            backend,
            "set_manual_collateral_price",
            Encode!(&ChainId(MONAD_CHAIN_ID), &"MON".to_string(), &200_000_000u64).unwrap(),
        ),
        "set_manual_collateral_price",
    )
    .expect("set_manual_collateral_price");

    decode_result(
        update_dev(
            pic,
            backend,
            "set_last_observed_block",
            Encode!(&ChainId(MONAD_CHAIN_ID), &seed).unwrap(),
        ),
        "set_last_observed_block",
    )
    .expect("set_last_observed_block");

    // Phase 1c: the burn-watch poll-scan is OFF by default (notify-then-verify is
    // the primary path). These tests exercise the POLL mechanism, so opt in.
    decode_result(
        update_dev(
            pic,
            backend,
            "set_burn_watch_poll_enabled",
            Encode!(&ChainId(MONAD_CHAIN_ID), &true).unwrap(),
        ),
        "set_burn_watch_poll_enabled",
    )
    .expect("set_burn_watch_poll_enabled");
}

fn ecdsa_settlement_addr(pic: &PocketIc, backend: Principal) -> Option<String> {
    match update_dev(
        pic,
        backend,
        "get_chain_settlement_address",
        Encode!(&ChainId(MONAD_CHAIN_ID)).unwrap(),
    ) {
        WasmResult::Reply(b) => match Decode!(&b, Result<String, ProtocolError>) {
            Ok(Ok(addr)) => Some(addr),
            _ => None,
        },
        WasmResult::Reject(_) => None,
    }
}

// ─── The test ────────────────────────────────────────────────────────────────

#[test]
fn phase1b_getlogs_chunks_wide_burn_scan_range() {
    let (pic, backend, mock) = boot();

    // Seed the cursor; the first burn-watch window is (seed, seed + SCAN_WINDOW].
    let seed: u64 = 5_000_000;
    configure_chain(&pic, backend, mock, seed);

    // Chain head one window above the seed PLUS finality_depth (M-07): the cursor
    // advances to the candidate seed + SCAN_WINDOW (win1_finalized) only once that
    // block is buried, i.e. block win1_finalized + 1 also exists. The first tick
    // then scans the full (seed, seed+W] range (a > 100-block range) that trips the
    // mock's 100-block getLogs cap unless the wrapper chunks it.
    let win1_finalized = seed + SCAN_WINDOW;
    update_any(&pic, mock, "set_blocks", Encode!(&(win1_finalized + 1), &(win1_finalized + 1)).unwrap());
    update_any(&pic, mock, "set_next_send_hash", Encode!(&"0xmint1".to_string()).unwrap());

    // A burn placed near the FAR end of the window: > 100 blocks past from_block
    // (seed + 1), so it lives beyond the first 100-block chunk and is only
    // reachable if get_logs pages through the whole range.
    let far_burn_block = win1_finalized - 56; // seed + SCAN_WINDOW - 56

    match ecdsa_settlement_addr(&pic, backend) {
        None => {
            // ── GATED subset (no signing): poison burn at the far end ──────────
            eprintln!("[phase1b getlogs-chunking] ECDSA UNAVAILABLE; running GATED subset (cursor-advance over a >100-block window)");

            // Poison burn cites a non-existent vault → InvalidBurn (skippable) on
            // every tick. It sits at the far end of the > 100-block window, so the
            // cursor can only advance past it if the chunked scan reaches it.
            push_burn_log(&pic, mock, 999_999, "0xdeadbeef", 1_000 * E8, "0xpoison", far_burn_block);
            advance_and_tick(&pic, 4);

            assert!(
                cursor(&pic, backend) >= win1_finalized,
                "chunking: cursor advanced past a >100-block scan window (got {}, want >= {}); pre-fix the un-chunked get_logs 413s and the cursor stalls at the seed {}",
                cursor(&pic, backend),
                win1_finalized,
                seed
            );
            // Supply invariant holds at 0 (nothing minted).
            let global: candid::Nat = query_unit(&pic, backend, "get_global_icusd_supply");
            assert_eq!(global, candid::Nat::from(0u32), "gated: supply invariant holds at 0");
            eprintln!("[phase1b getlogs-chunking] GATED subset PASSED: cursor advanced past the >100-block window via chunked get_logs");
            return;
        }
        Some(settlement_addr) => {
            // ── FULL guard (ECDSA): good burn at the far end is observed once ──
            eprintln!("[phase1b getlogs-chunking] ECDSA AVAILABLE; settlement={settlement_addr}; running FULL guard");

            // Fund the hot wallet so the settlement submit-path gas gate passes.
            update_any(
                &pic,
                mock,
                "set_balance",
                Encode!(&settlement_addr, &candid::Nat::from(1_000u128 * E18)).unwrap(),
            );

            // Stand up an Open vault: 100 MON @ $2 backs 100 icUSD (200% CR).
            let collateral_e18 = 100u128 * E18;
            let debt_e8s = 100u128 * E8;
            let recipient = "0x000000000000000000000000000000000000c0de".to_string();
            let vault_id: u64 = match update_dev(
                &pic,
                backend,
                "open_chain_vault",
                Encode!(
                    &ChainId(MONAD_CHAIN_ID),
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
            let custody = v.custody_address.clone();

            // Deposit lands; observer flips to MintPending + enqueues mint. The
            // mint-confirm path scans a SINGLE block ([b, b], diff 0) — under the
            // 100-block cap — so the mint confirms even with an un-chunked
            // get_logs. Push the Mint log at the finalized block.
            update_any(
                &pic,
                mock,
                "set_balance",
                Encode!(&custody, &candid::Nat::from(collateral_e18)).unwrap(),
            );
            advance_and_tick(&pic, 2);
            push_mint_log(&pic, mock, vault_id, &recipient, debt_e8s, "0xmint1", win1_finalized);
            // M-07: pin the mint receipt to its Mint-log block (win1_finalized = the
            // candidate, buried by the head) so confirm_op finalizes it; the
            // auto-mine would otherwise leave the receipt at the unburied tip.
            advance_and_tick(&pic, 2);
            update_any(&pic, mock, "set_receipt", Encode!(&"0xmint1".to_string(), &true, &win1_finalized).unwrap());
            advance_and_tick(&pic, 2);

            let v = get_vault(&pic, backend, vault_id).expect("vault after mint confirm");
            assert_eq!(v.status, ChainVaultStatus::Open, "mint confirmed => Open");
            assert_eq!(v.debt_e8s, candid::Nat::from(debt_e8s), "mint confirmed => debt 100e8");

            // ── Wide-range burn scan ───────────────────────────────────────────
            // Move to the SECOND window and place a GOOD 40e8 burn near its far
            // end (> 100 blocks past that window's from_block). The mint log was
            // topic-filtered out of the burn scan; clear it to keep the mock tidy.
            let win2_finalized = win1_finalized + SCAN_WINDOW;
            let far_burn_block_2 = win2_finalized - 56;
            update_any(&pic, mock, "clear_logs", Encode!().unwrap());
            // M-07: head one block above the second candidate (win2_finalized) so
            // the cursor advances onto it and scans the (win1_finalized,
            // win2_finalized] window where the far burn sits.
            update_any(&pic, mock, "set_blocks", Encode!(&(win2_finalized + 1), &(win2_finalized + 1)).unwrap());
            push_burn_log(&pic, mock, vault_id, &recipient, 40 * E8, "0xgoodburn", far_burn_block_2);

            advance_and_tick(&pic, 6);

            // Pre-fix: the wide burn scan 413s every tick → burn never seen → debt
            // stays 100e8 and the cursor stalls at the seed. Post-fix: chunked scan
            // reaches the far burn → applied exactly once → debt 60e8.
            let v = get_vault(&pic, backend, vault_id).expect("vault after burn scan");
            assert_eq!(
                v.debt_e8s,
                candid::Nat::from(60 * E8),
                "chunking: far-end 40e8 burn observed via chunked get_logs => debt 60e8 (pre-fix the wide scan 413s and debt stays 100e8)"
            );
            assert_eq!(v.status, ChainVaultStatus::Open, "partial burn keeps vault Open");

            assert!(
                cursor(&pic, backend) >= win2_finalized,
                "chunking: cursor advanced past both >100-block windows; got {}",
                cursor(&pic, backend)
            );

            let global: candid::Nat = query_unit(&pic, backend, "get_global_icusd_supply");
            assert_eq!(
                global,
                candid::Nat::from(60 * E8),
                "chunking: global supply == on-chain truth 60e8 (100e8 minted − 40e8 burned)"
            );
            let audit: SupplyAuditWire = query_unit(&pic, backend, "get_supply_audit");
            assert_eq!(audit.total_e8s, v.debt_e8s, "chunking: chain supply == vault debt == 60e8");

            eprintln!("[phase1b getlogs-chunking] FULL guard PASSED: far-end burn observed via chunked get_logs, debt 60e8, supply == on-chain truth");
        }
    }
}

/// #2b cycle-gate: when no chain-vault has debt, the burn-watch must SKIP the
/// get_logs scan (a Burn can only repay a vault that has debt) while STILL
/// advancing the cursor (so mint-confirm's finality probe stays current).
///
/// The mock is armed with `fail_always("eth_getLogs", ...)` so ANY get_logs call
/// fails. With no vault (total chain-vault debt == 0):
///   - PRE-fix: the observer calls get_logs → fails → returns before advancing →
///     the cursor STALLS at the seed.
///   - POST-fix: the scan is skipped entirely → the cursor advances to the
///     probed finalized height. No ECDSA / signing needed.
#[test]
fn phase1b_burn_watch_skips_getlogs_when_no_debt() {
    let (pic, backend, mock) = boot();

    let seed: u64 = 6_000_000;
    configure_chain(&pic, backend, mock, seed);

    // Arm a PERSISTENT getLogs failure: every eth_getLogs returns an IcError, so
    // if the observer scans at all the cursor cannot advance.
    update_any(
        &pic,
        mock,
        "fail_always",
        Encode!(&"eth_getLogs".to_string(), &"forced getLogs failure (no-debt skip guard)".to_string()).unwrap(),
    );

    // Chain head one window above the seed PLUS finality_depth (M-07): the finality
    // probe (eth_getBlockByNumber, NOT failed) advances the cursor to `finalized`
    // only once that candidate is buried (block finalized + 1 exists), and only IF
    // the scan is skipped.
    let finalized = seed + SCAN_WINDOW;
    let head = finalized + 1;
    update_any(&pic, mock, "set_blocks", Encode!(&head, &head).unwrap());

    // No vault opened → total chain-vault debt == 0.
    advance_and_tick(&pic, 4);

    assert!(
        cursor(&pic, backend) >= finalized,
        "no-debt fast path: cursor advanced to {} without scanning (got {}); pre-fix the forced get_logs failure stalls it at seed {}",
        finalized,
        cursor(&pic, backend),
        seed
    );
    let global: candid::Nat = query_unit(&pic, backend, "get_global_icusd_supply");
    assert_eq!(global, candid::Nat::from(0u32), "no mint => supply 0");
    eprintln!("[phase1b getlogs-chunking] no-debt skip PASSED: cursor advanced with get_logs forced-failing (scan skipped)");
}
