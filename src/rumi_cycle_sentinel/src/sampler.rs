//! Cycle Sentinel's periodic maintenance loop.
//!
//! The timer is deliberately a thin scheduler around the durable state
//! machines in `self_recovery`, `funding`, and `observation`.  It does not
//! keep work in heap-only state: timer IDs and the in-flight bit are runtime
//! handles, while every operation that can cross an await is persisted by the
//! funding modules before the external call starts.
//!
//! The maintenance order is part of the safety contract and must stay:
//!
//! 1. self-recovery;
//! 2. resume pending operations;
//! 3. sample every registered target;
//! 4. evaluate new automatic top-ups.
//!
//! Public queries never call this module.  They project the stable/cache
//! state synchronously; only this timer (and explicit signer updates) may
//! perform inter-canister calls.

use std::cell::Cell;
use std::collections::BTreeSet;
#[cfg(target_arch = "wasm32")]
use std::time::Duration;

use candid::{Nat, Principal};

use crate::{funding, icp_cmc, observation, self_recovery, state, types};

/// The reviewed inventory imported at bootstrap.  Registry records are still
/// governed after installation; this table only makes the complete project
/// inventory visible from the first install and leaves every entry disabled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BootstrapTargetSpec {
    pub(crate) name: &'static str,
    pub(crate) principal_text: &'static str,
    pub(crate) observation_mode: types::ObservationMode,
}

/// Exactly the fifteen existing CycleOps entries plus the Conflux public
/// frontend.  Do not silently substitute a similarly named canister: the
/// principal is the identity that is eventually observed/funded.
pub(crate) const BOOTSTRAP_TARGETS: [BootstrapTargetSpec; 16] = [
    BootstrapTargetSpec {
        name: "rumi_points",
        principal_text: "bfnu3-6aaaa-aaaab-qhanq-cai",
        observation_mode: types::ObservationMode::SelfReport,
    },
    BootstrapTargetSpec {
        name: "arb_bot",
        principal_text: "ucjxv-nqaaa-aaaaj-qrsaq-cai",
        observation_mode: types::ObservationMode::BlackholeRelay,
    },
    BootstrapTargetSpec {
        name: "icusd_ledger",
        principal_text: "t6bor-paaaa-aaaap-qrd5q-cai",
        observation_mode: types::ObservationMode::BlackholeRelay,
    },
    BootstrapTargetSpec {
        name: "rumi_treasury",
        principal_text: "tlg74-oiaaa-aaaap-qrd6a-cai",
        observation_mode: types::ObservationMode::SelfReport,
    },
    BootstrapTargetSpec {
        name: "rumi_stability_pool",
        principal_text: "tmhzi-dqaaa-aaaap-qrd6q-cai",
        observation_mode: types::ObservationMode::SelfReport,
    },
    BootstrapTargetSpec {
        name: "rumi_protocol_backend",
        principal_text: "tfesu-vyaaa-aaaap-qrd7a-cai",
        observation_mode: types::ObservationMode::SelfReport,
    },
    BootstrapTargetSpec {
        name: "vault_frontend",
        principal_text: "tcfua-yaaaa-aaaap-qrd7q-cai",
        observation_mode: types::ObservationMode::BlackholeRelay,
    },
    BootstrapTargetSpec {
        name: "rumi_homepage",
        principal_text: "t2xrh-2aaaa-aaaap-qreaa-cai",
        observation_mode: types::ObservationMode::BlackholeRelay,
    },
    BootstrapTargetSpec {
        name: "icusd_index",
        principal_text: "6niqu-siaaa-aaaap-qrjeq-cai",
        observation_mode: types::ObservationMode::BlackholeRelay,
    },
    BootstrapTargetSpec {
        name: "test_icp_ledger_canister",
        principal_text: "pspic-iaaaa-aaaap-qrkna-cai",
        observation_mode: types::ObservationMode::BlackholeRelay,
    },
    BootstrapTargetSpec {
        name: "rumi_3pool",
        principal_text: "fohh4-yyaaa-aaaap-qtkpa-cai",
        observation_mode: types::ObservationMode::SelfReport,
    },
    BootstrapTargetSpec {
        name: "threeusd_index",
        principal_text: "jagpu-pyaaa-aaaap-qtm6q-cai",
        observation_mode: types::ObservationMode::BlackholeRelay,
    },
    BootstrapTargetSpec {
        name: "liquidation_bot",
        principal_text: "nygob-3qaaa-aaaap-qttcq-cai",
        observation_mode: types::ObservationMode::SelfReport,
    },
    BootstrapTargetSpec {
        name: "rumi_amm",
        principal_text: "ijlzs-2yaaa-aaaap-quaaq-cai",
        observation_mode: types::ObservationMode::SelfReport,
    },
    BootstrapTargetSpec {
        name: "rumi_analytics",
        principal_text: "dtlu2-uqaaa-aaaap-qugcq-cai",
        observation_mode: types::ObservationMode::SelfReport,
    },
    BootstrapTargetSpec {
        name: "conflux_public_frontend",
        principal_text: "a52ri-naaaa-aaaas-qgy4a-cai",
        observation_mode: types::ObservationMode::Unobserved,
    },
];

