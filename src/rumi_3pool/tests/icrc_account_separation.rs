//! ICRC balances and allowances are isolated by canonical full account.
mod common;

use candid::{decode_one, encode_one, Nat};
use common::deploy_pool_with_liquidity_and_swaps;
use icrc_ledger_types::{
    icrc1::{
        account::Account,
        transfer::{TransferArg, TransferError},
    },
    icrc2::{
        allowance::{Allowance, AllowanceArgs},
        approve::{ApproveArgs, ApproveError},
        transfer_from::{TransferFromArgs, TransferFromError},
    },
};
use pocket_ic::WasmResult;

fn update<T: candid::CandidType>(
    h: &common::ThreePoolHarness,
    caller: candid::Principal,
    method: &str,
    arg: T,
) -> Vec<u8> {
    match h
        .pic
        .update_call(h.three_pool, caller, method, encode_one(arg).unwrap())
        .expect("update call")
    {
        WasmResult::Reply(bytes) => bytes,
        other => panic!("{method} failed: {other:?}"),
    }
}

fn balance(h: &common::ThreePoolHarness, account: Account) -> Nat {
    query(h, "icrc1_balance_of", account)
}

fn query<T: candid::CandidType, R: candid::CandidType + for<'de> candid::Deserialize<'de>>(
    h: &common::ThreePoolHarness,
    method: &str,
    arg: T,
) -> R {
    match h
        .pic
        .query_call(h.three_pool, h.user, method, encode_one(arg).unwrap())
        .expect("query")
    {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode query reply"),
        other => panic!("{method} failed: {other:?}"),
    }
}

#[test]
fn balances_and_allowances_use_full_account_identity() {
    let h = deploy_pool_with_liquidity_and_swaps(0);
    let default = Account {
        owner: h.user,
        subaccount: None,
    };
    let zero_alias = Account {
        owner: h.user,
        subaccount: Some([0; 32]),
    };
    let first = Account {
        owner: h.user,
        subaccount: Some([1; 32]),
    };
    let other = Account {
        owner: h.user,
        subaccount: Some([2; 32]),
    };
    let initial = balance(&h, default.clone());
    assert!(initial > Nat::from(777u64));
    let supply_before: Nat = query(&h, "icrc1_total_supply", ());

    let moved: Result<Nat, TransferError> = decode_one(&update(
        &h,
        h.user,
        "icrc1_transfer",
        TransferArg {
            from_subaccount: None,
            to: first.clone(),
            amount: Nat::from(777u64),
            fee: None,
            memo: None,
            created_at_time: None,
        },
    ))
    .unwrap();
    moved.expect("transfer default balance into a sibling subaccount");
    assert_eq!(
        balance(&h, default.clone()),
        initial.clone() - Nat::from(777u64)
    );
    assert_eq!(balance(&h, zero_alias), balance(&h, default.clone()));
    assert_eq!(balance(&h, first.clone()), Nat::from(777u64));
    assert_eq!(balance(&h, other.clone()), Nat::from(0u64));
    let supply_after: Nat = query(&h, "icrc1_total_supply", ());
    assert_eq!(supply_after, supply_before);

    // A principal-level self check is insufficient once accounts are split:
    // the default account cannot spend from its sibling subaccount without an
    // allowance, even though both accounts share the caller principal.
    let sibling_spend: Result<Nat, TransferFromError> = decode_one(&update(
        &h,
        h.user,
        "icrc2_transfer_from",
        TransferFromArgs {
            spender_subaccount: None,
            from: first.clone(),
            to: Account {
                owner: h.admin,
                subaccount: Some([8; 32]),
            },
            amount: Nat::from(1u64),
            fee: None,
            memo: None,
            created_at_time: None,
        },
    ))
    .unwrap();
    assert!(matches!(
        sibling_spend,
        Err(TransferFromError::InsufficientAllowance { .. })
    ));

    // An exact full-account self transfer_from remains exempt from allowance.
    let exact_self_spend: Result<Nat, TransferFromError> = decode_one(&update(
        &h,
        h.user,
        "icrc2_transfer_from",
        TransferFromArgs {
            spender_subaccount: Some([1; 32]),
            from: first.clone(),
            to: Account {
                owner: h.admin,
                subaccount: Some([8; 32]),
            },
            amount: Nat::from(1u64),
            fee: None,
            memo: None,
            created_at_time: None,
        },
    ))
    .unwrap();
    exact_self_spend.expect("identical owner and spender accounts need no allowance");

    let spender = Account {
        owner: h.admin,
        subaccount: Some([3; 32]),
    };
    let approve: Result<Nat, ApproveError> = decode_one(&update(
        &h,
        h.user,
        "icrc2_approve",
        ApproveArgs {
            from_subaccount: Some([1; 32]),
            spender: spender.clone(),
            amount: Nat::from(300u64),
            expected_allowance: None,
            expires_at: None,
            fee: None,
            memo: None,
            created_at_time: None,
        },
    ))
    .unwrap();
    approve.expect("approve exact owner and spender accounts");
    let exact: Allowance = query(
        &h,
        "icrc2_allowance",
        AllowanceArgs {
            account: first.clone(),
            spender: spender.clone(),
        },
    );
    let wrong_spender: Allowance = query(
        &h,
        "icrc2_allowance",
        AllowanceArgs {
            account: first.clone(),
            spender: Account {
                owner: h.admin,
                subaccount: None,
            },
        },
    );
    let wrong_owner: Allowance = query(
        &h,
        "icrc2_allowance",
        AllowanceArgs {
            account: other.clone(),
            spender: spender.clone(),
        },
    );
    assert_eq!(exact.allowance, Nat::from(300u64));
    assert_eq!(wrong_spender.allowance, Nat::from(0u64));
    assert_eq!(wrong_owner.allowance, Nat::from(0u64));

    let wrong_from: Result<Nat, TransferFromError> = decode_one(&update(
        &h,
        h.admin,
        "icrc2_transfer_from",
        TransferFromArgs {
            spender_subaccount: Some([3; 32]),
            from: other,
            to: Account {
                owner: h.admin,
                subaccount: Some([4; 32]),
            },
            amount: Nat::from(100u64),
            fee: None,
            memo: None,
            created_at_time: None,
        },
    ))
    .unwrap();
    assert!(matches!(
        wrong_from,
        Err(TransferFromError::InsufficientAllowance { .. })
    ));

    let taken: Result<Nat, TransferFromError> = decode_one(&update(
        &h,
        h.admin,
        "icrc2_transfer_from",
        TransferFromArgs {
            spender_subaccount: Some([3; 32]),
            from: first.clone(),
            to: Account {
                owner: h.admin,
                subaccount: Some([4; 32]),
            },
            amount: Nat::from(100u64),
            fee: None,
            memo: None,
            created_at_time: None,
        },
    ))
    .unwrap();
    taken.expect("exact allowance authorizes transfer from exact account");
    assert_eq!(balance(&h, first), Nat::from(676u64));
    assert_eq!(
        balance(
            &h,
            Account {
                owner: h.admin,
                subaccount: Some([4; 32])
            }
        ),
        Nat::from(100u64)
    );
}
