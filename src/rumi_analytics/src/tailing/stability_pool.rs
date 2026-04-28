//! Stability-pool event tailing. Pulls events from rumi_stability_pool's
//! own event log (get_pool_events / get_pool_event_count) and pushes
//! Deposit / Withdraw / ClaimCollateral into evt_stability so the Top SP
//! Depositors leaderboard and address-page SP timeline start working.
//!
//! The cursor starts at 0 on first deploy, so the first tail run backfills
//! the full SP event history.

use crate::{sources, state, storage};
use storage::cursors;
use storage::events::*;
use super::{BATCH_SIZE, update_cursor_success, update_cursor_error, update_cursor_source_count};

pub async fn run() {
    let sp = state::read_state(|s| s.sources.stability_pool);

    // Skip if not yet configured.
    if sp == candid::Principal::anonymous() {
        return;
    }

    let cursor = cursors::stability_events::get();

    let count = match sources::stability_pool::get_pool_event_count(sp).await {
        Ok(c) => c,
        Err(e) => {
            ic_cdk::println!("[tail_stability_pool] get_pool_event_count failed: {}", e);
            state::mutate_state(|s| {
                s.error_counters.stability_pool += 1;
                update_cursor_error(s, cursors::CURSOR_ID_STABILITY_EVENTS, e);
            });
            return;
        }
    };

    state::mutate_state(|s| {
        update_cursor_source_count(s, cursors::CURSOR_ID_STABILITY_EVENTS, count);
    });

    if count <= cursor { return; }

    let fetch_len = (count - cursor).min(BATCH_SIZE);
    let events = match sources::stability_pool::get_pool_events(sp, cursor, fetch_len).await {
        Ok(e) => e,
        Err(e) => {
            ic_cdk::println!("[tail_stability_pool] get_pool_events failed: {}", e);
            state::mutate_state(|s| {
                s.error_counters.stability_pool += 1;
                update_cursor_error(s, cursors::CURSOR_ID_STABILITY_EVENTS, e);
            });
            return;
        }
    };

    let mut processed = 0u64;
    for (i, event) in events.iter().enumerate() {
        let event_id = cursor + i as u64;
        route_sp_event(event_id, event);
        processed += 1;
    }

    if processed > 0 {
        cursors::stability_events::set(cursor + processed);
        state::mutate_state(|s| {
            update_cursor_success(s, cursors::CURSOR_ID_STABILITY_EVENTS, ic_cdk::api::time());
        });
    }
}

fn route_sp_event(event_id: u64, event: &sources::stability_pool::PoolEvent) {
    use sources::stability_pool::PoolEventType::*;

    match &event.event_type {
        Deposit { amount, .. } => {
            evt_stability::push(AnalyticsStabilityEvent {
                timestamp_ns: event.timestamp,
                source_event_id: event_id,
                caller: event.caller,
                action: StabilityAction::Deposit,
                amount: *amount,
            });
        }
        DepositAs3USD { amount_in, .. } => {
            evt_stability::push(AnalyticsStabilityEvent {
                timestamp_ns: event.timestamp,
                source_event_id: event_id,
                caller: event.caller,
                action: StabilityAction::Deposit,
                amount: *amount_in,
            });
        }
        Withdraw { amount, .. } => {
            evt_stability::push(AnalyticsStabilityEvent {
                timestamp_ns: event.timestamp,
                source_event_id: event_id,
                caller: event.caller,
                action: StabilityAction::Withdraw,
                amount: *amount,
            });
        }
        ClaimCollateral { amount, .. } => {
            evt_stability::push(AnalyticsStabilityEvent {
                timestamp_ns: event.timestamp,
                source_event_id: event_id,
                caller: event.caller,
                action: StabilityAction::ClaimReturns,
                amount: *amount,
            });
        }
        // InterestReceived is a protocol-internal credit, not a user action.
        // Skip it silently — it doesn't affect the depositor leaderboard.
        InterestReceived { .. } => {}
    }
}
