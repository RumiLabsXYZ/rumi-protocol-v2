//! CL-01: a cold fee await must not commit a stale LP balance.

mod common;

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use common::{deploy_pool_with_liquidity_and_swaps, ThreePoolHarness};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
use icrc_ledger_types::icrc2::approve::{ApproveArgs, ApproveError};
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
use pocket_ic::common::rest::{
    CanisterHttpReply, CanisterHttpRequest, CanisterHttpResponse, MockCanisterHttpResponse,
};
use pocket_ic::WasmResult;
use rumi_3pool::types::{PoolStatus, ThreePoolError};

const LP_AMOUNT: u128 = 100_000_000;

fn reply<T: for<'de> Deserialize<'de> + CandidType>(result: WasmResult) -> T {
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode reply"),
        WasmResult::Reject(message) => panic!("call rejected: {message}"),
    }
}

fn lp_transfer(to: Principal, amount: u128) -> Vec<u8> {
    encode_one(TransferArg {
        from_subaccount: None,
        to: Account { owner: to, subaccount: None },
        fee: None,
        created_at_time: None,
        memo: None,
        amount: Nat::from(amount),
    }).unwrap()
}

fn seed_withdrawer(h: &ThreePoolHarness, withdrawer: Principal) {
    let result: Result<Nat, TransferError> = reply(h.pic.update_call(
        h.three_pool,
        h.user,
        "icrc1_transfer",
        lp_transfer(withdrawer, LP_AMOUNT),
    ).unwrap());
    result.expect("seed withdrawer LP");
}

fn lp_balance(h: &ThreePoolHarness, owner: Principal) -> u128 {
    let balance: Nat = reply(h.pic.query_call(
        h.three_pool,
        Principal::anonymous(),
        "icrc1_balance_of",
        encode_one(Account { owner, subaccount: None }).unwrap(),
    ).unwrap());
    balance.0.try_into().unwrap()
}

fn total_supply(h: &ThreePoolHarness) -> u128 {
    let supply: Nat = reply(h.pic.query_call(
        h.three_pool,
        Principal::anonymous(),
        "icrc1_total_supply",
        encode_args(()).unwrap(),
    ).unwrap());
    supply.0.try_into().unwrap()
}

fn pool_reserves(h: &ThreePoolHarness) -> [u128; 3] {
    let status: PoolStatus = reply(h.pic.query_call(
        h.three_pool,
        Principal::anonymous(),
        "get_pool_status",
        encode_args(()).unwrap(),
    ).unwrap());
    status.balances
}

fn ledger_reserves(h: &ThreePoolHarness) -> [u128; 3] {
    std::array::from_fn(|index| {
        let balance: Nat = reply(h.pic.query_call(
            h.ledgers[index],
            Principal::anonymous(),
            "icrc1_balance_of",
            encode_one(Account { owner: h.three_pool, subaccount: None }).unwrap(),
        ).unwrap());
        balance.0.try_into().unwrap()
    })
}

fn arm_fee_gate(h: &ThreePoolHarness) {
    let _: () = reply(h.pic.update_call(
        h.three_pool,
        h.admin,
        "test_gate_next_fee_lookup",
        encode_args(()).unwrap(),
    ).unwrap());
}

fn wait_for_fee_gate(h: &ThreePoolHarness) -> CanisterHttpRequest {
    for _ in 0..80 {
        h.pic.tick();
        if let Some(request) = h.pic.get_canister_http().into_iter()
            .find(|request| request.url == "https://3pool-fee-gate.test/hold") {
            return request;
        }
    }
    panic!("withdrawal did not reach the controlled cold fee-lookup await");
}

fn release_fee_gate(h: &ThreePoolHarness, request: CanisterHttpRequest) {
    h.pic.mock_canister_http_response(MockCanisterHttpResponse {
        subnet_id: request.subnet_id,
        request_id: request.request_id,
        response: CanisterHttpResponse::CanisterHttpReply(CanisterHttpReply {
            status: 200,
            headers: vec![],
            body: b"release".to_vec(),
        }),
        additional_responses: vec![],
    });
}

