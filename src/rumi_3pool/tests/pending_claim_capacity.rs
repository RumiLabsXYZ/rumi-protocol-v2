//! Pending-claim capacity must be reserved before value can move. The flaky
//! ledgers reproduce two independent add-liquidity refunds failing after two
//! successful pulls without requiring an outage or external ledger.
use candid::{decode_args, decode_one, encode_args, encode_one, Nat, Principal};
use icrc_ledger_types::icrc::generic_value::ICRC3Value;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc3::blocks::{GetBlocksRequest, GetBlocksResult};
use icrc_ledger_types::icrc2::approve::ApproveArgs;
use num_traits::ToPrimitive;
use pocket_ic::common::rest::{CanisterHttpReply, CanisterHttpResponse, MockCanisterHttpResponse};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use rumi_3pool::payouts::{
    GlobalPayoutInventoryPage, PayoutEntitlement, PayoutInputAction, PayoutKind, PayoutOutcome,
};
use rumi_3pool::types::{PoolStatus, ThreePoolError, ThreePoolInitArgs, ThreePoolPendingClaim, TokenConfig};

const USER_FUNDS: [u128; 3] = [
    1_000_000_000_000_000,
    10_000_000_000_000,
    10_000_000_000_000,
];
const SEED: [u128; 3] = [100_000_000_000_000, 1_000_000_000_000, 1_000_000_000_000];
const ADD: [u128; 3] = [100_000_000_000, 1_000_000_000, 1_000_000_000];

struct Harness {
    pic: PocketIc,
    admin: Principal,
    user: Principal,
    pool: Principal,
    ledgers: [Principal; 3],
}

fn reply(result: WasmResult) -> Vec<u8> {
    match result {
        WasmResult::Reply(bytes) => bytes,
        other => panic!("unexpected canister result: {other:?}"),
    }
}

fn wasm_artifact(name: &str) -> Vec<u8> {
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("target")
        });
    std::fs::read(
        target_dir
            .join("wasm32-unknown-unknown")
            .join("release")
            .join(name),
    )
    .unwrap_or_else(|error| panic!("read locally built {name} Wasm: {error}"))
}

fn setup() -> Harness {
    setup_with_pool_wasm(wasm_artifact("rumi_3pool.wasm"))
}

fn setup_with_pool_wasm(pool_wasm: Vec<u8>) -> Harness {
    setup_with_pool_wasm_and_seed(pool_wasm, true)
}

fn setup_with_pool_wasm_and_seed(pool_wasm: Vec<u8>, seed_liquidity: bool) -> Harness {
    let pic = PocketIcBuilder::new().with_application_subnet().build();
    let user = Principal::self_authenticating(&[11, 12, 13]);
    let admin = Principal::self_authenticating(&[21, 22, 23]);
    let minting = Principal::self_authenticating(&[31, 32, 33]);
    let mut ledgers = Vec::new();

    for balance in USER_FUNDS.iter() {
        let ledger = pic.create_canister();
        pic.add_cycles(ledger, 2_000_000_000_000);
        pic.install_canister(
            ledger,
            wasm_artifact("flaky_ledger.wasm"),
            encode_args(()).unwrap(),
            None,
        );
        pic.update_call(
            ledger,
            minting,
            "mint",
            encode_args((
                Account {
                    owner: user,
                    subaccount: None,
                },
                Nat::from(*balance),
            ))
            .unwrap(),
        )
        .unwrap();
        ledgers.push(ledger);
    }
    let ledgers: [Principal; 3] = ledgers.try_into().unwrap();

    // Install the pool only after creating the ledgers, then approve the pool
    // as spender on each ledger.
    let pool = pic.create_canister();
    pic.add_cycles(pool, 2_000_000_000_000);
    let pool_args = ThreePoolInitArgs {
        tokens: [
            TokenConfig {
                ledger_id: ledgers[0],
                symbol: "T0".into(),
                decimals: 8,
                precision_mul: 10_000_000_000,
            },
            TokenConfig {
                ledger_id: ledgers[1],
                symbol: "T1".into(),
                decimals: 6,
                precision_mul: 1_000_000_000_000,
            },
            TokenConfig {
                ledger_id: ledgers[2],
                symbol: "T2".into(),
                decimals: 6,
                precision_mul: 1_000_000_000_000,
            },
        ],
        initial_a: 100,
        swap_fee_bps: 4,
        admin_fee_bps: 5000,
        admin,
    };
    pic.install_canister(
        pool,
        pool_wasm,
        encode_one(pool_args).unwrap(),
        None,
    );
    for ledger in ledgers {
        pic.update_call(
            ledger,
            user,
            "icrc2_approve",
            encode_one(ApproveArgs {
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
            })
            .unwrap(),
        )
        .unwrap();
    }
    if seed_liquidity {
        let seeded: Result<Nat, ThreePoolError> = decode_one(&reply(
            pic.update_call(
                pool,
                user,
                "add_liquidity",
                encode_args((SEED.to_vec(), 0u128)).unwrap(),
            )
            .unwrap(),
        ))
        .unwrap();
        seeded.expect("bootstrap liquidity");
    }

    Harness {
        pic,
        admin,
        user,
        pool,
        ledgers,
    }
}

