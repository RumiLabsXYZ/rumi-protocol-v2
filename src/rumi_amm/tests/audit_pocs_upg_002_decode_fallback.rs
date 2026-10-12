//! UPG-002 regression fence: rumi_amm post_upgrade must not trap when the
//! snapshot blob fails to decode against any known schema version.
//!
//! Audit report: audit-reports/2026-04-22-28e9896/raw-pass-results/upgrade-safety.json (UPG-002).
//!
//! Before the Wave-6 fix, `src/rumi_amm/src/state.rs::load_from_stable_memory`
//! tried V-current..V1 in sequence, but the final V1 fallback used
//! `Decode!(...).expect(...)` which traps on failure. A trap in post_upgrade
//! bricks the canister.
//!
//! The fix extracts the version-walking logic into `try_decode_state`, which
//! returns `None` if every known version fails.
//!
//! UPDATE (audit 2026-06-05, SAT-004): the caller `load_from_stable_memory` now
//! TRAPS when every known version fails, instead of wiping to
//! `AmmState::default()`. The old "AMM positions are reconstructable from ledger
//! balances" justification was false — pool reserves are internal-only
//! accounting and the per-LP reward state cannot be reconstructed. A silent
//! wipe of live pools is the 2026-05-18 incident class. `try_decode_state` still
//! returns `None` on undecodable input (the unit-testable contract below); only
//! the caller's reaction changed from wipe to trap.
//!
//! Also added: `AmmStateV5`, a frozen snapshot of the current shape, so the next
//! non-Option field added to `AmmState` decodes via V5 instead of silently
//! falling through to V4 (which would drop `protocol_backend_principal` and all
//! post-V4 state).
//!
//! These unit tests exercise `try_decode_state` directly:
//! 1. Valid encoded state decodes round-trip.
//! 2. Corrupt bytes return None (no trap, no panic).
//! 3. Truncated and empty bytes return None.
//! 4. A fully-populated current state round-trips with post-V4 fields intact.

use candid::{CandidType, Encode, Principal};
use rumi_amm::state::{try_decode_state, AmmState};
use rumi_amm::types::*;
use std::collections::BTreeMap;

/// Frozen pre-journal state shape, matching the deployed V5 snapshot.
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

#[test]
fn upg_002_valid_state_decodes_round_trip() {
    let original = AmmState::default();
    let bytes = Encode!(&original).expect("encode of default AmmState should succeed");

    let decoded = try_decode_state(&bytes);
    assert!(
        decoded.is_some(),
        "UPG-002: valid encoded AmmState must decode via try_decode_state",
    );
}

#[test]
fn upg_002_corrupt_bytes_return_none_no_trap() {
    let corrupt = vec![0xffu8; 1024];

    let decoded = try_decode_state(&corrupt);
    assert!(
        decoded.is_none(),
        "UPG-002: corrupt bytes must return None (caller traps without wiping state), not panic here",
    );
}

#[test]
fn upg_002_truncated_bytes_return_none_no_trap() {
    let original = AmmState::default();
    let bytes = Encode!(&original).expect("encode should succeed");

    let truncated = &bytes[..bytes.len() / 2];

    let decoded = try_decode_state(truncated);
    assert!(
        decoded.is_none(),
        "UPG-002: truncated bytes must return None, not panic or trap",
    );
}

#[test]
fn upg_002_empty_bytes_return_none_no_trap() {
    let decoded = try_decode_state(&[]);
    assert!(
        decoded.is_none(),
        "UPG-002: empty bytes must return None, not panic or trap",
    );
}

#[test]
fn sat_004_populated_state_preserves_post_v4_fields() {
    // SAT-004: the fields the live AmmState carries beyond V4
    // (protocol_backend_principal, event logs + counters, tvl_samples) must
    // survive an upgrade round-trip. Before the AmmStateV5 snapshot, the next
    // non-Option field added to AmmState would route the decode to V4 and
    // silently reset protocol_backend_principal to None (halting reward
    // distribution) and drop all post-V4 state. This pins the round-trip for
    // the current shape so a broken/ reordered decode is caught.
    let mut state = AmmState::default();
    let backend = candid::Principal::from_text("aaaaa-aa").unwrap();
    state.protocol_backend_principal = Some(backend);
    state.next_swap_event_id = 42;
    state.next_claim_id = 7;

    let bytes = Encode!(&state).expect("encode populated AmmState");
    let decoded = try_decode_state(&bytes).expect("populated state must decode");

    assert_eq!(
        decoded.protocol_backend_principal,
        Some(backend),
        "SAT-004: protocol_backend_principal must survive decode, else reward \
         distribution halts after upgrade",
    );
    assert_eq!(
        decoded.next_swap_event_id, 42,
        "SAT-004: post-V4 counters must survive the decode",
    );
    assert_eq!(decoded.next_claim_id, 7);
}

