//! PocketIC coverage for the 3pool icUSD concentration admission policy.
//!
//! These tests intentionally exercise the public query and update endpoints
//! against real ICRC-1 ledgers.  In particular, a rejected deposit must be
//! decided before `add_liquidity` pulls any token from the caller.

mod common;

use candid::{decode_one, encode_args, encode_one, Nat, Principal};
use common::{deploy_pool_with_liquidity_and_swaps, ThreePoolHarness};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
use pocket_ic::WasmResult;
use rumi_3pool::receipts::{
    IngressReceiptErrorV1, IngressReceiptV1, SwapReceiptErrorV1, SwapReceiptStatusV1,
    SwapReceiptV1, SwapRequestV1,
};
use rumi_3pool::types::{PoolStatus, QuoteSwapResult, ThreePoolError};

const ICUSD_1M: u128 = 100_000_000_000_000;
const STABLE_1M: u128 = 1_000_000_000_000;

fn reply_bytes(result: WasmResult) -> Vec<u8> {
    match result {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("canister call rejected: {message}"),
    }
}

fn pool_status(h: &ThreePoolHarness) -> PoolStatus {
    decode_one(&reply_bytes(
        h.pic
            .query_call(
                h.three_pool,
                Principal::anonymous(),
                "get_pool_status",
                encode_args(()).unwrap(),
            )
            .expect("get_pool_status failed"),
    ))
    .expect("decode get_pool_status")
}

fn ledger_balance(h: &ThreePoolHarness, ledger: Principal, owner: Principal) -> u128 {
    let nat: Nat = decode_one(&reply_bytes(
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
            .expect("icrc1_balance_of failed"),
    ))
    .expect("decode icrc1_balance_of");
    nat.0.try_into().expect("ledger balance does not fit u128")
}

fn user_balances(h: &ThreePoolHarness) -> [u128; 3] {
    std::array::from_fn(|i| ledger_balance(h, h.ledgers[i], h.user))
}

fn pool_ledger_balances(h: &ThreePoolHarness) -> [u128; 3] {
    std::array::from_fn(|i| ledger_balance(h, h.ledgers[i], h.three_pool))
}

fn add_quote(h: &ThreePoolHarness, amounts: [u128; 3]) -> Result<Nat, ThreePoolError> {
    decode_one(&reply_bytes(
        h.pic
            .query_call(
                h.three_pool,
                Principal::anonymous(),
                "calc_add_liquidity_query",
                encode_args((amounts.to_vec(), 0u128)).unwrap(),
            )
            .expect("calc_add_liquidity_query failed"),
    ))
    .expect("decode calc_add_liquidity_query")
}

fn add_update(
    h: &ThreePoolHarness,
    amounts: [u128; 3],
) -> Result<Nat, IngressReceiptErrorV1> {
    let result: Result<IngressReceiptV1, IngressReceiptErrorV1> = decode_one(&reply_bytes(
        h.pic
            .update_call(
                h.three_pool,
                h.user,
                "add_liquidity_with_receipt_v1",
                encode_args((vec![42u8; 32], amounts.to_vec(), 0u128)).unwrap(),
            )
            .expect("add_liquidity failed"),
    ))
    .expect("decode add_liquidity_with_receipt_v1");
    result.map(|receipt| Nat::from(receipt.result_lp.expect("successful receipt LP amount")))
}

fn assert_policy_reject_error(label: &str, error: impl std::fmt::Debug) {
    let description = format!("{error:?}");
    assert!(
        description.contains("Deposit rejected: icUSD concentration exceeds the 66.6% limit"),
        "{label} should explain the concentration policy rejection, got: {description}"
    );
}

fn swap_quote(
    h: &ThreePoolHarness,
    token_in: u8,
    token_out: u8,
    amount_in: u128,
) -> Result<QuoteSwapResult, ThreePoolError> {
    decode_one(&reply_bytes(
        h.pic
            .query_call(
                h.three_pool,
                Principal::anonymous(),
                "quote_swap",
                encode_args((token_in, token_out, amount_in)).unwrap(),
            )
            .expect("quote_swap failed"),
    ))
    .expect("decode quote_swap")
}

