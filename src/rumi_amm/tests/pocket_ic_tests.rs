use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Principal};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::approve::ApproveArgs;
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};

use rumi_amm::types::*;

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

// ─── WASM loaders ───

fn icrc1_ledger_wasm() -> Vec<u8> {
    include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec()
}

fn amm_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_amm.wasm").to_vec()
}

// ─── Test Harness ───

struct TestEnv {
    pic: PocketIc,
    amm_id: Principal,
    token_a_id: Principal, // "3USD" (8 decimals)
    token_b_id: Principal, // "ICP"  (8 decimals)
    admin: Principal,
    user: Principal,
}

fn setup() -> TestEnv {
    let pic = PocketIcBuilder::new()
        .with_application_subnet()
        .build();

    let minting_account = Principal::self_authenticating(&[100, 100, 100]);
    let admin = Principal::self_authenticating(&[5, 6, 7, 8]);
    let user = Principal::self_authenticating(&[1, 2, 3, 4]);

    // Deploy two ICRC-1 ledgers (3USD and ICP mock, both 8 decimals)
    let token_a_id = deploy_ledger(&pic, minting_account, admin, user, "3USD", "3USD", 8, 1_000_000_00000000); // 1M tokens
    let token_b_id = deploy_ledger(&pic, minting_account, admin, user, "ICP", "ICP", 8, 1_000_000_00000000);

    // Deploy AMM canister
    let amm_id = pic.create_canister_with_settings(Some(admin), None);
    pic.add_cycles(amm_id, 2_000_000_000_000);

    let amm_init = AmmInitArgs { admin };
    let encoded = encode_one(amm_init).expect("Failed to encode AMM init args");
    pic.install_canister(amm_id, amm_wasm(), encoded, Some(admin));

    TestEnv { pic, amm_id, token_a_id, token_b_id, admin, user }
}

fn deploy_ledger(
    pic: &PocketIc,
    minting_account: Principal,
    admin: Principal,
    user: Principal,
    name: &str,
    symbol: &str,
    decimals: u8,
    initial_balance: u128,
) -> Principal {
    let ledger_id = pic.create_canister();
    pic.add_cycles(ledger_id, 2_000_000_000_000);

    let init_args = LedgerInitArgs {
        minting_account: Account { owner: minting_account, subaccount: None },
        fee_collector_account: None,
        transfer_fee: candid::Nat::from(0u64), // Zero fees for cleaner testing
        decimals: Some(decimals),
        max_memo_length: Some(32),
        token_name: name.to_string(),
        token_symbol: symbol.to_string(),
        metadata: vec![],
        initial_balances: vec![(
            Account { owner: user, subaccount: None },
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

    let ledger_arg = LedgerArg::Init(init_args);
    let encoded = encode_args((ledger_arg,)).expect("Failed to encode ledger init args");
    pic.install_canister(ledger_id, icrc1_ledger_wasm(), encoded, None);
    ledger_id
}

fn approve_amm(env: &TestEnv, ledger_id: Principal) {
    let approve_args = ApproveArgs {
        from_subaccount: None,
        spender: Account { owner: env.amm_id, subaccount: None },
        amount: candid::Nat::from(u128::MAX),
        expected_allowance: None,
        expires_at: None,
        fee: None,
        memo: None,
        created_at_time: None,
    };

    let result = env.pic
        .update_call(ledger_id, env.user, "icrc2_approve", encode_one(approve_args).unwrap())
        .expect("icrc2_approve call failed");

    match result {
        WasmResult::Reply(_) => {}
        WasmResult::Reject(msg) => panic!("icrc2_approve rejected: {}", msg),
    }
}

// ─── Helper: decode Result<T, AmmError> from Candid ───

fn decode_ok<T: CandidType + for<'de> Deserialize<'de>>(result: WasmResult) -> T {
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<T, AmmError> = decode_one(&bytes).expect("Failed to decode result");
            res.expect("Call returned Err")
        }
        WasmResult::Reject(msg) => panic!("Call rejected: {}", msg),
    }
}

fn decode_err(result: WasmResult) -> AmmError {
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<candid::Nat, AmmError> = decode_one(&bytes).expect("Failed to decode result");
            res.unwrap_err()
        }
        WasmResult::Reject(msg) => panic!("Call rejected (expected Err variant): {}", msg),
    }
}

