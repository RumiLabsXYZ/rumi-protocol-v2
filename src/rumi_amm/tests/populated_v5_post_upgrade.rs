//! Exercises the release Wasm's real `post_upgrade` path against a frozen,
//! populated V5 snapshot. The predecessor module deliberately has no
//! `pre_upgrade` hook, so it cannot overwrite the injected snapshot.
//!
//! This is an opt-in release test. Set both environment variables to the
//! source-matched production artifact and its reviewed SHA-256 before running.

use candid::{decode_one, encode_args, encode_one, CandidType, Principal};
use pocket_ic::{PocketIcBuilder, WasmResult};
use rumi_amm::state::OutboundPayout;
use rumi_amm::types::*;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, VecDeque};

/// Frozen pre-journal state shape used by the deployed V5 stable snapshot.
#[derive(CandidType)]
struct LegacyAmmStateV5 {
    admin: Principal,
    pools: BTreeMap<PoolId, Pool>,
    pool_creation_open: bool,
    maintenance_mode: bool,
    pending_claims: Vec<PendingClaim>,
    next_claim_id: u64,
    swap_events: Vec<AmmSwapEvent>,
    next_swap_event_id: u64,
    liquidity_events: Vec<AmmLiquidityEvent>,
    next_liquidity_event_id: u64,
    admin_events: Vec<AmmAdminEvent>,
    next_admin_event_id: u64,
    holder_snapshots: Vec<HolderSnapshot>,
    reward_events: Vec<AmmRewardEvent>,
    next_reward_event_id: u64,
    claim_events: Vec<AmmClaimEvent>,
    next_claim_event_id: u64,
    protocol_backend_principal: Option<Principal>,
    tvl_samples: Vec<TvlSample>,
}

fn reply<T: for<'de> Deserialize<'de> + CandidType>(result: WasmResult) -> T {
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode AMM reply"),
        WasmResult::Reject(reason) => panic!("AMM rejected instead of replying: {reason}"),
    }
}

fn expect_held<T: for<'de> Deserialize<'de> + CandidType>(result: WasmResult, expected: &str) {
    let result: Result<T, AmmError> = reply(result);
    assert!(
        matches!(result, Err(AmmError::InvalidInput { reason }) if reason == expected),
        "production ingress must return its specific held error"
    );
}

fn snapshot() -> (LegacyAmmStateV5, Principal, Principal, String) {
    let admin = Principal::from_text("aaaaa-aa").expect("management principal");
    let lp = Principal::self_authenticating(b"v5-snapshot-lp-a");
    let second_lp = Principal::self_authenticating(b"v5-snapshot-lp-b");
    let token_a = Principal::self_authenticating(b"v5-snapshot-token-a");
    let token_b = Principal::self_authenticating(b"v5-snapshot-token-b");
    let pool_id = "3USD_ICP".to_string();

    let mut lp_shares = BTreeMap::new();
    lp_shares.insert(lp, 60_000);
    lp_shares.insert(second_lp, 40_000);
    let mut lp_rewards = BTreeMap::new();
    lp_rewards.insert(
        lp,
        RewardState {
            reward_debt: 7_500,
            claimable: 125,
        },
    );
    lp_rewards.insert(
        second_lp,
        RewardState {
            reward_debt: 5_000,
            claimable: 75,
        },
    );
    let mut processed_donation_nonces = VecDeque::new();
    processed_donation_nonces.extend([901, 902]);
    let mut pools = BTreeMap::new();
    pools.insert(
        pool_id.clone(),
        Pool {
            token_a,
            token_b,
            reserve_a: 12_345_678,
            reserve_b: 87_654_321,
            fee_bps: 30,
            protocol_fee_bps: 100,
            curve: CurveType::ConstantProduct,
            lp_shares,
            total_lp_shares: 100_000,
            protocol_fees_a: 321,
            protocol_fees_b: 654,
            paused: true,
            subaccount_a: [0x31; 32],
            subaccount_b: [0x32; 32],
            lp_rewards,
            acc_reward_per_share: 125_000_000_000,
            pending_no_lp: 23,
            total_rewards_distributed: 12_500,
            processed_donation_nonces,
            reward_balance_snapshot: 6_789,
        },
    );
    let swap_events = vec![AmmSwapEvent {
        id: 40,
        caller: lp,
        pool_id: pool_id.clone(),
        token_in: token_a,
        amount_in: 1_001,
        token_out: token_b,
        amount_out: 997,
        fee: 3,
        timestamp: 10_000,
    }];
    let liquidity_events = vec![AmmLiquidityEvent {
        id: 41,
        caller: lp,
        pool_id: pool_id.clone(),
        action: AmmLiquidityAction::AddLiquidity,
        token_a,
        amount_a: 12_000,
        token_b,
        amount_b: 24_000,
        lp_shares: 8_000,
        timestamp: 10_001,
    }];
    let admin_events = vec![AmmAdminEvent {
        id: 42,
        caller: admin,
        action: AmmAdminAction::PausePool {
            pool_id: pool_id.clone(),
        },
        timestamp: 10_002,
    }];
    let holder_snapshots = vec![HolderSnapshot {
        token: "icUSD".to_string(),
        timestamp: 10_003,
        holder_count: 2,
        total_supply: 5_000_000,
        top_holders: vec![
            HolderEntry {
                holder: lp,
                balance: 3_000_000,
            },
            HolderEntry {
                holder: second_lp,
                balance: 2_000_000,
            },
        ],
    }];
    let reward_events = vec![AmmRewardEvent {
        id: 43,
        pool_id: pool_id.clone(),
        amount: 12_500,
        total_shares_at_time: 100_000,
        nonce: 902,
        timestamp: 10_004,
    }];
    let claim_events = vec![AmmClaimEvent {
        id: 44,
        pool_id: pool_id.clone(),
        claimant: lp,
        amount: 125,
        timestamp: 10_005,
    }];
    let tvl_samples = vec![TvlSample {
        pool_id: pool_id.clone(),
        timestamp: 10_006,
        reserve_a: 12_345_678,
        reserve_b: 87_654_321,
        price_a_e8s: 100_000_000,
        price_b_e8s: 25_000_000,
        tvl_usd_e8s: 34_259_258,
    }];

    (
        LegacyAmmStateV5 {
            admin,
            pools,
            pool_creation_open: true,
            maintenance_mode: true,
            pending_claims: vec![PendingClaim {
                id: 8,
                pool_id: "legacy_pool".to_string(),
                claimant: admin,
                token: admin,
                subaccount: [3; 32],
                amount: 1_234,
                reason: "legacy unresolved transfer".to_string(),
                created_at: 99,
            }],
            next_claim_id: 9,
            swap_events,
            next_swap_event_id: 41,
            liquidity_events,
            next_liquidity_event_id: 42,
            admin_events,
            next_admin_event_id: 43,
            holder_snapshots,
            reward_events,
            next_reward_event_id: 44,
            claim_events,
            next_claim_event_id: 45,
            protocol_backend_principal: Some(admin),
            tvl_samples,
        },
        admin,
        lp,
        pool_id,
    )
}

