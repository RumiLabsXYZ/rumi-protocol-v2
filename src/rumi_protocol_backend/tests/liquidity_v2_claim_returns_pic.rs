//! PocketIC recovery test for a committed native-ICP ClaimReturns payout whose
//! backend callback traps before it can consume the ledger reply.

#![cfg(feature = "liquidity-returns-pic-test")]

include!("common/bot_claim_fixture.rs");

use rumi_protocol_backend::{
    InitArg, LiquidityStatus, LiquidityV2Kind, LiquidityV2Phase, LiquidityV2RequestState,
    ProtocolArg, UpgradeArg,
};

const CLAIM_AMOUNT: u64 = 25_000_000;
const ICP_FEE: u64 = 10_000;

#[derive(CandidType, Deserialize)]
struct NativeQueryBlocksHead {
    chain_length: u64,
}

fn liquidity_test_backend_wasm() -> Vec<u8> {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target")
        });
    std::fs::read(
        target
            .join("wasm32-unknown-unknown/release")
            .join("rumi_protocol_backend_liquidity_returns_test.wasm"),
    )
    .expect("read feature-gated ClaimReturns backend Wasm")
}

fn status(pic: &PocketIc, backend: Principal, owner: Principal) -> LiquidityStatus {
    let result = pic
        .query_call(
            backend,
            owner,
            "get_liquidity_status",
            encode_one(owner).unwrap(),
        )
        .expect("get liquidity status");
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode liquidity status"),
        WasmResult::Reject(message) => panic!("liquidity status rejected: {message}"),
    }
}

fn request_state(pic: &PocketIc, backend: Principal, owner: Principal) -> LiquidityV2RequestState {
    let result = pic
        .query_call(
            backend,
            owner,
            "get_my_liquidity_v2_request_state",
            encode_args(()).unwrap(),
        )
        .expect("get ClaimReturns request state");
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode request state"),
        WasmResult::Reject(message) => panic!("request-state query rejected: {message}"),
    }
}