// ════════════════════════════════════════════════════════════════════════
// Tests
// ════════════════════════════════════════════════════════════════════════

#[test]
fn test_health() {
    let env = setup();
    let result = env.pic
        .query_call(env.amm_id, Principal::anonymous(), "health", encode_args(()).unwrap())
        .expect("health query failed");

    match result {
        WasmResult::Reply(bytes) => {
            let status: String = decode_one(&bytes).expect("Failed to decode health");
            assert!(status.contains("0 pool"), "Should report 0 pools: {}", status);
        }
        WasmResult::Reject(msg) => panic!("health rejected: {}", msg),
    }
}

#[test]
fn test_create_pool() {
    let env = setup();

    // Admin creates a pool
    let args = CreatePoolArgs {
        token_a: env.token_a_id,
        token_b: env.token_b_id,
        fee_bps: 30,
        curve: CurveType::ConstantProduct,
    };

    let result = env.pic
        .update_call(env.amm_id, env.admin, "create_pool", encode_one(args.clone()).unwrap())
        .expect("create_pool call failed");

    let pool_id: String = decode_ok(result);
    assert!(!pool_id.is_empty(), "Pool ID should not be empty");

    // Verify pool exists via get_pool
    let query_result = env.pic
        .query_call(env.amm_id, Principal::anonymous(), "get_pool", encode_one(pool_id.clone()).unwrap())
        .expect("get_pool query failed");

    match query_result {
        WasmResult::Reply(bytes) => {
            let info: Option<PoolInfo> = decode_one(&bytes).expect("Failed to decode PoolInfo");
            let info = info.expect("Pool should exist");
            assert_eq!(info.fee_bps, 30);
            assert_eq!(info.protocol_fee_bps, 0);
            assert_eq!(info.reserve_a, 0);
            assert_eq!(info.reserve_b, 0);
            assert!(!info.paused);
        }
        WasmResult::Reject(msg) => panic!("get_pool rejected: {}", msg),
    }

    // Duplicate pool creation should fail
    let dup_result = env.pic
        .update_call(env.amm_id, env.admin, "create_pool", encode_one(args).unwrap())
        .expect("create_pool call failed");

    match dup_result {
        WasmResult::Reply(bytes) => {
            let res: Result<String, AmmError> = decode_one(&bytes).expect("decode failed");
            assert!(matches!(res, Err(AmmError::PoolAlreadyExists)));
        }
        WasmResult::Reject(msg) => panic!("create_pool rejected: {}", msg),
    }
}

#[test]
fn test_create_pool_unauthorized() {
    let env = setup();

    let args = CreatePoolArgs {
        token_a: env.token_a_id,
        token_b: env.token_b_id,
        fee_bps: 30,
        curve: CurveType::ConstantProduct,
    };

    // Non-admin should fail
    let result = env.pic
        .update_call(env.amm_id, env.user, "create_pool", encode_one(args).unwrap())
        .expect("create_pool call failed");

    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<String, AmmError> = decode_one(&bytes).expect("decode failed");
            assert!(matches!(res, Err(AmmError::PoolCreationClosed)));
        }
        WasmResult::Reject(msg) => panic!("create_pool rejected: {}", msg),
    }
}

/// Helper: create pool and return pool_id
fn create_test_pool(env: &TestEnv) -> String {
    let args = CreatePoolArgs {
        token_a: env.token_a_id,
        token_b: env.token_b_id,
        fee_bps: 30,
        curve: CurveType::ConstantProduct,
    };
    let result = env.pic
        .update_call(env.amm_id, env.admin, "create_pool", encode_one(args).unwrap())
        .expect("create_pool call failed");
    decode_ok(result)
}

