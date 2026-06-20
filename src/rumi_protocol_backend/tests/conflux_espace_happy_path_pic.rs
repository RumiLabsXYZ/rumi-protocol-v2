//! Conflux eSpace testnet (chain 71) end-to-end happy-path integration test
//! against the scripted mock EVM RPC canister.
//!
//! This mirrors `phase1b_monad_happy_path_pic.rs` for Conflux. It is the
//! integration PROOF that the Conflux rail flows through the SHARED EVM worker
//! (observer + settlement) with NO Conflux special-case, exactly like any
//! non-Solana chain. The two things that make Conflux distinct from Monad and
//! are exercised here:
//!
//!   1. `min_quorum_providers: Some(1)` (set by `conflux_testnet_register_arg()`):
//!      a SINGLE mock provider satisfies the read quorum, so balance/block/log
//!      reads succeed. (Monad's default floor is 3; the Monad happy-path test is
//!      stale at this commit precisely because it never set this field (see the
//!      session report). Conflux's relaxed floor is what makes a one-provider
//!      mock work.)
//!   2. `getlogs_max_range = 1000` for chain 71 (vs Monad's 100): the backend
//!      chunks the burn-watch scan into <= 1000-block sub-queries, and the mock's
//!      getLogs range cap is raised to 1000 (`set_getlogs_max_range`) so a chunk
//!      at the chain's max is NOT rejected.
//!
//! Finality: Conflux uses the SAME consensus-safe specific-block probe as Monad
//! (never a `finalized` tag), but with a LARGE `finality_depth` (100). A block
//! `candidate` is treated final only when `candidate + 100` exists, so the
//! observer's burn-watch cursor advances to `candidate = last_observed + 1024`
//! only when `finalized_block >= candidate + 100`. The mint/withdrawal receipts
//! are relocated to the (advanced) cursor block before confirmation so the
//! settlement finality gate (`receipt_block <= finalized_cursor`) is satisfied,
//! honoring the depth-100 burial.
//!
//! Flow (OPEN-THEN-VERIFY, the owner's Design-B):
//!   register/config (chain 71, contract, manual CFX price, evm_rpc principal)
//!     -> open_chain_vault            => vault AwaitingDeposit, supply 0
//!     -> deposit lands on custody    => observer flips MintPending + enqueues mint
//!     -> settlement submits mint     => tx broadcast; receipt relocated to cursor
//!     -> settlement confirms mint    => vault Open, debt 100e8, chain_supply 100e8
//!     -> burn 40e8 on chain          => observer: debt 60e8, supply 60e8
//!     -> burn 60e8 on chain          => observer: debt 0,     supply 0
//!     -> withdraw_chain_collateral   => vault Closing -> (confirm) Closed, supply 0
//!
//! At each labeled step the test asserts
//!   get_global_icusd_supply() == sum(per-chain audit supply) == expected
//! and the chain-71 supply == sum of chain-71 vault debt. The supply invariant
//! is the POINT of this test.
//!
//! tECDSA-in-PocketIC: like the Monad test, this boots with an II subnet so
//! PocketIC can provision `test_key_1`, and PROBES ECDSA via
//! `get_chain_settlement_address`. If ECDSA is unavailable in this PocketIC
//! build, the test runs the GATED subset (register/config + invariant-at-0) and
//! auto-upgrades to the full path on an ECDSA-capable build.

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

/// Mirror of the backend `RegisterChainArg`, INCLUDING `min_quorum_providers`
/// (the field Conflux relies on: `Some(1)` lets a single mock provider satisfy
/// the read quorum). `opt nat32` on the wire.
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
    owner_evm: Option<String>,
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

