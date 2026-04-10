//! CSV serialization helpers for analytics row types.
//!
//! Each function takes a slice of rows and returns a complete CSV string
//! including a header line. Option fields emit an empty column when None.
//! Vec fields (per-collateral breakdowns, prices, balances) are serialized as
//! a single JSON-like column so the CSV remains rectangular.

use crate::storage::{
    DailyTvlRow, DailyVaultSnapshotRow, DailyStabilityRow,
};
use crate::storage::rollups::{DailySwapRollup, DailyLiquidationRollup, DailyFeeRollup};
use crate::storage::fast::FastPriceSnapshot;

// ── DailyTvlRow ─────────────────────────────────────────────────────────────

pub fn tvl_to_csv(rows: &[DailyTvlRow]) -> String {
    let mut out = String::from(
        "timestamp_ns,total_icp_collateral_e8s,total_icusd_supply_e8s,\
system_collateral_ratio_bps,stability_pool_deposits_e8s,\
three_pool_reserve_0_e8s,three_pool_reserve_1_e8s,three_pool_reserve_2_e8s,\
three_pool_virtual_price_e18,three_pool_lp_supply_e8s\n",
    );
    for r in rows {
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{}\n",
            r.timestamp_ns,
            r.total_icp_collateral_e8s,
            r.total_icusd_supply_e8s,
            r.system_collateral_ratio_bps,
            r.stability_pool_deposits_e8s.map_or(String::new(), |v| v.to_string()),
            r.three_pool_reserve_0_e8s.map_or(String::new(), |v| v.to_string()),
            r.three_pool_reserve_1_e8s.map_or(String::new(), |v| v.to_string()),
            r.three_pool_reserve_2_e8s.map_or(String::new(), |v| v.to_string()),
            r.three_pool_virtual_price_e18.map_or(String::new(), |v| v.to_string()),
            r.three_pool_lp_supply_e8s.map_or(String::new(), |v| v.to_string()),
        ));
    }
    out
}

// ── DailyVaultSnapshotRow ────────────────────────────────────────────────────

pub fn vaults_to_csv(rows: &[DailyVaultSnapshotRow]) -> String {
    let mut out = String::from(
        "timestamp_ns,total_vault_count,total_collateral_usd_e8s,total_debt_e8s,\
median_cr_bps,collaterals\n",
    );
    for r in rows {
        // Serialize collaterals as a semicolon-separated list of
        // principal:vault_count:collateral_e8s:debt_e8s:min_cr:max_cr:median_cr:price_e8s
        // tuples so a single CSV column holds the full breakdown.
        let collaterals: String = r
            .collaterals
            .iter()
            .map(|c| {
                format!(
                    "{}:{}:{}:{}:{}:{}:{}:{}",
                    c.collateral_type,
                    c.vault_count,
                    c.total_collateral_e8s,
                    c.total_debt_e8s,
                    c.min_cr_bps,
                    c.max_cr_bps,
                    c.median_cr_bps,
                    c.price_usd_e8s,
                )
            })
            .collect::<Vec<_>>()
            .join(";");
        out.push_str(&format!(
            "{},{},{},{},{},\"{}\"\n",
            r.timestamp_ns,
            r.total_vault_count,
            r.total_collateral_usd_e8s,
            r.total_debt_e8s,
            r.median_cr_bps,
            collaterals,
        ));
    }
    out
}

// ── DailyStabilityRow ────────────────────────────────────────────────────────

pub fn stability_to_csv(rows: &[DailyStabilityRow]) -> String {
    let mut out = String::from(
        "timestamp_ns,total_deposits_e8s,total_depositors,total_liquidations_executed,\
total_interest_received_e8s,stablecoin_balances,collateral_gains\n",
    );
    for r in rows {
        let sb: String = r
            .stablecoin_balances
            .iter()
            .map(|(p, v)| format!("{}:{}", p, v))
            .collect::<Vec<_>>()
            .join(";");
        let cg: String = r
            .collateral_gains
            .iter()
            .map(|(p, v)| format!("{}:{}", p, v))
            .collect::<Vec<_>>()
            .join(";");
        out.push_str(&format!(
            "{},{},{},{},{},\"{}\",\"{}\"\n",
            r.timestamp_ns,
            r.total_deposits_e8s,
            r.total_depositors,
            r.total_liquidations_executed,
            r.total_interest_received_e8s,
            sb,
            cg,
        ));
    }
    out
}

// ── DailySwapRollup ──────────────────────────────────────────────────────────

pub fn swaps_to_csv(rows: &[DailySwapRollup]) -> String {
    let mut out = String::from(
        "timestamp_ns,three_pool_swap_count,amm_swap_count,three_pool_volume_e8s,\
amm_volume_e8s,three_pool_fees_e8s,amm_fees_e8s,unique_swappers\n",
    );
    for r in rows {
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{}\n",
            r.timestamp_ns,
            r.three_pool_swap_count,
            r.amm_swap_count,
            r.three_pool_volume_e8s,
            r.amm_volume_e8s,
            r.three_pool_fees_e8s,
            r.amm_fees_e8s,
            r.unique_swappers,
        ));
    }
    out
}

// ── DailyLiquidationRollup ───────────────────────────────────────────────────

pub fn liquidations_to_csv(rows: &[DailyLiquidationRollup]) -> String {
    let mut out = String::from(
        "timestamp_ns,full_count,partial_count,redistribution_count,\
total_collateral_seized_e8s,total_debt_covered_e8s,by_collateral\n",
    );
    for r in rows {
        let bc: String = r
            .by_collateral
            .iter()
            .map(|(p, v)| format!("{}:{}", p, v))
            .collect::<Vec<_>>()
            .join(";");
        out.push_str(&format!(
            "{},{},{},{},{},{},\"{}\"\n",
            r.timestamp_ns,
            r.full_count,
            r.partial_count,
            r.redistribution_count,
            r.total_collateral_seized_e8s,
            r.total_debt_covered_e8s,
            bc,
        ));
    }
    out
}

// ── DailyFeeRollup ───────────────────────────────────────────────────────────

pub fn fees_to_csv(rows: &[DailyFeeRollup]) -> String {
    let mut out = String::from(
        "timestamp_ns,borrowing_fees_e8s,borrow_count,swap_fees_e8s,\
redemption_fees_e8s,redemption_count\n",
    );
    for r in rows {
        out.push_str(&format!(
            "{},{},{},{},{},{}\n",
            r.timestamp_ns,
            r.borrowing_fees_e8s.map_or(String::new(), |v| v.to_string()),
            r.borrow_count,
            r.swap_fees_e8s,
            r.redemption_fees_e8s.map_or(String::new(), |v| v.to_string()),
            r.redemption_count,
        ));
    }
    out
}

// ── FastPriceSnapshot ────────────────────────────────────────────────────────

pub fn fast_prices_to_csv(rows: &[FastPriceSnapshot]) -> String {
    let mut out = String::from("timestamp_ns,prices\n");
    for r in rows {
        // Serialize as principal:price_usd:symbol tuples separated by semicolons.
        let prices: String = r
            .prices
            .iter()
            .map(|(p, price, sym)| format!("{}:{:.8}:{}", p, price, sym))
            .collect::<Vec<_>>()
            .join(";");
        out.push_str(&format!("{},\"{}\"\n", r.timestamp_ns, prices));
    }
    out
}
