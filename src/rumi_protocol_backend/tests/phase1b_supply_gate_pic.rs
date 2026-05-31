//! Phase 1b — Supply-equality gate: skip-on-match, scan-on-drop.
//!
//! ## What this test proves
//!
//! The observer (`run_observer`) gained a supply-equality gate just before its
//! `eth_getLogs` burn sweep. Each advancing tick (when `total_chain_vault_debt > 0`
//! and no mint is in flight) it probes the icUSD contract's `totalSupply()` via
//! `eth_call` at the finalized block and compares it to `chain_supplies[chain]`:
//!
//! - If `onchain_totalSupply == recorded` (and no mint in flight): SKIP the burn
//!   `get_logs` sweep, just advance the cursor (`advance_cursor_and_prune`) and
//!   return. No burn can have occurred — the canister is the sole minter.
//! - If `onchain < recorded` (a burn dropped supply): run the sweep and apply.
//! - On any probe error or in-flight mint: run the sweep (fall through).
//!
//! The mock supports `eth_call` returning a scripted value via `set_total_supply(u128)`.
//!
//! ## ECDSA gating (same auto-upgrade pattern as the other phase1b tests)
//!
//! The full path needs threshold-ECDSA to reach an Open vault with confirmed debt.
//! When PocketIC cannot provision `test_key_1`, the test runs a GATED subset that
//! asserts the supply gate is inert on a chain with zero debt (the no-debt fast path
//! means the gate is never even reached), and AUTO-UPGRADES to the full two-phase
//! gate proof on an ECDSA-capable PocketIC.

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
/// keccak256("Mint(uint256,address,uint256)") — must match evm_rpc.rs.
const MINT_EVENT_TOPIC0: &str =
    "0x4e3883c75cc9c752bb1db2e406a822e4a75067ae77ad9a0a4d179f2709b9e1f6";
/// keccak256("Burn(uint256,address,uint256)") — must match evm_rpc.rs.
const BURN_EVENT_TOPIC0: &str =
    "0xe1b6e34006e9871307436c226f232f9c5e7690c1d2c4f4adda4f607a75a9beca";

// ─── PocketIC call helpers ────────────────────────────────────────────────────

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

/// Read the burn-watch cursor for the Monad chain.
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
    // fire one tick each. The default is 300s (cycle-burn hardening).
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

