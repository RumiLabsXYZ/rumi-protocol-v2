// Native Clippy builds do not retain the generated IC entrypoint references
// that connect these private modules in the Wasm canister. Keep native-host
// dead-code noise scoped to this crate; all other warnings remain strict.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use candid::Principal;
use rumi_cycle_manager::{
    self_cycles_status, CycleManagerCyclesStatus, DEFAULT_FREEZE_THRESHOLD_SECS,
    DEFAULT_LOW_WATERMARK_CYCLES,
};

mod cycles_ledger;
mod funding;
mod governance;
mod history;
mod icp_cmc;
mod icrc21;
mod observation;
mod public_api;
mod sampler;
mod self_recovery;
mod state;
mod telemetry_access;
#[cfg(feature = "test_endpoints")]
mod test_support;
mod types;

/// The IC wall clock, in seconds — every `governance::*_at` function takes
/// this as an explicit parameter rather than reading `ic_cdk::api::time()`
/// itself, so this is the one place that conversion happens.
fn now_secs() -> u64 {
    ic_cdk::api::time() / 1_000_000_000
}

/// The IC wall clock, in nanoseconds — feeds `funding`/`self_recovery`'s
/// globally monotonic `created_at_time` allocation
/// (`state::next_created_at_time_ns`), which needs genuine nanosecond
/// precision rather than `now_secs() * 1_000_000_000`.
fn now_ns() -> u64 {
    ic_cdk::api::time()
}

#[ic_cdk::init]
fn init(args: types::InitArgs) {
    if let Err(err) = state::init(args) {
        ic_cdk::trap(&format!("rumi_cycle_sentinel: init failed: {err:?}"));
    }
    if let Err(err) = sampler::bootstrap_targets(ic_cdk::id()) {
        ic_cdk::trap(&format!(
            "rumi_cycle_sentinel: bootstrap inventory failed: {err:?}"
        ));
    }
    sampler::setup_timer();
}

#[ic_cdk::post_upgrade]
fn post_upgrade() {
    // A source transfer marker represents only an in-message await.  Upgrade
    // interrupts that await, so clear the ephemeral marker while preserving
    // the durable operation, source hold, and exact retry snapshot.
    state::reset_icp_source_attempts_on_upgrade();
    // A notify marker represents only an in-message CMC await. Upgrade
    // interrupts that await; clear the marker while retaining the exact
    // transfer block so the same notify call can be retried safely.
    state::reset_icp_notify_attempts_on_upgrade();
    if let Err(err) = state::validate_whole_state(ic_cdk::id()) {
        ic_cdk::trap(&format!(
            "rumi_cycle_sentinel: post_upgrade state validation failed: {err:?}"
        ));
    }
    state::migrate_single_operator_setup_usage_on_upgrade();
    sampler::setup_timer();
}

const MAX_ANONYMOUS_CONSENT_INGRESS_BYTES: usize = 16 * 1024;

fn sentinel_ingress_allowed(caller: Principal, method: &str, argument_bytes: usize) -> bool {
    if caller != Principal::anonymous() {
        return true;
    }
    method == "icrc21_canister_call_consent_message"
        && argument_bytes <= MAX_ANONYMOUS_CONSENT_INGRESS_BYTES
}

/// Reject anonymous update ingress before Candid decoding, except for the
/// bounded standard consent-message request. Method bodies retain their own auth.
#[ic_cdk::inspect_message]
fn inspect_message() {
    let allowed = sentinel_ingress_allowed(
        ic_cdk::caller(),
        &ic_cdk::api::call::method_name(),
        ic_cdk::api::call::arg_data_raw_size(),
    );
    if allowed {
        ic_cdk::api::call::accept_message();
    }
}

#[ic_cdk::query(guard = "require_telemetry_viewer")]
fn cycles_status() -> CycleManagerCyclesStatus {
    self_cycles_status(
        DEFAULT_LOW_WATERMARK_CYCLES,
        true,
        DEFAULT_FREEZE_THRESHOLD_SECS,
    )
}

