//! Regressions for 3pool fee drift, dust liabilities, and receipt recovery.

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::approve::ApproveArgs;
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use rumi_3pool::receipts::{
    SwapReceiptErrorV1, SwapReceiptStatusV1, SwapRequestV1, SwapTransferStatusV1,
};
use rumi_3pool::types::{ThreePoolError, ThreePoolInitArgs, ThreePoolPendingClaim, TokenConfig};

const INITIAL_BALANCE: u128 = 1_000_000_000_000_000;

fn pool_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_3pool.wasm").to_vec()
}

fn flaky_ledger_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/flaky_ledger.wasm").to_vec()
}

#[derive(CandidType, Deserialize)]
struct FlakyAccount {
    owner: Principal,
    subaccount: Option<[u8; 32]>,
}

struct Env {
    pic: PocketIc,
    admin: Principal,
    user: Principal,
    pool: Principal,
    ledgers: [Principal; 3],
}

fn reply<T: for<'de> Deserialize<'de> + CandidType>(result: WasmResult, label: &str) -> T {
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).unwrap_or_else(|e| panic!("{label}: {e}")),
        WasmResult::Reject(message) => panic!("{label} rejected: {message}"),
    }
}

fn update<T: for<'de> Deserialize<'de> + CandidType>(
    pic: &PocketIc,
    canister: Principal,
    caller: Principal,
    method: &str,
    args: Vec<u8>,
) -> T {
    let result = pic
        .update_call(canister, caller, method, args)
        .expect("update ingress failed");
    reply(result, method)
}

fn query<T: for<'de> Deserialize<'de> + CandidType>(
    pic: &PocketIc,
    canister: Principal,
    method: &str,
    args: Vec<u8>,
) -> T {
    let result = pic
        .query_call(canister, Principal::anonymous(), method, args)
        .expect("query ingress failed");
    reply(result, method)
}

fn setup(initial_fee: u128) -> Env {
    let pic = PocketIcBuilder::new().with_application_subnet().build();
    let admin = Principal::self_authenticating(&[5, 6, 7, 8]);
    let user = Principal::self_authenticating(&[1, 2, 3, 4]);

    let ledgers: [Principal; 3] = std::array::from_fn(|_| {
        let id = pic.create_canister();
        pic.add_cycles(id, 2_000_000_000_000);
        pic.install_canister(id, flaky_ledger_wasm(), encode_one(()).unwrap(), None);
        update::<()>(
            &pic,
            id,
            Principal::anonymous(),
            "set_fee",
            encode_one(Nat::from(initial_fee)).unwrap(),
        );
        let account = FlakyAccount {
            owner: user,
            subaccount: None,
        };
        update::<()>(
            &pic,
            id,
            Principal::anonymous(),
            "mint",
            encode_args((account, Nat::from(INITIAL_BALANCE))).unwrap(),
        );
        id
    });

    let pool = pic.create_canister();
    pic.add_cycles(pool, 2_000_000_000_000);
    let init = ThreePoolInitArgs {
        tokens: [
            TokenConfig {
                ledger_id: ledgers[0],
                symbol: "T0".into(),
                decimals: 8,
                precision_mul: 1,
            },
            TokenConfig {
                ledger_id: ledgers[1],
                symbol: "T1".into(),
                decimals: 8,
                precision_mul: 1,
            },
            TokenConfig {
                ledger_id: ledgers[2],
                symbol: "T2".into(),
                decimals: 8,
                precision_mul: 1,
            },
        ],
        initial_a: 100,
        swap_fee_bps: 4,
        admin_fee_bps: 5_000,
        admin,
    };
    pic.install_canister(pool, pool_wasm(), encode_one(init).unwrap(), None);

    for ledger in ledgers {
        let approval = ApproveArgs {
            from_subaccount: None,
            spender: Account {
                owner: pool,
                subaccount: None,
            },
            amount: Nat::from(u128::MAX),
            expected_allowance: None,
            expires_at: None,
            fee: None,
            memo: None,
            created_at_time: None,
        };
        update::<Result<Nat, rumi_3pool::types::ThreePoolError>>(
            &pic,
            ledger,
            user,
            "icrc2_approve",
            encode_one(approval).unwrap(),
        )
        .expect("pool approval failed");
    }

    let seeded: Result<Nat, ThreePoolError> = update(
        &pic,
        pool,
        user,
        "add_liquidity",
        encode_args((vec![100_000_000_000_000u128; 3], 0u128)).unwrap(),
    );
    seeded.expect("initial liquidity failed");

    Env {
        pic,
        admin,
        user,
        pool,
        ledgers,
    }
}

