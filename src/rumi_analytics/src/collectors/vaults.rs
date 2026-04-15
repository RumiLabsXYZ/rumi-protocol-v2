//! Daily vault snapshot collector.
//!
//! Queries get_all_vaults and get_collateral_totals concurrently, then builds
//! per-collateral CR stats and a protocol-wide snapshot row. Both calls are
//! required; failure of either aborts the snapshot and increments the backend
//! error counter.

use crate::{sources, state, storage};
use std::collections::HashMap;

/// Compute the median of an already-sorted slice of u32 values.
/// For even-length slices, returns the average of the two middle values
/// rounded down (integer division). Returns 0 for empty slices.
pub fn median_of_sorted(sorted: &[u32]) -> u32 {
    let n = sorted.len();
    if n == 0 {
        return 0;
    }
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        let lo = sorted[n / 2 - 1] as u64;
        let hi = sorted[n / 2] as u64;
        ((lo + hi) / 2) as u32
    }
}

pub async fn run() -> Result<(), String> {
    let backend_id = state::read_state(|s| s.sources.backend);

    let (vaults_res, totals_res) = futures::join!(
        sources::backend::get_all_vaults(backend_id),
        sources::backend::get_collateral_totals(backend_id),
    );

    let vaults = match vaults_res {
        Ok(v) => v,
        Err(e) => {
            state::mutate_state(|s| s.error_counters.backend += 1);
            return Err(format!("[vaults] get_all_vaults failed: {}", e));
        }
    };

    let collateral_totals = match totals_res {
        Ok(t) => t,
        Err(e) => {
            state::mutate_state(|s| s.error_counters.backend += 1);
            return Err(format!("[vaults] get_collateral_totals failed: {}", e));
        }
    };

    // Build price and decimals lookups from collateral_type.
    let price_map: HashMap<candid::Principal, f64> = collateral_totals
        .iter()
        .map(|ct| (ct.collateral_type, ct.price))
        .collect();
    let decimals_map: HashMap<candid::Principal, u8> = collateral_totals
        .iter()
        .map(|ct| (ct.collateral_type, ct.decimals))
        .collect();

    // For each vault with debt > 0, compute its CR in bps and group by
    // collateral_type.
    let mut crs_by_collateral: HashMap<candid::Principal, Vec<u32>> = HashMap::new();
    let mut all_crs: Vec<u32> = Vec::new();

    for vault in &vaults {
        let debt = vault.borrowed_icusd_amount.saturating_add(vault.accrued_interest);
        if debt == 0 {
            continue;
        }
        let price = match price_map.get(&vault.collateral_type) {
            Some(&p) => p,
            None => continue,
        };
        // Normalize collateral to whole units using actual token decimals
        // (ckETH=18, ckXAUT=6, ICP/ckBTC/others=8)
        let decimals = decimals_map.get(&vault.collateral_type).copied().unwrap_or(8);
        let collateral_usd = vault.collateral_amount as f64 * price
            / 10f64.powi(decimals as i32);
        let debt_usd = debt as f64 / 1e8;
        let cr_bps = (collateral_usd / debt_usd * 10_000.0)
            .clamp(0.0, u32::MAX as f64) as u32;

        crs_by_collateral
            .entry(vault.collateral_type)
            .or_default()
            .push(cr_bps);
        all_crs.push(cr_bps);
    }

    // Build per-collateral stats.
    let mut collaterals: Vec<storage::CollateralStats> = Vec::new();
    for ct in &collateral_totals {
        let mut crs = crs_by_collateral
            .remove(&ct.collateral_type)
            .unwrap_or_default();
        crs.sort_unstable();

        let (min_cr_bps, max_cr_bps, median_cr_bps) = if crs.is_empty() {
            (0u32, 0u32, 0u32)
        } else {
            (
                *crs.first().unwrap(),
                *crs.last().unwrap(),
                median_of_sorted(&crs),
            )
        };

        let price_usd_e8s = (ct.price * 1e8) as u64;

        collaterals.push(storage::CollateralStats {
            collateral_type: ct.collateral_type,
            vault_count: crs.len() as u32,
            total_collateral_e8s: ct.total_collateral,
            total_debt_e8s: ct.total_debt,
            min_cr_bps,
            max_cr_bps,
            median_cr_bps,
            price_usd_e8s,
        });
    }

    // Protocol-wide stats.
    let total_vault_count = all_crs.len() as u32;

    all_crs.sort_unstable();
    let protocol_median_cr_bps = median_of_sorted(&all_crs);

    // Total collateral USD value: sum of (total_collateral / 10^decimals * price)
    // for each collateral type, converted to e8s.
    let total_collateral_usd_e8s: u64 = collateral_totals
        .iter()
        .map(|ct| {
            let units = ct.total_collateral as f64 / 10f64.powi(ct.decimals as i32);
            (units * ct.price * 1e8) as u64
        })
        .fold(0u64, |acc, x| acc.saturating_add(x));

    let total_debt_e8s: u64 = collateral_totals
        .iter()
        .map(|ct| ct.total_debt)
        .fold(0u64, |acc, x| acc.saturating_add(x));

    let row = storage::DailyVaultSnapshotRow {
        timestamp_ns: ic_cdk::api::time(),
        total_vault_count,
        total_collateral_usd_e8s,
        total_debt_e8s,
        median_cr_bps: protocol_median_cr_bps,
        collaterals,
    };
    storage::daily_vaults::push(row);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::median_of_sorted;

    #[test]
    fn empty_slice_returns_zero() {
        assert_eq!(median_of_sorted(&[]), 0);
    }

    #[test]
    fn single_element() {
        assert_eq!(median_of_sorted(&[42]), 42);
    }

    #[test]
    fn odd_length_picks_middle() {
        assert_eq!(median_of_sorted(&[10, 20, 30]), 20);
        assert_eq!(median_of_sorted(&[5, 15, 25, 35, 45]), 25);
    }

    #[test]
    fn even_length_averages_two_middle_rounded_down() {
        // [10, 20]: (10+20)/2 = 15
        assert_eq!(median_of_sorted(&[10, 20]), 15);
        // [10, 11]: (10+11)/2 = 10 (rounds down)
        assert_eq!(median_of_sorted(&[10, 11]), 10);
        // [1, 2, 3, 4]: middle pair is (2, 3), (2+3)/2 = 2
        assert_eq!(median_of_sorted(&[1, 2, 3, 4]), 2);
        // [100, 200, 300, 400]: (200+300)/2 = 250
        assert_eq!(median_of_sorted(&[100, 200, 300, 400]), 250);
    }

    #[test]
    fn large_values_do_not_overflow() {
        // Two values near u32::MAX
        let a = u32::MAX - 1;
        let b = u32::MAX;
        // (a + b) would overflow u32, but we cast to u64 first.
        let expected = ((a as u64 + b as u64) / 2) as u32;
        assert_eq!(median_of_sorted(&[a, b]), expected);
    }
}
