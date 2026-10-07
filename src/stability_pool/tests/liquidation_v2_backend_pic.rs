//! PocketIC end-to-end proof for the V2 Stability Pool liquidation rail.
//!
//! This deliberately installs the real backend, real Stability Pool and the
//! repository's official ICRC ledger. It opens an ICP-backed vault, makes it
//! liquidatable through the XRC mock, deposits icUSD into the pool, then asks
//! the pool to liquidate and checks both ledger movements and vault state.

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc3::blocks::{GetBlocksRequest, GetBlocksResult};
use pocket_ic::common::rest::{CanisterHttpReply, CanisterHttpResponse, MockCanisterHttpResponse};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use rumi_3pool::types::{ThreePoolInitArgs, TokenConfig};
use sha2::{Digest, Sha224, Sha256};
use stability_pool::types::{
    CollateralInfo, CollateralStatus, SpLiquidationToken, StabilityPoolInitArgs, StablecoinConfig,
};
use std::{
    collections::HashMap,
    process::Command,
    time::{Duration, SystemTime},
};

const LEDGER_FEE: u64 = 10_000;

fn backend_wasm() -> Vec<u8> {
    std::env::var_os("RUMI_THREE_USD_REFUND_BACKEND_WASM")
        .or_else(|| std::env::var_os("RUMI_THREE_USD_BACKEND_WASM"))
        .map(std::path::PathBuf::from)
        .map(std::fs::read)
        .unwrap_or_else(|| {
            std::fs::read(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm"
            ))
        })
        .unwrap()
}
fn positive_refund_pic_enabled() -> bool {
    std::env::var_os("RUMI_THREE_USD_REFUND_BACKEND_WASM").is_some()
}
fn pool_wasm() -> Vec<u8> {
    std::env::var_os("RUMI_THREE_USD_STABILITY_POOL_WASM")
        .map(std::path::PathBuf::from)
        .map(std::fs::read)
        .unwrap_or_else(|| {
            std::fs::read(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../target/wasm32-unknown-unknown/release/stability_pool.wasm"
            ))
        })
        .unwrap()
}
fn three_pool_wasm() -> Vec<u8> {
    std::env::var_os("RUMI_THREE_USD_3POOL_WASM")
        .map(std::path::PathBuf::from)
        .map(std::fs::read)
        .unwrap_or_else(|| {
            std::fs::read(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../target/wasm32-unknown-unknown/release/rumi_3pool.wasm"
            ))
        })
        .unwrap()
}
fn ledger_wasm() -> Vec<u8> {
    include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec()
}
fn xrc_wasm() -> Vec<u8> {
    include_bytes!("../../xrc_demo/xrc/xrc.wasm").to_vec()
}

fn admin() -> Principal {
    Principal::self_authenticating(b"sp-v2-pic-admin")
}
fn user() -> Principal {
    Principal::self_authenticating(b"sp-v2-pic-user")
}
fn treasury() -> Principal {
    Principal::self_authenticating(b"sp-v2-pic-treasury")
}
fn account(owner: Principal) -> Account {
    Account {
        owner,
        subaccount: None,
    }
}

#[derive(CandidType, Deserialize)]
struct FeatureFlags {
    icrc2: bool,
}
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
struct NativeIcpInitArgs {
    minting_account: String,
    icrc1_minting_account: Option<Account>,
    initial_values: Vec<(String, IcpTokens)>,
    max_message_size_bytes: Option<u64>,
    transaction_window: Option<LedgerDuration>,
    archive_options: Option<NativeArchiveOptions>,
    send_whitelist: Vec<Principal>,
    transfer_fee: Option<IcpTokens>,
    token_symbol: Option<String>,
    token_name: Option<String>,
    feature_flags: Option<FeatureFlags>,
}
#[derive(CandidType, Deserialize)]
enum NativeLedgerArg {
    #[serde(rename = "Init")]
    Init(NativeIcpInitArgs),
}
#[derive(CandidType, Deserialize)]
struct IcpTokens {
    e8s: u64,
}
#[derive(CandidType, Deserialize)]
struct LedgerDuration {
    secs: u64,
    nanos: u32,
}
#[derive(CandidType, Deserialize)]
struct NativeArchiveOptions {
    num_blocks_to_archive: u64,
    max_transactions_per_response: Option<u64>,
    trigger_threshold: u64,
    max_message_size_bytes: Option<u64>,
    cycles_for_archive_creation: Option<u64>,
    node_max_memory_size_bytes: Option<u64>,
    controller_id: Principal,
    more_controller_ids: Option<Vec<Principal>>,
}
#[derive(CandidType, Deserialize)]
struct MetadataValue {
    #[serde(rename = "Text")]
    text: Option<String>,
    #[serde(rename = "Nat")]
    nat: Option<Nat>,
    #[serde(rename = "Int")]
    int: Option<i64>,
    #[serde(rename = "Blob")]
    blob: Option<Vec<u8>>,
}
#[derive(CandidType, Deserialize)]
struct LedgerInitArgs {
    minting_account: Account,
    fee_collector_account: Option<Account>,
    transfer_fee: Nat,
    decimals: Option<u8>,
    max_memo_length: Option<u16>,
    token_name: String,
    token_symbol: String,
    metadata: Vec<(String, MetadataValue)>,
    initial_balances: Vec<(Account, Nat)>,
    feature_flags: Option<FeatureFlags>,
    maximum_number_of_accounts: Option<u64>,
    accounts_overflow_trim_quantity: Option<u64>,
    archive_options: ArchiveOptions,
}
#[derive(CandidType, Deserialize)]
enum LedgerArg {
    #[serde(rename = "Init")]
    Init(LedgerInitArgs),
}
#[derive(CandidType, Deserialize)]
struct MockXrc {
    rates: HashMap<String, u64>,
}
#[derive(CandidType, Deserialize)]
struct OpenVaultSuccess {
    vault_id: u64,
}
#[derive(CandidType, Deserialize)]
struct VaultArg {
    vault_id: u64,
    amount: u64,
}
fn ledger(
    pic: &PocketIc,
    mint: Principal,
    fee: u64,
    symbol: &str,
    balances: Vec<(Account, Nat)>,
    controller: Principal,
) -> Principal {
    let id = pic.create_canister();
    pic.add_cycles(id, 2_000_000_000_000);
    let args = LedgerInitArgs {
        minting_account: account(mint),
        fee_collector_account: None,
        transfer_fee: Nat::from(fee),
        decimals: Some(8),
        max_memo_length: Some(64),
        token_name: symbol.into(),
        token_symbol: symbol.into(),
        metadata: vec![],
        initial_balances: balances,
        feature_flags: Some(FeatureFlags { icrc2: true }),
        maximum_number_of_accounts: None,
        accounts_overflow_trim_quantity: None,
        archive_options: ArchiveOptions {
            num_blocks_to_archive: 2_000,
            trigger_threshold: 1_000,
            controller_id: controller,
            max_transactions_per_response: None,
            max_message_size_bytes: None,
            cycles_for_archive_creation: None,
            node_max_memory_size_bytes: None,
            more_controller_ids: None,
        },
    };
    pic.install_canister(
        id,
        ledger_wasm(),
        encode_args((LedgerArg::Init(args),)).unwrap(),
        None,
    );
    id
}

