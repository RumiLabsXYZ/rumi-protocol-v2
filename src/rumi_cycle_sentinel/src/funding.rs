//! Ordinary target funding: eligibility, and the manual/timer-shared
//! entry point into `cycles::run_ordinary`.
//!
//! `cycles` (this module's submodule) owns the durable Cycles Ledger outbox
//! executor shared by ordinary targets (this file) and Sentinel's own
//! self-recovery lane (`self_recovery.rs`): reservation precompute-then-
//! commit, `FundingOperation` open/submit, the `withdraw` await, and outcome
//! resolution (state transition, reservation settle/release, compaction).
//! Manual (`manual_top_up_at`) and later timer callers both funnel through
//! `cycles::run_ordinary`/`cycles::resume`, so they share the exact same
//! reservations and cannot race past them — the in-flight check inside
//! `TargetReservationState::reserve` (and, for self-recovery,
//! `SelfRecoveryState::begin`) is what actually enforces "one unresolved
//! operation," synchronously, before either caller's own await ever starts.

use candid::Principal;

use crate::state;
use crate::types::{self, AdvisoryCyclesBalance, ObservationMode, PublicTargetState, TargetRecord};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EligibilityError {
    UnregisteredTarget,
    Disabled,
    /// Auto top-up is off and this is not a manual call.
    NotAutoTopup,
    Paused,
    Unobserved,
    /// Any unresolved self-recovery operation (including `Unknown` or
    /// `Quarantined`) suppresses ordinary distribution entirely.
    SelfRecoveryUnresolved,
    NoSample,
    /// The latest sample is dated after the caller's clock. A future-dated
    /// observation must never be treated as fresh low-balance evidence.
    FutureSample,
    StaleSample,
    /// The latest sample is `Unreachable`, or its balance overflowed
    /// (`AdvisoryCyclesBalance::Overflow`) — either way there is no
    /// trustworthy balance to compare against the threshold.
    Unreachable,
    /// Observed balance is strictly above the registry threshold.
    NotLow,
    OperationInFlight,
    Cooldown,
}

/// Ordinary eligibility, shared by `manual_top_up_at` and (later) the
/// timer's low-balance sweep — see the module doc. Pure: reads only
/// already-loaded state, performs no await and no mutation, so it can be
/// re-checked cheaply and is safe to call before ever allocating an
/// operation id.
///
/// `manual` bypasses only the `auto_topup` gate (design: "Manual top-up
/// requires a signer and still obeys the target policy, caps, cooldown,
/// reserve, and one-operation rule" — `auto_topup` is not among those; it
/// specifically gates the AUTOMATIC low-balance lane).
pub(crate) fn check_ordinary_eligibility(
    target: Principal,
    now_secs: u64,
    manual: bool,
) -> Result<(TargetRecord, u128), EligibilityError> {
    let record = state::get_target(target).ok_or(EligibilityError::UnregisteredTarget)?;
    if !record.enabled() {
        return Err(EligibilityError::Disabled);
    }
    if !manual && !record.auto_topup() {
        return Err(EligibilityError::NotAutoTopup);
    }
    if record.paused() {
        return Err(EligibilityError::Paused);
    }
    if record.observation_mode() == ObservationMode::Unobserved {
        return Err(EligibilityError::Unobserved);
    }
    if state::get_self_recovery_state().is_suppressing_distribution() {
        return Err(EligibilityError::SelfRecoveryUnresolved);
    }
    let global_policy = state::global_config().global_policy;
    let sample = state::latest_sample(target).ok_or(EligibilityError::NoSample)?;
    if sample.timestamp_secs > now_secs {
        return Err(EligibilityError::FutureSample);
    }
    if now_secs.saturating_sub(sample.timestamp_secs) > global_policy.stale_after_secs() {
        return Err(EligibilityError::StaleSample);
    }
    if sample.state == PublicTargetState::Unreachable {
        return Err(EligibilityError::Unreachable);
    }
    let balance = sample
        .balance
        .as_ref()
        .and_then(AdvisoryCyclesBalance::low_balance_value)
        .ok_or(EligibilityError::Unreachable)?;
    let threshold = record.funding_policy().low_balance_threshold_cycles();
    if balance > threshold {
        return Err(EligibilityError::NotLow);
    }
    let reservation = state::get_target_reservation(target);
    if reservation.in_flight_operation_id().is_some() {
        return Err(EligibilityError::OperationInFlight);
    }
    if reservation.is_on_cooldown(now_secs) {
        return Err(EligibilityError::Cooldown);
    }
    Ok((record, balance))
}

/// Signer-gated manual top-up: the design's `manual_top_up(principal)`.
/// Enters the exact same `cycles::run_ordinary` path a future timer sweep
/// will use, with `FundingTrigger::ManualTopup` as the only difference in
/// input — it cannot bypass policy, caps, cooldown, or the reserve.
pub async fn manual_top_up_at(
    caller: Principal,
    now_secs: u64,
    now_ns: u64,
    target: Principal,
) -> Result<types::FundingOperation, cycles::FundingError> {
    if !state::is_signer(caller) {
        return Err(cycles::FundingError::NotSigner);
    }
    cycles::run_ordinary(target, types::FundingTrigger::ManualTopup, now_secs, now_ns).await
}

/// The Task 4 "separate refresh seam" (design requirement 4): queries the
/// Cycles Ledger's actual balance/fee and replaces Sentinel's cached
/// snapshot. `prepare`/`reserve_*` never do this themselves — see
/// `types::SourceReserveState::refresh`'s doc comment. Callable by tests
/// deterministically (`state::set_source_reserve` directly) and, later, by
/// the Task 6 sampler/timer.
pub async fn refresh_cycles_ledger_cache(
    now_secs: u64,
    sentinel_id: Principal,
) -> Result<(), cycles::FundingError> {
    cycles::require_sentinel_identity(ic_cdk::id(), sentinel_id)?;
    // Capture the durable funding-state generation BEFORE the first await.
    // Any reservation, attempt marker, settlement, or competing cache write
    // advances it; the returned query is then rejected instead of replacing
    // a newer source snapshot with stale balance data.
    let expected_generation = state::source_refresh_generation();
    let (balance_cycles, fee_cycles) =
        crate::cycles_ledger::query_balance_and_fee(types::cycles_ledger_principal(), sentinel_id)
            .await
            .map_err(cycles::FundingError::CacheQuery)?;
    state::commit_source_reserve_refresh(expected_generation, balance_cycles, fee_cycles, now_secs)
        .map_err(|err| match err {
            state::SourceRefreshCommitError::StaleGeneration => {
                cycles::FundingError::StaleCacheRefresh
            }
            state::SourceRefreshCommitError::SourceReserve(source) => {
                cycles::FundingError::SourceReserve(source)
            }
        })
}

/// The durable Cycles Ledger outbox executor: reservation precompute-then-
/// commit, `FundingOperation` open/submit, the `withdraw` await, and
/// outcome resolution. Shared by ordinary targets (`run_ordinary`, called
/// from `manual_top_up_at` above and, later, the timer's low-balance
/// sweep) and Sentinel's own self-recovery lane (`self_recovery.rs`, which
/// calls `execute` directly after its own `prepare`-equivalent — see that
/// file for why self-recovery's reservation precompute is NOT here: it
/// draws on `SelfRecoveryState` and the placeholder `TargetFundingPolicy`
/// construction, both self-recovery-specific concerns owned by that file).
pub mod cycles {
    use candid::{Nat, Principal};

    use crate::cycles_ledger::{self, QuarantinedCyclesEvidence, WithdrawOutcome};
    use crate::state;
    use crate::types::{
        self, AttachConfirmedBlockError, CyclesFundingState, CyclesWithdrawSnapshot,
        FundingAttemptResultClass, FundingOperation, FundingOperationOpenError,
        FundingOperationState, FundingOperationTransitionError, FundingRailArguments,
        FundingTrigger, RollingSpendReleaseError, RollingSpendReserveError,
        RollingSpendSettleError, SelfRecoveryReleaseError, SelfRecoverySettleError,
        SelfRecoveryStateError, SourceAttemptError, SourceReserveError, SourceSettleError,
        TargetReleaseError, TargetReservationError, TargetSettleError, TerminalFundingSummary,
        TerminalFundingSummaryError,
    };

    use super::EligibilityError;

