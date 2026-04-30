//! Wave-10 LIQ-008 fence (Layer 3 — canister boundary).
//!
//! Layer 1+2 fences (in `audit_pocs_liq_008_circuit_breaker.rs`) cover the
//! pure breaker math: state defaults, CBOR round-trip, append-and-prune,
//! ceiling-cross trip semantics, sticky latch behavior, window=0/ceiling=0
//! disabling. They cannot exercise:
//!
//!   * the canister-boundary path where a real liquidation walks through
//!     `mutate_state`, `record_liquidation_for_breaker`, the BreakerTripped
//!     event emission, and ProtocolStatus exposure;
//!   * the `check_vaults` gate (manual liquidations stay open even after
//!     the breaker is tripped);
//!   * the admin-clear path (operator flips the latch and auto-publishing
//!     resumes);
//!   * persistence of all four LIQ-008 fields across canister upgrades.
//!
//! This file fences those paths end-to-end. The fixture is lifted from
//! `audit_pocs_liq_005_deficit_account_pic.rs` — same icRC-1 ledger deploy
//! helper, mock-XRC, boot sequence — extended to expose the four LIQ-008
//! ProtocolStatus fields and the three admin endpoints.

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use std::time::{Duration, SystemTime};

use rumi_protocol_backend::ProtocolError;

// ─── Local mirrors of ICRC-1 Candid types (standard ic-icrc1-ledger) ───

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
struct UpgradeArg {
    mode: Option<String>,
    description: Option<String>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum ProtocolArgVariant {
    Init(ProtocolInitArg),
    Upgrade(UpgradeArg),
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

// ─── ProtocolStatus mirror (only the LIQ-005 + LIQ-008 tail) ───
//
// Candid records are field-keyed (not positional), so we can subset.
// Mode + total_icusd_borrowed + the LIQ-005 four are kept for compatibility
// with the reused fixture; the four new LIQ-008 fields are what these
// tests assert against.
#[derive(CandidType, Deserialize, Clone, Debug)]
struct ProtocolStatusSubset {
    mode: ProtocolMode,
    total_icusd_borrowed: u64,
    protocol_deficit_icusd: u64,
    total_deficit_repaid_icusd: u64,
    deficit_repayment_fraction: f64,
    deficit_readonly_threshold_e8s: u64,
    breaker_window_ns: u64,
    breaker_window_debt_ceiling_e8s: u64,
    windowed_liquidation_total_e8s: u64,
    liquidation_breaker_tripped: bool,
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
enum ProtocolMode {
    GeneralAvailability,
    Recovery,
    ReadOnly,
}

// ─── WASM fixtures ───

fn icrc1_ledger_wasm() -> Vec<u8> {
    include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec()
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

fn protocol_status(pic: &PocketIc, protocol_id: Principal) -> ProtocolStatusSubset {
    let result = pic
        .query_call(
            protocol_id,
            Principal::anonymous(),
            "get_protocol_status",
            encode_args(()).unwrap(),
        )
        .expect("get_protocol_status call failed");
    match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode get_protocol_status"),
        WasmResult::Reject(m) => panic!("get_protocol_status rejected: {}", m),
    }
}

fn set_breaker_window_ns_admin(fixture: &Fixture, ns: u64) {
    let result = fixture
        .pic
        .update_call(
            fixture.protocol_id,
            fixture.developer,
            "set_breaker_window_ns",
            encode_args((ns,)).unwrap(),
        )
        .expect("set_breaker_window_ns call failed");
    let parsed: Result<(), ProtocolError> = match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode set_breaker_window_ns"),
        WasmResult::Reject(m) => panic!("set_breaker_window_ns rejected: {}", m),
    };
    parsed.expect("set_breaker_window_ns returned error");
}

fn set_breaker_window_debt_ceiling_e8s_admin(fixture: &Fixture, ceiling: u64) {
    let result = fixture
        .pic
        .update_call(
            fixture.protocol_id,
            fixture.developer,
            "set_breaker_window_debt_ceiling_e8s",
            encode_args((ceiling,)).unwrap(),
        )
        .expect("set_breaker_window_debt_ceiling_e8s call failed");
    let parsed: Result<(), ProtocolError> = match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode set_breaker_ceiling"),
        WasmResult::Reject(m) => panic!("set_breaker_ceiling rejected: {}", m),
    };
    parsed.expect("set_breaker_ceiling returned error");
}

fn clear_liquidation_breaker_admin(fixture: &Fixture) {
    let result = fixture
        .pic
        .update_call(
            fixture.protocol_id,
            fixture.developer,
            "clear_liquidation_breaker",
            encode_args(()).unwrap(),
        )
        .expect("clear_liquidation_breaker call failed");
    let parsed: Result<(), ProtocolError> = match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode clear_liquidation_breaker"),
        WasmResult::Reject(m) => panic!("clear_liquidation_breaker rejected: {}", m),
    };
    parsed.expect("clear_liquidation_breaker returned error");
}

