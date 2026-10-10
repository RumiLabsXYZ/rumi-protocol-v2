//! Wave-4 ICC-002: 3USD compensating refund on
//! `stability_pool_liquidate_with_reserves` writedown failure — Layer 3
//! PocketIC fence.
//!
//! Wave-4 commit aabe002 added a compensating refund: when
//! `transfer_3usd_to_reserves` succeeds but `liquidate_vault_debt_already_burned`
//! returns Err, the backend refunds the pulled 3USD back to the stability pool
//! via Wave-3's idempotent transfer. The Wave-4 commit message explicitly
//! deferred a dedicated PocketIC fence for the refund path; this file closes
//! that gap.
//!
//! # What this file pins down
//!
//! Three scenarios at the canister boundary:
//!
//!   1. `icc_002_pic_happy_path_no_refund_no_orphan` — the control case.
//!      A clean reserves-path call: 3USD pulled, writedown succeeds,
//!      `protocol_3usd_reserves` accumulates the pulled amount, no refund
//!      side-channel triggered. Pins the success accounting so the failure
//!      tests have a baseline to contrast against.
//!
//!   2. `icc_002_pic_writedown_failure_refunds_3usd_to_sp` — the refund
//!      path. We arm `set_sp_writedown_disabled(true)` after the SP has
//!      approved the protocol to spend its 3USD; the entry point's pre-pull
//!      validation (which does NOT check the kill switch) passes, the pull
//!      lands, and `liquidate_vault_debt_already_burned` rejects with
//!      `TemporarilyUnavailable` before any state mutation. The refund is
//!      journaled before dispatch, then the worker restores the SP's 3USD
//!      balance while `protocol_3usd_reserves` stays at zero.
//!
//!   3. `icc_002_pic_refund_failure_enqueues_durable_retry_and_heals` — the
//!      refund-of-refund failure. Same setup as #2, but the 3USD ledger
//!      is `flaky_ledger` with `set_fail_transfers(true)`. The pull
//!      (`icrc2_transfer_from`) is unaffected by that knob and lands; the
//!      writedown rejects (kill switch); the refund (`icrc1_transfer`)
//!      fails. Asserts the fix that replaced the old strand-and-log-CRITICAL
//!      behavior:
//!        * the protocol surfaces the original writedown error,
//!        * the failed refund is persisted to `pending_3usd_refunds`
//!          (queried via `get_pending_3usd_refunds`) rather than lost,
//!        * once the ledger recovers, `process_pending_transfer` drains the
//!          queue, the SP is made whole, and the reserves subaccount empties.
//!      This closes the drift where a stranded refund left the SP's live 3USD
//!      below its tracked aggregate, blocking non-sole-holder withdrawals.
//!
//! # Why the kill switch is the right injection
//!
//! `liquidate_vault_debt_already_burned` checks `sp_writedown_disabled`
//! BEFORE the proof verification path. That makes the kill switch the
//! cleanest reliable trigger that returns `Err` without depending on
//! mid-flight state interleaving — and without exercising the
//! `fetch_and_validate_block` ICRC-3 round-trip that the
//! `flaky_ledger` doesn't implement. The Wave-4 refund arm is
//! status-agnostic: it fires on any `Err` from the writedown, so a
//! kill-switch reject exercises it identically to a real
//! "vault closed mid-flight" or "proof verification failed" error.
//!
//! Legacy V1 scenarios use `flaky_ledger` because the current `rumi_3pool` LP
//! token rejects the non-default reserve subaccount used by V1. This tests
//! V1 refund accounting without implying the old route works on current 3pool;
//! the V2 route uses the default account after its separate cutover.

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use ic_cdk::api::management_canister::http_request::{
    HttpResponse as MgmtHttpResponse, TransformArgs,
};
use pocket_ic::{PocketIc, WasmResult};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

#[path = "../../rumi_3pool/tests/common/mod.rs"]
mod three_pool_test_harness;

use rumi_protocol_backend::ProtocolError;

// ─── ic-icrc1-ledger Candid mirrors (standard ledger used as 3pool / icusd / icp) ───

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

// ─── HTTP request mirrors (for /logs probing) ───

