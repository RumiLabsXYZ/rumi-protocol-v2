//! B-01b regression fence: `receive_donation` must hold the same `PoolGuard`
//! as `swap`/`donate`/`add_liquidity`/`remove_liquidity` so that the on-chain
//! balance read and the internal-balance read used to compute `expected_min`
//! cannot straddle a concurrent state mutation. Closes GHSA-62cr-vcj8-663h
//! Finding 4.
//!
//! Without the guard, a swap completing inside `receive_donation`'s await
//! window changes `s.balances[idx]` between the two reads, producing a stale
//! comparison that can let the balance check pass when it should not (or
//! fail when it should not), corrupting internal accounting relative to the
//! ledger.
//!
//! The guard placement (audit fence B-01b at src/rumi_3pool/src/lib.rs:863)
//! is verified by the existing B-01 fence pattern plus this regression test
//! that exercises the guard's release path on the receive_donation error
//! branch. If the guard ever fails to release on the early-error path, the
//! next mutating operation traps with `PoolLocked` and this test fires.

mod common;

use candid::{decode_one, encode_args};
use common::deploy_pool_with_liquidity_and_swaps;
use pocket_ic::WasmResult;
use rumi_3pool::types::ThreePoolError;

fn decode_swap_reply(result: WasmResult) -> Result<u128, ThreePoolError> {
    let WasmResult::Reply(bytes) = result else {
        panic!("swap call rejected: {:?}", result);
    };
    decode_one(&bytes).expect("swap reply decode failed")
}

fn decode_unit_reply(result: WasmResult) -> Result<(), ThreePoolError> {
    let WasmResult::Reply(bytes) = result else {
        panic!("receive_donation call rejected: {:?}", result);
    };
    decode_one(&bytes).expect("receive_donation reply decode failed")
}

/// B-01b release fence: receive_donation acquires the PoolGuard before its
/// `icrc1_balance_of` await. The guard must be released even when the
/// function returns early (e.g. when the on-chain balance check fails because
/// no extra tokens were transferred to the pool address). If the guard ever
/// leaks across that error path, every subsequent mutating call returns
/// `PoolLocked` until the canister restarts.
#[test]
fn b_01b_pool_guard_released_after_receive_donation_error() {
    let h = deploy_pool_with_liquidity_and_swaps(0);

    // First call: receive_donation will fail at the balance check because we
    // never transferred extra tokens to the pool address. The fence here is
    // the implicit `Drop` on the guard releasing the canister-wide lock on
    // this error path.
    let donate_payload = encode_args((0u8, 1_000_000u128)).unwrap();
    let donate_res = h
        .pic
        .update_call(h.three_pool, h.admin, "receive_donation", donate_payload)
        .expect("receive_donation update failed");
    let r_donate = decode_unit_reply(donate_res);
    assert!(
        matches!(r_donate, Err(ThreePoolError::TransferFailed { .. })),
        "expected TransferFailed from balance check, got {:?}",
        r_donate
    );

    // Second call: a normal swap shares the same PoolGuard. If the guard
    // leaked, this returns `PoolLocked`. The expected outcome is success.
    let swap_payload = encode_args((0u8, 1u8, 1_000_000_000u128, 0u128)).unwrap();
    let swap_res = h
        .pic
        .update_call(h.three_pool, h.user, "swap", swap_payload)
        .expect("swap update failed");
    let r_swap = decode_swap_reply(swap_res);
    assert!(
        !matches!(r_swap, Err(ThreePoolError::PoolLocked)),
        "swap after receive_donation error must NOT be PoolLocked (guard leaked): {:?}",
        r_swap
    );
    assert!(
        r_swap.is_ok(),
        "swap after receive_donation error must succeed: {:?}",
        r_swap
    );
}