fn cold_race(
    h: &ThreePoolHarness,
    withdrawer: Principal,
    recipient: Principal,
    withdrawal_method: &str,
    withdrawal_args: Vec<u8>,
    transfer_method: &str,
    transfer_caller: Principal,
    transfer_args: Vec<u8>,
    decode_withdrawal: impl FnOnce(WasmResult) -> Result<(), ThreePoolError>,
    decode_transfer: impl FnOnce(WasmResult) -> bool,
) {
    let supply_before = total_supply(h);
    let reserves_before = pool_reserves(h);
    let ledgers_before = ledger_reserves(h);

    arm_fee_gate(h);
    let withdrawal = h.pic.submit_call(
        h.three_pool, withdrawer, withdrawal_method, withdrawal_args,
    ).unwrap();
    let gate = wait_for_fee_gate(h);
    let transfer = h.pic.submit_call(
        h.three_pool, transfer_caller, transfer_method, transfer_args,
    ).unwrap();

    assert!(decode_transfer(h.pic.await_call(transfer).unwrap()), "LP transfer failed");
    release_fee_gate(h, gate);
    let result = decode_withdrawal(h.pic.await_call(withdrawal).unwrap());
    assert!(
        matches!(result, Err(ThreePoolError::InsufficientLiquidity)),
        "withdrawal must recheck LP after its fee await; got {result:?}"
    );
    assert_eq!(lp_balance(h, withdrawer), 0);
    assert_eq!(lp_balance(h, recipient), LP_AMOUNT);
    assert_eq!(total_supply(h), supply_before, "failed withdrawal changed LP supply");
    assert_eq!(pool_reserves(h), reserves_before, "failed withdrawal changed tracked reserves");
    assert_eq!(ledger_reserves(h), ledgers_before, "failed withdrawal paid reserve tokens");
}

#[test]
fn cl_01_cold_proportional_withdrawal_rechecks_after_owner_transfer() {
    let h = deploy_pool_with_liquidity_and_swaps(0);
    let withdrawer = Principal::self_authenticating(&[68, 68, 68]);
    let recipient = Principal::self_authenticating(&[79, 79, 79]);
    seed_withdrawer(&h, withdrawer);

    cold_race(
        &h,
        withdrawer,
        recipient,
        "remove_liquidity",
        encode_args((LP_AMOUNT, vec![0u128; 3])).unwrap(),
        "icrc1_transfer",
        withdrawer,
        lp_transfer(recipient, LP_AMOUNT),
        |result| {
            let result: Result<Vec<Nat>, ThreePoolError> = reply(result);
            result.map(|_| ())
        },
        |result| {
            let result: Result<Nat, TransferError> = reply(result);
            result.is_ok()
        },
    );
}

#[test]
fn cl_01_cold_one_coin_withdrawal_rechecks_after_owner_transfer() {
    let h = deploy_pool_with_liquidity_and_swaps(0);
    let withdrawer = Principal::self_authenticating(&[28, 28, 28]);
    let recipient = Principal::self_authenticating(&[48, 48, 48]);
    seed_withdrawer(&h, withdrawer);

    cold_race(
        &h,
        withdrawer,
        recipient,
        "remove_one_coin",
        encode_args((LP_AMOUNT, 0u8, 0u128)).unwrap(),
        "icrc1_transfer",
        withdrawer,
        lp_transfer(recipient, LP_AMOUNT),
        |result| {
            let result: Result<Nat, ThreePoolError> = reply(result);
            result.map(|_| ())
        },
        |result| {
            let result: Result<Nat, TransferError> = reply(result);
            result.is_ok()
        },
    );
}

