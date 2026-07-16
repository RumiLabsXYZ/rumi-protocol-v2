//! Regression fence (2026-07-16): the stability pool must pay the ckStable
//! ledger fee only as many times as it actually gets charged when it absorbs
//! a liquidation, and its tracked aggregate (`total_stablecoin_balances`)
//! must stay EXACTLY equal to the live `icrc1_balance_of(pool)` afterwards.
//!
//! # The bug this guards
//!
//! When the stability pool (SP) absorbs a liquidation using a nonzero-fee
//! stablecoin (ckUSDT/ckUSDC) via the non-LP path in
//! `stability_pool/src/liquidation.rs`, the pool's tokens are moved off its
//! own ledger account TWICE:
//!
//!   1. `icrc2_approve` (SP -> backend spender) — charges the SP one ledger
//!      fee immediately, regardless of whether the backend ever pulls.
//!   2. The backend's `icrc2_transfer_from` (pulling the approved tokens
//!      into the backend) — charges the SP a SECOND ledger fee, standard
//!      ICRC-2 behavior (the `from` account always pays the transfer fee).
//!
//! Pre-fix, the SP's bookkeeping (`deduct_fee_from_pool`) only fired once,
//! after the approve. So on every ckStable liquidation the tracked aggregate
//! stayed one ledger fee ABOVE the pool's real on-ledger balance. That drift
//! accumulates liquidation after liquidation and eventually trips the
//! withdraw guard for non-sole-holders (insufficient live balance to honor
//! the tracked aggregate).
//!
//! The fix (already applied, not touched by this test) calls
//! `deduct_fee_from_pool` a SECOND time, using the live `icrc1_fee`, right
//! after a successful pull (see `liquidation.rs`, the non-LP loop: approve ->
//! deduct fee -> backend call -> on success, deduct fee again).
//!
//! # What this test does
//!
//! Deploys the full three-canister stack (backend + stability_pool + a
//! nonzero-fee ckUSDT-style ICRC-1/2 ledger, plus ICP/icUSD ledgers and a
//! mock XRC), registers ckUSDT as an accepted SP stablecoin AND as a backend
//! stable-repayment token, deposits ckUSDT into the SP from a depositor,
//! opens and deeply underwaters an ICP vault, then drives the SP's real
//! `execute_liquidation` (the permissionless entry point used in production)
//! so it actually calls `icrc2_approve` + the backend's
//! `liquidate_vault_partial_with_stable` (which does `icrc2_transfer_from`)
//! on the live ckUSDT ledger — the exact double-fee code path.
//!
//! Asserts, after the liquidation:
//!   * `icrc1_balance_of(ckUSDT, sp_id) == get_pool_status(sp_id).stablecoin_balances[ckUSDT]`
//!     EXACTLY (no one-fee drift).
//!   * the live balance and the aggregate both dropped by exactly
//!     `principal_consumed + 2 * ledger_fee` (the two fee charges the SP
//!     books, plus the realized principal the backend actually pulled).
//!
//! Harness lifted from `audit_pocs_icc_002_3usd_refund.rs` (backend + ICP/
//! icUSD ledger + mock-XRC deploy pattern) and
//! `audit_pocs_bk_001_002_concurrent_liquidation_pic.rs` (the ICP
//! price-drop-and-wait pattern needed to make a vault actually liquidatable
//! through the CR-gated `liquidate_vault_partial_with_stable` path — unlike
//! the LP/reserves path used by the ICC-002 fixture, this path DOES gate on
//! CR). `stability_pool`'s own `pocket_ic_3usd.rs` supplied the SP
//! deployment/registration/deposit/pool-status call shapes; `StablecoinConfig`
//! / `StabilityPoolInitArgs` / `StabilityPoolError` / `LiquidationResult` are
//! mirrored locally here (matching `stability_pool/src/types.rs`) since
//! `stability_pool` is not a dependency of this crate and this file must not
//! touch non-test source.

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use rumi_protocol_backend::{ProtocolError, StableTokenType, SuccessWithFee};

// ─── ic-icrc1-ledger Candid mirrors (standard ledger; matches the ICC-002 template) ───

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

