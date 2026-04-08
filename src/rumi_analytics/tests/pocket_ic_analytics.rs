//! PocketIC integration tests for rumi_analytics Phase 1.
//!
//! Covers four behaviours:
//!   1. Supply cache populates after the 60s pull cycle.
//!   2. Daily TVL row is written after the 86400s daily timer fires.
//!   3. Pagination returns a continuation cursor when limit is reached.
//!   4. Stable storage (TVL log + supply cache) survives an upgrade.
//!
//! ICRC-1 ledger init types are copied verbatim from
//! src/rumi_amm/tests/pocket_ic_tests.rs. Backend deployment follows
//! src/rumi_protocol_backend/tests/pocket_ic_tests.rs but skips the XRC mock:
//! analytics only calls `get_protocol_status`, which is a pure query that
//! reads stable state and never touches XRC. The xrc_principal is set to an
//! anonymous placeholder.

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Principal};
use icrc_ledger_types::icrc1::account::Account;
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};

// ─── ICRC-1 ledger init types (copied from rumi_amm tests) ───

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
enum MetadataValue {
    Nat(candid::Nat),
    Int(candid::Int),
    Text(String),
    Blob(Vec<u8>),
}

#[derive(CandidType, Deserialize)]
enum LedgerArg {
    Init(LedgerInitArgs),
}

// ─── Backend init types (copied from rumi_protocol_backend tests) ───

#[derive(CandidType, Deserialize)]
struct ProtocolInitArg {
    xrc_principal: Principal,
    icusd_ledger_principal: Principal,
    icp_ledger_principal: Principal,
    fee_e8s: u64,
    developer_principal: Principal,
}

#[derive(CandidType, Deserialize)]
enum ProtocolArgVariant {
    Init(ProtocolInitArg),
}

// ─── Wasm loaders ───

fn icrc1_ledger_wasm() -> Vec<u8> {
    include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec()
}

fn analytics_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_analytics.wasm").to_vec()
}

fn backend_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm")
        .to_vec()
}

// ─── Analytics candid types (mirror rumi_analytics.did) ───

#[derive(CandidType, Deserialize)]
struct AnalyticsInitArgs {
    admin: Principal,
    backend: Principal,
    icusd_ledger: Principal,
    three_pool: Principal,
    stability_pool: Principal,
    amm: Principal,
}

#[derive(CandidType, Deserialize)]
struct RangeQueryArg {
    from_ts: Option<u64>,
    to_ts: Option<u64>,
    limit: Option<u32>,
    offset: Option<u64>,
}

#[derive(CandidType, Deserialize, Debug)]
struct DailyTvlRow {
    timestamp_ns: u64,
    total_icp_collateral_e8s: candid::Nat,
    total_icusd_supply_e8s: candid::Nat,
    system_collateral_ratio_bps: u32,
}

#[derive(CandidType, Deserialize, Debug)]
struct TvlSeriesResponse {
    rows: Vec<DailyTvlRow>,
    next_from_ts: Option<u64>,
}

#[derive(CandidType, Deserialize)]
struct HttpRequest {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

#[derive(CandidType, Deserialize)]
struct HttpResponse {
    status_code: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

// ─── Test environment ───

struct Env {
    pic: PocketIc,
    analytics: Principal,
    icusd_ledger: Principal,
    backend: Principal,
    admin: Principal,
}

fn deploy_ledger(
    pic: &PocketIc,
    minting: Principal,
    admin: Principal,
    holder: Principal,
    name: &str,
    symbol: &str,
    decimals: u8,
    initial_balance: u128,
) -> Principal {
    let ledger_id = pic.create_canister();
    pic.add_cycles(ledger_id, 2_000_000_000_000);

    let init_args = LedgerInitArgs {
        minting_account: Account { owner: minting, subaccount: None },
        fee_collector_account: None,
        transfer_fee: candid::Nat::from(0u64),
        decimals: Some(decimals),
        max_memo_length: Some(32),
        token_name: name.to_string(),
        token_symbol: symbol.to_string(),
        metadata: vec![],
        initial_balances: vec![(
            Account { owner: holder, subaccount: None },
            candid::Nat::from(initial_balance),
        )],
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

    let encoded = encode_args((LedgerArg::Init(init_args),)).expect("encode ledger init");
    pic.install_canister(ledger_id, icrc1_ledger_wasm(), encoded, None);
    ledger_id
}

/// Deploy the real rumi_protocol_backend wasm. analytics only calls
/// `get_protocol_status` (a pure query that reads stable state), so the XRC
/// principal is a placeholder. The protocol starts with no vaults so collateral
/// totals are zero. That is fine: the tests assert structural behaviour
/// (rows written, cursor returned, upgrade survives), not specific dollar
/// amounts.
fn deploy_backend(
    pic: &PocketIc,
    admin: Principal,
    icp_ledger: Principal,
    icusd_ledger: Principal,
) -> Principal {
    let backend_id = pic.create_canister_with_settings(Some(admin), None);
    pic.add_cycles(backend_id, 2_000_000_000_000);

    let init = ProtocolInitArg {
        xrc_principal: Principal::anonymous(),
        icusd_ledger_principal: icusd_ledger,
        icp_ledger_principal: icp_ledger,
        fee_e8s: 10_000,
        developer_principal: admin,
    };
    let encoded =
        encode_args((ProtocolArgVariant::Init(init),)).expect("encode backend init");
    pic.install_canister(backend_id, backend_wasm(), encoded, Some(admin));
    backend_id
}

fn setup() -> Env {
    let pic = PocketIcBuilder::new().with_application_subnet().build();

    let minting = Principal::self_authenticating(&[100, 100, 100]);
    let admin = Principal::self_authenticating(&[5, 6, 7, 8]);
    let user = Principal::self_authenticating(&[1, 2, 3, 4]);

    // Real ICRC-1 ledger as the icusd fixture, with a non-zero total supply
    // so the analytics supply collector returns a positive number.
    let icusd_ledger = deploy_ledger(
        &pic,
        minting,
        admin,
        user,
        "icUSD test",
        "ICUSDT",
        8,
        1_000_000_00000000,
    );
    // A second ledger to satisfy the backend's icp_ledger_principal slot.
    let icp_ledger = deploy_ledger(
        &pic, minting, admin, user, "ICP test", "ICPT", 8, 1_000_000_00000000,
    );

    let backend = deploy_backend(&pic, admin, icp_ledger, icusd_ledger);

    let analytics = pic.create_canister_with_settings(Some(admin), None);
    pic.add_cycles(analytics, 2_000_000_000_000);
    let init = AnalyticsInitArgs {
        admin,
        backend,
        icusd_ledger,
        three_pool: Principal::anonymous(),
        stability_pool: Principal::anonymous(),
        amm: Principal::anonymous(),
    };
    pic.install_canister(
        analytics,
        analytics_wasm(),
        encode_one(init).unwrap(),
        Some(admin),
    );

    Env { pic, analytics, icusd_ledger, backend, admin }
}

fn http_get(env: &Env, path: &str) -> HttpResponse {
    let req = HttpRequest {
        method: "GET".into(),
        url: path.into(),
        headers: vec![],
        body: vec![],
    };
    let result = env
        .pic
        .query_call(
            env.analytics,
            env.admin,
            "http_request",
            encode_one(req).unwrap(),
        )
        .expect("http_request query");
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).unwrap(),
        WasmResult::Reject(msg) => panic!("http_request rejected: {}", msg),
    }
}

fn get_tvl_series(env: &Env, q: RangeQueryArg) -> TvlSeriesResponse {
    let result = env
        .pic
        .query_call(
            env.analytics,
            env.admin,
            "get_tvl_series",
            encode_one(q).unwrap(),
        )
        .expect("get_tvl_series query");
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).unwrap(),
        WasmResult::Reject(msg) => panic!("get_tvl_series rejected: {}", msg),
    }
}

