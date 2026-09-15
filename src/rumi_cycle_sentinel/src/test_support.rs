//! Opt-in integration fixtures for persisted outbox states.
//!
//! This module is intentionally behind the `test_endpoints` Cargo feature.
//! It exists only to put a valid operation and its linked reservations at a
//! chosen lifecycle state before a PocketIC upgrade test.  It is not imported
//! by a normal build, and `lib.rs` exports no endpoint from it unless that
//! feature is explicitly selected.

use candid::{CandidType, Nat, Principal};
use serde::Deserialize;

use crate::funding::cycles::ROLLING_CAP_WINDOW_SECS;
use crate::icp_cmc::{self, IcpXdrConversionRate};
use crate::state;
use crate::types::{
    Alarm, AlarmKind, AlarmStatus, Criticality, CyclesFundingState, CyclesWithdrawSnapshot,
    Environment, FundingAttemptResultClass, FundingOperation, FundingOperationState,
    FundingRailArguments, FundingTrigger, IcpFundingState, ObservationMode, ProposalPayload,
    ProposalRecord, PublicTargetState, Sample, TargetArgs, TargetFundingPolicyArgs,
    TargetRegistrationContext, TerminalFundingSummary,
};
use std::collections::BTreeSet;

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TestCyclesState {
    PlannedReserved,
    Submitted,
    Confirmed,
    Unknown,
    Complete,
    Terminal,
    Quarantined,
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TestIcpState {
    PlannedReserved,
    LedgerSubmitted,
    TransferUnknown,
    TransferConfirmed,
    NotifyPending,
    Complete,
    Refunded,
    Terminal,
    Quarantined,
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TestOperationState {
    Cycles(TestCyclesState),
    Icp(TestIcpState),
}

/// Each live stable-bound fixture runs in an independent canister instance.
/// Keeping the largest stores separate makes the PocketIC upgrade/query proof
/// exercise the real stable structures without combining every maximum into a
/// single instruction-heavy state image.
#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TestBoundsMode {
    Targets,
    Samples,
    Alarms,
    Proposals,
    TerminalSummaries,
}

/// Narrow, read-only projection used by PocketIC tests to prove that an
/// injected durable operation survived an upgrade.  The production interface
/// deliberately exposes no operation lookup, so this type is only reachable
/// through the `test_endpoints` feature.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub(crate) struct TestOperationView {
    pub id: u64,
    pub target: Principal,
    pub target_registry_revision: u64,
    pub reserved_amount_cycles: Nat,
    pub state: FundingOperationState,
    pub attempt_count: u32,
    pub confirmed_block_index: Option<u64>,
    pub actual_cycles: Option<Nat>,
    pub refund_block_hint: Option<u64>,
    pub refund_block_index: Option<u64>,
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TestTargetFlags {
    pub enabled: bool,
    pub auto_topup: bool,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub(crate) struct TestBoundsReport {
    pub target_count: u64,
    pub sample_count: u64,
    pub alarm_count: u64,
    pub proposal_count: u64,
    pub terminal_summary_count: u64,
    pub target_overflow_rejected: bool,
    pub sample_overflow_pruned: bool,
    pub alarm_overflow_rejected: bool,
    pub alarm_oldest_pruned: bool,
    pub proposal_overflow_rejected: bool,
}

fn bounds_counts(sample_target: Principal) -> TestBoundsReport {
    TestBoundsReport {
        target_count: state::target_count(),
        sample_count: state::sample_meta(sample_target)
            .map(|meta| meta.filled_slots as u64)
            .unwrap_or_default(),
        alarm_count: state::alarm_count(),
        proposal_count: state::proposal_count(),
        terminal_summary_count: state::terminal_summary_count(),
        target_overflow_rejected: false,
        sample_overflow_pruned: false,
        alarm_overflow_rejected: false,
        alarm_oldest_pruned: false,
        proposal_overflow_rejected: false,
    }
}

/// Fill one bounded store from the live feature-gated Wasm and exercise its
/// overflow/retention behavior. Each mode runs in its own canister instance;
/// production has no endpoint capable of arbitrary state injection.
pub(crate) fn fill_bounds(
    mode: TestBoundsMode,
    now_secs: u64,
    now_ns: u64,
    sentinel_id: Principal,
) -> Result<TestBoundsReport, String> {
    let initial_target = state::target_principals()
        .into_iter()
        .next()
        .ok_or_else(|| "bootstrap target missing".to_string())?;
    let global = state::global_config();
    let mut existing: BTreeSet<Principal> = state::target_principals().into_iter().collect();
    let target_policy = state::get_target(initial_target)
        .ok_or_else(|| "bootstrap target missing".to_string())?
        .funding_policy()
        .to_args();
    let mut target_overflow_rejected = false;
    if mode == TestBoundsMode::Targets {
        let mut candidate_index = 0u16;
        while state::target_count() < crate::types::MAX_TARGETS as u64 {
            let principal = Principal::from_slice(&[0x7f, (candidate_index & 0xff) as u8]);
            candidate_index = candidate_index.saturating_add(1);
            if existing.contains(&principal) {
                continue;
            }
            let record = crate::types::TargetRecord::register(
                TargetArgs {
                    principal,
                    display_name: format!("bulk-target-{candidate_index}"),
                    project: "Rumi Protocol".to_string(),
                    environment: Environment::Test,
                    criticality: Criticality::Experimental,
                    observation_mode: ObservationMode::SelfReport,
                    tags: vec![],
                    funding_policy: TargetFundingPolicyArgs {
                        low_balance_threshold_cycles: target_policy
                            .low_balance_threshold_cycles
                            .clone(),
                        refill_cycles: target_policy.refill_cycles.clone(),
                        daily_cap_cycles: target_policy.daily_cap_cycles.clone(),
                        cooldown_secs: target_policy.cooldown_secs,
                        burn_anomaly_limit_cycles_per_day: None,
                    },
                },
                &TargetRegistrationContext {
                    sentinel_id,
                    existing_target_count: existing.len(),
                    existing_target_principals: &existing,
                    global_policy: &global.global_policy,
                },
            )
            .map_err(failure)?;
            state::insert_target(record).map_err(failure)?;
            existing.insert(principal);
        }
        let overflow_principal = Principal::from_slice(&[0x7e, 0xff]);
        if !existing.contains(&overflow_principal) {
            let overflow = crate::types::TargetRecord::register(
                TargetArgs {
                    principal: overflow_principal,
                    display_name: "bulk-overflow".to_string(),
                    project: "Rumi Protocol".to_string(),
                    environment: Environment::Test,
                    criticality: Criticality::Experimental,
                    observation_mode: ObservationMode::SelfReport,
                    tags: vec![],
                    funding_policy: target_policy,
                },
                &TargetRegistrationContext {
                    sentinel_id,
                    existing_target_count: existing.len(),
                    existing_target_principals: &existing,
                    global_policy: &global.global_policy,
                },
            );
            if let Ok(record) = overflow {
                target_overflow_rejected = state::insert_target(record).is_err();
            } else {
                target_overflow_rejected = true;
            }
        }
    }

    // Fill and wrap one target's ring. The 2161st write must evict exactly
    // the oldest physical slot while preserving monotonic logical history.
    let mut sample_overflow_pruned = false;
    if mode == TestBoundsMode::Samples {
        for offset in 0..=crate::types::MAX_SAMPLES_PER_TARGET {
            state::record_sample(
                initial_target,
                Sample {
                    timestamp_secs: now_secs.saturating_add(offset as u64),
                    balance: Some(crate::types::AdvisoryCyclesBalance::Exact(1_000_000)),
                    state: PublicTargetState::Healthy,
                    reported_operational_healthy: Some(true),
                    burn_cycles_per_hour: None,
                },
            )
            .map_err(failure)?;
        }
        sample_overflow_pruned = state::sample_meta(initial_target)
            .map(|meta| {
                meta.filled_slots as usize == crate::types::MAX_SAMPLES_PER_TARGET
                    && meta.total_writes == (crate::types::MAX_SAMPLES_PER_TARGET + 1) as u64
            })
            .unwrap_or(false);
    }

    // Direct alarm records avoid a 1,024-call ingress loop while exercising
    // the same bounded storage function used by production alarm detection.
    let mut alarm_overflow_rejected = false;
    let mut alarm_oldest_pruned = false;
    if mode == TestBoundsMode::Alarms {
        let mut first_alarm_id = None;
        for _ in 0..crate::types::MAX_ALARMS {
            let id = state::next_alarm_id();
            first_alarm_id.get_or_insert(id);
            state::insert_alarm(Alarm {
                id,
                target: None,
                kind: AlarmKind::BurnAnomaly,
                status: AlarmStatus::Open,
                opened_at_secs: now_secs,
                acknowledged_at_secs: None,
                resolved_at_secs: None,
            })
            .map_err(failure)?;
        }
        let id = state::next_alarm_id();
        alarm_overflow_rejected = state::insert_alarm(Alarm {
            id,
            target: None,
            kind: AlarmKind::LowBalance,
            status: AlarmStatus::Open,
            opened_at_secs: now_secs,
            acknowledged_at_secs: None,
            resolved_at_secs: None,
        })
        .is_err();
        let _ = state::alarms::resolve_at(None, AlarmKind::BurnAnomaly, now_secs);
        let replacement_id = state::next_alarm_id();
        state::insert_alarm(Alarm {
            id: replacement_id,
            target: None,
            kind: AlarmKind::Unreachable,
            status: AlarmStatus::Open,
            opened_at_secs: now_secs.saturating_add(1),
            acknowledged_at_secs: None,
            resolved_at_secs: None,
        })
        .map_err(failure)?;
        alarm_oldest_pruned = first_alarm_id.is_some_and(|id| state::get_alarm(id).is_none());
    }

    // Proposals have no safe eviction for open records. Use valid open
    // UpdateTarget payloads so the live store reaches exactly its cap.
    let mut proposal_overflow_rejected = false;
    if mode == TestBoundsMode::Proposals {
        let first_proposal_target = state::target_principals()
            .into_iter()
            .next()
            .ok_or_else(|| "target missing after bulk fill".to_string())?;
        for _ in 0..crate::types::MAX_PROPOSALS {
            let id = state::next_proposal_id();
            state::insert_proposal(ProposalRecord::new(
                id,
                ProposalPayload::UpdateTarget {
                    principal: first_proposal_target,
                    patch: crate::types::TargetPatch {
                        display_name: None,
                        project: None,
                        environment: None,
                        criticality: None,
                        observation_mode: None,
                        tags: None,
                        funding_policy: None,
                        enabled: None,
                        auto_topup: None,
                    },
                },
                Principal::from_slice(&[9; 10]),
                now_secs,
            ))
            .map_err(failure)?;
        }
        proposal_overflow_rejected = state::insert_proposal(ProposalRecord::new(
            state::next_proposal_id(),
            ProposalPayload::UpdateTarget {
                principal: first_proposal_target,
                patch: crate::types::TargetPatch {
                    display_name: None,
                    project: None,
                    environment: None,
                    criticality: None,
                    observation_mode: None,
                    tags: None,
                    funding_policy: None,
                    enabled: None,
                    auto_topup: None,
                },
            },
            Principal::from_slice(&[9; 10]),
            now_secs,
        ))
        .is_err();
    }

    // Create compact terminal records directly from legal operation state
    // transitions. No reservations are needed for an already-resolved test
    // fixture, so compaction itself is the only path exercised here.
    if mode == TestBoundsMode::TerminalSummaries {
        let all_targets: Vec<Principal> = state::target_principals().into_iter().collect();
        for index in 0..crate::types::MAX_TERMINAL_SUMMARIES {
            let target = all_targets[index % all_targets.len()];
            let record = state::get_target(target).ok_or_else(|| "target missing".to_string())?;
            let amount = record.funding_policy().refill_cycles();
            let operation_id = state::next_operation_id();
            let created = state::next_created_at_time_ns(now_ns.saturating_add(index as u64))
                .map_err(failure)?;
            let op = FundingOperation::open(
                operation_id,
                target,
                record.revision(),
                record.funding_policy().clone(),
                FundingTrigger::ManualTopup,
                FundingRailArguments::Cycles(CyclesWithdrawSnapshot {
                    destination: target,
                    from_subaccount: None,
                    amount_cycles: amount,
                    fee_cycles: 1,
                    created_at_time_ns: created,
                }),
                amount,
                now_secs,
            )
            .map_err(failure)?;
            let op = op
                .record_attempt(
                    FundingOperationState::Cycles(CyclesFundingState::Submitted),
                    now_secs,
                    FundingAttemptResultClass::Indeterminate,
                )
                .map_err(failure)?
                .record_attempt(
                    FundingOperationState::Cycles(CyclesFundingState::Confirmed),
                    now_secs,
                    FundingAttemptResultClass::Success,
                )
                .map_err(failure)?
                .attach_confirmed_block(index as u64)
                .map_err(failure)?
                .record_attempt(
                    FundingOperationState::Cycles(CyclesFundingState::Complete),
                    now_secs,
                    FundingAttemptResultClass::Success,
                )
                .map_err(failure)?;
            let summary = TerminalFundingSummary::from_resolved(&op, now_secs).map_err(failure)?;
            state::insert_operation(op).map_err(failure)?;
            state::compact_operation(operation_id, summary).map_err(failure)?;
        }
    }

    let mut report = bounds_counts(initial_target);
    report.target_overflow_rejected = target_overflow_rejected;
    report.sample_overflow_pruned = sample_overflow_pruned;
    report.alarm_overflow_rejected = alarm_overflow_rejected;
    report.alarm_oldest_pruned = alarm_oldest_pruned;
    report.proposal_overflow_rejected = proposal_overflow_rejected;
    Ok(report)
}

pub(crate) fn get_bound_counts(sample_target: Principal) -> TestBoundsReport {
    assert!(
        state::get_target(sample_target).is_some(),
        "bound sample target remains registered"
    );
    bounds_counts(sample_target)
}

pub(crate) fn operation_view(operation_id: u64) -> Option<TestOperationView> {
    state::get_operation(operation_id).map(|operation| TestOperationView {
        id: operation.id(),
        target: operation.target(),
        target_registry_revision: operation.target_registry_revision(),
        reserved_amount_cycles: Nat::from(operation.reserved_amount_cycles()),
        state: operation.state(),
        attempt_count: operation.attempts().len() as u32,
        confirmed_block_index: operation.confirmed_block_index(),
        actual_cycles: operation.actual_cycles().map(Nat::from),
        refund_block_hint: operation.refund_block_hint(),
        refund_block_index: operation.refund_block_index(),
    })
}

fn failure(error: impl std::fmt::Debug) -> String {
    format!("test operation injection failed: {error:?}")
}

/// The ICP fallback needs enough global capacity for its required 10% headroom
/// above a target's refill.  Test-only injection may widen the policy in
/// memory, while preserving every other checked policy field; production
/// governance still owns the real policy and this code is not compiled there.
fn widen_global_cap_for_fixture() {
    let current = state::global_config();
    if current.global_policy.global_daily_cap_cycles() >= 1_000_000_000_000_000 {
        return;
    }
    let mut args = current.global_policy.to_args();
    args.global_daily_cap_cycles = Nat::from(1_000_000_000_000_000u128);
    let policy = crate::types::GlobalPolicy::validate(&args)
        .expect("test-only global fixture policy remains valid");
    state::set_global_config(state::GlobalConfig {
        signers: current.signers,
        approval_threshold: current.approval_threshold,
        global_policy: policy,
    });
    // The ICP snapshot deliberately includes 10% headroom above the refill.
    // Bootstrap uses the global cap as each target's daily cap, so widen the
    // fixture target caps as well; this is test-only policy setup and never
    // changes production governance defaults.
    let global = state::global_config();
    for principal in state::target_principals() {
        if let Some(record) = state::get_target(principal) {
            let mut funding_policy = record.funding_policy().to_args();
            funding_policy.daily_cap_cycles = Nat::from(crate::types::MAX_TARGET_DAILY_CAP_CYCLES);
            let updated = record
                .apply_patch(
                    crate::types::TargetPatch {
                        display_name: None,
                        project: None,
                        environment: None,
                        criticality: None,
                        observation_mode: None,
                        tags: None,
                        funding_policy: Some(funding_policy),
                        enabled: None,
                        auto_topup: None,
                    },
                    &global.global_policy,
                )
                .expect("test-only target cap widening remains valid");
            state::insert_target(updated).expect("test-only target update remains bounded");
        }
    }
}

fn reserve_ordinary(
    operation_id: u64,
    target: Principal,
    amount_cycles: u128,
    now_secs: u64,
    source: crate::types::SourceReserveState,
    source_amount_cycles: u128,
) -> Result<(), String> {
    let record = state::get_target(target).ok_or_else(|| "target not registered".to_string())?;
    let global = state::global_config();
    let target_reservation = state::get_target_reservation(target)
        .reserve(
            operation_id,
            amount_cycles,
            now_secs,
            ROLLING_CAP_WINDOW_SECS,
            record.funding_policy().daily_cap_cycles(),
        )
        .map_err(failure)?;
    let global_reservation = state::get_global_rolling_spend()
        .reserve(
            operation_id,
            amount_cycles,
            now_secs,
            ROLLING_CAP_WINDOW_SECS,
            global.global_policy.global_daily_cap_cycles(),
        )
        .map_err(failure)?;
    let source_reservation = source
        .reserve_ordinary(
            operation_id,
            source_amount_cycles,
            global
                .global_policy
                .self_recovery_policy()
                .protected_reserve_cycles(),
            now_secs,
            global.global_policy.stale_after_secs(),
        )
        .map_err(failure)?;
    state::set_target_reservation(target, target_reservation);
    state::set_global_rolling_spend(global_reservation);
    state::set_source_reserve(source_reservation);
    Ok(())
}

fn reserve_icp(
    operation_id: u64,
    target: Principal,
    amount_cycles: u128,
    source_amount_e8s: u128,
    now_secs: u64,
) -> Result<(), String> {
    let record = state::get_target(target).ok_or_else(|| "target not registered".to_string())?;
    let global = state::global_config();
    let target_reservation = state::get_target_reservation(target)
        .reserve(
            operation_id,
            amount_cycles,
            now_secs,
            ROLLING_CAP_WINDOW_SECS,
            record.funding_policy().daily_cap_cycles(),
        )
        .map_err(failure)?;
    let global_reservation = state::get_global_rolling_spend()
        .reserve(
            operation_id,
            amount_cycles,
            now_secs,
            ROLLING_CAP_WINDOW_SECS,
            global.global_policy.global_daily_cap_cycles(),
        )
        .map_err(failure)?;
    let source = state::get_icp_source_reserve()
        .reserve_ordinary(
            operation_id,
            source_amount_e8s,
            global.global_policy.min_icp_reserve_e8s(),
            now_secs,
            crate::types::icp_source_cache_max_age_secs(),
        )
        .map_err(failure)?;
    state::set_target_reservation(target, target_reservation);
    state::set_global_rolling_spend(global_reservation);
    state::set_icp_source_reserve(source);
    Ok(())
}

fn persist_transition(
    current: FundingOperation,
    next: FundingOperation,
) -> Result<FundingOperation, String> {
    state::update_operation(next.clone()).map_err(failure)?;
    // Keep this assertion close to every fixture transition: if a future
    // state-machine change makes a requested state illegal, the test endpoint
    // fails at the exact edge instead of leaving an invalid stable record.
    let _ = current;
    Ok(next)
}

fn cycles_operation(
    target: Principal,
    desired: TestCyclesState,
    now_secs: u64,
    now_ns: u64,
    sentinel_id: Principal,
) -> Result<u64, String> {
    let record = state::get_target(target).ok_or_else(|| "target not registered".to_string())?;
    let amount_cycles = record.funding_policy().refill_cycles();
    let fee_cycles = 1u128;
    let operation_id = state::next_operation_id();
    let created_at_time_ns = state::next_created_at_time_ns(now_ns).map_err(failure)?;
    let snapshot = CyclesWithdrawSnapshot {
        destination: target,
        from_subaccount: None,
        amount_cycles,
        fee_cycles,
        created_at_time_ns,
    };
    let op = FundingOperation::open(
        operation_id,
        target,
        record.revision(),
        record.funding_policy().clone(),
        FundingTrigger::ManualTopup,
        FundingRailArguments::Cycles(snapshot),
        amount_cycles,
        now_secs,
    )
    .map_err(failure)?;
    // The fixture is self-contained: it refreshes a large synthetic source
    // cache, then uses the exact same reservation helpers as production.
    let source = state::get_source_reserve();
    let source = if source.cache().is_some() {
        source
    } else {
        source
            .refresh(1_000_000_000_000_000_000u128, fee_cycles, now_secs)
            .map_err(failure)?
    };
    reserve_ordinary(
        operation_id,
        target,
        amount_cycles,
        now_secs,
        source,
        amount_cycles
            .checked_add(fee_cycles)
            .ok_or_else(|| "source amount overflow".to_string())?,
    )?;
    state::insert_operation(op.clone()).map_err(failure)?;
    let mut current = op;
    if !matches!(desired, TestCyclesState::PlannedReserved) {
        current = persist_transition(
            current.clone(),
            current
                .record_attempt(
                    FundingOperationState::Cycles(CyclesFundingState::Submitted),
                    now_secs,
                    FundingAttemptResultClass::Indeterminate,
                )
                .map_err(failure)?,
        )?;
    }
    if matches!(
        desired,
        TestCyclesState::Confirmed | TestCyclesState::Complete
    ) {
        current = persist_transition(
            current.clone(),
            current
                .record_attempt(
                    FundingOperationState::Cycles(CyclesFundingState::Confirmed),
                    now_secs,
                    FundingAttemptResultClass::Success,
                )
                .map_err(failure)?,
        )?;
        let with_block = current.attach_confirmed_block(1).map_err(failure)?;
        current = persist_transition(current.clone(), with_block)?;
    }
    if matches!(desired, TestCyclesState::Unknown) {
        current = persist_transition(
            current.clone(),
            current
                .record_attempt(
                    FundingOperationState::Cycles(CyclesFundingState::Unknown),
                    now_secs,
                    FundingAttemptResultClass::Indeterminate,
                )
                .map_err(failure)?,
        )?;
    }
    if matches!(desired, TestCyclesState::Terminal) {
        let from = current.clone();
        let _ = persist_transition(
            from.clone(),
            from.record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Terminal),
                now_secs,
                FundingAttemptResultClass::TerminalFailure,
            )
            .map_err(failure)?,
        )?;
    }
    if matches!(desired, TestCyclesState::Quarantined) {
        current = persist_transition(
            current.clone(),
            current
                .record_attempt(
                    FundingOperationState::Cycles(CyclesFundingState::Quarantined),
                    now_secs,
                    FundingAttemptResultClass::Indeterminate,
                )
                .map_err(failure)?,
        )?;
    }
    if matches!(desired, TestCyclesState::Complete) {
        let _ = persist_transition(
            current.clone(),
            current
                .record_attempt(
                    FundingOperationState::Cycles(CyclesFundingState::Complete),
                    now_secs,
                    FundingAttemptResultClass::Success,
                )
                .map_err(failure)?,
        )?;
    }
    if matches!(
        desired,
        TestCyclesState::Complete | TestCyclesState::Terminal
    ) {
        let target_state = state::get_target_reservation(target);
        let global_state = state::get_global_rolling_spend();
        let source_state = state::get_source_reserve();
        if matches!(desired, TestCyclesState::Complete) {
            state::set_target_reservation(
                target,
                target_state
                    .settle_spend(
                        target_state.in_flight_operation_id().unwrap(),
                        now_secs,
                        0,
                        ROLLING_CAP_WINDOW_SECS,
                    )
                    .map_err(failure)?,
            );
            state::set_global_rolling_spend(
                global_state
                    .settle(operation_id, now_secs, ROLLING_CAP_WINDOW_SECS)
                    .map_err(failure)?,
            );
            state::set_source_reserve(
                source_state
                    .settle(operation_id, amount_cycles + fee_cycles)
                    .map_err(failure)?,
            );
        } else {
            state::set_target_reservation(
                target,
                target_state
                    .release_no_spend(operation_id)
                    .map_err(failure)?,
            );
            state::set_global_rolling_spend(
                global_state
                    .release_no_spend(operation_id)
                    .map_err(failure)?,
            );
            state::set_source_reserve(source_state.settle(operation_id, 0).map_err(failure)?);
        }
    }
    let _ = sentinel_id;
    Ok(operation_id)
}