fn swap_update(
    h: &ThreePoolHarness,
    token_in: u8,
    token_out: u8,
    amount_in: u128,
    min_out: u128,
) -> Result<Nat, ThreePoolError> {
    let result: Result<SwapReceiptV1, SwapReceiptErrorV1> = decode_one(&reply_bytes(
        h.pic
            .update_call(
                h.three_pool,
                h.user,
                "swap_with_receipt_v1",
                encode_one(SwapRequestV1 {
                    intent_id: vec![43u8; 32],
                    i: token_in,
                    j: token_out,
                    dx: amount_in,
                    min_dy: min_out,
                })
                .unwrap(),
            )
            .expect("swap failed"),
    ))
    .expect("decode swap_with_receipt_v1");
    result
        .map(|receipt| {
            assert_eq!(receipt.status, SwapReceiptStatusV1::Completed);
            Nat::from(receipt.gross_output.expect("completed swap gross output"))
        })
        .map_err(|_| ThreePoolError::TransferFailed {
            token: "swap".to_string(),
            reason: "receipt-backed test swap failed".to_string(),
        })
}

fn record_donation(h: &ThreePoolHarness, token_index: usize, amount: u128) {
    let transfer = TransferArg {
        from_subaccount: None,
        to: Account {
            owner: h.three_pool,
            subaccount: None,
        },
        fee: None,
        created_at_time: None,
        memo: None,
        amount: Nat::from(amount),
    };
    let transfer_result: Result<Nat, TransferError> = decode_one(&reply_bytes(
        h.pic
            .update_call(
                h.ledgers[token_index],
                h.user,
                "icrc1_transfer",
                encode_one(transfer).unwrap(),
            )
            .expect("icUSD donation transfer failed"),
    ))
    .expect("decode icUSD donation transfer");
    transfer_result.expect("icUSD donation transfer returned an error");

    let received: Result<(), ThreePoolError> = decode_one(&reply_bytes(
        h.pic
            .update_call(
                h.three_pool,
                h.admin,
                "receive_donation",
                encode_args((token_index as u8, amount)).unwrap(),
            )
            .expect("receive_donation failed"),
    ))
    .expect("decode receive_donation");
    received.expect("receive_donation returned an error");
}

fn record_icusd_donation(h: &ThreePoolHarness, amount: u128) {
    record_donation(h, 0, amount);
}

fn move_above_cap(h: &ThreePoolHarness) {
    // The balanced fixture starts at 1M / 1M / 1M.  Adding 3M icUSD yields
    // 4M / 1M / 1M, so icUSD is just over 666/1000 of the normalized pool.
    record_icusd_donation(h, 3 * ICUSD_1M);
    let status = pool_status(h);
    assert_eq!(status.balances, [4 * ICUSD_1M, STABLE_1M, STABLE_1M]);
}

fn move_to_exact_cap(h: &ThreePoolHarness) {
    // Target normalized balances are 3,996,000 / 1,004,000 / 1,000,000,
    // whose icUSD share is exactly 666/1000.  The fixture starts at
    // 1,000,000 / 1,000,000 / 1,000,000.
    record_icusd_donation(h, 2_996_000 * 100_000_000);
    record_donation(h, 1, 4_000 * 1_000_000);
    let status = pool_status(h);
    let icusd_xp = status.balances[0] * 10_000_000_000;
    let total_xp =
        icusd_xp + status.balances[1] * 1_000_000_000_000 + status.balances[2] * 1_000_000_000_000;
    assert_eq!(icusd_xp * 1_000, total_xp * 666);
}

#[test]
fn icusd_deposit_crossing_cap_is_rejected_before_any_ledger_pull() {
    let h = deploy_pool_with_liquidity_and_swaps(0);
    let before_status = pool_status(&h);
    let before_user = user_balances(&h);
    let before_pool_ledgers = pool_ledger_balances(&h);

    // Starting from 1M / 1M / 1M, a 4M icUSD-only deposit would produce an
    // icUSD share above 666/1000.  The quote and update must agree that it is
    // invalid, and the update must fail before transfer_from is attempted.
    let amounts = [4 * ICUSD_1M, 0, 0];
    let quote_error = h
        .pic
        .query_call(
            h.three_pool,
            Principal::anonymous(),
            "calc_add_liquidity_query",
            encode_args((amounts.to_vec(), 0u128)).unwrap(),
        )
        .expect_err("cap-crossing quote should reject before returning a value");
    assert_policy_reject_error("cap-crossing quote", quote_error);

    let updated = add_update(&h, amounts);
    assert!(
        matches!(&updated, Err(IngressReceiptErrorV1::InvalidRequest)),
        "cap-crossing update returned an unexpected result: {updated:?}"
    );

    let after_status = pool_status(&h);
    assert_eq!(after_status.balances, before_status.balances);
    assert_eq!(after_status.lp_total_supply, before_status.lp_total_supply);
    assert_eq!(
        user_balances(&h),
        before_user,
        "user was debited before rejection"
    );
    assert_eq!(
        pool_ledger_balances(&h),
        before_pool_ledgers,
        "pool ledger balances changed before rejection"
    );
}