/// Minimal mirror of `ProtocolError`: these chain endpoints only ever return the
/// `ChainAdmin(text)` variant on the happy path. Any other variant fails the
/// Candid decode loudly (the correct signal that something unexpected happened).
#[derive(CandidType, Deserialize, Clone, Debug)]
enum ProtocolError {
    ChainAdmin(String),
    /// M2 self-serve rejection (bad nonce, wrong signer, etc.). The `_evm`
    /// methods return this; mirrored so the rejection-path asserts decode.
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
/// keccak256("Burn(uint256,address,uint256)"), must match evm_rpc.rs.
const BURN_EVENT_TOPIC0: &str =
    "0xe1b6e34006e9871307436c226f232f9c5e7690c1d2c4f4adda4f607a75a9beca";

/// Conflux register arg's `finality_depth` (mirrors conflux_testnet_register_arg()).
const CONFLUX_FINALITY_DEPTH: u64 = 100;
/// Observer block-scan window (`MAX_BLOCK_SCAN_WINDOW`); the cursor advances in
/// jumps of this size. Must match the backend constant.
const SCAN_WINDOW: u64 = 1024;

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

// ─── Supply-invariant assertion (the POINT of this test) ─────────────────────

/// Assert the canonical multi-chain supply invariant at a labeled step:
///   get_global_icusd_supply() == sum(per-chain audit supply) == expected_e8s,
/// and the Conflux chain's audited supply matches the expected value.
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

// ─── The test ────────────────────────────────────────────────────────────────

#[test]
fn conflux_espace_happy_path_supply_invariant() {
    let (pic, backend, mock) = boot();

    // ── Step 1: point the backend's EVM wrapper at the mock ──────────────────
    decode_result(
        update_dev(&pic, backend, "set_evm_rpc_principal", Encode!(&mock).unwrap()),
        "set_evm_rpc_principal",
    )
    .expect("set_evm_rpc_principal");

    // Conflux's backend getlogs_max_range is 1000; raise the mock's cap to match
    // so a 1000-block burn-watch chunk is NOT rejected. Also enable the eSpace
    // extra receipt fields so the receipt parser tolerance is exercised.
    update_any(&pic, mock, "set_getlogs_max_range", Encode!(&1000u64).unwrap());
    update_any(&pic, mock, "set_espace_receipt_fields", Encode!(&true).unwrap());

    // ── Step 2: register Conflux + contract + manual CFX price ───────────────
    // Use the SAME shape conflux_testnet_register_arg() produces: chain 71,
    // finality_depth 100, EIP-1559, native decimals 18, min_quorum_providers 1.
    // (A single mock endpoint is enough because the floor is relaxed to 1.)
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

    // $0.15 / CFX in e8 => 15_000_000.
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

    // ── Seed the burn-watch cursor ───────────────────────────────────────────
    // Seed well above SCAN_WINDOW so the small block numbers no longer apply and
    // the cursor advances in SCAN_WINDOW (1024) jumps once the chain head allows.
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

    // Burn-watch poll-scan is OFF by default; this test observes burns via the
    // POLL mechanism, so opt in.
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

    // Invariant holds at zero after register/config (Design B: nothing minted).
    assert_supply(&pic, backend, 0, "after register/config");

    // ── Block plan (finality_depth = 100) ────────────────────────────────────
    // The cursor advances to `candidate = last_observed + 1024` only when
    // `is_block_final(candidate, 100)` holds, i.e. block `candidate + 100`
    // exists (`finalized_block >= candidate + 100`).
    //   cursor1 = seed + 1024            (after step 5/6 ticks)
    // Set the chain head so cursor1 is final: finalized >= cursor1 + 100.
    let cursor1 = seed + SCAN_WINDOW; // 1_001_024
    let head1 = cursor1 + CONFLUX_FINALITY_DEPTH + 24; // 1_001_148 (margin)
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
                eprintln!("[conflux happy-path] ECDSA UNAVAILABLE (get_chain_settlement_address returned Err: {e:?}); running GATED subset");
                None
            }
            Err(decode_err) => {
                eprintln!("[conflux happy-path] get_chain_settlement_address decode error ({decode_err}); running GATED subset");
                None
            }
        },
        WasmResult::Reject(msg) => {
            eprintln!("[conflux happy-path] ECDSA UNAVAILABLE (get_chain_settlement_address rejected: {msg}); running GATED subset");
            None
        }
    };

    let settlement_addr = match settlement_addr {
        Some(addr) => {
            eprintln!("[conflux happy-path] ECDSA AVAILABLE; settlement address = {addr}; running FULL happy path");
            addr
        }
        None => {
            assert_supply(&pic, backend, 0, "gated: ECDSA unavailable, invariant at 0");
            return;
        }
    };

    // ════════════════════════════════════════════════════════════════════════
    // FULL HAPPY PATH (ECDSA available)
    // ════════════════════════════════════════════════════════════════════════

    // Fund the settlement (hot-wallet) address so the submit-path gas gate passes.
    update_any(
        &pic,
        mock,
        "set_balance",
        Encode!(&settlement_addr, &candid::Nat::from(1_000_000u128 * E18)).unwrap(),
    );

    // ── Step 4: open_chain_vault (AwaitingDeposit, no mint) ──────────────────
    // 1400 CFX @ $0.15 = $210 backing 100 icUSD debt => 210% CR (>= 133%).
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
    assert_eq!(v.debt_e8s, candid::Nat::from(0u32), "open => no confirmed debt");
    assert_eq!(
        v.pending_mint_e8s,
        candid::Nat::from(debt_e8s),
        "open => pending_mint carries intended mint"
    );
    let custody = v.custody_address.clone();
    assert!(custody.starts_with("0x"), "custody address is 0x hex: {custody}");
    assert_supply(&pic, backend, 0, "after open (AwaitingDeposit)");

    // ── Step 5: deposit lands; observer flips to MintPending + enqueues mint ─
    update_any(
        &pic,
        mock,
        "set_balance",
        Encode!(&custody, &candid::Nat::from(collateral_e18)).unwrap(),
    );
    advance_and_tick(&pic, 2);

    let v = get_vault(&pic, backend, vault_id).expect("vault after deposit-verify");
    assert_eq!(
        v.status,
        ChainVaultStatus::MintPending,
        "deposit verified => MintPending"
    );
    assert_supply(&pic, backend, 0, "after deposit-verify (MintPending)");

    // ── Step 6: settlement submits the mint, then confirms it ────────────────
    // The settlement worker broadcasts 0xcfxmint1; the mock auto-mines a receipt
    // at the current finalized_block (head1, which is cursor1 + 100 + 24 and thus
    // ABOVE the advanced cursor). The settlement finality gate requires the
    // receipt block to be <= the finalized CURSOR, so we RELOCATE the receipt and
    // its Mint log to `cursor1` (the deepest block the observer treats as final)
    // before confirmation, honoring the depth-100 burial. Submit happens in the
    // first window; relocate; then confirm in the next windows.
    advance_and_tick(&pic, 1); // submit (Queued -> Inflight)

    // Relocate the auto-mined receipt + push the Mint log at the finalized cursor.
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
    assert_eq!(
        v.debt_e8s,
        candid::Nat::from(debt_e8s),
        "mint confirmed => debt = 100e8"
    );
    assert_eq!(v.pending_mint_e8s, candid::Nat::from(0u32), "pending mint cleared on confirm");
    // THE invariant flips from 0 to 100e8 exactly here.
    assert_supply(&pic, backend, 100 * E8, "after mint confirm (Open)");

    // ── Step 7: burn 40e8 on chain; observer decrements debt + supply ────────
    // Advance the chain head so the cursor climbs cursor1 -> cursor2; the burn at
    // a block in (cursor1, cursor2] is scanned. The scan window (1024) is wider
    // than ... well, equal to ... the getlogs cap minus margin, so the backend
    // chunks the 1024-block scan into 2 sub-queries (1000 + 24), each within the
    // chain-71 max range of 1000, which the mock now accepts.
    let cursor2 = cursor1 + SCAN_WINDOW; // 1_002_048
    let head2 = cursor2 + CONFLUX_FINALITY_DEPTH + 24;
    update_any(&pic, mock, "set_blocks", Encode!(&head2, &head2).unwrap());
    let burn1_block = cursor1 + 500; // within (cursor1, cursor2]
    push_burn_log(&pic, mock, vault_id, &recipient, 40 * E8, "0xcfxburn1", burn1_block);
    advance_and_tick(&pic, 3);

    let v = get_vault(&pic, backend, vault_id).expect("vault after burn 40");
    assert_eq!(v.debt_e8s, candid::Nat::from(60 * E8), "after burn40 => debt 60e8");
    assert_eq!(v.status, ChainVaultStatus::Open, "partial burn keeps vault Open");
    assert_supply(&pic, backend, 60 * E8, "after burn 40e8");

    // ── Step 8: burn the remaining 60e8; debt + supply go to 0 ───────────────
    let cursor3 = cursor2 + SCAN_WINDOW; // 1_003_072
    let head3 = cursor3 + CONFLUX_FINALITY_DEPTH + 24;
    update_any(&pic, mock, "set_blocks", Encode!(&head3, &head3).unwrap());
    let burn2_block = cursor2 + 700; // within (cursor2, cursor3]
    push_burn_log(&pic, mock, vault_id, &recipient, 60 * E8, "0xcfxburn2", burn2_block);
    advance_and_tick(&pic, 3);

    let v = get_vault(&pic, backend, vault_id).expect("vault after burn 60");
    assert_eq!(v.debt_e8s, candid::Nat::from(0u32), "after burn60 => debt 0");
    assert_supply(&pic, backend, 0, "after burn remaining 60e8");

    // ── Step 9: withdraw all collateral; vault Closing -> Closed ─────────────
    update_any(&pic, mock, "set_next_send_hash", Encode!(&"0xcfxwd1".to_string()).unwrap());
    let dest = "0x000000000000000000000000000000000000dead".to_string();
    decode_result(
        update_dev(
            &pic,
            backend,
            "withdraw_chain_collateral",
            Encode!(&vault_id, &candid::Nat::from(collateral_e18), &dest).unwrap(),
        ),
        "withdraw_chain_collateral",
    )
    .expect("withdraw_chain_collateral");

    // Immediately after enqueue (before settlement confirms) the vault is Closing.
    let v = get_vault(&pic, backend, vault_id).expect("vault after withdraw enqueue");
    assert_eq!(v.status, ChainVaultStatus::Closing, "withdraw all => Closing");
    assert_supply(&pic, backend, 0, "after withdraw enqueue (Closing)");

    // Settlement submits 0xcfxwd1 (auto-mined at head3, above the cursor); like
    // the mint, relocate the receipt to the finalized cursor (cursor3) so the
    // finality gate passes, then confirm Closing -> Closed.
    advance_and_tick(&pic, 1); // submit (Queued -> Inflight)
    update_any(
        &pic,
        mock,
        "set_receipt",
        Encode!(&"0xcfxwd1".to_string(), &true, &cursor3).unwrap(),
    );
    advance_and_tick(&pic, 4); // confirm

    let v = get_vault(&pic, backend, vault_id).expect("vault after withdraw confirm");
    assert_eq!(v.status, ChainVaultStatus::Closed, "withdraw confirmed => Closed");

    // ── Step 10: final invariant (supply 0, audit total 0) ───────────────────
    assert_supply(&pic, backend, 0, "final (Closed)");
    let audit: SupplyAuditWire = query_unit(&pic, backend, "get_supply_audit");
    assert_eq!(audit.total_e8s, candid::Nat::from(0u32), "final audit total 0");

    eprintln!("[conflux happy-path] FULL happy path PASSED: supply invariant held 0 -> 100e8 -> 60e8 -> 0 across open/deposit/mint/burn/withdraw/close on chain 71 (finality_depth=100, getlogs_max_range=1000, min_quorum_providers=1)");
}

