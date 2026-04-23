//! Analytics endpoints for AMM pools: windowed time series, pool stats,
//! top swappers / LPs, and per-pool event filters. These mirror the shape
//! of the equivalent rumi_3pool endpoints so the Explorer `/e/pool/{id}`
//! page can render either pool source with minimal branching.
//!
//! Time series are derived at query time by bucketing the existing
//! swap and liquidity events (we do not maintain a separate collector).
//! Frequently-requested shapes are served from a 60s TTL cache to keep
//! repeat calls cheap.

use candid::Principal;
use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap};

use crate::state::read_state;
use crate::types::*;

// ─── Constants ───

const NANOS_PER_SEC: u64 = 1_000_000_000;

const SERIES_DEFAULT_POINTS: u32 = 100;
const SERIES_MIN_POINTS: u32 = 1;
const SERIES_MAX_POINTS: u32 = 500;

const TOP_DEFAULT_LIMIT: u32 = 50;
const TOP_MIN_LIMIT: u32 = 1;
const TOP_MAX_LIMIT: u32 = 200;

/// TTL for cached analytics responses. Repeat callers within this window
/// see the same response without re-bucketing events.
const ANALYTICS_TTL_NS: u64 = 60 * NANOS_PER_SEC;

// ─── Caches (thread-local, non-persisted) ───

type VolumeCacheKey = (PoolId, AmmStatsWindow, u32);
type BalanceCacheKey = (PoolId, AmmStatsWindow, u32);
type FeeCacheKey = (PoolId, AmmStatsWindow, u32);
type StatsCacheKey = (PoolId, AmmStatsWindow);
type SwappersCacheKey = (PoolId, AmmStatsWindow, u32);
type LpsCacheKey = (PoolId, u32);

thread_local! {
    static VOLUME_CACHE: RefCell<HashMap<VolumeCacheKey, (u64, Vec<AmmVolumePoint>)>> =
        RefCell::new(HashMap::new());
    static BALANCE_CACHE: RefCell<HashMap<BalanceCacheKey, (u64, Vec<AmmBalancePoint>)>> =
        RefCell::new(HashMap::new());
    static FEE_CACHE: RefCell<HashMap<FeeCacheKey, (u64, Vec<AmmFeePoint>)>> =
        RefCell::new(HashMap::new());
    static STATS_CACHE: RefCell<HashMap<StatsCacheKey, (u64, AmmPoolStats)>> =
        RefCell::new(HashMap::new());
    static SWAPPERS_CACHE: RefCell<HashMap<SwappersCacheKey, (u64, Vec<(Principal, u64, u128)>)>> =
        RefCell::new(HashMap::new());
    static LPS_CACHE: RefCell<HashMap<LpsCacheKey, (u64, Vec<(Principal, u128, u32)>)>> =
        RefCell::new(HashMap::new());
}

/// Drop every cached analytics response for a pool. Call after a new swap
/// or liquidity event is recorded so the next reader sees fresh data.
pub fn invalidate_cache_for_pool(pool_id: &PoolId) {
    VOLUME_CACHE.with(|c| c.borrow_mut().retain(|(p, _, _), _| p != pool_id));
    BALANCE_CACHE.with(|c| c.borrow_mut().retain(|(p, _, _), _| p != pool_id));
    FEE_CACHE.with(|c| c.borrow_mut().retain(|(p, _, _), _| p != pool_id));
    STATS_CACHE.with(|c| c.borrow_mut().retain(|(p, _), _| p != pool_id));
    SWAPPERS_CACHE.with(|c| c.borrow_mut().retain(|(p, _, _), _| p != pool_id));
    LPS_CACHE.with(|c| c.borrow_mut().retain(|(p, _), _| p != pool_id));
}

// Test-only helper: reset every cache regardless of pool.
#[cfg(test)]
pub fn clear_all_caches() {
    VOLUME_CACHE.with(|c| c.borrow_mut().clear());
    BALANCE_CACHE.with(|c| c.borrow_mut().clear());
    FEE_CACHE.with(|c| c.borrow_mut().clear());
    STATS_CACHE.with(|c| c.borrow_mut().clear());
    SWAPPERS_CACHE.with(|c| c.borrow_mut().clear());
    LPS_CACHE.with(|c| c.borrow_mut().clear());
}

// ─── Window / clamping helpers ───

/// Returns the window duration in nanoseconds, or `None` for `All` which
/// means "from the first recorded event onward".
fn window_duration_ns(window: AmmStatsWindow) -> Option<u64> {
    let secs: u64 = match window {
        AmmStatsWindow::Hour => 3_600,
        AmmStatsWindow::Day => 24 * 3_600,
        AmmStatsWindow::Week => 7 * 24 * 3_600,
        AmmStatsWindow::Month => 30 * 24 * 3_600,
        AmmStatsWindow::All => return None,
    };
    Some(secs * NANOS_PER_SEC)
}

fn clamp_points(points: u32) -> u32 {
    if points == 0 {
        SERIES_DEFAULT_POINTS
    } else {
        points.clamp(SERIES_MIN_POINTS, SERIES_MAX_POINTS)
    }
}

