//! 3pool liquidity event tailing. Writes to EVT_LIQUIDITY.

use crate::{sources, state, storage};
use storage::cursors;
use storage::events::*;
use super::{BATCH_SIZE, update_cursor_success, update_cursor_error, update_cursor_source_count};

fn nat_to_u64(n: &candid::Nat) -> u64 {
    use num_traits::ToPrimitive;
    n.0.to_u64().unwrap_or(u64::MAX)
}

fn convert_action(action: &sources::three_pool::ThreePoolLiquidityAction) -> LiquidityAction {
    use sources::three_pool::ThreePoolLiquidityAction::*;
    match action {
        AddLiquidity => LiquidityAction::Add,
        RemoveLiquidity => LiquidityAction::Remove,
        RemoveOneCoin => LiquidityAction::RemoveOneCoin,
        Donate => LiquidityAction::Donate,
    }
}

pub async fn run() {
    let three_pool = state::read_state(|s| s.sources.three_pool);
    let cursor = cursors::three_pool_liquidity::get();

    let count = match sources::three_pool::get_liquidity_event_count(three_pool).await {
        Ok(c) => c,
        Err(e) => {
            ic_cdk::println!("[tail_3pool_liq] get_liquidity_event_count failed: {}", e);
            state::mutate_state(|s| {
                s.error_counters.three_pool += 1;
                update_cursor_error(s, cursors::CURSOR_ID_3POOL_LIQUIDITY, e);
            });
            return;
        }
    };

    state::mutate_state(|s| {
        update_cursor_source_count(s, cursors::CURSOR_ID_3POOL_LIQUIDITY, count);
    });

    if count <= cursor { return; }

    let fetch_len = (count - cursor).min(BATCH_SIZE);
    let events = match sources::three_pool::get_liquidity_events(three_pool, cursor, fetch_len).await {
        Ok(e) => e,
        Err(e) => {
            ic_cdk::println!("[tail_3pool_liq] get_liquidity_events failed: {}", e);
            state::mutate_state(|s| {
                s.error_counters.three_pool += 1;
                update_cursor_error(s, cursors::CURSOR_ID_3POOL_LIQUIDITY, e);
            });
            return;
        }
    };

    let mut processed = 0u64;
    for evt in &events {
        evt_liquidity::push(AnalyticsLiquidityEvent {
            timestamp_ns: evt.timestamp,
            source_event_id: evt.id,
            caller: evt.caller,
            action: convert_action(&evt.action),
            amounts: evt.amounts.iter().map(|n| nat_to_u64(n)).collect(),
            lp_amount: nat_to_u64(&evt.lp_amount),
            coin_index: evt.coin_index,
            fee: evt.fee.as_ref().map(|n| nat_to_u64(n)),
        });
        processed += 1;
    }

    if processed > 0 {
        cursors::three_pool_liquidity::set(cursor + processed);
        state::mutate_state(|s| {
            update_cursor_success(s, cursors::CURSOR_ID_3POOL_LIQUIDITY, ic_cdk::api::time());
        });
    }
}
