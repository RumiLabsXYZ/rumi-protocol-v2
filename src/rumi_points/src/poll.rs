//! Inter-canister event polling (Phase 2). Pull-based: each source is polled with
//! a forward cursor, and the decoded events are normalized and applied
//! (auto-registering principals on their first qualifying action).
//!
//! NOT unit-testable (inter-canister calls); validated by the PocketIC E2E. Every
//! call result is handled (no `unwrap`/trap), so the single-poll guard always
//! releases: a trap would otherwise leave `POLL_IN_PROGRESS` stuck (canister traps
//! do not run Rust destructors). One source failing is logged and skipped without
//! advancing its cursor, so it does not block the others.
//!
//! Cursor model (see `events.rs`):
//!   - backend, 3pool: forward endpoints that return an explicit `next_start`.
//!   - SP, AMM: oldest-first index endpoints; advance by returned count via
//!     `ingest_batch` (valid while `id == index`, i.e. before their logs trim,
//!     far beyond Season-1 volume at current TVL).
//!
//! There is intentionally NO periodic timer here (which would burn cycles every
//! tick). Phase 2 exposes an admin `trigger_poll`; the production timer cadence is
//! a small follow-up (`setup_timers`).

use std::cell::RefCell;
use std::time::Duration;

use candid::Principal;
use ic_cdk::api::call::RejectionCode;
use ic_cdk_timers::TimerId;

use crate::events::{self, SourceId};
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
    if !state::try_begin_poll() {
        return 0; // a poll is already in flight
    }
    let mut applied = 0;
    applied += poll_backend().await;
    applied += poll_three_pool().await;
    applied += poll_stability_pool().await;
    applied += poll_amm().await;
    state::end_poll();
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

async fn poll_stability_pool() -> usize {
    let canister = match source_canister(SourceId::StabilityPool) {
        Some(c) => c,
        None => return 0,
    };
    let cursor = state::get_cursor(SourceId::StabilityPool.tag());
    let res: CallResult<(Vec<stability_pool::PoolEvent>,)> =
        ic_cdk::call(canister, "get_pool_events", (cursor, SP_BATCH)).await;
    match res {
        Ok((raw,)) => {
            let events: Vec<_> = raw.into_iter().map(stability_pool::normalize).collect();
            // Index endpoint: ingest_batch advances the cursor to max(id)+1 (== next
            // index while id==index). See the trim caveat in events.rs.
            events::ingest_batch(SourceId::StabilityPool, &events)
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
    let res: CallResult<(Vec<amm::AmmLiquidityEvent>,)> =
        ic_cdk::call(canister, "get_amm_liquidity_events", (cursor, AMM_BATCH)).await;
    match res {
        Ok((raw,)) => {
            let events: Vec<_> = raw.into_iter().map(amm::normalize).collect();
            events::ingest_batch(SourceId::Amm, &events)
        }
        Err((code, msg)) => {
            ic_cdk::println!("[poll] AMM get_amm_liquidity_events failed: {:?} {}", code, msg);
            0
        }
    }
}
