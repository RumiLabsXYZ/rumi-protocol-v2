//! Regression for POOLS-004: failed ICRC-2 pulls must not consume LP allowance.

mod common;

use candid::{decode_one, encode_one, CandidType, Deserialize, Nat, Principal};
use common::deploy_pool_with_liquidity_and_swaps;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::allowance::{Allowance, AllowanceArgs};
use icrc_ledger_types::icrc2::approve::ApproveArgs;
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
use pocket_ic::WasmResult;

fn reply<T: for<'de> Deserialize<'de> + CandidType>(result: WasmResult) -> T {
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode reply"),
        WasmResult::Reject(message) => panic!("call rejected: {message}"),
    }
}

#[test]
fn insufficient_lp_balance_does_not_consume_spender_allowance() {
    let h = deploy_pool_with_liquidity_and_swaps(0);
    let owner = Principal::self_authenticating(&[44, 55, 66]);
    let spender = Principal::self_authenticating(&[77, 88, 99]);
    let approved = 123_456u128;

    let approve: Result<Nat, icrc_ledger_types::icrc2::approve::ApproveError> = reply(
        h.pic
            .update_call(
                h.three_pool,
                owner,
                "icrc2_approve",
                encode_one(ApproveArgs {
                    from_subaccount: None,
                    spender: Account {
                        owner: spender,
                        subaccount: None,
                    },
                    amount: Nat::from(approved),
                    expected_allowance: None,
                    expires_at: None,
                    fee: None,
                    memo: None,
                    created_at_time: None,
                })
                .unwrap(),
            )
            .unwrap(),
    );
    approve.expect("approve the spender");

    let transfer: Result<Nat, TransferFromError> = reply(
        h.pic
            .update_call(
                h.three_pool,
                spender,
                "icrc2_transfer_from",
                encode_one(TransferFromArgs {
                    spender_subaccount: None,
                    from: Account {
                        owner,
                        subaccount: None,
                    },
                    to: Account {
                        owner: spender,
                        subaccount: None,
                    },
                    amount: Nat::from(approved),
                    fee: Some(Nat::from(0u64)),
                    memo: None,
                    created_at_time: None,
                })
                .unwrap(),
            )
            .unwrap(),
    );
    assert!(matches!(
        transfer,
        Err(TransferFromError::InsufficientFunds { .. })
    ));

    let remaining: Allowance = reply(
        h.pic
            .query_call(
                h.three_pool,
                owner,
                "icrc2_allowance",
                encode_one(AllowanceArgs {
                    account: Account {
                        owner,
                        subaccount: None,
                    },
                    spender: Account {
                        owner: spender,
                        subaccount: None,
                    },
                })
                .unwrap(),
            )
            .unwrap(),
    );
    assert_eq!(remaining.allowance, Nat::from(approved));
}
