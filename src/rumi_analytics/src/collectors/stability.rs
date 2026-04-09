//! Daily stability pool snapshot collector.

use crate::{sources, state, storage};

pub async fn run() -> Result<(), String> {
    let sp_id = state::read_state(|s| s.sources.stability_pool);

    let status = match sources::stability_pool::get_pool_status(sp_id).await {
        Ok(s) => s,
        Err(e) => {
            state::mutate_state(|s| s.error_counters.stability_pool += 1);
            return Err(e);
        }
    };

    let row = storage::DailyStabilityRow {
        timestamp_ns: ic_cdk::api::time(),
        total_deposits_e8s: status.total_deposits_e8s,
        total_depositors: status.total_depositors,
        total_liquidations_executed: status.total_liquidations_executed,
        total_interest_received_e8s: status.total_interest_received_e8s,
        stablecoin_balances: status.stablecoin_balances,
        collateral_gains: status.collateral_gains,
    };
    storage::daily_stability::push(row);

    Ok(())
}