// ─────────────────────────── Governance (Task 2A) ───────────────────────────
//
// Every wrapper below does nothing but read the IC execution context
// (`ic_cdk::caller()`, `now_secs()`, `ic_cdk::id()`) and hand it to the
// matching deterministic `governance::*_at` function — see governance.rs's
// module doc for why the caller/clock/canister-id are threaded as explicit
// parameters instead of read there directly.

#[ic_cdk::update]
fn propose_register_target(args: types::TargetArgs) -> Result<u64, governance::GovernanceError> {
    governance::propose_register_target_at(ic_cdk::caller(), now_secs(), ic_cdk::id(), args)
}

#[ic_cdk::update]
fn propose_update_target(
    principal: Principal,
    patch: types::TargetPatch,
) -> Result<u64, governance::GovernanceError> {
    governance::propose_update_target_at(ic_cdk::caller(), now_secs(), principal, patch)
}

#[ic_cdk::update]
fn propose_update_targets(
    updates: Vec<types::TargetUpdate>,
) -> Result<u64, governance::GovernanceError> {
    governance::propose_update_targets_at(ic_cdk::caller(), now_secs(), updates)
}

#[ic_cdk::update]
fn propose_remove_target(principal: Principal) -> Result<u64, governance::GovernanceError> {
    governance::propose_remove_target_at(ic_cdk::caller(), now_secs(), principal)
}

#[ic_cdk::update]
fn propose_set_global_policy(
    args: types::GlobalPolicyArgs,
) -> Result<u64, governance::GovernanceError> {
    governance::propose_set_global_policy_at(ic_cdk::caller(), now_secs(), args)
}

#[ic_cdk::update]
fn propose_add_signer(signer: Principal) -> Result<u64, governance::GovernanceError> {
    governance::propose_add_signer_at(ic_cdk::caller(), now_secs(), signer)
}

#[ic_cdk::update]
fn propose_remove_signer(signer: Principal) -> Result<u64, governance::GovernanceError> {
    governance::propose_remove_signer_at(ic_cdk::caller(), now_secs(), signer)
}

#[ic_cdk::update]
fn propose_set_signer_threshold(threshold: u32) -> Result<u64, governance::GovernanceError> {
    governance::propose_set_signer_threshold_at(ic_cdk::caller(), now_secs(), threshold)
}

#[ic_cdk::update]
fn propose_unpause_target(principal: Principal) -> Result<u64, governance::GovernanceError> {
    governance::propose_unpause_target_at(ic_cdk::caller(), now_secs(), principal)
}

#[ic_cdk::update]
fn approve_proposal(id: u64) -> Result<bool, governance::GovernanceError> {
    governance::approve_proposal_at(ic_cdk::caller(), id)
}

#[ic_cdk::update]
fn execute_proposal(id: u64) -> Result<(), governance::GovernanceError> {
    governance::execute_proposal_at(ic_cdk::caller(), now_secs(), ic_cdk::id(), id)
}

#[ic_cdk::update]
fn cancel_proposal(id: u64) -> Result<(), governance::GovernanceError> {
    governance::cancel_proposal_at(ic_cdk::caller(), id)
}

#[ic_cdk::update]
fn pause_target(principal: Principal) -> Result<(), governance::GovernanceError> {
    governance::pause_target_at(ic_cdk::caller(), principal)
}

// ─────────────────────────── Signer queries and funding updates ───────────────────────────

#[ic_cdk::query]
fn get_my_permissions() -> Result<types::PermissionsView, public_api::AuthenticatedQueryError> {
    public_api::get_my_permissions_at(ic_cdk::caller())
}

/// Explicitly replaces the signer set with the three user-designated
/// operator principals and sets the approval quorum to one. Funding policy
/// and its timelocks are untouched. Approvals on existing open proposals are
/// cleared so no action inherits approvals from the previous quorum.
#[ic_cdk::update(guard = "require_telemetry_operator")]
fn configure_single_operator_governance(
) -> Result<types::OperatorDashboard, types::SingleOperatorSetupError> {
    public_api::configure_single_operator_governance_at(ic_cdk::caller(), now_secs())
}