#[test]
fn legacy_add_liquidity_route_rejects_without_pulling_tokens() {
    let h = deploy_pool_with_liquidity_and_swaps(0);
    let before_user = user_balances(&h);
    let before_pool = pool_ledger_balances(&h);
    let result: Result<Nat, ThreePoolError> = decode_one(&reply_bytes(
        h.pic
            .update_call(
                h.three_pool,
                h.user,
                "add_liquidity",
                encode_args((vec![1_000_000u128; 3], 0u128)).unwrap(),
            )
            .expect("legacy add_liquidity call failed at transport"),
    ))
    .expect("decode legacy add_liquidity");
    assert!(matches!(result, Err(ThreePoolError::TransferFailed { .. })));
    assert_eq!(
        user_balances(&h),
        before_user,
        "legacy route pulled user funds"
    );
    assert_eq!(
        pool_ledger_balances(&h),
        before_pool,
        "legacy route changed pool ledger balances"
    );
}

#[test]
fn below_cap_200_plus_100_deposit_is_allowed_when_result_stays_below_cap() {
    let h = deploy_pool_with_liquidity_and_swaps(0);

    // The incoming pair is approximately 2/3 icUSD, but the existing pool is
    // only 1/3 icUSD, so the resulting pool remains well below 666/1000.
    let amounts = [200_000 * 100_000_000, 100_000 * 1_000_000, 0];
    let quoted = add_quote(&h, amounts).expect("below-cap deposit should be accepted");
    let minted = add_update(&h, amounts).expect("below-cap deposit should be accepted");
    assert_eq!(
        minted, quoted,
        "update must mint exactly the quoted LP amount"
    );
}

#[test]
fn exact_cap_proportional_deposit_is_accepted() {
    let h = deploy_pool_with_liquidity_and_swaps(0);
    move_to_exact_cap(&h);

    // Add 1% of every normalized leg.  The resulting pool remains exactly at
    // the cap, so the at-or-below path must accept the proportional deposit.
    let amounts = [39_960 * 100_000_000, 10_040 * 1_000_000, 10_000 * 1_000_000];
    let quoted = add_quote(&h, amounts).expect("exact-cap proportional quote should succeed");
    let minted = add_update(&h, amounts).expect("exact-cap proportional deposit should succeed");
    assert_eq!(
        minted, quoted,
        "update must mint exactly the quoted LP amount"
    );

    let status = pool_status(&h);
    let icusd_xp = status.balances[0] * 10_000_000_000;
    let total_xp =
        icusd_xp + status.balances[1] * 1_000_000_000_000 + status.balances[2] * 1_000_000_000_000;
    assert_eq!(icusd_xp * 1_000, total_xp * 666);
}

#[test]
fn above_cap_icusd_plus_one_stable_is_accepted_and_quote_matches_update() {
    let h = deploy_pool_with_liquidity_and_swaps(0);
    move_above_cap(&h);

    // Equal normalized icUSD and ckUSDT makes the concentration fall from
    // 4/6 to 5/8, and exercises the one-stable corrective route.
    let amounts = [ICUSD_1M, STABLE_1M, 0];
    let quoted = add_quote(&h, amounts).expect("paired quote should be accepted");
    let minted = add_update(&h, amounts).expect("paired deposit should be accepted");
    assert_eq!(
        minted, quoted,
        "update must mint exactly the quoted LP amount"
    );

    let status = pool_status(&h);
    assert_eq!(status.balances, [5 * ICUSD_1M, 2 * STABLE_1M, STABLE_1M]);
}

