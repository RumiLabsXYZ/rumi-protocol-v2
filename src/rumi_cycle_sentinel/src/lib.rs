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
mod observation;
mod public_api;
mod sampler;
mod self_recovery;
mod state;
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
    sampler::setup_timer();
}

#[ic_cdk::query]
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

/// Manual maintenance uses the same Cycles Ledger state machine as the timer.
/// The timer may additionally choose the ICP/CMC fallback after a proven
/// no-spend result; it never trusts a caller-provided rail or amount.
#[ic_cdk::update]
async fn manual_top_up(target: Principal) -> Result<types::FundingOperation, String> {
    funding::manual_top_up_at(ic_cdk::caller(), now_secs(), now_ns(), target)
        .await
        .map_err(|err| format!("{err:?}"))
}

/// Resolve an ambiguous operation conservatively.  Both rails retain their
/// immutable snapshots and reservations until this signer-gated call has
/// completed; self-recovery refuses the spent-only path and still requires a
/// delivery proof.
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

// ─────────────────────────── Public cached telemetry ───────────────────────────

#[ic_cdk::query]
fn get_public_overview() -> types::PublicOverview {
    public_api::get_public_overview_at(now_secs())
}

#[ic_cdk::query]
fn list_public_targets(
    cursor: Option<String>,
    limit: u16,
) -> Result<types::PublicPage<types::PublicTargetRow>, public_api::PublicQueryError> {
    public_api::list_public_targets_at(cursor, limit, now_secs())
}

#[ic_cdk::query]
fn get_public_target(principal: Principal) -> Option<types::PublicTargetRow> {
    public_api::get_public_target_at(principal, now_secs())
}

#[ic_cdk::query]
fn list_public_samples(
    principal: Principal,
    cursor: Option<String>,
    limit: u16,
) -> Result<types::PublicPage<types::Sample>, public_api::PublicQueryError> {
    public_api::list_public_samples_at(principal, cursor, limit)
}

#[ic_cdk::query]
fn list_public_topups(
    principal: Principal,
    cursor: Option<String>,
    limit: u16,
) -> Result<types::PublicPage<types::PublicTopupSummary>, public_api::PublicQueryError> {
    public_api::list_public_topups_at(principal, cursor, limit)
}

#[ic_cdk::query]
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
