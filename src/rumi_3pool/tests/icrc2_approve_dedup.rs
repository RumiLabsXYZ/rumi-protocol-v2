//! Exact ICRC-2 approval retries return their first block, including after an
//! upgrade, without reapplying the approval or appending another block.
mod common;

use candid::{decode_one, encode_args, encode_one, Nat};
use common::{deploy_pool_with_liquidity_and_swaps, three_pool_wasm};
use icrc_ledger_types::{
    icrc1::account::Account,
    icrc2::approve::{ApproveArgs, ApproveError},
};
use pocket_ic::WasmResult;

fn approve(h: &common::ThreePoolHarness, args: &ApproveArgs) -> Result<Nat, ApproveError> {
    let result = h
        .pic
        .update_call(
            h.three_pool,
            h.user,
            "icrc2_approve",
            encode_one(args).unwrap(),
        )
        .expect("icrc2_approve call");
    let WasmResult::Reply(bytes) = result else {
        panic!("icrc2_approve rejected: {result:?}");
    };
    decode_one(&bytes).expect("decode icrc2_approve result")
}

#[test]
fn exact_approval_retry_returns_original_block_across_upgrade() {
    let h = deploy_pool_with_liquidity_and_swaps(0);
    let now = h
        .pic
        .get_time()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let args = ApproveArgs {
        from_subaccount: None,
        spender: Account {
            owner: h.admin,
            subaccount: None,
        },
        amount: Nat::from(777u64),
        expected_allowance: Some(Nat::from(0u64)),
        expires_at: Some(now.saturating_add(60_000_000_000)),
        fee: Some(Nat::from(0u64)),
        memo: Some(icrc_ledger_types::icrc1::transfer::Memo::from(
            b"approval-retry".to_vec(),
        )),
        created_at_time: Some(now),
    };

    let first: u64 = approve(&h, &args)
        .expect("initial approval succeeds")
        .0
        .try_into()
        .unwrap();
    let after_first = h.icrc3_log_length();

    // The exact retry must be recognized before expected_allowance is
    // rechecked: the committed allowance is now 777 rather than 0.
    assert!(matches!(
        approve(&h, &args),
        Err(ApproveError::Duplicate { duplicate_of }) if duplicate_of == Nat::from(first)
    ));
    assert_eq!(h.icrc3_log_length(), after_first);

    h.pic
        .upgrade_canister(
            h.three_pool,
            three_pool_wasm(),
            encode_args(()).unwrap(),
            None,
        )
        .expect("upgrade 3pool");

    assert!(matches!(
        approve(&h, &args),
        Err(ApproveError::Duplicate { duplicate_of }) if duplicate_of == Nat::from(first)
    ));
    assert_eq!(h.icrc3_log_length(), after_first);
}

#[test]
fn approvals_without_created_at_time_are_not_deduplicated() {
    let h = deploy_pool_with_liquidity_and_swaps(0);
    let args = ApproveArgs {
        from_subaccount: None,
        spender: Account {
            owner: h.admin,
            subaccount: None,
        },
        amount: Nat::from(777u64),
        expected_allowance: None,
        expires_at: None,
        fee: None,
        memo: None,
        created_at_time: None,
    };

    let before = h.icrc3_log_length();
    let first = approve(&h, &args).expect("first untimestamped approval");
    let second = approve(&h, &args).expect("repeated untimestamped approval");
    assert_ne!(first, second);
    assert_eq!(h.icrc3_log_length(), before + 2);
}