thread_local! {
    /// Timer IDs do not survive an upgrade; this slot is runtime-only and is
    /// always replaced by `setup_timer` after init/post-upgrade.
    static MAINTENANCE_TIMER: Cell<Option<ic_cdk_timers::TimerId>> = const { Cell::new(None) };
    /// Prevents a fast/overlapping interval callback from spawning a second
    /// maintenance future while the first one is awaiting external calls.
    static TICK_IN_FLIGHT: Cell<bool> = const { Cell::new(false) };
    /// The deadline of the one-shot runtime timer currently armed by this
    /// canister.  It is deliberately transient: upgrades and governed policy
    /// changes re-arm it from the live policy.  Manual maintenance never
    /// changes it, so public telemetry cannot imply that a manual check
    /// postponed the scheduled one.
    static NEXT_SCHEDULED_CHECK_AT_SECS: Cell<Option<u64>> = const { Cell::new(None) };
    /// Test-only completion generation.  This is runtime state, not stable
    /// protocol state, and is deliberately omitted from production Candid.
    #[cfg(feature = "test_endpoints")]
    static COMPLETED_TICK_GENERATION: Cell<u64> = const { Cell::new(0) };
}

/// The next actual, transient timer deadline.  This is intentionally not
/// reconstructed from target observations: a manual maintenance pass changes
/// those observations but never changes the timer schedule.
pub(crate) fn next_scheduled_check_at_secs() -> Option<u64> {
    NEXT_SCHEDULED_CHECK_AT_SECS.with(Cell::get)
}

fn deadline_after(now_ns: u64, interval_secs: u64) -> u64 {
    now_ns.saturating_add(interval_secs.saturating_mul(1_000_000_000)) / 1_000_000_000
}

fn record_next_scheduled_check(now_ns: u64, interval_secs: u64) {
    NEXT_SCHEDULED_CHECK_AT_SECS
        .with(|deadline| deadline.set(Some(deadline_after(now_ns, interval_secs))));
}