fn set_test_cap(h: &Harness, cap: u64) {
    let result: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(
                h.pool,
                h.admin,
                "test_set_pending_claim_limit",
                encode_one(cap).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    result.expect("set test claim cap");
}

fn assert_capacity_rejected(error: ThreePoolError) {
    match error {
        ThreePoolError::TransferFailed { token, reason } => {
            assert_eq!(token, "pending_claims");
            assert!(
                reason.contains("capacity"),
                "missing capacity diagnostic: {reason}"
            );
            assert!(
                reason.contains("inspect the payout journal before retrying"),
                "missing recovery-safe diagnostic: {reason}"
            );
        }
        other => panic!("expected compatible capacity error, got {other:?}"),
    }
}

fn balance(h: &Harness, ledger: Principal, owner: Principal) -> u128 {
    let result: Nat = decode_one(&reply(
        h.pic
            .query_call(
                ledger,
                Principal::anonymous(),
                "icrc1_balance_of",
                encode_one(Account {
                    owner,
                    subaccount: None,
                })
                .unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    result.0.try_into().unwrap()
}

fn lp_balance(h: &Harness) -> u128 {
    balance(h, h.pool, h.user)
}

fn liquidity_event_count_v2(h: &Harness) -> u64 {
    decode_one(&reply(
        h.pic.query_call(
            h.pool,
            Principal::anonymous(),
            "get_liquidity_event_count_v2",
            encode_args(()).unwrap(),
        ).unwrap(),
    )).unwrap()
}

fn claims(h: &Harness) -> Vec<ThreePoolPendingClaim> {
    decode_one(&reply(
        h.pic
            .query_call(
                h.pool,
                Principal::anonymous(),
                "get_pending_claims",
                encode_args((0u64, 100u64)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap()
}

fn payout(h: &Harness, owner: Principal, id: u64) -> PayoutEntitlement {
    decode_one::<Option<PayoutEntitlement>>(&reply(
        h.pic
            .query_call(
                h.pool,
                owner,
                "get_payout_entitlement",
                encode_one(id).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap()
    .expect("payout entitlement exists")
}

fn ledger_block_for_memo(h: &Harness, ledger: Principal, memo: &[u8]) -> u64 {
    let args = vec![GetBlocksRequest {
        start: Nat::from(0u64),
        length: Nat::from(1_000u64),
    }];
    let bytes = h
        .pic
        .query_call(
            ledger,
            Principal::anonymous(),
            "icrc3_get_blocks",
            encode_one(args).unwrap(),
        )
        .unwrap();
    let result: GetBlocksResult = decode_one(&reply(bytes)).unwrap();
    result
        .blocks
        .iter()
        .find_map(|block| {
            let ICRC3Value::Map(block_map) = &block.block else { return None };
            let ICRC3Value::Map(tx) = block_map.get("tx")? else { return None };
            match tx.get("memo")? {
                ICRC3Value::Blob(actual) if actual.as_ref() == memo => block.id.0.to_u64(),
                _ => None,
            }
        })
        .expect("exact payout memo must be present in ledger history")
}

fn payouts(h: &Harness, owner: Principal) -> Vec<PayoutEntitlement> {
    decode_one(&reply(
        h.pic
            .query_call(
                h.pool,
                owner,
                "get_payout_entitlements",
                encode_args((0u64, 100u64)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap()
}

fn global_payout_inventory(
    h: &Harness,
    caller: Principal,
    after_id: Option<u64>,
    limit: u64,
) -> Result<GlobalPayoutInventoryPage, ThreePoolError> {
    decode_one(&reply(
        h.pic
            .query_call(
                h.pool,
                caller,
                "get_global_payout_inventory",
                encode_args((after_id, limit)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap()
}

fn payout_storage_counts(h: &Harness) -> (u64, u64) {
    decode_args(&reply(
        h.pic.query_call(
            h.pool,
            Principal::anonymous(),
            "test_payout_storage_counts",
            encode_args(()).unwrap(),
        ).unwrap(),
    )).unwrap()
}

fn set_allowance(h: &Harness, ledger: Principal, amount: u128) {
    let _ = h.pic.update_call(
        ledger,
        h.user,
        "icrc2_approve",
        encode_one(ApproveArgs {
            from_subaccount: None,
            spender: Account { owner: h.pool, subaccount: None },
            amount: Nat::from(amount),
            expected_allowance: None,
            expires_at: None,
            fee: None,
            memo: None,
            created_at_time: None,
        }).unwrap(),
    ).unwrap();
}

fn call_ledger_flag(h: &Harness, ledger: Principal, method: &str, value: bool) {
    h.pic
        .update_call(ledger, h.user, method, encode_one(value).unwrap())
        .unwrap();
}

fn call_ledger_nat(h: &Harness, ledger: Principal, method: &str, value: u128) {
    h.pic
        .update_call(ledger, h.user, method, encode_one(Nat::from(value)).unwrap())
        .unwrap();
}

#[test]
fn add_liquidity_records_both_failed_refunds_when_two_prior_pulls_succeeded() {
    let h = setup();
    // Admission reserves the maximum three independent late refunds before
    // the first pull, even though this scenario only produces two claims.
    set_test_cap(&h, 3);
    for ledger in [h.ledgers[0], h.ledgers[1]] {
        h.pic
            .update_call(
                ledger,
                h.user,
                "set_fail_transfers",
                encode_one(true).unwrap(),
            )
            .unwrap();
    }
    h.pic
        .update_call(
            h.ledgers[2],
            h.user,
            "set_fail_transfer_from",
            encode_one(true).unwrap(),
        )
        .unwrap();

    let result: Result<Nat, ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(
                h.pool,
                h.user,
                "add_liquidity",
                encode_args((ADD.to_vec(), 0u128)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    assert!(matches!(result, Err(ThreePoolError::TransferFailed { .. })), "unexpected add result: {result:?}");

    let recorded = claims(&h);
    assert_eq!(recorded.len(), 2, "both failed refunds need durable claims");
    assert_eq!(
        recorded
            .iter()
            .map(|claim| claim.token_index)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(
        recorded
            .iter()
            .map(|claim| claim.amount)
            .collect::<Vec<_>>(),
        vec![ADD[0], ADD[1]]
    );
    assert!(recorded.iter().all(|claim| claim.claimant == h.user));
    assert_eq!(
        balance(&h, h.ledgers[0], h.user),
        USER_FUNDS[0] - SEED[0] - ADD[0]
    );
    assert_eq!(
        balance(&h, h.ledgers[1], h.user),
        USER_FUNDS[1] - SEED[1] - ADD[1]
    );

    // Mixed recovery: one previously rejected ledger is available again while
    // the other remains failed. The first claim exact-replays; the second stays.
    call_ledger_flag(&h, h.ledgers[0], "set_fail_transfers", false);
    let recovered: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(
                h.pool,
                h.user,
                "claim_pending",
                encode_one(recorded[0].id).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    recovered.expect("first refund should recover with its stored transfer tuple");
    let remaining = claims(&h);
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, recorded[1].id);
}

#[test]
fn add_liquidity_reserves_every_possible_late_refund_slot_before_pull() {
    let h = setup();
    set_test_cap(&h, 2);
    let before = h.ledgers.map(|ledger| balance(&h, ledger, h.user));
    let result: Result<Nat, ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(
                h.pool,
                h.user,
                "add_liquidity",
                encode_args((ADD.to_vec(), 0u128)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    assert_capacity_rejected(result.unwrap_err());
    assert_eq!(h.ledgers.map(|ledger| balance(&h, ledger, h.user)), before);
    assert!(claims(&h).is_empty());
}

#[test]
fn insufficient_capacity_rejects_before_any_add_liquidity_pull_and_keeps_old_claim() {
    let h = setup();
    set_test_cap(&h, 1);
    let first_id: u64 = decode_one(&reply(
        h.pic
            .update_call(
                h.pool,
                h.admin,
                "test_insert_pending_claim",
                encode_args((0u8, 777u128)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    let before = h.ledgers.map(|ledger| balance(&h, ledger, h.user));

    let result: Result<Nat, ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(
                h.pool,
                h.user,
                "add_liquidity",
                encode_args((ADD.to_vec(), 0u128)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    assert_capacity_rejected(result.unwrap_err());
    assert_eq!(h.ledgers.map(|ledger| balance(&h, ledger, h.user)), before);
    let existing = claims(&h);
    assert_eq!(existing.len(), 1);
    assert_eq!(existing[0].id, first_id);
    assert_eq!(existing[0].amount, 777);
}

#[test]
fn add_liquidity_rejects_fee_sized_leg_before_any_pull_or_claim() {
    let h = setup();
    call_ledger_nat(&h, h.ledgers[0], "set_fee", 1_000);
    let before = h.ledgers.map(|ledger| balance(&h, ledger, h.user));
    let payouts_before = payout_storage_counts(&h);
    let result: Result<Nat, ThreePoolError> = decode_one(&reply(
        h.pic.update_call(
            h.pool,
            h.user,
            "add_liquidity",
            encode_args(([500u128, ADD[1], ADD[2]].to_vec(), 0u128)).unwrap(),
        ).unwrap(),
    )).unwrap();
    assert!(matches!(result, Err(ThreePoolError::InsufficientOutput { .. })), "unexpected add result: {result:?}");
    assert_eq!(h.ledgers.map(|ledger| balance(&h, ledger, h.user)), before);
    assert!(claims(&h).is_empty());
    assert_eq!(payout_storage_counts(&h), payouts_before, "dust leg must be rejected before payout reservation");
}

#[test]
fn swap_fee_query_rejection_happens_before_any_input_or_payout_identity() {
    let h = setup();
    // Bootstrap add-liquidity uses uncached strict fee queries, so the swap's
    // first output-fee lookup is cold and reaches this rejecting ledger.
    call_ledger_flag(&h, h.ledgers[1], "set_fail_fee_query", true);
    let user_input_before = balance(&h, h.ledgers[0], h.user);
    let user_output_before = balance(&h, h.ledgers[1], h.user);
    let pool_input_before = balance(&h, h.ledgers[0], h.pool);
    let pool_output_before = balance(&h, h.ledgers[1], h.pool);
    let payouts_before = payout_storage_counts(&h);

    let failed: Result<Nat, ThreePoolError> = decode_one(&reply(
        h.pic.update_call(
            h.pool,
            h.user,
            "swap",
            encode_args((0u8, 1u8, 100_000_000u128, 1u128)).unwrap(),
        ).unwrap(),
    )).unwrap();
    assert!(matches!(
        failed,
        Err(ThreePoolError::TransferFailed { ref token, ref reason })
            if token == "T1" && reason.contains("icrc1_fee query failed")
    ));
    assert_eq!(balance(&h, h.ledgers[0], h.user), user_input_before);
    assert_eq!(balance(&h, h.ledgers[1], h.user), user_output_before);
    assert_eq!(balance(&h, h.ledgers[0], h.pool), pool_input_before);
    assert_eq!(balance(&h, h.ledgers[1], h.pool), pool_output_before);
    assert_eq!(payout_storage_counts(&h), payouts_before);
    assert!(claims(&h).is_empty());
}

#[test]
fn swap_rejects_fee_sized_input_before_pull_and_accepts_above_fee_control() {
    let h = setup();
    // T1 has six decimals and T0 has eight. This direction can quote a
    // positive output even when the entire input is needed for its refund fee.
    call_ledger_nat(&h, h.ledgers[1], "set_fee", 1_000);
    let user_before = h.ledgers.map(|ledger| balance(&h, ledger, h.user));
    let pool_before = h.ledgers.map(|ledger| balance(&h, ledger, h.pool));
    let payouts_before = payout_storage_counts(&h);

    let rejected: Result<Nat, ThreePoolError> = decode_one(&reply(
        h.pic.update_call(
            h.pool,
            h.user,
            "swap",
            encode_args((1u8, 0u8, 1_000u128, 0u128)).unwrap(),
        ).unwrap(),
    )).unwrap();
    assert!(matches!(rejected, Err(ThreePoolError::InsufficientOutput { .. })),
        "fee-sized input must be rejected even with positive net output: {rejected:?}");
    assert_eq!(h.ledgers.map(|ledger| balance(&h, ledger, h.user)), user_before);
    assert_eq!(h.ledgers.map(|ledger| balance(&h, ledger, h.pool)), pool_before);
    assert_eq!(payout_storage_counts(&h), payouts_before);
    assert!(claims(&h).is_empty());

    let admitted: Result<Nat, ThreePoolError> = decode_one(&reply(
        h.pic.update_call(
            h.pool,
            h.user,
            "swap",
            encode_args((1u8, 0u8, 1_001u128, 0u128)).unwrap(),
        ).unwrap(),
    )).unwrap();
    assert!(admitted.is_ok(), "above-fee control should remain tradable: {admitted:?}");
    // The ledger charges the input transfer fee in addition to `dx`.
    assert_eq!(user_before[1] - balance(&h, h.ledgers[1], h.user), 1_001 + 1_000);
    assert!(balance(&h, h.ledgers[0], h.user) > user_before[0]);
}

#[test]
fn swap_fee_rise_after_quote_rejects_pinned_input_without_moving_funds() {
    let h = setup();
    call_ledger_nat(&h, h.ledgers[1], "set_fee", 100);
    let user_before = h.ledgers.map(|ledger| balance(&h, ledger, h.user));
    let pool_before = h.ledgers.map(|ledger| balance(&h, ledger, h.pool));
    let payouts_before = payout_storage_counts(&h);
    let _: () = decode_one(&reply(h.pic.update_call(
        h.pool,
        h.admin,
        "test_gate_next_swap_input_pull",
        encode_args(()).unwrap(),
    ).unwrap())).unwrap();

    let swap = h.pic.submit_call(
        h.pool,
        h.user,
        "swap",
        encode_args((1u8, 0u8, 101u128, 0u128)).unwrap(),
    ).unwrap();
    let gate = (0..80).find_map(|_| {
        h.pic.tick();
        h.pic.get_canister_http().into_iter()
            .find(|request| request.url == "https://3pool-swap-fee-gate.test/hold")
    }).expect("swap reached the barrier after its input fee query");
    call_ledger_nat(&h, h.ledgers[1], "set_fee", 1_000);
    h.pic.mock_canister_http_response(MockCanisterHttpResponse {
        subnet_id: gate.subnet_id,
        request_id: gate.request_id,
        response: CanisterHttpResponse::CanisterHttpReply(CanisterHttpReply {
            status: 200,
            headers: vec![],
            body: b"release".to_vec(),
        }),
        additional_responses: vec![],
    });
    let rejected: Result<Nat, ThreePoolError> = decode_one(&reply(h.pic.await_call(swap).unwrap())).unwrap();
    assert!(matches!(rejected, Err(ThreePoolError::TransferFailed { ref token, ref reason })
        if token == "T1" && reason.contains("BadFee")),
        "the pinned input fee must cause a no-effect ledger rejection: {rejected:?}");
    assert_eq!(h.ledgers.map(|ledger| balance(&h, ledger, h.user)), user_before);
    assert_eq!(h.ledgers.map(|ledger| balance(&h, ledger, h.pool)), pool_before);
    assert_eq!(payout_storage_counts(&h), payouts_before);
    assert!(claims(&h).is_empty());
}

#[test]
fn donation_pins_fresh_input_fee_after_output_cache_becomes_stale() {
    let h = setup();
    // A T1 -> T0 swap warms T0's outbound fee cache at zero.
    let warm: Result<Nat, ThreePoolError> = decode_one(&reply(h.pic.update_call(
        h.pool, h.user, "swap",
        encode_args((1u8, 0u8, 100_000u128, 0u128)).unwrap(),
    ).unwrap())).unwrap();
    warm.expect("warm T0 fee cache");
    call_ledger_nat(&h, h.ledgers[0], "set_fee", 100);
    let user_before = balance(&h, h.ledgers[0], h.user);
    let pool_before = balance(&h, h.ledgers[0], h.pool);

    let donated: Result<(), ThreePoolError> = decode_one(&reply(h.pic.update_call(
        h.pool, h.user, "donate",
        encode_args((0u8, 1_000u128)).unwrap(),
    ).unwrap())).unwrap();
    donated.expect("donation must query and pin the current input fee");
    assert_eq!(user_before - balance(&h, h.ledgers[0], h.user), 1_100);
    assert_eq!(balance(&h, h.ledgers[0], h.pool) - pool_before, 1_000);
    assert!(claims(&h).is_empty());
}

#[test]
fn proportional_exit_fee_query_rejection_happens_before_lp_or_reserve_debit() {
    let h = setup();
    call_ledger_flag(&h, h.ledgers[0], "set_fail_fee_query", true);
    let lp_before = lp_balance(&h);
    let pool_balances_before = h.ledgers.map(|ledger| balance(&h, ledger, h.pool));
    let payouts_before = payout_storage_counts(&h);

    let failed: Result<Vec<Nat>, ThreePoolError> = decode_one(&reply(
        h.pic.update_call(
            h.pool,
            h.user,
            "remove_liquidity",
            encode_args((100_000_000u128, vec![0u128; 3])).unwrap(),
        ).unwrap(),
    )).unwrap();
    assert!(matches!(
        failed,
        Err(ThreePoolError::TransferFailed { ref reason, .. })
            if reason.contains("icrc1_fee query failed")
    ));
    assert_eq!(lp_balance(&h), lp_before);
    assert_eq!(h.ledgers.map(|ledger| balance(&h, ledger, h.pool)), pool_balances_before);
    assert_eq!(payout_storage_counts(&h), payouts_before);
    assert!(claims(&h).is_empty());
}

#[test]
fn add_liquidity_fails_closed_when_actual_fee_query_fails() {
    let h = setup();
    // Prime the general fee cache at zero through a donation, which has no
    // promised outbound refund. The live fee is then higher, so add admission
    // must bypass that stale cache and fail closed.
    let donated: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic.update_call(
            h.pool,
            h.user,
            "donate",
            encode_args((0u8, 1_000u128)).unwrap(),
        ).unwrap(),
    )).unwrap();
    donated.expect("donation should not require the add-liquidity refund fee guard");
    call_ledger_nat(&h, h.ledgers[0], "set_fee", 1_000);
    call_ledger_flag(&h, h.ledgers[0], "set_fail_fee_query", true);
    let before = h.ledgers.map(|ledger| balance(&h, ledger, h.user));
    let payouts_before = payout_storage_counts(&h);

    let failed_query: Result<Nat, ThreePoolError> = decode_one(&reply(
        h.pic.update_call(
            h.pool,
            h.user,
            "add_liquidity",
            encode_args(([500u128, ADD[1], ADD[2]].to_vec(), 0u128)).unwrap(),
        ).unwrap(),
    )).unwrap();
    assert!(matches!(
        failed_query,
        Err(ThreePoolError::TransferFailed { ref token, ref reason })
            if token == "T0" && reason.contains("actual refund fee")
    ));
    assert_eq!(h.ledgers.map(|ledger| balance(&h, ledger, h.user)), before);
    assert!(claims(&h).is_empty());
    assert_eq!(payout_storage_counts(&h), payouts_before);

    call_ledger_flag(&h, h.ledgers[0], "set_fail_fee_query", false);
    let known_dust: Result<Nat, ThreePoolError> = decode_one(&reply(
        h.pic.update_call(
            h.pool,
            h.user,
            "add_liquidity",
            encode_args(([500u128, ADD[1], ADD[2]].to_vec(), 0u128)).unwrap(),
        ).unwrap(),
    )).unwrap();
    assert!(matches!(known_dust, Err(ThreePoolError::InsufficientOutput { .. })));
    assert_eq!(h.ledgers.map(|ledger| balance(&h, ledger, h.user)), before);
    assert_eq!(payout_storage_counts(&h), payouts_before);
}

#[test]
fn committed_add_input_with_lost_reply_is_reconciled_before_refund() {
    let h = setup();
    let before = balance(&h, h.ledgers[0], h.user);
    h.pic
        .update_call(
            h.ledgers[0],
            h.user,
            "set_phantom_failures",
            encode_one(1u32).unwrap(),
        )
        .unwrap();

    let result: Result<Nat, ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(
                h.pool,
                h.user,
                "add_liquidity",
                encode_args((ADD.to_vec(), 0u128)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    assert!(matches!(result, Err(ThreePoolError::TransferFailed { .. })));
    let held = payouts(&h, h.user)
        .into_iter()
        .find(|item| {
            item.kind == PayoutKind::AddLiquidityRefund
                && item.input_transfer.as_ref().map(|transfer| transfer.amount) == Some(ADD[0])
        })
        .expect("prebound refund identity remains visible");
    assert_eq!(held.input_transfer.as_ref().unwrap().amount, ADD[0]);
    assert!(balance(&h, h.ledgers[0], h.user) < before);
    let after_committed_pull = balance(&h, h.ledgers[0], h.user);

    let recovered: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(
                h.pool,
                h.user,
                "claim_pending",
                encode_one(held.id).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    recovered.expect("exact input replay proves the committed pull before refund");
    assert_eq!(balance(&h, h.ledgers[0], h.user), before);
    assert!(balance(&h, h.ledgers[0], h.user) > after_committed_pull);
    assert!(claims(&h).iter().all(|claim| claim.id != held.id));
}

#[test]
fn aged_ambiguous_add_input_requires_directly_anchored_archive_chain_before_refund() {
    let h = setup();
    let balance_before = balance(&h, h.ledgers[0], h.user);
    h.pic.update_call(
        h.ledgers[0], h.user, "set_phantom_failures", encode_one(1u32).unwrap(),
    ).unwrap();
    let result: Result<Nat, ThreePoolError> = decode_one(&reply(
        h.pic.update_call(
            h.pool, h.user, "add_liquidity", encode_args((ADD.to_vec(), 0u128)).unwrap(),
        ).unwrap(),
    )).unwrap();
    assert!(matches!(result, Err(ThreePoolError::TransferFailed { .. })));
    let held = payouts(&h, h.user).into_iter()
        .find(|item| item.kind == PayoutKind::AddLiquidityRefund
            && item.input_transfer.as_ref().map(|transfer| transfer.amount) == Some(ADD[0]))
        .expect("ambiguous input identity is retained");
    let input = held.input_transfer.clone().unwrap();
    let block_index = ledger_block_for_memo(&h, h.ledgers[0], input.memo.as_ref().unwrap());
    assert_eq!(balance_before - balance(&h, h.ledgers[0], h.user), ADD[0]);

    let unauthorized: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic.update_call(
            h.pool, Principal::anonymous(), "reconcile_aged_input",
            encode_args((held.id, block_index)).unwrap(),
        ).unwrap(),
    )).unwrap();
    assert!(matches!(unauthorized, Err(ThreePoolError::Unauthorized)));
    let early: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic.update_call(
            h.pool, h.user, "reconcile_aged_input",
            encode_args((held.id, block_index)).unwrap(),
        ).unwrap(),
    )).unwrap();
    assert!(matches!(early, Err(ThreePoolError::TransferFailed { .. })));

    let now = h.pic.get_time();
    h.pic.set_time(now + std::time::Duration::from_secs(24 * 60 * 60));
    let wrong: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic.update_call(
            h.pool, h.user, "reconcile_aged_input",
            encode_args((held.id, block_index.saturating_sub(1))).unwrap(),
        ).unwrap(),
    )).unwrap();
    assert!(matches!(wrong, Err(ThreePoolError::TransferFailed { .. })));
    assert!(matches!(payout(&h, h.user, held.id).input_outcome,
        Some(rumi_3pool::payouts::PayoutInputOutcome::Unresolved { .. })));

    h.pic.update_call(
        h.ledgers[0], h.user, "set_icrc3_archive_range",
        encode_one(Some((block_index, 1u64))).unwrap(),
    ).unwrap();
    let unavailable_archive: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic.update_call(
            h.pool, h.user, "reconcile_aged_input",
            encode_args((held.id, block_index)).unwrap(),
        ).unwrap(),
    )).unwrap();
    assert!(matches!(unavailable_archive, Err(ThreePoolError::TransferFailed { .. })));

    // Leave nine archived child blocks followed by one directly served anchor.
    // The first reconciliation persists only the verified prefix; after an
    // upgrade, the next bounded page reaches the direct anchor and settles.
    for _ in 0..9 {
        let _: () = decode_one(&reply(h.pic.update_call(
            h.ledgers[0], h.admin, "mint",
            encode_args((Account { owner: h.admin, subaccount: None }, Nat::from(1u8))).unwrap(),
        ).unwrap())).unwrap();
    }
    h.pic.update_call(
        h.ledgers[0], h.user, "set_icrc3_archive_range",
        encode_one(Some((block_index, 9u64))).unwrap(),
    ).unwrap();
    h.pic.update_call(
        h.ledgers[0], h.user, "set_icrc3_archive_callback_enabled", encode_one(true).unwrap(),
    ).unwrap();
    let forged_candidate: Result<(), ThreePoolError> = decode_one(&reply(h.pic.update_call(
        h.pool, h.user, "reconcile_aged_input", encode_args((held.id, block_index + 1)).unwrap(),
    ).unwrap())).unwrap();
    assert!(matches!(forged_candidate, Err(ThreePoolError::TransferFailed { .. })));

    // A callback can return the exact candidate but a child with a false
    // parent hash. That archived evidence must be rejected and its cursor
    // discarded before retrying the uncorrupted history.
    h.pic.update_call(
        h.ledgers[0], h.user, "set_icrc3_corrupt_archived_phash_at",
        encode_one(Some(block_index + 1)).unwrap(),
    ).unwrap();
    let broken_link: Result<(), ThreePoolError> = decode_one(&reply(h.pic.update_call(
        h.pool, h.user, "reconcile_aged_input", encode_args((held.id, block_index)).unwrap(),
    ).unwrap())).unwrap();
    assert!(matches!(broken_link, Err(ThreePoolError::TransferFailed { .. })));
    h.pic.update_call(
        h.ledgers[0], h.user, "set_icrc3_corrupt_archived_phash_at", encode_one(None::<u64>).unwrap(),
    ).unwrap();

    let bounded_progress: Result<(), ThreePoolError> = decode_one(&reply(h.pic.update_call(
        h.pool, h.user, "reconcile_aged_input", encode_args((held.id, block_index)).unwrap(),
    ).unwrap())).unwrap();
    assert!(matches!(bounded_progress, Err(ThreePoolError::TransferFailed { .. })));
    assert!(matches!(payout(&h, h.user, held.id).input_outcome,
        Some(rumi_3pool::payouts::PayoutInputOutcome::Unresolved { .. })));

    h.pic.upgrade_canister(
        h.pool, wasm_artifact("rumi_3pool.wasm"), encode_args(()).unwrap(), None,
    ).unwrap();
    let wrong_cursor_identity: Result<(), ThreePoolError> = decode_one(&reply(h.pic.update_call(
        h.pool, h.user, "reconcile_aged_input", encode_args((held.id, block_index + 1)).unwrap(),
    ).unwrap())).unwrap();
    assert!(matches!(wrong_cursor_identity, Err(ThreePoolError::TransferFailed { .. })));
    let restarted: Result<(), ThreePoolError> = decode_one(&reply(h.pic.update_call(
        h.pool, h.user, "reconcile_aged_input", encode_args((held.id, block_index)).unwrap(),
    ).unwrap())).unwrap();
    assert!(matches!(restarted, Err(ThreePoolError::TransferFailed { .. })),
        "changing the candidate must discard the cursor and restart bounded proof");

    // Move the archive boundary over the would-be direct anchor. The verified
    // cursor may advance through callback data but must remain held at log end.
    h.pic.update_call(
        h.ledgers[0], h.user, "set_icrc3_archive_range",
        encode_one(Some((block_index, 10u64))).unwrap(),
    ).unwrap();
    let archived_anchor: Result<(), ThreePoolError> = decode_one(&reply(h.pic.update_call(
        h.pool, h.user, "reconcile_aged_input", encode_args((held.id, block_index)).unwrap(),
    ).unwrap())).unwrap();
    assert!(matches!(archived_anchor, Err(ThreePoolError::TransferFailed { .. })));
    let absent_anchor: Result<(), ThreePoolError> = decode_one(&reply(h.pic.update_call(
        h.pool, h.user, "reconcile_aged_input", encode_args((held.id, block_index)).unwrap(),
    ).unwrap())).unwrap();
    assert!(matches!(absent_anchor, Err(ThreePoolError::TransferFailed { .. })));
    assert!(!payout(&h, h.user, held.id).settled);

    // A later block served directly by the ledger supplies the required
    // anchor; archived data alone has not changed the held entitlement.
    let _: () = decode_one(&reply(h.pic.update_call(
        h.ledgers[0], h.admin, "mint",
        encode_args((Account { owner: h.admin, subaccount: None }, Nat::from(1u8))).unwrap(),
    ).unwrap())).unwrap();
    let reconciled: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic.update_call(
            h.pool, h.user, "reconcile_aged_input",
            encode_args((held.id, block_index)).unwrap(),
        ).unwrap(),
    )).unwrap();
    reconciled.expect("archived exact ICRC-2 proof should authorize the original refund after upgrade");
    assert_eq!(balance(&h, h.ledgers[0], h.user), balance_before);
    assert!(payout(&h, h.user, held.id).settled);
    let replay: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic.update_call(
            h.pool, h.user, "reconcile_aged_input",
            encode_args((held.id, block_index)).unwrap(),
        ).unwrap(),
    )).unwrap();
    assert!(matches!(replay, Err(ThreePoolError::TransferFailed { .. })));
    assert_eq!(balance(&h, h.ledgers[0], h.user), balance_before);
}

#[test]
fn aged_ambiguous_swap_input_proof_resumes_only_the_bound_output() {
    let h = setup();
    let input_before = balance(&h, h.ledgers[0], h.user);
    let output_before = balance(&h, h.ledgers[1], h.user);
    h.pic.update_call(
        h.ledgers[0], h.user, "set_phantom_failures", encode_one(1u32).unwrap(),
    ).unwrap();
    let result: Result<u128, ThreePoolError> = decode_one(&reply(
        h.pic.update_call(
            h.pool, h.user, "swap",
            encode_args((0u8, 1u8, 100_000_000u128, 1u128)).unwrap(),
        ).unwrap(),
    )).unwrap();
    assert!(matches!(result, Err(ThreePoolError::TransferFailed { .. })));
    let refund = payouts(&h, h.user).into_iter()
        .find(|item| item.kind == PayoutKind::SwapInputRefund)
        .expect("swap input and refund identity remain bound");
    let parent_id = refund.compensation_for.expect("prebound output");
    let input = refund.input_transfer.as_ref().unwrap();
    let block_index = ledger_block_for_memo(&h, h.ledgers[0], input.memo.as_ref().unwrap());
    assert_eq!(balance(&h, h.ledgers[0], h.user), input_before - 100_000_000);
    assert_eq!(balance(&h, h.ledgers[1], h.user), output_before);

    let now = h.pic.get_time();
    h.pic.set_time(now + std::time::Duration::from_secs(24 * 60 * 60));
    let recovered: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic.update_call(
            h.pool, h.user, "reconcile_aged_input",
            encode_args((refund.id, block_index)).unwrap(),
        ).unwrap(),
    )).unwrap();
    recovered.expect("confirmed input resumes the original output entitlement");
    assert!(balance(&h, h.ledgers[1], h.user) > output_before);
    assert_eq!(balance(&h, h.ledgers[0], h.user), input_before - 100_000_000);
    assert!(payout(&h, h.user, parent_id).settled);
    let canceled_refund = payout(&h, h.user, refund.id);
    assert!(canceled_refund.settled, "confirmed input evidence is retained");
    assert_eq!(canceled_refund.dispatch_ready, Some(false));
    assert!(canceled_refund.attempts.iter().all(|attempt|
        matches!(attempt.outcome, PayoutOutcome::Prepared)),
        "input refund was never dispatched");
    assert!(claims(&h).is_empty());
    let output_after = balance(&h, h.ledgers[1], h.user);
    let replay: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic.update_call(
            h.pool, h.user, "reconcile_aged_input",
            encode_args((refund.id, block_index)).unwrap(),
        ).unwrap(),
    )).unwrap();
    assert!(matches!(replay, Err(ThreePoolError::TransferFailed { .. })));
    assert_eq!(balance(&h, h.ledgers[1], h.user), output_after);
}

#[test]
fn committed_swap_input_with_lost_reply_recovers_same_identity_without_second_pull() {
    let h = setup();
    let input_before = balance(&h, h.ledgers[0], h.user);
    let output_before = balance(&h, h.ledgers[1], h.user);
    h.pic
        .update_call(
            h.ledgers[0],
            h.user,
            "set_phantom_failures",
            encode_one(1u32).unwrap(),
        )
        .unwrap();

    let result: Result<u128, ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(
                h.pool,
                h.user,
                "swap",
                encode_args((0u8, 1u8, 100_000_000u128, 1u128)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    assert!(matches!(result, Err(ThreePoolError::TransferFailed { .. })));
    let output = payouts(&h, h.user)
        .into_iter()
        .find(|item| item.kind == PayoutKind::SwapOutput)
        .expect("output identity must stay bound to the input");
    let pull = balance(&h, h.ledgers[0], h.user);
    assert!(pull < input_before, "the fake ledger committed the input pull");
    assert_eq!(input_before - pull, 100_000_000);

    let recovered: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(
                h.pool,
                h.user,
                "claim_pending",
                encode_one(output.id).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    recovered.expect("same input tuple replays as Duplicate before output dispatch");
    assert_eq!(balance(&h, h.ledgers[0], h.user), pull);
    assert!(balance(&h, h.ledgers[1], h.user) > output_before);
    assert!(payout(&h, h.user, output.id).settled);
    assert!(claims(&h).iter().all(|claim| claim.id != output.id));
}

#[test]
fn repeated_same_amount_inputs_have_distinct_persisted_dedup_memos() {
    let h = setup();
    let initial = balance(&h, h.ledgers[0], h.user);

    for _ in 0..2 {
        let added: Result<Nat, ThreePoolError> = decode_one(&reply(
            h.pic
                .update_call(
                    h.pool,
                    h.user,
                    "add_liquidity",
                    encode_args((ADD.to_vec(), 0u128)).unwrap(),
                )
                .unwrap(),
        ))
        .unwrap();
        added.expect("same-amount add should pull each operation once");
    }
    for _ in 0..2 {
        let swapped: Result<u128, ThreePoolError> = decode_one(&reply(
            h.pic
                .update_call(
                    h.pool,
                    h.user,
                    "swap",
                    encode_args((0u8, 1u8, 100_000_000u128, 1u128)).unwrap(),
                )
                .unwrap(),
        ))
        .unwrap();
        swapped.expect("same-amount swaps must not deduplicate distinct pulls");
    }
    for _ in 0..2 {
        let donated: Result<(), ThreePoolError> = decode_one(&reply(
            h.pic
                .update_call(
                    h.pool,
                    h.user,
                    "donate",
                    encode_args((0u8, 12_345u128)).unwrap(),
                )
                .unwrap(),
        ))
        .unwrap();
        donated.expect("same-amount donations must not deduplicate distinct pulls");
    }

    let expected_input = 2 * ADD[0] + 2 * 100_000_000 + 2 * 12_345;
    assert_eq!(initial - balance(&h, h.ledgers[0], h.user), expected_input);
    let records = payouts(&h, h.user);
    for action in [PayoutInputAction::AddLiquidity, PayoutInputAction::Swap, PayoutInputAction::Donation] {
        let expected_gross = match action {
            PayoutInputAction::AddLiquidity => ADD[0],
            PayoutInputAction::Swap => 100_000_000,
            PayoutInputAction::Donation => 12_345,
        };
        let pair: Vec<_> = records
            .iter()
            .filter(|record| record.input_action == Some(action) && record.gross == expected_gross)
            .collect();
        assert_eq!(pair.len(), 2, "both same-amount operations retain their input identities");
        assert_ne!(pair[0].id, pair[1].id);
        assert_ne!(
            pair[0].input_transfer.as_ref().unwrap().memo,
            pair[1].input_transfer.as_ref().unwrap().memo,
            "per-operation memo is part of the ICRC2 dedup tuple",
        );
    }
}

#[test]
fn repeated_no_effect_operations_retire_rows_and_allow_honest_successor() {
    let h = setup();
    for ledger in h.ledgers {
        set_allowance(&h, ledger, 0);
    }
    let initial_counts = payout_storage_counts(&h);
    let initial_balances = h.ledgers.map(|ledger| balance(&h, ledger, h.user));

    for _ in 0..100 {
        let add: Result<Nat, ThreePoolError> = decode_one(&reply(
            h.pic.update_call(
                h.pool, h.user, "add_liquidity",
                encode_args((ADD.to_vec(), 0u128)).unwrap(),
            ).unwrap(),
        )).unwrap();
        assert!(add.is_err());

        let swap: Result<u128, ThreePoolError> = decode_one(&reply(
            h.pic.update_call(
                h.pool, h.user, "swap",
                encode_args((0u8, 1u8, 100_000_000u128, 1u128)).unwrap(),
            ).unwrap(),
        )).unwrap();
        assert!(swap.is_err());

        let donate: Result<(), ThreePoolError> = decode_one(&reply(
            h.pic.update_call(
                h.pool, h.user, "donate",
                encode_args((0u8, 12_345u128)).unwrap(),
            ).unwrap(),
        )).unwrap();
        assert!(donate.is_err());
    }

    assert_eq!(payout_storage_counts(&h), initial_counts,
        "definitive no-effect attempts must not grow retained rows or append-only evidence");
    assert_eq!(h.ledgers.map(|ledger| balance(&h, ledger, h.user)), initial_balances,
        "rejected allowance attempts must not debit inputs");

    for ledger in h.ledgers {
        set_allowance(&h, ledger, u128::MAX);
    }
    let honest: Result<Nat, ThreePoolError> = decode_one(&reply(
        h.pic.update_call(
            h.pool, h.user, "add_liquidity",
            encode_args((ADD.to_vec(), 0u128)).unwrap(),
        ).unwrap(),
    )).unwrap();
    honest.expect("an honest approved add after rejected spam must succeed");
    let after = payout_storage_counts(&h);
    assert!(after.0 >= initial_counts.0 + 3,
        "confirmed ingress identities must remain owner-visible after settlement");
    assert!(after.1 > initial_counts.1,
        "confirmed ingress receipt evidence must remain append-only");
}

#[test]
fn same_time_identical_input_requests_each_debit_once() {
    let h = setup();
    let amount = 7_777u128;
    let before = balance(&h, h.ledgers[0], h.user);
    let ids: Result<(u64, u64), String> = decode_one(&reply(
        h.pic
            .update_call(
                h.pool,
                h.user,
                "test_same_time_input_pair",
                encode_args((0u8, amount)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    let (first_id, second_id) = ids.expect("test pulls should both commit");
    let first = payout(&h, h.user, first_id).input_transfer.unwrap();
    let second = payout(&h, h.user, second_id).input_transfer.unwrap();
    assert_eq!(first.created_at_time, second.created_at_time);
    assert_eq!(first.ledger, second.ledger);
    assert_eq!(first.from, second.from);
    assert_eq!(first.to, second.to);
    assert_eq!(first.amount, second.amount);
    assert_ne!(first.memo, second.memo);
    assert_eq!(before - balance(&h, h.ledgers[0], h.user), 2 * amount);
}

#[test]
fn admin_fee_legs_are_prebound_before_trap_and_recover_after_upgrade() {
    let h = setup();
    let fees = [500u128, 600, 700];
    h.pic
        .update_call(
            h.pool,
            h.admin,
            "test_seed_admin_fees",
            encode_one(fees).unwrap(),
        )
        .unwrap();
    h.pic
        .update_call(
            h.pool,
            h.admin,
            "test_trap_after_admin_fee_first_leg",
            encode_args(()).unwrap(),
        )
        .unwrap();

    let interrupted = h.pic.update_call(
        h.pool,
        h.admin,
        "withdraw_admin_fees",
        encode_args(()).unwrap(),
    );
    assert!(interrupted.is_err(), "test trap must interrupt after leg one: {interrupted:?}");
    let entitlements: Vec<_> = payouts(&h, h.admin)
        .into_iter()
        .filter(|record| record.kind == PayoutKind::AdminFeeWithdrawal)
        .collect();
    assert_eq!(entitlements.len(), 3, "all fee legs must be persisted before first dispatch");
    assert!(entitlements.iter().all(|record| record.attempts.len() == 1));
    assert_eq!(claims(&h).len(), 3, "each debited liability has a recovery projection");
    let admin_fees: Vec<u128> = decode_one(&reply(
        h.pic.query_call(h.pool, h.admin, "get_admin_fees", encode_args(()).unwrap()).unwrap(),
    )).unwrap();
    assert_eq!(admin_fees, vec![0, 0, 0]);
    assert_eq!(balance(&h, h.ledgers[0], h.admin), fees[0]);
    assert_eq!(balance(&h, h.ledgers[1], h.admin), 0);
    assert_eq!(balance(&h, h.ledgers[2], h.admin), 0);

    h.pic.upgrade_canister(
        h.pool,
        wasm_artifact("rumi_3pool.wasm"),
        encode_args(()).unwrap(),
        None,
    ).unwrap();
    for claim in claims(&h) {
        let recovered: Result<(), ThreePoolError> = decode_one(&reply(
            h.pic.update_call(
                h.pool,
                h.admin,
                "claim_pending",
                encode_one(claim.id).unwrap(),
            ).unwrap(),
        )).unwrap();
        recovered.expect("every prebound fee leg must recover after upgrade");
    }
    for (ledger, fee) in h.ledgers.into_iter().zip(fees) {
        assert_eq!(balance(&h, ledger, h.admin), fee);
    }
}

#[test]
fn admin_fee_below_ledger_fee_remains_accrued() {
    let h = setup();
    h.pic.update_call(
        h.pool, h.admin, "test_seed_admin_fees", encode_one([100u128, 0, 0]).unwrap(),
    ).unwrap();
    call_ledger_nat(&h, h.ledgers[0], "set_fee", 100);
    let withdrawn: Result<Vec<u128>, ThreePoolError> = decode_one(&reply(
        h.pic.update_call(h.pool, h.admin, "withdraw_admin_fees", encode_args(()).unwrap()).unwrap(),
    )).unwrap();
    assert_eq!(withdrawn.unwrap(), vec![0, 0, 0]);
    let accrued: Vec<u128> = decode_one(&reply(
        h.pic.query_call(h.pool, h.admin, "get_admin_fees", encode_args(()).unwrap()).unwrap(),
    )).unwrap();
    assert_eq!(accrued, vec![100, 0, 0], "fee dust is not silently written off");
    assert!(claims(&h).is_empty());
}

#[test]
fn admin_fee_query_rejection_preserves_the_liability() {
    let h = setup();
    let fees = [12_345u128, 0, 0];
    h.pic.update_call(
        h.pool,
        h.admin,
        "test_seed_admin_fees",
        encode_one(fees).unwrap(),
    ).unwrap();
    call_ledger_flag(&h, h.ledgers[0], "set_fail_fee_query", true);
    let payouts_before = payout_storage_counts(&h);

    let failed: Result<Vec<u128>, ThreePoolError> = decode_one(&reply(
        h.pic.update_call(
            h.pool,
            h.admin,
            "withdraw_admin_fees",
            encode_args(()).unwrap(),
        ).unwrap(),
    )).unwrap();
    assert!(matches!(
        failed,
        Err(ThreePoolError::TransferFailed { ref token, ref reason })
            if token == "T0" && reason.contains("icrc1_fee query failed")
    ));
    let accrued: Vec<u128> = decode_one(&reply(
        h.pic.query_call(h.pool, h.admin, "get_admin_fees", encode_args(()).unwrap()).unwrap(),
    )).unwrap();
    assert_eq!(accrued, fees, "fee lookup failure must not clear any liability");
    assert_eq!(payout_storage_counts(&h), payouts_before);
    assert!(claims(&h).is_empty());
}

#[test]
fn receive_donation_cannot_reclassify_unresolved_donation_input() {
    let h = setup();
    let amount = 54_321u128;
    h.pic
        .update_call(
            h.ledgers[0],
            h.user,
            "set_phantom_failures",
            encode_one(1u32).unwrap(),
        )
        .unwrap();
    let result: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(
                h.pool,
                h.user,
                "donate",
                encode_args((0u8, amount)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    assert!(matches!(result, Err(ThreePoolError::TransferFailed { .. })));
    let held = payouts(&h, h.user)
        .into_iter()
        .find(|record| record.input_action == Some(PayoutInputAction::Donation))
        .expect("committed donation pull remains a recoverable entitlement");
    let before: PoolStatus = decode_one(&reply(
        h.pic.query_call(h.pool, h.user, "get_pool_status", encode_args(()).unwrap()).unwrap(),
    )).unwrap();
    let acknowledged: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(
                h.pool,
                h.admin,
                "receive_donation",
                encode_args((0u8, amount)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    assert!(matches!(acknowledged, Err(ThreePoolError::PoolLocked)));
    let after: PoolStatus = decode_one(&reply(
        h.pic
            .query_call(h.pool, h.user, "get_pool_status", encode_args(()).unwrap())
            .unwrap(),
    ))
    .unwrap();
    assert_eq!(after.balances, before.balances);

    // Simulate an upgrade from payout data written before the token-scoped
    // index existed. Backfill is one entry per update in this test: donation
    // stays locked until the cursor reaches the end, then the index still
    // detects the unresolved input.
    h.pic
        .update_call(
            h.pool,
            h.admin,
            "test_reset_unsettled_input_index_backfill",
            encode_args(()).unwrap(),
        )
        .unwrap();
    let index_state: (bool, u64) = decode_args(&reply(
        h.pic
            .query_call(
                h.pool,
                h.admin,
                "test_unsettled_input_index_state",
                encode_args(()).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    assert_eq!(index_state, (false, 0));

    let retained = payout_storage_counts(&h).0;
    let mut index_state: (bool, u64) = (false, 0);
    for _ in 0..=retained {
        let acknowledged: Result<(), ThreePoolError> = decode_one(&reply(
            h.pic
                .update_call(
                    h.pool,
                    h.admin,
                    "receive_donation",
                    encode_args((0u8, amount)).unwrap(),
                )
                .unwrap(),
        ))
        .unwrap();
        assert!(matches!(acknowledged, Err(ThreePoolError::PoolLocked)));
        index_state = decode_args(&reply(
            h.pic
                .query_call(
                    h.pool,
                    h.admin,
                    "test_unsettled_input_index_state",
                    encode_args(()).unwrap(),
                )
                .unwrap(),
        ))
        .unwrap();
        if index_state.0 {
            break;
        }
    }
    assert_eq!(
        index_state,
        (true, 1),
        "bounded backfill reaches and indexes the unresolved input"
    );

    let recovered: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(
                h.pool,
                h.user,
                "claim_pending",
                encode_one(held.id).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    recovered.expect("exact input replay resolves and accounts the donation once");
    assert!(payout(&h, h.user, held.id).settled);
    let index_state: (bool, u64) = decode_args(&reply(
        h.pic
            .query_call(
                h.pool,
                h.admin,
                "test_unsettled_input_index_state",
                encode_args(()).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    assert_eq!(
        index_state,
        (true, 0),
        "settling inbound value removes its index entry"
    );
}

#[test]
fn receive_donation_keeps_unsettled_exit_liability_out_of_donation_slack() {
    let h = setup();
    set_test_cap(&h, 0);
    h.pic.update_call(
        h.ledgers[0], h.user, "set_bad_fee_failures", encode_one(1u32).unwrap(),
    ).unwrap();
    call_ledger_nat(&h, h.ledgers[0], "set_fee", 100);

    let exit: Result<Vec<Nat>, ThreePoolError> = decode_one(&reply(h.pic.update_call(
        h.pool, h.user, "remove_liquidity",
        encode_args((100_000_000u128, vec![0u128; 3])).unwrap(),
    ).unwrap())).unwrap();
    assert!(matches!(exit, Err(ThreePoolError::TransferFailed { .. })));
    let entitlement = payouts(&h, h.user).into_iter()
        .find(|p| p.kind == PayoutKind::RemoveLiquidity && p.token_index == 0 && !p.settled)
        .expect("failed exit remains an unsettled current payout");
    assert!(claims(&h).is_empty(), "journal-only exit must not require a pending_claim row");

    let before: PoolStatus = decode_one(&reply(h.pic.query_call(
        h.pool, h.user, "get_pool_status", encode_args(()).unwrap(),
    ).unwrap())).unwrap();
    let on_chain_before = balance(&h, h.ledgers[0], h.pool);
    assert_eq!(
        on_chain_before,
        before.balances[0] + entitlement.gross,
        "the failed no-effect exit leaves exactly its owed gross amount as apparent slack",
    );
    let donation: Result<(), ThreePoolError> = decode_one(&reply(h.pic.update_call(
        h.pool, h.admin, "receive_donation",
        encode_args((0u8, entitlement.gross)).unwrap(),
    ).unwrap())).unwrap();
    assert!(matches!(donation, Err(ThreePoolError::PoolLocked)));
    let after_rejection: PoolStatus = decode_one(&reply(h.pic.query_call(
        h.pool, h.user, "get_pool_status", encode_args(()).unwrap(),
    ).unwrap())).unwrap();
    assert_eq!(after_rejection.balances, before.balances, "rejected acknowledgement changes no reserves");
    assert_eq!(balance(&h, h.ledgers[0], h.pool), on_chain_before);

    // Simulate the pre-index stable layout: the current payout rows remain,
    // while the newly introduced index must backfill one bounded page at a time.
    h.pic.update_call(
        h.pool, h.admin, "test_reset_unsettled_exit_index_backfill", encode_args(()).unwrap(),
    ).unwrap();
    h.pic.upgrade_canister(
        h.pool, wasm_artifact("rumi_3pool.wasm"), encode_args(()).unwrap(), None,
    ).expect("upgrade preserves old payout rows while the new index is incomplete");
    // The test-only batch override is heap state and resets on upgrade. Reset
    // the empty index again after upgrade so the following calls exercise one
    // stable row per bounded backfill step against the preserved payout map.
    h.pic.update_call(
        h.pool, h.admin, "test_reset_unsettled_exit_index_backfill", encode_args(()).unwrap(),
    ).unwrap();
    let mut index_state: (bool, u64) = decode_args(&reply(h.pic.query_call(
        h.pool, h.admin, "test_unsettled_exit_index_state", encode_args(()).unwrap(),
    ).unwrap())).unwrap();
    assert_eq!(index_state, (false, 0));

    let result: Result<(), ThreePoolError> = decode_one(&reply(h.pic.update_call(
        h.pool, h.admin, "receive_donation",
        encode_args((0u8, entitlement.gross)).unwrap(),
    ).unwrap())).unwrap();
    assert!(matches!(result, Err(ThreePoolError::PoolLocked)));
    index_state = decode_args(&reply(h.pic.query_call(
        h.pool, h.admin, "test_unsettled_exit_index_state", encode_args(()).unwrap(),
    ).unwrap())).unwrap();
    assert_eq!(
        index_state.0,
        false,
        "the first bounded page must leave backfill incomplete",
    );

    let retained_rows = payout_storage_counts(&h).0 as usize;
    let mut saw_liability_while_incomplete = index_state.1 > 0;
    for _ in 0..=retained_rows {
        let result: Result<(), ThreePoolError> = decode_one(&reply(h.pic.update_call(
            h.pool, h.admin, "receive_donation",
            encode_args((0u8, entitlement.gross)).unwrap(),
        ).unwrap())).unwrap();
        assert!(matches!(result, Err(ThreePoolError::PoolLocked)));
        index_state = decode_args(&reply(h.pic.query_call(
            h.pool, h.admin, "test_unsettled_exit_index_state", encode_args(()).unwrap(),
        ).unwrap())).unwrap();
        saw_liability_while_incomplete |= !index_state.0 && index_state.1 > 0;
        if index_state.0 { break; }
    }
    assert!(index_state.0, "bounded backfill completes without accepting donation during migration");
    assert!(index_state.1 > 0, "backfill records the unsettled exit payout");
    assert!(
        saw_liability_while_incomplete,
        "the exit liability must be indexed while later backfill pages remain and donation stays locked",
    );

    let recovered: Result<(), ThreePoolError> = decode_one(&reply(h.pic.update_call(
        h.pool, h.user, "claim_pending", encode_one(entitlement.id).unwrap(),
    ).unwrap())).unwrap();
    recovered.expect("the exact rejected transfer should recover");
    assert!(payout(&h, h.user, entitlement.id).settled);

    // A real fresh transfer creates new slack. Once the old exit is paid, that
    // separately transferred value can be acknowledged as a donation.
    let fresh_amount = 50_000u128;
    let transfer: Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError> = decode_one(&reply(h.pic.update_call(
        h.ledgers[0], h.user, "icrc1_transfer",
        encode_one(icrc_ledger_types::icrc1::transfer::TransferArg {
            from_subaccount: None,
            to: Account { owner: h.pool, subaccount: None },
            amount: Nat::from(fresh_amount),
            fee: None,
            memo: None,
            created_at_time: None,
        }).unwrap(),
    ).unwrap())).unwrap();
    transfer.expect("fresh user-funded deposit transfer");
    let accepted: Result<(), ThreePoolError> = decode_one(&reply(h.pic.update_call(
        h.pool, h.admin, "receive_donation", encode_args((0u8, fresh_amount)).unwrap(),
    ).unwrap())).unwrap();
    accepted.expect("freshly transferred, unencumbered slack is accepted");
}

#[test]
fn committed_output_with_lost_reply_replays_exact_identity_once_and_settles_after_upgrade() {
    let h = setup();
    let input_before = balance(&h, h.ledgers[0], h.user);
    let output_before = balance(&h, h.ledgers[1], h.user);
    h.pic
        .update_call(
            h.ledgers[1],
            h.user,
            "set_phantom_failures",
            encode_one(1u32).unwrap(),
        )
        .unwrap();

    let result: Result<u128, ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(
                h.pool,
                h.user,
                "swap",
                encode_args((0u8, 1u8, 100_000_000u128, 1u128)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    assert!(matches!(result, Err(ThreePoolError::TransferFailed { .. })));
    let claim = claims(&h).into_iter().find(|claim| claim.token_index == 1).unwrap();
    let before_retry = balance(&h, h.ledgers[1], h.user);
    assert!(before_retry > output_before, "phantom mode must commit before reporting failure");
    assert_eq!(input_before - balance(&h, h.ledgers[0], h.user), 100_000_000);
    let first = payout(&h, h.user, claim.id);
    let identity = &first.attempts[0].transfer;
    assert_eq!(identity.ledger, h.ledgers[1]);
    assert_eq!(identity.to.owner, h.user);
    assert_eq!(identity.gross, first.gross);
    assert_eq!(identity.net + identity.fee, identity.gross);
    assert_eq!(identity.memo.len(), 32);
    assert!(matches!(first.attempts[0].outcome, PayoutOutcome::Unresolved { .. }));

    h.pic
        .upgrade_canister(
            h.pool,
            wasm_artifact("rumi_3pool.wasm"),
            encode_args(()).unwrap(),
            None,
        )
        .unwrap();
    let after_upgrade = payout(&h, h.user, claim.id);
    assert_eq!(after_upgrade.attempts[0].transfer, *identity);

    let retried: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(h.pool, h.user, "claim_pending", encode_one(claim.id).unwrap())
            .unwrap(),
    ))
    .unwrap();
    retried.expect("exact replay should return Duplicate and settle the swap");
    assert_eq!(balance(&h, h.ledgers[1], h.user), before_retry);
    assert_eq!(balance(&h, h.ledgers[0], h.user), input_before - 100_000_000);
    assert!(claims(&h).iter().all(|pending| pending.id != claim.id));
    assert!(payout(&h, h.user, claim.id).settled);
}

#[test]
fn aged_ambiguous_payout_accepts_only_exact_positive_block_proof_and_survives_upgrade() {
    let h = setup();
    h.pic
        .update_call(
            h.ledgers[1],
            h.user,
            "set_phantom_failures",
            encode_one(1u32).unwrap(),
        )
        .unwrap();
    let result: Result<u128, ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(
                h.pool,
                h.user,
                "swap",
                encode_args((0u8, 1u8, 100_000_000u128, 1u128)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    assert!(matches!(result, Err(ThreePoolError::TransferFailed { .. })));

    let claim = claims(&h).into_iter().find(|claim| claim.token_index == 1).unwrap();
    let before_reconciliation = balance(&h, h.ledgers[1], h.user);
    let pending = payout(&h, h.user, claim.id);
    let attempt = pending.attempts.last().unwrap().clone();
    assert!(matches!(attempt.outcome, PayoutOutcome::Unresolved { .. }));
    let block_index = ledger_block_for_memo(&h, h.ledgers[1], &attempt.transfer.memo);

    let unauthorized: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(
                h.pool,
                Principal::anonymous(),
                "reconcile_aged_payout",
                encode_args((claim.id, block_index)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    assert!(matches!(unauthorized, Err(ThreePoolError::Unauthorized)));

    let too_early: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(
                h.pool,
                h.user,
                "reconcile_aged_payout",
                encode_args((claim.id, block_index)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    assert!(matches!(too_early, Err(ThreePoolError::TransferFailed { .. })));

    let now = h.pic.get_time();
    h.pic.set_time(
        now + std::time::Duration::from_nanos(24 * 60 * 60 * 1_000_000_000),
    );
    let wrong_block: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(
                h.pool,
                h.user,
                "reconcile_aged_payout",
                encode_args((claim.id, block_index.saturating_sub(1))).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    assert!(matches!(wrong_block, Err(ThreePoolError::TransferFailed { .. })));
    assert!(matches!(
        &payout(&h, h.user, claim.id).attempts.last().unwrap().outcome,
        PayoutOutcome::Unresolved { .. }
    ));

    // The fixture advertises an archive callback that is deliberately not
    // implemented. That response cannot confirm the payout or create a
    // settlement-capable proof cursor.
    h.pic.update_call(
        h.ledgers[1],
        h.user,
        "set_icrc3_archive_range",
        encode_one(Some((block_index, 1u64))).unwrap(),
    ).unwrap();
    let unverifiable_archive: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic.update_call(
            h.pool,
            h.user,
            "reconcile_aged_payout",
            encode_args((claim.id, block_index)).unwrap(),
        ).unwrap(),
    )).unwrap();
    assert!(matches!(unverifiable_archive, Err(ThreePoolError::TransferFailed { .. })));
    assert!(matches!(
        &payout(&h, h.user, claim.id).attempts.last().unwrap().outcome,
        PayoutOutcome::Unresolved { .. }
    ));
    h.pic.update_call(
        h.ledgers[1],
        h.user,
        "set_icrc3_archive_range",
        encode_one(None::<(u64, u64)>).unwrap(),
    ).unwrap();

    let reconciled: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(
                h.pool,
                h.user,
                "reconcile_aged_payout",
                encode_args((claim.id, block_index)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    reconciled.expect("exact aged positive block proof must settle through claim_pending");
    assert_eq!(balance(&h, h.ledgers[1], h.user), before_reconciliation);
    assert!(claims(&h).iter().all(|pending| pending.id != claim.id));

    h.pic
        .upgrade_canister(
            h.pool,
            wasm_artifact("rumi_3pool.wasm"),
            encode_args(()).unwrap(),
            None,
        )
        .unwrap();
    let after_upgrade = payout(&h, h.user, claim.id);
    assert!(after_upgrade.settled);
    assert_eq!(
        after_upgrade.attempts.last().unwrap().outcome,
        PayoutOutcome::Confirmed { block: Nat::from(block_index) },
    );
    assert_eq!(balance(&h, h.ledgers[1], h.user), before_reconciliation);
}

#[test]
fn aged_ambiguous_payout_reconciles_archived_chain_after_upgrade_without_repayment() {
    let h = setup();
    h.pic
        .update_call(
            h.ledgers[1],
            h.user,
            "set_phantom_failures",
            encode_one(1u32).unwrap(),
        )
        .unwrap();
    let result: Result<u128, ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(
                h.pool,
                h.user,
                "swap",
                encode_args((0u8, 1u8, 100_000_000u128, 1u128)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    assert!(matches!(result, Err(ThreePoolError::TransferFailed { .. })));

    let claim = claims(&h)
        .into_iter()
        .find(|claim| claim.token_index == 1)
        .unwrap();
    let user_balance_before_reconciliation = balance(&h, h.ledgers[1], h.user);
    let attempt = payout(&h, h.user, claim.id)
        .attempts
        .last()
        .unwrap()
        .clone();
    let block_index = ledger_block_for_memo(&h, h.ledgers[1], &attempt.transfer.memo);
    assert!(matches!(attempt.outcome, PayoutOutcome::Unresolved { .. }));

    // Extend the ledger after the ambiguous transfer so the proof has 33
    // archived links followed by a directly served child as its anchor.
    for _ in 0..33 {
        let _: () = decode_one(&reply(
            h.pic
                .update_call(
                    h.ledgers[1],
                    h.admin,
                    "mint",
                    encode_args((
                        Account {
                            owner: h.admin,
                            subaccount: None,
                        },
                        Nat::from(1u8),
                    ))
                    .unwrap(),
                )
                .unwrap(),
        ))
        .unwrap();
    }

    let now = h.pic.get_time();
    h.pic.set_time(now + std::time::Duration::from_secs(24 * 60 * 60));
    h.pic
        .update_call(
            h.ledgers[1],
            h.user,
            "set_icrc3_archive_range",
            encode_one(Some((block_index, 33u64))).unwrap(),
        )
        .unwrap();
    h.pic
        .update_call(
            h.ledgers[1],
            h.user,
            "set_icrc3_archive_callback_enabled",
            encode_one(true).unwrap(),
        )
        .unwrap();

    let bounded_progress: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(
                h.pool,
                h.user,
                "reconcile_aged_payout",
                encode_args((claim.id, block_index)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    assert!(matches!(
        bounded_progress,
        Err(ThreePoolError::TransferFailed { .. })
    ));
    assert!(matches!(
        &payout(&h, h.user, claim.id).attempts.last().unwrap().outcome,
        PayoutOutcome::Unresolved { .. }
    ));
    assert_eq!(
        balance(&h, h.ledgers[1], h.user),
        user_balance_before_reconciliation
    );

    h.pic
        .upgrade_canister(
            h.pool,
            wasm_artifact("rumi_3pool.wasm"),
            encode_args(()).unwrap(),
            None,
        )
        .unwrap();
    // The verifier caps each page at eight blocks, matching its callback cap
    // so one archive callback per block still advances on each bounded retry.
    for step in 0..4 {
        let reconciled: Result<(), ThreePoolError> = decode_one(&reply(
            h.pic
                .update_call(
                    h.pool,
                    h.user,
                    "reconcile_aged_payout",
                    encode_args((claim.id, block_index)).unwrap(),
                )
                .unwrap(),
        ))
        .unwrap();
        if step < 3 {
            assert!(matches!(
                reconciled,
                Err(ThreePoolError::TransferFailed { .. })
            ));
            assert!(matches!(
                &payout(&h, h.user, claim.id).attempts.last().unwrap().outcome,
                PayoutOutcome::Unresolved { .. }
            ));
        } else {
            reconciled.expect("the archived chain must reach its direct-ledger anchor");
        }
    }

    assert_eq!(balance(&h, h.ledgers[1], h.user), user_balance_before_reconciliation);
    assert!(claims(&h).iter().all(|pending| pending.id != claim.id));
    let settled = payout(&h, h.user, claim.id);
    assert!(settled.settled);
    assert_eq!(
        settled.attempts.last().unwrap().outcome,
        PayoutOutcome::Confirmed {
            block: Nat::from(block_index)
        },
    );
}

#[test]
fn legacy_claim_survives_upgrade_and_cannot_create_a_fresh_transfer() {
    let h = setup();
    let id: u64 = decode_one(&reply(
        h.pic
            .update_call(
                h.pool,
                h.admin,
                "test_insert_pending_claim",
                encode_args((1u8, 777u128)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    h.pic
        .upgrade_canister(
            h.pool,
            wasm_artifact("rumi_3pool.wasm"),
            encode_args(()).unwrap(),
            None,
        )
        .unwrap();
    let before = balance(&h, h.ledgers[1], h.user);
    let result: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(h.pool, h.admin, "claim_pending", encode_one(id).unwrap())
            .unwrap(),
    ))
    .unwrap();
    assert!(matches!(
        result,
        Err(ThreePoolError::TransferFailed { ref reason, .. })
            if reason.contains("legacy claim has no bound transfer identity")
    ));
    assert_eq!(balance(&h, h.ledgers[1], h.user), before);
    assert!(claims(&h).iter().any(|claim| claim.id == id));
}

#[test]
fn proven_bad_fee_starts_new_exact_attempt_with_refreshed_fee() {
    let h = setup();
    // Exits remain available when the secondary claim index is full. The
    // payout journal already contains the exact durable recovery identity.
    set_test_cap(&h, 0);
    h.pic
        .update_call(
            h.ledgers[0],
            h.user,
            "set_bad_fee_failures",
            encode_one(1u32).unwrap(),
        )
        .unwrap();
    call_ledger_nat(&h, h.ledgers[0], "set_fee", 100);
    let events_before = liquidity_event_count_v2(&h);
    let lp_before = lp_balance(&h);

    let result: Result<Vec<Nat>, ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(
                h.pool,
                h.user,
                "remove_liquidity",
                encode_args((100_000_000u128, vec![0u128; 3])).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    let failure_reason = match result {
        Err(ThreePoolError::TransferFailed { reason, .. }) => reason,
        other => panic!("unexpected proportional exit result: {other:?}"),
    };
    assert!(claims(&h).is_empty(), "exit recovery must not need a pending-claim slot");
    let claim = payouts(&h, h.user).into_iter()
        .find(|item| item.kind == PayoutKind::RemoveLiquidity && item.token_index == 0)
        .expect("failed exit remains discoverable in the payout journal");
    assert!(!claim.settled, "failed transfer must stay unsettled until recovery");
    assert_eq!(liquidity_event_count_v2(&h), events_before + 1);
    assert!(failure_reason.contains(&format!("claim_pending({})", claim.id)));
    assert!(failure_reason.contains(&format!("get_payout_entitlement({})", claim.id)));
    assert!(failure_reason.contains("get_payout_entitlements"));
    let before_retry = balance(&h, h.ledgers[0], h.user);
    call_ledger_flag(&h, h.ledgers[0], "set_fail_fee_query", true);
    let fee_query_failed: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic.update_call(
            h.pool,
            h.user,
            "claim_pending",
            encode_one(claim.id).unwrap(),
        ).unwrap(),
    )).unwrap();
    assert!(matches!(
        fee_query_failed,
        Err(ThreePoolError::TransferFailed { ref reason, .. })
            if reason.contains("cannot rearm payout") && reason.contains("icrc1_fee query failed")
    ));
    let held = payout(&h, h.user, claim.id);
    assert_eq!(held.attempts.len(), 1, "failed fee lookup must not replace or append an attempt");
    assert!(!held.settled);
    assert!(
        claims(&h).is_empty(),
        "zero-capacity journal-only exits must not fabricate a secondary claim projection"
    );
    assert!(payouts(&h, h.user).iter().any(|item| {
        item.id == claim.id
            && !item.settled
            && item.attempts.len() == 1
            && matches!(
                item.attempts[0].outcome,
                PayoutOutcome::RejectedNoTransfer { .. }
            )
    }), "the payout journal must retain the exact rejected attempt for recovery");
    assert_eq!(balance(&h, h.ledgers[0], h.user), before_retry);

    call_ledger_flag(&h, h.ledgers[0], "set_fail_fee_query", false);
    call_ledger_nat(&h, h.ledgers[0], "set_fee", 250);
    let retried: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic
        .update_call(h.pool, h.user, "claim_pending", encode_one(claim.id).unwrap())
            .unwrap(),
    ))
    .unwrap();
    retried.expect("typed BadFee proves no transfer and allows a new attempt");
    let updated = payout(&h, h.user, claim.id);
    assert!(updated.settled, "confirmed proportional payout must close its entitlement");
    assert_eq!(updated.attempts.len(), 2);
    assert_eq!(updated.attempts[1].transfer.fee, 250);
    assert_eq!(updated.attempts[1].transfer.net + 250, claim.gross);
    assert_eq!(balance(&h, h.ledgers[0], h.user) - before_retry, claim.gross - 250);
    assert_eq!(lp_balance(&h), lp_before - 100_000_000, "retry must not burn LP again");
    assert_eq!(liquidity_event_count_v2(&h), events_before + 1, "retry must not duplicate the exit event");
}

#[test]
fn one_coin_exit_at_claim_capacity_keeps_exact_recoverable_payout() {
    let h = setup();
    set_test_cap(&h, 0);
    h.pic.update_call(
        h.ledgers[0],
        h.user,
        "set_bad_fee_failures",
        encode_one(1u32).unwrap(),
    ).unwrap();
    call_ledger_nat(&h, h.ledgers[0], "set_fee", 100);
    let events_before = liquidity_event_count_v2(&h);
    let lp_before = lp_balance(&h);

    let result: Result<Nat, ThreePoolError> = decode_one(&reply(
        h.pic.update_call(
            h.pool,
            h.user,
            "remove_one_coin",
            encode_args((100_000_000u128, 0u8, 0u128)).unwrap(),
        ).unwrap(),
    )).unwrap();
    let failure_reason = match result {
        Err(ThreePoolError::TransferFailed { reason, .. }) => reason,
        other => panic!("unexpected one-coin exit result: {other:?}"),
    };
    assert!(claims(&h).is_empty());
    let entitlement = payouts(&h, h.user).into_iter()
        .find(|item| item.kind == PayoutKind::RemoveOneCoin && item.token_index == 0)
        .expect("one-coin payout remains discoverable at claim capacity");
    assert!(!entitlement.settled, "failed transfer must stay unsettled until recovery");
    assert_eq!(liquidity_event_count_v2(&h), events_before + 1);
    assert!(failure_reason.contains(&format!("claim_pending({})", entitlement.id)));
    assert!(failure_reason.contains(&format!("get_payout_entitlement({})", entitlement.id)));
    assert!(failure_reason.contains("get_payout_entitlements"));
    let before_retry = balance(&h, h.ledgers[0], h.user);

    let recovered: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic.update_call(
            h.pool,
            h.user,
            "claim_pending",
            encode_one(entitlement.id).unwrap(),
        ).unwrap(),
    )).unwrap();
    recovered.expect("exact payout entitlement should recover without a claim-index slot");
    let updated = payout(&h, h.user, entitlement.id);
    assert!(updated.settled);
    assert_eq!(balance(&h, h.ledgers[0], h.user) - before_retry, entitlement.gross - 100);
    assert_eq!(lp_balance(&h), lp_before - 100_000_000, "retry must not burn LP again");
    assert_eq!(liquidity_event_count_v2(&h), events_before + 1, "retry must not duplicate the exit event");
}

#[test]
fn swap_compensation_identity_survives_upgrade_and_repeated_parent_recovery() {
    let h = setup();
    let input_before = balance(&h, h.ledgers[0], h.user);
    h.pic
        .update_call(
            h.ledgers[1],
            h.user,
            "set_bad_fee_failures",
            encode_one(1u32).unwrap(),
        )
        .unwrap();
    h.pic
        .update_call(
            h.ledgers[0],
            h.user,
            "set_phantom_icrc1_failures",
            encode_one(1u32).unwrap(),
        )
        .unwrap();

    let result: Result<u128, ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(
                h.pool,
                h.user,
                "swap",
                encode_args((0u8, 1u8, 100_000_000u128, 1u128)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    assert!(matches!(result, Err(ThreePoolError::TransferFailed { .. })));

    let before_upgrade = payouts(&h, h.user);
    let output = before_upgrade
        .iter()
        .find(|p| p.kind == PayoutKind::SwapOutput)
        .unwrap();
    let compensation_id = output.compensation_id.expect("refund link must be persisted");
    let compensation = payout(&h, h.user, compensation_id);
    assert_eq!(compensation.compensation_for, Some(output.id));
    assert!(matches!(
        &compensation.attempts[0].outcome,
        PayoutOutcome::Unresolved { .. }
    ));
    let after_lost_refund_reply = balance(&h, h.ledgers[0], h.user);
    assert_eq!(input_before - after_lost_refund_reply, 0);

    h.pic
        .upgrade_canister(
            h.pool,
            wasm_artifact("rumi_3pool.wasm"),
            encode_args(()).unwrap(),
            None,
        )
        .unwrap();
    let recovery: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(
                h.pool,
                h.user,
                "claim_pending",
                encode_one(compensation_id).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    recovery.expect("exact refund replay should find the original committed transfer");
    let after_replay = balance(&h, h.ledgers[0], h.user);
    assert_eq!(after_replay, after_lost_refund_reply);
    let closed_output = payout(&h, h.user, output.id);
    assert!(closed_output.settled);
    assert_eq!(closed_output.compensation_id, Some(compensation_id));

    let repeated: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(
                h.pool,
                h.user,
                "claim_pending",
                encode_one(output.id).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    repeated.expect("repeated parent recovery must resolve the same compensation");
    assert_eq!(balance(&h, h.ledgers[0], h.user), after_replay);
}

#[test]
fn settled_swap_claim_cannot_clear_another_swaps_payout_fence() {
    let h = setup();
    let run_swap = || {
        decode_one::<Result<u128, ThreePoolError>>(&reply(
            h.pic
                .update_call(
                    h.pool,
                    h.user,
                    "swap",
                    encode_args((0u8, 1u8, 100_000_000u128, 1u128)).unwrap(),
                )
                .unwrap(),
        ))
        .unwrap()
    };

    run_swap().expect("first swap succeeds");
    let old = payouts(&h, h.user)
        .into_iter()
        .find(|p| p.kind == PayoutKind::SwapOutput && p.settled)
        .expect("first swap payout is settled");

    h.pic
        .update_call(
            h.ledgers[1],
            h.user,
            "set_phantom_failures",
            encode_one(1u32).unwrap(),
        )
        .unwrap();
    assert!(matches!(run_swap(), Err(ThreePoolError::TransferFailed { .. })));
    let current = payouts(&h, h.user)
        .into_iter()
        .find(|p| p.kind == PayoutKind::SwapOutput && !p.settled)
        .expect("ambiguous second output remains unresolved");

    let stale_recovery: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(h.pool, h.user, "claim_pending", encode_one(old.id).unwrap())
            .unwrap(),
    ))
    .unwrap();
    stale_recovery.expect("old confirmed payout may be read idempotently");

    assert!(matches!(run_swap(), Err(ThreePoolError::PoolLocked)));
    assert!(matches!(
        &payout(&h, h.user, current.id).attempts.last().unwrap().outcome,
        PayoutOutcome::Unresolved { .. }
    ));
}

#[test]
fn global_payout_inventory_is_admin_only_and_paginates_gappy_ids() {
    let h = setup_with_pool_wasm_and_seed(
        wasm_artifact("rumi_3pool_test_endpoints.wasm"),
        false,
    );
    let other_owner = Principal::self_authenticating(&[41, 42, 43]);
    for (id, owner, settled) in [
        (2u64, h.user, false),
        (5, other_owner, false),
        (9, h.user, true),
    ]
    .into_iter()
    .chain((10u64..=29).map(|id| (id, h.user, false)))
    {
        h.pic
            .update_call(
                h.pool,
                h.admin,
                "test_insert_payout_entitlement",
                encode_args((id, owner, settled)).unwrap(),
            )
            .unwrap();
    }

    let user_view = payouts(&h, h.user);
    assert!(user_view.iter().all(|entry| entry.owner == h.user));
    assert!(user_view.iter().any(|entry| entry.id == 2));
    assert!(user_view.iter().any(|entry| entry.id == 9));
    let other_view = payouts(&h, other_owner);
    assert_eq!(other_view.iter().map(|entry| entry.id).collect::<Vec<_>>(), [5]);

    let first = global_payout_inventory(&h, h.admin, None, 2).unwrap();
    assert_eq!(first.total_count, 23);
    assert_eq!(first.entries.iter().map(|entry| entry.id).collect::<Vec<_>>(), [2, 5]);
    assert_eq!(first.next_cursor, Some(5));

    let second = global_payout_inventory(&h, h.admin, first.next_cursor, 2).unwrap();
    assert_eq!(second.total_count, 23);
    assert_eq!(second.entries.iter().map(|entry| entry.id).collect::<Vec<_>>(), [9, 10]);
    assert!(second.entries[0].settled, "settled retained row remains in the inventory");
    assert_eq!(second.next_cursor, Some(10));

    let capped = global_payout_inventory(&h, h.admin, None, u64::MAX).unwrap();
    assert_eq!(capped.total_count, 23);
    assert_eq!(capped.entries.len(), 20, "requested limit is capped at 20");
    assert_eq!(capped.entries.last().map(|entry| entry.id), Some(26));
    assert_eq!(capped.next_cursor, Some(26));

    let exact_boundary = global_payout_inventory(&h, h.admin, Some(26), 3).unwrap();
    assert_eq!(exact_boundary.entries.iter().map(|entry| entry.id).collect::<Vec<_>>(), [27, 28, 29]);
    assert_eq!(exact_boundary.next_cursor, None);

    let unauthorized = global_payout_inventory(&h, h.user, None, 2);
    assert!(matches!(unauthorized, Err(ThreePoolError::Unauthorized)));
    let anonymous = global_payout_inventory(&h, Principal::anonymous(), None, 2);
    assert!(matches!(anonymous, Err(ThreePoolError::Unauthorized)));
}
