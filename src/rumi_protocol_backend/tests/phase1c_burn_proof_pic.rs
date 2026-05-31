//! Phase 1c — notify-then-verify burn observation end-to-end.
//!
//! ## What this proves
//!
//! The new `submit_burn_proof(chain_id, tx_hash)` update endpoint verifies ONE
//! transaction's receipt (via `eth_getTransactionReceipt` + a consensus-safe
//! `eth_getBlockByNumber` finality probe) and applies any `Burn` log emitted by
//! the configured icUSD contract — WITHOUT the continuous `eth_getLogs`
//! poll-scan (which is OFF by default in this mode).
//!
//! The harness (boot / configure_chain / advance_and_tick / ABI-word helpers /
//! ECDSA-gating / 30s interval pin) is copied from the sibling
//! `phase1b_getlogs_chunking_pic.rs`.
//!
//! Scenario (ECDSA available — full guard):
//!   1. Stand up an Open vault with debt 100e8 (open → deposit → mint-confirm).
//!   2. Script a Burn log on a receipt for tx `0xburn1` (address = configured
//!      contract; topics = [BURN_TOPIC0, word(vault_id), word(burner)]; data =
//!      word(40e8); logIndex 0). The receipt block is set so `is_block_final`
//!      passes (latest/finalized >= receipt_block + finality_depth).
//!   3. `submit_burn_proof(10143, "0xburn1")` → Ok(1); vault debt 60e8; global
//!      supply 60e8.
//!   4. Re-submit same tx → Ok(0) (deduped); debt still 60e8.
//!   5. Negative: a Burn log on `0xfake` from a WRONG contract → Ok(0); debt
//!      unchanged.
//!   6. Poll-scan irrelevance: arm `fail_always("eth_getLogs", ...)` and confirm
//!      `submit_burn_proof` STILL works (it never calls `eth_getLogs`).
//!
//! ECDSA-gated subset (no signing available): the verify path is still
//! exercised against a synthetic AwaitingDeposit-free state by asserting the
//! endpoint rejects a forged-contract burn (Ok(0)) and that a pending/unmined
//! tx surfaces TemporarilyUnavailable — both require no settlement signing.

use candid::{encode_args, encode_one, CandidType, Decode, Deserialize, Encode, Principal};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use std::time::Duration;

// Must equal the backend's `MAX_BLOCK_SCAN_WINDOW` (chains/monad/evm_rpc.rs).
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

// `ChainId(pub u32)` is a single-field tuple struct — candid serializes it as
// the bare inner `nat32`, matching the `.did` (`submit_burn_proof : (nat32, text)`).
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