#[test]
#[ignore = "requires source-matched RUMI_AMM_PRODUCTION_WASM and reviewed SHA-256"]
fn production_post_upgrade_preserves_populated_v5_snapshot() {
    let wasm_path = std::env::var("RUMI_AMM_PRODUCTION_WASM")
        .expect("set RUMI_AMM_PRODUCTION_WASM to the source-matched upload artifact");
    let expected_sha = std::env::var("RUMI_AMM_PRODUCTION_WASM_SHA256")
        .expect("set RUMI_AMM_PRODUCTION_WASM_SHA256 to the reviewed upload hash");
    let wasm = std::fs::read(&wasm_path).expect("read production AMM Wasm");
    assert_eq!(
        format!("{:x}", Sha256::digest(&wasm)),
        expected_sha,
        "wrong production AMM artifact"
    );

    let (legacy, admin, lp, pool_id) = snapshot();
    let caller = Principal::self_authenticating(b"v5-snapshot-caller");
    let nonexistent_ledger = Principal::self_authenticating(b"v5-snapshot-no-ledger");
    let legacy_bytes = candid::encode_one(&legacy).expect("encode frozen V5 state");
    let mut stable_memory = Vec::with_capacity(8 + legacy_bytes.len());
    stable_memory.extend_from_slice(&(legacy_bytes.len() as u64).to_le_bytes());
    stable_memory.extend_from_slice(&legacy_bytes);

    let pic = PocketIcBuilder::new().with_application_subnet().build();
    let amm = pic.create_canister_with_settings(Some(admin), None);
    pic.add_cycles(amm, 2_000_000_000_000);
    // An empty, valid Wasm module has no pre_upgrade export to rewrite the
    // frozen V5 bytes before the candidate's post_upgrade hook reads them.
    pic.install_canister(amm, b"\0asm\x01\0\0\0".to_vec(), vec![], Some(admin));
    pic.set_stable_memory(
        amm,
        stable_memory,
        pocket_ic::common::rest::BlobCompression::NoCompression,
    );
    pic.upgrade_canister(
        amm,
        wasm,
        encode_one(AmmInitArgs { admin }).expect("encode upgrade init args"),
        Some(admin),
    )
    .expect("production Wasm must upgrade and run post_upgrade on populated V5 state");

    let pools: Vec<PoolInfo> = reply(
        pic.query_call(amm, admin, "get_pools", encode_args(()).unwrap())
            .unwrap(),
    );
    assert_eq!(pools.len(), 1);
    assert_eq!(pools[0].pool_id, pool_id);
    assert_eq!(
        (pools[0].reserve_a, pools[0].reserve_b),
        (12_345_678, 87_654_321)
    );
    assert_eq!(pools[0].total_lp_shares, 100_000);
    assert!(pools[0].paused);
    assert_eq!(
        reply::<u128>(
            pic.query_call(
                amm,
                admin,
                "get_lp_balance",
                encode_args((pool_id.clone(), lp)).unwrap(),
            )
            .unwrap(),
        ),
        60_000
    );
    assert_eq!(
        reply::<candid::Nat>(
            pic.query_call(
                amm,
                admin,
                "get_pending_rewards",
                encode_args((pool_id.clone(), lp)).unwrap(),
            )
            .unwrap(),
        ),
        candid::Nat::from(125_u64)
    );
    assert_eq!(
        reply::<candid::Nat>(
            pic.query_call(
                amm,
                admin,
                "get_pending_rewards",
                encode_args((
                    pool_id.clone(),
                    Principal::self_authenticating(b"v5-snapshot-lp-b"),
                ))
                .unwrap(),
            )
            .unwrap(),
        ),
        candid::Nat::from(75_u64)
    );
    assert!(reply::<bool>(
        pic.query_call(amm, admin, "is_maintenance_mode", encode_args(()).unwrap())
            .unwrap()
    ));

    let swap_request = vec![7_u8; 32];
    let add_request = vec![8_u8; 32];
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
            encode_args((
                add_request.clone(),
                pool_id.clone(),
                100_u128,
                100_u128,
                1_u128,
            ))
            .unwrap(),
        )
        .unwrap(),
        "add_liquidity_v2 remains held until both inbound legs have durable phase replay",
    );

    let claims: Vec<PendingClaim> = reply(
        pic.query_call(amm, admin, "get_pending_claims", encode_args(()).unwrap())
            .unwrap(),
    );
    assert_eq!(
        candid::encode_one(&claims).unwrap(),
        candid::encode_one(&legacy.pending_claims).unwrap(),
        "legacy claim liability must remain exactly preserved"
    );
    let swaps: Vec<AmmSwapEvent> = reply(
        pic.query_call(
            amm,
            admin,
            "get_amm_swap_events",
            encode_args((0_u64, 100_u64)).unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(
        candid::encode_one(&swaps).unwrap(),
        candid::encode_one(&legacy.swap_events).unwrap()
    );
    let liquidity: Vec<AmmLiquidityEvent> = reply(
        pic.query_call(
            amm,
            admin,
            "get_amm_liquidity_events",
            encode_args((0_u64, 100_u64)).unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(
        candid::encode_one(&liquidity).unwrap(),
        candid::encode_one(&legacy.liquidity_events).unwrap()
    );
    let admins: Vec<AmmAdminEvent> = reply(
        pic.query_call(
            amm,
            admin,
            "get_amm_admin_events",
            encode_args((0_u64, 100_u64)).unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(
        candid::encode_one(&admins).unwrap(),
        candid::encode_one(&legacy.admin_events).unwrap()
    );
    let snapshots: Vec<HolderSnapshot> = reply(
        pic.query_call(
            amm,
            admin,
            "get_holder_snapshots",
            encode_args(("icUSD".to_string(), 0_u64, 100_u64)).unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(
        candid::encode_one(&snapshots).unwrap(),
        candid::encode_one(&legacy.holder_snapshots).unwrap()
    );

    let held_claim: Result<(), AmmError> = reply(
        pic.update_call(amm, admin, "claim_pending", encode_args((8_u64,)).unwrap())
            .unwrap(),
    );
    assert!(matches!(
        held_claim,
        Err(AmmError::InvalidInput { reason })
            if reason == "legacy pending claim has no bound outbound payout identity and is held for reconciliation"
    ));
    for request_id in [swap_request, add_request] {
        let status: Result<rumi_amm::state::InboundOperationStatus, AmmError> = reply(
            pic.query_call(
                amm,
                caller,
                "get_inbound_operation",
                encode_args((request_id,)).unwrap(),
            )
            .unwrap(),
        );
        assert!(matches!(
            status,
            Err(AmmError::InvalidInput { reason }) if reason == "inbound operation not found"
        ));
    }
    let next_sequence: Result<u64, AmmError> = reply(
        pic.query_call(
            amm,
            admin,
            "get_next_inbound_sequence",
            encode_args(()).unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(
        next_sequence.unwrap(),
        1,
        "held ingress and V5 upgrade leave the inbound journal empty"
    );
    let payouts: Result<Vec<OutboundPayout>, AmmError> = reply(
        pic.query_call(
            amm,
            admin,
            "get_unresolved_outbound_payouts",
            encode_args((0_u64, 100_u64)).unwrap(),
        )
        .unwrap(),
    );
    assert!(
        payouts.unwrap().is_empty(),
        "held ingress and V5 upgrade leave the payout journal empty"
    );
}