#[derive(CandidType, Deserialize, Clone, Debug)]
struct HttpRequest {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct HttpResponse {
    status_code: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

#[derive(serde::Deserialize, Debug)]
struct LogEntryWire {
    #[allow(dead_code)]
    timestamp: u64,
    message: String,
}

#[derive(serde::Deserialize, Debug)]
struct LogWire {
    entries: Vec<LogEntryWire>,
}

// ─── Backend init / vault types (mirrored locally) ───

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
    Upgrade(ProtocolUpgradeArg),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct ProtocolUpgradeArg {
    mode: Option<()>,
    description: Option<String>,
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
struct StabilityPoolLiquidationResult {
    success: bool,
    vault_id: u64,
    liquidated_debt: u64,
    collateral_received: u64,
    collateral_type: String,
    block_index: u64,
    fee: u64,
    collateral_price_e8s: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct StabilityPoolInitArgs {
    protocol_canister_id: Principal,
    authorized_admins: Vec<Principal>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct StabilityPoolStablecoinConfig {
    ledger_id: Principal,
    symbol: String,
    decimals: u8,
    priority: u8,
    is_active: bool,
    transfer_fee: Option<u64>,
    is_lp_token: Option<bool>,
    underlying_pool: Option<Principal>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum StabilityPoolCollateralStatus {
    Active,
    Paused,
    Frozen,
    Sunset,
    Deprecated,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct StabilityPoolCollateralInfo {
    ledger_id: Principal,
    symbol: String,
    decimals: u8,
    status: StabilityPoolCollateralStatus,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct LiquidatableVaultInfo {
    vault_id: u64,
    collateral_type: Principal,
    debt_amount: u64,
    collateral_amount: u64,
    recommended_liquidation_amount: u64,
    collateral_price_e8s: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct PoolLiquidationResult {
    vault_id: u64,
    stables_consumed: BTreeMap<Principal, u64>,
    collateral_gained: u64,
    collateral_type: Principal,
    success: bool,
    error_message: Option<String>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct UserPositionView {
    stablecoin_balances: BTreeMap<Principal, u64>,
    collateral_gains: BTreeMap<Principal, u64>,
}

// ─── WASM fixtures ───

fn icrc1_ledger_wasm() -> Vec<u8> {
    include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec()
}

fn protocol_wasm() -> Vec<u8> {
    if let Some(path) = std::env::var_os("RUMI_N13_BACKEND_WASM") {
        return std::fs::read(&path)
            .unwrap_or_else(|error| panic!("read RUMI_N13_BACKEND_WASM {path:?}: {error}"));
    }
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm")
        .to_vec()
}

fn flaky_ledger_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/flaky_ledger.wasm").to_vec()
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

fn get_sp_position(pic: &PocketIc, sp: Principal, user: Principal) -> UserPositionView {
    match pic
        .query_call(
            sp,
            Principal::anonymous(),
            "get_user_position",
            encode_args((Some(user),)).unwrap(),
        )
        .expect("query Stability Pool position")
    {
        WasmResult::Reply(bytes) => decode_one::<Option<UserPositionView>>(&bytes)
            .expect("decode Stability Pool position")
            .expect("depositor position exists"),
        WasmResult::Reject(message) => {
            panic!("Stability Pool position query rejected: {message}")
        }
    }
}

fn protocol_3usd_reserves_subaccount() -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"protocol_3usd_reserves");
    hasher.finalize().into()
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

fn deploy_flaky_ledger(pic: &PocketIc) -> Principal {
    let id = pic.create_canister();
    pic.add_cycles(id, 2_000_000_000_000);
    pic.install_canister(id, flaky_ledger_wasm(), encode_one(()).unwrap(), None);
    id
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
        WasmResult::Reply(b) => decode_one(&b).expect("decode approve"),
        WasmResult::Reject(m) => panic!("approve rejected: {}", m),
    };
    parsed.expect("approve returned ledger error");
}

fn icrc1_balance_of(pic: &PocketIc, ledger: Principal, account_arg: Account) -> u128 {
    let result = pic
        .query_call(
            ledger,
            Principal::anonymous(),
            "icrc1_balance_of",
            encode_one(account_arg).unwrap(),
        )
        .expect("icrc1_balance_of call failed");
    let parsed: Nat = match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode balance"),
        WasmResult::Reject(m) => panic!("balance rejected: {}", m),
    };
    parsed.0.try_into().unwrap_or(0)
}

fn flaky_mint(pic: &PocketIc, ledger: Principal, owner: Principal, amount: u128) {
    let acct = Account {
        owner,
        subaccount: None,
    };
    pic.update_call(
        ledger,
        Principal::anonymous(),
        "mint",
        encode_args((acct, Nat::from(amount))).unwrap(),
    )
    .expect("flaky mint failed");
}

fn flaky_set_fail_transfers(pic: &PocketIc, ledger: Principal, fail: bool) {
    pic.update_call(
        ledger,
        Principal::anonymous(),
        "set_fail_transfers",
        encode_one(fail).unwrap(),
    )
    .expect("set_fail_transfers failed");
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
    if let WasmResult::Reject(m) = result {
        panic!("set_exchange_rate rejected: {}", m);
    }
}

fn get_protocol_3usd_reserves(pic: &PocketIc, protocol_id: Principal) -> u64 {
    let result = pic
        .query_call(
            protocol_id,
            Principal::anonymous(),
            "get_protocol_3usd_reserves",
            encode_args(()).unwrap(),
        )
        .expect("get_protocol_3usd_reserves call failed");
    match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode reserves"),
        WasmResult::Reject(m) => panic!("get_protocol_3usd_reserves rejected: {}", m),
    }
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct PendingThreeUsdRefundView {
    stability_pool: Principal,
    ledger: Principal,
    amount_e8s: u64,
    vault_id: u64,
    retry_count: u8,
    op_nonce: Nat,
}

fn get_pending_3usd_refunds(
    pic: &PocketIc,
    protocol_id: Principal,
) -> Vec<PendingThreeUsdRefundView> {
    let result = pic
        .query_call(
            protocol_id,
            Principal::anonymous(),
            "get_pending_3usd_refunds",
            encode_args(()).unwrap(),
        )
        .expect("get_pending_3usd_refunds call failed");
    match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode pending 3usd refunds"),
        WasmResult::Reject(m) => panic!("get_pending_3usd_refunds rejected: {}", m),
    }
}

/// Drive the backend's self-rescheduling `process_pending_transfer` timer by
/// advancing PocketIC time past its 5s reschedule window and ticking.
fn drain_pending_transfers(pic: &PocketIc) {
    for _ in 0..6 {
        pic.advance_time(Duration::from_secs(6));
        for _ in 0..8 {
            pic.tick();
        }
    }
}

fn fetch_info_logs(pic: &PocketIc, protocol_id: Principal) -> Vec<String> {
    let req = HttpRequest {
        method: "GET".to_string(),
        url: "/logs?priority=info".to_string(),
        headers: vec![],
        body: vec![],
    };
    let result = pic
        .query_call(
            protocol_id,
            Principal::anonymous(),
            "http_request",
            encode_one(req).unwrap(),
        )
        .expect("http_request call failed");
    let response: HttpResponse = match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode http response"),
        WasmResult::Reject(m) => panic!("http_request rejected: {}", m),
    };
    let body = String::from_utf8(response.body).expect("logs body utf8");
    let log: LogWire = serde_json::from_str(&body).expect("parse logs json");
    log.entries.into_iter().map(|e| e.message).collect()
}

// Suppress unused-warning on the management-canister TransformArgs alias —
// keeping the import here documents that the protocol's http_request uses
// the standard ic-cdk transform shape, so any future additions to the test
// mirroring outcalls have a reference.
const _: fn(TransformArgs) -> MgmtHttpResponse = |_| MgmtHttpResponse {
    status: candid::Nat::from(0u64),
    headers: vec![],
    body: vec![],
};

fn call_set_sp_writedown_disabled(
    pic: &PocketIc,
    protocol_id: Principal,
    developer: Principal,
    disabled: bool,
) {
    let result = pic
        .update_call(
            protocol_id,
            developer,
            "set_sp_writedown_disabled",
            encode_args((disabled,)).unwrap(),
        )
        .expect("set_sp_writedown_disabled call failed");
    let parsed: Result<(), ProtocolError> = match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode set_sp_writedown_disabled"),
        WasmResult::Reject(m) => panic!("set_sp_writedown_disabled rejected: {}", m),
    };
    parsed.expect("set_sp_writedown_disabled returned error");
}

fn call_sp_liquidate_with_reserves(
    pic: &PocketIc,
    protocol_id: Principal,
    sp: Principal,
    vault_id: u64,
    icusd_debt: u64,
    three_usd_amount: u64,
    three_usd_ledger: Principal,
) -> Result<StabilityPoolLiquidationResult, ProtocolError> {
    let result = pic
        .update_call(
            protocol_id,
            sp,
            "stability_pool_liquidate_with_reserves",
            encode_args((vault_id, icusd_debt, three_usd_amount, three_usd_ledger))
                .unwrap(),
        )
        .expect("stability_pool_liquidate_with_reserves call failed");
    match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode SP liq result"),
        WasmResult::Reject(m) => panic!("SP liq rejected: {}", m),
    }
}

/// Exercise the receipt-gated reserve absorb with a real Stability Pool and
/// real ledger canisters. The standard ICP ledger serves the payout block
/// immediately; a staged held-then-released block requires a controllable
/// ICRC-3 fixture and remains covered by the SP direct-block unit tests.
#[test]
fn n13_real_sp_promotes_exact_3usd_payout_once() {
    use rumi_protocol_backend::{
        state::ThreeUsdReserveCollateralPayout,
        ThreeUsdReserveIngressV2Status as V2Status,
        ThreeUsdReserveIngressV2StatusView,
    };

    let f = setup_fixture_with_real_sp(ThreePoolKind::Standard);
    let amount = 1_000_000_000u64;

    for (method, args) in [
        (
            "register_stablecoin",
            encode_one(StabilityPoolStablecoinConfig {
                ledger_id: f.three_pool_ledger,
                symbol: "3USD".into(),
                decimals: 8,
                priority: 1,
                is_active: true,
                transfer_fee: Some(0),
                is_lp_token: Some(true),
                underlying_pool: Some(f.three_pool_ledger),
            })
            .unwrap(),
        ),
        (
            "register_collateral",
            encode_one(StabilityPoolCollateralInfo {
                ledger_id: f.icp_ledger,
                symbol: "ICP".into(),
                decimals: 8,
                status: StabilityPoolCollateralStatus::Active,
            })
            .unwrap(),
        ),
    ] {
        match f.pic.update_call(f.sp_principal, f.developer, method, args).unwrap() {
            WasmResult::Reply(_) => {}
            WasmResult::Reject(message) => panic!("SP {method} rejected: {message}"),
        }
    }

    icrc2_approve_call(&f.pic, f.three_pool_ledger, f.test_user, f.sp_principal, amount as u128);
    match f.pic.update_call(
        f.sp_principal,
        f.test_user,
        "deposit",
        encode_args((f.three_pool_ledger, amount)).unwrap(),
    ).unwrap() {
        WasmResult::Reply(_) => {}
        WasmResult::Reject(message) => panic!("SP 3USD deposit rejected: {message}"),
    }
    f.pic.advance_time(Duration::from_secs(1));
    for _ in 0..10 { f.pic.tick(); }
    let before = get_sp_position(&f.pic, f.sp_principal, f.test_user);
    assert_eq!(before.stablecoin_balances.get(&f.three_pool_ledger), Some(&amount));
    assert_eq!(before.collateral_gains.get(&f.icp_ledger).copied().unwrap_or(0), 0);

    xrc_set_rate(&f.pic, f.xrc_id, f.developer, "ICP", "USD", 20_000_000);
    for _ in 0..3 {
        f.pic.advance_time(Duration::from_secs(481));
        for _ in 0..10 { f.pic.tick(); }
    }
    match f.pic.update_call(
        f.protocol_id,
        f.sp_principal,
        "acknowledge_three_usd_reserve_v2_client",
        encode_args(()).unwrap(),
    ).unwrap() {
        WasmResult::Reply(bytes) => decode_one::<Result<(), ProtocolError>>(&bytes)
            .unwrap().expect("SP V2 acknowledgement"),
        WasmResult::Reject(message) => panic!("V2 acknowledgement rejected: {message}"),
    }
    match f.pic.update_call(
        f.protocol_id,
        f.developer,
        "set_three_usd_reserve_ingress_enabled",
        encode_one(true).unwrap(),
    ).unwrap() {
        WasmResult::Reply(bytes) => decode_one::<Result<(), ProtocolError>>(&bytes)
            .unwrap().expect("enable V2 ingress"),
        WasmResult::Reject(message) => panic!("V2 enable rejected: {message}"),
    }

    let notification = || encode_args((vec![LiquidatableVaultInfo {
        vault_id: f.vault_id,
        collateral_type: f.icp_ledger,
        debt_amount: amount,
        collateral_amount: 5_000_000_000,
        recommended_liquidation_amount: amount,
        collateral_price_e8s: 20_000_000,
    }],)).unwrap();
    let first: Vec<PoolLiquidationResult> = match f.pic.update_call(
        f.sp_principal,
        f.protocol_id,
        "notify_liquidatable_vaults",
        notification(),
    ).unwrap() {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode SP liquidation"),
        WasmResult::Reject(message) => panic!("SP liquidation rejected: {message}"),
    };
    assert_eq!(first.len(), 1);
    assert!(!first[0].success, "first call must hold accounting for timer reconciliation");
    assert_eq!(first[0].collateral_gained, 0);
    assert_eq!(get_sp_position(&f.pic, f.sp_principal, f.test_user)
        .collateral_gains.get(&f.icp_ledger).copied().unwrap_or(0), 0,
        "backend commitment alone must not create a claimable collateral gain");
    assert_eq!(get_sp_position(&f.pic, f.sp_principal, f.test_user)
        .stablecoin_balances.get(&f.three_pool_ledger), Some(&amount),
        "3USD book debit remains held until payout settlement");

    // The first update persists the SP intent and returns promptly. Its
    // 30-second recovery timer resumes the exact backend ingress tuple and
    // then verifies the backend's collateral payout candidate against the
    // real ledger's direct ICRC-3 block before promoting gains.
    f.pic.advance_time(Duration::from_secs(31));
    for _ in 0..10 { f.pic.tick(); }

    // The fresh SP's first durable absorb ID is 1. Bind the promoted gain to
    // the backend candidate and the real ledger's net transfer.
    let status: ThreeUsdReserveIngressV2StatusView = match f.pic.query_call(
        f.protocol_id,
        f.sp_principal,
        "get_stability_pool_liquidate_with_reserves_v2_status",
        encode_args((f.vault_id, 1u64)).unwrap(),
    ).unwrap() {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode V2 status"),
        WasmResult::Reject(message) => panic!("V2 status rejected: {message}"),
    };
    assert!(
        matches!(
            status.status,
            V2Status::Absorbed { .. } | V2Status::AbsorbedRefundPending { .. }
        ),
        "backend absorb must be committed, got {:?}",
        status.status
    );
    let payout: Option<ThreeUsdReserveCollateralPayout> = match f.pic.query_call(
        f.protocol_id,
        f.sp_principal,
        "get_stability_pool_liquidate_with_reserves_v2_payout_candidate",
        encode_args((f.vault_id, 1u64)).unwrap(),
    ).unwrap() {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode exact payout candidate"),
        WasmResult::Reject(message) => panic!("payout candidate rejected: {message}"),
    };
    let payout = payout.expect("committed backend payout candidate");
    assert_eq!(payout.ledger, f.icp_ledger);
    assert_eq!(payout.source.owner, f.protocol_id);
    assert_eq!(payout.destination.owner, f.sp_principal);
    assert!(payout.candidate_block_index.is_some());
    assert_eq!(payout.net_e8s + payout.expected_fee_e8s, payout.gross_e8s);

    let after = get_sp_position(&f.pic, f.sp_principal, f.test_user);
    assert_eq!(after.stablecoin_balances.get(&f.three_pool_ledger).copied().unwrap_or(0), 0);
    assert_eq!(after.collateral_gains.get(&f.icp_ledger), Some(&payout.net_e8s));
    assert_eq!(icrc1_balance_of(&f.pic, f.icp_ledger, account(f.sp_principal)), payout.net_e8s as u128);

    let balance_before_replay = icrc1_balance_of(&f.pic, f.three_pool_ledger, account(f.sp_principal));
    match f.pic.update_call(
        f.protocol_id,
        f.sp_principal,
        "stability_pool_liquidate_with_reserves_v2",
        encode_args((f.vault_id, 1u64, amount, amount, f.three_pool_ledger)).unwrap(),
    ).unwrap() {
        WasmResult::Reply(bytes) => {
            let replay: Result<StabilityPoolLiquidationResult, ProtocolError> = decode_one(&bytes).unwrap();
            assert!(replay.expect("exact backend replay").success);
        }
        WasmResult::Reject(message) => panic!("exact backend replay rejected: {message}"),
    }
    assert_eq!(icrc1_balance_of(&f.pic, f.three_pool_ledger, account(f.sp_principal)), balance_before_replay,
        "exact replay must not pull 3USD again");
    let _replay_signal = f.pic.update_call(
        f.sp_principal,
        f.protocol_id,
        "notify_liquidatable_vaults",
        notification(),
    ).expect("replay liquidation notification");
    assert_eq!(get_sp_position(&f.pic, f.sp_principal, f.test_user).collateral_gains, after.collateral_gains,
        "replayed signal cannot credit the receipt-backed gain twice");
}

// ─── Fixture ───

struct Fixture {
    pic: PocketIc,
    protocol_id: Principal,
    xrc_id: Principal,
    icp_ledger: Principal,
    /// Whichever ledger the SP holds 3USD on AND the protocol resolves
    /// `s.three_pool_canister` to. For the happy/refund-success cases this
    /// is the real `rumi_3pool` LP canister; for the refund-failure case
    /// this is the `flaky_ledger`.
    three_pool_ledger: Principal,
    sp_principal: Principal,
    developer: Principal,
    /// 50 ICP collateral, 10 icUSD borrowed. Liquidatable after price drop.
    vault_id: u64,
    /// 3USD pre-minted to the SP. Used to verify refund accounting.
    sp_three_pool_balance: u64,
    test_user: Principal,
}

/// Mode for fixture setup: real 3pool (with ICRC-3) or flaky
/// (no ICRC-3, with failure-injection knobs).
enum ThreePoolKind {
    Standard,
    Flaky,
}

fn setup_fixture(three_pool_kind: ThreePoolKind) -> Fixture {
    setup_fixture_with_backend_wasm(three_pool_kind, protocol_wasm())
}

fn setup_fixture_with_real_sp(three_pool_kind: ThreePoolKind) -> Fixture {
    // The caller must name the freshly built candidate explicitly. A shared
    // Cargo target can contain a successful but stale Wasm from another tree.
    let backend_path = std::env::var_os("RUMI_N13_BACKEND_WASM")
        .expect("set RUMI_N13_BACKEND_WASM to the source-matched backend Wasm");
    let backend_wasm = std::fs::read(&backend_path)
        .unwrap_or_else(|error| panic!("read RUMI_N13_BACKEND_WASM {backend_path:?}: {error}"));
    let path = std::env::var_os("RUMI_N13_SP_WASM")
        .expect("set RUMI_N13_SP_WASM to the source-matched Stability Pool Wasm");
    let sp_wasm = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("read RUMI_N13_SP_WASM {path:?}: {error}"));
    setup_fixture_with_backend_wasm_and_sp(
        three_pool_kind,
        backend_wasm,
        Some(sp_wasm),
    )
}

fn setup_fixture_with_backend_wasm(
    three_pool_kind: ThreePoolKind,
    backend_wasm: Vec<u8>,
) -> Fixture {
    setup_fixture_with_backend_wasm_and_sp(three_pool_kind, backend_wasm, None)
}

fn setup_fixture_with_backend_wasm_and_sp(
    three_pool_kind: ThreePoolKind,
    backend_wasm: Vec<u8>,
    stability_pool_wasm: Option<Vec<u8>>,
) -> Fixture {
    let pool_harness = three_pool_test_harness::deploy_pool_with_liquidity_and_swaps(0);
    let pool_owner = pool_harness.user;
    let three_pool_ledger = pool_harness.three_pool;
    let pic = pool_harness.pic;
    // The harness advances the 3pool canister through its historical snapshot
    // schedule; every fixture needs enough cycles for subsequent LP calls.
    pic.add_cycles(three_pool_ledger, 2_000_000_000_000);

    let test_user = Principal::self_authenticating(b"icc_002_pic_user");
    let developer = Principal::self_authenticating(b"icc_002_pic_developer");
    let sp_principal = if stability_pool_wasm.is_some() {
        let id = pic.create_canister();
        pic.add_cycles(id, 2_000_000_000_000);
        id
    } else {
        Principal::self_authenticating(b"icc_002_pic_sp")
    };

    let protocol_id = pic.create_canister();
    pic.add_cycles(protocol_id, 2_000_000_000_000);
    pic.set_controllers(protocol_id, None, vec![Principal::anonymous(), developer])
        .expect("set_controllers failed");

    // Keep the collateral ledger's minting account separate from the backend:
    // transfers into the minting account are burns, not vault custody.
    let icp_ledger = deploy_icrc1_ledger(
        &pic,
        account(developer),
        10_000,
        vec![(account(test_user), Nat::from(1_000_000_000_000u64))],
        "Internet Computer Protocol",
        "ICP",
        developer,
    );

    let icusd_ledger = deploy_icrc1_ledger(
        &pic,
        account(protocol_id),
        10_000,
        vec![],
        "icUSD",
        "icUSD",
        developer,
    );

    let pool_owner_balance = icrc1_balance_of(&pic, three_pool_ledger, account(pool_owner));
    let sp_three_pool_balance = u64::try_from(pool_owner_balance)
        .expect("3pool LP balance fits test amount type");
    let transfer = icrc_ledger_types::icrc1::transfer::TransferArg {
        from_subaccount: None,
        to: icrc_ledger_types::icrc1::account::Account {
            // The real-SP case deposits through its public ICRC-2 flow below;
            // backend-only cases retain their directly seeded ledger balance.
            owner: if stability_pool_wasm.is_some() { test_user } else { sp_principal },
            subaccount: None,
        },
        amount: Nat::from(sp_three_pool_balance),
        fee: Some(Nat::from(0u64)),
        memo: None,
        created_at_time: None,
    };
    let transferred = pic
        .update_call(
            three_pool_ledger,
            pool_owner,
            "icrc1_transfer",
            encode_args((transfer,)).unwrap(),
        )
        .expect("seed stability pool with real 3pool LP");
    match transferred {
        WasmResult::Reply(bytes) => {
            let result: Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError> =
                decode_one(&bytes).expect("decode 3pool LP transfer");
            result.expect("transfer 3pool LP to SP");
        }
        WasmResult::Reject(message) => panic!("3pool LP transfer rejected: {message}"),
    }
    let three_pool_ledger = match three_pool_kind {
        ThreePoolKind::Standard => three_pool_ledger,
        ThreePoolKind::Flaky => {
            let id = deploy_flaky_ledger(&pic);
            flaky_mint(&pic, id, sp_principal, sp_three_pool_balance as u128);
            id
        }
    };

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
        backend_wasm,
        encode_args((init,)).expect("encode protocol init"),
        None,
    );

