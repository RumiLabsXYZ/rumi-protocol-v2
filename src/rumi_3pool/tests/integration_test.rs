mod common;

use candid::{decode_one, encode_args, encode_one, Principal};
use common::{
    icrc1_ledger_wasm, three_pool_wasm, ArchiveOptions, FeatureFlags, LedgerArg, LedgerInitArgs,
};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::approve::ApproveArgs;
use pocket_ic::{PocketIcBuilder, WasmResult};

use rumi_3pool::types::*;

// ─── Integration Test ───

#[test]
fn test_3pool_deploy_add_liquidity_and_swap() {
    // ── 1. Create PocketIC environment ──
    let pic = PocketIcBuilder::new()
        .with_application_subnet()
        .build();

    let minting_account = Principal::self_authenticating(&[100, 100, 100]);
    let test_user = Principal::self_authenticating(&[1, 2, 3, 4]);
    let admin = Principal::self_authenticating(&[5, 6, 7, 8]);

    // ── 2. Deploy 3 ICRC-1 ledger canisters ──
    // Amounts:
    //   icUSD:  8 decimals, 10M = 10_000_000 * 10^8 = 1_000_000_000_000_000
    //   ckUSDT: 6 decimals, 10M = 10_000_000 * 10^6 = 10_000_000_000_000
    //   ckUSDC: 6 decimals, 10M = 10_000_000 * 10^6 = 10_000_000_000_000

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
            minting_account: Account {
                owner: minting_account,
                subaccount: None,
            },
            fee_collector_account: None,
            transfer_fee: candid::Nat::from(0u64), // Zero fees for cleaner testing
            decimals: Some(spec.decimals),
            max_memo_length: Some(32),
            token_name: spec.name.to_string(),
            token_symbol: spec.symbol.to_string(),
            metadata: vec![],
            initial_balances: vec![(
                Account {
                    owner: test_user,
                    subaccount: None,
                },
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

        let ledger_arg = LedgerArg::Init(init_args);
        let encoded = encode_args((ledger_arg,)).expect("Failed to encode ledger init args");

        pic.install_canister(ledger_id, icrc1_ledger_wasm(), encoded, None);
        ledger_ids.push(ledger_id);
    }

    let icusd_ledger = ledger_ids[0];
    let ckusdt_ledger = ledger_ids[1];
    let ckusdc_ledger = ledger_ids[2];

    // ── 3. Deploy rumi_3pool canister ──
    // Precision multipliers:
    //   icUSD  (8 dec): precision_mul = 10^(18-8) = 10^10
    //   ckUSDT (6 dec): precision_mul = 10^(18-6) = 10^12
    //   ckUSDC (6 dec): precision_mul = 10^(18-6) = 10^12
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

    let pool_encoded = encode_one(pool_init_args).expect("Failed to encode 3pool init args");
    pic.install_canister(pool_id, three_pool_wasm(), pool_encoded, None);

    // ── 4. Verify health ──
    let health_result = pic
        .query_call(pool_id, Principal::anonymous(), "health", encode_args(()).unwrap())
        .expect("health query failed");

    match health_result {
        WasmResult::Reply(bytes) => {
            let status: String = decode_one(&bytes).expect("Failed to decode health");
            assert_eq!(status, "ok", "health() should return 'ok'");
        }
        WasmResult::Reject(msg) => panic!("health() rejected: {}", msg),
    }

    // ── 5. ICRC-2 approve the 3pool canister on all 3 ledgers ──
    for ledger_id in &ledger_ids {
        let approve_args = ApproveArgs {
            from_subaccount: None,
            spender: Account {
                owner: pool_id,
                subaccount: None,
            },
            amount: candid::Nat::from(u128::MAX),
            expected_allowance: None,
            expires_at: None,
            fee: None,
            memo: None,
            created_at_time: None,
        };

        let result = pic
            .update_call(
                *ledger_id,
                test_user,
                "icrc2_approve",
                encode_one(approve_args).unwrap(),
            )
            .expect("icrc2_approve call failed");

        match result {
            WasmResult::Reply(_) => {} // Success
            WasmResult::Reject(msg) => panic!("icrc2_approve rejected: {}", msg),
        }
    }

    // ── 6. Add liquidity: 1M of each token ──
    // icUSD:  1M * 10^8  = 100_000_000_000_000
    // ckUSDT: 1M * 10^6  = 1_000_000_000_000
    // ckUSDC: 1M * 10^6  = 1_000_000_000_000
    let add_liq_amounts: Vec<u128> = vec![
        100_000_000_000_000,  // 1M icUSD  (8 dec)
        1_000_000_000_000,    // 1M ckUSDT (6 dec)
        1_000_000_000_000,    // 1M ckUSDC (6 dec)
    ];
    let min_lp: u128 = 0;

    let add_liq_result = pic
        .update_call(
            pool_id,
            test_user,
            "add_liquidity",
            encode_args((add_liq_amounts.clone(), min_lp)).unwrap(),
        )
        .expect("add_liquidity call failed");

    let lp_minted: u128 = match add_liq_result {
        WasmResult::Reply(bytes) => {
            let result: Result<candid::Nat, ThreePoolError> =
                decode_one(&bytes).expect("Failed to decode add_liquidity result");
            let nat = result.expect("add_liquidity returned an error");
            nat.0.try_into().expect("LP minted does not fit in u128")
        }
        WasmResult::Reject(msg) => panic!("add_liquidity rejected: {}", msg),
    };

    assert!(lp_minted > 0, "LP tokens minted should be > 0, got {}", lp_minted);

    // ── 7. Verify pool status after adding liquidity ──
    let status_after_add = query_pool_status(&pic, pool_id);

    assert_eq!(
        status_after_add.balances[0], add_liq_amounts[0],
        "icUSD balance should be 1M"
    );
    assert_eq!(
        status_after_add.balances[1], add_liq_amounts[1],
        "ckUSDT balance should be 1M"
    );
    assert_eq!(
        status_after_add.balances[2], add_liq_amounts[2],
        "ckUSDC balance should be 1M"
    );
    assert!(
        status_after_add.lp_total_supply > 0,
        "LP total supply should be > 0"
    );

    // ── 8. calc_swap: get quote for swapping 1000 icUSD -> ckUSDT ──
    // 1000 icUSD = 1000 * 10^8 = 100_000_000_000
    let swap_dx: u128 = 100_000_000_000; // 1000 icUSD
    let i: u8 = 0; // icUSD
    let j: u8 = 1; // ckUSDT

    let calc_result = pic
        .query_call(
            pool_id,
            test_user,
            "calc_swap",
            encode_args((i, j, swap_dx)).unwrap(),
        )
        .expect("calc_swap query failed");

    let quoted_dy: u128 = match calc_result {
        WasmResult::Reply(bytes) => {
            let result: Result<candid::Nat, ThreePoolError> =
                decode_one(&bytes).expect("Failed to decode calc_swap result");
            let nat = result.expect("calc_swap returned an error");
            nat.0.try_into().expect("calc_swap output does not fit in u128")
        }
        WasmResult::Reject(msg) => panic!("calc_swap rejected: {}", msg),
    };

    assert!(quoted_dy > 0, "Swap quote should be > 0, got {}", quoted_dy);

    // ── 9. Execute swap: 1000 icUSD -> ckUSDT ──
    // Use the quoted amount as min_dy (exact match, since no state changes between quote and swap)
    let swap_result = pic
        .update_call(
            pool_id,
            test_user,
            "swap",
            encode_args((i, j, swap_dx, quoted_dy)).unwrap(),
        )
        .expect("swap call failed");

    let actual_dy: u128 = match swap_result {
        WasmResult::Reply(bytes) => {
            let result: Result<candid::Nat, ThreePoolError> =
                decode_one(&bytes).expect("Failed to decode swap result");
            let nat = result.expect("swap returned an error");
            nat.0.try_into().expect("swap output does not fit in u128")
        }
        WasmResult::Reject(msg) => panic!("swap rejected: {}", msg),
    };

    assert_eq!(
        actual_dy, quoted_dy,
        "Actual swap output should match the quote"
    );

    // ── 10. Verify pool status after swap ──
    let status_after_swap = query_pool_status(&pic, pool_id);

    // icUSD balance should have increased (user deposited 1000 icUSD)
    assert!(
        status_after_swap.balances[0] > add_liq_amounts[0],
        "icUSD pool balance should have increased after swap"
    );
    // ckUSDT balance should have decreased (user withdrew ckUSDT)
    assert!(
        status_after_swap.balances[1] < add_liq_amounts[1],
        "ckUSDT pool balance should have decreased after swap"
    );

    // ── 11. Verify LP balance for user ──
    let lp_balance_result = pic
        .query_call(
            pool_id,
            test_user,
            "get_lp_balance",
            encode_one(test_user).unwrap(),
        )
        .expect("get_lp_balance query failed");

    let user_lp: u128 = match lp_balance_result {
        WasmResult::Reply(bytes) => {
            let nat: candid::Nat = decode_one(&bytes).expect("Failed to decode get_lp_balance");
            nat.0.try_into().expect("LP balance does not fit in u128")
        }
        WasmResult::Reject(msg) => panic!("get_lp_balance rejected: {}", msg),
    };

    assert!(user_lp > 0, "User LP balance should be > 0, got {}", user_lp);
    assert_eq!(
        user_lp, lp_minted,
        "User LP balance should equal the minted amount"
    );
}

// ─── Helper: query pool status ───

fn query_pool_status(
    pic: &pocket_ic::PocketIc,
    pool_id: Principal,
) -> PoolStatus {
    let result = pic
        .query_call(
            pool_id,
            Principal::anonymous(),
            "get_pool_status",
            encode_args(()).unwrap(),
        )
        .expect("get_pool_status query failed");

    match result {
        WasmResult::Reply(bytes) => {
            decode_one::<PoolStatus>(&bytes).expect("Failed to decode PoolStatus")
        }
        WasmResult::Reject(msg) => panic!("get_pool_status rejected: {}", msg),
    }
}

// ─── Dynamic fee + bot/explorer endpoint tests ───

struct TestEnv {
    pic: pocket_ic::PocketIc,
    pool_id: Principal,
    test_user: Principal,
    admin: Principal,
}

/// Build a fully initialized 3pool environment with a balanced 1M / 1M / 1M pool.
fn setup_balanced_pool() -> TestEnv {
    let pic = PocketIcBuilder::new().with_application_subnet().build();

    let minting_account = Principal::self_authenticating(&[100, 100, 100]);
    let test_user = Principal::self_authenticating(&[1, 2, 3, 4]);
    let admin = Principal::self_authenticating(&[5, 6, 7, 8]);

    struct LedgerSpec {
        name: &'static str,
        symbol: &'static str,
        decimals: u8,
        initial_balance: u128,
    }

    let ledger_specs = [
        LedgerSpec { name: "icUSD", symbol: "icUSD", decimals: 8, initial_balance: 1_000_000_000_000_000 },
        LedgerSpec { name: "ckUSDT", symbol: "ckUSDT", decimals: 6, initial_balance: 10_000_000_000_000 },
        LedgerSpec { name: "ckUSDC", symbol: "ckUSDC", decimals: 6, initial_balance: 10_000_000_000_000 },
    ];

    let mut ledger_ids = Vec::new();
    for spec in &ledger_specs {
        let ledger_id = pic.create_canister();
        pic.add_cycles(ledger_id, 2_000_000_000_000);
        let init_args = LedgerInitArgs {
            minting_account: Account { owner: minting_account, subaccount: None },
            fee_collector_account: None,
            transfer_fee: candid::Nat::from(0u64),
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
        let encoded = encode_args((LedgerArg::Init(init_args),)).unwrap();
        pic.install_canister(ledger_id, icrc1_ledger_wasm(), encoded, None);
        ledger_ids.push(ledger_id);
    }

    let pool_init_args = ThreePoolInitArgs {
        tokens: [
            TokenConfig { ledger_id: ledger_ids[0], symbol: "icUSD".to_string(), decimals: 8, precision_mul: 10_000_000_000 },
            TokenConfig { ledger_id: ledger_ids[1], symbol: "ckUSDT".to_string(), decimals: 6, precision_mul: 1_000_000_000_000 },
            TokenConfig { ledger_id: ledger_ids[2], symbol: "ckUSDC".to_string(), decimals: 6, precision_mul: 1_000_000_000_000 },
        ],
        initial_a: 100,
        swap_fee_bps: 4,
        admin_fee_bps: 5000,
        admin,
    };

    let pool_id = pic.create_canister();
    pic.add_cycles(pool_id, 2_000_000_000_000);
    pic.install_canister(pool_id, three_pool_wasm(), encode_one(pool_init_args).unwrap(), None);

    // Approve pool on all 3 ledgers.
    for ledger_id in &ledger_ids {
        let approve_args = ApproveArgs {
            from_subaccount: None,
            spender: Account { owner: pool_id, subaccount: None },
            amount: candid::Nat::from(u128::MAX),
            expected_allowance: None,
            expires_at: None,
            fee: None,
            memo: None,
            created_at_time: None,
        };
        pic.update_call(*ledger_id, test_user, "icrc2_approve", encode_one(approve_args).unwrap())
            .expect("icrc2_approve failed");
    }

    // Seed pool with 1M / 1M / 1M.
    let add_liq_amounts: Vec<u128> = vec![100_000_000_000_000, 1_000_000_000_000, 1_000_000_000_000];
    let res = pic
        .update_call(pool_id, test_user, "add_liquidity", encode_args((add_liq_amounts, 0u128)).unwrap())
        .expect("add_liquidity failed");
    if let WasmResult::Reply(bytes) = res {
        let r: Result<candid::Nat, ThreePoolError> = decode_one(&bytes).unwrap();
        r.expect("add_liquidity err");
    }

    TestEnv { pic, pool_id, test_user, admin }
}

fn quote_swap(env: &TestEnv, i: u8, j: u8, dx: u128) -> QuoteSwapResult {
    let res = env
        .pic
        .query_call(env.pool_id, Principal::anonymous(), "quote_swap", encode_args((i, j, dx)).unwrap())
        .expect("quote_swap failed");
    match res {
        WasmResult::Reply(bytes) => {
            let r: Result<QuoteSwapResult, ThreePoolError> = decode_one(&bytes).unwrap();
            r.expect("quote_swap returned err")
        }
        WasmResult::Reject(msg) => panic!("quote_swap rejected: {}", msg),
    }
}

fn do_swap(env: &TestEnv, i: u8, j: u8, dx: u128) -> u128 {
    let res = env
        .pic
        .update_call(env.pool_id, env.test_user, "swap", encode_args((i, j, dx, 0u128)).unwrap())
        .expect("swap failed");
    match res {
        WasmResult::Reply(bytes) => {
            let r: Result<candid::Nat, ThreePoolError> = decode_one(&bytes).unwrap();
            r.expect("swap returned err").0.try_into().unwrap()
        }
        WasmResult::Reject(msg) => panic!("swap rejected: {}", msg),
    }
}

fn get_swap_events_v2(env: &TestEnv) -> Vec<SwapEventV2> {
    let res = env
        .pic
        .query_call(env.pool_id, Principal::anonymous(), "get_swap_events_v2", encode_args((1000u64, 0u64)).unwrap())
        .expect("get_swap_events_v2 failed");
    match res {
        WasmResult::Reply(bytes) => decode_one::<Vec<SwapEventV2>>(&bytes).unwrap(),
        WasmResult::Reject(msg) => panic!("rejected: {}", msg),
    }
}

fn last_swap_event(env: &TestEnv) -> SwapEventV2 {
    // get_swap_events_v2 returns newest-first.
    get_swap_events_v2(env).into_iter().next().expect("no swap events")
}

/// Lower the imbalance saturation so that modest swaps clearly exercise the
/// dynamic fee curve (default saturation of 0.25 requires very large swaps to
/// move the SSD-based imbalance metric meaningfully on a 1M/1M/1M pool).
fn set_aggressive_fee_curve(env: &TestEnv) {
    let params = FeeCurveParams { min_fee_bps: 1, max_fee_bps: 99, imb_saturation: 1_000_000 };
    let res = env
        .pic
        .update_call(env.pool_id, env.admin, "set_fee_curve_params", encode_one(params).unwrap())
        .expect("set_fee_curve_params call failed");
    if let WasmResult::Reply(bytes) = res {
        let r: Result<(), ThreePoolError> = decode_one(&bytes).unwrap();
        r.expect("set_fee_curve_params returned err");
    }
}

#[test]
fn test_dynamic_fee_imbalancing_swap_pays_more() {
    let env = setup_balanced_pool();
    set_aggressive_fee_curve(&env);
    // Imbalance the pool with a sizeable icUSD -> ckUSDT swap.
    let dx: u128 = 100_000 * 100_000_000; // 100k icUSD
    do_swap(&env, 0, 1, dx);
    let ev = last_swap_event(&env);
    assert!(
        ev.fee_bps > 1,
        "imbalancing swap fee_bps should exceed MIN (got {}, imb_after={})",
        ev.fee_bps, ev.imbalance_after
    );
    assert!(!ev.is_rebalancing, "swap should be marked imbalancing");
}

#[test]
fn test_dynamic_fee_rebalancing_swap_pays_min() {
    let env = setup_balanced_pool();
    // First push the pool out of balance: icUSD -> ckUSDT.
    do_swap(&env, 0, 1, 50_000 * 100_000_000);
    // Now swap back the other way (ckUSDT -> icUSD): rebalancing.
    let q = quote_swap(&env, 1, 0, 1_000 * 1_000_000);
    assert!(q.is_rebalancing, "expected rebalancing quote");
    assert_eq!(q.fee_bps, 1, "rebalancing trades must pay MIN_FEE (1 bps)");
}

#[test]
fn test_dominant_flow_taxed() {
    let env = setup_balanced_pool();
    set_aggressive_fee_curve(&env);
    // Repeatedly do icUSD -> ckUSDT, the dominant direction. Fee should grow.
    let dx: u128 = 50_000 * 100_000_000;
    let mut fees = Vec::new();
    for _ in 0..4 {
        do_swap(&env, 0, 1, dx);
        fees.push(last_swap_event(&env).fee_bps);
    }
    assert!(
        fees.last().unwrap() > fees.first().unwrap(),
        "dominant flow fee should grow: {:?}",
        fees
    );
    assert!(
        *fees.last().unwrap() > 1,
        "later dominant-flow fee should exceed MIN, got {:?}",
        fees
    );
}

#[test]
fn test_set_fee_curve_params_admin_only() {
    let env = setup_balanced_pool();
    let new_params = FeeCurveParams {
        min_fee_bps: 2,
        max_fee_bps: 80,
        imb_saturation: 200_000_000,
    };

    // Non-admin attempt: should fail.
    let non_admin = Principal::self_authenticating(&[42, 42, 42]);
    let res = env
        .pic
        .update_call(env.pool_id, non_admin, "set_fee_curve_params", encode_one(new_params.clone()).unwrap())
        .expect("call returned");
    if let WasmResult::Reply(bytes) = res {
        let r: Result<(), ThreePoolError> = decode_one(&bytes).unwrap();
        assert!(matches!(r, Err(ThreePoolError::Unauthorized)), "non-admin must be rejected");
    }

    // Admin: should succeed.
    let res = env
        .pic
        .update_call(env.pool_id, env.admin, "set_fee_curve_params", encode_one(new_params.clone()).unwrap())
        .expect("call returned");
    if let WasmResult::Reply(bytes) = res {
        let r: Result<(), ThreePoolError> = decode_one(&bytes).unwrap();
        r.expect("admin set_fee_curve_params failed");
    }

    // Verify it stuck.
    let res = env
        .pic
        .query_call(env.pool_id, Principal::anonymous(), "get_fee_curve_params", encode_args(()).unwrap())
        .expect("query failed");
    if let WasmResult::Reply(bytes) = res {
        let got: FeeCurveParams = decode_one(&bytes).unwrap();
        assert_eq!(got.min_fee_bps, 2);
        assert_eq!(got.max_fee_bps, 80);
        assert_eq!(got.imb_saturation, 200_000_000);
    }
}

#[test]
fn test_quote_swap_matches_swap() {
    let env = setup_balanced_pool();
    let dx: u128 = 10_000 * 100_000_000;
    let q = quote_swap(&env, 0, 1, dx);
    let actual = do_swap(&env, 0, 1, dx);
    assert_eq!(actual, q.amount_out, "quote and swap output must match");
    let ev = last_swap_event(&env);
    assert_eq!(ev.fee_bps, q.fee_bps, "fee_bps must match between quote and swap");
}

#[test]
fn test_get_pool_health_basic() {
    let env = setup_balanced_pool();
    let res = env
        .pic
        .query_call(env.pool_id, Principal::anonymous(), "get_pool_health", encode_args(()).unwrap())
        .expect("get_pool_health failed");
    match res {
        WasmResult::Reply(bytes) => {
            let h: PoolHealth = decode_one(&bytes).unwrap();
            // Balanced pool: imbalance should be very small.
            assert!(h.current_imbalance < 10_000_000, "balanced pool imbalance should be ~0, got {}", h.current_imbalance);
            assert_eq!(h.fee_at_min, 1, "fee_at_min should be MIN_FEE_BPS=1");
            assert!(h.fee_at_max_imbalance_swap >= h.fee_at_min);
        }
        WasmResult::Reject(msg) => panic!("get_pool_health rejected: {}", msg),
    }
}

// ─── Task 23: Migration / upgrade test ───

#[test]
fn test_upgrade_preserves_swap_events() {
    let env = setup_balanced_pool();
    // Do a few swaps.
    for _ in 0..3 {
        do_swap(&env, 0, 1, 1_000 * 100_000_000);
    }
    // Snapshot v2 events before upgrade.
    let before = get_swap_events_v2(&env);
    assert_eq!(before.len(), 3);

    // Upgrade canister to itself (re-runs post_upgrade migration).
    env.pic
        .upgrade_canister(env.pool_id, three_pool_wasm(), encode_args(()).unwrap(), None)
        .expect("upgrade failed");

    let after = get_swap_events_v2(&env);
    assert_eq!(after.len(), 3, "events must survive upgrade");
    assert_eq!(after[0].id, before[0].id);
    assert_eq!(after[2].id, before[2].id);
}

// ─── C1 regression: virtual_price grows from swap and remove_one_coin fees ───
//
// Before the fix, swap and remove_one_coin both subtracted `output + fee` from
// the pool's internal balance, double-deducting the LP-fee portion. That kept
// virtual_price flat (or even shrank it on heavy fees) since the LP fee never
// actually stayed in the pool. After the fix only `output + admin_fee_share`
// is deducted, so the LP-fee portion accrues to LPs via VP growth.

fn get_lp_balance(env: &TestEnv, who: Principal) -> u128 {
    let res = env
        .pic
        .query_call(env.pool_id, Principal::anonymous(), "get_lp_balance", encode_one(who).unwrap())
        .expect("get_lp_balance failed");
    match res {
        WasmResult::Reply(bytes) => {
            let nat: candid::Nat = decode_one(&bytes).unwrap();
            nat.0.try_into().unwrap()
        }
        WasmResult::Reject(msg) => panic!("get_lp_balance rejected: {}", msg),
    }
}

fn do_remove_one_coin(env: &TestEnv, lp_burn: u128, idx: u8) -> u128 {
    let res = env
        .pic
        .update_call(env.pool_id, env.test_user, "remove_one_coin", encode_args((lp_burn, idx, 0u128)).unwrap())
        .expect("remove_one_coin failed");
    match res {
        WasmResult::Reply(bytes) => {
            let r: Result<candid::Nat, ThreePoolError> = decode_one(&bytes).unwrap();
            r.expect("remove_one_coin err").0.try_into().unwrap()
        }
        WasmResult::Reject(msg) => panic!("remove_one_coin rejected: {}", msg),
    }
}

fn get_liquidity_events_v2(env: &TestEnv) -> Vec<LiquidityEventV2> {
    let res = env
        .pic
        .query_call(
            env.pool_id,
            Principal::anonymous(),
            "get_liquidity_events_by_principal",
            encode_args((env.test_user, 0u64, 1000u64)).unwrap(),
        )
        .expect("get_liquidity_events_by_principal failed");
    match res {
        WasmResult::Reply(bytes) => decode_one::<Vec<LiquidityEventV2>>(&bytes).unwrap(),
        WasmResult::Reject(msg) => panic!("rejected: {}", msg),
    }
}

#[test]
fn test_virtual_price_grows_from_swap_and_remove_one_coin_fees() {
    let env = setup_balanced_pool();
    set_aggressive_fee_curve(&env);

    // ── Phase 1: Swap path. Do a sizeable swap and verify VP grew. ──
    //
    // VP before the swap is the VP after the initial add_liquidity. Read it
    // from the seeded liquidity event.
    let liq_events_initial = get_liquidity_events_v2(&env);
    assert!(!liq_events_initial.is_empty(), "expected initial add_liquidity event");
    let vp_after_seed = liq_events_initial[0].virtual_price_after;
    assert!(vp_after_seed > 0, "seeded VP must be > 0");

    // Imbalancing icUSD -> ckUSDT swap exercises the dynamic fee curve.
    do_swap(&env, 0, 1, 100_000 * 100_000_000);
    let swap_ev = last_swap_event(&env);
    assert!(swap_ev.fee > 0, "swap should have charged a fee");
    assert!(swap_ev.fee_bps > 1, "should pay more than min fee");
    assert!(
        swap_ev.virtual_price_after > vp_after_seed,
        "VP must grow after a fee-bearing swap: before={}, after={}, fee={}, fee_bps={}",
        vp_after_seed, swap_ev.virtual_price_after, swap_ev.fee, swap_ev.fee_bps
    );

    let vp_after_swap = swap_ev.virtual_price_after;

    // ── Phase 2: remove_one_coin path. Burn a small fraction and verify VP grew. ──
    let user_lp = get_lp_balance(&env, env.test_user);
    let lp_burn = user_lp / 100; // 1% — leaves plenty in the pool
    assert!(lp_burn > 0, "user must hold LP");
    do_remove_one_coin(&env, lp_burn, 1); // pull ckUSDT, imbalancing
    let liq_events = get_liquidity_events_v2(&env);
    let last_liq = liq_events.last().expect("expected remove_one_coin event");
    assert!(matches!(last_liq.action, LiquidityAction::RemoveOneCoin));
    assert!(last_liq.fee.unwrap_or(0) > 0, "remove_one_coin should charge a fee");
    assert!(
        last_liq.virtual_price_after > vp_after_swap,
        "VP must grow after a fee-bearing remove_one_coin: before={}, after={}, fee={:?}",
        vp_after_swap, last_liq.virtual_price_after, last_liq.fee
    );
}
