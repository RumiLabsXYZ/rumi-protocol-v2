// Regression tests for the 2026-06-09 audit fixes in rumi_3pool.
//
//   IC-S-003: transfer_to_user silently skips sends of amount <= ledger fee.
//     swap / remove_liquidity / remove_one_coin must reject up front when the
//     payable output nets to zero, and claim_pending must reject dust claims
//     with a clear error instead of consuming them.
//   ICRC-001: the 3USD token must implement ICRC-1 standard dedup for
//     created_at_time (TooOld / CreatedInFuture / Duplicate).
//
// Requires the rumi_3pool wasm built with `--features test_endpoints`
// (test_insert_pending_claim), same as icrc3_hash_cache.rs.

mod common;

use candid::{decode_one, encode_args, encode_one, Nat, Principal};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
use icrc_ledger_types::icrc2::approve::{ApproveArgs, ApproveError};
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
use pocket_ic::WasmResult;
use rumi_3pool::icrc_token::PERMITTED_DRIFT_NS;
use rumi_3pool::types::*;
use sha2::{Digest, Sha256};

use common::{
    deploy_pool_with_liquidity_and_swaps, deploy_pool_with_liquidity_fee_and_swaps,
    ThreePoolHarness,
};

const LEDGER_FEE: u128 = 10_000;

// ─── Call helpers ───

fn reply_bytes(res: WasmResult) -> Vec<u8> {
    match res {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(msg) => panic!("call rejected: {msg}"),
    }
}

fn ledger_balance(h: &ThreePoolHarness, ledger: Principal, owner: Principal) -> u128 {
    let account = Account { owner, subaccount: None };
    let res = h
        .pic
        .query_call(ledger, Principal::anonymous(), "icrc1_balance_of", encode_one(account).unwrap())
        .expect("icrc1_balance_of failed");
    let n: Nat = decode_one(&reply_bytes(res)).unwrap();
    n.0.try_into().unwrap()
}

fn lp_balance(h: &ThreePoolHarness, owner: Principal) -> u128 {
    ledger_balance(h, h.three_pool, owner)
}

fn pool_balances(h: &ThreePoolHarness) -> [u128; 3] {
    let res = h
        .pic
        .query_call(h.three_pool, Principal::anonymous(), "get_pool_status", encode_args(()).unwrap())
        .expect("get_pool_status failed");
    let status: PoolStatus = decode_one(&reply_bytes(res)).unwrap();
    status.balances
}

fn pool_lp_supply(h: &ThreePoolHarness) -> u128 {
    let res = h
        .pic
        .query_call(h.three_pool, Principal::anonymous(), "get_pool_status", encode_args(()).unwrap())
        .expect("get_pool_status failed");
    let status: PoolStatus = decode_one(&reply_bytes(res)).unwrap();
    status.lp_total_supply
}

fn pool_ledger_balances(h: &ThreePoolHarness) -> [u128; 3] {
    [
        ledger_balance(h, h.ledgers[0], h.three_pool),
        ledger_balance(h, h.ledgers[1], h.three_pool),
        ledger_balance(h, h.ledgers[2], h.three_pool),
    ]
}

fn swap(h: &ThreePoolHarness, i: u8, j: u8, dx: u128, min_dy: u128) -> Result<u128, ThreePoolError> {
    let res = h
        .pic
        .update_call(h.three_pool, h.user, "swap", encode_args((i, j, dx, min_dy)).unwrap())
        .expect("swap call failed");
    let r: Result<Nat, ThreePoolError> = decode_one(&reply_bytes(res)).unwrap();
    r.map(|n| n.0.try_into().unwrap())
}

fn remove_liquidity(
    h: &ThreePoolHarness,
    lp_burn: u128,
    min_amounts: Vec<u128>,
) -> Result<Vec<u128>, ThreePoolError> {
    let res = h
        .pic
        .update_call(
            h.three_pool,
            h.user,
            "remove_liquidity",
            encode_args((lp_burn, min_amounts)).unwrap(),
        )
        .expect("remove_liquidity call failed");
    let r: Result<Vec<Nat>, ThreePoolError> = decode_one(&reply_bytes(res)).unwrap();
    r.map(|v| v.into_iter().map(|n| n.0.try_into().unwrap()).collect())
}