// ─── helpers that build mock-control args ────────────────────────────────────

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
        WasmResult::Reply(b) => {
            Decode!(&b, Option<ChainVaultV1>).expect("decode get_chain_vault")
        }
        WasmResult::Reject(msg) => panic!("get_chain_vault rejected: {msg}"),
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

// ════════════════════════════════════════════════════════════════════════════
// M2: EVM-native self-serve (EIP-712 signed, ANONYMOUS caller) end-to-end.
//
// Proves the full self-serve loop with the IC caller = Principal::anonymous():
//   open_chain_vault_evm -> deposit -> mint -> borrow_chain_vault_evm -> mint
//   -> repay (burn) -> close_chain_vault_evm -> Closed
// plus replay-nonce + wrong-signer rejection. The vault is owned by the
// synthetic principal derived from the recovered EVM signer; the anonymous calls
// reaching the method body proves the inspect_message accept-list works.
// ════════════════════════════════════════════════════════════════════════════

use rumi_protocol_backend::chains::evm::eip712::{
    domain_separator, intent_digest, intent_struct_hash, synthetic_owner, IntentAction, VaultIntent,
};

const EVM_CONTRACT: &str = "0x00000000000000000000000000000000cf1c0de5";

/// The fixed scalar=1 secp256k1 signer + its canonical EVM address.
fn evm_signer() -> (k256::ecdsa::SigningKey, String) {
    use k256::ecdsa::{SigningKey, VerifyingKey};
    let mut b = [0u8; 32];
    b[31] = 1;
    let sk = SigningKey::from_bytes(&b.into()).unwrap();
    let pk = VerifyingKey::from(&sk).to_encoded_point(false).as_bytes().to_vec();
    let addr = rumi_protocol_backend::chains::evm::tecdsa::evm_address_from_pubkey(&pk).unwrap();
    (sk, addr)
}

