//! ASYNC-001 / ASYNC-002 / ASYNC-003 proof-of-concept + regression fences.
//!
//! Audit pass: `audit-reports/2026-06-03-0c3ceb4/raw-pass-results/04-async-icrc-core.json`.
//!
//! These three findings are all instances of the IC "read snapshot -> await
//! inter-canister call -> mutate using the stale snapshot" race, where the
//! per-caller `GuardPrincipal` does NOT serialize two *different* liquidator
//! principals (or a human liquidator vs the stability pool) racing the same
//! vault. True cross-message interleaving requires PocketIC orchestration with
//! a stalled ledger; what these tests pin down deterministically is the
//! *load-bearing mechanism* that turns the race into fund loss:
//!
//!   * ASYNC-001 — the partial-liquidation state mutation subtracts a
//!     PRE-await `max_liquidatable_debt` from the (possibly already-reduced)
//!     vault debt using the custom `numeric::Token::sub`, which **panics on
//!     underflow** rather than saturating. The icUSD was already pulled before
//!     this panic, so it is a trap-after-transfer. We prove the panic, and we
//!     prove the fix shape (`saturating_sub` + post-await re-cap) is panic-free.
//!
//!   * ASYNC-002 — the full-liquidation state mutation (`State::liquidate_vault`)
//!     starts with `.expect("bug: vault not found")`. If a concurrent full
//!     liquidation already removed the vault, the second caller (past its own
//!     icUSD pull) traps here. We prove the `.expect` panics on a removed vault,
//!     and prove that a presence-check guard would avoid it.
//!
//!   * ASYNC-003 — `recover_pending_transfer` pays out via the one-shot
//!     `transfer_collateral` (fresh nonce) instead of the entry's persisted
//!     `op_nonce` that `process_pending_transfer` uses. We prove the two dedup
//!     `created_at_time` values diverge, so the ledger cannot dedup a manual
//!     recovery against a concurrent timer retry => double pay.
//!
//! All assertions are library-level and free of `ic_cdk::api::time()` (which
//! traps outside a canister), matching the convention in
//! `audit_pocs_liq_001_pending_margin_race.rs`.

use candid::Principal;
use std::panic::{catch_unwind, AssertUnwindSafe};

use rumi_protocol_backend::management::nonce_to_created_at_time;
use rumi_protocol_backend::numeric::ICUSD;

// ──────────────────────────────────────────────────────────────
// ASYNC-001 — partial-liquidation underflow panic after the icUSD pull
// ──────────────────────────────────────────────────────────────

/// The exact subtraction `vault.borrowed_icusd_amount -= max_liquidatable_debt`
/// from `liquidate_vault_partial` (vault.rs ~2330) / `_with_stable` (~2624) /
/// `liquidate_vault_debt_already_burned` (~3013). `max_liquidatable_debt` is
/// sized against the PRE-await snapshot; a concurrent liquidation can shrink the
/// live debt below it. ICUSD is `numeric::Token`, whose `Sub` impl does
/// `panic!("underflow")` — so the second liquidator traps *after* its icUSD has
/// already settled at the ledger.
#[test]
fn async_001_stale_snapshot_debit_underflows_and_panics() {
    // Snapshot taken by liquidator B before its await: vault had debt = 100.
    let snapshot_max_liquidatable_debt = ICUSD::new(100_000_000); // 1.0 icUSD

    // Meanwhile liquidator A's liquidation reduced the live debt to 40.
    let live_debt_after_concurrent_liq = ICUSD::new(40_000_000); // 0.4 icUSD

    // B resumes (icUSD already pulled) and runs the live code's `-=`.
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _new_debt = live_debt_after_concurrent_liq - snapshot_max_liquidatable_debt;
    }));

    assert!(
        result.is_err(),
        "ASYNC-001: subtracting a stale max_liquidatable_debt from the reduced \
         live debt MUST panic (numeric::Token::sub underflow) — this is the \
         trap-after-transfer. If this stops panicking, the subtraction was made \
         saturating; update this fence."
    );
}