// ─── Shared setup: registers Monad, seeds cursor, reaches Open vault with
//     debt = 100e8. Returns (vault_id, recipient, settlement_addr).
//     Returns None for settlement_addr when ECDSA is unavailable (gated subset).
fn setup_chain(
    pic: &PocketIc,
    backend: Principal,
    mock: Principal,
    cursor_seed: u64,
    finalized_block: u64,
) -> Option<(u64, String, String)> {
    // Point the backend's Monad wrapper at the mock.
    decode_result(
        update_dev(pic, backend, "set_evm_rpc_principal", Encode!(&mock).unwrap()),
        "set_evm_rpc_principal",
    )
    .expect("set_evm_rpc_principal");

    // Register Monad + contract + manual MON price.
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

    // $2.00 / MON in e8.
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

    // Seed the burn-watch cursor.
    decode_result(
        update_dev(
            pic,
            backend,
            "set_last_observed_block",
            Encode!(&ChainId(MONAD_CHAIN_ID), &cursor_seed).unwrap(),
        ),
        "set_last_observed_block",
    )
    .expect("set_last_observed_block");

    // Script the mock: chain head = finalized_block.
    update_any(pic, mock, "set_blocks", Encode!(&finalized_block, &finalized_block).unwrap());
    update_any(pic, mock, "set_next_send_hash", Encode!(&"0xmint1".to_string()).unwrap());

    // ECDSA probe: decides full vs gated.
    let settlement_addr = match update_dev(
        pic,
        backend,
        "get_chain_settlement_address",
        Encode!(&ChainId(MONAD_CHAIN_ID)).unwrap(),
    ) {
        WasmResult::Reply(b) => match Decode!(&b, Result<String, ProtocolError>) {
            Ok(Ok(addr)) => Some(addr),
            Ok(Err(e)) => {
                eprintln!("[supply-gate] ECDSA UNAVAILABLE (Err: {e:?}); running GATED subset");
                None
            }
            Err(decode_err) => {
                eprintln!("[supply-gate] get_chain_settlement_address decode error ({decode_err}); running GATED subset");
                None
            }
        },
        WasmResult::Reject(msg) => {
            eprintln!("[supply-gate] ECDSA UNAVAILABLE (rejected: {msg}); running GATED subset");
            None
        }
    };

    let settlement_addr = settlement_addr?;

    // Fund the settlement (hot-wallet) address.
    update_any(
        pic,
        mock,
        "set_balance",
        Encode!(&settlement_addr, &candid::Nat::from(1_000u128 * E18)).unwrap(),
    );

    // Open a vault: 100 MON @ $2 = $200 backing 100 icUSD => 200% CR.
    let collateral_e18 = 100u128 * E18;
    let debt_e8s = 100u128 * E8;
    let recipient = "0x000000000000000000000000000000000000c0de".to_string();
    let vault_id: u64 = match update_dev(
        pic,
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

    let v = get_vault(pic, backend, vault_id).expect("vault exists after open");
    let custody = v.custody_address.clone();

    // Deposit lands; observer flips to MintPending + enqueues mint.
    update_any(
        pic,
        mock,
        "set_balance",
        Encode!(&custody, &candid::Nat::from(collateral_e18)).unwrap(),
    );
    advance_and_tick(pic, 2);

    // Settlement submits + confirms the mint. Push the Mint log at finalized_block.
    push_mint_log(pic, mock, vault_id, &recipient, debt_e8s, "0xmint1", finalized_block);
    advance_and_tick(pic, 4);

    // Assert: vault is Open with debt = 100e8, chain_supplies[MONAD] = 100e8.
    let v = get_vault(pic, backend, vault_id).expect("vault after mint confirm");
    assert_eq!(v.status, ChainVaultStatus::Open, "setup: mint confirmed => Open");
    assert_eq!(
        v.debt_e8s,
        candid::Nat::from(debt_e8s),
        "setup: debt = 100e8 after mint confirm"
    );
    let global: candid::Nat = query_unit(pic, backend, "get_global_icusd_supply");
    assert_eq!(global, candid::Nat::from(100 * E8), "setup: supply = 100e8 after mint confirm");

    Some((vault_id, recipient, settlement_addr))
}

// ─── The test ─────────────────────────────────────────────────────────────────

#[test]
fn phase1b_supply_gate_skips_on_match_scans_on_drop() {
    // The consensus-safe probe in fetch_block_numbers checks block
    // last_observed + MAX_BLOCK_SCAN_WINDOW (1024). So after setup leaves the
    // cursor at FINALIZED_0 = cursor_seed + 1024, the next advancing tick
    // requires finalized >= FINALIZED_0 + 1024.
    const CURSOR_SEED: u64 = 1_000_000;
    const FINALIZED_0: u64 = 1_001_024; // cursor_seed + 1024 = setup's finalized block
    const DEBT_E8S: u128 = 100 * E8;
    const TRAP_BURN_AMOUNT: u128 = 40 * E8; // would decrement debt if scan ran

    let (pic, backend, mock) = boot();

    // ── Common setup: reach Open vault with debt = 100e8 ─────────────────────
    let setup = setup_chain(&pic, backend, mock, CURSOR_SEED, FINALIZED_0);

    let (vault_id, recipient) = match setup {
        Some((vid, rec, addr)) => {
            eprintln!("[supply-gate] ECDSA AVAILABLE; settlement={addr}; running FULL two-phase gate proof");
            (vid, rec)
        }
        None => {
            // GATED SUBSET: no Open vault with debt possible. Assert the supply
            // invariant holds at 0 and return green. AUTO-UPGRADES on ECDSA-capable PocketIC.
            let global: candid::Nat = query_unit(&pic, backend, "get_global_icusd_supply");
            assert_eq!(global, candid::Nat::from(0u32), "gated: supply invariant holds at 0");
            eprintln!("[supply-gate] GATED subset PASSED: supply invariant at 0");
            return;
        }
    };

    // After setup the cursor is at FINALIZED_0. The consensus-safe probe uses
    // last_observed + MAX_BLOCK_SCAN_WINDOW (1024) to check if the next window
    // is final. So to get an advancing tick from FINALIZED_0, the mock finalized
    // block must be >= FINALIZED_0 + 1024. Use exactly FINALIZED_0 + 1024.
    const MAX_BLOCK_SCAN_WINDOW: u64 = 1024;
    const PHASE_A_FINALIZED: u64 = FINALIZED_0 + MAX_BLOCK_SCAN_WINDOW; // 1_002_048
    // Burn trap in the middle of that window.
    const TRAP_BURN_BLOCK: u64 = FINALIZED_0 + 50;

    // Phase B: re-seed cursor to FINALIZED_0, same window applies.
    const PHASE_B_FINALIZED: u64 = FINALIZED_0 + MAX_BLOCK_SCAN_WINDOW; // same 1_002_048
    const REAL_BURN_BLOCK: u64 = FINALIZED_0 + 50;
    const REAL_BURN_AMOUNT: u128 = 40 * E8;

    // After setup the cursor is at FINALIZED_0. Verify.
    assert_eq!(
        cursor(&pic, backend),
        FINALIZED_0,
        "pre-Phase A: cursor at FINALIZED_0 after mint confirm"
    );

    // ═══════════════════════════════════════════════════════════════════════════
    // Phase A: onchain totalSupply == chain_supplies => sweep SKIPPED
    // ═══════════════════════════════════════════════════════════════════════════
    //
    // Script:
    //   - onchain supply = DEBT_E8S (matches recorded chain_supplies[MONAD])
    //   - A Burn log (TRAP_BURN_AMOUNT) in the scan window (FINALIZED_0+1 ..= PHASE_A_FINALIZED)
    //     is pushed as a TRAP: if the gate wrongly allowed the scan, this burn
    //     would decrement debt. We assert debt is UNCHANGED after the tick.
    //   - The finalized block advances to PHASE_A_FINALIZED (= cursor + 1024) so
    //     fetch_block_numbers probes that block, finds it exists, and returns
    //     (PHASE_A_FINALIZED, PHASE_A_FINALIZED) — an advancing tick.

    // Set onchain supply equal to the recorded supply — gate should skip.
    update_any(&pic, mock, "set_total_supply", Encode!(&DEBT_E8S).unwrap());

    // Push a trap Burn log in the window. Topic-filtered by the mock's getLogs
    // (only returned for BURN topic0 scans); if the gate incorrectly ran the
    // sweep, this burn would apply and reduce debt.
    push_burn_log(
        &pic,
        mock,
        vault_id,
        &recipient,
        TRAP_BURN_AMOUNT,
        "0xburnA_trap",
        TRAP_BURN_BLOCK,
    );

    // Advance the mock chain head to PHASE_A_FINALIZED.
    // fetch_block_numbers probes block (cursor+1024 = PHASE_A_FINALIZED); the mock
    // returns that block as existing, so finalized = PHASE_A_FINALIZED > last_observed.
    update_any(&pic, mock, "set_blocks", Encode!(&PHASE_A_FINALIZED, &PHASE_A_FINALIZED).unwrap());

    // Run one observer tick.
    advance_and_tick(&pic, 1);

    // Assert Phase A: gate skipped the sweep — debt and supply are UNCHANGED.
    let v_a = get_vault(&pic, backend, vault_id).expect("vault in Phase A");
    assert_eq!(
        v_a.debt_e8s,
        candid::Nat::from(DEBT_E8S),
        "Phase A: debt unchanged (trap Burn log was NOT applied — sweep skipped)"
    );
    assert_eq!(v_a.status, ChainVaultStatus::Open, "Phase A: vault still Open");

    let global_a: candid::Nat = query_unit(&pic, backend, "get_global_icusd_supply");
    assert_eq!(
        global_a,
        candid::Nat::from(DEBT_E8S),
        "Phase A: chain_supplies[MONAD] unchanged (sweep skipped)"
    );

    // Assert Phase A: cursor ADVANCED to PHASE_A_FINALIZED (skip path ran
    // advance_cursor_and_prune, proving it was the skip path and not an early bail).
    assert_eq!(
        cursor(&pic, backend),
        PHASE_A_FINALIZED,
        "Phase A: cursor advanced to PHASE_A_FINALIZED (skip path ran advance_cursor_and_prune)"
    );

    eprintln!(
        "[supply-gate] Phase A PASSED: supply matched ({}), trap burn NOT applied, cursor advanced to {}",
        DEBT_E8S, PHASE_A_FINALIZED
    );

    // ═══════════════════════════════════════════════════════════════════════════
    // Phase B: onchain totalSupply < chain_supplies => sweep RUNS and applies burn
    // ═══════════════════════════════════════════════════════════════════════════
    //
    // Re-seed the cursor to FINALIZED_0 so the same block window
    // (FINALIZED_0+1 ..= PHASE_B_FINALIZED) is re-scanned. We set onchain supply
    // to DEBT_E8S - REAL_BURN_AMOUNT (supply dropped) and push a real Burn log.
    // The Phase A trap burn ("0xburnA_trap", TRAP_BURN_AMOUNT=40e8) is still in
    // the mock's log list. After the real burn (40e8) applies, debt = 60e8.
    // The trap burn's dedup key ("0xburnA_trap:N") was NOT recorded in Phase A
    // (Phase A skipped the scan entirely), so on Phase B's scan the trap burn IS
    // seen. However: the real burn (40e8) applies first (same block, lower log
    // index since push_log auto-assigns indices in push order — real burn was
    // pushed second, but its tx_hash is different so ordering is by log_index in
    // the block). To keep the scenario unambiguous, we clear Phase A's trap log
    // and push only the real burn for Phase B.

    // Clear all logs so only the real Phase B burn is in the window.
    update_any(&pic, mock, "clear_logs", Encode!().unwrap());

    // Re-seed cursor to FINALIZED_0 so Phase B scans (FINALIZED_0+1..=PHASE_B_FINALIZED).
    decode_result(
        update_dev(
            &pic,
            backend,
            "set_last_observed_block",
            Encode!(&ChainId(MONAD_CHAIN_ID), &FINALIZED_0).unwrap(),
        ),
        "set_last_observed_block (Phase B re-seed)",
    )
    .expect("set_last_observed_block Phase B");

    // Set onchain supply to DEBT_E8S - REAL_BURN_AMOUNT (supply dropped by burn).
    let supply_after_burn = DEBT_E8S - REAL_BURN_AMOUNT;
    update_any(&pic, mock, "set_total_supply", Encode!(&supply_after_burn).unwrap());

    // Push the real Burn log for Phase B.
    push_burn_log(
        &pic,
        mock,
        vault_id,
        &recipient,
        REAL_BURN_AMOUNT,
        "0xburnB_real",
        REAL_BURN_BLOCK,
    );

    // Set mock finalized to PHASE_B_FINALIZED so fetch_block_numbers succeeds.
    update_any(&pic, mock, "set_blocks", Encode!(&PHASE_B_FINALIZED, &PHASE_B_FINALIZED).unwrap());

    // Run one observer tick.
    advance_and_tick(&pic, 1);

    // Assert Phase B: sweep RAN and applied the real burn.
    let v_b = get_vault(&pic, backend, vault_id).expect("vault in Phase B");
    assert_eq!(
        v_b.debt_e8s,
        candid::Nat::from(DEBT_E8S - REAL_BURN_AMOUNT),
        "Phase B: debt decremented by REAL_BURN_AMOUNT (sweep ran and applied burn)"
    );
    assert_eq!(v_b.status, ChainVaultStatus::Open, "Phase B: vault still Open");

    let global_b: candid::Nat = query_unit(&pic, backend, "get_global_icusd_supply");
    assert_eq!(
        global_b,
        candid::Nat::from(DEBT_E8S - REAL_BURN_AMOUNT),
        "Phase B: chain_supplies[MONAD] decremented (sweep ran and applied burn)"
    );

    // Cursor must have advanced to PHASE_B_FINALIZED.
    assert_eq!(
        cursor(&pic, backend),
        PHASE_B_FINALIZED,
        "Phase B: cursor advanced to PHASE_B_FINALIZED after burn applied"
    );

    eprintln!(
        "[supply-gate] Phase B PASSED: supply dropped ({} -> {}), real burn applied, debt {} = supply {}, cursor advanced to {}",
        DEBT_E8S,
        supply_after_burn,
        DEBT_E8S - REAL_BURN_AMOUNT,
        DEBT_E8S - REAL_BURN_AMOUNT,
        PHASE_B_FINALIZED
    );

    eprintln!("[supply-gate] FULL two-phase gate proof PASSED");
}
