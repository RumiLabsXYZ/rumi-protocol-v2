//! Timer wiring. Phase 1 sets up all four tier intervals but only the daily
//! tier (TVL collector) and the pull-cycle supply refresh do real work.
//! Subsequent phases populate the other callbacks.

use std::time::Duration;

use crate::{collectors, sources, state};

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

/// Pull cycle: Phase 1 only refreshes the supply cache. Phase 4 extends this
/// with event tailing across every source stream.
async fn pull_cycle() {
    refresh_supply_cache().await;
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
    if let Err(e) = collectors::tvl::run().await {
        ic_cdk::println!("rumi_analytics: daily TVL snapshot failed: {}", e);
    }
}
