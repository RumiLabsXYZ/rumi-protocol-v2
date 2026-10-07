//! AMM swap event tailing. Writes to EVT_SWAPS.

use crate::{sources, state, storage};
use storage::cursors;
use storage::events::*;
use super::{BATCH_SIZE, update_cursor_success, update_cursor_error, update_cursor_source_count};

fn nat_to_u64(n: &candid::Nat) -> u64 {
    use num_traits::ToPrimitive;
    n.0.to_u64().unwrap_or(u64::MAX)
}

pub async fn run() {
    let amm = state::read_state(|s| s.sources.amm);
    let cursor = cursors::amm_swaps::get();

    let count = match sources::amm::get_amm_swap_event_count(amm).await {
        Ok(c) => c,
        Err(e) => {
            ic_cdk::println!("[tail_amm] get_amm_swap_event_count failed: {}", e);
            state::mutate_state(|s| {
                s.error_counters.amm += 1;
                update_cursor_error(s, cursors::CURSOR_ID_AMM_SWAPS, e);
            });
            return;
        }
    };

    state::mutate_state(|s| {
        update_cursor_source_count(s, cursors::CURSOR_ID_AMM_SWAPS, count);
    });

    if count <= cursor { return; }

    let fetch_len = (count - cursor).min(BATCH_SIZE);
    let events = match sources::amm::get_amm_swap_events(amm, cursor, fetch_len).await {
        Ok(e) => e,
        Err(e) => {
            ic_cdk::println!("[tail_amm] get_amm_swap_events failed: {}", e);
            state::mutate_state(|s| {
                s.error_counters.amm += 1;
                update_cursor_error(s, cursors::CURSOR_ID_AMM_SWAPS, e);
            });
            return;
        }
    };

    // A prior overlapping timer run may have committed this cursor while we
    // awaited the AMM. Only the first callback may append this batch.
    if cursors::amm_swaps::get() != cursor {
        return;
    }

    if events.is_empty() {
        state::mutate_state(|s| update_cursor_error(
            s, cursors::CURSOR_ID_AMM_SWAPS,
            format!("AMM reports {count} swap events but returned no row at cursor {cursor}"),
        ));
        return;
    }
    let mut next_cursor = cursor;
    let mut evicted = 0u64;
    for evt in &events {
        if evt.id < next_cursor {
            continue;
        }
        evicted = evicted.saturating_add(evt.id.saturating_sub(next_cursor));
        evt_swaps::push(AnalyticsSwapEvent {
            timestamp_ns: evt.timestamp,
            source: SwapSource::Amm,
            source_event_id: evt.id,
            caller: evt.caller,
            token_in: evt.token_in,
            token_out: evt.token_out,
            amount_in: nat_to_u64(&evt.amount_in),
            amount_out: nat_to_u64(&evt.amount_out),
            fee: nat_to_u64(&evt.fee),
        });
        next_cursor = evt.id.saturating_add(1);
    }

    if next_cursor > cursor {
        cursors::amm_swaps::set(next_cursor);
        state::mutate_state(|s| {
            if evicted > 0 {
                s.error_counters.amm = s.error_counters.amm.saturating_add(1);
                update_cursor_error(s, cursors::CURSOR_ID_AMM_SWAPS,
                    format!("AMM swap history gap: {evicted} events evicted before analytics read them"));
            } else {
                update_cursor_success(s, cursors::CURSOR_ID_AMM_SWAPS, ic_cdk::api::time());
            }
        });
    }
}
