//! CL-01 regression after upgrading a canister with the pre-Phase-A heap layout.
//!
//! Compatibility fixture provenance: exact old package source commit
//! `bf45fd9fe82f9389b9dfba323bdc788d74db63e6`, immediately before A1
//! (`6d5d664b`). This is not the original production Wasm: it was rebuilt from
//! that exact source using the cached dependency lock with SHA-256
//! `dfa3a4fbc50231569e5276fcd6f405a0c04b317f8692e8e4763a68c3d0fcc7bd`.
//! The historical lock SHA-256 is
//! `73e9cdfbc3e7d3630fbeb3c0c4bb48aa0fa0ff74aada72ae8ec2823a3e702efe`, but
//! its pinned `proc-macro2 1.0.93` was unavailable offline. The compatibility
//! lock resolves cached versions, including `proc-macro2 1.0.106`, `candid
//! 0.10.12`, `ic-cdk 0.12.2`, `ic-cdk-macros 0.8.4`, and `serde 1.0.217`.
//! The old `pre_upgrade` Candid-encodes the full `ThreePoolState`, writing an
//! 8-byte little-endian length at stable offset 0 and the bytes at offset 8.
//! Fixture SHA-256: `701670b8fa0e8bb565cd9a1d173ec9e09bad834643a368a1ad1879b845e80b2a`.
//! New side of this run: the current modified checkout, built with
//! `--features test_endpoints`; Wasm SHA-256
//! `b86a371947c245c22a7e99b30810994bf0e78f00ec5428763d01645b8645f0d4`.

mod common;

use candid::{decode_one, encode_args, encode_one, Nat, Principal};
use common::{
    deploy_pool_with_liquidity_and_swaps, three_pool_test_endpoints_wasm, ThreePoolHarness,
};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
use icrc_ledger_types::icrc2::approve::{ApproveArgs, ApproveError};
use pocket_ic::common::rest::{
    CanisterHttpReply, CanisterHttpRequest, CanisterHttpResponse, MockCanisterHttpResponse,
};
use pocket_ic::WasmResult;
use rumi_3pool::types::{PoolStatus, ThreePoolError, ThreePoolInitArgs, TokenConfig};
use sha2::{Digest, Sha256};

const LP_AMOUNT: u128 = 100_000_000;
const LEGACY_WASM: &[u8] = include_bytes!("fixtures/rumi_3pool_pre_phase_a_bf45fd9f.wasm");
const LEGACY_WASM_SHA256: &str = "701670b8fa0e8bb565cd9a1d173ec9e09bad834643a368a1ad1879b845e80b2a";

fn reply<T: for<'de> candid::Deserialize<'de> + candid::CandidType>(result: WasmResult) -> T {
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode reply"),
        WasmResult::Reject(message) => panic!("call rejected: {message}"),
    }
}

fn lp_transfer(to: Principal, amount: u128) -> Vec<u8> {
    encode_one(TransferArg {
        from_subaccount: None,
        to: Account {
            owner: to,
            subaccount: None,
        },
        fee: None,
        created_at_time: None,
        memo: None,
        amount: Nat::from(amount),
    })
    .unwrap()
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
        result.expect("approve legacy pool on underlying ledger");
    }

    let deposit = vec![
        50_000_000_000_000u128,
        500_000_000_000u128,
        500_000_000_000u128,
    ];
    let minted: Result<u128, ThreePoolError> = reply(
        h.pic
            .update_call(
                pool,
                h.user,
                "add_liquidity",
                encode_args((deposit, 0u128)).unwrap(),
            )
            .unwrap(),
    );
    assert!(minted.expect("seed legacy pool LP") > LP_AMOUNT);
    pool
}

fn lp_balance(h: &ThreePoolHarness, pool: Principal, owner: Principal) -> u128 {
    let balance: Nat = reply(
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
    );
    balance.0.try_into().unwrap()
}

fn total_supply(h: &ThreePoolHarness, pool: Principal) -> u128 {
    let supply: Nat = reply(
        h.pic
            .query_call(
                pool,
                Principal::anonymous(),
                "icrc1_total_supply",
                encode_args(()).unwrap(),
            )
            .unwrap(),
    );
    supply.0.try_into().unwrap()
}

fn admin_fees(h: &ThreePoolHarness, pool: Principal) -> Vec<u128> {
    reply(
        h.pic
            .query_call(
                pool,
                Principal::anonymous(),
                "get_admin_fees",
                encode_args(()).unwrap(),
            )
            .unwrap(),
    )
}

fn pool_reserves(h: &ThreePoolHarness, pool: Principal) -> [u128; 3] {
    let status: PoolStatus = reply(
        h.pic
            .query_call(
                pool,
                Principal::anonymous(),
                "get_pool_status",
                encode_args(()).unwrap(),
            )
            .unwrap(),
    );
    status.balances
}

fn ledger_reserves(h: &ThreePoolHarness, pool: Principal) -> [u128; 3] {
    std::array::from_fn(|index| {
        let balance: Nat = reply(
            h.pic
                .query_call(
                    h.ledgers[index],
                    Principal::anonymous(),
                    "icrc1_balance_of",
                    encode_one(Account {
                        owner: pool,
                        subaccount: None,
                    })
                    .unwrap(),
                )
                .unwrap(),
        );
        balance.0.try_into().unwrap()
    })
}