    if let Some(wasm) = stability_pool_wasm {
        pic.install_canister(
            sp_principal,
            wasm,
            encode_one(StabilityPoolInitArgs {
                protocol_canister_id: protocol_id,
                authorized_admins: vec![developer],
            })
            .expect("encode Stability Pool init args"),
            None,
        );
    }

    pic.advance_time(Duration::from_secs(1));
    for _ in 0..10 {
        pic.tick();
    }

    // Quiet down dynamic curves so the writedown math stays predictable.
    let _ = pic.update_call(
        protocol_id,
        developer,
        "set_rate_curve_markers",
        encode_args((None::<Principal>, vec![(1.5f64, 1.0f64), (3.0f64, 1.0f64)])).unwrap(),
    );
    let _ = pic.update_call(
        protocol_id,
        developer,
        "set_borrowing_fee",
        encode_args((0.0f64,)).unwrap(),
    );
    let _ = pic.update_call(
        protocol_id,
        developer,
        "set_interest_rate",
        encode_args((icp_ledger, 0.0f64)).unwrap(),
    );

    let _ = pic.update_call(
        protocol_id,
        developer,
        "set_stability_pool_principal",
        encode_args((sp_principal,)).unwrap(),
    );
    let _ = pic.update_call(
        protocol_id,
        developer,
        "set_three_pool_canister",
        encode_args((three_pool_ledger,)).unwrap(),
    );

