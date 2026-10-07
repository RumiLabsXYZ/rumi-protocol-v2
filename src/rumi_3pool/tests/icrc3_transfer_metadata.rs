//! New 3USD ICRC-3 transfers carry the metadata required by exact receipt
//! verification. Historical blocks remain byte-for-byte hash-compatible.

mod common;

use candid::{decode_one, encode_one, CandidType, Nat, Principal};
use common::{deploy_pool_with_liquidity_and_swaps, ThreePoolHarness};
use icrc_ledger_types::{
    icrc1::{
        account::Account,
        transfer::{Memo, TransferArg, TransferError},
    },
    icrc2::{
        approve::{ApproveArgs, ApproveError},
        transfer_from::{TransferFromArgs, TransferFromError},
    },
};
use pocket_ic::WasmResult;
use rumi_3pool::icrc3::{BlockWithId, Icrc3Value};
use std::collections::BTreeSet;

const TRANSFER_AMOUNT: u128 = 777;

fn account_value(owner: Principal) -> Icrc3Value {
    Icrc3Value::Array(vec![Icrc3Value::Blob(owner.as_slice().to_vec())])
}

fn call<T: CandidType>(h: &ThreePoolHarness, caller: Principal, method: &str, args: T) -> Vec<u8> {
    match h
        .pic
        .update_call(h.three_pool, caller, method, encode_one(args).unwrap())
        .expect("ledger call")
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("{method} rejected: {message}"),
    }
}

fn block(h: &ThreePoolHarness, block_index: u64) -> BlockWithId {
    h.icrc3_get_blocks(block_index, 1)
        .into_iter()
        .next()
        .expect("transfer block exists")
}

fn transaction_fields(block: &BlockWithId) -> &[(String, Icrc3Value)] {
    let Icrc3Value::Map(fields) = &block.block else {
        panic!("ICRC-3 block must be a map")
    };
    let tx = fields
        .iter()
        .find(|(key, _)| key == "tx")
        .map(|(_, value)| value)
        .expect("block has tx field");
    let Icrc3Value::Map(fields) = tx else {
        panic!("ICRC-3 tx must be a map")
    };
    fields
}

fn assert_exact_transaction_keys(fields: &[(String, Icrc3Value)], keys: &[&str]) {
    let actual: BTreeSet<_> = fields.iter().map(|(key, _)| key.as_str()).collect();
    let expected: BTreeSet<_> = keys.iter().copied().collect();
    assert_eq!(actual, expected);
}

fn assert_common_transfer_fields(
    fields: &[(String, Icrc3Value)],
    from: Principal,
    to: Principal,
    amount: u128,
    memo: &[u8],
    created_at_time: u64,
) {
    let get = |key: &str| {
        fields
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    };
    assert_eq!(get("op"), Some(&Icrc3Value::Text("xfer".into())));
    assert_eq!(get("from"), Some(&account_value(from)));
    assert_eq!(get("to"), Some(&account_value(to)));
    assert_eq!(get("amt"), Some(&Icrc3Value::Nat(Nat::from(amount))));
    assert_eq!(get("memo"), Some(&Icrc3Value::Blob(memo.to_vec())));
    assert_eq!(
        get("ts"),
        Some(&Icrc3Value::Nat(Nat::from(created_at_time)))
    );
}

fn transfer_args(to: Principal, memo: Vec<u8>, created_at_time: u64) -> TransferArg {
    TransferArg {
        from_subaccount: None,
        to: Account {
            owner: to,
            subaccount: None,
        },
        amount: Nat::from(TRANSFER_AMOUNT),
        fee: None,
        memo: Some(Memo::from(memo)),
        created_at_time: Some(created_at_time),
    }
}

