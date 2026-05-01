//! INT-002 regression fence: the async interest-distribution path must
//! recover its snapshot on mint failure without losing concurrent
//! accrual that landed during the await window.
//!
//! Audit report:
//!   * `audit-reports/2026-04-22-28e9896/raw-pass-results/debt-interest.json`
//!     finding INT-002.
//!
//! # What the bug was
//!
//! `treasury::flush_pending_interest` reads `pending_interest_for_pools`,
//! removes the bucket entry BEFORE awaiting `distribute_interest`, then
//! relies on the inner mints to succeed. Each inner mint
//! (`mint_interest_to_treasury`, `mint_interest_to_stability_pool`,
//! `donate_to_three_pool`) only logs on failure — there was no path back
//! to the bucket. A single transient mint failure permanently dropped
//! that tick's interest revenue.
//!
//! Naively restoring the original snapshot on failure would also lose
//! any concurrent harvest that landed in the bucket during the await
//! window: `BTreeMap::insert` would overwrite the concurrent value
//! rather than merging it.
//!
//! # How this file tests the fix
//!
//! The fix introduces a snapshot-then-decrement pair on `State`:
//!
//!   * `take_pending_interest_for_pool` — atomic snapshot + remove of a
//!     collateral bucket. Returns the e8s amount that was held.
//!   * `restore_pending_interest_for_pool` — saturating_add of an
//!     unminted amount back into the bucket. Used on the failure arm.
//!
//! Both tests construct a populated bucket, call `take_…` to capture
//! the snapshot (mirroring the `mutate_state` step before the await),
//! then directly mutate the bucket to simulate a concurrent harvest
//! landing during the await. The success arm asserts that the bucket
//! retains only the concurrent harvest (the snapshot was minted). The
//! failure arm asserts that calling `restore_…` merges the snapshot
//! back via `saturating_add`, so the bucket holds snapshot + concurrent
//! and neither side is silently overwritten.

use candid::Principal;

use rumi_protocol_backend::state::State;
use rumi_protocol_backend::InitArg;

fn fresh_state() -> State {
    State::from(InitArg {
        xrc_principal: Principal::anonymous(),
        icusd_ledger_principal: Principal::anonymous(),
        icp_ledger_principal: Principal::from_slice(&[10]),
        fee_e8s: 0,
        developer_principal: Principal::anonymous(),
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    })
}

const INITIAL_BUCKET_E8S: u64 = 100_000_000;
const CONCURRENT_HARVEST_E8S: u64 = 50_000_000;

#[test]
fn int_002_success_arm_preserves_concurrent_accrual() {
    let mut state = fresh_state();
    let ct = Principal::from_slice(&[10]);

    state
        .pending_interest_for_pools
        .insert(ct, INITIAL_BUCKET_E8S);

    // Step 1: atomic snapshot + zero — the fix's pre-await mutation.
    let snapshot = state.take_pending_interest_for_pool(ct);
    assert_eq!(
        snapshot, INITIAL_BUCKET_E8S,
        "snapshot must capture the full bucket value before the await"
    );
    assert!(
        !state.pending_interest_for_pools.contains_key(&ct),
        "bucket must be drained to zero so concurrent harvests accumulate cleanly"
    );

    // Step 2: simulate a concurrent harvest landing during the await.
    *state
        .pending_interest_for_pools
        .entry(ct)
        .or_insert(0) += CONCURRENT_HARVEST_E8S;

    // Step 3a: success arm — snapshot was minted, no restore needed.
    // The bucket holds ONLY the concurrent harvest (the snapshot is gone
    // because the recipients received it as a real ledger mint).
    assert_eq!(
        state.pending_interest_for_pools.get(&ct).copied(),
        Some(CONCURRENT_HARVEST_E8S),
        "success arm: bucket retains concurrent harvest only; snapshot was minted out"
    );
}

#[test]
fn int_002_failure_arm_restores_snapshot_and_preserves_concurrent_accrual() {
    let mut state = fresh_state();
    let ct = Principal::from_slice(&[10]);

    state
        .pending_interest_for_pools
        .insert(ct, INITIAL_BUCKET_E8S);

    // Step 1: atomic snapshot + zero.
    let snapshot = state.take_pending_interest_for_pool(ct);

    // Step 2: simulate a concurrent harvest landing during the await.
    *state
        .pending_interest_for_pools
        .entry(ct)
        .or_insert(0) += CONCURRENT_HARVEST_E8S;

    // Step 3b: failure arm — restore the unminted snapshot via the
    // fix's saturating_add pair.
    state.restore_pending_interest_for_pool(ct, snapshot);

    // The bucket must now hold BOTH the original snapshot (recovered)
    // AND the concurrent harvest (preserved). A naive `insert(ct,
    // snapshot)` would overwrite the concurrent value — that is the
    // race the fix closes.
    assert_eq!(
        state.pending_interest_for_pools.get(&ct).copied(),
        Some(INITIAL_BUCKET_E8S + CONCURRENT_HARVEST_E8S),
        "failure restore must merge via saturating_add, not overwrite"
    );
}

#[test]
fn int_002_restore_saturates_at_u64_max() {
    let mut state = fresh_state();
    let ct = Principal::from_slice(&[10]);

    // Bucket is already at the u64 ceiling. Restoring any non-zero
    // snapshot must not panic — saturating_add caps the bucket.
    state
        .pending_interest_for_pools
        .insert(ct, u64::MAX);

    state.restore_pending_interest_for_pool(ct, INITIAL_BUCKET_E8S);

    assert_eq!(
        state.pending_interest_for_pools.get(&ct).copied(),
        Some(u64::MAX),
        "restore must saturate at u64::MAX rather than wrap or panic"
    );
}

#[test]
fn int_002_take_on_empty_bucket_returns_zero() {
    let mut state = fresh_state();
    let ct = Principal::from_slice(&[10]);

    let snapshot = state.take_pending_interest_for_pool(ct);

    assert_eq!(
        snapshot, 0,
        "take on a missing bucket must return 0, not panic or insert a sentinel"
    );
    assert!(
        !state.pending_interest_for_pools.contains_key(&ct),
        "take must not create a zero entry as a side effect"
    );
}
