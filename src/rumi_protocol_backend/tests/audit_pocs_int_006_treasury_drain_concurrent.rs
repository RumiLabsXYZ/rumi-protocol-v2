//! INT-006 regression fence: the treasury-interest drain must zero its
//! snapshot atomically with the read and recover via saturating_add on
//! mint failure, so concurrent increments landing during the await
//! survive both arms.
//!
//! Audit report:
//!   * `audit-reports/2026-04-22-28e9896/raw-pass-results/debt-interest.json`
//!     finding INT-006.
//!
//! # What the bug was
//!
//! `treasury::drain_pending_treasury_interest` read
//! `pending_treasury_interest` into a local, awaited
//! `mint_icusd(pending, treasury)`, then on success unconditionally
//! assigned `s.pending_treasury_interest = ICUSD::new(0)`. If another
//! code path credited `pending_treasury_interest` during the await
//! window, the post-mint zeroing dropped that credit. On failure no
//! restore ran (concurrent credits survived) — the asymmetry is the
//! anti-pattern the audit flagged.
//!
//! No code path currently writes to `pending_treasury_interest` (the
//! field is effectively dead today), but if a future change adds a
//! writer the existing zero-after-await drain would silently lose
//! revenue. This file pins the contract for that future writer.
//!
//! # How this file tests the fix
//!
//! The fix introduces a snapshot-then-decrement pair on `State`:
//!
//!   * `take_pending_treasury_interest` — atomic snapshot + zero of the
//!     field. Returns the ICUSD amount that was held.
//!   * `restore_pending_treasury_interest` — saturating_add of an
//!     unminted amount back into the field. Used on the failure arm.
//!
//! Mirroring the INT-002 fences: each test pre-loads the field, takes
//! the snapshot (the `mutate_state` step that lands before the await),
//! then directly mutates the field to simulate a concurrent credit
//! landing during the await. The success arm asserts the field retains
//! only the concurrent credit. The failure arm asserts that calling
//! `restore_…` merges the snapshot back via `saturating_add`.

use candid::Principal;

use rumi_protocol_backend::numeric::ICUSD;
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

const INITIAL_FIELD_E8S: u64 = 100_000_000;
const CONCURRENT_CREDIT_E8S: u64 = 50_000_000;

#[test]
fn int_006_success_arm_preserves_concurrent_credit() {
    let mut state = fresh_state();

    state.pending_treasury_interest = ICUSD::new(INITIAL_FIELD_E8S);

    // Step 1: atomic snapshot + zero — the fix's pre-await mutation.
    let snapshot = state.take_pending_treasury_interest();
    assert_eq!(
        snapshot.to_u64(),
        INITIAL_FIELD_E8S,
        "snapshot must capture the full field value before the await"
    );
    assert_eq!(
        state.pending_treasury_interest.to_u64(),
        0,
        "field must be drained to zero so concurrent credits accumulate cleanly"
    );

    // Step 2: simulate a concurrent credit landing during the await.
    state.pending_treasury_interest =
        state.pending_treasury_interest + ICUSD::new(CONCURRENT_CREDIT_E8S);

    // Step 3a: success arm — snapshot was minted, no restore needed.
    // The field holds ONLY the concurrent credit.
    assert_eq!(
        state.pending_treasury_interest.to_u64(),
        CONCURRENT_CREDIT_E8S,
        "success arm: field retains concurrent credit only; snapshot was minted out"
    );
}

#[test]
fn int_006_failure_arm_restores_snapshot_and_preserves_concurrent_credit() {
    let mut state = fresh_state();

    state.pending_treasury_interest = ICUSD::new(INITIAL_FIELD_E8S);

    // Step 1: atomic snapshot + zero.
    let snapshot = state.take_pending_treasury_interest();

    // Step 2: simulate a concurrent credit landing during the await.
    state.pending_treasury_interest =
        state.pending_treasury_interest + ICUSD::new(CONCURRENT_CREDIT_E8S);

    // Step 3b: failure arm — restore the unminted snapshot via the
    // fix's saturating_add pair.
    state.restore_pending_treasury_interest(snapshot);

    // The field must now hold BOTH the original snapshot (recovered)
    // AND the concurrent credit (preserved). A naive
    // `pending_treasury_interest = snapshot` would overwrite the
    // concurrent value — that is the race the fix closes.
    assert_eq!(
        state.pending_treasury_interest.to_u64(),
        INITIAL_FIELD_E8S + CONCURRENT_CREDIT_E8S,
        "failure restore must merge via saturating_add, not overwrite"
    );
}

#[test]
fn int_006_restore_saturates_at_u64_max() {
    let mut state = fresh_state();

    // Field is already at the u64 ceiling. Restoring any non-zero
    // snapshot must not panic — saturating_add caps the field.
    state.pending_treasury_interest = ICUSD::new(u64::MAX);

    state.restore_pending_treasury_interest(ICUSD::new(INITIAL_FIELD_E8S));

    assert_eq!(
        state.pending_treasury_interest.to_u64(),
        u64::MAX,
        "restore must saturate at u64::MAX rather than wrap or panic"
    );
}

#[test]
fn int_006_take_on_zero_field_returns_zero() {
    let mut state = fresh_state();

    assert_eq!(
        state.pending_treasury_interest.to_u64(),
        0,
        "fresh state has zero pending treasury interest"
    );

    let snapshot = state.take_pending_treasury_interest();

    assert_eq!(
        snapshot.to_u64(),
        0,
        "take on a zero field must return 0 cleanly"
    );
    assert_eq!(
        state.pending_treasury_interest.to_u64(),
        0,
        "field stays at zero after take of zero"
    );
}
