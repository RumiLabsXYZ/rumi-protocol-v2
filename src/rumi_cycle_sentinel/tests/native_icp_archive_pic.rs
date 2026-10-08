//! Ignored PocketIC regression for Sentinel's native ICP `query_blocks`
//! decoder and ledger-supplied archive callback. It installs the pinned NNS
//! ledger release, archives a real ICRC-1 transfer, then reconciles that exact
//! receipt through Sentinel's production `attach_block_proof` method.
//!
//! Required environment:
//! - `POCKET_IC_BIN`: local PocketIC server executable.
//! - `RUMI_TEST_NNS_LEDGER_WASM_GZ`: pinned official NNS ledger gzip.
//! - `RUMI_TEST_SENTINEL_WASM`: test_endpoints Sentinel Wasm.
//! - `RUMI_TEST_SENTINEL_MOCK_WASM`: Cycle Sentinel test mock Wasm.
//!
//! Run through `scripts/test-sentinel-native-icp-archive-pic.sh`.

use candid::{CandidType, Decode, Encode, IDLArgs, IDLValue, Nat, Principal};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use serde::Deserialize;
use sha2::{Digest, Sha224, Sha256};
use std::{
    process::Command,
    time::{Duration, UNIX_EPOCH},
};

const LEDGER_TEXT: &str = "ryjl3-tyaaa-aaaaa-aaaba-cai";
const CMC_TEXT: &str = "rkp4c-7iaaa-aaaaa-aaaca-cai";
const TARGET_TEXT: &str = "bfnu3-6aaaa-aaaab-qhanq-cai";
const LEDGER_SHA256: &str = "51f4be010f23064137defacd627ffbec024c5133210c68ca3b80ab8f257101d6";

#[derive(CandidType)]
struct InitArgs {
    signers: Vec<Principal>,
    approval_threshold: u32,
    global_policy: GlobalPolicyArgs,
}

#[derive(CandidType)]
struct GlobalPolicyArgs {
    global_daily_cap_cycles: Nat,
    sample_interval_secs: u64,
    stale_after_secs: u64,
    min_icp_reserve_e8s: Nat,
    timelocks: Timelocks,
    self_recovery_policy: SelfRecoveryPolicyArgs,
}

#[derive(CandidType)]
struct Timelocks {
    target_registry_secs: u64,
    spend_policy_secs: u64,
    signer_change_secs: u64,
    unpause_secs: u64,
}

#[derive(CandidType)]
struct SelfRecoveryPolicyArgs {
    protected_reserve_cycles: Nat,
    daily_cap_cycles: Nat,
    low_balance_threshold_cycles: Nat,
    refill_cycles: Nat,
}

#[derive(CandidType)]
struct MockInit {
    role: MockRole,
}

#[derive(CandidType)]
enum MockRole {
    Cmc,
}

#[derive(CandidType)]
enum TestIcpState {
    LedgerSubmitted,
}

#[derive(CandidType)]
enum TestOperationState {
    Icp(TestIcpState),
}

#[derive(CandidType, Deserialize)]
struct FixedBytes32([u8; 32]);

#[derive(CandidType, Deserialize)]
enum IcpCmcDelivery {
    DirectTopUp,
    SharedReserveMint,
}

#[derive(CandidType, Deserialize)]
#[allow(dead_code)]
struct IcpCmcSnapshot {
    source_principal: Principal,
    ledger_principal: Principal,
    cmc_principal: Principal,
    source_subaccount: Option<FixedBytes32>,
    cmc_account_identifier: FixedBytes32,
    target_canister: Principal,
    delivery: IcpCmcDelivery,
    amount_e8s: u64,
    fee_e8s: u64,
    memo: u64,
    created_at_time_ns: u64,
    rate_xdr_permyriad_per_icp: u64,
    rate_timestamp_secs: u64,
    expected_cycles: u128,
}

#[derive(CandidType)]
struct LedgerInitArgs {
    minting_account: String,
    icrc1_minting_account: Option<Account>,
    initial_values: Vec<(String, IcpTokens)>,
    max_message_size_bytes: Option<u64>,
    transaction_window: Option<LedgerDuration>,
    archive_options: Option<LedgerArchiveOptions>,
    send_whitelist: Vec<Principal>,
    transfer_fee: Option<IcpTokens>,
    token_symbol: Option<String>,
    token_name: Option<String>,
    feature_flags: Option<LedgerFeatureFlags>,
}

#[derive(CandidType)]
struct Account {
    owner: Principal,
    subaccount: Option<[u8; 32]>,
}

#[derive(CandidType)]
struct LedgerDuration {
    secs: u64,
    nanos: u32,
}

