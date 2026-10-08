//! Pending-claim capacity must be reserved before value can move. The flaky
//! ledgers reproduce two independent add-liquidity refunds failing after two
//! successful pulls without requiring an outage or external ledger.
use candid::{decode_one, encode_args, encode_one, Nat, Principal};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::approve::ApproveArgs;
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
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