fn make_intent(
    action: IntentAction,
    owner: &str,
    vault_id: u64,
    collateral_wei: u128,
    debt_e8s: u128,
    nonce: u64,
) -> VaultIntent {
    VaultIntent {
        action: action.as_u8(),
        chain_id: CONFLUX_CHAIN_ID as u64,
        owner: owner.to_string(),
        vault_id,
        collateral_wei,
        debt_e8s,
        recipient: owner.to_string(), // M2: recipient forced == owner
        nonce,
        deadline_secs: 9_999_999_999,
    }
}

/// Sign an intent for `EVM_CONTRACT` with the fixed key → 65-byte r||s||v sig.
fn sign(sk: &k256::ecdsa::SigningKey, intent: &VaultIntent) -> Vec<u8> {
    use k256::ecdsa::{RecoveryId, Signature};
    let digest = intent_digest(
        &domain_separator(intent.chain_id, EVM_CONTRACT).unwrap(),
        &intent_struct_hash(intent).unwrap(),
    );
    let (sig, rid): (Signature, RecoveryId) = sk.sign_prehash_recoverable(&digest).unwrap();
    let mut out = sig.to_bytes().to_vec();
    out.push(27 + u8::from(rid));
    out
}

/// dev-gated register + config for Conflux (steps shared with the happy path).
fn configure_conflux(pic: &PocketIc, backend: Principal, mock: Principal, seed: u64) {
    decode_result(
        update_dev(pic, backend, "set_evm_rpc_principal", Encode!(&mock).unwrap()),
        "set_evm_rpc_principal",
    )
    .expect("set_evm_rpc_principal");
    update_any(pic, mock, "set_getlogs_max_range", Encode!(&1000u64).unwrap());
    update_any(pic, mock, "set_espace_receipt_fields", Encode!(&true).unwrap());
    let reg = RegisterChainArg {
        chain_id: ChainId(CONFLUX_CHAIN_ID),
        display_name: "ConfluxESpaceTestnet".to_string(),
        rpc_endpoints: vec!["https://evmtestnet.confluxrpc.com".to_string()],
        finality_depth: CONFLUX_FINALITY_DEPTH as u32,
        gas_strategy: GasStrategy::EvmEip1559 { max_priority_fee_gwei: 1, max_fee_gwei_ceiling: 100 },
        chain_native_decimals: 18,
        min_quorum_providers: Some(1),
    };
    decode_result(update_dev(pic, backend, "register_chain", Encode!(&reg).unwrap()), "register_chain")
        .expect("register_chain");
    decode_result(
        update_dev(pic, backend, "set_chain_contract", Encode!(&ChainId(CONFLUX_CHAIN_ID), &EVM_CONTRACT.to_string()).unwrap()),
        "set_chain_contract",
    )
    .expect("set_chain_contract");
    decode_result(
        update_dev(pic, backend, "set_manual_collateral_price", Encode!(&ChainId(CONFLUX_CHAIN_ID), &"CFX".to_string(), &15_000_000u64).unwrap()),
        "set_manual_collateral_price",
    )
    .expect("set_manual_collateral_price");
    decode_result(
        update_dev(pic, backend, "set_last_observed_block", Encode!(&ChainId(CONFLUX_CHAIN_ID), &seed).unwrap()),
        "set_last_observed_block",
    )
    .expect("set_last_observed_block");
    decode_result(
        update_dev(pic, backend, "set_burn_watch_poll_enabled", Encode!(&ChainId(CONFLUX_CHAIN_ID), &true).unwrap()),
        "set_burn_watch_poll_enabled",
    )
    .expect("set_burn_watch_poll_enabled");
}

