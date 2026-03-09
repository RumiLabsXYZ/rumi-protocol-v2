use ic_cdk::{query, init, pre_upgrade, post_upgrade};
use ic_canister_log::log;

pub mod types;
pub mod state;
pub mod math;

mod logs;

use crate::types::*;
use crate::state::{mutate_state, read_state};
use crate::logs::INFO;

// ─── Init / Upgrade ───

#[init]
fn init(args: ThreePoolInitArgs) {
    mutate_state(|s| s.initialize(args));
    log!(INFO, "Rumi 3pool initialized. Admin: {}, A: {}, swap_fee: {} bps",
        read_state(|s| s.config.admin),
        read_state(|s| s.config.initial_a),
        read_state(|s| s.config.swap_fee_bps));
}

#[pre_upgrade]
fn pre_upgrade() {
    log!(INFO, "Rumi 3pool pre-upgrade: saving state to stable memory");
    state::save_to_stable_memory();
}

#[post_upgrade]
fn post_upgrade() {
    state::load_from_stable_memory();
    log!(INFO, "Rumi 3pool post-upgrade: state restored. LP supply: {}, initialized: {}",
        read_state(|s| s.lp_total_supply),
        read_state(|s| s.is_initialized));
}

// ─── Queries ───

#[query]
pub fn health() -> String {
    "ok".to_string()
}
