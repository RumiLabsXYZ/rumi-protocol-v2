//! Pending-claim capacity must be reserved before value can move. The flaky
//! ledgers reproduce two independent add-liquidity refunds failing after two
//! successful pulls without requiring an outage or external ledger.
use candid::{decode_one, encode_args, encode_one, Nat, Principal};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::approve::ApproveArgs;
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use rumi_3pool::payouts::{PayoutEntitlement, PayoutOutcome};
use rumi_3pool::types::{ThreePoolError, ThreePoolInitArgs, ThreePoolPendingClaim, TokenConfig};

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
        wasm_artifact("rumi_3pool.wasm"),
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
    decode_one(&reply(
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
    set_test_cap(&h, 2);
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
    assert!(matches!(result, Err(ThreePoolError::TransferFailed { .. })));

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
    assert!(matches!(
        result,
        Err(ThreePoolError::PendingClaimCapacityReached)
    ));
    assert_eq!(h.ledgers.map(|ledger| balance(&h, ledger, h.user)), before);
    let existing = claims(&h);
    assert_eq!(existing.len(), 1);
    assert_eq!(existing[0].id, first_id);
    assert_eq!(existing[0].amount, 777);
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
    assert!(matches!(result, Err(ThreePoolError::LegacyClaimHeld)));
    assert_eq!(balance(&h, h.ledgers[1], h.user), before);
    assert!(claims(&h).iter().any(|claim| claim.id == id));
}

#[test]
fn proven_bad_fee_starts_new_exact_attempt_with_refreshed_fee() {
    let h = setup();
    h.pic
        .update_call(
            h.ledgers[0],
            h.user,
            "set_bad_fee_failures",
            encode_one(1u32).unwrap(),
        )
        .unwrap();
    call_ledger_nat(&h, h.ledgers[0], "set_fee", 100);

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
    assert!(matches!(result, Err(ThreePoolError::TransferFailed { .. })));
    let claim = claims(&h).into_iter().find(|claim| claim.token_index == 0).unwrap();
    let before_retry = balance(&h, h.ledgers[0], h.user);
    call_ledger_nat(&h, h.ledgers[0], "set_fee", 250);
    let retried: Result<(), ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(h.pool, h.user, "claim_pending", encode_one(claim.id).unwrap())
            .unwrap(),
    ))
    .unwrap();
    retried.expect("typed BadFee proves no transfer and allows a new attempt");
    let updated = payout(&h, h.user, claim.id);
    assert_eq!(updated.attempts.len(), 2);
    assert_eq!(updated.attempts[1].transfer.fee, 250);
    assert_eq!(updated.attempts[1].transfer.net + 250, claim.amount);
    assert_eq!(balance(&h, h.ledgers[0], h.user) - before_retry, claim.amount - 250);
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
    assert_eq!(input_before - after_lost_refund_reply, 20_000);

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
