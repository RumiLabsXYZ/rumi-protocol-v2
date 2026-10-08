//! P08-06 regression: donations must be backed by funds in excess of all
//! existing pool liabilities, not merely the LP reserve balance.

mod common;

use candid::{decode_one, encode_args, encode_one, Nat, Principal};
use common::deploy_pool_with_liquidity_and_swaps;
use icrc_ledger_types::icrc1::{account::Account, transfer::TransferArg};
use pocket_ic::WasmResult;
use rumi_3pool::types::{PoolStatus, ThreePoolError};

fn call_result<T: for<'de> candid::Deserialize<'de> + candid::CandidType>(
    result: WasmResult,
    context: &str,
) -> T {
    let WasmResult::Reply(bytes) = result else {
        panic!("{context} rejected: {result:?}");
    };
    decode_one(&bytes).unwrap_or_else(|e| panic!("{context} decode failed: {e}"))
}

fn receive_donation(h: &common::ThreePoolHarness, amount: u128) -> Result<(), ThreePoolError> {
    let result = h
        .pic
        .update_call(
            h.three_pool,
            h.admin,
            "receive_donation",
            encode_args((1u8, amount)).unwrap(),
        )
        .expect("receive_donation update failed");
    call_result(result, "receive_donation")
}

fn pool_status(h: &common::ThreePoolHarness) -> PoolStatus {
    let result = h
        .pic
        .query_call(
            h.three_pool,
            Principal::anonymous(),
            "get_pool_status",
            encode_args(()).unwrap(),
        )
        .expect("get_pool_status query failed");
    call_result(result, "get_pool_status")
}

fn transfer_ckusdt_to_pool(h: &common::ThreePoolHarness, amount: u128) {
    let result = h
        .pic
        .update_call(
            h.ledgers[1],
            h.user,
            "icrc1_transfer",
            encode_one(TransferArg {
                from_subaccount: None,
                to: Account {
                    owner: h.three_pool,
                    subaccount: None,
                },
                fee: None,
                created_at_time: None,
                memo: None,
                amount: Nat::from(amount),
            })
            .unwrap(),
        )
        .expect("ckUSDT transfer failed");
    let _: Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError> =
        call_result(result, "ckUSDT transfer");
}

#[test]
fn receive_donation_cannot_reclassify_admin_fees_or_pending_claims_as_reserves() {
    let h = deploy_pool_with_liquidity_and_swaps(0);

    // Accrue a real output-side admin fee and confirm the amount is nonzero.
    let swap = h
        .pic
        .update_call(
            h.three_pool,
            h.user,
            "swap",
            encode_args((0u8, 1u8, 100_000_000_000u128, 0u128)).unwrap(),
        )
        .expect("swap update failed");
    let swap_result: Result<u128, ThreePoolError> = call_result(swap, "swap");
    swap_result.expect("swap failed");
    let admin_fees: Vec<u128> = call_result(
        h.pic
            .query_call(
                h.three_pool,
                Principal::anonymous(),
                "get_admin_fees",
                encode_args(()).unwrap(),
            )
            .expect("get_admin_fees query failed"),
        "get_admin_fees",
    );
    let fee = admin_fees[1];
    assert!(fee > 0, "swap should accrue a ckUSDT admin fee");

    let before_fee_check = pool_status(&h).balances[1];
    assert!(
        matches!(
            receive_donation(&h, fee),
            Err(ThreePoolError::TransferFailed { .. })
        ),
        "admin fees alone must not back a donation"
    );
    assert_eq!(
        pool_status(&h).balances[1],
        before_fee_check,
        "rejected fee-backed donation must not mutate reserves"
    );

    // Put exactly the claim amount in the pool account, then record it as a
    // pending outbound liability. Without counting claims, this balance would
    // appear sufficient to credit the same amount again as a donation.
    let claim_amount = 10_000u128;
    transfer_ckusdt_to_pool(&h, claim_amount);

    let insert_claim = h
        .pic
        .update_call(
            h.three_pool,
            h.admin,
            "test_insert_pending_claim",
            encode_args((1u8, claim_amount)).unwrap(),
        )
        .expect("test_insert_pending_claim update failed");
    let _: u64 = call_result(insert_claim, "test_insert_pending_claim");

    let before_claim_check = pool_status(&h).balances[1];
    assert!(
        matches!(
            receive_donation(&h, claim_amount),
            Err(ThreePoolError::TransferFailed { .. })
        ),
        "pending claim backing must not be reclassified as reserves"
    );
    assert_eq!(
        pool_status(&h).balances[1],
        before_claim_check,
        "rejected claim-backed donation must not mutate reserves"
    );

    // A separately funded donation remains valid even while both liability
    // classes are outstanding.
    let donation_amount = 25_000u128;
    transfer_ckusdt_to_pool(&h, donation_amount);
    let before_valid_donation = pool_status(&h).balances[1];
    assert!(receive_donation(&h, donation_amount).is_ok());
    assert_eq!(
        pool_status(&h).balances[1],
        before_valid_donation + donation_amount,
        "funded donation should increase LP reserves"
    );
}
