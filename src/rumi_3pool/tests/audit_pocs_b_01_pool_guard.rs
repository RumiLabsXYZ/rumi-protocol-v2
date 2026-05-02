//! B-01 regression fence: rumi_3pool must serialize concurrent operations on the
//! single pool so that two callers cannot price against the same pre-state.
//!
//! Without the fix, two `swap` messages submitted in parallel can both read
//! `s.balances` before either updates state, both compute the same output, and
//! both transfer that output, leaking LP value. The PoolGuard pattern (ported
//! from rumi_amm) gates entry to mutating async paths so the second concurrent
//! caller gets a clear `PoolLocked` error and can retry rather than racing.
//!
//! The audit detail and threat model live in
//! `.claude/security-docs/2026-05-02-wave-14-avai-parity-plan.md` (gitignored).

mod common;

use candid::{decode_one, encode_args, encode_one, Principal};
use common::deploy_pool_with_liquidity_and_swaps;
use pocket_ic::WasmResult;
use rumi_3pool::types::ThreePoolError;

/// Decode a `swap` reply from PocketIC into the inner Result.
fn decode_swap_reply(result: WasmResult) -> Result<u128, ThreePoolError> {
    let WasmResult::Reply(bytes) = result else {
        panic!("swap call rejected: {:?}", result);
    };
    decode_one(&bytes).expect("swap reply decode failed")
}

/// B-01 core fence: two concurrent swaps from the same user must not both
/// price against the pre-state. With the PoolGuard, exactly one succeeds and
/// the other returns `PoolLocked`. Without the guard, both would succeed
/// (the bug we are fixing).
#[test]
fn b_01_concurrent_swaps_serialize() {
    let h = deploy_pool_with_liquidity_and_swaps(0);

    // Two non-trivial swaps. Same caller is fine; the lock is per-canister.
    // Use a small dx so that even sequentially both swaps succeed (i.e. the
    // pool has plenty of inventory and slippage is not the limiter).
    let dx: u128 = 1_000_000_000; // 10 icUSD with 8 decimals
    let min_dy: u128 = 0;

    let payload_a = encode_args((0u8, 1u8, dx, min_dy)).unwrap();
    let payload_b = encode_args((0u8, 1u8, dx, min_dy)).unwrap();

    // Submit both swap messages without waiting for completion. Both enter the
    // canister's queue; the canister will interleave them at the first await
    // (the inter-canister transfer_from call to the icUSD ledger).
    let id_a = h
        .pic
        .submit_call(h.three_pool, h.user, "swap", payload_a)
        .expect("submit swap A failed");
    let id_b = h
        .pic
        .submit_call(h.three_pool, h.user, "swap", payload_b)
        .expect("submit swap B failed");

    // Drive the IC forward until both messages complete.
    let res_a = h.pic.await_call(id_a).expect("await swap A failed");
    let res_b = h.pic.await_call(id_b).expect("await swap B failed");

    let r_a = decode_swap_reply(res_a);
    let r_b = decode_swap_reply(res_b);

    // Exactly one must succeed; the other must be rejected with PoolLocked.
    let outcomes: Vec<&Result<u128, ThreePoolError>> = vec![&r_a, &r_b];
    let oks: Vec<&Result<u128, ThreePoolError>> =
        outcomes.iter().filter(|r| r.is_ok()).copied().collect();
    let pool_locked: Vec<&Result<u128, ThreePoolError>> = outcomes
        .iter()
        .filter(|r| matches!(r, Err(ThreePoolError::PoolLocked)))
        .copied()
        .collect();

    assert_eq!(
        oks.len(),
        1,
        "exactly one concurrent swap should succeed, got results: A={:?} B={:?}",
        r_a,
        r_b
    );
    assert_eq!(
        pool_locked.len(),
        1,
        "the other concurrent swap should fail with PoolLocked, got results: A={:?} B={:?}",
        r_a,
        r_b
    );
}

/// B-01 release fence: after a successful swap, the lock is released so the
/// next sequential swap succeeds. Verifies the Drop impl runs on Ok return.
#[test]
fn b_01_lock_released_after_success() {
    let h = deploy_pool_with_liquidity_and_swaps(0);
    let dx: u128 = 1_000_000_000;

    // Two sequential (await each) swaps must both succeed.
    for label in ["first", "second"] {
        let payload = encode_args((0u8, 1u8, dx, 0u128)).unwrap();
        let res = h
            .pic
            .update_call(h.three_pool, h.user, "swap", payload)
            .expect("update swap failed");
        let r = decode_swap_reply(res);
        assert!(
            r.is_ok(),
            "{label} sequential swap must succeed (lock released between calls): {:?}",
            r
        );
    }
}

