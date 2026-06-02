//! Test-only mock of `rumi_protocol_backend.get_events_forward_filtered` for the
//! rumi_points ingestion E2E. Its response types match the backend `.did`
//! structurally, so rumi_points' real poll path (inter-canister call -> candid
//! decode -> normalize -> auto-register -> cursor advance) is exercised exactly as
//! it would be against the real backend. The 95-variant superset-decode case is
//! covered separately by the canary tests in `source_types.rs`.
//!
//! Behavior: the first forward window (start == 0) returns one synthetic
//! `borrow_from_vault` for `ryjl3-tyaaa-aaaaa-aaaba-cai` at an in-season
//! timestamp, with `next_start = 1, reached_end = true`. Subsequent windows are
//! empty (caught up).

use candid::{CandidType, Principal};
use serde::Deserialize;

/// The subset of the backend's `EventTypeFilter` that rumi_points sends. We only
/// need to DECODE it (the mock ignores the filter), and the values rumi_points
/// sends are all among these.
#[derive(CandidType, Deserialize, Clone)]
enum EventTypeFilter {
    OpenVault,
    CloseVault,
    Borrow,
    Repay,
    Liquidation,
    PartialLiquidation,
    Redemption,
    StabilityPoolDeposit,
    StabilityPoolWithdraw,
}

/// A faithful subset of the backend `Event` (snake_case candid labels via serde
/// rename, matching the real enum). Only the variants this mock emits.
#[derive(CandidType, Deserialize, Clone)]
enum Event {
    #[serde(rename = "borrow_from_vault")]
    BorrowFromVault {
        block_index: u64,
        vault_id: u64,
        timestamp: Option<u64>,
        fee_amount: u64,
        caller: Option<Principal>,
        borrowed_amount: u64,
    },
    #[serde(rename = "close_vault")]
    CloseVault {
        vault_id: u64,
        block_index: Option<u64>,
        timestamp: Option<u64>,
    },
}

#[derive(CandidType, Deserialize, Clone)]
struct ForwardFilteredEventsResponse {
    events: Vec<(u64, Event)>,
    next_start: u64,
    reached_end: bool,
}

/// Within the rumi_points default season window (June 1 .. Aug 31 2026).
const IN_SEASON_TS_NS: u64 = 1_780_300_000_000_000_000;

#[ic_cdk::query]
fn get_events_forward_filtered(
    start: u64,
    _max_scan: u64,
    _types: Option<Vec<EventTypeFilter>>,
) -> ForwardFilteredEventsResponse {
    if start == 0 {
        let caller = Principal::from_text("ryjl3-tyaaa-aaaaa-aaaba-cai").unwrap();
        ForwardFilteredEventsResponse {
            events: vec![(
                0,
                Event::BorrowFromVault {
                    block_index: 0,
                    vault_id: 1,
                    timestamp: Some(IN_SEASON_TS_NS),
                    fee_amount: 0,
                    caller: Some(caller),
                    borrowed_amount: 1_000,
                },
            )],
            next_start: 1,
            reached_end: true,
        }
    } else {
        ForwardFilteredEventsResponse {
            events: vec![],
            next_start: start,
            reached_end: true,
        }
    }
}
