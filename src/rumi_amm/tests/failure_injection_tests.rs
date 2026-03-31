// Failure-injection tests for the Rumi AMM.
//
// These tests use a "flaky ledger" canister that can be configured to fail
// transfers on demand, allowing us to exercise partial-failure recovery paths
// that are impossible to test with real ICRC-1 ledgers (which don't fail when
// balances and allowances are correct).
//
// Scenarios covered:
//   1. swap: input transfer OK, output transfer fails → state rollback
//   2. add_liquidity: token_a transfer OK, token_b fails → refund token_a
//   3. remove_liquidity: shares burned, output transfer(s) fail → protocol-conservative
//   4. add_liquidity with real ledgers: token_b not approved → refund token_a

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::approve::ApproveArgs;
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};

use rumi_amm::types::*;

// ─── WASM loaders ───

fn amm_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_amm.wasm").to_vec()
}

fn flaky_ledger_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/flaky_ledger.wasm").to_vec()
}

fn icrc1_ledger_wasm() -> Vec<u8> {
    include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec()
}

// ─── Flaky ledger Account type for Candid encoding ───

#[derive(CandidType, Clone, Debug, Deserialize)]
struct FlakyAccount {
    owner: Principal,
    subaccount: Option<[u8; 32]>,
}

// ─── ICRC-1 ledger init types (for the real ledger test) ───

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

#[derive(CandidType, Deserialize)]
enum MetadataValue {
    Nat(Nat),
    Int(candid::Int),
    Text(String),
    Blob(Vec<u8>),
}

#[derive(CandidType, Deserialize)]
enum LedgerArg {
    Init(LedgerInitArgs),
}

// ─── Test Harness ───

struct FlakyTestEnv {
    pic: PocketIc,
    amm_id: Principal,
    token_a_id: Principal,
    token_b_id: Principal,
    admin: Principal,
    user: Principal,
}

fn setup_flaky() -> FlakyTestEnv {
    let pic = PocketIcBuilder::new()
        .with_application_subnet()
        .build();

    let admin = Principal::self_authenticating(&[5, 6, 7, 8]);
    let user = Principal::self_authenticating(&[1, 2, 3, 4]);

    // Deploy two flaky ledgers
    let token_a_id = deploy_flaky_ledger(&pic);
    let token_b_id = deploy_flaky_ledger(&pic);

    // Deploy AMM
    let amm_id = pic.create_canister_with_settings(Some(admin), None);
    pic.add_cycles(amm_id, 2_000_000_000_000);
    let amm_init = AmmInitArgs { admin };
    pic.install_canister(amm_id, amm_wasm(), encode_one(amm_init).unwrap(), Some(admin));

    // Mint tokens to user
    let user_account = FlakyAccount { owner: user, subaccount: None };
    let mint_amount = Nat::from(1_000_000_00000000u128); // 1M tokens

    pic.update_call(token_a_id, Principal::anonymous(), "mint",
        encode_args((user_account.clone(), mint_amount.clone())).unwrap())
        .expect("mint token_a failed");
    pic.update_call(token_b_id, Principal::anonymous(), "mint",
        encode_args((user_account, mint_amount)).unwrap())
        .expect("mint token_b failed");

    // User approves the AMM on both flaky ledgers
    approve_flaky(&pic, token_a_id, user, amm_id);
    approve_flaky(&pic, token_b_id, user, amm_id);

    FlakyTestEnv { pic, amm_id, token_a_id, token_b_id, admin, user }
}

fn deploy_flaky_ledger(pic: &PocketIc) -> Principal {
    let id = pic.create_canister();
    pic.add_cycles(id, 2_000_000_000_000);
    pic.install_canister(id, flaky_ledger_wasm(), encode_one(()).unwrap(), None);
    id
}

