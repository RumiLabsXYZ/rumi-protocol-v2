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
//!      `TemporarilyUnavailable` before any state mutation. The Wave-4
//!      `Err` arm fires the refund: the SP's 3USD balance is restored,
//!      `protocol_3usd_reserves` stays at zero, and the INFO log carries
//!      a `refunded ... after liquidation rollback` line keyed to the vault.
//!
//!   3. `icc_002_pic_refund_failure_logs_critical_and_strands_3usd` — the
//!      refund-of-refund failure. Same setup as #2, but the 3USD ledger
//!      is `flaky_ledger` with `set_fail_transfers(true)`. The pull
//!      (`icrc2_transfer_from`) is unaffected by that knob and lands; the
//!      writedown rejects (kill switch); the refund (`icrc1_transfer`)
//!      fails. Asserts:
//!        * the protocol surfaces the original writedown error,
//!        * `protocol_3usd_reserves` still stays at zero (the writedown
//!          never reached its mutation),
//!        * the SP did NOT get its 3USD back (the refund actually failed),
//!        * an INFO log entry with `CRITICAL: refund of` fires so the
//!          on-call operator sees the stranding.
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
//! Fixtures are lifted from `audit_pocs_liq_004_icrc3_burn_proof_pic.rs`
//! and `audit_pocs_icrc_idempotent.rs` (for the flaky-ledger Candid
//! mirrors).

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use ic_cdk::api::management_canister::http_request::{
    HttpResponse as MgmtHttpResponse, TransformArgs,
};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime};

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

// ─── WASM fixtures ───

fn icrc1_ledger_wasm() -> Vec<u8> {
    include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec()
}