#[test]
fn test_add_liquidity_initial() {
    let env = setup();
    let pool_id = create_test_pool(&env);

    // Approve AMM on both ledgers
    approve_amm(&env, env.token_a_id);
    approve_amm(&env, env.token_b_id);

    // Add initial liquidity: 10,000 of each token (8 decimals)
    let amount_a: u128 = 10_000_00000000; // 10k * 1e8
    let amount_b: u128 = 10_000_00000000;

    let result = env.pic
        .update_call(
            env.amm_id, env.user, "add_liquidity",
            encode_args((pool_id.clone(), amount_a, amount_b, 0u128)).unwrap(),
        )
        .expect("add_liquidity call failed");

    let shares: candid::Nat = decode_ok(result);
    let shares_u128: u128 = shares.0.try_into().unwrap();
    assert!(shares_u128 > 0, "Should have received LP shares");

    // Verify reserves
    let query_result = env.pic
        .query_call(env.amm_id, Principal::anonymous(), "get_pool", encode_one(pool_id.clone()).unwrap())
        .expect("get_pool query failed");

    match query_result {
        WasmResult::Reply(bytes) => {
            let info: Option<PoolInfo> = decode_one(&bytes).expect("decode failed");
            let info = info.unwrap();
            assert_eq!(info.reserve_a, amount_a);
            assert_eq!(info.reserve_b, amount_b);
            assert!(info.total_lp_shares > 0);
        }
        WasmResult::Reject(msg) => panic!("get_pool rejected: {}", msg),
    }

    // Verify user LP balance
    let lp_result = env.pic
        .query_call(
            env.amm_id, Principal::anonymous(), "get_lp_balance",
            encode_args((pool_id.clone(), env.user)).unwrap(),
        )
        .expect("get_lp_balance query failed");

    match lp_result {
        WasmResult::Reply(bytes) => {
            let balance: candid::Nat = decode_one(&bytes).expect("decode failed");
            let bal: u128 = balance.0.try_into().unwrap();
            // User shares = total - MINIMUM_LIQUIDITY (1000)
            assert_eq!(bal, shares_u128 - 1000);
        }
        WasmResult::Reject(msg) => panic!("get_lp_balance rejected: {}", msg),
    }
}

#[test]
fn test_swap() {
    let env = setup();
    let pool_id = create_test_pool(&env);

    approve_amm(&env, env.token_a_id);
    approve_amm(&env, env.token_b_id);

    // Add liquidity: 100k of each
    let liq_amount: u128 = 100_000_00000000;
    env.pic.update_call(
        env.amm_id, env.user, "add_liquidity",
        encode_args((pool_id.clone(), liq_amount, liq_amount, 0u128)).unwrap(),
    ).expect("add_liquidity failed");

    // Get quote for swapping 1000 token_a -> token_b
    let swap_in: u128 = 1_000_00000000; // 1000 tokens

    let quote_result = env.pic
        .query_call(
            env.amm_id, Principal::anonymous(), "get_quote",
            encode_args((pool_id.clone(), env.token_a_id, swap_in)).unwrap(),
        )
        .expect("get_quote query failed");

    let quoted_out: u128 = match quote_result {
        WasmResult::Reply(bytes) => {
            let res: Result<candid::Nat, AmmError> = decode_one(&bytes).expect("decode failed");
            let nat = res.expect("get_quote returned Err");
            nat.0.try_into().unwrap()
        }
        WasmResult::Reject(msg) => panic!("get_quote rejected: {}", msg),
    };
    assert!(quoted_out > 0, "Quote should be > 0");
    assert!(quoted_out < swap_in, "Output should be less than input (constant product + fee)");

    // Execute the swap
    let swap_result = env.pic
        .update_call(
            env.amm_id, env.user, "swap",
            encode_args((pool_id.clone(), env.token_a_id, swap_in, 0u128)).unwrap(),
        )
        .expect("swap call failed");

    let swap_res: SwapResult = decode_ok(swap_result);
    assert_eq!(swap_res.amount_out, quoted_out, "Swap output should match quote");
    assert!(swap_res.fee > 0, "Fee should be > 0");

    // Verify reserves changed: one side increased, the other decreased
    let query_result = env.pic
        .query_call(env.amm_id, Principal::anonymous(), "get_pool", encode_one(pool_id.clone()).unwrap())
        .expect("get_pool query failed");

    match query_result {
        WasmResult::Reply(bytes) => {
            let info: Option<PoolInfo> = decode_one(&bytes).expect("decode failed");
            let info = info.unwrap();
            // Pool sorts tokens by principal — find which side got the input
            let (input_reserve, output_reserve) = if info.token_a == env.token_a_id {
                (info.reserve_a, info.reserve_b)
            } else {
                (info.reserve_b, info.reserve_a)
            };
            assert!(input_reserve > liq_amount, "Input side reserve should have increased");
            assert!(output_reserve < liq_amount, "Output side reserve should have decreased");
        }
        WasmResult::Reject(msg) => panic!("get_pool rejected: {}", msg),
    }
}

