//! AUTH-001 regression fence: notify_liquidatable_vaults must reject any caller
//! that is not the registered protocol canister.
//!
//! Audit report: audit-reports/2026-04-22-28e9896/verification-results.md (AUTH-001).
//! Bundles SP-004 and DOS-009 because all three are the same caller-gate hole on
//! the same endpoint.
//!
//! Before the Wave-2 fix, `src/stability_pool/src/lib.rs:153-166` logged a warning
//! and fell through when caller != protocol_canister_id. Any principal (including
//! Principal::anonymous()) could feed fabricated LiquidatableVaultInfo entries
//! through the SP's liquidation pipeline. The fix matches the receive_interest_revenue
//! pattern: reject the caller and return an empty result vector.
//!
//! This integration test deploys the real SP canister, registers a stablecoin and
//! a fake protocol principal, then calls notify_liquidatable_vaults from
//! Principal::anonymous() with a fabricated vault entry. It asserts:
//!  1. The call returns an empty Vec<LiquidationResult> (not Err, not panic, the
//!     signature is infallible).
//!  2. No LiquidationNotification event is appended to the pool event log
//!     attributing the rejected call. Allowing rejected callers to write events
//!     would itself be a DoS on the event log (DOS-009).

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Principal};
use icrc_ledger_types::icrc1::account::Account;
use pocket_ic::{PocketIcBuilder, WasmResult};
use stability_pool::types::*;

// ─── Candid types for ICRC-1 ledger init (same shape as pocket_ic_3usd.rs) ───

#[derive(CandidType, Deserialize)]
struct FeatureFlags { icrc2: bool }

#[derive(CandidType, Deserialize)]
struct ArchiveOptions {
    num_blocks_to_archive: u64,
    trigger_threshold: u64,
    controller_id: Principal,
    max_transactions_per_response: Option<u64>,
    max_message_size_bytes: Option<u64>,
    cycles_for_archive_creation: Option<u64>,
    node_max_memory_size_bytes: Option<u64>,
    more_controller_ids: Option<Vec<Principal>>,
}

#[derive(CandidType, Deserialize)]
enum MetadataValue { Nat(candid::Nat), Int(candid::Int), Text(String), Blob(Vec<u8>) }

#[derive(CandidType, Deserialize)]
struct LedgerInitArgs {
    minting_account: Account,
    fee_collector_account: Option<Account>,
    transfer_fee: candid::Nat,
    decimals: Option<u8>,
    max_memo_length: Option<u16>,
    token_name: String,
    token_symbol: String,
    metadata: Vec<(String, MetadataValue)>,
    initial_balances: Vec<(Account, candid::Nat)>,
    feature_flags: Option<FeatureFlags>,
    maximum_number_of_accounts: Option<u64>,
    accounts_overflow_trim_quantity: Option<u64>,
    archive_options: ArchiveOptions,
}

#[derive(CandidType, Deserialize)]
enum LedgerArg { Init(LedgerInitArgs) }

fn icrc1_ledger_wasm() -> Vec<u8> {
    include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec()
}

fn stability_pool_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/stability_pool.wasm").to_vec()
}