fn remove_one_coin(
    h: &ThreePoolHarness,
    lp_burn: u128,
    coin_index: u8,
    min_amount: u128,
) -> Result<u128, ThreePoolError> {
    let res = h
        .pic
        .update_call(
            h.three_pool,
            h.user,
            "remove_one_coin",
            encode_args((lp_burn, coin_index, min_amount)).unwrap(),
        )
        .expect("remove_one_coin call failed");
    let r: Result<Nat, ThreePoolError> = decode_one(&reply_bytes(res)).unwrap();
    r.map(|n| n.0.try_into().unwrap())
}

fn lp_transfer(
    h: &ThreePoolHarness,
    to: Principal,
    amount: u128,
    memo: Option<Vec<u8>>,
    created_at_time: Option<u64>,
) -> Result<Nat, TransferError> {
    let args = TransferArg {
        from_subaccount: None,
        to: Account { owner: to, subaccount: None },
        fee: None,
        created_at_time,
        memo: memo.map(|m| icrc_ledger_types::icrc1::transfer::Memo(serde_bytes::ByteBuf::from(m))),
        amount: Nat::from(amount),
    };
    let res = h
        .pic
        .update_call(h.three_pool, h.user, "icrc1_transfer", encode_one(args).unwrap())
        .expect("icrc1_transfer call failed");
    decode_one(&reply_bytes(res)).unwrap()
}

