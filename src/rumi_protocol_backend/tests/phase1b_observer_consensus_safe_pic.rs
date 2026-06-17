//! Phase 1b — Layer-2 consensus-safe observer regression test (Gate-3 fix).
//!
//! This is the focused regression for the EVM-RPC Layer-2 failure that blocked
//! Gate-4 on mainnet-staging: a VOLATILE block-head read (`eth_blockNumber`, or
//! any `latest`/`finalized` BLOCK TAG) differs across the EVM RPC canister's
//! subnet replicas on a fast-finality chain like Monad → IC HTTPS-outcall
//! consensus never agrees → the call fails EVERY tick. The fix (Task A2a) reads
//! finalized height by probing a SPECIFIC, already-final block NUMBER via the
//! typed `eth_getBlockByNumber(Number(N))` (byte-identical across replicas), and
//! (Task A2b) decouples deposit-watch from the block-height path so a block-read
//! failure degrades the observer to deposit-only instead of aborting the tick.
//!
//! To prove the backend never falls back to the volatile read, the mock is armed
//! with `fail_always("eth_blockNumber", ...)`: EVERY `request` call whose
//! JSON-RPC method is "eth_blockNumber" returns a real-wire-shaped `IcError`
//! for the entire test run. Using the PERSISTENT `fail_always` (not the
//! one-shot `fail_next`) is important: a reverted backend would error on tick 1
//! (one-shot consumed) then succeed on tick 2+ and advance the cursor anyway,
//! causing a false PASS. With `fail_always`, every tick that calls
//! `eth_blockNumber` via `request` fails permanently, so the cursor can only
//! advance via the TYPED `eth_getBlockByNumber` probe. The test passing proves
//! the backend ONLY uses the consensus-safe typed probe. Reverting to the
//! volatile read re-breaks this test reliably across all ticks.
//!
//! Two subsets (same auto-upgrade pattern as the happy-path test):
//!   - ECDSA available: open a vault, fund custody, tick → assert the vault
//!     reaches `MintPending` (deposit-watch ran despite `eth_blockNumber` being
//!     "broken") AND the burn-watch cursor advanced to seed+1024 via the typed
//!     probe.
//!   - ECDSA unavailable: assert the burn-watch cursor advances to seed+1024 anyway
//!     (the cursor advance needs no signing) and the supply invariant holds at 0.

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

/// Minimal mirror of `ProtocolError`: these chain endpoints only ever return the
/// `ChainAdmin(text)` variant on the happy path.
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
/// The cursor is seeded here; finalized = SEED + 1024 (MAX_BLOCK_SCAN_WINDOW), so
/// one observer tick advances the cursor to SEED_PLUS_WINDOW via the typed probe.
const SEED_BLOCK: u64 = 2_000_000;
const SEED_PLUS_WINDOW: u64 = 2_001_024;

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

/// Read the burn-watch cursor for the Monad chain via `get_last_observed_block`.
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

// ─── The test ────────────────────────────────────────────────────────────────