#[test]
fn cl_01_approved_spender_transfer_during_cold_fee_await_is_safe() {
    let h = deploy_pool_with_liquidity_and_swaps(0);
    let withdrawer = Principal::self_authenticating(&[67, 67, 67]);
    let spender = Principal::self_authenticating(&[78, 78, 78]);
    seed_withdrawer(&h, withdrawer);
    let approval: Result<Nat, ApproveError> = reply(h.pic.update_call(
        h.three_pool,
        withdrawer,
        "icrc2_approve",
        encode_one(ApproveArgs {
            from_subaccount: None,
            spender: Account { owner: spender, subaccount: None },
            amount: Nat::from(LP_AMOUNT),
            expected_allowance: None,
            expires_at: None,
            fee: None,
            memo: None,
            created_at_time: None,
        }).unwrap(),
    ).unwrap());
    approval.expect("approve spender");

    let transfer_from = encode_one(TransferFromArgs {
        spender_subaccount: None,
        from: Account { owner: withdrawer, subaccount: None },
        to: Account { owner: spender, subaccount: None },
        amount: Nat::from(LP_AMOUNT),
        fee: Some(Nat::from(0u64)),
        memo: None,
        created_at_time: None,
    }).unwrap();
    cold_race(
        &h,
        withdrawer,
        spender,
        "remove_one_coin",
        encode_args((LP_AMOUNT, 1u8, 0u128)).unwrap(),
        "icrc2_transfer_from",
        spender,
        transfer_from,
        |result| {
            let result: Result<Nat, ThreePoolError> = reply(result);
            result.map(|_| ())
        },
        |result| {
            let result: Result<Nat, TransferFromError> = reply(result);
            result.is_ok()
        },
    );
}

#[test]
fn cl_01_warm_fee_concurrent_withdrawal_and_transfer_preserve_lp_accounting() {
    let h = deploy_pool_with_liquidity_and_swaps(0);
    let withdrawer = Principal::self_authenticating(&[69, 69, 69]);
    let recipient = Principal::self_authenticating(&[80, 80, 80]);

    let warm: Result<Nat, ThreePoolError> = reply(h.pic.update_call(
        h.three_pool,
        h.user,
        "remove_one_coin",
        encode_args((10_000_000u128, 0u8, 0u128)).unwrap(),
    ).unwrap());
    warm.expect("warm outbound ledger fee cache");
    seed_withdrawer(&h, withdrawer);
    let supply_before = total_supply(&h);

    // On the warm path the cached fee does not suspend the withdrawal. PocketIC
    // may therefore execute either submitted message first; each serial order
    // must preserve LP supply and ownership accounting.
    let withdrawal = h.pic.submit_call(
        h.three_pool,
        withdrawer,
        "remove_one_coin",
        encode_args((LP_AMOUNT, 0u8, 0u128)).unwrap(),
    ).unwrap();
    let transfer = h.pic.submit_call(
        h.three_pool,
        withdrawer,
        "icrc1_transfer",
        lp_transfer(recipient, LP_AMOUNT),
    ).unwrap();
    let transfer_result: Result<Nat, TransferError> = reply(h.pic.await_call(transfer).unwrap());
    let withdrawal_result: Result<Nat, ThreePoolError> = reply(h.pic.await_call(withdrawal).unwrap());
    match (withdrawal_result, transfer_result) {
        (Ok(_), Err(TransferError::InsufficientFunds { .. })) => {
            assert_eq!(lp_balance(&h, withdrawer), 0);
            assert_eq!(lp_balance(&h, recipient), 0);
            assert_eq!(total_supply(&h), supply_before - LP_AMOUNT);
        }
        (Err(ThreePoolError::InsufficientLiquidity), Ok(_)) => {
            assert_eq!(lp_balance(&h, withdrawer), 0);
            assert_eq!(lp_balance(&h, recipient), LP_AMOUNT);
            assert_eq!(total_supply(&h), supply_before);
        }
        (withdrawal, transfer) => panic!(
            "warm-cache calls must resolve in one valid serial order; withdrawal={withdrawal:?}, transfer={transfer:?}"
        ),
    }
}