/// B-01 release-on-error fence: if a swap fails (e.g. slippage exceeded), the
/// lock must still be released so the next call succeeds. Drop must run on
/// the early-error path too.
#[test]
fn b_01_lock_released_after_error() {
    let h = deploy_pool_with_liquidity_and_swaps(0);

    // Set min_dy absurdly high so the swap fails with SlippageExceeded
    // (early-return BEFORE the awaits, but still inside the function body).
    let payload = encode_args((0u8, 1u8, 1_000_000_000u128, u128::MAX)).unwrap();
    let res = h
        .pic
        .update_call(h.three_pool, h.user, "swap", payload)
        .expect("update slippage swap failed");
    let r = decode_swap_reply(res);
    assert!(
        matches!(r, Err(ThreePoolError::SlippageExceeded)),
        "expected SlippageExceeded, got {:?}",
        r
    );

    // Lock must be free now: a normal swap must succeed.
    let payload2 = encode_args((0u8, 1u8, 1_000_000_000u128, 0u128)).unwrap();
    let res2 = h
        .pic
        .update_call(h.three_pool, h.user, "swap", payload2)
        .expect("update post-error swap failed");
    let r2 = decode_swap_reply(res2);
    assert!(
        r2.is_ok(),
        "swap after a slippage error must succeed (lock released): {:?}",
        r2
    );
}

/// B-01 surface coverage: add_liquidity and remove_liquidity must also be
/// covered by the guard. We verify by checking that a concurrent
/// (add_liquidity || swap) pair serializes with one of them returning
/// `PoolLocked`.
#[test]
fn b_01_add_liquidity_concurrent_with_swap_serializes() {
    let h = deploy_pool_with_liquidity_and_swaps(0);

    // Concurrent add_liquidity + swap.
    let amounts: Vec<u128> = vec![1_000_000_000, 10_000_000, 10_000_000]; // 10 of each token
    let add_payload = encode_args((amounts, 0u128)).unwrap();
    let swap_payload = encode_args((0u8, 1u8, 1_000_000_000u128, 0u128)).unwrap();

    let id_a = h
        .pic
        .submit_call(h.three_pool, h.user, "add_liquidity", add_payload)
        .expect("submit add_liquidity failed");
    let id_b = h
        .pic
        .submit_call(h.three_pool, h.user, "swap", swap_payload)
        .expect("submit swap failed");

    let res_a = h.pic.await_call(id_a).expect("await add_liquidity failed");
    let res_b = h.pic.await_call(id_b).expect("await swap failed");

    let WasmResult::Reply(a_bytes) = res_a else {
        panic!("add_liquidity rejected");
    };
    let r_a: Result<candid::Nat, ThreePoolError> =
        decode_one(&a_bytes).expect("decode add_liquidity reply");
    let r_b = decode_swap_reply(res_b);

    let a_locked = matches!(&r_a, Err(ThreePoolError::PoolLocked));
    let b_locked = matches!(&r_b, Err(ThreePoolError::PoolLocked));
    let a_ok = r_a.is_ok();
    let b_ok = r_b.is_ok();

    assert!(
        (a_ok && b_locked) || (a_locked && b_ok),
        "concurrent add_liquidity + swap must serialize: add_liquidity={:?} swap={:?}",
        r_a,
        r_b
    );
}

/// B-01 single-test guard for `remove_liquidity` parity. We seed liquidity
/// already in the harness, then run sequential remove_liquidity twice to
/// confirm the guard does not deadlock the path on its own.
#[test]
fn b_01_remove_liquidity_sequential_does_not_deadlock() {
    let h = deploy_pool_with_liquidity_and_swaps(0);

    // Read LP balance for the user.
    let lp_balance_bytes = h
        .pic
        .query_call(
            h.three_pool,
            Principal::anonymous(),
            "get_lp_balance",
            encode_one(h.user).unwrap(),
        )
        .expect("get_lp_balance failed");
    let WasmResult::Reply(lp_bytes) = lp_balance_bytes else {
        panic!("get_lp_balance rejected");
    };
    let lp_total: u128 = decode_one(&lp_bytes).expect("decode lp balance");
    assert!(lp_total > 0, "user must hold LP after add_liquidity bootstrap");

    // Two small sequential removes — a tenth of LP each.
    let lp_burn = lp_total / 100;
    let min_amounts: Vec<u128> = vec![0, 0, 0];
    for label in ["first", "second"] {
        let payload = encode_args((lp_burn, min_amounts.clone())).unwrap();
        let res = h
            .pic
            .update_call(h.three_pool, h.user, "remove_liquidity", payload)
            .expect("update remove_liquidity failed");
        let WasmResult::Reply(bytes) = res else {
            panic!("{label} remove_liquidity rejected");
        };
        let r: Result<Vec<candid::Nat>, ThreePoolError> =
            decode_one(&bytes).expect("decode remove_liquidity reply");
        assert!(
            r.is_ok(),
            "{label} sequential remove_liquidity must succeed: {:?}",
            r
        );
    }
}
