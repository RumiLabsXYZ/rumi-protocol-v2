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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::fast::FastPriceSnapshot;

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
}
