//! Pending-claim capacity must be reserved before value can move. The flaky
//! ledgers reproduce two independent add-liquidity refunds failing after two
//! successful pulls without requiring an outage or external ledger.
use candid::{decode_args, decode_one, encode_args, encode_one, Nat, Principal};
use icrc_ledger_types::icrc::generic_value::ICRC3Value;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc3::blocks::{GetBlocksRequest, GetBlocksResult};
use icrc_ledger_types::icrc2::approve::ApproveArgs;
use num_traits::ToPrimitive;
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use rumi_3pool::payouts::{PayoutEntitlement, PayoutInputAction, PayoutKind, PayoutOutcome};
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
    assert!(matches!(result, Err(ThreePoolError::PendingClaimCapacityReached)));
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