// ─── Tests ───

#[test]
fn supply_cache_populated_after_pull_cycle() {
    let env = setup();

    // Drive the 60s pull cycle. A few extra ticks let async inter-canister
    // calls (icrc1_total_supply, get_protocol_status) settle.
    env.pic.advance_time(std::time::Duration::from_secs(65));
    for _ in 0..5 {
        env.pic.tick();
    }

    let after = http_get(&env, "/api/supply");
    assert_eq!(after.status_code, 200, "expected /api/supply to return 200");
    let body = String::from_utf8(after.body).unwrap();
    let supply: f64 = body.trim().parse().expect("parse supply body as f64");
    assert!(supply > 0.0, "supply should be > 0, got {}", supply);
}

#[test]
fn daily_tvl_row_written_after_daily_timer() {
    let env = setup();
    env.pic.advance_time(std::time::Duration::from_secs(86_400 + 65));
    for _ in 0..5 {
        env.pic.tick();
    }

    let resp = get_tvl_series(
        &env,
        RangeQueryArg { from_ts: None, to_ts: None, limit: None, offset: None },
    );
    assert!(!resp.rows.is_empty(), "expected at least one TVL row");
    let row = &resp.rows[0];
    // The backend has no vaults in this fixture, so total_icp_collateral_e8s
    // is zero. The point of the test is that a row was written by the timer,
    // not the magnitude of the value. Assert structural fields instead.
    assert!(row.timestamp_ns > 0, "row should have a non-zero timestamp");
}

#[test]
fn pagination_returns_next_cursor_when_full() {
    let env = setup();
    for _ in 0..3 {
        env.pic.advance_time(std::time::Duration::from_secs(86_400 + 65));
        for _ in 0..5 {
            env.pic.tick();
        }
    }

    let resp = get_tvl_series(
        &env,
        RangeQueryArg { from_ts: None, to_ts: None, limit: Some(2), offset: None },
    );
    assert_eq!(resp.rows.len(), 2, "expected limit to cap rows at 2");
    assert!(resp.next_from_ts.is_some(), "expected continuation cursor");
}

#[test]
fn upgrade_preserves_supply_cache_and_tvl_log() {
    let env = setup();
    env.pic.advance_time(std::time::Duration::from_secs(86_400 + 65));
    for _ in 0..5 {
        env.pic.tick();
    }

    let before_rows = get_tvl_series(
        &env,
        RangeQueryArg { from_ts: None, to_ts: None, limit: None, offset: None },
    )
    .rows
    .len();
    let before_supply = http_get(&env, "/api/supply").body;
    assert!(before_rows > 0, "precondition: TVL log should have a row");

    env.pic
        .upgrade_canister(
            env.analytics,
            analytics_wasm(),
            encode_one(AnalyticsInitArgs {
                admin: env.admin,
                backend: env.backend,
                icusd_ledger: env.icusd_ledger,
                three_pool: Principal::anonymous(),
                stability_pool: Principal::anonymous(),
                amm: Principal::anonymous(),
            })
            .unwrap(),
            Some(env.admin),
        )
        .expect("upgrade analytics");

    let after_rows = get_tvl_series(
        &env,
        RangeQueryArg { from_ts: None, to_ts: None, limit: None, offset: None },
    )
    .rows
    .len();
    let after_supply = http_get(&env, "/api/supply").body;
    assert_eq!(before_rows, after_rows, "TVL log lost rows on upgrade");
    assert_eq!(before_supply, after_supply, "supply cache lost on upgrade");
}