#[test]
fn test_swap_slippage_protection() {
    let env = setup();
    let pool_id = create_test_pool(&env);

    approve_amm(&env, env.token_a_id);
    approve_amm(&env, env.token_b_id);

    let liq_amount: u128 = 100_000_00000000;
    env.pic.update_call(
        env.amm_id, env.user, "add_liquidity",
        encode_args((pool_id.clone(), liq_amount, liq_amount, 0u128)).unwrap(),
    ).expect("add_liquidity failed");

    // Swap with an impossibly high min_amount_out should fail
    let swap_in: u128 = 1_000_00000000;
    let impossible_min: u128 = swap_in * 2; // Can't get 2x out of constant product

    let result = env.pic
        .update_call(
            env.amm_id, env.user, "swap",
            encode_args((pool_id.clone(), env.token_a_id, swap_in, impossible_min)).unwrap(),
        )
        .expect("swap call failed");

    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<SwapResult, AmmError> = decode_one(&bytes).expect("decode failed");
            assert!(matches!(res, Err(AmmError::InsufficientOutput { .. })));
        }
        WasmResult::Reject(msg) => panic!("swap rejected: {}", msg),
    }
}

#[test]
fn test_remove_liquidity() {
    let env = setup();
    let pool_id = create_test_pool(&env);

    approve_amm(&env, env.token_a_id);
    approve_amm(&env, env.token_b_id);

    let liq_amount: u128 = 50_000_00000000;
    let add_result = env.pic.update_call(
        env.amm_id, env.user, "add_liquidity",
        encode_args((pool_id.clone(), liq_amount, liq_amount, 0u128)).unwrap(),
    ).expect("add_liquidity failed");

    let total_shares: u128 = {
        let nat: candid::Nat = decode_ok(add_result);
        nat.0.try_into().unwrap()
    };
    let user_shares = total_shares - 1000; // MINIMUM_LIQUIDITY locked

    // Remove half the user's shares
    let remove_shares = user_shares / 2;
    let result = env.pic
        .update_call(
            env.amm_id, env.user, "remove_liquidity",
            encode_args((pool_id.clone(), remove_shares, 0u128, 0u128)).unwrap(),
        )
        .expect("remove_liquidity call failed");

    // Decode the tuple result
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<(candid::Nat, candid::Nat), AmmError> =
                decode_one(&bytes).expect("decode failed");
            let (amount_a, amount_b) = res.expect("remove_liquidity returned Err");
            let a: u128 = amount_a.0.try_into().unwrap();
            let b: u128 = amount_b.0.try_into().unwrap();
            assert!(a > 0, "Should have received token_a");
            assert!(b > 0, "Should have received token_b");
            // Should get approximately half the liquidity
            assert!(a > liq_amount / 3 && a < liq_amount / 2 + 1);
            assert!(b > liq_amount / 3 && b < liq_amount / 2 + 1);
        }
        WasmResult::Reject(msg) => panic!("remove_liquidity rejected: {}", msg),
    }

    // Verify remaining LP balance
    let lp_result = env.pic
        .query_call(
            env.amm_id, Principal::anonymous(), "get_lp_balance",
            encode_args((pool_id.clone(), env.user)).unwrap(),
        )
        .expect("get_lp_balance query failed");

    match lp_result {
        WasmResult::Reply(bytes) => {
            let balance: candid::Nat = decode_one(&bytes).expect("decode failed");
            let bal: u128 = balance.0.try_into().unwrap();
            assert_eq!(bal, user_shares - remove_shares);
        }
        WasmResult::Reject(msg) => panic!("get_lp_balance rejected: {}", msg),
    }
}

