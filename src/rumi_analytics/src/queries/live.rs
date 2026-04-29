//! Live computed analytics queries. Pure functions that read from StableLogs
//! and return computed results. Each public function has a corresponding
//! pure computation helper (prefixed `compute_`) that accepts slices for
//! unit testing.

use candid::Principal;
use std::collections::{HashMap, HashSet};
use crate::{state, storage, types};

const DEFAULT_BUCKET_SECS: u64 = 3_600;
const DEFAULT_TWAP_WINDOW_SECS: u64 = 3_600;
const DEFAULT_VOL_WINDOW_SECS: u64 = 86_400;
const DEFAULT_APY_WINDOW_DAYS: u32 = 7;
const DEFAULT_TRADE_WINDOW_SECS: u64 = 86_400;
const NANOS_PER_SEC: u64 = 1_000_000_000;

/// Safety cap on snapshot loads to avoid blowing instruction limits.
/// 20,000 fast snapshots = ~69 days at 5-min intervals (plenty for any query window).
const MAX_SNAPSHOT_LOAD: usize = 20_000;

// ─── OHLC ───

pub fn get_ohlc(query: types::OhlcQuery) -> types::OhlcResponse {
    let bucket_secs = query.bucket_secs.unwrap_or(DEFAULT_BUCKET_SECS);
    let from = query.from_ts.unwrap_or(0);
    let to = query.to_ts.unwrap_or(u64::MAX);
    let limit = query.limit.unwrap_or(500).min(2000) as usize;

    let snapshots = storage::fast::fast_prices::range(from, to, MAX_SNAPSHOT_LOAD);

    let (candles, symbol) = compute_ohlc(&snapshots, query.collateral, bucket_secs, limit);

    types::OhlcResponse {
        candles,
        collateral: query.collateral,
        symbol,
        bucket_secs,
    }
}

pub fn compute_ohlc(
    snapshots: &[storage::fast::FastPriceSnapshot],
    collateral: Principal,
    bucket_secs: u64,
    limit: usize,
) -> (Vec<types::OhlcCandle>, String) {
    let bucket_ns = bucket_secs.saturating_mul(NANOS_PER_SEC);
    if bucket_ns == 0 || snapshots.is_empty() {
        return (vec![], String::new());
    }

    let mut symbol = String::new();
    let mut prices: Vec<(u64, f64)> = Vec::new();
    for snap in snapshots {
        for (p, price, sym) in &snap.prices {
            if *p == collateral {
                prices.push((snap.timestamp_ns, *price));
                if symbol.is_empty() {
                    symbol = sym.clone();
                }
                break;
            }
        }
    }

    if prices.is_empty() {
        return (vec![], symbol);
    }

    let mut candles: Vec<types::OhlcCandle> = Vec::new();
    let first_ts = prices[0].0;
    let bucket_start = first_ts - (first_ts % bucket_ns);
    let mut current_bucket = bucket_start;
    let mut open = prices[0].1;
    let mut high = open;
    let mut low = open;
    let mut close = open;

    for &(ts, price) in &prices {
        if ts >= current_bucket + bucket_ns {
            candles.push(types::OhlcCandle {
                timestamp_ns: current_bucket,
                open, high, low, close,
            });
            if candles.len() >= limit {
                return (candles, symbol);
            }
            current_bucket = ts - (ts % bucket_ns);
            open = price;
            high = price;
            low = price;
            close = price;
        } else {
            if price > high { high = price; }
            if price < low { low = price; }
            close = price;
        }
    }

    if candles.len() < limit {
        candles.push(types::OhlcCandle {
            timestamp_ns: current_bucket,
            open, high, low, close,
        });
    }

    (candles, symbol)
}

// ─── TWAP ───

pub fn get_twap(query: types::TwapQuery) -> types::TwapResponse {
    let window_secs = query.window_secs.unwrap_or(DEFAULT_TWAP_WINDOW_SECS);
    let now = ic_cdk::api::time();
    let from = now.saturating_sub(window_secs.saturating_mul(NANOS_PER_SEC));

    let snapshots = storage::fast::fast_prices::range(from, now, MAX_SNAPSHOT_LOAD);
    let entries = compute_twap(&snapshots);

    types::TwapResponse { entries, window_secs }
}

pub fn compute_twap(
    snapshots: &[storage::fast::FastPriceSnapshot],
) -> Vec<types::TwapEntry> {
    if snapshots.is_empty() {
        return vec![];
    }

    let mut acc: HashMap<Principal, (f64, u32, f64, String)> = HashMap::new();

    for snap in snapshots {
        for (p, price, sym) in &snap.prices {
            let entry = acc.entry(*p).or_insert((0.0, 0, 0.0, sym.clone()));
            entry.0 += price;
            entry.1 += 1;
            entry.2 = *price;
        }
    }

    let mut entries: Vec<types::TwapEntry> = acc
        .into_iter()
        .map(|(collateral, (sum, count, latest, symbol))| {
            types::TwapEntry {
                collateral,
                symbol,
                twap_price: if count > 0 { sum / count as f64 } else { 0.0 },
                latest_price: latest,
                sample_count: count,
            }
        })
        .collect();

    entries.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    entries
}

// ─── Realized Volatility ───

pub fn get_volatility(query: types::VolatilityQuery) -> types::VolatilityResponse {
    let window_secs = query.window_secs.unwrap_or(DEFAULT_VOL_WINDOW_SECS);
    let now = ic_cdk::api::time();
    let from = now.saturating_sub(window_secs.saturating_mul(NANOS_PER_SEC));

    let snapshots = storage::fast::fast_prices::range(from, now, MAX_SNAPSHOT_LOAD);
    let (vol, count, symbol) = compute_volatility(&snapshots, query.collateral);

    types::VolatilityResponse {
        collateral: query.collateral,
        symbol,
        annualized_vol_pct: vol,
        sample_count: count,
        window_secs,
    }
}

/// Returns (annualized_vol_pct, sample_count, symbol).
pub fn compute_volatility(
    snapshots: &[storage::fast::FastPriceSnapshot],
    collateral: Principal,
) -> (f64, u32, String) {
    let mut symbol = String::new();
    let mut prices: Vec<f64> = Vec::new();

    for snap in snapshots {
        for (p, price, sym) in &snap.prices {
            if *p == collateral && *price > 0.0 {
                prices.push(*price);
                if symbol.is_empty() {
                    symbol = sym.clone();
                }
                break;
            }
        }
    }

    if prices.len() < 2 {
        return (0.0, prices.len() as u32, symbol);
    }

    let log_returns: Vec<f64> = prices
        .windows(2)
        .map(|w| (w[1] / w[0]).ln())
        .collect();

    let n = log_returns.len() as f64;
    let mean = log_returns.iter().sum::<f64>() / n;
    let variance = log_returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / n;
    let std_dev = variance.sqrt();

    // Annualize: 5-minute intervals -> 105,120 intervals per year (365 * 24 * 12).
    let intervals_per_year: f64 = 365.0 * 24.0 * 12.0;
    let annualized = std_dev * intervals_per_year.sqrt() * 100.0;

    (annualized, prices.len() as u32, symbol)
}

// ─── Peg Deviation ───

pub fn get_peg_status() -> Option<types::PegStatus> {
    let n = storage::fast::fast_3pool::len();
    if n == 0 {
        return None;
    }
    let snap = storage::fast::fast_3pool::get(n - 1)?;
    Some(compute_peg_status(&snap))
}

pub fn compute_peg_status(snap: &storage::fast::Fast3PoolSnapshot) -> types::PegStatus {
    // Normalize all balances to a common scale (highest decimal). Legacy rows
    // that pre-date the decimals field fall back to 8 per token (3pool standard).
    let decimals = snap.decimals.as_deref().unwrap_or(&[]);
    let max_dec = decimals.iter().copied().max().unwrap_or(8);
    let normalized: Vec<u128> = snap.balances.iter()
        .enumerate()
        .map(|(i, b)| {
            let dec = decimals.get(i).copied().unwrap_or(max_dec);
            let scale = 10u128.pow((max_dec - dec) as u32);
            b * scale
        })
        .collect();

    let total: u128 = normalized.iter().sum();
    let count = normalized.len();

    let (balance_ratios, max_imbalance_pct) = if count > 0 && total > 0 {
        let target = total as f64 / count as f64;
        let ratios: Vec<f64> = normalized.iter()
            .map(|b| *b as f64 / target)
            .collect();
        let max_dev = ratios.iter()
            .map(|r| (r - 1.0).abs())
            .fold(0.0f64, f64::max);
        (ratios, max_dev * 100.0)
    } else {
        (vec![], 0.0)
    };

    types::PegStatus {
        timestamp_ns: snap.timestamp_ns,
        pool_balances: snap.balances.clone(),
        virtual_price: snap.virtual_price,
        balance_ratios,
        max_imbalance_pct,
    }
}

