use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Principal};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
use icrc_ledger_types::icrc3::blocks::{GetBlocksRequest, GetBlocksResult};
use icrc_ledger_types::icrc2::approve::ApproveArgs;
use pocket_ic::{PocketIcBuilder, WasmResult};
use stability_pool::types::*;

// ─── Candid types for ICRC-1 ledger initialization ───

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
enum MetadataValue {
    Nat(candid::Nat),
    Int(candid::Int),
    Text(String),
    Blob(Vec<u8>),
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
enum LedgerArg {
    Init(LedgerInitArgs),
}

// ─── 3pool init types ───

use rumi_3pool::types::{ThreePoolInitArgs, TokenConfig, PoolStatus, ThreePoolError};

// ─── WASM loaders ───

fn icrc1_ledger_wasm() -> Vec<u8> {
    include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec()
}

fn three_pool_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_3pool.wasm").to_vec()
}

fn stability_pool_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/stability_pool.wasm").to_vec()
}

// ─── Test Environment ───

#[allow(dead_code)]
struct TestEnv {
    pic: pocket_ic::PocketIc,
    admin: Principal,
    test_user: Principal,
    minting_account: Principal,
    icusd_ledger: Principal,
    ckusdt_ledger: Principal,
    ckusdc_ledger: Principal,
    pool_id: Principal,
    sp_id: Principal,
    protocol_id: Principal,
}

fn setup_test_env() -> TestEnv {
    let pic = PocketIcBuilder::new()
        .with_application_subnet()
        .build();

    let minting_account = Principal::self_authenticating(&[100, 100, 100]);
    let test_user = Principal::self_authenticating(&[1, 2, 3, 4]);
    let admin = Principal::self_authenticating(&[5, 6, 7, 8]);
    // Fake protocol canister — not deployed but referenced
    let protocol_id = Principal::self_authenticating(&[9, 10, 11, 12]);

    // ── Deploy 3 ICRC-1 ledgers ──
    struct LedgerSpec {
        name: &'static str,
        symbol: &'static str,
        decimals: u8,
        initial_balance: u128,
    }

    let ledger_specs = [
        LedgerSpec {
            name: "icUSD",
            symbol: "icUSD",
            decimals: 8,
            initial_balance: 1_000_000_000_000_000, // 10M with 8 decimals
        },
        LedgerSpec {
            name: "ckUSDT",
            symbol: "ckUSDT",
            decimals: 6,
            initial_balance: 10_000_000_000_000, // 10M with 6 decimals
        },
        LedgerSpec {
            name: "ckUSDC",
            symbol: "ckUSDC",
            decimals: 6,
            initial_balance: 10_000_000_000_000, // 10M with 6 decimals
        },
    ];

    let mut ledger_ids = Vec::new();
    for spec in &ledger_specs {
        let ledger_id = pic.create_canister();
        pic.add_cycles(ledger_id, 2_000_000_000_000);

        let init_args = LedgerInitArgs {
            minting_account: Account { owner: minting_account, subaccount: None },
            fee_collector_account: None,
            transfer_fee: candid::Nat::from(0u64), // Zero fees for cleaner testing
            decimals: Some(spec.decimals),
            max_memo_length: Some(32),
            token_name: spec.name.to_string(),
            token_symbol: spec.symbol.to_string(),
            metadata: vec![],
            initial_balances: vec![(
                Account { owner: test_user, subaccount: None },
                candid::Nat::from(spec.initial_balance),
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
        ledger_ids.push(ledger_id);
    }

    let icusd_ledger = ledger_ids[0];
    let ckusdt_ledger = ledger_ids[1];
    let ckusdc_ledger = ledger_ids[2];

    // ── Deploy 3pool ──
    let pool_init_args = ThreePoolInitArgs {
        tokens: [
            TokenConfig {
                ledger_id: icusd_ledger,
                symbol: "icUSD".to_string(),
                decimals: 8,
                precision_mul: 10_000_000_000, // 10^10
            },
            TokenConfig {
                ledger_id: ckusdt_ledger,
                symbol: "ckUSDT".to_string(),
                decimals: 6,
                precision_mul: 1_000_000_000_000, // 10^12
            },
            TokenConfig {
                ledger_id: ckusdc_ledger,
                symbol: "ckUSDC".to_string(),
                decimals: 6,
                precision_mul: 1_000_000_000_000, // 10^12
            },
        ],
        initial_a: 100,
        swap_fee_bps: 4,
        admin_fee_bps: 5000,
        admin,
    };

    // The Stability Pool recognizes the canonical 3USD ledger principal.
    // Allocate that identity in PocketIC so the fixture exercises the real
    // LP-ledger admission path instead of failing on a synthetic principal.
    let pool_id = Principal::from_text("fohh4-yyaaa-aaaap-qtkpa-cai")
        .expect("canonical 3USD ledger principal");
    pic.create_canister_with_id(None, None, pool_id)
        .expect("create canonical 3pool canister");
    pic.add_cycles(pool_id, 2_000_000_000_000);
    pic.install_canister(pool_id, three_pool_wasm(), encode_one(pool_init_args).unwrap(), None);

    // ── Deploy stability pool ──
    let sp_init = StabilityPoolInitArgs {
        protocol_canister_id: protocol_id,
        authorized_admins: vec![admin],
    };

    let sp_id = pic.create_canister();
    pic.add_cycles(sp_id, 2_000_000_000_000);
    pic.install_canister(sp_id, stability_pool_wasm(), encode_one(sp_init).unwrap(), None);

    // ── Approve all ledgers for both 3pool and stability pool ──
    for ledger_id in &ledger_ids {
        // Approve 3pool
        approve(&pic, *ledger_id, test_user, pool_id, u128::MAX);
        // Approve stability pool
        approve(&pic, *ledger_id, test_user, sp_id, u128::MAX);
    }

    // ── Seed 3pool with liquidity (1M each) ──
    let add_liq_amounts: Vec<u128> = vec![
        100_000_000_000_000,  // 1M icUSD  (8 dec)
        1_000_000_000_000,    // 1M ckUSDT (6 dec)
        1_000_000_000_000,    // 1M ckUSDC (6 dec)
    ];
    let result = pic.update_call(pool_id, test_user, "add_liquidity", encode_args((add_liq_amounts, 0u128)).unwrap())
        .expect("add_liquidity call failed");
    match result {
        WasmResult::Reply(bytes) => {
            let r: Result<candid::Nat, ThreePoolError> = decode_one(&bytes).expect("decode");
            let lp = r.expect("add_liquidity error");
            assert!(lp > candid::Nat::from(0u64), "LP tokens should be > 0");
        }
        WasmResult::Reject(msg) => panic!("add_liquidity rejected: {}", msg),
    }

    // ── Register stablecoins in stability pool ──
    register_stablecoin(&pic, sp_id, admin, StablecoinConfig {
        ledger_id: icusd_ledger,
        symbol: "icUSD".to_string(),
        decimals: 8,
        priority: 1,
        is_active: true,
        transfer_fee: Some(10_000),
        is_lp_token: None,
        underlying_pool: None,
    });
    register_stablecoin(&pic, sp_id, admin, StablecoinConfig {
        ledger_id: ckusdt_ledger,
        symbol: "ckUSDT".to_string(),
        decimals: 6,
        priority: 2,
        is_active: true,
        transfer_fee: Some(10_000),
        is_lp_token: None,
        underlying_pool: None,
    });
    register_stablecoin(&pic, sp_id, admin, StablecoinConfig {
        ledger_id: ckusdc_ledger,
        symbol: "ckUSDC".to_string(),
        decimals: 6,
        priority: 2,
        is_active: true,
        transfer_fee: Some(10_000),
        is_lp_token: None,
        underlying_pool: None,
    });

    // The 3pool canister is the 3USD LP-token ledger and implements ICRC-1/2.
    // Tests register it individually when they exercise direct LP deposits.
    // The conversion route stays closed until add-liquidity has durable receipts.

    TestEnv {
        pic,
        admin,
        test_user,
        minting_account,
        icusd_ledger,
        ckusdt_ledger,
        ckusdc_ledger,
        pool_id,
        sp_id,
        protocol_id,
    }
}

// ─── Helpers ───

fn approve(pic: &pocket_ic::PocketIc, ledger: Principal, owner: Principal, spender: Principal, amount: u128) {
    let args = ApproveArgs {
        from_subaccount: None,
        spender: Account { owner: spender, subaccount: None },
        amount: candid::Nat::from(amount),
        expected_allowance: None,
        expires_at: None,
        fee: None,
        memo: None,
        created_at_time: None,
    };
    let result = pic.update_call(ledger, owner, "icrc2_approve", encode_one(args).unwrap())
        .expect("approve call failed");
    match result {
        WasmResult::Reply(_) => {}
        WasmResult::Reject(msg) => panic!("approve rejected: {}", msg),
    }
}

fn register_stablecoin(pic: &pocket_ic::PocketIc, sp_id: Principal, admin: Principal, config: StablecoinConfig) {
    let result = pic.update_call(sp_id, admin, "register_stablecoin", encode_one(config.clone()).unwrap())
        .expect("register_stablecoin call failed");
    match result {
        WasmResult::Reply(bytes) => {
            let r: Result<(), StabilityPoolError> = decode_one(&bytes).expect("decode register_stablecoin");
            r.expect(&format!("register_stablecoin failed for {}", config.symbol));
        }
        WasmResult::Reject(msg) => panic!("register_stablecoin rejected: {}", msg),
    }
}

fn get_pool_status(pic: &pocket_ic::PocketIc, sp_id: Principal) -> StabilityPoolStatus {
    let result = pic.query_call(sp_id, Principal::anonymous(), "get_pool_status", encode_args(()).unwrap())
        .expect("get_pool_status call failed");
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode pool status"),
        WasmResult::Reject(msg) => panic!("get_pool_status rejected: {}", msg),
    }
}

fn get_user_position(pic: &pocket_ic::PocketIc, sp_id: Principal, user: Principal) -> Option<UserStabilityPosition> {
    let result = pic.query_call(sp_id, user, "get_user_position", encode_one(Some(user)).unwrap())
        .expect("get_user_position call failed");
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode user position"),
        WasmResult::Reject(msg) => panic!("get_user_position rejected: {}", msg),
    }
}

fn query_3pool_status(pic: &pocket_ic::PocketIc, pool_id: Principal) -> PoolStatus {
    let result = pic.query_call(pool_id, Principal::anonymous(), "get_pool_status", encode_args(()).unwrap())
        .expect("3pool get_pool_status call failed");
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode 3pool status"),
        WasmResult::Reject(msg) => panic!("3pool get_pool_status rejected: {}", msg),
    }
}