    // Vault: 50 ICP / 10 icUSD borrowed at $10 ICP → 5000% CR. Drop later
    // when needed, but the kill-switch and happy-path tests don't need a
    // price drop because the SP-writedown path doesn't gate on CR.
    icrc2_approve_call(&pic, icp_ledger, test_user, protocol_id, 5_000_010_000u128);
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
        amount: 1_000_000_000u64, // 10 icUSD borrowed
    };
    let borrow_result = pic
        .update_call(
            protocol_id,
            test_user,
            "borrow_from_vault",
            encode_args((borrow_arg,)).unwrap(),
        )
        .expect("borrow_from_vault failed");
    if let WasmResult::Reply(bytes) = borrow_result {
        let r: Result<SuccessWithFee, ProtocolError> = decode_one(&bytes).expect("decode borrow");
        r.expect("borrow_from_vault returned error");
    } else if let WasmResult::Reject(msg) = borrow_result {
        panic!("borrow rejected: {}", msg);
    }

    // Quiet test: keep the rate stable. Re-issuing a no-op rate write
    // also forces the protocol to re-cache the price so the next call
    // sees fresh state.
    xrc_set_rate(&pic, xrc_id, developer, "ICP", "USD", 1_000_000_000);

    Fixture {
        pic,
        protocol_id,
        xrc_id,
        icp_ledger,
        three_pool_ledger,
        sp_principal,
        developer,
        vault_id,
        sp_three_pool_balance,
        test_user,
    }
}

// ─── Tests ───

