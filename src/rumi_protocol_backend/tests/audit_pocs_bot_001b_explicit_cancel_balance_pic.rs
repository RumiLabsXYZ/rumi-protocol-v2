//! Wave-12 BOT-001b: explicit `bot_cancel_liquidation` collateral-return
//! verification — Layer 3 PocketIC fence.
//!
//! Wave 11 closed the *unattended* path (`check_vaults` auto-cancel after the
//! 10-min timeout). Wave 12 closes the *explicit* path: the bot itself
//! calling `bot_cancel_liquidation` with the claim still in place. Before
//! Wave 12, that endpoint queried the protocol's collateral balance but
//! only logged the result — a buggy bot could clear its claim and restore
//! its budget without ever returning the seized collateral.
//!
//! This fence exercises the canister-boundary path:
//!
//!   * `bot_cancel_liquidation` requires a verified gross-collateral return
//!     proof before clearing the claim;
//!   * a missing or under-sized proof returns
//!     `Err(ProtocolError::GenericError(_))` and leaves `bot_claims` and
//!     `bot_budget_remaining_e8s` UNCHANGED — the bot is forced to retry
//!     its transfer or follow the exact collateral-return proof flow;
//!   * an exact full-gross return (with the bot paying its return fee) succeeds —
//!     the claim is cleared and the budget restored, preserving the
//!     pre-Wave-12 happy path.
//!
//! The native-ledger test below also exercises a return block through the
//! official ledger's archive callback.
//!
//! Fixture is lifted from `audit_pocs_bot_001_auto_cancel_balance_pic.rs`.
//! As that fixture's comments call out, the ICP ledger uses a dedicated
//! minter (NOT the protocol) so the protocol holds a real balance the
//! gate can observe.

use candid::{
    decode_one, encode_args, encode_one, CandidType, Deserialize, IDLArgs, IDLValue, Nat, Principal,
};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use sha2::{Digest, Sha224, Sha256};
use std::{
    process::Command,
    time::{Duration, SystemTime},
};

use rumi_protocol_backend::ProtocolError;

