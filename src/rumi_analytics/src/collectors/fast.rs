//! Fast (5-minute) snapshot collector. Captures collateral prices and 3pool state.

use crate::{sources, state, storage};

pub async fn run() -> Result<(), String> {
    let (backend_id, three_pool_id, amm_id) = state::read_state(|s| {
        (s.sources.backend, s.sources.three_pool, s.sources.amm)
    });

    let (prices_res, pool_res, amm_pools_res) = futures::join!(
        sources::backend::get_collateral_totals(backend_id),
        sources::three_pool::get_pool_status(three_pool_id),
        sources::amm::get_pools(amm_id),
    );

    let now = ic_cdk::api::time();

    match prices_res {
        Ok(totals) => {
            // Refresh the heap-side decimals lookup before we drop `totals`.
            // Persistent across upgrades via SlimState; pricing queries read
            // it to avoid the silently-wrong "all collateral is 8 decimals"
            // assumption that inflated 18-decimal tokens (ckETH) by 1e10.
            let decimals_map: std::collections::HashMap<candid::Principal, u8> = totals
                .iter()
                .map(|t| (t.collateral_type, t.decimals))
                .collect();
            state::mutate_state(|s| s.collateral_decimals = Some(decimals_map));

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
                decimals: Some(tp.decimals),
            });
        }
        Err(e) => {
            ic_cdk::println!("[fast] get_pool_status error: {}", e);
            state::mutate_state(|s| s.error_counters.three_pool += 1);
        }
    }

    match amm_pools_res {
        Ok(pools) => {
            use num_traits::ToPrimitive;
            let snaps: Vec<storage::AmmPoolSnapshot> = pools
                .into_iter()
                .map(|p| storage::AmmPoolSnapshot {
                    pool_id: p.pool_id,
                    token_a: p.token_a,
                    token_b: p.token_b,
                    reserve_a: p.reserve_a.0.to_u128().unwrap_or(0),
                    reserve_b: p.reserve_b.0.to_u128().unwrap_or(0),
                    total_lp_shares: p.total_lp_shares.0.to_u128().unwrap_or(0),
                })
                .collect();
            state::mutate_state(|s| s.amm_pools = Some(snaps));
        }
        Err(e) => {
            ic_cdk::println!("[fast] amm get_pools error: {}", e);
            state::mutate_state(|s| s.error_counters.amm += 1);
        }
    }

    Ok(())
}