#[test]
fn test_pause_unpause() {
    let env = setup();
    let pool_id = create_test_pool(&env);

    approve_amm(&env, env.token_a_id);
    approve_amm(&env, env.token_b_id);

    let liq_amount: u128 = 50_000_00000000;
    env.pic.update_call(
        env.amm_id, env.user, "add_liquidity",
        encode_args((pool_id.clone(), liq_amount, liq_amount, 0u128)).unwrap(),
    ).expect("add_liquidity failed");

    // Admin pauses the pool
    let pause_result = env.pic
        .update_call(env.amm_id, env.admin, "pause_pool", encode_one(pool_id.clone()).unwrap())
        .expect("pause_pool failed");

    match pause_result {
        WasmResult::Reply(bytes) => {
            let res: Result<(), AmmError> = decode_one(&bytes).expect("decode failed");
            res.expect("pause_pool returned Err");
        }
        WasmResult::Reject(msg) => panic!("pause_pool rejected: {}", msg),
    }

    // Swap should fail when paused
    let swap_result = env.pic
        .update_call(
            env.amm_id, env.user, "swap",
            encode_args((pool_id.clone(), env.token_a_id, 1_000_00000000u128, 0u128)).unwrap(),
        )
        .expect("swap call failed");

    match swap_result {
        WasmResult::Reply(bytes) => {
            let res: Result<SwapResult, AmmError> = decode_one(&bytes).expect("decode failed");
            assert!(matches!(res, Err(AmmError::PoolPaused)));
        }
        WasmResult::Reject(msg) => panic!("swap rejected: {}", msg),
    }

    // Unpause
    let unpause_result = env.pic
        .update_call(env.amm_id, env.admin, "unpause_pool", encode_one(pool_id.clone()).unwrap())
        .expect("unpause_pool failed");

    match unpause_result {
        WasmResult::Reply(bytes) => {
            let res: Result<(), AmmError> = decode_one(&bytes).expect("decode failed");
            res.expect("unpause_pool returned Err");
        }
        WasmResult::Reject(msg) => panic!("unpause_pool rejected: {}", msg),
    }

    // Swap should work again
    let swap_result = env.pic
        .update_call(
            env.amm_id, env.user, "swap",
            encode_args((pool_id.clone(), env.token_a_id, 1_000_00000000u128, 0u128)).unwrap(),
        )
        .expect("swap call failed");

    match swap_result {
        WasmResult::Reply(bytes) => {
            let res: Result<SwapResult, AmmError> = decode_one(&bytes).expect("decode failed");
            res.expect("Swap should succeed after unpause");
        }
        WasmResult::Reject(msg) => panic!("swap rejected: {}", msg),
    }
}

