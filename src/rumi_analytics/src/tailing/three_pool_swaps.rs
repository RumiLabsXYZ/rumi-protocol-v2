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
    tail_v1().await;
    tail_v2().await;
}

async fn tail_v1() {
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

/// Normalize a u128 amount to e8s (8-decimal) using its 3pool token slot.
/// v2 events carry amounts as `u128` rather than `Nat`, so we widen the
/// internal `u64` representation through u128 arithmetic before clamping.
fn normalize_u128_to_e8s(raw: u128, token_idx: u8) -> u64 {
    let decimals = THREE_POOL_DECIMALS.get(token_idx as usize).copied().unwrap_or(8);
    let normalized: u128 = if decimals >= 8 {
        raw / 10u128.pow((decimals - 8) as u32)
    } else {
        raw.saturating_mul(10u128.pow((8 - decimals) as u32))
    };
    normalized.min(u64::MAX as u128) as u64
}

/// Tail v2 swap events. Mirrors `tail_v1` but reads from the v2 log via
/// `get_swap_events_v2`, which paginates newest-first and exposes no count
/// endpoint. We derive the total from the newest event's `id` field. Skips
/// `migrated == true` entries since their v1 originals are already in
/// `evt_swaps` via `tail_v1`.
async fn tail_v2() {
    let three_pool = state::read_state(|s| s.sources.three_pool);
    let cursor = cursors::three_pool_swaps_v2::get();

    // Probe newest to derive total. Empty log → total = 0.
    let probe = match sources::three_pool::get_swap_events_v2(three_pool, 1, 0).await {
        Ok(e) => e,
        Err(e) => {
            ic_cdk::println!("[tail_3pool_swaps_v2] probe failed: {}", e);
            state::mutate_state(|s| {
                s.error_counters.three_pool += 1;
                update_cursor_error(s, cursors::CURSOR_ID_3POOL_SWAPS, e);
            });
            return;
        }
    };
    let total: u64 = probe.first().map(|e| e.id + 1).unwrap_or(0);

    state::mutate_state(|s| {
        update_cursor_source_count(s, cursors::CURSOR_ID_3POOL_SWAPS, total);
    });

    if total <= cursor { return; }

    // Fetch the oldest BATCH_SIZE unseen entries. v2 returns newest-first
    // within (limit, offset), so we offset past the newer entries we'll pick
    // up on later ticks and reverse the slice locally to push in id order.
    let unseen = total - cursor;
    let fetch_len = unseen.min(BATCH_SIZE);
    let offset = unseen - fetch_len;
    let mut events = match sources::three_pool::get_swap_events_v2(three_pool, fetch_len, offset).await {
        Ok(e) => e,
        Err(e) => {
            ic_cdk::println!("[tail_3pool_swaps_v2] get_swap_events_v2 failed: {}", e);
            state::mutate_state(|s| {
                s.error_counters.three_pool += 1;
                update_cursor_error(s, cursors::CURSOR_ID_3POOL_SWAPS, e);
            });
            return;
        }
    };
    events.reverse();

    // Sanity check: under offset/limit arithmetic the oldest fetched event
    // must have id == cursor. If it doesn't, the source grew between our
    // probe and our fetch (or returned an unexpected slice). Bail without
    // advancing — next tick re-probes and retries.
    if let Some(first) = events.first() {
        if first.id != cursor {
            ic_cdk::println!(
                "[tail_3pool_swaps_v2] id drift: expected first id={}, got {} (probe total={}). Will retry.",
                cursor, first.id, total
            );
            return;
        }
    }

    let mut advanced = 0u64;
    for evt in &events {
        // Always advance the cursor — even for migrated entries we skip — so
        // a v2 log dominated by migrated rows doesn't pin the cursor.
        advanced += 1;
        if evt.migrated {
            continue;
        }
        let (Some(token_in), Some(token_out)) =
            (token_principal(evt.token_in), token_principal(evt.token_out))
        else {
            ic_cdk::println!("[tail_3pool_swaps_v2] skipping event with out-of-range token index: in={} out={}", evt.token_in, evt.token_out);
            continue;
        };
        evt_swaps::push(AnalyticsSwapEvent {
            timestamp_ns: evt.timestamp,
            source: SwapSource::ThreePool,
            source_event_id: evt.id,
            caller: evt.caller,
            token_in,
            token_out,
            amount_in: normalize_u128_to_e8s(evt.amount_in, evt.token_in),
            amount_out: normalize_u128_to_e8s(evt.amount_out, evt.token_out),
            fee: normalize_u128_to_e8s(evt.fee, evt.token_out),
        });
    }

    if advanced > 0 {
        cursors::three_pool_swaps_v2::set(cursor + advanced);
        state::mutate_state(|s| {
            update_cursor_success(s, cursors::CURSOR_ID_3POOL_SWAPS, ic_cdk::api::time());
        });
    }
}
