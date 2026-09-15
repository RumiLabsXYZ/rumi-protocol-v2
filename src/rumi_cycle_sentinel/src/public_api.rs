//! Anonymous, cached telemetry projections and opaque cursor helpers.

use candid::{CandidType, Nat, Principal};
use serde::{Deserialize, Serialize};

use crate::state;
use crate::types::{
    self, Criticality, Environment, FundingOperation, ObservationMode, PageError, PermissionsView,
    ProposalRecord, PublicAlarm, PublicOverview, PublicPage, PublicTargetRow, PublicTargetState,
    RecentTopups, Sample, TargetRecord,
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

/// Errors returned by signer-only read methods.  Public telemetry queries use
/// `PublicQueryError`; keeping the authorization error in this separate type
/// makes it impossible for a future public projection to accidentally become
/// a signer information oracle.
#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthenticatedQueryError {
    NotSigner,
    InvalidCursor,
    FutureCursor,
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

// Keep the projection helper's explicit inputs aligned with the public row's
// fixed fields; grouping them would obscure the Candid-shaped mapping.
#[allow(clippy::too_many_arguments)]
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

// Keep the projection helper's explicit inputs aligned with the public row's
// fixed fields; grouping them would obscure the Candid-shaped mapping.
#[allow(clippy::too_many_arguments)]
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
        state: if !target.enabled() || target.observation_mode() == ObservationMode::Unobserved {
            PublicTargetState::Unobserved
        } else {
            PublicTargetState::Unreachable
        },
        reported_operational_healthy: None,
        burn_cycles_per_hour: None,
    };
    // Disabled inventory entries are deliberately never observed.  Ignore a
    // stale sample from an earlier enabled period and render them as
    // Unobserved until governance enables them again.
    let selected = if target.enabled() {
        sample.as_ref().unwrap_or(&fallback)
    } else {
        &fallback
    };
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
    row.next_sample_at_secs = target
        .enabled()
        .then_some(sample.as_ref())
        .flatten()
        .map(|sample| {
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

/// Projects a source-account cache into the amount that is actually
/// available for a new fallback/withdrawal operation.  The cache is only a
/// point-in-time observation, so it is not shown once it is outside the same
/// freshness window used by the corresponding funding rail.  Pending holds
/// include the amount plus fee and must be removed before exposing an
/// available balance; arithmetic failure is treated as unknown rather than
/// clamped to zero (or wrapped).
fn available_from_cached_balance(
    balance: u128,
    pending: Option<u128>,
    protected_floor: u128,
    as_of_secs: u64,
    now_secs: u64,
    max_age_secs: u64,
) -> Option<u128> {
    if as_of_secs > now_secs || now_secs.saturating_sub(as_of_secs) > max_age_secs {
        return None;
    }
    balance
        .checked_sub(pending?)
        .and_then(|available| available.checked_sub(protected_floor))
}

/// Returns only source capacity that can safely be spent by the ordinary
/// rail.  The Cycles Ledger cache is denominated in cycles, while the
/// protected self-recovery floor is also cycles, so both are subtracted here.
fn cycles_ledger_available_cycles(now_secs: u64) -> Option<Nat> {
    let policy = state::global_config().global_policy;
    let reserve = state::get_source_reserve();
    let cache = reserve.cache()?;
    available_from_cached_balance(
        cache.balance_cycles,
        reserve.pending_total_cycles().ok(),
        policy.self_recovery_policy().protected_reserve_cycles(),
        cache.as_of_secs,
        now_secs,
        policy.stale_after_secs(),
    )
    .map(Nat::from)
}

/// Returns only ICP capacity that can safely be spent by the CMC fallback.
/// `min_icp_reserve_e8s` is a governed floor and is therefore not advertised
/// as spendable availability.  The ICP source cache intentionally has its
/// own freshness bound because it gates an actual ledger debit.
fn icp_available_e8s(now_secs: u64) -> Option<Nat> {
    let policy = state::global_config().global_policy;
    let reserve = state::get_icp_source_reserve();
    let cache = reserve.cache()?;
    available_from_cached_balance(
        cache.balance_e8s,
        reserve.pending_total_e8s().ok(),
        policy.min_icp_reserve_e8s(),
        cache.as_of_secs,
        now_secs,
        types::icp_source_cache_max_age_secs(),
    )
    .map(Nat::from)
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
        cycles_ledger_available_cycles: cycles_ledger_available_cycles(now_secs),
        icp_available_e8s: icp_available_e8s(now_secs),
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

/// Signer-only governance projection.  The proposal payloads include the
/// exact requested mutation and approvals, so this is deliberately separate
/// from the anonymous cached telemetry surface.  Cursor encoding is shared
/// with the other bounded history queries but remains opaque to callers.
pub fn list_governance_proposals_at(
    caller: Principal,
    cursor: Option<String>,
    limit: u16,
) -> Result<PublicPage<ProposalRecord>, AuthenticatedQueryError> {
    if !state::is_signer(caller) {
        return Err(AuthenticatedQueryError::NotSigner);
    }
    let proposal_id = cursor
        .map(|value| decode_cursor(&value).ok_or(AuthenticatedQueryError::InvalidCursor))
        .transpose()?;
    let page_size = bounded_page_limit(limit);
    let mut proposals = state::list_proposals_after(proposal_id, page_size + 1);
    let has_more = proposals.len() > page_size;
    if has_more {
        proposals.pop();
    }
    let next_cursor = has_more
        .then(|| proposals.last().map(|proposal| encode_cursor(proposal.id)))
        .flatten();
    PublicPage::try_new(proposals, next_cursor).map_err(|_| AuthenticatedQueryError::TooManyItems)
}

/// Signer-only funding-operation inventory. `resolve_unknown_as_spent` and
/// `attach_block_proof` both require an `operation_id`, but the anonymous
/// public telemetry projections (`list_public_topups_at`,
/// `list_public_samples_at`, ...) intentionally omit operation ids and any
/// in-flight/quarantined detail. This is the only surface that gives a
/// signer the exact id plus full immutable snapshot and current state of
/// every live unresolved/quarantined `FundingOperation`, so reconciliation
/// never has to guess an id. Cursor encoding is shared with the other
/// bounded history queries but remains opaque to callers.
pub fn list_unresolved_funding_operations_at(
    caller: Principal,
    cursor: Option<String>,
    limit: u16,
) -> Result<PublicPage<FundingOperation>, AuthenticatedQueryError> {
    if !state::is_signer(caller) {
        return Err(AuthenticatedQueryError::NotSigner);
    }
    let operation_id = cursor
        .map(|value| decode_cursor(&value).ok_or(AuthenticatedQueryError::InvalidCursor))
        .transpose()?;
    let page_size = bounded_page_limit(limit);
    let mut operations = state::list_unresolved_operations_after(operation_id, page_size + 1);
    let has_more = operations.len() > page_size;
    if has_more {
        operations.pop();
    }
    let next_cursor = has_more
        .then(|| operations.last().map(|op| encode_cursor(op.id())))
        .flatten();
    PublicPage::try_new(operations, next_cursor).map_err(|_| AuthenticatedQueryError::TooManyItems)
}

pub fn get_my_permissions_at(
    caller: Principal,
) -> Result<PermissionsView, AuthenticatedQueryError> {
    if !state::is_signer(caller) {
        return Err(AuthenticatedQueryError::NotSigner);
    }
    Ok(PermissionsView { is_signer: true })
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

    #[test]
    fn available_balance_subtracts_pending_hold_and_protected_floor() {
        assert_eq!(
            available_from_cached_balance(100, Some(25), 10, 100, 100, 600),
            Some(65)
        );
    }

    #[test]
    fn available_balance_is_unknown_for_stale_or_future_cache() {
        assert_eq!(
            available_from_cached_balance(100, Some(0), 0, 1, 602, 600),
            None
        );
        assert_eq!(
            available_from_cached_balance(100, Some(0), 0, 603, 602, 600),
            None
        );
    }

    #[test]
    fn available_balance_fails_closed_on_hold_or_floor_underflow() {
        assert_eq!(
            available_from_cached_balance(100, Some(101), 0, 100, 100, 600),
            None
        );
        assert_eq!(
            available_from_cached_balance(100, Some(25), 76, 100, 100, 600),
            None
        );
        assert_eq!(
            available_from_cached_balance(100, None, 0, 100, 100, 600),
            None
        );
    }

    // ── `list_unresolved_funding_operations_at` (signer-only operation inventory) ──

    fn test_signer(seed: u8) -> Principal {
        Principal::from_slice(&[seed; 10])
    }

    fn test_target_principal(seed: u8) -> Principal {
        Principal::from_slice(&[7, seed])
    }

    fn test_funding_policy_args() -> types::TargetFundingPolicyArgs {
        types::TargetFundingPolicyArgs {
            low_balance_threshold_cycles: Nat::from(1u64),
            refill_cycles: Nat::from(10u64),
            daily_cap_cycles: Nat::from(100u64),
            cooldown_secs: 60,
            burn_anomaly_limit_cycles_per_day: None,
        }
    }

    fn test_global_policy() -> types::GlobalPolicy {
        types::GlobalPolicy::validate(&types::GlobalPolicyArgs {
            global_daily_cap_cycles: Nat::from(1_000_000u64),
            sample_interval_secs: 300,
            stale_after_secs: 600,
            min_icp_reserve_e8s: Nat::from(100_000_000u64),
            timelocks: types::GovernanceTimelocksArgs {
                target_registry_secs: 3_600,
                spend_policy_secs: 3_600,
                signer_change_secs: 3_600,
                unpause_secs: 3_600,
            },
            self_recovery_policy: types::SelfRecoveryPolicyArgs {
                protected_reserve_cycles: Nat::from(1_000u64),
                daily_cap_cycles: Nat::from(100u64),
                low_balance_threshold_cycles: Nat::from(1u64),
                refill_cycles: Nat::from(10u64),
            },
        })
        .unwrap()
    }

    fn set_only_signer(signer: Principal, global_policy: types::GlobalPolicy) {
        state::set_global_config(state::GlobalConfig {
            signers: vec![signer],
            approval_threshold: 1,
            global_policy,
        });
    }

    fn register_test_target(seed: u8, global: &types::GlobalPolicy) -> Principal {
        let empty = std::collections::BTreeSet::new();
        let ctx = types::TargetRegistrationContext {
            sentinel_id: Principal::from_slice(&[9; 5]),
            existing_target_count: 0,
            existing_target_principals: &empty,
            global_policy: global,
        };
        let args = types::TargetArgs {
            principal: test_target_principal(seed),
            display_name: "svc".to_string(),
            project: "proj".to_string(),
            environment: Environment::Production,
            criticality: Criticality::Standard,
            observation_mode: ObservationMode::SelfReport,
            tags: vec![],
            funding_policy: test_funding_policy_args(),
        };
        let record = types::TargetRecord::register(args, &ctx).unwrap();
        let principal = record.principal();
        state::insert_target(record).unwrap();
        principal
    }

    fn test_operation(
        id: u64,
        target: Principal,
        global: &types::GlobalPolicy,
    ) -> FundingOperation {
        let funding_policy =
            types::TargetFundingPolicy::validate(&test_funding_policy_args(), global).unwrap();
        let rail_arguments = types::FundingRailArguments::Cycles(types::CyclesWithdrawSnapshot {
            destination: target,
            from_subaccount: None,
            amount_cycles: 10,
            fee_cycles: 0,
            created_at_time_ns: 10_000_000_000,
        });
        FundingOperation::open(
            id,
            target,
            1,
            funding_policy,
            types::FundingTrigger::LowBalanceAutoTopup,
            rail_arguments,
            10,
            10,
        )
        .unwrap()
    }

    #[test]
    fn list_unresolved_funding_operations_at_rejects_non_signer() {
        set_only_signer(test_signer(1), test_global_policy());
        assert_eq!(
            list_unresolved_funding_operations_at(test_signer(2), None, 10),
            Err(AuthenticatedQueryError::NotSigner)
        );
    }

    #[test]
    fn list_unresolved_funding_operations_at_rejects_anonymous_caller() {
        set_only_signer(test_signer(1), test_global_policy());
        assert_eq!(
            list_unresolved_funding_operations_at(Principal::anonymous(), None, 10),
            Err(AuthenticatedQueryError::NotSigner)
        );
    }

    #[test]
    fn list_unresolved_funding_operations_at_returns_full_operation_for_signer() {
        let global = test_global_policy();
        let signer = test_signer(3);
        set_only_signer(signer, global.clone());
        let target = register_test_target(1, &global);
        let op = test_operation(1, target, &global);
        state::insert_operation(op.clone()).unwrap();

        let page = list_unresolved_funding_operations_at(signer, None, 10).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].id(), 1);
        assert_eq!(page.items[0].target(), target);
        assert_eq!(page.items[0].state(), op.state());
        assert_eq!(page.next_cursor, None);
    }

    #[test]
    fn list_unresolved_funding_operations_at_paginates_with_opaque_cursor() {
        let global = test_global_policy();
        let signer = test_signer(4);
        set_only_signer(signer, global.clone());
        let target = register_test_target(2, &global);
        for id in 1..=2u64 {
            state::insert_operation(test_operation(id, target, &global)).unwrap();
        }

        let first = list_unresolved_funding_operations_at(signer, None, 1).unwrap();
        assert_eq!(first.items.len(), 1);
        assert_eq!(first.items[0].id(), 1);
        let cursor = first
            .next_cursor
            .expect("a partial page must carry a cursor");
        assert_ne!(
            cursor, "1",
            "cursor must be opaque, not the raw operation id"
        );

        let second = list_unresolved_funding_operations_at(signer, Some(cursor), 1).unwrap();
        assert_eq!(second.items.len(), 1);
        assert_eq!(second.items[0].id(), 2);
        assert_eq!(second.next_cursor, None);
    }

    #[test]
    fn list_unresolved_funding_operations_at_rejects_malformed_cursor() {
        let signer = test_signer(5);
        set_only_signer(signer, test_global_policy());
        assert_eq!(
            list_unresolved_funding_operations_at(signer, Some("not-a-cursor".to_string()), 10),
            Err(AuthenticatedQueryError::InvalidCursor)
        );
    }
}