/// Wallet signers request this standard, unauthenticated update before they
/// forward a call.  It only returns a bounded human-readable description and
/// never changes Sentinel state or grants authority.
#[ic_cdk::update]
fn icrc21_canister_call_consent_message(
    request: icrc21::ConsentMessageRequest,
) -> icrc21::ConsentMessageResult {
    icrc21::icrc21_canister_call_consent_message(request)
}

#[ic_cdk::query]
fn icrc10_supported_standards() -> Vec<icrc21::StandardRecord> {
    icrc21::icrc10_supported_standards()
}

#[ic_cdk::query]
fn list_governance_proposals(
    cursor: Option<String>,
    limit: u16,
) -> Result<types::PublicPage<types::ProposalRecord>, public_api::AuthenticatedQueryError> {
    public_api::list_governance_proposals_at(ic_cdk::caller(), cursor, limit)
}

/// Signer-only inventory of every live unresolved/quarantined
/// `FundingOperation`, with its immutable `operation_id` and full state —
/// the reconciliation input `resolve_unknown_as_spent`/`attach_block_proof`
/// need but the anonymous public telemetry projections never expose.
#[ic_cdk::query]
fn list_unresolved_funding_operations(
    cursor: Option<String>,
    limit: u16,
) -> Result<types::PublicPage<types::FundingOperation>, public_api::AuthenticatedQueryError> {
    public_api::list_unresolved_funding_operations_at(ic_cdk::caller(), cursor, limit)
}

fn require_maintenance_signer(
    caller: Principal,
    is_signer: impl FnOnce() -> bool,
) -> Result<(), String> {
    if caller == Principal::anonymous() {
        return Err("anonymous caller not allowed".to_string());
    }
    if !is_signer() {
        return Err(format!("{:?}", governance::GovernanceError::NotSigner));
    }
    Ok(())
}

/// Requests one full maintenance pass using the timer's existing durable
/// policy, funding, and recovery path.  It neither changes the timer nor
/// overrides caps, cooldowns, reserves, or governance configuration.
#[ic_cdk::update]
async fn run_maintenance_now() -> Result<(), String> {
    let caller = ic_cdk::caller();
    // Authenticate before acquiring the shared work guard or beginning any
    // observation/funding work.  Anonymous traffic therefore cannot start a
    // maintenance pass or cause source calls by repeatedly requesting one.
    require_maintenance_signer(caller, || state::is_signer(caller))?;
    sampler::run_maintenance_now()
        .await
        .map_err(|err| err.to_string())
}

/// Manual maintenance uses the same Cycles Ledger state machine as the timer.
/// The timer may additionally choose the ICP/CMC fallback after a proven
/// no-spend result; it never trusts a caller-provided rail or amount.
#[ic_cdk::update]
async fn manual_top_up(target: Principal) -> Result<types::FundingOperation, String> {
    funding::manual_top_up_at(ic_cdk::caller(), now_secs(), now_ns(), target)
        .await
        .map_err(|err| format!("{err:?}"))
}

/// Request a signer-authorized top-up using an explicit rail and raw amount.
/// CyclesLedger amounts are cycles; IcpCmc amounts are ICP e8s.
#[ic_cdk::update]
async fn manual_top_up_with_amount(
    target: Principal,
    rail: types::FundingRail,
    amount: u128,
) -> Result<types::FundingOperation, String> {
    funding::manual_top_up_with_amount_at(
        ic_cdk::caller(),
        now_secs(),
        now_ns(),
        target,
        rail,
        amount,
    )
    .await
}

/// Refuse evidence-free signer resolution. Ambiguous ICP/CMC and Cycles Ledger
/// operations remain held until their rail-specific proof path verifies evidence.