/// **Happy-path control.** With the kill switch off and the standard
/// in-tree 3pool LP ledger backing the reserves path, a clean liquidation
/// pulls 3USD into the protocol's reserves subaccount, the writedown
/// commits, `protocol_3usd_reserves` accumulates the pulled amount, and
/// no refund-side log fires. Pins the success accounting that the
/// failure tests below contrast against.
#[test]
fn icc_002_pic_happy_path_no_refund_no_orphan() {
    let f = setup_fixture(ThreePoolKind::Flaky);

    // The backend now checks vault health after pulling reserves. Publish
    // three distinct low-price observations so this vault is liquidatable.
    xrc_set_rate(&f.pic, f.xrc_id, f.developer, "ICP", "USD", 20_000_000);
    for _ in 0..3 {
        f.pic.advance_time(Duration::from_secs(481));
        for _ in 0..10 {
            f.pic.tick();
        }
    }

    let icusd_debt: u64 = 500_000_000; // 5 icUSD
    let three_usd_amount: u64 = 500_000_000; // 1:1 with virtual price ≈ 1

    icrc2_approve_call(
        &f.pic,
        f.three_pool_ledger,
        f.sp_principal,
        f.protocol_id,
        (three_usd_amount as u128) * 2,
    );

    let sp_balance_before = icrc1_balance_of(
        &f.pic,
        f.three_pool_ledger,
        account(f.sp_principal),
    );
    assert_eq!(
        sp_balance_before, f.sp_three_pool_balance as u128,
        "SP starts with full 3USD balance"
    );

    let reserves_before = get_protocol_3usd_reserves(&f.pic, f.protocol_id);
    assert_eq!(reserves_before, 0, "no reserves before any SP liquidation");

    let liq = call_sp_liquidate_with_reserves(
        &f.pic,
        f.protocol_id,
        f.sp_principal,
        f.vault_id,
        icusd_debt,
        three_usd_amount,
        f.three_pool_ledger,
    )
    .expect("happy path must succeed");

    assert!(liq.success, "liquidation must report success");
    assert_eq!(liq.vault_id, f.vault_id);
    assert_eq!(liq.liquidated_debt, icusd_debt);
    assert!(
        liq.collateral_received > 0,
        "collateral must be released to the SP"
    );

    let sp_balance_after = icrc1_balance_of(
        &f.pic,
        f.three_pool_ledger,
        account(f.sp_principal),
    );
    assert_eq!(
        sp_balance_after,
        sp_balance_before - three_usd_amount as u128,
        "SP balance must drop by exactly the pulled amount on the success path (no refund happened)"
    );

    let reserves_after = get_protocol_3usd_reserves(&f.pic, f.protocol_id);
    assert_eq!(
        reserves_after, three_usd_amount,
        "protocol_3usd_reserves must accumulate the pulled amount on success"
    );

    // The in-tree 3pool LP ledger records requested subaccounts in ICRC-3 but
    // aggregates balances by owner. This legacy path checks the SP balance
    // delta and reserve counter; the P08 proof-bound case checks the exact
    // transfer tuple. Do not infer physical subaccount isolation here.

    // Sanity: no refund log on the happy path.
    let logs = fetch_info_logs(&f.pic, f.protocol_id);
    assert!(
        !logs
            .iter()
            .any(|m| m.contains("after liquidation rollback")),
        "happy path must not emit the Wave-4 refund log; saw logs: {:?}",
        logs
    );
}

