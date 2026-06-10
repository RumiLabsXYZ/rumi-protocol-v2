//! Inter-canister event polling (Phase 2). Pull-based: each source is polled with
//! a forward cursor, and the decoded events are normalized and applied
//! (auto-registering principals on their first qualifying action).
//!
//! The network calls are NOT unit-testable (validated by the PocketIC E2E), but
//! the windowing/filter/ingest core is pure and tested below. Every call result
//! is handled (no `unwrap`/trap); the single-poll `state::PollGuard` is RAII
//! (AR-S-001), so even a trap releases it. One source failing is logged and
//! skipped without advancing its cursor, so it does not block the others.
//!
//! Cursor model (see `events.rs`): every persisted cursor is ID-based (the next
//! event id we want).
//!   - backend, 3pool: forward endpoints that return an explicit `next_start`.
//!   - SP, AMM: oldest-first endpoints indexed by ARRAY POSITION over logs that
//!     trim their oldest entries (SP drains at MAX_POOL_EVENTS=10_000, AMM at
//!     MAX_LIQUIDITY_EVENTS=50_000). Once a log rotates, id != index, so the
//!     poller must NOT pass its id cursor as the position. Instead it probes the
//!     window's first event to learn the id/index offset, fetches by position
//!     (`plan_window`), filters in-batch by `event_id >= cursor`, and persists
//!     `max(id)+1` via `ingest_batch` (PTS-001; same id-cursor semantics as
//!     before, so no state migration). Pattern proven in
//!     rumi_analytics/src/tailing/stability_pool.rs.
//!
//! There is intentionally NO periodic timer here (which would burn cycles every
//! tick). Phase 2 exposes an admin `trigger_poll`; the production timer cadence is
//! a small follow-up (`setup_timers`).

use std::cell::RefCell;
use std::time::Duration;

use candid::Principal;
use ic_cdk::api::call::RejectionCode;
use ic_cdk_timers::TimerId;

use crate::events::{self, IngestedEvent, SourceId};
use crate::source_types::{amm, backend, stability_pool, three_pool};
use crate::state;

thread_local! {
    /// The live poll-timer id (transient; timers do not survive upgrades).
    static POLL_TIMER: RefCell<Option<TimerId>> = RefCell::new(None);
}

/// (Re)register the periodic poll timer from the persisted config (Phase 2b).
/// Cancels any existing timer first. Called from `init` / `post_upgrade` (timers
/// must be re-registered after an upgrade) and whenever the admin changes the
/// poll config. Off by default: registers nothing until an operator enables it,
/// so a fresh canister burns no cycles polling.
pub fn setup_poll_timer() {
    POLL_TIMER.with(|t| {
        if let Some(id) = t.borrow_mut().take() {
            ic_cdk_timers::clear_timer(id);
        }
    });
    if state::poll_enabled() {
        let interval = Duration::from_secs(state::poll_interval_secs());
        let id = ic_cdk_timers::set_timer_interval(interval, || {
            // poll_all is async; spawn it. Its single-poll guard handles the case
            // where a tick fires while the previous poll is still in flight.
            ic_cdk::spawn(async {
                let _ = poll_all().await;
            });
        });
        POLL_TIMER.with(|t| *t.borrow_mut() = Some(id));
    }
}

// Per-call scan/batch bounds (kept under the source endpoints' own caps).
const BACKEND_SCAN: u64 = 500;
const THREE_POOL_SCAN: u64 = 500;
const SP_BATCH: u64 = 500;
const AMM_BATCH: u64 = 200;

type CallResult<T> = Result<T, (RejectionCode, String)>;

/// Poll every configured source once, under the single-poll guard. Returns the
/// number of events applied across all sources.
pub async fn poll_all() -> usize {
    // RAII guard (AR-S-001): released on every exit path INCLUDING a trap (the
    // dropped future runs destructors), so a panicking poll never wedges polling.
    let _guard = match state::PollGuard::new() {
        Some(g) => g,
        None => return 0, // a poll is already in flight
    };
    let mut applied = 0;
    applied += poll_backend().await;
    applied += poll_three_pool().await;
    applied += poll_stability_pool().await;
    applied += poll_amm().await;
    applied
}

fn source_canister(source: SourceId) -> Option<Principal> {
    state::get_source_canister(source.tag())
}

