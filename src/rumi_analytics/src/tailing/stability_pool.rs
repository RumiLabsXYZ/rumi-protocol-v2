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
//! produces more than BATCH_SIZE events between pulls the oldest of those
//! would be missed, but that rate (>500 SP events/5 min at the default
//! cadence) is unrealistic.

use super::{update_cursor_error, update_cursor_source_count, update_cursor_success, BATCH_SIZE};
use crate::{sources, state, storage};
use storage::cursors;
use storage::events::*;

fn record_sp_error(error: String) {
    state::mutate_state(|state| {
        state.error_counters.stability_pool += 1;
        update_cursor_error(state, cursors::CURSOR_ID_STABILITY_EVENTS, error);
    });
}

fn probe_requires_tail(
    next_wanted_id: u64,
    newest: Option<&sources::stability_pool::PoolEvent>,
) -> bool {
    newest.is_some_and(|event| event.id >= next_wanted_id)
}

fn select_unseen_events<'a>(
    next_wanted_id: u64,
    events: &'a [sources::stability_pool::PoolEvent],
) -> (Vec<&'a sources::stability_pool::PoolEvent>, Option<u64>) {
    let unseen: Vec<_> = events
        .iter()
        .filter(|event| event.id >= next_wanted_id)
        .collect();
    let next_cursor = unseen
        .iter()
        .map(|event| event.id)
        .max()
        .map(|id| id.saturating_add(1));
    (unseen, next_cursor)
}

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
            record_sp_error(e);
            return;
        }
    };

    state::mutate_state(|s| {
        update_cursor_source_count(s, cursors::CURSOR_ID_STABILITY_EVENTS, count);
    });

    if count == 0 {
        return;
    }

    // Probe-before-fetch: SP events (deposit/withdraw/claim) are rare, so on
    // almost every tick there is nothing new. Fetch just the newest event
    // (position count-1, length 1) and compare its monotonic id against the
    // cursor. If the newest id is already ingested, skip the full BATCH_SIZE
    // pull — which otherwise ships up to 500 events every tick even when idle.
    let newest =
        match sources::stability_pool::get_pool_events(sp, count.saturating_sub(1), 1).await {
            Ok(e) => e,
            Err(e) => {
                ic_cdk::println!("[tail_sp] probe get_pool_events failed: {}", e);
                record_sp_error(e);
                return;
            }
        };
    if !probe_requires_tail(next_wanted_id, newest.first()) {
        // Newest event already ingested, or the log is momentarily empty:
        // nothing to do this tick.
        return;
    }

    // Fetch from the tail of the current window (at most BATCH_SIZE events).
    // If new events arrived faster than one tick can process, the oldest ones
    // may have already been rotated out — acceptable given the tick interval
    // and a 10k ring buffer.
    let start = count.saturating_sub(BATCH_SIZE);
    let length = count - start;
    let events = match sources::stability_pool::get_pool_events(sp, start, length).await {
        Ok(e) => e,
        Err(e) => {
            ic_cdk::println!("[tail_sp] get_pool_events failed: {}", e);
            record_sp_error(e);
            return;
        }
    };

    let (unseen, next_cursor) = select_unseen_events(next_wanted_id, &events);
    for event in unseen {
        route_sp_event(event);
    }

    if let Some(next_cursor) = next_cursor {
        // Save highest unseen id + 1 so the next tick filters out seen ids.
        cursors::stability_events::set(next_cursor);
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

#[cfg(test)]
mod tests {
    use super::*;
    use candid::Principal;
    use sources::stability_pool::{PoolEvent, PoolEventType};

    fn event(id: u64) -> PoolEvent {
        PoolEvent {
            id,
            timestamp: id,
            caller: Principal::anonymous(),
            event_type: PoolEventType::ConfigurationUpdated,
        }
    }

    #[test]
    fn idle_probe_skips_tail_fetch_and_new_probe_requests_it() {
        assert!(!probe_requires_tail(11, Some(&event(10))));
        assert!(!probe_requires_tail(11, None));
        assert!(probe_requires_tail(11, Some(&event(11))));
        assert!(probe_requires_tail(11, Some(&event(12))));
    }

    #[test]
    fn ring_rotation_filters_seen_ids_and_advances_without_duplicates() {
        let first_window: Vec<PoolEvent> = (98..=102).map(event).collect();
        let (first_unseen, next) = select_unseen_events(101, &first_window);
        assert_eq!(
            first_unseen
                .iter()
                .map(|event| event.id)
                .collect::<Vec<_>>(),
            vec![101, 102]
        );
        assert_eq!(next, Some(103));

        let rotated_window: Vec<PoolEvent> = (100..=104).map(event).collect();
        let (second_unseen, next) = select_unseen_events(next.unwrap(), &rotated_window);
        assert_eq!(
            second_unseen
                .iter()
                .map(|event| event.id)
                .collect::<Vec<_>>(),
            vec![103, 104]
        );
        assert_eq!(next, Some(105));
    }

    #[test]
    fn query_failure_records_error_without_advancing_cursor() {
        state::replace_state(storage::SlimState::default());
        let cursor_before = cursors::stability_events::get();

        record_sp_error("probe failed".to_string());

        let (error_count, last_error) = state::read_state(|state| {
            (
                state.error_counters.stability_pool,
                state
                    .cursor_last_error
                    .as_ref()
                    .and_then(|errors| errors.get(&cursors::CURSOR_ID_STABILITY_EVENTS))
                    .cloned(),
            )
        });
        assert_eq!(error_count, 1);
        assert_eq!(last_error.as_deref(), Some("probe failed"));
        assert_eq!(cursors::stability_events::get(), cursor_before);
    }
}
