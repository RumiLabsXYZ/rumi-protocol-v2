// src/rumi_3pool/tests/swap_fees_window.rs
//
// Verifies the `get_swap_fees_over_window` query: it should sum the live
// SwapEventV2 fees (normalized by the output token's precision_mul) over a
// trailing window of N days.
//
// Strategy: spin up the standard PocketIC harness, perform a real swap,
// then exercise the window boundary by advancing virtual time and re-
// querying with a tight window.

mod common;

use candid::{decode_one, encode_args, encode_one, Nat, Principal};
use pocket_ic::WasmResult;
use rumi_3pool::types::ThreePoolError;

use common::{deploy_pool_with_liquidity_and_swaps, ThreePoolHarness};

/// Call the new `get_swap_fees_over_window(window_days)` query and unwrap to u128.
fn get_swap_fees_over_window(harness: &ThreePoolHarness, window_days: u32) -> u128 {
    let bytes = harness
        .pic
        .query_call(
            harness.three_pool,
            Principal::anonymous(),
            "get_swap_fees_over_window",
            encode_one(window_days).unwrap(),
        )
        .expect("get_swap_fees_over_window query failed");
    let WasmResult::Reply(reply) = bytes else {
        panic!("get_swap_fees_over_window rejected")
    };
    let nat: Nat = decode_one(&reply).unwrap();
    nat.0
        .try_into()
        .expect("get_swap_fees_over_window result does not fit in u128")
}

/// Execute a single swap on the harness pool. Returns the realized output
/// amount in native decimals of the output token.
fn execute_swap(
    harness: &ThreePoolHarness,
    token_in: u8,
    token_out: u8,
    amount_in: u128,
) -> u128 {
    let res = harness
        .pic
        .update_call(
            harness.three_pool,
            harness.user,
            "swap",
            encode_args((token_in, token_out, amount_in, 0u128)).unwrap(),
        )
        .expect("swap update call failed");
    let WasmResult::Reply(reply) = res else {
        panic!("swap rejected")
    };
    let result: Result<Nat, ThreePoolError> = decode_one(&reply).unwrap();
    let nat = result.expect("swap returned err");
    nat.0
        .try_into()
        .expect("swap output does not fit in u128")
}

#[test]
fn empty_log_returns_zero() {
    // n_swaps=0 means add_liquidity has run but no swaps have executed.
    // The harness performs `n_swaps` LP-token transfers (not actual swaps),
    // so the swap_v2 log is still empty.
    let harness = deploy_pool_with_liquidity_and_swaps(0);

    // Every window we care about should be zero.
    assert_eq!(get_swap_fees_over_window(&harness, 0), 0);
    assert_eq!(get_swap_fees_over_window(&harness, 1), 0);
    assert_eq!(get_swap_fees_over_window(&harness, 7), 0);
    assert_eq!(get_swap_fees_over_window(&harness, 30), 0);
}

#[test]
fn single_swap_contributes_nonzero_fee_to_recent_window() {
    let harness = deploy_pool_with_liquidity_and_swaps(0);

    // Swap 1000 icUSD (8 dec) -> ckUSDT (6 dec). The harness uses
    // swap_fee_bps=4 (0.04%), so a 1000-token swap pays ~0.4 of ckUSDT
    // (i.e. ~400_000 in native 6-dec units) in fee. Exact value depends
    // on the stableswap curve and admin fee split, but it MUST be > 0.
    let dx: u128 = 100_000_000_000; // 1000 icUSD with 8 decimals
    let dy = execute_swap(&harness, 0, 1, dx);
    assert!(dy > 0, "swap should produce nonzero output");

    // A 30d window should now include the just-executed swap.
    let fees_30d = get_swap_fees_over_window(&harness, 30);
    assert!(
        fees_30d > 0,
        "30d window must include the just-executed swap fee, got {}",
        fees_30d
    );

    // Sanity: a 1d window also includes it (clock has not advanced).
    let fees_1d = get_swap_fees_over_window(&harness, 1);
    assert_eq!(
        fees_30d, fees_1d,
        "single swap, no time advance: 1d and 30d windows must match"
    );
}

#[test]
fn window_excludes_swaps_older_than_window() {
    let harness = deploy_pool_with_liquidity_and_swaps(0);

    // Swap inside ckUSDT->icUSD direction so the test exercises a non-zero
    // precision_mul on the output side too. dx is in ckUSDT 6-dec units.
    let dx_usdt: u128 = 1_000_000_000; // 1000 ckUSDT (6 dec)
    let _dy = execute_swap(&harness, 1, 0, dx_usdt);

    // Window includes the swap immediately.
    let fees_immediately = get_swap_fees_over_window(&harness, 7);
    assert!(fees_immediately > 0);

    // Advance PocketIC time by 10 days. The swap_v2 entry's timestamp
    // (captured at swap time) is now older than a 7d trailing window
    // anchored at the new "now".
    harness
        .pic
        .advance_time(std::time::Duration::from_secs(10 * 86_400));
    // PocketIC ticks the canister timers on advance_time, but query
    // calls also re-read ic_cdk::api::time(), which is what the query
    // uses for "now". Tick once to make sure the wall clock observed
    // by the next query call is the advanced one.
    harness.pic.tick();

    // 7d window should now exclude the swap.
    let fees_7d_after = get_swap_fees_over_window(&harness, 7);
    assert_eq!(
        fees_7d_after, 0,
        "after 10d advance, 7d window must exclude the swap, got {}",
        fees_7d_after
    );

    // 30d window should still include it.
    let fees_30d_after = get_swap_fees_over_window(&harness, 30);
    assert_eq!(
        fees_30d_after, fees_immediately,
        "30d window must still include the swap after 10d advance"
    );
}

#[test]
fn multiple_swaps_in_window_accumulate() {
    let harness = deploy_pool_with_liquidity_and_swaps(0);

    let dx_icusd: u128 = 50_000_000_000; // 500 icUSD (8 dec)

    // First swap.
    let _ = execute_swap(&harness, 0, 1, dx_icusd);
    let fees_after_one = get_swap_fees_over_window(&harness, 30);
    assert!(fees_after_one > 0);

    // Second swap a little later.
    harness.pic.advance_time(std::time::Duration::from_secs(60));
    harness.pic.tick();
    let _ = execute_swap(&harness, 0, 2, dx_icusd);
    let fees_after_two = get_swap_fees_over_window(&harness, 30);

    assert!(
        fees_after_two > fees_after_one,
        "second swap must increase the 30d total: {} -> {}",
        fees_after_one,
        fees_after_two
    );
}