#[derive(CandidType)]
struct LedgerFeatureFlags {
    icrc2: bool,
}

#[derive(CandidType)]
struct LedgerArchiveOptions {
    num_blocks_to_archive: u64,
    max_transactions_per_response: Option<u64>,
    trigger_threshold: u64,
    max_message_size_bytes: Option<u64>,
    cycles_for_archive_creation: Option<u64>,
    node_max_memory_size_bytes: Option<u64>,
    controller_id: Principal,
    more_controller_ids: Option<Vec<Principal>>,
}

#[derive(CandidType)]
enum LedgerArg {
    Init(LedgerInitArgs),
}

#[derive(CandidType)]
struct IcpTokens {
    e8s: u64,
}

#[derive(CandidType)]
struct IcrcTransferArg {
    from_subaccount: Option<[u8; 32]>,
    to: Account,
    amount: Nat,
    fee: Option<Nat>,
    memo: Option<Vec<u8>>,
    created_at_time: Option<u64>,
}

#[derive(CandidType)]
struct GetBlocksArgs {
    start: u64,
    length: u64,
}

fn principal(text: &str) -> Principal {
    Principal::from_text(text).expect("valid fixed principal")
}

fn load_nns_ledger_wasm() -> Vec<u8> {
    let path = std::env::var("RUMI_TEST_NNS_LEDGER_WASM_GZ")
        .expect("set RUMI_TEST_NNS_LEDGER_WASM_GZ to the pinned NNS ledger release gzip");
    let gzip = std::fs::read(&path).unwrap_or_else(|error| panic!("read {path}: {error}"));
    assert_eq!(
        format!("{:x}", Sha256::digest(&gzip)),
        LEDGER_SHA256,
        "pinned NNS ledger release gzip hash"
    );
    let output = Command::new("gzip")
        .args(["-dc", &path])
        .output()
        .expect("run gzip to decompress the pinned NNS ledger");
    assert!(
        output.status.success(),
        "gzip failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn wasm_from_env(name: &str) -> Vec<u8> {
    let path = std::env::var(name).unwrap_or_else(|_| panic!("set {name} to the built test Wasm"));
    std::fs::read(&path).unwrap_or_else(|error| panic!("read {path}: {error}"))
}

fn account_identifier(owner: Principal, subaccount: Option<[u8; 32]>) -> String {
    let subaccount = subaccount.unwrap_or([0; 32]);
    let mut hasher = Sha224::new();
    hasher.update(b"\x0Aaccount-id");
    hasher.update(owner.as_slice());
    hasher.update(subaccount);
    let hash = hasher.finalize();
    let mut crc = !0u32;
    for byte in &hash {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    let mut identifier = Vec::with_capacity(32);
    identifier.extend_from_slice(&(!crc).to_be_bytes());
    identifier.extend_from_slice(&hash);
    identifier
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn call_update(
    pic: &PocketIc,
    canister: Principal,
    caller: Principal,
    method: &str,
    args: Vec<u8>,
) -> Vec<u8> {
    match pic
        .update_call(canister, caller, method, args)
        .expect("PocketIC update call")
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("{method} rejected: {message}"),
    }
}

fn result_block_index(bytes: &[u8]) -> u64 {
    let values = IDLArgs::from_bytes(bytes).expect("decode ledger transfer response");
    let IDLValue::Variant(result) = &values.args[0] else {
        panic!("transfer response is not Result")
    };
    assert_eq!(
        result.0.id.get_id(),
        candid::idl_hash("Ok"),
        "ledger transfer failed: {values:?}"
    );
    match &result.0.val {
        IDLValue::Nat(value) => u64::try_from(value.0.clone()).expect("block index fits u64"),
        IDLValue::Nat64(value) => *value,
        other => panic!("unexpected block index {other:?}"),
    }
}

fn assert_ok_result(bytes: &[u8], label: &str) {
    let values =
        IDLArgs::from_bytes(bytes).unwrap_or_else(|error| panic!("decode {label}: {error}"));
    let IDLValue::Variant(result) = &values.args[0] else {
        panic!("{label} is not a Result")
    };
    assert_eq!(
        result.0.id.get_id(),
        candid::idl_hash("Ok"),
        "{label} failed: {values:?}"
    );
}

fn record_field<'a>(value: &'a IDLValue, name: &str) -> &'a IDLValue {
    let IDLValue::Record(fields) = value else {
        panic!("expected record, got {value:?}")
    };
    fields
        .iter()
        .find(|field| field.id.get_id() == candid::idl_hash(name))
        .map(|field| &field.val)
        .unwrap_or_else(|| panic!("missing {name} field in {value:?}"))
}

