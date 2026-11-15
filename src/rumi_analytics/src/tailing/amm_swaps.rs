//! AMM swap event tailing. Writes to EVT_SWAPS.

use std::cell::Cell;

use crate::{sources, state, storage};
use storage::cursors;
use storage::events::*;
use super::{BATCH_SIZE, update_cursor_success, update_cursor_error, update_cursor_source_count};

thread_local! {
    static RUN_IN_PROGRESS: Cell<bool> = const { Cell::new(false) };
}

struct RunGuard;

impl RunGuard {
    fn try_acquire() -> Option<Self> {
        RUN_IN_PROGRESS.with(|in_progress| {
            if in_progress.replace(true) {
                None
            } else {
                Some(Self)
            }
        })
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        RUN_IN_PROGRESS.with(|in_progress| in_progress.set(false));
    }
}

fn nat_to_u64(n: &candid::Nat) -> u64 {
    use num_traits::ToPrimitive;
    n.0.to_u64().unwrap_or(u64::MAX)
}

fn select_unseen_events<'a>(
    next_wanted_id: u64,
    oldest_available_id: u64,
    events: &'a [sources::amm::AmmSwapEvent],
) -> (Vec<&'a sources::amm::AmmSwapEvent>, Option<u64>, bool) {
    let gap = next_wanted_id < oldest_available_id;
    let first_wanted_id = next_wanted_id.max(oldest_available_id);
    let unseen: Vec<_> = events
        .iter()
        .filter(|event| event.id >= first_wanted_id)
        .collect();
    let next_cursor = unseen.last().map(|event| event.id.saturating_add(1));
    (unseen, next_cursor, gap)
}

pub async fn run() {
    // The scheduler may start another tail pass while this one is awaiting the
    // source query. Keep the read/fetch/append/commit sequence single-flight.
    let Some(_run_guard) = RunGuard::try_acquire() else {
        return;
    };

    let amm = state::read_state(|s| s.sources.amm);
    let requested_id = cursors::amm_swaps::get();
    let window = match sources::amm::get_amm_swap_events_from_id(amm, requested_id, BATCH_SIZE).await {
        Ok(window) => window,
        Err(e) => {
            ic_cdk::println!("[tail_amm] get_amm_swap_events_from_id failed: {}", e);
            state::mutate_state(|s| {
                s.error_counters.amm = s.error_counters.amm.saturating_add(1);
                update_cursor_error(s, cursors::CURSOR_ID_AMM_SWAPS, e);
            });
            return;
        }
    };

    // Re-read after the await so a completed earlier pass can never make this
    // invocation append or commit a stale page.
    let current_id = cursors::amm_swaps::get();
    let (unseen, next_cursor, gap) =
        select_unseen_events(current_id, window.oldest_available_id, &window.events);

    state::mutate_state(|s| {
        update_cursor_source_count(s, cursors::CURSOR_ID_AMM_SWAPS, window.next_event_id);
        if gap {
            s.error_counters.amm = s.error_counters.amm.saturating_add(1);
            update_cursor_error(
                s,
                cursors::CURSOR_ID_AMM_SWAPS,
                format!(
                    "AMM swap history gap: next wanted event ID {} is older than retained ID {}; resuming at oldest retained event",
                    current_id, window.oldest_available_id
                ),
            );
        }
    });

    for evt in &unseen {
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
    }

    let cursor_after = next_cursor.unwrap_or_else(|| {
        if gap {
            window.oldest_available_id
        } else {
            current_id
        }
    });
    if cursor_after > cursors::amm_swaps::get() {
        cursors::amm_swaps::set(cursor_after);
    }

    if !unseen.is_empty() {
        state::mutate_state(|s| {
            update_cursor_success(s, cursors::CURSOR_ID_AMM_SWAPS, ic_cdk::api::time());
            // Preserve the dropped-history diagnostic until a gap-free pass.
            if gap {
                update_cursor_error(
                    s,
                    cursors::CURSOR_ID_AMM_SWAPS,
                    format!(
                        "AMM swap history gap: next wanted event ID {} is older than retained ID {}; resumed at oldest retained event",
                        current_id, window.oldest_available_id
                    ),
                );
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use candid::Principal;

    fn event(id: u64) -> sources::amm::AmmSwapEvent {
        sources::amm::AmmSwapEvent {
            id,
            caller: Principal::anonymous(),
            pool_id: "pool".to_string(),
            token_in: Principal::anonymous(),
            amount_in: candid::Nat::from(1u8),
            token_out: Principal::anonymous(),
            amount_out: candid::Nat::from(1u8),
            fee: candid::Nat::from(0u8),
            timestamp: id,
        }
    }

    #[test]
    fn overlapping_tail_runs_are_rejected_and_guard_releases() {
        let first = RunGuard::try_acquire().expect("first invocation acquires guard");
        assert!(RunGuard::try_acquire().is_none(), "overlapping invocation must skip");
        drop(first);
        assert!(RunGuard::try_acquire().is_some(), "guard releases after completion");
    }

    #[test]
    fn retrying_same_page_after_cursor_commit_is_idempotent() {
        let page = [event(50_000), event(50_001)];
        let (first, next, gap) = select_unseen_events(50_000, 1, &page);
        assert!(!gap);
        assert_eq!(first.iter().map(|event| event.id).collect::<Vec<_>>(), [50_000, 50_001]);
        let committed = next.expect("page advances cursor");

        let (retry, retry_next, retry_gap) = select_unseen_events(committed, 1, &page);
        assert!(retry.is_empty());
        assert_eq!(retry_next, None);
        assert!(!retry_gap);
    }

    #[test]
    fn retention_gap_resumes_at_oldest_id_and_reports_skipped_prefix() {
        let page = [event(50_001), event(50_002)];
        let (unseen, next, gap) = select_unseen_events(40_000, 50_001, &page);
        assert!(gap);
        assert_eq!(unseen.iter().map(|event| event.id).collect::<Vec<_>>(), [50_001, 50_002]);
        assert_eq!(next, Some(50_003));
    }
}