#[test]
fn above_cap_70_percent_icusd_deposit_is_rejected_before_any_ledger_pull() {
    let h = deploy_pool_with_liquidity_and_swaps(0);
    move_above_cap(&h);
    let before_status = pool_status(&h);
    let before_user = user_balances(&h);
    let before_pool_ledgers = pool_ledger_balances(&h);

    // The 700/300 normalized deposit is non-worsening relative to the old
    // pool share, but its own share exceeds 666/1000 and must be rejected.
    let amounts = [700_000u128 * 100_000_000, 300_000u128 * 1_000_000, 0];
    let quote_error = h
        .pic
        .query_call(
            h.three_pool,
            Principal::anonymous(),
            "calc_add_liquidity_query",
            encode_args((amounts.to_vec(), 0u128)).unwrap(),
        )
        .expect_err("70/30 quote should reject before returning a value");
    assert_policy_reject_error("70/30 quote", quote_error);
    let updated = add_update(&h, amounts);
    assert!(
        matches!(&updated, Err(IngressReceiptErrorV1::InvalidRequest)),
        "70/30 update returned an unexpected result: {updated:?}"
    );
    assert_eq!(pool_status(&h).balances, before_status.balances);
    assert_eq!(
        pool_status(&h).lp_total_supply,
        before_status.lp_total_supply
    );
    assert_eq!(user_balances(&h), before_user);
    assert_eq!(pool_ledger_balances(&h), before_pool_ledgers);
}

#[test]
fn above_cap_666_334_icusd_deposit_is_accepted() {
    let h = deploy_pool_with_liquidity_and_swaps(0);
    move_above_cap(&h);

    // The incoming deposit is exactly 666/1000 normalized icUSD and is
    // accepted even though the existing pool is already above the cap.
    let amounts = [666_000 * 100_000_000, 334_000 * 1_000_000, 0];
    let quoted = add_quote(&h, amounts).expect("666/334 quote should be accepted");
    let minted = add_update(&h, amounts).expect("666/334 deposit should be accepted");
    assert_eq!(
        minted, quoted,
        "update must mint exactly the quoted LP amount"
    );
}

#[test]
fn above_cap_icusd_plus_both_stables_is_accepted_and_quote_matches_update() {
    let h = deploy_pool_with_liquidity_and_swaps(0);
    move_above_cap(&h);

    // Both stable legs together equal the normalized icUSD leg.  This keeps
    // the acceptance rule independent of which stable receives the pairing.
    let amounts = [ICUSD_1M, STABLE_1M / 2, STABLE_1M / 2];
    let quoted = add_quote(&h, amounts).expect("two-stable quote should be accepted");
    let minted = add_update(&h, amounts).expect("two-stable deposit should be accepted");
    assert_eq!(
        minted, quoted,
        "update must mint exactly the quoted LP amount"
    );

    let status = pool_status(&h);
    assert_eq!(
        status.balances,
        [
            5 * ICUSD_1M,
            STABLE_1M + STABLE_1M / 2,
            STABLE_1M + STABLE_1M / 2
        ]
    );
}

#[test]
fn above_cap_stable_only_correction_is_accepted() {
    let h = deploy_pool_with_liquidity_and_swaps(0);
    move_above_cap(&h);

    let amounts = [0, STABLE_1M, 0];
    let quoted = add_quote(&h, amounts).expect("stable-only quote should be accepted");
    let minted = add_update(&h, amounts).expect("stable-only deposit should be accepted");
    assert_eq!(
        minted, quoted,
        "update must mint exactly the quoted LP amount"
    );

    let status = pool_status(&h);
    assert_eq!(status.balances, [4 * ICUSD_1M, 2 * STABLE_1M, STABLE_1M]);
}

#[test]
fn above_cap_swap_quote_and_update_remain_unchanged() {
    let h = deploy_pool_with_liquidity_and_swaps(0);
    move_above_cap(&h);

    // A corrective ckUSDT -> icUSD swap remains available while the pool is
    // above the deposit cap.  The cap is specific to icUSD add-liquidity
    // admission and must not alter swap quote/update parity.
    let amount_in = STABLE_1M / 1_000;
    let quoted = swap_quote(&h, 1, 0, amount_in).expect("swap quote should be accepted");
    let actual =
        swap_update(&h, 1, 0, amount_in, quoted.amount_out).expect("swap should be accepted");
    assert_eq!(
        actual,
        Nat::from(quoted.amount_out),
        "swap output must match its quote"
    );
}