fn set_fee(env: &Env, ledger: Principal, fee: u128) {
    update::<()>(
        &env.pic,
        ledger,
        Principal::anonymous(),
        "set_fee",
        encode_one(Nat::from(fee)).unwrap(),
    );
}

fn bad_fee_once(env: &Env, ledger: Principal) {
    update::<()>(
        &env.pic,
        ledger,
        Principal::anonymous(),
        "set_bad_fee_failures",
        encode_one(1u32).unwrap(),
    );
}

fn phantom_failure_once(env: &Env, ledger: Principal) {
    update::<()>(
        &env.pic,
        ledger,
        Principal::anonymous(),
        "set_phantom_transfer_failures",
        encode_one(1u32).unwrap(),
    );
}

fn balance(env: &Env, ledger: Principal, owner: Principal) -> u128 {
    let account = FlakyAccount {
        owner,
        subaccount: None,
    };
    let n: Nat = query(
        &env.pic,
        ledger,
        "icrc1_balance_of",
        encode_one(account).unwrap(),
    );
    n.0.try_into().unwrap()
}

fn admin_fees(env: &Env) -> Vec<u128> {
    query(
        &env.pic,
        env.pool,
        "get_admin_fees",
        encode_one(()).unwrap(),
    )
}

fn swap(env: &Env, amount: u128) -> u128 {
    let r: Result<u128, ThreePoolError> = update(
        &env.pic,
        env.pool,
        env.user,
        "swap",
        encode_args((0u8, 1u8, amount, 0u128)).unwrap(),
    );
    r.expect("swap failed")
}

#[test]
fn stale_cached_fee_is_rejected_refreshed_and_retry_succeeds() {
    let env = setup(10_000);
    swap(&env, 100_000_000_000); // warm the output-token fee cache
    let pool_before = balance(&env, env.ledgers[1], env.pool);
    let user_before = balance(&env, env.ledgers[1], env.user);
    set_fee(&env, env.ledgers[1], 20_000);

    let first: Result<u128, ThreePoolError> = update(
        &env.pic,
        env.pool,
        env.user,
        "swap",
        encode_args((0u8, 1u8, 100_000_000_000u128, 0u128)).unwrap(),
    );
    assert!(matches!(first, Err(ThreePoolError::TransferFailed { .. })));
    assert_eq!(balance(&env, env.ledgers[1], env.pool), pool_before);

    let second: Result<u128, ThreePoolError> = update(
        &env.pic,
        env.pool,
        env.user,
        "swap",
        encode_args((0u8, 1u8, 100_000_000_000u128, 0u128)).unwrap(),
    );
    second.expect("retry must use the refreshed fee");
    assert!(balance(&env, env.ledgers[1], env.user) > user_before);
}

#[test]
fn admin_fee_at_or_below_refreshed_ledger_fee_is_retained() {
    let env = setup(0);
    // This amount creates a positive admin fee well below the later 1,000-unit
    // ledger fee. The small payout path must refresh stale-high cache data.
    swap(&env, 2_000_000);
    let accrued = admin_fees(&env)[1];
    assert!(
        accrued > 0 && accrued <= 1_000,
        "expected positive dust admin fee, got {accrued}"
    );

    let pool_before = balance(&env, env.ledgers[1], env.pool);
    let admin_before = balance(&env, env.ledgers[1], env.admin);
    set_fee(&env, env.ledgers[1], 1_000);

    let first: Result<Vec<u128>, ThreePoolError> = update(
        &env.pic,
        env.pool,
        env.admin,
        "withdraw_admin_fees",
        encode_one(()).unwrap(),
    );
    assert_eq!(first.expect("first admin call failed")[1], 0);
    assert_eq!(
        admin_fees(&env)[1],
        accrued,
        "failed transfer must leave dust obligation recorded"
    );

    let second: Result<Vec<u128>, ThreePoolError> = update(
        &env.pic,
        env.pool,
        env.admin,
        "withdraw_admin_fees",
        encode_one(()).unwrap(),
    );
    assert_eq!(second.expect("second admin call failed")[1], 0);
    assert_eq!(
        admin_fees(&env)[1],
        accrued,
        "fee-at-or-below-fee branch must retain the claim"
    );
    assert_eq!(balance(&env, env.ledgers[1], env.pool), pool_before);
    assert_eq!(balance(&env, env.ledgers[1], env.admin), admin_before);
}

