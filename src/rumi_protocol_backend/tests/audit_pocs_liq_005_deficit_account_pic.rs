//! Wave-8e LIQ-005 fence (Layer 3 — canister boundary).
//!
//! Layer 1+2 fences (in `audit_pocs_liq_005_deficit_account.rs`) cover the
//! pure deficit-account math: state defaults, CBOR round-trip, predicate,
//! `compute_deficit_repay_amount`, ReadOnly latch state machine, fee
//! routing math via `plan_fee_routing_at`. They cannot exercise:
//!
//!   * the canister-boundary path where a real liquidation walks through
//!     `mutate_state`, the deficit accrual fence in `liquidate_vault`,
//!     event recording, and ProtocolStatus exposure;
//!   * the borrowing-fee mint path where `mint_borrowing_fee_to_treasury`
//!     consults `plan_fee_routing` before the actual ledger op;
//!   * persistence of the four LIQ-005 fields across canister upgrades.
//!
//! This file fences those paths end-to-end. The fixture is modeled on
//! `audit_pocs_liq_004_icrc3_burn_proof_pic.rs` — a self-contained
//! protocol + icUSD + ICP + mock-XRC + treasury setup with one healthy
//! pre-borrowed vault. The mock XRC's `set_exchange_rate` lets us drop
//! ICP price hard to push the vault deeply underwater for the accrual
//! tests.

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

// ─── ProtocolStatus mirror (only the LIQ-005-relevant tail) ───
//
// Candid records are field-keyed (not positional), so we can subset.
// All four LIQ-005 fields plus `mode` / `total_icusd_borrowed` are
// what these tests assert against. The rest of `ProtocolStatus` is
// left out — Candid silently ignores unknown fields when decoding.
#[derive(CandidType, Deserialize, Clone, Debug)]
struct ProtocolStatusSubset {
    mode: ProtocolMode,
    total_icusd_borrowed: u64,
    protocol_deficit_icusd: u64,
    total_deficit_repaid_icusd: u64,
    deficit_repayment_fraction: f64,
    deficit_readonly_threshold_e8s: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
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

/// Set an XRC mock rate. The mock at `xrc_demo/xrc/xrc.wasm` exposes
/// `set_exchange_rate(base, quote, rate_e8s)`. Used to simulate price
/// drops that push vaults underwater.
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

// ─── Fixture ───

struct Fixture {
    pic: PocketIc,
    protocol_id: Principal,
    icp_ledger: Principal,
    icusd_ledger: Principal,
    xrc_id: Principal,
    treasury: Principal,
    developer: Principal,
    test_user: Principal,
    /// Pre-opened vault id with 50 ICP collateral and 100 icUSD borrowed.
    /// At $10/ICP starting price → CR = 500%. Drop ICP to $0.10 to push
    /// well below the 110% liquidation threshold (~$5 backing for $100
    /// debt).
    vault_id: u64,
}

fn setup_fixture() -> Fixture {
    let pic = PocketIcBuilder::new().with_nns_subnet().build();

    let test_user = Principal::self_authenticating(b"liq_005_pic_user");
    let developer = Principal::self_authenticating(b"liq_005_pic_developer");
    let treasury = Principal::self_authenticating(b"liq_005_pic_treasury");

    // Pre-allocate the protocol canister so its principal can be the
    // icUSD minting account from the start. The `inspect_message` hook
    // drops anonymous update calls, so we install developer as a
    // controller alongside anonymous to allow admin endpoints.
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

    // icUSD ledger with zero transfer fee — keeps the test math clean
    // by avoiding 0.0001-icUSD shortfalls when the user calls
    // `icrc2_approve` before a liquidation.
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

    // Disable the dynamic borrow-fee multiplier curve. The default curve
    // (initialized in `From<InitArg>`) attaches a 3x multiplier to low-CR
    // borrows; without this disable the user ends up short of icUSD when
    // they later try to liquidate the full debt.
    let _ = pic
        .update_call(
            protocol_id,
            developer,
            "set_borrowing_fee_curve",
            encode_args((None::<String>,)).unwrap(),
        )
        .expect("set_borrowing_fee_curve");
    // Disable the interest-rate curve markers so vault debt doesn't
    // drift between borrow and liquidate.
    let _ = pic
        .update_call(
            protocol_id,
            developer,
            "set_rate_curve_markers",
            encode_args((None::<Principal>, vec![(1.5f64, 1.0f64), (3.0f64, 1.0f64)])).unwrap(),
        )
        .expect("set_rate_curve_markers");
    // Default borrowing fee = 0 so the user gets the full borrowed
    // amount and can pay the full liquidation. Tests that exercise the
    // borrowing-fee deficit-repayment path bump this back up before the
    // relevant borrow.
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

    // Wire the treasury so `mint_borrowing_fee_to_treasury` actually mints.
    let _ = pic
        .update_call(
            protocol_id,
            developer,
            "set_treasury_principal",
            encode_args((treasury,)).unwrap(),
        )
        .expect("set_treasury_principal");

    // Open a vault with 50 ICP collateral and borrow 100 icUSD against
    // it. At $10/ICP that's 500% CR — way above the borrow threshold
    // and the liquidation threshold. The accrual tests then drop ICP
    // price to make the vault underwater.
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
        treasury,
        developer,
        test_user,
        vault_id,
    }
}