fn liquidate_full(fixture: &Fixture, vault_id: u64) -> Result<SuccessWithFee, ProtocolError> {
    let result = fixture
        .pic
        .update_call(
            fixture.protocol_id,
            fixture.test_user,
            "liquidate_vault",
            encode_args((vault_id,)).unwrap(),
        )
        .expect("liquidate_vault call failed");
    match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode liquidate"),
        WasmResult::Reject(m) => panic!("liquidate rejected: {}", m),
    }
}

// ─── Fixture ───

struct Fixture {
    pic: PocketIc,
    protocol_id: Principal,
    icp_ledger: Principal,
    icusd_ledger: Principal,
    xrc_id: Principal,
    developer: Principal,
    test_user: Principal,
    /// Pre-opened vault id with 50 ICP collateral and 100 icUSD borrowed.
    /// At $10/ICP starting price → CR = 500%. Drop ICP to $0.10 to push
    /// well below the 110% liquidation threshold.
    vault_id: u64,
}

fn setup_fixture() -> Fixture {
    let pic = PocketIcBuilder::new().with_nns_subnet().build();

    let test_user = Principal::self_authenticating(b"liq_008_pic_user");
    let developer = Principal::self_authenticating(b"liq_008_pic_developer");
    let treasury = Principal::self_authenticating(b"liq_008_pic_treasury");

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

    // Disable the dynamic borrow-fee multiplier curve, interest-rate curve,
    // and base borrowing fee so vault math stays clean across the test.
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
        xrc_id,
        developer,
        test_user,
        vault_id,
    }
}

/// Open a SECOND vault with the same 50-ICP/100-icUSD shape. Returns the
/// new vault id so tests can liquidate vault #1 to seed the breaker, then
/// hit vault #2 via the manual endpoint to prove it stays open after trip.
fn open_second_vault(fixture: &Fixture) -> u64 {
    icrc2_approve_call(
        &fixture.pic,
        fixture.icp_ledger,
        fixture.test_user,
        fixture.protocol_id,
        50_000_000_000u128,
    );
    let r = fixture
        .pic
        .update_call(
            fixture.protocol_id,
            fixture.test_user,
            "open_vault",
            encode_args((5_000_000_000u64, None::<Principal>)).unwrap(),
        )
        .expect("open_vault");
    let vault = match r {
        WasmResult::Reply(b) => {
            let r: Result<OpenVaultSuccess, ProtocolError> = decode_one(&b).unwrap();
            r.unwrap().vault_id
        }
        WasmResult::Reject(m) => panic!("open rejected: {}", m),
    };
    let borrow_res = fixture
        .pic
        .update_call(
            fixture.protocol_id,
            fixture.test_user,
            "borrow_from_vault",
            encode_args((VaultArg {
                vault_id: vault,
                amount: 10_000_000_000u64,
            },))
            .unwrap(),
        )
        .expect("borrow");
    match borrow_res {
        WasmResult::Reply(b) => {
            let r: Result<SuccessWithFee, ProtocolError> = decode_one(&b).unwrap();
            r.expect("borrow returned error");
        }
        WasmResult::Reject(m) => panic!("borrow rejected: {}", m),
    }
    vault
}

/// Drop ICP price to `new_price_e8s` and tick until the protocol's cached
/// price reflects it.
fn drop_icp_price(fixture: &Fixture, new_price_e8s: u64) {
    xrc_set_rate(
        &fixture.pic,
        fixture.xrc_id,
        fixture.developer,
        "ICP",
        "USD",
        new_price_e8s,
    );
    fixture.pic.advance_time(Duration::from_secs(310));
    for _ in 0..10 {
        fixture.pic.tick();
    }
}

// ─── Tests ───

const NS_PER_SEC: u64 = 1_000_000_000;

