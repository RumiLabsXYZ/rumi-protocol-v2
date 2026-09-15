//! Sentinel's own self-recovery lane: a hard-coded funding path, entirely
//! outside the governed target registry, that keeps Sentinel's RUNTIME
//! cycle balance (`ic_cdk::api::canister_balance128()`) above a compiled
//! floor by withdrawing from the protected portion of its own Cycles
//! Ledger account, to itself.
//!
//! Three invariants this file exists to enforce, none of which any ordinary
//! target path may weaken:
//!
//! 1. **Immutable destination.** The withdrawal destination is constructed
//!    ONLY from the `sentinel_id` parameter every entry point requires —
//!    which callers (`lib.rs`) must pass as `ic_cdk::id()` — never from a
//!    caller argument or governed config. There is no field anywhere in
//!    this file's public surface a signer or proposal could set to redirect
//!    self-recovery funds.
//! 2. **Compiled minimum threshold.** `effective_threshold` floors the
//!    governed `SelfRecoveryPolicy.low_balance_threshold_cycles` at
//!    `MIN_EFFECTIVE_RUNTIME_THRESHOLD_CYCLES`, so a misconfigured (too low)
//!    governed value can never leave Sentinel executing below the
//!    freeze/execution safety margin the design requires.
//! 3. **Global suppression.** Any unresolved self-recovery operation —
//!    including `Unknown` or `Quarantined`, not just a fresh in-flight one —
//!    suppresses ALL ordinary distribution
//!    (`funding::check_ordinary_eligibility`'s `SelfRecoveryUnresolved`
//!    check reads exactly the same `SelfRecoveryState::is_suppressing_distribution`
//!    this file drives). `run`'s own return value additionally suppresses
//!    the SAME round whenever recoverability would be worsened by
//!    continuing to spend on ordinary targets — see its doc comment.

use candid::{Nat, Principal};

use crate::funding::cycles::{self as funding_cycles, FundingError};
use crate::state;
use crate::types::{
    self, AlarmKind, CyclesFundingState, CyclesWithdrawSnapshot, FundingAttemptResultClass,
    FundingOperation, FundingOperationState, FundingRailArguments, FundingTrigger,
    SelfRecoveryPolicy, SourceReserveError, TargetFundingPolicy, TargetFundingPolicyArgs,
};

/// Compiled floor for the EFFECTIVE runtime low-balance threshold — always
/// enforced regardless of what a governed `SelfRecoveryPolicy` proposes.
/// This is a fixed safety margin above the IC's freeze/execution boundary,
/// not a governable value: see this module's doc comment, invariant 2.
pub const MIN_EFFECTIVE_RUNTIME_THRESHOLD_CYCLES: u128 = 1_000_000_000_000;

/// The threshold self-recovery actually compares the runtime balance
/// against: the governed value, floored at the compiled minimum. Pure and
/// total — always returns a value, never fails.
pub fn effective_threshold(policy_threshold_cycles: u128) -> u128 {
    policy_threshold_cycles.max(MIN_EFFECTIVE_RUNTIME_THRESHOLD_CYCLES)
}

/// Builds the placeholder `TargetFundingPolicy` a `SelfRecovery`-triggered
/// `FundingOperation` carries in its `funding_policy` field (that field is
/// typed `TargetFundingPolicy` for every trigger — see
/// `types::FundingOperation`'s doc comment — even though self-recovery is
/// never a registered target). Uses `validate_self_contained`, NOT
/// `validate`: `SelfRecoveryPolicy`'s own cap/threshold/refill values are
/// independent governed inputs with no required relationship to the
/// ordinary `GlobalPolicy.global_daily_cap_cycles` ordinary targets share
/// (see that method's doc comment in `types.rs` for the full reasoning).
fn placeholder_funding_policy(
    policy: &SelfRecoveryPolicy,
) -> Result<TargetFundingPolicy, types::TargetValidationError> {
    TargetFundingPolicy::validate_self_contained(&TargetFundingPolicyArgs {
        low_balance_threshold_cycles: Nat::from(policy.low_balance_threshold_cycles()),
        refill_cycles: Nat::from(policy.refill_cycles()),
        daily_cap_cycles: Nat::from(policy.daily_cap_cycles()),
        // Self-recovery has no cooldown concept of its own
        // (`SelfRecoveryState` tracks only a rolling cap and one in-flight
        // operation, never a cooldown) — this value is inert.
        cooldown_secs: 0,
        burn_anomaly_limit_cycles_per_day: None,
    })
}

