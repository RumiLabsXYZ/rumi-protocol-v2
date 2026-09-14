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
mod self_recovery;
mod state;
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