#[cfg(feature = "test_endpoints")]
pub(crate) fn completed_tick_generation() -> u64 {
    COMPLETED_TICK_GENERATION.with(Cell::get)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BootstrapError {
    InvalidTarget(types::TargetValidationError),
    InsertTarget(state::InsertTargetError),
}

/// Install the reviewed inventory on a fresh canister.  `TargetRecord::register`
/// is intentionally used instead of a struct literal so all target policy
/// bounds are checked and the record's constructor-enforced
/// `enabled=false/auto_topup=false` defaults remain in force.
pub(crate) fn bootstrap_targets(sentinel_id: Principal) -> Result<(), BootstrapError> {
    let global = state::global_config();
    // The bootstrap policy is a safe, valid placeholder.  It is not an
    // activation decision: every record remains disabled until governance
    // supplies the reviewed per-target values.
    let daily_cap = global
        .global_policy
        .global_daily_cap_cycles()
        .min(types::MAX_TARGET_DAILY_CAP_CYCLES);
    let refill = daily_cap.min(types::MAX_REFILL_CYCLES);
    let threshold = refill.min(types::MAX_LOW_BALANCE_THRESHOLD_CYCLES);
    debug_assert!(daily_cap > 0 && refill > 0 && threshold > 0);

    for spec in BOOTSTRAP_TARGETS {
        let principal = Principal::from_text(spec.principal_text)
            .expect("Cycle Sentinel bootstrap principal must be valid");
        let existing = state::target_principals();
        let context = types::TargetRegistrationContext {
            sentinel_id,
            existing_target_count: state::target_count() as usize,
            existing_target_principals: &existing,
            global_policy: &global.global_policy,
        };
        let record = types::TargetRecord::register(
            types::TargetArgs {
                principal,
                display_name: spec.name.to_string(),
                project: "Rumi Protocol".to_string(),
                environment: types::Environment::Production,
                criticality: types::Criticality::Standard,
                observation_mode: spec.observation_mode,
                tags: vec![],
                funding_policy: types::TargetFundingPolicyArgs {
                    low_balance_threshold_cycles: Nat::from(threshold),
                    refill_cycles: Nat::from(refill),
                    daily_cap_cycles: Nat::from(daily_cap),
                    cooldown_secs: 0,
                    burn_anomaly_limit_cycles_per_day: None,
                },
            },
            &context,
        )
        .map_err(BootstrapError::InvalidTarget)?;
        state::insert_target(record).map_err(BootstrapError::InsertTarget)?;
    }
    Ok(())
}

/// (Re-)arm the maintenance interval.  Timers are transient runtime handles,
/// so calling this on both init and post-upgrade is required; clearing first
/// also makes repeated setup idempotent during tests or future policy reloads.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn setup_timer() {}

#[cfg(target_arch = "wasm32")]
pub(crate) fn setup_timer() {
    MAINTENANCE_TIMER.with(|slot| {
        if let Some(previous) = slot.take() {
            ic_cdk_timers::clear_timer(previous);
        }
    });
    // In the pinned timer implementation `clear_timer` removes the task
    // before a queued self-call can invoke its closure.  A closure already
    // running has no await before it arms its successor, so a governed policy
    // update executes afterwards and clears that successor before arming the
    // policy's replacement.  An old callback therefore cannot restore an old
    // cadence after a policy reload.
    arm_next_timer();
}

/// `ic-cdk-timers`' interval helper re-arms from its outer global-timer
/// dispatch timestamp, which is not exposed to this canister.  Use a
/// one-shot timer instead so we record the exact deadline calculated from the
/// callback's clock and the currently governed policy.  That makes the
/// public deadline truthful and keeps manual checks out of scheduling.
#[cfg(target_arch = "wasm32")]
fn arm_next_timer() {
    let interval_secs = state::global_config()
        .global_policy
        .sample_interval_secs()
        .max(1);
    let now_ns = ic_cdk::api::time();
    record_next_scheduled_check(now_ns, interval_secs);
    let timer = ic_cdk_timers::set_timer(Duration::from_secs(interval_secs), || {
        // Re-arm before starting potentially long maintenance work.  The
        // shared in-flight guard below still prevents overlapping passes.
        arm_next_timer();
        if let Ok(guard) = TickGuard::acquire() {
            ic_cdk::spawn(async move {
                run_tick_with_guard(guard).await;
            });
        }
    });
    MAINTENANCE_TIMER.with(|slot| slot.set(Some(timer)));
}

struct TickGuard;

impl TickGuard {
    fn acquire() -> Result<Self, MaintenanceError> {
        TICK_IN_FLIGHT.with(|in_flight| {
            if in_flight.get() {
                Err(MaintenanceError::Busy)
            } else {
                in_flight.set(true);
                Ok(Self)
            }
        })
    }
}

impl Drop for TickGuard {
    fn drop(&mut self) {
        // This runs even when an awaited future is aborted, so one failed
        // maintenance pass cannot permanently wedge future timer callbacks.
        TICK_IN_FLIGHT.with(|in_flight| in_flight.set(false));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MaintenanceError {
    Busy,
}

impl std::fmt::Display for MaintenanceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => formatter.write_str("maintenance already running"),
        }
    }
}

