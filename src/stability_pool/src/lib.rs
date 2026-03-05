use ic_cdk::{query, update, init, pre_upgrade, post_upgrade};
use candid::Principal;
use ic_canister_log::log;
use std::collections::BTreeMap;

pub mod types;
pub mod state;
pub mod deposits;
pub mod liquidation;
pub mod logs;

use crate::types::*;
use crate::state::{mutate_state, read_state};
use crate::logs::INFO;

// ─── Init / Upgrade ───

#[init]
fn init(args: StabilityPoolInitArgs) {
    mutate_state(|s| s.initialize(args));
    log!(INFO, "Stability Pool initialized. Protocol: {}",
        read_state(|s| s.protocol_canister_id));
}

#[pre_upgrade]
fn pre_upgrade() {
    log!(INFO, "Stability Pool pre-upgrade: saving state to stable memory");
    state::save_to_stable_memory();
}

#[post_upgrade]
fn post_upgrade() {
    state::load_from_stable_memory();
    log!(INFO, "Stability Pool post-upgrade: state restored. {} depositors, {} liquidations",
        read_state(|s| s.deposits.len()),
        read_state(|s| s.total_liquidations_executed));

    if let Err(error) = read_state(|s| s.validate_state()) {
        ic_cdk::trap(&format!("State validation failed after upgrade: {}", error));
    }
}

// ─── Deposit / Withdraw / Claim ───

#[update]
pub async fn deposit(token_ledger: Principal, amount: u64) -> Result<(), StabilityPoolError> {
    crate::deposits::deposit(token_ledger, amount).await
}

#[update]
pub async fn withdraw(token_ledger: Principal, amount: u64) -> Result<(), StabilityPoolError> {
    crate::deposits::withdraw(token_ledger, amount).await
}

#[update]
pub async fn claim_collateral(collateral_ledger: Principal) -> Result<u64, StabilityPoolError> {
    crate::deposits::claim_collateral(collateral_ledger).await
}

#[update]
pub async fn claim_all_collateral() -> Result<BTreeMap<Principal, u64>, StabilityPoolError> {
    crate::deposits::claim_all_collateral().await
}

// ─── Opt-in / Opt-out ───

#[update]
pub fn opt_out_collateral(collateral_type: Principal) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    mutate_state(|s| s.opt_out_collateral(&caller, collateral_type))
}

#[update]
pub fn opt_in_collateral(collateral_type: Principal) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    mutate_state(|s| s.opt_in_collateral(&caller, collateral_type))
}

// ─── Liquidation (Push + Fallback) ───

/// Called by the backend to push liquidatable vault notifications.
#[update]
pub async fn notify_liquidatable_vaults(vaults: Vec<LiquidatableVaultInfo>) -> Vec<LiquidationResult> {
    // Optionally: validate caller is the protocol canister
    let caller = ic_cdk::api::caller();
    let expected = read_state(|s| s.protocol_canister_id);
    if caller != expected {
        log!(INFO, "notify_liquidatable_vaults called by {} (expected {}). Allowing for now.",
            caller, expected);
        // TODO: decide whether to enforce caller == protocol_canister_id
    }
    crate::liquidation::notify_liquidatable_vaults(vaults).await
}

/// Public fallback: trigger liquidation for a specific vault.
#[update]
pub async fn execute_liquidation(vault_id: u64) -> Result<LiquidationResult, StabilityPoolError> {
    crate::liquidation::execute_liquidation(vault_id).await
}

// ─── Queries ───

#[query]
pub fn get_pool_status() -> StabilityPoolStatus {
    read_state(|s| s.get_pool_status())
}

#[query]
pub fn get_user_position(user: Option<Principal>) -> Option<UserStabilityPosition> {
    let target = user.unwrap_or_else(ic_cdk::api::caller);
    read_state(|s| s.get_user_position(&target))
}

#[query]
pub fn get_liquidation_history(limit: Option<u64>) -> Vec<PoolLiquidationRecord> {
    let limit = limit.unwrap_or(50).min(100) as usize;
    read_state(|s| {
        s.liquidation_history.iter().rev().take(limit).cloned().collect()
    })
}

#[query]
pub fn check_pool_capacity(collateral_type: Principal, debt_amount_e8s: u64) -> bool {
    read_state(|s| s.effective_pool_for_collateral(&collateral_type) >= debt_amount_e8s)
}

#[query]
pub fn validate_pool_state() -> Result<String, String> {
    read_state(|s| s.validate_state().map(|_| "Pool state is consistent".to_string()))
}

// ─── Admin: Registry Management ───

#[update]
pub fn register_stablecoin(config: StablecoinConfig) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    mutate_state(|s| s.register_stablecoin(config));
    Ok(())
}

#[update]
pub fn register_collateral(info: CollateralInfo) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    mutate_state(|s| s.register_collateral(info));
    Ok(())
}

// ─── Admin: Configuration ───

#[update]
pub fn update_pool_configuration(new_config: PoolConfiguration) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    mutate_state(|s| s.configuration = new_config);
    Ok(())
}

#[update]
pub fn emergency_pause() -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    mutate_state(|s| s.configuration.emergency_pause = true);
    log!(INFO, "Emergency pause activated by {}", caller);
    Ok(())
}

#[update]
pub fn resume_operations() -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    mutate_state(|s| s.configuration.emergency_pause = false);
    log!(INFO, "Operations resumed by {}", caller);
    Ok(())
}
