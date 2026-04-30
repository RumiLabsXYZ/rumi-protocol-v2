//! AMM liquidity event tailing. Mirrors AddLiquidity / RemoveLiquidity events
//! into evt_amm_liquidity so the address-value query can reconstruct per-
//! principal LP-share timelines without making inter-canister calls.

use crate::{sources, state, storage};
use storage::cursors;
use storage::events::*;
use super::{BATCH_SIZE, update_cursor_success, update_cursor_error, update_cursor_source_count};

fn nat_to_u64(n: &candid::Nat) -> u64 {
    use num_traits::ToPrimitive;
    n.0.to_u64().unwrap_or(u64::MAX)
}

fn convert_action(action: &sources::amm::AmmLiquidityAction) -> LiquidityAction {
    use sources::amm::AmmLiquidityAction::*;
    match action {
        AddLiquidity => LiquidityAction::Add,
        RemoveLiquidity => LiquidityAction::Remove,
    }
}

pub async fn run() {
    let amm = state::read_state(|s| s.sources.amm);
    let cursor = cursors::amm_liquidity::get();

    let count = match sources::amm::get_amm_liquidity_event_count(amm).await {
        Ok(c) => c,
        Err(e) => {
            ic_cdk::println!("[tail_amm_liq] get_amm_liquidity_event_count failed: {}", e);
            state::mutate_state(|s| {
                s.error_counters.amm += 1;
                update_cursor_error(s, cursors::CURSOR_ID_AMM_LIQUIDITY, e);
            });
            return;
        }
    };

    state::mutate_state(|s| {
        update_cursor_source_count(s, cursors::CURSOR_ID_AMM_LIQUIDITY, count);
    });

    if count <= cursor { return; }

    let fetch_len = (count - cursor).min(BATCH_SIZE);
    let events = match sources::amm::get_amm_liquidity_events(amm, cursor, fetch_len).await {
        Ok(e) => e,
        Err(e) => {
            ic_cdk::println!("[tail_amm_liq] get_amm_liquidity_events failed: {}", e);
            state::mutate_state(|s| {
                s.error_counters.amm += 1;
                update_cursor_error(s, cursors::CURSOR_ID_AMM_LIQUIDITY, e);
            });
            return;
        }
    };

    let mut processed = 0u64;
    for evt in &events {
        evt_amm_liquidity::push(AnalyticsAmmLiquidityEvent {
            timestamp_ns: evt.timestamp,
            source_event_id: evt.id,
            caller: evt.caller,
            pool_id: evt.pool_id.clone(),
            action: convert_action(&evt.action),
            lp_shares: nat_to_u64(&evt.lp_shares),
        });
        processed += 1;
    }

    if processed > 0 {
        cursors::amm_liquidity::set(cursor + processed);
        state::mutate_state(|s| {
            update_cursor_success(s, cursors::CURSOR_ID_AMM_LIQUIDITY, ic_cdk::api::time());
        });
    }
}
