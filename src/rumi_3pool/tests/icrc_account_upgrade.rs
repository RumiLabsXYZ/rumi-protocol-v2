//! Upgrade from the principal-keyed stable ledger at f6b02f31.
//!
//! The predecessor Wasm is built from that exact source commit using the
//! original Cargo.lock (SHA-256
//! `61ed8aa8208cfd12aa526e9226a08980814485a2d224c64c57e558d699afc24c`),
//! rustc 1.92.0 / cargo 1.92.0, and `test_endpoints` only for this PocketIC
//! fixture. Its Wasm SHA-256 is pinned below.
//! Its stable maps at MemoryIds 1 and 2 use principal-only keys. They are
//! preserved by the new implementation as default-account balances and
//! default/default allowances. Historical ICRC-3 blocks retain their original
//! subaccount tuples; those tuples do not retroactively repartition balances.
mod common;

use candid::{decode_one, encode_args, encode_one, Nat, Principal};
use common::{
    deploy_pool_with_liquidity_and_swaps, three_pool_test_endpoints_wasm, ThreePoolHarness,
};
use icrc_ledger_types::{
    icrc1::{
        account::Account,
        transfer::{TransferArg, TransferError},
    },
    icrc2::{
        allowance::{Allowance, AllowanceArgs},
        approve::{ApproveArgs, ApproveError},
    },
};
use pocket_ic::WasmResult;
use rumi_3pool::{
    icrc3::{BlockWithId, GetBlocksArgs, GetBlocksResult, Icrc3Value},
    receipts::{IngressReceiptErrorV1, IngressReceiptV1},
    types::{ThreePoolInitArgs, TokenConfig},
};
use sha2::{Digest, Sha256};

const PREDECESSOR_WASM: &[u8] =
    include_bytes!("fixtures/rumi_3pool_pre_account_separation_f6b02f31_test_endpoints.wasm");
const PREDECESSOR_WASM_SHA256: &str =
    "9683b40fad00a9b55b364705fcf8092c3c005e737e99e1e0af005b48a6b904f0";

fn reply<T: for<'de> candid::Deserialize<'de> + candid::CandidType>(result: WasmResult) -> T {
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode reply"),
        WasmResult::Reject(message) => panic!("call rejected: {message}"),
    }
}

fn account_balance(h: &ThreePoolHarness, pool: Principal, account: Account) -> Nat {
    reply(
        h.pic
            .query_call(
                pool,
                Principal::anonymous(),
                "icrc1_balance_of",
                encode_one(account).unwrap(),
            )
            .unwrap(),
    )
}

fn account_allowance(
    h: &ThreePoolHarness,
    pool: Principal,
    account: Account,
    spender: Account,
) -> Allowance {
    reply(
        h.pic
            .query_call(
                pool,
                Principal::anonymous(),
                "icrc2_allowance",
                encode_one(AllowanceArgs { account, spender }).unwrap(),
            )
            .unwrap(),
    )
}

fn blocks(h: &ThreePoolHarness, pool: Principal, start: u64, length: u64) -> Vec<BlockWithId> {
    let args = vec![GetBlocksArgs {
        start: Nat::from(start),
        length: Nat::from(length),
    }];
    let result: GetBlocksResult = reply(
        h.pic
            .query_call(
                pool,
                Principal::anonymous(),
                "icrc3_get_blocks",
                encode_one(args).unwrap(),
            )
            .unwrap(),
    );
    result.blocks
}

fn install_predecessor(h: &ThreePoolHarness) -> Principal {
    let pool = h.pic.create_canister();
    h.pic.add_cycles(pool, 2_000_000_000_000);
    let init = ThreePoolInitArgs {
        tokens: [
            TokenConfig {
                ledger_id: h.ledgers[0],
                symbol: "icUSD".to_string(),
                decimals: 8,
                precision_mul: 10_000_000_000,
            },
            TokenConfig {
                ledger_id: h.ledgers[1],
                symbol: "ckUSDT".to_string(),
                decimals: 6,
                precision_mul: 1_000_000_000_000,
            },
            TokenConfig {
                ledger_id: h.ledgers[2],
                symbol: "ckUSDC".to_string(),
                decimals: 6,
                precision_mul: 1_000_000_000_000,
            },
        ],
        initial_a: 100,
        swap_fee_bps: 4,
        admin_fee_bps: 5000,
        admin: h.admin,
    };
    h.pic.install_canister(
        pool,
        PREDECESSOR_WASM.to_vec(),
        encode_one(init).unwrap(),
        None,
    );

    for ledger in h.ledgers {
        let approval = ApproveArgs {
            from_subaccount: None,
            spender: Account {
                owner: pool,
                subaccount: None,
            },
            amount: Nat::from(u128::MAX),
            expected_allowance: None,
            expires_at: None,
            fee: None,
            memo: None,
            created_at_time: None,
        };
        let result: Result<Nat, ApproveError> = reply(
            h.pic
                .update_call(
                    ledger,
                    h.user,
                    "icrc2_approve",
                    encode_one(approval).unwrap(),
                )
                .unwrap(),
        );
        result.expect("approve predecessor pool on underlying ledger");
    }
    pool
}

