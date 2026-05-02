//! Wave-9a DOS-008: stability-pool `get_pool_events` must clamp the
//! caller-supplied `length` argument before slicing the pool-event log,
//! so a single query call's reply size and per-call slice cost stay
//! bounded regardless of caller input.
//!
//! Audit report:
//!   * `audit-reports/2026-04-22-28e9896/findings.json` finding DOS-008
//!     ("get_pool_events on stability pool accepts unbounded length
//!     argument").
//!
//! # What the bug was
//!
//! Pre-fix `get_pool_events(start, length)` did
//!
//!   ```text
//!   let end = (start + length).min(total) as usize;
//!   events[start as usize..end].to_vec()
//!   ```
//!
//! with no upper cap on `length`. A caller could pass
//! `length = u64::MAX` and force the canister to slice up to the full
//! pool-event log into a single reply, paying the per-element cycle and
//! reply-size cost in one shot. As the SP's `pool_events` log grows over
//! time (capped at `MAX_POOL_EVENTS` ~ 10k internally), the slice would
//! eventually approach the IC's 2 MB reply limit and consume hundreds of
//! millions of instructions on a single query.
//!
//! # How this file tests the fix
//!
//! The slicing logic was extracted from the `#[query]` wrapper into the
//! pure `pool_events_page(events, start, length)` helper so it can be
//! driven directly without standing up a canister fixture. Two
//! behavioural fences plus one numeric fence:
//!
//!   * `dos_008_get_pool_events_clamps_unbounded_length_arg` — the
//!     load-bearing audit fence. Build a 600-event log; call
//!     `pool_events_page(events, 0, u64::MAX)`. Assert the reply length
//!     is exactly `MAX_POOL_EVENTS_PAGE` (500) — i.e., the clamp ran.
//!     Pre-fix code would have returned 600 events (the full log).
//!
//!   * `dos_008_pool_events_page_respects_natural_total_under_cap` —
//!     regression fence. Build a 50-event log (well under the cap);
//!     ask for `length = 1000`. The natural `total` should win, so the
//!     reply is 50 events. Without this case the previous fence cannot
//!     distinguish "clamp fired" from "log was empty".
//!
//!   * `dos_008_pool_events_page_cursor_round_trip` — page-correctness
//!     fence. Walk a 1100-event log in three calls of 500 + 500 + 100
//!     and assert (a) the IDs reassemble in order with no gaps or
//!     duplicates, (b) each call's reply is bounded by
//!     `MAX_POOL_EVENTS_PAGE`, (c) the third call sees only the
//!     remainder of the log.

use candid::Principal;

use stability_pool::types::{PoolEvent, PoolEventType};
use stability_pool::{pool_events_page, MAX_POOL_EVENTS_PAGE};

/// Build a synthetic pool-event log of the requested length. Each event
/// gets a unique `id` so the cursor-round-trip fence can verify slice
/// boundaries by inspecting reassembled ids.
fn synth_events(n: u64) -> Vec<PoolEvent> {
    (0..n)
        .map(|i| PoolEvent {
            id: i,
            timestamp: i,
            caller: Principal::anonymous(),
            event_type: PoolEventType::OptInCollateral {
                collateral_type: Principal::anonymous(),
            },
        })
        .collect()
}

#[test]
fn dos_008_get_pool_events_clamps_unbounded_length_arg() {
    // 600-event log so the post-clamp reply (500) is strictly smaller
    // than the pre-fix unbounded reply would have been (600). If the
    // clamp regresses, the reply will exceed MAX_POOL_EVENTS_PAGE.
    let events = synth_events(600);

    let page = pool_events_page(&events, 0, u64::MAX);

    assert_eq!(
        page.len() as u64,
        MAX_POOL_EVENTS_PAGE,
        "DOS-008: caller-supplied length must be clamped to MAX_POOL_EVENTS_PAGE \
         before slicing — got reply of {} events for a {}-event log with length=u64::MAX",
        page.len(),
        events.len(),
    );
    // Cap is the lower of (length, total). Confirm the slice boundary
    // matches: ids 0..MAX_POOL_EVENTS_PAGE.
    assert_eq!(page.first().map(|e| e.id), Some(0));
    assert_eq!(
        page.last().map(|e| e.id),
        Some(MAX_POOL_EVENTS_PAGE - 1),
        "first page must end at id=MAX_POOL_EVENTS_PAGE-1, not before",
    );
}

#[test]
fn dos_008_pool_events_page_respects_natural_total_under_cap() {
    // Regression fence: when total < MAX_POOL_EVENTS_PAGE the natural
    // `total` should still bound the reply. This rules out a "broken
    // clamp returns MAX_POOL_EVENTS_PAGE elements regardless" failure
    // mode where the implementation hard-pads the response.
    let events = synth_events(50);

    let page = pool_events_page(&events, 0, 1_000);

    assert_eq!(
        page.len(),
        50,
        "natural total < cap: reply must be the full log, not pad to cap",
    );
    assert_eq!(page.first().map(|e| e.id), Some(0));
    assert_eq!(page.last().map(|e| e.id), Some(49));
}

#[test]
fn dos_008_pool_events_page_cursor_round_trip() {
    // 1100-event log: walk it in three calls and assert the cap fires
    // on each call until the tail is reached. Concretely: the first two
    // calls should each return MAX_POOL_EVENTS_PAGE (500) events, the
    // third should return the remainder (100) and an empty fourth call
    // should mean the cursor reached the end.
    const TOTAL: u64 = 1_100;
    let events = synth_events(TOTAL);

    let mut cursor: u64 = 0;
    let mut collected: Vec<u64> = Vec::with_capacity(TOTAL as usize);
    let mut call_count: u32 = 0;
    while cursor < TOTAL {
        let page = pool_events_page(&events, cursor, u64::MAX);
        assert!(
            page.len() as u64 <= MAX_POOL_EVENTS_PAGE,
            "DOS-008: every call's reply must be bounded by MAX_POOL_EVENTS_PAGE; \
             cursor={} got {} events",
            cursor,
            page.len(),
        );
        // No empty page allowed mid-walk — that would loop forever.
        assert!(
            !page.is_empty(),
            "non-empty log mid-walk must return at least one event \
             (cursor={}, total={})",
            cursor,
            TOTAL,
        );
        for ev in &page {
            collected.push(ev.id);
        }
        cursor += page.len() as u64;
        call_count += 1;
    }

    assert_eq!(
        call_count, 3,
        "1100-event log at 500-per-call ceiling should take exactly 3 calls",
    );
    assert_eq!(
        collected.len() as u64,
        TOTAL,
        "cursor walk must collect every event (no gaps, no duplicates)",
    );
    // ids reassemble in order: this catches off-by-one on the boundary
    // between calls (skipping the seam event, or returning it twice).
    assert!(
        collected.iter().enumerate().all(|(i, id)| *id == i as u64),
        "cursor walk must yield ids 0..TOTAL in order without gaps or duplicates",
    );

    // Past the end: an extra call with cursor == TOTAL should be empty.
    let past_end = pool_events_page(&events, TOTAL, u64::MAX);
    assert!(past_end.is_empty(), "calling past total must return empty page");
}