fn approve_flaky(pic: &PocketIc, ledger_id: Principal, user: Principal, spender: Principal) {
    #[derive(CandidType)]
    struct FlakyApproveArgs {
        from_subaccount: Option<[u8; 32]>,
        spender: FlakyAccount,
        amount: Nat,
        expected_allowance: Option<Nat>,
        expires_at: Option<u64>,
        fee: Option<Nat>,
        memo: Option<Vec<u8>>,
        created_at_time: Option<u64>,
    }

    let args = FlakyApproveArgs {
        from_subaccount: None,
        spender: FlakyAccount { owner: spender, subaccount: None },
        amount: Nat::from(u128::MAX),
        expected_allowance: None,
        expires_at: None,
        fee: None,
        memo: None,
        created_at_time: None,
    };

    let result = pic.update_call(ledger_id, user, "icrc2_approve", encode_one(args).unwrap())
        .expect("approve failed");
    match result {
        WasmResult::Reply(_) => {}
        WasmResult::Reject(msg) => panic!("approve rejected: {}", msg),
    }
}

fn create_pool(env: &FlakyTestEnv) -> String {
    let args = CreatePoolArgs {
        token_a: env.token_a_id,
        token_b: env.token_b_id,
        fee_bps: 30,
        curve: CurveType::ConstantProduct,
    };
    let result = env.pic
        .update_call(env.amm_id, env.admin, "create_pool", encode_one(args).unwrap())
        .expect("create_pool failed");
    decode_ok::<String>(result)
}

fn add_initial_liquidity(env: &FlakyTestEnv, pool_id: &str, amount: u128) {
    let result = env.pic
        .update_call(env.amm_id, env.user, "add_liquidity",
            encode_args((pool_id.to_string(), amount, amount, 0u128)).unwrap())
        .expect("add_liquidity failed");
    let _shares: Nat = decode_ok(result);
}

fn set_fail_transfers(env: &FlakyTestEnv, ledger_id: Principal, fail: bool) {
    env.pic.update_call(ledger_id, Principal::anonymous(), "set_fail_transfers",
        encode_one(fail).unwrap())
        .expect("set_fail_transfers failed");
}

fn set_fail_transfer_from(env: &FlakyTestEnv, ledger_id: Principal, fail: bool) {
    env.pic.update_call(ledger_id, Principal::anonymous(), "set_fail_transfer_from",
        encode_one(fail).unwrap())
        .expect("set_fail_transfer_from failed");
}

fn get_pool_info(env: &FlakyTestEnv, pool_id: &str) -> Option<PoolInfo> {
    let result = env.pic
        .query_call(env.amm_id, Principal::anonymous(), "get_pool",
            encode_one(pool_id.to_string()).unwrap())
        .expect("get_pool failed");
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode PoolInfo failed"),
        WasmResult::Reject(msg) => panic!("get_pool rejected: {}", msg),
    }
}

fn get_user_lp_balance(env: &FlakyTestEnv, pool_id: &str) -> u128 {
    let result = env.pic
        .query_call(env.amm_id, Principal::anonymous(), "get_lp_balance",
            encode_args((pool_id.to_string(), env.user)).unwrap())
        .expect("get_lp_balance failed");
    match result {
        WasmResult::Reply(bytes) => {
            let n: Nat = decode_one(&bytes).expect("decode Nat failed");
            n.0.try_into().unwrap()
        }
        WasmResult::Reject(msg) => panic!("get_lp_balance rejected: {}", msg),
    }
}

fn get_flaky_balance(env: &FlakyTestEnv, ledger_id: Principal, owner: Principal, subaccount: Option<[u8; 32]>) -> u128 {
    let account = FlakyAccount { owner, subaccount };
    let result = env.pic
        .query_call(ledger_id, Principal::anonymous(), "icrc1_balance_of",
            encode_one(account).unwrap())
        .expect("balance_of failed");
    match result {
        WasmResult::Reply(bytes) => {
            let n: Nat = decode_one(&bytes).expect("decode Nat failed");
            n.0.try_into().unwrap()
        }
        WasmResult::Reject(msg) => panic!("balance_of rejected: {}", msg),
    }
}