#[test]
fn test_set_fees() {
    let env = setup();
    let pool_id = create_test_pool(&env);

    // Set fee
    let result = env.pic
        .update_call(env.amm_id, env.admin, "set_fee", encode_args((pool_id.clone(), 50u16)).unwrap())
        .expect("set_fee failed");
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<(), AmmError> = decode_one(&bytes).expect("decode failed");
            res.expect("set_fee returned Err");
        }
        WasmResult::Reject(msg) => panic!("set_fee rejected: {}", msg),
    }

    // Set protocol fee
    let result = env.pic
        .update_call(env.amm_id, env.admin, "set_protocol_fee", encode_args((pool_id.clone(), 2000u16)).unwrap())
        .expect("set_protocol_fee failed");
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<(), AmmError> = decode_one(&bytes).expect("decode failed");
            res.expect("set_protocol_fee returned Err");
        }
        WasmResult::Reject(msg) => panic!("set_protocol_fee rejected: {}", msg),
    }

    // Verify via get_pool
    let query_result = env.pic
        .query_call(env.amm_id, Principal::anonymous(), "get_pool", encode_one(pool_id.clone()).unwrap())
        .expect("get_pool failed");

    match query_result {
        WasmResult::Reply(bytes) => {
            let info: Option<PoolInfo> = decode_one(&bytes).expect("decode failed");
            let info = info.unwrap();
            assert_eq!(info.fee_bps, 50);
            assert_eq!(info.protocol_fee_bps, 2000);
        }
        WasmResult::Reject(msg) => panic!("get_pool rejected: {}", msg),
    }

    // Non-admin should fail
    let result = env.pic
        .update_call(env.amm_id, env.user, "set_fee", encode_args((pool_id.clone(), 10u16)).unwrap())
        .expect("set_fee failed");
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<(), AmmError> = decode_one(&bytes).expect("decode failed");
            assert!(matches!(res, Err(AmmError::Unauthorized)));
        }
        WasmResult::Reject(msg) => panic!("set_fee rejected: {}", msg),
    }
}

#[test]
fn test_get_pools() {
    let env = setup();
    let pool_id = create_test_pool(&env);

    let result = env.pic
        .query_call(env.amm_id, Principal::anonymous(), "get_pools", encode_args(()).unwrap())
        .expect("get_pools failed");

    match result {
        WasmResult::Reply(bytes) => {
            let pools: Vec<PoolInfo> = decode_one(&bytes).expect("decode failed");
            assert_eq!(pools.len(), 1);
            assert_eq!(pools[0].pool_id, pool_id);
        }
        WasmResult::Reject(msg) => panic!("get_pools rejected: {}", msg),
    }
}

#[test]
fn test_swap_invalid_token() {
    let env = setup();
    let pool_id = create_test_pool(&env);

    approve_amm(&env, env.token_a_id);
    approve_amm(&env, env.token_b_id);

    let liq_amount: u128 = 50_000_00000000;
    env.pic.update_call(
        env.amm_id, env.user, "add_liquidity",
        encode_args((pool_id.clone(), liq_amount, liq_amount, 0u128)).unwrap(),
    ).expect("add_liquidity failed");

    // Swap with a bogus token principal
    let bogus_token = Principal::self_authenticating(&[99, 99, 99]);
    let result = env.pic
        .update_call(
            env.amm_id, env.user, "swap",
            encode_args((pool_id.clone(), bogus_token, 1_000_00000000u128, 0u128)).unwrap(),
        )
        .expect("swap call failed");

    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<SwapResult, AmmError> = decode_one(&bytes).expect("decode failed");
            assert!(matches!(res, Err(AmmError::InvalidToken)));
        }
        WasmResult::Reject(msg) => panic!("swap rejected: {}", msg),
    }
}

