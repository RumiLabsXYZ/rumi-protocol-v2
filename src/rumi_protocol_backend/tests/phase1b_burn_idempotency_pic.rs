//! Phase 1b — C-1 regression: idempotent burn application + skip-poison-and-continue.
//!
//! ## The bug this guards (C-1, supply-divergence)
//!
//! The burn-watch loop in `chains/monad/deposit_watch.rs::run_observer` used to
//! `break` on the FIRST `apply_burn_to_state` failure and advance the cursor only
//! when EVERY burn in the range applied. A single PERMANENT-INVALID burn — a
//! permissionless `Burn(vault_id)` over-repaying a vault's remaining debt, or
//! citing a closed vault (both PUBLIC per IcUSD.sol) — would then STALL the cursor
//! and force the WHOLE range to re-scan every tick. Because `apply_burn_to_state`
//! had NO idempotency, any already-applied PARTIAL burn in that range got applied
//! AGAIN on the re-scan: `debt_e8s` and `chain_supplies` BOTH decremented a second
//! time. Since they move together the Timer-B self-check (which compares
//! `sum(chain_supplies)` to `total_chain_vault_debt`) stayed satisfied and NEVER
//! fired — tracked supply silently diverged from on-chain truth.
//!
//! ## What this test proves
//!
//! Drive a vault to Open with debt=100e8 (supply=100e8) through the full
//! open → deposit → mint path (same harness as the happy-path test). Then push
//! TWO burn logs at the SAME finalized block into the burn-watch range:
//!   - a GOOD partial burn (40e8) — valid, should apply EXACTLY once → debt 60e8
//!   - a POISON burn (1_000e8 ≫ remaining debt) — `apply_burn_to_state` rejects it
//!     as InvalidBurn (over-repay) on EVERY tick.
//! Then advance SEVERAL observer ticks (so the range is re-scanned multiple times).
//!
//! Assertions (all would FAIL on the pre-fix code):
//!   (a) the good burn's debt decrement happened EXACTLY ONCE — debt == 60e8
//!       (pre-fix: re-scan re-applies the 40e8 burn → 20e8, then underflow-rejects
//!        further, but supply also double-decrements → silent divergence).
//!   (b) the burn-watch cursor ADVANCED past the poison block (it no longer stalls).
//!   (c) `get_global_icusd_supply == sum(vault debt) == 60e8` — supply matches
//!       on-chain truth (100e8 minted − 40e8 burned), not a doubly-decremented value.
//!
//! ## ECDSA gating (same auto-upgrade pattern as the happy-path test)
//!
//! The full path needs threshold-ECDSA (open/mint/settlement). When PocketIC
//! cannot provision `test_key_1`, the C-1 reproduction cannot be set up (no Open
//! vault with debt), so the test runs a GATED subset that still asserts the supply
//! invariant holds at 0 and the burn-watch cursor advances past a poison-only range
//! (which needs no signing). The gated subset AUTO-UPGRADES to the full C-1 guard
//! on an ECDSA-capable PocketIC.

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

/// Push a Burn log with an explicit log_index (for the same-tx-different-log-index test).
fn push_burn_log_at(
    pic: &PocketIc,
    mock: Principal,
    vault_id: u64,
    burner: &str,
    amount_e8s: u128,
    tx_hash: &str,
    block: u64,
    log_index: u64,
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
        "push_log_at",
        Encode!(&topics, &data, &tx_hash.to_string(), &block, &log_index).unwrap(),
    );
}

// ─── The test ────────────────────────────────────────────────────────────────

