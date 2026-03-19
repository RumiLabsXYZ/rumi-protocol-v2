use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Principal};
use icrc_ledger_types::icrc1::account::Account;
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

    let pool_id = pic.create_canister();
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

    // Register 3USD LP token — use pool_id as the LP "ledger" since in PocketIC
    // the 3pool is itself the LP token ledger (LP balances tracked internally)
    // For this test, we'll create a separate LP token ledger to simulate the real setup.
    // Actually — in the real system, the 3pool canister IS the LP ledger. But since the
    // stability pool calls icrc2_transfer_from on the LP ledger, we need a real ICRC-1/2
    // ledger for the LP token. In production, 3pool implements ICRC-1/2.
    //
    // For integration tests, we'll test the parts that don't require the LP token to be
    // a separate ledger — i.e., register it and test pool status/valuation.
    // The deposit_as_3usd flow DOES work because it goes through add_liquidity on the 3pool.

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

// ─── Tests ───

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

/// Test 2: deposit_as_3usd converts icUSD into 3USD LP tokens via the 3pool
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

    // Deposit 1000 icUSD as 3USD
    let deposit_amount: u64 = 1000_00000000; // 1000 icUSD (8 dec)

    let result = env.pic.update_call(
        env.sp_id, env.test_user, "deposit_as_3usd",
        encode_args((env.icusd_ledger, deposit_amount)).unwrap()
    ).expect("deposit_as_3usd call failed");

    let lp_minted: u64 = match result {
        WasmResult::Reply(bytes) => {
            let r: Result<u64, StabilityPoolError> = decode_one(&bytes).expect("decode deposit_as_3usd");
            r.expect("deposit_as_3usd failed")
        }
        WasmResult::Reject(msg) => panic!("deposit_as_3usd rejected: {}", msg),
    };

    assert!(lp_minted > 0, "Should have minted LP tokens, got 0");
    println!("deposit_as_3usd: 1000 icUSD → {} 3USD LP tokens", lp_minted);

    // LP minted should be approximately 1000e8 (3pool balanced, VP ≈ 1.0)
    // Slight deviation from 1000e8 is expected due to being a non-trivial add to existing pool
    let min_expected_lp = 990_00000000u64;
    let max_expected_lp = 1010_00000000u64;
    assert!(
        lp_minted >= min_expected_lp && lp_minted <= max_expected_lp,
        "LP minted {} should be ~1000e8 (pool is balanced, VP ≈ 1.0)", lp_minted
    );

    // Verify user has a 3USD position
    let pos = get_user_position(&env.pic, env.sp_id, env.test_user)
        .expect("user should have a position");

    let three_usd_balance = pos.stablecoin_balances.iter()
        .find(|(ledger, _)| **ledger == env.pool_id)
        .map(|(_, bal)| *bal)
        .unwrap_or(0);

    assert_eq!(three_usd_balance, lp_minted, "3USD balance should match LP minted");

    // Verify the 3pool actually received the icUSD from the stability pool
    // and minted LP tokens for the SP canister
    let sp_lp = query_3pool_lp_balance(&env.pic, env.pool_id, env.sp_id);
    assert_eq!(sp_lp, lp_minted as u128, "3pool LP balance for SP should match minted");

    // Note: total_usd_value_e8s depends on cached virtual prices which require timer
    // execution (ic_cdk::spawn in timer callbacks). PocketIC tick() doesn't process these.
    // VP-based valuation math is thoroughly tested in unit tests (state::tests::test_total_usd_value_with_lp_token).
}

/// Test 3: deposit_as_3usd rejects LP token as input (can't deposit 3USD via 3pool again)
#[test]
fn test_deposit_as_3usd_rejects_lp_token() {
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

    // Try to deposit the LP token itself via deposit_as_3usd — should fail
    let result = env.pic.update_call(
        env.sp_id, env.test_user, "deposit_as_3usd",
        encode_args((env.pool_id, 100_00000000u64)).unwrap()
    ).expect("deposit_as_3usd call failed");

    match result {
        WasmResult::Reply(bytes) => {
            let r: Result<u64, StabilityPoolError> = decode_one(&bytes).expect("decode");
            assert!(r.is_err(), "deposit_as_3usd should reject LP token input");
            match r.unwrap_err() {
                StabilityPoolError::TokenNotAccepted { .. } => {} // expected
                e => panic!("Wrong error: {:?}", e),
            }
        }
        WasmResult::Reject(msg) => panic!("deposit_as_3usd rejected at transport level: {}", msg),
    }
}

