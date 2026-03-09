use ic_cdk::{query, init, pre_upgrade, post_upgrade};
use ic_canister_log::log;

pub mod types;
pub mod state;
pub mod math;

mod logs;

use crate::logs::INFO;

// ─── Init / Upgrade ───

#[init]
fn init() {
    log!(INFO, "Rumi 3pool canister initialized");
}

#[pre_upgrade]
fn pre_upgrade() {
    log!(INFO, "Rumi 3pool pre-upgrade: saving state to stable memory");
}

#[post_upgrade]
fn post_upgrade() {
    log!(INFO, "Rumi 3pool post-upgrade: state restored");
}

// ─── Queries ───

#[query]
pub fn health() -> String {
    "ok".to_string()
}