async fn poll_backend() -> usize {
    let canister = match source_canister(SourceId::Backend) {
        Some(c) => c,
        None => return 0,
    };
    let tag = SourceId::Backend.tag();
    let cursor = state::get_cursor(tag);
    let res: CallResult<(backend::ForwardFilteredEventsResponse,)> = ic_cdk::call(
        canister,
        "get_events_forward_filtered",
        (cursor, BACKEND_SCAN, Some(backend::points_event_filter())),
    )
    .await;
    match res {
        Ok((resp,)) => {
            let (events, next_start, _reached_end) = backend::normalize_forward(resp);
            let n = events.len();
            events::apply_events(&events);
            state::set_cursor(tag, next_start);
            n
        }
        Err((code, msg)) => {
            ic_cdk::println!("[poll] backend get_events_forward_filtered failed: {:?} {}", code, msg);
            0
        }
    }
}

async fn poll_three_pool() -> usize {
    let canister = match source_canister(SourceId::ThreePool) {
        Some(c) => c,
        None => return 0,
    };
    let tag = SourceId::ThreePool.tag();
    let cursor = state::get_cursor(tag);
    let res: CallResult<(three_pool::ForwardLiquidityEventsV2,)> =
        ic_cdk::call(canister, "get_liquidity_events_v2_forward", (cursor, THREE_POOL_SCAN)).await;
    match res {
        Ok((resp,)) => {
            let (events, next_start, _reached_end) = three_pool::normalize_forward(resp);
            let n = events.len();
            events::apply_events(&events);
            state::set_cursor(tag, next_start);
            n
        }
        Err((code, msg)) => {
            ic_cdk::println!("[poll] 3pool get_liquidity_events_v2_forward failed: {:?} {}", code, msg);
            0
        }
    }
}

// ── PTS-001: rotation-safe windowing over position-indexed source logs ──────
// The SP/AMM `get_*_events(start, length)` endpoints slice a Vec by ARRAY
// POSITION, and both logs trim their oldest entries when full. Event ids are
// assigned from a monotonic counter, so a trimmed log holds the CONTIGUOUS id
// range [first_id, first_id + count). Knowing the first id in the window maps
// the id cursor to its position exactly; before rotation first_id == 0 and the
// position equals the cursor (the pre-fix behavior, no double-ingest).

/// `(start_position, length)` of the fetch that resumes at id `cursor`, given the
/// window's current `count` and the id of its first (oldest) retained event.
/// `None` when there is nothing new to fetch. If the cursor's id was already
/// trimmed away (`cursor < first_id`), starts at position 0: those events are
/// unrecoverable and waiting cannot bring them back.
pub fn plan_window(cursor: u64, count: u64, first_id_in_window: u64, batch: u64) -> Option<(u64, u64)> {
    if count == 0 {
        return None;
    }
    let start = cursor.saturating_sub(first_id_in_window);
    if start >= count {
        return None;
    }
    Some((start, batch.min(count - start)))
}

/// Filter a position-fetched window down to the events at/after the id cursor and
/// ingest them. The filter makes a re-fetch of an already-seen range a no-op
/// (`ingest_batch` then leaves the cursor unchanged on an empty batch), so a
/// stale `count`/probe race can never double-ingest. Returns the number applied.
fn ingest_window(source: SourceId, window: Vec<IngestedEvent>, cursor: u64) -> usize {
    let events: Vec<_> = window.into_iter().filter(|e| e.event_id >= cursor).collect();
    events::ingest_batch(source, &events)
}

async fn poll_stability_pool() -> usize {
    let canister = match source_canister(SourceId::StabilityPool) {
        Some(c) => c,
        None => return 0,
    };
    let cursor = state::get_cursor(SourceId::StabilityPool.tag());
    let count: u64 = match ic_cdk::call::<_, (u64,)>(canister, "get_pool_event_count", ()).await {
        Ok((c,)) => c,
        Err((code, msg)) => {
            ic_cdk::println!("[poll] SP get_pool_event_count failed: {:?} {}", code, msg);
            return 0;
        }
    };
    // Probe the oldest retained event to learn the id/index offset (PTS-001).
    let probe: CallResult<(Vec<stability_pool::PoolEvent>,)> =
        ic_cdk::call(canister, "get_pool_events", (0u64, 1u64)).await;
    let first_id = match probe {
        Ok((evs,)) => match evs.first() {
            Some(e) => e.id,
            None => return 0, // emptied between the two calls
        },
        Err((code, msg)) => {
            ic_cdk::println!("[poll] SP get_pool_events probe failed: {:?} {}", code, msg);
            return 0;
        }
    };
    let (start, length) = match plan_window(cursor, count, first_id, SP_BATCH) {
        Some(w) => w,
        None => return 0,
    };
    let res: CallResult<(Vec<stability_pool::PoolEvent>,)> =
        ic_cdk::call(canister, "get_pool_events", (start, length)).await;
    match res {
        Ok((raw,)) => {
            let events: Vec<_> = raw.into_iter().map(stability_pool::normalize).collect();
            ingest_window(SourceId::StabilityPool, events, cursor)
        }
        Err((code, msg)) => {
            ic_cdk::println!("[poll] SP get_pool_events failed: {:?} {}", code, msg);
            0
        }
    }
}