#[ic_cdk::update]
fn resolve_unknown_as_spent(operation_id: u64) -> Result<types::FundingOperation, String> {
    if !state::is_signer(ic_cdk::caller()) {
        return Err(format!("{:?}", governance::GovernanceError::NotSigner));
    }
    let operation =
        state::get_operation(operation_id).ok_or_else(|| "operation not found".to_string())?;
    match operation.rail() {
        types::FundingRail::CyclesLedger => {
            funding::cycles::resolve_unknown_as_spent(operation_id, now_secs())
                .map_err(|err| format!("{err:?}"))
        }
        types::FundingRail::IcpCmc => {
            funding::icp::resolve_unknown_as_spent(operation_id, now_secs())
                .map_err(|err| format!("{err:?}"))
        }
    }
}

/// Attach a verified ICP Ledger block proof.  Cycles Ledger block proofs are
/// intentionally not accepted from a bare block index: the Cycles Ledger
/// adapter must independently verify its source/destination before calling
/// the core reconciler, so an unverified caller value cannot settle funds.
#[ic_cdk::update]
async fn attach_block_proof(
    operation_id: u64,
    block_index: u64,
) -> Result<types::FundingOperation, String> {
    if !state::is_signer(ic_cdk::caller()) {
        return Err(format!("{:?}", governance::GovernanceError::NotSigner));
    }
    let operation =
        state::get_operation(operation_id).ok_or_else(|| "operation not found".to_string())?;
    if operation.rail() != types::FundingRail::IcpCmc {
        return Err("Cycles Ledger block proof requires verified adapter evidence".to_string());
    }
    funding::icp::attach_block_proof(operation_id, block_index, now_secs(), ic_cdk::id())
        .await
        .map_err(|err| format!("{err:?}"))
}

/// Attach a verified CMC refund block proof to a quarantined ICP/CMC
/// operation. Same signer-gated authorization pattern as `attach_block_proof`
/// above; `funding::icp::attach_refund_block_proof` independently reads and
/// verifies the refund block against the operation's immutable snapshot
/// before deriving any settlement, so a caller-supplied block index alone
/// can never settle funds.
#[ic_cdk::update]
async fn attach_refund_block_proof(
    operation_id: u64,
    block_index: u64,
) -> Result<types::FundingOperation, String> {
    if !state::is_signer(ic_cdk::caller()) {
        return Err(format!("{:?}", governance::GovernanceError::NotSigner));
    }
    let operation =
        state::get_operation(operation_id).ok_or_else(|| "operation not found".to_string())?;
    if operation.rail() != types::FundingRail::IcpCmc {
        return Err("ICP refund block proof requires the ICP/CMC rail".to_string());
    }
    funding::icp::attach_refund_block_proof(operation_id, block_index, now_secs(), ic_cdk::id())
        .await
        .map_err(|err| format!("{err:?}"))
}

/// Test-only durable-state fixture.  It is compiled and exported only when
/// `--features test_endpoints` is explicitly selected; production Candid and
/// Wasm therefore cannot accept an arbitrary persisted operation state.
#[cfg(feature = "test_endpoints")]
#[ic_cdk::update]
fn test_inject_operation(
    target: Principal,
    state: test_support::TestOperationState,
) -> Result<u64, String> {
    if !state::is_signer(ic_cdk::caller()) {
        return Err(format!("{:?}", governance::GovernanceError::NotSigner));
    }
    test_support::inject_operation(target, state, now_secs(), now_ns(), ic_cdk::id())
}

/// Read-only test projection for upgrade/restart assertions.  This endpoint is
/// intentionally absent from production Candid and Wasm.
#[cfg(feature = "test_endpoints")]
#[ic_cdk::query]
fn test_get_operation(operation_id: u64) -> Option<test_support::TestOperationView> {
    test_support::operation_view(operation_id)
}

/// Read-only test projection of an immutable ICP call snapshot. This endpoint
/// exists only in the opt-in integration Wasm and is absent from production.
#[cfg(feature = "test_endpoints")]
#[ic_cdk::query]
fn test_get_icp_snapshot(operation_id: u64) -> Result<Option<types::IcpCmcSnapshot>, String> {
    if !state::is_signer(ic_cdk::caller()) {
        return Err(format!("{:?}", governance::GovernanceError::NotSigner));
    }
    let snapshot =
        state::get_operation(operation_id).and_then(|operation| match operation.rail_arguments() {
            types::FundingRailArguments::Icp(snapshot) => Some(snapshot.clone()),
            types::FundingRailArguments::Cycles(_) => None,
        });
    Ok(snapshot)
}

