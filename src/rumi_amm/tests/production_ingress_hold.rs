//! Release fence for the production AMM Wasm. The feature-enabled PocketIC Wasm
//! exercises recovery internals; this test checks the separately built upload
//! artifact does not expose a half-journaled deposit path.
//!
//! Run explicitly with RUMI_AMM_PRODUCTION_WASM pointing at the source-matched
//! production artifact. This test is ignored in the default suite because that
//! artifact is built separately from the `pocketic-test` fixture Wasm.

use candid::{decode_one, encode_args, encode_one, Principal};
use pocket_ic::{PocketIcBuilder, WasmResult};
use rumi_amm::state::{InboundOperationStatus, OutboundPayout};
use rumi_amm::types::{
    AmmError, AmmInitArgs, AmmLiquidityEvent, AmmSwapEvent, PendingClaim, PoolInfo, SwapResult,
};
use sha2::{Digest, Sha256};

fn reply<T: for<'de> candid::Deserialize<'de> + candid::CandidType>(result: WasmResult) -> T {
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode AMM reply"),
        WasmResult::Reject(reason) => {
            panic!("AMM rejected instead of returning held error: {reason}")
        }
    }
}

fn expect_held<T: for<'de> candid::Deserialize<'de> + candid::CandidType>(
    result: WasmResult,
    expected_reason: &str,
) {
    let outcome: Result<T, AmmError> = reply(result);
    assert!(
        matches!(outcome, Err(AmmError::InvalidInput { reason }) if reason == expected_reason),
        "production ingress must return the specific held error"
    );
}

#[test]
#[ignore = "requires RUMI_AMM_PRODUCTION_WASM from a source-matched production build"]
fn production_wasm_holds_all_unjournaled_swap_and_deposit_ingress() {
    let wasm_path = std::env::var("RUMI_AMM_PRODUCTION_WASM")
        .expect("set RUMI_AMM_PRODUCTION_WASM to the source-matched upload artifact");
    let expected_sha = std::env::var("RUMI_AMM_PRODUCTION_WASM_SHA256")
        .expect("set RUMI_AMM_PRODUCTION_WASM_SHA256 to the reviewed upload hash");
    let wasm = std::fs::read(&wasm_path).expect("read production AMM Wasm");
    let actual_sha = format!("{:x}", Sha256::digest(&wasm));
    assert_eq!(actual_sha, expected_sha, "wrong production AMM artifact");
    let pic = PocketIcBuilder::new().with_application_subnet().build();
    let admin = Principal::self_authenticating(b"amm-release-admin");
    let caller = Principal::self_authenticating(b"amm-release-caller");
    let nonexistent_ledger = Principal::self_authenticating(b"no-ledger-installed");
    let amm = pic.create_canister_with_settings(Some(admin), None);
    pic.add_cycles(amm, 2_000_000_000_000);
    pic.install_canister(
        amm,
        wasm,
        encode_one(AmmInitArgs { admin }).expect("encode init args"),
        Some(admin),
    );

    let pool_id = "unregistered_pool".to_string();
    expect_held::<SwapResult>(
        pic.update_call(
            amm,
            caller,
            "swap",
            encode_args((pool_id.clone(), nonexistent_ledger, 100_u128, 1_u128)).unwrap(),
        )
        .unwrap(),
        "legacy swap is disabled; use swap_v2 with a durable request ID",
    );
    let swap_request = vec![7_u8; 32];
    let add_request = vec![8_u8; 32];
    expect_held::<SwapResult>(
        pic.update_call(
            amm,
            caller,
            "swap_v2",
            encode_args((
                swap_request.clone(),
                pool_id.clone(),
                nonexistent_ledger,
                100_u128,
                1_u128,
            ))
            .unwrap(),
        )
        .unwrap(),
        "swap_v2 is held pending effect-then-error and upgrade lifecycle verification",
    );
    expect_held::<u128>(
        pic.update_call(
            amm,
            caller,
            "add_liquidity",
            encode_args((pool_id.clone(), 100_u128, 100_u128, 1_u128)).unwrap(),
        )
        .unwrap(),
        "legacy add_liquidity is disabled; use add_liquidity_v2 with a durable request ID",
    );
    expect_held::<u128>(
        pic.update_call(
            amm,
            caller,
            "add_liquidity_v2",
            encode_args((add_request.clone(), pool_id, 100_u128, 100_u128, 1_u128)).unwrap(),
        )
        .unwrap(),
        "add_liquidity_v2 remains held until both inbound legs have durable phase replay",
    );

    let pools: Vec<PoolInfo> = reply(
        pic.query_call(amm, caller, "get_pools", encode_args(()).unwrap())
            .unwrap(),
    );
    let claims: Vec<PendingClaim> = reply(
        pic.query_call(amm, caller, "get_pending_claims", encode_args(()).unwrap())
            .unwrap(),
    );
    let swaps: Vec<AmmSwapEvent> = reply(
        pic.query_call(
            amm,
            caller,
            "get_amm_swap_events",
            encode_args((0_u64, 10_u64)).unwrap(),
        )
        .unwrap(),
    );
    let liquidity: Vec<AmmLiquidityEvent> = reply(
        pic.query_call(
            amm,
            caller,
            "get_amm_liquidity_events",
            encode_args((0_u64, 10_u64)).unwrap(),
        )
        .unwrap(),
    );
    for request_id in [swap_request, add_request] {
        let status: Result<InboundOperationStatus, AmmError> = reply(
            pic.query_call(
                amm,
                caller,
                "get_inbound_operation",
                encode_args((request_id,)).unwrap(),
            )
            .unwrap(),
        );
        assert!(
            matches!(status, Err(AmmError::InvalidInput { reason }) if reason == "inbound operation not found"),
            "held ingress must not reserve an inbound journal row"
        );
    }
    let next_sequence: Result<u64, AmmError> = reply(
        pic.query_call(
            amm,
            caller,
            "get_next_inbound_sequence",
            encode_args(()).unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(next_sequence.unwrap(), 1);
    let payouts: Result<Vec<OutboundPayout>, AmmError> = reply(
        pic.query_call(
            amm,
            admin,
            "get_unresolved_outbound_payouts",
            encode_args((0_u64, 100_u64)).unwrap(),
        )
        .unwrap(),
    );
    assert!(payouts.unwrap().is_empty());
    assert!(pools.is_empty() && claims.is_empty() && swaps.is_empty() && liquidity.is_empty());
}