/// P08-02 integration fence: a proof-verified V2 ingress pulls from the SP's
/// default account into the backend's default account, and an exact replay
/// returns the committed result without another pull. The real 3pool canister
/// supplies ICRC-2 and ICRC-3 behavior; the backend sees the registered SP
/// principal as caller, matching its production authorization boundary.
#[test]
fn p08_02_v2_ingress_is_default_account_proof_bound_and_replay_safe() {
    use rumi_protocol_backend::{
        state::PriceSource, AddCollateralArg, StabilityPoolLiquidationResult,
        ThreeUsdReserveIngressV2Status as Status, ThreeUsdReserveIngressV2StatusView,
    };

    let f = setup_fixture(ThreePoolKind::Standard);
    let debt_e8s = 500_000_000u64;
    let amount_e8s = 500_000_000u64;
    let disabled_absorb_id = 40u64;
    let absorb_id = 41u64;

    let enabled: bool = match f
        .pic
        .query_call(
            f.protocol_id,
            Principal::anonymous(),
            "get_three_usd_reserve_ingress_enabled",
            encode_args(()).unwrap(),
        )
        .expect("query V2 gate")
    {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode V2 gate"),
        WasmResult::Reject(message) => panic!("V2 gate query rejected: {message}"),
    };
    assert!(!enabled, "V2 reserve ingress must be default-off");

    // The configured 3pool ledger cannot enter the generic collateral payout
    // path. This must reject before querying ledger metadata.
    let overlap_registration: Result<(), ProtocolError> = match f
        .pic
        .update_call(
            f.protocol_id,
            f.developer,
            "add_collateral_token",
            encode_one(AddCollateralArg {
                ledger_canister_id: f.three_pool_ledger,
                price_source: PriceSource::Xrc {
                    base_asset: "3USD".into(),
                    base_asset_class: Default::default(),
                    quote_asset: "USD".into(),
                    quote_asset_class: Default::default(),
                },
                liquidation_ratio: 1.33,
                borrow_threshold_ratio: 1.5,
                liquidation_bonus: 1.15,
                borrowing_fee: 0.0,
                debt_ceiling: u64::MAX,
                min_vault_debt: 0,
                interest_rate_apr: 0.0,
                min_collateral_deposit: 0,
                display_color: None,
                redemption_fee_floor: None,
                redemption_fee_ceiling: None,
                redemption_tier: None,
            })
            .unwrap(),
        )
        .expect("overlap registration call")
    {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode overlap registration"),
        WasmResult::Reject(message) => panic!("overlap registration rejected: {message}"),
    };
    assert!(
        format!("{overlap_registration:?}").contains("configured 3pool ledger cannot be registered as collateral"),
        "configured 3pool ledger registration must fail closed: {overlap_registration:?}"
    );

    // Conversely, an already registered collateral ledger cannot replace the
    // configured 3pool, and the rejected transition leaves the old value.
    let overlap_setter: Result<(), ProtocolError> = match f
        .pic
        .update_call(
            f.protocol_id,
            f.developer,
            "set_three_pool_canister",
            encode_one(f.icp_ledger).unwrap(),
        )
        .expect("overlap setter call")
    {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode overlap setter"),
        WasmResult::Reject(message) => panic!("overlap setter rejected: {message}"),
    };
    assert!(
        format!("{overlap_setter:?}").contains("registered collateral ledger cannot be configured as 3pool"),
        "registered collateral cannot replace 3pool: {overlap_setter:?}"
    );
    let configured_pool: Option<Principal> = match f
        .pic
        .query_call(
            f.protocol_id,
            Principal::anonymous(),
            "get_three_pool_canister",
            encode_args(()).unwrap(),
        )
        .expect("query configured 3pool after rejected setter")
    {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode configured 3pool"),
        WasmResult::Reject(message) => panic!("configured 3pool query rejected: {message}"),
    };
    assert_eq!(configured_pool, Some(f.three_pool_ledger));

    // The disabled endpoint must reject before a transfer, even for the
    // otherwise valid registered Stability Pool identity.
    let disabled_call = f
        .pic
        .update_call(
            f.protocol_id,
            f.sp_principal,
            "stability_pool_liquidate_with_reserves_v2",
            encode_args((
                f.vault_id,
                disabled_absorb_id,
                debt_e8s,
                amount_e8s,
                f.three_pool_ledger,
            ))
            .unwrap(),
        )
        .expect("disabled V2 call transport");
    let disabled_result: Result<StabilityPoolLiquidationResult, ProtocolError> = match disabled_call {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode disabled V2 result"),
        WasmResult::Reject(message) => panic!("disabled V2 call rejected: {message}"),
    };
    assert!(disabled_result.is_err(), "disabled V2 ingress must not run");
    assert_eq!(
        icrc1_balance_of(&f.pic, f.three_pool_ledger, account(f.sp_principal)),
        f.sp_three_pool_balance as u128,
        "disabled V2 ingress must not pull from the Stability Pool"
    );
    let disabled_status: ThreeUsdReserveIngressV2StatusView = match f
        .pic
        .query_call(
            f.protocol_id,
            f.sp_principal,
            "get_stability_pool_liquidate_with_reserves_v2_status",
            encode_args((f.vault_id, disabled_absorb_id)).unwrap(),
        )
        .expect("query disabled V2 terminal status")
    {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode disabled terminal status"),
        WasmResult::Reject(message) => panic!("disabled status rejected: {message}"),
    };
    assert!(matches!(
        disabled_status.status,
        Status::PreTransferRejected { .. }
    ), "disabled gate should be a durable typed no-pull terminal");

    let acknowledged: Result<(), ProtocolError> = match f
        .pic
        .update_call(
            f.protocol_id,
            f.sp_principal,
            "acknowledge_three_usd_reserve_v2_client",
            encode_args(()).unwrap(),
        )
        .expect("acknowledge V2 client interface")
    {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode V2 acknowledgement"),
        WasmResult::Reject(message) => panic!("V2 acknowledgement rejected: {message}"),
    };
    acknowledged.expect("registered Stability Pool may acknowledge V2 in the isolated test canister");

    let enabled_result: Result<(), ProtocolError> = match f
        .pic
        .update_call(
            f.protocol_id,
            f.developer,
            "set_three_usd_reserve_ingress_enabled",
            encode_one(true).unwrap(),
        )
        .expect("enable V2 ingress")
    {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode enable result"),
        WasmResult::Reject(message) => panic!("enable V2 rejected: {message}"),
    };
    enabled_result.expect("developer may enable V2 in the isolated test canister");

    // This endpoint now re-checks vault health after the reserve transfer.
    // Lower the mock ICP/USD quote and let the normal XRC timer publish it so
    // the fixture is actually liquidatable (50 ICP * $0.20 / $10 debt = 100%).
    xrc_set_rate(&f.pic, f.xrc_id, f.developer, "ICP", "USD", 20_000_000);
    // ICP outliers require three distinct, source-timestamped observations.
    for _ in 0..3 {
        f.pic.advance_time(Duration::from_secs(481));
        for _ in 0..10 {
            f.pic.tick();
        }
    }
    #[derive(CandidType, Deserialize)]
    struct PriceView {
        price_e8s: u128,
    }
    let cached_price: PriceView = match f
        .pic
        .query_call(
            f.protocol_id,
            Principal::anonymous(),
            "get_icp_usd_price_e8s",
            encode_args(()).unwrap(),
        )
        .expect("query lowered ICP/USD price")
    {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode lowered ICP/USD price"),
        WasmResult::Reject(message) => panic!("ICP/USD price query rejected: {message}"),
    };
    assert_eq!(cached_price.price_e8s, 20_000_000, "mock XRC price must be published before liquidation");
    assert!(50.0 * 0.2 / 10.0 < 1.33, "test vault must be below the liquidation threshold");

    let pool_cycles_before_approve = f
        .pic
        .canister_status(f.three_pool_ledger, Some(Principal::anonymous()))
        .expect("read isolated 3pool cycles before approval")
        .cycles;
    eprintln!("3pool cycles before P08 approval: {}", pool_cycles_before_approve.0);
    // This focused proof performs several additional ledger calls after the
    // shared harness's initial setup. Keep the test-only cycle budget separate
    // from production policy so exhaustion cannot mask the proof assertions.
    f.pic.add_cycles(f.three_pool_ledger, 2_000_000_000_000);

    icrc2_approve_call(
        &f.pic,
        f.three_pool_ledger,
        f.sp_principal,
        f.protocol_id,
        (amount_e8s as u128) * 2,
    );

    let fee: u64 = match f
        .pic
        .query_call(
            f.three_pool_ledger,
            Principal::anonymous(),
            "icrc1_fee",
            encode_args(()).unwrap(),
        )
        .expect("query 3pool ledger fee")
    {
        WasmResult::Reply(bytes) => decode_one::<Nat>(&bytes)
            .expect("decode 3pool fee")
            .0
            .try_into()
            .expect("3pool fee fits u64"),
        WasmResult::Reject(message) => panic!("3pool fee query rejected: {message}"),
    };
    let sp_before = icrc1_balance_of(
        &f.pic,
        f.three_pool_ledger,
        account(f.sp_principal),
    );
    let backend_default_before = icrc1_balance_of(
        &f.pic,
        f.three_pool_ledger,
        account(f.protocol_id),
    );
    let call_v2 = || {
        f.pic
            .update_call(
                f.protocol_id,
                f.sp_principal,
                "stability_pool_liquidate_with_reserves_v2",
                encode_args((
                    f.vault_id,
                    absorb_id,
                    debt_e8s,
                    amount_e8s,
                    f.three_pool_ledger,
                ))
                .unwrap(),
            )
            .expect("V2 reserve ingress update")
    };
    let result: Result<StabilityPoolLiquidationResult, ProtocolError> = match call_v2() {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode V2 result"),
        WasmResult::Reject(message) => panic!("V2 reserve ingress rejected: {message}"),
    };
    let result = result.expect("V2 ingress and exact ICRC-3 proof should succeed");
    assert!(result.success);
    assert_eq!(result.vault_id, f.vault_id);
    assert_eq!(result.liquidated_debt, debt_e8s);

    let status: ThreeUsdReserveIngressV2StatusView = match f
        .pic
        .query_call(
            f.protocol_id,
            f.sp_principal,
            "get_stability_pool_liquidate_with_reserves_v2_status",
            encode_args((f.vault_id, absorb_id)).unwrap(),
        )
        .expect("query V2 durable status")
    {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode V2 status"),
        WasmResult::Reject(message) => panic!("V2 status rejected: {message}"),
    };
    assert_eq!(status.stability_pool, f.sp_principal);
    assert_eq!(status.vault_id, f.vault_id);
    assert_eq!(status.absorb_id, absorb_id);
    let (transfer_block_index, ingress_fee) = match status.status {
        Status::Absorbed {
            transfer_block_index,
            ingress_fee_e8s,
            result: status_result,
            proportional_refund,
        } => {
            assert_eq!(status_result, result);
            assert!(proportional_refund.is_none(), "full debt coverage needs no refund");
            (transfer_block_index, ingress_fee_e8s)
        }
        other => panic!("expected proof-verified absorbed status, got {other:?}"),
    };
    assert_eq!(ingress_fee, fee, "status fee must match the verified ICRC-3 transfer fee");
    assert_eq!(result.block_index, transfer_block_index);
    assert_eq!(
        icrc1_balance_of(&f.pic, f.three_pool_ledger, account(f.sp_principal)),
        sp_before - amount_e8s as u128 - ingress_fee as u128,
        "ICRC-2 transferFrom charges amount plus its verified fee to the SP default account"
    );
    assert_eq!(
        icrc1_balance_of(&f.pic, f.three_pool_ledger, account(f.protocol_id)),
        backend_default_before + amount_e8s as u128,
        "V2 custody destination must be the backend default account"
    );
    // The in-tree 3pool ledger intentionally keys balance by owner principal
    // and preserves subaccounts only in ICRC-3 blocks. The proof verified above
    // already asserts the exact default-account destination, so a balance
    // query cannot distinguish this from the legacy reserves subaccount.

    let replay: Result<StabilityPoolLiquidationResult, ProtocolError> = match call_v2() {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode V2 replay"),
        WasmResult::Reject(message) => panic!("V2 replay rejected: {message}"),
    };
    assert_eq!(replay.expect("exact replay returns committed result"), result);
    assert_eq!(
        icrc1_balance_of(&f.pic, f.three_pool_ledger, account(f.sp_principal)),
        sp_before - amount_e8s as u128 - ingress_fee as u128,
        "same absorb ID replay must not pull a second time"
    );
    let changed_request: Result<StabilityPoolLiquidationResult, ProtocolError> = match f
        .pic
        .update_call(
            f.protocol_id,
            f.sp_principal,
            "stability_pool_liquidate_with_reserves_v2",
            encode_args((
                f.vault_id,
                absorb_id,
                debt_e8s,
                amount_e8s + 1,
                f.three_pool_ledger,
            ))
            .unwrap(),
        )
        .expect("changed identity request transport")
    {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode changed request result"),
        WasmResult::Reject(message) => panic!("changed request rejected: {message}"),
    };
    assert!(changed_request.is_err(), "an absorb ID cannot be rebound to new arguments");
    assert_eq!(
        icrc1_balance_of(&f.pic, f.three_pool_ledger, account(f.sp_principal)),
        sp_before - amount_e8s as u128 - ingress_fee as u128,
        "rebound ID must not transfer additional LP tokens"
    );
}

