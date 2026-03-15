use candid::{CandidType, Deserialize};
use ic_cdk_macros::{init, post_upgrade, pre_upgrade, query, update};
use ic_canister_log::{declare_log_buffer, log};

mod state;
mod process;
mod swap;

use state::{BotConfig, BotLiquidationEvent, BotState, LiquidatableVaultInfo};

declare_log_buffer!(name = INFO, capacity = 1000);

#[derive(CandidType, Deserialize)]
pub struct BotInitArgs {
    pub config: BotConfig,
}

#[init]
fn init(args: BotInitArgs) {
    state::init_state(BotState {
        config: Some(args.config),
        ..Default::default()
    });
    setup_timer();
}

#[pre_upgrade]
fn pre_upgrade() {
    state::save_to_stable_memory();
}

#[post_upgrade]
fn post_upgrade() {
    state::load_from_stable_memory();
    setup_timer();
}

fn setup_timer() {
    ic_cdk_timers::set_timer_interval(
        std::time::Duration::from_secs(30),
        || ic_cdk::spawn(process::process_pending()),
    );
}

#[update]
fn notify_liquidatable_vaults(vaults: Vec<LiquidatableVaultInfo>) {
    let caller = ic_cdk::api::caller();
    let backend = state::read_state(|s| {
        s.config.as_ref().map(|c| c.backend_principal)
    });
    if Some(caller) != backend {
        log!(INFO, "Rejected notification from unauthorized caller: {}", caller);
        return;
    }
    let count = vaults.len();
    state::mutate_state(|s| {
        s.pending_vaults = vaults;
    });
    log!(INFO, "Received {} liquidatable vaults from backend", count);
}

#[query]
fn get_bot_stats() -> state::BotStats {
    state::read_state(|s| s.stats.clone())
}

#[query]
fn get_liquidation_events(offset: u64, limit: u64) -> Vec<BotLiquidationEvent> {
    state::read_state(|s| {
        let len = s.liquidation_events.len();
        let start = (len as u64).saturating_sub(offset + limit) as usize;
        let end = (len as u64).saturating_sub(offset) as usize;
        s.liquidation_events[start..end].to_vec()
    })
}

#[update]
fn set_config(config: BotConfig) {
    let caller = ic_cdk::api::caller();
    let is_admin = state::read_state(|s| {
        s.config.as_ref().map(|c| c.admin == caller).unwrap_or(false)
    });
    if !is_admin {
        ic_cdk::trap("Unauthorized: only admin can set config");
    }
    state::mutate_state(|s| s.config = Some(config));
}
