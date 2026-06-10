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
use pocket_ic::WasmResult;
use rumi_3pool::types::*;

use common::{deploy_pool_with_liquidity_and_swaps, deploy_pool_with_liquidity_fee_and_swaps,
             ThreePoolHarness};

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

#[test]
fn ic_s_003_claim_of_dust_rejected_at_claim_time() {
    let h = deploy_pool_with_liquidity_fee_and_swaps(0, LEDGER_FEE);

    // Inject a pending claim below the ledger fee (test_endpoints build).
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
        Err(ThreePoolError::TransferFailed { reason, .. }) => {
            assert!(
                reason.contains("ledger fee"),
                "error must explain the dust rejection, got: {reason}"
            );
        }
        other => panic!("dust claim must fail with a clear TransferFailed, got {other:?}"),
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
            assert_eq!(duplicate_of, block, "duplicate_of must be the original block index");
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