/// Wave-10 LIQ-008 PocketIC #1: an underwater liquidation appends to the
/// rolling window AND, when the windowed total crosses the configured
/// ceiling, flips `liquidation_breaker_tripped`. This is the load-bearing
/// predicate; if it's wrong the rest of the suite is misleading.
#[test]
fn liq_008_pic_breaker_trips_after_cumulative_threshold() {
    let f = setup_fixture();

    // Sanity: defaults — 30-min window, ceiling 0 (disabled), latch false.
    let s0 = protocol_status(&f.pic, f.protocol_id);
    assert_eq!(s0.breaker_window_ns, 30 * 60 * NS_PER_SEC);
    assert_eq!(s0.breaker_window_debt_ceiling_e8s, 0);
    assert_eq!(s0.windowed_liquidation_total_e8s, 0);
    assert!(!s0.liquidation_breaker_tripped);

    // Configure a tight ceiling: 50 icUSD = 5_000_000_000 e8s. The vault
    // has 100 icUSD debt cleared on a single full liquidation so the very
    // first liquidation crosses the threshold and trips the breaker.
    set_breaker_window_debt_ceiling_e8s_admin(&f, 5_000_000_000u64);

    drop_icp_price(&f, 10_000_000); // $0.10 — push well underwater

    icrc2_approve_call(
        &f.pic,
        f.icusd_ledger,
        f.test_user,
        f.protocol_id,
        100_000_000_000u128,
    );
    liquidate_full(&f, f.vault_id).expect("liquidate_vault returned error");

    let s1 = protocol_status(&f.pic, f.protocol_id);
    assert!(
        s1.windowed_liquidation_total_e8s > 0,
        "windowed total must be > 0 after a liquidation"
    );
    assert!(
        s1.liquidation_breaker_tripped,
        "breaker must trip on cumulative debt {} >= ceiling {}",
        s1.windowed_liquidation_total_e8s,
        s1.breaker_window_debt_ceiling_e8s
    );
    assert!(
        s1.windowed_liquidation_total_e8s >= s1.breaker_window_debt_ceiling_e8s,
        "windowed total {} below ceiling {} but breaker tripped",
        s1.windowed_liquidation_total_e8s,
        s1.breaker_window_debt_ceiling_e8s
    );
}

/// Wave-10 LIQ-008 PocketIC #2: after the breaker trips, manual
/// liquidation endpoints stay open. The breaker only gates `check_vaults`
/// auto-publishing; user-callable `liquidate_vault` keeps working.
#[test]
fn liq_008_pic_manual_liquidation_still_works_after_trip() {
    let f = setup_fixture();
    let second_vault = open_second_vault(&f);

    // Configure a low ceiling and trip the breaker via vault #1.
    set_breaker_window_debt_ceiling_e8s_admin(&f, 5_000_000_000u64);
    drop_icp_price(&f, 10_000_000);
    icrc2_approve_call(
        &f.pic,
        f.icusd_ledger,
        f.test_user,
        f.protocol_id,
        200_000_000_000u128,
    );
    liquidate_full(&f, f.vault_id).expect("seeding liquidation must succeed");

    let s_after_trip = protocol_status(&f.pic, f.protocol_id);
    assert!(s_after_trip.liquidation_breaker_tripped, "breaker must be tripped");

    // The manual `liquidate_vault` endpoint must still succeed against
    // vault #2 even with the breaker tripped. The breaker only gates
    // `check_vaults` auto-publishing.
    liquidate_full(&f, second_vault)
        .expect("manual liquidate_vault must succeed even with breaker tripped");
}

/// Wave-10 LIQ-008 PocketIC #3: `clear_liquidation_breaker` flips the
/// latch back to false. Auto-publishing resumes on the next `check_vaults`
/// tick. Confirmed via the ProtocolStatus flag.
#[test]
fn liq_008_pic_admin_clear_resumes_publishing() {
    let f = setup_fixture();

    set_breaker_window_debt_ceiling_e8s_admin(&f, 5_000_000_000u64);
    drop_icp_price(&f, 10_000_000);
    icrc2_approve_call(
        &f.pic,
        f.icusd_ledger,
        f.test_user,
        f.protocol_id,
        100_000_000_000u128,
    );
    liquidate_full(&f, f.vault_id).expect("seeding liquidation must succeed");

    assert!(
        protocol_status(&f.pic, f.protocol_id).liquidation_breaker_tripped,
        "breaker must trip before clear"
    );

    clear_liquidation_breaker_admin(&f);
    assert!(
        !protocol_status(&f.pic, f.protocol_id).liquidation_breaker_tripped,
        "clear_liquidation_breaker must flip the latch to false"
    );
}

