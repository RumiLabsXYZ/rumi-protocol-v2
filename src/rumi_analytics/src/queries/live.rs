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
    let total: u128 = snap.balances.iter().sum();
    let count = snap.balances.len();

    let (balance_ratios, max_imbalance_pct) = if count > 0 && total > 0 {
        let target = total as f64 / count as f64;
        let ratios: Vec<f64> = snap.balances.iter()
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
        };
        let status = compute_peg_status(&snap);
        assert!(status.balance_ratios.is_empty());
        assert_eq!(status.max_imbalance_pct, 0.0);
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
}
