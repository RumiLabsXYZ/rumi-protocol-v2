//! Target observation and immutable blackhole verification.

use candid::{CandidType, Nat, Principal};
use rumi_cycle_manager::CycleManagerCyclesStatus;
use serde::{Deserialize, Serialize};

use crate::types::{AdvisoryCyclesBalance, ObservationMode, PublicTargetState};
use crate::{history, state};

pub const BLACKHOLE_PRINCIPAL_TEXT: &str = "e3mmv-5qaaa-aaaah-aadma-cai";
pub const PINNED_BLACKHOLE_HASH: [u8; 32] = [
    0x21, 0x0c, 0x94, 0x1e, 0x5e, 0xca, 0x77, 0xda, 0xac, 0x31, 0x4d, 0xa9, 0x15, 0x17, 0x48, 0x3a,
    0xc1, 0x71, 0x26, 0x45, 0x27, 0xe3, 0xd0, 0xd7, 0x13, 0xb9, 0x2b, 0xb9, 0x52, 0x39, 0xd7, 0xde,
];

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanisterState {
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "stopping")]
    Stopping,
    #[serde(rename = "stopped")]
    Stopped,
}

/// The smallest exact subset of the management-canister `canister_status`
/// record needed by Sentinel. Candid record decoding safely ignores fields
/// not used by this projection, while retaining the actual field labels and
/// nesting of the blackhole interface.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct BlackholeSettings {
    pub controllers: Vec<Principal>,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct BlackholeStatus {
    pub status: CanisterState,
    pub settings: BlackholeSettings,
    pub module_hash: Option<Vec<u8>>,
    pub cycles: Nat,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObservationError {
    Decode,
    ProxyHashMismatch,
    ProxyControllersMismatch,
    TargetControllersMismatch,
    BalanceOverflow,
    CallFailed,
    UnregisteredTarget,
    Storage,
    StaleRegistry,
}

/// Observe and persist one target sample. This is the sampler-facing path;
/// it has no Candid entry point and is never used by anonymous queries.
pub async fn sample_target(
    target: Principal,
    now_secs: u64,
) -> Result<crate::types::Sample, ObservationError> {
    let record = state::get_target(target).ok_or(ObservationError::UnregisteredTarget)?;
    let observation = match record.observation_mode() {
        ObservationMode::SelfReport => {
            observe_self_report(
                target,
                record.funding_policy().low_balance_threshold_cycles(),
            )
            .await
        }
        ObservationMode::BlackholeRelay => {
            observe_blackhole(
                target,
                record.funding_policy().low_balance_threshold_cycles(),
            )
            .await
        }
        ObservationMode::Unobserved => Observation {
            balance: None,
            state: PublicTargetState::Unobserved,
            reported_healthy: None,
            observation_mode: ObservationMode::Unobserved,
        },
    };
    // Governance may edit/remove/re-register a target while the external
    // status call is awaiting. Never persist an observation under a changed
    // policy snapshot.
    let current = state::get_target(target).ok_or(ObservationError::StaleRegistry)?;
    // Revision numbers alone are not sufficient here: remove-and-re-register
    // can legitimately restart a new record at revision one.  Compare the
    // complete immutable/governed snapshot so no result can be attributed to
    // a replacement target.
    if current != record {
        return Err(ObservationError::StaleRegistry);
    }
    let prior = state::latest_sample(target);
    let start_secs = prior.as_ref().map(|sample| sample.timestamp_secs);
    let confirmed: Vec<u128> =
        state::list_terminal_summaries_for_target(target, crate::types::MAX_TERMINAL_SUMMARIES)
            .into_iter()
            .filter(|summary| summary.outcome() == crate::types::FundingOutcome::Completed)
            .filter(|summary| {
                start_secs.is_none_or(|start| {
                    summary.resolved_at_secs() > start && summary.resolved_at_secs() <= now_secs
                })
            })
            .map(|summary| summary.amount_cycles())
            .collect();
    let (summary_count, oldest_retained_resolved_at_secs) = state::terminal_summary_coverage();
    // Any unresolved operation remains a possible credit until reconciled;
    // this is conservative for NotifyPending/quarantined work whose final
    // ledger effect may arrive after its last local update.
    let unknown = state::list_unresolved_ordinary_operations()
        .into_iter()
        .any(|op| op.target() == target);
    let raw_burn = history::calculate_burn_for_interval(
        start_secs,
        summary_count,
        crate::types::MAX_TERMINAL_SUMMARIES,
        oldest_retained_resolved_at_secs,
        prior.as_ref().and_then(|sample| {
            sample
                .balance
                .as_ref()
                .and_then(AdvisoryCyclesBalance::low_balance_value)
        }),
        observation
            .balance
            .as_ref()
            .and_then(AdvisoryCyclesBalance::low_balance_value),
        &confirmed,
        unknown,
    );
    let burn = raw_burn.and_then(|raw| {
        start_secs
            .and_then(|start| history::normalize_burn_per_hour(raw, now_secs.saturating_sub(start)))
    });
    let sample = crate::types::Sample {
        timestamp_secs: now_secs,
        balance: observation.balance,
        state: observation.state,
        reported_operational_healthy: observation.reported_healthy,
        burn_cycles_per_hour: burn,
    };
    state::record_sample(target, sample.clone()).map_err(|_| ObservationError::Storage)?;
    apply_burn_anomaly(
        target,
        sample.burn_cycles_per_hour,
        record.funding_policy().burn_anomaly_limit_cycles_per_day(),
        now_secs,
    );
    Ok(sample)
}

/// A burn-rate overflow is itself anomalous.  Saturating multiplication would
/// turn that condition into a possibly-small number and could suppress the
/// fail-safe pause.
fn burn_anomaly_exceeds(hourly: u128, daily_limit: u128) -> bool {
    hourly
        .checked_mul(24)
        .is_none_or(|daily| daily > daily_limit)
}

/// Apply the observation-side safety transition. `None` burn, including an
/// unknown funding interval, deliberately does nothing: indeterminate data
/// must not either raise a false anomaly or clear an existing one.
pub(crate) fn apply_burn_anomaly(
    target: Principal,
    hourly: Option<u128>,
    daily_limit: Option<u128>,
    now_secs: u64,
) {
    match (hourly, daily_limit) {
        (Some(hourly), Some(limit)) if burn_anomaly_exceeds(hourly, limit) => {
            let _ = state::alarms::raise_at(
                Some(target),
                crate::types::AlarmKind::BurnAnomaly,
                now_secs,
            );
            state::pause_target_for_anomaly(target);
        }
        (Some(_), Some(_)) => {
            state::alarms::resolve_at(Some(target), crate::types::AlarmKind::BurnAnomaly, now_secs);
        }
        _ => {}
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub balance: Option<AdvisoryCyclesBalance>,
    pub state: PublicTargetState,
    /// Self-report operational health is advisory and is never used for the
    /// funded-state decision.
    pub reported_healthy: Option<bool>,
    pub observation_mode: ObservationMode,
}

impl Observation {
    pub fn healthy(balance: u128) -> Self {
        Self {
            balance: Some(AdvisoryCyclesBalance::Exact(balance)),
            state: PublicTargetState::Healthy,
            reported_healthy: None,
            observation_mode: ObservationMode::BlackholeRelay,
        }
    }
}

pub fn blackhole_principal() -> Principal {
    Principal::from_text(BLACKHOLE_PRINCIPAL_TEXT).expect("pinned blackhole principal is valid")
}

pub fn verify_proxy_status(
    proxy: Principal,
    status: &BlackholeStatus,
) -> Result<(), ObservationError> {
    if status.module_hash.as_deref() != Some(PINNED_BLACKHOLE_HASH.as_slice()) {
        return Err(ObservationError::ProxyHashMismatch);
    }
    if status.settings.controllers.as_slice() != [proxy] {
        return Err(ObservationError::ProxyControllersMismatch);
    }
    Ok(())
}

pub fn classify_blackhole_target(
    status: &BlackholeStatus,
    proxy: Principal,
    threshold: u128,
) -> Result<Observation, ObservationError> {
    if !status.settings.controllers.contains(&proxy) {
        return Err(ObservationError::TargetControllersMismatch);
    }
    let balance = AdvisoryCyclesBalance::from_nat(&status.cycles);
    if balance.low_balance_value().is_none() {
        return Ok(Observation {
            balance: None,
            state: PublicTargetState::Unreachable,
            reported_healthy: None,
            observation_mode: ObservationMode::BlackholeRelay,
        });
    }
    if status.module_hash.as_ref().is_none_or(Vec::is_empty) {
        return Ok(Observation {
            balance: Some(balance),
            state: PublicTargetState::Uninstalled,
            reported_healthy: None,
            observation_mode: ObservationMode::BlackholeRelay,
        });
    }
    if status.status != CanisterState::Running {
        return Ok(Observation {
            balance: Some(balance),
            state: PublicTargetState::Stopped,
            reported_healthy: None,
            observation_mode: ObservationMode::BlackholeRelay,
        });
    }
    let value = balance.low_balance_value().expect("overflow handled above");
    Ok(Observation {
        balance: Some(balance),
        state: if value <= threshold {
            PublicTargetState::Low
        } else {
            PublicTargetState::Healthy
        },
        reported_healthy: None,
        observation_mode: ObservationMode::BlackholeRelay,
    })
}

/// Decode only the established Cycle Manager wire contract. Classification is
/// deliberately a separate operation because target-reported `low_watermark`
/// is advisory and must never replace the registry threshold.
pub fn decode_self_report(bytes: &[u8]) -> Result<CycleManagerCyclesStatus, ObservationError> {
    candid::decode_one(bytes).map_err(|_| ObservationError::Decode)
}

pub fn classify_self_report(status: &CycleManagerCyclesStatus, threshold: u128) -> Observation {
    let balance = AdvisoryCyclesBalance::from_nat(&status.balance);
    let Some(value) = balance.low_balance_value() else {
        return Observation {
            balance: None,
            state: PublicTargetState::Unreachable,
            reported_healthy: Some(status.healthy),
            observation_mode: ObservationMode::SelfReport,
        };
    };
    Observation {
        balance: Some(balance),
        state: if value <= threshold {
            PublicTargetState::Low
        } else {
            PublicTargetState::Healthy
        },
        reported_healthy: Some(status.healthy),
        observation_mode: ObservationMode::SelfReport,
    }
}

pub fn classify_self_report_bytes(
    bytes: &[u8],
    threshold: u128,
) -> Result<Observation, ObservationError> {
    let status: CycleManagerCyclesStatus =
        candid::decode_one(bytes).map_err(|_| ObservationError::Decode)?;
    Ok(classify_self_report(&status, threshold))
}

/// Read a self-reporting target. Public queries never call this function; it
/// is reserved for the timer/sampler introduced by the integration task.
pub async fn observe_self_report(target: Principal, threshold: u128) -> Observation {
    match ic_cdk::call::<(), (CycleManagerCyclesStatus,)>(target, "cycles_status", ()).await {
        Ok((status,)) => classify_self_report(&status, threshold),
        Err(_) => Observation {
            balance: None,
            state: PublicTargetState::Unreachable,
            reported_healthy: None,
            observation_mode: ObservationMode::SelfReport,
        },
    }
}

pub async fn observe_blackhole(target: Principal, threshold: u128) -> Observation {
    let proxy = blackhole_principal();
    let own = ic_cdk::call::<(BlackholeRequest,), (BlackholeStatus,)>(
        proxy,
        "canister_status",
        (BlackholeRequest { canister_id: proxy },),
    )
    .await;
    if let Ok((own_status,)) = own {
        if verify_proxy_status(proxy, &own_status).is_err() {
            return unreachable_observation(ObservationMode::BlackholeRelay);
        }
    } else {
        return unreachable_observation(ObservationMode::BlackholeRelay);
    }
    match ic_cdk::call::<(BlackholeRequest,), (BlackholeStatus,)>(
        proxy,
        "canister_status",
        (BlackholeRequest {
            canister_id: target,
        },),
    )
    .await
    {
        Ok((status,)) => classify_blackhole_target(&status, proxy, threshold)
            .unwrap_or_else(|_| unreachable_observation(ObservationMode::BlackholeRelay)),
        Err(_) => unreachable_observation(ObservationMode::BlackholeRelay),
    }
}

fn unreachable_observation(mode: ObservationMode) -> Observation {
    Observation {
        balance: None,
        state: PublicTargetState::Unreachable,
        reported_healthy: None,
        observation_mode: mode,
    }
}

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlackholeRequest {
    pub canister_id: Principal,
}

#[cfg(test)]
mod tests {
    use super::*;
    use candid::{Nat, Principal};

    fn proxy() -> Principal {
        Principal::from_text(BLACKHOLE_PRINCIPAL_TEXT).unwrap()
    }
    fn target() -> Principal {
        Principal::from_slice(&[42, 1])
    }
    fn status(
        controllers: Vec<Principal>,
        module_hash: Option<Vec<u8>>,
        state: CanisterState,
        cycles: u128,
    ) -> BlackholeStatus {
        BlackholeStatus {
            settings: BlackholeSettings { controllers },
            module_hash,
            status: state,
            cycles: Nat::from(cycles),
        }
    }

    #[test]
    fn verifies_proxy_hash_and_exact_self_controller() {
        let good = status(
            vec![proxy()],
            Some(PINNED_BLACKHOLE_HASH.to_vec()),
            CanisterState::Running,
            1,
        );
        assert_eq!(verify_proxy_status(proxy(), &good), Ok(()));
        assert!(verify_proxy_status(
            proxy(),
            &status(
                vec![],
                Some(PINNED_BLACKHOLE_HASH.to_vec()),
                CanisterState::Running,
                1
            )
        )
        .is_err());
        assert!(verify_proxy_status(
            proxy(),
            &status(
                vec![proxy(), target()],
                Some(PINNED_BLACKHOLE_HASH.to_vec()),
                CanisterState::Running,
                1
            )
        )
        .is_err());
        assert!(verify_proxy_status(
            proxy(),
            &status(vec![proxy()], Some(vec![1; 32]), CanisterState::Running, 1)
        )
        .is_err());
    }

    #[test]
    fn verifies_target_proxy_running_and_installed_invariants_independently() {
        let good = status(
            vec![proxy(), target()],
            Some(vec![7; 32]),
            CanisterState::Running,
            99,
        );
        assert_eq!(
            classify_blackhole_target(&good, proxy(), 90),
            Ok(Observation::healthy(99))
        );
        assert!(classify_blackhole_target(
            &status(
                vec![target()],
                Some(vec![7; 32]),
                CanisterState::Running,
                99
            ),
            proxy(),
            100
        )
        .is_err());
        assert!(classify_blackhole_target(
            &status(vec![proxy(), target()], None, CanisterState::Running, 99),
            proxy(),
            100
        )
        .is_ok_and(|x| x.state == PublicTargetState::Uninstalled));
        assert!(classify_blackhole_target(
            &status(
                vec![proxy(), target()],
                Some(vec![7; 32]),
                CanisterState::Stopped,
                99
            ),
            proxy(),
            100
        )
        .is_ok_and(|x| x.state == PublicTargetState::Stopped));
        let stopped = classify_blackhole_target(
            &status(
                vec![proxy(), target()],
                Some(vec![7; 32]),
                CanisterState::Stopped,
                99,
            ),
            proxy(),
            100,
        )
        .unwrap();
        assert_eq!(stopped.balance, Some(AdvisoryCyclesBalance::Exact(99)));
    }

    #[test]
    fn exact_threshold_is_low_and_overflow_is_not_a_low_trigger() {
        let at = status(
            vec![proxy(), target()],
            Some(vec![7; 32]),
            CanisterState::Running,
            100,
        );
        assert_eq!(
            classify_blackhole_target(&at, proxy(), 100).unwrap().state,
            PublicTargetState::Low
        );
        let over = BlackholeStatus {
            cycles: Nat::from(u128::MAX) + Nat::from(1u8),
            ..at
        };
        assert_eq!(
            classify_blackhole_target(&over, proxy(), 100)
                .unwrap()
                .state,
            PublicTargetState::Unreachable
        );
    }

    #[test]
    fn every_public_state_is_distinct_and_unobserved_has_no_balance() {
        let installed = Some(vec![7; 32]);
        assert_eq!(
            classify_blackhole_target(
                &status(
                    vec![proxy(), target()],
                    installed.clone(),
                    CanisterState::Running,
                    101
                ),
                proxy(),
                100,
            )
            .unwrap()
            .state,
            PublicTargetState::Healthy
        );
        assert_eq!(
            classify_blackhole_target(
                &status(
                    vec![proxy(), target()],
                    installed.clone(),
                    CanisterState::Running,
                    100
                ),
                proxy(),
                100,
            )
            .unwrap()
            .state,
            PublicTargetState::Low
        );
        assert_eq!(
            classify_blackhole_target(
                &status(
                    vec![proxy(), target()],
                    installed.clone(),
                    CanisterState::Stopped,
                    100
                ),
                proxy(),
                100,
            )
            .unwrap()
            .state,
            PublicTargetState::Stopped
        );
        assert_eq!(
            classify_blackhole_target(
                &status(vec![proxy(), target()], None, CanisterState::Running, 100),
                proxy(),
                100,
            )
            .unwrap()
            .state,
            PublicTargetState::Uninstalled
        );
        let overflow = BlackholeStatus {
            cycles: Nat::from(u128::MAX) + Nat::from(1u8),
            ..status(
                vec![proxy(), target()],
                installed,
                CanisterState::Running,
                100,
            )
        };
        assert_eq!(
            classify_blackhole_target(&overflow, proxy(), 100)
                .unwrap()
                .state,
            PublicTargetState::Unreachable
        );
        let unobserved = Observation {
            balance: None,
            state: PublicTargetState::Unobserved,
            reported_healthy: None,
            observation_mode: ObservationMode::Unobserved,
        };
        assert_eq!(unobserved.state, PublicTargetState::Unobserved);
        assert_eq!(unobserved.balance, None);
    }

    #[test]
    fn burn_anomaly_overflow_is_fail_safe_and_clean_value_is_not_anomaly() {
        assert!(burn_anomaly_exceeds(u128::MAX, u128::MAX));
        assert!(!burn_anomaly_exceeds(1, 24));
        assert!(burn_anomaly_exceeds(2, 24));
    }

    #[test]
    fn decodes_self_report_using_cycle_manager_contract() {
        let value = rumi_cycle_manager::cycles_status_from_parts(10, 5, true, 1);
        let bytes = candid::encode_one(value).unwrap();
        let result = decode_self_report(&bytes).unwrap();
        assert_eq!(result.balance, Nat::from(10u8));
        assert_eq!(
            classify_self_report(&result, 5).state,
            PublicTargetState::Healthy
        );
    }

    #[test]
    fn self_report_low_watermark_cannot_override_registry_threshold() {
        let status = rumi_cycle_manager::cycles_status_from_parts(50, 1_000, false, 1);
        let classified = classify_self_report(&status, 100);
        assert_eq!(classified.state, PublicTargetState::Low);
        assert_eq!(classified.reported_healthy, Some(false));
    }

    #[test]
    fn status_tags_round_trip_with_lowercase_serde_labels() {
        // Encode the actual management-canister wire type, then decode into
        // the exact local subset.  Encoding and decoding the same custom enum
        // would not prove compatibility with the management interface.
        let encoded =
            candid::encode_one(ic_cdk::api::management_canister::main::CanisterStatusType::Running)
                .unwrap();
        assert_eq!(
            candid::decode_one::<CanisterState>(&encoded).unwrap(),
            CanisterState::Running
        );
    }

    #[test]
    fn malformed_self_report_is_unreachable_without_balance() {
        let result = decode_self_report(&[0, 1, 2]).unwrap_err();
        assert_eq!(result, ObservationError::Decode);
    }
}