#[test]
fn phase1b_observer_consensus_safe_cursor_advances() {
    let (pic, backend, mock) = boot();

    // Point the backend's Monad wrapper at the mock.
    decode_result(
        update_dev(&pic, backend, "set_evm_rpc_principal", Encode!(&mock).unwrap()),
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
        // One mock RPC provider: relax the per-chain quorum floor (default 3)
        // to 1 so the mock-backed financial reads satisfy quorum.
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
            Encode!(&ChainId(MONAD_CHAIN_ID), &"0x00000000000000000000000000000000deadbeef".to_string())
                .unwrap(),
        ),
        "set_chain_contract",
    )
    .expect("set_chain_contract");

    decode_result(
        update_dev(
            &pic,
            backend,
            "set_manual_collateral_price",
            Encode!(&ChainId(MONAD_CHAIN_ID), &"MON".to_string(), &200_000_000u64).unwrap(),
        ),
        "set_manual_collateral_price",
    )
    .expect("set_manual_collateral_price");

    // ── Break the volatile read (persistent barrier) ─────────────────────────
    // Arm a PERSISTENT real-wire IcError for eth_blockNumber via `fail_always`.
    // Every `request` call whose JSON-RPC method is "eth_blockNumber" will fail
    // with IcError(SysTransient) for the lifetime of this test — not just the
    // first one (which is all `fail_next` would cover).
    //
    // Why `fail_always` beats `fail_next` here: a reverted backend (volatile
    // read) would error on tick 1 from `fail_next`, then succeed on tick 2
    // (arming already consumed) and advance the cursor anyway — the test would
    // pass even on the broken implementation. With `fail_always`, EVERY tick
    // that calls `eth_blockNumber` via `request` fails, so the cursor can NEVER
    // advance on the broken path. The test ONLY passes when the backend uses the
    // TYPED `eth_getBlockByNumber` method, which bypasses `request` entirely and
    // is unaffected by this arming.
    update_any(
        &pic,
        mock,
        "fail_always",
        Encode!(&"eth_blockNumber".to_string(), &"no consensus".to_string()).unwrap(),
    );

    // Seed the burn-watch cursor and set the chain head one window (1024) above it.
    decode_result(
        update_dev(
            &pic,
            backend,
            "set_last_observed_block",
            Encode!(&ChainId(MONAD_CHAIN_ID), &SEED_BLOCK).unwrap(),
        ),
        "set_last_observed_block",
    )
    .expect("set_last_observed_block");
    // M-07 (FINAL-1): fetch_block_numbers advances the cursor to a candidate
    // block (SEED + MAX_BLOCK_SCAN_WINDOW = SEED_PLUS_WINDOW) ONLY once that
    // candidate is buried under finality_depth confirmations, i.e. block
    // `candidate + finality_depth` also exists. finality_depth is 1 here, so the
    // chain head must sit one block ABOVE SEED_PLUS_WINDOW for the cursor to
    // advance to SEED_PLUS_WINDOW. Setting the head exactly at SEED_PLUS_WINDOW
    // (the pre-M-07 expectation) leaves the candidate unfinalized and stalls.
    update_any(
        &pic,
        mock,
        "set_blocks",
        Encode!(&(SEED_PLUS_WINDOW + 1), &(SEED_PLUS_WINDOW + 1)).unwrap(),
    );

    assert_eq!(cursor(&pic, backend), SEED_BLOCK, "cursor seeded to SEED_BLOCK");

    // ── ECDSA probe: decide full vs gated ────────────────────────────────────
    let ecdsa_ok = match update_dev(
        &pic,
        backend,
        "get_chain_settlement_address",
        Encode!(&ChainId(MONAD_CHAIN_ID)).unwrap(),
    ) {
        WasmResult::Reply(b) => match Decode!(&b, Result<String, ProtocolError>) {
            Ok(Ok(addr)) => Some(addr),
            _ => None,
        },
        WasmResult::Reject(_) => None,
    };

    if let Some(settlement_addr) = ecdsa_ok {
        eprintln!("[phase1b consensus-safe] ECDSA AVAILABLE; running FULL subset (deposit-watch + cursor advance)");

        // Fund the settlement (hot-wallet) address so the gas gate passes.
        update_any(
            &pic,
            mock,
            "set_balance",
            Encode!(&settlement_addr, &candid::Nat::from(1_000u128 * E18)).unwrap(),
        );

        // Open a vault (AwaitingDeposit, no mint).
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
        assert_eq!(v.status, ChainVaultStatus::AwaitingDeposit, "open => AwaitingDeposit");
        let custody = v.custody_address.clone();

        // Fund the custody address so the deposit-watch verifies on the next tick.
        update_any(
            &pic,
            mock,
            "set_balance",
            Encode!(&custody, &candid::Nat::from(collateral_e18)).unwrap(),
        );

        advance_and_tick(&pic, 3);

        // Deposit-watch ran DESPITE eth_blockNumber being "broken": the vault must
        // have flipped to MintPending (consensus-safe balance-only path).
        let v = get_vault(&pic, backend, vault_id).expect("vault after tick");
        assert_eq!(
            v.status,
            ChainVaultStatus::MintPending,
            "deposit-watch ran despite broken eth_blockNumber => MintPending"
        );

        // The burn-watch cursor advanced via the consensus-safe TYPED probe (not
        // the volatile eth_blockNumber, which is armed to fail).
        assert_eq!(
            cursor(&pic, backend),
            SEED_PLUS_WINDOW,
            "cursor advanced to SEED+1024 via consensus-safe typed probe"
        );
    } else {
        eprintln!("[phase1b consensus-safe] ECDSA UNAVAILABLE; running GATED subset (cursor advance only — needs no signing)");

        // No vault, no signing — the burn-watch cursor advance is purely the typed
        // block probe + an (empty) getLogs scan. Tick and assert it advanced.
        advance_and_tick(&pic, 3);

        assert_eq!(
            cursor(&pic, backend),
            SEED_PLUS_WINDOW,
            "cursor advanced to SEED+1024 via consensus-safe typed probe (no signing needed)"
        );

        // Supply invariant holds at 0 (nothing minted).
        let global: candid::Nat = query_unit(&pic, backend, "get_global_icusd_supply");
        assert_eq!(global, candid::Nat::from(0u32), "supply invariant holds at 0");
    }

    eprintln!("[phase1b consensus-safe] PASSED: observer advanced the burn-watch cursor without ever reading the volatile eth_blockNumber");
}