/// Runs exactly the same guarded maintenance pass as the timer.  This starts
/// no timer and changes no policy, so a signer-requested check cannot delay
/// the independent scheduled callback.
pub(crate) async fn run_maintenance_now() -> Result<(), MaintenanceError> {
    let guard = TickGuard::acquire()?;
    run_tick_with_guard(guard).await;
    Ok(())
}

async fn run_tick_with_guard(_guard: TickGuard) {
    let now_ns = ic_cdk::api::time();
    let now_secs = now_ns / 1_000_000_000;
    tick_at(now_secs, now_ns, ic_cdk::id()).await;
    #[cfg(feature = "test_endpoints")]
    COMPLETED_TICK_GENERATION.with(|generation| {
        generation.set(generation.get().saturating_add(1));
    });
}

/// One complete maintenance pass.  The explicit clock/identity parameters
/// keep ordering and identity seams inspectable in deterministic tests; the
/// production timer passes the actual canister identity and IC clock.
pub(crate) async fn tick_at(now_secs: u64, now_ns: u64, sentinel_id: Principal) {
    // 1. Self-recovery is first.  It may suppress all ordinary distribution
    // for this round when the protected reserve is low or unresolved.
    let self_recovery_ok = self_recovery::run(now_secs, now_ns, sentinel_id).await;

    // 2. Resume durable pending operations before opening any new operation.
    // Snapshot the list before awaiting; each executor re-reads the durable
    // operation and reservations, so edits/removals cannot mutate a stale
    // in-flight snapshot or create a second operation.
    let pending = state::list_nonterminal_operations();
    // A resolved operation resumed during this pass must not be followed by
    // a second automatic evaluation against the same sample.  This matters
    // when cooldown_secs == 0: the sample is intentionally refreshed later
    // in this tick, but it still describes the pre-top-up balance.
    let mut resumed_targets = BTreeSet::new();
    for operation in pending {
        match operation.rail() {
            types::FundingRail::CyclesLedger => {
                // Self-recovery already resumed its own lane in step 1.  It
                // must never be sent through the ordinary ICP fallback.
                if operation.trigger() == types::FundingTrigger::SelfRecovery {
                    continue;
                }
                if let Ok((resolved, outcome)) =
                    funding::cycles::resume_with_outcome(operation.id(), now_secs).await
                {
                    resumed_targets.insert(resolved.target());
                    maybe_fallback_after_no_spend(
                        resolved.target(),
                        resolved.trigger(),
                        outcome,
                        now_secs,
                        now_ns,
                        sentinel_id,
                    )
                    .await;
                }
            }
            types::FundingRail::IcpCmc => {
                if let Ok(resolved) =
                    funding::icp::resume(operation.id(), now_secs, sentinel_id).await
                {
                    // A resumed operation may have completed, quarantined, or
                    // otherwise advanced while this tick awaited an external
                    // ledger/CMC reply. Do not start another rail for the same
                    // target later in this tick.
                    resumed_targets.insert(resolved.target());
                }
            }
        }
    }

    // 3. Sample every registered target.  A sample failure is represented by
    // an explicit Unreachable row/alarm; it is never treated as zero cycles.
    let targets = state::list_targets_after(None, types::MAX_TARGETS);
    for target in targets {
        // Disabled registry entries are inventory only.  They remain
        // Unobserved and must not incur self-report/blackhole calls or alarms
        // until governance enables them after preflight.
        if !target.enabled() {
            continue;
        }
        let principal = target.principal();
        match observation::sample_target(principal, now_secs).await {
            Ok(sample) => update_observation_alarms(principal, sample.state, now_secs),
            Err(observation::ObservationError::StaleRegistry)
            | Err(observation::ObservationError::UnregisteredTarget) => {
                // A governance edit/remove raced the external observation;
                // do not raise an alarm for a target snapshot we did not
                // persist.
            }
            Err(_) => {
                let _ = state::alarms::raise_at(
                    Some(principal),
                    types::AlarmKind::Unreachable,
                    now_secs,
                );
            }
        }
    }

    // Source caches are deliberately refreshed by the timer, never by public
    // query projection.  Refresh after sampling and before step 4 so a new
    // top-up attempt has a fresh reserve/fee snapshot without changing the
    // required safety order above.
    let _ = funding::refresh_cycles_ledger_cache(now_secs, sentinel_id).await;
    let _ = funding::icp::refresh_icp_ledger_cache(now_secs, sentinel_id).await;

    // 4. Do not open new ordinary operations while self-recovery is unresolved
    // or unable to protect its own runtime balance.
    if !self_recovery_ok {
        return;
    }
    for target in state::list_targets_after(None, types::MAX_TARGETS) {
        if !target.enabled() || resumed_targets.contains(&target.principal()) {
            continue;
        }
        match funding::cycles::run_ordinary_with_outcome(
            target.principal(),
            types::FundingTrigger::LowBalanceAutoTopup,
            now_secs,
            now_ns,
        )
        .await
        {
            Ok((resolved, outcome)) => {
                maybe_fallback_after_no_spend(
                    resolved.target(),
                    resolved.trigger(),
                    outcome,
                    now_secs,
                    now_ns,
                    sentinel_id,
                )
                .await
            }
            // `prepare_ordinary` only returns this after consuming a fresh
            // source-cache snapshot and proving balance < pending + floor +
            // exact withdrawal. No stale/unknown/ledger-error condition is
            // admitted to the ICP conversion lane.
            Err(funding::cycles::FundingError::SourceReserve(
                types::SourceReserveError::InsufficientReserve,
            )) => {
                maybe_convert_shared_reserve_after_proven_insufficiency(
                    target.principal(),
                    sentinel_id,
                )
                .await;
            }
            Err(_) => {}
        }
    }
}