fn decode_ok<T: CandidType + for<'de> Deserialize<'de>>(result: WasmResult) -> T {
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<T, AmmError> = decode_one(&bytes).expect("Failed to decode result");
            res.expect("Call returned Err")
        }
        WasmResult::Reject(msg) => panic!("Call rejected: {}", msg),
    }
}

fn decode_amm_err(result: WasmResult) -> AmmError {
    match result {
        WasmResult::Reply(bytes) => {
            // Try decoding as different Result types
            if let Ok(res) = decode_one::<Result<Nat, AmmError>>(&bytes) {
                return res.unwrap_err();
            }
            if let Ok(res) = decode_one::<Result<SwapResult, AmmError>>(&bytes) {
                return res.unwrap_err();
            }
            if let Ok(res) = decode_one::<Result<(Nat, Nat), AmmError>>(&bytes) {
                return res.unwrap_err();
            }
            panic!("Could not decode AmmError from reply");
        }
        WasmResult::Reject(msg) => panic!("Call rejected (expected Err variant): {}", msg),
    }
}

// ════════════════════════════════════════════════════════════════════════
// Test 1: Swap — output transfer fails, state should rollback
// ════════════════════════════════════════════════════════════════════════

#[test]
fn test_swap_output_transfer_failure_rollback() {
    let env = setup_flaky();
    let pool_id = create_pool(&env);
    let liq_amount: u128 = 100_000_00000000;

    // Add liquidity (both transfers succeed)
    add_initial_liquidity(&env, &pool_id, liq_amount);

    // Snapshot pool state before swap
    let pool_before = get_pool_info(&env, &pool_id).unwrap();

    // Now make the OUTPUT ledger fail icrc1_transfer
    // Determine which token is token_a in the pool (sorted by principal)
    let pool_token_a = pool_before.token_a;
    let pool_token_b = pool_before.token_b;

    // We'll swap token_a -> token_b, so the output transfer is on token_b's ledger
    let output_ledger = pool_token_b;
    set_fail_transfers(&env, output_ledger, true);

    // Attempt the swap
    let swap_in: u128 = 1_000_00000000;
    let result = env.pic
        .update_call(env.amm_id, env.user, "swap",
            encode_args((pool_id.clone(), pool_token_a, swap_in, 0u128)).unwrap())
        .expect("swap call failed");

    // Should return TransferFailed for output
    let err = decode_amm_err(result);
    match &err {
        AmmError::TransferFailed { token, .. } => {
            assert_eq!(token, "output", "Should be output transfer failure");
        }
        other => panic!("Expected TransferFailed for output, got: {:?}", other),
    }

    // Verify pool state is unchanged (rollback worked)
    let pool_after = get_pool_info(&env, &pool_id).unwrap();
    assert_eq!(pool_before.reserve_a, pool_after.reserve_a,
        "reserve_a should be unchanged after rollback");
    assert_eq!(pool_before.reserve_b, pool_after.reserve_b,
        "reserve_b should be unchanged after rollback");
    assert_eq!(pool_before.total_lp_shares, pool_after.total_lp_shares,
        "LP shares should be unchanged");
}

// ════════════════════════════════════════════════════════════════════════
// Test 2: Add liquidity — token_b transfer fails, token_a should be refunded
// ════════════════════════════════════════════════════════════════════════