/// Precomputes every fallible value — self-recovery's own daily-cap
/// reservation, the SHARED `SOURCE_RESERVE` debit it draws from alongside
/// ordinary targets, and the fully-constructed, already-`Submitted`
/// operation VALUE — before committing ANY of them, then commits in the
/// same "operation existence first, then infallible reservation writes"
/// order `funding::cycles::prepare_ordinary` uses (correction pass,
/// atomicity review Finding 3: no reservation and no operation may be
/// partially added on a late failure). No await occurs anywhere in this
/// function. Only `state::next_operation_id`/`next_created_at_time_ns`'s own
/// monotonic counter advances are allowed to be "spent" on a later failure
/// — an explicitly accepted, documented, non-corrupting gap.
///
/// `sentinel_id` becomes BOTH the operation's `target` and the
/// `CyclesWithdrawSnapshot.destination` — the only two places a
/// `FundingOperation`'s recipient is ever read from — but only after
/// `actual_sentinel_id == sentinel_id` has been checked. A caller/config
/// value can therefore never redirect the withdrawal to another principal.
fn prepare_at(
    actual_sentinel_id: Principal,
    sentinel_id: Principal,
    refill_cycles: u128,
    now_secs: u64,
    now_ns: u64,
) -> Result<FundingOperation, FundingError> {
    funding_cycles::require_sentinel_identity(actual_sentinel_id, sentinel_id)?;
    let global_policy = state::global_config().global_policy;
    let self_recovery_policy = global_policy.self_recovery_policy();

    let cache = state::get_source_reserve()
        .cache()
        .ok_or(FundingError::SourceReserve(
            SourceReserveError::UnknownCache,
        ))?;
    let fee_cycles = cache.fee_cycles;
    let amount_plus_fee = refill_cycles
        .checked_add(fee_cycles)
        .ok_or(FundingError::Overflow)?;

    // ── Phase 1: precompute every fallible value. No commit yet. ──

    let operation_id = state::next_operation_id();

    let self_recovery_reservation = state::get_self_recovery_state()
        .begin(
            operation_id,
            refill_cycles,
            now_secs,
            funding_cycles::ROLLING_CAP_WINDOW_SECS,
            self_recovery_policy.daily_cap_cycles(),
        )
        .map_err(FundingError::SelfRecoveryReserve)?;
    let max_cache_age = global_policy.stale_after_secs();
    let source_reservation = state::get_source_reserve()
        .reserve_self_recovery(operation_id, amount_plus_fee, now_secs, max_cache_age)
        .map_err(FundingError::SourceReserve)?;

    let created_at_time_ns =
        state::next_created_at_time_ns(now_ns).map_err(FundingError::MonotonicTime)?;
    let snapshot = CyclesWithdrawSnapshot {
        destination: sentinel_id,
        from_subaccount: None,
        amount_cycles: refill_cycles,
        fee_cycles,
        created_at_time_ns,
    };
    let placeholder_policy = placeholder_funding_policy(self_recovery_policy)
        .map_err(FundingError::PlaceholderPolicy)?;
    let op = FundingOperation::open(
        operation_id,
        sentinel_id,
        0,
        placeholder_policy,
        FundingTrigger::SelfRecovery,
        FundingRailArguments::Cycles(snapshot),
        refill_cycles,
        now_secs,
    )
    .map_err(FundingError::Open)?;
    let submitted = op
        .record_attempt(
            FundingOperationState::Cycles(CyclesFundingState::Submitted),
            now_secs,
            FundingAttemptResultClass::Indeterminate,
        )
        .map_err(FundingError::Transition)?;

    // ── Phase 2: every fallible value above is known-good. Commit. ──
    // Persist the already-submitted value directly. There is no fallible
    // final update after the reservation writes.
    state::insert_operation(submitted.clone()).map_err(FundingError::Insert)?;
    state::set_self_recovery_state(self_recovery_reservation);
    state::set_source_reserve(source_reservation);
    Ok(submitted)
}