#[test]
fn test_maintenance_mode() {
    let env = setup();
    let pool_id = create_test_pool(&env);

    approve_amm(&env, env.token_a_id);
    approve_amm(&env, env.token_b_id);

    // Add liquidity while not in maintenance mode
    let liq_amount: u128 = 50_000_00000000;
    env.pic.update_call(
        env.amm_id, env.user, "add_liquidity",
        encode_args((pool_id.clone(), liq_amount, liq_amount, 0u128)).unwrap(),
    ).expect("add_liquidity failed");

    // Enable maintenance mode
    let result = env.pic
        .update_call(env.amm_id, env.admin, "set_maintenance_mode", encode_one(true).unwrap())
        .expect("set_maintenance_mode failed");
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<(), AmmError> = decode_one(&bytes).expect("decode failed");
            res.expect("set_maintenance_mode returned Err");
        }
        WasmResult::Reject(msg) => panic!("set_maintenance_mode rejected: {}", msg),
    }

    // Verify is_maintenance_mode returns true
    let query_result = env.pic
        .query_call(env.amm_id, Principal::anonymous(), "is_maintenance_mode", encode_args(()).unwrap())
        .expect("is_maintenance_mode failed");
    match query_result {
        WasmResult::Reply(bytes) => {
            let mode: bool = decode_one(&bytes).expect("decode failed");
            assert!(mode, "Should be in maintenance mode");
        }
        WasmResult::Reject(msg) => panic!("is_maintenance_mode rejected: {}", msg),
    }

    // Swap should fail
    let swap_result = env.pic
        .update_call(
            env.amm_id, env.user, "swap",
            encode_args((pool_id.clone(), env.token_a_id, 1_000_00000000u128, 0u128)).unwrap(),
        )
        .expect("swap call failed");
    match swap_result {
        WasmResult::Reply(bytes) => {
            let res: Result<SwapResult, AmmError> = decode_one(&bytes).expect("decode failed");
            assert!(matches!(res, Err(AmmError::MaintenanceMode)));
        }
        WasmResult::Reject(msg) => panic!("swap rejected: {}", msg),
    }

    // add_liquidity should fail
    let add_result = env.pic
        .update_call(
            env.amm_id, env.user, "add_liquidity",
            encode_args((pool_id.clone(), 1_000_00000000u128, 1_000_00000000u128, 0u128)).unwrap(),
        )
        .expect("add_liquidity call failed");
    match add_result {
        WasmResult::Reply(bytes) => {
            let res: Result<candid::Nat, AmmError> = decode_one(&bytes).expect("decode failed");
            assert!(matches!(res, Err(AmmError::MaintenanceMode)));
        }
        WasmResult::Reject(msg) => panic!("add_liquidity rejected: {}", msg),
    }

    // remove_liquidity should STILL WORK
    let lp_balance: u128 = {
        let r = env.pic.query_call(
            env.amm_id, Principal::anonymous(), "get_lp_balance",
            encode_args((pool_id.clone(), env.user)).unwrap(),
        ).expect("get_lp_balance failed");
        match r {
            WasmResult::Reply(bytes) => {
                let n: candid::Nat = decode_one(&bytes).expect("decode failed");
                n.0.try_into().unwrap()
            }
            WasmResult::Reject(msg) => panic!("get_lp_balance rejected: {}", msg),
        }
    };

    let remove_shares = lp_balance / 4;
    let remove_result = env.pic
        .update_call(
            env.amm_id, env.user, "remove_liquidity",
            encode_args((pool_id.clone(), remove_shares, 0u128, 0u128)).unwrap(),
        )
        .expect("remove_liquidity call failed");
    match remove_result {
        WasmResult::Reply(bytes) => {
            let res: Result<(candid::Nat, candid::Nat), AmmError> = decode_one(&bytes).expect("decode failed");
            res.expect("remove_liquidity should succeed in maintenance mode");
        }
        WasmResult::Reject(msg) => panic!("remove_liquidity rejected: {}", msg),
    }

    // Disable maintenance mode
    env.pic.update_call(env.amm_id, env.admin, "set_maintenance_mode", encode_one(false).unwrap())
        .expect("set_maintenance_mode failed");

    // Swap should work again
    let swap_result = env.pic
        .update_call(
            env.amm_id, env.user, "swap",
            encode_args((pool_id.clone(), env.token_a_id, 1_000_00000000u128, 0u128)).unwrap(),
        )
        .expect("swap call failed");
    match swap_result {
        WasmResult::Reply(bytes) => {
            let res: Result<SwapResult, AmmError> = decode_one(&bytes).expect("decode failed");
            res.expect("Swap should succeed after disabling maintenance mode");
        }
        WasmResult::Reject(msg) => panic!("swap rejected: {}", msg),
    }
}