#[test]
fn test_add_liquidity_token_b_failure_refunds_token_a() {
    let env = setup_flaky();
    let pool_id = create_pool(&env);

    // First add some initial liquidity so pool isn't empty
    let init_liq: u128 = 50_000_00000000;
    add_initial_liquidity(&env, &pool_id, init_liq);

    // Record user's token balances and pool state before the failing add
    let user_balance_a_before = get_flaky_balance(&env, env.token_a_id, env.user, None);
    let user_balance_b_before = get_flaky_balance(&env, env.token_b_id, env.user, None);
    let pool_before = get_pool_info(&env, &pool_id).unwrap();
    let lp_before = get_user_lp_balance(&env, &pool_id);

    // Make token_b's transfer_from fail
    let pool_token_b = pool_before.token_b;
    set_fail_transfer_from(&env, pool_token_b, true);

    // Attempt to add liquidity — token_a transfer should succeed, token_b should fail
    let add_amount: u128 = 10_000_00000000;
    let result = env.pic
        .update_call(env.amm_id, env.user, "add_liquidity",
            encode_args((pool_id.clone(), add_amount, add_amount, 0u128)).unwrap())
        .expect("add_liquidity call failed");

    // Should return error
    let err = decode_amm_err(result);
    match &err {
        AmmError::TransferFailed { token, .. } => {
            assert_eq!(token, "token_b", "Should report token_b failure");
        }
        other => panic!("Expected TransferFailed for token_b, got: {:?}", other),
    }

    // Verify user's token_a was refunded (balance should be same as before)
    let user_balance_a_after = get_flaky_balance(&env, env.token_a_id, env.user, None);
    assert_eq!(user_balance_a_before, user_balance_a_after,
        "User's token_a balance should be restored after refund");

    // Verify user's token_b was never taken
    let user_balance_b_after = get_flaky_balance(&env, env.token_b_id, env.user, None);
    assert_eq!(user_balance_b_before, user_balance_b_after,
        "User's token_b balance should be unchanged");

    // Verify pool state is unchanged
    let pool_after = get_pool_info(&env, &pool_id).unwrap();
    assert_eq!(pool_before.reserve_a, pool_after.reserve_a, "reserve_a should be unchanged");
    assert_eq!(pool_before.reserve_b, pool_after.reserve_b, "reserve_b should be unchanged");
    assert_eq!(pool_before.total_lp_shares, pool_after.total_lp_shares, "total shares unchanged");

    // Verify no LP shares were minted
    let lp_after = get_user_lp_balance(&env, &pool_id);
    assert_eq!(lp_before, lp_after, "No new LP shares should have been minted");
}

// ════════════════════════════════════════════════════════════════════════
// Test 3: Remove liquidity — transfers fail, shares burned (protocol-conservative)
// ════════════════════════════════════════════════════════════════════════

#[test]
fn test_remove_liquidity_transfer_failure_burns_shares() {
    let env = setup_flaky();
    let pool_id = create_pool(&env);
    let liq_amount: u128 = 50_000_00000000;

    add_initial_liquidity(&env, &pool_id, liq_amount);

    let pool_before = get_pool_info(&env, &pool_id).unwrap();
    let lp_before = get_user_lp_balance(&env, &pool_id);
    assert!(lp_before > 0, "User should have LP shares");

    // Make BOTH ledgers fail icrc1_transfer (so neither token_a nor token_b can be sent out)
    set_fail_transfers(&env, env.token_a_id, true);
    set_fail_transfers(&env, env.token_b_id, true);

    // Attempt to remove half the user's shares
    let remove_shares = lp_before / 2;
    let result = env.pic
        .update_call(env.amm_id, env.user, "remove_liquidity",
            encode_args((pool_id.clone(), remove_shares, 0u128, 0u128)).unwrap())
        .expect("remove_liquidity call failed");

    // Should return TransferFailed
    let err = decode_amm_err(result);
    match &err {
        AmmError::TransferFailed { token, reason } => {
            assert_eq!(token, "output");
            assert!(reason.contains("token_a") && reason.contains("token_b"),
                "Should report both failures: {}", reason);
        }
        other => panic!("Expected TransferFailed, got: {:?}", other),
    }

    // Verify shares WERE burned (protocol-conservative design)
    let lp_after = get_user_lp_balance(&env, &pool_id);
    assert_eq!(lp_after, lp_before - remove_shares,
        "Shares should have been burned even though transfers failed");

    // Verify reserves were decremented (tokens are in subaccount, admin can reconcile)
    let pool_after = get_pool_info(&env, &pool_id).unwrap();
    assert!(pool_after.reserve_a < pool_before.reserve_a,
        "reserve_a should have decreased");
    assert!(pool_after.reserve_b < pool_before.reserve_b,
        "reserve_b should have decreased");
}