#[derive(CandidType, Deserialize, Clone, Debug)]
enum LedgerArg {
    #[serde(rename = "Init")]
    Init(LedgerInitArgs),
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

// ─── Backend init / vault Candid mirrors ───

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
struct LiquidatableVault {
    vault_id: u64,
}

// ─── stability_pool Candid mirrors (matches `stability_pool/src/types.rs`) ───

#[derive(CandidType, Deserialize, Clone, Debug)]
struct StabilityPoolInitArgs {
    protocol_canister_id: Principal,
    authorized_admins: Vec<Principal>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct StablecoinConfig {
    ledger_id: Principal,
    symbol: String,
    decimals: u8,
    priority: u8,
    is_active: bool,
    transfer_fee: Option<u64>,
    is_lp_token: Option<bool>,
    underlying_pool: Option<Principal>,
}

/// Only the field this test needs from `StabilityPoolStatus`. Candid record
/// decoding matches by field name/hash, not position or completeness — extra
/// wire fields we don't declare here (total_deposits_e8s, stablecoin_registry,
/// collateral_registry, etc.) are simply skipped.
#[derive(CandidType, Deserialize, Clone, Debug)]
struct StabilityPoolStatusView {
    stablecoin_balances: BTreeMap<Principal, u64>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct LiquidationResult {
    vault_id: u64,
    stables_consumed: BTreeMap<Principal, u64>,
    collateral_gained: u64,
    collateral_type: Principal,
    success: bool,
    error_message: Option<String>,
}

/// Full mirror of `stability_pool::types::StabilityPoolError` so `.expect()`
/// on a `Result<_, StabilityPoolError>` decode succeeds and prints a useful
/// message on the (unexpected) error path.
#[derive(CandidType, Deserialize, Clone, Debug)]
enum StabilityPoolError {
    InsufficientBalance {
        token: Principal,
        required: u64,
        available: u64,
    },
    AmountTooLow {
        minimum_e8s: u64,
    },
    NoPositionFound,
    InsufficientPoolBalance,
    Unauthorized,
    TokenNotAccepted {
        ledger: Principal,
    },
    TokenNotActive {
        ledger: Principal,
    },
    CollateralNotFound {
        ledger: Principal,
    },
    LedgerTransferFailed {
        reason: String,
    },
    InterCanisterCallFailed {
        target: String,
        method: String,
    },
    LiquidationFailed {
        vault_id: u64,
        reason: String,
    },
    EmergencyPaused,
    SystemBusy,
    AlreadyOptedOut {
        collateral: Principal,
    },
    AlreadyOptedIn {
        collateral: Principal,
    },
    PayoutAddressRequired {
        collateral: Principal,
    },
    InvalidPayoutAddress {
        reason: String,
    },
    XrpClaimStillOutstanding {
        claim_id: u64,
    },
    XrpClaimStatusCheckFailed {
        reason: String,
    },
    RefundClaimNotFound,
}

// ─── WASM fixtures ───

fn icrc1_ledger_wasm() -> Vec<u8> {
    include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec()
}

fn protocol_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm")
        .to_vec()
}

fn stability_pool_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/stability_pool.wasm").to_vec()
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
        rates: vec![("ICP/USD".to_string(), 1_000_000_000)], // $10.00 (e8s, 8 decimals)
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
    let init = LedgerInitArgs {
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

/// Same as `deploy_icrc1_ledger` but with an explicit `decimals` (the ICP/
/// icUSD ledgers above are hardcoded to 8; ckUSDT-style ledgers are 6).
fn deploy_icrc1_ledger_with_decimals(
    pic: &PocketIc,
    minting_account: Account,
    transfer_fee: u64,
    decimals: u8,
    initial_balances: Vec<(Account, Nat)>,
    name: &str,
    symbol: &str,
    controller: Principal,
) -> Principal {
    let ledger_id = pic.create_canister();
    pic.add_cycles(ledger_id, 2_000_000_000_000);
    let init = LedgerInitArgs {
        minting_account,
        fee_collector_account: None,
        transfer_fee: Nat::from(transfer_fee),
        decimals: Some(decimals),
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

fn open_and_borrow(
    pic: &PocketIc,
    protocol_id: Principal,
    icp_ledger: Principal,
    user: Principal,
    collateral_e8s: u64,
    borrow_e8s: u64,
) -> u64 {
    icrc2_approve_call(pic, icp_ledger, user, protocol_id, collateral_e8s as u128);
    let open_result = pic
        .update_call(
            protocol_id,
            user,
            "open_vault",
            encode_args((collateral_e8s, None::<Principal>)).unwrap(),
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
    let borrow_result = pic
        .update_call(
            protocol_id,
            user,
            "borrow_from_vault",
            encode_args((VaultArg {
                vault_id,
                amount: borrow_e8s,
            },))
            .unwrap(),
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
    vault_id
}

fn register_stablecoin(
    pic: &PocketIc,
    sp_id: Principal,
    admin: Principal,
    config: StablecoinConfig,
) {
    let symbol = config.symbol.clone();
    let result = pic
        .update_call(
            sp_id,
            admin,
            "register_stablecoin",
            encode_one(config).unwrap(),
        )
        .expect("register_stablecoin call failed");
    match result {
        WasmResult::Reply(bytes) => {
            let r: Result<(), StabilityPoolError> =
                decode_one(&bytes).expect("decode register_stablecoin");
            r.unwrap_or_else(|e| panic!("register_stablecoin failed for {}: {:?}", symbol, e));
        }
        WasmResult::Reject(msg) => panic!("register_stablecoin rejected: {}", msg),
    }
}

fn sp_deposit(pic: &PocketIc, sp_id: Principal, depositor: Principal, ledger: Principal, amount: u64) {
    let result = pic
        .update_call(
            sp_id,
            depositor,
            "deposit",
            encode_args((ledger, amount)).unwrap(),
        )
        .expect("deposit call failed");
    match result {
        WasmResult::Reply(bytes) => {
            let r: Result<(), StabilityPoolError> = decode_one(&bytes).expect("decode deposit");
            r.expect("deposit failed");
        }
        WasmResult::Reject(msg) => panic!("deposit rejected: {}", msg),
    }
}

fn get_pool_status(pic: &PocketIc, sp_id: Principal) -> StabilityPoolStatusView {
    let result = pic
        .query_call(
            sp_id,
            Principal::anonymous(),
            "get_pool_status",
            encode_args(()).unwrap(),
        )
        .expect("get_pool_status call failed");
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode pool status"),
        WasmResult::Reject(msg) => panic!("get_pool_status rejected: {}", msg),
    }
}

fn execute_liquidation(
    pic: &PocketIc,
    sp_id: Principal,
    caller: Principal,
    vault_id: u64,
) -> Result<LiquidationResult, StabilityPoolError> {
    let result = pic
        .update_call(
            sp_id,
            caller,
            "execute_liquidation",
            encode_args((vault_id,)).unwrap(),
        )
        .expect("execute_liquidation call failed");
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode execute_liquidation"),
        WasmResult::Reject(msg) => panic!("execute_liquidation rejected: {}", msg),
    }
}

// ─── Test ───

/// Full end-to-end fence: the SP's `execute_liquidation` pulls a nonzero-fee
/// ckUSDT-style stablecoin from itself via the approve + backend
/// transfer_from path (the exact double-fee code path this bug lived in),
/// and afterwards the SP's live ckUSDT ledger balance must equal its tracked
/// `total_stablecoin_balances` aggregate EXACTLY.
#[test]
fn sp_ckstable_liquidation_keeps_live_balance_equal_to_aggregate() {
    let pic = PocketIcBuilder::new().with_nns_subnet().build();

    let victim = Principal::self_authenticating(b"sp_fee_acct_victim");
    let developer = Principal::self_authenticating(b"sp_fee_acct_developer");
    let sp_admin = Principal::self_authenticating(b"sp_fee_acct_sp_admin");
    let depositor = Principal::self_authenticating(b"sp_fee_acct_depositor");
    let keeper = Principal::self_authenticating(b"sp_fee_acct_keeper");

    // ── Backend canister ──
    let protocol_id = pic.create_canister();
    pic.add_cycles(protocol_id, 2_000_000_000_000);
    pic.set_controllers(protocol_id, None, vec![Principal::anonymous(), developer])
        .expect("set_controllers failed");

    let icp_ledger = deploy_icrc1_ledger(
        &pic,
        account(protocol_id),
        10_000,
        vec![(account(victim), Nat::from(1_000_000_000_000u64))],
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

    // Quiet down dynamic curves so the liquidation math stays predictable
    // (mirrors the BK-001/002 concurrent-liquidation fixture).
    for (method, payload) in [
        (
            "set_borrowing_fee_curve",
            encode_args((None::<String>,)).unwrap(),
        ),
        ("set_borrowing_fee", encode_args((0.0f64,)).unwrap()),
    ] {
        pic.update_call(protocol_id, developer, method, payload)
            .expect(method);
    }
    pic.update_call(
        protocol_id,
        developer,
        "set_rate_curve_markers",
        encode_args((None::<Principal>, vec![(1.5f64, 1.0f64), (3.0f64, 1.0f64)])).unwrap(),
    )
    .expect("set_rate_curve_markers");
    pic.update_call(
        protocol_id,
        developer,
        "set_interest_rate",
        encode_args((icp_ledger, 0.0f64)).unwrap(),
    )
    .expect("set_interest_rate");

    // ── ckUSDT-style nonzero-fee ledger (the SP consumes THIS token) ──
    let ckusdt_fee = 10_000u64; // 0.01 ckUSDT at 6 decimals
    let depositor_initial_balance = 2_000_000_000u64; // 2,000 ckUSDT
    let ckusdt_ledger = deploy_icrc1_ledger_with_decimals(
        &pic,
        account(developer), // minting account (unused as a real spender here)
        ckusdt_fee,
        6,
        vec![(account(depositor), Nat::from(depositor_initial_balance))],
        "ckUSDT",
        "ckUSDT",
        developer,
    );

    // Wire ckUSDT into the backend as an accepted stable-repayment/liquidation token.
    pic.update_call(
        protocol_id,
        developer,
        "set_stable_token_enabled",
        encode_args((StableTokenType::CKUSDT, true)).unwrap(),
    )
    .expect("set_stable_token_enabled call failed");
    pic.update_call(
        protocol_id,
        developer,
        "set_stable_ledger_principal",
        encode_args((StableTokenType::CKUSDT, ckusdt_ledger)).unwrap(),
    )
    .expect("set_stable_ledger_principal call failed");

    // ── Stability pool canister ──
    let sp_init = StabilityPoolInitArgs {
        protocol_canister_id: protocol_id,
        authorized_admins: vec![sp_admin],
    };
    let sp_id = pic.create_canister();
    pic.add_cycles(sp_id, 2_000_000_000_000);
    pic.install_canister(
        sp_id,
        stability_pool_wasm(),
        encode_one(sp_init).unwrap(),
        None,
    );

    pic.update_call(
        protocol_id,
        developer,
        "set_stability_pool_principal",
        encode_args((sp_id,)).unwrap(),
    )
    .expect("set_stability_pool_principal call failed");

    register_stablecoin(
        &pic,
        sp_id,
        sp_admin,
        StablecoinConfig {
            ledger_id: ckusdt_ledger,
            symbol: "ckUSDT".to_string(),
            decimals: 6,
            priority: 2,
            is_active: true,
            transfer_fee: Some(ckusdt_fee),
            is_lp_token: None,
            underlying_pool: None,
        },
    );

    // Depositor funds the SP with ckUSDT (this is the balance the liquidation
    // will draw from).
    let deposit_amount = 500_000_000u64; // 500 ckUSDT ($500 equivalent)
    icrc2_approve_call(
        &pic,
        ckusdt_ledger,
        depositor,
        sp_id,
        depositor_initial_balance as u128,
    );
    sp_deposit(&pic, sp_id, depositor, ckusdt_ledger, deposit_amount);

    // Pre-liquidation sanity: live ledger balance and tracked aggregate agree
    // exactly (deposit credits the pool for the full pulled amount; the
    // depositor pays their own approve/transfer_from fees out of their own
    // balance, not out of the deposited amount).
    let live_before = icrc1_balance_of(&pic, ckusdt_ledger, account(sp_id));
    let agg_before = get_pool_status(&pic, sp_id)
        .stablecoin_balances
        .get(&ckusdt_ledger)
        .copied()
        .unwrap_or(0);
    assert_eq!(
        live_before, deposit_amount as u128,
        "SP's live ckUSDT balance must equal the deposited amount"
    );
    assert_eq!(
        agg_before, deposit_amount,
        "SP's tracked ckUSDT aggregate must equal the deposited amount"
    );
    assert_eq!(
        live_before, agg_before as u128,
        "precondition: live balance and tracked aggregate must already agree \
         before the liquidation under test"
    );

    // XRC needs a USDT/USD rate for the backend's depeg check
    // (`ensure_stable_not_depegged`) inside `liquidate_vault_partial_with_stable`.
    xrc_set_rate(&pic, xrc_id, developer, "USDT", "USD", 100_000_000); // $1.00

    // ── Open and deeply underwater a vault: 50 ICP / 100 icUSD at $10/ICP,
    // then crash ICP to $0.10 (mirrors BK-001/002's proven price-drop fixture). ──
    let collateral_e8s = 5_000_000_000u64; // 50 ICP
    let borrow_e8s = 10_000_000_000u64; // 100 icUSD
    let vault_id = open_and_borrow(
        &pic,
        protocol_id,
        icp_ledger,
        victim,
        collateral_e8s,
        borrow_e8s,
    );

    xrc_set_rate(&pic, xrc_id, developer, "ICP", "USD", 10_000_000); // $0.10
    let mut liquidatable = false;
    for _ in 0..12 {
        pic.advance_time(Duration::from_secs(310));
        for _ in 0..6 {
            pic.tick();
        }
        let result = pic
            .query_call(
                protocol_id,
                Principal::anonymous(),
                "get_liquidatable_vaults",
                encode_args(()).unwrap(),
            )
            .expect("get_liquidatable_vaults");
        if let WasmResult::Reply(b) = result {
            if let Ok(vaults) = decode_one::<Vec<LiquidatableVault>>(&b) {
                if vaults.iter().any(|v| v.vault_id == vault_id) {
                    liquidatable = true;
                    break;
                }
            }
        }
    }
    assert!(
        liquidatable,
        "fixture precondition: vault must be liquidatable (cached ICP price \
         propagated to $0.10) before driving the SP's execute_liquidation"
    );

    // ── Drive the SP's real liquidation path: this is what actually exercises
    // icrc2_approve + the backend's icrc2_transfer_from double-fee code path. ──
    let live_fee = {
        let result = pic
            .query_call(
                ckusdt_ledger,
                Principal::anonymous(),
                "icrc1_fee",
                encode_args(()).unwrap(),
            )
            .expect("icrc1_fee call failed");
        let fee: Nat = match result {
            WasmResult::Reply(b) => decode_one(&b).expect("decode fee"),
            WasmResult::Reject(m) => panic!("icrc1_fee rejected: {}", m),
        };
        let fee: u64 = fee.0.try_into().expect("fee overflow");
        assert_eq!(fee, ckusdt_fee, "sanity: live ledger fee matches init arg");
        fee
    };

    let liq = execute_liquidation(&pic, sp_id, keeper, vault_id)
        .expect("execute_liquidation must succeed against a liquidatable vault");
    assert!(
        liq.success,
        "liquidation must report success; error_message={:?}",
        liq.error_message
    );
    assert_eq!(liq.vault_id, vault_id);
    assert!(
        liq.collateral_gained > 0,
        "SP must have received ICP collateral from the liquidation"
    );

    let principal_consumed = *liq
        .stables_consumed
        .get(&ckusdt_ledger)
        .expect("ckUSDT must be the token consumed by this liquidation (only registered token)");
    assert!(
        principal_consumed > 0,
        "the liquidation must have actually consumed ckUSDT principal"
    );

    // ── THE INVARIANT: live balance and tracked aggregate must be EXACTLY equal ──
    let live_after = icrc1_balance_of(&pic, ckusdt_ledger, account(sp_id));
    let pool_status_after = get_pool_status(&pic, sp_id);
    let agg_after = pool_status_after
        .stablecoin_balances
        .get(&ckusdt_ledger)
        .copied()
        .unwrap_or(0);

    assert_eq!(
        live_after, agg_after as u128,
        "BUG REGRESSION: SP's live ckUSDT ledger balance ({}) must equal its \
         tracked aggregate ({}) after absorbing a nonzero-fee-stablecoin \
         liquidation. A live < aggregate drift means the pool paid the ledger \
         fee more times than it booked (the double-fee bug); a live > aggregate \
         drift means it booked more than it paid.",
        live_after, agg_after
    );

    // ── Exact accounting: both sides must have dropped by precisely
    // principal_consumed + 2 * ledger_fee (one fee on the SP's icrc2_approve,
    // a second on the backend's icrc2_transfer_from pull). This is the
    // "no off-by-one-fee" check — the fix debits the SECOND fee exactly once,
    // not zero times (pre-fix bug) and not twice (an over-correction bug). ──
    let expected_drop = principal_consumed + 2 * live_fee;
    let live_drop = live_before - live_after;
    let agg_drop = (agg_before - agg_after) as u128;

    assert_eq!(
        live_drop, expected_drop as u128,
        "live ckUSDT balance must drop by exactly principal_consumed ({}) + \
         2 * ledger_fee ({} x2); saw drop={}",
        principal_consumed, live_fee, live_drop
    );
    assert_eq!(
        agg_drop, expected_drop as u128,
        "tracked ckUSDT aggregate must drop by exactly principal_consumed ({}) + \
         2 * ledger_fee ({} x2); saw drop={}",
        principal_consumed, live_fee, agg_drop
    );
}