fn clamp_limit(limit: u32) -> u32 {
    if limit == 0 {
        TOP_DEFAULT_LIMIT
    } else {
        limit.clamp(TOP_MIN_LIMIT, TOP_MAX_LIMIT)
    }
}

fn now_ns() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        ic_cdk::api::time()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        TEST_TIME.with(|t| *t.borrow())
    }
}

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    static TEST_TIME: RefCell<u64> = RefCell::new(1_700_000_000 * NANOS_PER_SEC);
}

#[cfg(test)]
pub fn set_test_time(ns: u64) {
    TEST_TIME.with(|t| *t.borrow_mut() = ns);
}

/// Returns `(window_start_ns, bucket_duration_ns, effective_points)` for a series query.
/// For `All`, the window starts at the earliest relevant timestamp in `ts_hint`
/// (or `now` if empty, producing a single-bucket point at now).
fn resolve_window(
    now: u64,
    window: AmmStatsWindow,
    points: u32,
    earliest_event_ns: Option<u64>,
) -> (u64, u64, u32) {
    let points = clamp_points(points);
    let window_start = match window_duration_ns(window) {
        Some(duration) => now.saturating_sub(duration),
        None => earliest_event_ns.unwrap_or(now),
    };
    let span = now.saturating_sub(window_start).max(1);
    let bucket = (span / points as u64).max(1);
    (window_start, bucket, points)
}

fn bucket_floor(ts_ns: u64, bucket_ns: u64, anchor_ns: u64) -> u64 {
    // Anchor buckets to the window_start so the last bucket ends exactly at `now`.
    if bucket_ns == 0 {
        return ts_ns;
    }
    let delta = ts_ns.saturating_sub(anchor_ns);
    anchor_ns + (delta / bucket_ns) * bucket_ns
}

// ─── Pool existence ───

fn pool_exists(pool_id: &PoolId) -> bool {
    read_state(|s| s.pools.contains_key(pool_id))
}

// ─── Volume series ───

pub fn get_volume_series(query: AmmSeriesQuery) -> Vec<AmmVolumePoint> {
    let points = clamp_points(query.points);
    let cache_key = (query.pool.clone(), query.window, points);
    let now = now_ns();

    let cached = VOLUME_CACHE.with(|c| c.borrow().get(&cache_key).cloned());
    if let Some((cached_at, response)) = cached {
        if now.saturating_sub(cached_at) < ANALYTICS_TTL_NS {
            return response;
        }
    }

    if !pool_exists(&query.pool) {
        return Vec::new();
    }

    let response = read_state(|s| {
        let earliest = s
            .swap_events
            .iter()
            .filter(|e| e.pool_id == query.pool)
            .map(|e| e.timestamp)
            .min();
        let (window_start, bucket_ns, _) = resolve_window(now, query.window, points, earliest);

        // bucket -> (vol_a, vol_b, count)
        let mut buckets: BTreeMap<u64, (u128, u128, u32)> = BTreeMap::new();
        let pool = match s.pools.get(&query.pool) {
            Some(p) => p,
            None => return Vec::new(),
        };
        let token_a = pool.token_a;

        for event in s.swap_events.iter() {
            if event.pool_id != query.pool {
                continue;
            }
            if event.timestamp < window_start || event.timestamp > now {
                continue;
            }
            let bucket = bucket_floor(event.timestamp, bucket_ns, window_start);
            let entry = buckets.entry(bucket).or_insert((0, 0, 0));
            if event.token_in == token_a {
                entry.0 = entry.0.saturating_add(event.amount_in);
                entry.1 = entry.1.saturating_add(event.amount_out);
            } else {
                entry.1 = entry.1.saturating_add(event.amount_in);
                entry.0 = entry.0.saturating_add(event.amount_out);
            }
            entry.2 = entry.2.saturating_add(1);
        }

        buckets
            .into_iter()
            .map(|(ts_ns, (vol_a, vol_b, count))| AmmVolumePoint {
                ts_ns,
                volume_a_e8s: vol_a,
                volume_b_e8s: vol_b,
                swap_count: count,
            })
            .collect()
    });

    VOLUME_CACHE.with(|c| {
        c.borrow_mut().insert(cache_key, (now, response.clone()));
    });
    response
}

// ─── Balance series ───
//
// Reserves are derived by rewinding swap + liquidity events from the
// current on-chain reserves backwards to each bucket boundary. Protocol
// fee splits are not stored per-event, so the reversal approximates by
// treating the total fee as staying in the reserve; this is exact when
// protocol_fee_bps=0 (the current configuration for all pools) and has
// sub-basis-point drift otherwise.

