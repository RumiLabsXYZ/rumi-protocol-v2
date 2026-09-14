use candid::Principal;
use rumi_cycle_manager::{
    self_cycles_status, CycleManagerCyclesStatus, DEFAULT_FREEZE_THRESHOLD_SECS,
    DEFAULT_LOW_WATERMARK_CYCLES,
};

mod governance;
mod state;
mod types;

/// The IC wall clock, in seconds — every `governance::*_at` function takes
/// this as an explicit parameter rather than reading `ic_cdk::api::time()`
/// itself, so this is the one place that conversion happens.
fn now_secs() -> u64 {
    ic_cdk::api::time() / 1_000_000_000
}

#[ic_cdk::init]
fn init(args: types::InitArgs) {
    if let Err(err) = state::init(args) {
        ic_cdk::trap(&format!("rumi_cycle_sentinel: init failed: {err:?}"));
    }
}

#[ic_cdk::post_upgrade]
fn post_upgrade() {
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