fn protocol_wasm() -> Vec<u8> {
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

// ─── Fixture ───

struct Fixture {
    pic: PocketIc,
    protocol_id: Principal,
    /// Whichever ledger the SP holds 3USD on AND the protocol resolves
    /// `s.three_pool_canister` to. For the happy/refund-success cases this
    /// is a standard zero-fee ic-icrc1-ledger; for the refund-failure case
    /// this is the `flaky_ledger`.
    three_pool_ledger: Principal,
    sp_principal: Principal,
    developer: Principal,
    /// 50 ICP collateral, 10 icUSD borrowed. Liquidatable after price drop.
    vault_id: u64,
    /// 3USD pre-minted to the SP. Used to verify refund accounting.
    sp_three_pool_balance: u64,
}

/// Mode for fixture setup: standard ic-icrc1-ledger (with ICRC-3) or flaky
/// (no ICRC-3, with failure-injection knobs).
enum ThreePoolKind {
    Standard,
    Flaky,
}

fn setup_fixture(three_pool_kind: ThreePoolKind) -> Fixture {
    let pic = PocketIcBuilder::new().with_nns_subnet().build();

    let test_user = Principal::self_authenticating(b"icc_002_pic_user");
    let developer = Principal::self_authenticating(b"icc_002_pic_developer");
    let sp_principal = Principal::self_authenticating(b"icc_002_pic_sp");

    let protocol_id = pic.create_canister();
    pic.add_cycles(protocol_id, 2_000_000_000_000);
    pic.set_controllers(protocol_id, None, vec![Principal::anonymous(), developer])
        .expect("set_controllers failed");

    let icp_ledger = deploy_icrc1_ledger(
        &pic,
        account(protocol_id),
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

    let sp_three_pool_balance = 1_000_000_000_000u64;
    let three_pool_ledger = match three_pool_kind {
        ThreePoolKind::Standard => deploy_icrc1_ledger(
            &pic,
            account(protocol_id),
            0, // zero fee for clean refund accounting
            vec![(account(sp_principal), Nat::from(sp_three_pool_balance))],
            "Rumi 3pool LP",
            "3USD",
            developer,
        ),
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
        protocol_wasm(),
        encode_args((init,)).expect("encode protocol init"),
        None,
    );

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
    icrc2_approve_call(&pic, icp_ledger, test_user, protocol_id, 5_000_000_000u128);
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
        three_pool_ledger,
        sp_principal,
        developer,
        vault_id,
        sp_three_pool_balance,
    }
}

// ─── Tests ───

/// **Happy-path control.** With the kill switch off and the standard
/// ic-icrc1-ledger backing the 3pool path, a clean reserves liquidation
/// pulls 3USD into the protocol's reserves subaccount, the writedown
/// commits, `protocol_3usd_reserves` accumulates the pulled amount, and
/// no refund-side log fires. Pins the success accounting that the
/// failure tests below contrast against.
#[test]
fn icc_002_pic_happy_path_no_refund_no_orphan() {
    let f = setup_fixture(ThreePoolKind::Standard);

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

    // Belt-and-suspenders: the protocol's reserves subaccount on the 3pool
    // ledger must hold the pulled tokens. (Zero-fee ledger so amounts match.)
    let reserves_subacct_balance = icrc1_balance_of(
        &f.pic,
        f.three_pool_ledger,
        Account {
            owner: f.protocol_id,
            subaccount: Some(protocol_3usd_reserves_subaccount()),
        },
    );
    assert_eq!(
        reserves_subacct_balance, three_usd_amount as u128,
        "protocol reserves subaccount must hold the pulled 3USD"
    );

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

/// **Refund happens.** Arm `set_sp_writedown_disabled(true)` so the
/// writedown rejects with `TemporarilyUnavailable` AFTER the entry-point
/// pre-validation has passed and the 3USD pull has landed. The Wave-4 `Err`
/// arm fires the refund: SP balance is restored, no orphan in
/// `protocol_3usd_reserves`, and the INFO log carries the refund line.
#[test]
fn icc_002_pic_writedown_failure_refunds_3usd_to_sp() {
    let f = setup_fixture(ThreePoolKind::Standard);

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
        "protocol reserves subaccount must be empty after the refund \
         (the pulled tokens were sent back to the SP)"
    );

    let logs = fetch_info_logs(&f.pic, f.protocol_id);
    let vault_tag = format!("vault {}", f.vault_id);
    assert!(
        logs.iter().any(|m| {
            m.contains("[stability_pool_liquidate_with_reserves] refunded")
                && m.contains(&vault_tag)
                && m.contains("after liquidation rollback")
        }),
        "expected Wave-4 refund INFO log keyed to vault #{}; saw logs: {:?}",
        f.vault_id, logs
    );
    assert!(
        !logs.iter().any(|m| m.contains("CRITICAL: refund of")),
        "refund succeeded — there must be no CRITICAL log"
    );
}

/// **Refund-of-refund failure.** Same setup as the prior test but the 3USD
/// ledger is `flaky_ledger` with `set_fail_transfers(true)`. The pull
/// (`icrc2_transfer_from`) is unaffected by that knob and lands; the
/// kill-switch reject fires the Wave-4 refund arm; the refund
/// (`icrc1_transfer`) fails. Asserts the protocol surfaces the writedown
/// error, the SP balance does NOT come back, and the operator-visible
/// CRITICAL log fires so the stranded tokens can be reconciled.
#[test]
fn icc_002_pic_refund_failure_logs_critical_and_strands_3usd() {
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

    let sp_balance_after = icrc1_balance_of(
        &f.pic,
        f.three_pool_ledger,
        account(f.sp_principal),
    );
    assert_eq!(
        sp_balance_after,
        sp_balance_before - three_usd_amount as u128,
        "SP balance MUST stay at the post-pull value: the refund failed and \
         the tokens are stranded in the protocol's reserves subaccount; \
         saw before={} after={}",
        sp_balance_before, sp_balance_after
    );

    let reserves_after = get_protocol_3usd_reserves(&f.pic, f.protocol_id);
    assert_eq!(
        reserves_after, 0,
        "the writedown rejected before the reserves counter increment, \
         so the in-state counter stays zero even though tokens were stranded \
         on-ledger — this divergence is what the CRITICAL log surfaces"
    );

    // The flaky ledger's `protocol_3usd_reserves` subaccount must hold the
    // stranded tokens (the pull landed; the refund did not).
    let reserves_subacct_balance = icrc1_balance_of(
        &f.pic,
        f.three_pool_ledger,
        Account {
            owner: f.protocol_id,
            subaccount: Some(protocol_3usd_reserves_subaccount()),
        },
    );
    assert_eq!(
        reserves_subacct_balance, three_usd_amount as u128,
        "stranded 3USD MUST sit in the protocol's reserves subaccount on the \
         3pool ledger — the refund could not move them; saw {}",
        reserves_subacct_balance
    );

    let logs = fetch_info_logs(&f.pic, f.protocol_id);
    let vault_tag = format!("vault {}", f.vault_id);
    assert!(
        logs.iter().any(|m| {
            m.contains("[stability_pool_liquidate_with_reserves] CRITICAL: refund of")
                && m.contains(&vault_tag)
                && m.contains("FAILED")
        }),
        "expected Wave-4 CRITICAL refund-failure INFO log keyed to vault #{}; \
         saw logs: {:?}",
        f.vault_id, logs
    );
    // And no successful-refund log.
    assert!(
        !logs.iter().any(|m| {
            m.contains("[stability_pool_liquidate_with_reserves] refunded")
                && m.contains("after liquidation rollback")
        }),
        "refund failed — there must be no success refund log"
    );
}
