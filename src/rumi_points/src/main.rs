//! Canister entry points for `rumi_points`. The library (`lib.rs` + modules)
//! holds the logic; this binary wires the IC lifecycle hooks and the candid
//! query/update surface to it. Phase 1 exposes read endpoints plus an admin-only
//! `register_test_principal` for proving upgrade survival before ingestion exists.

use candid::Principal;
use ic_canister_log::{declare_log_buffer, log};

use rumi_points::types::{
    EpochSummary, IngestStatus, InitArgs, LeaderboardEntry, PointsConfig, PointsError,
    PrincipalState, RegistrationInfo, SourceStatus,
};
use rumi_points::{poll, state};

// Canister debug-log buffer (retrievable in later phases; for now feeds the
// replica debug log on lifecycle events).
declare_log_buffer!(name = INFO, capacity = 2000);

fn main() {}

// ── Lifecycle ───────────────────────────────────────────────────────────────

#[ic_cdk::init]
fn init(args: Option<InitArgs>) {
    state::init_state(args, ic_cdk::caller());
    // No-op while the poll timer is off by default; consistent entry point.
    poll::setup_poll_timer();
    let cfg = state::points_config();
    log!(
        INFO,
        "rumi_points init: admin={}, excluded={}, season=[{}..{}]",
        cfg.admin,
        cfg.excluded_count,
        cfg.season_start_ns,
        cfg.season_end_ns
    );
}

#[ic_cdk::pre_upgrade]
fn pre_upgrade() {
    state::save_state_to_stable();
}

#[ic_cdk::post_upgrade]
fn post_upgrade() {
    state::restore_from_stable_or_trap();
    // Timers do not survive upgrades: re-register from the persisted config.
    poll::setup_poll_timer();
    let cfg = state::points_config();
    log!(
        INFO,
        "rumi_points post_upgrade: admin={}, excluded={}, registered={}",
        cfg.admin,
        cfg.excluded_count,
        cfg.registered_count
    );
}

// ── Queries ───────────────────────────────────────────────────────────────

#[ic_cdk::query]
fn get_principal_state(principal: Principal) -> Option<PrincipalState> {
    state::get_principal_state(&principal)
}

#[ic_cdk::query]
fn get_leaderboard(offset: u32, limit: u32) -> Vec<LeaderboardEntry> {
    state::leaderboard(offset, limit)
}

#[ic_cdk::query]
fn get_epoch_history(offset: u32, limit: u32) -> Vec<EpochSummary> {
    state::epoch_history(offset as u64, limit as u64)
}

#[ic_cdk::query]
fn is_registered(principal: Principal) -> bool {
    state::is_registered(&principal)
}

#[ic_cdk::query]
fn get_registration_info(principal: Principal) -> Option<RegistrationInfo> {
    state::registration_info(&principal)
}

#[ic_cdk::query]
fn is_excluded(principal: Principal) -> bool {
    state::is_excluded(&principal)
}

#[ic_cdk::query]
fn get_excluded_principals() -> Vec<Principal> {
    state::excluded_principals()
}

#[ic_cdk::query]
fn get_points_config() -> PointsConfig {
    state::points_config()
}

// ── Updates (admin-gated) ───────────────────────────────────────────────────

/// Admin-only. Phase 1 testing aid: enrolls a principal so upgrade survival can
/// be demonstrated before event ingestion (Phase 2/3) exists.
#[ic_cdk::update]
fn register_test_principal(principal: Principal) -> Result<(), PointsError> {
    state::register_test_principal(ic_cdk::caller(), principal, ic_cdk::api::time())
}

#[ic_cdk::update]
fn add_excluded_principal(principal: Principal) -> Result<(), PointsError> {
    state::add_excluded(ic_cdk::caller(), principal)
}

#[ic_cdk::update]
fn remove_excluded_principal(principal: Principal) -> Result<(), PointsError> {
    state::remove_excluded(ic_cdk::caller(), principal)
}

#[ic_cdk::update]
fn set_excluded_principals(principals: Vec<Principal>) -> Result<(), PointsError> {
    state::set_excluded(ic_cdk::caller(), principals)
}

// ── Phase 2: ingestion control ──────────────────────────────────────────────

/// Admin-set a source canister id (point the poller at the right canister per
/// environment, e.g. local replica ids). Tag: 0 = backend, 1 = 3pool, 2 = SP,
/// 3 = AMM.
#[ic_cdk::update]
fn set_source_canister(source_tag: u8, canister: Principal) -> Result<(), PointsError> {
    state::set_source_canister(ic_cdk::caller(), source_tag, canister)
}

/// Admin-only manual poll of all configured sources. Returns the number of events
/// applied. Works regardless of the periodic timer (used for the E2E and backfill).
#[ic_cdk::update]
async fn trigger_poll() -> Result<u64, PointsError> {
    if !state::is_admin(ic_cdk::caller()) {
        return Err(PointsError::Unauthorized);
    }
    Ok(poll::poll_all().await as u64)
}

/// Admin: turn the periodic poll timer on/off (Phase 2b). Off by default. Enable
/// during the season after configuring sources; disable after season end.
#[ic_cdk::update]
fn set_poll_enabled(enabled: bool) -> Result<(), PointsError> {
    state::set_poll_enabled(ic_cdk::caller(), enabled)?;
    poll::setup_poll_timer();
    Ok(())
}

/// Admin: set the poll cadence in seconds (clamped to a floor to bound cycle burn).
#[ic_cdk::update]
fn set_poll_interval_secs(secs: u64) -> Result<(), PointsError> {
    state::set_poll_interval(ic_cdk::caller(), secs)?;
    poll::setup_poll_timer();
    Ok(())
}

#[ic_cdk::query]
fn get_ingest_status() -> IngestStatus {
    let sources = state::source_canisters()
        .into_iter()
        .map(|(tag, canister)| SourceStatus {
            tag,
            canister,
            cursor: state::get_cursor(tag),
        })
        .collect();
    IngestStatus {
        sources,
        registered_count: state::registered_count(),
        poll_enabled: state::poll_enabled(),
        poll_interval_secs: state::poll_interval_secs(),
    }
}

ic_cdk::export_candid!();

#[cfg(test)]
mod candid_tests {
    use candid_parser::utils::{service_equal, CandidSource};
    use std::path::Path;

    /// The committed `rumi_points.did` must stay structurally equal to the
    /// interface generated from the endpoint signatures. Catches schema drift.
    #[test]
    fn candid_interface_matches_did_file() {
        let generated = super::__export_service();
        service_equal(
            CandidSource::Text(&generated),
            CandidSource::File(Path::new("rumi_points.did")),
        )
        .unwrap_or_else(|e| {
            panic!(
                "rumi_points.did is out of sync with the canister interface:\n{e}\n\n\
                 --- generated interface ---\n{generated}"
            )
        });
    }
}