fn native_icp_ledger_wasm() -> Vec<u8> {
    const EXPECTED_GZIP_SHA256: &str =
        "51f4be010f23064137defacd627ffbec024c5133210c68ca3b80ab8f257101d6";
    let path = std::env::var("RUMI_TEST_NNS_LEDGER_WASM_GZ")
        .unwrap_or_else(|_| "/private/tmp/rumi-nns-ledger-69b755.wasm.gz".into());
    let gzip = std::fs::read(&path).unwrap_or_else(|error| panic!("read {path}: {error}"));
    assert_eq!(
        format!("{:x}", Sha256::digest(&gzip)),
        EXPECTED_GZIP_SHA256,
        "pinned official NNS ledger gzip hash"
    );
    let output = Command::new("gzip")
        .args(["-dc", &path])
        .output()
        .expect("run gzip to decompress pinned official NNS ledger");
    assert!(
        output.status.success(),
        "gzip failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn native_icp_account_identifier(owner: Principal) -> String {
    let mut hasher = Sha224::new();
    hasher.update(b"\x0Aaccount-id");
    hasher.update(owner.as_slice());
    hasher.update([0u8; 32]);
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

fn deploy_native_icp_ledger(
    pic: &PocketIc,
    initial_owner: Principal,
    initial_balance_e8s: u64,
    controller: Principal,
) -> Principal {
    let ledger = Principal::from_text("ryjl3-tyaaa-aaaaa-aaaba-cai").unwrap();
    pic.create_canister_with_id(None, None, ledger)
        .expect("create canonical NNS ledger principal");
    pic.add_cycles(ledger, 5_000_000_000_000_000);
    let minting_account = Principal::management_canister();
    let init = NativeLedgerArg::Init(NativeIcpInitArgs {
        minting_account: native_icp_account_identifier(minting_account),
        icrc1_minting_account: Some(account(minting_account)),
        initial_values: vec![(
            native_icp_account_identifier(initial_owner),
            IcpTokens {
                e8s: initial_balance_e8s,
            },
        )],
        max_message_size_bytes: Some(1_048_576),
        transaction_window: None,
        archive_options: Some(NativeArchiveOptions {
            num_blocks_to_archive: 1_000,
            max_transactions_per_response: Some(100),
            trigger_threshold: 1_000,
            max_message_size_bytes: Some(1_048_576),
            cycles_for_archive_creation: Some(1_000_000_000_000),
            node_max_memory_size_bytes: Some(1_073_741_824),
            controller_id: controller,
            more_controller_ids: None,
        }),
        send_whitelist: Vec::new(),
        transfer_fee: Some(IcpTokens { e8s: LEDGER_FEE }),
        token_symbol: Some("ICP".into()),
        token_name: Some("Internet Computer".into()),
        feature_flags: Some(FeatureFlags { icrc2: true }),
    });
    pic.install_canister(
        ledger,
        native_icp_ledger_wasm(),
        encode_args((init,)).unwrap(),
        None,
    );
    ledger
}

fn call(
    pic: &PocketIc,
    canister: Principal,
    sender: Principal,
    method: &str,
    args: Vec<u8>,
) -> WasmResult {
    pic.update_call(canister, sender, method, args)
        .unwrap_or_else(|e| panic!("{method}: {e}"))
}
fn result<T: CandidType + for<'a> Deserialize<'a>>(reply: WasmResult, method: &str) -> T {
    match reply {
        WasmResult::Reply(bytes) => {
            decode_one(&bytes).unwrap_or_else(|e| panic!("decode {method}: {e}"))
        }
        WasmResult::Reject(message) => panic!("{method} rejected: {message}"),
    }
}
fn balance(pic: &PocketIc, ledger: Principal, owner: Principal) -> u64 {
    let bytes = pic
        .query_call(
            ledger,
            Principal::anonymous(),
            "icrc1_balance_of",
            encode_one(account(owner)).unwrap(),
        )
        .unwrap();
    let nat: Nat = result(bytes, "icrc1_balance_of");
    nat.0.try_into().unwrap()
}

fn wait_for_canister_http(
    pic: &PocketIc,
    url: &str,
) -> pocket_ic::common::rest::CanisterHttpRequest {
    for _ in 0..500 {
        pic.tick();
        if let Some(request) = pic
            .get_canister_http()
            .into_iter()
            .find(|request| request.url == url)
        {
            return request;
        }
    }
    panic!("timed out waiting for PocketIC HTTP outcall {url}");
}

fn wait_for_canister_http_after(
    pic: &PocketIc,
    url: &str,
    previous_request_id: u64,
) -> pocket_ic::common::rest::CanisterHttpRequest {
    for _ in 0..500 {
        pic.tick();
        if let Some(request) = pic
            .get_canister_http()
            .into_iter()
            .find(|request| request.url == url && request.request_id != previous_request_id)
        {
            return request;
        }
    }
    panic!("timed out waiting for a new PocketIC HTTP outcall {url}");
}

fn release_canister_http(pic: &PocketIc, request: pocket_ic::common::rest::CanisterHttpRequest) {
    pic.mock_canister_http_response(MockCanisterHttpResponse {
        subnet_id: request.subnet_id,
        request_id: request.request_id,
        response: CanisterHttpResponse::CanisterHttpReply(CanisterHttpReply {
            status: 200,
            headers: vec![],
            body: b"released".to_vec(),
        }),
        additional_responses: vec![],
    });
}

fn fee_absent_or_zero(value: Option<&icrc_ledger_types::icrc::generic_value::ICRC3Value>) -> bool {
    match value {
        None => true,
        Some(icrc_ledger_types::icrc::generic_value::ICRC3Value::Nat(fee)) => {
            fee == &Nat::from(0u64)
        }
        _ => false,
    }
}
fn now_ns(pic: &PocketIc) -> u64 {
    pic.get_time()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("PocketIC time follows Unix epoch")
        .as_nanos()
        .try_into()
        .expect("PocketIC timestamp fits u64")
}

#[test]
fn official_icrc_ledger_minting_account_pull_and_mint_refund_preserve_receipt_fields() {
    let pic = PocketIcBuilder::new().with_nns_subnet().build();
    let backend = pic.create_canister();
    let sp = Principal::self_authenticating(b"v2-ledger-probe-sp");
    let admin = admin();
    let ledger_id = ledger(&pic, backend, LEDGER_FEE, "icUSD", vec![], admin);

    // The backend principal is the ledger minting account. Mint 5 icUSD to
    // the SP, then let that same principal act as ICRC-2 spender and pull 2.
    let mint_to_sp = icrc_ledger_types::icrc1::transfer::TransferArg {
        from_subaccount: None,
        to: account(sp),
        fee: None,
        created_at_time: None,
        memo: None,
        amount: Nat::from(500_000_000u64),
    };
    let minted: Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError> = result(
        call(
            &pic,
            ledger_id,
            backend,
            "icrc1_transfer",
            encode_one(mint_to_sp).unwrap(),
        ),
        "mint to SP",
    );
    let mint_block: u64 = minted.expect("mint to SP").0.try_into().unwrap();

    let approval_memo = b"sp-v2-probe-approval".to_vec();
    let approval_created_at = now_ns(&pic);
    let approval_expires_at =
        approval_created_at + Duration::from_secs(30 * 24 * 60 * 60).as_nanos() as u64;
    let approval = icrc_ledger_types::icrc2::approve::ApproveArgs {
        from_subaccount: None,
        spender: account(backend),
        amount: Nat::from(200_000_000u64),
        expected_allowance: None,
        expires_at: Some(approval_expires_at),
        fee: None,
        memo: Some(approval_memo.clone().into()),
        created_at_time: Some(approval_created_at),
    };
    let approved: Result<Nat, icrc_ledger_types::icrc2::approve::ApproveError> = result(
        call(
            &pic,
            ledger_id,
            sp,
            "icrc2_approve",
            encode_one(approval).unwrap(),
        ),
        "approve backend",
    );
    let approval_block: u64 = approved.expect("approval").0.try_into().unwrap();
    let balance_after_approval = balance(&pic, ledger_id, sp);
    assert_eq!(
        balance_after_approval, 499_990_000,
        "the 10,000 approval fee is debited once"
    );
    let pull_memo = b"sp-v2-probe-burn".to_vec();
    let pull_created_at = now_ns(&pic);
    let pull = icrc_ledger_types::icrc2::transfer_from::TransferFromArgs {
        spender_subaccount: None,
        from: account(sp),
        to: account(backend),
        amount: Nat::from(200_000_000u64),
        fee: None,
        memo: Some(pull_memo.clone().into()),
        created_at_time: Some(pull_created_at),
    };
    let pulled: Result<Nat, icrc_ledger_types::icrc2::transfer_from::TransferFromError> = result(
        call(
            &pic,
            ledger_id,
            backend,
            "icrc2_transfer_from",
            encode_one(pull).unwrap(),
        ),
        "backend transfer_from to minting account",
    );
    let pull_block: u64 = pulled
        .expect("transfer_from to minting account")
        .0
        .try_into()
        .unwrap();
    let balance_after_pull = balance(&pic, ledger_id, sp);
    assert_eq!(
        balance_after_approval - balance_after_pull,
        200_000_000,
        "2burn debits principal only; no transfer fee is charged"
    );
    assert_eq!(balance_after_pull, 299_990_000);
    assert_eq!(balance(&pic, ledger_id, backend), 0);

    let block_at = |index: u64| -> icrc_ledger_types::icrc::generic_value::ICRC3Value {
        let response: GetBlocksResult = result(
            pic.query_call(
                ledger_id,
                Principal::anonymous(),
                "icrc3_get_blocks",
                encode_one(vec![GetBlocksRequest {
                    start: Nat::from(index),
                    length: Nat::from(1u64),
                }])
                .unwrap(),
            )
            .unwrap(),
            "icrc3_get_blocks",
        );
        response
            .blocks
            .into_iter()
            .find(|row| row.id == Nat::from(index))
            .expect("block found")
            .block
    };
    let pull_value = block_at(pull_block);
    let pull_decoded =
        rumi_protocol_backend::icrc3_proof::decode_block(&pull_value).expect("decode pull block");
    assert_eq!(pull_decoded.op, "burn");
    assert_eq!(pull_decoded.amount, 200_000_000);
    let pull_map = match &pull_value {
        icrc_ledger_types::icrc::generic_value::ICRC3Value::Map(map) => map,
        other => panic!("unexpected block: {other:?}"),
    };
    let pull_tx = match pull_map.get("tx") {
        Some(icrc_ledger_types::icrc::generic_value::ICRC3Value::Map(map)) => map,
        other => panic!("missing tx: {other:?}"),
    };
    assert_eq!(
        pull_tx.get("op"),
        Some(&icrc_ledger_types::icrc::generic_value::ICRC3Value::Text(
            "burn".into()
        ))
    );
    assert!(
        fee_absent_or_zero(pull_tx.get("fee")),
        "minting-account burn must not charge a positive transfer fee"
    );
    assert_eq!(pull_decoded.from, Some(account(sp)));
    assert_eq!(pull_decoded.to, None, "burn block has no recipient");
    assert_eq!(pull_decoded.spender, Some(account(backend)));
    assert_eq!(pull_decoded.memo, Some(pull_memo));
    assert_eq!(pull_decoded.created_at_time, Some(pull_created_at));
    let approval_value = block_at(approval_block);
    let approval_decoded = rumi_protocol_backend::icrc3_proof::decode_block(&approval_value)
        .expect("decode approval block");
    assert_eq!(approval_decoded.op, "approve");
    assert_eq!(approval_decoded.amount, 200_000_000);
    assert_eq!(approval_decoded.from, Some(account(sp)));
    assert_eq!(approval_decoded.spender, Some(account(backend)));
    assert_eq!(approval_decoded.memo, Some(approval_memo));
    assert_eq!(approval_decoded.created_at_time, Some(approval_created_at));
    assert_eq!(approval_decoded.expires_at, Some(approval_expires_at));
    let approval_map = match &approval_value {
        icrc_ledger_types::icrc::generic_value::ICRC3Value::Map(map) => map,
        other => panic!("unexpected approval block: {other:?}"),
    };
    let approval_tx = match approval_map.get("tx") {
        Some(icrc_ledger_types::icrc::generic_value::ICRC3Value::Map(map)) => map,
        other => panic!("missing approval tx: {other:?}"),
    };
    assert_eq!(
        approval_tx.get("op"),
        Some(&icrc_ledger_types::icrc::generic_value::ICRC3Value::Text(
            "approve".into()
        ))
    );
    println!(
        "official icUSD approval: block={approval_block}, raw={approval_value:?}, decoded_op={}, balance_fee_debit={}",
        approval_decoded.op,
        500_000_000u64 - balance_after_approval
    );
    println!("official icUSD pull: block={pull_block}, raw={pull_value:?}, decoded={pull_decoded:?}, SP balance after pull={}",
        balance(&pic, ledger_id, sp));

    // Backend refunds by minting from its own account. The ledger's raw ICRC-3
    // block may use `tx.op` instead of top-level `btype`; the decoded operation
    // must be `mint`, with the exact receipt tuple preserved and no fee.
    let refund_memo = b"sp-v2-probe-refund".to_vec();
    let refund_created_at = now_ns(&pic);
    // Model the full protocol-paid compensation tuple: principal, the prior
    // approval fee, and the (zero) minting-account pull fee.
    let principal_refund = 200_000_000u64;
    let approval_fee_refund = LEDGER_FEE;
    let pull_fee_refund = 0u64;
    let full_refund = principal_refund + approval_fee_refund + pull_fee_refund;
    let refund = icrc_ledger_types::icrc1::transfer::TransferArg {
        from_subaccount: None,
        to: account(sp),
        fee: None,
        created_at_time: Some(refund_created_at),
        memo: Some(refund_memo.clone().into()),
        amount: Nat::from(full_refund),
    };
    let refunded: Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError> = result(
        call(
            &pic,
            ledger_id,
            backend,
            "icrc1_transfer",
            encode_one(refund).unwrap(),
        ),
        "backend mint refund",
    );
    let refund_block: u64 = refunded.expect("backend mint refund").0.try_into().unwrap();
    let refund_value = block_at(refund_block);
    let refund_map = match &refund_value {
        icrc_ledger_types::icrc::generic_value::ICRC3Value::Map(map) => map,
        other => panic!("unexpected block: {other:?}"),
    };
    let refund_decoded = rumi_protocol_backend::icrc3_proof::decode_block(&refund_value)
        .expect("decode refund block");
    assert_eq!(refund_decoded.op, "mint");
    assert_eq!(refund_decoded.amount, u128::from(full_refund));
    assert_eq!(refund_decoded.from, None);
    assert_eq!(refund_decoded.to, Some(account(sp)));
    assert_eq!(refund_decoded.spender, None);
    assert_eq!(refund_decoded.memo, Some(refund_memo));
    assert_eq!(refund_decoded.created_at_time, Some(refund_created_at));
    let refund_tx = match refund_map.get("tx") {
        Some(icrc_ledger_types::icrc::generic_value::ICRC3Value::Map(map)) => map,
        other => panic!("missing refund tx: {other:?}"),
    };
    assert_eq!(
        refund_tx.get("op"),
        Some(&icrc_ledger_types::icrc::generic_value::ICRC3Value::Text(
            "mint".into()
        ))
    );
    assert!(
        fee_absent_or_zero(refund_tx.get("fee")),
        "mint refund must not charge a positive transfer fee"
    );
    assert_eq!(
        balance(&pic, ledger_id, sp),
        500_000_000,
        "the protocol mint refund restores principal plus the approval fee exactly"
    );
    assert_eq!(balance(&pic, ledger_id, backend), 0);
    assert!(mint_block < pull_block && pull_block < refund_block);
    println!("official icUSD refund: block={refund_block}, raw={refund_value:?}, decoded={refund_decoded:?}, SP balance after refund={}",
        balance(&pic, ledger_id, sp));

    // Explicit `fee = 0` is also accepted for a pull to the minting account.
    // The exact balance delta shows whether an optional zero fee changes burn
    // semantics or merely spells the same fee-free operation explicitly.
    let second_mint = icrc_ledger_types::icrc1::transfer::TransferArg {
        from_subaccount: None,
        to: account(sp),
        fee: None,
        created_at_time: None,
        memo: None,
        amount: Nat::from(100_000_000u64),
    };
    let second_mint: Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError> = result(
        call(
            &pic,
            ledger_id,
            backend,
            "icrc1_transfer",
            encode_one(second_mint).unwrap(),
        ),
        "second mint",
    );
    second_mint.expect("second mint");
    let second_approval = icrc_ledger_types::icrc2::approve::ApproveArgs {
        from_subaccount: None,
        spender: account(backend),
        amount: Nat::from(100_000_000u64),
        expected_allowance: None,
        expires_at: None,
        fee: None,
        memo: None,
        created_at_time: None,
    };
    let second_approval: Result<Nat, icrc_ledger_types::icrc2::approve::ApproveError> = result(
        call(
            &pic,
            ledger_id,
            sp,
            "icrc2_approve",
            encode_one(second_approval).unwrap(),
        ),
        "second approval",
    );
    second_approval.expect("second approval");
    let before_zero_fee_pull = balance(&pic, ledger_id, sp);
    let zero_fee_pull = icrc_ledger_types::icrc2::transfer_from::TransferFromArgs {
        spender_subaccount: None,
        from: account(sp),
        to: account(backend),
        amount: Nat::from(100_000_000u64),
        fee: Some(Nat::from(0u64)),
        memo: None,
        created_at_time: None,
    };
    let zero_fee_pull: Result<Nat, icrc_ledger_types::icrc2::transfer_from::TransferFromError> =
        result(
            call(
                &pic,
                ledger_id,
                backend,
                "icrc2_transfer_from",
                encode_one(zero_fee_pull).unwrap(),
            ),
            "explicit zero-fee pull",
        );
    let zero_fee_block: u64 = zero_fee_pull
        .expect("explicit zero-fee pull")
        .0
        .try_into()
        .unwrap();
    assert_eq!(
        before_zero_fee_pull - balance(&pic, ledger_id, sp),
        100_000_000
    );
    let zero_value = block_at(zero_fee_block);
    let zero_decoded = rumi_protocol_backend::icrc3_proof::decode_block(&zero_value)
        .expect("decode explicit zero-fee pull");
    assert_eq!(zero_decoded.op, "burn");
    assert_eq!(zero_decoded.amount, 100_000_000);
    assert_eq!(zero_decoded.from, Some(account(sp)));
    assert_eq!(zero_decoded.to, None);
    assert_eq!(zero_decoded.spender, Some(account(backend)));
    let zero_map = match &zero_value {
        icrc_ledger_types::icrc::generic_value::ICRC3Value::Map(map) => map,
        other => panic!("unexpected explicit-zero-fee block: {other:?}"),
    };
    let zero_tx = match zero_map.get("tx") {
        Some(icrc_ledger_types::icrc::generic_value::ICRC3Value::Map(map)) => map,
        other => panic!("missing explicit-zero-fee tx: {other:?}"),
    };
    assert_eq!(
        zero_tx.get("op"),
        Some(&icrc_ledger_types::icrc::generic_value::ICRC3Value::Text(
            "burn".into()
        ))
    );
    assert!(fee_absent_or_zero(zero_tx.get("fee")));
    println!("official icUSD explicit-zero-fee pull: block={zero_fee_block}, raw={zero_value:?}, decoded={zero_decoded:?}, SP debit={}",
        before_zero_fee_pull - balance(&pic, ledger_id, sp));
}

#[test]
#[ignore = "V2 admission/executor remain fail-closed; requires coordinated test-only enablement and freshly rebuilt backend/SP release Wasms"]
fn real_backend_and_pool_complete_icp_vault_liquidation_v2() {
    let pic = PocketIcBuilder::new().with_nns_subnet().build();
    let admin = admin();
    let user = user();
    let backend = pic.create_canister();
    pic.add_cycles(backend, 2_000_000_000_000);
    pic.set_controllers(backend, None, vec![Principal::anonymous(), admin])
        .unwrap();

    let icp = ledger(
        &pic,
        admin,
        LEDGER_FEE,
        "ICP",
        vec![(account(user), Nat::from(50_000_000_000u64))],
        admin,
    );
    let icusd = ledger(&pic, backend, LEDGER_FEE, "icUSD", vec![], admin);
    let xrc = pic.create_canister();
    pic.add_cycles(xrc, 1_000_000_000_000);
    pic.install_canister(
        xrc,
        xrc_wasm(),
        encode_one(MockXrc {
            rates: HashMap::from([("ICP/USD".into(), 1_000_000_000)]),
        })
        .unwrap(),
        None,
    );
    pic.set_time(SystemTime::UNIX_EPOCH + Duration::from_secs(1_711_324_800));

    let init = rumi_protocol_backend::ProtocolArg::Init(rumi_protocol_backend::InitArg {
        xrc_principal: xrc,
        icusd_ledger_principal: icusd,
        icp_ledger_principal: icp,
        fee_e8s: LEDGER_FEE,
        developer_principal: admin,
        treasury_principal: Some(treasury()),
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    });
    pic.install_canister(backend, backend_wasm(), encode_args((init,)).unwrap(), None);
    pic.advance_time(Duration::from_secs(1));
    for _ in 0..10 {
        pic.tick();
    }
    for (method, args) in [
        (
            "set_borrowing_fee_curve",
            encode_args((None::<String>,)).unwrap(),
        ),
        ("set_borrowing_fee", encode_args((0.0f64,)).unwrap()),
        ("set_interest_rate", encode_args((icp, 0.0f64)).unwrap()),
        (
            "set_treasury_principal",
            encode_args((treasury(),)).unwrap(),
        ),
    ] {
        let _ = call(&pic, backend, admin, method, args);
    }

    let sp = pic.create_canister();
    pic.add_cycles(sp, 2_000_000_000_000);
    pic.install_canister(
        sp,
        pool_wasm(),
        encode_one(StabilityPoolInitArgs {
            protocol_canister_id: backend,
            authorized_admins: vec![admin],
        })
        .unwrap(),
        None,
    );
    let _ = call(
        &pic,
        backend,
        admin,
        "set_stability_pool_principal",
        encode_one(sp).unwrap(),
    );
    let _ = call(
        &pic,
        sp,
        admin,
        "register_stablecoin",
        encode_one(StablecoinConfig {
            ledger_id: icusd,
            symbol: "icUSD".into(),
            decimals: 8,
            priority: 1,
            is_active: true,
            transfer_fee: Some(LEDGER_FEE),
            is_lp_token: Some(false),
            underlying_pool: None,
        })
        .unwrap(),
    );
    let _ = call(
        &pic,
        sp,
        admin,
        "register_collateral",
        encode_one(CollateralInfo {
            ledger_id: icp,
            symbol: "ICP".into(),
            decimals: 8,
            status: CollateralStatus::Active,
        })
        .unwrap(),
    );

    // Open a 50 ICP / 100 icUSD vault, then make its collateral unhealthy.
    let approve_icp = icrc_ledger_types::icrc2::approve::ApproveArgs {
        from_subaccount: None,
        spender: account(backend),
        amount: Nat::from(50_000_000_000u64),
        expected_allowance: None,
        expires_at: None,
        fee: None,
        memo: None,
        created_at_time: None,
    };
    let _ = call(
        &pic,
        icp,
        user,
        "icrc2_approve",
        encode_one(approve_icp).unwrap(),
    );
    let opened: Result<OpenVaultSuccess, candid::Reserved> = result(
        call(
            &pic,
            backend,
            user,
            "open_vault",
            encode_args((5_000_000_000u64, None::<Principal>)).unwrap(),
        ),
        "open_vault",
    );
    let vault_id = opened.expect("vault opens").vault_id;
    let borrowed: Result<rumi_protocol_backend::SuccessWithFee, candid::Reserved> = result(
        call(
            &pic,
            backend,
            user,
            "borrow_from_vault",
            encode_one(VaultArg {
                vault_id,
                amount: 10_000_000_000,
            })
            .unwrap(),
        ),
        "borrow icUSD",
    );
    borrowed.expect("vault borrows");
    let _ = call(
        &pic,
        xrc,
        admin,
        "set_exchange_rate",
        encode_args(("ICP".to_string(), "USD".to_string(), 10_000_000u64)).unwrap(),
    );
    pic.advance_time(Duration::from_secs(490));
    for _ in 0..10 {
        pic.tick();
    }

    // Fund the pool with real icUSD and exercise its public V2 route.
    let mint = icrc_ledger_types::icrc1::transfer::TransferArg {
        from_subaccount: None,
        to: account(user),
        fee: None,
        created_at_time: None,
        memo: None,
        amount: Nat::from(20_000_000_000u64),
    };
    let minted: Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError> = result(
        call(
            &pic,
            icusd,
            backend,
            "icrc1_transfer",
            encode_one(mint).unwrap(),
        ),
        "mint liquidation icUSD",
    );
    minted.expect("backend minter funds user");
    let approve_pool = icrc_ledger_types::icrc2::approve::ApproveArgs {
        from_subaccount: None,
        spender: account(sp),
        amount: Nat::from(20_000_010_000u64),
        expected_allowance: None,
        expires_at: None,
        fee: None,
        memo: None,
        created_at_time: None,
    };
    let _ = call(
        &pic,
        icusd,
        user,
        "icrc2_approve",
        encode_one(approve_pool).unwrap(),
    );
    let deposited: Result<(), candid::Reserved> = result(
        call(
            &pic,
            sp,
            user,
            "deposit",
            encode_args((icusd, 20_000_000_000u64)).unwrap(),
        ),
        "deposit",
    );
    deposited.expect("pool deposit succeeds");

    let executed: Result<u64, candid::Reserved> = result(
        call(
            &pic,
            sp,
            user,
            "execute_liquidation_v2",
            encode_args((vault_id, SpLiquidationToken::IcUsd, 19_990_000_000u64)).unwrap(),
        ),
        "execute_liquidation_v2",
    );
    let request_id = executed.expect("V2 liquidation dispatches");

    // Recovery is idempotent. The public execute call performs local receipt
    // verification and ACK before returning, so the backend journal should be
    // compacted to Acknowledged while the SP retains its durable local result.
    let _ = call(
        &pic,
        sp,
        user,
        "retry_sp_liquidation_v2",
        encode_one(request_id).unwrap(),
    );
    let status_reply = pic
        .query_call(
            backend,
            sp,
            "get_stability_pool_liquidation_v2_status",
            encode_one(request_id).unwrap(),
        )
        .unwrap();
    let status: Result<rumi_protocol_backend::SpLiquidationV2StatusView, candid::Reserved> =
        result(status_reply, "get_stability_pool_liquidation_v2_status");
    let status = status.expect("registered SP can read backend journal");
    assert_eq!(status.stability_pool, sp);
    assert_eq!(status.request_id, request_id);
    assert!(status.request.is_none());
    assert!(matches!(
        status.status,
        rumi_protocol_backend::SpLiquidationV2Status::Acknowledged
    ));
    let local_reply = pic
        .query_call(
            sp,
            user,
            "get_sp_liquidation_v2_status",
            encode_one(request_id).unwrap(),
        )
        .unwrap();
    let local: Option<stability_pool::types::SpLiquidationV2LocalStatus> =
        result(local_reply, "get_sp_liquidation_v2_status");
    let local = local.expect("SP retains local completed tombstone");
    assert_eq!(
        local.phase,
        stability_pool::types::SpLiquidationV2LocalPhase::Complete
    );
    assert!(local.stable_debit_applied);
    assert!(local.backend_acknowledged);
    assert!(local.approval_fee_accounted);
    let pool_usd = balance(&pic, icusd, sp);
    assert!(
        pool_usd < 20_000_000_000,
        "SP must have paid liquidation principal: {pool_usd}"
    );
    assert!(
        balance(&pic, icp, sp) > 0,
        "SP must receive collateral payout"
    );
    let vaults = pic
        .query_call(
            backend,
            Principal::anonymous(),
            "get_liquidatable_vaults",
            encode_args(()).unwrap(),
        )
        .unwrap();
    let liquidatable: Vec<rumi_protocol_backend::vault::CandidVault> =
        result(vaults, "get_liquidatable_vaults");
    assert!(
        !liquidatable.iter().any(|row| row.vault_id == vault_id),
        "liquidated vault leaves liquidatable set"
    );
}

/// Feature-on receipt path using the real backend, Stability Pool, 3pool,
/// and official ledger Wasms. Run with
/// `--features test-three-usd-reserve-ingress-v2-admission` and set the
/// `RUMI_THREE_USD_*_WASM` variables to source-matched feature artifacts.
#[cfg(feature = "test-three-usd-reserve-ingress-v2-admission")]
#[test]
fn real_backend_and_pool_complete_3usd_reserve_absorption_v2() {
    run_real_backend_and_pool_complete_3usd_reserve_absorption_v2(
        ThreeUsdAbsorptionDriver::BackendPush,
    );
}

/// Exercise the same full-debt receipt/accounting fixture through the public
/// permissionless fallback instead of the backend notification push.
#[cfg(feature = "test-three-usd-reserve-ingress-v2-admission")]
#[test]
fn real_backend_and_pool_complete_3usd_reserve_absorption_v2_public_keeper_fallback() {
    run_real_backend_and_pool_complete_3usd_reserve_absorption_v2(
        ThreeUsdAbsorptionDriver::PublicKeeper,
    );
}

/// Upgrade both canisters after the exact 3USD pull is journaled, attach the
/// observed ICRC-3 block as a recovery candidate, and finish the same absorb.
/// This covers post-pull/pre-terminal recovery, not a lost reply after commit.
#[cfg(feature = "test-three-usd-reserve-ingress-v2-admission")]
#[test]
fn backend_and_pool_upgrade_after_3usd_pull_recover_exact_candidate() {
    run_real_backend_and_pool_complete_3usd_reserve_absorption_v2(
        ThreeUsdAbsorptionDriver::UpgradeRecovery,
    );
}

/// Commit the backend absorption, then interrupt the outstanding notification
/// before its reply can finalize the SP journal. Recovery must use the
/// backend's exact terminal receipts without dispatching a second 3USD pull.
#[cfg(feature = "test-three-usd-reserve-ingress-v2-admission")]
#[test]
fn backend_commit_with_lost_reply_recovers_without_second_3usd_pull() {
    run_real_backend_and_pool_complete_3usd_reserve_absorption_v2(
        ThreeUsdAbsorptionDriver::LostReplyAfterBackendCommit,
    );
}

#[cfg(feature = "test-three-usd-reserve-ingress-v2-admission")]
#[derive(Clone, Copy)]
enum ThreeUsdAbsorptionDriver {
    BackendPush,
    PublicKeeper,
    UpgradeRecovery,
    LostReplyAfterBackendCommit,
}

#[cfg(feature = "test-three-usd-reserve-ingress-v2-admission")]
fn run_real_backend_and_pool_complete_3usd_reserve_absorption_v2(driver: ThreeUsdAbsorptionDriver) {
    use rumi_protocol_backend::{
        ThreeUsdReserveIngressV2Status, ThreeUsdReserveIngressV2StatusView,
    };
    use stability_pool::types::StabilityPoolStatus;

    let pic = PocketIcBuilder::new().with_nns_subnet().build();
    let admin = admin();
    let user = user();
    let backend = pic.create_canister();
    pic.add_cycles(backend, 2_000_000_000_000);
    pic.set_controllers(backend, None, vec![Principal::anonymous(), admin])
        .unwrap();

    // The backend's ICP proof verifier requires the native NNS ledger's
    // `query_blocks` schema. This official ledger also enables ICRC-2 for the
    // receipt-backed vault-open pull.
    // Include enough user-owned ICP to open a debt-free buffer vault. At the
    // confirmed $0.10 price it keeps system TCR above 100%, while the target
    // 50-ICP vault itself remains deeply liquidatable.
    let icp = deploy_native_icp_ledger(&pic, user, 2_000_000_000_000, admin);
    // The official ICRC ledger permits initial balances for a non-minter;
    // the backend remains its minting account for icUSD liquidation work.
    let icusd = ledger(
        &pic,
        backend,
        LEDGER_FEE,
        "icUSD",
        vec![(account(user), Nat::from(1_000_000_000_000_000u64))],
        admin,
    );
    let ckusdt = ledger(
        &pic,
        admin,
        0,
        "ckUSDT",
        vec![(account(user), Nat::from(1_000_000_000_000_000u64))],
        admin,
    );
    let ckusdc = ledger(
        &pic,
        admin,
        0,
        "ckUSDC",
        vec![(account(user), Nat::from(1_000_000_000_000_000u64))],
        admin,
    );
    let pool = Principal::from_text("fohh4-yyaaa-aaaap-qtkpa-cai")
        .expect("canonical 3USD ledger principal");
    pic.create_canister_with_id(None, None, pool)
        .expect("create canonical 3pool canister");
    pic.add_cycles(pool, 2_000_000_000_000);
    pic.install_canister(
        pool,
        three_pool_wasm(),
        encode_one(ThreePoolInitArgs {
            tokens: [
                TokenConfig {
                    ledger_id: icusd,
                    symbol: "icUSD".into(),
                    decimals: 8,
                    precision_mul: 10_000_000_000,
                },
                TokenConfig {
                    ledger_id: ckusdt,
                    symbol: "ckUSDT".into(),
                    decimals: 8,
                    precision_mul: 10_000_000_000,
                },
                TokenConfig {
                    ledger_id: ckusdc,
                    symbol: "ckUSDC".into(),
                    decimals: 8,
                    precision_mul: 10_000_000_000,
                },
            ],
            initial_a: 100,
            swap_fee_bps: 4,
            admin_fee_bps: 5_000,
            admin,
        })
        .unwrap(),
        None,
    );

    // Seed real 3pool reserves so the LP virtual price and the LP token
    // deposited into the Stability Pool come from the deployed pool canister.
    for (ledger_id, ledger_fee) in [(icusd, LEDGER_FEE), (ckusdt, 0), (ckusdc, 0)] {
        let approve = icrc_ledger_types::icrc2::approve::ApproveArgs {
            from_subaccount: None,
            spender: account(pool),
            amount: Nat::from(100_000_000_000_000u128 + ledger_fee as u128),
            expected_allowance: None,
            expires_at: None,
            fee: None,
            memo: None,
            created_at_time: None,
        };
        let approved: Result<Nat, icrc_ledger_types::icrc2::approve::ApproveError> = result(
            call(
                &pic,
                ledger_id,
                user,
                "icrc2_approve",
                encode_one(approve).unwrap(),
            ),
            "approve 3pool liquidity",
        );
        approved.expect("ledger approves 3pool");
    }
    let added: Result<
        rumi_3pool::receipts::IngressReceiptV1,
        rumi_3pool::receipts::IngressReceiptErrorV1,
    > = result(
        call(
            &pic,
            pool,
            user,
            "add_liquidity_with_receipt_v1",
            encode_args((vec![0x3du8; 32], vec![100_000_000_000_000u128; 3], 0u128)).unwrap(),
        ),
        "add 3pool liquidity with receipt",
    );
    let added = added.expect("3pool liquidity receipt accepted");
    assert_eq!(
        added.status,
        rumi_3pool::receipts::IngressStatusV1::Completed,
        "3pool ingress did not complete: {added:?}"
    );
    assert!(added.result_lp.expect("3pool LP mint receipt") > 0);

    let xrc = pic.create_canister();
    pic.add_cycles(xrc, 1_000_000_000_000);
    pic.install_canister(
        xrc,
        xrc_wasm(),
        encode_one(MockXrc {
            rates: HashMap::from([("ICP/USD".into(), 1_000_000_000)]),
        })
        .unwrap(),
        None,
    );
    pic.set_time(SystemTime::UNIX_EPOCH + Duration::from_secs(1_711_324_800));
    let init = rumi_protocol_backend::ProtocolArg::Init(rumi_protocol_backend::InitArg {
        xrc_principal: xrc,
        icusd_ledger_principal: icusd,
        icp_ledger_principal: icp,
        fee_e8s: LEDGER_FEE,
        developer_principal: admin,
        treasury_principal: Some(treasury()),
        stability_pool_principal: None,
        ckusdt_ledger_principal: Some(ckusdt),
        ckusdc_ledger_principal: Some(ckusdc),
    });
    pic.install_canister(backend, backend_wasm(), encode_args((init,)).unwrap(), None);
    pic.advance_time(Duration::from_secs(1));
    for _ in 0..10 {
        pic.tick();
    }
    for (method, args) in [
        (
            "set_borrowing_fee_curve",
            encode_args((None::<String>,)).unwrap(),
        ),
        ("set_borrowing_fee", encode_args((0.0f64,)).unwrap()),
        ("set_interest_rate", encode_args((icp, 0.0f64)).unwrap()),
        (
            "set_treasury_principal",
            encode_args((treasury(),)).unwrap(),
        ),
        ("set_three_pool_canister", encode_one(pool).unwrap()),
    ] {
        let reply = call(&pic, backend, admin, method, args);
        let configured: Result<(), candid::Reserved> = result(reply, method);
        configured.unwrap_or_else(|error| panic!("{method}: {error:?}"));
    }

    let sp = pic.create_canister();
    pic.add_cycles(sp, 2_000_000_000_000);
    pic.install_canister(
        sp,
        pool_wasm(),
        encode_one(StabilityPoolInitArgs {
            protocol_canister_id: backend,
            authorized_admins: vec![admin],
        })
        .unwrap(),
        None,
    );
    let configured: Result<(), candid::Reserved> = result(
        call(
            &pic,
            backend,
            admin,
            "set_stability_pool_principal",
            encode_one(sp).unwrap(),
        ),
        "register Stability Pool",
    );
    configured.expect("register Stability Pool");
    for config in [
        StablecoinConfig {
            ledger_id: icusd,
            symbol: "icUSD".into(),
            decimals: 8,
            priority: 1,
            is_active: true,
            transfer_fee: Some(LEDGER_FEE),
            is_lp_token: Some(false),
            underlying_pool: None,
        },
        StablecoinConfig {
            ledger_id: ckusdt,
            symbol: "ckUSDT".into(),
            decimals: 8,
            priority: 2,
            is_active: true,
            transfer_fee: Some(0),
            is_lp_token: Some(false),
            underlying_pool: None,
        },
        StablecoinConfig {
            ledger_id: ckusdc,
            symbol: "ckUSDC".into(),
            decimals: 8,
            priority: 2,
            is_active: true,
            transfer_fee: Some(0),
            is_lp_token: Some(false),
            underlying_pool: None,
        },
        StablecoinConfig {
            ledger_id: pool,
            symbol: "3USD".into(),
            decimals: 8,
            priority: 0,
            is_active: true,
            transfer_fee: Some(0),
            is_lp_token: Some(true),
            underlying_pool: Some(pool),
        },
    ] {
        let registered: Result<(), candid::Reserved> = result(
            call(
                &pic,
                sp,
                admin,
                "register_stablecoin",
                encode_one(config).unwrap(),
            ),
            "register stablecoin",
        );
        registered.expect("stablecoin registered");
    }
    let collateral: Result<(), candid::Reserved> = result(
        call(
            &pic,
            sp,
            admin,
            "register_collateral",
            encode_one(CollateralInfo {
                ledger_id: icp,
                symbol: "ICP".into(),
                decimals: 8,
                status: CollateralStatus::Active,
            })
            .unwrap(),
        ),
        "register ICP collateral",
    );
    collateral.expect("ICP collateral registered");

    let approve_icp = icrc_ledger_types::icrc2::approve::ApproveArgs {
        from_subaccount: None,
        spender: account(backend),
        amount: Nat::from(5_000_000_000u64 + LEDGER_FEE),
        expected_allowance: None,
        expires_at: None,
        fee: None,
        memo: None,
        created_at_time: None,
    };
    let approved: Result<Nat, icrc_ledger_types::icrc2::approve::ApproveError> = result(
        call(
            &pic,
            icp,
            user,
            "icrc2_approve",
            encode_one(approve_icp).unwrap(),
        ),
        "approve ICP collateral",
    );
    approved.expect("ICP approved");
    let opened: Result<
        rumi_protocol_backend::InboundCollateralStatusView,
        rumi_protocol_backend::ProtocolError,
    > = result(
        call(
            &pic,
            backend,
            user,
            "open_vault_v2",
            encode_args((1u128, 5_000_000_000u64, None::<Principal>)).unwrap(),
        ),
        "open ICP vault via V2 receipt",
    );
    let opened = opened.expect("open_vault_v2 accepted");
    let vault_id = match opened.result {
        Some(rumi_protocol_backend::InboundCollateralResultView::Open { vault_id, .. }) => vault_id,
        other => panic!(
            "open_vault_v2 did not complete: phase={:?}, error={:?}, result={other:?}",
            opened.phase, opened.last_error
        ),
    };
    let borrowed: Result<
        rumi_protocol_backend::SuccessWithFee,
        rumi_protocol_backend::ProtocolError,
    > = result(
        call(
            &pic,
            backend,
            user,
            "borrow_from_vault",
            encode_one(VaultArg {
                vault_id,
                amount: 10_000_000_000,
            })
            .unwrap(),
        ),
        "borrow icUSD",
    );
    borrowed.unwrap_or_else(|error| {
        let pending: Result<
            Vec<rumi_protocol_backend::state::PendingBorrowMint>,
            rumi_protocol_backend::ProtocolError,
        > = result(
            pic.query_call(
                backend,
                admin,
                "list_pending_borrow_mints",
                encode_args((None::<u128>, 100u16)).unwrap(),
            )
            .expect("query pending borrow mint journal"),
            "list_pending_borrow_mints",
        );
        eprintln!("pending borrow mint journal: {:?}", pending.unwrap());
        let recent: GetBlocksResult = result(
            pic.query_call(
                icusd,
                Principal::anonymous(),
                "icrc3_get_blocks",
                encode_one(vec![GetBlocksRequest {
                    start: Nat::from(0u64),
                    length: Nat::from(32u64),
                }])
                .unwrap(),
            )
            .expect("query recent icUSD blocks"),
            "icrc3_get_blocks icUSD diagnostics",
        );
        for row in recent.blocks {
            let decoded = rumi_protocol_backend::icrc3_proof::decode_block(&row.block);
            eprintln!(
                "icUSD block {}: decoded={decoded:?}, raw={:?}",
                row.id, row.block
            );
        }
        panic!("vault borrow failed: {error:?}");
    });
    let approve_buffer = icrc_ledger_types::icrc2::approve::ApproveArgs {
        from_subaccount: None,
        spender: account(backend),
        amount: Nat::from(1_000_000_000_000u64 + LEDGER_FEE),
        expected_allowance: None,
        expires_at: None,
        fee: None,
        memo: None,
        created_at_time: None,
    };
    let approved_buffer: Result<Nat, icrc_ledger_types::icrc2::approve::ApproveError> = result(
        call(
            &pic,
            icp,
            user,
            "icrc2_approve",
            encode_one(approve_buffer).unwrap(),
        ),
        "approve ICP buffer vault collateral",
    );
    approved_buffer.expect("buffer ICP approved");
    let opened_buffer: Result<
        rumi_protocol_backend::InboundCollateralStatusView,
        rumi_protocol_backend::ProtocolError,
    > = result(
        call(
            &pic,
            backend,
            user,
            "open_vault_v2",
            encode_args((2u128, 1_000_000_000_000u64, None::<Principal>)).unwrap(),
        ),
        "open debt-free TCR buffer vault via V2 receipt",
    );
    let opened_buffer = opened_buffer.expect("buffer open accepted");
    let buffer_vault_id = match opened_buffer.result {
        Some(rumi_protocol_backend::InboundCollateralResultView::Open { vault_id, .. }) => vault_id,
        other => panic!(
            "buffer open_vault_v2 did not complete: phase={:?}, error={:?}, result={other:?}",
            opened_buffer.phase, opened_buffer.last_error
        ),
    };
    assert_ne!(buffer_vault_id, vault_id);
    let proportional_refund_vault_id = if positive_refund_pic_enabled()
        && !matches!(
            driver,
            ThreeUsdAbsorptionDriver::LostReplyAfterBackendCommit
        ) {
        let approve_refund_vault_icp = icrc_ledger_types::icrc2::approve::ApproveArgs {
            from_subaccount: None,
            spender: account(backend),
            amount: Nat::from(5_000_000_000u64 + LEDGER_FEE),
            expected_allowance: None,
            expires_at: None,
            fee: None,
            memo: None,
            created_at_time: None,
        };
        let approved: Result<Nat, icrc_ledger_types::icrc2::approve::ApproveError> = result(
            call(
                &pic,
                icp,
                user,
                "icrc2_approve",
                encode_one(approve_refund_vault_icp).unwrap(),
            ),
            "approve ICP for proportional-refund vault",
        );
        approved.expect("ICP approved for proportional-refund vault");
        let opened: Result<
            rumi_protocol_backend::InboundCollateralStatusView,
            rumi_protocol_backend::ProtocolError,
        > = result(
            call(
                &pic,
                backend,
                user,
                "open_vault_v2",
                encode_args((3u128, 5_000_000_000u64, None::<Principal>)).unwrap(),
            ),
            "open second indebted vault via V2 receipt",
        );
        let opened = opened.expect("proportional-refund vault opened");
        let id = match opened.result {
            Some(rumi_protocol_backend::InboundCollateralResultView::Open { vault_id, .. }) => {
                vault_id
            }
            other => panic!(
                "proportional-refund vault open did not complete: phase={:?}, error={:?}, result={other:?}",
                opened.phase, opened.last_error
            ),
        };
        assert_eq!(id, 3, "the test-only barrier is pinned to the third vault");
        let borrowed: Result<
            rumi_protocol_backend::SuccessWithFee,
            rumi_protocol_backend::ProtocolError,
        > = result(
            call(
                &pic,
                backend,
                user,
                "borrow_from_vault",
                encode_one(VaultArg {
                    vault_id: id,
                    amount: 10_000_000_000,
                })
                .unwrap(),
            ),
            "borrow icUSD for proportional-refund vault",
        );
        borrowed.expect("proportional-refund vault borrows");
        Some(id)
    } else {
        None
    };
    let interval_set: Result<(), rumi_protocol_backend::ProtocolError> = result(
        call(
            &pic,
            backend,
            admin,
            "set_xrc_fetch_interval_secs",
            encode_one(60u64).unwrap(),
        ),
        "set test XRC fetch interval",
    );
    interval_set.expect("test XRC fetch interval set");
    let _ = call(
        &pic,
        xrc,
        admin,
        "set_exchange_rate",
        encode_args(("ICP".to_string(), "USD".to_string(), 10_000_000u64)).unwrap(),
    );
    // A $10 -> $0.10 move is outside the backend's price sanity band. Confirm
    // the stable outlier through three consecutive source fetches before
    // expecting the liquidation path to use it.
    for _ in 0..3 {
        // The source confirmer also requires distinct samples to be at least
        // 300 seconds apart, so step just beyond that floor each time.
        pic.advance_time(Duration::from_secs(301));
        for _ in 0..10 {
            pic.tick();
        }
    }

    let lp_deposit = 10_000_000_000u64;
    let approve_lp = icrc_ledger_types::icrc2::approve::ApproveArgs {
        from_subaccount: None,
        spender: account(sp),
        amount: Nat::from(lp_deposit),
        expected_allowance: None,
        expires_at: None,
        fee: None,
        memo: None,
        created_at_time: None,
    };
    let approved: Result<Nat, icrc_ledger_types::icrc2::approve::ApproveError> = result(
        call(
            &pic,
            pool,
            user,
            "icrc2_approve",
            encode_one(approve_lp).unwrap(),
        ),
        "approve 3USD deposit",
    );
    approved.expect("3USD approved");
    let deposited: Result<(), candid::Reserved> = result(
        call(
            &pic,
            sp,
            user,
            "deposit",
            encode_args((pool, lp_deposit)).unwrap(),
        ),
        "deposit 3USD into SP",
    );
    deposited.expect("3USD deposit succeeds");
    // The three spaced oracle-confirmation windows above already span more
    // than three SP virtual-price refresh intervals after its LP registration.

    let sp_lp_before = balance(&pic, pool, sp);
    let backend_lp_before = balance(&pic, pool, backend);
    let status_before: StabilityPoolStatus = result(
        pic.query_call(
            sp,
            Principal::anonymous(),
            "get_pool_status",
            encode_args(()).unwrap(),
        )
        .unwrap(),
        "pool status before absorption",
    );
    let tracked_lp_before = status_before
        .stablecoin_balances
        .get(&pool)
        .copied()
        .unwrap_or(0);
    assert_eq!(sp_lp_before, u64::from(lp_deposit));
    assert_eq!(tracked_lp_before, lp_deposit);

    match driver {
        ThreeUsdAbsorptionDriver::BackendPush | ThreeUsdAbsorptionDriver::UpgradeRecovery => {
            let notified: Result<String, rumi_protocol_backend::ProtocolError> = result(
                call(
                    &pic,
                    backend,
                    admin,
                    "dev_test_pool_only_liquidation",
                    encode_one(vault_id).unwrap(),
                ),
                "push test liquidation notification with backend price snapshot",
            );
            let notification = notified.expect("backend sent priced liquidation notification");
            assert!(notification.contains("sent to stability pool"));
        }
        ThreeUsdAbsorptionDriver::LostReplyAfterBackendCommit => {
            let notification = pic
                .submit_call(
                    backend,
                    admin,
                    "dev_test_pool_only_liquidation",
                    encode_one(vault_id).unwrap(),
                )
                .expect("submit backend liquidation notification");
            let mut committed = false;
            for _ in 0..100 {
                pic.tick();
                let status: Result<ThreeUsdReserveIngressV2StatusView, candid::Reserved> = result(
                    pic.query_call(
                        backend,
                        sp,
                        "get_three_usd_reserve_ingress_v2_status",
                        encode_args((vault_id, 1u64)).unwrap(),
                    )
                    .expect("query backend ingress while notification is pending"),
                    "backend ingress during lost-reply fixture",
                );
                if matches!(
                    status.expect("SP reads ingress status").status,
                    ThreeUsdReserveIngressV2Status::Absorbed { .. }
                ) {
                    committed = true;
                    break;
                }
            }
            assert!(
                committed,
                "backend did not reach durable Absorbed status while notification was pending"
            );

            // Interrupt the still-pending pool callback only after the
            // backend has durably committed its proof-keyed result and exact
            // ledger receipts. Its stable pending row is then recovered from
            // that terminal backend status.
            pic.upgrade_canister(
                sp,
                pool_wasm(),
                encode_one(StabilityPoolInitArgs {
                    protocol_canister_id: backend,
                    authorized_admins: vec![admin],
                })
                .unwrap(),
                None,
            )
            .expect("upgrade SP after backend commit and before reply handling");
            let _interrupted_notification = pic.await_call(notification);
            let sp_balance_before_recovery = balance(&pic, pool, sp);
            let recovery = pic
                .submit_call(
                    sp,
                    admin,
                    "recover_three_usd_absorb_v2",
                    encode_args((1u64, None::<u64>, None::<u64>, None::<u64>)).unwrap(),
                )
                .expect("submit SP recovery from committed backend status");
            let recovered: Result<(), stability_pool::types::StabilityPoolError> = result(
                pic.await_call(recovery)
                    .expect("lost-reply recovery call completes"),
                "recover committed 3USD absorb after lost reply",
            );
            recovered.expect("committed backend receipts finalize SP journal");
            assert_eq!(
                balance(&pic, pool, sp),
                sp_balance_before_recovery,
                "recovering the committed absorption must not pull 3USD again"
            );
        }
        ThreeUsdAbsorptionDriver::PublicKeeper => {
            let read_info = |id| {
                let reply = pic
                    .query_call(
                        backend,
                        Principal::anonymous(),
                        "get_liquidatable_vault_info",
                        encode_one(id).unwrap(),
                    )
                    .expect("query liquidatable vault info");
                result::<Option<rumi_protocol_backend::LiquidatableVaultInfo>>(
                    reply,
                    "get_liquidatable_vault_info",
                )
            };
            let target = read_info(vault_id).expect("underwater target is queryable");
            assert_eq!(target.vault_id, vault_id);
            assert!(target.recommended_liquidation_amount > 0);
            assert!(
                read_info(buffer_vault_id).is_none(),
                "healthy buffer is excluded"
            );
            assert!(read_info(u64::MAX).is_none(), "missing vault is excluded");

            let sp_balance_before = balance(&pic, pool, sp);
            let anonymous: Result<
                stability_pool::types::LiquidationResult,
                stability_pool::types::StabilityPoolError,
            > = result(
                call(
                    &pic,
                    sp,
                    Principal::anonymous(),
                    "execute_liquidation",
                    encode_one(vault_id).unwrap(),
                ),
                "anonymous public liquidation attempt",
            );
            assert!(
                matches!(
                    anonymous,
                    Err(stability_pool::types::StabilityPoolError::Unauthorized)
                ),
                "anonymous keeper must be rejected before any ledger movement: {anonymous:?}"
            );
            assert_eq!(balance(&pic, pool, sp), sp_balance_before);

            let liquidation: Result<
                stability_pool::types::LiquidationResult,
                stability_pool::types::StabilityPoolError,
            > = result(
                call(
                    &pic,
                    sp,
                    user,
                    "execute_liquidation",
                    encode_one(vault_id).unwrap(),
                ),
                "authenticated public keeper liquidation",
            );
            assert!(
                liquidation
                    .expect("public keeper fallback succeeds")
                    .success,
                "public fallback should complete the same absorption"
            );
        }
    }

    let ingress_view: Result<ThreeUsdReserveIngressV2StatusView, candid::Reserved> = result(
        pic.query_call(
            backend,
            sp,
            "get_three_usd_reserve_ingress_v2_status",
            encode_args((vault_id, 1u64)).unwrap(),
        )
        .unwrap(),
        "backend ingress status before SP recovery",
    );
    let ingress_view = ingress_view.expect("registered SP reads ingress status");
    assert!(
        matches!(
            &ingress_view.status,
            ThreeUsdReserveIngressV2Status::Absorbed { .. }
        ),
        "priced push did not complete 3USD ingress: {ingress_view:?}"
    );

    // The first SP absorb identity is 1 in this fresh canister. Its successful
    // admin recovery call is idempotent and confirms the local terminal row.
    let recovered: Result<(), candid::Reserved> = result(
        call(
            &pic,
            sp,
            admin,
            "recover_three_usd_absorb_v2",
            encode_args((1u64, None::<u64>, None::<u64>, None::<u64>)).unwrap(),
        ),
        "confirm terminal SP absorb",
    );
    recovered.expect("SP absorb is complete");
    assert_eq!(ingress_view.stability_pool, sp);
    assert_eq!(ingress_view.absorb_id, 1);
    let request = ingress_view.request.expect("durable request retained");
    assert_eq!(request.ledger, pool);
    assert!(request.three_usd_amount_e8s > 0);
    let (transfer_index, transfer_tuple, proof, terminal_result, refund, payout_receipt) =
        match ingress_view.status {
            ThreeUsdReserveIngressV2Status::Absorbed {
                transfer_block_index,
                transfer_tuple,
                proof,
                result,
                proportional_refund,
                collateral_payout_receipt,
            } => (
                transfer_block_index,
                transfer_tuple,
                proof,
                result,
                proportional_refund,
                collateral_payout_receipt,
            ),
            other => panic!("ingress lacks exact terminal receipts: {other:?}"),
        };
    assert!(terminal_result.success);
    assert_eq!(terminal_result.vault_id, vault_id);
    assert!(terminal_result.collateral_received > 0);
    assert_eq!(
        terminal_result.liquidated_debt,
        request.icusd_debt_covered_e8s
    );
    assert_eq!(proof.block_index, transfer_index);
    assert_eq!(proof.vault_id_memo, vault_id);
    assert_eq!(
        proof.ledger_kind,
        rumi_protocol_backend::icrc3_proof::SpProofLedger::ThreePoolTransferDefault
    );
    assert_eq!(transfer_tuple.source, account(sp));
    assert_eq!(transfer_tuple.destination, account(backend));
    assert_eq!(transfer_tuple.amount_e8s, request.three_usd_amount_e8s);
    assert_eq!(transfer_tuple.fee_e8s, None);
    assert_eq!(transfer_tuple.spender_owner, backend);
    assert_eq!(transfer_tuple.spender_subaccount, None);

    // Fetch the exact ledger block named by the backend's proof and validate
    // the transferFrom transaction against the journaled tuple independently.
    let blocks: GetBlocksResult = result(
        pic.query_call(
            pool,
            Principal::anonymous(),
            "icrc3_get_blocks",
            encode_args((vec![GetBlocksRequest {
                start: transfer_index.into(),
                length: Nat::from(1u64),
            }],))
            .unwrap(),
        )
        .unwrap(),
        "3pool ingress block",
    );
    let block = blocks
        .blocks
        .iter()
        .find(|block| block.id == Nat::from(transfer_index))
        .expect("exact ingress block retained");
    let decoded = rumi_protocol_backend::icrc3_proof::decode_block(&block.block)
        .expect("decode ingress ICRC-3 block");
    rumi_protocol_backend::icrc3_proof::validate_three_usd_reserve_ingress_block(
        &decoded,
        &transfer_tuple,
    )
    .expect("ingress ICRC-3 block matches exact tuple");

    let expected_refund = request.three_usd_amount_e8s.saturating_sub(
        ((request.three_usd_amount_e8s as u128 * terminal_result.liquidated_debt as u128)
            / request.icusd_debt_covered_e8s as u128) as u64,
    );
    assert_eq!(
        expected_refund, 0,
        "this fixture covers full-debt absorption; a separate fixture must prove proportional refunds"
    );
    let refund_amount = refund
        .as_ref()
        .map_or(0, |receipt| receipt.tuple.amount_e8s);
    match (expected_refund, refund.as_ref()) {
        (0, None) => {}
        (amount, Some(receipt)) if amount > 0 => {
            assert_eq!(receipt.tuple.amount_e8s, amount);
            assert_eq!(receipt.tuple.source_owner, backend);
            assert_eq!(receipt.tuple.destination.owner, sp);
            assert_eq!(receipt.tuple.destination.subaccount, None);
        }
        other => panic!("proportional refund receipt mismatch: {other:?}"),
    }
    let principal_consumed = request.three_usd_amount_e8s - refund_amount;
    assert!(principal_consumed > 0);
    assert_eq!(payout_receipt.tuple.ledger, icp);
    assert_eq!(payout_receipt.tuple.source.owner, backend);
    assert_eq!(payout_receipt.tuple.destination, account(sp));
    assert_eq!(
        payout_receipt.tuple.net_amount_e8s + payout_receipt.tuple.fee_e8s,
        payout_receipt.tuple.gross_amount_e8s,
    );

    assert_eq!(sp_lp_before - balance(&pic, pool, sp), principal_consumed);
    assert_eq!(
        balance(&pic, pool, backend) - backend_lp_before,
        request.three_usd_amount_e8s - refund_amount,
    );
    let status_after: StabilityPoolStatus = result(
        pic.query_call(
            sp,
            Principal::anonymous(),
            "get_pool_status",
            encode_args(()).unwrap(),
        )
        .unwrap(),
        "pool status after absorption",
    );
    assert_eq!(
        tracked_lp_before
            - status_after
                .stablecoin_balances
                .get(&pool)
                .copied()
                .unwrap_or_default(),
        principal_consumed
    );
    assert_eq!(
        balance(&pic, icp, sp),
        payout_receipt.tuple.net_amount_e8s,
        "SP receives exactly the ledger-net collateral payout in the receipt",
    );
    assert_eq!(
        terminal_result.collateral_received,
        payout_receipt.tuple.gross_amount_e8s,
        "backend result records the gross collateral release; the receipt separately proves the ledger-net SP payout"
    );

    if let Some(refund_vault_id) = proportional_refund_vault_id {
        // Replenish the SP with real LP tokens for a second, independent
        // absorption. The feature Wasm pauses vault #3 after the real 3USD
        // transfer commits and before the backend snapshots vault debt.
        let approve_second_lp = icrc_ledger_types::icrc2::approve::ApproveArgs {
            from_subaccount: None,
            spender: account(sp),
            amount: Nat::from(lp_deposit),
            expected_allowance: None,
            expires_at: None,
            fee: None,
            memo: None,
            created_at_time: None,
        };
        let approved: Result<Nat, icrc_ledger_types::icrc2::approve::ApproveError> = result(
            call(
                &pic,
                pool,
                user,
                "icrc2_approve",
                encode_one(approve_second_lp).unwrap(),
            ),
            "approve LP for proportional-refund absorption",
        );
        approved.expect("second LP deposit approved");
        let deposited: Result<(), candid::Reserved> = result(
            call(
                &pic,
                sp,
                user,
                "deposit",
                encode_args((pool, lp_deposit)).unwrap(),
            ),
            "deposit LP for proportional-refund absorption",
        );
        deposited.expect("second LP deposit succeeds");

        let sp_before_refund_case = balance(&pic, pool, sp);
        let backend_before_refund_case = balance(&pic, pool, backend);
        let status_before_refund_case: StabilityPoolStatus = result(
            pic.query_call(
                sp,
                Principal::anonymous(),
                "get_pool_status",
                encode_args(()).unwrap(),
            )
            .unwrap(),
            "pool status before proportional-refund absorption",
        );
        let tracked_before_refund_case = status_before_refund_case
            .stablecoin_balances
            .get(&pool)
            .copied()
            .unwrap_or_default();
        assert_eq!(sp_before_refund_case, lp_deposit);
        assert_eq!(tracked_before_refund_case, lp_deposit);

        let notification_call = match driver {
            ThreeUsdAbsorptionDriver::BackendPush
            | ThreeUsdAbsorptionDriver::UpgradeRecovery
            | ThreeUsdAbsorptionDriver::LostReplyAfterBackendCommit => pic
                .submit_call(
                    backend,
                    admin,
                    "dev_test_pool_only_liquidation",
                    encode_one(refund_vault_id).unwrap(),
                )
                .expect("submit second liquidation notification"),
            ThreeUsdAbsorptionDriver::PublicKeeper => pic
                .submit_call(
                    sp,
                    user,
                    "execute_liquidation",
                    encode_one(refund_vault_id).unwrap(),
                )
                .expect("submit second public keeper liquidation"),
        };
        let gate = wait_for_canister_http(
            &pic,
            "https://three-usd-proportional-refund.test/after-pull",
        );
        let held_ingress: Result<ThreeUsdReserveIngressV2StatusView, candid::Reserved> = result(
            pic.query_call(
                backend,
                sp,
                "get_three_usd_reserve_ingress_v2_status",
                encode_args((refund_vault_id, 2u64)).unwrap(),
            )
            .unwrap(),
            "query ingress while paused after 3USD pull",
        );
        let held_ingress = held_ingress.expect("registered SP reads held ingress status");
        let held_request = held_ingress
            .request
            .expect("request is pinned before repayment interleaving");
        assert!(held_request.icusd_debt_covered_e8s > 9_000_000_000);

        if matches!(driver, ThreeUsdAbsorptionDriver::UpgradeRecovery) {
            let transfer_block_index = match &held_ingress.status {
                ThreeUsdReserveIngressV2Status::TransferPending {
                    stage:
                        rumi_protocol_backend::ThreeUsdReserveIngressV2PendingStage::TransferConfirmed {
                            transfer_block_index,
                            ..
                        },
                } => *transfer_block_index,
                other => panic!("expected confirmed pull at held barrier: {other:?}"),
            };
            let sp_after_pull = balance(&pic, pool, sp);
            assert_eq!(
                sp_before_refund_case - sp_after_pull,
                lp_deposit,
                "the first attempt must pull the exact pinned amount once"
            );

            // The backend is suspended after persisting TransferConfirmed. Its
            // upgrade must retain that tuple and block index; the SP's pending
            // absorb must also survive its own upgrade after the interrupted
            // inter-canister call settles.
            pic.upgrade_canister(
                backend,
                backend_wasm(),
                encode_one(rumi_protocol_backend::ProtocolArg::Upgrade(
                    rumi_protocol_backend::UpgradeArg {
                        mode: None,
                        description: Some("3USD held ingress recovery test".into()),
                    },
                ))
                .unwrap(),
                None,
            )
            .expect("upgrade backend while ingress is held after pull");
            // The outstanding management HTTP response was bound to the
            // interrupted pre-upgrade message. Release it so PocketIC can
            // drain that callback/rejection before the SP is upgraded.
            if pic
                .get_canister_http()
                .iter()
                .any(|request| request.request_id == gate.request_id)
            {
                release_canister_http(&pic, gate.clone());
            }
            for _ in 0..20 {
                pic.tick();
            }
            let _interrupted_notification = pic.await_call(notification_call);
            for _ in 0..5 {
                pic.tick();
            }
            pic.upgrade_canister(
                sp,
                pool_wasm(),
                encode_one(StabilityPoolInitArgs {
                    protocol_canister_id: backend,
                    authorized_admins: vec![admin],
                })
                .unwrap(),
                None,
            )
            .expect("upgrade SP with its held absorb journal");

            let after_backend_upgrade: Result<
                ThreeUsdReserveIngressV2StatusView,
                candid::Reserved,
            > = result(
                pic.query_call(
                    backend,
                    sp,
                    "get_three_usd_reserve_ingress_v2_status",
                    encode_args((refund_vault_id, 2u64)).unwrap(),
                )
                .unwrap(),
                "read retained ingress after backend upgrade",
            );
            let after_backend_upgrade =
                after_backend_upgrade.expect("registered SP reads retained ingress after upgrade");
            assert!(matches!(
                &after_backend_upgrade.status,
                ThreeUsdReserveIngressV2Status::TransferPending {
                    stage:
                        rumi_protocol_backend::ThreeUsdReserveIngressV2PendingStage::TransferConfirmed {
                            transfer_block_index: retained,
                            ..
                        },
                } if *retained == transfer_block_index
            ));

            let attached: Result<(), rumi_protocol_backend::ProtocolError> = result(
                call(
                    &pic,
                    backend,
                    sp,
                    "attach_my_three_usd_reserve_ingress_v2_candidate",
                    encode_args((refund_vault_id, 2u64, transfer_block_index)).unwrap(),
                ),
                "attach exact confirmed ingress block",
            );
            attached.expect("exact confirmed block is idempotently accepted");

            // The public SP recovery endpoint dispatches before it consumes
            // optional candidates. The backend already has this exact
            // confirmed block, so the SP can safely reconcile from that
            // durable status after the explicit idempotent attachment above.
            let recovery_call = pic
                .submit_call(
                    sp,
                    admin,
                    "recover_three_usd_absorb_v2",
                    encode_args((2u64, None::<u64>, None::<u64>, None::<u64>)).unwrap(),
                )
                .expect("submit SP recovery after exact block attachment");
            let retry_gate = wait_for_canister_http_after(
                &pic,
                "https://three-usd-proportional-refund.test/after-pull",
                gate.request_id,
            );
            assert_eq!(
                sp_after_pull,
                balance(&pic, pool, sp),
                "retry from TransferConfirmed must not pull 3USD again"
            );
            release_canister_http(&pic, retry_gate);
            for _ in 0..20 {
                pic.tick();
            }
            let recovered: Result<(), stability_pool::types::StabilityPoolError> = result(
                pic.await_call(recovery_call)
                    .expect("exact-candidate recovery call completes"),
                "complete exact-candidate SP recovery",
            );
            recovered.expect("exact transfer candidate reconciles the held absorb");
            assert_eq!(balance(&pic, pool, sp), sp_after_pull);
        } else {
            // The backend has already pulled the SP's 10 icUSD-equivalent 3USD,
            // but has not yet read the vault. Repay 1 icUSD through the official
            // ledger and V2 repayment endpoint while that call is parked.
            let repay_amount = 1_000_000_000u64;
            let approve_repay = icrc_ledger_types::icrc2::approve::ApproveArgs {
                from_subaccount: None,
                spender: account(backend),
                amount: Nat::from(repay_amount),
                expected_allowance: None,
                expires_at: None,
                fee: None,
                memo: None,
                created_at_time: None,
            };
            let approved_repay: Result<Nat, icrc_ledger_types::icrc2::approve::ApproveError> =
                result(
                    call(
                        &pic,
                        icusd,
                        user,
                        "icrc2_approve",
                        encode_one(approve_repay).unwrap(),
                    ),
                    "approve icUSD repayment during ingress pause",
                );
            approved_repay.expect("icUSD repayment approved");
            let repaid: Result<
                rumi_protocol_backend::RepaymentV2StatusView,
                rumi_protocol_backend::ProtocolError,
            > = result(
                call(
                    &pic,
                    backend,
                    user,
                    "repay_to_vault_v2",
                    encode_args((
                        1u128,
                        VaultArg {
                            vault_id: refund_vault_id,
                            amount: repay_amount,
                        },
                    ))
                    .unwrap(),
                ),
                "repay while proportional-refund ingress is paused",
            );
            let repaid = repaid.expect("concurrent repayment commits");
            assert_eq!(
                repaid.phase,
                rumi_protocol_backend::RepaymentV2Phase::Complete
            );
            assert_eq!(repaid.effective_amount_raw, repay_amount);
            let vaults: Vec<rumi_protocol_backend::vault::CandidVault> = result(
                pic.query_call(
                    backend,
                    Principal::anonymous(),
                    "get_vaults",
                    encode_one(Some(user)).unwrap(),
                )
                .unwrap(),
                "read repaid vault before releasing ingress",
            );
            let repaid_vault = vaults
                .iter()
                .find(|vault| vault.vault_id == refund_vault_id)
                .expect("repaid vault remains open");
            assert_eq!(repaid_vault.borrowed_icusd_amount, 9_000_000_000);

            release_canister_http(&pic, gate);
            for _ in 0..20 {
                pic.tick();
            }
            match driver {
                ThreeUsdAbsorptionDriver::BackendPush => {
                    let notification: Result<String, rumi_protocol_backend::ProtocolError> = result(
                        pic.await_call(notification_call).unwrap_or_else(|error| {
                            panic!("second liquidation notification: {error}")
                        }),
                        "complete second liquidation notification",
                    );
                    assert!(notification
                        .expect("second backend notification succeeds")
                        .contains("sent to stability pool"));
                }
                ThreeUsdAbsorptionDriver::PublicKeeper => {
                    let liquidation: Result<
                        stability_pool::types::LiquidationResult,
                        stability_pool::types::StabilityPoolError,
                    > = result(
                        pic.await_call(notification_call).unwrap_or_else(|error| {
                            panic!("second public keeper liquidation: {error}")
                        }),
                        "complete second public keeper liquidation",
                    );
                    assert!(
                        liquidation
                            .expect("second public keeper liquidation succeeds")
                            .success
                    );
                }
                ThreeUsdAbsorptionDriver::UpgradeRecovery
                | ThreeUsdAbsorptionDriver::LostReplyAfterBackendCommit => {
                    unreachable!("handled above")
                }
            }
        }

        let ingress: Result<ThreeUsdReserveIngressV2StatusView, candid::Reserved> = result(
            pic.query_call(
                backend,
                sp,
                "get_three_usd_reserve_ingress_v2_status",
                encode_args((refund_vault_id, 2u64)).unwrap(),
            )
            .unwrap(),
            "query proportional-refund ingress",
        );
        let ingress = ingress.expect("registered SP reads proportional-refund ingress");
        let request = ingress.request.expect("second ingress request retained");
        let (_transfer_index, transfer_tuple, terminal, refund) = match ingress.status {
            ThreeUsdReserveIngressV2Status::Absorbed {
                transfer_block_index,
                transfer_tuple,
                result,
                proportional_refund,
                ..
            } => (
                transfer_block_index,
                transfer_tuple,
                result,
                proportional_refund,
            ),
            other => panic!("second ingress did not absorb: {other:?}"),
        };
        assert_eq!(request.icusd_debt_covered_e8s, 10_000_000_000);
        assert_eq!(request.three_usd_amount_e8s, lp_deposit);
        assert_eq!(
            terminal.liquidated_debt,
            if matches!(
                driver,
                ThreeUsdAbsorptionDriver::UpgradeRecovery
                    | ThreeUsdAbsorptionDriver::LostReplyAfterBackendCommit
            ) {
                10_000_000_000
            } else {
                9_000_000_000
            }
        );
        assert_eq!(transfer_tuple.amount_e8s, lp_deposit);
        let expected_refund = lp_deposit
            - ((lp_deposit as u128 * terminal.liquidated_debt as u128)
                / request.icusd_debt_covered_e8s as u128) as u64;
        if matches!(
            driver,
            ThreeUsdAbsorptionDriver::UpgradeRecovery
                | ThreeUsdAbsorptionDriver::LostReplyAfterBackendCommit
        ) {
            assert_eq!(expected_refund, 0);
            assert!(refund.is_none());
        } else {
            assert!(expected_refund > 0);
            let refund = refund
                .as_ref()
                .expect("positive proportional refund has a real receipt");
            assert_eq!(refund.tuple.amount_e8s, expected_refund);
            assert_eq!(refund.tuple.source_owner, backend);
            assert_eq!(refund.tuple.destination, account(sp));
            let refund_blocks: GetBlocksResult = result(
                pic.query_call(
                    pool,
                    Principal::anonymous(),
                    "icrc3_get_blocks",
                    encode_args((vec![GetBlocksRequest {
                        start: refund.block_index.into(),
                        length: Nat::from(1u64),
                    }],))
                    .unwrap(),
                )
                .unwrap(),
                "fetch proportional-refund ledger block",
            );
            let refund_block = refund_blocks
                .blocks
                .iter()
                .find(|block| block.id == Nat::from(refund.block_index))
                .expect("refund block retained by official ledger");
            let decoded_refund =
                rumi_protocol_backend::icrc3_proof::decode_block(&refund_block.block)
                    .expect("decode official 3USD refund block");
            rumi_protocol_backend::icrc3_proof::validate_three_usd_default_source_refund_block(
                &decoded_refund,
                &refund.tuple,
            )
            .expect("refund block proves exact backend-to-SP transfer");
        }
        let principal_consumed = lp_deposit - expected_refund;
        assert_eq!(
            sp_before_refund_case - balance(&pic, pool, sp),
            principal_consumed
        );
        assert_eq!(
            balance(&pic, pool, backend) - backend_before_refund_case,
            principal_consumed
        );
        let status_after_refund_case: StabilityPoolStatus = result(
            pic.query_call(
                sp,
                Principal::anonymous(),
                "get_pool_status",
                encode_args(()).unwrap(),
            )
            .unwrap(),
            "pool status after proportional-refund absorption",
        );
        assert_eq!(
            tracked_before_refund_case
                - status_after_refund_case
                    .stablecoin_balances
                    .get(&pool)
                    .copied()
                    .unwrap_or_default(),
            principal_consumed
        );
    }
}