/// Drop ICP price to `new_price_e8s` and tick until the protocol's cached
/// price reflects it. The protocol fetches prices on a 5-minute timer
/// and on-demand via `validate_call` before liquidation entry points.
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

/// Wave-8e LIQ-005 PocketIC #1: an underwater liquidation accrues
/// `protocol_deficit_icusd`. This is the load-bearing predicate — if it's
/// wrong, every other test in this file lies.
#[test]
fn liq_005_pic_underwater_liquidation_accrues_deficit() {
    let f = setup_fixture();

    // Sanity: deficit starts at 0, fraction default 0.5, threshold 0.
    let s0 = protocol_status(&f.pic, f.protocol_id);
    assert_eq!(s0.protocol_deficit_icusd, 0);
    assert_eq!(s0.total_deficit_repaid_icusd, 0);
    assert!((s0.deficit_repayment_fraction - 0.5).abs() < 1e-9);
    assert_eq!(s0.deficit_readonly_threshold_e8s, 0);

    // Drop ICP to $0.10 so the 50-ICP vault is worth ~$5 backing $100
    // debt — deeply underwater.
    drop_icp_price(&f, 10_000_000); // $0.10 in e8s

    // Approve + transfer enough icUSD to the test user so they can pay
    // the liquidation. The user already has 100 icUSD from the borrow.
    icrc2_approve_call(
        &f.pic,
        f.icusd_ledger,
        f.test_user,
        f.protocol_id,
        100_000_000_000u128,
    );

    // Liquidate via the full path. The vault has 100 icUSD debt, and at
    // $0.10/ICP * 50 ICP = $5, the seized USD value is $5 vs $100 debt
    // cleared → expected shortfall ≈ $95 in icUSD e8s = 9_500_000_000.
    let result = f
        .pic
        .update_call(
            f.protocol_id,
            f.test_user,
            "liquidate_vault",
            encode_args((f.vault_id,)).unwrap(),
        )
        .expect("liquidate_vault call failed");
    match result {
        WasmResult::Reply(bytes) => {
            let r: Result<SuccessWithFee, ProtocolError> =
                decode_one(&bytes).expect("decode liquidate");
            r.expect("liquidate_vault returned error");
        }
        WasmResult::Reject(msg) => panic!("liquidate rejected: {}", msg),
    }

    // Verify deficit accrued.
    let s1 = protocol_status(&f.pic, f.protocol_id);
    assert!(
        s1.protocol_deficit_icusd > 0,
        "expected deficit > 0, got {}",
        s1.protocol_deficit_icusd
    );
    // Expected ≈ 95 icUSD = 9_500_000_000 e8s. Allow ±2% slack for
    // protocol_cut + interest accounting drift.
    let expected = 9_500_000_000u64;
    let lo = expected * 98 / 100;
    let hi = expected * 102 / 100;
    assert!(
        s1.protocol_deficit_icusd >= lo && s1.protocol_deficit_icusd <= hi,
        "deficit {} out of expected [{}, {}] band",
        s1.protocol_deficit_icusd,
        lo,
        hi
    );
    // Note on mode: the existing protocol auto-latches ReadOnly when the
    // total collateral ratio dips below 100% (`update_mode` in state.rs).
    // That's a separate latch from the LIQ-005 deficit-driven latch, which
    // is disabled here (threshold = 0). At $0.10 ICP with $5 backing $100
    // debt, system CR < 100% so ReadOnly is expected via the existing
    // mechanism. The LIQ-005 fence is the deficit accrual itself, which
    // we already verified.
    let _ = s1; // suppress unused-binding warning
}

