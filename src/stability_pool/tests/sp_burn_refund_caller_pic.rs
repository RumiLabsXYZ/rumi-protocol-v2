//! Composed Stability Pool caller-side recovery test.
//!
//! Runs the production Stability Pool Wasm against the official ICRC-1 ledger
//! and a narrowly scoped mock protocol canister. The mock approves an XRP
//! reservation, then rejects absorption after the SP has committed a real
//! burn, and returns a receipt for a real compensating ledger mint. The SP
//! must verify that mint independently, clear the exact pending intent, and
//! leave depositor accounting and XRP claims unchanged.

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use icrc_ledger_types::icrc3::transactions::{GetTransactionsRequest, GetTransactionsResponse};
use pocket_ic::common::rest::{CanisterHttpReply, CanisterHttpResponse, MockCanisterHttpResponse};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use rumi_protocol_backend::sp_burn_refund::SpBurnRefundReceipt;
use rumi_protocol_backend::state::xrp_collateral_principal;
use stability_pool::types::{
    CollateralInfo, CollateralStatus, LiquidatableVaultInfo, LiquidationResult,
    NativeXrpPendingPayout, StabilityPoolError, StabilityPoolInitArgs, StabilityPoolStatus,
    StablecoinConfig, UserStabilityPosition,
};
use std::path::PathBuf;
use std::time::Duration;

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

fn account(owner: Principal) -> Account {
    Account {
        owner,
        subaccount: None,
    }
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct MockBackendInit {
    ledger: Principal,
    stability_pool: Principal,
    wrong_first_receipt: bool,
    fail_first_refund_before_mint: bool,
    fail_first_refund_after_mint: bool,
    trap_first_absorb_reply: bool,
    status_reports_accepted: bool,
}

fn stability_pool_wasm() -> Vec<u8> {
    let mut path = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target"));
    path.push("wasm32-unknown-unknown/release/stability_pool.wasm");
    std::fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "read Stability Pool Wasm at {}: {error}; build stability_pool for wasm32-unknown-unknown first",
            path.display()
        )
    })
}

fn official_ledger_wasm() -> Vec<u8> {
    include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec()
}

fn flaky_ledger_wasm() -> Vec<u8> {
    let mut path = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/rumi-security-target"));
    path.push("wasm32-unknown-unknown/release/flaky_ledger.wasm");
    std::fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "read test-only flaky ledger Wasm at {}: {error}",
            path.display()
        )
    })
}

fn mock_backend_wasm() -> Vec<u8> {
    let mut path = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/rumi-security-target"));
    path.push("wasm32-unknown-unknown/release/sp_burn_refund_mock_backend.wasm");
    std::fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "read test-only mock backend Wasm at {}: {error}",
            path.display()
        )
    })
}