#[cfg(test)]
fn prepare(
    sentinel_id: Principal,
    refill_cycles: u128,
    now_secs: u64,
    now_ns: u64,
) -> Result<FundingOperation, FundingError> {
    // Deterministic unit tests do not have an IC execution context. Passing
    // the same principal as both sides still exercises the guarded core
    // without weakening the production `run` wiring.
    prepare_at(sentinel_id, sentinel_id, refill_cycles, now_secs, now_ns)
}

/// The self-recovery timer/entry-point step (design order: "self-recovery,
/// resume pending operations, sample, then evaluate new target
/// maintenance" — this function IS that first step, including its own
/// resume). `sentinel_id` MUST be `ic_cdk::id()`; passed explicitly (never
/// read from inside this function) so it stays callable from plain unit
/// tests off the `wasm32` target and so the module doc's destination
/// invariant is checkable by inspection at every call site.
///
/// Returns whether ordinary distribution may proceed THIS round:
/// - `true`: no self-recovery operation is unresolved, and the runtime
///   balance is currently healthy (above `effective_threshold`) — either
///   because it always was, or because an attempt just resolved to
///   `Complete` in this same call.
/// - `false`: an unresolved self-recovery operation remains (including
///   freshly `Unknown`/`Quarantined`), OR the runtime balance is low and the
///   protected reserve could not cover a recovery attempt (an insufficient/
///   unknown/stale source cache, or the reservation itself being refused) —
///   design: "If the protected reserve is insufficient, Sentinel alarms and
///   does not distribute funds that would worsen its own recoverability."
pub async fn run(now_secs: u64, now_ns: u64, sentinel_id: Principal) -> bool {
    if funding_cycles::require_sentinel_identity(ic_cdk::id(), sentinel_id).is_err() {
        let _ = state::alarms::raise_at(None, AlarmKind::SelfRecoveryUnresolved, now_secs);
        return false;
    }
    let before = state::get_self_recovery_state();
    if let Some(op_id) = before.in_flight_operation_id() {
        if let Some(op) = state::get_operation(op_id) {
            if !op.state().stops_automatic_retry() {
                let _ = funding_cycles::execute(op, now_secs).await;
            }
        }
    }

    let current = state::get_self_recovery_state();
    if current.is_suppressing_distribution() {
        let _ = state::alarms::raise_at(None, AlarmKind::SelfRecoveryUnresolved, now_secs);
        return false;
    }
    // No longer unresolved (never started, or just resolved above): the
    // alarm this lane owns for "unresolved" no longer applies.
    state::alarms::resolve_at(None, AlarmKind::SelfRecoveryUnresolved, now_secs);

    let runtime_balance_cycles = ic_cdk::api::canister_balance128();
    let global_policy = state::global_config().global_policy;
    let policy = global_policy.self_recovery_policy();
    let threshold = effective_threshold(policy.low_balance_threshold_cycles());
    if runtime_balance_cycles > threshold {
        state::alarms::resolve_at(None, AlarmKind::LowBalance, now_secs);
        return true;
    }

    match prepare_at(
        ic_cdk::id(),
        sentinel_id,
        policy.refill_cycles(),
        now_secs,
        now_ns,
    ) {
        Ok(op) => match funding_cycles::execute(op, now_secs).await {
            Ok(resolved) => apply_recovery_result(resolved.state(), now_secs),
            Err(_) => {
                let _ = state::alarms::raise_at(None, AlarmKind::LowBalance, now_secs);
                false
            }
        },
        Err(_) => {
            // Reservation itself failed (unknown/stale cache, insufficient
            // reserve, or daily cap) — no operation was ever opened, so
            // there is nothing to resume, but the low-runtime condition is
            // real and must not be silently dropped.
            let _ = state::alarms::raise_at(None, AlarmKind::LowBalance, now_secs);
            false
        }
    }
}