#[test]
fn icrc1_and_icrc2_transfer_blocks_bind_memo_time_and_fee_shape() {
    let h = deploy_pool_with_liquidity_and_swaps(0);
    let recipient = Principal::self_authenticating(b"3usd-transfer-metadata-recipient");
    let memo = b"exact-refund-memo".to_vec();
    let direct_time = h
        .pic
        .get_time()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;

    let direct_result: Result<Nat, TransferError> = decode_one(&call(
        &h,
        h.user,
        "icrc1_transfer",
        transfer_args(recipient, memo.clone(), direct_time),
    ))
    .expect("decode direct transfer result");
    let direct_index: u64 = direct_result
        .expect("direct transfer succeeds")
        .0
        .try_into()
        .unwrap();
    let after_direct = h.icrc3_log_length();
    let direct_duplicate: Result<Nat, TransferError> = decode_one(&call(
        &h,
        h.user,
        "icrc1_transfer",
        transfer_args(recipient, memo.clone(), direct_time),
    ))
    .expect("decode direct duplicate");
    assert!(matches!(
        direct_duplicate,
        Err(TransferError::Duplicate { duplicate_of }) if duplicate_of == Nat::from(direct_index)
    ));
    assert_eq!(
        h.icrc3_log_length(),
        after_direct,
        "duplicate adds no block"
    );

    let direct_block = block(&h, direct_index);
    assert_eq!(direct_block.id, Nat::from(direct_index));
    let direct_tx = transaction_fields(&direct_block);
    assert_exact_transaction_keys(direct_tx, &["op", "from", "to", "amt", "fee", "memo", "ts"]);
    assert_common_transfer_fields(
        direct_tx,
        h.user,
        recipient,
        TRANSFER_AMOUNT,
        &memo,
        direct_time,
    );
    assert_eq!(
        direct_tx
            .iter()
            .find(|(key, _)| key == "fee")
            .map(|(_, value)| value),
        Some(&Icrc3Value::Nat(Nat::from(0u64))),
        "ICRC-1 block must prove its effective zero transaction fee"
    );

    let approve: Result<Nat, ApproveError> = decode_one(&call(
        &h,
        h.user,
        "icrc2_approve",
        ApproveArgs {
            from_subaccount: None,
            spender: Account {
                owner: h.admin,
                subaccount: None,
            },
            amount: Nat::from(TRANSFER_AMOUNT),
            expected_allowance: None,
            expires_at: None,
            fee: None,
            memo: None,
            created_at_time: None,
        },
    ))
    .expect("decode transfer_from approval");
    approve.expect("approve transfer_from spender");

    let transfer_from_time = direct_time.saturating_add(1);
    let transfer_from_args = TransferFromArgs {
        spender_subaccount: None,
        from: Account {
            owner: h.user,
            subaccount: None,
        },
        to: Account {
            owner: recipient,
            subaccount: None,
        },
        amount: Nat::from(TRANSFER_AMOUNT),
        fee: None,
        memo: Some(Memo::from(b"exact-ingress-memo".to_vec())),
        created_at_time: Some(transfer_from_time),
    };
    let transfer_from_result: Result<Nat, TransferFromError> = decode_one(&call(
        &h,
        h.admin,
        "icrc2_transfer_from",
        transfer_from_args.clone(),
    ))
    .expect("decode transfer_from result");
    let transfer_from_index: u64 = transfer_from_result
        .expect("transfer_from succeeds")
        .0
        .try_into()
        .unwrap();
    let after_transfer_from = h.icrc3_log_length();
    let duplicate: Result<Nat, TransferFromError> = decode_one(&call(
        &h,
        h.admin,
        "icrc2_transfer_from",
        transfer_from_args,
    ))
    .expect("decode transfer_from duplicate");
    assert!(matches!(
        duplicate,
        Err(TransferFromError::Duplicate { duplicate_of })
            if duplicate_of == Nat::from(transfer_from_index)
    ));
    assert_eq!(
        h.icrc3_log_length(),
        after_transfer_from,
        "duplicate transfer_from adds no block"
    );

    let transfer_from_block = block(&h, transfer_from_index);
    let transfer_from_tx = transaction_fields(&transfer_from_block);
    assert_exact_transaction_keys(
        transfer_from_tx,
        &["op", "from", "to", "amt", "memo", "ts", "spender"],
    );
    assert_common_transfer_fields(
        transfer_from_tx,
        h.user,
        recipient,
        TRANSFER_AMOUNT,
        b"exact-ingress-memo",
        transfer_from_time,
    );
    assert_eq!(
        transfer_from_tx
            .iter()
            .find(|(key, _)| key == "spender")
            .map(|(_, value)| value),
        Some(&account_value(h.admin)),
        "ICRC-2 block must retain the spender account"
    );

    // SP V2 pins the approval fee, memo, timestamp, and expiry in its
    // receipt tuple. Exercise an explicit zero fee because that is the 3pool
    // ledger's accepted fee and must be visible in both block fee evidence
    // and the transaction record.
    let approval_time = direct_time.saturating_add(2);
    let expires_at = direct_time.saturating_add(60_000_000_000);
    let approval_memo = b"sp-approval-proof".to_vec();
    let approval_result: Result<Nat, ApproveError> = decode_one(&call(
        &h,
        h.user,
        "icrc2_approve",
        ApproveArgs {
            from_subaccount: None,
            spender: Account {
                owner: h.admin,
                subaccount: None,
            },
            amount: Nat::from(TRANSFER_AMOUNT),
            expected_allowance: None,
            expires_at: Some(expires_at),
            fee: Some(Nat::from(0u64)),
            memo: Some(Memo::from(approval_memo.clone())),
            created_at_time: Some(approval_time),
        },
    ))
    .expect("decode approval receipt result");
    let approval_index: u64 = approval_result
        .expect("metadata-bearing approval succeeds")
        .0
        .try_into()
        .unwrap();
    let approval_block = block(&h, approval_index);
    let approval_tx = transaction_fields(&approval_block);
    assert_exact_transaction_keys(
        approval_tx,
        &[
            "op",
            "from",
            "spender",
            "amt",
            "fee",
            "memo",
            "ts",
            "expires_at",
        ],
    );
    let get = |key: &str| {
        approval_tx
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    };
    assert_eq!(get("op"), Some(&Icrc3Value::Text("approve".into())));
    assert_eq!(get("from"), Some(&account_value(h.user)));
    assert_eq!(get("spender"), Some(&account_value(h.admin)));
    assert_eq!(
        get("amt"),
        Some(&Icrc3Value::Nat(Nat::from(TRANSFER_AMOUNT)))
    );
    assert_eq!(get("fee"), Some(&Icrc3Value::Nat(Nat::from(0u64))));
    assert_eq!(get("memo"), Some(&Icrc3Value::Blob(approval_memo)));
    assert_eq!(get("ts"), Some(&Icrc3Value::Nat(Nat::from(approval_time))));
    assert_eq!(
        get("expires_at"),
        Some(&Icrc3Value::Nat(Nat::from(expires_at)))
    );
    let Icrc3Value::Map(top_level) = approval_block.block else {
        panic!("approval block must be a map")
    };
    assert!(top_level
        .iter()
        .any(|(key, value)| { key == "fee" && value == &Icrc3Value::Nat(Nat::from(0u64)) }));
}
