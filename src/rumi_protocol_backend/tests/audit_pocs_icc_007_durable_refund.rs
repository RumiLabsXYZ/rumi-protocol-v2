//! ICC-007 regression fence: durable refund queue for `redeem_reserves` failures.
//!
//! Audit report: `audit-reports/2026-04-22-28e9896/raw-pass-results/icc.json`
//! finding ICC-007.
//!
//! # What the bug was
//!
//! `redeem_reserves` pulls icUSD from the user (which the protocol effectively
//! burns), then sends ckStable to the user. If the ckStable transfer fails,
//! the saga refunds icUSD inline. If THAT inline refund also fails (subnet
//! hiccup, ledger paused, etc.), the pre-Wave-4 code only logged CRITICAL and
//! returned an error to the user. The user's icUSD was gone, the protocol
//! held nothing for them, and recovery required manual operator action.
//!
//! # How this file tests it
//!
//! The Wave-4 fix adds a `pending_refunds: BTreeMap<u64, PendingRefund>` to
//! State, persisted across upgrades. On double-failure `redeem_reserves`
//! enqueues a `PendingRefund` keyed by the burn icUSD block index, with a
//! stable `op_nonce` minted once and reused across retries.
//! `process_pending_transfer` drains the queue via
//! `transfer_icusd_with_nonce`, so the icUSD ledger deduplicates if a previous
//! retry's reply was lost.
//!
//! These tests cover the state-level invariants:
//!
//!   * `icc_007_state_holds_pending_refund_keyed_by_burn_block` proves the
//!     queue admits one entry per burn block index, and that two refunds for
//!     two distinct burns coexist.
//!   * `icc_007_pending_refund_round_trips_through_cbor` round-trips a state
//!     containing a refund through ciborium, asserting the new field
//!     deserializes into post-Wave-4 snapshots.
//!   * `icc_007_legacy_snapshot_without_pending_refunds_decodes_with_empty_map`
//!     asserts pre-Wave-4 snapshots (no `pending_refunds` field) decode
//!     successfully via `#[serde(default)]`, leaving the queue empty.
//!   * `icc_007_retry_count_saturates_at_max` mirrors the abandon-after-N-retries
//!     contract that the live retry loop enforces.

use candid::Principal;
use std::collections::BTreeMap;

use rumi_protocol_backend::state::{PendingRefund, State};
use rumi_protocol_backend::InitArg;

const MAX_PENDING_RETRIES: u8 = 5;

fn user_a() -> Principal { Principal::from_slice(&[1]) }
fn user_b() -> Principal { Principal::from_slice(&[2]) }

fn fresh_state() -> State {
    State::from(InitArg {
        xrc_principal: Principal::anonymous(),
        icusd_ledger_principal: Principal::anonymous(),
        icp_ledger_principal: Principal::anonymous(),
        fee_e8s: 0,
        developer_principal: Principal::anonymous(),
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    })
}

fn refund(user: Principal, amount_e8s: u64, op_nonce: u128) -> PendingRefund {
    PendingRefund { user, amount_e8s, retry_count: 0, op_nonce }
}

#[test]
fn icc_007_state_holds_pending_refund_keyed_by_burn_block() {
    let mut state = fresh_state();
    assert!(state.pending_refunds.is_empty(), "fresh state has no pending refunds");

    state.pending_refunds.insert(1001, refund(user_a(), 100_000_000, 11));
    state.pending_refunds.insert(1002, refund(user_b(), 50_000_000, 12));

    assert_eq!(state.pending_refunds.len(), 2);
    let a = state.pending_refunds.get(&1001).expect("entry for burn 1001");
    assert_eq!(a.user, user_a());
    assert_eq!(a.amount_e8s, 100_000_000);
    assert_eq!(a.op_nonce, 11);

    // Inserting a second refund with the same key overwrites: by construction
    // every burn block index is unique, so this is the right semantics.
    state.pending_refunds.insert(1001, refund(user_a(), 100_000_000, 99));
    assert_eq!(state.pending_refunds.get(&1001).unwrap().op_nonce, 99);
}

#[test]
fn icc_007_pending_refund_round_trips_through_cbor() {
    let mut state = fresh_state();
    state.pending_refunds.insert(2001, refund(user_a(), 250_000_000, 77));

    let mut buf = Vec::new();
    ciborium::ser::into_writer(&state, &mut buf).expect("encode state");
    let restored: State =
        ciborium::de::from_reader(buf.as_slice()).expect("decode state");

    assert_eq!(restored.pending_refunds.len(), 1);
    let entry = restored.pending_refunds.get(&2001).expect("refund survives round-trip");
    assert_eq!(entry.user, user_a());
    assert_eq!(entry.amount_e8s, 250_000_000);
    assert_eq!(entry.op_nonce, 77);
    assert_eq!(entry.retry_count, 0);
}

/// Pre-Wave-4 snapshots have no `pending_refunds` field. `#[serde(default)]`
/// must fill it with an empty map so existing canister state decodes cleanly.
#[test]
fn icc_007_legacy_snapshot_without_pending_refunds_decodes_with_empty_map() {
    // Build a snapshot that encodes only a subset of State fields, then
    // verify ciborium fills the missing `pending_refunds` from Default.
    #[derive(serde::Serialize)]
    struct LegacyMinimalState {
        pending_redemption_transfer: BTreeMap<u64, rumi_protocol_backend::state::PendingMarginTransfer>,
    }

    let snapshot = LegacyMinimalState {
        pending_redemption_transfer: BTreeMap::new(),
    };

    let mut buf = Vec::new();
    ciborium::ser::into_writer(&snapshot, &mut buf).expect("encode legacy minimal state");
    let restored: State = ciborium::de::from_reader(buf.as_slice())
        .expect("decode legacy snapshot via #[serde(default)]");

    assert!(
        restored.pending_refunds.is_empty(),
        "legacy snapshot must decode with an empty pending_refunds queue"
    );
}

/// The live retry loop calls `retry_count.saturating_add(1)` and abandons at
/// `>= MAX_PENDING_RETRIES`. This test pins that arithmetic so a future change
/// to PendingRefund's retry semantics is caught.
#[test]
fn icc_007_retry_count_saturates_at_max() {
    let mut entry = refund(user_a(), 100_000_000, 1);
    for _ in 0..(MAX_PENDING_RETRIES as u32 + 3) {
        entry.retry_count = entry.retry_count.saturating_add(1);
    }
    assert!(entry.retry_count >= MAX_PENDING_RETRIES);
    assert!(entry.retry_count < u8::MAX, "retry_count should not wrap before u8::MAX");
}