#[test]
fn payout_journal_upgrade_decodes_frozen_v5_snapshot_without_dropping_state() {
    let backend = Principal::from_text("aaaaa-aa").unwrap();
    let lp = Principal::self_authenticating(&[0x51]);
    let second_lp = Principal::self_authenticating(&[0x52]);
    let token_a = Principal::self_authenticating(&[0x61]);
    let token_b = Principal::self_authenticating(&[0x62]);
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
    let mut processed_donation_nonces = std::collections::VecDeque::new();
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
        caller: backend,
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
    let legacy = LegacyAmmStateV5 {
        admin: backend,
        pools,
        pool_creation_open: true,
        maintenance_mode: true,
        pending_claims: vec![PendingClaim {
            id: 8,
            pool_id: "legacy_pool".to_string(),
            claimant: backend,
            token: backend,
            subaccount: [3; 32],
            amount: 1234,
            reason: "legacy unresolved transfer".to_string(),
            created_at: 99,
        }],
        next_claim_id: 9,
        swap_events: swap_events.clone(),
        next_swap_event_id: 41,
        liquidity_events: liquidity_events.clone(),
        next_liquidity_event_id: 42,
        admin_events: admin_events.clone(),
        next_admin_event_id: 43,
        holder_snapshots: holder_snapshots.clone(),
        reward_events: reward_events.clone(),
        next_reward_event_id: 44,
        claim_events: claim_events.clone(),
        next_claim_event_id: 45,
        protocol_backend_principal: Some(backend),
        tvl_samples: tvl_samples.clone(),
    };
    let bytes = Encode!(&legacy).expect("encode frozen V5 state");
    let decoded = try_decode_state(&bytes).expect("V5 snapshot must decode through fallback");
    assert!(decoded.maintenance_mode);
    assert!(decoded.pool_creation_open);
    assert_eq!(decoded.admin, backend);
    assert_eq!(decoded.next_claim_id, 9);
    assert_eq!(
        decoded.pending_claims.len(),
        1,
        "legacy claim must remain held after upgrade"
    );
    assert_eq!(decoded.pending_claims[0].id, 8);
    assert_eq!(
        Encode!(&decoded.pending_claims).unwrap(),
        Encode!(&legacy.pending_claims).unwrap(),
        "the exact legacy liability must survive the V5 decode"
    );
    let expected_pool = legacy.pools.get(&pool_id).unwrap();
    let migrated_pool = decoded
        .pools
        .get(&pool_id)
        .expect("legacy pool survives migration");
    assert_eq!(
        Encode!(expected_pool).unwrap(),
        Encode!(migrated_pool).unwrap()
    );
    assert_eq!(
        Encode!(&decoded.swap_events).unwrap(),
        Encode!(&swap_events).unwrap()
    );
    assert_eq!(
        Encode!(&decoded.liquidity_events).unwrap(),
        Encode!(&liquidity_events).unwrap()
    );
    assert_eq!(
        Encode!(&decoded.admin_events).unwrap(),
        Encode!(&admin_events).unwrap()
    );
    assert_eq!(
        Encode!(&decoded.holder_snapshots).unwrap(),
        Encode!(&holder_snapshots).unwrap()
    );
    assert_eq!(
        Encode!(&decoded.reward_events).unwrap(),
        Encode!(&reward_events).unwrap()
    );
    assert_eq!(
        Encode!(&decoded.claim_events).unwrap(),
        Encode!(&claim_events).unwrap()
    );
    assert_eq!(
        Encode!(&decoded.tvl_samples).unwrap(),
        Encode!(&tvl_samples).unwrap()
    );
    assert_eq!(decoded.next_swap_event_id, 41);
    assert_eq!(decoded.next_liquidity_event_id, 42);
    assert_eq!(decoded.next_admin_event_id, 43);
    assert_eq!(decoded.next_reward_event_id, 44);
    assert_eq!(decoded.next_claim_event_id, 45);
    assert_eq!(decoded.protocol_backend_principal, Some(backend));
    assert!(
        migrated_pool.paused,
        "legacy pool remains paused during held migration"
    );
    assert!(
        decoded.maintenance_mode,
        "migration retains the held rollout state"
    );
    assert!(decoded.inbound_operations.is_empty());
    assert_eq!(decoded.inbound_sequence_high_water, 0);
    assert!(decoded.outbound_payouts.is_empty());
    assert!(decoded.outbound_payout_links.is_empty());
    assert_eq!(decoded.next_outbound_payout_id, 0);
}

#[test]
fn inbound_operation_and_terminal_replay_marker_survive_state_round_trip() {
    use rumi_amm::state::{
        InboundLeg, InboundLegStatus, InboundOperation, InboundOperationKind, InboundOperationPhase,
    };
    let caller = Principal::from_text("aaaaa-aa").unwrap();
    let mut state = AmmState::default();
    state.inbound_operations.push(InboundOperation {
        request_id: vec![7; 32],
        caller,
        pool_id: "pool".to_string(),
        kind: InboundOperationKind::Swap,
        argument_digest: vec![8; 32],
        legs: vec![InboundLeg {
            ledger: caller,
            from: caller,
            to_subaccount: Some([3; 32]),
            amount: 42,
            fee: None,
            memo: vec![9; 32],
            created_at_time: 123,
            status: InboundLegStatus::Confirmed(17),
        }],
        created_at_time: 123,
        phase: InboundOperationPhase::Completed,
        output_payout_id: Some(11),
        result_amount: Some(40),
        output_ledger_fee: Some(3),
        result_fee: Some(2),
        protocol_fee: Some(1),
        token_in: Some(caller),
        sequence_managed: Some(true),
        held_reason: None,
    });
    let bytes = Encode!(&state).unwrap();
    let decoded = try_decode_state(&bytes).expect("inbound operation state decodes");
    let op = &decoded.inbound_operations[0];
    assert_eq!(op.request_id, vec![7; 32]);
    assert_eq!(op.output_ledger_fee, Some(3));
    assert_eq!(op.phase, InboundOperationPhase::Completed);
    assert_eq!(op.legs[0].status, InboundLegStatus::Confirmed(17));
    assert_eq!(op.output_payout_id, Some(11));
    assert_eq!(op.result_amount, Some(40));
}
