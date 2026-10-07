use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use icrc_ledger_types::icrc3::archive::QueryArchiveFn;
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use sha2::{Digest, Sha224, Sha256};
use std::{
    process::Command,
    time::{Duration, SystemTime},
};

use rumi_protocol_backend::event::Event;
use rumi_protocol_backend::EventTypeFilter;
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

#[derive(CandidType, Deserialize, Clone, Debug)]
enum NativeLedgerArg {
    #[serde(rename = "Init")]
    Init(NativeIcpInitArgs),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct IcpTokens {
    e8s: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct LedgerDuration {
    secs: u64,
    nanos: u32,
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
    ckusdc_ledger_principal: Option<Principal>,
    fee_e8s: u64,
    developer_principal: Principal,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum ProtocolArgVariant {
    Init(ProtocolInitArg),
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
    debt_covered: u64,
    collateral_price_e8s: u64,
    claim_timestamp: Option<u64>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct BotStatsResponse {
    liquidation_bot_principal: Option<Principal>,
    budget_total_e8s: u64,
    budget_remaining_e8s: u64,
    budget_start_timestamp: u64,
    total_debt_covered_e8s: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct BotConfig {
    backend_principal: Principal,
    treasury_principal: Principal,
    admin: Principal,
    max_slippage_bps: u16,
    icp_ledger: Principal,
    ckusdc_ledger: Principal,
    icpswap_pool: Principal,
    icpswap_zero_for_one: Option<bool>,
    icp_fee_e8s: Option<u64>,
    ckusdc_fee_e6: Option<u64>,
    three_pool_principal: Option<Principal>,
    kong_swap_principal: Option<Principal>,
    ckusdt_ledger: Option<Principal>,
    icusd_ledger: Option<Principal>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct BotInitArgs {
    config: BotConfig,
}

#[derive(CandidType, Deserialize, Clone, Debug, Default)]
struct GetEventsArg {
    start: u64,
    length: u64,
    types: Option<Vec<EventTypeFilter>>,
    principal: Option<Principal>,
    collateral_token: Option<Principal>,
    time_range: Option<EventTimeRange>,
    min_size_e8s: Option<u64>,
    admin_labels: Option<Vec<String>>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct EventTimeRange {
    start_ns: u64,
    end_ns: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct GetEventsFilteredResponse {
    total: u64,
    events: Vec<(u64, Event)>,
}

// ─── WASM fixtures ───

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
    archive_trigger_threshold: u64,
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
            num_blocks_to_archive: archive_trigger_threshold,
            max_transactions_per_response: Some(100),
            trigger_threshold: archive_trigger_threshold,
            max_message_size_bytes: Some(1_048_576),
            cycles_for_archive_creation: Some(1_000_000_000_000),
            node_max_memory_size_bytes: Some(1_073_741_824),
            controller_id: controller,
            more_controller_ids: None,
        }),
        send_whitelist: Vec::new(),
        transfer_fee: Some(IcpTokens { e8s: 10_000 }),
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

fn flaky_ledger_wasm() -> Vec<u8> {
    include_bytes!("../../../../target/wasm32-unknown-unknown/release/flaky_ledger.wasm").to_vec()
}

fn liquidation_bot_wasm() -> Vec<u8> {
    include_bytes!("../../../../target/wasm32-unknown-unknown/release/liquidation_bot.wasm")
        .to_vec()
}

fn protocol_wasm() -> Vec<u8> {
    include_bytes!("../../../../target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm")
        .to_vec()
}

fn xrc_wasm() -> Vec<u8> {
    include_bytes!("../../../xrc_demo/xrc/xrc.wasm").to_vec()
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

fn open_vault_v2_call(
    pic: &PocketIc,
    protocol_id: Principal,
    owner: Principal,
    request_id: u128,
    collateral_amount: u64,
) -> u64 {
    let result = pic
        .update_call(
            protocol_id,
            owner,
            "open_vault_v2",
            encode_args((request_id, collateral_amount, None::<Principal>)).unwrap(),
        )
        .expect("open_vault_v2 update failed");
    let bytes = match result {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("open_vault_v2 rejected: {message}"),
    };
    let opened: Result<rumi_protocol_backend::InboundCollateralStatusView, ProtocolError> =
        decode_one(&bytes).expect("decode open_vault_v2 result");
    let status = opened.expect("open_vault_v2 returned error");
    let result = match status.result.clone() {
        Some(result) => result,
        None => panic!(
            "open_vault_v2 did not complete: phase={:?}, last_error={:?}, \
             candidate_block_index={:?}, had_ambiguous_attempt={}, status={status:?}",
            status.phase,
            status.last_error,
            status.candidate_block_index,
            status.had_ambiguous_attempt,
        ),
    };
    match result {
        rumi_protocol_backend::InboundCollateralResultView::Open { vault_id, .. } => vault_id,
        other => panic!("unexpected open_vault_v2 result: {other:?}"),
    }
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

#[derive(CandidType, Deserialize)]
struct NativeGetBlocksArgs {
    start: u64,
    length: u64,
}

#[derive(CandidType, Deserialize)]
struct NativeQueryBlocksArchiveView {
    archived_blocks: Vec<NativeArchivedBlocksRange>,
}

#[derive(CandidType, Deserialize)]
struct NativeBlockRange {
    blocks: Vec<candid::Reserved>,
}

#[derive(CandidType, Deserialize)]
enum NativeArchiveError {
    BadFirstBlockIndex {
        requested_index: u64,
        first_valid_index: u64,
    },
    Other {
        error_code: u64,
        error_message: String,
    },
}

#[derive(CandidType, Deserialize)]
struct NativeArchivedBlocksRange {
    start: u64,
    length: u64,
    callback: QueryArchiveFn<NativeGetBlocksArgs, Result<NativeBlockRange, NativeArchiveError>>,
}

fn native_block_is_archived(f: &Fixture, block_index: u64) -> bool {
    let result = f
        .pic
        .query_call(
            f.icp_ledger,
            Principal::anonymous(),
            "query_blocks",
            encode_one(NativeGetBlocksArgs {
                start: block_index,
                length: 1,
            })
            .expect("encode query_blocks"),
        )
        .expect("query_blocks call failed");
    let response: NativeQueryBlocksArchiveView = match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode query_blocks archive view"),
        WasmResult::Reject(message) => panic!("query_blocks rejected: {message}"),
    };
    let matching: Vec<_> = response
        .archived_blocks
        .iter()
        .filter(|descriptor| {
            descriptor.length > 0
                && descriptor.start <= block_index
                && descriptor
                    .start
                    .checked_add(descriptor.length)
                    .is_some_and(|end| block_index < end)
        })
        .collect();
    assert!(
        matching.len() <= 1,
        "official ledger returned overlapping archive descriptors for block {block_index}"
    );
    let Some(descriptor) = matching.first() else {
        return false;
    };
    assert_ne!(
        descriptor.callback.canister_id, f.icp_ledger,
        "archived receipt should be served by the official ledger archive canister"
    );
    assert!(
        !descriptor.callback.method.is_empty(),
        "archive descriptor must identify its callback method"
    );
    true
}

fn xrc_set_rate(
    pic: &PocketIc,
    xrc: Principal,
    sender: Principal,
    base: &str,
    quote: &str,
    rate_e8s: u64,
) {
    let result = pic
        .update_call(
            xrc,
            sender,
            "set_exchange_rate",
            encode_args((base.to_string(), quote.to_string(), rate_e8s)).unwrap(),
        )
        .expect("set_exchange_rate call failed");
    match result {
        WasmResult::Reply(_) => {}
        WasmResult::Reject(m) => panic!("set_exchange_rate rejected: {}", m),
    }
}

fn set_xrc_fetch_interval(pic: &PocketIc, protocol_id: Principal, developer: Principal, secs: u64) {
    let result = pic
        .update_call(
            protocol_id,
            developer,
            "set_xrc_fetch_interval_secs",
            encode_args((secs,)).unwrap(),
        )
        .expect("set_xrc_fetch_interval_secs call failed");
    let parsed: Result<(), ProtocolError> = match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode XRC interval update"),
        WasmResult::Reject(message) => panic!("set_xrc_fetch_interval_secs rejected: {message}"),
    };
    parsed.expect("set_xrc_fetch_interval_secs returned error");
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

fn get_active_claim_ids(pic: &PocketIc, protocol_id: Principal) -> Vec<u64> {
    let result = pic
        .query_call(
            protocol_id,
            Principal::anonymous(),
            "get_bot_claim_vault_ids",
            encode_args(()).unwrap(),
        )
        .expect("get_bot_claim_vault_ids failed");
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode active bot claim ids"),
        WasmResult::Reject(message) => panic!("get_bot_claim_vault_ids rejected: {message}"),
    }
}

fn get_reconciliation_events(pic: &PocketIc, protocol_id: Principal) -> Vec<Event> {
    let args = GetEventsArg {
        start: 0,
        length: 200,
        types: Some(vec![EventTypeFilter::BotClaimReconciliationNeeded]),
        ..Default::default()
    };
    let result = pic
        .query_call(
            protocol_id,
            Principal::anonymous(),
            "get_events_filtered",
            encode_one(args).unwrap(),
        )
        .expect("get_events_filtered failed");
    let parsed: GetEventsFilteredResponse = match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode filtered events"),
        WasmResult::Reject(message) => panic!("get_events_filtered rejected: {message}"),
    };
    parsed.events.into_iter().map(|(_, event)| event).collect()
}

fn now_ns(pic: &PocketIc) -> u64 {
    pic.get_time()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("PocketIC time precedes Unix epoch")
        .as_nanos() as u64
}

fn claim_return_subaccount(vault_id: u64, claim_timestamp: u64) -> [u8; 32] {
    let mut subaccount = [0u8; 32];
    subaccount[..8].copy_from_slice(&vault_id.to_be_bytes());
    subaccount[8..16].copy_from_slice(&claim_timestamp.to_be_bytes());
    subaccount[16..24].copy_from_slice(b"BOTRET01");
    subaccount
}

fn transfer_exact_claim_return(
    fixture: &Fixture,
    vault_id: u64,
    claim_timestamp: u64,
    collateral_amount: u64,
) -> (u64, u64, u64) {
    const FEE_E8S: u64 = 10_000;
    let created_at_time = now_ns(&fixture.pic).max(claim_timestamp);
    let mut memo = b"BOTRET02".to_vec();
    memo.extend_from_slice(&vault_id.to_be_bytes());
    memo.extend_from_slice(&claim_timestamp.to_be_bytes());
    let args = TransferArg {
        from_subaccount: None,
        to: Account {
            owner: fixture.protocol_id,
            subaccount: Some(claim_return_subaccount(vault_id, claim_timestamp)),
        },
        amount: Nat::from(collateral_amount + FEE_E8S),
        fee: Some(Nat::from(FEE_E8S)),
        memo: Some(memo),
        created_at_time: Some(created_at_time),
    };
    let result = fixture
        .pic
        .update_call(
            fixture.icp_ledger,
            fixture.bot_id,
            "icrc1_transfer",
            encode_one(args).unwrap(),
        )
        .expect("claim-generation return transfer failed");
    let block_index = match result {
        WasmResult::Reply(bytes) => {
            let parsed: Result<Nat, TransferError> =
                decode_one(&bytes).expect("decode return transfer");
            use num_traits::ToPrimitive;
            parsed
                .expect("claim-generation return transfer returned error")
                .0
                .to_u64()
                .unwrap()
        }
        WasmResult::Reject(message) => panic!("return transfer rejected: {message}"),
    };
    (block_index, created_at_time, FEE_E8S)
}

fn cancel_with_generation(
    fixture: &Fixture,
    vault_id: u64,
    claim_timestamp: u64,
    return_block_index: u64,
    return_created_at_time: u64,
    return_fee_e8s: u64,
) -> Result<(), ProtocolError> {
    let result = fixture
        .pic
        .update_call(
            fixture.protocol_id,
            fixture.bot_id,
            "bot_cancel_liquidation_with_generation",
            encode_args((
                vault_id,
                claim_timestamp,
                return_block_index,
                return_created_at_time,
                return_fee_e8s,
            ))
            .unwrap(),
        )
        .expect("generation-bound cancel call failed");
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode generation-bound cancel"),
        WasmResult::Reject(message) => panic!("generation-bound cancel rejected: {message}"),
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

fn enroll_bot_request_id_floor(fixture: &Fixture) -> u64 {
    let result = fixture
        .pic
        .update_call(
            fixture.protocol_id,
            fixture.developer,
            "enroll_bot_claim_request_id_floor",
            encode_args(()).unwrap(),
        )
        .expect("enroll_bot_claim_request_id_floor failed");
    let parsed: Result<u64, ProtocolError> = match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode request-ID enrollment"),
        WasmResult::Reject(message) => panic!("request-ID enrollment rejected: {message}"),
    };
    parsed.expect("request-ID floor enrollment failed")
}

fn bot_claim_call(
    fixture: &Fixture,
    bot: Principal,
    vault_id: u64,
    request_id: u64,
) -> Result<BotLiquidationResult, ProtocolError> {
    let result = fixture
        .pic
        .update_call(
            fixture.protocol_id,
            bot,
            "bot_claim_liquidation_with_request_id",
            encode_args((vault_id, request_id)).unwrap(),
        )
        .expect("bot_claim_liquidation call failed");
    match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode bot_claim_liquidation"),
        WasmResult::Reject(m) => panic!("bot_claim_liquidation rejected: {}", m),
    }
}

// ─── Fixture ───

struct Fixture {
    pic: PocketIc,
    protocol_id: Principal,
    bot_id: Principal,
    icp_ledger: Principal,
    #[allow(dead_code)]
    icusd_ledger: Principal,
    ckusdc_ledger: Principal,
    xrc_id: Principal,
    developer: Principal,
    #[allow(dead_code)]
    test_user: Principal,
    /// Pre-opened vault id with 50 ICP collateral and 100 icUSD borrowed.
    /// At $10/ICP starting price → CR = 500%. Drop ICP to $2.50 to push
    /// below the 133% liquidation threshold without latching ReadOnly.
    vault_id: u64,
    second_vault_id: u64,
}

fn setup_fixture() -> Fixture {
    setup_fixture_with_archive_threshold(1_000)
}

fn setup_fixture_with_archive_threshold(archive_trigger_threshold: u64) -> Fixture {
    let pic = PocketIcBuilder::new().with_nns_subnet().build();

    let test_user = Principal::self_authenticating(b"bot_001b_pic_user");
    let developer = Principal::self_authenticating(b"bot_001b_pic_developer");
    let treasury = Principal::self_authenticating(b"bot_001b_pic_treasury");
    let protocol_id = pic.create_canister();
    pic.add_cycles(protocol_id, 2_000_000_000_000);
    pic.set_controllers(protocol_id, None, vec![Principal::anonymous(), developer])
        .expect("set_controllers failed");
    let bot_id = pic.create_canister();
    pic.add_cycles(bot_id, 2_000_000_000_000);

    let icp_ledger = deploy_native_icp_ledger(
        &pic,
        test_user,
        1_000_000_000_000,
        developer,
        archive_trigger_threshold,
    );

    // Use the receipt-capable flaky ledger for icUSD. Configuring the backend
    // as its minting account makes borrow produce the exact typed 1mint block
    // required by the current borrow-receipt verifier.
    let icusd_ledger = pic.create_canister();
    pic.add_cycles(icusd_ledger, 2_000_000_000_000);
    pic.install_canister(
        icusd_ledger,
        flaky_ledger_wasm(),
        encode_one(()).unwrap(),
        None,
    );

    // Keep payment proofs on a separate ledger whose minting account is not
    // the backend. Transfers to the backend must remain transfer blocks, not
    // be interpreted by ICRC-1 as burns.
    let ckusdc_ledger = pic.create_canister();
    pic.add_cycles(ckusdc_ledger, 2_000_000_000_000);
    pic.install_canister(
        ckusdc_ledger,
        flaky_ledger_wasm(),
        encode_one(()).unwrap(),
        None,
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
        ckusdc_ledger_principal: Some(ckusdc_ledger),
    });
    pic.install_canister(
        protocol_id,
        protocol_wasm(),
        encode_args((init,)).expect("encode protocol init"),
        None,
    );

    let minting_account_result = pic
        .update_call(
            icusd_ledger,
            Principal::anonymous(),
            "set_minting_account",
            encode_one(Some(account(protocol_id))).unwrap(),
        )
        .expect("configure icUSD minting account");
    match minting_account_result {
        WasmResult::Reply(bytes) => decode_one::<()>(&bytes).expect("decode minting account setup"),
        WasmResult::Reject(message) => panic!("set_minting_account rejected: {message}"),
    }

    pic.install_canister(
        bot_id,
        liquidation_bot_wasm(),
        encode_one(BotInitArgs {
            config: BotConfig {
                backend_principal: protocol_id,
                treasury_principal: treasury,
                admin: developer,
                max_slippage_bps: 200,
                icp_ledger,
                ckusdc_ledger,
                icpswap_pool: xrc_id,
                icpswap_zero_for_one: Some(true),
                icp_fee_e8s: Some(10_000),
                ckusdc_fee_e6: Some(0),
                three_pool_principal: None,
                kong_swap_principal: None,
                ckusdt_ledger: None,
                icusd_ledger: Some(icusd_ledger),
            },
        })
        .expect("encode liquidation bot init"),
        None,
    );

    // Keep the outlier-confirmation samples inside the ten-minute ICP-price
    // freshness window. The test intentionally retains production sanity
    // checks and confirms the new rate through the normal XRC timer.
    set_xrc_fetch_interval(&pic, protocol_id, developer, 60);

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

    icrc2_approve_call(&pic, icp_ledger, test_user, protocol_id, 50_000_000_000u128);
    let vault_id = open_vault_v2_call(&pic, protocol_id, test_user, 1, 5_000_000_000);

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

    // Open a second equally collateralized vault before changing the XRC
    // price. The shared ICP ledger account will then contain unrelated pool
    // collateral that must never stand in for either claim's return proof.
    icrc2_approve_call(&pic, icp_ledger, test_user, protocol_id, 50_000_000_000u128);
    let second_vault_id = open_vault_v2_call(&pic, protocol_id, test_user, 2, 5_000_000_000);
    let second_borrow = pic
        .update_call(
            protocol_id,
            test_user,
            "borrow_from_vault",
            encode_args((VaultArg {
                vault_id: second_vault_id,
                amount: 10_000_000_000,
            },))
            .unwrap(),
        )
        .expect("second borrow_from_vault failed");
    match second_borrow {
        WasmResult::Reply(bytes) => {
            let r: Result<SuccessWithFee, ProtocolError> =
                decode_one(&bytes).expect("decode second borrow");
            r.expect("second borrow_from_vault returned error");
        }
        WasmResult::Reject(msg) => panic!("second borrow rejected: {}", msg),
    }

    Fixture {
        pic,
        protocol_id,
        bot_id,
        icp_ledger,
        icusd_ledger,
        ckusdc_ledger,
        xrc_id,
        developer,
        test_user,
        vault_id,
        second_vault_id,
    }
}

/// Walk ICP down in fresh, within-band XRC steps. Each step is accepted by
/// the production sanity rule (ratio remains above 0.7), so this fixture does
/// not depend on outlier-confirmation timing.
fn drop_icp_price(fixture: &Fixture, new_price_e8s: u64) {
    for rate in [
        750_000_000,
        562_500_000,
        421_875_000,
        316_406_250,
        new_price_e8s,
    ] {
        xrc_set_rate(
            &fixture.pic,
            fixture.xrc_id,
            fixture.developer,
            "ICP",
            "USD",
            rate,
        );
        fixture.pic.advance_time(Duration::from_secs(61));
        for _ in 0..15 {
            fixture.pic.tick();
        }
    }
}

/// Make `vault_id` underwater AND configure the bot, then have the bot
/// claim the vault. After this call, the bot principal holds the
/// collateral and the protocol has a live `bot_claims` entry. Returns
/// `(pre_claim_budget, claim)` so callers can compare against the
/// post-claim budget without races against the bot-config write.
fn seed_bot_claim(fixture: &Fixture) -> (u64, u64, BotLiquidationResult) {
    set_liquidation_bot_config_admin(fixture, fixture.bot_id, 1_000_000_000_000u64);
    let request_id_floor = enroll_bot_request_id_floor(fixture);
    assert_eq!(
        request_id_floor, 0,
        "fresh bot should begin at request ID zero"
    );

    // Drop to $2.50/ICP. Vault: 50 ICP × $2.50 / $100 debt = 125% CR
    // (< 133% liq threshold → liquidatable) yet TCR also = 125% (> 100%
    // → no ReadOnly auto-latch, so subsequent operations stay open).
    drop_icp_price(fixture, 250_000_000);

    let pre_claim_budget = get_bot_stats(&fixture.pic, fixture.protocol_id).budget_remaining_e8s;

    let claim = bot_claim_call(fixture, fixture.bot_id, fixture.vault_id, 1)
        .expect("bot_claim_liquidation must succeed against underwater vault");
    let claim_timestamp = claim
        .claim_timestamp
        .expect("claim result must expose its persisted generation");

    (pre_claim_budget, claim_timestamp, claim)
}
