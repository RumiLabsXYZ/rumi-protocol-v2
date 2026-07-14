//! Timer wiring. Phase 3 populates the daily tier with three collectors.
//! Phase 4 adds event tailing and ICRC-3 block tailing to the pull cycle.
//!
//! Wave-9d (DOS-010) replaced the 60s `pull_cycle` join-storm with a
//! per-source schedule walked by a fast tick. Each source fires once per
//! window; offsets persist across upgrade. The default window widened from
//! 60s to 300s and the walker tick from 5s to 30s (2026-07-02 cycle-burn
//! tuning), and both are runtime-overridable via the admin `set_pull_schedule`
//! endpoint. See `pull_schedule.rs` for the pure logic.

use crate::{collectors, pull_schedule, sources, state, tailing};
use std::cell::Cell;
use std::time::Duration;

thread_local! {
    /// Active walker-tick timer, tracked so `register_pull_tick_timer` can
    /// clear and re-arm it when the admin retunes the tick interval. Timers
    /// don't survive upgrade, so this is re-populated by `setup_timers`.
    static PULL_TICK_TIMER_ID: Cell<Option<ic_cdk_timers::TimerId>> = const { Cell::new(None) };
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SetupContext {
    Init,
    PostUpgrade,
}

fn replace_interval_timer<T: Copy>(
    slot: &Cell<Option<T>>,
    clear_existing: impl FnOnce(T),
    register_new: impl FnOnce() -> T,
) {
    if let Some(old) = slot.take() {
        clear_existing(old);
    }
    slot.set(Some(register_new()));
}

/// (Re-)arm the schedule-walker timer at the currently effective tick. Clears
/// any prior timer first so calling this repeatedly (e.g. from the admin
/// setter) never stacks duplicate walkers. Mirrors the backend's
/// `register_observer_timer` clear-then-arm pattern.
pub fn register_pull_tick_timer() {
    let tick_secs =
        pull_schedule::effective_tick_secs(state::read_state(|s| s.pull_tick_secs_override)).max(1);
    PULL_TICK_TIMER_ID.with(|cell| {
        replace_interval_timer(cell, ic_cdk_timers::clear_timer, || {
            ic_cdk_timers::set_timer_interval(Duration::from_secs(tick_secs), || {
                ic_cdk::spawn(pull_due_sources())
            })
        });
    });
}

pub fn setup_timers(ctx: SetupContext) {
    // Wave-9d DOS-010: ensure every source has a `next_pull_at_ns`
    // entry. On Init the schedule is empty → fresh offsets. On
    // PostUpgrade existing offsets are preserved, only newly added
    // sources are seeded.
    let now = ic_cdk::api::time();
    let period_ns =
        pull_schedule::effective_period_ns(state::read_state(|s| s.pull_period_secs_override));
    state::mutate_state(|s| {
        let existing = s.source_next_pull_ns.clone().unwrap_or_default();
        let mut merged = pull_schedule::reseed_preserving(
            now,
            &existing,
            pull_schedule::ALL_SOURCE_IDS,
            period_ns,
        );
        // One-time re-spread when the persisted layout trails the current
        // one. Without this, deadlines seeded under the old 60s window are
        // all stale after an upgrade that widens the window to 300s, so every
        // source would fire together on the first tick and stay synchronized
        // one-burst-per-window — reviving the DOS-010 join-storm. Re-spreading
        // once restores the even stagger. Preserved thereafter (later upgrades
        // see the current version and keep in-flight offsets, so this never
        // starves a high-offset source on repeated upgrades).
        if s.schedule_layout_version.unwrap_or(0) < pull_schedule::CURRENT_SCHEDULE_LAYOUT {
            merged =
                pull_schedule::seed_initial_schedule(now, pull_schedule::ALL_SOURCE_IDS, period_ns);
            s.schedule_layout_version = Some(pull_schedule::CURRENT_SCHEDULE_LAYOUT);
        }
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

    // Wave-9d DOS-010: walk the per-source schedule every effective tick
    // (30s default). Each source fires once per effective window (300s
    // default); the burst of concurrent inter-canister calls is spread
    // across the window. Registered via the re-armable helper so the admin
    // setter can retune the tick without a redeploy.
    register_pull_tick_timer();
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
/// `next_pull_at_ns` is past, and bump their next deadline by the effective
/// window. Typically fires 1 source per tick (the ~33s default offset exceeds
/// the 30s tick, so sources rarely cluster).
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
    // the past when the next tick arrives. The period is read live so an
    // admin retune takes effect on the next advance.
    let period_ns =
        pull_schedule::effective_period_ns(state::read_state(|s| s.pull_period_secs_override));
    state::mutate_state(|s| {
        let mut schedule = s.source_next_pull_ns.clone().unwrap_or_default();
        pull_schedule::advance_schedule_for_fired(&mut schedule, now, &due, period_ns);
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

#[cfg(test)]
mod tests {
    use super::replace_interval_timer;
    use std::cell::Cell;

    #[test]
    fn replacing_interval_timer_clears_old_before_storing_new() {
        let slot = Cell::new(Some(7_u8));
        let cleared = Cell::new(None);
        let registered = Cell::new(false);

        replace_interval_timer(
            &slot,
            |old| {
                assert!(
                    !registered.get(),
                    "old timer must clear before registration"
                );
                cleared.set(Some(old));
            },
            || {
                registered.set(true);
                9
            },
        );

        assert_eq!(cleared.get(), Some(7));
        assert!(registered.get());
        assert_eq!(slot.get(), Some(9));
    }

    #[test]
    fn first_interval_timer_registration_does_not_clear() {
        let slot = Cell::new(None::<u8>);
        let clear_count = Cell::new(0_u8);

        replace_interval_timer(&slot, |_| clear_count.set(clear_count.get() + 1), || 3);

        assert_eq!(clear_count.get(), 0);
        assert_eq!(slot.get(), Some(3));
    }
}