// ════════════════════════════════════════════════════════════════════════
// Test 4: Swap — input transfer fails (simple early-exit, no partial state)
// ════════════════════════════════════════════════════════════════════════

#[test]
fn test_swap_input_transfer_failure_no_state_change() {
    let env = setup_flaky();
    let pool_id = create_pool(&env);
    let liq_amount: u128 = 100_000_00000000;

    add_initial_liquidity(&env, &pool_id, liq_amount);

    let pool_before = get_pool_info(&env, &pool_id).unwrap();

    // Make token_a's transfer_from fail (so the input pull fails)
    let pool_token_a = pool_before.token_a;
    set_fail_transfer_from(&env, pool_token_a, true);

    // Attempt swap
    let swap_in: u128 = 1_000_00000000;
    let result = env.pic
        .update_call(env.amm_id, env.user, "swap",
            encode_args((pool_id.clone(), pool_token_a, swap_in, 0u128)).unwrap())
        .expect("swap call failed");

    let err = decode_amm_err(result);
    match &err {
        AmmError::TransferFailed { token, .. } => {
            assert_eq!(token, "input", "Should be input transfer failure");
        }
        other => panic!("Expected TransferFailed for input, got: {:?}", other),
    }

    // Pool state should be completely unchanged
    let pool_after = get_pool_info(&env, &pool_id).unwrap();
    assert_eq!(pool_before.reserve_a, pool_after.reserve_a);
    assert_eq!(pool_before.reserve_b, pool_after.reserve_b);
}

// ════════════════════════════════════════════════════════════════════════
// Test 5: Add liquidity — token_a fails (simple early-exit)
// ════════════════════════════════════════════════════════════════════════

#[test]
fn test_add_liquidity_token_a_failure_no_state_change() {
    let env = setup_flaky();
    let pool_id = create_pool(&env);
    let liq_amount: u128 = 50_000_00000000;

    add_initial_liquidity(&env, &pool_id, liq_amount);

    let pool_before = get_pool_info(&env, &pool_id).unwrap();
    let lp_before = get_user_lp_balance(&env, &pool_id);

    // Make token_a's transfer_from fail
    let pool_token_a = pool_before.token_a;
    set_fail_transfer_from(&env, pool_token_a, true);

    let add_amount: u128 = 10_000_00000000;
    let result = env.pic
        .update_call(env.amm_id, env.user, "add_liquidity",
            encode_args((pool_id.clone(), add_amount, add_amount, 0u128)).unwrap())
        .expect("add_liquidity call failed");

    let err = decode_amm_err(result);
    match &err {
        AmmError::TransferFailed { token, .. } => {
            assert_eq!(token, "token_a", "Should report token_a failure");
        }
        other => panic!("Expected TransferFailed for token_a, got: {:?}", other),
    }

    let pool_after = get_pool_info(&env, &pool_id).unwrap();
    assert_eq!(pool_before.reserve_a, pool_after.reserve_a);
    assert_eq!(pool_before.reserve_b, pool_after.reserve_b);
    assert_eq!(lp_before, get_user_lp_balance(&env, &pool_id));
}

// ════════════════════════════════════════════════════════════════════════
// Test 6: Real ledger — add_liquidity with unapproved token_b
// ════════════════════════════════════════════════════════════════════════

struct RealLedgerEnv {
    pic: PocketIc,
    amm_id: Principal,
    token_a_id: Principal,
    token_b_id: Principal,
    admin: Principal,
    user: Principal,
}