/// Submit a signed intent as the ANONYMOUS caller; decode `Result<u64, _>`.
fn open_evm(pic: &PocketIc, backend: Principal, intent: &VaultIntent, sig: &[u8]) -> Result<u64, ProtocolError> {
    match pic
        .update_call(backend, Principal::anonymous(), "open_chain_vault_evm", Encode!(intent, &sig.to_vec()).unwrap())
        .expect("open_chain_vault_evm call")
    {
        WasmResult::Reply(b) => Decode!(&b, Result<u64, ProtocolError>).expect("decode open_evm"),
        WasmResult::Reject(msg) => panic!("open_chain_vault_evm rejected at transport (inspect_message?): {msg}"),
    }
}

/// Submit a signed borrow/withdraw/close intent as ANONYMOUS; decode `Result<(), _>`.
fn unit_evm(pic: &PocketIc, backend: Principal, method: &str, intent: &VaultIntent, sig: &[u8]) -> Result<(), ProtocolError> {
    match pic
        .update_call(backend, Principal::anonymous(), method, Encode!(intent, &sig.to_vec()).unwrap())
        .unwrap_or_else(|e| panic!("{method} call failed: {e}"))
    {
        WasmResult::Reply(b) => Decode!(&b, Result<(), ProtocolError>).unwrap_or_else(|e| panic!("decode {method}: {e}")),
        WasmResult::Reject(msg) => panic!("{method} rejected at transport (inspect_message?): {msg}"),
    }
}