/// After registering a 3USD LP token, advance time to trigger the virtual price timer.
/// The stability pool fetches virtual prices every 300s. On init, it fires immediately
/// but only for LP tokens already registered. Since we register 3USD AFTER init,
/// we need to advance time and tick to trigger the next fetch.
fn query_3pool_lp_balance(pic: &pocket_ic::PocketIc, pool_id: Principal, owner: Principal) -> u128 {
    let result = pic.query_call(pool_id, owner, "get_lp_balance", encode_one(owner).unwrap())
        .expect("get_lp_balance call failed");
    match result {
        WasmResult::Reply(bytes) => {
            let nat: candid::Nat = decode_one(&bytes).expect("decode lp balance");
            nat.0.try_into().expect("lp balance overflow")
        }
        WasmResult::Reject(msg) => panic!("get_lp_balance rejected: {}", msg),
    }
}

fn ledger_balance(pic: &pocket_ic::PocketIc, ledger: Principal, owner: Principal) -> u128 {
    let account = Account { owner, subaccount: None };
    let result = pic.query_call(ledger, Principal::anonymous(), "icrc1_balance_of", encode_one(account).unwrap())
        .expect("icrc1_balance_of call failed");
    match result {
        WasmResult::Reply(bytes) => {
            let balance: candid::Nat = decode_one(&bytes).expect("decode balance");
            balance.0.try_into().expect("balance overflow")
        }
        WasmResult::Reject(msg) => panic!("icrc1_balance_of rejected: {}", msg),
    }
}

fn icrc3_log_length(pic: &pocket_ic::PocketIc, ledger: Principal) -> u64 {
    let args = vec![GetBlocksRequest {
        start: 0u64.into(),
        length: candid::Nat::from(0u64),
    }];
    let result = pic
        .query_call(
            ledger,
            Principal::anonymous(),
            "icrc3_get_blocks",
            encode_args((args,)).unwrap(),
        )
        .expect("icrc3_get_blocks call failed");
    let blocks: GetBlocksResult = match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode ICRC-3 blocks"),
        WasmResult::Reject(message) => panic!("icrc3_get_blocks rejected: {message}"),
    };
    blocks
        .log_length
        .0
        .to_string()
        .parse()
        .expect("ICRC-3 log length fits u64")
}

fn assert_3usd_conversion_unavailable_without_debit(
    env: &TestEnv,
    input_ledger: Principal,
    amount: u64,
) {
    let input_balance = |owner| {
        if input_ledger == env.pool_id {
            query_3pool_lp_balance(&env.pic, env.pool_id, owner)
        } else {
            ledger_balance(&env.pic, input_ledger, owner)
        }
    };
    let user_input_before = input_balance(env.test_user);
    let pool_input_before = input_balance(env.sp_id);
    let user_lp_before = query_3pool_lp_balance(&env.pic, env.pool_id, env.test_user);
    let sp_lp_before = query_3pool_lp_balance(&env.pic, env.pool_id, env.sp_id);
    let input_log_before = icrc3_log_length(&env.pic, input_ledger);
    let pool_log_before = icrc3_log_length(&env.pic, env.pool_id);
    let status_before = get_pool_status(&env.pic, env.sp_id);
    let recorded_input_before = status_before
        .stablecoin_balances
        .get(&input_ledger)
        .copied()
        .unwrap_or_default();
    let recorded_sp_lp_before = status_before
        .stablecoin_balances
        .get(&env.pool_id)
        .copied()
        .unwrap_or_default();
    let recorded_user_lp_before = get_user_position(&env.pic, env.sp_id, env.test_user)
        .and_then(|position| position.stablecoin_balances.get(&env.pool_id).copied())
        .unwrap_or_default();

    let result = env
        .pic
        .update_call(
            env.sp_id,
            env.test_user,
            "deposit_as_3usd",
            encode_args((input_ledger, amount)).unwrap(),
        )
        .expect("deposit_as_3usd call failed");
    match result {
        WasmResult::Reply(bytes) => match decode_one::<Result<u64, StabilityPoolError>>(&bytes)
            .expect("decode deposit_as_3usd")
        {
            Err(StabilityPoolError::InterCanisterCallFailed { method, target }) => {
                assert!(method.contains("receipt-backed conversion unavailable"));
                assert!(method.contains("no input tokens were pulled"));
                assert!(target.contains("3pool"));
            }
            other => panic!("expected receipt-backed conversion rejection, got {other:?}"),
        },
        WasmResult::Reject(message) => {
            panic!("deposit_as_3usd rejected at transport level: {message}")
        }
    }

    assert_eq!(input_balance(env.test_user), user_input_before, "user input moved");
    assert_eq!(input_balance(env.sp_id), pool_input_before, "SP input balance changed");
    assert_eq!(
        query_3pool_lp_balance(&env.pic, env.pool_id, env.test_user),
        user_lp_before,
        "user LP balance changed",
    );
    assert_eq!(
        query_3pool_lp_balance(&env.pic, env.pool_id, env.sp_id),
        sp_lp_before,
        "SP received or lost 3USD LP",
    );
    assert_eq!(
        get_pool_status(&env.pic, env.sp_id)
            .stablecoin_balances
            .get(&input_ledger)
            .copied()
            .unwrap_or_default(),
        recorded_input_before,
        "SP accounting changed for the input ledger",
    );
    assert_eq!(
        icrc3_log_length(&env.pic, input_ledger),
        input_log_before,
        "closed route must not write an input-ledger transaction",
    );
    assert_eq!(
        icrc3_log_length(&env.pic, env.pool_id),
        pool_log_before,
        "closed route must not write a 3pool transaction",
    );
    let status_after = get_pool_status(&env.pic, env.sp_id);
    assert_eq!(
        status_after
            .stablecoin_balances
            .get(&env.pool_id)
            .copied()
            .unwrap_or_default(),
        recorded_sp_lp_before,
        "3USD LP accounting changed",
    );
    let recorded_user_lp_after = get_user_position(&env.pic, env.sp_id, env.test_user)
        .and_then(|position| position.stablecoin_balances.get(&env.pool_id).copied())
        .unwrap_or_default();
    assert_eq!(recorded_user_lp_after, recorded_user_lp_before, "user received 3USD credit");
}

// ─── Tests ───

