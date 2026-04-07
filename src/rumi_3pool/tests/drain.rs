//! Integration test for the A6 one-shot drain.
//!
//! Lives in its own file (not under the lib's `#[cfg(test)]`) to get a
//! fresh process for thread-local stable storage isolation. The lib's
//! unit tests share thread-locals across `#[test]` functions, which
//! would interfere with the drain's count assertions.
//!
//! NOTE: this test does NOT exercise `read_legacy_blob` because that
//! function calls `ic_cdk::api::stable::*`, which has no host fallback
//! outside a canister runtime. The byte-level round-trip belongs in A8
//! as a PocketIC test against an actual mainnet wasm. What this test
//! DOES exercise is `drain_legacy_state` end-to-end, which is the
//! correctness-critical part of the drain.

use candid::Principal;
use rumi_3pool::storage;
use rumi_3pool::storage::migration::LegacyThreePoolState;
use rumi_3pool::types::{
    Icrc3Block, Icrc3Transaction, LiquidityAction, LiquidityEventV1,
    LiquidityEventV2, SwapEventV1, SwapEventV2, ThreePoolAdminEvent,
    VirtualPriceSnapshot,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const FIXTURE: &str = include_str!("fixtures/mainnet_snapshot_2026-04-07.json");

fn dummy_swap_v1(id: u64, p: Principal) -> SwapEventV1 {
    SwapEventV1 {
        id,
        timestamp: id,
        caller: p,
        token_in: 0,
        token_out: 1,
        amount_in: 1,
        amount_out: 1,
        fee: 0,
    }
}

fn dummy_liq_v1(id: u64, p: Principal) -> LiquidityEventV1 {
    LiquidityEventV1 {
        id,
        timestamp: id,
        caller: p,
        action: LiquidityAction::AddLiquidity,
        amounts: [1, 1, 1],
        lp_amount: 1,
        coin_index: None,
        fee: None,
    }
}

fn dummy_swap_v2(id: u64, p: Principal) -> SwapEventV2 {
    SwapEventV2 {
        id,
        timestamp: id,
        caller: p,
        token_in: 0,
        token_out: 1,
        amount_in: 1,
        amount_out: 1,
        fee: 0,
        fee_bps: 4,
        imbalance_before: 0,
        imbalance_after: 0,
        is_rebalancing: false,
        pool_balances_after: [1, 1, 1],
        virtual_price_after: 1_000_000_000_000_000_000,
        migrated: true,
    }
}

fn dummy_liq_v2(id: u64, p: Principal) -> LiquidityEventV2 {
    LiquidityEventV2 {
        id,
        timestamp: id,
        caller: p,
        action: LiquidityAction::AddLiquidity,
        amounts: [1, 1, 1],
        lp_amount: 1,
        coin_index: None,
        fee: None,
        fee_bps: None,
        imbalance_before: 0,
        imbalance_after: 0,
        is_rebalancing: false,
        pool_balances_after: [1, 1, 1],
        virtual_price_after: 1_000_000_000_000_000_000,
        migrated: true,
    }
}

fn dummy_block(id: u64, p: Principal) -> Icrc3Block {
    Icrc3Block {
        id,
        timestamp: id,
        tx: Icrc3Transaction::Mint { to: p, amount: 1 },
    }
}

fn dummy_vp_snap(id: u64) -> VirtualPriceSnapshot {
    VirtualPriceSnapshot {
        timestamp_secs: id,
        virtual_price: 1_000_000_000_000_000_000,
        lp_total_supply: 1,
    }
}

#[test]
fn drain_legacy_state_preserves_fixture() {
    let fx: Value = serde_json::from_str(FIXTURE).expect("parse fixture json");

    // ─── Build LegacyThreePoolState from the fixture ──────────────────────
    let lp_total_supply: u128 = fx["pool_status"]["lp_total_supply"]
        .as_str().unwrap().parse().unwrap();
    let balances: [u128; 3] = {
        let arr = fx["pool_status"]["balances"].as_array().unwrap();
        [
            arr[0].as_str().unwrap().parse().unwrap(),
            arr[1].as_str().unwrap().parse().unwrap(),
            arr[2].as_str().unwrap().parse().unwrap(),
        ]
    };
    let admin_fees: [u128; 3] = {
        let arr = fx["admin_fees"].as_array().unwrap();
        [
            arr[0].as_str().unwrap().parse().unwrap(),
            arr[1].as_str().unwrap().parse().unwrap(),
            arr[2].as_str().unwrap().parse().unwrap(),
        ]
    };

    let mut lp_balances: BTreeMap<Principal, u128> = BTreeMap::new();
    for h in fx["lp_holders"].as_array().unwrap() {
        let p = Principal::from_text(h["principal"].as_str().unwrap()).unwrap();
        let b: u128 = h["balance"].as_str().unwrap().parse().unwrap();
        lp_balances.insert(p, b);
    }
    let expected_holders = lp_balances.len() as u64;
    let expected_holders_balances: Vec<(Principal, u128)> =
        lp_balances.iter().map(|(k, v)| (*k, *v)).collect();

    let counts = &fx["counts"];
    let swap_v1_count = counts["swap_events_v1"].as_u64().unwrap();
    let liq_v1_count = counts["liquidity_events_v1"].as_u64().unwrap();
    let admin_count = counts["admin_events"].as_u64().unwrap();
    let blocks_count = counts["icrc3_blocks"].as_u64().unwrap();
    let vp_count = counts["vp_snapshots"].as_u64().unwrap();

    // Use a sentinel principal that no other test in this binary uses, so
    // log isolation does not depend on counts being globally unique.
    let dummy_p = Principal::from_text("aaaaa-aa").unwrap();

    let swap_events: Vec<SwapEventV1> =
        (0..swap_v1_count).map(|i| dummy_swap_v1(i, dummy_p)).collect();
    let liquidity_events: Vec<LiquidityEventV1> =
        (0..liq_v1_count).map(|i| dummy_liq_v1(i, dummy_p)).collect();
    let admin_events: Vec<ThreePoolAdminEvent> = (0..admin_count)
        .map(|i| ThreePoolAdminEvent {
            id: i,
            timestamp: i,
            caller: dummy_p,
            action: rumi_3pool::types::ThreePoolAdminAction::SetPaused { paused: false },
        })
        .collect();
    let blocks: Vec<Icrc3Block> =
        (0..blocks_count).map(|i| dummy_block(i, dummy_p)).collect();
    let vp_snapshots: Vec<VirtualPriceSnapshot> =
        (0..vp_count).map(dummy_vp_snap).collect();

    // Throw a couple of v2 events in too to exercise those code paths
    // (real fixture v2 counts will land in A8 with the recapture).
    let swap_events_v2 = vec![dummy_swap_v2(0, dummy_p), dummy_swap_v2(1, dummy_p)];
    let liquidity_events_v2 = vec![dummy_liq_v2(0, dummy_p)];

    let legacy = LegacyThreePoolState {
        config: storage::SlimState::default().config,
        balances,
        lp_balances,
        lp_total_supply,
        lp_allowances: None,
        lp_tx_count: Some(blocks_count),
        vp_snapshots: Some(vp_snapshots),
        blocks: Some(blocks),
        last_block_hash: None,
        admin_fees,
        is_paused: false,
        is_initialized: true,
        authorized_burn_callers: Some({
            let mut s = BTreeSet::new();
            s.insert(dummy_p);
            s
        }),
        swap_events: Some(swap_events),
        liquidity_events: Some(liquidity_events),
        admin_events: Some(admin_events),
        swap_events_v2: Some(swap_events_v2),
        liquidity_events_v2: Some(liquidity_events_v2),
    };

    // ─── Capture baselines (this binary may run multiple #[test] later) ──
    let base_swap_v1 = storage::swap_v1::len();
    let base_liq_v1 = storage::liq_v1::len();
    let base_swap_v2 = storage::swap_v2::len();
    let base_liq_v2 = storage::liq_v2::len();
    let base_admin_ev = storage::admin_ev::len();
    let base_vp_snap = storage::vp_snap::len();
    let base_blocks = storage::blocks::len();
    let base_lp_holders = storage::lp_balance_len();

    // ─── Drain ────────────────────────────────────────────────────────────
    storage::migration::drain_legacy_state(legacy);

    // ─── Assertions ───────────────────────────────────────────────────────
    assert_eq!(
        storage::swap_v1::len() - base_swap_v1,
        swap_v1_count,
        "swap_v1 count mismatch"
    );
    assert_eq!(
        storage::liq_v1::len() - base_liq_v1,
        liq_v1_count,
        "liq_v1 count mismatch"
    );
    assert_eq!(
        storage::swap_v2::len() - base_swap_v2,
        2,
        "swap_v2 count mismatch"
    );
    assert_eq!(
        storage::liq_v2::len() - base_liq_v2,
        1,
        "liq_v2 count mismatch"
    );
    assert_eq!(
        storage::admin_ev::len() - base_admin_ev,
        admin_count,
        "admin_ev count mismatch"
    );
    assert_eq!(
        storage::vp_snap::len() - base_vp_snap,
        vp_count,
        "vp_snap count mismatch"
    );
    assert_eq!(
        storage::blocks::len() - base_blocks,
        blocks_count,
        "blocks count mismatch"
    );
    assert_eq!(
        storage::lp_balance_len() - base_lp_holders,
        expected_holders,
        "lp_balance holders count mismatch"
    );

    // Per-holder balance check.
    let mut sum: u128 = 0;
    for (p, expected) in &expected_holders_balances {
        let got = storage::lp_balance_get(p);
        assert_eq!(got, *expected, "balance mismatch for {p}");
        sum = sum.checked_add(*expected).expect("balance sum overflow");
    }
    assert_eq!(sum, lp_total_supply, "sum(lp_balances) != lp_total_supply");

    // Burn-caller membership.
    assert!(
        storage::burn_caller_contains(&dummy_p),
        "drained burn caller missing"
    );
}
