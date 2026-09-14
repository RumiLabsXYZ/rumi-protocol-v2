//! Anonymous, cached telemetry projections and opaque cursor helpers.

use candid::{CandidType, Nat, Principal};
use serde::{Deserialize, Serialize};

use crate::state;
use crate::types::{
    self, Criticality, Environment, ObservationMode, PageError, PublicAlarm, PublicOverview,
    PublicPage, PublicTargetRow, PublicTargetState, RecentTopups, Sample, TargetRecord,
};

const CURSOR_MASK: u64 = 0x9e37_79b9_7f4a_7c15;

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicQueryError {
    InvalidCursor,
    FutureCursor,
    StaleCursor,
    TargetNotFound,
    TooManyItems,
}

impl From<PageError> for PublicQueryError {
    fn from(_: PageError) -> Self {
        Self::TooManyItems
    }
}

pub fn encode_cursor(sequence: u64) -> String {
    format!("v1{:016x}", sequence ^ CURSOR_MASK)
}

pub fn decode_cursor(value: &str) -> Option<u64> {
    value
        .strip_prefix("v1")
        .filter(|v| v.len() == 16)
        .and_then(|v| u64::from_str_radix(v, 16).ok())
        .map(|v| v ^ CURSOR_MASK)
}

pub fn bounded_page_limit(limit: u16) -> usize {
    usize::from(limit).clamp(1, types::MAX_PUBLIC_PAGE)
}

