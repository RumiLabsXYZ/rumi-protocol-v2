//! Daily TVL collector.
//!
//! Reads current protocol status from the backend, builds a DailyTvlRow,
//! appends it to the DAILY_TVL StableLog. On error, increments the backend
//! error counter and returns Err so the caller can log it. Never panics,
//! never writes a partial row.

use crate::{sources, state, storage};

pub async fn run() -> Result<(), String> {
    let backend = state::read_state(|s| s.sources.backend);
    let status = match sources::backend::get_protocol_status(backend).await {
        Ok(s) => s,
        Err(e) => {
            state::mutate_state(|s| s.error_counters.backend += 1);
            return Err(e);
        }
    };

    // Convert f64 CR (e.g. 1.85 = 185%) to basis points (18500).
    // Saturate at u32::MAX to be safe; the source CR is bounded by protocol logic.
    let cr_bps = (status.total_collateral_ratio * 10_000.0)
        .clamp(0.0, u32::MAX as f64) as u32;

    let row = storage::DailyTvlRow {
        timestamp_ns: ic_cdk::api::time(),
        total_icp_collateral_e8s: status.total_icp_margin as u128,
        total_icusd_supply_e8s: status.total_icusd_borrowed as u128,
        system_collateral_ratio_bps: cr_bps,
    };
    storage::daily_tvl::push(row);

    state::mutate_state(|s| s.last_daily_snapshot_ns = ic_cdk::api::time());
    Ok(())
}
