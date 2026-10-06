//! The backend must be able to retry a committed 3pool donation without
//! applying its internal balance increase a second time, including after an
//! upgrade. A reused operation ID with a different payload must fail closed.
mod common;

use candid::{decode_one, encode_args, encode_one, Nat};
use common::{deploy_pool_with_liquidity_and_swaps, three_pool_wasm};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::TransferArg;
use pocket_ic::WasmResult;
use rumi_3pool::types::{PoolStatus, ThreePoolError};

fn reply(result: WasmResult) -> Vec<u8> {
    match result {
        WasmResult::Reply(bytes) => bytes,
        other => panic!("unexpected reject: {other:?}"),
    }
}

fn status(h: &common::ThreePoolHarness) -> PoolStatus {
    decode_one(&reply(
        h.pic
            .query_call(h.three_pool, h.admin, "get_pool_status", encode_args(()).unwrap())
            .unwrap(),
    ))
    .unwrap()
}

#[test]
fn exact_donation_receipt_survives_retry_and_upgrade() {
    let h = deploy_pool_with_liquidity_and_swaps(0);
    let op_nonce = 77_001u128;
    let amount = 123_456_789u128;
    let before = status(&h).balances[0];
    let transfer = TransferArg {
        from_subaccount: None,
        to: Account { owner: h.three_pool, subaccount: None },
        amount: Nat::from(amount),
        fee: None,
        memo: None,
        created_at_time: None,
    };
    let transferred = h.pic
        .update_call(h.ledgers[0], h.user, "icrc1_transfer", encode_one(transfer).unwrap())
        .unwrap();
    assert!(matches!(transferred, WasmResult::Reply(_)), "ledger transfer: {transferred:?}");

    let submit = |nonce: u128, credited: u128| {
        h.pic.update_call(
            h.three_pool,
            h.admin,
            "receive_donation_with_id",
            encode_args((Nat::from(nonce), 0u8, Nat::from(credited))).unwrap(),
        ).unwrap()
    };
    let first: Result<(), ThreePoolError> = decode_one(&reply(submit(op_nonce, amount))).unwrap();
    first.expect("initial donation acknowledgment");
    assert_eq!(status(&h).balances[0], before + amount);

    let duplicate: Result<(), ThreePoolError> = decode_one(&reply(submit(op_nonce, amount))).unwrap();
    duplicate.expect("exact retry must return success");
    let conflict: Result<(), ThreePoolError> = decode_one(&reply(submit(op_nonce, amount + 1))).unwrap();
    assert!(matches!(conflict, Err(ThreePoolError::DonationIntentConflict)));
    assert_eq!(status(&h).balances[0], before + amount, "retry/conflict must not re-credit");

    h.pic.upgrade_canister(h.three_pool, three_pool_wasm(), encode_args(()).unwrap(), None).unwrap();
    let after_upgrade: Result<(), ThreePoolError> = decode_one(&reply(submit(op_nonce, amount))).unwrap();
    after_upgrade.expect("stable receipt must survive upgrade");
    assert_eq!(status(&h).balances[0], before + amount, "upgrade replay must not re-credit");
}