#[test]
fn phase1b_burn_idempotency_skips_poison_and_never_double_applies() {
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
            &pic,
            backend,
            "set_manual_collateral_price",
            Encode!(&ChainId(MONAD_CHAIN_ID), &"MON".to_string(), &200_000_000u64).unwrap(),
        ),
        "set_manual_collateral_price",
    )
    .expect("set_manual_collateral_price");

    // Seed the burn-watch cursor well above 256 so the small legacy block numbers
    // never apply; one tick advances the cursor by MAX_BLOCK_SCAN_WINDOW (256).
    decode_result(
        update_dev(
            &pic,
            backend,
            "set_last_observed_block",
            Encode!(&ChainId(MONAD_CHAIN_ID), &1_000_000u64).unwrap(),
        ),
        "set_last_observed_block",
    )
    .expect("set_last_observed_block");

    // Chain head = seed + 256 so the first tick advances 1_000_000 -> 1_000_256.
    update_any(&pic, mock, "set_blocks", Encode!(&1_000_256u64, &1_000_256u64).unwrap());
    update_any(&pic, mock, "set_next_send_hash", Encode!(&"0xmint1".to_string()).unwrap());

    // ── ECDSA probe: decide full vs gated ────────────────────────────────────
    let settlement_addr = match update_dev(
        &pic,
        backend,
        "get_chain_settlement_address",
        Encode!(&ChainId(MONAD_CHAIN_ID)).unwrap(),
    ) {
        WasmResult::Reply(b) => match Decode!(&b, Result<String, ProtocolError>) {
            Ok(Ok(addr)) => Some(addr),
            Ok(Err(e)) => {
                eprintln!("[phase1b burn-idempotency] ECDSA UNAVAILABLE (Err: {e:?}); running GATED subset");
                None
            }
            Err(decode_err) => {
                eprintln!("[phase1b burn-idempotency] get_chain_settlement_address decode error ({decode_err}); running GATED subset");
                None
            }
        },
        WasmResult::Reject(msg) => {
            eprintln!("[phase1b burn-idempotency] ECDSA UNAVAILABLE (rejected: {msg}); running GATED subset");
            None
        }
    };

    let settlement_addr = match settlement_addr {
        Some(addr) => {
            eprintln!("[phase1b burn-idempotency] ECDSA AVAILABLE; settlement address = {addr}; running FULL C-1 guard");
            addr
        }
        None => {
            // GATED SUBSET: cannot mint without ECDSA, so we cannot stand up an Open
            // vault with debt. Instead prove the cursor advances past a POISON-only
            // burn range (the core skip-poison-and-continue guarantee, which needs
            // no signing). The poison cites a non-existent vault → InvalidBurn on
            // every tick; pre-fix this stalled the cursor forever.
            push_burn_log(&pic, mock, 4242, "0xdeadbeef", 1_000e8 as u128, "0xpoison", 1_000_100);
            advance_and_tick(&pic, 4);

            assert!(
                cursor(&pic, backend) >= 1_000_256,
                "gated: cursor advanced past poison-only range (skip-poison-and-continue); got {}",
                cursor(&pic, backend)
            );
            // Supply invariant holds at 0 (nothing minted).
            let global: candid::Nat = query_unit(&pic, backend, "get_global_icusd_supply");
            assert_eq!(global, candid::Nat::from(0u32), "gated: supply invariant holds at 0");
            eprintln!("[phase1b burn-idempotency] GATED subset PASSED: cursor advanced past poison-only range");
            return;
        }
    };

    // ════════════════════════════════════════════════════════════════════════
    // FULL C-1 GUARD (ECDSA available)
    // ════════════════════════════════════════════════════════════════════════

    // Fund the settlement (hot-wallet) address so the submit-path gas gate passes.
    update_any(
        &pic,
        mock,
        "set_balance",
        Encode!(&settlement_addr, &candid::Nat::from(1_000u128 * E18)).unwrap(),
    );

    // ── Stand up an Open vault with debt=100e8, supply=100e8 ─────────────────
    // 100 MON @ $2 = $200 backing 100 icUSD => 200% CR.
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

    // Deposit lands; observer flips to MintPending + enqueues mint.
    update_any(
        &pic,
        mock,
        "set_balance",
        Encode!(&custody, &candid::Nat::from(collateral_e18)).unwrap(),
    );
    advance_and_tick(&pic, 2);

    // Settlement submits + confirms the mint. Push the Mint log at the finalized
    // block so the confirm path reads the on-chain amount.
    push_mint_log(&pic, mock, vault_id, &recipient, debt_e8s, "0xmint1", 1_000_256);
    advance_and_tick(&pic, 4);

    let v = get_vault(&pic, backend, vault_id).expect("vault after mint confirm");
    assert_eq!(v.status, ChainVaultStatus::Open, "mint confirmed => Open");
    assert_eq!(v.debt_e8s, candid::Nat::from(debt_e8s), "mint confirmed => debt 100e8");
    let global: candid::Nat = query_unit(&pic, backend, "get_global_icusd_supply");
    assert_eq!(global, candid::Nat::from(100 * E8), "supply 100e8 after mint");

    // ── Set up the C-1 scenario ──────────────────────────────────────────────
    // Clear the Mint log (production topic-filters it out of the burn scan; we
    // model that exclusion). Then push TWO burns at the SAME finalized block:
    //   1. GOOD partial burn 40e8 (valid)
    //   2. POISON burn 1_000e8 (over-repay: ≫ remaining 60e8) — InvalidBurn forever
    // Both land in the (1_000_256, 1_000_512] scan window. The poison sits AFTER
    // the good burn in block/log order, so the pre-fix loop applies the good burn,
    // then hits the poison, sets burn_ok=false, and breaks WITHOUT advancing the
    // cursor — forcing the whole range to re-scan every tick and re-apply the 40e8.
    update_any(&pic, mock, "clear_logs", Encode!().unwrap());
    update_any(&pic, mock, "set_blocks", Encode!(&1_000_512u64, &1_000_512u64).unwrap());
    // Same block (1_000_300) for both; the good burn first, poison second.
    push_burn_log(&pic, mock, vault_id, &recipient, 40 * E8, "0xgoodburn", 1_000_300);
    push_burn_log(&pic, mock, vault_id, "0xattacker", 1_000 * E8, "0xpoisonburn", 1_000_300);

    // Advance SEVERAL ticks so the range is (re-)scanned multiple times. On the
    // PRE-FIX code, each re-scan re-applies the 40e8 good burn and double-decrements
    // supply; on the FIXED code, the good burn is deduped (applied exactly once) and
    // the poison is skip-logged so the cursor advances past it.
    advance_and_tick(&pic, 6);

    // (a) The good burn's debt decrement happened EXACTLY ONCE: debt == 60e8.
    let v = get_vault(&pic, backend, vault_id).expect("vault after burn scenario");
    assert_eq!(
        v.debt_e8s,
        candid::Nat::from(60 * E8),
        "C-1: good 40e8 burn applied EXACTLY once => debt 60e8 (not double-applied)"
    );
    assert_eq!(v.status, ChainVaultStatus::Open, "partial burn keeps vault Open");

    // (b) The burn-watch cursor ADVANCED past the poison block (no longer stalls).
    assert!(
        cursor(&pic, backend) >= 1_000_512,
        "C-1: cursor advanced past the poison burn (skip-poison-and-continue); got {}",
        cursor(&pic, backend)
    );

    // (c) Supply matches on-chain truth: 100e8 minted − 40e8 burned = 60e8, AND
    //     equals sum(vault debt). This is the invariant the silent double-apply
    //     used to violate while keeping supply==debt (both decremented twice).
    let global: candid::Nat = query_unit(&pic, backend, "get_global_icusd_supply");
    assert_eq!(
        global,
        candid::Nat::from(60 * E8),
        "C-1: global supply == on-chain truth 60e8 (no silent double-decrement)"
    );
    let audit: SupplyAuditWire = query_unit(&pic, backend, "get_supply_audit");
    assert_eq!(
        audit.total_e8s,
        candid::Nat::from(60 * E8),
        "C-1: supply audit total == 60e8"
    );
    // And supply == sum(vault debt) (the audit total is the chain supply; the vault
    // debt is read above as 60e8 — they agree, both at on-chain truth).
    assert_eq!(
        audit.total_e8s,
        v.debt_e8s,
        "C-1: chain supply == vault debt (== 60e8, on-chain truth)"
    );

    eprintln!("[phase1b burn-idempotency] FULL C-1 guard PASSED: good burn applied once (debt 60e8), poison skipped, cursor advanced, supply == on-chain truth 60e8");
}