// ─── APY ───

pub fn get_apys(query: types::ApyQuery) -> types::ApyResponse {
    let window_days = query.window_days.unwrap_or(DEFAULT_APY_WINDOW_DAYS).max(1);
    let now = ic_cdk::api::time();
    let window_ns = (window_days as u64).saturating_mul(86_400).saturating_mul(NANOS_PER_SEC);
    let from = now.saturating_sub(window_ns);

    let swap_rollups = storage::rollups::daily_swaps::range(from, now, MAX_SNAPSHOT_LOAD);
    let tvl_rows = storage::daily_tvl::range(from, now, MAX_SNAPSHOT_LOAD);
    let stability_rows = storage::daily_stability::range(from, now, MAX_SNAPSHOT_LOAD);

    let lp_apy = compute_lp_apy(&swap_rollups, &tvl_rows, window_days);
    let sp_apy = compute_sp_apy(&stability_rows, window_days);

    types::ApyResponse {
        lp_apy_pct: lp_apy,
        sp_apy_pct: sp_apy,
        window_days,
    }
}

/// 3pool LP APY: annualized swap fee yield.
/// APY = (total_swap_fees / days) / avg_pool_tvl * 365 * 100
pub fn compute_lp_apy(
    swap_rollups: &[storage::rollups::DailySwapRollup],
    tvl_rows: &[storage::DailyTvlRow],
    window_days: u32,
) -> Option<f64> {
    if swap_rollups.is_empty() || tvl_rows.is_empty() {
        return None;
    }

    let total_fees: u64 = swap_rollups.iter()
        .map(|r| r.three_pool_fees_e8s)
        .sum();

    // Average 3pool TVL from daily TVL rows (sum of all 3 reserves).
    let tvl_sum: f64 = tvl_rows.iter()
        .filter_map(|r| {
            let r0 = r.three_pool_reserve_0_e8s? as f64;
            let r1 = r.three_pool_reserve_1_e8s? as f64;
            let r2 = r.three_pool_reserve_2_e8s? as f64;
            Some(r0 + r1 + r2)
        })
        .sum();

    let tvl_count = tvl_rows.iter()
        .filter(|r| r.three_pool_reserve_0_e8s.is_some())
        .count();

    if tvl_count == 0 {
        return None;
    }

    let avg_tvl = tvl_sum / tvl_count as f64;
    if avg_tvl <= 0.0 {
        return None;
    }

    let daily_fees = total_fees as f64 / window_days as f64;
    let apy = (daily_fees / avg_tvl) * 365.0 * 100.0;
    Some(apy)
}

/// Stability pool APY: annualized interest yield.
/// `total_interest_received_e8s` is cumulative, so we take the delta between
/// the last and first snapshots in the window.
/// APY = (interest_delta / days) / avg_deposits * 365 * 100
pub fn compute_sp_apy(
    stability_rows: &[storage::DailyStabilityRow],
    window_days: u32,
) -> Option<f64> {
    if stability_rows.len() < 2 {
        return None;
    }

    let first = stability_rows.first().unwrap();
    let last = stability_rows.last().unwrap();
    let interest_delta = last.total_interest_received_e8s
        .saturating_sub(first.total_interest_received_e8s);

    let avg_deposits: f64 = stability_rows.iter()
        .map(|r| r.total_deposits_e8s as f64)
        .sum::<f64>() / stability_rows.len() as f64;

    if avg_deposits <= 0.0 {
        return None;
    }

    let daily_interest = interest_delta as f64 / window_days as f64;
    let apy = (daily_interest / avg_deposits) * 365.0 * 100.0;
    Some(apy)
}

// ─── Trade Activity ───

pub fn get_trade_activity(query: types::TradeActivityQuery) -> types::TradeActivityResponse {
    let window_secs = query.window_secs.unwrap_or(DEFAULT_TRADE_WINDOW_SECS);
    let now = ic_cdk::api::time();
    let from = now.saturating_sub(window_secs.saturating_mul(NANOS_PER_SEC));

    let events = storage::events::evt_swaps::range(from, now, MAX_SNAPSHOT_LOAD);
    compute_trade_activity(&events, window_secs)
}

pub fn compute_trade_activity(
    events: &[storage::events::AnalyticsSwapEvent],
    window_secs: u64,
) -> types::TradeActivityResponse {
    let mut tp_count: u32 = 0;
    let mut amm_count: u32 = 0;
    let mut total_volume: u64 = 0;
    let mut total_fees: u64 = 0;
    let mut traders: HashSet<Principal> = HashSet::new();

    for e in events {
        traders.insert(e.caller);
        total_volume = total_volume.saturating_add(e.amount_in);
        total_fees = total_fees.saturating_add(e.fee);
        match e.source {
            storage::events::SwapSource::ThreePool => tp_count += 1,
            storage::events::SwapSource::Amm => amm_count += 1,
        }
    }

    let total = tp_count + amm_count;
    let avg_size = if total > 0 { total_volume / total as u64 } else { 0 };

    types::TradeActivityResponse {
        window_secs,
        total_swaps: total,
        three_pool_swaps: tp_count,
        amm_swaps: amm_count,
        total_volume_e8s: total_volume,
        total_fees_e8s: total_fees,
        unique_traders: traders.len() as u32,
        avg_trade_size_e8s: avg_size,
    }
}

// ─── Top Holders ───

/// TTL for the top-holders cache. Sorting can touch tens of thousands of
/// accounts, so we serve repeat callers from cache for one minute.
const TOP_HOLDERS_TTL_NS: u64 = 60 * NANOS_PER_SEC;

const TOP_HOLDERS_DEFAULT_LIMIT: u32 = 50;
const TOP_HOLDERS_MAX_LIMIT: u32 = 200;

thread_local! {
    static TOP_HOLDERS_CACHE: std::cell::RefCell<HashMap<(Principal, u32), (u64, types::TopHoldersResponse)>> =
        std::cell::RefCell::new(HashMap::new());
}

fn resolve_token(token: Principal) -> Option<storage::balance_tracker::Token> {
    let (icusd_ledger, three_pool) = state::read_state(|s| (s.sources.icusd_ledger, s.sources.three_pool));
    if token == icusd_ledger {
        Some(storage::balance_tracker::Token::IcUsd)
    } else if token == three_pool {
        Some(storage::balance_tracker::Token::ThreeUsd)
    } else {
        None
    }
}

/// Invalidate the cached entries for a token. Call after the holder snapshot
/// collector writes a new sample so the next caller sees fresh data.
#[allow(dead_code)]
pub fn invalidate_top_holders_cache(token: Principal) {
    TOP_HOLDERS_CACHE.with(|cache| {
        cache.borrow_mut().retain(|(t, _), _| *t != token);
    });
}

pub fn get_top_holders(query: types::TopHoldersQuery) -> types::TopHoldersResponse {
    let limit_raw = query.limit.unwrap_or(TOP_HOLDERS_DEFAULT_LIMIT);
    let limit = if limit_raw == 0 { TOP_HOLDERS_DEFAULT_LIMIT } else { limit_raw }
        .clamp(1, TOP_HOLDERS_MAX_LIMIT);

    let now = ic_cdk::api::time();
    let cache_key = (query.token, limit);

    let cached = TOP_HOLDERS_CACHE.with(|cache| {
        cache.borrow().get(&cache_key).cloned()
    });
    if let Some((cached_at, response)) = cached {
        if now.saturating_sub(cached_at) < TOP_HOLDERS_TTL_NS {
            return response;
        }
    }

    let response = match resolve_token(query.token) {
        Some(token) => {
            let all = storage::balance_tracker::all_balances(token);
            let total_supply = storage::balance_tracker::total_supply_tracked(token);
            let by_principal = aggregate_by_principal(&all);
            compute_top_holders(&by_principal, total_supply, limit as usize, query.token, now, "balance_tracker")
        }
        None => types::TopHoldersResponse {
            token: query.token,
            total_holders: 0,
            total_supply_e8s: 0,
            generated_at_ns: now,
            rows: Vec::new(),
            source: "unsupported".to_string(),
        },
    };

    TOP_HOLDERS_CACHE.with(|cache| {
        cache.borrow_mut().insert(cache_key, (now, response.clone()));
    });

    response
}