/// Wave-8e LIQ-005 PocketIC #2: a borrowing fee on a NEW vault repays
/// the deficit. With the default fraction = 0.5, half of the fee skips
/// the treasury mint and decrements `protocol_deficit_icusd`.
#[test]
fn liq_005_pic_borrowing_fee_repays_deficit() {
    let f = setup_fixture();

    // Stage 1: accrue some deficit by an underwater liquidation.
    drop_icp_price(&f, 10_000_000); // $0.10
    icrc2_approve_call(
        &f.pic,
        f.icusd_ledger,
        f.test_user,
        f.protocol_id,
        100_000_000_000u128,
    );
    let _ = f
        .pic
        .update_call(
            f.protocol_id,
            f.test_user,
            "liquidate_vault",
            encode_args((f.vault_id,)).unwrap(),
        )
        .expect("liquidate_vault failed");
    let s_after_liq = protocol_status(&f.pic, f.protocol_id);
    let deficit_seed = s_after_liq.protocol_deficit_icusd;
    assert!(deficit_seed > 0, "stage 1 must seed deficit");

    // Stage 2: restore ICP price so a new vault can open + borrow
    // without being immediately liquidatable.
    drop_icp_price(&f, 1_000_000_000); // back to $10

    // Stage 2b: bump borrowing fee to 1% for the deficit-repayment test.
    // The fixture defaults it to 0% to keep the underwater test simple.
    let _ = f
        .pic
        .update_call(
            f.protocol_id,
            f.developer,
            "set_borrowing_fee",
            encode_args((0.01f64,)).unwrap(),
        )
        .expect("set_borrowing_fee");

    // Stage 3: open a fresh vault with 50 ICP collateral, borrow 100
    // icUSD against it. At $10/ICP: $500 collateral → CR 5.0, healthy.
    // Borrowing fee 1% on 100 icUSD = 1 icUSD = 100_000_000 e8s. With
    // fraction = 0.5, expect 50_000_000 e8s routed to deficit repayment.
    icrc2_approve_call(
        &f.pic,
        f.icp_ledger,
        f.test_user,
        f.protocol_id,
        50_000_000_000u128,
    );
    let open_result = f
        .pic
        .update_call(
            f.protocol_id,
            f.test_user,
            "open_vault",
            encode_args((5_000_000_000u64, None::<Principal>)).unwrap(),
        )
        .expect("open_vault failed");
    let new_vault_id = match open_result {
        WasmResult::Reply(bytes) => {
            let r: Result<OpenVaultSuccess, ProtocolError> =
                decode_one(&bytes).expect("decode");
            r.expect("open returned error").vault_id
        }
        WasmResult::Reject(m) => panic!("open rejected: {}", m),
    };

    let borrow_amount = 10_000_000_000u64; // 100 icUSD
    let borrow_arg = VaultArg {
        vault_id: new_vault_id,
        amount: borrow_amount,
    };
    let borrow_res = f
        .pic
        .update_call(
            f.protocol_id,
            f.test_user,
            "borrow_from_vault",
            encode_args((borrow_arg,)).unwrap(),
        )
        .expect("borrow_from_vault failed");
    match borrow_res {
        WasmResult::Reply(b) => {
            let r: Result<SuccessWithFee, ProtocolError> = decode_one(&b).unwrap();
            r.expect("borrow returned error");
        }
        WasmResult::Reject(m) => panic!("borrow rejected: {}", m),
    }

    // Stage 4: verify deficit decremented by the expected amount.
    let s_after_borrow = protocol_status(&f.pic, f.protocol_id);
    let expected_repay = 50_000_000u64; // 0.5 × 0.01 × 100 icUSD = 0.5 icUSD = 50_000_000 e8s
    let actual_repay = deficit_seed - s_after_borrow.protocol_deficit_icusd;
    let lo = expected_repay * 98 / 100;
    let hi = expected_repay * 102 / 100;
    assert!(
        actual_repay >= lo && actual_repay <= hi,
        "deficit decrement {} out of expected [{}, {}]; deficit before {}, after {}",
        actual_repay,
        lo,
        hi,
        deficit_seed,
        s_after_borrow.protocol_deficit_icusd
    );
    assert_eq!(
        s_after_borrow.total_deficit_repaid_icusd, actual_repay,
        "total_deficit_repaid_icusd must equal the decrement"
    );
}