#[test]
fn receipt_swap_persists_claim_when_input_is_too_small_for_refund_fee() {
    let env = setup(0);
    set_fee(&env, env.ledgers[0], 2_000);

    let enabled: Result<(), SwapReceiptErrorV1> = update(
        &env.pic,
        env.pool,
        env.admin,
        "set_swap_receipt_client_v1",
        encode_args((env.user, true)).unwrap(),
    );
    enabled.expect("admin should enable the receipt client");

    // The output leg will be deterministically rejected after input succeeds.
    // At dx <= the input-ledger fee, an on-ledger refund cannot be submitted;
    // the correct recovery is a durable pending claim and a cleared fence.
    bad_fee_once(&env, env.ledgers[1]);
    let request = SwapRequestV1 {
        intent_id: vec![0xA5; 32],
        i: 0,
        j: 1,
        dx: 1_000,
        min_dy: 1,
    };
    let receipt: Result<rumi_3pool::receipts::SwapReceiptV1, SwapReceiptErrorV1> = update(
        &env.pic,
        env.pool,
        env.user,
        "swap_with_receipt_v1",
        encode_one(request.clone()).unwrap(),
    );
    let receipt = receipt.expect("receipt submission should return its stored result");
    assert_eq!(receipt.status, SwapReceiptStatusV1::Failed);
    assert_eq!(
        receipt.input.as_ref().unwrap().status,
        SwapTransferStatusV1::Confirmed
    );
    assert_eq!(
        receipt.output.as_ref().unwrap().status,
        SwapTransferStatusV1::Rejected
    );
    assert!(
        receipt.refund.is_none(),
        "no dust refund transfer should be fabricated"
    );
    assert!(receipt
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("pending claim"));

    let claims: Vec<ThreePoolPendingClaim> = query(
        &env.pic,
        env.pool,
        "get_pending_claims",
        encode_args((0u64, 100u64)).unwrap(),
    );
    let claim = claims
        .iter()
        .find(|claim| claim.claimant == env.user && claim.token_index == 0)
        .expect("confirmed input must have a durable user claim");
    assert_eq!(claim.ledger, env.ledgers[0]);
    assert_eq!(claim.amount, request.dx);

    // The fee is still above the claim amount, so the existing claim route
    // must retain the obligation and fail without trapping the pool fence.
    let claim_result: Result<(), ThreePoolError> = update(
        &env.pic,
        env.pool,
        env.user,
        "claim_pending",
        encode_one(claim.id).unwrap(),
    );
    assert!(matches!(
        claim_result,
        Err(ThreePoolError::TransferFailed { .. })
    ));
    let after: Vec<ThreePoolPendingClaim> = query(
        &env.pic,
        env.pool,
        "get_pending_claims",
        encode_args((0u64, 100u64)).unwrap(),
    );
    assert!(after
        .iter()
        .any(|entry| entry.id == claim.id && entry.amount == request.dx));

    // A fee reduction must be discoverable even though the retained claim is
    // smaller than its previously cached fee and would otherwise never reach
    // the ledger's BadFee response path.
    set_fee(&env, env.ledgers[0], 0);
    let recovered: Result<(), ThreePoolError> = update(
        &env.pic,
        env.pool,
        env.user,
        "claim_pending",
        encode_one(claim.id).unwrap(),
    );
    recovered.expect("claim should become payable after fee decreases");
    let after_recovery: Vec<ThreePoolPendingClaim> = query(
        &env.pic,
        env.pool,
        "get_pending_claims",
        encode_args((0u64, 100u64)).unwrap(),
    );
    assert!(!after_recovery.iter().any(|entry| entry.id == claim.id));
}