fn install_ledger(
    pic: &PocketIc,
    ledger: Principal,
    minter: Principal,
    user: Principal,
    controller: Principal,
) {
    let init = LedgerInitArgs {
        minting_account: account(minter),
        fee_collector_account: None,
        transfer_fee: Nat::from(10_000u64),
        decimals: Some(8),
        max_memo_length: Some(64),
        token_name: "icUSD".into(),
        token_symbol: "icUSD".into(),
        metadata: vec![],
        initial_balances: vec![(account(user), Nat::from(1_000_000_000u64))],
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
    pic.add_cycles(ledger, 2_000_000_000_000);
    pic.install_canister(
        ledger,
        official_ledger_wasm(),
        encode_args((LedgerArg::Init(init),)).expect("encode ledger init"),
        None,
    );
}

fn call_reply<T>(
    pic: &PocketIc,
    canister: Principal,
    caller: Principal,
    method: &str,
    args: Vec<u8>,
) -> T
where
    T: for<'de> Deserialize<'de> + CandidType,
{
    let result = pic
        .update_call(canister, caller, method, args)
        .unwrap_or_else(|error| panic!("{method} call: {error}"));
    match result {
        WasmResult::Reply(bytes) => {
            decode_one(&bytes).unwrap_or_else(|error| panic!("decode {method}: {error}"))
        }
        WasmResult::Reject(message) => panic!("{method} rejected: {message}"),
    }
}

fn query_reply<T>(
    pic: &PocketIc,
    canister: Principal,
    caller: Principal,
    method: &str,
    args: Vec<u8>,
) -> T
where
    T: for<'de> Deserialize<'de> + CandidType,
{
    let result = pic
        .query_call(canister, caller, method, args)
        .unwrap_or_else(|error| panic!("{method} query: {error}"));
    match result {
        WasmResult::Reply(bytes) => {
            decode_one(&bytes).unwrap_or_else(|error| panic!("decode {method}: {error}"))
        }
        WasmResult::Reject(message) => panic!("{method} query rejected: {message}"),
    }
}

fn expect_sp_ok<T: std::fmt::Debug>(result: Result<T, StabilityPoolError>, label: &str) -> T {
    result.unwrap_or_else(|error| panic!("{label} returned {error:?}"))
}

fn ledger_balance(pic: &PocketIc, ledger: Principal, owner: Principal) -> u64 {
    let balance: Nat = query_reply(
        pic,
        ledger,
        Principal::anonymous(),
        "icrc1_balance_of",
        encode_one(account(owner)).expect("encode account"),
    );
    balance.0.try_into().expect("balance fits u64")
}

fn ledger_transactions(
    pic: &PocketIc,
    ledger: Principal,
    start: u64,
    length: u64,
) -> GetTransactionsResponse {
    query_reply(
        pic,
        ledger,
        Principal::anonymous(),
        "get_transactions",
        encode_one(GetTransactionsRequest {
            start: Nat::from(start),
            length: Nat::from(length),
        })
        .expect("encode get_transactions"),
    )
}

#[test]
fn identical_same_round_deposits_get_distinct_ledger_blocks_and_remain_withdrawable() {
    let pic = PocketIcBuilder::new().with_nns_subnet().build();
    let admin = Principal::self_authenticating(b"sp-same-round-admin");
    let user = Principal::self_authenticating(b"sp-same-round-depositor");
    let protocol = Principal::self_authenticating(b"sp-same-round-protocol");
    let sp = pic.create_canister();
    let ledger = pic.create_canister();
    pic.add_cycles(sp, 2_000_000_000_000);
    install_ledger(&pic, ledger, admin, user, admin);
    pic.install_canister(
        sp,
        stability_pool_wasm(),
        encode_one(StabilityPoolInitArgs {
            protocol_canister_id: protocol,
            authorized_admins: vec![admin],
        })
        .expect("encode SP init"),
        None,
    );

    expect_sp_ok(
        call_reply::<Result<(), StabilityPoolError>>(
            &pic,
            sp,
            admin,
            "register_stablecoin",
            encode_one(StablecoinConfig {
                ledger_id: ledger,
                symbol: "icUSD".into(),
                decimals: 8,
                priority: 1,
                is_active: true,
                transfer_fee: Some(10_000),
                is_lp_token: Some(false),
                underlying_pool: None,
            })
            .expect("encode stablecoin registration"),
        ),
        "register_stablecoin",
    );

    let deposit_amount = 100_000_000u64;
    let initial_user_balance = ledger_balance(&pic, ledger, user);
    let approval: Result<Nat, ApproveError> = call_reply(
        &pic,
        ledger,
        user,
        "icrc2_approve",
        encode_one(ApproveArgs {
            from_subaccount: None,
            spender: account(sp),
            amount: Nat::from(deposit_amount * 2 + 20_000),
            expected_allowance: None,
            expires_at: None,
            fee: None,
            memo: None,
            created_at_time: None,
        })
        .expect("encode ledger approval"),
    );
    approval.expect("approve both deposits and their transfer fees");

    let before = ledger_transactions(&pic, ledger, 0, 0);
    let before_log_length: u64 = before.log_length.0.try_into().expect("log length fits u64");
    let deposit_time = pic.get_time() + Duration::from_secs(1);
    pic.set_time(deposit_time);
    let time_ns = deposit_time
        .duration_since(std::time::UNIX_EPOCH)
        .expect("PocketIC time is after UNIX epoch")
        .as_nanos() as u64;

    for attempt in 0..2 {
        // PocketIC advances system time by one nanosecond after each update.
        // Reset it so both calls enter the canister at the same IC timestamp;
        // the SP's persisted ledger timestamp still makes transfer identities unique.
        pic.set_time(deposit_time);
        assert_eq!(pic.get_time(), deposit_time);
        expect_sp_ok(
            call_reply::<Result<(), StabilityPoolError>>(
                &pic,
                sp,
                user,
                "deposit",
                encode_args((ledger, deposit_amount)).expect("encode SP deposit"),
            ),
            &format!("same-round deposit {attempt}"),
        );
    }

    let after = ledger_transactions(&pic, ledger, before_log_length, 2);
    let end_log_length: u64 = after.log_length.0.try_into().expect("log length fits u64");
    assert_eq!(end_log_length, before_log_length + 2, "one transfer block per deposit");
    assert_eq!(after.first_index, Nat::from(before_log_length));
    assert_eq!(after.transactions.len(), 2, "both blocks must be present");
    let created_at_times: Vec<u64> = after
        .transactions
        .iter()
        .map(|transaction| {
            let transfer = transaction.transfer.as_ref().expect("deposit block is a transfer");
            assert_eq!(transfer.from.owner, user);
            assert_eq!(transfer.to.owner, sp);
            assert_eq!(transfer.amount, Nat::from(deposit_amount));
            transfer
                .created_at_time
                .expect("ledger transaction retains transfer_from identity")
        })
        .collect();
    assert_eq!(created_at_times, vec![time_ns, time_ns + 1]);
    let block_ids = [before_log_length, before_log_length + 1];
    assert_ne!(block_ids[0], block_ids[1], "transfers have distinct ledger block IDs");

    let credited = deposit_amount * 2;
    let position: Option<UserStabilityPosition> = query_reply(
        &pic,
        sp,
        user,
        "get_user_position",
        encode_one(Some(user)).expect("encode user position query"),
    );
    let position = position.expect("depositor position is present");
    let recorded = position
        .stablecoin_balances
        .iter()
        .find(|(token, _)| **token == ledger)
        .map(|(_, amount)| *amount)
        .unwrap_or(0);
    assert_eq!(recorded, credited, "each distinct transfer credits exactly once");
    let status: StabilityPoolStatus = query_reply(
        &pic,
        sp,
        Principal::anonymous(),
        "get_pool_status",
        encode_args(()).expect("encode status query"),
    );
    assert_eq!(status.total_deposits_e8s, credited);
    assert_eq!(ledger_balance(&pic, ledger, sp), credited);

    expect_sp_ok(
        call_reply::<Result<(), StabilityPoolError>>(
            &pic,
            sp,
            user,
            "withdraw",
            encode_args((ledger, credited)).expect("encode SP withdrawal"),
        ),
        "withdraw both deposits",
    );
    assert_eq!(ledger_balance(&pic, ledger, sp), 0);
    let final_status: StabilityPoolStatus = query_reply(
        &pic,
        sp,
        Principal::anonymous(),
        "get_pool_status",
        encode_args(()).expect("encode final status query"),
    );
    assert_eq!(final_status.total_deposits_e8s, 0);
    assert!(ledger_balance(&pic, ledger, user) > initial_user_balance - credited);
}

fn run_native_xrp_refund_case(
    wrong_first_receipt: bool,
    use_history_reconcile: bool,
    use_lost_reply_scan: bool,
    retry_unpaid_refund: bool,
    retry_after_post_mint_error: bool,
    recover_via_timer: bool,
    simulate_ambiguous_absorb: bool,
) {
    let pic = PocketIcBuilder::new().with_nns_subnet().build();
    let admin = Principal::self_authenticating(b"sp-refund-admin");
    let user = Principal::self_authenticating(b"sp-refund-depositor");
    let mock_backend = pic.create_canister();
    let sp = pic.create_canister();
    let ledger = pic.create_canister();
    pic.add_cycles(mock_backend, 2_000_000_000_000);
    pic.add_cycles(sp, 2_000_000_000_000);
    if use_lost_reply_scan {
        pic.install_canister(ledger, flaky_ledger_wasm(), encode_one(()).unwrap(), None);
        call_reply::<()>(
            &pic,
            ledger,
            admin,
            "set_minting_account",
            encode_one(Some(account(mock_backend))).unwrap(),
        );
        call_reply::<()>(
            &pic,
            ledger,
            admin,
            "mint",
            encode_args((account(user), Nat::from(1_000_000_000u64))).unwrap(),
        );
    } else {
        install_ledger(&pic, ledger, mock_backend, user, admin);
    }

    let mock_init = MockBackendInit {
        ledger,
        stability_pool: sp,
        wrong_first_receipt,
        fail_first_refund_before_mint: retry_unpaid_refund,
        fail_first_refund_after_mint: retry_after_post_mint_error,
        trap_first_absorb_reply: simulate_ambiguous_absorb,
        status_reports_accepted: simulate_ambiguous_absorb,
    };
    pic.install_canister(
        mock_backend,
        mock_backend_wasm(),
        encode_one(mock_init).expect("encode mock protocol init"),
        None,
    );
    let init = StabilityPoolInitArgs {
        protocol_canister_id: mock_backend,
        authorized_admins: vec![admin],
    };
    pic.install_canister(
        sp,
        stability_pool_wasm(),
        encode_one(init.clone()).expect("encode SP init"),
        None,
    );

    expect_sp_ok(
        call_reply::<Result<(), StabilityPoolError>>(
            &pic,
            sp,
            admin,
            "register_stablecoin",
            encode_one(StablecoinConfig {
                ledger_id: ledger,
                symbol: "icUSD".into(),
                decimals: 8,
                priority: 1,
                is_active: true,
                transfer_fee: Some(10_000),
                is_lp_token: Some(false),
                underlying_pool: None,
            })
            .expect("encode stablecoin registration"),
        ),
        "register_stablecoin",
    );
    let xrp = xrp_collateral_principal();
    expect_sp_ok(
        call_reply::<Result<(), StabilityPoolError>>(
            &pic,
            sp,
            admin,
            "register_collateral",
            encode_one(CollateralInfo {
                ledger_id: xrp,
                symbol: "XRP".into(),
                decimals: 6,
                status: CollateralStatus::Active,
            })
            .expect("encode XRP collateral"),
        ),
        "register_collateral",
    );

    let deposited = 500_000_000u64;
    let approve = ApproveArgs {
        from_subaccount: None,
        spender: account(sp),
        amount: Nat::from(deposited + 10_000),
        expected_allowance: None,
        expires_at: None,
        fee: None,
        memo: None,
        created_at_time: None,
    };
    let approved: Result<Nat, ApproveError> = call_reply(
        &pic,
        ledger,
        user,
        "icrc2_approve",
        encode_one(approve).expect("encode ledger approve"),
    );
    approved.expect("approve SP to take test deposit");
    expect_sp_ok(
        call_reply::<Result<(), StabilityPoolError>>(
            &pic,
            sp,
            user,
            "deposit",
            encode_args((ledger, deposited)).expect("encode SP deposit"),
        ),
        "deposit",
    );
    expect_sp_ok(
        call_reply::<Result<(), StabilityPoolError>>(
            &pic,
            sp,
            user,
            "opt_in_native_collateral",
            encode_args((xrp, "rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh".to_string()))
                .expect("encode native XRP opt-in"),
        ),
        "opt_in_native_collateral",
    );

    if use_lost_reply_scan {
        call_reply::<()>(
            &pic,
            ledger,
            admin,
            "set_phantom_transfer_failures",
            encode_one(1u32).unwrap(),
        );
    }

    let status_before: StabilityPoolStatus = query_reply(
        &pic,
        sp,
        Principal::anonymous(),
        "get_pool_status",
        encode_args(()).expect("encode status query"),
    );
    let position_before: Option<UserStabilityPosition> = query_reply(
        &pic,
        sp,
        Principal::anonymous(),
        "get_user_position",
        encode_args((Some(user),)).expect("encode position query"),
    );
    let position_before = position_before.expect("depositor position");
    let ledger_balance_before = ledger_balance(&pic, ledger, sp);
    assert_eq!(ledger_balance_before, deposited);
    let initial_log: icrc_ledger_types::icrc3::blocks::GetBlocksResult = query_reply(
        &pic,
        ledger,
        Principal::anonymous(),
        "icrc3_get_blocks",
        encode_one(vec![icrc_ledger_types::icrc3::blocks::GetBlocksRequest {
            start: Nat::from(0u64),
            length: Nat::from(0u64),
        }])
        .expect("encode initial ledger log-length query"),
    );
    let initial_log_length: u64 = initial_log
        .log_length
        .0
        .try_into()
        .expect("initial log length fits u64");

    let vault_id = 81_004;
    let debt = 250_000_000u64;
    let notification: Vec<LiquidationResult> = call_reply(
        &pic,
        sp,
        mock_backend,
        "notify_liquidatable_vaults",
        encode_args((vec![LiquidatableVaultInfo {
            vault_id,
            collateral_type: xrp,
            debt_amount: debt,
            collateral_amount: 3_000_000,
            recommended_liquidation_amount: debt,
            collateral_price_e8s: 50_000_000,
        }],))
        .expect("encode liquidatable notification"),
    );
    assert_eq!(notification.len(), 1, "SP should process the XRP candidate");
    assert!(
        !notification[0].success,
        "expired backend reservation must reject absorption"
    );
    let failure = notification[0].error_message.as_deref().unwrap_or_default();
    if use_lost_reply_scan {
        assert!(
            failure.contains("GenericError"),
            "unexpected outcome: {failure}"
        );
    } else if simulate_ambiguous_absorb {
        assert!(failure.contains("remains pending"), "unexpected outcome: {failure}");
    } else if retry_unpaid_refund {
        assert!(
            failure.contains("injected definite first refund failure"),
            "unexpected outcome: {failure}"
        );
    } else if retry_after_post_mint_error {
        assert!(
            failure.contains("injected response error after refund mint was persisted"),
            "unexpected outcome: {failure}"
        );
    } else if wrong_first_receipt {
        assert!(
            failure.contains("refund receipt does not match"),
            "unexpected outcome: {failure}"
        );
    } else {
        assert!(
            failure.contains("refunded in verified ledger block"),
            "unexpected outcome: {failure}"
        );
    }

    let status_after: StabilityPoolStatus = query_reply(
        &pic,
        sp,
        Principal::anonymous(),
        "get_pool_status",
        encode_args(()).expect("encode status query"),
    );
    let position_after: Option<UserStabilityPosition> = query_reply(
        &pic,
        sp,
        Principal::anonymous(),
        "get_user_position",
        encode_args((Some(user),)).expect("encode position query"),
    );
    assert_eq!(
        status_after.total_deposits_e8s,
        status_before.total_deposits_e8s
    );
    assert_eq!(
        status_after.total_depositors,
        status_before.total_depositors
    );
    assert_eq!(
        status_after.total_liquidations_executed,
        status_before.total_liquidations_executed
    );
    assert_eq!(
        status_after.stablecoin_balances,
        status_before.stablecoin_balances
    );
    let position_after = position_after.expect("depositor remains present");
    assert_eq!(
        position_after.stablecoin_balances,
        position_before.stablecoin_balances
    );
    assert_eq!(
        position_after.collateral_gains,
        position_before.collateral_gains
    );
    if !use_lost_reply_scan && !retry_unpaid_refund {
        assert_eq!(ledger_balance(&pic, ledger, sp), ledger_balance_before);
    }

    let payouts: Vec<NativeXrpPendingPayout> = query_reply(
        &pic,
        sp,
        user,
        "get_my_native_xrp_payouts",
        encode_args(()).expect("encode payout query"),
    );
    assert!(
        payouts.is_empty(),
        "rejected absorb must not create native collateral claims"
    );

    if use_lost_reply_scan && !recover_via_timer {
        let log_result: icrc_ledger_types::icrc3::blocks::GetBlocksResult = query_reply(
            &pic,
            ledger,
            Principal::anonymous(),
            "icrc3_get_blocks",
            encode_one(vec![icrc_ledger_types::icrc3::blocks::GetBlocksRequest {
                start: Nat::from(0u64),
                length: Nat::from(0u64),
            }])
            .unwrap(),
        );
        let log_length: u64 = log_result.log_length.0.try_into().unwrap();
        let block_index = log_length - 1;
        let unauthorized: Result<u64, StabilityPoolError> = call_reply(
            &pic,
            sp,
            user,
            "scan_pending_icusd_burn_history",
            encode_args((vault_id, block_index, 1u64)).unwrap(),
        );
        assert!(matches!(
            unauthorized,
            Err(StabilityPoolError::Unauthorized)
        ));

        let balance_after_lost_reply = ledger_balance(&pic, ledger, sp);
        assert_eq!(balance_after_lost_reply, ledger_balance_before - debt);
        let found: Result<u64, StabilityPoolError> = call_reply(
            &pic,
            sp,
            admin,
            "scan_pending_icusd_burn_history",
            encode_args((vault_id, block_index, 1u64)).unwrap(),
        );
        assert_eq!(
            found.expect("history scanner must find the exact burn"),
            block_index
        );

        // Exact proof re-read is idempotent, and another history scan cannot
        // rediscover an intent after it has advanced out of Prepared.
        expect_sp_ok(
            call_reply::<Result<(), StabilityPoolError>>(
                &pic,
                sp,
                admin,
                "reconcile_pending_icusd_burn",
                encode_args((vault_id, block_index)).unwrap(),
            ),
            "read back scanner-attached burn proof",
        );
        let rescanned: Result<u64, StabilityPoolError> = call_reply(
            &pic,
            sp,
            admin,
            "scan_pending_icusd_burn_history",
            encode_args((vault_id, block_index, 1u64)).unwrap(),
        );
        assert!(matches!(
            rescanned,
            Err(StabilityPoolError::LiquidationFailed { .. })
        ));
        assert_eq!(ledger_balance(&pic, ledger, sp), balance_after_lost_reply);
        return;
    }

    if use_lost_reply_scan && recover_via_timer {
        let balance_after_lost_reply = ledger_balance(&pic, ledger, sp);
        assert_eq!(balance_after_lost_reply, ledger_balance_before - debt);
        let original_burn_block: Option<u64> = query_reply(
            &pic,
            mock_backend,
            Principal::anonymous(),
            "get_mock_burn_block_index",
            encode_args(()).expect("encode burn-block query"),
        );
        assert!(original_burn_block.is_none(), "lost transfer reply must leave the SP without a block proof");

        // This mock has no liquidatable-vault feed. The public fallback also
        // cannot recover the vault after it disappears from that feed.
        let stale_retry: Result<LiquidationResult, StabilityPoolError> = call_reply(
            &pic,
            sp,
            user,
            "execute_liquidation",
            encode_one(vault_id).expect("encode stale liquidation retry"),
        );
        assert!(stale_retry.is_err(), "feed-based retry must not recover the vault");

        pic.upgrade_canister(
            sp,
            stability_pool_wasm(),
            encode_one(init).expect("encode SP upgrade"),
            None,
        )
        .expect("upgrade with proofless attempted burn intent pending");
        pic.tick();
        pic.advance_time(std::time::Duration::from_secs(601));
        for _ in 0..10 {
            pic.tick();
        }

        assert_eq!(
            ledger_balance(&pic, ledger, sp),
            ledger_balance_before,
            "timer must recover the exact duplicate block and compensate once"
        );
        let refund_block: Option<u64> = query_reply(
            &pic,
            mock_backend,
            Principal::anonymous(),
            "get_mock_refund_block_index",
            encode_args(()).expect("encode refund-block query"),
        );
        assert!(refund_block.is_some(), "recovery must complete the exact refund");
        let log_result: icrc_ledger_types::icrc3::blocks::GetBlocksResult = query_reply(
            &pic,
            ledger,
            Principal::anonymous(),
            "icrc3_get_blocks",
            encode_one(vec![icrc_ledger_types::icrc3::blocks::GetBlocksRequest {
                start: Nat::from(0u64),
                length: Nat::from(0u64),
            }])
            .expect("encode ledger log-length query"),
        );
        assert_eq!(
            log_result.log_length,
            Nat::from(initial_log_length + 2u64),
            "one burn and one refund after the initial log; replay must not burn twice"
        );
        let final_status: StabilityPoolStatus = query_reply(
            &pic,
            sp,
            Principal::anonymous(),
            "get_pool_status",
            encode_args(()).expect("encode recovered status query"),
        );
        assert_eq!(final_status.total_deposits_e8s, status_before.total_deposits_e8s);
        return;
    }

    if retry_unpaid_refund || retry_after_post_mint_error {
        if retry_unpaid_refund {
            assert_eq!(ledger_balance(&pic, ledger, sp), ledger_balance_before - debt);
        } else {
            assert_eq!(ledger_balance(&pic, ledger, sp), ledger_balance_before);
        }
        let first_refund_block: Option<u64> = query_reply(
            &pic,
            mock_backend,
            Principal::anonymous(),
            "get_mock_refund_block_index",
            encode_args(()).expect("encode refund-block query"),
        );
        if retry_after_post_mint_error {
            assert!(first_refund_block.is_some(), "the mock persisted the minted refund before returning its error");
        } else {
            assert!(first_refund_block.is_none(), "the definite failure occurred before mint dispatch");
        }
        pic.upgrade_canister(
            sp,
            stability_pool_wasm(),
            encode_one(init).expect("encode SP upgrade"),
            None,
        )
        .expect("upgrade with the exact burned refund intent pending");
        if recover_via_timer {
            // The mock deliberately has no liquidatable-vault API. Advancing
            // the timer must recover this vault directly from the persisted
            // burned intent, after its original notification has ended.
            if simulate_ambiguous_absorb {
                call_reply::<()>(
                    &pic,
                    mock_backend,
                    admin,
                    "hold_mock_status_reply",
                    encode_args(()).expect("encode status hold"),
                );
            }
            pic.tick(); // run deferred post-upgrade timer setup
            pic.advance_time(std::time::Duration::from_secs(601));
            for _ in 0..10 {
                pic.tick();
            }
            if simulate_ambiguous_absorb {
                let status_calls: u64 = query_reply(
                    &pic,
                    mock_backend,
                    Principal::anonymous(),
                    "get_mock_status_call_count",
                    encode_args(()).expect("encode status-call query"),
                );
                assert!(status_calls > 0, "timer must enter the held backend status call");

                // Keep status suspended across a competing admin refund retry.
                // The recovery path must own the SP liquidation guard through
                // the status await, so the retry cannot mint during ambiguity.
                let competing_refund = pic
                    .submit_call(
                        sp,
                        admin,
                        "retry_pending_icusd_burn_refund",
                        encode_one(vault_id).expect("encode competing refund retry"),
                    )
                    .expect("submit competing refund retry");
                let competing_refund: Result<SpBurnRefundReceipt, StabilityPoolError> = match pic
                    .await_call(competing_refund)
                    .expect("await competing refund retry")
                {
                    WasmResult::Reply(bytes) => {
                        decode_one(&bytes).expect("decode competing refund retry")
                    }
                    WasmResult::Reject(message) => {
                        panic!("competing refund retry rejected: {message}")
                    }
                };
                assert!(
                    matches!(competing_refund, Err(StabilityPoolError::SystemBusy)),
                    "refund must remain excluded while status is unresolved: {competing_refund:?}"
                );
                assert_eq!(
                    ledger_balance(&pic, ledger, sp),
                    ledger_balance_before - debt,
                    "a competing refund must not mint while status is held"
                );

                let gate_request = pic
                    .get_canister_http()
                    .into_iter()
                    .find(|request| request.url == "https://status-gate.test/hold")
                    .expect("backend status must be parked at the controlled HTTP outcall");
                pic.mock_canister_http_response(MockCanisterHttpResponse {
                    subnet_id: gate_request.subnet_id,
                    request_id: gate_request.request_id,
                    response: CanisterHttpResponse::CanisterHttpReply(CanisterHttpReply {
                        status: 200,
                        headers: vec![],
                        body: b"released".to_vec(),
                    }),
                    additional_responses: vec![],
                });
                for _ in 0..10 {
                    pic.tick();
                }

                assert_eq!(
                    ledger_balance(&pic, ledger, sp),
                    ledger_balance_before - debt,
                    "accepted absorb must retain the exact burned amount"
                );
                let status_calls: u64 = query_reply(
                    &pic,
                    mock_backend,
                    Principal::anonymous(),
                    "get_mock_status_call_count",
                    encode_args(()).expect("encode status-call query"),
                );
                assert!(status_calls > 0, "timer must resolve the backend outcome before recovery");
                let refund_block: Option<u64> = query_reply(
                    &pic,
                    mock_backend,
                    Principal::anonymous(),
                    "get_mock_refund_block_index",
                    encode_args(()).expect("encode refund-block query"),
                );
                assert!(refund_block.is_none(), "accepted absorb must not be refunded");
                let direct_retry: Result<SpBurnRefundReceipt, StabilityPoolError> = call_reply(
                    &pic,
                    sp,
                    admin,
                    "retry_pending_icusd_burn_refund",
                    encode_one(vault_id).expect("encode refund retry"),
                );
                assert!(direct_retry.is_err(), "accepted absorb must not remain refund-retryable");
                let absorb_retry: Result<LiquidationResult, StabilityPoolError> = call_reply(
                    &pic,
                    sp,
                    user,
                    "execute_liquidation",
                    encode_one(vault_id).expect("encode liquidation retry"),
                );
                assert!(absorb_retry.is_err(), "recovered vault absent from feed must not receive a stale absorb");
                assert_eq!(ledger_balance(&pic, ledger, sp), ledger_balance_before - debt);
                let status_after_recovery: StabilityPoolStatus = query_reply(
                    &pic,
                    sp,
                    Principal::anonymous(),
                    "get_pool_status",
                    encode_args(()).expect("encode recovered pool status"),
                );
                assert_eq!(
                    status_after_recovery.total_deposits_e8s,
                    status_before.total_deposits_e8s - debt,
                    "cached accepted result must be booked exactly once"
                );
                return;
            }
        } else {
            let unauthorized: Result<SpBurnRefundReceipt, StabilityPoolError> = call_reply(
                &pic,
                sp,
                user,
                "retry_pending_icusd_burn_refund",
                encode_one(vault_id).expect("encode refund retry"),
            );
            assert!(matches!(unauthorized, Err(StabilityPoolError::Unauthorized)));
            let retried: Result<SpBurnRefundReceipt, StabilityPoolError> = call_reply(
                &pic,
                sp,
                admin,
                "retry_pending_icusd_burn_refund",
                encode_one(vault_id).expect("encode refund retry"),
            );
            let receipt = expect_sp_ok(retried, "retry the same proven burn refund");
            assert_eq!(receipt.amount_e8s, debt);
            assert_eq!(receipt.recipient, sp);
            if let Some(first_refund_block) = first_refund_block {
                assert_eq!(
                    receipt.refund_block_index, first_refund_block,
                    "retry must return the original persisted refund receipt"
                );
            }
        }
        assert_eq!(ledger_balance(&pic, ledger, sp), ledger_balance_before);
        if !recover_via_timer {
            let second: Result<SpBurnRefundReceipt, StabilityPoolError> = call_reply(
                &pic,
                sp,
                admin,
                "retry_pending_icusd_burn_refund",
                encode_one(vault_id).expect("encode terminal replay"),
            );
            assert!(matches!(second, Err(StabilityPoolError::LiquidationFailed { .. })));
        }
        assert_eq!(ledger_balance(&pic, ledger, sp), ledger_balance_before);
    }

    if wrong_first_receipt {
        assert_eq!(ledger_balance(&pic, ledger, sp), ledger_balance_before);
        let burn_block: Option<u64> = query_reply(
            &pic,
            mock_backend,
            Principal::anonymous(),
            "get_mock_burn_block_index",
            encode_args(()).expect("encode burn-block query"),
        );
        let burn_block = burn_block.expect("mock captured the official ledger burn block");
        expect_sp_ok(
            call_reply::<Result<(), StabilityPoolError>>(
                &pic,
                sp,
                admin,
                "reconcile_pending_icusd_burn",
                encode_args((vault_id, burn_block)).expect("encode burn reconciliation"),
            ),
            "reconcile already verified canonical burn block",
        );

        let (method, args) = if use_history_reconcile {
            (
                "reconcile_pending_icusd_burn_refund_from_history",
                encode_one(vault_id).expect("encode history refund reconciliation"),
            )
        } else {
            let refund_block: Option<u64> = query_reply(
                &pic,
                mock_backend,
                Principal::anonymous(),
                "get_mock_refund_block_index",
                encode_args(()).expect("encode refund-block query"),
            );
            let refund_block = refund_block.expect("mock captured the already minted refund block");
            (
                "reconcile_pending_icusd_burn_refund",
                encode_args((vault_id, refund_block)).expect("encode refund reconciliation"),
            )
        };
        let unauthorized: Result<SpBurnRefundReceipt, StabilityPoolError> = call_reply(
            &pic,
            sp,
            user,
            method,
            args.clone(),
        );
        assert!(matches!(unauthorized, Err(StabilityPoolError::Unauthorized)));
        let reconciled: Result<SpBurnRefundReceipt, StabilityPoolError> = call_reply(
            &pic,
            sp,
            admin,
            method,
            args,
        );
        let reconciled = expect_sp_ok(reconciled, "admin refund-block reconciliation");
        assert!(reconciled.refund_block_index > 0);
        assert_eq!(ledger_balance(&pic, ledger, sp), ledger_balance_before);
    }

    // This balance-affecting opt-out is blocked while an absorb intent exists.
    // Its success proves the compensated intent was removed; deposits remain
    // unchanged because the probe only changes future XRP eligibility.
    expect_sp_ok(
        call_reply::<Result<(), StabilityPoolError>>(
            &pic,
            sp,
            user,
            "opt_out_collateral",
            encode_one(xrp).expect("encode opt-out"),
        ),
        "opt-out after compensation",
    );
    let final_status: StabilityPoolStatus = query_reply(
        &pic,
        sp,
        Principal::anonymous(),
        "get_pool_status",
        encode_args(()).expect("encode final status query"),
    );
    assert_eq!(
        final_status.total_deposits_e8s,
        status_before.total_deposits_e8s
    );
    assert_eq!(
        final_status.stablecoin_balances,
        status_before.stablecoin_balances
    );
}

#[test]
fn native_xrp_absorb_rejection_refunds_exact_burn_without_depositor_or_claim_accounting() {
    run_native_xrp_refund_case(false, false, false, false, false, false, false);
}

#[test]
fn malformed_refund_receipt_stays_locked_until_admin_verifies_existing_burn_and_refund_blocks() {
    run_native_xrp_refund_case(true, false, false, false, false, false, false);
}

#[test]
fn admin_can_recover_a_lost_refund_index_from_positive_history_proof() {
    run_native_xrp_refund_case(true, true, false, false, false, false, false);
}

#[test]
fn admin_scans_history_to_reconcile_prepared_lost_reply_burn() {
    run_native_xrp_refund_case(false, false, true, false, false, false, false);
}

#[test]
fn timer_recovers_proofless_native_xrp_burn_after_vault_leaves_feed() {
    run_native_xrp_refund_case(false, false, true, false, false, true, false);
}

#[test]
fn admin_retries_a_proven_burn_refund_after_a_definite_first_no_effect_failure() {
    run_native_xrp_refund_case(false, false, false, true, false, false, false);
}

#[test]
fn timer_recovers_burn_refund_without_a_liquidatable_vault_notification() {
    run_native_xrp_refund_case(false, false, false, true, false, true, false);
}

#[test]
fn held_backend_status_excludes_competing_refund_and_books_absorb_once() {
    run_native_xrp_refund_case(false, false, false, true, false, true, true);
}

#[test]
fn admin_retries_the_persisted_refund_after_backend_error_following_mint() {
    run_native_xrp_refund_case(false, false, false, false, true, false, false);
}