fn wait_for_fee_gate(h: &ThreePoolHarness) -> CanisterHttpRequest {
    for _ in 0..80 {
        h.pic.tick();
        if let Some(request) = h
            .pic
            .get_canister_http()
            .into_iter()
            .find(|request| request.url == "https://3pool-fee-gate.test/hold")
        {
            return request;
        }
    }
    panic!("withdrawal did not reach the controlled cold fee lookup");
}

#[test]
fn cl01_legacy_heap_upgrade_preserves_lp_then_rechecks_concurrent_transfer() {
    assert_eq!(
        format!("{:x}", Sha256::digest(LEGACY_WASM)),
        LEGACY_WASM_SHA256
    );

    let h = deploy_pool_with_liquidity_and_swaps(0);
    let legacy_pool = install_legacy_pool(&h);
    let withdrawer = Principal::self_authenticating(&[68, 68, 68]);
    let recipient = Principal::self_authenticating(&[79, 79, 79]);

    let swap_result: Result<u128, ThreePoolError> = reply(
        h.pic
            .update_call(
                legacy_pool,
                h.user,
                "swap",
                encode_args((0u8, 1u8, 100_000_000_000u128, 0u128)).unwrap(),
            )
            .unwrap(),
    );
    swap_result.expect("accrue legacy swap admin fee");
    let admin_fees_before_upgrade = admin_fees(&h, legacy_pool);
    assert!(
        admin_fees_before_upgrade.iter().any(|fee| *fee > 0),
        "legacy swap must accrue a nonzero admin fee: {admin_fees_before_upgrade:?}"
    );

    let transfer: Result<Nat, TransferError> = reply(
        h.pic
            .update_call(
                legacy_pool,
                h.user,
                "icrc1_transfer",
                lp_transfer(withdrawer, LP_AMOUNT),
            )
            .unwrap(),
    );
    transfer.expect("seed pre-upgrade withdrawer LP");

    let supply_before_upgrade = total_supply(&h, legacy_pool);
    let reserves_before_upgrade = pool_reserves(&h, legacy_pool);
    h.pic
        .upgrade_canister(
            legacy_pool,
            three_pool_test_endpoints_wasm(),
            encode_args(()).unwrap(),
            None,
        )
        .expect("legacy heap-layout upgrade failed");

    assert_eq!(lp_balance(&h, legacy_pool, withdrawer), LP_AMOUNT);
    assert_eq!(total_supply(&h, legacy_pool), supply_before_upgrade);
    assert_eq!(pool_reserves(&h, legacy_pool), reserves_before_upgrade);
    assert_eq!(
        admin_fees(&h, legacy_pool),
        admin_fees_before_upgrade,
        "populated admin fees must survive the legacy heap upgrade"
    );

    let supply_before_race = total_supply(&h, legacy_pool);
    let reserves_before_race = pool_reserves(&h, legacy_pool);
    let ledgers_before_race = ledger_reserves(&h, legacy_pool);
    let admin_fees_before_race = admin_fees(&h, legacy_pool);
    let _: () = reply(
        h.pic
            .update_call(
                legacy_pool,
                h.admin,
                "test_gate_next_fee_lookup",
                encode_args(()).unwrap(),
            )
            .unwrap(),
    );

    let withdrawal = h
        .pic
        .submit_call(
            legacy_pool,
            withdrawer,
            "remove_liquidity",
            encode_args((LP_AMOUNT, vec![0u128; 3])).unwrap(),
        )
        .unwrap();
    let fee_gate = wait_for_fee_gate(&h);
    let concurrent_transfer = h
        .pic
        .submit_call(
            legacy_pool,
            withdrawer,
            "icrc1_transfer",
            lp_transfer(recipient, LP_AMOUNT),
        )
        .unwrap();
    let transfer_result: Result<Nat, TransferError> =
        reply(h.pic.await_call(concurrent_transfer).unwrap());
    transfer_result.expect("concurrent LP transfer");

    h.pic.mock_canister_http_response(MockCanisterHttpResponse {
        subnet_id: fee_gate.subnet_id,
        request_id: fee_gate.request_id,
        response: CanisterHttpResponse::CanisterHttpReply(CanisterHttpReply {
            status: 200,
            headers: vec![],
            body: b"release".to_vec(),
        }),
        additional_responses: vec![],
    });

    let withdrawal_result: Result<Vec<Nat>, ThreePoolError> =
        reply(h.pic.await_call(withdrawal).unwrap());
    assert!(
        matches!(
            withdrawal_result,
            Err(ThreePoolError::InsufficientLiquidity)
        ),
        "upgraded withdrawal must recheck LP after fee await: {withdrawal_result:?}"
    );
    assert_eq!(lp_balance(&h, legacy_pool, withdrawer), 0);
    assert_eq!(lp_balance(&h, legacy_pool, recipient), LP_AMOUNT);
    assert_eq!(total_supply(&h, legacy_pool), supply_before_race);
    assert_eq!(pool_reserves(&h, legacy_pool), reserves_before_race);
    assert_eq!(ledger_reserves(&h, legacy_pool), ledgers_before_race);
    assert_eq!(
        admin_fees(&h, legacy_pool),
        admin_fees_before_race,
        "failed withdrawal must preserve accrued admin fees"
    );
}
