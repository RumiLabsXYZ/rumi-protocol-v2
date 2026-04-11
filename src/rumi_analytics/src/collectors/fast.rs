//! Fast (5-minute) snapshot collector. Captures collateral prices and 3pool state.

use crate::{sources, state, storage};

pub async fn run() -> Result<(), String> {
    let (backend_id, three_pool_id) = state::read_state(|s| {
        (s.sources.backend, s.sources.three_pool)
    });

    let (prices_res, pool_res) = futures::join!(
        sources::backend::get_collateral_totals(backend_id),
        sources::three_pool::get_pool_status(three_pool_id),
    );

    let now = ic_cdk::api::time();

    match prices_res {
        Ok(totals) => {
            let prices = totals.into_iter()
                .map(|t| (t.collateral_type, t.price, t.symbol))
                .collect();
            storage::fast::fast_prices::push(storage::fast::FastPriceSnapshot {
                timestamp_ns: now,
                prices,
            });
        }
        Err(e) => {
            ic_cdk::println!("[fast] get_collateral_totals error: {}", e);
            state::mutate_state(|s| s.error_counters.backend += 1);
        }
    }

    match pool_res {
        Ok(tp) => {
            storage::fast::fast_3pool::push(storage::fast::Fast3PoolSnapshot {
                timestamp_ns: now,
                balances: tp.balances,
                virtual_price: tp.virtual_price,
                lp_total_supply: tp.lp_total_supply,
                decimals: tp.decimals,
            });
        }
        Err(e) => {
            ic_cdk::println!("[fast] get_pool_status error: {}", e);
            state::mutate_state(|s| s.error_counters.three_pool += 1);
        }
    }

    Ok(())
}