/// Sum balances per owner, dropping subaccount distinctions. Returned as an
/// unsorted vector for the caller to rank.
fn aggregate_by_principal(
    accounts: &[(storage::balance_tracker::Account, u64)],
) -> Vec<(Principal, u64)> {
    let mut by_principal: HashMap<Principal, u64> = HashMap::new();
    for (acct, balance) in accounts {
        let entry = by_principal.entry(acct.owner).or_insert(0);
        *entry = entry.saturating_add(*balance);
    }
    by_principal.into_iter().collect()
}

/// Pure helper: rank `holders` desc, take `limit`, compute share basis points
/// against `total_supply`. Falls back to the sum of ranked balances when
/// `total_supply` is zero, so share_bps stays meaningful for small datasets.
pub fn compute_top_holders(
    holders: &[(Principal, u64)],
    total_supply: u64,
    limit: usize,
    token: Principal,
    now_ns: u64,
    source: &str,
) -> types::TopHoldersResponse {
    let mut sorted = holders.to_vec();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    let total_holders = sorted.len() as u32;
    sorted.truncate(limit);

    let denom: u128 = if total_supply > 0 {
        total_supply as u128
    } else {
        sorted.iter().map(|(_, b)| *b as u128).sum::<u128>().max(1)
    };

    let rows: Vec<types::TopHolderRow> = sorted.into_iter().map(|(principal, balance)| {
        let share = ((balance as u128).saturating_mul(10_000) / denom).min(10_000) as u32;
        types::TopHolderRow {
            principal,
            balance_e8s: balance,
            share_bps: share,
        }
    }).collect();

    types::TopHoldersResponse {
        token,
        total_holders,
        total_supply_e8s: total_supply,
        generated_at_ns: now_ns,
        rows,
        source: source.to_string(),
    }
}

// ─── Top Counterparties ───

/// Default lookback when callers don't pass a window: 30 days.
const DEFAULT_WINDOW_NS: u64 = 30 * 86_400 * NANOS_PER_SEC;

/// Principal-ranking cache TTL: 30s.
const TOP_PRINCIPALS_TTL_NS: u64 = 30 * NANOS_PER_SEC;

/// Admin-breakdown cache TTL: 5 minutes (admin events are rare).
const ADMIN_BREAKDOWN_TTL_NS: u64 = 5 * 60 * NANOS_PER_SEC;

const TOP_PRINCIPALS_DEFAULT_LIMIT: u32 = 50;
const TOP_PRINCIPALS_MAX_LIMIT: u32 = 200;
const MAX_EVENT_LOAD: usize = 50_000;

thread_local! {
    static TOP_COUNTERPARTIES_CACHE: std::cell::RefCell<HashMap<(Principal, u64, u32), (u64, types::TopCounterpartiesResponse)>> =
        std::cell::RefCell::new(HashMap::new());
    static TOP_SP_DEPOSITORS_CACHE: std::cell::RefCell<HashMap<(u64, u32), (u64, types::TopSpDepositorsResponse)>> =
        std::cell::RefCell::new(HashMap::new());
    static ADMIN_BREAKDOWN_CACHE: std::cell::RefCell<HashMap<u64, (u64, types::AdminEventBreakdownResponse)>> =
        std::cell::RefCell::new(HashMap::new());
}

fn resolve_window_ns(window_ns: Option<u64>) -> u64 {
    match window_ns {
        Some(0) => DEFAULT_WINDOW_NS,
        Some(w) => w,
        None => DEFAULT_WINDOW_NS,
    }
}

/// Whether a cached entry stamped at `cached_at_ns` is still fresh relative to
/// `now_ns` and a `ttl_ns` lifetime. Extracted so cache behavior is unit-
/// testable without needing a canister runtime.
pub fn cache_is_fresh(cached_at_ns: u64, now_ns: u64, ttl_ns: u64) -> bool {
    now_ns.saturating_sub(cached_at_ns) < ttl_ns
}

fn resolve_principal_limit(limit: Option<u32>) -> u32 {
    let raw = limit.unwrap_or(TOP_PRINCIPALS_DEFAULT_LIMIT);
    if raw == 0 { TOP_PRINCIPALS_DEFAULT_LIMIT } else { raw }
        .clamp(1, TOP_PRINCIPALS_MAX_LIMIT)
}

pub fn get_top_counterparties(query: types::TopCounterpartiesQuery) -> types::TopCounterpartiesResponse {
    let window_ns = resolve_window_ns(query.window_ns);
    let limit = resolve_principal_limit(query.limit);
    let now = ic_cdk::api::time();
    let cache_key = (query.principal, window_ns, limit);

    if let Some((ts, resp)) = TOP_COUNTERPARTIES_CACHE.with(|c| c.borrow().get(&cache_key).cloned()) {
        if cache_is_fresh(ts, now, TOP_PRINCIPALS_TTL_NS) {
            return resp;
        }
    }

    let from = now.saturating_sub(window_ns);
    let vault_evs = storage::events::evt_vaults::range(from, now, MAX_EVENT_LOAD);
    let swap_evs = storage::events::evt_swaps::range(from, now, MAX_EVENT_LOAD);
    let liq_evs = storage::events::evt_liquidity::range(from, now, MAX_EVENT_LOAD);
    let stab_evs = storage::events::evt_stability::range(from, now, MAX_EVENT_LOAD);

    let (three_pool, amm) = state::read_state(|s| (s.sources.three_pool, s.sources.amm));
    let sp = state::read_state(|s| s.sources.stability_pool);

    // Build vault_id -> vault_owner from Opened events in this window. For
    // historical redemption-vs-owner relationships we rely on open events also
    // being in range; acceptable for the default window.
    let mut vault_owners: HashMap<u64, Principal> = HashMap::new();
    for e in &vault_evs {
        if matches!(e.event_kind, storage::events::VaultEventKind::Opened) {
            vault_owners.insert(e.vault_id, e.owner);
        }
    }

    let rows = compute_top_counterparties(
        query.principal,
        &vault_evs,
        &swap_evs,
        &liq_evs,
        &stab_evs,
        &vault_owners,
        three_pool,
        amm,
        sp,
        limit as usize,
    );

    let resp = types::TopCounterpartiesResponse {
        principal: query.principal,
        window_ns,
        generated_at_ns: now,
        rows,
    };
    TOP_COUNTERPARTIES_CACHE.with(|c| {
        c.borrow_mut().insert(cache_key, (now, resp.clone()));
    });
    resp
}