    /// The design's per-target/global/self-recovery rolling-cap window: 24
    /// hours. A fixed protocol constant, not a governed policy input (the
    /// governed inputs are the CAP amounts themselves —
    /// `TargetFundingPolicy::daily_cap_cycles`,
    /// `GlobalPolicy::global_daily_cap_cycles`,
    /// `SelfRecoveryPolicy::daily_cap_cycles` — not the window length).
    pub(crate) const ROLLING_CAP_WINDOW_SECS: u64 = 86_400;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum FundingError {
        Eligibility(EligibilityError),
        NotSigner,
        NotFound,
        WrongRail,
        /// The operation has already `stops_automatic_retry()` — nothing
        /// left to resume (it is either resolved, or `Quarantined` awaiting
        /// a signer, not an automatic retry).
        NotResumable,
        /// A self-recovery operation or source-account query was given a
        /// caller-supplied principal other than this canister's actual
        /// identity. Never issue a withdrawal under an unverified owner.
        SentinelIdentityMismatch,
        /// The cache query completed after another reservation, attempt,
        /// settlement, or source-cache write changed funding state. Its
        /// result is stale and must be discarded.
        StaleCacheRefresh,
        /// A quarantined Cycles operation requires an explicit, independently
        /// verified reconciliation decision. A Duplicate reply alone is not
        /// such evidence.
        Reconciliation(ReconciliationError),
        CacheQuery(cycles_ledger::CacheQueryError),
        SourceAttempt(SourceAttemptError),
        SourceReserve(SourceReserveError),
        TargetReserve(TargetReservationError),
        GlobalReserve(RollingSpendReserveError),
        SelfRecoveryReserve(SelfRecoveryStateError),
        /// `refill_cycles.checked_add(fee_cycles)` overflowed `u128`.
        Overflow,
        Open(FundingOperationOpenError),
        Insert(state::InsertOperationError),
        Transition(FundingOperationTransitionError),
        Update(state::UpdateOperationError),
        MonotonicTime(state::MonotonicTimeError),
        AttachBlock(AttachConfirmedBlockError),
        TargetSettle(TargetSettleError),
        TargetRelease(TargetReleaseError),
        GlobalSettle(RollingSpendSettleError),
        GlobalRelease(RollingSpendReleaseError),
        SelfRecoverySettle(SelfRecoverySettleError),
        SelfRecoveryRelease(SelfRecoveryReleaseError),
        SourceSettle(SourceSettleError),
        TerminalSummary(TerminalFundingSummaryError),
        Compact(state::CompactOperationError),
        /// `self_recovery.rs`'s placeholder `TargetFundingPolicy` (built
        /// from `SelfRecoveryPolicy`'s own values via
        /// `TargetFundingPolicy::validate_self_contained`) failed its own
        /// self-contained bounds.
        PlaceholderPolicy(types::TargetValidationError),
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum ReconciliationError {
        NotQuarantined,
        /// Self-recovery remains suppressed until independent evidence proves
        /// the recovery delivered to Sentinel itself.
        SelfRecoveryDeliveryProofRequired,
        /// A caller supplied a known debit greater than the immutable amount
        /// plus fee held by this operation.
        KnownDebitExceedsHeld,
        /// A zero known debit must use the explicit `NoSpend` outcome so a
        /// fee/debit decision can never be silently weakened.
        ZeroKnownDebit,
    }

    /// Public/wiring callers must pass this canister's identity itself; a
    /// configuration value or user argument must never select the source
    /// account or self-recovery destination. Kept pure so tests can exercise
    /// the identity boundary without an IC execution context.
    pub(crate) fn require_sentinel_identity(
        actual: Principal,
        requested: Principal,
    ) -> Result<(), FundingError> {
        if actual == requested {
            Ok(())
        } else {
            Err(FundingError::SentinelIdentityMismatch)
        }
    }

    /// Precomputes every fallible value this prepare path needs — the three
    /// reservation VALUES, and the fully-constructed, already-`Submitted`
    /// operation VALUE — before committing ANY of them, then commits in an
    /// order chosen so that the riskiest-to-orphan write (the operation
    /// itself existing in `FUNDING_OPERATIONS`) happens first: if
    /// `insert_operation` fails, no reservation has been written yet; the
    /// reservation writes themselves are then infallible `state::set_*`
    /// calls using values already validated as `Ok` (correction pass,
    /// atomicity review Finding 2 — see the module doc for the full
    /// "no target/global/self/source reservation and no operation may be
    /// partially added" requirement this satisfies). Only
    /// `state::next_operation_id`/`next_created_at_time_ns`'s own monotonic
    /// counter advances are allowed to be "spent" on a later failure — an
    /// explicitly accepted, documented, non-corrupting gap (design:
    /// "Counter gaps are acceptable if documented and monotonic").
    fn prepare_ordinary(
        target_principal: Principal,
        trigger: FundingTrigger,
        now_secs: u64,
        now_ns: u64,
    ) -> Result<FundingOperation, FundingError> {
        let (record, _observed_balance) = super::check_ordinary_eligibility(
            target_principal,
            now_secs,
            trigger == FundingTrigger::ManualTopup,
        )
        .map_err(FundingError::Eligibility)?;

        let global_policy = state::global_config().global_policy;
        let refill_cycles = record.funding_policy().refill_cycles();

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

        let target_reservation = state::get_target_reservation(target_principal)
            .reserve(
                operation_id,
                refill_cycles,
                now_secs,
                ROLLING_CAP_WINDOW_SECS,
                record.funding_policy().daily_cap_cycles(),
            )
            .map_err(FundingError::TargetReserve)?;
        let global_reservation = state::get_global_rolling_spend()
            .reserve(
                operation_id,
                refill_cycles,
                now_secs,
                ROLLING_CAP_WINDOW_SECS,
                global_policy.global_daily_cap_cycles(),
            )
            .map_err(FundingError::GlobalReserve)?;
        let protected_reserve = global_policy
            .self_recovery_policy()
            .protected_reserve_cycles();
        let max_cache_age = global_policy.stale_after_secs();
        let source_reservation = state::get_source_reserve()
            .reserve_ordinary(
                operation_id,
                amount_plus_fee,
                protected_reserve,
                now_secs,
                max_cache_age,
            )
            .map_err(FundingError::SourceReserve)?;

        let created_at_time_ns =
            state::next_created_at_time_ns(now_ns).map_err(FundingError::MonotonicTime)?;
        let snapshot = CyclesWithdrawSnapshot {
            destination: target_principal,
            from_subaccount: None,
            amount_cycles: refill_cycles,
            fee_cycles,
            created_at_time_ns,
        };
        let op = FundingOperation::open(
            operation_id,
            target_principal,
            record.revision(),
            record.funding_policy().clone(),
            trigger,
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
        // `insert_operation` first: if it fails (structurally
        // near-impossible here — fresh id, live target, registry headroom
        // already implied by the reservation checks above — but still a
        // typed `Result`), nothing below has run yet, so no reservation is
        // ever left orphaned by an operation that was never persisted.
        // Persist the already-submitted value directly.  There is no
        // fallible "final prepare" update after the reservation writes: a
        // crash or typed error cannot leave a PlannedReserved operation
        // paired with committed reservations, and all writes after this
        // insert are infallible stable-cell/map replacements.
        state::insert_operation(submitted.clone()).map_err(FundingError::Insert)?;
        state::set_target_reservation(target_principal, target_reservation);
        state::set_global_rolling_spend(global_reservation);
        state::set_source_reserve(source_reservation);
        Ok(submitted)
    }

    /// Reconstructs the exact `WithdrawArgs` from the operation's persisted,
    /// immutable `CyclesWithdrawSnapshot` and calls the ledger — used both
    /// for a freshly-submitted operation and for an exact retry of an
    /// `Unknown` one (design: "Exact retry reconstructs byte-equivalent
    /// WithdrawArgs from the persisted snapshot"). `pub(crate)` so
    /// `self_recovery.rs` can call it directly after its own prepare step.
    pub(crate) async fn execute(
        op: FundingOperation,
        now_secs: u64,
    ) -> Result<FundingOperation, FundingError> {
        let FundingRailArguments::Cycles(snapshot) = op.rail_arguments().clone() else {
            return Err(FundingError::WrongRail);
        };
        if op.trigger() == FundingTrigger::SelfRecovery {
            require_sentinel_identity(ic_cdk::id(), snapshot.destination)?;
        }
        // A previous call may have durably recorded the ledger's confirmed
        // block and then been interrupted before completion. Recover from
        // that proof without issuing a duplicate external call.
        if matches!(
            op.state(),
            FundingOperationState::Cycles(CyclesFundingState::Confirmed)
        ) && op.confirmed_block_index().is_some()
        {
            return complete_confirmed_operation(op, now_secs);
        }
        // Mark the source debit before the await. Refreshes are rejected
        // while this marker is present, so a post-call cache observation can
        // never be mistaken for a pre-call snapshot and debited twice.
        state::mark_source_attempt(op.id(), now_secs).map_err(FundingError::SourceAttempt)?;
        let args = cycles_ledger::WithdrawArgs {
            amount: Nat::from(snapshot.amount_cycles),
            from_subaccount: snapshot.from_subaccount.as_ref().map(|s| s.to_vec()),
            to: snapshot.destination,
            created_at_time: Some(snapshot.created_at_time_ns),
        };
        let outcome = cycles_ledger::withdraw(types::cycles_ledger_principal(), args).await;
        resolve_operation(op, outcome, now_secs)
    }

    /// `prepare_ordinary` then `execute` — the manual/timer-shared entry
    /// point for a brand-new ordinary funding attempt.
    pub async fn run_ordinary(
        target: Principal,
        trigger: FundingTrigger,
        now_secs: u64,
        now_ns: u64,
    ) -> Result<FundingOperation, FundingError> {
        let op = prepare_ordinary(target, trigger, now_secs, now_ns)?;
        execute(op, now_secs).await
    }

    /// Retries an already-open, still-retryable Cycles-rail operation
    /// (ordinary or self-recovery) by id — the primitive a later timer's
    /// "resume pending operations" step loops over. Rejects an operation
    /// that has already stopped automatic retry (resolved, or
    /// `Quarantined` awaiting a signer).
    pub async fn resume(
        operation_id: u64,
        now_secs: u64,
    ) -> Result<FundingOperation, FundingError> {
        let op = state::get_operation(operation_id).ok_or(FundingError::NotFound)?;
        if op.rail() != types::FundingRail::CyclesLedger {
            return Err(FundingError::WrongRail);
        }
        if op.state().stops_automatic_retry() {
            return Err(FundingError::NotResumable);
        }
        execute(op, now_secs).await
    }

    /// Resolves a quarantined Cycles operation only from an explicit,
    /// independently verified outcome. `Duplicate` is intentionally absent
    /// from `QuarantinedCyclesEvidence`: the pinned ledger records before its
    /// management-canister deposit, so that reply alone cannot prove delivery.
    ///
    /// This is the bounded core path for the future Task 6 signer-gated
    /// reconciliation endpoint. It conservatively settles source balance and
    /// target/global/self-recovery caps using the explicit evidence. For
    /// self-recovery, only `Delivered { block_index }` is accepted; until that
    /// proof arrives, the unresolved operation and suppression remain intact.
    pub(crate) fn reconcile_quarantined_cycles(
        operation_id: u64,
        evidence: QuarantinedCyclesEvidence,
        now_secs: u64,
    ) -> Result<FundingOperation, FundingError> {
        let op = state::get_operation(operation_id).ok_or(FundingError::NotFound)?;
        if op.rail() != types::FundingRail::CyclesLedger {
            return Err(FundingError::WrongRail);
        }
        if op.state() != FundingOperationState::Cycles(CyclesFundingState::Quarantined) {
            return Err(FundingError::Reconciliation(
                ReconciliationError::NotQuarantined,
            ));
        }
        if op.trigger() == FundingTrigger::SelfRecovery
            && !matches!(evidence, QuarantinedCyclesEvidence::Delivered { .. })
        {
            return Err(FundingError::Reconciliation(
                ReconciliationError::SelfRecoveryDeliveryProofRequired,
            ));
        }
        let FundingRailArguments::Cycles(snapshot) = op.rail_arguments().clone() else {
            return Err(FundingError::WrongRail);
        };
        let held = snapshot
            .amount_cycles
            .checked_add(snapshot.fee_cycles)
            .ok_or(FundingError::Overflow)?;
        let (resolved, delivered, known_spent) = match evidence {
            QuarantinedCyclesEvidence::Delivered { block_index } => (
                op.reconcile_quarantined_cycles(
                    CyclesFundingState::Complete,
                    Some(block_index),
                    now_secs,
                )
                .map_err(FundingError::Transition)?,
                true,
                held,
            ),
            QuarantinedCyclesEvidence::NoSpend => (
                op.reconcile_quarantined_cycles(CyclesFundingState::Terminal, None, now_secs)
                    .map_err(FundingError::Transition)?,
                false,
                0,
            ),
            QuarantinedCyclesEvidence::FeeDebited { known_spent_cycles } => {
                if known_spent_cycles == 0 {
                    return Err(FundingError::Reconciliation(
                        ReconciliationError::ZeroKnownDebit,
                    ));
                }
                if known_spent_cycles > held {
                    return Err(FundingError::Reconciliation(
                        ReconciliationError::KnownDebitExceedsHeld,
                    ));
                }
                (
                    op.reconcile_quarantined_cycles(CyclesFundingState::Terminal, None, now_secs)
                        .map_err(FundingError::Transition)?,
                    false,
                    known_spent_cycles,
                )
            }
        };
        let (settlement, source) = compute_settlement(&resolved, delivered, known_spent, now_secs)?;
        let summary = TerminalFundingSummary::from_resolved(&resolved, now_secs)
            .map_err(FundingError::TerminalSummary)?;
        state::update_operation(resolved.clone()).map_err(FundingError::Update)?;
        commit_settlement(settlement, source);
        state::compact_operation(resolved.id(), summary).map_err(FundingError::Compact)?;
        Ok(resolved)
    }

    /// The precomputed, not-yet-committed result of settling every
    /// reservation a resolved/terminal operation holds — target+global
    /// (ordinary) or the self-recovery ledger, ALWAYS plus the shared
    /// `SOURCE_RESERVE` debit regardless of trigger (design: "Self-recovery
    /// uses the same shared source account reservation"). Computed by
    /// `compute_settlement` (pure, no `state::set_*` calls) and applied
    /// atomically by `commit_settlement` — the split that fixes the
    /// correction pass's CRITICAL atomicity finding: every fallible
    /// settlement calculation is known-good BEFORE the operation's own
    /// resolved state, or any reservation, is ever persisted.
    enum Settlement {
        Ordinary {
            target: Principal,
            target_reservation: types::TargetReservationState,
            global_reservation: types::GlobalRollingSpendState,
        },
        SelfRecovery(types::SelfRecoveryState),
    }

    /// Precomputes the settlement `op` (already advanced, in-memory, to its
    /// final `Complete`/`Terminal` state — see `resolve_operation`) would
    /// require, WITHOUT writing anything to stable storage. `delivered`
    /// selects settle-with-cooldown (target/global/self-recovery) vs.
    /// release-no-spend; `source_known_spent_cycles` is the amount known to
    /// have actually left the Cycles Ledger account (full amount+fee on
    /// delivery, the known net debit on a proven fee-affecting terminal
    /// failure, or `0` for a proven clean no-spend — see
    /// `cycles_ledger::WithdrawOutcome`'s doc comment for exactly which
    /// case is which).
    fn compute_settlement(
        op: &FundingOperation,
        delivered: bool,
        source_known_spent_cycles: u128,
        now_secs: u64,
    ) -> Result<(Settlement, types::SourceReserveState), FundingError> {
        let reservation = if op.trigger() == FundingTrigger::SelfRecovery {
            let self_recovery = state::get_self_recovery_state();
            let updated = if delivered {
                self_recovery
                    .complete(op.id(), now_secs, ROLLING_CAP_WINDOW_SECS)
                    .map_err(FundingError::SelfRecoverySettle)?
            } else {
                self_recovery
                    .release_no_spend(op.id())
                    .map_err(FundingError::SelfRecoveryRelease)?
            };
            Settlement::SelfRecovery(updated)
        } else {
            let target_reservation = state::get_target_reservation(op.target());
            let updated_target = if delivered {
                target_reservation
                    .settle_spend(
                        op.id(),
                        now_secs,
                        op.funding_policy().cooldown_secs(),
                        ROLLING_CAP_WINDOW_SECS,
                    )
                    .map_err(FundingError::TargetSettle)?
            } else {
                target_reservation
                    .release_no_spend(op.id())
                    .map_err(FundingError::TargetRelease)?
            };

            let global_reservation = state::get_global_rolling_spend();
            let updated_global = if delivered {
                global_reservation
                    .settle(op.id(), now_secs, ROLLING_CAP_WINDOW_SECS)
                    .map_err(FundingError::GlobalSettle)?
            } else {
                global_reservation
                    .release_no_spend(op.id())
                    .map_err(FundingError::GlobalRelease)?
            };
            Settlement::Ordinary {
                target: op.target(),
                target_reservation: updated_target,
                global_reservation: updated_global,
            }
        };

        let source = state::get_source_reserve();
        let updated_source = source
            .settle(op.id(), source_known_spent_cycles)
            .map_err(FundingError::SourceSettle)?;
        Ok((reservation, updated_source))
    }

    /// Applies an already-precomputed `Settlement` plus source-reserve
    /// value. Every call site (`state::set_*`) here is an infallible
    /// `RefCell` write against values already validated `Ok` by
    /// `compute_settlement` — this function cannot itself fail.
    fn commit_settlement(reservation: Settlement, source: types::SourceReserveState) {
        match reservation {
            Settlement::Ordinary {
                target,
                target_reservation,
                global_reservation,
            } => {
                state::set_target_reservation(target, target_reservation);
                state::set_global_rolling_spend(global_reservation);
            }
            Settlement::SelfRecovery(self_recovery) => {
                state::set_self_recovery_state(self_recovery);
            }
        }
        state::set_source_reserve(source);
    }

    /// Completes a Cycles Ledger operation whose `Confirmed` state and block
    /// proof were durably persisted before an upgrade/trap interrupted the
    /// original call. The persisted proof is authoritative; no second
    /// ledger call is made and any caller-supplied retry outcome is ignored.
    /// Settlement is recomputed from the immutable snapshot and committed
    /// exactly once at this operation boundary.
    fn complete_confirmed_operation(
        op: FundingOperation,
        now_secs: u64,
    ) -> Result<FundingOperation, FundingError> {
        let FundingRailArguments::Cycles(snapshot) = op.rail_arguments().clone() else {
            return Err(FundingError::WrongRail);
        };
        let complete = op
            .record_attempt_with_bounded_compaction(
                FundingOperationState::Cycles(CyclesFundingState::Complete),
                now_secs,
                FundingAttemptResultClass::Success,
            )
            .map_err(FundingError::Transition)?;
        let known_spent = snapshot
            .amount_cycles
            .checked_add(snapshot.fee_cycles)
            .ok_or(FundingError::Overflow)?;
        let (settlement, source) = compute_settlement(&complete, true, known_spent, now_secs)?;
        let summary = TerminalFundingSummary::from_resolved(&complete, now_secs)
            .map_err(FundingError::TerminalSummary)?;

        state::update_operation(complete.clone()).map_err(FundingError::Update)?;
        commit_settlement(settlement, source);
        state::compact_operation(complete.id(), summary).map_err(FundingError::Compact)?;
        Ok(complete)
    }

    fn raise_quarantine_alarm(op: &FundingOperation, now_secs: u64) {
        if op.trigger() == FundingTrigger::SelfRecovery {
            let _ =
                state::alarms::raise_at(None, types::AlarmKind::SelfRecoveryUnresolved, now_secs);
        } else {
            let _ = state::alarms::raise_at(
                Some(op.target()),
                types::AlarmKind::FundingQuarantined,
                now_secs,
            );
        }
    }

    /// The single place a `WithdrawOutcome` is turned into a persisted
    /// state transition and (when resolved) reservation settlement plus
    /// compaction — see the module doc's result-handling summary. Entirely
    /// synchronous: given an already-open `op` and a `WithdrawOutcome`
    /// value, this performs no inter-canister calls itself, which is what
    /// makes it directly unit-testable without a live ledger.
    ///
    /// **Atomicity (correction pass, CRITICAL fix).** For every branch that
    /// resolves the operation (`Confirmed`/`TerminalNoSpend`/
    /// `TerminalFeeDebited`/`TerminalFullAmountDebited`), the FULL final
    /// in-memory operation value, its settlement (`compute_settlement`), and
    /// its terminal summary are all computed and validated as `Ok` BEFORE
    /// `state::update_operation` ever persists the resolved state. If any of
    /// those precompute steps fails, this function returns `Err` having
    /// written nothing at all — the operation stays exactly as it was
    /// passed in (whatever `Submitted`/`Unknown` state it was already
    /// persisted at), and every reservation is untouched. Only once
    /// settlement and the summary are both known-good does this function
    /// begin committing: persist the resolved operation, then the
    /// settlement, then compact — mirroring `prepare_ordinary`'s own
    /// precompute-then-commit discipline. `Unknown`/`Quarantined` never
    /// reach any of this: they only ever persist a state transition, never
    /// touch a reservation, and are therefore inherently safe to write
    /// immediately.
    pub(crate) fn resolve_operation(
        op: FundingOperation,
        outcome: WithdrawOutcome,
        now_secs: u64,
    ) -> Result<FundingOperation, FundingError> {
        let FundingRailArguments::Cycles(snapshot) = op.rail_arguments().clone() else {
            return Err(FundingError::WrongRail);
        };
        // `execute` normally catches this path before calling the ledger, but
        // keep the resolver idempotent for upgrade/reconciliation code that
        // invokes it directly with a persisted proof and any stale outcome.
        if matches!(
            op.state(),
            FundingOperationState::Cycles(CyclesFundingState::Confirmed)
        ) && op.confirmed_block_index().is_some()
        {
            return complete_confirmed_operation(op, now_secs);
        }
        match outcome {
            WithdrawOutcome::Confirmed(block) => {
                // ── Precompute: fully advance `op` in memory, then compute
                // its settlement and summary. Nothing is written yet. ──
                let confirmed = op
                    .record_attempt_with_bounded_compaction(
                        FundingOperationState::Cycles(CyclesFundingState::Confirmed),
                        now_secs,
                        FundingAttemptResultClass::Success,
                    )
                    .map_err(FundingError::Transition)?
                    .attach_confirmed_block(block)
                    .map_err(FundingError::AttachBlock)?;
                let complete = confirmed
                    .record_attempt_with_bounded_compaction(
                        FundingOperationState::Cycles(CyclesFundingState::Complete),
                        now_secs,
                        FundingAttemptResultClass::Success,
                    )
                    .map_err(FundingError::Transition)?;
                let known_spent = snapshot
                    .amount_cycles
                    .checked_add(snapshot.fee_cycles)
                    .ok_or(FundingError::Overflow)?;
                let (settlement, source) =
                    compute_settlement(&complete, true, known_spent, now_secs)?;
                let summary = TerminalFundingSummary::from_resolved(&complete, now_secs)
                    .map_err(FundingError::TerminalSummary)?;

                // ── Commit: everything above is known-good. ──
                state::update_operation(confirmed).map_err(FundingError::Update)?;
                state::update_operation(complete.clone()).map_err(FundingError::Update)?;
                commit_settlement(settlement, source);
                state::compact_operation(complete.id(), summary).map_err(FundingError::Compact)?;
                Ok(complete)
            }
            WithdrawOutcome::Duplicate(_duplicate_block) => {
                // The pinned Cycles Ledger records a transaction/hash before
                // attempting the management-canister deposit.  Its
                // `Duplicate` reply therefore proves only that an earlier
                // request was recorded, not that delivery succeeded.  Do
                // not settle any reservation or attach the duplicate block
                // as delivery proof; retain the operation for signer-led
                // reconciliation instead.
                let quarantined = if op.attempts().len() >= types::MAX_FUNDING_ATTEMPTS - 1 {
                    op.quarantine_after_attempt_limit(now_secs)
                } else {
                    op.record_attempt(
                        FundingOperationState::Cycles(CyclesFundingState::Quarantined),
                        now_secs,
                        FundingAttemptResultClass::Indeterminate,
                    )
                }
                .map_err(FundingError::Transition)?;
                state::update_operation(quarantined.clone()).map_err(FundingError::Update)?;
                raise_quarantine_alarm(&quarantined, now_secs);
                Ok(quarantined)
            }
            WithdrawOutcome::Unknown => {
                // Never consume the final append slot with another
                // ambiguous attempt.  Quarantine in that slot (or replace
                // the terminal record for an already-full legacy history)
                // so the operation remains bounded and signer-resolvable.
                let unknown = if op.attempts().len() >= types::MAX_FUNDING_ATTEMPTS - 1 {
                    op.quarantine_after_attempt_limit(now_secs)
                } else {
                    op.record_attempt(
                        FundingOperationState::Cycles(CyclesFundingState::Unknown),
                        now_secs,
                        FundingAttemptResultClass::Indeterminate,
                    )
                }
                .map_err(FundingError::Transition)?;
                state::update_operation(unknown.clone()).map_err(FundingError::Update)?;
                if matches!(
                    unknown.state(),
                    FundingOperationState::Cycles(CyclesFundingState::Quarantined)
                ) {
                    raise_quarantine_alarm(&unknown, now_secs);
                }
                // Ambiguous: every reservation stays exactly as-is.
                Ok(unknown)
            }
            WithdrawOutcome::Quarantined => {
                let quarantined = if op.attempts().len() >= types::MAX_FUNDING_ATTEMPTS - 1 {
                    op.quarantine_after_attempt_limit(now_secs)
                } else {
                    op.record_attempt(
                        FundingOperationState::Cycles(CyclesFundingState::Quarantined),
                        now_secs,
                        FundingAttemptResultClass::Indeterminate,
                    )
                }
                .map_err(FundingError::Transition)?;
                state::update_operation(quarantined.clone()).map_err(FundingError::Update)?;
                raise_quarantine_alarm(&quarantined, now_secs);
                // Still reserved: needs a signer, never falls through to ICP.
                Ok(quarantined)
            }
            WithdrawOutcome::TerminalNoSpend => {
                let terminal = op
                    .record_attempt(
                        FundingOperationState::Cycles(CyclesFundingState::Terminal),
                        now_secs,
                        FundingAttemptResultClass::TerminalFailure,
                    )
                    .map_err(FundingError::Transition)?;
                let (settlement, source) = compute_settlement(&terminal, false, 0, now_secs)?;
                let summary = TerminalFundingSummary::from_resolved(&terminal, now_secs)
                    .map_err(FundingError::TerminalSummary)?;

                state::update_operation(terminal.clone()).map_err(FundingError::Update)?;
                commit_settlement(settlement, source);
                state::compact_operation(terminal.id(), summary).map_err(FundingError::Compact)?;
                Ok(terminal)
            }
            WithdrawOutcome::TerminalFeeDebited { fee_block: _ } => {
                // Pinned-ledger accounting (correction pass, ledger-security
                // review): `fee_block: Some(_)` proves a KNOWN net debit of
                // `2 * fee` (burn amount+fee, refund only amount-fee as a
                // penalty) — see `WithdrawOutcome::TerminalFeeDebited`'s doc
                // comment for the full evidence trail.
                let terminal = op
                    .record_attempt(
                        FundingOperationState::Cycles(CyclesFundingState::Terminal),
                        now_secs,
                        FundingAttemptResultClass::TerminalFailure,
                    )
                    .map_err(FundingError::Transition)?;
                let known_spent = snapshot
                    .fee_cycles
                    .checked_mul(2)
                    .ok_or(FundingError::Overflow)?;
                let (settlement, source) =
                    compute_settlement(&terminal, false, known_spent, now_secs)?;
                let summary = TerminalFundingSummary::from_resolved(&terminal, now_secs)
                    .map_err(FundingError::TerminalSummary)?;

                state::update_operation(terminal.clone()).map_err(FundingError::Update)?;
                commit_settlement(settlement, source);
                state::compact_operation(terminal.id(), summary).map_err(FundingError::Compact)?;
                Ok(terminal)
            }
            WithdrawOutcome::TerminalFullAmountDebited => {
                // Pinned-ledger accounting: `fee_block: None` proves the
                // FULL held `amount + fee` is gone with no refund at all
                // (`amount <= fee`) — see the same doc comment.
                let terminal = op
                    .record_attempt(
                        FundingOperationState::Cycles(CyclesFundingState::Terminal),
                        now_secs,
                        FundingAttemptResultClass::TerminalFailure,
                    )
                    .map_err(FundingError::Transition)?;
                let known_spent = snapshot
                    .amount_cycles
                    .checked_add(snapshot.fee_cycles)
                    .ok_or(FundingError::Overflow)?;
                let (settlement, source) =
                    compute_settlement(&terminal, false, known_spent, now_secs)?;
                let summary = TerminalFundingSummary::from_resolved(&terminal, now_secs)
                    .map_err(FundingError::TerminalSummary)?;

                state::update_operation(terminal.clone()).map_err(FundingError::Update)?;
                commit_settlement(settlement, source);
                state::compact_operation(terminal.id(), summary).map_err(FundingError::Compact)?;
                Ok(terminal)
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::types::{
            AdvisoryCyclesBalance, Criticality, Environment, GlobalPolicy, GlobalPolicyArgs,
            GovernanceTimelocksArgs, InitArgs, ObservationMode, PublicTargetState, Sample,
            SelfRecoveryPolicyArgs, TargetArgs, TargetFundingPolicyArgs, TargetPatch,
            TargetRegistrationContext,
        };
        use std::collections::BTreeSet;

        fn sentinel_id() -> Principal {
            Principal::from_slice(&[9, 9, 9])
        }

        fn target_principal(seed: u8) -> Principal {
            Principal::from_slice(&[seed, 1, 2, 3])
        }

        fn test_governance_timelocks_args() -> GovernanceTimelocksArgs {
            GovernanceTimelocksArgs {
                target_registry_secs: 1,
                spend_policy_secs: 1,
                signer_change_secs: 1,
                unpause_secs: 1,
            }
        }

        fn test_self_recovery_args(
            protected_reserve: u128,
            cap: u128,
            threshold: u128,
            refill: u128,
        ) -> SelfRecoveryPolicyArgs {
            SelfRecoveryPolicyArgs {
                protected_reserve_cycles: Nat::from(protected_reserve),
                daily_cap_cycles: Nat::from(cap),
                low_balance_threshold_cycles: Nat::from(threshold),
                refill_cycles: Nat::from(refill),
            }
        }

        fn test_global_policy(daily_cap: u128, stale_after_secs: u64) -> GlobalPolicy {
            GlobalPolicy::validate(&GlobalPolicyArgs {
                global_daily_cap_cycles: Nat::from(daily_cap),
                sample_interval_secs: 60,
                stale_after_secs,
                min_icp_reserve_e8s: Nat::from(0u32),
                timelocks: test_governance_timelocks_args(),
                self_recovery_policy: test_self_recovery_args(1_000, 500, 1, 10),
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

        fn register_and_prime_target(
            seed: u8,
            global: &GlobalPolicy,
            threshold: u128,
            refill: u128,
            cap: u128,
            cooldown_secs: u64,
        ) -> Principal {
            let principal = target_principal(seed);
            let empty = BTreeSet::new();
            let ctx = TargetRegistrationContext {
                sentinel_id: sentinel_id(),
                existing_target_count: 0,
                existing_target_principals: &empty,
                global_policy: global,
            };
            let args = TargetArgs {
                principal,
                display_name: "svc".to_string(),
                project: "proj".to_string(),
                environment: Environment::Production,
                criticality: Criticality::Standard,
                observation_mode: ObservationMode::SelfReport,
                tags: vec![],
                funding_policy: TargetFundingPolicyArgs {
                    low_balance_threshold_cycles: Nat::from(threshold),
                    refill_cycles: Nat::from(refill),
                    daily_cap_cycles: Nat::from(cap),
                    cooldown_secs,
                    burn_anomaly_limit_cycles_per_day: None,
                },
            };
            let mut record = types::TargetRecord::register(args, &ctx).unwrap();
            record = record
                .apply_patch(
                    TargetPatch {
                        display_name: None,
                        project: None,
                        environment: None,
                        criticality: None,
                        observation_mode: None,
                        tags: None,
                        funding_policy: None,
                        enabled: Some(true),
                        auto_topup: Some(true),
                    },
                    global,
                )
                .unwrap();
            state::insert_target(record).unwrap();
            principal
        }

        fn seed_fresh_cache(balance: u128, fee: u128, now_secs: u64) {
            let cache = state::get_source_reserve()
                .refresh(balance, fee, now_secs)
                .unwrap();
            state::set_source_reserve(cache);
        }

        fn sample(secs: u64, balance: u128, state_: PublicTargetState) -> Sample {
            Sample {
                timestamp_secs: secs,
                balance: Some(AdvisoryCyclesBalance::Exact(balance)),
                state: state_,
                reported_operational_healthy: None,
                burn_cycles_per_hour: None,
            }
        }

        // ─── prepare_ordinary: reservation visible before adapter invocation ───

        #[test]
        fn prepare_ordinary_persists_planned_reserved_then_submitted_before_any_await() {
            let global = test_global_policy(1_000_000, 600);
            init_test_state(global.clone());
            let target = register_and_prime_target(1, &global, 100, 10, 1_000, 60);
            state::record_sample(target, sample(1_000, 5, PublicTargetState::Low)).unwrap();
            seed_fresh_cache(1_000_000, 1, 1_000);

            let op = prepare_ordinary(
                target,
                FundingTrigger::LowBalanceAutoTopup,
                1_000,
                1_000_000_000_000,
            )
            .unwrap();
            assert_eq!(
                op.state(),
                FundingOperationState::Cycles(CyclesFundingState::Submitted)
            );
            // Visible in FUNDING_OPERATIONS — this is the durable evidence
            // required BEFORE the ledger is ever called.
            assert_eq!(state::get_operation(op.id()), Some(op.clone()));
            // Target/global/source reservations are all already committed.
            assert_eq!(
                state::get_target_reservation(target).in_flight_operation_id(),
                Some(op.id())
            );
            assert!(state::get_global_rolling_spend()
                .rolling_spend()
                .pending()
                .iter()
                .any(|p| p.operation_id == op.id()));
            assert!(state::get_source_reserve()
                .pending()
                .iter()
                .any(|p| p.operation_id == op.id()));
        }

        #[test]
        fn prepare_ordinary_fails_closed_with_unknown_cache_and_reserves_nothing() {
            let global = test_global_policy(1_000_000, 600);
            init_test_state(global.clone());
            let target = register_and_prime_target(1, &global, 100, 10, 1_000, 60);
            state::record_sample(target, sample(1_000, 5, PublicTargetState::Low)).unwrap();
            // No `seed_fresh_cache` call: the cache is unknown.
            assert_eq!(
                prepare_ordinary(target, FundingTrigger::LowBalanceAutoTopup, 1_000, 0),
                Err(FundingError::SourceReserve(
                    SourceReserveError::UnknownCache
                ))
            );
            assert!(state::get_target_reservation(target)
                .in_flight_operation_id()
                .is_none());
            assert!(state::get_global_rolling_spend()
                .rolling_spend()
                .pending()
                .is_empty());
        }

        #[test]
        fn prepare_ordinary_fails_closed_with_stale_cache() {
            let global = test_global_policy(1_000_000, 600);
            init_test_state(global.clone());
            let target = register_and_prime_target(1, &global, 100, 10, 1_000, 60);
            // Sample is fresh (within 600s of `now_secs`); only the cache is stale.
            state::record_sample(target, sample(999_900, 5, PublicTargetState::Low)).unwrap();
            seed_fresh_cache(1_000_000, 1, 0);
            assert_eq!(
                prepare_ordinary(target, FundingTrigger::LowBalanceAutoTopup, 1_000_000, 0),
                Err(FundingError::SourceReserve(SourceReserveError::StaleCache))
            );
        }

        #[test]
        fn prepare_ordinary_rejects_future_dated_target_sample() {
            let global = test_global_policy(1_000_000, 600);
            init_test_state(global.clone());
            let target = register_and_prime_target(1, &global, 100, 10, 1_000, 60);
            // The sample reports a low balance, but its timestamp is after
            // the caller's current time. It is not valid evidence for a
            // funding decision.
            state::record_sample(target, sample(1_001, 5, PublicTargetState::Low)).unwrap();
            seed_fresh_cache(1_000_000, 0, 1_000);
            assert_eq!(
                prepare_ordinary(target, FundingTrigger::ManualTopup, 1_000, 0),
                Err(FundingError::Eligibility(EligibilityError::FutureSample))
            );
            assert!(state::get_target_reservation(target)
                .in_flight_operation_id()
                .is_none());
        }

        #[test]
        fn prepare_ordinary_rejects_second_call_while_reservation_in_flight_timer_manual_race() {
            let global = test_global_policy(1_000_000, 600);
            init_test_state(global.clone());
            let target = register_and_prime_target(1, &global, 100, 10, 1_000, 60);
            state::record_sample(target, sample(1_000, 5, PublicTargetState::Low)).unwrap();
            seed_fresh_cache(1_000_000, 0, 1_000);

            let first = prepare_ordinary(target, FundingTrigger::ManualTopup, 1_000, 0).unwrap();
            // A concurrent timer-triggered attempt for the SAME target must
            // be rejected by the reservation itself, before it ever reaches
            // its own await — proving manual and timer callers cannot race
            // past the shared reservation.
            assert_eq!(
                prepare_ordinary(target, FundingTrigger::LowBalanceAutoTopup, 1_000, 0),
                Err(FundingError::Eligibility(
                    EligibilityError::OperationInFlight
                ))
            );
            // Only one operation was ever opened.
            assert_eq!(state::get_operation(first.id()), Some(first));
        }

        #[test]
        fn prepare_ordinary_snapshot_survives_a_concurrent_target_edit() {
            let global = test_global_policy(1_000_000, 600);
            init_test_state(global.clone());
            let target = register_and_prime_target(1, &global, 100, 10, 1_000, 60);
            state::record_sample(target, sample(1_000, 5, PublicTargetState::Low)).unwrap();
            seed_fresh_cache(1_000_000, 0, 1_000);
            let op = prepare_ordinary(target, FundingTrigger::ManualTopup, 1_000, 0).unwrap();

            // Governance edits the target's display name after the
            // operation is already open (revision advances).
            let edited = state::get_target(target)
                .unwrap()
                .apply_patch(
                    TargetPatch {
                        display_name: Some("renamed".to_string()),
                        project: None,
                        environment: None,
                        criticality: None,
                        observation_mode: None,
                        tags: None,
                        funding_policy: None,
                        enabled: None,
                        auto_topup: None,
                    },
                    &global,
                )
                .unwrap();
            state::insert_target(edited).unwrap();

            // The already-open operation's own immutable snapshot is
            // unaffected: same revision, same destination, same amount.
            let stored = state::get_operation(op.id()).unwrap();
            assert_eq!(
                stored.target_registry_revision(),
                op.target_registry_revision()
            );
            assert_eq!(stored.rail_arguments(), op.rail_arguments());
        }

        /// Correction pass, atomicity review Finding 3: a forced LATE
        /// prepare failure (here, `insert_operation`'s `TooManyOperations`,
        /// deliberately triggered after every reservation VALUE has already
        /// been computed and validated as `Ok`) must not leave any
        /// target/global/source reservation committed with no
        /// corresponding operation.
        #[test]
        fn prepare_ordinary_forced_late_insert_failure_leaves_no_orphaned_reservation() {
            let global = test_global_policy(1_000_000, 600);
            init_test_state(global.clone());
            let target = register_and_prime_target(1, &global, 100, 10, 1_000, 60);
            state::record_sample(target, sample(1_000, 5, PublicTargetState::Low)).unwrap();
            seed_fresh_cache(1_000_000, 0, 1_000);

            // Fill FUNDING_OPERATIONS to its bound with synthetic
            // self-recovery-triggered filler operations (which bypass the
            // registered-target check `insert_operation` also enforces),
            // so the real `prepare_ordinary` call below reaches
            // `insert_operation` only to be rejected with
            // `TooManyOperations` — a genuine, forced late failure, not a
            // hypothetical one.
            for i in 0..(types::MAX_FUNDING_OPERATIONS as u64) {
                let snapshot = CyclesWithdrawSnapshot {
                    destination: sentinel_id(),
                    from_subaccount: None,
                    amount_cycles: 1,
                    fee_cycles: 0,
                    created_at_time_ns: i,
                };
                let policy =
                    types::TargetFundingPolicy::validate_self_contained(&TargetFundingPolicyArgs {
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
                prepare_ordinary(target, FundingTrigger::ManualTopup, 1_000, 0),
                Err(FundingError::Insert(
                    state::InsertOperationError::TooManyOperations
                ))
            );
            // No reservation was left orphaned by the failed insert.
            assert!(state::get_target_reservation(target)
                .in_flight_operation_id()
                .is_none());
            assert!(state::get_global_rolling_spend()
                .rolling_spend()
                .pending()
                .is_empty());
            assert!(state::get_source_reserve().pending().is_empty());
        }

        // ─── resolve_operation: exact retry, Duplicate, TooOld, rejection, ambiguous ───

        fn open_submitted_op(
            id: u64,
            target: Principal,
            trigger: FundingTrigger,
            amount: u128,
            fee: u128,
            now_secs: u64,
        ) -> FundingOperation {
            let global = test_global_policy(1_000_000, 600);
            let funding_policy =
                types::TargetFundingPolicy::validate_self_contained(&TargetFundingPolicyArgs {
                    low_balance_threshold_cycles: Nat::from(1u32),
                    refill_cycles: Nat::from(amount),
                    daily_cap_cycles: Nat::from(amount.max(1) * 10),
                    cooldown_secs: 60,
                    burn_anomaly_limit_cycles_per_day: None,
                })
                .unwrap();
            let _ = &global;
            let snapshot = CyclesWithdrawSnapshot {
                destination: target,
                from_subaccount: None,
                amount_cycles: amount,
                fee_cycles: fee,
                created_at_time_ns: now_secs * 1_000_000_000,
            };
            let op = FundingOperation::open(
                id,
                target,
                1,
                funding_policy,
                trigger,
                FundingRailArguments::Cycles(snapshot),
                amount,
                now_secs,
            )
            .unwrap();
            let submitted = op
                .record_attempt(
                    FundingOperationState::Cycles(CyclesFundingState::Submitted),
                    now_secs,
                    FundingAttemptResultClass::Indeterminate,
                )
                .unwrap();
            state::insert_operation(op).unwrap();
            state::update_operation(submitted.clone()).unwrap();
            submitted
        }

        fn seed_ordinary_reservations(op: &FundingOperation, now_secs: u64) {
            let target_reservation = state::get_target_reservation(op.target())
                .reserve(
                    op.id(),
                    op.reserved_amount_cycles(),
                    now_secs,
                    ROLLING_CAP_WINDOW_SECS,
                    1_000_000,
                )
                .unwrap();
            state::set_target_reservation(op.target(), target_reservation);
            let global_reservation = state::get_global_rolling_spend()
                .reserve(
                    op.id(),
                    op.reserved_amount_cycles(),
                    now_secs,
                    ROLLING_CAP_WINDOW_SECS,
                    1_000_000,
                )
                .unwrap();
            state::set_global_rolling_spend(global_reservation);
            let FundingRailArguments::Cycles(snapshot) = op.rail_arguments() else {
                unreachable!()
            };
            let amount_plus_fee = snapshot.amount_cycles + snapshot.fee_cycles;
            seed_fresh_cache(1_000_000, snapshot.fee_cycles, now_secs);
            let source_reservation = state::get_source_reserve()
                .reserve_ordinary(op.id(), amount_plus_fee, 0, now_secs, 1_000_000)
                .unwrap();
            state::set_source_reserve(source_reservation);
        }

        fn init_bare() {
            let global = test_global_policy(1_000_000, 600);
            init_test_state(global);
        }

        fn register_bare_target(seed: u8) -> Principal {
            let global = test_global_policy(1_000_000, 600);
            register_and_prime_target(seed, &global, 100, 10, 1_000_000, 60)
        }

        #[test]
        fn resolve_operation_ok_settles_reservations_applies_cooldown_and_compacts() {
            init_bare();
            let target = register_bare_target(1);
            let op =
                open_submitted_op(1, target, FundingTrigger::LowBalanceAutoTopup, 10, 1, 1_000);
            seed_ordinary_reservations(&op, 1_000);

            let resolved =
                resolve_operation(op.clone(), WithdrawOutcome::Confirmed(77), 1_000).unwrap();
            assert_eq!(
                resolved.state(),
                FundingOperationState::Cycles(CyclesFundingState::Complete)
            );
            assert_eq!(resolved.confirmed_block_index(), Some(77));
            // Compacted: no longer in the live operations store.
            assert_eq!(state::get_operation(op.id()), None);
            // Target reservation settled and on cooldown.
            assert!(state::get_target_reservation(target)
                .in_flight_operation_id()
                .is_none());
            assert!(state::get_target_reservation(target).is_on_cooldown(1_000));
            // Global and source reservations released too.
            assert!(state::get_global_rolling_spend()
                .rolling_spend()
                .pending()
                .is_empty());
            assert!(state::get_source_reserve().pending().is_empty());
            // Source cache balance debited by amount + fee (11).
            assert_eq!(
                state::get_source_reserve().cache().unwrap().balance_cycles,
                1_000_000 - 11
            );
        }

        #[test]
        fn resolve_operation_recovers_persisted_confirmed_block_without_a_second_call() {
            init_bare();
            let target = register_bare_target(1);
            let op =
                open_submitted_op(1, target, FundingTrigger::LowBalanceAutoTopup, 10, 1, 1_000);
            seed_ordinary_reservations(&op, 1_000);
            // Model the durable crash point: the source attempt is marked,
            // then the ledger has persisted an authoritative `Confirmed`
            // block, but completion/settlement has not run yet.
            state::mark_source_attempt(op.id(), 1_001).unwrap();
            let confirmed = op
                .record_attempt(
                    FundingOperationState::Cycles(CyclesFundingState::Confirmed),
                    1_001,
                    FundingAttemptResultClass::Success,
                )
                .unwrap()
                .attach_confirmed_block(77)
                .unwrap();
            state::update_operation(confirmed.clone()).unwrap();

            // The stale outcome is intentionally ignored: recovery uses the
            // persisted block proof and never issues a second ledger call.
            let recovered = resolve_operation(confirmed, WithdrawOutcome::Unknown, 1_002).unwrap();
            assert_eq!(
                recovered.state(),
                FundingOperationState::Cycles(CyclesFundingState::Complete)
            );
            assert_eq!(recovered.confirmed_block_index(), Some(77));
            assert_eq!(state::get_operation(op.id()), None);
            assert!(state::get_target_reservation(target)
                .in_flight_operation_id()
                .is_none());
            assert!(state::get_global_rolling_spend()
                .rolling_spend()
                .pending()
                .is_empty());
            assert!(state::get_source_reserve().pending().is_empty());
            assert_eq!(
                state::get_source_reserve().cache().unwrap().balance_cycles,
                1_000_000 - 11
            );
        }

        #[test]
        fn resolve_operation_unknown_with_31_prior_attempts_reaches_complete() {
            init_bare();
            let target = register_bare_target(1);
            let op =
                open_submitted_op(1, target, FundingTrigger::LowBalanceAutoTopup, 10, 1, 1_000);
            seed_ordinary_reservations(&op, 1_000);

            // The submitted record plus thirty exact ambiguous retries fill
            // 31 of the 32 bounded slots.  Confirmation must compact one
            // redundant retry record so the following Complete record still
            // has a durable slot.
            let mut current = op;
            for offset in 0..30 {
                current =
                    resolve_operation(current, WithdrawOutcome::Unknown, 1_001 + offset).unwrap();
            }
            assert_eq!(current.attempts().len(), types::MAX_FUNDING_ATTEMPTS - 1);
            assert_eq!(
                current.state(),
                FundingOperationState::Cycles(CyclesFundingState::Unknown)
            );

            let complete =
                resolve_operation(current, WithdrawOutcome::Confirmed(77), 2_000).unwrap();
            assert_eq!(
                complete.state(),
                FundingOperationState::Cycles(CyclesFundingState::Complete)
            );
            assert_eq!(complete.confirmed_block_index(), Some(77));
            assert_eq!(complete.attempts().len(), types::MAX_FUNDING_ATTEMPTS - 1);
            assert_eq!(state::get_operation(complete.id()), None);
            assert!(state::get_source_reserve().pending().is_empty());
        }

        #[test]
        fn resolve_operation_recovers_persisted_confirmed_block_with_full_history() {
            init_bare();
            let target = register_bare_target(1);
            let op =
                open_submitted_op(1, target, FundingTrigger::LowBalanceAutoTopup, 10, 1, 1_000);
            seed_ordinary_reservations(&op, 1_000);
            state::mark_source_attempt(op.id(), 1_001).unwrap();

            // Model a legacy crash point with a full 32-record history whose
            // final record is already Confirmed and carries its block proof.
            // Completion must replace one redundant non-terminal record and
            // remain bounded, rather than stranding the operation.
            let mut full = op.clone();
            for offset in 0..30 {
                full = full
                    .record_attempt(
                        FundingOperationState::Cycles(CyclesFundingState::Submitted),
                        1_002 + offset,
                        FundingAttemptResultClass::RetryableFailure,
                    )
                    .unwrap();
            }
            full = full
                .record_attempt(
                    FundingOperationState::Cycles(CyclesFundingState::Confirmed),
                    2_000,
                    FundingAttemptResultClass::Success,
                )
                .unwrap()
                .attach_confirmed_block(77)
                .unwrap();
            assert_eq!(full.attempts().len(), types::MAX_FUNDING_ATTEMPTS);
            state::update_operation(full.clone()).unwrap();

            let complete = resolve_operation(full, WithdrawOutcome::Unknown, 2_001).unwrap();
            assert_eq!(
                complete.state(),
                FundingOperationState::Cycles(CyclesFundingState::Complete)
            );
            assert_eq!(complete.confirmed_block_index(), Some(77));
            assert_eq!(complete.attempts().len(), types::MAX_FUNDING_ATTEMPTS);
            assert_eq!(state::get_operation(complete.id()), None);
            assert!(state::get_source_reserve().pending().is_empty());
        }

        /// Refresh linearization trace: a query samples balance B, an
        /// in-flight withdrawal settles against the pre-query source state,
        /// and the old query result attempts to commit afterward. The
        /// durable generation captured before the query must reject B rather
        /// than overwrite the locally debited cache.
        #[test]
        fn source_refresh_rejects_sampled_balance_after_withdrawal_settlement() {
            init_bare();
            let target = register_bare_target(1);
            let op =
                open_submitted_op(1, target, FundingTrigger::LowBalanceAutoTopup, 10, 1, 1_000);
            seed_ordinary_reservations(&op, 1_000);

            // Refresh starts here and samples B, but has not committed it.
            let refresh_generation = state::source_refresh_generation();
            let sampled_balance_b = 900u128;
            let sampled_fee_b = 1u128;
            let sampled_at_b = 1_002u64;

            // The withdrawal settles while that query is in flight. This
            // advances the generation and debits the known amount+fee from
            // the cache snapshot that predates the attempt.
            state::mark_source_attempt(op.id(), 1_001).unwrap();
            resolve_operation(op, WithdrawOutcome::Confirmed(77), 1_001).unwrap();
            assert_eq!(
                state::get_source_reserve().cache().unwrap().balance_cycles,
                1_000_000 - 11
            );

            // The query's B result is now stale. It must not replace the
            // debited balance or resurrect the settled source debit.
            assert_eq!(
                state::commit_source_reserve_refresh(
                    refresh_generation,
                    sampled_balance_b,
                    sampled_fee_b,
                    sampled_at_b,
                ),
                Err(state::SourceRefreshCommitError::StaleGeneration)
            );
            assert_eq!(
                state::get_source_reserve().cache().unwrap().balance_cycles,
                1_000_000 - 11
            );
        }

        #[test]
        fn source_refresh_generation_allows_only_one_competing_commit() {
            init_bare();
            seed_fresh_cache(1_000, 1, 1_000);
            let generation = state::source_refresh_generation();

            state::commit_source_reserve_refresh(generation, 900, 1, 1_001).unwrap();
            assert_eq!(
                state::commit_source_reserve_refresh(generation, 800, 1, 1_002),
                Err(state::SourceRefreshCommitError::StaleGeneration)
            );
            assert_eq!(
                state::get_source_reserve().cache().unwrap().balance_cycles,
                900
            );
        }

        #[test]
        fn resolve_operation_duplicate_is_quarantined_without_delivery_proof() {
            init_bare();
            let target = register_bare_target(1);
            let op =
                open_submitted_op(1, target, FundingTrigger::LowBalanceAutoTopup, 10, 0, 1_000);
            seed_ordinary_reservations(&op, 1_000);

            let resolved =
                resolve_operation(op.clone(), WithdrawOutcome::Duplicate(55), 1_000).unwrap();
            assert_eq!(
                resolved.state(),
                FundingOperationState::Cycles(CyclesFundingState::Quarantined)
            );
            assert_eq!(resolved.confirmed_block_index(), None);
            assert_eq!(state::get_operation(op.id()), Some(resolved));
            assert_eq!(
                state::get_target_reservation(target).in_flight_operation_id(),
                Some(op.id())
            );
            assert!(state::get_source_reserve()
                .pending()
                .iter()
                .any(|pending| pending.operation_id == op.id()));
        }

        #[test]
        fn resolve_operation_duplicate_after_unknown_remains_quarantined_and_reserved() {
            init_bare();
            let target = register_bare_target(1);
            let op =
                open_submitted_op(1, target, FundingTrigger::LowBalanceAutoTopup, 10, 0, 1_000);
            seed_ordinary_reservations(&op, 1_000);
            let unknown = resolve_operation(op, WithdrawOutcome::Unknown, 1_000).unwrap();
            let quarantined =
                resolve_operation(unknown.clone(), WithdrawOutcome::Duplicate(55), 1_001).unwrap();
            assert_eq!(
                quarantined.state(),
                FundingOperationState::Cycles(CyclesFundingState::Quarantined)
            );
            assert_eq!(quarantined.confirmed_block_index(), None);
            assert_eq!(state::get_operation(quarantined.id()), Some(quarantined));
            assert!(state::get_target_reservation(target)
                .in_flight_operation_id()
                .is_some());
        }

        #[test]
        fn reconcile_quarantined_cycles_requires_explicit_evidence_and_accounts_fee_debit() {
            init_bare();
            let target = register_bare_target(1);
            let op =
                open_submitted_op(1, target, FundingTrigger::LowBalanceAutoTopup, 10, 3, 1_000);
            seed_ordinary_reservations(&op, 1_000);
            let quarantined =
                resolve_operation(op.clone(), WithdrawOutcome::Duplicate(55), 1_000).unwrap();

            // The Duplicate result itself cannot be passed as a resolution;
            // the only accepted path is an explicit evidence decision.
            let reconciled = reconcile_quarantined_cycles(
                quarantined.id(),
                QuarantinedCyclesEvidence::FeeDebited {
                    known_spent_cycles: 6,
                },
                1_001,
            )
            .unwrap();
            assert_eq!(
                reconciled.state(),
                FundingOperationState::Cycles(CyclesFundingState::Terminal)
            );
            assert_eq!(state::get_operation(op.id()), None);
            assert_eq!(
                state::get_source_reserve().cache().unwrap().balance_cycles,
                1_000_000 - 6
            );
        }

        #[test]
        fn self_recovery_quarantined_cycles_stays_suppressed_without_delivery_proof() {
            init_bare();
            let op =
                open_submitted_op(1, sentinel_id(), FundingTrigger::SelfRecovery, 10, 1, 1_000);
            let self_recovery = state::get_self_recovery_state()
                .begin(op.id(), 10, 1_000, ROLLING_CAP_WINDOW_SECS, 1_000_000)
                .unwrap();
            state::set_self_recovery_state(self_recovery);
            seed_fresh_cache(1_000_000, 1, 1_000);
            let source = state::get_source_reserve()
                .reserve_self_recovery(op.id(), 11, 1_000, 1_000_000)
                .unwrap();
            state::set_source_reserve(source);
            let quarantined = resolve_operation(op, WithdrawOutcome::Duplicate(55), 1_000).unwrap();

            assert_eq!(
                reconcile_quarantined_cycles(
                    quarantined.id(),
                    QuarantinedCyclesEvidence::NoSpend,
                    1_001,
                ),
                Err(FundingError::Reconciliation(
                    ReconciliationError::SelfRecoveryDeliveryProofRequired
                ))
            );
            assert_eq!(state::get_operation(quarantined.id()), Some(quarantined));
            assert!(state::get_self_recovery_state().is_suppressing_distribution());
        }

        #[test]
        fn resolve_operation_exhausted_unknown_history_quarantines_and_remains_resolvable() {
            init_bare();
            let target = register_bare_target(1);
            let op =
                open_submitted_op(1, target, FundingTrigger::LowBalanceAutoTopup, 10, 0, 1_000);
            seed_ordinary_reservations(&op, 1_000);

            // The initial Submitted record consumes one slot.  The first
            // MAX-2 ambiguous replies append Unknown records; the next
            // reply must consume the final slot as an explicit Quarantined
            // terminal decision rather than leaving an unbounded Unknown
            // retry loop.
            let mut current = op;
            for offset in 0..(types::MAX_FUNDING_ATTEMPTS - 1) {
                current =
                    resolve_operation(current, WithdrawOutcome::Unknown, 1_001 + offset as u64)
                        .unwrap();
            }

            assert_eq!(current.attempts().len(), types::MAX_FUNDING_ATTEMPTS);
            assert_eq!(
                current.state(),
                FundingOperationState::Cycles(CyclesFundingState::Quarantined)
            );
            assert_eq!(
                current.attempts().as_slice().last().unwrap().phase,
                FundingOperationState::Cycles(CyclesFundingState::Quarantined)
            );
            assert_eq!(state::get_operation(current.id()), Some(current.clone()));
            assert!(current.state().stops_automatic_retry());
            assert!(!current.state().is_resolved());
            // Reservations remain linked to the retained operation, so a
            // signer/reconciliation path can still resolve it later.
            assert_eq!(
                state::get_target_reservation(target).in_flight_operation_id(),
                Some(current.id())
            );
            assert!(state::get_global_rolling_spend()
                .rolling_spend()
                .pending()
                .iter()
                .any(|pending| pending.operation_id == current.id()));
            assert!(state::get_source_reserve()
                .pending()
                .iter()
                .any(|pending| pending.operation_id == current.id()));
        }

        #[test]
        fn resolve_operation_unknown_retains_every_reservation_and_stays_live() {
            init_bare();
            let target = register_bare_target(1);
            let op =
                open_submitted_op(1, target, FundingTrigger::LowBalanceAutoTopup, 10, 0, 1_000);
            seed_ordinary_reservations(&op, 1_000);

            let resolved = resolve_operation(op.clone(), WithdrawOutcome::Unknown, 1_000).unwrap();
            assert_eq!(
                resolved.state(),
                FundingOperationState::Cycles(CyclesFundingState::Unknown)
            );
            // Still fully live: not compacted, every reservation untouched.
            assert_eq!(state::get_operation(op.id()), Some(resolved));
            assert_eq!(
                state::get_target_reservation(target).in_flight_operation_id(),
                Some(op.id())
            );
            assert!(state::get_global_rolling_spend()
                .rolling_spend()
                .pending()
                .iter()
                .any(|p| p.operation_id == op.id()));
            assert!(state::get_source_reserve()
                .pending()
                .iter()
                .any(|p| p.operation_id == op.id()));
        }

        #[test]
        fn resolve_operation_exact_retry_from_unknown_to_confirmed() {
            init_bare();
            let target = register_bare_target(1);
            let op =
                open_submitted_op(1, target, FundingTrigger::LowBalanceAutoTopup, 10, 0, 1_000);
            seed_ordinary_reservations(&op, 1_000);
            let unknown = resolve_operation(op, WithdrawOutcome::Unknown, 1_000).unwrap();

            // Exact retry reconstructs the SAME persisted arguments and this
            // time gets a definitive proof.
            let resolved =
                resolve_operation(unknown.clone(), WithdrawOutcome::Confirmed(9), 1_100).unwrap();
            assert_eq!(
                resolved.state(),
                FundingOperationState::Cycles(CyclesFundingState::Complete)
            );
            assert_eq!(resolved.confirmed_block_index(), Some(9));
            assert_eq!(state::get_operation(unknown.id()), None);
        }

        #[test]
        fn resolve_operation_too_old_quarantines_and_retains_reservations() {
            init_bare();
            let target = register_bare_target(1);
            let op =
                open_submitted_op(1, target, FundingTrigger::LowBalanceAutoTopup, 10, 0, 1_000);
            seed_ordinary_reservations(&op, 1_000);

            let resolved =
                resolve_operation(op.clone(), WithdrawOutcome::Quarantined, 1_000).unwrap();
            assert_eq!(
                resolved.state(),
                FundingOperationState::Cycles(CyclesFundingState::Quarantined)
            );
            assert_eq!(state::get_operation(op.id()), Some(resolved));
            assert_eq!(
                state::get_target_reservation(target).in_flight_operation_id(),
                Some(op.id())
            );
            // A quarantined operation never automatically retries again.
            assert!(state::get_operation(op.id())
                .unwrap()
                .state()
                .stops_automatic_retry());
            assert!(!state::get_operation(op.id()).unwrap().state().is_resolved());
        }

        #[test]
        fn resolve_operation_call_rejection_becomes_unknown_never_falls_through_to_icp() {
            init_bare();
            let target = register_bare_target(1);
            let op =
                open_submitted_op(1, target, FundingTrigger::LowBalanceAutoTopup, 10, 0, 1_000);
            seed_ordinary_reservations(&op, 1_000);
            // An inter-canister call rejection classifies as `Unknown` in
            // `cycles_ledger::withdraw` (see that module's tests) — this
            // proves `resolve_operation` handles it identically to any
            // other `Unknown`: no rail switch, no terminal state.
            let resolved = resolve_operation(op.clone(), WithdrawOutcome::Unknown, 1_000).unwrap();
            assert_eq!(resolved.rail(), types::FundingRail::CyclesLedger);
            assert_eq!(
                resolved.state(),
                FundingOperationState::Cycles(CyclesFundingState::Unknown)
            );
        }

        #[test]
        fn resolve_operation_ambiguous_generic_error_is_unknown_not_terminal() {
            init_bare();
            let target = register_bare_target(1);
            let op =
                open_submitted_op(1, target, FundingTrigger::LowBalanceAutoTopup, 10, 0, 1_000);
            seed_ordinary_reservations(&op, 1_000);
            let resolved = resolve_operation(op.clone(), WithdrawOutcome::Unknown, 1_000).unwrap();
            assert_eq!(
                resolved.state(),
                FundingOperationState::Cycles(CyclesFundingState::Unknown)
            );
            assert!(!resolved.state().is_resolved());
        }

        #[test]
        fn resolve_operation_terminal_no_spend_releases_without_debiting_cache() {
            init_bare();
            let target = register_bare_target(1);
            let op =
                open_submitted_op(1, target, FundingTrigger::LowBalanceAutoTopup, 10, 1, 1_000);
            seed_ordinary_reservations(&op, 1_000);

            let resolved =
                resolve_operation(op.clone(), WithdrawOutcome::TerminalNoSpend, 1_000).unwrap();
            assert_eq!(
                resolved.state(),
                FundingOperationState::Cycles(CyclesFundingState::Terminal)
            );
            assert_eq!(state::get_operation(op.id()), None);
            assert!(state::get_target_reservation(target)
                .in_flight_operation_id()
                .is_none());
            // No cooldown on a proven no-spend terminal.
            assert!(!state::get_target_reservation(target).is_on_cooldown(1_000));
            assert_eq!(
                state::get_source_reserve().cache().unwrap().balance_cycles,
                1_000_000
            );
        }

        #[test]
        fn resolve_operation_fee_debited_terminal_conservatively_debits_double_the_fee() {
            // Correction pass (ledger-security review): the pinned ledger
            // burns amount+fee then refunds only amount-fee on a
            // `FailedToWithdraw { fee_block: Some(_) }` — a known net debit
            // of 2x the fee, not one fee.
            init_bare();
            let target = register_bare_target(1);
            let op =
                open_submitted_op(1, target, FundingTrigger::LowBalanceAutoTopup, 10, 3, 1_000);
            seed_ordinary_reservations(&op, 1_000);

            let resolved = resolve_operation(
                op.clone(),
                WithdrawOutcome::TerminalFeeDebited { fee_block: 4 },
                1_000,
            )
            .unwrap();
            assert_eq!(
                resolved.state(),
                FundingOperationState::Cycles(CyclesFundingState::Terminal)
            );
            // Target/global released — delivery is proven NOT to have
            // happened even though a fee was debited.
            assert!(state::get_target_reservation(target)
                .in_flight_operation_id()
                .is_none());
            // The known net debit is 2x the 3-cycle fee (6), not one fee,
            // and not the full amount+fee (13) that was held pending.
            assert_eq!(
                state::get_source_reserve().cache().unwrap().balance_cycles,
                1_000_000 - 6
            );
        }

        /// Correction pass: `fee_block: None` is a deterministic, fully
        /// decoded case (verified against the pinned ledger source) whose
        /// known net debit is the FULL held `amount + fee`, not zero and
        /// not ambiguous.
        #[test]
        fn resolve_operation_full_amount_debited_terminal_debits_the_entire_held_amount() {
            init_bare();
            let target = register_bare_target(1);
            let op =
                open_submitted_op(1, target, FundingTrigger::LowBalanceAutoTopup, 10, 3, 1_000);
            seed_ordinary_reservations(&op, 1_000);

            let resolved = resolve_operation(
                op.clone(),
                WithdrawOutcome::TerminalFullAmountDebited,
                1_000,
            )
            .unwrap();
            assert_eq!(
                resolved.state(),
                FundingOperationState::Cycles(CyclesFundingState::Terminal)
            );
            assert!(state::get_target_reservation(target)
                .in_flight_operation_id()
                .is_none());
            // The entire held amount+fee (13) is debited: nothing was
            // refunded.
            assert_eq!(
                state::get_source_reserve().cache().unwrap().balance_cycles,
                1_000_000 - 13
            );
            assert_eq!(state::get_operation(op.id()), None);
        }

        // ─── resolve_operation atomicity (correction pass, CRITICAL fix) ───

        /// Reproduces the ORIGINAL Finding 1 failure trace exactly: a
        /// `Confirmed` outcome where the target reservation WOULD settle
        /// successfully, but the global reservation independently fails to
        /// settle (here, because no matching `GLOBAL_ROLLING_SPEND` pending
        /// entry was ever seeded — the same "stale/missing" shape the real
        /// bound failure produced). The fix must ensure NEITHER settlement
        /// commits and the operation is NOT persisted as `Complete` — proven
        /// via a byte-exact before/after comparison of the operation and
        /// every reservation store, not just a "some field changed" check.
        #[test]
        fn resolve_operation_settlement_failure_leaves_original_operation_and_target_reservation_untouched(
        ) {
            init_bare();
            let target = register_bare_target(1);
            let op =
                open_submitted_op(1, target, FundingTrigger::LowBalanceAutoTopup, 10, 0, 1_000);
            // Seed ONLY the target and source reservations — deliberately
            // omit the global one, so `global.settle()` fails with
            // `RollingSpendSettleError::UnknownOperation`.
            let target_reservation = state::get_target_reservation(op.target())
                .reserve(
                    op.id(),
                    op.reserved_amount_cycles(),
                    1_000,
                    ROLLING_CAP_WINDOW_SECS,
                    1_000_000,
                )
                .unwrap();
            state::set_target_reservation(op.target(), target_reservation);
            seed_fresh_cache(1_000_000, 0, 1_000);
            let source_reservation = state::get_source_reserve()
                .reserve_ordinary(op.id(), op.reserved_amount_cycles(), 0, 1_000, 1_000_000)
                .unwrap();
            state::set_source_reserve(source_reservation);

            let before_op = state::get_operation(op.id()).unwrap();
            let before_target = state::get_target_reservation(target);
            let before_source = state::get_source_reserve();

            let result = resolve_operation(op.clone(), WithdrawOutcome::Confirmed(1), 1_000);
            assert_eq!(
                result,
                Err(FundingError::GlobalSettle(
                    RollingSpendSettleError::UnknownOperation
                ))
            );

            // Nothing committed: not the operation (still `Submitted`, not
            // `Complete`), not the target reservation that WOULD have
            // settled successfully, not the source reservation either.
            assert_eq!(state::get_operation(op.id()), Some(before_op.clone()));
            assert_eq!(
                before_op.state(),
                FundingOperationState::Cycles(CyclesFundingState::Submitted)
            );
            assert_eq!(state::get_target_reservation(target), before_target);
            assert_eq!(state::get_source_reserve(), before_source);
            assert!(state::get_target_reservation(target)
                .in_flight_operation_id()
                .is_some());
        }

        /// Same guarantee, forced via the source-settlement step instead of
        /// the global one (proving the fix covers every settlement branch,
        /// not just the target/global pair): a cached balance too low to
        /// cover the known-spent amount fails `SourceReserveState::settle`'s
        /// checked-subtraction invariant, and the fix must still leave
        /// EVERY reservation — target, global, and source — and the
        /// operation itself completely unchanged. `validate_whole_state`
        /// must also still pass afterward: no orphaned linkage, no stuck
        /// resolved operation.
        #[test]
        fn resolve_operation_source_settlement_failure_leaves_every_reservation_untouched() {
            init_bare();
            let target = register_bare_target(1);
            let op =
                open_submitted_op(1, target, FundingTrigger::LowBalanceAutoTopup, 10, 1, 1_000);
            seed_ordinary_reservations(&op, 1_000);
            // Corrupt the cache to hold LESS than the known-spend (11) a
            // `Confirmed` outcome will require, without touching `pending`.
            let corrupted = state::get_source_reserve().refresh(5, 1, 1_001).unwrap();
            state::set_source_reserve(corrupted);

            let before_op = state::get_operation(op.id()).unwrap();
            let before_target = state::get_target_reservation(target);
            let before_global = state::get_global_rolling_spend();
            let before_source = state::get_source_reserve();

            let result = resolve_operation(op.clone(), WithdrawOutcome::Confirmed(1), 1_000);
            assert_eq!(
                result,
                Err(FundingError::SourceSettle(
                    types::SourceSettleError::KnownSpendExceedsCachedBalance
                ))
            );

            assert_eq!(state::get_operation(op.id()), Some(before_op.clone()));
            assert_eq!(
                before_op.state(),
                FundingOperationState::Cycles(CyclesFundingState::Submitted)
            );
            assert_eq!(state::get_target_reservation(target), before_target);
            assert_eq!(state::get_global_rolling_spend(), before_global);
            assert_eq!(state::get_source_reserve(), before_source);

            // The whole-state validator must still pass: the operation is
            // still unresolved and every reservation still names it.
            assert_eq!(state::validate_whole_state(sentinel_id()), Ok(()));
        }

        /// Same guarantee for a TERMINAL (no-delivery) resolution path, not
        /// just a successful one — forced via the same source-cache
        /// corruption, this time against a `TerminalFeeDebited` outcome.
        #[test]
        fn resolve_operation_terminal_settlement_failure_leaves_operation_unresolved() {
            init_bare();
            let target = register_bare_target(1);
            let op =
                open_submitted_op(1, target, FundingTrigger::LowBalanceAutoTopup, 10, 3, 1_000);
            seed_ordinary_reservations(&op, 1_000);
            // The known net debit for `TerminalFeeDebited` is `2 * fee` = 6;
            // corrupt the cache to hold less than that.
            let corrupted = state::get_source_reserve().refresh(2, 3, 1_001).unwrap();
            state::set_source_reserve(corrupted);

            let before_op = state::get_operation(op.id()).unwrap();
            let before_target = state::get_target_reservation(target);

            let result = resolve_operation(
                op.clone(),
                WithdrawOutcome::TerminalFeeDebited { fee_block: 9 },
                1_000,
            );
            assert_eq!(
                result,
                Err(FundingError::SourceSettle(
                    types::SourceSettleError::KnownSpendExceedsCachedBalance
                ))
            );
            assert_eq!(state::get_operation(op.id()), Some(before_op));
            assert_eq!(state::get_target_reservation(target), before_target);
            assert_eq!(state::validate_whole_state(sentinel_id()), Ok(()));
        }

        // ─── cooldown / cap edges ───

        #[test]
        fn resolve_operation_settle_respects_target_cooldown_edge() {
            init_bare();
            let target = register_bare_target(1);
            let op =
                open_submitted_op(1, target, FundingTrigger::LowBalanceAutoTopup, 10, 0, 1_000);
            seed_ordinary_reservations(&op, 1_000);
            resolve_operation(op, WithdrawOutcome::Confirmed(1), 1_000).unwrap();
            // The target's own configured cooldown is 60s (registered via
            // `register_and_prime_target`'s default).
            assert!(state::get_target_reservation(target).is_on_cooldown(1_059));
            assert!(!state::get_target_reservation(target).is_on_cooldown(1_060));
        }

        // ─── self-recovery shares the source reservation ───

        #[test]
        fn ordinary_and_self_recovery_operations_share_the_source_reservation() {
            init_bare();
            let target = register_bare_target(1);
            let ordinary = open_submitted_op(
                1,
                target,
                FundingTrigger::LowBalanceAutoTopup,
                500_000,
                0,
                1_000,
            );
            seed_ordinary_reservations(&ordinary, 1_000);

            let self_recovery_op = open_submitted_op(
                2,
                sentinel_id(),
                FundingTrigger::SelfRecovery,
                600_000,
                0,
                1_000,
            );
            let self_recovery_reservation = state::get_self_recovery_state()
                .begin(
                    self_recovery_op.id(),
                    600_000,
                    1_000,
                    ROLLING_CAP_WINDOW_SECS,
                    1_000_000,
                )
                .unwrap();
            state::set_self_recovery_state(self_recovery_reservation);
            // The shared source cache only has 1,000,000 total; ordinary
            // already reserved 500,000, so self-recovery cannot ALSO
            // reserve 600,000 — the two lanes cannot race past each other.
            let source_result = state::get_source_reserve().reserve_self_recovery(
                self_recovery_op.id(),
                600_000,
                1_000,
                1_000_000,
            );
            assert_eq!(source_result, Err(SourceReserveError::InsufficientReserve));
        }

        // ─── protected reserve ───

        #[test]
        fn ordinary_reservation_cannot_consume_the_protected_self_recovery_reserve() {
            init_bare();
            let target = register_bare_target(1);
            seed_fresh_cache(1_000, 0, 1_000);
            // Protected reserve of 400 out of a 1,000 balance: only 600 is
            // free for ordinary spending.
            let result =
                state::get_source_reserve().reserve_ordinary(1, 601, 400, 1_000, 1_000_000);
            assert_eq!(result, Err(SourceReserveError::InsufficientReserve));
            let ok = state::get_source_reserve().reserve_ordinary(1, 600, 400, 1_000, 1_000_000);
            assert!(ok.is_ok());
            let _ = target;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        Criticality, Environment, GlobalPolicy, GlobalPolicyArgs, GovernanceTimelocksArgs,
        ObservationMode, Sample, SelfRecoveryPolicyArgs, TargetArgs, TargetFundingPolicyArgs,
        TargetRegistrationContext,
    };
    use candid::Nat;
    use std::collections::BTreeSet;

    fn sentinel_id() -> Principal {
        Principal::from_slice(&[9, 9, 9])
    }

    fn target_principal(seed: u8) -> Principal {
        Principal::from_slice(&[seed, 1, 2, 3])
    }

    fn test_governance_timelocks_args() -> GovernanceTimelocksArgs {
        GovernanceTimelocksArgs {
            target_registry_secs: 1,
            spend_policy_secs: 1,
            signer_change_secs: 1,
            unpause_secs: 1,
        }
    }

    fn test_self_recovery_args() -> SelfRecoveryPolicyArgs {
        SelfRecoveryPolicyArgs {
            protected_reserve_cycles: Nat::from(1_000u32),
            daily_cap_cycles: Nat::from(500u32),
            low_balance_threshold_cycles: Nat::from(1u32),
            refill_cycles: Nat::from(10u32),
        }
    }

    fn test_global_policy(stale_after_secs: u64) -> GlobalPolicy {
        GlobalPolicy::validate(&GlobalPolicyArgs {
            global_daily_cap_cycles: Nat::from(1_000_000u32),
            sample_interval_secs: 60,
            stale_after_secs,
            min_icp_reserve_e8s: Nat::from(0u32),
            timelocks: test_governance_timelocks_args(),
            self_recovery_policy: test_self_recovery_args(),
        })
        .unwrap()
    }

    fn init_test_state(global: GlobalPolicy) {
        state::init(types::InitArgs {
            signers: vec![Principal::from_slice(&[1])],
            approval_threshold: 1,
            global_policy: global.to_args(),
        })
        .unwrap();
    }

    fn register_test_target(seed: u8, global: &GlobalPolicy, auto_topup: bool) -> Principal {
        let principal = target_principal(seed);
        let empty = BTreeSet::new();
        let ctx = TargetRegistrationContext {
            sentinel_id: sentinel_id(),
            existing_target_count: 0,
            existing_target_principals: &empty,
            global_policy: global,
        };
        let args = TargetArgs {
            principal,
            display_name: "svc".to_string(),
            project: "proj".to_string(),
            environment: Environment::Production,
            criticality: Criticality::Standard,
            observation_mode: ObservationMode::SelfReport,
            tags: vec![],
            funding_policy: TargetFundingPolicyArgs {
                low_balance_threshold_cycles: Nat::from(100u32),
                refill_cycles: Nat::from(10u32),
                daily_cap_cycles: Nat::from(1_000u32),
                cooldown_secs: 60,
                burn_anomaly_limit_cycles_per_day: None,
            },
        };
        let mut record = types::TargetRecord::register(args, &ctx).unwrap();
        record = record
            .apply_patch(
                types::TargetPatch {
                    display_name: None,
                    project: None,
                    environment: None,
                    criticality: None,
                    observation_mode: None,
                    tags: None,
                    funding_policy: None,
                    enabled: Some(true),
                    auto_topup: Some(auto_topup),
                },
                global,
            )
            .unwrap();
        state::insert_target(record).unwrap();
        principal
    }

    fn sample(secs: u64, balance: u128, state_: PublicTargetState) -> Sample {
        Sample {
            timestamp_secs: secs,
            balance: Some(AdvisoryCyclesBalance::Exact(balance)),
            state: state_,
            reported_operational_healthy: None,
            burn_cycles_per_hour: None,
        }
    }

    #[test]
    fn eligibility_rejects_unregistered_target() {
        init_test_state(test_global_policy(600));
        assert_eq!(
            check_ordinary_eligibility(target_principal(1), 1_000, false),
            Err(EligibilityError::UnregisteredTarget)
        );
    }

    #[test]
    fn eligibility_rejects_disabled_target() {
        let global = test_global_policy(600);
        init_test_state(global.clone());
        let principal = target_principal(1);
        let empty = BTreeSet::new();
        let ctx = TargetRegistrationContext {
            sentinel_id: sentinel_id(),
            existing_target_count: 0,
            existing_target_principals: &empty,
            global_policy: &global,
        };
        let args = TargetArgs {
            principal,
            display_name: "svc".to_string(),
            project: "proj".to_string(),
            environment: Environment::Production,
            criticality: Criticality::Standard,
            observation_mode: ObservationMode::SelfReport,
            tags: vec![],
            funding_policy: TargetFundingPolicyArgs {
                low_balance_threshold_cycles: Nat::from(100u32),
                refill_cycles: Nat::from(10u32),
                daily_cap_cycles: Nat::from(1_000u32),
                cooldown_secs: 60,
                burn_anomaly_limit_cycles_per_day: None,
            },
        };
        state::insert_target(types::TargetRecord::register(args, &ctx).unwrap()).unwrap();
        assert_eq!(
            check_ordinary_eligibility(principal, 1_000, false),
            Err(EligibilityError::Disabled)
        );
    }

    #[test]
    fn eligibility_rejects_auto_topup_off_for_non_manual_but_manual_bypasses_it() {
        let global = test_global_policy(600);
        init_test_state(global.clone());
        let target = register_test_target(1, &global, false);
        state::record_sample(target, sample(1_000, 5, PublicTargetState::Low)).unwrap();
        assert_eq!(
            check_ordinary_eligibility(target, 1_000, false),
            Err(EligibilityError::NotAutoTopup)
        );
        assert!(check_ordinary_eligibility(target, 1_000, true).is_ok());
    }

    #[test]
    fn eligibility_rejects_paused_target() {
        let global = test_global_policy(600);
        init_test_state(global.clone());
        let target = register_test_target(1, &global, true);
        let paused = state::get_target(target).unwrap().pause();
        state::insert_target(paused).unwrap();
        state::record_sample(target, sample(1_000, 5, PublicTargetState::Low)).unwrap();
        assert_eq!(
            check_ordinary_eligibility(target, 1_000, true),
            Err(EligibilityError::Paused)
        );
    }

    #[test]
    fn eligibility_rejects_unresolved_self_recovery() {
        let global = test_global_policy(600);
        init_test_state(global.clone());
        let target = register_test_target(1, &global, true);
        state::record_sample(target, sample(1_000, 5, PublicTargetState::Low)).unwrap();
        let self_recovery = state::get_self_recovery_state()
            .begin(1, 10, 1_000, 86_400, 500)
            .unwrap();
        state::set_self_recovery_state(self_recovery);
        assert_eq!(
            check_ordinary_eligibility(target, 1_000, true),
            Err(EligibilityError::SelfRecoveryUnresolved)
        );
    }

    #[test]
    fn eligibility_rejects_stale_sample() {
        let global = test_global_policy(600);
        init_test_state(global.clone());
        let target = register_test_target(1, &global, true);
        state::record_sample(target, sample(100, 5, PublicTargetState::Low)).unwrap();
        assert_eq!(
            check_ordinary_eligibility(target, 100 + 601, true),
            Err(EligibilityError::StaleSample)
        );
        // At exactly the boundary it is still fresh.
        assert!(check_ordinary_eligibility(target, 100 + 600, true).is_ok());
    }

    #[test]
    fn eligibility_rejects_unreachable_sample() {
        let global = test_global_policy(600);
        init_test_state(global.clone());
        let target = register_test_target(1, &global, true);
        state::record_sample(
            target,
            Sample {
                timestamp_secs: 1_000,
                balance: None,
                state: PublicTargetState::Unreachable,
                reported_operational_healthy: None,
                burn_cycles_per_hour: None,
            },
        )
        .unwrap();
        assert_eq!(
            check_ordinary_eligibility(target, 1_000, true),
            Err(EligibilityError::Unreachable)
        );
    }

    #[test]
    fn eligibility_exact_threshold_is_low_and_above_is_not() {
        let global = test_global_policy(600);
        init_test_state(global.clone());
        let target = register_test_target(1, &global, true);
        state::record_sample(target, sample(1_000, 100, PublicTargetState::Low)).unwrap();
        assert!(check_ordinary_eligibility(target, 1_000, true).is_ok());
        state::record_sample(target, sample(1_001, 101, PublicTargetState::Healthy)).unwrap();
        assert_eq!(
            check_ordinary_eligibility(target, 1_001, true),
            Err(EligibilityError::NotLow)
        );
    }

    #[test]
    fn eligibility_rejects_in_flight_operation_and_cooldown() {
        let global = test_global_policy(600);
        init_test_state(global.clone());
        let target = register_test_target(1, &global, true);
        state::record_sample(target, sample(1_000, 5, PublicTargetState::Low)).unwrap();
        let reserved = state::get_target_reservation(target)
            .reserve(1, 10, 1_000, 86_400, 1_000)
            .unwrap();
        state::set_target_reservation(target, reserved);
        assert_eq!(
            check_ordinary_eligibility(target, 1_000, true),
            Err(EligibilityError::OperationInFlight)
        );

        let settled = state::get_target_reservation(target)
            .settle_spend(1, 1_000, 500, 86_400)
            .unwrap();
        state::set_target_reservation(target, settled);
        assert_eq!(
            check_ordinary_eligibility(target, 1_000, true),
            Err(EligibilityError::Cooldown)
        );
        assert!(check_ordinary_eligibility(target, 1_000 + 500, true).is_ok());
    }
}