/// Read-only test projection for asserting bootstrap safety defaults.  The
/// production public row deliberately omits control-plane flags.
#[cfg(feature = "test_endpoints")]
#[ic_cdk::query]
fn test_get_target_flags(target: Principal) -> Option<test_support::TestTargetFlags> {
    state::get_target(target).map(|record| test_support::TestTargetFlags {
        enabled: record.enabled(),
        auto_topup: record.auto_topup(),
    })
}

/// Starts the ordinary timer-triggered adapter path without running the
/// complete maintenance loop. This feature-gated seam lets PocketIC submit a
/// concurrent manual ingress while the Cycles Ledger await is unresolved.
#[cfg(feature = "test_endpoints")]
#[ic_cdk::update]
async fn test_start_timer_funding(target: Principal) -> Result<types::FundingOperation, String> {
    if !state::is_signer(ic_cdk::caller()) {
        return Err(format!("{:?}", governance::GovernanceError::NotSigner));
    }
    funding::cycles::run_ordinary(
        target,
        types::FundingTrigger::LowBalanceAutoTopup,
        now_secs(),
        now_ns(),
    )
    .await
    .map_err(|err| format!("{err:?}"))
}

/// Feature-gated PocketIC fixture for one explicit stable bound. The test
/// harness boots a fresh canister for each mode so no upgrade/query call must
/// carry every maximum-sized store at once.
#[cfg(feature = "test_endpoints")]
#[ic_cdk::update]
fn test_fill_bounds(
    mode: test_support::TestBoundsMode,
) -> Result<test_support::TestBoundsReport, String> {
    if !state::is_signer(ic_cdk::caller()) {
        return Err(format!("{:?}", governance::GovernanceError::NotSigner));
    }
    test_support::fill_bounds(mode, now_secs(), now_ns(), ic_cdk::id())
}

#[cfg(feature = "test_endpoints")]
#[ic_cdk::query]
fn test_get_bound_counts(sample_target: Principal) -> test_support::TestBoundsReport {
    test_support::get_bound_counts(sample_target)
}

/// Reports completion of the scheduled maintenance pass for bounded
/// PocketIC progress. This runtime-only generation is intentionally absent
/// from production Candid and Wasm.
#[cfg(feature = "test_endpoints")]
#[ic_cdk::query]
fn test_get_completed_tick_generation() -> u64 {
    sampler::completed_tick_generation()
}

// ─────────────────────────── Alarms (Task 2B) ───────────────────────────

#[ic_cdk::update]
fn acknowledge_alarm(id: u64) -> Result<bool, governance::GovernanceError> {
    governance::acknowledge_alarm_at(ic_cdk::caller(), now_secs(), id)
}

// ─────────────────────────── Private cached telemetry ───────────────────────────

fn require_telemetry_viewer() -> Result<(), String> {
    if telemetry_access::is_telemetry_viewer(ic_cdk::caller()) {
        Ok(())
    } else {
        Err("caller is not authorized to view private telemetry".to_string())
    }
}

fn require_telemetry_operator() -> Result<(), String> {
    if telemetry_access::is_operator(ic_cdk::caller()) {
        Ok(())
    } else {
        Err("caller is not an authorized Cycle Sentinel operator".to_string())
    }
}

#[ic_cdk::query(guard = "require_telemetry_viewer")]
fn get_operator_dashboard() -> Result<types::OperatorDashboard, types::OperatorDashboardError> {
    public_api::get_operator_dashboard_at(ic_cdk::caller(), now_secs())
}

#[ic_cdk::query(guard = "require_telemetry_viewer")]
fn get_public_overview() -> types::PublicOverview {
    public_api::get_public_overview_at(now_secs())
}

#[ic_cdk::query(guard = "require_telemetry_viewer")]
fn list_public_targets(
    cursor: Option<String>,
    limit: u16,
) -> Result<types::PublicPage<types::PublicTargetRow>, public_api::PublicQueryError> {
    public_api::list_public_targets_at(cursor, limit, now_secs())
}

