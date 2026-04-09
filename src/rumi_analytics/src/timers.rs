//! Timer wiring. Phase 3 populates the daily tier with three collectors.
//! Phase 4 adds event tailing and ICRC-3 block tailing to the pull cycle.

use std::time::Duration;
use crate::{collectors, sources, state, tailing};

pub fn setup_timers() {
    ic_cdk_timers::set_timer_interval(Duration::from_secs(60), || {
        ic_cdk::spawn(pull_cycle());
    });
    ic_cdk_timers::set_timer_interval(Duration::from_secs(300), || {
        // Phase 5: fast snapshot.
    });
    ic_cdk_timers::set_timer_interval(Duration::from_secs(3600), || {
        // Phase 5: hourly snapshot.
    });
    ic_cdk_timers::set_timer_interval(Duration::from_secs(86400), || {
        ic_cdk::spawn(daily_snapshot());
    });
}

async fn pull_cycle() {
    refresh_supply_cache().await;

    // Event tailing (Phase 4)
    tailing::backend_events::run().await;
    tailing::three_pool_swaps::run().await;
    tailing::three_pool_liquidity::run().await;
    tailing::amm_swaps::run().await;

    // ICRC-3 block tailing (Phase 4)
    tailing::icrc3::tail_icusd_blocks().await;
    tailing::icrc3::tail_3pool_blocks().await;

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
}