/// CL-07 value/principal preflight: the real 3pool principal is both the LP
/// ledger and the source of virtual price. Wrong-principal and under-valued
/// pulls reject before any SP balance change; the happy-path control above
/// confirms a correctly covered amount succeeds.
#[test]
fn cl07_rejects_wrong_ledger_and_under_valued_reserves_before_pull() {
    let f = setup_fixture(ThreePoolKind::Standard);
    let debt = 500_000_000u64;
    let balance_before = icrc1_balance_of(
        &f.pic,
        f.three_pool_ledger,
        account(f.sp_principal),
    );

    let wrong_ledger = call_sp_liquidate_with_reserves(
        &f.pic,
        f.protocol_id,
        f.sp_principal,
        f.vault_id,
        debt,
        debt,
        Principal::anonymous(),
    )
    .expect_err("ledger principal must match the configured pool");
    assert!(format!("{wrong_ledger:?}").contains("does not match the configured"));

    let under_valued = call_sp_liquidate_with_reserves(
        &f.pic,
        f.protocol_id,
        f.sp_principal,
        f.vault_id,
        debt,
        1,
        f.three_pool_ledger,
    )
    .expect_err("1 e8 LP unit cannot cover 5 icUSD");
    assert!(format!("{under_valued:?}").contains("below debt covered"));

    assert_eq!(
        icrc1_balance_of(&f.pic, f.three_pool_ledger, account(f.sp_principal)),
        balance_before,
        "preflight failures must not pull LP from the SP"
    );
}

/// **Refund happens.** Arm `set_sp_writedown_disabled(true)` so the
/// writedown rejects with `TemporarilyUnavailable` AFTER the entry-point
/// pre-validation has passed and the 3USD pull has landed. The Wave-4 `Err`
/// arm journals the refund before dispatch. The worker restores the SP
/// balance and leaves no orphan in `protocol_3usd_reserves`.
#[test]
fn icc_002_pic_writedown_failure_refunds_3usd_to_sp() {
    let f = setup_fixture(ThreePoolKind::Flaky);

    let icusd_debt: u64 = 500_000_000;
    let three_usd_amount: u64 = 500_000_000;

    icrc2_approve_call(
        &f.pic,
        f.three_pool_ledger,
        f.sp_principal,
        f.protocol_id,
        (three_usd_amount as u128) * 2,
    );

    // Engage the kill switch BEFORE the SP call. The entry point's
    // pre-pull validation does not check `sp_writedown_disabled`; only
    // `liquidate_vault_debt_already_burned` does. So the pull lands and the
    // post-pull writedown rejects, exercising the Wave-4 refund arm.
    call_set_sp_writedown_disabled(&f.pic, f.protocol_id, f.developer, true);

    let sp_balance_before = icrc1_balance_of(
        &f.pic,
        f.three_pool_ledger,
        account(f.sp_principal),
    );

    let err = call_sp_liquidate_with_reserves(
        &f.pic,
        f.protocol_id,
        f.sp_principal,
        f.vault_id,
        icusd_debt,
        three_usd_amount,
        f.three_pool_ledger,
    )
    .expect_err("writedown must reject when kill switch is engaged");

    // The protocol surfaces the writedown's TemporarilyUnavailable error
    // unchanged after the refund. (The refund's success/failure does NOT
    // alter the returned error — that is the Wave-4 contract.)
    assert!(
        matches!(err, ProtocolError::TemporarilyUnavailable(_)),
        "expected TemporarilyUnavailable from kill switch; got {:?}",
        err
    );

    let pending = get_pending_3usd_refunds(&f.pic, f.protocol_id);
    assert_eq!(pending.len(), 1, "the full refund must be journaled before dispatch");
    assert_eq!(pending[0].amount_e8s, three_usd_amount);
    drain_pending_transfers(&f.pic);

    let sp_balance_after = icrc1_balance_of(
        &f.pic,
        f.three_pool_ledger,
        account(f.sp_principal),
    );
    assert_eq!(
        sp_balance_after, sp_balance_before,
        "SP balance MUST be restored to pre-call value (zero-fee ledger), \
         confirming the refund landed; saw before={} after={}",
        sp_balance_before, sp_balance_after
    );

    let reserves_after = get_protocol_3usd_reserves(&f.pic, f.protocol_id);
    assert_eq!(
        reserves_after, 0,
        "protocol_3usd_reserves MUST stay at zero — the writedown rejected \
         BEFORE the state mutation that increments it"
    );

    assert!(get_pending_3usd_refunds(&f.pic, f.protocol_id).is_empty(),
        "verified immediate refund must leave no unresolved liability");
}

/// **Fresh refund failure is durable, not stranded.** Same setup as the prior test
/// but the 3USD ledger is `flaky_ledger` with `set_fail_transfers(true)`. The
/// pull (`icrc2_transfer_from`) is unaffected and lands; the kill-switch reject
/// fires the refund arm; the refund (`icrc1_transfer`) fails.
///
/// Pre-fix, that failure only logged CRITICAL and left the 3USD stranded in the
/// protocol's reserves subaccount, dropping the SP's live balance below its
/// tracked aggregate and blocking every non-sole-holder withdrawal. This
/// current-source scenario creates a fresh row with retry provenance, verifies
/// it remains eligible for bounded automatic retry, and confirms the queue
/// drains when the ledger recovers.
#[test]
fn icc_002_pic_refund_failure_enqueues_durable_retry_and_heals() {
    let f = setup_fixture(ThreePoolKind::Flaky);

    let icusd_debt: u64 = 500_000_000;
    let three_usd_amount: u64 = 500_000_000;

    icrc2_approve_call(
        &f.pic,
        f.three_pool_ledger,
        f.sp_principal,
        f.protocol_id,
        (three_usd_amount as u128) * 2,
    );

    call_set_sp_writedown_disabled(&f.pic, f.protocol_id, f.developer, true);

    // Arm the flaky ledger to fail icrc1_transfer (used by the refund) but
    // NOT icrc2_transfer_from (used by the initial pull). Order matters:
    // arm AFTER the approve above, since approve uses icrc2_approve which
    // is also unaffected by this knob.
    flaky_set_fail_transfers(&f.pic, f.three_pool_ledger, true);

    let sp_balance_before = icrc1_balance_of(
        &f.pic,
        f.three_pool_ledger,
        account(f.sp_principal),
    );

    let err = call_sp_liquidate_with_reserves(
        &f.pic,
        f.protocol_id,
        f.sp_principal,
        f.vault_id,
        icusd_debt,
        three_usd_amount,
        f.three_pool_ledger,
    )
    .expect_err("writedown must reject when kill switch is engaged");

    assert!(
        matches!(err, ProtocolError::TemporarilyUnavailable(_)),
        "expected TemporarilyUnavailable from kill switch; got {:?}",
        err
    );

    // Immediately after the failed refund: the SP is still short (tokens are in
    // reserves) BUT the refund is now durably queued rather than lost.
    let sp_balance_after_fail = icrc1_balance_of(
        &f.pic,
        f.three_pool_ledger,
        account(f.sp_principal),
    );
    assert_eq!(
        sp_balance_after_fail,
        sp_balance_before - three_usd_amount as u128,
        "SP balance is temporarily short after the refund failed; \
         saw before={} after={}",
        sp_balance_before, sp_balance_after_fail
    );

    let pending = get_pending_3usd_refunds(&f.pic, f.protocol_id);
    assert_eq!(
        pending.len(),
        1,
        "the failed refund MUST be persisted to pending_3usd_refunds for retry, \
         not stranded; saw {:?}",
        pending
    );
    assert_eq!(pending[0].stability_pool, f.sp_principal);
    assert_eq!(pending[0].ledger, f.three_pool_ledger);
    assert_eq!(pending[0].vault_id, f.vault_id);
    assert_eq!(
        pending[0].amount_e8s, three_usd_amount,
        "queued refund amount must equal the stranded excess (3USD fee is 0)"
    );

    // Now the ledger recovers. Draining the timer must settle the refund.
    flaky_set_fail_transfers(&f.pic, f.three_pool_ledger, false);
    drain_pending_transfers(&f.pic);

    let pending_after = get_pending_3usd_refunds(&f.pic, f.protocol_id);
    assert!(
        pending_after.is_empty(),
        "the durable refund queue MUST drain once the ledger recovers; saw {:?}",
        pending_after
    );

    let sp_balance_healed = icrc1_balance_of(
        &f.pic,
        f.three_pool_ledger,
        account(f.sp_principal),
    );
    assert_eq!(
        sp_balance_healed, sp_balance_before,
        "the SP MUST be made whole after the queue drains (3USD fee is 0); \
         saw before={} healed={}",
        sp_balance_before, sp_balance_healed
    );

    let reserves_subacct_balance = icrc1_balance_of(
        &f.pic,
        f.three_pool_ledger,
        Account {
            owner: f.protocol_id,
            subaccount: Some(protocol_3usd_reserves_subaccount()),
        },
    );
    assert_eq!(
        reserves_subacct_balance, 0,
        "the reserves subaccount MUST be drained back to the SP after retry; saw {}",
        reserves_subacct_balance
    );

}