async fn maybe_convert_shared_reserve_after_proven_insufficiency(
    target: Principal,
    sentinel_id: Principal,
) {
    let rate = match icp_cmc::query_rate(icp_cmc::cmc_principal()).await {
        Ok(rate) => rate,
        Err(_) => return,
    };
    // The rate query is an await. Refresh the admission timestamp so cache
    // and rate freshness are evaluated when the durable ICP reservation is
    // opened, not when this timer pass began.
    let admission_now_ns = ic_cdk::api::time();
    let _ = funding::icp::run_after_proven_cycles_insufficient(
        target,
        types::FundingTrigger::LowBalanceAutoTopup,
        admission_now_ns / 1_000_000_000,
        admission_now_ns,
        rate,
        sentinel_id,
    )
    .await;
}

fn update_observation_alarms(target: Principal, status: types::PublicTargetState, now_secs: u64) {
    match status {
        types::PublicTargetState::Low => {
            let _ = state::alarms::raise_at(Some(target), types::AlarmKind::LowBalance, now_secs);
        }
        types::PublicTargetState::Unreachable => {
            let _ = state::alarms::raise_at(Some(target), types::AlarmKind::Unreachable, now_secs);
        }
        _ => {
            state::alarms::resolve_at(Some(target), types::AlarmKind::LowBalance, now_secs);
            state::alarms::resolve_at(Some(target), types::AlarmKind::Unreachable, now_secs);
        }
    }
}