#[allow(clippy::too_many_arguments)]
pub fn compute_top_counterparties(
    target: Principal,
    vault_events: &[storage::events::AnalyticsVaultEvent],
    swap_events: &[storage::events::AnalyticsSwapEvent],
    liquidity_events: &[storage::events::AnalyticsLiquidityEvent],
    stability_events: &[storage::events::AnalyticsStabilityEvent],
    vault_owners: &HashMap<u64, Principal>,
    three_pool: Principal,
    amm: Principal,
    stability_pool: Principal,
    limit: usize,
) -> Vec<types::TopCounterpartyRow> {
    let mut cp: HashMap<Principal, (u64, u64)> = HashMap::new();
    let mut bump = |p: Principal, vol: u64| {
        let entry = cp.entry(p).or_insert((0, 0));
        entry.0 = entry.0.saturating_add(1);
        entry.1 = entry.1.saturating_add(vol);
    };

    for e in vault_events {
        let owner_of_vault = vault_owners.get(&e.vault_id).copied();
        let ev_actor = e.owner;
        if ev_actor == target {
            // Target acted on someone else's vault (e.g. a redemption) → the
            // vault owner is the counterparty.
            if let Some(actual_owner) = owner_of_vault {
                if actual_owner != target {
                    bump(actual_owner, e.amount);
                }
            }
        } else if owner_of_vault == Some(target) {
            // Someone else touched target's vault (liquidator, redeemer).
            bump(ev_actor, e.amount);
        }
    }

    for e in swap_events {
        if e.caller == target {
            let pool = match e.source {
                storage::events::SwapSource::ThreePool => three_pool,
                storage::events::SwapSource::Amm => amm,
            };
            bump(pool, e.amount_in);
        }
    }

    for e in liquidity_events {
        if e.caller == target {
            let vol: u64 = e.amounts.iter().copied().fold(0u64, |acc, a| acc.saturating_add(a));
            bump(three_pool, vol);
        }
    }

    for e in stability_events {
        if e.caller == target {
            bump(stability_pool, e.amount);
        }
    }

    let mut rows: Vec<(Principal, u64, u64)> = cp.into_iter()
        .map(|(p, (c, v))| (p, c, v))
        .collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)));
    rows.truncate(limit);
    rows.into_iter().map(|(counterparty, interaction_count, volume_e8s)| {
        types::TopCounterpartyRow { counterparty, interaction_count, volume_e8s }
    }).collect()
}

/// Invalidate the cached counterparties for a principal. Kept for parity with
/// PR #87's `invalidate_top_holders_cache`.
#[allow(dead_code)]
pub fn invalidate_top_counterparties_cache(principal: Principal) {
    TOP_COUNTERPARTIES_CACHE.with(|c| {
        c.borrow_mut().retain(|(p, _, _), _| *p != principal);
    });
}

// ─── Top Stability Pool Depositors ───

pub fn get_top_sp_depositors(query: types::TopSpDepositorsQuery) -> types::TopSpDepositorsResponse {
    let window_ns = resolve_window_ns(query.window_ns);
    let limit = resolve_principal_limit(query.limit);
    let now = ic_cdk::api::time();
    let cache_key = (window_ns, limit);

    if let Some((ts, resp)) = TOP_SP_DEPOSITORS_CACHE.with(|c| c.borrow().get(&cache_key).cloned()) {
        if cache_is_fresh(ts, now, TOP_PRINCIPALS_TTL_NS) {
            return resp;
        }
    }

    let from = now.saturating_sub(window_ns);
    let window_evs = storage::events::evt_stability::range(from, now, MAX_EVENT_LOAD);
    let all_evs = storage::events::evt_stability::range(0, now, MAX_EVENT_LOAD);

    let rows = compute_top_sp_depositors(&window_evs, &all_evs, limit as usize);

    let resp = types::TopSpDepositorsResponse {
        window_ns,
        generated_at_ns: now,
        rows,
    };
    TOP_SP_DEPOSITORS_CACHE.with(|c| {
        c.borrow_mut().insert(cache_key, (now, resp.clone()));
    });
    resp
}

pub fn compute_top_sp_depositors(
    window_events: &[storage::events::AnalyticsStabilityEvent],
    all_events: &[storage::events::AnalyticsStabilityEvent],
    limit: usize,
) -> Vec<types::TopSpDepositorRow> {
    use storage::events::StabilityAction;

    // Windowed totals per principal.
    let mut window_stats: HashMap<Principal, (u64, i64)> = HashMap::new();
    for e in window_events {
        let entry = window_stats.entry(e.caller).or_insert((0, 0));
        match e.action {
            StabilityAction::Deposit => {
                entry.0 = entry.0.saturating_add(e.amount);
                entry.1 = entry.1.saturating_add(e.amount as i64);
            }
            StabilityAction::Withdraw => {
                entry.1 = entry.1.saturating_sub(e.amount as i64);
            }
            StabilityAction::ClaimReturns => {}
        }
    }

    // All-time net balance per principal.
    let mut lifetime_net: HashMap<Principal, i64> = HashMap::new();
    for e in all_events {
        let entry = lifetime_net.entry(e.caller).or_insert(0);
        match e.action {
            StabilityAction::Deposit => *entry = entry.saturating_add(e.amount as i64),
            StabilityAction::Withdraw => *entry = entry.saturating_sub(e.amount as i64),
            StabilityAction::ClaimReturns => {}
        }
    }

    let mut rows: Vec<types::TopSpDepositorRow> = window_stats.into_iter()
        .filter(|(_, (dep, _))| *dep > 0)
        .map(|(principal, (total_deposited_e8s, net_position_e8s))| {
            let current = lifetime_net.get(&principal).copied().unwrap_or(0);
            let current_balance_e8s = if current < 0 { 0 } else { current as u64 };
            types::TopSpDepositorRow {
                principal,
                total_deposited_e8s,
                current_balance_e8s,
                net_position_e8s,
            }
        })
        .collect();
    rows.sort_by(|a, b| b.total_deposited_e8s.cmp(&a.total_deposited_e8s));
    rows.truncate(limit);
    rows
}

// ─── Admin Event Breakdown ───

pub fn get_admin_event_breakdown(query: types::AdminEventBreakdownQuery) -> types::AdminEventBreakdownResponse {
    let window_ns = resolve_window_ns(query.window_ns);
    let now = ic_cdk::api::time();

    if let Some((ts, resp)) = ADMIN_BREAKDOWN_CACHE.with(|c| c.borrow().get(&window_ns).cloned()) {
        if now.saturating_sub(ts) < ADMIN_BREAKDOWN_TTL_NS {
            return resp;
        }
    }

    let from = now.saturating_sub(window_ns);
    let events = storage::events::evt_admin::range(from, now, MAX_EVENT_LOAD);
    let labels = compute_admin_event_breakdown(&events);

    let resp = types::AdminEventBreakdownResponse {
        window_ns,
        generated_at_ns: now,
        labels,
    };
    ADMIN_BREAKDOWN_CACHE.with(|c| {
        c.borrow_mut().insert(window_ns, (now, resp.clone()));
    });
    resp
}

pub fn compute_admin_event_breakdown(
    events: &[storage::events::AnalyticsAdminEvent],
) -> Vec<types::AdminEventLabelCount> {
    let mut agg: HashMap<String, (u64, u64)> = HashMap::new();
    for e in events {
        let entry = agg.entry(e.label.clone()).or_insert((0, 0));
        entry.0 = entry.0.saturating_add(1);
        if e.timestamp_ns > entry.1 {
            entry.1 = e.timestamp_ns;
        }
    }
    let mut rows: Vec<types::AdminEventLabelCount> = agg.into_iter()
        .map(|(label, (count, last))| types::AdminEventLabelCount {
            label,
            count,
            last_at_ns: if last > 0 { Some(last) } else { None },
        })
        .collect();
    rows.sort_by(|a, b| b.count.cmp(&a.count).then(a.label.cmp(&b.label)));
    rows
}

// ─── Protocol Summary ───

