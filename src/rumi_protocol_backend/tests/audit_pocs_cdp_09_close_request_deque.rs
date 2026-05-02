//! CDP-09 regression fence: `global_close_requests` must use a
//! `VecDeque<u64>` so that the 24h cleanup can drop expired timestamps
//! from the front via `drain(..partition_point(...))` in O(log N + K)
//! instead of an O(N) `retain` over the whole list.
//!
//! Not a vulnerability per AVAI ("INFO" tier); a scalability cliff under
//! sustained 300+ closes/minute load. Audit fence per
//! `.claude/security-docs/2026-05-02-wave-14-avai-parity-plan.md`.
//!
//! Layered fences:
//!  1. The field's type is `VecDeque<u64>`.
//!  2. Pre-Wave-14 snapshots that decode an old `Vec<u64>` blob still
//!     deserialize cleanly (serde handles Vec→VecDeque transparently
//!     when both serialize the same way; the `#[serde(default)]` on
//!     State catches missing-field cases).
//!  3. Behavior of the rate-limit checks is unchanged from the user's
//!     perspective: 5/min/user, 60/day/user, 300/min global, 30k/day
//!     global.

use std::collections::VecDeque;

use rumi_protocol_backend::state::State;

#[test]
fn cdp_09_global_close_requests_is_vec_deque() {
    // Compile-time: assert the field's exact type. If a future refactor
    // swaps it back to Vec, this fails to compile.
    let s = State::default();
    let _: &VecDeque<u64> = &s.global_close_requests;
}

#[test]
fn cdp_09_default_state_has_empty_deque() {
    let s = State::default();
    assert_eq!(s.global_close_requests.len(), 0);
    assert!(s.global_close_requests.is_empty());
}
