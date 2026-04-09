//! Daily TVL collector.
//!
//! Reads current protocol status from the backend canister, stability pool, and
//! 3pool concurrently. Backend failure aborts the entire row (no partial write).
//! Stability pool and 3pool failures are soft: they log an error, increment the
//! per-source error counter, and store None for those fields. Never panics,
//! never writes a partial row.

use crate::{sources, state, storage};

pub async fn run() -> Result<(), String> {
    let (backend_id, stability_pool_id, three_pool_id) = state::read_state(|s| {
        (
            s.sources.backend,
            s.sources.stability_pool,
            s.sources.three_pool,
        )
    });

    // Fire all three queries concurrently.
    let (backend_res, sp_res, tp_res) = futures::join!(
        sources::backend::get_protocol_status(backend_id),
        sources::stability_pool::get_pool_status(stability_pool_id),
        sources::three_pool::get_pool_status(three_pool_id),
    );

    // Backend is required; abort on failure.
    let status = match backend_res {
        Ok(s) => s,
        Err(e) => {
            state::mutate_state(|s| s.error_counters.backend += 1);
            return Err(e);
        }
    };

    // Stability pool is optional; log and continue on failure.
    let sp_deposits = match sp_res {
        Ok(sp) => Some(sp.total_deposits_e8s),
        Err(e) => {
            ic_cdk::println!("[tvl] stability_pool error: {}", e);
            state::mutate_state(|s| s.error_counters.stability_pool += 1);
            None
        }
    };

    // 3pool is optional; log and continue on failure.
    let (tp_reserve_0, tp_reserve_1, tp_reserve_2, tp_virtual_price, tp_lp_supply) =
        match tp_res {
            Ok(tp) => {
                let reserve_0 = tp.balances.first().copied();
                let reserve_1 = tp.balances.get(1).copied();
                let reserve_2 = tp.balances.get(2).copied();
                (
                    reserve_0,
                    reserve_1,
                    reserve_2,
                    Some(tp.virtual_price),
                    Some(tp.lp_total_supply),
                )
            }
            Err(e) => {
                ic_cdk::println!("[tvl] three_pool error: {}", e);
                state::mutate_state(|s| s.error_counters.three_pool += 1);
                (None, None, None, None, None)
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
        stability_pool_deposits_e8s: sp_deposits,
        three_pool_reserve_0_e8s: tp_reserve_0,
        three_pool_reserve_1_e8s: tp_reserve_1,
        three_pool_reserve_2_e8s: tp_reserve_2,
        three_pool_virtual_price_e18: tp_virtual_price,
        three_pool_lp_supply_e8s: tp_lp_supply,
    };
    storage::daily_tvl::push(row);

    state::mutate_state(|s| s.last_daily_snapshot_ns = ic_cdk::api::time());
    Ok(())
}