pub fn get_protocol_summary() -> types::ProtocolSummary {
    let now = ic_cdk::api::time();
    let day_ns = 86_400u64.saturating_mul(NANOS_PER_SEC);
    let day_ago = now.saturating_sub(day_ns);
    let week_ns = 7u64.saturating_mul(day_ns);
    let week_ago = now.saturating_sub(week_ns);

    // Latest vault snapshot for TVL/CR/vault count.
    let vault_n = storage::daily_vaults::len();
    let (tvl, debt, cr, vaults) = if vault_n > 0 {
        storage::daily_vaults::get(vault_n - 1)
            .map(|v| (v.total_collateral_usd_e8s, v.total_debt_e8s, v.median_cr_bps, v.total_vault_count))
            .unwrap_or((0, 0, 0, 0))
    } else {
        (0, 0, 0, 0)
    };

    // Circulating supply from cache.
    let supply = state::read_state(|s| s.circulating_supply_icusd_e8s);

    // 24h trade activity from EVT_SWAPS.
    let swap_events = storage::events::evt_swaps::range(day_ago, now, MAX_SNAPSHOT_LOAD);
    let activity = compute_trade_activity(&swap_events, 86_400);

    // Peg status from latest 3pool snapshot.
    let peg = get_peg_status();

    // APYs over 7 days.
    let swap_rollups = storage::rollups::daily_swaps::range(week_ago, now, MAX_SNAPSHOT_LOAD);
    let tvl_rows = storage::daily_tvl::range(week_ago, now, MAX_SNAPSHOT_LOAD);
    let stability_rows = storage::daily_stability::range(week_ago, now, MAX_SNAPSHOT_LOAD);
    let lp_apy = compute_lp_apy(&swap_rollups, &tvl_rows, 7);
    let sp_apy = compute_sp_apy(&stability_rows, 7);

    // Price TWAPs over 1 hour.
    let hour_ago = now.saturating_sub(3_600u64.saturating_mul(NANOS_PER_SEC));
    let price_snaps = storage::fast::fast_prices::range(hour_ago, now, MAX_SNAPSHOT_LOAD);
    let prices = compute_twap(&price_snaps);

    types::ProtocolSummary {
        timestamp_ns: now,
        total_collateral_usd_e8s: tvl,
        total_debt_e8s: debt,
        system_cr_bps: cr,
        total_vault_count: vaults,
        circulating_supply_icusd_e8s: supply,
        volume_24h_e8s: activity.total_volume_e8s,
        swap_count_24h: activity.total_swaps,
        peg,
        lp_apy_pct: lp_apy,
        sp_apy_pct: sp_apy,
        prices,
    }
}

// ─── Fee Breakdown Window (Task 3.1) ───

pub fn get_fee_breakdown_window(query: types::FeeBreakdownQuery) -> types::FeeBreakdownResponse {
    let now = ic_cdk::api::time();
    let start = match query.window_ns {
        Some(window) => now.saturating_sub(window),
        None => 0,
    };

    let vault_events = storage::events::evt_vaults::range(start, now, usize::MAX);
    let swap_events = storage::events::evt_swaps::range(start, now, usize::MAX);

    let (borrow_fees, redemption_fees, borrow_count, redemption_count) =
        compute_vault_fee_totals(&vault_events);
    let swap_fees: u64 = swap_events.iter().map(|e| e.fee).sum();
    let swap_count = swap_events.len() as u32;

    types::FeeBreakdownResponse {
        borrow_fees_icusd_e8s: borrow_fees,
        redemption_fees_icusd_e8s: redemption_fees,
        swap_fees_icusd_e8s: swap_fees,
        borrow_count,
        redemption_count,
        swap_count,
        start_ns: start,
        end_ns: now,
    }
}

pub fn compute_vault_fee_totals(
    vault_events: &[storage::events::AnalyticsVaultEvent],
) -> (u64, u64, u32, u32) {
    let mut borrow_fees: u64 = 0;
    let mut redemption_fees: u64 = 0;
    let mut borrow_count: u32 = 0;
    let mut redemption_count: u32 = 0;
    for e in vault_events {
        match e.event_kind {
            storage::events::VaultEventKind::Borrowed => {
                borrow_fees = borrow_fees.saturating_add(e.fee_amount);
                borrow_count += 1;
            }
            storage::events::VaultEventKind::Redeemed => {
                redemption_fees = redemption_fees.saturating_add(e.fee_amount);
                redemption_count += 1;
            }
            _ => {}
        }
    }
    (borrow_fees, redemption_fees, borrow_count, redemption_count)
}

// ─── SP Depositor Principals (Task 3.2) ───

