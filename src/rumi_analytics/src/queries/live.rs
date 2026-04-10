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

// ─── OHLC ───

pub fn get_ohlc(query: types::OhlcQuery) -> types::OhlcResponse {
    let bucket_secs = query.bucket_secs.unwrap_or(DEFAULT_BUCKET_SECS);
    let from = query.from_ts.unwrap_or(0);
    let to = query.to_ts.unwrap_or(u64::MAX);
    let limit = query.limit.unwrap_or(500).min(2000) as usize;

    let snapshots = storage::fast::fast_prices::range(from, to, usize::MAX);

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

    let snapshots = storage::fast::fast_prices::range(from, now, usize::MAX);
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

    let snapshots = storage::fast::fast_prices::range(from, now, usize::MAX);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::fast::FastPriceSnapshot;
    use crate::storage::fast::Fast3PoolSnapshot;

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
}
