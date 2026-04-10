//! Hourly snapshot collector. Captures cycle balance and fee curve state.

use crate::{sources, state, storage};

pub async fn run() -> Result<(), String> {
    let backend_id = state::read_state(|s| s.sources.backend);
    let now = ic_cdk::api::time();

    let cycle_balance = ic_cdk::api::canister_balance128();
    storage::hourly::hourly_cycles::push(storage::hourly::HourlyCycleSnapshot {
        timestamp_ns: now,
        cycle_balance,
    });

    match sources::backend::get_collateral_totals(backend_id).await {
        Ok(totals) => {
            let system_cr_bps = {
                // total_collateral is in raw e8s, price is USD per whole token.
                // collateral_value_e8s = total_collateral * price (same as vaults.rs).
                // total_debt is in icUSD e8s. CR = collateral_value / debt.
                let total_coll_value: f64 = totals.iter()
                    .map(|t| t.total_collateral as f64 * t.price)
                    .sum();
                let total_debt: f64 = totals.iter()
                    .map(|t| t.total_debt as f64)
                    .sum();
                if total_debt > 0.0 {
                    ((total_coll_value / total_debt) * 10_000.0).clamp(0.0, u32::MAX as f64) as u32
                } else {
                    0
                }
            };
            let collateral_stats = totals.into_iter()
                .map(|t| (t.collateral_type, t.total_debt, t.total_collateral, t.price))
                .collect();
            storage::hourly::hourly_fee_curve::push(storage::hourly::HourlyFeeCurveSnapshot {
                timestamp_ns: now,
                system_cr_bps,
                collateral_stats,
            });
        }
        Err(e) => {
            ic_cdk::println!("[hourly] get_collateral_totals error: {}", e);
            state::mutate_state(|s| s.error_counters.backend += 1);
        }
    }

    Ok(())
}
