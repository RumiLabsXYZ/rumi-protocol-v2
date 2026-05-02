//! Timer wiring. Phase 3 populates the daily tier with three collectors.
//! Phase 4 adds event tailing and ICRC-3 block tailing to the pull cycle.
//!
//! Wave-9d (DOS-010) replaced the 60s `pull_cycle` join-storm with a
//! per-source schedule walked by a 5s tick. Each source still fires
//! every 60s; offsets persist across upgrade. See
//! `pull_schedule.rs` for the pure logic.

use std::time::Duration;
use crate::{collectors, pull_schedule, sources, state, tailing};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SetupContext {
    Init,
    PostUpgrade,
}

pub fn setup_timers(ctx: SetupContext) {
    // Wave-9d DOS-010: ensure every source has a `next_pull_at_ns`
    // entry. On Init the schedule is empty → fresh offsets. On
    // PostUpgrade existing offsets are preserved, only newly added
    // sources are seeded.
    let now = ic_cdk::api::time();
    state::mutate_state(|s| {
        let existing = s.source_next_pull_ns.clone().unwrap_or_default();
        let merged = pull_schedule::reseed_preserving(now, &existing, pull_schedule::ALL_SOURCE_IDS);
        s.source_next_pull_ns = Some(merged);
    });

    // Fire the daily snapshot only on Init — on post_upgrade the pre-existing
    // row already covers today, so re-firing would create duplicate daily
    // snapshots. Fast snapshot, however, also seeds heap-only state that's
    // wiped by upgrade (collateral_decimals map), so we always fire it
    // immediately. An extra fast row at upgrade time is cheap (5-min log
    // cadence) and avoids a 5-minute window where pricing falls back to
    // 8-decimal defaults — the bug that motivated PR #141.
    if ctx == SetupContext::Init {
        ic_cdk_timers::set_timer(Duration::from_secs(0), || {
            ic_cdk::spawn(daily_snapshot());
        });
    }
    ic_cdk_timers::set_timer(Duration::from_secs(0), || {
        ic_cdk::spawn(fast_snapshot());
    });

    // Wave-9d DOS-010: walk the per-source schedule every
    // PULL_CYCLE_TICK_SECS (5s). Each source still fires every 60s; the
    // burst of 9 concurrent inter-canister calls is now spread across
    // the window.
    ic_cdk_timers::set_timer_interval(
        Duration::from_secs(pull_schedule::PULL_CYCLE_TICK_SECS),
        || ic_cdk::spawn(pull_due_sources()),
    );
    ic_cdk_timers::set_timer_interval(Duration::from_secs(300), || {
        ic_cdk::spawn(fast_snapshot());
    });
    ic_cdk_timers::set_timer_interval(Duration::from_secs(3600), || {
        ic_cdk::spawn(hourly_snapshot());
    });
    ic_cdk_timers::set_timer_interval(Duration::from_secs(86400), || {
        ic_cdk::spawn(daily_snapshot());
    });
}

/// Wave-9d DOS-010: walk the per-source schedule, fire any sources whose
/// `next_pull_at_ns` is past, and bump their next deadline by
/// `PULL_CYCLE_PERIOD_NS`. Typically fires 1 source per tick (sometimes 2
/// when offsets cluster against the 5s tick grid).
async fn pull_due_sources() {
    let now = ic_cdk::api::time();
    let due: Vec<u8> = state::read_state(|s| {
        let schedule = s.source_next_pull_ns.clone().unwrap_or_default();
        pull_schedule::compute_due_sources(now, &schedule, pull_schedule::ALL_SOURCE_IDS)
    });

    if due.is_empty() {
        return;
    }

    // Advance the schedule BEFORE awaiting any sources so a slow source
    // doesn't double-fire on the next tick if its deadline is still in
    // the past when the next tick arrives.
    state::mutate_state(|s| {
        let mut schedule = s.source_next_pull_ns.clone().unwrap_or_default();
        pull_schedule::advance_schedule_for_fired(&mut schedule, now, &due);
        s.source_next_pull_ns = Some(schedule);
    });

    // Fire each due source concurrently. Within a single tick that's at
    // most 1-2 sources (vs 9 in the pre-Wave-9d sync storm). A slow
    // source here doesn't block sources due on subsequent ticks because
    // each tick is its own spawn().
    let futs: Vec<_> = due.iter().map(|&id| run_source(id)).collect();
    futures::future::join_all(futs).await;

    // Update pull cycle timestamp (analytics freshness contract).
    state::mutate_state(|s| {
        s.last_pull_cycle_ns = Some(ic_cdk::api::time());
    });
}