#[test]
fn predecessor_stable_balances_allowances_and_blocks_survive_account_upgrade() {
    assert_eq!(
        format!("{:x}", Sha256::digest(PREDECESSOR_WASM)),
        PREDECESSOR_WASM_SHA256
    );
    let h = deploy_pool_with_liquidity_and_swaps(0);
    let pool = install_predecessor(&h);

    let deposit = vec![
        100_000_000_000_000u128,
        1_000_000_000_000,
        1_000_000_000_000,
    ];
    let receipt: Result<IngressReceiptV1, IngressReceiptErrorV1> = reply(
        h.pic
            .update_call(
                pool,
                h.user,
                "add_liquidity_with_receipt_v1",
                encode_args((serde_bytes::ByteBuf::from(vec![0x61; 32]), deposit, 0u128)).unwrap(),
            )
            .unwrap(),
    );
    receipt.expect("seed predecessor LP balance through its test ingress endpoint");

    let old_allowance = 42_000u64;
    let approve: Result<Nat, ApproveError> = reply(
        h.pic
            .update_call(
                pool,
                h.user,
                "icrc2_approve",
                encode_one(ApproveArgs {
                    from_subaccount: None,
                    spender: Account {
                        owner: h.admin,
                        subaccount: None,
                    },
                    amount: Nat::from(old_allowance),
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
    approve.expect("create predecessor principal-keyed allowance");

    let old_default_balance = account_balance(
        &h,
        pool,
        Account {
            owner: h.user,
            subaccount: None,
        },
    );
    let old_supply: Nat = reply(
        h.pic
            .query_call(
                pool,
                Principal::anonymous(),
                "icrc1_total_supply",
                encode_args(()).unwrap(),
            )
            .unwrap(),
    );
    let old_subaccount_balance = account_balance(
        &h,
        pool,
        Account {
            owner: h.user,
            subaccount: Some([0x77; 32]),
        },
    );
    assert_eq!(
        old_subaccount_balance, old_default_balance,
        "predecessor intentionally ignored subaccounts"
    );
    let old_nondefault_allowance = account_allowance(
        &h,
        pool,
        Account {
            owner: h.user,
            subaccount: Some([0x77; 32]),
        },
        Account {
            owner: h.admin,
            subaccount: Some([0x88; 32]),
        },
    );
    assert_eq!(old_nondefault_allowance.allowance, Nat::from(old_allowance));

    // This predecessor transaction logs the chosen destination subaccount but
    // credits only the principal-keyed balance, so there is no safe historical
    // evidence from which to assign any part of that balance to the subaccount.
    let historical_subaccount = [0x77; 32];
    let old_transfer: Result<Nat, TransferError> = reply(
        h.pic
            .update_call(
                pool,
                h.user,
                "icrc1_transfer",
                encode_one(TransferArg {
                    from_subaccount: None,
                    to: Account {
                        owner: h.user,
                        subaccount: Some(historical_subaccount),
                    },
                    amount: Nat::from(777u64),
                    fee: None,
                    memo: None,
                    created_at_time: None,
                })
                .unwrap(),
            )
            .unwrap(),
    );
    let old_transfer_index: u64 = old_transfer
        .expect("predecessor self-transfer")
        .0
        .try_into()
        .unwrap();
    let historical_block = blocks(&h, pool, old_transfer_index, 1)
        .pop()
        .expect("predecessor transfer block");
    let Icrc3Value::Map(fields) = &historical_block.block else {
        panic!("block map")
    };
    let tx = fields
        .iter()
        .find(|(k, _)| k == "tx")
        .map(|(_, v)| v)
        .expect("tx field");
    let Icrc3Value::Map(tx_fields) = tx else {
        panic!("tx map")
    };
    let destination = tx_fields
        .iter()
        .find(|(k, _)| k == "to")
        .map(|(_, v)| v)
        .expect("to field");
    assert_eq!(
        destination,
        &Icrc3Value::Array(vec![
            Icrc3Value::Blob(h.user.as_slice().to_vec()),
            Icrc3Value::Blob(historical_subaccount.to_vec()),
        ])
    );

    h.pic
        .upgrade_canister(
            pool,
            three_pool_test_endpoints_wasm(),
            encode_args(()).unwrap(),
            None,
        )
        .expect("upgrade predecessor stable maps");

    assert_eq!(
        account_balance(
            &h,
            pool,
            Account {
                owner: h.user,
                subaccount: None
            }
        ),
        old_default_balance
    );
    assert_eq!(
        account_balance(
            &h,
            pool,
            Account {
                owner: h.user,
                subaccount: Some([0; 32])
            }
        ),
        old_default_balance
    );
    assert_eq!(
        account_balance(
            &h,
            pool,
            Account {
                owner: h.user,
                subaccount: Some(historical_subaccount)
            }
        ),
        Nat::from(0u64)
    );
    let new_supply: Nat = reply(
        h.pic
            .query_call(
                pool,
                Principal::anonymous(),
                "icrc1_total_supply",
                encode_args(()).unwrap(),
            )
            .unwrap(),
    );
    assert_eq!(new_supply, old_supply);

    let migrated_allowance = account_allowance(
        &h,
        pool,
        Account {
            owner: h.user,
            subaccount: None,
        },
        Account {
            owner: h.admin,
            subaccount: None,
        },
    );
    assert_eq!(migrated_allowance.allowance, Nat::from(old_allowance));
    assert_eq!(
        account_allowance(
            &h,
            pool,
            Account {
                owner: h.user,
                subaccount: Some(historical_subaccount)
            },
            Account {
                owner: h.admin,
                subaccount: None
            }
        )
        .allowance,
        Nat::from(0u64)
    );
    assert_eq!(
        account_allowance(
            &h,
            pool,
            Account {
                owner: h.user,
                subaccount: None
            },
            Account {
                owner: h.admin,
                subaccount: Some([0x88; 32])
            }
        )
        .allowance,
        Nat::from(0u64)
    );

    let historical_after = blocks(&h, pool, old_transfer_index, 1)
        .pop()
        .expect("historical transfer remains");
    assert_eq!(
        historical_after, historical_block,
        "upgrade preserves the original ICRC-3 account tuple"
    );
}