// ─── Local mirrors of ICRC-1 Candid types ───

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
struct Account {
    owner: Principal,
    subaccount: Option<[u8; 32]>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct FeatureFlags {
    icrc2: bool,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
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

#[derive(CandidType, Deserialize, Clone, Debug)]
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

#[derive(CandidType, Deserialize, Clone, Debug)]
struct InitArgs {
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

#[derive(CandidType, Deserialize, Clone, Debug)]
enum LedgerArg {
    #[serde(rename = "Init")]
    Init(InitArgs),
    #[serde(rename = "Upgrade")]
    Upgrade(Option<()>),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct IcpTokens {
    e8s: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
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

#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeLedgerInitArgs {
    minting_account: String,
    icrc1_minting_account: Option<Account>,
    initial_values: Vec<(String, IcpTokens)>,
    max_message_size_bytes: Option<u64>,
    transaction_window: Option<NativeLedgerDuration>,
    archive_options: Option<NativeArchiveOptions>,
    send_whitelist: Vec<Principal>,
    transfer_fee: Option<IcpTokens>,
    token_symbol: Option<String>,
    token_name: Option<String>,
    feature_flags: Option<FeatureFlags>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeLedgerDuration {
    secs: u64,
    nanos: u32,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum NativeLedgerArg {
    #[serde(rename = "Init")]
    Init(NativeLedgerInitArgs),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeGetBlocksArgs {
    start: u64,
    length: u64,
}

const NATIVE_ICP_LEDGER_TEXT: &str = "ryjl3-tyaaa-aaaaa-aaaba-cai";
const NATIVE_ICP_LEDGER_GZIP_SHA256: &str =
    "51f4be010f23064137defacd627ffbec024c5133210c68ca3b80ab8f257101d6";

#[derive(CandidType, Deserialize, Clone, Debug)]
struct ApproveArgs {
    from_subaccount: Option<[u8; 32]>,
    spender: Account,
    amount: Nat,
    expected_allowance: Option<Nat>,
    expires_at: Option<u64>,
    fee: Option<Nat>,
    memo: Option<Vec<u8>>,
    created_at_time: Option<u64>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum ApproveError {
    BadFee { expected_fee: Nat },
    InsufficientFunds { balance: Nat },
    AllowanceChanged { current_allowance: Nat },
    Expired { ledger_time: u64 },
    TooOld,
    CreatedInFuture { ledger_time: u64 },
    Duplicate { duplicate_of: Nat },
    TemporarilyUnavailable,
    GenericError { error_code: Nat, message: String },
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct TransferArg {
    from_subaccount: Option<[u8; 32]>,
    to: Account,
    amount: Nat,
    fee: Option<Nat>,
    memo: Option<Vec<u8>>,
    created_at_time: Option<u64>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum TransferError {
    BadFee { expected_fee: Nat },
    BadBurn { min_burn_amount: Nat },
    InsufficientFunds { balance: Nat },
    TooOld,
    CreatedInFuture { ledger_time: u64 },
    Duplicate { duplicate_of: Nat },
    TemporarilyUnavailable,
    GenericError { error_code: Nat, message: String },
}

// ─── Backend init / vault types ───

#[derive(CandidType, Deserialize, Clone, Debug)]
struct ProtocolInitArg {
    xrc_principal: Principal,
    icusd_ledger_principal: Principal,
    icp_ledger_principal: Principal,
    fee_e8s: u64,
    developer_principal: Principal,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum ProtocolArgVariant {
    Init(ProtocolInitArg),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum StableTokenType {
    CKUSDT,
    CKUSDC,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct VaultArg {
    vault_id: u64,
    amount: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct OpenVaultSuccess {
    vault_id: u64,
    block_index: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct SuccessWithFee {
    block_index: u64,
    fee_amount_paid: u64,
    collateral_amount_received: Option<u64>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct BotLiquidationResult {
    vault_id: u64,
    collateral_amount: u64,
    collateral_received_amount: Option<u64>,
    collateral_outbound_fee: Option<u64>,
    debt_covered: u64,
    collateral_price_e8s: u64,
    claim_generation: u64,
    collateral_return_memo: Vec<u8>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct BotCollateralReturnProofArg {
    vault_id: u64,
    claim_generation: u64,
    block_index: u64,
    amount: u64,
    created_at_time: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct BotStatsResponse {
    liquidation_bot_principal: Option<Principal>,
    budget_total_e8s: u64,
    budget_remaining_e8s: u64,
    budget_start_timestamp: u64,
    total_debt_covered_e8s: u64,
}

// ─── WASM fixtures ───

fn icrc1_ledger_wasm() -> Vec<u8> {
    include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec()
}

fn official_native_icp_ledger_wasm() -> Vec<u8> {
    let path = std::env::var("RUMI_TEST_NNS_LEDGER_WASM_GZ")
        .expect("set RUMI_TEST_NNS_LEDGER_WASM_GZ to the pinned official NNS ledger gzip");
    let gzip = std::fs::read(&path).unwrap_or_else(|error| panic!("read {path}: {error}"));
    assert_eq!(
        format!("{:x}", Sha256::digest(&gzip)),
        NATIVE_ICP_LEDGER_GZIP_SHA256,
        "pinned official NNS ledger gzip hash"
    );
    let output = Command::new("gzip")
        .args(["-dc", &path])
        .output()
        .expect("run gzip to decompress the pinned official NNS ledger");
    assert!(
        output.status.success(),
        "gzip failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn native_account_identifier_text(owner: Principal) -> String {
    let mut hasher = Sha224::new();
    hasher.update(b"\x0Aaccount-id");
    hasher.update(owner.as_slice());
    hasher.update([0; 32]);
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

fn protocol_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm")
        .to_vec()
}

fn xrc_wasm() -> Vec<u8> {
    include_bytes!("../../xrc_demo/xrc/xrc.wasm").to_vec()
}

#[derive(CandidType, Deserialize, Clone, Debug, Default)]
struct MockXRC {
    rates: Vec<(String, u64)>,
}

fn prepare_mock_xrc() -> Vec<u8> {
    let mock = MockXRC {
        rates: vec![("ICP/USD".to_string(), 1_000_000_000)], // $10.00 (e8s)
    };
    encode_one(mock).expect("encode mock XRC init")
}

// ─── Helpers ───

fn account(owner: Principal) -> Account {
    Account {
        owner,
        subaccount: None,
    }
}

fn deploy_icrc1_ledger(
    pic: &PocketIc,
    minting_account: Account,
    transfer_fee: u64,
    initial_balances: Vec<(Account, Nat)>,
    name: &str,
    symbol: &str,
    controller: Principal,
) -> Principal {
    let ledger_id = pic.create_canister();
    pic.add_cycles(ledger_id, 2_000_000_000_000);
    let init = InitArgs {
        minting_account,
        fee_collector_account: None,
        transfer_fee: Nat::from(transfer_fee),
        decimals: Some(8),
        max_memo_length: Some(64),
        token_name: name.into(),
        token_symbol: symbol.into(),
        metadata: vec![],
        initial_balances,
        feature_flags: Some(FeatureFlags { icrc2: true }),
        maximum_number_of_accounts: None,
        accounts_overflow_trim_quantity: None,
        archive_options: ArchiveOptions {
            num_blocks_to_archive: 2000,
            trigger_threshold: 1000,
            controller_id: controller,
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
        encode_args((LedgerArg::Init(init),)).expect("encode ledger init"),
        None,
    );
    ledger_id
}

fn icrc2_approve_call(
    pic: &PocketIc,
    ledger: Principal,
    sender: Principal,
    spender: Principal,
    amount: u128,
) {
    let args = ApproveArgs {
        from_subaccount: None,
        spender: account(spender),
        amount: Nat::from(amount),
        expected_allowance: None,
        expires_at: None,
        fee: None,
        memo: None,
        created_at_time: None,
    };
    let result = pic
        .update_call(ledger, sender, "icrc2_approve", encode_one(args).unwrap())
        .expect("icrc2_approve call failed");
    let parsed: Result<Nat, ApproveError> = match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode icrc2_approve"),
        WasmResult::Reject(m) => panic!("icrc2_approve rejected: {}", m),
    };
    parsed.expect("approve returned error");
}

fn icrc1_transfer_call(
    pic: &PocketIc,
    ledger: Principal,
    sender: Principal,
    to: Principal,
    amount: u128,
) {
    let args = TransferArg {
        from_subaccount: None,
        to: account(to),
        amount: Nat::from(amount),
        fee: None,
        memo: None,
        created_at_time: None,
    };
    let result = pic
        .update_call(ledger, sender, "icrc1_transfer", encode_one(args).unwrap())
        .expect("icrc1_transfer call failed");
    let parsed: Result<Nat, TransferError> = match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode icrc1_transfer"),
        WasmResult::Reject(m) => panic!("icrc1_transfer rejected: {}", m),
    };
    parsed.expect("transfer returned error");
}

fn icrc1_transfer_tuple_call(
    pic: &PocketIc,
    ledger: Principal,
    sender: Principal,
    to: Principal,
    amount: u64,
    memo: Vec<u8>,
    created_at_time: u64,
) -> u64 {
    use num_traits::ToPrimitive;

    let args = TransferArg {
        from_subaccount: None,
        to: account(to),
        amount: Nat::from(amount),
        fee: None,
        memo: Some(memo),
        created_at_time: Some(created_at_time),
    };
    let result = pic
        .update_call(ledger, sender, "icrc1_transfer", encode_one(args).unwrap())
        .expect("icrc1_transfer tuple call failed");
    let parsed: Result<Nat, TransferError> = match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode icrc1_transfer tuple"),
        WasmResult::Reject(message) => panic!("icrc1_transfer tuple rejected: {message}"),
    };
    parsed
        .expect("collateral return transfer should commit")
        .0
        .to_u64()
        .expect("collateral return block should fit u64")
}

fn record_return_proof(fixture: &Fixture, proof: BotCollateralReturnProofArg) {
    record_return_proof_result(fixture, proof).expect("return proof should be accepted");
}

fn record_return_proof_result(
    fixture: &Fixture,
    proof: BotCollateralReturnProofArg,
) -> Result<(), ProtocolError> {
    let result = fixture
        .pic
        .update_call(
            fixture.protocol_id,
            fixture.developer,
            "bot_record_collateral_return_proof",
            encode_one(proof).unwrap(),
        )
        .expect("bot_record_collateral_return_proof call failed");
    let parsed: Result<(), ProtocolError> = match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode return proof"),
        WasmResult::Reject(message) => panic!("return proof rejected: {message}"),
    };
    parsed
}

fn record_field<'a>(value: &'a IDLValue, name: &str) -> &'a IDLValue {
    let IDLValue::Record(fields) = value else {
        panic!("expected record, got {value:?}")
    };
    fields
        .iter()
        .find(|field| field.id.get_id() == candid::idl_hash(name))
        .map(|field| &field.val)
        .unwrap_or_else(|| panic!("missing {name} in {value:?}"))
}

fn idl_u64(value: &IDLValue) -> u64 {
    match value {
        IDLValue::Nat64(value) => *value,
        IDLValue::Nat(value) => u64::try_from(value.0.clone()).expect("value fits u64"),
        other => panic!("expected nat64/nat, got {other:?}"),
    }
}

fn native_block_is_archived(pic: &PocketIc, ledger: Principal, block_index: u64) -> bool {
    let response = pic
        .query_call(
            ledger,
            Principal::anonymous(),
            "query_blocks",
            encode_one(NativeGetBlocksArgs {
                start: block_index,
                length: 1,
            })
            .unwrap(),
        )
        .expect("native ICP query_blocks call");
    let bytes = match response {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("native query_blocks rejected: {message}"),
    };
    let decoded = IDLArgs::from_bytes(&bytes).expect("decode native query_blocks response");
    let response = &decoded.args[0];
    let IDLValue::Vec(blocks) = record_field(response, "blocks") else {
        panic!("native query_blocks blocks is not a vector")
    };
    if !blocks.is_empty() {
        return false;
    }
    let IDLValue::Vec(archives) = record_field(response, "archived_blocks") else {
        panic!("native query_blocks archived_blocks is not a vector")
    };
    archives.iter().any(|archive| {
        let start = idl_u64(record_field(archive, "start"));
        let length = idl_u64(record_field(archive, "length"));
        start <= block_index
            && start
                .checked_add(length)
                .is_some_and(|end| block_index < end)
    })
}

fn current_ledger_time_ns(pic: &PocketIc) -> u64 {
    pic.get_time()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("PocketIC time should be after Unix epoch")
        .as_nanos() as u64
}

fn icrc1_balance_of_call(pic: &PocketIc, ledger: Principal, owner: Principal) -> u64 {
    let result = pic
        .query_call(
            ledger,
            Principal::anonymous(),
            "icrc1_balance_of",
            encode_one(account(owner)).unwrap(),
        )
        .expect("icrc1_balance_of call failed");
    let parsed: Nat = match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode balance"),
        WasmResult::Reject(m) => panic!("balance rejected: {}", m),
    };
    use num_traits::ToPrimitive;
    parsed.0.to_u64().unwrap_or(0)
}

fn get_bot_stats(pic: &PocketIc, protocol_id: Principal) -> BotStatsResponse {
    let result = pic
        .query_call(
            protocol_id,
            Principal::anonymous(),
            "get_bot_stats",
            encode_args(()).unwrap(),
        )
        .expect("get_bot_stats call failed");
    match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode get_bot_stats"),
        WasmResult::Reject(m) => panic!("get_bot_stats rejected: {}", m),
    }
}

fn set_liquidation_bot_config_admin(
    fixture: &Fixture,
    bot_principal: Principal,
    monthly_budget_e8s: u64,
) {
    let result = fixture
        .pic
        .update_call(
            fixture.protocol_id,
            fixture.developer,
            "set_liquidation_bot_config",
            encode_args((bot_principal, monthly_budget_e8s)).unwrap(),
        )
        .expect("set_liquidation_bot_config call failed");
    let parsed: Result<(), ProtocolError> = match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode set_liquidation_bot_config"),
        WasmResult::Reject(m) => panic!("set_liquidation_bot_config rejected: {}", m),
    };
    parsed.expect("set_liquidation_bot_config returned error");
}

fn bot_claim_call(
    fixture: &Fixture,
    bot: Principal,
    vault_id: u64,
) -> Result<BotLiquidationResult, ProtocolError> {
    let result = fixture
        .pic
        .update_call(
            fixture.protocol_id,
            bot,
            "bot_claim_liquidation",
            encode_args((vault_id,)).unwrap(),
        )
        .expect("bot_claim_liquidation call failed");
    match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode bot_claim_liquidation"),
        WasmResult::Reject(m) => panic!("bot_claim_liquidation rejected: {}", m),
    }
}

fn bot_cancel_liquidation_call(
    fixture: &Fixture,
    bot: Principal,
    vault_id: u64,
) -> Result<(), ProtocolError> {
    let result = fixture
        .pic
        .update_call(
            fixture.protocol_id,
            bot,
            "bot_cancel_liquidation",
            encode_args((vault_id,)).unwrap(),
        )
        .expect("bot_cancel_liquidation call failed");
    match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode bot_cancel_liquidation"),
        WasmResult::Reject(m) => panic!("bot_cancel_liquidation rejected: {}", m),
    }
}

fn set_collateral_price_for_bot10_test(fixture: &Fixture, price_usd: f64) {
    let result = fixture
        .pic
        .update_call(
            fixture.protocol_id,
            fixture.developer,
            "dev_set_collateral_price",
            encode_args((fixture.icp_ledger, price_usd)).unwrap(),
        )
        .expect("dev_set_collateral_price call failed");
    let response: Result<String, ProtocolError> = match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode dev_set_collateral_price"),
        WasmResult::Reject(message) => panic!("dev_set_collateral_price rejected: {message}"),
    };
    response.expect("dev_set_collateral_price returned error");
}

// ─── Fixture ───

struct Fixture {
    pic: PocketIc,
    protocol_id: Principal,
    icp_ledger: Principal,
    #[allow(dead_code)]
    icusd_ledger: Principal,
    developer: Principal,
    #[allow(dead_code)]
    test_user: Principal,
    /// Pre-opened vault id with 50 ICP collateral and 100 icUSD borrowed.
    /// At $10/ICP starting price → CR = 500%. Drop ICP to $2.50 to push
    /// below the 133% liquidation threshold without latching ReadOnly.
    vault_id: u64,
}

fn setup_fixture() -> Fixture {
    setup_fixture_with_native_ledger(None)
}

fn setup_fixture_with_native_ledger(native_ledger_wasm: Option<Vec<u8>>) -> Fixture {
    let pic = PocketIcBuilder::new().with_nns_subnet().build();

    let test_user = Principal::self_authenticating(b"bot_001b_pic_user");
    let developer = Principal::self_authenticating(b"bot_001b_pic_developer");
    let treasury = Principal::self_authenticating(b"bot_001b_pic_treasury");
    // Wave-11/12 BOT-001*: the ICP ledger MUST use a minting account that is
    // NOT the protocol. The LIQ-008 fixture uses `protocol_id` as the
    // minting account for convenience, but that turns every protocol →
    // bot transfer into a mint (and bot → protocol return into a burn) —
    // which makes `icrc1_balance_of(protocol_id)` always 0 and breaks the
    // BOT-001b gate's premise. A separate minter lets the protocol hold a
    // real balance that the gate can observe.
    let icp_minter = Principal::self_authenticating(b"bot_001b_pic_icp_minter");

    let protocol_id = pic.create_canister();
    pic.add_cycles(protocol_id, 2_000_000_000_000);
    pic.set_controllers(protocol_id, None, vec![Principal::anonymous(), developer])
        .expect("set_controllers failed");

    let icp_ledger = if let Some(wasm) = native_ledger_wasm {
        let ledger =
            Principal::from_text(NATIVE_ICP_LEDGER_TEXT).expect("native ICP ledger principal");
        pic.create_canister_with_id(None, None, ledger)
            .expect("create fixed native ICP ledger principal");
        pic.add_cycles(ledger, 5_000_000_000_000_000);
        let init = NativeLedgerArg::Init(NativeLedgerInitArgs {
            minting_account: native_account_identifier_text(Principal::management_canister()),
            icrc1_minting_account: Some(account(Principal::management_canister())),
            initial_values: vec![
                (
                    native_account_identifier_text(test_user),
                    IcpTokens {
                        e8s: 1_000_000_000_000,
                    },
                ),
                (
                    native_account_identifier_text(developer),
                    IcpTokens {
                        e8s: 1_000_000_000_000,
                    },
                ),
            ],
            max_message_size_bytes: Some(1_048_576),
            transaction_window: None,
            archive_options: Some(NativeArchiveOptions {
                num_blocks_to_archive: 1,
                max_transactions_per_response: Some(100),
                trigger_threshold: 2,
                max_message_size_bytes: Some(1_048_576),
                cycles_for_archive_creation: Some(1_000_000_000_000),
                node_max_memory_size_bytes: Some(1_073_741_824),
                controller_id: developer,
                more_controller_ids: None,
            }),
            send_whitelist: Vec::new(),
            transfer_fee: Some(IcpTokens { e8s: 10_000 }),
            token_symbol: Some("ICP".into()),
            token_name: Some("Internet Computer".into()),
            feature_flags: Some(FeatureFlags { icrc2: true }),
        });
        pic.install_canister(ledger, wasm, encode_args((init,)).unwrap(), None);
        ledger
    } else {
        deploy_icrc1_ledger(
            &pic,
            account(icp_minter),
            10_000,
            vec![
                (account(test_user), Nat::from(1_000_000_000_000u64)),
                // The bot must supply the outbound and return fees from its
                // own balance to return the full gross claim collateral.
                (account(developer), Nat::from(100_000u64)),
            ],
            "Internet Computer Protocol",
            "ICP",
            developer,
        )
    };

    // icUSD ledger keeps protocol as minter — that's how icUSD actually
    // works (the protocol mints/burns icUSD on borrow/repay). Only the
    // collateral ledger needs a separate minter.
    let icusd_ledger = deploy_icrc1_ledger(
        &pic,
        account(protocol_id),
        0,
        vec![],
        "icUSD",
        "icUSD",
        developer,
    );

    let xrc_id = pic.create_canister();
    pic.add_cycles(xrc_id, 1_000_000_000_000);
    pic.install_canister(xrc_id, xrc_wasm(), prepare_mock_xrc(), None);

    pic.set_time(SystemTime::UNIX_EPOCH + Duration::from_secs(1_711_324_800));

    let init = ProtocolArgVariant::Init(ProtocolInitArg {
        fee_e8s: 10_000,
        icp_ledger_principal: icp_ledger,
        xrc_principal: xrc_id,
        icusd_ledger_principal: icusd_ledger,
        developer_principal: developer,
    });
    pic.install_canister(
        protocol_id,
        protocol_wasm(),
        encode_args((init,)).expect("encode protocol init"),
        None,
    );

    pic.advance_time(Duration::from_secs(1));
    for _ in 0..10 {
        pic.tick();
    }

    // Quiet down rate / fee curves so the vault math stays predictable across
    // ticks. Same boilerplate as the LIQ-008 / BOT-001 fixtures.
    let _ = pic
        .update_call(
            protocol_id,
            developer,
            "set_borrowing_fee_curve",
            encode_args((None::<String>,)).unwrap(),
        )
        .expect("set_borrowing_fee_curve");
    let _ = pic
        .update_call(
            protocol_id,
            developer,
            "set_rate_curve_markers",
            encode_args((None::<Principal>, vec![(1.5f64, 1.0f64), (3.0f64, 1.0f64)])).unwrap(),
        )
        .expect("set_rate_curve_markers");
    let _ = pic
        .update_call(
            protocol_id,
            developer,
            "set_borrowing_fee",
            encode_args((0.0f64,)).unwrap(),
        )
        .expect("set_borrowing_fee");
    let _ = pic
        .update_call(
            protocol_id,
            developer,
            "set_interest_rate",
            encode_args((icp_ledger, 0.0f64)).unwrap(),
        )
        .expect("set_interest_rate");

    let _ = pic
        .update_call(
            protocol_id,
            developer,
            "set_treasury_principal",
            encode_args((treasury,)).unwrap(),
        )
        .expect("set_treasury_principal");

    // Bot claim admission pins a ckUSDC ledger for later exact payment proof.
    let payment_ledger = deploy_icrc1_ledger(
        &pic,
        account(treasury),
        10_000,
        vec![(account(developer), Nat::from(100_000_000_000u64))],
        "ckUSDC fixture",
        "ckUSDC",
        developer,
    );
    let stable_ledger_result = pic
        .update_call(
            protocol_id,
            developer,
            "set_stable_ledger_principal",
            encode_args((StableTokenType::CKUSDC, payment_ledger)).unwrap(),
        )
        .expect("set_stable_ledger_principal call failed");
    let stable_ledger_result: Result<(), ProtocolError> = match stable_ledger_result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode stable ledger setter"),
        WasmResult::Reject(message) => panic!("stable ledger setter rejected: {message}"),
    };
    stable_ledger_result.expect("ckUSDC ledger configuration should succeed");

    icrc2_approve_call(&pic, icp_ledger, test_user, protocol_id, 50_000_000_000u128);
    let open_result = pic
        .update_call(
            protocol_id,
            test_user,
            "open_vault",
            encode_args((5_000_000_000u64, None::<Principal>)).unwrap(),
        )
        .expect("open_vault failed");
    let vault_id = match open_result {
        WasmResult::Reply(bytes) => {
            let r: Result<OpenVaultSuccess, ProtocolError> =
                decode_one(&bytes).expect("decode open_vault");
            r.expect("open_vault returned error").vault_id
        }
        WasmResult::Reject(msg) => panic!("open_vault rejected: {}", msg),
    };

    let borrow_arg = VaultArg {
        vault_id,
        amount: 10_000_000_000u64, // 100 icUSD borrowed
    };
    let borrow_result = pic
        .update_call(
            protocol_id,
            test_user,
            "borrow_from_vault",
            encode_args((borrow_arg,)).unwrap(),
        )
        .expect("borrow_from_vault failed");
    match borrow_result {
        WasmResult::Reply(bytes) => {
            let r: Result<SuccessWithFee, ProtocolError> =
                decode_one(&bytes).expect("decode borrow");
            r.expect("borrow_from_vault returned error");
        }
        WasmResult::Reject(msg) => panic!("borrow rejected: {}", msg),
    }

    Fixture {
        pic,
        protocol_id,
        icp_ledger,
        icusd_ledger,
        developer,
        test_user,
        vault_id,
    }
}

/// Set the cached ICP price through the same developer-only test endpoint
/// used by the BOT-10 PocketIC fixture.
fn drop_icp_price(fixture: &Fixture, new_price_e8s: u64) {
    let price_usd = new_price_e8s as f64 / 100_000_000.0;
    let result = fixture
        .pic
        .update_call(
            fixture.protocol_id,
            fixture.developer,
            "dev_set_collateral_price",
            encode_args((fixture.icp_ledger, price_usd)).unwrap(),
        )
        .expect("dev_set_collateral_price call failed");
    let parsed: Result<String, ProtocolError> = match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode dev_set_collateral_price"),
        WasmResult::Reject(message) => panic!("dev_set_collateral_price rejected: {message}"),
    };
    parsed.expect("dev_set_collateral_price returned error");
}

fn set_collateral_ledger_fee(fixture: &Fixture, fee_e8s: u64) {
    let result = fixture
        .pic
        .update_call(
            fixture.protocol_id,
            fixture.developer,
            "set_collateral_ledger_fee",
            encode_args((fixture.icp_ledger, fee_e8s)).unwrap(),
        )
        .expect("set_collateral_ledger_fee call failed");
    let parsed: Result<(), ProtocolError> = match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode collateral fee setter"),
        WasmResult::Reject(message) => panic!("set_collateral_ledger_fee rejected: {message}"),
    };
    parsed.expect("set_collateral_ledger_fee returned error");
}

/// Make `vault_id` underwater AND configure the bot, then have the bot
/// claim the vault. After this call, the bot principal holds the
/// collateral and the protocol has a live `bot_claims` entry. Returns
/// `(pre_claim_budget, claim)` so callers can compare against the
/// post-claim budget without races against the bot-config write.
fn seed_bot_claim(fixture: &Fixture) -> (u64, BotLiquidationResult) {
    // Bot = developer for fixture simplicity. Budget large enough for the
    // full 100 icUSD claim ($10k).
    set_liquidation_bot_config_admin(fixture, fixture.developer, 1_000_000_000_000u64);

    // Drop to $2.50/ICP. Vault: 50 ICP × $2.50 / $100 debt = 125% CR
    // (< 133% liq threshold → liquidatable) yet TCR also = 125% (> 100%
    // → no ReadOnly auto-latch, so subsequent operations stay open).
    drop_icp_price(fixture, 250_000_000);

    let pre_claim_budget = get_bot_stats(&fixture.pic, fixture.protocol_id).budget_remaining_e8s;

    let claim = bot_claim_call(fixture, fixture.developer, fixture.vault_id)
        .expect("bot_claim_liquidation must succeed against underwater vault");

    (pre_claim_budget, claim)
}

#[derive(Debug, PartialEq, Eq)]
struct StuckClaimObservableState {
    vaults: Vec<(u64, u64, u64, u64)>,
    active_claim_ids: Vec<u64>,
    bot_budget: (u64, u64, u64, u64),
    vault_events: Vec<(u64, rumi_protocol_backend::event::Event)>,
}

fn stuck_claim_observable_state(fixture: &Fixture) -> StuckClaimObservableState {
    let vaults: Vec<rumi_protocol_backend::vault::CandidVault> = match fixture
        .pic
        .query_call(
            fixture.protocol_id,
            Principal::anonymous(),
            "get_vaults",
            encode_args((Some(fixture.test_user),)).unwrap(),
        )
        .expect("get_vaults query failed")
    {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode get_vaults"),
        WasmResult::Reject(message) => panic!("get_vaults rejected: {message}"),
    };
    let vaults = vaults
        .into_iter()
        .map(|vault| {
            (
                vault.vault_id,
                vault.borrowed_icusd_amount,
                vault.collateral_amount,
                vault.accrued_interest,
            )
        })
        .collect();

    let active_claim_ids = match fixture
        .pic
        .query_call(
            fixture.protocol_id,
            Principal::anonymous(),
            "get_bot_claim_vault_ids",
            encode_args(()).unwrap(),
        )
        .expect("get_bot_claim_vault_ids query failed")
    {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode active claim ids"),
        WasmResult::Reject(message) => panic!("get_bot_claim_vault_ids rejected: {message}"),
    };

    let vault_events = match fixture
        .pic
        .query_call(
            fixture.protocol_id,
            Principal::anonymous(),
            "get_vault_history",
            encode_args((fixture.vault_id,)).unwrap(),
        )
        .expect("get_vault_history query failed")
    {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode get_vault_history"),
        WasmResult::Reject(message) => panic!("get_vault_history rejected: {message}"),
    };

    let stats = get_bot_stats(&fixture.pic, fixture.protocol_id);
    StuckClaimObservableState {
        vaults,
        active_claim_ids,
        bot_budget: (
            stats.budget_total_e8s,
            stats.budget_remaining_e8s,
            stats.budget_start_timestamp,
            stats.total_debt_covered_e8s,
        ),
        vault_events,
    }
}

fn admin_resolve_stuck_claim_call(
    fixture: &Fixture,
    caller: Principal,
    apply_debt_reduction: bool,
) -> Result<(), ProtocolError> {
    let result = fixture
        .pic
        .update_call(
            fixture.protocol_id,
            caller,
            "admin_resolve_stuck_claim",
            encode_args((fixture.vault_id, apply_debt_reduction)).unwrap(),
        )
        .expect("admin_resolve_stuck_claim call failed");
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode admin resolver result"),
        WasmResult::Reject(message) => panic!("admin resolver rejected: {message}"),
    }
}

fn borrow_claimed_vault_call(fixture: &Fixture) -> Result<SuccessWithFee, ProtocolError> {
    let result = fixture
        .pic
        .update_call(
            fixture.protocol_id,
            fixture.test_user,
            "borrow_from_vault",
            encode_args((VaultArg {
                vault_id: fixture.vault_id,
                amount: 1_000_000_000,
            },))
            .unwrap(),
        )
        .expect("borrow_from_vault call failed");
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode borrow_from_vault result"),
        WasmResult::Reject(message) => panic!("borrow_from_vault rejected: {message}"),
    }
}

// ─── Tests ───

/// BOT-001b PIC #1: when the bot did NOT return the collateral, the
/// explicit `bot_cancel_liquidation` must reject with `GenericError` whose
/// message references the missing verified return proof, leaving `bot_claims`
/// and the budget unchanged while the bot still holds the net collateral.
#[test]
fn bot_001b_pic_explicit_cancel_rejected_when_balance_below_required() {
    let f = setup_fixture();

    let (pre_claim_budget, claim) = seed_bot_claim(&f);

    // Sanity: the budget was deducted by the claim.
    let post_claim_budget = get_bot_stats(&f.pic, f.protocol_id).budget_remaining_e8s;
    assert!(
        post_claim_budget < pre_claim_budget,
        "bot_claim_liquidation must deduct from budget (before {} after {})",
        pre_claim_budget,
        post_claim_budget
    );

    // Sanity: the bot now holds the net collateral after the explicit fee.
    let bot_balance = icrc1_balance_of_call(&f.pic, f.icp_ledger, f.developer);
    let expected_net = claim
        .collateral_amount
        .checked_sub(claim.collateral_outbound_fee.expect("new claim fee"))
        .expect("claim collateral covers its fee");
    assert_eq!(claim.collateral_received_amount, Some(expected_net));
    assert!(
        bot_balance >= expected_net,
        "bot must hold the net collateral; got {} expected at least {}",
        bot_balance,
        expected_net
    );

    // The protocol may still hold some residual collateral (the portion of
    // the vault NOT transferred to the bot). What matters for the gate is
    // that this residual is BELOW the gross claim amount — i.e. the proof
    // gate has a real shortfall to detect. If this
    // assertion ever fires, the bot's claim transfer didn't actually
    // remove enough collateral and the rest of the test is moot.
    let required = claim.collateral_amount;
    let protocol_balance_before = icrc1_balance_of_call(&f.pic, f.icp_ledger, f.protocol_id);
    assert!(
        protocol_balance_before < required,
        "protocol balance {} must be below gross claim {} while the bot retains its net collateral",
        protocol_balance_before,
        required
    );

    // Bot calls cancel without first returning the collateral → BOT-001b
    // gate must reject.
    let cancel_result = bot_cancel_liquidation_call(&f, f.developer, f.vault_id);
    let err = match cancel_result {
        Ok(()) => panic!("bot_cancel_liquidation must reject when collateral was not returned"),
        Err(e) => e,
    };
    let err_msg = match err {
        ProtocolError::GenericError(s) => s,
        other => panic!(
            "expected GenericError shortfall rejection, got {:?}",
            other
        ),
    };
    assert!(
        err_msg.contains("verified collateral-return proof"),
        "missing-proof error must reference the required claim-bound proof, got: {}",
        err_msg
    );
    assert!(
        err_msg.contains(&format!("vault #{}", f.vault_id)),
        "shortfall error must reference the vault id, got: {}",
        err_msg
    );

    // The rejection must NOT have cleared the claim or restored the
    // budget — `bot_claims` entry stays put, budget unchanged from
    // post-claim baseline.
    let final_budget = get_bot_stats(&f.pic, f.protocol_id).budget_remaining_e8s;
    assert_eq!(
        final_budget, post_claim_budget,
        "BOT-001b gate must prevent budget restore when collateral wasn't returned (saw budget {} vs expected {})",
        final_budget, post_claim_budget
    );

    // Probe: a *successful* cancel after the bot returns the collateral
    // confirms the claim was preserved across the rejection (otherwise the
    // follow-up cancel would error with "No active claim").
    let return_amount = claim.collateral_amount;
    let return_time = current_ledger_time_ns(&f.pic);
    let return_block = icrc1_transfer_tuple_call(
        &f.pic,
        f.icp_ledger,
        f.developer,
        f.protocol_id,
        return_amount,
        claim.collateral_return_memo.clone(),
        return_time,
    );
    record_return_proof(
        &f,
        BotCollateralReturnProofArg {
            vault_id: f.vault_id,
            claim_generation: claim.claim_generation,
            block_index: return_block,
            amount: return_amount,
            created_at_time: return_time,
        },
    );
    bot_cancel_liquidation_call(&f, f.developer, f.vault_id)
        .expect("retry must succeed once collateral is returned");
    let restored_budget = get_bot_stats(&f.pic, f.protocol_id).budget_remaining_e8s;
    assert_eq!(
        restored_budget, pre_claim_budget,
        "successful retry must restore budget to pre-claim baseline (saw {} expected {})",
        restored_budget, pre_claim_budget
    );
}

/// BOT-001b PIC #2: when the bot DID return the collateral, the explicit
/// `bot_cancel_liquidation` succeeds — clearing the claim and restoring
/// the budget. This preserves the pre-Wave-12 happy path so we don't
/// regress the bot's normal swap-failed retry flow.
#[test]
fn bot_001b_pic_explicit_cancel_succeeds_when_balance_sufficient() {
    let f = setup_fixture();

    let (pre_claim_budget, claim) = seed_bot_claim(&f);

    let post_claim_budget = get_bot_stats(&f.pic, f.protocol_id).budget_remaining_e8s;
    assert!(
        post_claim_budget < pre_claim_budget,
        "bot_claim_liquidation must deduct from budget"
    );

    // Bot returns the full gross claim amount to the protocol, paying the
    // return fee separately from its own balance.
    let return_amount = claim.collateral_amount;
    let return_time = current_ledger_time_ns(&f.pic);
    let return_block = icrc1_transfer_tuple_call(
        &f.pic,
        f.icp_ledger,
        f.developer,
        f.protocol_id,
        return_amount,
        claim.collateral_return_memo.clone(),
        return_time,
    );
    record_return_proof(
        &f,
        BotCollateralReturnProofArg {
            vault_id: f.vault_id,
            claim_generation: claim.claim_generation,
            block_index: return_block,
            amount: return_amount,
            created_at_time: return_time,
        },
    );

    // Sanity: the protocol's main account must now hold AT LEAST the full
    // gross claim amount so cancellation can restore the vault.
    let protocol_balance = icrc1_balance_of_call(&f.pic, f.icp_ledger, f.protocol_id);
    let required = claim.collateral_amount;
    assert!(
        protocol_balance >= required,
        "protocol balance {} must cover required {} after bot return",
        protocol_balance,
        required
    );

    // Explicit cancel must now succeed.
    bot_cancel_liquidation_call(&f, f.developer, f.vault_id)
        .expect("bot_cancel_liquidation must succeed when collateral is returned");

    // Budget restored to pre-claim baseline.
    let final_budget = get_bot_stats(&f.pic, f.protocol_id).budget_remaining_e8s;
    assert_eq!(
        final_budget, pre_claim_budget,
        "successful cancel must restore budget (saw {} expected pre-claim baseline {})",
        final_budget, pre_claim_budget
    );

    // Claim entry must be cleared: a second cancel call must error with
    // "No active claim", proving the first cancel truly removed it.
    let retry_err = bot_cancel_liquidation_call(&f, f.developer, f.vault_id)
        .expect_err("second cancel must fail; first one already cleared the claim");
    match retry_err {
        ProtocolError::GenericError(msg) => assert!(
            msg.contains("No active claim"),
            "expected 'No active claim' on second cancel, got: {}",
            msg
        ),
        other => panic!(
            "expected GenericError on second cancel, got {:?}",
            other
        ),
    }
}

/// Exercises both production bot receipt paths against the pinned official
/// native ICP ledger. The outbound claim block is verified while hot; the
/// return block is deliberately archived before the backend records it. The
/// backend fee config is drifted above the real ledger fee, and a return that
/// is short by one e8s of gross claim collateral is rejected.
#[test]
#[ignore = "requires the pinned official NNS ledger gzip and built backend Wasm; set RUMI_TEST_NNS_LEDGER_WASM_GZ"]
fn native_icp_bot_claim_and_return_proofs_use_archives_and_conserve_fees() {
    let f = setup_fixture_with_native_ledger(Some(official_native_icp_ledger_wasm()));
    // Make mutable backend config stale-high before admission. Claim creation
    // must refresh the ledger fee and pin the native ledger's real fee.
    set_collateral_ledger_fee(&f, 30_000);
    let backend_before = icrc1_balance_of_call(&f.pic, f.icp_ledger, f.protocol_id);
    let bot_before = icrc1_balance_of_call(&f.pic, f.icp_ledger, f.developer);
    let (_, claim) = seed_bot_claim(&f);

    let actual_fee = 10_000u64;
    let expected_net = claim
        .collateral_amount
        .checked_sub(actual_fee)
        .expect("gross claim exceeds native ledger fee");
    let backend_after = icrc1_balance_of_call(&f.pic, f.icp_ledger, f.protocol_id);
    let bot_after = icrc1_balance_of_call(&f.pic, f.icp_ledger, f.developer);
    assert_eq!(claim.collateral_received_amount, Some(expected_net));
    assert_eq!(claim.collateral_outbound_fee, Some(actual_fee));
    assert_eq!(
        backend_before.checked_sub(backend_after),
        Some(claim.collateral_amount),
        "backend total debit must be gross C = transfer amount N + ledger fee F"
    );
    assert_eq!(
        bot_after.checked_sub(bot_before),
        Some(expected_net),
        "bot account receives only the transfer amount N"
    );

    // Keep the stale-high config in place after outbound settlement. Return
    // proof must use the exact amount credited and ignore this mutable fee.
    let under_return_time = current_ledger_time_ns(&f.pic);
    let under_return_amount = claim.collateral_amount - 1;
    let under_return_block = icrc1_transfer_tuple_call(
        &f.pic,
        f.icp_ledger,
        f.developer,
        f.protocol_id,
        under_return_amount,
        claim.collateral_return_memo.clone(),
        under_return_time,
    );
    let under_return = record_return_proof_result(
        &f,
        BotCollateralReturnProofArg {
            vault_id: f.vault_id,
            claim_generation: claim.claim_generation,
            block_index: under_return_block,
            amount: under_return_amount,
            created_at_time: under_return_time,
        },
    );
    assert!(
        matches!(under_return, Err(ProtocolError::GenericError(ref message)) if message.contains("full gross claim amount")),
        "a return one e8s below the gross claim must fail even when its fee is separately paid, got {under_return:?}"
    );

    let wrong_memo_time = under_return_time + 1;
    let exact_return_amount = claim.collateral_amount;
    let wrong_memo_block = icrc1_transfer_tuple_call(
        &f.pic,
        f.icp_ledger,
        f.developer,
        f.protocol_id,
        exact_return_amount,
        b"wrong claim generation".to_vec(),
        wrong_memo_time,
    );
    let wrong_memo = record_return_proof_result(
        &f,
        BotCollateralReturnProofArg {
            vault_id: f.vault_id,
            claim_generation: claim.claim_generation,
            block_index: wrong_memo_block,
            amount: exact_return_amount,
            created_at_time: wrong_memo_time,
        },
    );
    assert!(
        matches!(wrong_memo, Err(ProtocolError::GenericError(ref message)) if message.contains("memo")),
        "wrong return memo must fail, got {wrong_memo:?}"
    );

    let return_time = wrong_memo_time + 1;
    let backend_before_exact_return =
        icrc1_balance_of_call(&f.pic, f.icp_ledger, f.protocol_id);
    let return_block = icrc1_transfer_tuple_call(
        &f.pic,
        f.icp_ledger,
        f.developer,
        f.protocol_id,
        exact_return_amount,
        claim.collateral_return_memo.clone(),
        return_time,
    );
    let mut archived = native_block_is_archived(&f.pic, f.icp_ledger, return_block);
    for index in 0..20u64 {
        if archived {
            break;
        }
        let filler_time = return_time + index + 1;
        icrc1_transfer_tuple_call(
            &f.pic,
            f.icp_ledger,
            f.test_user,
            f.developer,
            1,
            format!("native-ledger-archive-filler-{index}").into_bytes(),
            filler_time,
        );
        for _ in 0..20 {
            f.pic.tick();
        }
        archived = native_block_is_archived(&f.pic, f.icp_ledger, return_block);
    }
    assert!(
        archived,
        "official ledger did not archive return block {return_block}"
    );

    record_return_proof(
        &f,
        BotCollateralReturnProofArg {
            vault_id: f.vault_id,
            claim_generation: claim.claim_generation,
            block_index: return_block,
            amount: exact_return_amount,
            created_at_time: return_time,
        },
    );
    let backend_after_exact_return =
        icrc1_balance_of_call(&f.pic, f.icp_ledger, f.protocol_id);
    assert_eq!(
        backend_before_exact_return.checked_add(exact_return_amount),
        Some(backend_after_exact_return),
        "the return credits exact gross C; sender-paid return fee is separate"
    );
    bot_cancel_liquidation_call(&f, f.developer, f.vault_id)
        .expect("archived exact gross return proof must permit cancellation");
    assert_eq!(
        icrc1_balance_of_call(&f.pic, f.icp_ledger, f.protocol_id),
        backend_after_exact_return,
        "cancellation must not debit the returned collateral from backend custody"
    );
}

/// BOT-10: the legacy admin endpoint keeps its Candid signature but fails closed.
/// Neither boolean can clear the active claim or change vault accounting/budget,
/// regardless of whether the caller is the developer or an ordinary user.
#[test]
fn bot_010_pic_proofless_admin_resolution_preserves_claim_state() {
    let fixture = setup_fixture();
    let ledger_result = fixture
        .pic
        .update_call(
            fixture.protocol_id,
            fixture.developer,
            "set_stable_ledger_principal",
            encode_args((StableTokenType::CKUSDC, fixture.icusd_ledger)).unwrap(),
        )
        .expect("set_stable_ledger_principal call failed");
    let ledger_result: Result<(), ProtocolError> = match ledger_result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode stable ledger setter"),
        WasmResult::Reject(message) => panic!("stable ledger setter rejected: {message}"),
    };
    ledger_result.expect("ckUSDC ledger configuration should succeed");

    set_liquidation_bot_config_admin(&fixture, fixture.developer, 1_000_000_000_000u64);
    set_collateral_price_for_bot10_test(&fixture, 2.50);
    let claim = bot_claim_call(&fixture, fixture.developer, fixture.vault_id)
        .expect("bot claim should succeed against the underwater vault");
    assert_eq!(claim.vault_id, fixture.vault_id);
    let claimed_state = stuck_claim_observable_state(&fixture);
    assert!(claimed_state.active_claim_ids.contains(&fixture.vault_id));
    assert!(claimed_state
        .vaults
        .iter()
        .any(|vault| vault.0 == fixture.vault_id));

    for apply_debt_reduction in [false, true] {
        for (caller, expected_message) in [
            (fixture.developer, "Unsafe stuck-claim recovery disabled"),
            (fixture.test_user, "Unauthorized: developer only"),
        ] {
            let before = stuck_claim_observable_state(&fixture);
            assert_eq!(before, claimed_state, "claim state changed before call");

            let error = admin_resolve_stuck_claim_call(&fixture, caller, apply_debt_reduction)
            .expect_err("proofless legacy resolver must fail closed");
            match error {
                ProtocolError::GenericError(message) => assert!(
                    message.contains(expected_message),
                    "unexpected error for caller {caller} and apply_debt_reduction={apply_debt_reduction}: {message}"
                ),
                other => panic!("unexpected resolver error: {other:?}"),
            }

            assert_eq!(
                stuck_claim_observable_state(&fixture),
                before,
                "legacy resolver mutated claim, vault, budget, or events for caller {caller} and apply_debt_reduction={apply_debt_reduction}"
            );

            let borrow_error = borrow_claimed_vault_call(&fixture)
                .expect_err("user borrowing must stay blocked while the bot claim is active");
            match borrow_error {
                ProtocolError::GenericError(message) => assert!(
                    message.contains("is locked — bot liquidation in progress"),
                    "expected the vault processing lock after caller {caller} and apply_debt_reduction={apply_debt_reduction}, got: {message}"
                ),
                other => panic!("unexpected borrow result while claim is active: {other:?}"),
            }
            assert_eq!(
                stuck_claim_observable_state(&fixture),
                before,
                "borrow probe changed claim, vault, budget, or events after caller {caller} and apply_debt_reduction={apply_debt_reduction}"
            );
        }
    }
}