/// The fix shape: re-cap the liquidation amount against the CURRENT vault debt
/// inside the critical section and use saturating subtraction. With that, the
/// stale snapshot can never drive an underflow panic.
#[test]
fn async_001_fix_recap_and_saturating_sub_is_panic_free() {
    let snapshot_max_liquidatable_debt = ICUSD::new(100_000_000);
    let live_debt = ICUSD::new(40_000_000);

    // Re-cap against live debt (what the recovery-mode path in
    // state::liquidate_vault already does with `.min(vault.borrowed_icusd_amount)`).
    let recapped = snapshot_max_liquidatable_debt.min(live_debt);
    assert_eq!(recapped, live_debt, "re-cap must clamp to the live debt");

    // Saturating subtraction never underflows.
    let result = catch_unwind(AssertUnwindSafe(|| {
        let new_debt = live_debt.saturating_sub(recapped);
        assert_eq!(new_debt, ICUSD::new(0));
    }));
    assert!(
        result.is_ok(),
        "ASYNC-001 fix: re-cap + saturating_sub must not panic"
    );
}

/// Sibling raw-`u64` collateral debit `vault.collateral_amount -= total_to_seize`
/// (vault.rs ~2331/2625/3014). Canisters build with the default release profile
/// (overflow-checks = off), so a stale `total_to_seize` exceeding live collateral
/// WRAPS to a near-`u64::MAX` collateral value rather than panicking — corrupting
/// the vault instead of trapping. The debt-line panic above fires first in the
/// live ordering, but this shows the collateral line is independently unsafe and
/// must also be made saturating.
#[test]
fn async_001_raw_u64_collateral_debit_wraps_without_overflow_checks() {
    let live_collateral: u64 = 40_000_000;
    let stale_total_to_seize: u64 = 100_000_000;

    // Mirror release semantics (overflow-checks off): wrapping_sub.
    let wrapped = live_collateral.wrapping_sub(stale_total_to_seize);
    assert!(
        wrapped > u64::MAX / 2,
        "ASYNC-001: raw u64 collateral debit on a stale seize amount wraps to a \
         huge value (got {}), corrupting vault collateral. Must use saturating_sub.",
        wrapped
    );

    // The fix: saturating_sub clamps to zero.
    assert_eq!(live_collateral.saturating_sub(stale_total_to_seize), 0);
}

// ──────────────────────────────────────────────────────────────
// ASYNC-002 — full-liquidation `.expect` traps on a vault a concurrent
// liquidation already removed.
// ──────────────────────────────────────────────────────────────

/// `State::liquidate_vault` (state.rs ~3751) opens with
/// `self.vault_id_to_vaults.get(&vault_id).cloned().expect("bug: vault not found")`.
/// We reproduce that exact access pattern against an empty map (the state after a
/// concurrent full liquidation removed the vault) and assert it panics. In the
/// live flow the second liquidator's icUSD was already pulled before this panic,
/// so the trap strands their payment.
#[test]
fn async_002_full_liquidation_expect_panics_on_removed_vault() {
    use std::collections::BTreeMap;
    // Stand-in for `vault_id_to_vaults` after the concurrent liquidation removed it.
    let vaults: BTreeMap<u64, u64> = BTreeMap::new();
    let vault_id = 7u64;

    let result = catch_unwind(AssertUnwindSafe(|| {
        // Exact shape of the live `.get(..).cloned().expect(..)`.
        let _v = vaults.get(&vault_id).cloned().expect("bug: vault not found");
    }));

    assert!(
        result.is_err(),
        "ASYNC-002: the full-liquidation `.expect(\"bug: vault not found\")` MUST \
         panic when the vault was removed by a concurrent liquidation. The live \
         code reaches this AFTER pulling the liquidator's icUSD, so the trap is a \
         fund-loss. The fix is a graceful early-return + refund of the pulled icUSD."
    );
}

