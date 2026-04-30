//! Timer wiring. Phase 3 populates the daily tier with three collectors.
//! Phase 4 adds event tailing and ICRC-3 block tailing to the pull cycle.

use std::time::Duration;
use crate::{collectors, sources, state, tailing};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SetupContext {
    Init,
    PostUpgrade,
}

pub fn setup_timers(ctx: SetupContext) {
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

    ic_cdk_timers::set_timer_interval(Duration::from_secs(60), || {
        ic_cdk::spawn(pull_cycle());
    });
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

async fn pull_cycle() {
    refresh_supply_cache().await;

    // Event tailing (Phase 4) - all sources are independent, run concurrently.
    futures::join!(
        tailing::backend_events::run(),
        tailing::three_pool_swaps::run(),
        tailing::three_pool_liquidity::run(),
        tailing::amm_swaps::run(),
        tailing::icrc3::tail_icusd_blocks(),
        tailing::icrc3::tail_3pool_blocks(),
        tailing::stability_pool::run(),
    );

    // Update pull cycle timestamp
    state::mutate_state(|s| {
        s.last_pull_cycle_ns = Some(ic_cdk::api::time());
    });
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
