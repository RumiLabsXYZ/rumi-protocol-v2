//! 3pool swap event tailing. Writes to EVT_SWAPS.

use candid::Principal;
use crate::{sources, state, storage};
use storage::cursors;
use storage::events::*;
use super::{BATCH_SIZE, update_cursor_success, update_cursor_error, update_cursor_source_count};

// Mainnet token principals for 3pool indices 0/1/2.
const THREE_POOL_TOKENS: [&str; 3] = [
    "t6bor-paaaa-aaaap-qrd5q-cai",  // icUSD  (8 decimals)
    "cngnf-vqaaa-aaaar-qag4q-cai",  // ckUSDT (6 decimals)
    "xevnm-gaaaa-aaaar-qafnq-cai",  // ckUSDC (6 decimals)
];

// Token decimals for 3pool indices 0/1/2.
const THREE_POOL_DECIMALS: [u8; 3] = [8, 6, 6];

fn token_principal(idx: u8) -> Option<Principal> {
    THREE_POOL_TOKENS.get(idx as usize)
        .and_then(|t| Principal::from_text(t).ok())
}

fn nat_to_u64(n: &candid::Nat) -> u64 {
    use num_traits::ToPrimitive;
    n.0.to_u64().unwrap_or(u64::MAX)
}

/// Normalize a raw token amount to e8s (8-decimal) representation.
/// For 6-decimal tokens (ckUSDT, ckUSDC), multiply by 100.
/// For 8-decimal tokens (icUSD), no change needed.
fn normalize_to_e8s(raw: u64, token_idx: u8) -> u64 {
    let decimals = THREE_POOL_DECIMALS.get(token_idx as usize).copied().unwrap_or(8);
    if decimals >= 8 {
        raw / 10u64.pow((decimals - 8) as u32)
    } else {
        raw.saturating_mul(10u64.pow((8 - decimals) as u32))
    }
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
        let (Some(token_in), Some(token_out)) =
            (token_principal(evt.token_in), token_principal(evt.token_out))
        else {
            ic_cdk::println!("[tail_3pool_swaps] skipping event with out-of-range token index: in={} out={}", evt.token_in, evt.token_out);
            processed += 1;
            continue;
        };
        evt_swaps::push(AnalyticsSwapEvent {
            timestamp_ns: evt.timestamp,
            source: SwapSource::ThreePool,
            source_event_id: evt.id,
            caller: evt.caller,
            token_in,
            token_out,
            amount_in: normalize_to_e8s(nat_to_u64(&evt.amount_in), evt.token_in),
            amount_out: normalize_to_e8s(nat_to_u64(&evt.amount_out), evt.token_out),
            fee: normalize_to_e8s(nat_to_u64(&evt.fee), evt.token_in),
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