fn pic_now_ns(h: &ThreePoolHarness) -> u64 {
    h.pic
        .get_time()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

// ─── IC-S-003 ───

#[test]
fn ic_s_003_swap_dust_output_rejected_before_input_pull() {
    let h = deploy_pool_with_liquidity_fee_and_swaps(0, LEDGER_FEE);

    // 0.005 icUSD (8 dec) -> ~5_000 e6s ckUSDT (6 dec), below the 10_000 fee:
    // the net output is zero, so the swap must be rejected with a typed error
    // BEFORE the input is pulled (min_dy = 0 must not bypass the gate).
    let user_icusd_before = ledger_balance(&h, h.ledgers[0], h.user);
    let user_ckusdt_before = ledger_balance(&h, h.ledgers[1], h.user);
    let pool_before = pool_balances(&h);

    let result = swap(&h, 0, 1, 500_000, 0);
    assert!(
        matches!(result, Err(ThreePoolError::InsufficientOutput { .. })),
        "dust-output swap must fail with InsufficientOutput, got {result:?}"
    );

    assert_eq!(
        ledger_balance(&h, h.ledgers[0], h.user),
        user_icusd_before,
        "input must not be pulled for a rejected dust swap"
    );
    assert_eq!(ledger_balance(&h, h.ledgers[1], h.user), user_ckusdt_before);
    assert_eq!(pool_balances(&h), pool_before, "pool balances must be untouched");

    // Sanity: a non-dust swap still works.
    let ok = swap(&h, 0, 1, 100_000_000_000, 0);
    assert!(ok.is_ok(), "normal swap must still succeed, got {ok:?}");
}

#[test]
fn ic_s_003_remove_one_coin_dust_rejected_before_lp_burn() {
    let h = deploy_pool_with_liquidity_fee_and_swaps(0, LEDGER_FEE);

    let lp_before = lp_balance(&h, h.user);
    let pool_before = pool_balances(&h);

    // lp_burn worth ~0.001 USD -> ~1_000 e6s of ckUSDT, below the 10_000 fee.
    let result = remove_one_coin(&h, 100_000, 1, 0);
    assert!(
        matches!(result, Err(ThreePoolError::InsufficientOutput { .. })),
        "dust remove_one_coin must fail with InsufficientOutput, got {result:?}"
    );

    assert_eq!(lp_balance(&h, h.user), lp_before, "LP must not be burned");
    assert_eq!(pool_balances(&h), pool_before);
}

#[test]
fn ic_s_003_remove_liquidity_dust_leg_rejected_before_lp_burn() {
    let h = deploy_pool_with_liquidity_fee_and_swaps(0, LEDGER_FEE);

    let lp_before = lp_balance(&h, h.user);
    let pool_before = pool_balances(&h);

    // lp_burn = 100_000 e8s LP gives ~33_333 e8s icUSD (above its 10_000 fee)
    // but only ~333 e6s on each 6-decimal leg, below their 10_000 fee. Any
    // payable leg netting to zero must reject the whole removal up front.
    let result = remove_liquidity(&h, 100_000, vec![0, 0, 0]);
    assert!(
        matches!(result, Err(ThreePoolError::InsufficientOutput { .. })),
        "removal with a dust leg must fail with InsufficientOutput, got {result:?}"
    );

    assert_eq!(lp_balance(&h, h.user), lp_before, "LP must not be burned");
    assert_eq!(pool_balances(&h), pool_before);
}

/// CL-01 proportional runtime regression: the pre-fix crossover showed this
/// submitted withdrawal can execute across the concurrent same-user LP transfer.
#[test]
fn cl_01_proportional_remove_rechecks_lp_after_fee_await() {
    let h = deploy_pool_with_liquidity_fee_and_swaps(0, 0);
    let before = lp_balance(&h, h.user);
    let burn = before / 4;
    assert!(burn > 0);
    let supply_before = pool_lp_supply(&h);
    let balances_before = pool_balances(&h);
    let ledger_balances_before = pool_ledger_balances(&h);
    let recipient = Principal::self_authenticating(&[31, 41, 59]);
    let to_transfer = before - (burn - 1);

    let withdrawal = h.pic.submit_call(
        h.three_pool,
        h.user,
        "remove_liquidity",
        encode_args((burn, vec![0u128; 3])).unwrap(),
    ).expect("submit remove_liquidity failed");
    lp_transfer(&h, recipient, to_transfer, None, None)
        .expect("same-user LP transfer failed");
    let response = h.pic.await_call(withdrawal).expect("await remove_liquidity failed");
    let result: Result<Vec<Nat>, ThreePoolError> = decode_one(&reply_bytes(response)).unwrap();
    assert!(matches!(result, Err(ThreePoolError::InsufficientLiquidity)),
        "withdrawal must reject after LP moved during fee lookup: {result:?}");
    assert_eq!(lp_balance(&h, h.user), burn - 1, "only concurrent transfer may debit user LP");
    assert_eq!(pool_lp_supply(&h), supply_before, "failed withdrawal must not burn supply");
    assert_eq!(pool_balances(&h), balances_before, "failed withdrawal must not debit pool balances");
    assert_eq!(pool_ledger_balances(&h), ledger_balances_before, "failed withdrawal must not transfer pool tokens");
}

#[test]
fn legacy_unbound_claim_is_visible_and_held_at_claim_time() {
    let h = deploy_pool_with_liquidity_fee_and_swaps(0, LEDGER_FEE);

    // Inject an old-format claim without the original transfer identity.
    let res = h
        .pic
        .update_call(
            h.three_pool,
            h.user,
            "test_insert_pending_claim",
            encode_args((1u8, 5_000u128)).unwrap(),
        )
        .expect("test_insert_pending_claim failed");
    let claim_id: u64 = decode_one(&reply_bytes(res)).unwrap();

    let user_ckusdt_before = ledger_balance(&h, h.ledgers[1], h.user);

    let res = h
        .pic
        .update_call(h.three_pool, h.user, "claim_pending", encode_one(claim_id).unwrap())
        .expect("claim_pending call failed");
    let r: Result<(), ThreePoolError> = decode_one(&reply_bytes(res)).unwrap();
    match r {
        Err(ThreePoolError::TransferFailed { reason, .. })
            if reason.contains("legacy claim has no bound transfer identity") => {}
        other => panic!("legacy claim must remain held without transfer identity, got {other:?}"),
    }

    // Nothing was sent and the claim is NOT consumed.
    assert_eq!(ledger_balance(&h, h.ledgers[1], h.user), user_ckusdt_before);
    let res = h
        .pic
        .query_call(
            h.three_pool,
            Principal::anonymous(),
            "get_pending_claims",
            encode_args((0u64, 100u64)).unwrap(),
        )
        .expect("get_pending_claims failed");
    let claims: Vec<ThreePoolPendingClaim> = decode_one(&reply_bytes(res)).unwrap();
    assert!(
        claims.iter().any(|c| c.id == claim_id),
        "dust claim must remain pending for recovery if the fee ever drops"
    );
}

// ─── ICRC-001 ───

#[test]
fn icrc_001_e2e_dedup_on_3usd_token() {
    let h = deploy_pool_with_liquidity_and_swaps(0);
    let recipient = Principal::self_authenticating(&[7, 7, 7]);
    let now = pic_now_ns(&h);

    // First send with created_at_time succeeds.
    let block = lp_transfer(&h, recipient, 1_000, Some(vec![1]), Some(now))
        .expect("first transfer must succeed");

    // Identical resubmission within the window is a Duplicate of that block.
    match lp_transfer(&h, recipient, 1_000, Some(vec![1]), Some(now)) {
        Err(TransferError::Duplicate { duplicate_of }) => {
            assert_eq!(
                duplicate_of, block,
                "duplicate_of must be the original block index"
            );
        }
        other => panic!("identical resubmission must be Duplicate, got {other:?}"),
    }

    // A different memo is a different transaction.
    lp_transfer(&h, recipient, 1_000, Some(vec![2]), Some(now))
        .expect("distinct memo must not be deduplicated");

    // created_at_time older than the 24h window is TooOld.
    let too_old = now.saturating_sub(25 * 60 * 60 * 1_000_000_000);
    assert!(
        matches!(
            lp_transfer(&h, recipient, 1_000, Some(vec![3]), Some(too_old)),
            Err(TransferError::TooOld)
        ),
        "transfer older than the window must be TooOld"
    );

    // created_at_time beyond the permitted drift is CreatedInFuture.
    let future = now + 5 * 60 * 1_000_000_000;
    assert!(
        matches!(
            lp_transfer(&h, recipient, 1_000, Some(vec![4]), Some(future)),
            Err(TransferError::CreatedInFuture { .. })
        ),
        "transfer from the future must be CreatedInFuture"
    );

    // None created_at_time keeps the legacy behavior: identical sends all pass.
    lp_transfer(&h, recipient, 1_000, Some(vec![5]), None).expect("first None-cat transfer");
    lp_transfer(&h, recipient, 1_000, Some(vec![5]), None).expect("second None-cat transfer");
}

#[test]
fn icrc_001_dedup_survives_upgrade_for_icrc1_and_icrc2() {
    let h = deploy_pool_with_liquidity_and_swaps(0);
    let spender = Principal::self_authenticating(&[8, 8, 8]);
    let recipient = Principal::self_authenticating(&[7, 7, 7]);
    let now = pic_now_ns(&h);

    let approve_args = ApproveArgs {
        from_subaccount: None,
        spender: Account {
            owner: spender,
            subaccount: None,
        },
        amount: Nat::from(20_000u64),
        expected_allowance: None,
        expires_at: None,
        fee: None,
        memo: Some(icrc_ledger_types::icrc1::transfer::Memo(
            serde_bytes::ByteBuf::from(vec![10]),
        )),
        created_at_time: Some(now),
    };
    let approve = h
        .pic
        .update_call(
            h.three_pool,
            h.user,
            "icrc2_approve",
            encode_one(approve_args.clone()).unwrap(),
        )
        .expect("icrc2_approve call failed");
    let approve_block: Result<Nat, ApproveError> = decode_one(&reply_bytes(approve)).unwrap();
    let approve_block = approve_block.expect("first approve must succeed");

    let transfer_from_args = TransferFromArgs {
        spender_subaccount: None,
        from: Account {
            owner: h.user,
            subaccount: None,
        },
        to: Account {
            owner: recipient,
            subaccount: None,
        },
        amount: Nat::from(1_000u64),
        fee: None,
        memo: Some(icrc_ledger_types::icrc1::transfer::Memo(
            serde_bytes::ByteBuf::from(vec![11]),
        )),
        created_at_time: Some(now + 1),
    };
    let transfer_from = h
        .pic
        .update_call(
            h.three_pool,
            spender,
            "icrc2_transfer_from",
            encode_one(transfer_from_args.clone()).unwrap(),
        )
        .expect("icrc2_transfer_from call failed");
    let transfer_from_block: Result<Nat, TransferFromError> =
        decode_one(&reply_bytes(transfer_from)).unwrap();
    let transfer_from_block = transfer_from_block.expect("first transfer_from must succeed");

    let icrc1_args = TransferArg {
        from_subaccount: None,
        to: Account {
            owner: recipient,
            subaccount: None,
        },
        amount: Nat::from(1_000u64),
        fee: None,
        memo: Some(icrc_ledger_types::icrc1::transfer::Memo(
            serde_bytes::ByteBuf::from(vec![12]),
        )),
        created_at_time: Some(now + 2),
    };
    let icrc1 = h
        .pic
        .update_call(
            h.three_pool,
            h.user,
            "icrc1_transfer",
            encode_one(icrc1_args.clone()).unwrap(),
        )
        .expect("icrc1_transfer call failed");
    let icrc1_block: Result<Nat, TransferError> = decode_one(&reply_bytes(icrc1)).unwrap();
    let icrc1_block = icrc1_block.expect("first icrc1 transfer must succeed");

    h.pic
        .upgrade_canister(
            h.three_pool,
            common::three_pool_wasm(),
            encode_args(()).unwrap(),
            None,
        )
        .expect("3pool upgrade failed");

    let approve = h
        .pic
        .update_call(
            h.three_pool,
            h.user,
            "icrc2_approve",
            encode_one(approve_args).unwrap(),
        )
        .expect("icrc2_approve retry call failed");
    let approve_retry: Result<Nat, ApproveError> = decode_one(&reply_bytes(approve)).unwrap();
    assert_eq!(
        approve_retry,
        Err(ApproveError::Duplicate {
            duplicate_of: approve_block
        }),
        "approve duplicate must preserve its original block across upgrade",
    );

    let transfer_from = h
        .pic
        .update_call(
            h.three_pool,
            spender,
            "icrc2_transfer_from",
            encode_one(transfer_from_args).unwrap(),
        )
        .expect("icrc2_transfer_from retry call failed");
    let transfer_from_retry: Result<Nat, TransferFromError> =
        decode_one(&reply_bytes(transfer_from)).unwrap();
    assert_eq!(
        transfer_from_retry,
        Err(TransferFromError::Duplicate {
            duplicate_of: transfer_from_block
        }),
        "transfer_from duplicate must preserve its original block across upgrade",
    );

    let icrc1 = h
        .pic
        .update_call(
            h.three_pool,
            h.user,
            "icrc1_transfer",
            encode_one(icrc1_args).unwrap(),
        )
        .expect("icrc1_transfer retry call failed");
    let icrc1_retry: Result<Nat, TransferError> = decode_one(&reply_bytes(icrc1)).unwrap();
    assert_eq!(
        icrc1_retry,
        Err(TransferError::Duplicate {
            duplicate_of: icrc1_block
        }),
        "icrc1 duplicate must preserve its original block across upgrade",
    );
}

#[test]
#[ignore = "requires RUMI_P08_07_LEGACY_WASM_PATH pointing to the verified f33 Wasm"]
fn icrc_001_legacy_heap_dedup_is_fenced_on_first_upgrade() {
    const LEGACY_WASM_SHA256: &str =
        "f02b8a7ff26b004c2cd4aa9772bbd5077f851bccc23efe7ef4f8e320e8f45d31";
    let legacy_path = std::env::var("RUMI_P08_07_LEGACY_WASM_PATH")
        .expect("set RUMI_P08_07_LEGACY_WASM_PATH to the verified f33 3pool Wasm");
    let legacy_wasm = std::fs::read(legacy_path).expect("read verified f33 3pool Wasm");
    let actual_sha256 = format!("{:x}", Sha256::digest(&legacy_wasm));
    assert_eq!(actual_sha256, LEGACY_WASM_SHA256, "unexpected legacy Wasm");

    let h = common::deploy_pool_with_liquidity_fee_and_swaps_with_wasm(0, 0, legacy_wasm);
    let recipient = Principal::self_authenticating(&[7, 7, 7]);
    let old_cat = pic_now_ns(&h);
    let old_args = TransferArg {
        from_subaccount: None,
        to: Account {
            owner: recipient,
            subaccount: None,
        },
        amount: Nat::from(1_000u64),
        fee: None,
        memo: Some(icrc_ledger_types::icrc1::transfer::Memo(
            serde_bytes::ByteBuf::from(vec![41]),
        )),
        created_at_time: Some(old_cat),
    };
    let first = h
        .pic
        .update_call(
            h.three_pool,
            h.user,
            "icrc1_transfer",
            encode_one(old_args.clone()).unwrap(),
        )
        .expect("legacy transfer call failed");
    let old_block: Result<Nat, TransferError> = decode_one(&reply_bytes(first)).unwrap();
    old_block.expect("legacy timestamped transfer should succeed before upgrade");

    let pre_upgrade_time = pic_now_ns(&h);
    h.pic.set_time(
        std::time::UNIX_EPOCH + std::time::Duration::from_nanos(pre_upgrade_time + 1),
    );
    h.pic
        .upgrade_canister(
            h.three_pool,
            common::three_pool_wasm(),
            encode_args(()).unwrap(),
            None,
        )
        .expect("first upgrade from heap-only dedup implementation failed");

    let post_upgrade_time = pic_now_ns(&h);
    let legacy_cat_cutoff = post_upgrade_time + PERMITTED_DRIFT_NS;
    let balance_before_replay = lp_balance(&h, h.user);
    let blocks_before_replay = h.icrc3_log_length();
    let replay = h
        .pic
        .update_call(
            h.three_pool,
            h.user,
            "icrc1_transfer",
            encode_one(old_args).unwrap(),
        )
        .expect("legacy replay call failed");
    let replay: Result<Nat, TransferError> = decode_one(&reply_bytes(replay)).unwrap();
    assert_eq!(replay, Err(TransferError::TemporarilyUnavailable));
    assert_eq!(lp_balance(&h, h.user), balance_before_replay);
    assert_eq!(h.icrc3_log_length(), blocks_before_replay);

    // A request above the one-time legacy CAT cutoff is valid after the
    // canister clock advances by 2ns, and is durably deduplicated thereafter.
    h.pic.set_time(
        std::time::UNIX_EPOCH + std::time::Duration::from_nanos(post_upgrade_time + 2),
    );
    let new_cat = legacy_cat_cutoff + 1;
    let new_args = TransferArg {
        from_subaccount: None,
        to: Account {
            owner: recipient,
            subaccount: None,
        },
        amount: Nat::from(2_000u64),
        fee: None,
        memo: Some(icrc_ledger_types::icrc1::transfer::Memo(
            serde_bytes::ByteBuf::from(vec![42]),
        )),
        created_at_time: Some(new_cat),
    };
    let new_transfer = h
        .pic
        .update_call(
            h.three_pool,
            h.user,
            "icrc1_transfer",
            encode_one(new_args.clone()).unwrap(),
        )
        .expect("new CAT transfer call failed");
    let new_block: Result<Nat, TransferError> = decode_one(&reply_bytes(new_transfer)).unwrap();
    let new_block = new_block.expect("new CAT above the legacy cutoff should succeed");

    h.pic
        .upgrade_canister(
            h.three_pool,
            common::three_pool_wasm(),
            encode_args(()).unwrap(),
            None,
        )
        .expect("second upgrade failed");
    let new_retry = h
        .pic
        .update_call(
            h.three_pool,
            h.user,
            "icrc1_transfer",
            encode_one(new_args).unwrap(),
        )
        .expect("new transfer retry call failed");
    let new_retry: Result<Nat, TransferError> = decode_one(&reply_bytes(new_retry)).unwrap();
    assert_eq!(
        new_retry,
        Err(TransferError::Duplicate {
            duplicate_of: new_block
        })
    );

    let legacy_retry = h
        .pic
        .update_call(
            h.three_pool,
            h.user,
            "icrc1_transfer",
            encode_one(TransferArg {
                created_at_time: Some(old_cat),
                memo: Some(icrc_ledger_types::icrc1::transfer::Memo(
                    serde_bytes::ByteBuf::from(vec![41]),
                )),
                from_subaccount: None,
                to: Account {
                    owner: recipient,
                    subaccount: None,
                },
                amount: Nat::from(1_000u64),
                fee: None,
            })
            .unwrap(),
        )
        .expect("legacy retry call after second upgrade failed");
    let legacy_retry: Result<Nat, TransferError> =
        decode_one(&reply_bytes(legacy_retry)).unwrap();
    assert_eq!(legacy_retry, Err(TransferError::TemporarilyUnavailable));
}
