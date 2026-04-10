//! Prometheus text format metrics endpoint.
//!
//! Serves the canonical set of Rumi analytics gauges and counters at /metrics.
//! All values are read from cached state — no inter-canister calls are made.

use crate::{state, storage};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn gauge(out: &mut String, name: &str, help: &str, value: f64) {
    out.push_str(&format!(
        "# HELP {} {}\n# TYPE {} gauge\n{} {}\n",
        name, help, name, name, value
    ));
}

fn counter(out: &mut String, name: &str, help: &str, label: &str, value: f64) {
    // Emit the HELP/TYPE header only once per metric family. The caller must
    // push the header before the first labeled series and skip it for
    // subsequent ones. We therefore split header emission from the series line.
    let _ = (name, help, label, value); // suppress lint; see render() below
}

// ── Public entry point ────────────────────────────────────────────────────────

pub fn render() -> String {
    let _ = counter; // silence dead-code warning; header logic is inline below

    let mut out = String::with_capacity(2048);

    // ── Supply gauges ────────────────────────────────────────────────────────
    let (icusd_e8s, three_usd_e8s) = state::read_state(|s| {
        (s.circulating_supply_icusd_e8s, s.circulating_supply_3usd_e8s)
    });

    gauge(
        &mut out,
        "rumi_icusd_supply_e8s",
        "Circulating supply of icUSD in e8s",
        icusd_e8s.unwrap_or(0) as f64,
    );
    gauge(
        &mut out,
        "rumi_3usd_supply_e8s",
        "Circulating supply of 3USD in e8s",
        three_usd_e8s.unwrap_or(0) as f64,
    );

    // ── Vault gauges (from latest DailyVaultSnapshotRow) ─────────────────────
    let vault_len = storage::daily_vaults::len();
    if vault_len > 0 {
        if let Some(row) = storage::daily_vaults::get(vault_len - 1) {
            gauge(
                &mut out,
                "rumi_total_vault_count",
                "Total number of active vaults",
                row.total_vault_count as f64,
            );
            gauge(
                &mut out,
                "rumi_total_collateral_usd_e8s",
                "Total collateral value across all vaults in USD e8s",
                row.total_collateral_usd_e8s as f64,
            );
            gauge(
                &mut out,
                "rumi_total_debt_e8s",
                "Total icUSD debt across all vaults in e8s",
                row.total_debt_e8s as f64,
            );
            gauge(
                &mut out,
                "rumi_system_cr_bps",
                "System-wide median collateral ratio in basis points",
                row.median_cr_bps as f64,
            );
        }
    }

    // ── TVL gauges (from latest DailyTvlRow) ─────────────────────────────────
    let tvl_len = storage::daily_tvl::len();
    if tvl_len > 0 {
        if let Some(row) = storage::daily_tvl::get(tvl_len - 1) {
            gauge(
                &mut out,
                "rumi_total_icp_collateral_e8s",
                "Total ICP collateral deposited across all vaults in e8s",
                row.total_icp_collateral_e8s as f64,
            );
            gauge(
                &mut out,
                "rumi_total_icusd_supply_e8s",
                "Total icUSD supply as recorded in the daily TVL snapshot in e8s",
                row.total_icusd_supply_e8s as f64,
            );
        }
    }

    // ── Error counters (labeled by source) ───────────────────────────────────
    let ec = state::read_state(|s| s.error_counters.clone());

    let counter_name = "rumi_collector_errors_total";
    out.push_str(&format!(
        "# HELP {} Cumulative collector errors by source canister\n# TYPE {} counter\n",
        counter_name, counter_name
    ));
    for (source, value) in [
        ("backend", ec.backend),
        ("icusd_ledger", ec.icusd_ledger),
        ("three_pool", ec.three_pool),
        ("stability_pool", ec.stability_pool),
        ("amm", ec.amm),
    ] {
        out.push_str(&format!(
            "{}{{source=\"{}\"}} {}\n",
            counter_name, source, value
        ));
    }

    // ── Storage size gauges ───────────────────────────────────────────────────
    gauge(
        &mut out,
        "rumi_storage_daily_tvl_rows",
        "Number of rows in the daily TVL storage log",
        storage::daily_tvl::len() as f64,
    );
    gauge(
        &mut out,
        "rumi_storage_daily_vault_rows",
        "Number of rows in the daily vault snapshot storage log",
        storage::daily_vaults::len() as f64,
    );
    gauge(
        &mut out,
        "rumi_storage_evt_swaps_rows",
        "Number of rows in the swap events storage log",
        storage::events::evt_swaps::len() as f64,
    );
    gauge(
        &mut out,
        "rumi_storage_evt_liquidations_rows",
        "Number of rows in the liquidation events storage log",
        storage::events::evt_liquidations::len() as f64,
    );
    gauge(
        &mut out,
        "rumi_storage_fast_prices_rows",
        "Number of rows in the fast price snapshot storage log",
        storage::fast::fast_prices::len() as f64,
    );

    out
}