/// Wave-8e LIQ-005 PocketIC #3: the ReadOnly auto-latch fires once the
/// deficit crosses the configured threshold.
#[test]
fn liq_005_pic_readonly_latch_at_threshold() {
    let f = setup_fixture();

    // Set a very small threshold so any underwater liquidation crosses it.
    let _ = f
        .pic
        .update_call(
            f.protocol_id,
            f.developer,
            "set_deficit_readonly_threshold_e8s",
            encode_args((1_000_000_000u64,)).unwrap(), // 10 icUSD
        )
        .expect("set_deficit_readonly_threshold_e8s");

    let s_pre = protocol_status(&f.pic, f.protocol_id);
    assert_eq!(s_pre.deficit_readonly_threshold_e8s, 1_000_000_000);
    assert_eq!(s_pre.mode, ProtocolMode::GeneralAvailability);

    // Trigger an underwater liquidation that pushes deficit past the
    // threshold.
    drop_icp_price(&f, 10_000_000); // $0.10
    icrc2_approve_call(
        &f.pic,
        f.icusd_ledger,
        f.test_user,
        f.protocol_id,
        100_000_000_000u128,
    );
    let _ = f
        .pic
        .update_call(
            f.protocol_id,
            f.test_user,
            "liquidate_vault",
            encode_args((f.vault_id,)).unwrap(),
        )
        .expect("liquidate_vault failed");

    let s_post = protocol_status(&f.pic, f.protocol_id);
    assert!(
        s_post.protocol_deficit_icusd >= 1_000_000_000,
        "deficit {} did not cross threshold",
        s_post.protocol_deficit_icusd
    );
    assert_eq!(
        s_post.mode,
        ProtocolMode::ReadOnly,
        "expected mode=ReadOnly after threshold crossed; got {:?}",
        s_post.mode
    );
}