fn setup_real_ledger_partial() -> RealLedgerEnv {
    let pic = PocketIcBuilder::new()
        .with_application_subnet()
        .build();

    let minting_account = Principal::self_authenticating(&[100, 100, 100]);
    let admin = Principal::self_authenticating(&[5, 6, 7, 8]);
    let user = Principal::self_authenticating(&[1, 2, 3, 4]);

    let token_a_id = deploy_real_ledger(&pic, minting_account, admin, user, "TokenA", "TKA");
    let token_b_id = deploy_real_ledger(&pic, minting_account, admin, user, "TokenB", "TKB");

    let amm_id = pic.create_canister_with_settings(Some(admin), None);
    pic.add_cycles(amm_id, 2_000_000_000_000);
    let amm_init = AmmInitArgs { admin };
    pic.install_canister(amm_id, amm_wasm(), encode_one(amm_init).unwrap(), Some(admin));

    RealLedgerEnv { pic, amm_id, token_a_id, token_b_id, admin, user }
}

fn deploy_real_ledger(
    pic: &PocketIc,
    minting_account: Principal,
    admin: Principal,
    user: Principal,
    name: &str,
    symbol: &str,
) -> Principal {
    let ledger_id = pic.create_canister();
    pic.add_cycles(ledger_id, 2_000_000_000_000);

    let init_args = LedgerInitArgs {
        minting_account: Account { owner: minting_account, subaccount: None },
        fee_collector_account: None,
        transfer_fee: Nat::from(0u64),
        decimals: Some(8),
        max_memo_length: Some(32),
        token_name: name.to_string(),
        token_symbol: symbol.to_string(),
        metadata: vec![],
        initial_balances: vec![(
            Account { owner: user, subaccount: None },
            Nat::from(1_000_000_00000000u128),
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
    pic.install_canister(ledger_id, icrc1_ledger_wasm(), encode_args((ledger_arg,)).unwrap(), None);
    ledger_id
}

fn approve_real(pic: &PocketIc, ledger_id: Principal, user: Principal, spender: Principal) {
    let approve_args = ApproveArgs {
        from_subaccount: None,
        spender: Account { owner: spender, subaccount: None },
        amount: Nat::from(u128::MAX),
        expected_allowance: None,
        expires_at: None,
        fee: None,
        memo: None,
        created_at_time: None,
    };
    let result = pic.update_call(ledger_id, user, "icrc2_approve", encode_one(approve_args).unwrap())
        .expect("approve failed");
    match result {
        WasmResult::Reply(_) => {}
        WasmResult::Reject(msg) => panic!("approve rejected: {}", msg),
    }
}

#[test]
fn test_add_liquidity_real_ledger_unapproved_token_b() {
    let env = setup_real_ledger_partial();

    // Create pool
    let args = CreatePoolArgs {
        token_a: env.token_a_id,
        token_b: env.token_b_id,
        fee_bps: 30,
        curve: CurveType::ConstantProduct,
    };
    let result = env.pic
        .update_call(env.amm_id, env.admin, "create_pool", encode_one(args).unwrap())
        .expect("create_pool failed");
    let pool_id: String = decode_ok(result);

    // Approve ONLY token_a (not token_b)
    approve_real(&env.pic, env.token_a_id, env.user, env.amm_id);
    // Intentionally NOT approving token_b

    // Also need initial liquidity for this pool... but we can't add it without both tokens.
    // Let's approve both, add initial liquidity, then revoke token_b.
    approve_real(&env.pic, env.token_b_id, env.user, env.amm_id);

    let init_liq: u128 = 50_000_00000000;
    let add_result = env.pic
        .update_call(env.amm_id, env.user, "add_liquidity",
            encode_args((pool_id.clone(), init_liq, init_liq, 0u128)).unwrap())
        .expect("add_liquidity failed");
    let _: Nat = decode_ok(add_result);

    // Query the pool to find out which token is token_a/token_b after sorting
    let pool_info = {
        let r = env.pic.query_call(env.amm_id, Principal::anonymous(), "get_pool",
            encode_one(pool_id.clone()).unwrap()).expect("get_pool failed");
        match r {
            WasmResult::Reply(bytes) => { let info: Option<PoolInfo> = decode_one(&bytes).unwrap(); info.unwrap() }
            WasmResult::Reject(msg) => panic!("get_pool rejected: {}", msg),
        }
    };

    // Revoke approval for whichever ledger is pool's "token_b" (the second one transferred).
    // This ensures token_a transfer succeeds but token_b transfer fails.
    let pool_token_b_ledger = pool_info.token_b;
    let pool_token_a_ledger = pool_info.token_a;

    let zero_approve = ApproveArgs {
        from_subaccount: None,
        spender: Account { owner: env.amm_id, subaccount: None },
        amount: Nat::from(0u64),
        expected_allowance: None,
        expires_at: None,
        fee: None,
        memo: None,
        created_at_time: None,
    };
    env.pic.update_call(pool_token_b_ledger, env.user, "icrc2_approve",
        encode_one(zero_approve).unwrap())
        .expect("revoke approval failed");

    // Record balances before the failing add (token_a in pool's ordering)
    let balance_a_before = {
        let r = env.pic.query_call(pool_token_a_ledger, Principal::anonymous(), "icrc1_balance_of",
            encode_one(Account { owner: env.user, subaccount: None }).unwrap())
            .expect("balance_of failed");
        match r {
            WasmResult::Reply(bytes) => { let n: Nat = decode_one(&bytes).unwrap(); let v: u128 = n.0.try_into().unwrap(); v }
            WasmResult::Reject(msg) => panic!("balance_of rejected: {}", msg),
        }
    };

    let pool_before = pool_info;

    // Try to add more liquidity — token_a transfer will succeed, token_b will fail (no allowance)
    let add_amount: u128 = 10_000_00000000;
    let result = env.pic
        .update_call(env.amm_id, env.user, "add_liquidity",
            encode_args((pool_id.clone(), add_amount, add_amount, 0u128)).unwrap())
        .expect("add_liquidity call failed");

    // Should fail with TransferFailed for token_b
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<Nat, AmmError> = decode_one(&bytes).expect("decode failed");
            match res {
                Err(AmmError::TransferFailed { token, .. }) => {
                    assert_eq!(token, "token_b",
                        "Should fail on token_b transfer (the second one)");
                }
                Err(other) => panic!("Expected TransferFailed for token_b, got: {:?}", other),
                Ok(_) => panic!("Expected error but got Ok"),
            }
        }
        WasmResult::Reject(msg) => panic!("add_liquidity rejected: {}", msg),
    }

    // Verify token_a was refunded (using pool's token_a ledger)
    let balance_a_after = {
        let r = env.pic.query_call(pool_token_a_ledger, Principal::anonymous(), "icrc1_balance_of",
            encode_one(Account { owner: env.user, subaccount: None }).unwrap())
            .expect("balance_of failed");
        match r {
            WasmResult::Reply(bytes) => { let n: Nat = decode_one(&bytes).unwrap(); let v: u128 = n.0.try_into().unwrap(); v }
            WasmResult::Reject(msg) => panic!("balance_of rejected: {}", msg),
        }
    };
    assert_eq!(balance_a_before, balance_a_after,
        "User's token_a balance should be restored after refund");

    // Verify pool state unchanged
    let pool_after = {
        let r = env.pic.query_call(env.amm_id, Principal::anonymous(), "get_pool",
            encode_one(pool_id.clone()).unwrap()).expect("get_pool failed");
        match r {
            WasmResult::Reply(bytes) => { let info: Option<PoolInfo> = decode_one(&bytes).unwrap(); info.unwrap() }
            WasmResult::Reject(msg) => panic!("get_pool rejected: {}", msg),
        }
    };
    assert_eq!(pool_before.reserve_a, pool_after.reserve_a, "reserve_a unchanged");
    assert_eq!(pool_before.reserve_b, pool_after.reserve_b, "reserve_b unchanged");
}