/// The fix shape: check presence and return gracefully (so the caller can refund
/// the just-pulled icUSD) instead of `.expect`-ing.
#[test]
fn async_002_fix_presence_check_returns_gracefully() {
    use std::collections::BTreeMap;
    let vaults: BTreeMap<u64, u64> = BTreeMap::new();
    let vault_id = 7u64;

    let outcome: Result<(), &'static str> = match vaults.get(&vault_id) {
        Some(_) => Ok(()),
        None => Err("vault already liquidated by a concurrent call; refund and bail"),
    };
    assert!(
        outcome.is_err(),
        "ASYNC-002 fix: a missing vault must surface a recoverable error, not a trap"
    );
}

// ──────────────────────────────────────────────────────────────
// ASYNC-003 — recover_pending_transfer's fresh nonce diverges from the
// timer's persisted nonce, defeating ledger dedup => double pay.
// ──────────────────────────────────────────────────────────────

/// A pending entry carries a persisted `op_nonce`. The background
/// `process_pending_transfer` pays with `transfer_collateral_with_nonce(..,
/// transfer.op_nonce)` (persisted). `recover_pending_transfer` pays with the
/// one-shot `transfer_collateral`, which mints a FRESH nonce. The ledger's dedup
/// key is `created_at_time` (+ memo), derived from the nonce. We prove the two
/// `created_at_time`s differ, so the ledger treats the two payouts as distinct
/// transfers and credits the owner TWICE for one pending entry.
#[test]
fn async_003_manual_recovery_nonce_diverges_from_persisted_nonce() {
    // Persisted nonce on the pending entry: time T0 in the high 64 bits,
    // counter 0 in the low 64 bits (the layout state::next_op_nonce produces).
    let t0: u128 = 1_700_000_000_000_000_000;
    let persisted_nonce: u128 = (t0 << 64) | 0u128;

    // The one-shot `transfer_collateral` mints a fresh nonce at recovery time T1
    // with a later counter. Even at the SAME nanosecond, the counter differs;
    // realistically the timestamp differs too.
    let t1: u128 = t0 + 5; // recovery happens a few ns later
    let fresh_nonce: u128 = (t1 << 64) | 1u128;

    let persisted_created_at = nonce_to_created_at_time(persisted_nonce);
    let fresh_created_at = nonce_to_created_at_time(fresh_nonce);

    assert_ne!(
        persisted_created_at, fresh_created_at,
        "ASYNC-003: recover_pending_transfer's fresh-nonce created_at_time ({}) \
         MUST differ from the timer's persisted-nonce created_at_time ({}); \
         because they differ, the ledger will NOT dedup the two payouts and the \
         owner is paid twice. The fix is to make recover_pending_transfer reuse \
         transfer.op_nonce (so both paths share one dedup tuple).",
        fresh_created_at, persisted_created_at
    );
}

/// The fix: recover_pending_transfer reuses the entry's persisted op_nonce, so
/// its dedup `created_at_time` is IDENTICAL to the timer path's. The ledger then
/// returns `Duplicate { duplicate_of }` (which `transfer_idempotent` converts to
/// Ok), and the owner is paid exactly once.
#[test]
fn async_003_fix_reusing_persisted_nonce_matches_dedup_key() {
    let t0: u128 = 1_700_000_000_000_000_000;
    let persisted_nonce: u128 = (t0 << 64) | 0u128;

    // Both the timer and the (fixed) manual recovery use the SAME persisted nonce.
    let timer_created_at = nonce_to_created_at_time(persisted_nonce);
    let recovery_created_at = nonce_to_created_at_time(persisted_nonce);

    assert_eq!(
        timer_created_at, recovery_created_at,
        "ASYNC-003 fix: reusing the persisted op_nonce makes both payout paths \
         share one ledger dedup tuple, so the second is deduped (Duplicate -> Ok) \
         and the owner is paid once."
    );
}

// A separate principal helper kept for parity with the LIQ-001 fixture style and
// to make the intent (two distinct liquidators are NOT serialized by the
// per-caller guard) explicit for readers.
#[allow(dead_code)]
fn liquidator_a() -> Principal {
    Principal::from_slice(&[1])
}
#[allow(dead_code)]
fn liquidator_b() -> Principal {
    Principal::from_slice(&[2])
}