pub fn get_balance_series(query: AmmSeriesQuery) -> Vec<AmmBalancePoint> {
    let points = clamp_points(query.points);
    let cache_key = (query.pool.clone(), query.window, points);
    let now = now_ns();

    let cached = BALANCE_CACHE.with(|c| c.borrow().get(&cache_key).cloned());
    if let Some((cached_at, response)) = cached {
        if now.saturating_sub(cached_at) < ANALYTICS_TTL_NS {
            return response;
        }
    }

    if !pool_exists(&query.pool) {
        return Vec::new();
    }

    let response = read_state(|s| {
        let pool = match s.pools.get(&query.pool) {
            Some(p) => p,
            None => return Vec::new(),
        };
        let token_a = pool.token_a;

        // Collect per-pool events into a single timeline.
        #[derive(Clone, Copy)]
        enum Delta {
            Swap { in_is_a: bool, amount_in: u128, amount_out: u128 },
            AddLiq { amount_a: u128, amount_b: u128 },
            RemoveLiq { amount_a: u128, amount_b: u128 },
        }

        let mut timeline: Vec<(u64, Delta)> = Vec::new();
        for e in s.swap_events.iter() {
            if e.pool_id != query.pool {
                continue;
            }
            timeline.push((
                e.timestamp,
                Delta::Swap {
                    in_is_a: e.token_in == token_a,
                    amount_in: e.amount_in,
                    amount_out: e.amount_out,
                },
            ));
        }
        for e in s.liquidity_events.iter() {
            if e.pool_id != query.pool {
                continue;
            }
            let delta = match e.action {
                AmmLiquidityAction::AddLiquidity => Delta::AddLiq {
                    amount_a: e.amount_a,
                    amount_b: e.amount_b,
                },
                AmmLiquidityAction::RemoveLiquidity => Delta::RemoveLiq {
                    amount_a: e.amount_a,
                    amount_b: e.amount_b,
                },
            };
            timeline.push((e.timestamp, delta));
        }

        let earliest = timeline.iter().map(|(t, _)| *t).min();
        let (window_start, bucket_ns, _) = resolve_window(now, query.window, points, earliest);

        // Descending by timestamp so we can walk backwards from `now`.
        timeline.sort_by_key(|(t, _)| Reverse(*t));

        let mut reserve_a: i128 = pool.reserve_a as i128;
        let mut reserve_b: i128 = pool.reserve_b as i128;

        // Emit one sample per bucket boundary, rewinding events that fall
        // in the interval (boundary, previous_boundary].
        let mut boundaries: Vec<u64> = Vec::with_capacity(points as usize);
        for i in 0..points {
            let ts = now.saturating_sub((i as u64).saturating_mul(bucket_ns));
            if ts < window_start {
                boundaries.push(window_start);
                break;
            }
            boundaries.push(ts);
        }

        let mut result: Vec<AmmBalancePoint> = Vec::with_capacity(boundaries.len());
        let mut idx = 0usize;
        for boundary in boundaries.iter() {
            while idx < timeline.len() && timeline[idx].0 > *boundary {
                match timeline[idx].1 {
                    Delta::Swap { in_is_a, amount_in, amount_out } => {
                        if in_is_a {
                            reserve_a -= amount_in as i128;
                            reserve_b += amount_out as i128;
                        } else {
                            reserve_b -= amount_in as i128;
                            reserve_a += amount_out as i128;
                        }
                    }
                    Delta::AddLiq { amount_a, amount_b } => {
                        reserve_a -= amount_a as i128;
                        reserve_b -= amount_b as i128;
                    }
                    Delta::RemoveLiq { amount_a, amount_b } => {
                        reserve_a += amount_a as i128;
                        reserve_b += amount_b as i128;
                    }
                }
                idx += 1;
            }
            result.push(AmmBalancePoint {
                ts_ns: *boundary,
                reserve_a_e8s: reserve_a.max(0) as u128,
                reserve_b_e8s: reserve_b.max(0) as u128,
            });
        }

        result.reverse(); // ascending by ts_ns
        result
    });

    BALANCE_CACHE.with(|c| {
        c.borrow_mut().insert(cache_key, (now, response.clone()));
    });
    response
}

// ─── Fee series ───
//
// The swap event's `fee` field is the total fee (LP + protocol) denominated
// in `token_in`. We attribute fees to the input token's side.

pub fn get_fee_series(query: AmmSeriesQuery) -> Vec<AmmFeePoint> {
    let points = clamp_points(query.points);
    let cache_key = (query.pool.clone(), query.window, points);
    let now = now_ns();

    let cached = FEE_CACHE.with(|c| c.borrow().get(&cache_key).cloned());
    if let Some((cached_at, response)) = cached {
        if now.saturating_sub(cached_at) < ANALYTICS_TTL_NS {
            return response;
        }
    }

    if !pool_exists(&query.pool) {
        return Vec::new();
    }

    let response = read_state(|s| {
        let earliest = s
            .swap_events
            .iter()
            .filter(|e| e.pool_id == query.pool)
            .map(|e| e.timestamp)
            .min();
        let (window_start, bucket_ns, _) = resolve_window(now, query.window, points, earliest);

        let pool = match s.pools.get(&query.pool) {
            Some(p) => p,
            None => return Vec::new(),
        };
        let token_a = pool.token_a;

        let mut buckets: BTreeMap<u64, (u128, u128)> = BTreeMap::new();
        for e in s.swap_events.iter() {
            if e.pool_id != query.pool {
                continue;
            }
            if e.timestamp < window_start || e.timestamp > now {
                continue;
            }
            let bucket = bucket_floor(e.timestamp, bucket_ns, window_start);
            let entry = buckets.entry(bucket).or_insert((0, 0));
            if e.token_in == token_a {
                entry.0 = entry.0.saturating_add(e.fee);
            } else {
                entry.1 = entry.1.saturating_add(e.fee);
            }
        }

        buckets
            .into_iter()
            .map(|(ts_ns, (fees_a, fees_b))| AmmFeePoint {
                ts_ns,
                fees_a_e8s: fees_a,
                fees_b_e8s: fees_b,
            })
            .collect()
    });

    FEE_CACHE.with(|c| {
        c.borrow_mut().insert(cache_key, (now, response.clone()));
    });
    response
}