fn encode_principal_cursor(principal: Principal) -> String {
    let mut out = String::from("p1");
    for byte in principal.as_slice() {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn decode_principal_cursor(value: &str) -> Option<Principal> {
    let bytes = value.strip_prefix("p1")?;
    if bytes.len() % 2 != 0 {
        return None;
    }
    let mut decoded = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.as_bytes().chunks_exact(2) {
        decoded.push(u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?);
    }
    (decoded.len() <= 29).then(|| Principal::from_slice(&decoded))
}

fn sample_without_balance() -> Sample {
    Sample {
        timestamp_secs: 10,
        balance: None,
        state: PublicTargetState::Unreachable,
        reported_operational_healthy: None,
        burn_cycles_per_hour: None,
    }
}

pub fn public_row_from_sample(
    principal: Principal,
    display_name: String,
    environment: Environment,
    criticality: Criticality,
    observation_mode: ObservationMode,
    sample: &Sample,
    threshold: u128,
    refill: u128,
    recent_topups: RecentTopups,
) -> PublicTargetRow {
    public_row_from_samples(
        principal,
        display_name,
        environment,
        criticality,
        observation_mode,
        sample,
        None,
        threshold,
        refill,
        recent_topups,
    )
}

pub fn public_row_from_samples(
    principal: Principal,
    display_name: String,
    environment: Environment,
    criticality: Criticality,
    observation_mode: ObservationMode,
    latest: &Sample,
    last_good: Option<&Sample>,
    threshold: u128,
    refill: u128,
    recent_topups: RecentTopups,
) -> PublicTargetRow {
    let balance = latest
        .balance
        .as_ref()
        .or_else(|| last_good.and_then(|sample| sample.balance.as_ref()));
    let burn_per_day = latest
        .burn_cycles_per_hour
        .map(|v| Nat::from(v) * Nat::from(24u8));
    let runway = balance
        .and_then(|b| b.low_balance_value())
        .and_then(|balance| {
            latest
                .burn_cycles_per_hour
                .filter(|v| *v > 0)
                .and_then(|burn| u64::try_from(balance.saturating_mul(3_600) / burn).ok())
        });
    PublicTargetRow {
        principal,
        display_name,
        project: String::new(),
        environment,
        criticality,
        observation_mode,
        state: latest.state,
        reported_operational_healthy: latest.reported_operational_healthy,
        advisory_balance_cycles: balance.map(|b| b.to_nat()),
        advisory_balance_overflowed: balance.is_some_and(|b| b.is_overflow()),
        low_balance_threshold_cycles: Nat::from(threshold),
        refill_cycles: Nat::from(refill),
        burn_cycles_per_day: burn_per_day,
        runway_secs: runway,
        as_of_secs: latest.timestamp_secs,
        last_success_at_secs: (latest.balance.is_some()
            && latest.state != PublicTargetState::Unreachable)
            .then_some(latest.timestamp_secs),
        stale_for_secs: None,
        next_sample_at_secs: None,
        recent_topups,
    }
}

fn target_row(target: &TargetRecord, now_secs: u64) -> PublicTargetRow {
    let sample = state::latest_sample(target.principal());
    let last_good = state::latest_successful_sample(target.principal());
    let fallback = Sample {
        timestamp_secs: 0,
        balance: None,
        state: if target.observation_mode() == ObservationMode::Unobserved {
            PublicTargetState::Unobserved
        } else {
            PublicTargetState::Unreachable
        },
        reported_operational_healthy: None,
        burn_cycles_per_hour: None,
    };
    let selected = sample.as_ref().unwrap_or(&fallback);
    // A failed latest attempt remains visibly Unreachable, but the last
    // genuine balance is still useful context. Its stale age is explicit and
    // the sampler never uses this fallback to trigger funding.
    let recent_topups = recent_topups(target.principal());
    let mut row = public_row_from_samples(
        target.principal(),
        target.display_name().to_string(),
        target.environment(),
        target.criticality(),
        target.observation_mode(),
        selected,
        last_good.as_ref(),
        target.funding_policy().low_balance_threshold_cycles(),
        target.funding_policy().refill_cycles(),
        recent_topups,
    );
    row.project = target.project().to_string();
    let meta = state::sample_meta(target.principal());
    row.last_success_at_secs = meta.and_then(|m| m.last_success_at_secs);
    row.stale_for_secs = row
        .last_success_at_secs
        .map(|at| now_secs.saturating_sub(at));
    row.next_sample_at_secs = sample.as_ref().map(|sample| {
        sample
            .timestamp_secs
            .saturating_add(state::global_config().global_policy.sample_interval_secs())
    });
    row
}

fn recent_topups(target: Principal) -> RecentTopups {
    let mut summaries =
        state::list_terminal_summaries_for_target(target, types::MAX_TERMINAL_SUMMARIES);
    summaries.sort_by_key(|summary| (summary.resolved_at_secs(), summary.operation_id()));
    let entries = summaries
        .into_iter()
        .rev()
        .take(types::MAX_RECENT_TOPUPS_PER_TARGET)
        .map(|summary| (&summary).into())
        .collect();
    RecentTopups::new(entries).expect("bounded recent topups")
}

pub fn get_public_overview_at(now_secs: u64) -> PublicOverview {
    let targets = state::list_targets_after(None, types::MAX_TARGETS);
    let mut overview = PublicOverview {
        target_count: targets.len() as u64,
        healthy_count: 0,
        low_count: 0,
        stopped_count: 0,
        uninstalled_count: 0,
        unreachable_count: 0,
        unobserved_count: 0,
        total_observed_cycles: Nat::from(0u8),
        runtime_cycles: Nat::from(ic_cdk::api::canister_balance128()),
        cycles_ledger_available_cycles: None,
        icp_available_e8s: None,
        protected_self_reserve_cycles: Some(Nat::from(
            state::global_config()
                .global_policy
                .self_recovery_policy()
                .protected_reserve_cycles(),
        )),
        alarm_count: 0,
        last_sample_at_secs: None,
        next_sample_at_secs: None,
    };
    for target in &targets {
        let row = target_row(target, now_secs);
        match row.state {
            PublicTargetState::Healthy => overview.healthy_count += 1,
            PublicTargetState::Low => overview.low_count += 1,
            PublicTargetState::Stopped => overview.stopped_count += 1,
            PublicTargetState::Uninstalled => overview.uninstalled_count += 1,
            PublicTargetState::Unreachable => overview.unreachable_count += 1,
            PublicTargetState::Unobserved => overview.unobserved_count += 1,
        }
        if let Some(balance) = row.advisory_balance_cycles {
            overview.total_observed_cycles += balance;
        }
        overview.last_sample_at_secs = overview.last_sample_at_secs.max(row.last_success_at_secs);
        overview.next_sample_at_secs = overview.next_sample_at_secs.max(row.next_sample_at_secs);
    }
    overview.alarm_count = state::list_alarms_after(None, types::MAX_ALARMS)
        .iter()
        .filter(|a| a.status != types::AlarmStatus::Resolved)
        .count() as u64;
    overview
}

pub fn list_public_targets_at(
    cursor: Option<String>,
    limit: u16,
    now_secs: u64,
) -> Result<PublicPage<PublicTargetRow>, PublicQueryError> {
    let principal = cursor
        .map(|c| decode_principal_cursor(&c).ok_or(PublicQueryError::InvalidCursor))
        .transpose()?;
    let page_size = bounded_page_limit(limit);
    let mut rows = state::list_targets_after(principal, page_size + 1);
    let has_more = rows.len() > page_size;
    if has_more {
        rows.pop();
    }
    let next_cursor = has_more
        .then(|| {
            rows.last()
                .map(|target| encode_principal_cursor(target.principal()))
        })
        .flatten();
    PublicPage::try_new(
        rows.iter()
            .map(|target| target_row(target, now_secs))
            .collect(),
        next_cursor,
    )
    .map_err(Into::into)
}

pub fn get_public_target_at(principal: Principal, now_secs: u64) -> Option<PublicTargetRow> {
    state::get_target(principal).map(|target| target_row(&target, now_secs))
}

pub fn list_public_samples_at(
    principal: Principal,
    cursor: Option<String>,
    limit: u16,
) -> Result<PublicPage<Sample>, PublicQueryError> {
    if state::get_target(principal).is_none() {
        return Err(PublicQueryError::TargetNotFound);
    }
    let sequence = cursor
        .map(|c| decode_cursor(&c).ok_or(PublicQueryError::InvalidCursor))
        .transpose()?;
    let page_size = bounded_page_limit(limit);
    let mut rows =
        state::list_samples(principal, sequence, page_size + 1).map_err(|e| match e {
            state::SampleCursorError::FutureCursor => PublicQueryError::FutureCursor,
            state::SampleCursorError::StaleCursor => PublicQueryError::StaleCursor,
        })?;
    let has_more = rows.len() > page_size;
    if has_more {
        rows.pop();
    }
    let next_cursor = has_more
        .then(|| rows.last().map(|(seq, _)| encode_cursor(*seq)))
        .flatten();
    PublicPage::try_new(
        rows.into_iter().map(|(_, sample)| sample).collect(),
        next_cursor,
    )
    .map_err(Into::into)
}

pub fn list_public_topups_at(
    principal: Principal,
    cursor: Option<String>,
    limit: u16,
) -> Result<PublicPage<crate::types::PublicTopupSummary>, PublicQueryError> {
    if state::get_target(principal).is_none() {
        return Err(PublicQueryError::TargetNotFound);
    }
    let page_size = bounded_page_limit(limit);
    let operation_id = cursor
        .map(|c| decode_cursor(&c).ok_or(PublicQueryError::InvalidCursor))
        .transpose()?;
    let mut summaries =
        state::list_terminal_summaries_for_target_after(principal, operation_id, page_size + 1);
    let has_more = summaries.len() > page_size;
    if has_more {
        summaries.pop();
    }
    let next_cursor = has_more
        .then(|| {
            summaries
                .last()
                .map(|summary| encode_cursor(summary.operation_id()))
        })
        .flatten();
    let page = summaries.iter().map(Into::into).collect();
    PublicPage::try_new(page, next_cursor).map_err(Into::into)
}

pub fn list_public_alarms_at(
    cursor: Option<String>,
    limit: u16,
) -> Result<PublicPage<PublicAlarm>, PublicQueryError> {
    let sequence = cursor
        .map(|c| decode_cursor(&c).ok_or(PublicQueryError::InvalidCursor))
        .transpose()?;
    let page_size = bounded_page_limit(limit);
    let mut alarms = state::list_alarms_after(sequence, page_size + 1);
    let has_more = alarms.len() > page_size;
    if has_more {
        alarms.pop();
    }
    let next_cursor = has_more
        .then(|| alarms.last().map(|alarm| encode_cursor(alarm.id)))
        .flatten();
    PublicPage::try_new(alarms.iter().map(Into::into).collect(), next_cursor).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        Criticality, Environment, ObservationMode, PublicTargetState, RecentTopups,
    };
    use candid::Principal;

    #[test]
    fn cursor_is_opaque_and_round_trips() {
        let cursor = encode_cursor(17);
        assert_ne!(cursor, "17");
        assert_eq!(decode_cursor(&cursor), Some(17));
        assert_eq!(decode_cursor("not-a-cursor"), None);
    }

    #[test]
    fn page_limit_is_clamped_to_one_hundred() {
        assert_eq!(bounded_page_limit(0), 1);
        assert_eq!(bounded_page_limit(100), 100);
        assert_eq!(bounded_page_limit(101), 100);
    }

    #[test]
    fn no_balance_is_not_rendered_as_zero() {
        let row = public_row_from_sample(
            Principal::from_slice(&[1]),
            "x".into(),
            Environment::Production,
            Criticality::Standard,
            ObservationMode::SelfReport,
            &sample_without_balance(),
            10,
            20,
            RecentTopups::new(vec![]).unwrap(),
        );
        assert_eq!(row.advisory_balance_cycles, None);
        assert_eq!(row.state, PublicTargetState::Unreachable);
    }

    #[test]
    fn failed_latest_attempt_preserves_last_good_balance_as_stale() {
        let good = Sample {
            timestamp_secs: 10,
            balance: Some(crate::types::AdvisoryCyclesBalance::Exact(7)),
            state: PublicTargetState::Healthy,
            reported_operational_healthy: Some(true),
            burn_cycles_per_hour: Some(1),
        };
        let failed = Sample {
            timestamp_secs: 20,
            balance: None,
            state: PublicTargetState::Unreachable,
            reported_operational_healthy: None,
            burn_cycles_per_hour: None,
        };
        let row = public_row_from_samples(
            Principal::from_slice(&[1]),
            "x".into(),
            Environment::Production,
            Criticality::Standard,
            ObservationMode::SelfReport,
            &failed,
            Some(&good),
            100,
            10,
            RecentTopups::new(vec![]).unwrap(),
        );
        assert_eq!(row.state, PublicTargetState::Unreachable);
        assert_eq!(row.as_of_secs, 20);
        assert_eq!(row.advisory_balance_cycles, Some(candid::Nat::from(7u8)));
    }

    #[test]
    fn daily_burn_uses_exact_nat_at_u128_max_boundary() {
        let sample = Sample {
            timestamp_secs: 10,
            balance: Some(crate::types::AdvisoryCyclesBalance::Exact(1)),
            state: PublicTargetState::Healthy,
            reported_operational_healthy: Some(true),
            burn_cycles_per_hour: Some(u128::MAX),
        };
        let row = public_row_from_sample(
            Principal::from_slice(&[1]),
            "x".into(),
            Environment::Production,
            Criticality::Standard,
            ObservationMode::SelfReport,
            &sample,
            10,
            20,
            RecentTopups::new(vec![]).unwrap(),
        );
        assert_eq!(
            row.burn_cycles_per_day,
            Some(Nat::from(u128::MAX) * Nat::from(24u8))
        );
    }

    #[test]
    fn malformed_principal_cursor_is_rejected_without_trapping() {
        assert_eq!(
            decode_principal_cursor("p1".to_string().as_str()),
            Some(Principal::from_slice(&[]))
        );
        assert_eq!(decode_principal_cursor("p1zz"), None);
        assert_eq!(
            decode_principal_cursor(&format!("p1{}", "ff".repeat(30))),
            None
        );
    }
}