fn icp_operation(
    target: Principal,
    desired: TestIcpState,
    now_secs: u64,
    now_ns: u64,
    sentinel_id: Principal,
) -> Result<u64, String> {
    widen_global_cap_for_fixture();
    let record = state::get_target(target).ok_or_else(|| "target not registered".to_string())?;
    let refill_cycles = record.funding_policy().refill_cycles();
    let rate = IcpXdrConversionRate {
        timestamp_seconds: now_secs,
        xdr_permyriad_per_icp: 10_000,
    };
    let amount_e8s = icp_cmc::icp_amount_e8s_for_cycles(refill_cycles, rate).map_err(failure)?;
    let expected_cycles = icp_cmc::expected_cycles(amount_e8s, rate).map_err(failure)?;
    let fee_e8s = 1u64;
    let held_e8s = (amount_e8s as u128)
        .checked_add(fee_e8s as u128)
        .ok_or_else(|| "ICP source amount overflow".to_string())?;
    let operation_id = state::next_operation_id();
    let created_at_time_ns = state::next_created_at_time_ns(now_ns).map_err(failure)?;
    let snapshot = crate::types::IcpCmcSnapshot {
        source_principal: sentinel_id,
        ledger_principal: icp_cmc::icp_ledger_principal(),
        cmc_principal: icp_cmc::cmc_principal(),
        source_subaccount: None,
        cmc_account_identifier: icp_cmc::cmc_subaccount(target),
        target_canister: target,
        amount_e8s,
        fee_e8s,
        memo: icp_cmc::TPUP_MEMO,
        created_at_time_ns,
        rate_xdr_permyriad_per_icp: rate.xdr_permyriad_per_icp,
        rate_timestamp_secs: rate.timestamp_seconds,
        expected_cycles,
    };
    let op = FundingOperation::open(
        operation_id,
        target,
        record.revision(),
        record.funding_policy().clone(),
        FundingTrigger::ManualTopup,
        FundingRailArguments::Icp(snapshot),
        expected_cycles,
        now_secs,
    )
    .map_err(failure)?;
    let source = state::get_icp_source_reserve();
    let source = if source.cache().is_some() {
        source
    } else {
        source
            .refresh(1_000_000_000_000_000_000u128, fee_e8s as u128, now_secs)
            .map_err(failure)?
    };
    // The fixture must install the refreshed value into stable state before
    // reserving.  Merely computing a local cache here leaves
    // `reserve_icp` to reread `UnknownCache`, which makes this test seam fail
    // whenever it is used immediately after boot.
    state::set_icp_source_reserve(source.clone());
    // Reserve amount is the immutable expected cycle amount; source hold is
    // the exact ICP amount plus its ledger fee.
    let _ = source;
    reserve_icp(operation_id, target, expected_cycles, held_e8s, now_secs)?;
    state::insert_operation(op.clone()).map_err(failure)?;
    let mut current = op;
    if !matches!(desired, TestIcpState::PlannedReserved) {
        current = persist_transition(
            current.clone(),
            current
                .record_attempt(
                    FundingOperationState::Icp(IcpFundingState::LedgerSubmitted),
                    now_secs,
                    FundingAttemptResultClass::Indeterminate,
                )
                .map_err(failure)?,
        )?;
    }
    if matches!(
        desired,
        TestIcpState::TransferUnknown
            | TestIcpState::TransferConfirmed
            | TestIcpState::NotifyPending
            | TestIcpState::Complete
            | TestIcpState::Refunded
            | TestIcpState::Terminal
    ) {
        if matches!(desired, TestIcpState::TransferUnknown) {
            current = persist_transition(
                current.clone(),
                current
                    .record_attempt(
                        FundingOperationState::Icp(IcpFundingState::TransferUnknown),
                        now_secs,
                        FundingAttemptResultClass::Indeterminate,
                    )
                    .map_err(failure)?,
            )?;
        } else {
            current = persist_transition(
                current.clone(),
                current
                    .record_attempt(
                        FundingOperationState::Icp(IcpFundingState::TransferConfirmed),
                        now_secs,
                        FundingAttemptResultClass::Success,
                    )
                    .map_err(failure)?,
            )?;
            let with_block = current.attach_confirmed_block(1).map_err(failure)?;
            current = persist_transition(current.clone(), with_block)?;
        }
    }
    if matches!(
        desired,
        TestIcpState::NotifyPending
            | TestIcpState::Complete
            | TestIcpState::Refunded
            | TestIcpState::Terminal
    ) {
        current = persist_transition(
            current.clone(),
            current
                .record_attempt(
                    FundingOperationState::Icp(IcpFundingState::NotifyPending),
                    now_secs,
                    FundingAttemptResultClass::Indeterminate,
                )
                .map_err(failure)?,
        )?;
    }
    if matches!(desired, TestIcpState::Complete) {
        // Complete ICP operations require CMC actual-cycles evidence. Build
        // both fields in memory and persist one valid successor; persisting
        // the intermediate Complete-without-evidence value would make the
        // stable decoder reject the operation on its next access.
        let complete = current
            .record_attempt(
                FundingOperationState::Icp(IcpFundingState::Complete),
                now_secs,
                FundingAttemptResultClass::Success,
            )
            .map_err(failure)?;
        let complete_with_actual = complete
            .attach_actual_cycles(expected_cycles)
            .map_err(failure)?;
        current = persist_transition(current.clone(), complete_with_actual)?;
    }
    if matches!(desired, TestIcpState::Terminal) {
        current = persist_transition(
            current.clone(),
            current
                .record_attempt(
                    FundingOperationState::Icp(IcpFundingState::Terminal),
                    now_secs,
                    FundingAttemptResultClass::TerminalFailure,
                )
                .map_err(failure)?,
        )?;
    }
    if matches!(desired, TestIcpState::Quarantined) {
        current = persist_transition(
            current.clone(),
            current
                .record_attempt(
                    FundingOperationState::Icp(IcpFundingState::Quarantined),
                    now_secs,
                    FundingAttemptResultClass::Indeterminate,
                )
                .map_err(failure)?,
        )?;
    }
    if matches!(desired, TestIcpState::Refunded) {
        // Walk the exact three persisted writes production performs, because
        // each one is the legal predecessor of the next:
        //
        // 1. `funding::icp::resolve_notify`'s `RefundedWithBlock` arm attaches
        //    the CMC hint while the operation is still `NotifyPending` and
        //    persists the hint together with the single `NotifyPending ->
        //    Quarantined` transition.  A quarantined record without the hint
        //    can never become `Refunded`: `reconcile_quarantined_icp` rejects
        //    it with `RefundBlockRequired`, which `state::update_operation`
        //    surfaces as `InvalidTransition`.
        let hinted = current.attach_refund_block_hint(2).map_err(failure)?;
        current = persist_transition(
            current.clone(),
            hinted
                .record_attempt(
                    FundingOperationState::Icp(IcpFundingState::Quarantined),
                    now_secs,
                    FundingAttemptResultClass::Indeterminate,
                )
                .map_err(failure)?,
        )?;
        // 2. `funding::icp::attach_refund_block_proof` persists the verified
        //    ledger block as its own same-state evidence step, the only write
        //    `is_valid_icp_refund_proof_attachment` accepts.
        let attached = current.attach_refund_block(2).map_err(failure)?;
        current = persist_transition(current.clone(), attached)?;
        // 3. Only then is the explicit `Quarantined -> Refunded`
        //    reconciliation a legal successor of what is actually stored.
        let refunded = current
            .reconcile_quarantined_icp(IcpFundingState::Refunded, None, None, None, now_secs)
            .map_err(failure)?;
        persist_transition(current.clone(), refunded)?;
    }
    if matches!(
        desired,
        TestIcpState::Complete | TestIcpState::Refunded | TestIcpState::Terminal
    ) {
        let target_state = state::get_target_reservation(target);
        let global_state = state::get_global_rolling_spend();
        let source_state = state::get_icp_source_reserve();
        let target_state = target_state
            .release_no_spend(operation_id)
            .map_err(failure)?;
        let global_state = global_state
            .release_no_spend(operation_id)
            .map_err(failure)?;
        let source_state = source_state
            .settle(operation_id, held_e8s)
            .map_err(failure)?;
        state::set_target_reservation(target, target_state);
        state::set_global_rolling_spend(global_state);
        state::set_icp_source_reserve(source_state);
    }
    Ok(operation_id)
}

pub(crate) fn inject_operation(
    target: Principal,
    desired: TestOperationState,
    now_secs: u64,
    now_ns: u64,
    sentinel_id: Principal,
) -> Result<u64, String> {
    // Widen once for the fixture so a single canister instance can hold one
    // operation per target while the suite walks every lifecycle state.
    // Production governance and policy are untouched because this module is
    // compiled only with `test_endpoints`.
    widen_global_cap_for_fixture();
    match desired {
        TestOperationState::Cycles(state) => {
            cycles_operation(target, state, now_secs, now_ns, sentinel_id)
        }
        TestOperationState::Icp(state) => {
            icp_operation(target, state, now_secs, now_ns, sentinel_id)
        }
    }
}