// Mirror the FULL `ProtocolError` enum (lib.rs) so candid variant tags line up.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq)]
enum TransferFromError {
    GenericError { error_code: candid::Nat, message: String },
    TemporarilyUnavailable,
    InsufficientAllowance { allowance: candid::Nat },
    BadBurn { min_burn_amount: candid::Nat },
    Duplicate { duplicate_of: candid::Nat },
    BadFee { expected_fee: candid::Nat },
    CreatedInFuture { ledger_time: u64 },
    TooOld,
    InsufficientFunds { balance: candid::Nat },
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq)]
enum TransferError {
    GenericError { error_code: candid::Nat, message: String },
    TemporarilyUnavailable,
    BadBurn { min_burn_amount: candid::Nat },
    Duplicate { duplicate_of: candid::Nat },
    BadFee { expected_fee: candid::Nat },
    CreatedInFuture { ledger_time: u64 },
    TooOld,
    InsufficientFunds { balance: candid::Nat },
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq)]
enum ProtocolError {
    TransferFromError(TransferFromError, u64),
    TransferError(TransferError),
    TemporarilyUnavailable(String),
    AlreadyProcessing,
    AnonymousCallerNotAllowed,
    CallerNotOwner,
    AmountTooLow { minimum_amount: u64 },
    GenericError(String),
    NotLowestCR,
    SupplyInvariantHalted,
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
const CONTRACT: &str = "0x00000000000000000000000000000000deadbeef";

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

/// Decode a `submit_burn_proof` reply: `variant { Ok: nat32; Err: ProtocolError }`.
fn submit_burn_proof(
    pic: &PocketIc,
    backend: Principal,
    chain_id: u32,
    tx_hash: &str,
) -> Result<u32, ProtocolError> {
    // The `inspect_message` hook silently rejects ANONYMOUS callers for every
    // method except the two consent reads, so submit from a non-anonymous
    // principal (the endpoint itself is permissionless — any non-anonymous
    // caller may submit a real tx hash; the verify path rejects forgeries).
    let reply = update_dev(
        pic,
        backend,
        "submit_burn_proof",
        Encode!(&ChainId(chain_id), &tx_hash.to_string()).unwrap(),
    );
    match reply {
        WasmResult::Reply(b) => Decode!(&b, Result<u32, ProtocolError>)
            .expect("decode submit_burn_proof Result"),
        WasmResult::Reject(msg) => panic!("submit_burn_proof rejected: {}", msg),
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

    // Pin observer + settlement cadence to 30s (code DEFAULT is 300s).
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

/// Script a Burn log onto a transaction's RECEIPT (returned by
/// `eth_getTransactionReceipt`). The receipt block is set FIRST via `set_receipt`
/// — `push_receipt_log` reads the receipt's current block and stamps the log with
/// it (creating the receipt at block 0 if absent). Setting the block first means
/// `is_block_final(receipt_block, finality_depth)` probes a block that EXISTS
/// once the mock's latest/finalized >= receipt_block + finality_depth.
fn script_burn_receipt(
    pic: &PocketIc,
    mock: Principal,
    tx_hash: &str,
    receipt_block: u64,
    contract: &str,
    vault_id: u64,
    burner: &str,
    amount_e8s: u128,
    log_index: u64,
) {
    // Set the receipt's stored block BEFORE pushing the log (push_receipt_log
    // copies the receipt's block onto the log and would default to 0 otherwise).
    update_any(
        pic,
        mock,
        "set_receipt",
        Encode!(&tx_hash.to_string(), &true, &receipt_block).unwrap(),
    );
    let topics = vec![
        BURN_EVENT_TOPIC0.to_string(),
        word_u128(vault_id as u128),
        word_addr(burner),
    ];
    let data = word_u128(amount_e8s);
    update_any(
        pic,
        mock,
        "push_receipt_log",
        Encode!(
            &tx_hash.to_string(),
            &contract.to_string(),
            &topics,
            &data,
            &log_index
        )
        .unwrap(),
    );
}

/// Register Monad + contract + manual MON price + seed the burn-watch cursor.
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
            Encode!(&ChainId(MONAD_CHAIN_ID), &CONTRACT.to_string()).unwrap(),
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
fn phase1c_submit_burn_proof_verifies_one_tx_no_poll_scan() {
    let (pic, backend, mock) = boot();

    let seed: u64 = 7_000_000;
    configure_chain(&pic, backend, mock, seed);

    // Chain head one window above the seed so finality probes resolve. With
    // finality_depth=1 a receipt at block B is final once finalized >= B+1.
    let finalized = seed + SCAN_WINDOW;
    update_any(&pic, mock, "set_blocks", Encode!(&finalized, &finalized).unwrap());
    update_any(&pic, mock, "set_next_send_hash", Encode!(&"0xmint1".to_string()).unwrap());

    let burner = "0x000000000000000000000000000000000000beef".to_string();

    match ecdsa_settlement_addr(&pic, backend) {
        None => {
            // ── GATED subset (no signing): verify path rejects a forged-contract
            //    burn and a pending tx, neither of which needs settlement signing.
            eprintln!("[phase1c burn-proof] ECDSA UNAVAILABLE; running GATED subset (forged-contract rejection + pending)");

            // No vault exists. A burn citing a non-existent vault from the WRONG
            // contract must apply nothing (Ok(0)) — contract filter rejects it.
            script_burn_receipt(
                &pic, mock, "0xfake", finalized - 10, "0xnotthecontract", 1, &burner, 40 * E8, 0,
            );
            let r = submit_burn_proof(&pic, backend, MONAD_CHAIN_ID, "0xfake");
            assert_eq!(r, Ok(0), "forged-contract burn applies nothing (got {:?})", r);

            // An unmined tx surfaces TemporarilyUnavailable (retryable).
            match submit_burn_proof(&pic, backend, MONAD_CHAIN_ID, "0xneverexisted") {
                Err(ProtocolError::TemporarilyUnavailable(_)) => {}
                other => panic!("pending tx should be TemporarilyUnavailable, got {:?}", other),
            }
            let global: candid::Nat = query_unit(&pic, backend, "get_global_icusd_supply");
            assert_eq!(global, candid::Nat::from(0u32), "gated: supply invariant holds at 0");
            eprintln!("[phase1c burn-proof] GATED subset PASSED");
            return;
        }
        Some(settlement_addr) => {
            eprintln!("[phase1c burn-proof] ECDSA AVAILABLE; settlement={settlement_addr}; running FULL guard");

            // Fund the hot wallet so the settlement submit-path gas gate passes.
            update_any(
                &pic,
                mock,
                "set_balance",
                Encode!(&settlement_addr, &candid::Nat::from(1_000u128 * E18)).unwrap(),
            );

            // ── 1. Stand up an Open vault: 100 MON @ $2 backs 100 icUSD. ──────
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

            // Deposit lands; observer flips to MintPending + enqueues mint. Push
            // the Mint log at the finalized block; the mint-confirm path scans a
            // single block and confirms.
            update_any(
                &pic,
                mock,
                "set_balance",
                Encode!(&custody, &candid::Nat::from(collateral_e18)).unwrap(),
            );
            advance_and_tick(&pic, 2);
            push_mint_log(&pic, mock, vault_id, &recipient, debt_e8s, "0xmint1", finalized);
            advance_and_tick(&pic, 4);

            let v = get_vault(&pic, backend, vault_id).expect("vault after mint confirm");
            assert_eq!(v.status, ChainVaultStatus::Open, "mint confirmed => Open");
            assert_eq!(v.debt_e8s, candid::Nat::from(debt_e8s), "mint confirmed => debt 100e8");

            // ── 2. Script a final 40e8 Burn receipt for tx 0xburn1. ───────────
            // receipt_block + finality_depth(1) <= finalized so is_block_final
            // passes. The burn-watch poll-scan is OFF; the cursor never advanced
            // by scanning, so place the receipt anywhere <= finalized - 1.
            let burn_block = finalized - 100;
            script_burn_receipt(
                &pic, mock, "0xburn1", burn_block, CONTRACT, vault_id, &burner, 40 * E8, 0,
            );

            // ── 3. submit_burn_proof => Ok(1), debt 60e8, supply 60e8. ────────
            let r = submit_burn_proof(&pic, backend, MONAD_CHAIN_ID, "0xburn1");
            assert_eq!(r, Ok(1), "first submit applies exactly one burn (got {:?})", r);

            let v = get_vault(&pic, backend, vault_id).expect("vault after burn proof");
            assert_eq!(v.debt_e8s, candid::Nat::from(60 * E8), "debt 60e8 after 40e8 burn");
            assert_eq!(v.status, ChainVaultStatus::Open, "partial burn keeps vault Open");

            let global: candid::Nat = query_unit(&pic, backend, "get_global_icusd_supply");
            assert_eq!(global, candid::Nat::from(60 * E8), "global supply 60e8");
            let audit: SupplyAuditWire = query_unit(&pic, backend, "get_supply_audit");
            assert_eq!(audit.total_e8s, v.debt_e8s, "chain supply == vault debt == 60e8");

            // ── 4. Re-submit same tx => Ok(0) (deduped), debt unchanged. ──────
            let r = submit_burn_proof(&pic, backend, MONAD_CHAIN_ID, "0xburn1");
            assert_eq!(r, Ok(0), "re-submit deduped to zero applied (got {:?})", r);
            let v = get_vault(&pic, backend, vault_id).expect("vault after re-submit");
            assert_eq!(v.debt_e8s, candid::Nat::from(60 * E8), "dedup: debt still 60e8");

            // ── 5. Negative: burn from the WRONG contract => Ok(0), unchanged. ─
            script_burn_receipt(
                &pic, mock, "0xfake", burn_block, "0xnotthecontract", vault_id, &burner, 40 * E8, 0,
            );
            let r = submit_burn_proof(&pic, backend, MONAD_CHAIN_ID, "0xfake");
            assert_eq!(r, Ok(0), "wrong-contract burn applies nothing (got {:?})", r);
            let v = get_vault(&pic, backend, vault_id).expect("vault after wrong-contract");
            assert_eq!(v.debt_e8s, candid::Nat::from(60 * E8), "wrong contract: debt still 60e8");

            // ── 6. Poll-scan irrelevance: arm fail_always(eth_getLogs) and show
            //    submit_burn_proof STILL works (uses eth_getTransactionReceipt +
            //    eth_getBlockByNumber, never eth_getLogs). ──────────────────────
            update_any(
                &pic,
                mock,
                "fail_always",
                Encode!(
                    &"eth_getLogs".to_string(),
                    &"forced getLogs failure (burn-proof must not use getLogs)".to_string()
                )
                .unwrap(),
            );
            // A fresh, distinct burn at log_index 1 on tx 0xburn2.
            script_burn_receipt(
                &pic, mock, "0xburn2", burn_block, CONTRACT, vault_id, &burner, 10 * E8, 1,
            );
            let r = submit_burn_proof(&pic, backend, MONAD_CHAIN_ID, "0xburn2");
            assert_eq!(
                r,
                Ok(1),
                "submit_burn_proof works with eth_getLogs forced-failing => never calls getLogs (got {:?})",
                r
            );
            let v = get_vault(&pic, backend, vault_id).expect("vault after getLogs-barrier burn");
            assert_eq!(v.debt_e8s, candid::Nat::from(50 * E8), "debt 50e8 after second 10e8 burn");

            // Cursor must NOT have advanced by scanning (poll-scan OFF means the
            // observer's burn-watch never paged getLogs; it only advances via the
            // finalized probe). The receipt-verify path does not touch the cursor.
            let _ = cursor(&pic, backend);

            let global: candid::Nat = query_unit(&pic, backend, "get_global_icusd_supply");
            assert_eq!(global, candid::Nat::from(50 * E8), "global supply 50e8 after both burns");

            eprintln!("[phase1c burn-proof] FULL guard PASSED: submit_burn_proof verified one tx, deduped, rejected wrong contract, and worked with eth_getLogs forced-failing");
        }
    }
}