#[test]
fn claim_returns_recovers_native_icp_payout_after_callback_trap_and_upgrade() {
    let pic = PocketIcBuilder::new().with_nns_subnet().build();
    let owner = Principal::self_authenticating(b"liquidity-claim-returns-owner");
    let developer = Principal::self_authenticating(b"liquidity-claim-returns-developer");
    pic.set_time(SystemTime::UNIX_EPOCH + Duration::from_secs(1_711_324_800));

    let backend = pic.create_canister();
    pic.add_cycles(backend, 2_000_000_000_000);
    let icp_ledger =
        deploy_native_icp_ledger(&pic, backend, CLAIM_AMOUNT + ICP_FEE, developer, 1_000);
    let xrc = pic.create_canister();
    pic.add_cycles(xrc, 1_000_000_000_000);
    pic.install_canister(xrc, xrc_wasm(), prepare_mock_xrc(), None);

    pic.install_canister(
        backend,
        liquidity_test_backend_wasm(),
        encode_one(ProtocolArg::Init(InitArg {
            xrc_principal: xrc,
            icusd_ledger_principal: icp_ledger,
            icp_ledger_principal: icp_ledger,
            fee_e8s: ICP_FEE,
            developer_principal: developer,
            treasury_principal: None,
            stability_pool_principal: None,
            ckusdt_ledger_principal: None,
            ckusdc_ledger_principal: None,
        }))
        .unwrap(),
        None,
    );

    let seed_result: Result<(), ProtocolError> = decode_one(&match pic
        .update_call(
            backend,
            developer,
            "test_seed_liquidity_return_and_trap",
            encode_args((owner, CLAIM_AMOUNT)).unwrap(),
        )
        .expect("test-only reward seed call")
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("reward seed rejected: {message}"),
    })
    .expect("decode reward seed result");
    seed_result.expect("developer-only test seed succeeds");
    assert_eq!(
        status(&pic, backend, owner).available_liquidity_reward,
        CLAIM_AMOUNT
    );
    assert_eq!(
        icrc1_balance_of_call(&pic, icp_ledger, backend),
        CLAIM_AMOUNT + ICP_FEE
    );
    assert_eq!(icrc1_balance_of_call(&pic, icp_ledger, owner), 0);
    let history_head = pic
        .query_call(
            icp_ledger,
            Principal::anonymous(),
            "query_blocks",
            encode_one(NativeGetBlocksArgs {
                start: 0,
                length: 0,
            })
            .unwrap(),
        )
        .expect("read native ICP history head before payout");
    let expected_block_index = match history_head {
        WasmResult::Reply(bytes) => {
            decode_one::<NativeQueryBlocksHead>(&bytes)
                .expect("decode native ICP history head")
                .chain_length
        }
        WasmResult::Reject(message) => panic!("native ICP history head rejected: {message}"),
    };

    // The NNS ledger commits this real native-ICP transfer. The feature-only
    // backend hook then traps at the callback before the returned block index
    // can be recorded. The update therefore appears lost to the owner while
    // the exact stable request and on-ledger payout both remain observable.
    let first = pic.update_call(
        backend,
        owner,
        "claim_liquidity_returns_v2",
        encode_one(1u128).unwrap(),
    );
    assert!(
        !matches!(first, Ok(WasmResult::Reply(_))),
        "expected callback trap after committed ICP payout, got {first:?}"
    );
    assert_eq!(icrc1_balance_of_call(&pic, icp_ledger, backend), 0);
    assert_eq!(icrc1_balance_of_call(&pic, icp_ledger, owner), CLAIM_AMOUNT);
    assert_eq!(
        status(&pic, backend, owner).available_liquidity_reward,
        CLAIM_AMOUNT
    );
    let held = request_state(&pic, backend, owner);
    assert_eq!(held.next_request_id, 2);
    let active = held
        .active_request
        .expect("request retained after callback trap");
    assert_eq!(active.request_id, 1);
    assert_eq!(active.kind, LiquidityV2Kind::ClaimReturns);
    assert_eq!(active.amount_raw, CLAIM_AMOUNT);
    assert_eq!(active.ledger, icp_ledger);
    assert_eq!(active.phase, LiquidityV2Phase::Pending);
    assert!(active.had_ambiguous_attempt);

    pic.upgrade_canister(
        backend,
        liquidity_test_backend_wasm(),
        encode_args((ProtocolArg::Upgrade(UpgradeArg {
            mode: None,
            description: Some("ClaimReturns native ICP callback recovery".into()),
        }),))
        .unwrap(),
        None,
    )
    .expect("upgrade while ClaimReturns is unresolved");

    let recovered: Result<u64, ProtocolError> = decode_one(&match pic
        .update_call(
            backend,
            owner,
            "claim_liquidity_returns_v2",
            encode_one(1u128).unwrap(),
        )
        .expect("same-ID recovery call")
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("same-ID recovery rejected: {message}"),
    })
    .expect("decode recovery result");
    let block_index = recovered.expect("exact native ICP history recovers payout");
    assert_eq!(block_index, expected_block_index);
    assert_eq!(icrc1_balance_of_call(&pic, icp_ledger, backend), 0);
    assert_eq!(icrc1_balance_of_call(&pic, icp_ledger, owner), CLAIM_AMOUNT);
    assert_eq!(status(&pic, backend, owner).available_liquidity_reward, 0);

    let completed = request_state(&pic, backend, owner);
    assert_eq!(completed.next_request_id, 2);
    assert!(completed.active_request.is_none());
    let latest = completed
        .latest_result
        .expect("terminal ClaimReturns result");
    assert_eq!(latest.kind, LiquidityV2Kind::ClaimReturns);
    assert_eq!(latest.phase, LiquidityV2Phase::Complete);
    assert_eq!(latest.result_block_index, Some(block_index));
    assert!(latest.had_ambiguous_attempt);

    let replay: Result<u64, ProtocolError> = decode_one(&match pic
        .update_call(
            backend,
            owner,
            "claim_liquidity_returns_v2",
            encode_one(1u128).unwrap(),
        )
        .expect("terminal replay call")
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("terminal replay rejected: {message}"),
    })
    .expect("decode terminal replay");
    assert_eq!(
        replay.expect("terminal replay returns original block"),
        block_index
    );
    assert_eq!(icrc1_balance_of_call(&pic, icp_ledger, backend), 0);
    assert_eq!(icrc1_balance_of_call(&pic, icp_ledger, owner), CLAIM_AMOUNT);
    assert_eq!(status(&pic, backend, owner).available_liquidity_reward, 0);
}