#[test]
fn test_ckusdc_max_withdraw_drains_position_net_of_ledger_fee() {
    let pic = PocketIcBuilder::new()
        .with_application_subnet()
        .build();
    let minting_account = Principal::self_authenticating(&[101, 101, 101]);
    let test_user = Principal::self_authenticating(&[11, 12, 13, 14]);
    let admin = Principal::self_authenticating(&[15, 16, 17, 18]);
    let protocol_id = Principal::self_authenticating(&[19, 20, 21, 22]);
    let ckusdc_fee = 10_000u64; // 0.01 ckUSDC at 6 decimals

    let ckusdc_ledger = pic.create_canister();
    pic.add_cycles(ckusdc_ledger, 2_000_000_000_000);
    let ledger_init = LedgerInitArgs {
        minting_account: Account { owner: minting_account, subaccount: None },
        fee_collector_account: None,
        transfer_fee: candid::Nat::from(ckusdc_fee),
        decimals: Some(6),
        max_memo_length: Some(32),
        token_name: "ckUSDC".to_string(),
        token_symbol: "ckUSDC".to_string(),
        metadata: vec![],
        initial_balances: vec![(
            Account { owner: test_user, subaccount: None },
            candid::Nat::from(5_000_000u64),
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
    pic.install_canister(
        ckusdc_ledger,
        icrc1_ledger_wasm(),
        encode_args((LedgerArg::Init(ledger_init),)).unwrap(),
        None,
    );

    let sp_init = StabilityPoolInitArgs {
        protocol_canister_id: protocol_id,
        authorized_admins: vec![admin],
    };
    let sp_id = pic.create_canister();
    pic.add_cycles(sp_id, 2_000_000_000_000);
    pic.install_canister(sp_id, stability_pool_wasm(), encode_one(sp_init).unwrap(), None);

    approve(&pic, ckusdc_ledger, test_user, sp_id, u128::MAX);
    register_stablecoin(&pic, sp_id, admin, StablecoinConfig {
        ledger_id: ckusdc_ledger,
        symbol: "ckUSDC".to_string(),
        decimals: 6,
        priority: 2,
        is_active: true,
        // Legacy bad value: registration normalizes it, and withdraw also
        // queries live icrc1_fee before computing the net amount.
        transfer_fee: Some(10),
        is_lp_token: None,
        underlying_pool: None,
    });

    let deposit_amount = 1_010_000u64; // 1.0100 ckUSDC
    let result = pic.update_call(
        sp_id, test_user, "deposit",
        encode_args((ckusdc_ledger, deposit_amount)).unwrap()
    ).expect("deposit call failed");
    match result {
        WasmResult::Reply(bytes) => {
            let r: Result<(), StabilityPoolError> = decode_one(&bytes).expect("decode deposit");
            r.expect("deposit failed");
        }
        WasmResult::Reject(msg) => panic!("deposit rejected: {}", msg),
    }
    assert_eq!(
        ledger_balance(&pic, ckusdc_ledger, sp_id),
        deposit_amount as u128,
        "pool ledger account should receive the full deposit amount",
    );

    let user_before = ledger_balance(&pic, ckusdc_ledger, test_user);
    let result = pic.update_call(
        sp_id, test_user, "withdraw",
        encode_args((ckusdc_ledger, deposit_amount)).unwrap()
    ).expect("withdraw call failed");
    match result {
        WasmResult::Reply(bytes) => {
            let r: Result<(), StabilityPoolError> = decode_one(&bytes).expect("decode withdraw");
            r.expect("withdraw failed");
        }
        WasmResult::Reject(msg) => panic!("withdraw rejected: {}", msg),
    }

    assert_eq!(
        ledger_balance(&pic, ckusdc_ledger, test_user) - user_before,
        (deposit_amount - ckusdc_fee) as u128,
        "user should receive the deposited amount net of the ckUSDC transfer fee",
    );
    assert_eq!(
        ledger_balance(&pic, ckusdc_ledger, sp_id),
        0,
        "max withdraw should drain the pool ledger account for this position",
    );
    assert!(
        get_user_position(&pic, sp_id, test_user).is_none(),
        "max withdraw should remove the user's empty position",
    );
}

fn run_ckusdc_sole_holder_ledger_shortfall_withdraw(withdraw_recorded_amount: bool) {
    let pic = PocketIcBuilder::new()
        .with_application_subnet()
        .build();
    let minting_account = Principal::self_authenticating(&[100, 100, 101]);
    let test_user = Principal::self_authenticating(&[1, 2, 3, 5]);
    let admin = Principal::self_authenticating(&[5, 6, 7, 9]);
    let protocol_id = Principal::self_authenticating(&[19, 20, 21, 23]);
    let sink = Principal::self_authenticating(&[42, 42, 42, 42]);
    let ckusdc_fee = 10_000u64;

    let ckusdc_ledger = pic.create_canister();
    pic.add_cycles(ckusdc_ledger, 2_000_000_000_000);
    let ledger_init = LedgerInitArgs {
        minting_account: Account { owner: minting_account, subaccount: None },
        fee_collector_account: None,
        transfer_fee: candid::Nat::from(ckusdc_fee),
        decimals: Some(6),
        max_memo_length: Some(32),
        token_name: "ckUSDC".to_string(),
        token_symbol: "ckUSDC".to_string(),
        metadata: vec![],
        initial_balances: vec![(
            Account { owner: test_user, subaccount: None },
            candid::Nat::from(5_000_000u64),
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
    pic.install_canister(
        ckusdc_ledger,
        icrc1_ledger_wasm(),
        encode_args((LedgerArg::Init(ledger_init),)).unwrap(),
        None,
    );

    let sp_init = StabilityPoolInitArgs {
        protocol_canister_id: protocol_id,
        authorized_admins: vec![admin],
    };
    let sp_id = pic.create_canister();
    pic.add_cycles(sp_id, 2_000_000_000_000);
    pic.install_canister(sp_id, stability_pool_wasm(), encode_one(sp_init).unwrap(), None);

    approve(&pic, ckusdc_ledger, test_user, sp_id, u128::MAX);
    register_stablecoin(&pic, sp_id, admin, StablecoinConfig {
        ledger_id: ckusdc_ledger,
        symbol: "ckUSDC".to_string(),
        decimals: 6,
        priority: 2,
        is_active: true,
        transfer_fee: Some(10),
        is_lp_token: None,
        underlying_pool: None,
    });

    let deposit_amount = 1_010_000u64;
    let result = pic
        .update_call(
            sp_id,
            test_user,
            "deposit",
            encode_args((ckusdc_ledger, deposit_amount)).unwrap(),
        )
        .expect("deposit call failed");
    match result {
        WasmResult::Reply(bytes) => {
            let r: Result<(), StabilityPoolError> = decode_one(&bytes).expect("decode deposit");
            r.expect("deposit failed");
        }
        WasmResult::Reject(msg) => panic!("deposit rejected: {}", msg),
    }

    let drift_transfer = TransferArg {
        from_subaccount: None,
        to: Account { owner: sink, subaccount: None },
        amount: 1u64.into(),
        fee: None,
        memo: None,
        created_at_time: None,
    };
    let result = pic
        .update_call(
            ckusdc_ledger,
            sp_id,
            "icrc1_transfer",
            encode_one(drift_transfer).unwrap(),
        )
        .expect("drift transfer call failed");
    match result {
        WasmResult::Reply(bytes) => {
            let r: Result<candid::Nat, TransferError> =
                decode_one(&bytes).expect("decode drift transfer");
            r.expect("drift transfer failed");
        }
        WasmResult::Reject(msg) => panic!("drift transfer rejected: {}", msg),
    }

    let live_pool_balance = ledger_balance(&pic, ckusdc_ledger, sp_id) as u64;
    assert_eq!(live_pool_balance, deposit_amount - ckusdc_fee - 1);

    let requested_withdrawal = if withdraw_recorded_amount {
        deposit_amount
    } else {
        live_pool_balance
    };
    let user_before = ledger_balance(&pic, ckusdc_ledger, test_user);
    let result = pic
        .update_call(
            sp_id,
            test_user,
            "withdraw",
            encode_args((ckusdc_ledger, requested_withdrawal)).unwrap(),
        )
        .expect("withdraw call failed");
    match result {
        WasmResult::Reply(bytes) => {
            let r: Result<(), StabilityPoolError> = decode_one(&bytes).expect("decode withdraw");
            r.expect("withdraw failed despite sole-holder ledger drift");
        }
        WasmResult::Reject(msg) => panic!("withdraw rejected: {}", msg),
    }

    assert_eq!(
        ledger_balance(&pic, ckusdc_ledger, test_user) - user_before,
        (live_pool_balance - ckusdc_fee) as u128,
        "user should receive the real ledger balance net of one transfer fee",
    );
    assert_eq!(
        ledger_balance(&pic, ckusdc_ledger, sp_id),
        0,
        "withdraw should drain the real pool ledger balance",
    );
    assert!(
        get_user_position(&pic, sp_id, test_user).is_none(),
        "sole-holder shortfall correction should remove the phantom balance",
    );
}

#[test]
fn test_ckusdc_max_withdraw_self_corrects_sole_holder_ledger_shortfall() {
    run_ckusdc_sole_holder_ledger_shortfall_withdraw(true);
}

#[test]
fn test_ckusdc_capped_max_withdraw_self_corrects_sole_holder_ledger_shortfall() {
    run_ckusdc_sole_holder_ledger_shortfall_withdraw(false);
}

/// Test 1: Direct deposit of icUSD into the stability pool works
#[test]
fn test_direct_icusd_deposit() {
    let env = setup_test_env();

    let deposit_amount: u64 = 100_00000000; // 100 icUSD

    let result = env.pic.update_call(
        env.sp_id, env.test_user, "deposit",
        encode_args((env.icusd_ledger, deposit_amount)).unwrap()
    ).expect("deposit call failed");

    match result {
        WasmResult::Reply(bytes) => {
            let r: Result<(), StabilityPoolError> = decode_one(&bytes).expect("decode deposit");
            r.expect("deposit failed");
        }
        WasmResult::Reject(msg) => panic!("deposit rejected: {}", msg),
    }

    // Verify user position
    let pos = get_user_position(&env.pic, env.sp_id, env.test_user)
        .expect("user should have a position");

    let icusd_balance = pos.stablecoin_balances.iter()
        .find(|(ledger, _)| **ledger == env.icusd_ledger)
        .map(|(_, bal)| *bal)
        .unwrap_or(0);

    assert_eq!(icusd_balance, deposit_amount, "icUSD balance should match deposit");

    // Verify pool status
    let status = get_pool_status(&env.pic, env.sp_id);
    assert_eq!(status.total_depositors, 1);
    assert_eq!(status.total_deposits_e8s, deposit_amount);
}

fn setup_deposit_recovery_env() -> TestEnv {
    let pic = PocketIcBuilder::new().with_application_subnet().build();
    let minting_account = Principal::self_authenticating(&[100, 100, 100]);
    let test_user = Principal::self_authenticating(&[1, 2, 3, 4]);
    let admin = Principal::self_authenticating(&[5, 6, 7, 8]);
    let protocol_id = Principal::self_authenticating(&[9, 10, 11, 12]);
    let icusd_ledger = pic.create_canister();
    pic.add_cycles(icusd_ledger, 2_000_000_000_000);
    let init_args = LedgerInitArgs {
        minting_account: Account { owner: minting_account, subaccount: None },
        fee_collector_account: None,
        transfer_fee: candid::Nat::from(0u64),
        decimals: Some(8),
        max_memo_length: Some(32),
        token_name: "icUSD".to_string(),
        token_symbol: "icUSD".to_string(),
        metadata: vec![],
        initial_balances: vec![(
            Account { owner: test_user, subaccount: None },
            candid::Nat::from(1_000_000_000_000_000u128),
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
    pic.install_canister(
        icusd_ledger,
        icrc1_ledger_wasm(),
        encode_args((LedgerArg::Init(init_args),)).unwrap(),
        None,
    );
    let sp_id = pic.create_canister();
    pic.add_cycles(sp_id, 2_000_000_000_000);
    pic.install_canister(
        sp_id,
        stability_pool_wasm(),
        encode_one(StabilityPoolInitArgs { protocol_canister_id: protocol_id, authorized_admins: vec![admin] }).unwrap(),
        None,
    );
    approve(&pic, icusd_ledger, test_user, sp_id, u128::MAX);
    register_stablecoin(&pic, sp_id, admin, StablecoinConfig {
        ledger_id: icusd_ledger,
        symbol: "icUSD".to_string(),
        decimals: 8,
        priority: 1,
        is_active: true,
        transfer_fee: Some(0),
        is_lp_token: None,
        underlying_pool: None,
    });
    TestEnv {
        pic,
        admin,
        test_user,
        minting_account,
        icusd_ledger,
        ckusdt_ledger: Principal::anonymous(),
        ckusdc_ledger: Principal::anonymous(),
        pool_id: Principal::anonymous(),
        sp_id,
        protocol_id,
    }
}

/// Recover an official-ledger transfer after its callback outcome was lost.
#[test]
fn official_ledger_simulated_lost_callback_reconciles_receipt_once() {
    let env = setup_deposit_recovery_env();
    let amount = 100_00000000u64;
    let seeded = env.pic.update_call(
        env.sp_id,
        env.test_user,
        "test_seed_unresolved_deposit_intent",
        encode_args((env.icusd_ledger, amount, None::<u64>)).unwrap(),
    ).expect("seed test-only ambiguous intent");
    let timestamp = match seeded {
        WasmResult::Reply(bytes) => decode_one::<Result<u64, StabilityPoolError>>(&bytes)
            .expect("decode intent seed").expect("seed ambiguous intent"),
        WasmResult::Reject(message) => panic!("intent seed rejected: {message}"),
    };
    let transfer = env.pic.update_call(
        env.icusd_ledger,
        env.sp_id,
        "icrc2_transfer_from",
        encode_one(TransferFromArgs {
            spender_subaccount: None,
            from: Account { owner: env.test_user, subaccount: None },
            to: Account { owner: env.sp_id, subaccount: None },
            amount: candid::Nat::from(amount),
            fee: None,
            memo: None,
            created_at_time: Some(timestamp),
        }).unwrap(),
    ).expect("execute official transfer_from");
    let actual_block = match transfer {
        WasmResult::Reply(bytes) => decode_one::<Result<candid::Nat, TransferFromError>>(&bytes)
            .expect("decode official transfer_from").expect("official transfer succeeds"),
        WasmResult::Reject(message) => panic!("official transfer_from rejected: {message}"),
    };
    let block_index: u64 = actual_block.0.try_into().expect("block index fits u64");
    assert_eq!(ledger_balance(&env.pic, env.icusd_ledger, env.sp_id), amount as u128);

    let pending = env.pic.query_call(
        env.sp_id,
        env.test_user,
        "get_pending_deposit_intent",
        encode_args(()).unwrap(),
    ).expect("query pending deposit");
    let pending: Option<PendingDepositIntent> = match pending {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode pending intent"),
        WasmResult::Reject(message) => panic!("pending intent query rejected: {message}"),
    };
    let pending = pending.expect("exact ambiguous intent remains pending");
    assert_eq!(pending.token_ledger, env.icusd_ledger);
    assert_eq!(pending.amount, amount);
    assert!(!pending.ambiguous_seen);
    assert_eq!(pending.in_flight_attempts, Some(1));
    assert_eq!(pending.transfer_created_at_time_ns, timestamp);

    let response = env.pic.query_call(
        env.icusd_ledger,
        Principal::anonymous(),
        "icrc3_get_blocks",
        encode_args((vec![GetBlocksRequest { start: block_index.into(), length: candid::Nat::from(1u64) }],)).unwrap(),
    ).expect("query official ICRC-3 block");
    let blocks: GetBlocksResult = match response {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode official ICRC-3 response"),
        WasmResult::Reject(message) => panic!("ICRC-3 query rejected: {message}"),
    };
    let ledger_block = blocks.blocks.iter().find(|block| block.id == candid::Nat::from(block_index))
        .expect("official ledger contains committed block");
    let decoded = rumi_protocol_backend::icrc3_proof::decode_block(&ledger_block.block)
        .expect("decode committed transfer_from block");
    assert_eq!(decoded.op, "xfer");
    assert_eq!(decoded.amount, u128::from(amount));
    assert_eq!(decoded.created_at_time, Some(pending.transfer_created_at_time_ns));
    assert_eq!(decoded.from.as_ref().unwrap().owner, env.test_user);
    assert_eq!(decoded.to.as_ref().unwrap().owner, env.sp_id);
    assert_eq!(decoded.spender.as_ref().unwrap().owner, env.sp_id);
    assert!(matches!(decoded.transaction_fee, None | Some(0)));

    let result = env.pic.update_call(
        env.sp_id,
        env.test_user,
        "reconcile_pending_deposit",
        encode_one(block_index).unwrap(),
    ).expect("submit positive reconciliation");
    match result {
        WasmResult::Reply(bytes) => decode_one::<Result<(), StabilityPoolError>>(&bytes)
            .expect("decode reconciliation").expect("exact receipt credits deposit"),
        WasmResult::Reject(message) => panic!("reconciliation rejected: {message}"),
    }
    let position = get_user_position(&env.pic, env.sp_id, env.test_user).expect("position");
    assert_eq!(position.stablecoin_balances.get(&env.icusd_ledger).copied(), Some(amount));
    assert_eq!(get_pool_status(&env.pic, env.sp_id).total_deposits_e8s, amount);
    let replay = env.pic.update_call(
        env.sp_id,
        env.test_user,
        "reconcile_pending_deposit",
        encode_one(block_index).unwrap(),
    ).expect("submit reconciliation replay");
    if let WasmResult::Reply(bytes) = replay {
        let _: Result<(), StabilityPoolError> = decode_one(&bytes).expect("decode replay");
    }
    assert_eq!(get_pool_status(&env.pic, env.sp_id).total_deposits_e8s, amount);
}

/// The official ledger returns TooOld again for the exact expired identity;
/// the complete archive-aware prefix then permits a fresh identity.
#[test]
fn official_ledger_too_old_and_complete_absence_scan_rotate_identity() {
    let env = setup_deposit_recovery_env();
    let amount = 100_00000000u64;
    let old_timestamp = 1u64;
    let seeded = env.pic.update_call(
        env.sp_id,
        env.test_user,
        "test_seed_unresolved_deposit_intent",
        encode_args((env.icusd_ledger, amount, Some(old_timestamp))).unwrap(),
    ).expect("seed old exact identity");
    match seeded {
        WasmResult::Reply(bytes) => assert_eq!(
            decode_one::<Result<u64, StabilityPoolError>>(&bytes)
                .expect("decode old intent seed").expect("seed old identity"),
            old_timestamp,
        ),
        WasmResult::Reject(message) => panic!("old intent seed rejected: {message}"),
    }
    let log_length_before = icrc3_log_length(&env.pic, env.icusd_ledger);
    let user_balance_before = ledger_balance(&env.pic, env.icusd_ledger, env.test_user);
    let pool_balance_before = ledger_balance(&env.pic, env.icusd_ledger, env.sp_id);
    let args = TransferFromArgs {
        spender_subaccount: None,
        from: Account { owner: env.test_user, subaccount: None },
        to: Account { owner: env.sp_id, subaccount: None },
        amount: candid::Nat::from(amount),
        fee: None,
        memo: None,
        created_at_time: Some(old_timestamp),
    };
    for _ in 0..2 {
        let result = env.pic.update_call(
            env.icusd_ledger,
            env.sp_id,
            "icrc2_transfer_from",
            encode_one(args.clone()).unwrap(),
        ).expect("call official old transfer_from");
        match result {
            WasmResult::Reply(bytes) => match decode_one::<Result<candid::Nat, TransferFromError>>(&bytes)
                .expect("decode old transfer_from")
            {
                Err(TransferFromError::TooOld) => {},
                other => panic!("same expired transfer identity was not TooOld: {other:?}"),
            },
            WasmResult::Reject(message) => panic!("old transfer_from rejected: {message}"),
        }
    }
    assert_eq!(icrc3_log_length(&env.pic, env.icusd_ledger), log_length_before);
    assert_eq!(ledger_balance(&env.pic, env.icusd_ledger, env.test_user), user_balance_before);
    assert_eq!(ledger_balance(&env.pic, env.icusd_ledger, env.sp_id), pool_balance_before);

    let marked = env.pic.update_call(
        env.sp_id,
        env.test_user,
        "test_mark_deposit_intent_too_old",
        encode_args((env.icusd_ledger, amount, old_timestamp)).unwrap(),
    ).expect("persist the observed typed TooOld result");
    match marked {
        WasmResult::Reply(bytes) => assert!(decode_one::<bool>(&bytes).expect("decode typed marker")),
        WasmResult::Reject(message) => panic!("typed TooOld marker rejected: {message}"),
    }
    let scanned = env.pic.update_call(
        env.sp_id,
        env.test_user,
        "reconcile_pending_deposit_history",
        encode_args(()).unwrap(),
    ).expect("run complete archive-aware absence scan");
    match scanned {
        WasmResult::Reply(bytes) => decode_one::<Result<(), StabilityPoolError>>(&bytes)
            .expect("decode absence scan").expect("complete history scan proves absence"),
        WasmResult::Reject(message) => panic!("absence scan rejected: {message}"),
    }
    let pending = env.pic.query_call(
        env.sp_id,
        env.test_user,
        "get_pending_deposit_intent",
        encode_args(()).unwrap(),
    ).expect("query rotated deposit identity");
    let pending: Option<PendingDepositIntent> = match pending {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode rotated intent"),
        WasmResult::Reject(message) => panic!("rotated intent query rejected: {message}"),
    };
    let pending = pending.expect("no-effect scan retains a fresh retry identity");
    assert_ne!(pending.transfer_created_at_time_ns, old_timestamp);
    assert_eq!(pending.attempt_no, Some(1));
    assert_eq!(pending.in_flight_attempts, Some(0));
    assert_eq!(pending.too_old_rejected, None);
    assert_eq!(ledger_balance(&env.pic, env.icusd_ledger, env.test_user), user_balance_before);
    assert_eq!(ledger_balance(&env.pic, env.icusd_ledger, env.sp_id), pool_balance_before);
}

/// Concurrent identical calls share one persisted transfer intent. The ledger
/// may accept the transfer once and return Duplicate for the other dispatch,
/// but the pool must credit only the one physical transfer.
#[test]
fn same_round_identical_deposits_do_not_double_credit_one_transfer() {
    let env = setup_test_env();
    let amount = 100_00000000u64;
    let first = env.pic.submit_call(
        env.sp_id,
        env.test_user,
        "deposit",
        encode_args((env.icusd_ledger, amount)).unwrap(),
    ).expect("first deposit submission failed");
    let second = env.pic.submit_call(
        env.sp_id,
        env.test_user,
        "deposit",
        encode_args((env.icusd_ledger, amount)).unwrap(),
    ).expect("second deposit submission failed");

    for call in [first, second] {
        let result = env.pic.await_call(call)
            .expect("deposit call execution failed");
        match result {
            WasmResult::Reply(bytes) => {
                let result = decode_one::<Result<(), StabilityPoolError>>(&bytes)
                    .expect("decode deposit");
                assert!(
                    result.is_ok() || matches!(&result, Err(StabilityPoolError::SystemBusy)),
                    "unexpected deposit result: {result:?}",
                );
            }
            WasmResult::Reject(message) => panic!("deposit rejected: {message}"),
        }
    }

    let position = get_user_position(&env.pic, env.sp_id, env.test_user)
        .expect("depositor position should exist");
    assert_eq!(
        position.stablecoin_balances.get(&env.icusd_ledger).copied(),
        Some(amount),
    );

    let pool_balance = env.pic.query_call(
        env.icusd_ledger,
        Principal::anonymous(),
        "icrc1_balance_of",
        encode_one(Account { owner: env.sp_id, subaccount: None }).unwrap(),
    ).expect("ledger balance query failed");
    let pool_balance = match pool_balance {
        WasmResult::Reply(bytes) => decode_one::<candid::Nat>(&bytes)
            .expect("decode ledger balance"),
        WasmResult::Reject(message) => panic!("ledger balance query rejected: {message}"),
    };
    assert_eq!(pool_balance, candid::Nat::from(amount));
}

/// A closed 3USD route submitted alongside an ordinary deposit must reject
/// before pulling, so the ordinary deposit remains the only funded operation.
#[test]
fn same_round_closed_3usd_route_does_not_alias_ordinary_deposit_pull() {
    let env = setup_test_env();
    register_stablecoin(&env.pic, env.sp_id, env.admin, StablecoinConfig {
        ledger_id: env.pool_id,
        symbol: "3USD".to_string(),
        decimals: 8,
        priority: 0,
        is_active: true,
        transfer_fee: Some(0),
        is_lp_token: Some(true),
        underlying_pool: Some(env.pool_id),
    });
    let amount = 100_00000000u64;
    let user_icusd_before = ledger_balance(&env.pic, env.icusd_ledger, env.test_user);
    let sp_icusd_before = ledger_balance(&env.pic, env.icusd_ledger, env.sp_id);
    let user_lp_before = query_3pool_lp_balance(&env.pic, env.pool_id, env.test_user);
    let sp_lp_before = query_3pool_lp_balance(&env.pic, env.pool_id, env.sp_id);
    let icusd_log_before = icrc3_log_length(&env.pic, env.icusd_ledger);
    let pool_log_before = icrc3_log_length(&env.pic, env.pool_id);

    // Queue both paths before driving their responses. The 3USD route must
    // reject without entering any ledger or 3pool call; the ordinary path then
    // performs the only input-token pull.
    let routed = env.pic.submit_call(
        env.sp_id,
        env.test_user,
        "deposit_as_3usd",
        encode_args((env.icusd_ledger, amount)).unwrap(),
    ).expect("deposit_as_3usd submission failed");
    let ordinary = env.pic.submit_call(
        env.sp_id,
        env.test_user,
        "deposit",
        encode_args((env.icusd_ledger, amount)).unwrap(),
    ).expect("ordinary deposit submission failed");

    let routed_result = env.pic.await_call(routed)
        .expect("deposit_as_3usd execution failed");
    match routed_result {
        WasmResult::Reply(bytes) => match decode_one::<Result<u64, StabilityPoolError>>(&bytes)
            .expect("decode deposit_as_3usd")
        {
            Err(StabilityPoolError::InterCanisterCallFailed { method, target }) => {
                assert!(method.contains("receipt-backed conversion unavailable"));
                assert!(method.contains("no input tokens were pulled"));
                assert!(target.contains("3pool"));
            }
            other => panic!("expected closed 3USD route, got {other:?}"),
        },
        WasmResult::Reject(message) => panic!("deposit_as_3usd rejected: {message}"),
    }

    let ordinary_result = env.pic.await_call(ordinary)
        .expect("ordinary deposit execution failed");
    match ordinary_result {
        WasmResult::Reply(bytes) => decode_one::<Result<(), StabilityPoolError>>(&bytes)
            .expect("decode ordinary deposit")
            .expect("ordinary deposit failed"),
        WasmResult::Reject(message) => panic!("ordinary deposit rejected: {message}"),
    }

    let position = get_user_position(&env.pic, env.sp_id, env.test_user)
        .expect("depositor position should exist");
    assert_eq!(
        position.stablecoin_balances.get(&env.icusd_ledger).copied(),
        Some(amount),
        "the ordinary position must have a separately funded pull",
    );
    assert_eq!(position.stablecoin_balances.get(&env.pool_id).copied(), None);

    let user_input_after = ledger_balance(&env.pic, env.icusd_ledger, env.test_user);
    let physical_input_balance = ledger_balance(&env.pic, env.icusd_ledger, env.sp_id);
    assert_eq!(
        physical_input_balance,
        sp_icusd_before + u128::from(amount),
        "ordinary deposit must account for its one physical pull",
    );
    assert_eq!(
        user_input_after,
        user_icusd_before - u128::from(amount),
        "only the ordinary deposit should debit the user",
    );
    assert_eq!(query_3pool_lp_balance(&env.pic, env.pool_id, env.test_user), user_lp_before);
    assert_eq!(
        query_3pool_lp_balance(&env.pic, env.pool_id, env.sp_id),
        sp_lp_before,
        "closed 3USD route must not mint LP for the SP",
    );
    assert_eq!(
        icrc3_log_length(&env.pic, env.icusd_ledger),
        icusd_log_before + 1,
        "the ordinary deposit must be the only icUSD ledger transaction",
    );
    assert_eq!(
        icrc3_log_length(&env.pic, env.pool_id),
        pool_log_before,
        "closed route must not write any 3pool transaction",
    );
}

/// Backend inline delivery and its timer retry can overlap or replay after a
/// lost response. The source mint block must credit interest once and survive
/// an SP upgrade so the same backend receipt remains acknowledged without a
/// second depositor credit.
#[test]
fn test_interest_v2_duplicate_receipt_is_idempotent_across_upgrade() {
    let env = setup_test_env();
    let deposit_amount = 100_00000000u64;
    let deposit = env.pic.update_call(
        env.sp_id,
        env.test_user,
        "deposit",
        encode_args((env.icusd_ledger, deposit_amount)).unwrap(),
    ).expect("deposit call");
    match deposit {
        WasmResult::Reply(bytes) => decode_one::<Result<(), StabilityPoolError>>(&bytes)
            .unwrap().expect("deposit succeeds"),
        WasmResult::Reject(message) => panic!("deposit rejected: {message}"),
    }

    let event_count_before = match env.pic.query_call(env.sp_id, Principal::anonymous(), "get_pool_event_count", encode_args(()).unwrap()).unwrap() {
        WasmResult::Reply(bytes) => decode_one::<u64>(&bytes).unwrap(),
        WasmResult::Reject(message) => panic!("event count query rejected: {message}"),
    };

    let notification = || encode_args((env.icusd_ledger, 10_00000000u64, None::<Principal>, 42u64)).unwrap();
    for _ in 0..2 {
        let result = env.pic.update_call(env.sp_id, env.protocol_id, "receive_interest_revenue_v2", notification())
            .expect("interest notification call");
        match result {
            WasmResult::Reply(bytes) => decode_one::<Result<(), StabilityPoolError>>(&bytes)
                .unwrap().expect("notification acknowledged"),
            WasmResult::Reject(message) => panic!("notification rejected: {message}"),
        }
    }

    for altered in [
        encode_args((env.icusd_ledger, 11_00000000u64, None::<Principal>, 42u64)).unwrap(),
        encode_args((env.icusd_ledger, 10_00000000u64, Some(env.pool_id), 42u64)).unwrap(),
        encode_args((env.ckusdc_ledger, 10_00000000u64, None::<Principal>, 42u64)).unwrap(),
    ] {
        let replay = env.pic.update_call(
            env.sp_id, env.protocol_id, "receive_interest_revenue_v2", altered,
        ).expect("altered receipt call");
        match replay {
            WasmResult::Reply(bytes) => assert!(matches!(
                decode_one::<Result<(), StabilityPoolError>>(&bytes).unwrap(),
                Err(StabilityPoolError::SystemBusy),
            ), "same mint block with changed ledger/amount/collateral must remain unacknowledged"),
            WasmResult::Reject(message) => panic!("altered receipt rejected: {message}"),
        }
    }

    let non_icusd = env.pic.update_call(
        env.sp_id,
        env.protocol_id,
        "receive_interest_revenue_v2",
        encode_args((env.ckusdc_ledger, 10_00000000u64, None::<Principal>, 43u64)).unwrap(),
    ).expect("registered non-icUSD interest call");
    match non_icusd {
        WasmResult::Reply(bytes) => assert!(matches!(
            decode_one::<Result<(), StabilityPoolError>>(&bytes).unwrap(),
            Err(StabilityPoolError::TokenNotAccepted { .. }),
        ), "registered non-icUSD ledger must not receive interest"),
        WasmResult::Reject(message) => panic!("non-icUSD interest rejected: {message}"),
    }

    let legacy = env.pic.update_call(
        env.sp_id,
        env.protocol_id,
        "receive_interest_revenue",
        encode_args((env.icusd_ledger, 20_00000000u64, None::<Principal>)).unwrap(),
    ).expect("legacy interest notification call");
    match legacy {
        WasmResult::Reply(bytes) => assert!(matches!(
            decode_one::<Result<(), StabilityPoolError>>(&bytes).unwrap(),
            Err(StabilityPoolError::SystemBusy),
        ), "legacy endpoint must fail closed without a source mint block"),
        WasmResult::Reject(message) => panic!("legacy interest notification rejected: {message}"),
    }
    let credited = get_user_position(&env.pic, env.sp_id, env.test_user).unwrap();
    assert_eq!(*credited.stablecoin_balances.iter().find(|(ledger, _)| **ledger == env.icusd_ledger).unwrap().1, deposit_amount + 10_00000000);
    assert_eq!(credited.total_interest_earned_e8s, 10_00000000);
    let event_count_after = match env.pic.query_call(env.sp_id, Principal::anonymous(), "get_pool_event_count", encode_args(()).unwrap()).unwrap() {
        WasmResult::Reply(bytes) => decode_one::<u64>(&bytes).unwrap(),
        WasmResult::Reject(message) => panic!("event count query rejected: {message}"),
    };
    assert_eq!(event_count_after, event_count_before + 1, "one source receipt emits one interest event");

    let wasm = stability_pool_wasm();
    let upgrade_args = StabilityPoolInitArgs {
        protocol_canister_id: env.protocol_id,
        authorized_admins: vec![env.admin],
    };
    env.pic.upgrade_canister(env.sp_id, wasm, encode_one(upgrade_args).unwrap(), None)
        .expect("upgrade stability pool");
    let replay = env.pic.update_call(env.sp_id, env.protocol_id, "receive_interest_revenue_v2", notification())
        .expect("post-upgrade replay call");
    match replay {
        WasmResult::Reply(bytes) => decode_one::<Result<(), StabilityPoolError>>(&bytes)
            .unwrap().expect("replay acknowledged"),
        WasmResult::Reject(message) => panic!("replay rejected: {message}"),
    }
    let after_upgrade = get_user_position(&env.pic, env.sp_id, env.test_user).unwrap();
    assert_eq!(*after_upgrade.stablecoin_balances.iter().find(|(ledger, _)| **ledger == env.icusd_ledger).unwrap().1, deposit_amount + 10_00000000);
    assert_eq!(after_upgrade.total_interest_earned_e8s, 10_00000000);
    let status = get_pool_status(&env.pic, env.sp_id);
    assert_eq!(status.total_interest_received_e8s, 10_00000000);
    let event_count_after_upgrade = match env.pic.query_call(env.sp_id, Principal::anonymous(), "get_pool_event_count", encode_args(()).unwrap()).unwrap() {
        WasmResult::Reply(bytes) => decode_one::<u64>(&bytes).unwrap(),
        WasmResult::Reject(message) => panic!("event count query rejected: {message}"),
    };
    assert_eq!(event_count_after_upgrade, event_count_after);
}

/// Reconciliation observability: after a clean deposit, the pool's tracked
/// aggregate matches its live ledger balance, `get_ledger_reconciliation`
/// reports it healthy, and the endpoint is admin-gated.
#[test]
fn test_get_ledger_reconciliation_reports_healthy_and_is_admin_gated() {
    let env = setup_test_env();
    let deposit_amount: u64 = 100_00000000; // 100 icUSD

    let result = env
        .pic
        .update_call(
            env.sp_id,
            env.test_user,
            "deposit",
            encode_args((env.icusd_ledger, deposit_amount)).unwrap(),
        )
        .expect("deposit call failed");
    match result {
        WasmResult::Reply(bytes) => decode_one::<Result<(), StabilityPoolError>>(&bytes)
            .unwrap()
            .expect("deposit failed"),
        WasmResult::Reject(msg) => panic!("deposit rejected: {}", msg),
    }

    // Non-admin callers are rejected (the endpoint triggers per-token outcalls).
    let denied = env
        .pic
        .update_call(
            env.sp_id,
            env.test_user,
            "get_ledger_reconciliation",
            encode_args(()).unwrap(),
        )
        .expect("call failed");
    match denied {
        WasmResult::Reply(bytes) => {
            let r: Result<Vec<LedgerReconciliationEntry>, StabilityPoolError> =
                decode_one(&bytes).unwrap();
            assert!(
                matches!(r, Err(StabilityPoolError::Unauthorized)),
                "non-admin must be Unauthorized, got {:?}",
                r
            );
        }
        WasmResult::Reject(msg) => panic!("rejected: {}", msg),
    }

    // Admin sees a healthy, exactly-balanced report for the deposited token.
    let ok = env
        .pic
        .update_call(
            env.sp_id,
            env.admin,
            "get_ledger_reconciliation",
            encode_args(()).unwrap(),
        )
        .expect("call failed");
    let entries: Vec<LedgerReconciliationEntry> = match ok {
        WasmResult::Reply(bytes) => {
            decode_one::<Result<Vec<LedgerReconciliationEntry>, StabilityPoolError>>(&bytes)
                .unwrap()
                .expect("admin reconciliation ok")
        }
        WasmResult::Reject(msg) => panic!("rejected: {}", msg),
    };
    let icusd = entries
        .iter()
        .find(|e| e.ledger == env.icusd_ledger)
        .expect("icUSD entry present");
    assert_eq!(icusd.recorded_e8s, deposit_amount, "recorded == deposit");
    assert_eq!(
        icusd.live_e8s, deposit_amount,
        "live ledger balance == deposit (pool paid no fee; depositor bore it)"
    );
    assert_eq!(icusd.delta_e8s, 0, "no drift");
    assert!(icusd.healthy, "icUSD must reconcile healthy");
    // Every registered token reconciles (all others at 0/0).
    assert!(
        entries.iter().all(|e| e.healthy),
        "no token should show a shortfall on a clean pool: {:?}",
        entries
    );
}

/// Test 2: deposit_as_3usd stays closed before pulling icUSD.
#[test]
fn test_deposit_as_3usd() {
    let env = setup_test_env();

    // First register the 3USD LP token. In PocketIC, the 3pool canister itself
    // tracks LP balances, but for the stability pool to call deposit_as_3usd,
    // it needs a 3USD config registered.
    // The 3pool's LP "ledger" is the pool_id itself (since 3pool implements ICRC-1/2 for LP).
    register_stablecoin(&env.pic, env.sp_id, env.admin, StablecoinConfig {
        ledger_id: env.pool_id, // 3pool canister IS the LP token ledger
        symbol: "3USD".to_string(),
        decimals: 8,
        priority: 0,
        is_active: true,
        transfer_fee: Some(0),
        is_lp_token: Some(true),
        underlying_pool: Some(env.pool_id),
    });

    let deposit_amount: u64 = 1000_00000000; // 1000 icUSD (8 dec)
    assert_3usd_conversion_unavailable_without_debit(&env, env.icusd_ledger, deposit_amount);
}

/// Test 3: the fail-closed conversion gate rejects before validating an LP input.
#[test]
fn test_deposit_as_3usd_closed_for_lp_input_before_movement() {
    let env = setup_test_env();

    register_stablecoin(&env.pic, env.sp_id, env.admin, StablecoinConfig {
        ledger_id: env.pool_id,
        symbol: "3USD".to_string(),
        decimals: 8,
        priority: 0,
        is_active: true,
        transfer_fee: Some(0),
        is_lp_token: Some(true),
        underlying_pool: Some(env.pool_id),
    });

    // The fail-closed gate precedes input-token validation and any transfer.
    assert_3usd_conversion_unavailable_without_debit(&env, env.pool_id, 100_00000000);
}

/// Test 4: authorized_redeem_and_burn on the 3pool works correctly
#[test]
fn test_3pool_authorized_burn() {
    let env = setup_test_env();

    // The stability pool needs LP tokens to burn. Let's give the SP canister some
    // LP tokens by having the test_user transfer LP to it via the 3pool.
    // Use the 3pool's ICRC-1 LP transfer directly; conversion is held closed.

    // First, add the SP as an authorized burn caller on the 3pool
    let result = env.pic.update_call(
        env.pool_id, env.admin, "add_authorized_burn_caller",
        encode_one(env.sp_id).unwrap()
    ).expect("add_authorized_burn_caller call failed");
    match result {
        WasmResult::Reply(_) => {}
        WasmResult::Reject(msg) => panic!("add_authorized_burn_caller rejected: {}", msg),
    }

    // Verify SP is now an authorized burn caller
    let result = env.pic.query_call(
        env.pool_id, Principal::anonymous(), "get_authorized_burn_callers",
        encode_args(()).unwrap()
    ).expect("get_authorized_burn_callers call failed");
    let callers: Vec<Principal> = match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode"),
        WasmResult::Reject(msg) => panic!("get_authorized_burn_callers rejected: {}", msg),
    };
    assert!(callers.contains(&env.sp_id), "SP should be an authorized burn caller");

    // Get the test_user's LP balance
    let lp_result = env.pic.query_call(
        env.pool_id, env.test_user, "get_lp_balance",
        encode_one(env.test_user).unwrap()
    ).expect("get_lp_balance call failed");
    let user_lp: u128 = match lp_result {
        WasmResult::Reply(bytes) => {
            let nat: candid::Nat = decode_one(&bytes).expect("decode lp balance");
            nat.0.try_into().expect("lp balance overflow")
        }
        WasmResult::Reject(msg) => panic!("get_lp_balance rejected: {}", msg),
    };
    assert!(user_lp > 0, "User should have LP tokens from initial add_liquidity");

    // Move existing LP directly to the SP so this test covers the pool's burn
    // authorization path without depending on the held conversion route.
    let lp_to_transfer = user_lp / 2;
    let transfer = TransferArg {
        from_subaccount: None,
        to: Account {
            owner: env.sp_id,
            subaccount: None,
        },
        amount: candid::Nat::from(lp_to_transfer),
        fee: None,
        memo: None,
        created_at_time: None,
    };
    let transfer_result = env
        .pic
        .update_call(env.pool_id, env.test_user, "icrc1_transfer", encode_one(transfer).unwrap())
        .expect("LP transfer call failed");
    match transfer_result {
        WasmResult::Reply(bytes) => {
            let result: Result<candid::Nat, TransferError> = decode_one(&bytes).expect("decode LP transfer");
            result.expect("LP transfer to SP failed");
        }
        WasmResult::Reject(message) => panic!("LP transfer rejected: {message}"),
    }
    let sp_lp = query_3pool_lp_balance(&env.pic, env.pool_id, env.sp_id);
    assert_eq!(sp_lp, lp_to_transfer, "SP LP balance should match direct transfer");

    // Now test authorized_redeem_and_burn: burn half the LP tokens, destroying icUSD
    let burn_lp = sp_lp / 2;
    let pool_status = query_3pool_status(&env.pic, env.pool_id);
    let vp = pool_status.virtual_price;
    // icUSD equivalent = burn_lp * vp / 1e18, but in 8-dec
    // vp is in 1e18. burn_lp is in 8-dec.
    let icusd_equiv = (burn_lp as u128 * vp / 1_000_000_000_000_000_000) as u64;
    println!("Burning {} LP tokens, icUSD equiv = {} (vp={})", burn_lp, icusd_equiv, vp);

    #[derive(CandidType)]
    struct AuthBurnArgs {
        token_ledger: Principal,
        token_amount: u128,
        lp_amount: u128,
        max_slippage_bps: u16,
    }

    let burn_args = AuthBurnArgs {
        token_ledger: env.icusd_ledger,
        token_amount: icusd_equiv as u128,
        lp_amount: burn_lp as u128,
        max_slippage_bps: 100, // 1% tolerance
    };

    let burn_result = env.pic.update_call(
        env.pool_id, env.sp_id, "authorized_redeem_and_burn",
        encode_one(burn_args).unwrap()
    ).expect("authorized_redeem_and_burn call failed");

    match burn_result {
        WasmResult::Reply(bytes) => {
            // The result is Result<RedeemAndBurnResult, ThreePoolError>
            #[derive(CandidType, Deserialize, Debug)]
            struct RedeemAndBurnResult {
                token_amount_burned: u128,
                lp_amount_burned: u128,
                burn_block_index: u64,
            }
            let r: Result<RedeemAndBurnResult, ThreePoolError> = decode_one(&bytes).expect("decode burn result");
            let result = r.expect("authorized_redeem_and_burn failed");
            println!("Burn succeeded: {} token burned, {} LP burned, block {}",
                result.token_amount_burned, result.lp_amount_burned, result.burn_block_index);
            assert_eq!(result.lp_amount_burned, burn_lp as u128);
            assert_eq!(result.token_amount_burned, icusd_equiv as u128);
        }
        WasmResult::Reject(msg) => panic!("authorized_redeem_and_burn rejected: {}", msg),
    }

    // Verify SP's LP balance decreased
    let sp_lp_after = env.pic.query_call(
        env.pool_id, env.sp_id, "get_lp_balance",
        encode_one(env.sp_id).unwrap()
    ).expect("get_lp_balance call failed");
    let sp_lp_after: u128 = match sp_lp_after {
        WasmResult::Reply(bytes) => {
            let nat: candid::Nat = decode_one(&bytes).expect("decode");
            nat.0.try_into().expect("overflow")
        }
        WasmResult::Reject(msg) => panic!("get_lp_balance rejected: {}", msg),
    };
    assert_eq!(sp_lp_after, sp_lp - burn_lp as u128, "SP LP balance should have decreased by burn amount");

    // Verify 3pool icUSD balance decreased (icUSD was destroyed)
    let pool_after = query_3pool_status(&env.pic, env.pool_id);
    assert!(
        pool_after.balances[0] < pool_status.balances[0],
        "3pool icUSD balance should have decreased after burn"
    );
}

/// Test 5: Mixed icUSD + 3USD deposits both track correctly in pool status
#[test]
fn test_mixed_pool_status_balances() {
    let env = setup_test_env();

    // Register 3USD LP token
    register_stablecoin(&env.pic, env.sp_id, env.admin, StablecoinConfig {
        ledger_id: env.pool_id,
        symbol: "3USD".to_string(),
        decimals: 8,
        priority: 0,
        is_active: true,
        transfer_fee: Some(0),
        is_lp_token: Some(true),
        underlying_pool: Some(env.pool_id),
    });

    // Deposit 1000 icUSD directly
    let direct_amount: u64 = 1000_00000000;
    let result = env.pic.update_call(
        env.sp_id, env.test_user, "deposit",
        encode_args((env.icusd_ledger, direct_amount)).unwrap()
    ).expect("deposit call failed");
    match result {
        WasmResult::Reply(bytes) => {
            let r: Result<(), StabilityPoolError> = decode_one(&bytes).expect("decode");
            r.expect("deposit failed");
        }
        WasmResult::Reject(msg) => panic!("deposit rejected: {}", msg),
    }

    // Deposit existing 3USD LP through the ordinary ICRC-2 deposit path.
    let three_usd_amount: u64 = 1000_00000000;
    approve(
        &env.pic,
        env.pool_id,
        env.test_user,
        env.sp_id,
        three_usd_amount as u128,
    );
    let result = env
        .pic
        .update_call(
            env.sp_id,
            env.test_user,
            "deposit",
            encode_args((env.pool_id, three_usd_amount)).unwrap(),
        )
        .expect("3USD LP deposit call failed");
    match result {
        WasmResult::Reply(bytes) => {
            let r: Result<(), StabilityPoolError> = decode_one(&bytes).expect("decode");
            r.expect("3USD LP deposit failed");
        }
        WasmResult::Reject(message) => panic!("3USD LP deposit rejected: {message}"),
    }

    // Pool should have both icUSD and 3USD tracked in stablecoin_balances
    let status = get_pool_status(&env.pic, env.sp_id);
    let icusd_pool_bal = status.stablecoin_balances.iter()
        .find(|(l, _)| **l == env.icusd_ledger).map(|(_, b)| *b).unwrap_or(0);
    let three_usd_pool_bal = status.stablecoin_balances.iter()
        .find(|(l, _)| **l == env.pool_id).map(|(_, b)| *b).unwrap_or(0);

    assert_eq!(icusd_pool_bal, direct_amount, "Pool should track 1000 icUSD");
    assert_eq!(three_usd_pool_bal, three_usd_amount, "Pool should track 3USD LP tokens");
    assert_eq!(status.total_depositors, 1, "Should be 1 depositor");
    println!("Pool balances: {} icUSD, {} 3USD LP", icusd_pool_bal, three_usd_pool_bal);

    // Note: total_deposits_e8s depends on cached virtual prices for LP valuation.
    // PocketIC tick() doesn't process ic_cdk::spawn in timer callbacks, so VP cache is empty.
    // VP-based valuation is covered by unit tests (state::tests::test_total_usd_value_with_lp_token).
}

/// Test 6: Unauthorized caller cannot burn LP tokens
#[test]
fn test_unauthorized_burn_rejected() {
    let env = setup_test_env();

    let random_caller = Principal::self_authenticating(&[99, 99, 99]);

    #[derive(CandidType)]
    struct AuthBurnArgs {
        token_ledger: Principal,
        token_amount: u128,
        lp_amount: u128,
        max_slippage_bps: u16,
    }

    let burn_args = AuthBurnArgs {
        token_ledger: env.icusd_ledger,
        token_amount: 100_00000000,
        lp_amount: 100_00000000,
        max_slippage_bps: 100,
    };

    let result = env.pic.update_call(
        env.pool_id, random_caller, "authorized_redeem_and_burn",
        encode_one(burn_args).unwrap()
    ).expect("authorized_redeem_and_burn call failed");

    match result {
        WasmResult::Reply(bytes) => {
            let r: Result<candid::Nat, ThreePoolError> = decode_one(&bytes).expect("decode");
            assert!(r.is_err(), "Unauthorized caller should be rejected");
        }
        WasmResult::Reject(_) => {} // Also acceptable — transport-level rejection
    }
}

/// Test 7: Mixed deposits (icUSD + 3USD) have correct balances and pool status
#[test]
fn test_mixed_deposit_balances() {
    let env = setup_test_env();

    // Register 3USD
    register_stablecoin(&env.pic, env.sp_id, env.admin, StablecoinConfig {
        ledger_id: env.pool_id,
        symbol: "3USD".to_string(),
        decimals: 8,
        priority: 0,
        is_active: true,
        transfer_fee: Some(0),
        is_lp_token: Some(true),
        underlying_pool: Some(env.pool_id),
    });

    // Deposit icUSD directly
    let icusd_deposit: u64 = 500_00000000;
    let result = env.pic.update_call(
        env.sp_id, env.test_user, "deposit",
        encode_args((env.icusd_ledger, icusd_deposit)).unwrap()
    ).expect("deposit call failed");
    match result {
        WasmResult::Reply(bytes) => {
            let r: Result<(), StabilityPoolError> = decode_one(&bytes).expect("decode");
            r.expect("deposit failed");
        }
        WasmResult::Reject(msg) => panic!("deposit rejected: {}", msg),
    }

    // Deposit existing 3USD LP directly; the separate conversion route is held.
    let three_usd_deposit: u64 = 500_00000000;
    approve(
        &env.pic,
        env.pool_id,
        env.test_user,
        env.sp_id,
        three_usd_deposit as u128,
    );
    let result = env
        .pic
        .update_call(
            env.sp_id,
            env.test_user,
            "deposit",
            encode_args((env.pool_id, three_usd_deposit)).unwrap(),
        )
        .expect("3USD LP deposit call failed");
    match result {
        WasmResult::Reply(bytes) => {
            let r: Result<(), StabilityPoolError> = decode_one(&bytes).expect("decode");
            r.expect("3USD LP deposit failed");
        }
        WasmResult::Reject(message) => panic!("3USD LP deposit rejected: {message}"),
    }

    println!("Mixed deposits: 500 icUSD + {} 3USD LP", three_usd_deposit);

    // Verify user has both token types
    let pos = get_user_position(&env.pic, env.sp_id, env.test_user)
        .expect("user should have a position");

    let icusd_bal = pos.stablecoin_balances.iter()
        .find(|(l, _)| **l == env.icusd_ledger)
        .map(|(_, b)| *b)
        .unwrap_or(0);
    let three_usd_bal = pos.stablecoin_balances.iter()
        .find(|(l, _)| **l == env.pool_id)
        .map(|(_, b)| *b)
        .unwrap_or(0);

    assert_eq!(icusd_bal, icusd_deposit, "icUSD balance should be 500e8");
    assert_eq!(three_usd_bal, three_usd_deposit, "3USD balance should match LP deposited");

    // Verify 3pool transferred the deposited LP to the SP.
    let sp_lp = query_3pool_lp_balance(&env.pic, env.pool_id, env.sp_id);
    assert_eq!(
        sp_lp,
        three_usd_deposit as u128,
        "3pool LP balance should match the direct deposit",
    );
}