#[test]
fn full_claim_store_rejects_new_liquidity_without_moving_tokens_or_dropping_claim() {
    let env = setup(0);
    let cap: Result<(), ThreePoolError> = update(
        &env.pic,
        env.pool,
        env.admin,
        "test_set_pending_claim_limit",
        encode_one(1u64).unwrap(),
    );
    cap.expect("admin should set the test claim limit");

    let first_id: u64 = update(
        &env.pic,
        env.pool,
        env.admin,
        "test_insert_pending_claim",
        encode_args((0u8, 777u128)).unwrap(),
    );
    let before = [
        balance(&env, env.ledgers[0], env.user),
        balance(&env, env.ledgers[1], env.user),
        balance(&env, env.ledgers[2], env.user),
    ];

    let rejected: Result<Nat, ThreePoolError> = update(
        &env.pic,
        env.pool,
        env.user,
        "add_liquidity",
        encode_args((vec![1_000_000u128; 3], 0u128)).unwrap(),
    );
    assert!(matches!(
        rejected,
        Err(ThreePoolError::PendingClaimCapacityReached)
    ));
    assert_eq!(
        [
            balance(&env, env.ledgers[0], env.user),
            balance(&env, env.ledgers[1], env.user),
            balance(&env, env.ledgers[2], env.user),
        ],
        before,
        "capacity rejection must happen before any input transfer"
    );

    let claims: Vec<ThreePoolPendingClaim> = query(
        &env.pic,
        env.pool,
        "get_pending_claims",
        encode_args((0u64, 100u64)).unwrap(),
    );
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].id, first_id);
    assert_eq!(claims[0].amount, 777);
}

#[test]
fn proportional_withdrawal_reserves_all_three_claim_slots_before_lp_burn() {
    let env = setup(0);
    let cap: Result<(), ThreePoolError> = update(
        &env.pic,
        env.pool,
        env.admin,
        "test_set_pending_claim_limit",
        encode_one(3u64).unwrap(),
    );
    cap.expect("admin should set the test claim limit");
    let first_id: u64 = update(
        &env.pic,
        env.pool,
        env.admin,
        "test_insert_pending_claim",
        encode_args((0u8, 777u128)).unwrap(),
    );
    let lp_before: u128 = query(
        &env.pic,
        env.pool,
        "get_lp_balance",
        encode_one(env.user).unwrap(),
    );

    let rejected: Result<Vec<u128>, ThreePoolError> = update(
        &env.pic,
        env.pool,
        env.user,
        "remove_liquidity",
        encode_args((lp_before, vec![0u128; 3])).unwrap(),
    );
    assert!(matches!(
        rejected,
        Err(ThreePoolError::PendingClaimCapacityReached)
    ));
    let lp_after: u128 = query(
        &env.pic,
        env.pool,
        "get_lp_balance",
        encode_one(env.user).unwrap(),
    );
    assert_eq!(lp_after, lp_before, "LP shares must remain unburned");

    let claims: Vec<ThreePoolPendingClaim> = query(
        &env.pic,
        env.pool,
        "get_pending_claims",
        encode_args((0u64, 100u64)).unwrap(),
    );
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].id, first_id);
    assert_eq!(claims[0].amount, 777);
}

#[test]
fn full_claim_store_rejects_new_liquidity_without_moving_tokens_or_dropping_claim() {
    let env = setup(0);
    let cap: Result<(), ThreePoolError> = update(
        &env.pic,
        env.pool,
        env.admin,
        "test_set_pending_claim_limit",
        encode_one(1u64).unwrap(),
    );
    cap.expect("admin should set the test claim limit");

    let first_id: u64 = update(
        &env.pic,
        env.pool,
        env.admin,
        "test_insert_pending_claim",
        encode_args((0u8, 777u128)).unwrap(),
    );
    let before = [
        balance(&env, env.ledgers[0], env.user),
        balance(&env, env.ledgers[1], env.user),
        balance(&env, env.ledgers[2], env.user),
    ];

    let rejected: Result<Nat, ThreePoolError> = update(
        &env.pic,
        env.pool,
        env.user,
        "add_liquidity",
        encode_args((vec![1_000_000u128; 3], 0u128)).unwrap(),
    );
    assert!(matches!(
        rejected,
        Err(ThreePoolError::PendingClaimCapacityReached)
    ));
    assert_eq!(
        [
            balance(&env, env.ledgers[0], env.user),
            balance(&env, env.ledgers[1], env.user),
            balance(&env, env.ledgers[2], env.user),
        ],
        before,
        "capacity rejection must happen before any input transfer"
    );

    let claims: Vec<ThreePoolPendingClaim> = query(
        &env.pic,
        env.pool,
        "get_pending_claims",
        encode_args((0u64, 100u64)).unwrap(),
    );
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].id, first_id);
    assert_eq!(claims[0].amount, 777);
}