/// Wave-10 LIQ-008 PocketIC #4: advancing time past the window causes
/// `windowed_liquidation_total_e8s` to drop because reads filter without
/// mutation. The latch stays sticky (T2 semantics) — admin clear is the
/// only way to re-enable auto-publishing.
#[test]
fn liq_008_pic_window_eviction_drops_old_entries() {
    let f = setup_fixture();

    // Use a very short window so the test doesn't have to advance time
    // by 30 minutes. Ceiling = 0 means we don't trip; we only verify
    // eviction behavior on the windowed total.
    set_breaker_window_ns_admin(&f, 60 * NS_PER_SEC);
    set_breaker_window_debt_ceiling_e8s_admin(&f, 0);

    drop_icp_price(&f, 10_000_000);
    icrc2_approve_call(
        &f.pic,
        f.icusd_ledger,
        f.test_user,
        f.protocol_id,
        100_000_000_000u128,
    );
    liquidate_full(&f, f.vault_id).expect("seeding liquidation must succeed");

    let total_before = protocol_status(&f.pic, f.protocol_id).windowed_liquidation_total_e8s;
    assert!(total_before > 0, "windowed total must be > 0 right after liquidation");

    // Advance past the window. Reads should filter out the entry without
    // requiring a write to prune it.
    f.pic.advance_time(Duration::from_secs(120));
    f.pic.tick();
    let total_after = protocol_status(&f.pic, f.protocol_id).windowed_liquidation_total_e8s;
    assert_eq!(
        total_after, 0,
        "windowed total {} must drop to 0 once the entry is past the window",
        total_after
    );

    // T2 semantics: even if the windowed total has rolled past the window,
    // the latch stays whatever it was. Here ceiling=0 so the breaker never
    // tripped — latch must still be false.
    assert!(!protocol_status(&f.pic, f.protocol_id).liquidation_breaker_tripped);
}

/// Wave-10 LIQ-008 PocketIC #5: canister upgrade preserves all four
/// breaker fields. Without `#[serde(default)]` plumbing on each field, an
/// upgrade would either trap or silently zero the breaker config.
#[test]
fn liq_008_pic_upgrade_preserves_breaker_state() {
    let f = setup_fixture();

    // Configure non-default values for window + ceiling.
    set_breaker_window_ns_admin(&f, 12 * 60 * NS_PER_SEC); // 12 min, not the default 30 min
    set_breaker_window_debt_ceiling_e8s_admin(&f, 5_000_000_000u64);

    // Trip the breaker so all four fields are at non-default values.
    drop_icp_price(&f, 10_000_000);
    icrc2_approve_call(
        &f.pic,
        f.icusd_ledger,
        f.test_user,
        f.protocol_id,
        100_000_000_000u128,
    );
    liquidate_full(&f, f.vault_id).expect("seeding liquidation must succeed");

    let pre = protocol_status(&f.pic, f.protocol_id);
    assert!(pre.liquidation_breaker_tripped, "breaker must trip for upgrade preservation test");
    assert_eq!(pre.breaker_window_ns, 12 * 60 * NS_PER_SEC);
    assert_eq!(pre.breaker_window_debt_ceiling_e8s, 5_000_000_000u64);
    assert!(pre.windowed_liquidation_total_e8s > 0);

    // Upgrade.
    let upgrade_arg = ProtocolArgVariant::Upgrade(UpgradeArg {
        mode: None,
        description: Some("LIQ-008 PIC upgrade fence".to_string()),
    });
    f.pic
        .upgrade_canister(
            f.protocol_id,
            protocol_wasm(),
            encode_args((upgrade_arg,)).expect("encode upgrade"),
            None,
        )
        .expect("upgrade_canister failed");
    f.pic.tick();

    let post = protocol_status(&f.pic, f.protocol_id);
    assert!(
        post.liquidation_breaker_tripped,
        "tripped flag must survive upgrade"
    );
    assert_eq!(
        post.breaker_window_ns, 12 * 60 * NS_PER_SEC,
        "breaker_window_ns drifted across upgrade"
    );
    assert_eq!(
        post.breaker_window_debt_ceiling_e8s, 5_000_000_000u64,
        "breaker_window_debt_ceiling_e8s drifted across upgrade"
    );
    // The upgrade does not advance time, so the windowed total should be
    // identical to pre-upgrade (entries are still inside the window).
    assert_eq!(
        post.windowed_liquidation_total_e8s, pre.windowed_liquidation_total_e8s,
        "windowed_liquidation_total_e8s drifted across upgrade"
    );
}