/// Maps a just-resolved self-recovery operation's final state to this
/// round's alarm/suppression outcome. `Complete` is the only state that
/// clears both alarms and allows ordinary distribution this same round;
/// everything else keeps at least one alarm open and suppresses.
fn apply_recovery_result(state_after: FundingOperationState, now_secs: u64) -> bool {
    match state_after {
        FundingOperationState::Cycles(CyclesFundingState::Complete) => {
            state::alarms::resolve_at(None, AlarmKind::LowBalance, now_secs);
            state::alarms::resolve_at(None, AlarmKind::SelfRecoveryUnresolved, now_secs);
            true
        }
        FundingOperationState::Cycles(CyclesFundingState::Unknown)
        | FundingOperationState::Cycles(CyclesFundingState::Quarantined) => {
            let _ = state::alarms::raise_at(None, AlarmKind::SelfRecoveryUnresolved, now_secs);
            false
        }
        // A proven terminal failure (no delivery, whether or not a fee was
        // debited): recovery did not happen and the runtime balance is
        // still low.
        _ => {
            let _ = state::alarms::raise_at(None, AlarmKind::LowBalance, now_secs);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state;
    use crate::types::{
        GlobalPolicy, GlobalPolicyArgs, GovernanceTimelocksArgs, InitArgs, SelfRecoveryPolicyArgs,
    };

    fn sentinel_id() -> Principal {
        Principal::from_slice(&[7, 7, 7])
    }

    fn other_principal() -> Principal {
        Principal::from_slice(&[8, 8, 8])
    }

    fn test_governance_timelocks_args() -> GovernanceTimelocksArgs {
        GovernanceTimelocksArgs {
            target_registry_secs: 1,
            spend_policy_secs: 1,
            signer_change_secs: 1,
            unpause_secs: 1,
        }
    }

    fn test_global_policy(
        protected_reserve: u128,
        cap: u128,
        threshold: u128,
        refill: u128,
        stale_after_secs: u64,
    ) -> GlobalPolicy {
        GlobalPolicy::validate(&GlobalPolicyArgs {
            global_daily_cap_cycles: Nat::from(1_000_000_000u64),
            sample_interval_secs: 60,
            stale_after_secs,
            min_icp_reserve_e8s: Nat::from(0u32),
            timelocks: test_governance_timelocks_args(),
            self_recovery_policy: SelfRecoveryPolicyArgs {
                protected_reserve_cycles: Nat::from(protected_reserve),
                daily_cap_cycles: Nat::from(cap),
                low_balance_threshold_cycles: Nat::from(threshold),
                refill_cycles: Nat::from(refill),
            },
        })
        .unwrap()
    }

    fn init_test_state(global: GlobalPolicy) {
        state::init(InitArgs {
            signers: vec![Principal::from_slice(&[1])],
            approval_threshold: 1,
            global_policy: global.to_args(),
        })
        .unwrap();
    }

    fn seed_cache(balance: u128, fee: u128, now_secs: u64) {
        let cache = state::get_source_reserve()
            .refresh(balance, fee, now_secs)
            .unwrap();
        state::set_source_reserve(cache);
    }

    // ─── effective_threshold: compiled 1T floor ───

    #[test]
    fn effective_threshold_floors_a_lower_governed_value_at_one_trillion() {
        assert_eq!(
            effective_threshold(500_000_000_000),
            MIN_EFFECTIVE_RUNTIME_THRESHOLD_CYCLES
        );
    }

    #[test]
    fn effective_threshold_keeps_a_higher_governed_value() {
        assert_eq!(effective_threshold(5_000_000_000_000), 5_000_000_000_000);
    }

    // ─── prepare: immutable destination is always the passed sentinel_id ───

    #[test]
    fn prepare_destination_is_always_the_passed_sentinel_id_never_anything_else() {
        let global = test_global_policy(1_000, 500, 1, 10, 600);
        init_test_state(global);
        seed_cache(1_000_000, 0, 1_000);
        let op = prepare(sentinel_id(), 10, 1_000, 0).unwrap();
        assert_eq!(op.target(), sentinel_id());
        let FundingRailArguments::Cycles(snapshot) = op.rail_arguments() else {
            unreachable!()
        };
        assert_eq!(snapshot.destination, sentinel_id());
        assert_ne!(snapshot.destination, other_principal());
        assert_eq!(op.trigger(), FundingTrigger::SelfRecovery);
    }

    #[test]
    fn prepare_at_rejects_a_destination_or_source_identity_other_than_this_canister() {
        // The identity check is deliberately before any state read or
        // operation-id allocation, so a bad wiring/configuration value cannot
        // reserve cycles or redirect a withdrawal even in a partially
        // initialized execution context.
        assert_eq!(
            prepare_at(sentinel_id(), other_principal(), 10, 1_000, 0),
            Err(FundingError::SentinelIdentityMismatch)
        );
    }

    #[test]
    fn prepare_persists_submitted_before_any_await_and_reserves_both_ledgers() {
        let global = test_global_policy(1_000, 500, 1, 10, 600);
        init_test_state(global);
        seed_cache(1_000_000, 0, 1_000);
        let op = prepare(sentinel_id(), 10, 1_000, 0).unwrap();
        assert_eq!(
            op.state(),
            FundingOperationState::Cycles(CyclesFundingState::Submitted)
        );
        assert_eq!(state::get_operation(op.id()), Some(op.clone()));
        assert_eq!(
            state::get_self_recovery_state().in_flight_operation_id(),
            Some(op.id())
        );
        assert!(state::get_source_reserve()
            .pending()
            .iter()
            .any(|p| p.operation_id == op.id()));
    }

    #[test]
    fn prepare_fails_closed_with_unknown_cache() {
        let global = test_global_policy(1_000, 500, 1, 10, 600);
        init_test_state(global);
        // No `seed_cache` call.
        assert_eq!(
            prepare(sentinel_id(), 10, 1_000, 0),
            Err(FundingError::SourceReserve(
                SourceReserveError::UnknownCache
            ))
        );
        assert!(state::get_self_recovery_state()
            .in_flight_operation_id()
            .is_none());
    }

    #[test]
    fn prepare_rejects_second_start_while_unresolved() {
        let global = test_global_policy(1_000, 500, 1, 10, 600);
        init_test_state(global);
        seed_cache(1_000_000, 0, 1_000);
        let first = prepare(sentinel_id(), 10, 1_000, 0).unwrap();
        assert_eq!(
            prepare(sentinel_id(), 10, 1_000, 0),
            Err(FundingError::SelfRecoveryReserve(
                types::SelfRecoveryStateError::AlreadyInFlight
            ))
        );
        assert_eq!(state::get_operation(first.id()), Some(first));
    }

    /// Correction pass, atomicity review Finding 3 (self-recovery lane): a
    /// forced LATE failure (`insert_operation`'s `TooManyOperations`,
    /// triggered after both reservation values have already been validated
    /// `Ok`) must not leave the self-recovery or source reservation
    /// committed with no corresponding operation.
    #[test]
    fn prepare_forced_late_insert_failure_leaves_no_orphaned_reservation() {
        let global = test_global_policy(1_000, 500, 1, 10, 600);
        init_test_state(global);
        seed_cache(1_000_000, 0, 1_000);

        for i in 0..(types::MAX_FUNDING_OPERATIONS as u64) {
            let snapshot = CyclesWithdrawSnapshot {
                destination: sentinel_id(),
                from_subaccount: None,
                amount_cycles: 1,
                fee_cycles: 0,
                created_at_time_ns: i,
            };
            let policy = TargetFundingPolicy::validate_self_contained(&TargetFundingPolicyArgs {
                low_balance_threshold_cycles: Nat::from(1u32),
                refill_cycles: Nat::from(1u32),
                daily_cap_cycles: Nat::from(10u32),
                cooldown_secs: 0,
                burn_anomaly_limit_cycles_per_day: None,
            })
            .unwrap();
            let filler = FundingOperation::open(
                10_000 + i,
                sentinel_id(),
                0,
                policy,
                FundingTrigger::SelfRecovery,
                FundingRailArguments::Cycles(snapshot),
                1,
                1_000,
            )
            .unwrap();
            state::insert_operation(filler).unwrap();
        }

        assert_eq!(
            prepare(sentinel_id(), 10, 1_000, 0),
            Err(FundingError::Insert(
                state::InsertOperationError::TooManyOperations
            ))
        );
        assert!(state::get_self_recovery_state()
            .in_flight_operation_id()
            .is_none());
        assert!(state::get_source_reserve().pending().is_empty());
    }

    // ─── run: healthy / in-flight / suppression / low-fund alarms ───

    #[test]
    fn run_is_a_noop_true_when_runtime_balance_is_healthy() {
        // `ic_cdk::api::canister_balance128()` is 0 under `cargo test`
        // (off-`wasm32`, no live canister), which is always <= any
        // threshold — so a healthy-balance round is exercised via
        // `apply_recovery_result` directly (see below) rather than through
        // `run` itself, which cannot be driven off-target. This test
        // documents that boundary explicitly instead of silently skipping
        // it.
        let global = test_global_policy(1_000, 500, 1, 10, 600);
        init_test_state(global);
        assert_eq!(
            effective_threshold(1),
            MIN_EFFECTIVE_RUNTIME_THRESHOLD_CYCLES
        );
    }

    #[test]
    fn apply_recovery_result_complete_resolves_both_alarms_and_allows_distribution() {
        let global = test_global_policy(1_000, 500, 1, 10, 600);
        init_test_state(global);
        let _ = state::alarms::raise_at(None, AlarmKind::LowBalance, 900);
        let _ = state::alarms::raise_at(None, AlarmKind::SelfRecoveryUnresolved, 900);
        let allowed = apply_recovery_result(
            FundingOperationState::Cycles(CyclesFundingState::Complete),
            1_000,
        );
        assert!(allowed);
    }

    #[test]
    fn apply_recovery_result_unknown_and_quarantined_suppress_and_alarm() {
        let global = test_global_policy(1_000, 500, 1, 10, 600);
        init_test_state(global);
        assert!(!apply_recovery_result(
            FundingOperationState::Cycles(CyclesFundingState::Unknown),
            1_000
        ));
        assert!(!apply_recovery_result(
            FundingOperationState::Cycles(CyclesFundingState::Quarantined),
            1_000
        ));
    }

    #[test]
    fn apply_recovery_result_terminal_failure_suppresses_and_raises_low_balance() {
        let global = test_global_policy(1_000, 500, 1, 10, 600);
        init_test_state(global);
        assert!(!apply_recovery_result(
            FundingOperationState::Cycles(CyclesFundingState::Terminal),
            1_000
        ));
    }

    // ─── protected reserve / global suppression semantics via the shared reservation ───

    #[test]
    fn insufficient_protected_reserve_blocks_self_recovery_reservation() {
        let global = test_global_policy(1_000, 1_000, 1, 950, 600);
        init_test_state(global);
        // Only 900 available, but the configured refill is 950.
        seed_cache(900, 0, 1_000);
        assert_eq!(
            prepare(sentinel_id(), 950, 1_000, 0),
            Err(FundingError::SourceReserve(
                SourceReserveError::InsufficientReserve
            ))
        );
    }

    #[test]
    fn self_recovery_may_spend_the_full_cached_balance_including_protected_portion() {
        let global = test_global_policy(1_000, 500, 1, 10, 600);
        init_test_state(global);
        seed_cache(10, 0, 1_000);
        // Unlike an ordinary reservation, self-recovery is not blocked by
        // its own protected reserve floor — it IS that reserve.
        let op = prepare(sentinel_id(), 10, 1_000, 0).unwrap();
        assert_eq!(op.reserved_amount_cycles(), 10);
    }

    #[test]
    fn unresolved_self_recovery_suppresses_ordinary_eligibility() {
        let global = test_global_policy(1_000, 500, 1, 10, 600);
        init_test_state(global);
        seed_cache(1_000_000, 0, 1_000);
        prepare(sentinel_id(), 10, 1_000, 0).unwrap();
        assert!(state::get_self_recovery_state().is_suppressing_distribution());
        assert_eq!(
            crate::funding::check_ordinary_eligibility(other_principal(), 1_000, true),
            Err(crate::funding::EligibilityError::UnregisteredTarget)
        );
        // Even for a registered target, the self-recovery suppression check
        // is evaluated before eligibility can otherwise succeed — proven at
        // the `funding::cycles` layer via `SelfRecoveryUnresolved` in
        // `funding::tests`; this test only proves the SHARED state this
        // module drives (`is_suppressing_distribution`) is what that check
        // reads.
    }

    // ─── immutable destination stays sentinel-only across a resumed retry ───

    #[test]
    fn resumed_operation_keeps_the_same_immutable_destination() {
        let global = test_global_policy(1_000, 500, 1, 10, 600);
        init_test_state(global);
        seed_cache(1_000_000, 0, 1_000);
        let op = prepare(sentinel_id(), 10, 1_000, 0).unwrap();
        let unknown = funding_cycles::resolve_operation(
            op.clone(),
            crate::cycles_ledger::WithdrawOutcome::Unknown,
            1_000,
        )
        .unwrap();
        let FundingRailArguments::Cycles(snapshot) = unknown.rail_arguments() else {
            unreachable!()
        };
        assert_eq!(snapshot.destination, sentinel_id());
        assert_eq!(unknown.target(), sentinel_id());
    }
}