/// Admit the ICP rail only when the Cycles Ledger result proves that no
/// source debit occurred.  Unknown, duplicate, and any known-debit outcome
/// remain on the original durable Cycles operation and are never converted
/// into a second source debit.
async fn maybe_fallback_after_no_spend(
    target: Principal,
    trigger: types::FundingTrigger,
    outcome: crate::cycles_ledger::WithdrawOutcome,
    _now_secs: u64,
    _now_ns: u64,
    sentinel_id: Principal,
) {
    if trigger == types::FundingTrigger::SelfRecovery
        || !funding::icp::can_fallback_after_cycles(outcome)
    {
        return;
    }
    let rate = match icp_cmc::query_rate(icp_cmc::cmc_principal()).await {
        Ok(rate) => rate,
        Err(_) => return,
    };
    let admission_now_ns = ic_cdk::api::time();
    let _ = funding::icp::run_after_cycles_no_spend(
        target,
        trigger,
        admission_now_ns / 1_000_000_000,
        admission_now_ns,
        outcome,
        rate,
        sentinel_id,
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_inventory_is_exactly_sixteen_and_unique() {
        let mut principals = std::collections::BTreeSet::new();
        for spec in BOOTSTRAP_TARGETS {
            let principal = Principal::from_text(spec.principal_text).unwrap();
            assert!(principals.insert(principal), "duplicate {}", spec.name);
        }
        assert_eq!(principals.len(), 16);
        assert_eq!(
            BOOTSTRAP_TARGETS
                .iter()
                .filter(|spec| spec.observation_mode == types::ObservationMode::Unobserved)
                .map(|spec| spec.name)
                .collect::<Vec<_>>(),
            vec!["conflux_public_frontend"]
        );
    }

    #[test]
    fn generic_blackhole_candidates_are_explicitly_listed_for_later_preflight() {
        let names = BOOTSTRAP_TARGETS
            .iter()
            .filter(|spec| spec.observation_mode == types::ObservationMode::BlackholeRelay)
            .map(|spec| spec.name)
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "arb_bot",
                "icusd_ledger",
                "vault_frontend",
                "rumi_homepage",
                "icusd_index",
                "test_icp_ledger_canister",
                "threeusd_index",
            ]
        );
        // Activation is a TargetRecord constructor invariant, not a property
        // of the observation-mode table.  The table intentionally contains no
        // enabled/auto-topup fields that could bypass that invariant.
    }

    #[test]
    fn deadline_uses_the_live_interval_from_the_callback_clock() {
        assert_eq!(deadline_after(10_500_000_000, 14_400), 14_410);
        assert_eq!(deadline_after(u64::MAX, 1), u64::MAX / 1_000_000_000);
    }

    #[test]
    fn recording_a_deadline_is_independent_of_maintenance_work() {
        record_next_scheduled_check(7_000_000_000, 14_400);
        assert_eq!(next_scheduled_check_at_secs(), Some(14_407));
        NEXT_SCHEDULED_CHECK_AT_SECS.with(|deadline| deadline.set(None));
    }

    #[test]
    fn shared_guard_reports_busy_and_releases_when_dropped() {
        TICK_IN_FLIGHT.with(|in_flight| in_flight.set(false));
        let guard = TickGuard::acquire().expect("first maintenance pass starts");
        assert!(matches!(TickGuard::acquire(), Err(MaintenanceError::Busy)));
        drop(guard);
        let second_guard = TickGuard::acquire().expect("dropped guard releases the tick");
        drop(second_guard);
        TICK_IN_FLIGHT.with(|in_flight| in_flight.set(false));
    }
}
