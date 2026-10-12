//! Keyed 3pool donation accounting is replay-safe across reply loss/upgrades.

mod common;

use candid::{decode_one, encode_args, encode_one, Nat, Principal};
use common::deploy_pool_with_liquidity_and_swaps;
use icrc_ledger_types::icrc1::{account::Account, transfer::TransferArg};
use pocket_ic::WasmResult;
use rumi_3pool::types::{DonationReceiptCapacityV1, DonationReceiptErrorV1, PoolStatus};

fn call_result<T: for<'de> candid::Deserialize<'de> + candid::CandidType>(
    result: WasmResult,
    context: &str,
) -> T {
    let WasmResult::Reply(bytes) = result else {
        panic!("{context} rejected: {result:?}");
    };
    decode_one(&bytes).unwrap_or_else(|e| panic!("{context} decode failed: {e}"))
}

fn pool_status(h: &common::ThreePoolHarness) -> PoolStatus {
    call_result(
        h.pic
            .query_call(
                h.three_pool,
                Principal::anonymous(),
                "get_pool_status",
                encode_args(()).unwrap(),
            )
            .unwrap(),
        "get_pool_status",
    )
}

fn donation_receipt_capacity(h: &common::ThreePoolHarness) -> DonationReceiptCapacityV1 {
    call_result(
        h.pic
            .query_call(
                h.three_pool,
                Principal::anonymous(),
                "get_donation_receipt_capacity_v1",
                encode_args(()).unwrap(),
            )
            .unwrap(),
        "get_donation_receipt_capacity_v1",
    )
}

fn transfer_to_pool(h: &common::ThreePoolHarness, amount: u128) {
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
        .unwrap();
    let _: Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError> =
        call_result(result, "icrc1_transfer");
}

fn receive_v2(
    h: &common::ThreePoolHarness,
    caller: Principal,
    op_nonce: u128,
    source_mint_block: u64,
    token_index: u8,
    amount: u128,
) -> Result<(), DonationReceiptErrorV1> {
    call_result(
        h.pic
            .update_call(
                h.three_pool,
                caller,
                "receive_donation_v2",
                encode_args((op_nonce, source_mint_block, token_index, amount)).unwrap(),
            )
            .unwrap(),
        "receive_donation_v2",
    )
}

#[test]
fn keyed_donation_is_authenticated_conflict_checked_and_upgrade_idempotent() {
    let h = deploy_pool_with_liquidity_and_swaps(0);
    let op_nonce = 0xfeed_u128;
    let source_mint_block = 88;
    let amount = 25_000u128;
    let capacity = donation_receipt_capacity(&h);
    assert_eq!(capacity.receipt_count, 0);
    assert_eq!(capacity.max_receipts, 1_000_000);
    assert_eq!(capacity.warning_threshold, 800_000);
    assert!(!capacity.warning_threshold_reached);

    assert!(matches!(
        receive_v2(&h, h.user, op_nonce, source_mint_block, 1, amount),
        Err(DonationReceiptErrorV1::Unauthorized),
    ));

    transfer_to_pool(&h, amount);
    let before = pool_status(&h).balances[1];
    // A rejected balance check must not commit the receipt. Retrying the same
    // key with the amount actually transferred can then account for it.
    assert!(matches!(
        receive_v2(&h, h.admin, op_nonce, source_mint_block, 1, amount + 1),
        Err(DonationReceiptErrorV1::TransferFailed { .. }),
    ));
    assert_eq!(
        receive_v2(&h, h.admin, op_nonce, source_mint_block, 1, amount),
        Ok(())
    );
    assert_eq!(pool_status(&h).balances[1], before + amount);
    assert_eq!(donation_receipt_capacity(&h).receipt_count, 1);

    assert_eq!(
        receive_v2(&h, h.admin, op_nonce, source_mint_block, 1, amount),
        Ok(())
    );
    assert_eq!(
        pool_status(&h).balances[1],
        before + amount,
        "exact replay must not add reserves twice"
    );
    assert_eq!(
        donation_receipt_capacity(&h).receipt_count,
        1,
        "replay must not consume receipt capacity"
    );
    assert!(matches!(
        receive_v2(&h, h.admin, op_nonce, source_mint_block + 1, 1, amount),
        Err(DonationReceiptErrorV1::PayloadConflict),
    ));
    assert!(matches!(
        receive_v2(&h, h.admin, op_nonce, source_mint_block, 1, amount + 1),
        Err(DonationReceiptErrorV1::PayloadConflict),
    ));

    // The caller is part of the key. A second authenticated controller may
    // use the same nonce independently without colliding with the admin's
    // receipt; its own exact replay still returns only one reserve credit.
    let second_caller = Principal::from_slice(&[9]);
    let mut controllers = h.pic.get_controllers(h.three_pool);
    let controller = *controllers.first().expect("pool must have a controller");
    if !controllers.contains(&second_caller) {
        controllers.push(second_caller);
        h.pic
            .set_controllers(h.three_pool, Some(controller), controllers)
            .expect("add test caller as controller");
    }
    transfer_to_pool(&h, amount);
    assert_eq!(
        receive_v2(&h, second_caller, op_nonce, source_mint_block, 1, amount),
        Ok(())
    );
    assert_eq!(pool_status(&h).balances[1], before + amount * 2);
    assert_eq!(donation_receipt_capacity(&h).receipt_count, 2);
    assert_eq!(
        receive_v2(&h, second_caller, op_nonce, source_mint_block, 1, amount),
        Ok(())
    );
    assert_eq!(pool_status(&h).balances[1], before + amount * 2);
    assert_eq!(
        donation_receipt_capacity(&h).receipt_count,
        2,
        "upgrade replay must retain one receipt per caller"
    );

    h.pic
        .upgrade_canister(
            h.three_pool,
            common::three_pool_wasm(),
            encode_args(()).unwrap(),
            None,
        )
        .expect("upgrade pool while preserving receipt and reserve state");
    assert_eq!(pool_status(&h).balances[1], before + amount * 2);
    assert_eq!(
        receive_v2(&h, h.admin, op_nonce, source_mint_block, 1, amount),
        Ok(())
    );
    assert_eq!(
        pool_status(&h).balances[1],
        before + amount * 2,
        "upgrade replay must remain idempotent"
    );
    assert!(matches!(
        receive_v2(&h, h.admin, op_nonce, source_mint_block, 1, amount + 1),
        Err(DonationReceiptErrorV1::PayloadConflict),
    ));
}