// ─── Review Minor #1: same-tx different-log-index dedup ──────────────────────
//
// Two Burn logs in the SAME transaction (same tx_hash) for the SAME vault and
// SAME amount are TWO DISTINCT on-chain burns. The canonical on-chain identity
// of a log is (tx_hash, log_index) — not (tx_hash, vault_id, amount). The old
// C-1 key `format!("{tx_hash}:{vault_id}:{amount_e8s}")` collapses them into
// one dedup entry, so the second real burn is silently dropped (vault's debt
// and chain supply are over-stated; the user loses credit).
//
// This test asserts BOTH burns applied → debt == 100e8 − 60e8 = 40e8, not 70e8
// (what the old single-key dedup would give by dropping the second 30e8 burn).
//
// IcUSD.burn() is permissionless; a wrapper contract CAN emit two identical
// Burn events in one tx. The fix uses `format!("{tx_hash}:{log_index}")` as the
// dedup key — the canonical EVM log identity.
#[test]
fn phase1b_burn_two_identical_burns_in_same_tx_both_applied() {
    let (pic, backend, mock) = boot();

    // Point the backend at the mock.
    decode_result(
        update_dev(&pic, backend, "set_evm_rpc_principal", Encode!(&mock).unwrap()),
        "set_evm_rpc_principal",
    )
    .expect("set_evm_rpc_principal");

    // Register Monad.
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
        update_dev(&pic, backend, "register_chain", Encode!(&reg).unwrap()),
        "register_chain",
    )
    .expect("register_chain");

    decode_result(
        update_dev(
            &pic,
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
            &pic,
            backend,
            "set_manual_collateral_price",
            Encode!(&ChainId(MONAD_CHAIN_ID), &"MON".to_string(), &200_000_000u64).unwrap(),
        ),
        "set_manual_collateral_price",
    )
    .expect("set_manual_collateral_price");

    // Seed the cursor well above 256 so scan windows stay in a clean range.
    decode_result(
        update_dev(
            &pic,
            backend,
            "set_last_observed_block",
            Encode!(&ChainId(MONAD_CHAIN_ID), &2_000_000u64).unwrap(),
        ),
        "set_last_observed_block",
    )
    .expect("set_last_observed_block");

    update_any(&pic, mock, "set_blocks", Encode!(&2_000_256u64, &2_000_256u64).unwrap());
    update_any(&pic, mock, "set_next_send_hash", Encode!(&"0xmint_same_tx".to_string()).unwrap());

    // ── ECDSA probe ──────────────────────────────────────────────────────────
    let settlement_addr = match update_dev(
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

    let settlement_addr = match settlement_addr {
        Some(addr) => {
            eprintln!("[phase1b same-tx-burns] ECDSA AVAILABLE; addr={addr}; running full test");
            addr
        }
        None => {
            eprintln!("[phase1b same-tx-burns] ECDSA UNAVAILABLE; skipping (needs Open vault with debt)");
            return;
        }
    };

    // Fund hot wallet.
    update_any(
        &pic,
        mock,
        "set_balance",
        Encode!(&settlement_addr, &candid::Nat::from(1_000u128 * E18)).unwrap(),
    );

    // ── Open a vault with debt = 100e8 ───────────────────────────────────────
    let collateral_e18 = 100u128 * E18;
    let debt_e8s = 100u128 * E8;
    let recipient = "0x000000000000000000000000000000000000abcd".to_string();
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

    // Deposit confirmed; observer flips to MintPending + enqueues mint.
    update_any(
        &pic,
        mock,
        "set_balance",
        Encode!(&custody, &candid::Nat::from(collateral_e18)).unwrap(),
    );
    advance_and_tick(&pic, 2);

    // Settlement submits + confirms the mint.
    push_mint_log(&pic, mock, vault_id, &recipient, debt_e8s, "0xmint_same_tx", 2_000_256);
    advance_and_tick(&pic, 4);

    let v = get_vault(&pic, backend, vault_id).expect("vault after mint confirm");
    assert_eq!(v.status, ChainVaultStatus::Open, "mint confirmed => Open");
    assert_eq!(v.debt_e8s, candid::Nat::from(debt_e8s), "debt == 100e8 after mint");

    // ── Same-tx two-identical-burns scenario ─────────────────────────────────
    // Advance the mock chain head into the next scan window.
    update_any(&pic, mock, "clear_logs", Encode!().unwrap());
    update_any(&pic, mock, "set_blocks", Encode!(&2_000_512u64, &2_000_512u64).unwrap());

    // Push TWO Burn logs with:
    //   - SAME tx_hash ("0xdualtx")
    //   - SAME vault_id
    //   - SAME amount_e8s (30e8 each)
    //   - DIFFERENT log_index (0 vs 1)
    // These are two distinct on-chain burns: vault burns 30e8 twice = 60e8 total.
    // Expected: debt == 100e8 - 60e8 = 40e8.
    // With the OLD key (tx:vault:amount): both map to the same key → second dropped → debt=70e8.
    // With the NEW key (tx:log_index): distinct keys → both applied → debt=40e8.
    push_burn_log_at(
        &pic, mock, vault_id, &recipient, 30 * E8, "0xdualtx", 2_000_300, 0,
    );
    push_burn_log_at(
        &pic, mock, vault_id, &recipient, 30 * E8, "0xdualtx", 2_000_300, 1,
    );

    // Multiple ticks to ensure re-scans don't double-apply anything.
    advance_and_tick(&pic, 6);

    // Both burns must have applied: debt == 40e8.
    let v = get_vault(&pic, backend, vault_id).expect("vault after dual-burn");
    assert_eq!(
        v.debt_e8s,
        candid::Nat::from(40 * E8),
        "Minor #1: both same-tx burns applied → debt 40e8 (not 70e8 from dropped second burn)"
    );
    assert_eq!(v.status, ChainVaultStatus::Open, "vault stays Open after partial burns");

    // Cursor must have advanced.
    assert!(
        cursor(&pic, backend) >= 2_000_512,
        "cursor advanced past dual-burn block; got {}",
        cursor(&pic, backend)
    );

    // Supply must match on-chain truth: 100e8 - 60e8 = 40e8.
    let global: candid::Nat = query_unit(&pic, backend, "get_global_icusd_supply");
    assert_eq!(
        global,
        candid::Nat::from(40 * E8),
        "Minor #1: global supply == 40e8 (both burns credited)"
    );
    let audit: SupplyAuditWire = query_unit(&pic, backend, "get_supply_audit");
    assert_eq!(
        audit.total_e8s,
        v.debt_e8s,
        "Minor #1: chain supply == vault debt == 40e8"
    );

    eprintln!("[phase1b same-tx-burns] PASSED: both same-tx burns applied → debt 40e8, supply 40e8, cursor advanced");
}
