//! 3pool swap event tailing. Writes to EVT_SWAPS.

use candid::Principal;
use crate::{sources, state, storage};
use storage::cursors;
use storage::events::*;
use super::{BATCH_SIZE, update_cursor_success, update_cursor_error, update_cursor_source_count};

const THREE_POOL_TOKENS: [&str; 3] = [
    "t6bor-paaaa-aaaap-qrd5q-cai",  // icUSD
    "cngnf-vqaaa-aaaar-qag4q-cai",  // ckUSDT
    "xevnm-gaaaa-aaaar-qafnq-cai",  // ckUSDC
];

fn token_principal(idx: u8) -> Principal {
    Principal::from_text(THREE_POOL_TOKENS[idx as usize]).unwrap()
}

fn nat_to_u64(n: &candid::Nat) -> u64 {
    use num_traits::ToPrimitive;
    n.0.to_u64().unwrap_or(u64::MAX)
}

pub async fn run() {
    let three_pool = state::read_state(|s| s.sources.three_pool);
    let cursor = cursors::three_pool_swaps::get();

    let count = match sources::three_pool::get_swap_event_count(three_pool).await {
        Ok(c) => c,
        Err(e) => {
            ic_cdk::println!("[tail_3pool_swaps] get_swap_event_count failed: {}", e);
            state::mutate_state(|s| {
                s.error_counters.three_pool += 1;
                update_cursor_error(s, cursors::CURSOR_ID_3POOL_SWAPS, e);
            });
            return;
        }
    };

    state::mutate_state(|s| {
        update_cursor_source_count(s, cursors::CURSOR_ID_3POOL_SWAPS, count);
    });

    if count <= cursor { return; }

    let fetch_len = (count - cursor).min(BATCH_SIZE);
    let events = match sources::three_pool::get_swap_events(three_pool, cursor, fetch_len).await {
        Ok(e) => e,
        Err(e) => {
            ic_cdk::println!("[tail_3pool_swaps] get_swap_events failed: {}", e);
            state::mutate_state(|s| {
                s.error_counters.three_pool += 1;
                update_cursor_error(s, cursors::CURSOR_ID_3POOL_SWAPS, e);
            });
            return;
        }
    };

    let mut processed = 0u64;
    for evt in &events {
        evt_swaps::push(AnalyticsSwapEvent {
            timestamp_ns: evt.timestamp,
            source: SwapSource::ThreePool,
            source_event_id: evt.id,
            caller: evt.caller,
            token_in: token_principal(evt.token_in),
            token_out: token_principal(evt.token_out),
            amount_in: nat_to_u64(&evt.amount_in),
            amount_out: nat_to_u64(&evt.amount_out),
            fee: nat_to_u64(&evt.fee),
        });
        processed += 1;
    }

    if processed > 0 {
        cursors::three_pool_swaps::set(cursor + processed);
        state::mutate_state(|s| {
            update_cursor_success(s, cursors::CURSOR_ID_3POOL_SWAPS, ic_cdk::api::time());
        });
    }
}
