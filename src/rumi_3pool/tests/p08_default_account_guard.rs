//! PocketIC regression for the 3USD default-account boundary.
mod common;

use candid::{decode_one, encode_args, encode_one, Nat, Principal};
use common::{deploy_pool_with_liquidity_and_swaps, ThreePoolHarness};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
use icrc_ledger_types::icrc2::allowance::AllowanceArgs;
use icrc_ledger_types::icrc2::approve::{ApproveArgs, ApproveError};
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
use pocket_ic::WasmResult;
use rumi_3pool::icrc3::{GetBlocksArgs, GetBlocksResult};
use rumi_3pool::types::{ThreePoolInitArgs, TokenConfig};

const LEGACY_WASM: &[u8] = include_bytes!("fixtures/rumi_3pool_pre_phase_a_bf45fd9f.wasm");

fn reply(result: WasmResult) -> Vec<u8> {
    match result {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("canister rejected: {message}"),
    }
}

fn balance(h: &ThreePoolHarness, account: Account) -> Nat {
    decode_one(&reply(
        h.pic
            .query_call(
                h.three_pool,
                Principal::anonymous(),
                "icrc1_balance_of",
                encode_one(account).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap()
}

fn allowance(h: &ThreePoolHarness, owner: Principal, spender: Principal) -> Nat {
    let result: icrc_ledger_types::icrc2::allowance::Allowance = decode_one(&reply(
        h.pic
            .query_call(
                h.three_pool,
                Principal::anonymous(),
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
    ))
    .unwrap();
    result.allowance
}

fn install_legacy_pool(h: &ThreePoolHarness) -> Principal {
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
    h.pic
        .install_canister(pool, LEGACY_WASM.to_vec(), encode_one(init).unwrap(), None);

    for ledger in h.ledgers {
        let args = ApproveArgs {
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
        let result: Result<Nat, ApproveError> = decode_one(&reply(
            h.pic
                .update_call(ledger, h.user, "icrc2_approve", encode_one(args).unwrap())
                .unwrap(),
        ))
        .unwrap();
        result.expect("approve legacy pool on underlying ledger");
    }

    let amounts = vec![
        50_000_000_000_000u128,
        500_000_000_000u128,
        500_000_000_000u128,
    ];
    let minted: Result<Nat, rumi_3pool::types::ThreePoolError> = decode_one(&reply(
        h.pic
            .update_call(
                pool,
                h.user,
                "add_liquidity",
                encode_args((amounts, 0u128)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    assert!(minted.expect("seed legacy pool 3USD") > Nat::from(1u8));
    pool
}

#[test]
fn nondefault_accounts_are_rejected_before_mutation() {
    let h = deploy_pool_with_liquidity_and_swaps(0);
    let nonzero = [7u8; 32];
    let zero = [0u8; 32];
    let initial = balance(
        &h,
        Account {
            owner: h.user,
            subaccount: None,
        },
    );
    assert!(
        initial > Nat::from(0u8),
        "harness must seed 3USD to the owner"
    );
    assert_eq!(
        balance(
            &h,
            Account {
                owner: h.user,
                subaccount: Some(zero)
            }
        ),
        initial
    );
    assert_eq!(
        balance(
            &h,
            Account {
                owner: h.user,
                subaccount: Some(nonzero)
            }
        ),
        Nat::from(0u8)
    );

    let recipient = Principal::self_authenticating(&[71, 72, 73]);
    let spender = Principal::self_authenticating(&[74, 75, 76]);
    let baseline_log = h.icrc3_log_length();

    for (from_subaccount, to_subaccount) in [(Some(nonzero), None), (None, Some(nonzero))] {
        let args = TransferArg {
            from_subaccount,
            to: Account {
                owner: recipient,
                subaccount: to_subaccount,
            },
            amount: Nat::from(10u8),
            fee: None,
            memo: None,
            created_at_time: None,
        };
        let result: Result<Nat, TransferError> = decode_one(&reply(
            h.pic
                .update_call(
                    h.three_pool,
                    h.user,
                    "icrc1_transfer",
                    encode_one(args).unwrap(),
                )
                .unwrap(),
        ))
        .unwrap();
        assert!(
            matches!(result, Err(TransferError::GenericError { .. })),
            "ICRC-1 nondefault account must fail: {result:?}"
        );
        assert_eq!(h.icrc3_log_length(), baseline_log);
        assert_eq!(
            balance(
                &h,
                Account {
                    owner: h.user,
                    subaccount: None
                }
            ),
            initial
        );
    }

    let approve_default = ApproveArgs {
        from_subaccount: None,
        spender: Account {
            owner: spender,
            subaccount: None,
        },
        amount: Nat::from(50u8),
        expected_allowance: None,
        expires_at: None,
        fee: None,
        memo: None,
        created_at_time: None,
    };
    let approved: Result<Nat, ApproveError> = decode_one(&reply(
        h.pic
            .update_call(
                h.three_pool,
                h.user,
                "icrc2_approve",
                encode_one(approve_default.clone()).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    approved.expect("seed allowance for transfer_from rejection checks");
    let approved_log = h.icrc3_log_length();
    let approved_amount = allowance(&h, h.user, spender);
    assert_eq!(approved_amount, Nat::from(50u8));

    for args in [
        ApproveArgs {
            from_subaccount: Some(nonzero),
            ..approve_default.clone()
        },
        ApproveArgs {
            spender: Account {
                owner: spender,
                subaccount: Some(nonzero),
            },
            ..approve_default.clone()
        },
    ] {
        let result: Result<Nat, ApproveError> = decode_one(&reply(
            h.pic
                .update_call(
                    h.three_pool,
                    h.user,
                    "icrc2_approve",
                    encode_one(args).unwrap(),
                )
                .unwrap(),
        ))
        .unwrap();
        assert!(
            matches!(result, Err(ApproveError::GenericError { .. })),
            "ICRC-2 approve nondefault account must fail: {result:?}"
        );
        assert_eq!(h.icrc3_log_length(), approved_log);
        assert_eq!(allowance(&h, h.user, spender), approved_amount);
    }

    for (from_subaccount, to_subaccount, spender_subaccount) in [
        (Some(nonzero), None, None),
        (None, Some(nonzero), None),
        (None, None, Some(nonzero)),
    ] {
        let args = TransferFromArgs {
            spender_subaccount,
            from: Account {
                owner: h.user,
                subaccount: from_subaccount,
            },
            to: Account {
                owner: recipient,
                subaccount: to_subaccount,
            },
            amount: Nat::from(10u8),
            fee: None,
            memo: None,
            created_at_time: None,
        };
        let result: Result<Nat, TransferFromError> = decode_one(&reply(
            h.pic
                .update_call(
                    h.three_pool,
                    spender,
                    "icrc2_transfer_from",
                    encode_one(args).unwrap(),
                )
                .unwrap(),
        ))
        .unwrap();
        assert!(
            matches!(result, Err(TransferFromError::GenericError { .. })),
            "ICRC-2 transfer_from nondefault account must fail: {result:?}"
        );
        assert_eq!(h.icrc3_log_length(), approved_log);
        assert_eq!(allowance(&h, h.user, spender), approved_amount);
        assert_eq!(
            balance(
                &h,
                Account {
                    owner: h.user,
                    subaccount: None
                }
            ),
            initial
        );
    }

    let transfer: Result<Nat, TransferError> = decode_one(&reply(
        h.pic
            .update_call(
                h.three_pool,
                h.user,
                "icrc1_transfer",
                encode_one(TransferArg {
                    from_subaccount: None,
                    to: Account {
                        owner: recipient,
                        subaccount: None,
                    },
                    amount: Nat::from(10u8),
                    fee: None,
                    memo: None,
                    created_at_time: None,
                })
                .unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    transfer.expect("default-account transfer should continue to work");
    assert_eq!(
        balance(
            &h,
            Account {
                owner: h.user,
                subaccount: None
            }
        ),
        initial - Nat::from(10u8)
    );
    assert_eq!(
        balance(
            &h,
            Account {
                owner: recipient,
                subaccount: None
            }
        ),
        Nat::from(10u8)
    );
    assert_eq!(h.icrc3_log_length(), approved_log + 1);
}

#[test]
fn legacy_nondefault_allowance_is_invalidated_once_across_upgrade() {
    let h = deploy_pool_with_liquidity_and_swaps(0);
    let legacy_pool = install_legacy_pool(&h);
    let owner = h.user;
    let spender = Principal::self_authenticating(&[81, 82, 83]);
    let nonzero = [9u8; 32];
    let legacy_amount = Nat::from(123u64);

    let old_approval: Result<Nat, ApproveError> = decode_one(&reply(
        h.pic
            .update_call(
                legacy_pool,
                owner,
                "icrc2_approve",
                encode_one(ApproveArgs {
                    from_subaccount: Some(nonzero),
                    spender: Account {
                        owner: spender,
                        subaccount: None,
                    },
                    amount: legacy_amount.clone(),
                    expected_allowance: None,
                    expires_at: None,
                    fee: None,
                    memo: None,
                    created_at_time: None,
                })
                .unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    old_approval.expect("legacy implementation accepts principal-keyed subaccount approval");

    h.pic
        .upgrade_canister(
            legacy_pool,
            common::three_pool_wasm(),
            encode_args(()).unwrap(),
            None,
        )
        .expect("upgrade from the pinned legacy fixture");
    assert_eq!(
        allowance_for_pool(&h, legacy_pool, owner, spender),
        Nat::from(0u8)
    );

    let balance_before = balance_for_pool(&h, legacy_pool, owner);
    let recipient = Principal::self_authenticating(&[84, 85, 86]);
    let log_before = log_length_for_pool(&h, legacy_pool);
    let legacy_grant_spend: Result<Nat, TransferFromError> = decode_one(&reply(
        h.pic
            .update_call(
                legacy_pool,
                spender,
                "icrc2_transfer_from",
                encode_one(TransferFromArgs {
                    spender_subaccount: None,
                    from: Account {
                        owner,
                        subaccount: None,
                    },
                    to: Account {
                        owner: recipient,
                        subaccount: None,
                    },
                    amount: Nat::from(1u8),
                    fee: None,
                    memo: None,
                    created_at_time: None,
                })
                .unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    assert!(
        matches!(legacy_grant_spend, Err(TransferFromError::InsufficientAllowance { ref allowance }) if *allowance == Nat::from(0u8)),
        "invalidated principal-keyed grant must be unspendable: {legacy_grant_spend:?}"
    );
    assert_eq!(balance_for_pool(&h, legacy_pool, owner), balance_before);
    assert_eq!(balance_for_pool(&h, legacy_pool, recipient), Nat::from(0u8));
    assert_eq!(log_length_for_pool(&h, legacy_pool), log_before);

    let new_approval: Result<Nat, ApproveError> = decode_one(&reply(
        h.pic
            .update_call(
                legacy_pool,
                owner,
                "icrc2_approve",
                encode_one(ApproveArgs {
                    from_subaccount: None,
                    spender: Account {
                        owner: spender,
                        subaccount: None,
                    },
                    amount: Nat::from(456u64),
                    expected_allowance: None,
                    expires_at: None,
                    fee: None,
                    memo: None,
                    created_at_time: None,
                })
                .unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    new_approval.expect("default-account approval after cutover");

    h.pic
        .upgrade_canister(
            legacy_pool,
            common::three_pool_wasm(),
            encode_args(()).unwrap(),
            None,
        )
        .expect("second upgrade after allowance cutover");
    assert_eq!(
        allowance_for_pool(&h, legacy_pool, owner, spender),
        Nat::from(456u64),
        "the one-time legacy invalidation must preserve approvals created after cutover"
    );
}

fn allowance_for_pool(
    h: &ThreePoolHarness,
    pool: Principal,
    owner: Principal,
    spender: Principal,
) -> Nat {
    let result: icrc_ledger_types::icrc2::allowance::Allowance = decode_one(&reply(
        h.pic
            .query_call(
                pool,
                Principal::anonymous(),
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
    ))
    .unwrap();
    result.allowance
}

fn balance_for_pool(h: &ThreePoolHarness, pool: Principal, owner: Principal) -> Nat {
    decode_one(&reply(
        h.pic
            .query_call(
                pool,
                Principal::anonymous(),
                "icrc1_balance_of",
                encode_one(Account {
                    owner,
                    subaccount: None,
                })
                .unwrap(),
            )
            .unwrap(),
    ))
    .unwrap()
}

fn log_length_for_pool(h: &ThreePoolHarness, pool: Principal) -> u64 {
    let result: GetBlocksResult = decode_one(&reply(
        h.pic
            .query_call(
                pool,
                Principal::anonymous(),
                "icrc3_get_blocks",
                encode_one(vec![GetBlocksArgs {
                    start: Nat::from(0u8),
                    length: Nat::from(0u8),
                }])
                .unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    result.log_length.0.try_into().unwrap()
}
