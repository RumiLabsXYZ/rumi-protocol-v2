//! rumi_analytics canister - Phase 1.
//! See docs/plans/2026-04-07-rumi-analytics-design.md.

use candid::Principal;

mod state;
mod storage;
mod collectors;
mod sources;

use crate::storage::{SlimState, SourceCanisterIds};

#[derive(candid::CandidType, candid::Deserialize)]
pub struct InitArgs {
    pub admin: Principal,
    pub backend: Principal,
    pub icusd_ledger: Principal,
    pub three_pool: Principal,
    pub stability_pool: Principal,
    pub amm: Principal,
}

#[ic_cdk_macros::init]
fn init(args: InitArgs) {
    let s = SlimState {
        admin: args.admin,
        sources: SourceCanisterIds {
            backend: args.backend,
            icusd_ledger: args.icusd_ledger,
            three_pool: args.three_pool,
            stability_pool: args.stability_pool,
            amm: args.amm,
        },
        ..SlimState::default()
    };
    storage::set_slim(s);
    state::hydrate_from_slim();
}

#[ic_cdk_macros::pre_upgrade]
fn pre_upgrade() {
    state::snapshot_slim_to_cell();
}

#[ic_cdk_macros::post_upgrade]
fn post_upgrade() {
    state::hydrate_from_slim();
}

#[ic_cdk_macros::query]
fn ping() -> &'static str {
    "rumi_analytics ok"
}

#[ic_cdk_macros::query]
fn get_admin() -> Principal {
    state::read_state(|s| s.admin)
}

ic_cdk::export_candid!();
