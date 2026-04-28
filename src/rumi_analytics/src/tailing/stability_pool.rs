//! Stability-pool event tailing. Pulls events from rumi_stability_pool's
//! own event log (get_pool_events / get_pool_event_count) and pushes
//! Deposit / Withdraw / ClaimCollateral into evt_stability so the Top SP
//! Depositors leaderboard and address-page SP timeline start working.
//!
//! The rumi_stability_pool canister keeps a ring buffer of at most 10,000
//! events (MAX_POOL_EVENTS). When the buffer fills, oldest events are drained.
//! get_pool_event_count() returns the current window size, not a lifetime
//! monotonic count; get_pool_events(start, length) indexes by array position.
//!
//! To survive ring-buffer rotation we use an id-based cursor: the stored value
//! is `last_seen_event_id + 1` (i.e. the lowest event.id we still want to
//! ingest). event.id is set from the SP canister's monotonic next_event_id
//! counter, so it is stable across buffer rotation. Saved value 0 means
//! "never run; want id >= 0".
//!
//! Each tick we fetch the latest BATCH_SIZE events from the tail of the
//! current window and filter by event.id >= next_wanted_id. If the SP
//! produces more than BATCH_SIZE events in 60 seconds the oldest of those
//! would be missed, but that rate (>500 SP events/min) is unrealistic.

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

    // Cursor stores `last_seen_event_id + 1`. Value 0 = never run.
    let next_wanted_id = cursors::stability_events::get();

    let count = match sources::stability_pool::get_pool_event_count(sp).await {
        Ok(c) => c,
        Err(e) => {
            ic_cdk::println!("[tail_sp] get_pool_event_count failed: {}", e);
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

    if count == 0 {
        return;
    }

    // Always fetch from the tail of the current window (at most BATCH_SIZE
    // events). If new events arrived faster than one tick can process, the
    // oldest ones may have already been rotated out — acceptable given the
    // 60-second tick interval and a 10k ring buffer.
    let start = count.saturating_sub(BATCH_SIZE);
    let length = count - start;
    let events = match sources::stability_pool::get_pool_events(sp, start, length).await {
        Ok(e) => e,
        Err(e) => {
            ic_cdk::println!("[tail_sp] get_pool_events failed: {}", e);
            state::mutate_state(|s| {
                s.error_counters.stability_pool += 1;
                update_cursor_error(s, cursors::CURSOR_ID_STABILITY_EVENTS, e);
            });
            return;
        }
    };

    // highest_id_seen starts one below next_wanted_id so that after the loop
    // `highest_id_seen + 1` equals the correct new cursor value even when
    // processed == 0.
    let mut highest_id_seen = next_wanted_id.saturating_sub(1);
    let mut processed = 0u64;
    for event in &events {
        if event.id < next_wanted_id {
            continue; // already ingested
        }
        route_sp_event(event);
        if event.id > highest_id_seen {
            highest_id_seen = event.id;
        }
        processed += 1;
    }

    if processed > 0 {
        // Save `highest_id_seen + 1` so next tick filters out already-seen ids.
        cursors::stability_events::set(highest_id_seen + 1);
        state::mutate_state(|s| {
            update_cursor_success(s, cursors::CURSOR_ID_STABILITY_EVENTS, ic_cdk::api::time());
        });
    }
}

fn route_sp_event(event: &sources::stability_pool::PoolEvent) {
    use sources::stability_pool::PoolEventType::*;

    match &event.event_type {
        Deposit { amount, .. } => {
            evt_stability::push(AnalyticsStabilityEvent {
                timestamp_ns: event.timestamp,
                source_event_id: event.id,
                caller: event.caller,
                action: StabilityAction::Deposit,
                amount: *amount,
            });
        }
        DepositAs3USD { amount_in, .. } => {
            evt_stability::push(AnalyticsStabilityEvent {
                timestamp_ns: event.timestamp,
                source_event_id: event.id,
                caller: event.caller,
                action: StabilityAction::Deposit,
                amount: *amount_in,
            });
        }
        Withdraw { amount, .. } => {
            evt_stability::push(AnalyticsStabilityEvent {
                timestamp_ns: event.timestamp,
                source_event_id: event.id,
                caller: event.caller,
                action: StabilityAction::Withdraw,
                amount: *amount,
            });
        }
        ClaimCollateral { amount, .. } => {
            evt_stability::push(AnalyticsStabilityEvent {
                timestamp_ns: event.timestamp,
                source_event_id: event.id,
                caller: event.caller,
                action: StabilityAction::ClaimReturns,
                amount: *amount,
            });
        }
        // Everything below is decoded so Vec<PoolEvent> deserialization
        // succeeds, but skipped — none of these affect the depositor
        // leaderboard. Listed exhaustively (rather than `_ => {}`) so adding
        // a new variant on the SP side without updating analytics produces a
        // compile error here, not a silent drop.
        InterestReceived { .. } => {}
        OptOutCollateral { .. } => {}
        OptInCollateral { .. } => {}
        LiquidationNotification { .. } => {}
        LiquidationExecuted { .. } => {}
        StablecoinRegistered { .. } => {}
        CollateralRegistered { .. } => {}
        ConfigurationUpdated => {}
        EmergencyPauseActivated => {}
        OperationsResumed => {}
        BalanceCorrected { .. } => {}
        CollateralGainCorrected { .. } => {}
    }
}