// ─── Pool stats ───

pub fn get_pool_stats(query: AmmStatsQuery) -> AmmPoolStats {
    let cache_key = (query.pool.clone(), query.window);
    let now = now_ns();

    let cached = STATS_CACHE.with(|c| c.borrow().get(&cache_key).cloned());
    if let Some((cached_at, response)) = cached {
        if now.saturating_sub(cached_at) < ANALYTICS_TTL_NS {
            return response;
        }
    }

    let empty_stats = AmmPoolStats {
        pool: query.pool.clone(),
        window: query.window,
        volume_a_e8s: 0,
        volume_b_e8s: 0,
        fees_a_e8s: 0,
        fees_b_e8s: 0,
        swap_count: 0,
        unique_swappers: 0,
        unique_lps: 0,
        generated_at_ns: now,
    };

    if !pool_exists(&query.pool) {
        return empty_stats;
    }

    let response = read_state(|s| {
        let pool = match s.pools.get(&query.pool) {
            Some(p) => p,
            None => return empty_stats.clone(),
        };
        let token_a = pool.token_a;
        let window_start = match window_duration_ns(query.window) {
            Some(d) => now.saturating_sub(d),
            None => 0,
        };

        let mut volume_a: u128 = 0;
        let mut volume_b: u128 = 0;
        let mut fees_a: u128 = 0;
        let mut fees_b: u128 = 0;
        let mut swap_count: u32 = 0;
        let mut swappers: std::collections::BTreeSet<Principal> = std::collections::BTreeSet::new();

        for e in s.swap_events.iter() {
            if e.pool_id != query.pool {
                continue;
            }
            if e.timestamp < window_start {
                continue;
            }
            if e.token_in == token_a {
                volume_a = volume_a.saturating_add(e.amount_in);
                volume_b = volume_b.saturating_add(e.amount_out);
                fees_a = fees_a.saturating_add(e.fee);
            } else {
                volume_b = volume_b.saturating_add(e.amount_in);
                volume_a = volume_a.saturating_add(e.amount_out);
                fees_b = fees_b.saturating_add(e.fee);
            }
            swap_count = swap_count.saturating_add(1);
            swappers.insert(e.caller);
        }

        // Unique LPs excludes the anonymous principal (which holds MINIMUM_LIQUIDITY
        // as a permanent lock-up from the first deposit).
        let unique_lps = pool
            .lp_shares
            .keys()
            .filter(|p| **p != Principal::anonymous())
            .count() as u32;

        AmmPoolStats {
            pool: query.pool.clone(),
            window: query.window,
            volume_a_e8s: volume_a,
            volume_b_e8s: volume_b,
            fees_a_e8s: fees_a,
            fees_b_e8s: fees_b,
            swap_count,
            unique_swappers: swappers.len() as u32,
            unique_lps,
            generated_at_ns: now,
        }
    });

    STATS_CACHE.with(|c| {
        c.borrow_mut().insert(cache_key, (now, response.clone()));
    });
    response
}

// ─── Top swappers ───
//
// Returns (caller, swap_count, volume_token_a_e8s). Volume is token-A
// denominated: for a→b swaps, the amount_in (which is token_a) contributes;
// for b→a swaps, the amount_out (which is token_a) contributes. This avoids
// mixing units for asymmetric pairs like 3USD/ICP.