#[test]
fn auth_001_anon_caller_rejected() {
    let pic = PocketIcBuilder::new().with_application_subnet().build();

    let admin = Principal::self_authenticating(&[5, 6, 7, 8]);
    let minting_account = Principal::self_authenticating(&[100, 100, 100]);
    let fake_protocol = Principal::self_authenticating(&[9, 10, 11, 12]);
    let attacker = Principal::anonymous();

    // Deploy a single ckUSDT-like ledger, just enough to register a stablecoin.
    let ledger_id = pic.create_canister();
    pic.add_cycles(ledger_id, 2_000_000_000_000);
    let init = LedgerInitArgs {
        minting_account: Account { owner: minting_account, subaccount: None },
        fee_collector_account: None,
        transfer_fee: candid::Nat::from(0u64),
        decimals: Some(6),
        max_memo_length: Some(32),
        token_name: "ckUSDT".to_string(),
        token_symbol: "ckUSDT".to_string(),
        metadata: vec![],
        initial_balances: vec![],
        feature_flags: Some(FeatureFlags { icrc2: true }),
        maximum_number_of_accounts: None,
        accounts_overflow_trim_quantity: None,
        archive_options: ArchiveOptions {
            num_blocks_to_archive: 2000,
            trigger_threshold: 1000,
            controller_id: admin,
            max_transactions_per_response: None,
            max_message_size_bytes: None,
            cycles_for_archive_creation: None,
            node_max_memory_size_bytes: None,
            more_controller_ids: None,
        },
    };
    pic.install_canister(
        ledger_id,
        icrc1_ledger_wasm(),
        encode_args((LedgerArg::Init(init),)).unwrap(),
        None,
    );

    // Deploy the stability pool with `fake_protocol` as the registered protocol.
    let sp_id = pic.create_canister();
    pic.add_cycles(sp_id, 2_000_000_000_000);
    pic.install_canister(
        sp_id,
        stability_pool_wasm(),
        encode_one(StabilityPoolInitArgs {
            protocol_canister_id: fake_protocol,
            authorized_admins: vec![admin],
        }).unwrap(),
        None,
    );

    // Register the ckUSDT ledger as an active stablecoin so the call cannot
    // bail out for a "no stablecoin registered" reason.
    let register_result = pic.update_call(
        sp_id,
        admin,
        "register_stablecoin",
        encode_one(StablecoinConfig {
            ledger_id,
            symbol: "ckUSDT".to_string(),
            decimals: 6,
            priority: 2,
            is_active: true,
            transfer_fee: Some(0),
            is_lp_token: None,
            underlying_pool: None,
        }).unwrap(),
    ).expect("register_stablecoin call failed");
    match register_result {
        WasmResult::Reply(bytes) => {
            let r: Result<(), StabilityPoolError> = decode_one(&bytes).expect("decode register");
            r.expect("register_stablecoin error");
        }
        WasmResult::Reject(msg) => panic!("register_stablecoin rejected: {}", msg),
    }

    // Snapshot pool event count BEFORE the rejected call.
    let events_before: u64 = match pic.query_call(
        sp_id,
        Principal::anonymous(),
        "get_pool_event_count",
        encode_args(()).unwrap(),
    ) {
        Ok(WasmResult::Reply(bytes)) => decode_one(&bytes).expect("decode count"),
        Ok(WasmResult::Reject(msg)) => panic!("pool_event_count rejected: {}", msg),
        Err(e) => panic!("pool_event_count failed: {:?}", e),
    };

    // Drive the attack: anonymous caller submits a fabricated liquidatable vault.
    let fabricated = LiquidatableVaultInfo {
        vault_id: 999_999,
        collateral_type: ledger_id,
        debt_amount: 100_000_000,
        collateral_amount: 100_000_000,
        recommended_liquidation_amount: 100_000_000,
        collateral_price_e8s: 1_00000000,
    };
    let result = pic.update_call(
        sp_id,
        attacker,
        "notify_liquidatable_vaults",
        encode_one(vec![fabricated]).unwrap(),
    ).expect("notify_liquidatable_vaults call failed at transport layer");

    let liquidations: Vec<LiquidationResult> = match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode liquidation result"),
        WasmResult::Reject(msg) => panic!(
            "notify_liquidatable_vaults rejected at transport layer (expected empty Vec, got reject): {}",
            msg
        ),
    };

    assert!(
        liquidations.is_empty(),
        "AUTH-001: anonymous caller must produce no liquidation results (got {} entries)",
        liquidations.len(),
    );

    // No event should have been written attributing the rejected call.
    let events_after: u64 = match pic.query_call(
        sp_id,
        Principal::anonymous(),
        "get_pool_event_count",
        encode_args(()).unwrap(),
    ) {
        Ok(WasmResult::Reply(bytes)) => decode_one(&bytes).expect("decode count"),
        Ok(WasmResult::Reject(msg)) => panic!("pool_event_count rejected: {}", msg),
        Err(e) => panic!("pool_event_count failed: {:?}", e),
    };

    assert_eq!(
        events_before, events_after,
        "AUTH-001 / DOS-009: rejected calls must not append to the pool event log \
         (event count went from {} to {})",
        events_before, events_after,
    );
}