/// A refund row created with the pre-P08 backend source at 9d5f359e keeps its
/// legacy hashed-reserve source and exact identity when upgraded to P08, but it
/// is held because the old snapshot cannot prove no earlier refund dispatch
/// committed. The exact candidate-block reconciliation endpoint is the only
/// way to clear this predecessor liability.
/// Set `RUMI_P08_PRE_P08_BACKEND_WASM` to that pinned source-built artifact.
#[test]
#[ignore = "requires the pre-P08 backend Wasm built from source 9d5f359e"]
fn p08_upgrade_preserves_parent_legacy_refund_identity_and_holds_without_receipt() {
    let parent_path = std::env::var("RUMI_P08_PRE_P08_BACKEND_WASM")
        .expect("set RUMI_P08_PRE_P08_BACKEND_WASM to the source-9d5f359e backend Wasm");
    let parent_wasm = std::fs::read(parent_path).expect("read parent backend Wasm");
    let parent_sha256 = format!("{:x}", Sha256::digest(&parent_wasm));
    assert_eq!(parent_sha256, "1d5f9a5b7980ceab1f2ecc19bfc3ce8900b33efd0428d5bc3e11b6affc9519eb",
        "unexpected parent backend artifact");

    let f = setup_fixture_with_backend_wasm(ThreePoolKind::Flaky, parent_wasm);
    let icusd_debt = 500_000_000u64;
    let three_usd_amount = 500_000_000u64;
    icrc2_approve_call(&f.pic, f.three_pool_ledger, f.sp_principal, f.protocol_id,
        (three_usd_amount as u128) * 2);
    call_set_sp_writedown_disabled(&f.pic, f.protocol_id, f.developer, true);
    flaky_set_fail_transfers(&f.pic, f.three_pool_ledger, true);
    let sp_balance_before = icrc1_balance_of(&f.pic, f.three_pool_ledger, account(f.sp_principal));

    let err = call_sp_liquidate_with_reserves(&f.pic, f.protocol_id, f.sp_principal, f.vault_id,
        icusd_debt, three_usd_amount, f.three_pool_ledger)
        .expect_err("parent writedown must reject with the kill switch engaged");
    assert!(matches!(err, ProtocolError::TemporarilyUnavailable(_)));

    let before = get_pending_3usd_refunds(&f.pic, f.protocol_id);
    assert_eq!(before.len(), 1, "parent should persist one legacy refund");
    let row = &before[0];
    assert_eq!(row.stability_pool, f.sp_principal);
    assert_eq!(row.ledger, f.three_pool_ledger);
    assert_eq!(row.vault_id, f.vault_id);
    assert_eq!(row.amount_e8s, three_usd_amount);
    assert_ne!(row.op_nonce, Nat::from(0u8));
    let reserve_account = Account { owner: f.protocol_id, subaccount: Some(protocol_3usd_reserves_subaccount()) };
    assert_eq!(icrc1_balance_of(&f.pic, f.three_pool_ledger, reserve_account.clone()),
        three_usd_amount as u128, "legacy refund remains in hashed reserve account before upgrade");
    let reserve_counter_before = get_protocol_3usd_reserves(&f.pic, f.protocol_id);

    let upgrade = ProtocolArgVariant::Upgrade(ProtocolUpgradeArg {
        mode: None,
        description: Some("P08 populated refund migration test".to_string()),
    });
    f.pic.upgrade_canister(f.protocol_id, protocol_wasm(), encode_args((upgrade,)).unwrap(), None)
        .expect("upgrade parent backend to P08");

    // Run the zero-delay post-upgrade timer explicitly. The old snapshot has no
    // fresh-row provenance marker, so P08 must retain it without dispatching.
    f.pic.tick();

    let after = get_pending_3usd_refunds(&f.pic, f.protocol_id);
    assert_eq!(after.len(), 1, "P08 must retain the pending refund");
    assert_eq!(after[0].stability_pool, row.stability_pool);
    assert_eq!(after[0].ledger, row.ledger);
    assert_eq!(after[0].amount_e8s, row.amount_e8s);
    assert_eq!(after[0].vault_id, row.vault_id);
    assert_eq!(after[0].retry_count, row.retry_count,
        "predecessor row must remain held; upgrade must not dispatch it automatically");
    assert_eq!(after[0].op_nonce, row.op_nonce, "retry must reuse exact nonce");
    assert_eq!(get_protocol_3usd_reserves(&f.pic, f.protocol_id), reserve_counter_before,
        "upgrade must not change reserve accounting");
    assert_eq!(icrc1_balance_of(&f.pic, f.three_pool_ledger, reserve_account.clone()),
        three_usd_amount as u128, "legacy refund source remains the hashed reserve account");

    // Recovering the ledger does not turn an ambiguous predecessor row into a
    // retryable fresh row. Drain timers and prove the identity and balances
    // stay unchanged until direct receipt reconciliation supplies proof.
    flaky_set_fail_transfers(&f.pic, f.three_pool_ledger, false);
    drain_pending_transfers(&f.pic);
    let rearm = f.pic.update_call(
        f.protocol_id,
        f.developer,
        "rearm_unsent_legacy_three_usd_refund",
        encode_one(row.op_nonce.clone()).unwrap(),
    ).expect("rearm endpoint call executes");
    let rearm_result: Result<(), ProtocolError> = match rearm {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode rearm result"),
        WasmResult::Reject(message) => panic!("rearm endpoint rejected: {message}"),
    };
    assert!(rearm_result.is_err(), "old snapshot row must not use fresh-row rearm");

    let still_held = get_pending_3usd_refunds(&f.pic, f.protocol_id);
    assert_eq!(still_held.len(), 1, "predecessor liability remains durable");
    assert_eq!(still_held[0].retry_count, row.retry_count);
    assert_eq!(still_held[0].op_nonce, row.op_nonce);
    assert_eq!(icrc1_balance_of(&f.pic, f.three_pool_ledger, account(f.sp_principal)),
        sp_balance_before - three_usd_amount as u128,
        "held predecessor refund must not credit the SP without exact receipt proof");
    assert_eq!(icrc1_balance_of(&f.pic, f.three_pool_ledger, reserve_account),
        three_usd_amount as u128,
        "held predecessor principal remains in the legacy hashed reserve account");
}