pub fn get_top_swappers(query: AmmTopSwappersQuery) -> Vec<(Principal, u64, u128)> {
    let limit = clamp_limit(query.limit);
    let cache_key = (query.pool.clone(), query.window, limit);
    let now = now_ns();

    let cached = SWAPPERS_CACHE.with(|c| c.borrow().get(&cache_key).cloned());
    if let Some((cached_at, response)) = cached {
        if now.saturating_sub(cached_at) < ANALYTICS_TTL_NS {
            return response;
        }
    }

    if !pool_exists(&query.pool) {
        return Vec::new();
    }

    let response = read_state(|s| {
        let pool = match s.pools.get(&query.pool) {
            Some(p) => p,
            None => return Vec::new(),
        };
        let token_a = pool.token_a;
        let window_start = match window_duration_ns(query.window) {
            Some(d) => now.saturating_sub(d),
            None => 0,
        };

        let mut acc: BTreeMap<Principal, (u64, u128)> = BTreeMap::new();
        for e in s.swap_events.iter() {
            if e.pool_id != query.pool {
                continue;
            }
            if e.timestamp < window_start {
                continue;
            }
            let entry = acc.entry(e.caller).or_insert((0, 0));
            entry.0 += 1;
            let token_a_volume = if e.token_in == token_a {
                e.amount_in
            } else {
                e.amount_out
            };
            entry.1 = entry.1.saturating_add(token_a_volume);
        }

        let mut v: Vec<(Principal, u64, u128)> =
            acc.into_iter().map(|(p, (c, vol))| (p, c, vol)).collect();
        // Sort by volume desc, then swap_count desc, then principal asc
        // so ordering is deterministic when volume ties (important for tests).
        v.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.cmp(&a.1)).then(a.0.cmp(&b.0)));
        v.truncate(limit as usize);
        v
    });

    SWAPPERS_CACHE.with(|c| {
        c.borrow_mut().insert(cache_key, (now, response.clone()));
    });
    response
}

// ─── Top LPs ───
//
// Returns (holder, lp_shares, bps_of_total). Source is the current
// on-chain lp_shares map (no snapshot replay — AMM LP balances mutate
// atomically with each add/remove, so the live state is authoritative).

pub fn get_top_lps(query: AmmTopLpsQuery) -> Vec<(Principal, u128, u32)> {
    let limit = clamp_limit(query.limit);
    let cache_key = (query.pool.clone(), limit);
    let now = now_ns();

    let cached = LPS_CACHE.with(|c| c.borrow().get(&cache_key).cloned());
    if let Some((cached_at, response)) = cached {
        if now.saturating_sub(cached_at) < ANALYTICS_TTL_NS {
            return response;
        }
    }

    if !pool_exists(&query.pool) {
        return Vec::new();
    }

    let response = read_state(|s| {
        let pool = match s.pools.get(&query.pool) {
            Some(p) => p,
            None => return Vec::new(),
        };
        let total = pool.total_lp_shares.max(1);
        let mut v: Vec<(Principal, u128, u32)> = pool
            .lp_shares
            .iter()
            .map(|(who, shares)| {
                let bps = (shares.saturating_mul(10_000) / total) as u32;
                (*who, *shares, bps)
            })
            .collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v.truncate(limit as usize);
        v
    });

    LPS_CACHE.with(|c| {
        c.borrow_mut().insert(cache_key, (now, response.clone()));
    });
    response
}

// ─── Events-by-principal (pool-scoped) ───

pub fn get_swap_events_by_principal(query: AmmEventsByPrincipalQuery) -> Vec<AmmSwapEvent> {
    read_state(|s| {
        s.swap_events
            .iter()
            .filter(|e| e.pool_id == query.pool && e.caller == query.who)
            .skip(query.start as usize)
            .take(query.length as usize)
            .cloned()
            .collect()
    })
}

pub fn get_liquidity_events_by_principal(
    query: AmmEventsByPrincipalQuery,
) -> Vec<AmmLiquidityEvent> {
    read_state(|s| {
        s.liquidity_events
            .iter()
            .filter(|e| e.pool_id == query.pool && e.caller == query.who)
            .skip(query.start as usize)
            .take(query.length as usize)
            .cloned()
            .collect()
    })
}

// ─── Events-by-time-range (pool-scoped) ───