#[test]
fn test_permissionless_pool_creation() {
    let env = setup();

    // Open pool creation
    let result = env.pic
        .update_call(env.amm_id, env.admin, "set_pool_creation_open", encode_one(true).unwrap())
        .expect("set_pool_creation_open failed");
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<(), AmmError> = decode_one(&bytes).expect("decode failed");
            res.expect("set_pool_creation_open returned Err");
        }
        WasmResult::Reject(msg) => panic!("set_pool_creation_open rejected: {}", msg),
    }

    // User creates a pool with valid fee
    let args = CreatePoolArgs {
        token_a: env.token_a_id,
        token_b: env.token_b_id,
        fee_bps: 30,
        curve: CurveType::ConstantProduct,
    };
    let result = env.pic
        .update_call(env.amm_id, env.user, "create_pool", encode_one(args).unwrap())
        .expect("create_pool failed");
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<String, AmmError> = decode_one(&bytes).expect("decode failed");
            res.expect("Permissionless pool creation should succeed");
        }
        WasmResult::Reject(msg) => panic!("create_pool rejected: {}", msg),
    }

    // User tries fee_bps = 0 — should fail
    let extra_a = Principal::self_authenticating(&[10, 11, 12]);
    let extra_b = Principal::self_authenticating(&[13, 14, 15]);
    let args_zero_fee = CreatePoolArgs {
        token_a: extra_a,
        token_b: extra_b,
        fee_bps: 0,
        curve: CurveType::ConstantProduct,
    };
    let result = env.pic
        .update_call(env.amm_id, env.user, "create_pool", encode_one(args_zero_fee).unwrap())
        .expect("create_pool failed");
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<String, AmmError> = decode_one(&bytes).expect("decode failed");
            assert!(matches!(res, Err(AmmError::FeeBpsOutOfRange)));
        }
        WasmResult::Reject(msg) => panic!("create_pool rejected: {}", msg),
    }

    // User tries fee_bps = 1001 — should fail
    let args_high_fee = CreatePoolArgs {
        token_a: extra_a,
        token_b: extra_b,
        fee_bps: 1001,
        curve: CurveType::ConstantProduct,
    };
    let result = env.pic
        .update_call(env.amm_id, env.user, "create_pool", encode_one(args_high_fee).unwrap())
        .expect("create_pool failed");
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<String, AmmError> = decode_one(&bytes).expect("decode failed");
            assert!(matches!(res, Err(AmmError::FeeBpsOutOfRange)));
        }
        WasmResult::Reject(msg) => panic!("create_pool rejected: {}", msg),
    }
}

#[test]
fn test_set_fee_validation() {
    let env = setup();
    let pool_id = create_test_pool(&env);

    // fee_bps > 10_000 should fail
    let result = env.pic
        .update_call(env.amm_id, env.admin, "set_fee", encode_args((pool_id.clone(), 10_001u16)).unwrap())
        .expect("set_fee failed");
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<(), AmmError> = decode_one(&bytes).expect("decode failed");
            assert!(matches!(res, Err(AmmError::FeeBpsOutOfRange)));
        }
        WasmResult::Reject(msg) => panic!("set_fee rejected: {}", msg),
    }

    // protocol_fee_bps > 10_000 should fail
    let result = env.pic
        .update_call(env.amm_id, env.admin, "set_protocol_fee", encode_args((pool_id.clone(), 10_001u16)).unwrap())
        .expect("set_protocol_fee failed");
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<(), AmmError> = decode_one(&bytes).expect("decode failed");
            assert!(matches!(res, Err(AmmError::FeeBpsOutOfRange)));
        }
        WasmResult::Reject(msg) => panic!("set_protocol_fee rejected: {}", msg),
    }

    // 10_000 exactly should succeed (it's valid — 100% fee)
    let result = env.pic
        .update_call(env.amm_id, env.admin, "set_fee", encode_args((pool_id.clone(), 10_000u16)).unwrap())
        .expect("set_fee failed");
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<(), AmmError> = decode_one(&bytes).expect("decode failed");
            res.expect("10_000 bps should be valid");
        }
        WasmResult::Reject(msg) => panic!("set_fee rejected: {}", msg),
    }
}
