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
    tail_v1().await;
    tail_v2().await;
}

async fn tail_v1() {
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

fn u128_to_u64(v: u128) -> u64 {
    v.min(u64::MAX as u128) as u64
}

/// Tail v2 liquidity events. Has a real `get_liquidity_event_count_v2`
/// endpoint (unlike the v2 swap log) so we can paginate without a probe.
async fn tail_v2() {
    let three_pool = state::read_state(|s| s.sources.three_pool);
    let cursor = cursors::three_pool_liquidity_v2::get();

    let total = match sources::three_pool::get_liquidity_event_count_v2(three_pool).await {
        Ok(c) => c,
        Err(e) => {
            ic_cdk::println!("[tail_3pool_liq_v2] get_liquidity_event_count_v2 failed: {}", e);
            state::mutate_state(|s| {
                s.error_counters.three_pool += 1;
                update_cursor_error(s, cursors::CURSOR_ID_3POOL_LIQUIDITY, e);
            });
            return;
        }
    };

    state::mutate_state(|s| {
        update_cursor_source_count(s, cursors::CURSOR_ID_3POOL_LIQUIDITY, total);
    });

    if total <= cursor { return; }

    // v2 returns newest-first within (limit, offset); offset past the newer
    // unseen tail and reverse so we push in id order.
    let unseen = total - cursor;
    let fetch_len = unseen.min(BATCH_SIZE);
    let offset = unseen - fetch_len;
    let mut events = match sources::three_pool::get_liquidity_events_v2(three_pool, fetch_len, offset).await {
        Ok(e) => e,
        Err(e) => {
            ic_cdk::println!("[tail_3pool_liq_v2] get_liquidity_events_v2 failed: {}", e);
            state::mutate_state(|s| {
                s.error_counters.three_pool += 1;
                update_cursor_error(s, cursors::CURSOR_ID_3POOL_LIQUIDITY, e);
            });
            return;
        }
    };
    events.reverse();

    // Drift guard — see the matching comment in `tail_v2` for swaps.
    if let Some(first) = events.first() {
        if first.id != cursor {
            ic_cdk::println!(
                "[tail_3pool_liq_v2] id drift: expected first id={}, got {} (count={}). Will retry.",
                cursor, first.id, total
            );
            return;
        }
    }

    let mut advanced = 0u64;
    for evt in &events {
        // Advance the cursor on migrated entries too — they were already
        // mirrored via tail_v1, and skipping them must not pin the cursor.
        advanced += 1;
        if evt.migrated {
            continue;
        }
        evt_liquidity::push(AnalyticsLiquidityEvent {
            timestamp_ns: evt.timestamp,
            source_event_id: evt.id,
            caller: evt.caller,
            action: convert_action(&evt.action),
            amounts: evt.amounts.iter().map(|&n| u128_to_u64(n)).collect(),
            lp_amount: u128_to_u64(evt.lp_amount),
            coin_index: evt.coin_index,
            fee: evt.fee.map(u128_to_u64),
        });
    }

    if advanced > 0 {
        cursors::three_pool_liquidity_v2::set(cursor + advanced);
        state::mutate_state(|s| {
            update_cursor_success(s, cursors::CURSOR_ID_3POOL_LIQUIDITY, ic_cdk::api::time());
        });
    }
}
