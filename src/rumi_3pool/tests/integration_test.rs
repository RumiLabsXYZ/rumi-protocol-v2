use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Principal};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::approve::ApproveArgs;
use pocket_ic::{PocketIcBuilder, WasmResult};

use rumi_3pool::types::*;

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

// ─── WASM loaders ───

fn icrc1_ledger_wasm() -> Vec<u8> {
    include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec()
}

fn three_pool_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_3pool.wasm").to_vec()
}

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