async fn poll_amm() -> usize {
    let canister = match source_canister(SourceId::Amm) {
        Some(c) => c,
        None => return 0,
    };
    let cursor = state::get_cursor(SourceId::Amm.tag());
    let count: u64 =
        match ic_cdk::call::<_, (u64,)>(canister, "get_amm_liquidity_event_count", ()).await {
            Ok((c,)) => c,
            Err((code, msg)) => {
                ic_cdk::println!("[poll] AMM get_amm_liquidity_event_count failed: {:?} {}", code, msg);
                return 0;
            }
        };
    let probe: CallResult<(Vec<amm::AmmLiquidityEvent>,)> =
        ic_cdk::call(canister, "get_amm_liquidity_events", (0u64, 1u64)).await;
    let first_id = match probe {
        Ok((evs,)) => match evs.first() {
            Some(e) => e.id,
            None => return 0,
        },
        Err((code, msg)) => {
            ic_cdk::println!("[poll] AMM get_amm_liquidity_events probe failed: {:?} {}", code, msg);
            return 0;
        }
    };
    let (start, length) = match plan_window(cursor, count, first_id, AMM_BATCH) {
        Some(w) => w,
        None => return 0,
    };
    let res: CallResult<(Vec<amm::AmmLiquidityEvent>,)> =
        ic_cdk::call(canister, "get_amm_liquidity_events", (start, length)).await;
    match res {
        Ok((raw,)) => {
            let events: Vec<_> = raw.into_iter().map(amm::normalize).collect();
            ingest_window(SourceId::Amm, events, cursor)
        }
        Err((code, msg)) => {
            ic_cdk::println!("[poll] AMM get_amm_liquidity_events failed: {:?} {}", code, msg);
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::IngestKind;
    use crate::types::InitArgs;
    use candid::Principal;

    fn tp(n: u8) -> Principal {
        Principal::from_slice(&[n, n, n, n, n])
    }

    fn init() {
        state::init_state(
            Some(InitArgs { admin: Some(tp(99)), ..Default::default() }),
            Principal::anonymous(),
        );
    }

    fn in_season_ts() -> u64 {
        crate::DEFAULT_SEASON_START_NS + 1_000
    }

    /// A simulated SP/AMM event log: position-indexed slices over a window whose
    /// event ids start at `first_id` (ids != indices once `first_id > 0`, exactly
    /// what the real logs look like after their oldest entries are trimmed).
    struct SimLog {
        first_id: u64,
        window: Vec<IngestedEvent>,
    }

    impl SimLog {
        /// `len` SP-deposit events with contiguous ids `[first_id, first_id+len)`,
        /// each from a distinct principal so registrations are countable.
        fn sp(first_id: u64, len: u64) -> Self {
            let window = (0..len)
                .map(|i| IngestedEvent {
                    source: SourceId::StabilityPool,
                    event_id: first_id + i,
                    caller: Some(tp((first_id + i) as u8)),
                    timestamp_ns: in_season_ts(),
                    kind: IngestKind::SpDeposit { token_ledger: tp(200), amount_e8s: 100 },
                })
                .collect();
            SimLog { first_id, window }
        }

        fn count(&self) -> u64 {
            self.window.len() as u64
        }

        /// Mirrors the source endpoints: slice by ARRAY POSITION.
        fn fetch(&self, start: u64, length: u64) -> Vec<IngestedEvent> {
            let total = self.count();
            if start >= total {
                return Vec::new();
            }
            let end = (start + length).min(total) as usize;
            self.window[start as usize..end].to_vec()
        }
    }

    /// Drive one poll over the simulated log through the SAME post-fetch path the
    /// real pollers use (plan_window + position fetch + ingest_window).
    fn poll_sim(log: &SimLog, batch: u64) -> usize {
        let cursor = state::get_cursor(SourceId::StabilityPool.tag());
        let (start, length) = match plan_window(cursor, log.count(), log.first_id, batch) {
            Some(w) => w,
            None => return 0,
        };
        ingest_window(SourceId::StabilityPool, log.fetch(start, length), cursor)
    }

    #[test]
    fn pts_001_plan_window_maps_id_cursor_to_position() {
        // Not yet rotated (first_id == 0): position == cursor, pre-fix behavior.
        assert_eq!(plan_window(0, 10, 0, 500), Some((0, 10)));
        assert_eq!(plan_window(7, 10, 0, 500), Some((7, 3)));
        assert_eq!(plan_window(10, 10, 0, 500), None); // caught up
        // Rotated: ids [100, 150) at positions [0, 50).
        assert_eq!(plan_window(120, 50, 100, 500), Some((20, 30)));
        assert_eq!(plan_window(150, 50, 100, 500), None); // caught up
        // Cursor's id already trimmed away: resume at the window start.
        assert_eq!(plan_window(40, 50, 100, 500), Some((0, 50)));
        // Length is capped at the batch size.
        assert_eq!(plan_window(100, 50, 100, 20), Some((0, 20)));
        // Empty window.
        assert_eq!(plan_window(0, 0, 0, 500), None);
    }

    #[test]
    fn pts_001_rotated_source_continues_ingestion() {
        init();
        // The poller had ingested up to id 120 when the log rotated: the window now
        // holds ids [100, 150) at positions [0, 50). Pre-fix, passing 120 as the
        // POSITION returned nothing forever; the windowed poll must resume at the
        // 30 not-yet-seen events without re-ingesting [100, 120).
        state::set_cursor(SourceId::StabilityPool.tag(), 120);
        let log = SimLog::sp(100, 50);
        assert_eq!(poll_sim(&log, 500), 30);
        assert_eq!(state::get_cursor(SourceId::StabilityPool.tag()), 150);
        // Events 120..150 registered their principals; 100..120 did not re-apply.
        assert!(state::is_registered(&tp(120)));
        assert!(state::is_registered(&tp(149)));
        assert!(!state::is_registered(&tp(119)));
        // Caught up: the next poll over the same window is a no-op.
        assert_eq!(poll_sim(&log, 500), 0);
        assert_eq!(state::get_cursor(SourceId::StabilityPool.tag()), 150);
    }

    #[test]
    fn pts_001_not_yet_rotated_window_does_not_double_ingest() {
        init();
        // first_id == 0 (id == index): two polls over the same window must ingest
        // exactly once, and re-applying the overlap must not change any state
        // (ingest_batch/register idempotence).
        let log = SimLog::sp(0, 10);
        assert_eq!(poll_sim(&log, 500), 10);
        assert_eq!(state::get_cursor(SourceId::StabilityPool.tag()), 10);
        let before = state::get_principal_state(&tp(5)).unwrap();
        assert_eq!(poll_sim(&log, 500), 0); // nothing new
        assert_eq!(state::get_cursor(SourceId::StabilityPool.tag()), 10);
        assert_eq!(state::get_principal_state(&tp(5)).unwrap(), before);
        assert_eq!(state::registered_count(), 10);
    }

    #[test]
    fn pts_001_catchup_pages_forward_in_batches() {
        init();
        // A rotated window larger than one batch is consumed across several polls
        // (the cursor advances by max(id)+1 each time), so a long stall recovers
        // without skipping the middle of the window.
        state::set_cursor(SourceId::StabilityPool.tag(), 100);
        let log = SimLog::sp(100, 50);
        assert_eq!(poll_sim(&log, 20), 20);
        assert_eq!(state::get_cursor(SourceId::StabilityPool.tag()), 120);
        assert_eq!(poll_sim(&log, 20), 20);
        assert_eq!(poll_sim(&log, 20), 10);
        assert_eq!(state::get_cursor(SourceId::StabilityPool.tag()), 150);
        assert_eq!(state::registered_count(), 50);
        assert_eq!(poll_sim(&log, 20), 0);
    }

    #[test]
    fn pts_001_trimmed_past_cursor_resumes_at_window_start() {
        init();
        // The poller stalled so long that even its cursor id was trimmed away:
        // ids [100, 110) remain but the cursor still says 50. The lost events are
        // unrecoverable; the poller must ingest what is left, not stall.
        state::set_cursor(SourceId::StabilityPool.tag(), 50);
        let log = SimLog::sp(100, 10);
        assert_eq!(poll_sim(&log, 500), 10);
        assert_eq!(state::get_cursor(SourceId::StabilityPool.tag()), 110);
    }
}