pub fn get_swap_events_by_time_range(query: AmmEventsByTimeRangeQuery) -> Vec<AmmSwapEvent> {
    read_state(|s| {
        s.swap_events
            .iter()
            .filter(|e| {
                e.pool_id == query.pool
                    && e.timestamp >= query.start_ns
                    && e.timestamp < query.end_ns
            })
            .take(query.limit as usize)
            .cloned()
            .collect()
    })
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{mutate_state, replace_state, AmmState};
    use crate::types::CurveType;
    use candid::Principal;
    use std::collections::BTreeMap;

    fn token_a() -> Principal {
        Principal::from_slice(&[10; 29])
    }
    fn token_b() -> Principal {
        Principal::from_slice(&[11; 29])
    }
    fn alice() -> Principal {
        Principal::from_slice(&[1; 29])
    }
    fn bob() -> Principal {
        Principal::from_slice(&[2; 29])
    }
    fn carol() -> Principal {
        Principal::from_slice(&[3; 29])
    }

    fn setup_pool(pool_id: &str) {
        let mut pool = Pool {
            token_a: token_a(),
            token_b: token_b(),
            reserve_a: 1_000_000_000,
            reserve_b: 2_000_000_000,
            fee_bps: 30,
            protocol_fee_bps: 0,
            curve: CurveType::ConstantProduct,
            lp_shares: BTreeMap::new(),
            total_lp_shares: 1_000_000,
            protocol_fees_a: 0,
            protocol_fees_b: 0,
            paused: false,
            subaccount_a: [0; 32],
            subaccount_b: [0; 32],
        };
        pool.lp_shares.insert(alice(), 600_000);
        pool.lp_shares.insert(bob(), 400_000);

        let mut state = AmmState::default();
        state.pools.insert(pool_id.to_string(), pool);
        replace_state(state);
        clear_all_caches();
    }

    fn push_swap(pool_id: &str, caller: Principal, ts_ns: u64, token_in_is_a: bool, amount_in: u128, amount_out: u128, fee: u128) {
        mutate_state(|s| {
            let pool = s.pools.get(pool_id).expect("pool exists");
            let (token_in, token_out) = if token_in_is_a {
                (pool.token_a, pool.token_b)
            } else {
                (pool.token_b, pool.token_a)
            };
            let id = s.next_swap_event_id;
            s.swap_events.push(AmmSwapEvent {
                id,
                caller,
                pool_id: pool_id.to_string(),
                token_in,
                amount_in,
                token_out,
                amount_out,
                fee,
                timestamp: ts_ns,
            });
            s.next_swap_event_id += 1;
        });
    }

    fn push_liquidity(pool_id: &str, caller: Principal, ts_ns: u64, add: bool, amount_a: u128, amount_b: u128, shares: u128) {
        mutate_state(|s| {
            let pool = s.pools.get(pool_id).expect("pool exists");
            let (ta, tb) = (pool.token_a, pool.token_b);
            let id = s.next_liquidity_event_id;
            s.liquidity_events.push(AmmLiquidityEvent {
                id,
                caller,
                pool_id: pool_id.to_string(),
                action: if add { AmmLiquidityAction::AddLiquidity } else { AmmLiquidityAction::RemoveLiquidity },
                token_a: ta,
                amount_a,
                token_b: tb,
                amount_b,
                lp_shares: shares,
                timestamp: ts_ns,
            });
            s.next_liquidity_event_id += 1;
        });
    }

    // Fix test time to a round value so bucket math is predictable.
    fn t0() -> u64 { 1_700_000_000 * NANOS_PER_SEC }
    fn minutes(n: u64) -> u64 { n * 60 * NANOS_PER_SEC }

    #[test]
    fn window_duration_resolves_correctly() {
        assert_eq!(window_duration_ns(AmmStatsWindow::Hour), Some(3_600 * NANOS_PER_SEC));
        assert_eq!(window_duration_ns(AmmStatsWindow::Day), Some(86_400 * NANOS_PER_SEC));
        assert_eq!(window_duration_ns(AmmStatsWindow::Week), Some(604_800 * NANOS_PER_SEC));
        assert_eq!(window_duration_ns(AmmStatsWindow::Month), Some(2_592_000 * NANOS_PER_SEC));
        assert_eq!(window_duration_ns(AmmStatsWindow::All), None);
    }

    #[test]
    fn clamp_points_enforces_bounds() {
        assert_eq!(clamp_points(0), SERIES_DEFAULT_POINTS);
        assert_eq!(clamp_points(1), 1);
        assert_eq!(clamp_points(500), 500);
        assert_eq!(clamp_points(600), 500);
    }

    #[test]
    fn clamp_limit_enforces_bounds() {
        assert_eq!(clamp_limit(0), TOP_DEFAULT_LIMIT);
        assert_eq!(clamp_limit(1), 1);
        assert_eq!(clamp_limit(200), 200);
        assert_eq!(clamp_limit(1000), 200);
    }

    #[test]
    fn empty_pool_returns_empty_series() {
        setup_pool("testpool");
        let now = t0() + minutes(30);
        set_test_time(now);
        let vol = get_volume_series(AmmSeriesQuery {
            pool: "testpool".into(),
            window: AmmStatsWindow::Hour,
            points: 10,
        });
        assert!(vol.is_empty(), "expected empty volume series, got {:?}", vol);
    }

    #[test]
    fn unknown_pool_returns_empty_series_and_empty_stats() {
        setup_pool("testpool");
        let now = t0() + minutes(30);
        set_test_time(now);
        let vol = get_volume_series(AmmSeriesQuery {
            pool: "missing".into(),
            window: AmmStatsWindow::Hour,
            points: 10,
        });
        assert!(vol.is_empty());

        let stats = get_pool_stats(AmmStatsQuery {
            pool: "missing".into(),
            window: AmmStatsWindow::Hour,
        });
        assert_eq!(stats.swap_count, 0);
        assert_eq!(stats.volume_a_e8s, 0);
    }

    #[test]
    fn volume_series_buckets_correctly() {
        setup_pool("testpool");
        let now = t0() + minutes(60);
        set_test_time(now);

        // 3 events across 2 buckets within a 1-hour window, 10 points → 6min buckets
        // Bucket 0 starts at window_start = now - 1h. Bucket 0 covers [window_start, window_start + 6min).
        // Place 2 events in bucket 0 (one a→b, one b→a) and 1 event in bucket 5.
        push_swap("testpool", alice(), t0() + minutes(2), true, 100, 90, 1);
        push_swap("testpool", bob(), t0() + minutes(4), false, 50, 25, 1);
        push_swap("testpool", alice(), t0() + minutes(32), true, 200, 180, 2);

        let series = get_volume_series(AmmSeriesQuery {
            pool: "testpool".into(),
            window: AmmStatsWindow::Hour,
            points: 10,
        });
        assert_eq!(series.len(), 2, "two non-empty buckets expected");

        // First bucket: 100+25 vol_a, 90+50 vol_b, 2 swaps
        assert_eq!(series[0].volume_a_e8s, 125);
        assert_eq!(series[0].volume_b_e8s, 140);
        assert_eq!(series[0].swap_count, 2);

        // Second bucket: 200 vol_a, 180 vol_b, 1 swap
        assert_eq!(series[1].volume_a_e8s, 200);
        assert_eq!(series[1].volume_b_e8s, 180);
        assert_eq!(series[1].swap_count, 1);
    }

    #[test]
    fn fee_series_attributes_to_input_token() {
        setup_pool("testpool");
        let now = t0() + minutes(60);
        set_test_time(now);

        push_swap("testpool", alice(), t0() + minutes(2), true, 100, 90, 3);  // fee on a
        push_swap("testpool", bob(), t0() + minutes(4), false, 50, 25, 1);    // fee on b
        push_swap("testpool", carol(), t0() + minutes(30), true, 200, 180, 6); // fee on a, different bucket

        let series = get_fee_series(AmmSeriesQuery {
            pool: "testpool".into(),
            window: AmmStatsWindow::Hour,
            points: 10,
        });
        assert_eq!(series.len(), 2);
        assert_eq!(series[0].fees_a_e8s, 3);
        assert_eq!(series[0].fees_b_e8s, 1);
        assert_eq!(series[1].fees_a_e8s, 6);
        assert_eq!(series[1].fees_b_e8s, 0);
    }

    #[test]
    fn top_swappers_sorts_by_volume_desc() {
        setup_pool("testpool");
        let now = t0() + minutes(60);
        set_test_time(now);

        push_swap("testpool", alice(), t0() + minutes(1), true, 300, 280, 9);
        push_swap("testpool", bob(), t0() + minutes(2), true, 500, 450, 15);
        push_swap("testpool", alice(), t0() + minutes(3), false, 100, 50, 1);
        push_swap("testpool", carol(), t0() + minutes(4), true, 100, 90, 3);

        let top = get_top_swappers(AmmTopSwappersQuery {
            pool: "testpool".into(),
            window: AmmStatsWindow::Hour,
            limit: 10,
        });
        assert_eq!(top.len(), 3);
        // bob=500, alice=300+50 (50 is amount_out from b→a swap)=350, carol=100
        assert_eq!(top[0].0, bob());
        assert_eq!(top[0].1, 1);
        assert_eq!(top[0].2, 500);
        assert_eq!(top[1].0, alice());
        assert_eq!(top[1].1, 2);
        assert_eq!(top[1].2, 350);
        assert_eq!(top[2].0, carol());
        assert_eq!(top[2].2, 100);
    }

    #[test]
    fn top_swappers_respects_limit() {
        setup_pool("testpool");
        let now = t0() + minutes(60);
        set_test_time(now);
        push_swap("testpool", alice(), t0() + minutes(1), true, 300, 280, 9);
        push_swap("testpool", bob(), t0() + minutes(2), true, 500, 450, 15);
        push_swap("testpool", carol(), t0() + minutes(3), true, 100, 90, 3);

        let top = get_top_swappers(AmmTopSwappersQuery {
            pool: "testpool".into(),
            window: AmmStatsWindow::Hour,
            limit: 2,
        });
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].0, bob());
        assert_eq!(top[1].0, alice());
    }

    #[test]
    fn top_lps_uses_current_shares_sorted_desc() {
        setup_pool("testpool");
        set_test_time(t0() + minutes(1));
        let top = get_top_lps(AmmTopLpsQuery {
            pool: "testpool".into(),
            limit: 10,
        });
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].0, alice());
        assert_eq!(top[0].1, 600_000);
        assert_eq!(top[0].2, 6_000); // 60% of 10_000 bps
        assert_eq!(top[1].0, bob());
        assert_eq!(top[1].1, 400_000);
        assert_eq!(top[1].2, 4_000);
    }

    #[test]
    fn pool_stats_aggregates_across_window() {
        setup_pool("testpool");
        let now = t0() + minutes(60);
        set_test_time(now);

        push_swap("testpool", alice(), t0() + minutes(5), true, 100, 90, 2);
        push_swap("testpool", bob(), t0() + minutes(10), false, 200, 180, 5);
        push_swap("testpool", alice(), t0() + minutes(50), true, 50, 45, 1);

        let stats = get_pool_stats(AmmStatsQuery {
            pool: "testpool".into(),
            window: AmmStatsWindow::Hour,
        });

        assert_eq!(stats.swap_count, 3);
        assert_eq!(stats.unique_swappers, 2);
        assert_eq!(stats.volume_a_e8s, 100 + 180 + 50); // a_in + a_out (from b→a) + a_in
        assert_eq!(stats.volume_b_e8s, 90 + 200 + 45);
        assert_eq!(stats.fees_a_e8s, 2 + 1);
        assert_eq!(stats.fees_b_e8s, 5);
        assert_eq!(stats.unique_lps, 2);
    }

    #[test]
    fn events_by_principal_filters_by_pool_and_caller() {
        setup_pool("pool_a");
        setup_pool("pool_b"); // second setup wipes state; re-add pool_a
        mutate_state(|s| {
            let pool = s.pools.get("pool_b").cloned().unwrap();
            s.pools.insert("pool_a".into(), pool);
        });
        set_test_time(t0() + minutes(60));

        push_swap("pool_a", alice(), t0() + minutes(1), true, 100, 90, 1);
        push_swap("pool_a", bob(), t0() + minutes(2), true, 200, 180, 2);
        push_swap("pool_b", alice(), t0() + minutes(3), true, 300, 280, 3);

        let alice_in_a = get_swap_events_by_principal(AmmEventsByPrincipalQuery {
            pool: "pool_a".into(),
            who: alice(),
            start: 0,
            length: 100,
        });
        assert_eq!(alice_in_a.len(), 1);
        assert_eq!(alice_in_a[0].amount_in, 100);

        let bob_in_b = get_swap_events_by_principal(AmmEventsByPrincipalQuery {
            pool: "pool_b".into(),
            who: bob(),
            start: 0,
            length: 100,
        });
        assert!(bob_in_b.is_empty());
    }

    #[test]
    fn events_by_time_range_filters_inclusive_start_exclusive_end() {
        setup_pool("testpool");
        set_test_time(t0() + minutes(60));

        push_swap("testpool", alice(), t0() + minutes(1), true, 100, 90, 1);
        push_swap("testpool", bob(), t0() + minutes(5), true, 200, 180, 2);
        push_swap("testpool", carol(), t0() + minutes(10), true, 300, 280, 3);

        let events = get_swap_events_by_time_range(AmmEventsByTimeRangeQuery {
            pool: "testpool".into(),
            start_ns: t0() + minutes(5),
            end_ns: t0() + minutes(10),
            limit: 100,
        });
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].caller, bob());
    }

    #[test]
    fn balance_series_rewinds_swaps_back_in_time() {
        setup_pool("testpool");
        // reserves start at (1_000_000_000, 2_000_000_000)
        let now = t0() + minutes(60);
        set_test_time(now);

        // a→b: reserve_a +100, reserve_b -90 (so before event: a=1B-100, b=2B+90)
        push_swap("testpool", alice(), t0() + minutes(30), true, 100, 90, 0);

        let series = get_balance_series(AmmSeriesQuery {
            pool: "testpool".into(),
            window: AmmStatsWindow::Hour,
            points: 10,
        });
        assert_eq!(series.len(), 10);

        // Most recent point (end of series) should be close to current reserves.
        let last = series.last().unwrap();
        assert_eq!(last.reserve_a_e8s, 1_000_000_000);
        assert_eq!(last.reserve_b_e8s, 2_000_000_000);

        // Earliest point should reflect the state BEFORE the swap.
        let first = series.first().unwrap();
        assert_eq!(first.reserve_a_e8s, 1_000_000_000 - 100);
        assert_eq!(first.reserve_b_e8s, 2_000_000_000 + 90);
    }

    #[test]
    fn balance_series_handles_liquidity_events() {
        setup_pool("testpool");
        let now = t0() + minutes(60);
        set_test_time(now);

        // Add liquidity event +500 a, +1000 b (so before: a=1B-500, b=2B-1000)
        push_liquidity("testpool", alice(), t0() + minutes(30), true, 500, 1000, 10_000);

        let series = get_balance_series(AmmSeriesQuery {
            pool: "testpool".into(),
            window: AmmStatsWindow::Hour,
            points: 10,
        });
        let first = series.first().unwrap();
        assert_eq!(first.reserve_a_e8s, 1_000_000_000 - 500);
        assert_eq!(first.reserve_b_e8s, 2_000_000_000 - 1000);
    }

    #[test]
    fn cache_returns_same_response_within_ttl_then_invalidates() {
        setup_pool("testpool");
        let now = t0() + minutes(60);
        set_test_time(now);

        push_swap("testpool", alice(), t0() + minutes(10), true, 100, 90, 1);

        let first = get_volume_series(AmmSeriesQuery {
            pool: "testpool".into(),
            window: AmmStatsWindow::Hour,
            points: 10,
        });
        assert_eq!(first.len(), 1);

        // Add another swap; without invalidation the cached result is returned.
        push_swap("testpool", bob(), t0() + minutes(20), true, 500, 450, 3);

        let cached_again = get_volume_series(AmmSeriesQuery {
            pool: "testpool".into(),
            window: AmmStatsWindow::Hour,
            points: 10,
        });
        assert_eq!(cached_again.len(), 1, "cache should still return stale result inside TTL");

        invalidate_cache_for_pool(&"testpool".to_string());

        let fresh = get_volume_series(AmmSeriesQuery {
            pool: "testpool".into(),
            window: AmmStatsWindow::Hour,
            points: 10,
        });
        assert_eq!(fresh.len(), 2, "after invalidation, new event appears");
    }
}