pub fn get_sp_depositor_principals() -> Vec<Principal> {
    let events = storage::events::evt_stability::range(0, u64::MAX, usize::MAX);
    let mut set: HashSet<Principal> = HashSet::new();
    for e in events {
        set.insert(e.caller);
    }
    set.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::fast::FastPriceSnapshot;
    use crate::storage::fast::Fast3PoolSnapshot;
    use crate::storage::rollups::DailySwapRollup;
    use crate::storage::{DailyTvlRow, DailyStabilityRow};

    fn make_price_snap(ts: u64, collateral: Principal, price: f64) -> FastPriceSnapshot {
        FastPriceSnapshot {
            timestamp_ns: ts,
            prices: vec![(collateral, price, "ICP".to_string())],
        }
    }

    #[test]
    fn ohlc_basic_bucketing() {
        let col = Principal::anonymous();
        let hour = 3_600 * NANOS_PER_SEC;
        let snaps = vec![
            make_price_snap(hour, col, 10.0),
            make_price_snap(hour + 300 * NANOS_PER_SEC, col, 12.0),
            make_price_snap(hour + 600 * NANOS_PER_SEC, col, 8.0),
            make_price_snap(hour + 900 * NANOS_PER_SEC, col, 11.0),
            make_price_snap(2 * hour, col, 11.5),
            make_price_snap(2 * hour + 300 * NANOS_PER_SEC, col, 13.0),
        ];
        let (candles, sym) = compute_ohlc(&snaps, col, 3600, 100);
        assert_eq!(sym, "ICP");
        assert_eq!(candles.len(), 2);
        assert_eq!(candles[0].open, 10.0);
        assert_eq!(candles[0].high, 12.0);
        assert_eq!(candles[0].low, 8.0);
        assert_eq!(candles[0].close, 11.0);
        assert_eq!(candles[1].open, 11.5);
        assert_eq!(candles[1].high, 13.0);
        assert_eq!(candles[1].close, 13.0);
    }

    #[test]
    fn ohlc_empty_snapshots() {
        let (candles, _) = compute_ohlc(&[], Principal::anonymous(), 3600, 100);
        assert!(candles.is_empty());
    }

    #[test]
    fn ohlc_limit_respected() {
        let col = Principal::anonymous();
        let hour = 3_600 * NANOS_PER_SEC;
        let snaps: Vec<_> = (0..10)
            .map(|i| make_price_snap(i * hour, col, 10.0 + i as f64))
            .collect();
        let (candles, _) = compute_ohlc(&snaps, col, 3600, 3);
        assert_eq!(candles.len(), 3);
    }

    #[test]
    fn ohlc_unknown_collateral_returns_empty() {
        let col = Principal::anonymous();
        let other = Principal::management_canister();
        let snaps = vec![make_price_snap(1_000_000_000, col, 10.0)];
        let (candles, _) = compute_ohlc(&snaps, other, 3600, 100);
        assert!(candles.is_empty());
    }

    #[test]
    fn twap_basic() {
        let col = Principal::anonymous();
        let snaps = vec![
            make_price_snap(1_000_000_000, col, 10.0),
            make_price_snap(2_000_000_000, col, 12.0),
            make_price_snap(3_000_000_000, col, 14.0),
        ];
        let entries = compute_twap(&snaps);
        assert_eq!(entries.len(), 1);
        assert!((entries[0].twap_price - 12.0).abs() < 0.001);
        assert_eq!(entries[0].latest_price, 14.0);
        assert_eq!(entries[0].sample_count, 3);
    }

    #[test]
    fn twap_multiple_collaterals() {
        let col_a = Principal::anonymous();
        let col_b = Principal::management_canister();
        let snaps = vec![
            FastPriceSnapshot {
                timestamp_ns: 1_000_000_000,
                prices: vec![
                    (col_a, 10.0, "ICP".to_string()),
                    (col_b, 50000.0, "ckBTC".to_string()),
                ],
            },
            FastPriceSnapshot {
                timestamp_ns: 2_000_000_000,
                prices: vec![
                    (col_a, 12.0, "ICP".to_string()),
                    (col_b, 51000.0, "ckBTC".to_string()),
                ],
            },
        ];
        let entries = compute_twap(&snaps);
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn twap_empty() {
        let entries = compute_twap(&[]);
        assert!(entries.is_empty());
    }

    #[test]
    fn volatility_basic() {
        let col = Principal::anonymous();
        let snaps = vec![
            make_price_snap(1_000_000_000, col, 100.0),
            make_price_snap(2_000_000_000, col, 102.0),
            make_price_snap(3_000_000_000, col, 98.0),
            make_price_snap(4_000_000_000, col, 101.0),
            make_price_snap(5_000_000_000, col, 103.0),
        ];
        let (vol, count, sym) = compute_volatility(&snaps, col);
        assert_eq!(count, 5);
        assert_eq!(sym, "ICP");
        assert!(vol > 0.0, "volatility should be positive");
    }

    #[test]
    fn volatility_constant_price() {
        let col = Principal::anonymous();
        let snaps = vec![
            make_price_snap(1_000_000_000, col, 10.0),
            make_price_snap(2_000_000_000, col, 10.0),
            make_price_snap(3_000_000_000, col, 10.0),
        ];
        let (vol, _, _) = compute_volatility(&snaps, col);
        assert_eq!(vol, 0.0, "constant price should have zero vol");
    }

    #[test]
    fn volatility_too_few_samples() {
        let col = Principal::anonymous();
        let snaps = vec![make_price_snap(1_000_000_000, col, 10.0)];
        let (vol, count, _) = compute_volatility(&snaps, col);
        assert_eq!(vol, 0.0);
        assert_eq!(count, 1);
    }

    #[test]
    fn peg_perfectly_balanced() {
        let snap = Fast3PoolSnapshot {
            timestamp_ns: 1_000_000_000,
            balances: vec![1_000_000, 1_000_000, 1_000_000],
            virtual_price: 100_000_000,
            lp_total_supply: 3_000_000,
            decimals: Some(vec![8, 8, 8]),
        };
        let status = compute_peg_status(&snap);
        assert_eq!(status.balance_ratios.len(), 3);
        for r in &status.balance_ratios {
            assert!((r - 1.0).abs() < 0.001);
        }
        assert!(status.max_imbalance_pct < 0.01);
    }

    #[test]
    fn peg_imbalanced() {
        let snap = Fast3PoolSnapshot {
            timestamp_ns: 1_000_000_000,
            balances: vec![1_500_000, 1_000_000, 500_000],
            virtual_price: 100_000_000,
            lp_total_supply: 3_000_000,
            decimals: Some(vec![8, 8, 8]),
        };
        let status = compute_peg_status(&snap);
        assert!((status.max_imbalance_pct - 50.0).abs() < 0.01);
    }

    #[test]
    fn peg_empty_balances() {
        let snap = Fast3PoolSnapshot {
            timestamp_ns: 1_000_000_000,
            balances: vec![],
            virtual_price: 0,
            lp_total_supply: 0,
            decimals: Some(vec![]),
        };
        let status = compute_peg_status(&snap);
        assert!(status.balance_ratios.is_empty());
        assert_eq!(status.max_imbalance_pct, 0.0);
    }

    #[test]
    fn peg_mixed_decimals_balanced() {
        let snap = Fast3PoolSnapshot {
            timestamp_ns: 1_000_000_000,
            balances: vec![10_000_000_000, 100_000_000, 100_000_000],
            virtual_price: 1_000_000_000_000_000_000,
            lp_total_supply: 300_000_000,
            decimals: Some(vec![8, 6, 6]),
        };
        let status = compute_peg_status(&snap);
        assert!(status.max_imbalance_pct < 0.01, "expected near-zero imbalance, got {}", status.max_imbalance_pct);
    }

    #[test]
    fn peg_status_handles_legacy_row_without_decimals() {
        // Legacy rows decoded from pre-decimals bytes have `decimals = None`.
        // compute_peg_status must treat the vec as empty and fall back to the
        // 3pool default of 8 per token without panicking.
        let snap = Fast3PoolSnapshot {
            timestamp_ns: 1_000_000_000,
            balances: vec![1_000_000, 1_000_000, 1_000_000],
            virtual_price: 100_000_000,
            lp_total_supply: 3_000_000,
            decimals: None,
        };
        let status = compute_peg_status(&snap);
        assert_eq!(status.balance_ratios.len(), 3);
        for r in &status.balance_ratios {
            assert!((r - 1.0).abs() < 0.001);
        }
        assert!(status.max_imbalance_pct < 0.01);
    }

    fn make_swap_rollup(fees: u64) -> DailySwapRollup {
        DailySwapRollup {
            timestamp_ns: 1_000_000_000,
            three_pool_swap_count: 10,
            amm_swap_count: 0,
            three_pool_volume_e8s: 1_000_000,
            amm_volume_e8s: 0,
            three_pool_fees_e8s: fees,
            amm_fees_e8s: 0,
            unique_swappers: 5,
        }
    }

    fn make_tvl_row(reserve: u128) -> DailyTvlRow {
        DailyTvlRow {
            timestamp_ns: 1_000_000_000,
            total_icp_collateral_e8s: 0,
            total_icusd_supply_e8s: 0,
            system_collateral_ratio_bps: 0,
            stability_pool_deposits_e8s: None,
            three_pool_reserve_0_e8s: Some(reserve),
            three_pool_reserve_1_e8s: Some(reserve),
            three_pool_reserve_2_e8s: Some(reserve),
            three_pool_virtual_price_e18: None,
            three_pool_lp_supply_e8s: None,
        }
    }

    fn make_stability_row(deposits: u64, interest: u64) -> DailyStabilityRow {
        DailyStabilityRow {
            timestamp_ns: 1_000_000_000,
            total_deposits_e8s: deposits,
            total_depositors: 10,
            total_liquidations_executed: 0,
            total_interest_received_e8s: interest,
            stablecoin_balances: vec![],
            collateral_gains: vec![],
        }
    }

    #[test]
    fn lp_apy_basic() {
        let swaps = vec![make_swap_rollup(100)];
        let tvls = vec![make_tvl_row(10_000)];
        let apy = compute_lp_apy(&swaps, &tvls, 1);
        assert!(apy.is_some());
        // daily_fees = 100, avg_tvl = 30_000, APY = (100/30000) * 365 * 100 = 121.67%
        let v = apy.unwrap();
        assert!((v - 121.67).abs() < 1.0, "expected ~121.67%, got {}", v);
    }

    #[test]
    fn lp_apy_empty() {
        assert!(compute_lp_apy(&[], &[], 7).is_none());
    }

    #[test]
    fn sp_apy_basic() {
        // total_interest_received_e8s is cumulative: 100 on day 1, 150 on day 2 = 50 delta
        let rows = vec![make_stability_row(100_000, 100), make_stability_row(100_000, 150)];
        let apy = compute_sp_apy(&rows, 1);
        assert!(apy.is_some());
        // delta = 150 - 100 = 50, daily_interest = 50, avg_deposits = 100_000
        // APY = (50/100000) * 365 * 100 = 18.25%
        let v = apy.unwrap();
        assert!((v - 18.25).abs() < 0.1, "expected ~18.25%, got {}", v);
    }

    #[test]
    fn sp_apy_single_row() {
        // Need at least 2 rows to compute a delta
        let rows = vec![make_stability_row(100_000, 50)];
        assert!(compute_sp_apy(&rows, 1).is_none());
    }

    #[test]
    fn sp_apy_zero_deposits() {
        let rows = vec![make_stability_row(0, 50), make_stability_row(0, 100)];
        assert!(compute_sp_apy(&rows, 1).is_none());
    }

    use crate::storage::events::{AnalyticsSwapEvent, SwapSource};

    fn make_swap_event(caller: Principal, source: SwapSource, amount: u64, fee: u64) -> AnalyticsSwapEvent {
        AnalyticsSwapEvent {
            timestamp_ns: 1_000_000_000,
            source,
            source_event_id: 0,
            caller,
            token_in: Principal::anonymous(),
            token_out: Principal::anonymous(),
            amount_in: amount,
            amount_out: amount.saturating_sub(fee),
            fee,
        }
    }

    #[test]
    fn trade_activity_basic() {
        let user_a = Principal::anonymous();
        let user_b = Principal::management_canister();
        let events = vec![
            make_swap_event(user_a, SwapSource::ThreePool, 1_000, 10),
            make_swap_event(user_a, SwapSource::ThreePool, 2_000, 20),
            make_swap_event(user_b, SwapSource::Amm, 3_000, 30),
        ];
        let res = compute_trade_activity(&events, 86_400);
        assert_eq!(res.total_swaps, 3);
        assert_eq!(res.three_pool_swaps, 2);
        assert_eq!(res.amm_swaps, 1);
        assert_eq!(res.total_volume_e8s, 6_000);
        assert_eq!(res.total_fees_e8s, 60);
        assert_eq!(res.unique_traders, 2);
        assert_eq!(res.avg_trade_size_e8s, 2_000);
    }

    #[test]
    fn trade_activity_empty() {
        let res = compute_trade_activity(&[], 86_400);
        assert_eq!(res.total_swaps, 0);
        assert_eq!(res.avg_trade_size_e8s, 0);
    }

    fn p(byte: u8) -> Principal {
        Principal::from_slice(&[byte; 29])
    }

    #[test]
    fn top_holders_ranks_descending_and_computes_share() {
        let token = p(99);
        let holders = vec![
            (p(1), 100),
            (p(2), 400),
            (p(3), 250),
            (p(4), 250),
        ];
        let total_supply = 1_000;
        let res = compute_top_holders(&holders, total_supply, 10, token, 12_345, "balance_tracker");

        assert_eq!(res.token, token);
        assert_eq!(res.total_holders, 4);
        assert_eq!(res.total_supply_e8s, 1_000);
        assert_eq!(res.generated_at_ns, 12_345);
        assert_eq!(res.source, "balance_tracker");
        assert_eq!(res.rows.len(), 4);
        assert_eq!(res.rows[0].principal, p(2));
        assert_eq!(res.rows[0].balance_e8s, 400);
        assert_eq!(res.rows[0].share_bps, 4_000);
        let lower_bps_sum: u32 = res.rows[1..].iter().map(|r| r.share_bps).sum();
        assert_eq!(lower_bps_sum, 6_000);
    }

    #[test]
    fn top_holders_truncates_to_limit_and_keeps_total_count() {
        let token = p(99);
        let holders: Vec<(Principal, u64)> = (0..30u8).map(|i| (p(i), 100 - i as u64)).collect();
        let res = compute_top_holders(&holders, 0, 5, token, 0, "balance_tracker");
        assert_eq!(res.total_holders, 30);
        assert_eq!(res.rows.len(), 5);
        assert_eq!(res.rows[0].balance_e8s, 100);
        assert_eq!(res.rows[4].balance_e8s, 96);
    }

    #[test]
    fn top_holders_falls_back_to_ranked_sum_when_supply_is_zero() {
        let token = p(99);
        let holders = vec![(p(1), 25), (p(2), 75)];
        let res = compute_top_holders(&holders, 0, 10, token, 0, "balance_tracker");
        assert_eq!(res.rows[0].principal, p(2));
        assert_eq!(res.rows[0].share_bps, 7_500);
        assert_eq!(res.rows[1].share_bps, 2_500);
    }

    #[test]
    fn top_holders_empty_input_returns_empty_rows() {
        let token = p(99);
        let res = compute_top_holders(&[], 0, 10, token, 7, "balance_tracker");
        assert_eq!(res.total_holders, 0);
        assert!(res.rows.is_empty());
        assert_eq!(res.generated_at_ns, 7);
    }

    // ── Top counterparties ─────────────────────────────────────────────────

    fn make_vault_event(vault_id: u64, owner: Principal, kind: crate::storage::events::VaultEventKind, amount: u64) -> crate::storage::events::AnalyticsVaultEvent {
        crate::storage::events::AnalyticsVaultEvent {
            timestamp_ns: 1_000,
            source_event_id: 0,
            vault_id,
            owner,
            event_kind: kind,
            collateral_type: Principal::anonymous(),
            amount,
            fee_amount: 0,
        }
    }

    fn make_liquidity_event(caller: Principal, amounts: Vec<u64>) -> crate::storage::events::AnalyticsLiquidityEvent {
        crate::storage::events::AnalyticsLiquidityEvent {
            timestamp_ns: 1_000,
            source_event_id: 0,
            caller,
            action: crate::storage::events::LiquidityAction::Add,
            amounts,
            lp_amount: 0,
            coin_index: None,
            fee: None,
        }
    }

    fn make_stability_event(caller: Principal, action: crate::storage::events::StabilityAction, amount: u64) -> crate::storage::events::AnalyticsStabilityEvent {
        crate::storage::events::AnalyticsStabilityEvent {
            timestamp_ns: 1_000,
            source_event_id: 0,
            caller,
            action,
            amount,
        }
    }

    fn make_admin_event(label: &str, ts: u64) -> crate::storage::events::AnalyticsAdminEvent {
        crate::storage::events::AnalyticsAdminEvent {
            timestamp_ns: ts,
            source_event_id: 0,
            label: label.to_string(),
        }
    }

    #[test]
    fn top_counterparties_ranks_by_interactions_and_volume() {
        use crate::storage::events::SwapSource;

        let target = p(1);
        let three_pool = p(100);
        let amm = p(101);
        let sp = p(102);

        let swaps = vec![
            AnalyticsSwapEvent {
                timestamp_ns: 1_000, source: SwapSource::ThreePool, source_event_id: 0,
                caller: target, token_in: p(50), token_out: p(51),
                amount_in: 1_000, amount_out: 995, fee: 5,
            },
            AnalyticsSwapEvent {
                timestamp_ns: 2_000, source: SwapSource::Amm, source_event_id: 1,
                caller: target, token_in: p(60), token_out: p(61),
                amount_in: 500, amount_out: 495, fee: 5,
            },
            AnalyticsSwapEvent {
                timestamp_ns: 3_000, source: SwapSource::ThreePool, source_event_id: 2,
                caller: target, token_in: p(50), token_out: p(51),
                amount_in: 2_000, amount_out: 1_990, fee: 10,
            },
        ];

        let rows = compute_top_counterparties(
            target,
            &[],
            &swaps,
            &[],
            &[],
            &HashMap::new(),
            three_pool, amm, sp,
            10,
        );

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].counterparty, three_pool);
        assert_eq!(rows[0].interaction_count, 2);
        assert_eq!(rows[0].volume_e8s, 3_000);
        assert_eq!(rows[1].counterparty, amm);
        assert_eq!(rows[1].interaction_count, 1);
    }

    #[test]
    fn top_counterparties_surfaces_other_principals_via_vault_ownership() {
        use crate::storage::events::VaultEventKind;

        let owner = p(1);
        let redeemer = p(2);
        let mut vault_owners = HashMap::new();
        vault_owners.insert(42u64, owner);

        let vault_events = vec![
            make_vault_event(42, owner, VaultEventKind::Opened, 0),
            // Redeemer acted on owner's vault.
            make_vault_event(42, redeemer, VaultEventKind::Redeemed, 500),
            // Owner acts on own vault — not a counterparty.
            make_vault_event(42, owner, VaultEventKind::Borrowed, 100),
        ];

        let rows = compute_top_counterparties(
            owner,
            &vault_events,
            &[],
            &[],
            &[],
            &vault_owners,
            p(100), p(101), p(102),
            10,
        );

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].counterparty, redeemer);
        assert_eq!(rows[0].interaction_count, 1);
        assert_eq!(rows[0].volume_e8s, 500);
    }

    #[test]
    fn top_counterparties_respects_limit() {
        let target = p(1);
        let three_pool = p(100);
        // Three separate liquidity events (all counted against the pool).
        let events = vec![
            make_liquidity_event(target, vec![100, 200]),
            make_liquidity_event(target, vec![50]),
            make_liquidity_event(target, vec![10]),
        ];
        // Pool is the only counterparty, so limit=1 is already the full result.
        let rows = compute_top_counterparties(
            target,
            &[],
            &[],
            &events,
            &[],
            &HashMap::new(),
            three_pool, p(101), p(102),
            1,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].interaction_count, 3);
        assert_eq!(rows[0].volume_e8s, 360);
    }

    #[test]
    fn top_counterparties_skips_events_unrelated_to_target() {
        use crate::storage::events::SwapSource;
        let target = p(1);
        let other = p(2);
        let swaps = vec![
            AnalyticsSwapEvent {
                timestamp_ns: 1_000, source: SwapSource::ThreePool, source_event_id: 0,
                caller: other, token_in: p(50), token_out: p(51),
                amount_in: 1_000, amount_out: 995, fee: 5,
            },
        ];
        let rows = compute_top_counterparties(
            target,
            &[],
            &swaps,
            &[],
            &[],
            &HashMap::new(),
            p(100), p(101), p(102),
            10,
        );
        assert!(rows.is_empty());
    }

    // ── Top SP depositors ─────────────────────────────────────────────────

    #[test]
    fn top_sp_depositors_ranks_by_total_deposited_in_window() {
        use crate::storage::events::StabilityAction;
        let whale = p(1);
        let shrimp = p(2);
        let window_events = vec![
            make_stability_event(whale, StabilityAction::Deposit, 1_000),
            make_stability_event(whale, StabilityAction::Deposit, 500),
            make_stability_event(whale, StabilityAction::Withdraw, 200),
            make_stability_event(shrimp, StabilityAction::Deposit, 100),
        ];
        let rows = compute_top_sp_depositors(&window_events, &window_events, 10);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].principal, whale);
        assert_eq!(rows[0].total_deposited_e8s, 1_500);
        assert_eq!(rows[0].net_position_e8s, 1_300);
        assert_eq!(rows[0].current_balance_e8s, 1_300);
        assert_eq!(rows[1].principal, shrimp);
        assert_eq!(rows[1].total_deposited_e8s, 100);
    }

    #[test]
    fn top_sp_depositors_uses_all_events_for_current_balance() {
        use crate::storage::events::StabilityAction;
        let user = p(1);
        let window = vec![
            make_stability_event(user, StabilityAction::Deposit, 500),
        ];
        let all_time = vec![
            make_stability_event(user, StabilityAction::Deposit, 1_000),
            make_stability_event(user, StabilityAction::Withdraw, 400),
            make_stability_event(user, StabilityAction::Deposit, 500),
        ];
        let rows = compute_top_sp_depositors(&window, &all_time, 10);
        assert_eq!(rows[0].total_deposited_e8s, 500);
        assert_eq!(rows[0].current_balance_e8s, 1_100);
    }

    #[test]
    fn top_sp_depositors_respects_limit_and_drops_claim_only_principals() {
        use crate::storage::events::StabilityAction;
        let events: Vec<_> = (1u8..=10)
            .map(|i| make_stability_event(p(i), StabilityAction::Deposit, (i as u64) * 100))
            .chain(std::iter::once(make_stability_event(p(99), StabilityAction::ClaimReturns, 50)))
            .collect();
        let rows = compute_top_sp_depositors(&events, &events, 3);
        assert_eq!(rows.len(), 3);
        // Top three by amount: p(10)=1000, p(9)=900, p(8)=800
        assert_eq!(rows[0].principal, p(10));
        assert_eq!(rows[2].total_deposited_e8s, 800);
        // The claim-only principal never appears in the deposits ranking.
        assert!(rows.iter().all(|r| r.principal != p(99)));
    }

    #[test]
    fn top_sp_depositors_empty_events_returns_empty() {
        let rows = compute_top_sp_depositors(&[], &[], 10);
        assert!(rows.is_empty());
    }

    // ── Admin event breakdown ─────────────────────────────────────────────

    #[test]
    fn admin_breakdown_counts_by_label_and_tracks_last_at() {
        let events = vec![
            make_admin_event("SetBorrowingFee", 1_000),
            make_admin_event("SetBorrowingFee", 3_000),
            make_admin_event("SetHealthyCr", 2_000),
        ];
        let rows = compute_admin_event_breakdown(&events);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label, "SetBorrowingFee");
        assert_eq!(rows[0].count, 2);
        assert_eq!(rows[0].last_at_ns, Some(3_000));
        assert_eq!(rows[1].label, "SetHealthyCr");
        assert_eq!(rows[1].count, 1);
    }

    #[test]
    fn admin_breakdown_sorts_count_desc_then_label() {
        let events = vec![
            make_admin_event("SetBorrowingFee", 1_000),
            make_admin_event("SetHealthyCr", 2_000),
            make_admin_event("AddCollateralType", 3_000),
        ];
        let rows = compute_admin_event_breakdown(&events);
        // All count==1, sort by label asc as tiebreaker.
        assert_eq!(rows[0].label, "AddCollateralType");
        assert_eq!(rows[1].label, "SetBorrowingFee");
        assert_eq!(rows[2].label, "SetHealthyCr");
    }

    #[test]
    fn admin_breakdown_empty_returns_empty() {
        assert!(compute_admin_event_breakdown(&[]).is_empty());
    }

    // ── Cache freshness ──────────────────────────────────────────────────

    #[test]
    fn cache_is_fresh_within_ttl_and_stale_after() {
        // 30s TTL, stamp at t=1_000 (in the same ns units).
        let ttl = 30 * NANOS_PER_SEC;
        let cached_at = 1_000_000_000u64;
        // Same instant → fresh.
        assert!(cache_is_fresh(cached_at, cached_at, ttl));
        // 29s later → still fresh.
        assert!(cache_is_fresh(cached_at, cached_at + 29 * NANOS_PER_SEC, ttl));
        // Exactly TTL later → stale (strict <).
        assert!(!cache_is_fresh(cached_at, cached_at + ttl, ttl));
        // 1 minute later → stale.
        assert!(!cache_is_fresh(cached_at, cached_at + 60 * NANOS_PER_SEC, ttl));
        // Now earlier than cached_at (clock skew/defensive) → saturating
        // subtraction yields 0 which is < ttl → still fresh.
        assert!(cache_is_fresh(cached_at + 5, cached_at, ttl));
    }

    #[test]
    fn aggregate_by_principal_sums_subaccounts() {
        use crate::storage::balance_tracker::Account;
        let owner = p(7);
        let other = p(8);
        let mut sub_a = [0u8; 32];
        sub_a[0] = 1;
        let mut sub_b = [0u8; 32];
        sub_b[0] = 2;
        let accounts = vec![
            (Account { owner, subaccount: None }, 100u64),
            (Account { owner, subaccount: Some(sub_a) }, 50u64),
            (Account { owner, subaccount: Some(sub_b) }, 25u64),
            (Account { owner: other, subaccount: None }, 10u64),
        ];
        let mut by_p = aggregate_by_principal(&accounts);
        by_p.sort_by_key(|(p, _)| *p);
        let owner_total = by_p.iter().find(|(p, _)| *p == owner).map(|(_, b)| *b).unwrap();
        let other_total = by_p.iter().find(|(p, _)| *p == other).map(|(_, b)| *b).unwrap();
        assert_eq!(owner_total, 175);
        assert_eq!(other_total, 10);
    }

    // ─── Task 3.1: fee breakdown compute helper ───

    fn make_fee_vault_event(
        kind: storage::events::VaultEventKind,
        fee_amount: u64,
    ) -> storage::events::AnalyticsVaultEvent {
        storage::events::AnalyticsVaultEvent {
            timestamp_ns: 1_000_000,
            source_event_id: 0,
            vault_id: 1,
            owner: Principal::anonymous(),
            event_kind: kind,
            collateral_type: Principal::anonymous(),
            amount: 1_000_000,
            fee_amount,
        }
    }

    #[test]
    fn fee_breakdown_empty_inputs_all_zeros() {
        let (bf, rf, bc, rc) = compute_vault_fee_totals(&[]);
        assert_eq!(bf, 0);
        assert_eq!(rf, 0);
        assert_eq!(bc, 0);
        assert_eq!(rc, 0);
    }

    #[test]
    fn fee_breakdown_borrow_and_redeem_counts_and_sums() {
        use storage::events::VaultEventKind;
        let events = vec![
            make_fee_vault_event(VaultEventKind::Borrowed, 500_000),
            make_fee_vault_event(VaultEventKind::Borrowed, 300_000),
            make_fee_vault_event(VaultEventKind::Redeemed, 100_000),
            make_fee_vault_event(VaultEventKind::Repaid, 0),  // should be ignored
        ];
        let (bf, rf, bc, rc) = compute_vault_fee_totals(&events);
        assert_eq!(bf, 800_000);
        assert_eq!(rf, 100_000);
        assert_eq!(bc, 2);
        assert_eq!(rc, 1);
    }

    #[test]
    fn fee_breakdown_saturating_add_on_large_fees() {
        use storage::events::VaultEventKind;
        let events = vec![
            make_fee_vault_event(VaultEventKind::Borrowed, u64::MAX),
            make_fee_vault_event(VaultEventKind::Borrowed, 1),
        ];
        let (bf, _, bc, _) = compute_vault_fee_totals(&events);
        assert_eq!(bf, u64::MAX); // saturating, not overflow
        assert_eq!(bc, 2);
    }
}
