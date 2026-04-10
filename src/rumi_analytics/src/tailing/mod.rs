//! Event tailing functions. Each module implements a single cursor's
//! fetch-process-advance cycle.

pub mod backend_events;
pub mod three_pool_swaps;
pub mod three_pool_liquidity;
pub mod amm_swaps;
pub mod icrc3;

pub const BATCH_SIZE: u64 = 500;
pub const BACKFILL_BATCH_SIZE: u64 = 1000;

// --- Cursor metadata helpers ---
// These update the Option<HashMap> fields in SlimState.

use crate::storage;

pub(crate) fn update_cursor_success(s: &mut storage::SlimState, cursor_id: u8, timestamp_ns: u64) {
    let map = s.cursor_last_success.get_or_insert_with(Default::default);
    map.insert(cursor_id, timestamp_ns);
    if let Some(err_map) = &mut s.cursor_last_error {
        err_map.remove(&cursor_id);
    }
}

pub(crate) fn update_cursor_error(s: &mut storage::SlimState, cursor_id: u8, error: String) {
    let map = s.cursor_last_error.get_or_insert_with(Default::default);
    map.insert(cursor_id, error);
}

pub(crate) fn update_cursor_source_count(s: &mut storage::SlimState, cursor_id: u8, count: u64) {
    let map = s.cursor_source_counts.get_or_insert_with(Default::default);
    map.insert(cursor_id, count);
}