#[test]
fn conflux_evm_self_serve_full_flow_and_rejections() {
    let (pic, backend, mock) = boot();
    let seed: u64 = 1_000_000;
    configure_conflux(&pic, backend, mock, seed);
    assert_supply(&pic, backend, 0, "after config");

    let (sk, owner) = evm_signer();
    let synthetic = synthetic_owner(
        rumi_protocol_backend::chains::config::ChainId(CONFLUX_CHAIN_ID),
        &owner,
    )
    .unwrap();

    // Block plan + settlement broadcast hash.
    let cursor1 = seed + SCAN_WINDOW;
    let head1 = cursor1 + CONFLUX_FINALITY_DEPTH + 24;
    update_any(&pic, mock, "set_blocks", Encode!(&head1, &head1).unwrap());
    update_any(&pic, mock, "set_next_send_hash", Encode!(&"0xevmmint1".to_string()).unwrap());

    // ECDSA probe — gated subset if PocketIC can't provision test_key_1.
    let settlement_addr = match update_dev(&pic, backend, "get_chain_settlement_address", Encode!(&ChainId(CONFLUX_CHAIN_ID)).unwrap()) {
        WasmResult::Reply(b) => match Decode!(&b, Result<String, ProtocolError>) {
            Ok(Ok(addr)) => Some(addr),
            _ => None,
        },
        WasmResult::Reject(_) => None,
    };
    let settlement_addr = match settlement_addr {
        Some(a) => a,
        None => {
            eprintln!("[evm self-serve] ECDSA unavailable; running gated subset (config + invariant 0)");
            // Even gated, a signed open must FAIL only at the derive (not auth):
            // verify the signature path is reached (returns an EvmAuth derive err).
            let intent = make_intent(IntentAction::Open, &owner, 0, 1_400 * E18, 100 * E8, 0);
            let sig = sign(&sk, &intent);
            assert!(matches!(open_evm(&pic, backend, &intent, &sig), Err(ProtocolError::EvmAuth(_))));
            assert_supply(&pic, backend, 0, "gated final");
            return;
        }
    };
    update_any(&pic, mock, "set_balance", Encode!(&settlement_addr, &candid::Nat::from(1_000_000u128 * E18)).unwrap());

    // ── OPEN (anonymous, EIP-712 signed, nonce 0) ────────────────────────────
    // $240 backing: after the +50 icUSD borrow below (debt 150), CR = 240/150 =
    // 160% stays above the 150% Conflux open/borrow gate (Increment 0 raised it
    // from 133% to mirror ICP's mint min CR).
    let collateral = 1_600u128 * E18; // $240 backing
    let open_intent = make_intent(IntentAction::Open, &owner, 0, collateral, 100 * E8, 0);
    let open_sig = sign(&sk, &open_intent);
    let vault_id = open_evm(&pic, backend, &open_intent, &open_sig).expect("open_evm Ok");
    let v = get_vault(&pic, backend, vault_id).expect("vault after open");
    assert_eq!(v.owner, synthetic, "vault owned by the synthetic principal");
    assert_eq!(v.owner_evm.as_deref(), Some(owner.to_lowercase().as_str()), "owner_evm stamped");
    assert_eq!(v.status, ChainVaultStatus::AwaitingDeposit);
    assert_eq!(v.mint_recipient, owner.to_lowercase(), "mint recipient == owner");
    let custody = v.custody_address.clone();
    assert_supply(&pic, backend, 0, "after open_evm");

    // ── deposit -> MintPending -> mint confirm (Open, supply 100e8) ──────────
    update_any(&pic, mock, "set_balance", Encode!(&custody, &candid::Nat::from(collateral)).unwrap());
    advance_and_tick(&pic, 2);
    assert_eq!(get_vault(&pic, backend, vault_id).unwrap().status, ChainVaultStatus::MintPending);
    advance_and_tick(&pic, 1); // submit
    update_any(&pic, mock, "set_receipt", Encode!(&"0xevmmint1".to_string(), &true, &cursor1).unwrap());
    push_mint_log(&pic, mock, vault_id, &owner, 100 * E8, "0xevmmint1", cursor1);
    advance_and_tick(&pic, 4); // confirm
    assert_eq!(get_vault(&pic, backend, vault_id).unwrap().status, ChainVaultStatus::Open);
    assert_supply(&pic, backend, 100 * E8, "after mint confirm");

    // ── BORROW (anonymous, nonce 1, +50e8) -> mint2 confirm (supply 150e8) ───
    update_any(&pic, mock, "set_next_send_hash", Encode!(&"0xevmmint2".to_string()).unwrap());
    let borrow_intent = make_intent(IntentAction::Borrow, &owner, vault_id, 0, 50 * E8, 1);
    let borrow_sig = sign(&sk, &borrow_intent);
    unit_evm(&pic, backend, "borrow_chain_vault_evm", &borrow_intent, &borrow_sig).expect("borrow_evm Ok");
    advance_and_tick(&pic, 1); // submit mint2
    // The settlement finality gate is `receipt_block <= observer cursor`; after
    // mint1 the cursor sits at cursor1, so the borrow mint receipt+log must land
    // at cursor1 too. This also puts a SECOND Mint log for this vault at cursor1,
    // exercising the per-op confirm's exact-tx-hash match (the M2 change that lets
    // a vault be minted to more than once).
    update_any(&pic, mock, "set_receipt", Encode!(&"0xevmmint2".to_string(), &true, &cursor1).unwrap());
    push_mint_log(&pic, mock, vault_id, &owner, 50 * E8, "0xevmmint2", cursor1);
    advance_and_tick(&pic, 4); // confirm mint2
    let v = get_vault(&pic, backend, vault_id).unwrap();
    assert_eq!(v.debt_e8s, candid::Nat::from(150 * E8), "borrow confirmed => debt 150e8");
    assert_supply(&pic, backend, 150 * E8, "after borrow mint confirm");

    // ── REPAY (burn full 150e8 on chain) -> debt 0, supply 0 ─────────────────
    let cursor2 = cursor1 + SCAN_WINDOW;
    let head2 = cursor2 + CONFLUX_FINALITY_DEPTH + 24;
    update_any(&pic, mock, "set_blocks", Encode!(&head2, &head2).unwrap());
    push_burn_log(&pic, mock, vault_id, &owner, 150 * E8, "0xevmburn1", cursor1 + 500);
    advance_and_tick(&pic, 3);
    assert_eq!(get_vault(&pic, backend, vault_id).unwrap().debt_e8s, candid::Nat::from(0u32), "repaid => debt 0");
    assert_supply(&pic, backend, 0, "after repay");

    // ── CLOSE (anonymous, nonce 2) -> Closing -> Closed ──────────────────────
    update_any(&pic, mock, "set_next_send_hash", Encode!(&"0xevmwd1".to_string()).unwrap());
    let close_intent = make_intent(IntentAction::Close, &owner, vault_id, 0, 0, 2);
    let close_sig = sign(&sk, &close_intent);
    unit_evm(&pic, backend, "close_chain_vault_evm", &close_intent, &close_sig).expect("close_evm Ok");
    assert_eq!(get_vault(&pic, backend, vault_id).unwrap().status, ChainVaultStatus::Closing, "close => Closing");
    advance_and_tick(&pic, 1); // submit withdrawal
    update_any(&pic, mock, "set_receipt", Encode!(&"0xevmwd1".to_string(), &true, &cursor2).unwrap());
    advance_and_tick(&pic, 4); // confirm
    assert_eq!(get_vault(&pic, backend, vault_id).unwrap().status, ChainVaultStatus::Closed, "close confirmed => Closed");
    assert_supply(&pic, backend, 0, "final (Closed)");

    // ── REJECTIONS ───────────────────────────────────────────────────────────
    // 1. Replay the original OPEN intent (nonce 0, already consumed) → bad nonce.
    match open_evm(&pic, backend, &open_intent, &open_sig) {
        Err(ProtocolError::EvmAuth(m)) => assert!(m.contains("nonce"), "expected nonce error, got {m}"),
        other => panic!("replayed open should fail with EvmAuth(nonce), got {other:?}"),
    }
    // 2. Wrong signer: a fresh OPEN intent (nonce 3) with a corrupted signature.
    let fresh = make_intent(IntentAction::Open, &owner, 0, collateral, 100 * E8, 3);
    let mut bad_sig = sign(&sk, &fresh);
    bad_sig[10] ^= 0xff; // corrupt r → recovers a different (or no) signer
    match open_evm(&pic, backend, &fresh, &bad_sig) {
        Err(ProtocolError::EvmAuth(_)) => {}
        other => panic!("corrupted-signature open should fail with EvmAuth, got {other:?}"),
    }

    eprintln!("[evm self-serve] FULL PASS: anonymous EIP-712 open/borrow/repay/close + replay/wrong-signer rejection on chain 71");
}
