//! Daily rollup collector. Scans EVT_* logs for events in the past 24h and
//! produces summary rows for liquidations, swaps, and fees.
//!
//! This runs synchronously (no inter-canister calls) since all data is local.

use candid::Principal;
use std::collections::{HashMap, HashSet};
use crate::storage;
use storage::events::*;
use storage::rollups;

const DAY_NS: u64 = 86_400_000_000_000;

pub fn run() {
    let now = ic_cdk::api::time();
    let day_start = now.saturating_sub(DAY_NS);

    rollup_liquidations(now, day_start);
    rollup_swaps(now, day_start);
    rollup_fees(now, day_start);
}

fn rollup_liquidations(now: u64, day_start: u64) {
    let events = evt_liquidations::range(day_start, now, usize::MAX);

    let mut full_count: u32 = 0;
    let mut partial_count: u32 = 0;
    let mut redistribution_count: u32 = 0;
    let mut total_collateral: u64 = 0;
    let mut total_debt: u64 = 0;
    let mut by_collateral: HashMap<Principal, u64> = HashMap::new();

    for e in &events {
        match e.liquidation_kind {
            LiquidationKind::Full => full_count += 1,
            LiquidationKind::Partial => partial_count += 1,
            LiquidationKind::Redistribution => redistribution_count += 1,
        }
        total_collateral = total_collateral.saturating_add(e.collateral_amount);
        total_debt = total_debt.saturating_add(e.debt_amount);
        if e.collateral_type != Principal::anonymous() {
            *by_collateral.entry(e.collateral_type).or_default() += e.collateral_amount;
        }
    }

    let mut by_coll_vec: Vec<(Principal, u64)> = by_collateral.into_iter().collect();
    by_coll_vec.sort_by(|a, b| b.1.cmp(&a.1));

    rollups::daily_liquidations::push(rollups::DailyLiquidationRollup {
        timestamp_ns: now,
        full_count,
        partial_count,
        redistribution_count,
        total_collateral_seized_e8s: total_collateral,
        total_debt_covered_e8s: total_debt,
        by_collateral: by_coll_vec,
    });
}

fn rollup_swaps(now: u64, day_start: u64) {
    let events = evt_swaps::range(day_start, now, usize::MAX);

    let mut tp_count: u32 = 0;
    let mut amm_count: u32 = 0;
    let mut tp_volume: u64 = 0;
    let mut amm_volume: u64 = 0;
    let mut tp_fees: u64 = 0;
    let mut amm_fees: u64 = 0;
    let mut swappers: HashSet<Principal> = HashSet::new();

    for e in &events {
        swappers.insert(e.caller);
        match e.source {
            SwapSource::ThreePool => {
                tp_count += 1;
                tp_volume = tp_volume.saturating_add(e.amount_in);
                tp_fees = tp_fees.saturating_add(e.fee);
            }
            SwapSource::Amm => {
                amm_count += 1;
                amm_volume = amm_volume.saturating_add(e.amount_in);
                amm_fees = amm_fees.saturating_add(e.fee);
            }
        }
    }

    rollups::daily_swaps::push(rollups::DailySwapRollup {
        timestamp_ns: now,
        three_pool_swap_count: tp_count,
        amm_swap_count: amm_count,
        three_pool_volume_e8s: tp_volume,
        amm_volume_e8s: amm_volume,
        three_pool_fees_e8s: tp_fees,
        amm_fees_e8s: amm_fees,
        unique_swappers: swappers.len() as u32,
    });
}

fn rollup_fees(now: u64, day_start: u64) {
    let swap_events = evt_swaps::range(day_start, now, usize::MAX);
    let vault_events = evt_vaults::range(day_start, now, usize::MAX);

    let swap_fees: u64 = swap_events.iter().map(|e| e.fee).sum();
    let mut borrow_count: u32 = 0;
    let mut redemption_count: u32 = 0;

    for e in &vault_events {
        match e.event_kind {
            VaultEventKind::Borrowed => borrow_count += 1,
            VaultEventKind::Redeemed => redemption_count += 1,
            _ => {}
        }
    }

    rollups::daily_fees::push(rollups::DailyFeeRollup {
        timestamp_ns: now,
        borrowing_fees_e8s: None,
        borrow_count,
        swap_fees_e8s: swap_fees,
        redemption_fees_e8s: None,
        redemption_count,
    });
}
