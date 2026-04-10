//! Daily holder snapshot collector. Reads BalanceTracker state and computes
//! Gini coefficient, top-50, distribution buckets, median balance.

use candid::Principal;
use crate::{state, storage};
use storage::balance_tracker::{self, Token};
use storage::holders::DailyHolderRow;

const NANOS_PER_DAY: u64 = 86_400_000_000_000;

const BUCKET_THRESHOLDS: [u64; 4] = [
    100_0000_0000,       // 100 tokens
    1_000_0000_0000,     // 1,000 tokens
    10_000_0000_0000,    // 10,000 tokens
    100_000_0000_0000,   // 100,000 tokens
];

pub fn compute_gini_bps(sorted_balances: &[u64]) -> u32 {
    let n = sorted_balances.len();
    if n <= 1 { return 0; }
    let total: u128 = sorted_balances.iter().map(|&b| b as u128).sum();
    if total == 0 { return 0; }
    let n128 = n as u128;
    let mut numerator: i128 = 0;
    for (i, &bal) in sorted_balances.iter().enumerate() {
        let rank = (i as i128 + 1) * 2 - n128 as i128 - 1;
        numerator += rank * bal as i128;
    }
    let gini = numerator as f64 / (n128 as f64 * total as f64);
    let gini_clamped = gini.clamp(0.0, 1.0);
    (gini_clamped * 10_000.0) as u32
}

pub fn compute_distribution_buckets(balances_e8s: &[u64]) -> Vec<u32> {
    let mut buckets = vec![0u32; BUCKET_THRESHOLDS.len() + 1];
    for &bal in balances_e8s {
        let idx = BUCKET_THRESHOLDS.iter().position(|&t| bal <= t).unwrap_or(BUCKET_THRESHOLDS.len());
        buckets[idx] += 1;
    }
    buckets
}

pub fn top_n_holders(holders: &[(Principal, u64)], n: usize) -> Vec<(Principal, u64)> {
    let mut sorted: Vec<_> = holders.to_vec();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    sorted.truncate(n);
    sorted
}

fn compute_median(sorted: &[u64]) -> u64 {
    let n = sorted.len();
    if n == 0 { return 0; }
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        let lo = sorted[n / 2 - 1] as u128;
        let hi = sorted[n / 2] as u128;
        ((lo + hi) / 2) as u64
    }
}

fn snapshot_token(token: Token, token_principal: Principal, now_ns: u64) -> DailyHolderRow {
    let all = balance_tracker::all_balances(token);
    let total_holders = all.len() as u32;
    let total_supply_tracked = balance_tracker::total_supply_tracked(token);

    let mut balances: Vec<u64> = all.iter().map(|(_, b)| *b).collect();
    let principal_balances: Vec<(Principal, u64)> = all.iter().map(|(a, b)| (a.owner.clone(), *b)).collect();

    balances.sort_unstable();
    let median = compute_median(&balances);
    let gini = compute_gini_bps(&balances);
    let buckets = compute_distribution_buckets(&balances);
    let top_50 = top_n_holders(&principal_balances, 50);

    let top_10_sum: u64 = {
        let mut sorted_desc = balances.clone();
        sorted_desc.sort_unstable_by(|a, b| b.cmp(a));
        sorted_desc.iter().take(10).sum()
    };
    let top_10_pct = if total_supply_tracked > 0 {
        ((top_10_sum as f64 / total_supply_tracked as f64) * 10_000.0) as u32
    } else { 0 };

    let day_start = now_ns.saturating_sub(NANOS_PER_DAY);
    let new_holders = balance_tracker::count_new_holders(token, day_start, now_ns);

    DailyHolderRow {
        timestamp_ns: now_ns,
        token: token_principal,
        total_holders,
        total_supply_tracked_e8s: total_supply_tracked,
        median_balance_e8s: median,
        top_50,
        top_10_pct_bps: top_10_pct,
        gini_bps: gini,
        new_holders_today: new_holders,
        distribution_buckets: buckets,
    }
}

pub async fn run() -> Result<(), String> {
    let (icusd_ledger, three_pool) = state::read_state(|s| {
        (s.sources.icusd_ledger, s.sources.three_pool)
    });
    let now = ic_cdk::api::time();

    if balance_tracker::holder_count(Token::IcUsd) > 0 {
        let row = snapshot_token(Token::IcUsd, icusd_ledger, now);
        storage::holders::daily_holders_icusd::push(row);
    }
    if balance_tracker::holder_count(Token::ThreeUsd) > 0 {
        let row = snapshot_token(Token::ThreeUsd, three_pool, now);
        storage::holders::daily_holders_3usd::push(row);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gini_perfect_equality() {
        let balances = vec![100, 100, 100, 100];
        assert_eq!(compute_gini_bps(&balances), 0);
    }

    #[test]
    fn gini_perfect_inequality() {
        let balances = vec![0, 0, 0, 1000];
        let g = compute_gini_bps(&balances);
        assert!(g > 7000, "expected high Gini, got {}", g);
    }

    #[test]
    fn gini_moderate_inequality() {
        let balances = vec![10, 20, 30, 40, 50];
        let g = compute_gini_bps(&balances);
        assert!(g > 0 && g < 5000, "expected moderate Gini, got {}", g);
    }

    #[test]
    fn gini_empty() {
        assert_eq!(compute_gini_bps(&[]), 0);
    }

    #[test]
    fn gini_single() {
        assert_eq!(compute_gini_bps(&[100]), 0);
    }

    #[test]
    fn distribution_buckets_correct() {
        let balances_e8s: Vec<u64> = vec![
            50_0000_0000,       // 50 tokens -> bucket 0
            500_0000_0000,      // 500 tokens -> bucket 1
            5_000_0000_0000,    // 5,000 tokens -> bucket 2
            50_000_0000_0000,   // 50,000 tokens -> bucket 3
            500_000_0000_0000,  // 500,000 tokens -> bucket 4
            10_0000_0000,       // 10 tokens -> bucket 0
        ];
        let buckets = compute_distribution_buckets(&balances_e8s);
        assert_eq!(buckets, vec![2, 1, 1, 1, 1]);
    }

    #[test]
    fn top_n_extracts_correct_top() {
        let holders = vec![
            (candid::Principal::anonymous(), 100u64),
            (candid::Principal::anonymous(), 500),
            (candid::Principal::anonymous(), 200),
        ];
        let top = top_n_holders(&holders, 2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].1, 500);
        assert_eq!(top[1].1, 200);
    }
}