#[ic_cdk::query(guard = "require_telemetry_viewer")]
fn get_public_target(principal: Principal) -> Option<types::PublicTargetRow> {
    public_api::get_public_target_at(principal, now_secs())
}

#[ic_cdk::query(guard = "require_telemetry_viewer")]
fn list_public_samples(
    principal: Principal,
    cursor: Option<String>,
    limit: u16,
) -> Result<types::PublicPage<types::Sample>, public_api::PublicQueryError> {
    public_api::list_public_samples_at(principal, cursor, limit)
}

#[ic_cdk::query(guard = "require_telemetry_viewer")]
fn list_public_topups(
    principal: Principal,
    cursor: Option<String>,
    limit: u16,
) -> Result<types::PublicPage<types::PublicTopupSummary>, public_api::PublicQueryError> {
    public_api::list_public_topups_at(principal, cursor, limit)
}

#[ic_cdk::query(guard = "require_telemetry_viewer")]
fn list_public_alarms(
    cursor: Option<String>,
    limit: u16,
) -> Result<types::PublicPage<types::PublicAlarm>, public_api::PublicQueryError> {
    public_api::list_public_alarms_at(cursor, limit)
}

ic_cdk::export_candid!();

// The opt-in `test_endpoints` feature intentionally adds an integration-only
// update method, so its exported service is not the production `.did` file.
// Keep the conformance check on the production interface only; the feature's
// test Wasm has no checked-in public declaration by design.
#[cfg(all(test, not(feature = "test_endpoints")))]
mod candid_tests {
    use candid_parser::utils::{service_equal, CandidSource};
    use std::path::Path;

    #[test]
    fn candid_interface_matches_did_file() {
        let generated = super::__export_service();
        service_equal(
            CandidSource::Text(&generated),
            CandidSource::File(Path::new("rumi_cycle_sentinel.did")),
        )
        .unwrap_or_else(|err| {
            panic!(
                "rumi_cycle_sentinel.did is out of sync with the canister interface:\n{err}\n\n{generated}"
            )
        });
    }
}

#[cfg(test)]
mod maintenance_authorization_tests {
    use super::*;

    #[test]
    fn maintenance_rejects_anonymous_before_signer_lookup() {
        let mut signer_lookup_called = false;
        assert_eq!(
            require_maintenance_signer(Principal::anonymous(), || {
                signer_lookup_called = true;
                true
            }),
            Err("anonymous caller not allowed".to_string())
        );
        assert!(!signer_lookup_called);
    }

    #[test]
    fn maintenance_rejects_non_signers() {
        assert_eq!(
            require_maintenance_signer(Principal::from_slice(&[7]), || false),
            Err(format!("{:?}", governance::GovernanceError::NotSigner))
        );
    }

    #[test]
    fn maintenance_allows_signers() {
        assert_eq!(
            require_maintenance_signer(Principal::from_slice(&[7]), || true),
            Ok(())
        );
    }
}

#[cfg(test)]
mod ingress_filter_tests {
    use super::{sentinel_ingress_allowed, MAX_ANONYMOUS_CONSENT_INGRESS_BYTES};
    use candid::Principal;

    #[test]
    fn anonymous_ingress_is_limited_to_bounded_consent_requests() {
        let anonymous = Principal::anonymous();
        assert!(sentinel_ingress_allowed(
            anonymous,
            "icrc21_canister_call_consent_message",
            MAX_ANONYMOUS_CONSENT_INGRESS_BYTES
        ));
        assert!(!sentinel_ingress_allowed(
            anonymous,
            "icrc21_canister_call_consent_message",
            MAX_ANONYMOUS_CONSENT_INGRESS_BYTES + 1
        ));
        assert!(!sentinel_ingress_allowed(
            anonymous,
            "run_maintenance_now",
            0
        ));
        assert!(sentinel_ingress_allowed(
            Principal::from_slice(&[42]),
            "run_maintenance_now",
            usize::MAX
        ));
    }
}