/// Dispatch a single source pull by id. Adding a new source means
/// adding it to `pull_schedule::ALL_SOURCE_IDS` AND adding a branch
/// here.
async fn run_source(source_id: u8) {
    use crate::storage::cursors;
    match source_id {
        pull_schedule::SOURCE_ID_SUPPLY_CACHE => refresh_supply_cache().await,
        cursors::CURSOR_ID_BACKEND_EVENTS => tailing::backend_events::run().await,
        cursors::CURSOR_ID_3POOL_SWAPS => tailing::three_pool_swaps::run().await,
        cursors::CURSOR_ID_3POOL_LIQUIDITY => tailing::three_pool_liquidity::run().await,
        cursors::CURSOR_ID_3POOL_BLOCKS => tailing::icrc3::tail_3pool_blocks().await,
        cursors::CURSOR_ID_AMM_SWAPS => tailing::amm_swaps::run().await,
        cursors::CURSOR_ID_STABILITY_EVENTS => tailing::stability_pool::run().await,
        cursors::CURSOR_ID_ICUSD_BLOCKS => tailing::icrc3::tail_icusd_blocks().await,
        cursors::CURSOR_ID_AMM_LIQUIDITY => tailing::amm_liquidity::run().await,
        _ => {
            // Schedule contains an id not in run_source — drop it
            // silently to avoid a permanent re-fire loop. Fix by adding
            // the missing match arm.
            ic_cdk::println!(
                "[rumi_analytics] pull_due_sources: unknown source id {}",
                source_id
            );
        }
    }
}

async fn refresh_supply_cache() {
    let ledger = state::read_state(|s| s.sources.icusd_ledger);
    match sources::icusd_ledger::icrc1_total_supply(ledger).await {
        Ok(total) => {
            state::mutate_state(|s| s.circulating_supply_icusd_e8s = Some(total));
        }
        Err(e) => {
            ic_cdk::println!("rumi_analytics: supply refresh failed: {}", e);
            state::mutate_state(|s| s.error_counters.icusd_ledger += 1);
        }
    }
}

async fn daily_snapshot() {
    let (tvl_res, vaults_res, stability_res, holders_res) = futures::join!(
        collectors::tvl::run(),
        collectors::vaults::run(),
        collectors::stability::run(),
        collectors::holders::run(),
    );

    if let Err(e) = tvl_res {
        ic_cdk::println!("rumi_analytics: daily TVL snapshot failed: {}", e);
    }
    if let Err(e) = vaults_res {
        ic_cdk::println!("rumi_analytics: daily vault snapshot failed: {}", e);
    }
    if let Err(e) = stability_res {
        ic_cdk::println!("rumi_analytics: daily stability snapshot failed: {}", e);
    }
    if let Err(e) = holders_res {
        ic_cdk::println!("rumi_analytics: daily holder snapshot failed: {}", e);
    }

    // Daily rollups (sync, no inter-canister calls)
    collectors::rollups::run();
}

async fn fast_snapshot() {
    if let Err(e) = collectors::fast::run().await {
        ic_cdk::println!("rumi_analytics: fast snapshot failed: {}", e);
    }
}

async fn hourly_snapshot() {
    if let Err(e) = collectors::hourly::run().await {
        ic_cdk::println!("rumi_analytics: hourly snapshot failed: {}", e);
    }
}