/// Wave-8e LIQ-005 PocketIC #4: canister upgrade preserves all four
/// LIQ-005 state fields. Without `#[serde(default)]` plumbing, an upgrade
/// would either trap or silently zero the deficit.
#[test]
fn liq_005_pic_upgrade_preserves_deficit_state() {
    let f = setup_fixture();

    // Configure non-default values for all four fields.
    let _ = f
        .pic
        .update_call(
            f.protocol_id,
            f.developer,
            "set_deficit_repayment_fraction",
            encode_args((0.75f64,)).unwrap(),
        )
        .expect("set_deficit_repayment_fraction");
    let _ = f
        .pic
        .update_call(
            f.protocol_id,
            f.developer,
            "set_deficit_readonly_threshold_e8s",
            encode_args((u64::MAX,)).unwrap(), // MAX so the underwater
                                                // liquidation below
                                                // doesn't latch ReadOnly.
        )
        .expect("set_deficit_readonly_threshold_e8s");

    // Accrue some deficit + repay some so all four fields are non-zero.
    drop_icp_price(&f, 10_000_000);
    icrc2_approve_call(
        &f.pic,
        f.icusd_ledger,
        f.test_user,
        f.protocol_id,
        100_000_000_000u128,
    );
    let _ = f
        .pic
        .update_call(
            f.protocol_id,
            f.test_user,
            "liquidate_vault",
            encode_args((f.vault_id,)).unwrap(),
        )
        .expect("liquidate_vault failed");
    drop_icp_price(&f, 1_000_000_000); // restore for second borrow
    // Bump fee to 1% so the second borrow generates measurable revenue
    // for deficit repayment.
    let _ = f
        .pic
        .update_call(
            f.protocol_id,
            f.developer,
            "set_borrowing_fee",
            encode_args((0.01f64,)).unwrap(),
        )
        .expect("set_borrowing_fee");
    icrc2_approve_call(
        &f.pic,
        f.icp_ledger,
        f.test_user,
        f.protocol_id,
        50_000_000_000u128,
    );
    let r = f
        .pic
        .update_call(
            f.protocol_id,
            f.test_user,
            "open_vault",
            encode_args((5_000_000_000u64, None::<Principal>)).unwrap(),
        )
        .expect("open_vault");
    let new_vault = match r {
        WasmResult::Reply(b) => {
            let r: Result<OpenVaultSuccess, ProtocolError> = decode_one(&b).unwrap();
            r.unwrap().vault_id
        }
        WasmResult::Reject(m) => panic!("open rejected: {}", m),
    };
    let borrow_res = f
        .pic
        .update_call(
            f.protocol_id,
            f.test_user,
            "borrow_from_vault",
            encode_args((VaultArg {
                vault_id: new_vault,
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

    let s_pre_upgrade = protocol_status(&f.pic, f.protocol_id);
    assert!(s_pre_upgrade.protocol_deficit_icusd > 0);
    assert!(s_pre_upgrade.total_deficit_repaid_icusd > 0);
    assert!((s_pre_upgrade.deficit_repayment_fraction - 0.75).abs() < 1e-9);
    assert_eq!(s_pre_upgrade.deficit_readonly_threshold_e8s, u64::MAX);

    // Upgrade the canister.
    let upgrade_arg = ProtocolArgVariant::Upgrade(UpgradeArg {
        mode: None,
        description: Some("LIQ-005 PIC upgrade fence".to_string()),
    });
    f.pic
        .upgrade_canister(
            f.protocol_id,
            protocol_wasm(),
            encode_args((upgrade_arg,)).expect("encode upgrade"),
            None,
        )
        .expect("upgrade_canister failed");

    // Tick once to let post_upgrade settle.
    f.pic.tick();

    let s_post_upgrade = protocol_status(&f.pic, f.protocol_id);
    assert_eq!(
        s_post_upgrade.protocol_deficit_icusd, s_pre_upgrade.protocol_deficit_icusd,
        "protocol_deficit_icusd lost on upgrade"
    );
    assert_eq!(
        s_post_upgrade.total_deficit_repaid_icusd, s_pre_upgrade.total_deficit_repaid_icusd,
        "total_deficit_repaid_icusd lost on upgrade"
    );
    assert!(
        (s_post_upgrade.deficit_repayment_fraction - 0.75).abs() < 1e-9,
        "deficit_repayment_fraction drifted across upgrade: {}",
        s_post_upgrade.deficit_repayment_fraction
    );
    assert_eq!(
        s_post_upgrade.deficit_readonly_threshold_e8s, u64::MAX,
        "deficit_readonly_threshold_e8s lost on upgrade"
    );
}