fn idl_u64(value: &IDLValue) -> u64 {
    match value {
        IDLValue::Nat64(value) => *value,
        IDLValue::Nat(value) => u64::try_from(value.0.clone()).expect("value fits u64"),
        other => panic!("expected nat64/nat, got {other:?}"),
    }
}

fn assert_archive_only_response(bytes: &[u8], block_index: u64) {
    let decoded = IDLArgs::from_bytes(bytes).expect("decode official query_blocks response");
    let response = &decoded.args[0];
    let IDLValue::Vec(blocks) = record_field(response, "blocks") else {
        panic!("query_blocks blocks is not a vector")
    };
    assert!(
        blocks.is_empty(),
        "target block must have left the ledger's hot range"
    );
    let IDLValue::Vec(archives) = record_field(response, "archived_blocks") else {
        panic!("query_blocks archived_blocks is not a vector")
    };
    assert!(
        archives.iter().any(|archive| {
            let start = idl_u64(record_field(archive, "start"));
            let length = idl_u64(record_field(archive, "length"));
            start <= block_index
                && start
                    .checked_add(length)
                    .is_some_and(|end| block_index < end)
        }),
        "ledger did not return an archive descriptor covering block {block_index}: {response:?}"
    );
}

#[test]
#[ignore = "requires the pinned official NNS ledger gzip, test_endpoints Wasms, and PocketIC server; run scripts/test-sentinel-native-icp-archive-pic.sh"]
fn sentinel_reconciles_exact_native_icp_receipt_from_official_archive_callback() {
    let ledger_wasm = load_nns_ledger_wasm();
    let sentinel_wasm = wasm_from_env("RUMI_TEST_SENTINEL_WASM");
    let mock_wasm = wasm_from_env("RUMI_TEST_SENTINEL_MOCK_WASM");
    let mut builder = PocketIcBuilder::new()
        .with_nns_subnet()
        .with_ii_subnet()
        .with_fiduciary_subnet()
        .with_application_subnet();
    if let Ok(server_url) = std::env::var("SENTINEL_POCKET_IC_SERVER_URL") {
        builder = builder.with_server_url(server_url.parse().expect("PocketIC server URL"));
    }
    let pic = builder.build();
    pic.set_time(UNIX_EPOCH + Duration::from_secs(1_711_324_800));

    let sentinel = pic.create_canister();
    let signer = Principal::from_slice(&[9; 10]);
    pic.add_cycles(sentinel, 100_000_000_000_000);

    let ledger = principal(LEDGER_TEXT);
    pic.create_canister_with_id(None, None, ledger)
        .expect("create fixed NNS ledger principal");
    pic.add_cycles(ledger, 5_000_000_000_000_000);
    let source_identifier = account_identifier(sentinel, None);
    let ledger_init = LedgerArg::Init(LedgerInitArgs {
        minting_account: account_identifier(Principal::management_canister(), None),
        icrc1_minting_account: Some(Account {
            owner: Principal::management_canister(),
            subaccount: None,
        }),
        initial_values: vec![(
            source_identifier,
            IcpTokens {
                e8s: 1_000_000_000_000,
            },
        )],
        max_message_size_bytes: Some(1_048_576),
        transaction_window: None,
        archive_options: Some(LedgerArchiveOptions {
            num_blocks_to_archive: 1,
            max_transactions_per_response: Some(100),
            trigger_threshold: 2,
            max_message_size_bytes: Some(1_048_576),
            cycles_for_archive_creation: Some(1_000_000_000_000),
            node_max_memory_size_bytes: Some(1_073_741_824),
            controller_id: signer,
            more_controller_ids: None,
        }),
        send_whitelist: Vec::new(),
        transfer_fee: Some(IcpTokens { e8s: 1 }),
        token_symbol: Some("ICP".into()),
        token_name: Some("Internet Computer".into()),
        feature_flags: Some(LedgerFeatureFlags { icrc2: true }),
    });
    pic.install_canister(ledger, ledger_wasm, Encode!(&ledger_init).unwrap(), None);

    let cmc = principal(CMC_TEXT);
    pic.create_canister_with_id(None, None, cmc)
        .expect("create fixed CMC fixture principal");
    pic.add_cycles(cmc, 10_000_000_000_000);
    pic.install_canister(
        cmc,
        mock_wasm,
        Encode!(&MockInit {
            role: MockRole::Cmc
        })
        .unwrap(),
        None,
    );

    let init = InitArgs {
        signers: vec![signer],
        approval_threshold: 1,
        global_policy: GlobalPolicyArgs {
            global_daily_cap_cycles: Nat::from(10_000_000_000_000u128),
            sample_interval_secs: 1,
            stale_after_secs: 10,
            min_icp_reserve_e8s: Nat::from(0u8),
            timelocks: Timelocks {
                target_registry_secs: 1,
                spend_policy_secs: 1,
                signer_change_secs: 1,
                unpause_secs: 1,
            },
            self_recovery_policy: SelfRecoveryPolicyArgs {
                protected_reserve_cycles: Nat::from(10_000_000_000_000u128),
                daily_cap_cycles: Nat::from(10_000_000_000_000u128),
                low_balance_threshold_cycles: Nat::from(1_000_000_000_000u128),
                refill_cycles: Nat::from(1_000_000_000_000u128),
            },
        },
    };
    pic.install_canister(sentinel, sentinel_wasm, Encode!(&init).unwrap(), None);

    let target = principal(TARGET_TEXT);
    let operation = call_update(
        &pic,
        sentinel,
        signer,
        "test_inject_operation",
        Encode!(
            &target,
            &TestOperationState::Icp(TestIcpState::LedgerSubmitted)
        )
        .unwrap(),
    );
    let operation_id: u64 = Decode!(&operation, Result<u64, String>)
        .expect("decode injected operation")
        .expect("inject ICP LedgerSubmitted fixture");
    let snapshot_bytes = match pic
        .query_call(
            sentinel,
            signer,
            "test_get_icp_snapshot",
            Encode!(&operation_id).unwrap(),
        )
        .expect("query immutable ICP snapshot")
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("snapshot query rejected: {message}"),
    };
    let snapshot: IcpCmcSnapshot = Decode!(&snapshot_bytes, Result<Option<IcpCmcSnapshot>, String>)
        .expect("decode immutable snapshot query")
        .expect("authorized snapshot query")
        .expect("injected ICP operation has a snapshot");
    assert_eq!(snapshot.source_principal, sentinel);
    assert_eq!(snapshot.ledger_principal, ledger);
    assert_eq!(snapshot.target_canister, target);

    let transfer = IcrcTransferArg {
        from_subaccount: None,
        to: Account {
            owner: snapshot.cmc_principal,
            subaccount: Some(snapshot.cmc_account_identifier.0),
        },
        amount: Nat::from(snapshot.amount_e8s),
        fee: Some(Nat::from(snapshot.fee_e8s)),
        memo: Some(snapshot.memo.to_le_bytes().to_vec()),
        created_at_time: Some(snapshot.created_at_time_ns),
    };
    let block_index = result_block_index(&call_update(
        &pic,
        ledger,
        sentinel,
        "icrc1_transfer",
        Encode!(&transfer).unwrap(),
    ));

    // Subsequent real transfers force the official ledger to move the exact
    // snapshot block into its own archive canister.
    for index in 0..12u64 {
        let filler = IcrcTransferArg {
            from_subaccount: None,
            to: Account {
                owner: signer,
                subaccount: None,
            },
            amount: Nat::from(1u64),
            fee: Some(Nat::from(snapshot.fee_e8s)),
            memo: Some(format!("sentinel-archive-filler-{index}").into_bytes()),
            created_at_time: Some(snapshot.created_at_time_ns + index + 1),
        };
        let _ = result_block_index(&call_update(
            &pic,
            ledger,
            sentinel,
            "icrc1_transfer",
            Encode!(&filler).unwrap(),
        ));
        for _ in 0..20 {
            pic.tick();
        }
        let response = match pic
            .query_call(
                ledger,
                Principal::anonymous(),
                "query_blocks",
                Encode!(&GetBlocksArgs {
                    start: block_index,
                    length: 1
                })
                .unwrap(),
            )
            .expect("query official ledger block range")
        {
            WasmResult::Reply(bytes) => bytes,
            WasmResult::Reject(message) => panic!("official query_blocks rejected: {message}"),
        };
        let decoded = IDLArgs::from_bytes(&response).expect("decode official query_blocks reply");
        let blocks = record_field(&decoded.args[0], "blocks");
        let IDLValue::Vec(blocks) = blocks else {
            panic!("blocks is not a vector")
        };
        if blocks.is_empty() {
            assert_archive_only_response(&response, block_index);
            break;
        }
        assert!(
            index < 11,
            "official NNS ledger did not archive block {block_index}"
        );
    }

    let attached = call_update(
        &pic,
        sentinel,
        signer,
        "attach_block_proof",
        Encode!(&operation_id, &block_index).unwrap(),
    );
    assert_ok_result(&attached, "Sentinel production proof attachment");
}