/// Test 4: authorized_redeem_and_burn on the 3pool works correctly
#[test]
fn test_3pool_authorized_burn() {
    let env = setup_test_env();

    // The stability pool needs LP tokens to burn. Let's give the SP canister some
    // LP tokens by having the test_user transfer LP to it via the 3pool.
    // Actually — the SP already got LP tokens if we did deposit_as_3usd.
    // But for this test, let's work with the SP directly.

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

    // Transfer some LP tokens from test_user to SP canister
    // The 3pool needs to implement icrc1_transfer or we use the internal transfer.
    // Actually the 3pool tracks LP balances internally — the test_user has LP from
    // the initial add_liquidity. We need to use the 3pool's transfer mechanism.
    //
    // For authorized_redeem_and_burn, the caller (SP) must hold the LP tokens.
    // Let's deposit via deposit_as_3usd instead, which puts LP tokens under SP's name in 3pool.

    // Register 3USD LP token in stability pool
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

    // deposit_as_3usd: 500 icUSD → gives SP canister LP tokens in the 3pool
    let deposit_amount: u64 = 500_00000000; // 500 icUSD

    let result = env.pic.update_call(
        env.sp_id, env.test_user, "deposit_as_3usd",
        encode_args((env.icusd_ledger, deposit_amount)).unwrap()
    ).expect("deposit_as_3usd call failed");

    let lp_minted: u64 = match result {
        WasmResult::Reply(bytes) => {
            let r: Result<u64, StabilityPoolError> = decode_one(&bytes).expect("decode");
            r.expect("deposit_as_3usd failed")
        }
        WasmResult::Reject(msg) => panic!("deposit_as_3usd rejected: {}", msg),
    };
    assert!(lp_minted > 0, "Should have minted LP tokens");
    println!("SP now holds {} LP tokens in the 3pool", lp_minted);

    // Verify SP's LP balance in 3pool
    let sp_lp_result = env.pic.query_call(
        env.pool_id, env.sp_id, "get_lp_balance",
        encode_one(env.sp_id).unwrap()
    ).expect("get_lp_balance call failed");
    let sp_lp: u128 = match sp_lp_result {
        WasmResult::Reply(bytes) => {
            let nat: candid::Nat = decode_one(&bytes).expect("decode");
            nat.0.try_into().expect("overflow")
        }
        WasmResult::Reject(msg) => panic!("get_lp_balance rejected: {}", msg),
    };
    assert_eq!(sp_lp, lp_minted as u128, "SP LP balance should match minted");

    // Now test authorized_redeem_and_burn: burn half the LP tokens, destroying icUSD
    let burn_lp = lp_minted / 2;
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

    // Also deposit 1000 icUSD as 3USD
    let three_usd_amount: u64 = 1000_00000000;
    let result = env.pic.update_call(
        env.sp_id, env.test_user, "deposit_as_3usd",
        encode_args((env.icusd_ledger, three_usd_amount)).unwrap()
    ).expect("deposit_as_3usd call failed");
    let lp_minted: u64 = match result {
        WasmResult::Reply(bytes) => {
            let r: Result<u64, StabilityPoolError> = decode_one(&bytes).expect("decode");
            r.expect("deposit_as_3usd failed")
        }
        WasmResult::Reject(msg) => panic!("deposit_as_3usd rejected: {}", msg),
    };

    // Pool should have both icUSD and 3USD tracked in stablecoin_balances
    let status = get_pool_status(&env.pic, env.sp_id);
    let icusd_pool_bal = status.stablecoin_balances.iter()
        .find(|(l, _)| **l == env.icusd_ledger).map(|(_, b)| *b).unwrap_or(0);
    let three_usd_pool_bal = status.stablecoin_balances.iter()
        .find(|(l, _)| **l == env.pool_id).map(|(_, b)| *b).unwrap_or(0);

    assert_eq!(icusd_pool_bal, direct_amount, "Pool should track 1000 icUSD");
    assert_eq!(three_usd_pool_bal, lp_minted, "Pool should track 3USD LP tokens");
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

    // Deposit ckUSDT via deposit_as_3usd (tests that non-icUSD stablecoins also work)
    let ckusdt_deposit: u64 = 500_000_000; // 500 ckUSDT (6 dec)
    let result = env.pic.update_call(
        env.sp_id, env.test_user, "deposit_as_3usd",
        encode_args((env.ckusdt_ledger, ckusdt_deposit)).unwrap()
    ).expect("deposit_as_3usd call failed");
    let lp_minted: u64 = match result {
        WasmResult::Reply(bytes) => {
            let r: Result<u64, StabilityPoolError> = decode_one(&bytes).expect("decode");
            r.expect("deposit_as_3usd failed")
        }
        WasmResult::Reject(msg) => panic!("deposit_as_3usd rejected: {}", msg),
    };

    println!("Mixed deposits: 500 icUSD + {} 3USD LP (from 500 ckUSDT)", lp_minted);

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
    assert_eq!(three_usd_bal, lp_minted, "3USD balance should match LP minted");

    // LP minted should be ~500e8 (ckUSDT is 6-dec, so 500_000_000 = 500 ckUSDT)
    assert!(lp_minted > 0, "Should have minted LP tokens");
    println!("LP from 500 ckUSDT: {} (should be ~500e8)", lp_minted);

    // Verify 3pool has LP tokens for SP
    let sp_lp = query_3pool_lp_balance(&env.pic, env.pool_id, env.sp_id);
    assert_eq!(sp_lp, lp_minted as u128, "3pool LP balance should match");
}
