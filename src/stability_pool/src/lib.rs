use ic_cdk::{query, update, init, pre_upgrade, post_upgrade};
use candid::Principal;
use ic_canister_log::log;
use std::collections::BTreeMap;
use std::time::Duration;

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
fn post_upgrade(_args: StabilityPoolInitArgs) {
    state::load_from_stable_memory();
    log!(INFO, "Stability Pool post-upgrade: state restored. {} depositors, {} liquidations",
        read_state(|s| s.deposits.len()),
        read_state(|s| s.total_liquidations_executed));

    if let Err(error) = read_state(|s| s.validate_state()) {
        ic_cdk::trap(&format!("State validation failed after upgrade: {}", error));
    }

    // Migration: fix stablecoin transfer_fee values
    mutate_state(|s| {
        for config in s.stablecoin_registry.values_mut() {
            match config.symbol.as_str() {
                "icUSD" => { config.transfer_fee = Some(100_000); }
                "3USD"  => { config.transfer_fee = Some(0); }
                _ => {}
            }
        }
    });
    log!(INFO, "Migration: corrected icUSD and 3USD transfer fees");

    // Defer timer setup to avoid ic0_call_new restriction during upgrade
    ic_cdk_timers::set_timer(Duration::ZERO, || {
        setup_virtual_price_timer();
    });
}

// ─── Virtual Price Timer ───

fn setup_virtual_price_timer() {
    // Fetch immediately on startup, then every 5 minutes.
    ic_cdk::spawn(fetch_virtual_prices());
    ic_cdk_timers::set_timer_interval(Duration::from_secs(300), || {
        ic_cdk::spawn(fetch_virtual_prices());
    });
}

async fn fetch_virtual_prices() {
    let lp_configs: Vec<(Principal, Principal)> = read_state(|s| {
        s.stablecoin_registry.iter()
            .filter(|(_, c)| c.is_lp_token.unwrap_or(false))
            .filter_map(|(ledger, c)| c.underlying_pool.map(|pool| (*ledger, pool)))
            .collect()
    });

    for (lp_ledger, pool_canister) in lp_configs {
        let result: Result<(ThreePoolStatus,), _> = ic_cdk::call(
            pool_canister, "get_pool_status", ()
        ).await;

        match result {
            Ok((status,)) => {
                mutate_state(|s| {
                    s.cached_virtual_prices
                        .get_or_insert_with(BTreeMap::new)
                        .insert(lp_ledger, status.virtual_price);
                });
            }
            Err(e) => {
                log!(INFO, "Failed to fetch virtual price from {}: {:?}", pool_canister, e);
            }
        }
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

/// Convenience: deposit a stablecoin (icUSD, ckUSDT, ckUSDC) and have the pool
/// mint 3USD on the user's behalf by depositing into the 3pool.
#[update]
pub async fn deposit_as_3usd(token_ledger: Principal, amount: u64) -> Result<u64, StabilityPoolError> {
    crate::deposits::deposit_as_3usd(token_ledger, amount).await
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

// ─── Interest Revenue ───

/// Receive interest revenue from the protocol backend and distribute pro-rata to depositors.
/// Only callable by the protocol canister.
///
/// `collateral_type` identifies which collateral's vault generated the interest.
/// Depositors who opted out of that collateral are excluded from the distribution.
/// The parameter is optional for backward compatibility with older backend versions.
#[update]
pub fn receive_interest_revenue(token_ledger: Principal, amount: u64, collateral_type: Option<Principal>) -> Result<(), StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    let expected = read_state(|s| s.protocol_canister_id);
    if caller != expected {
        return Err(StabilityPoolError::Unauthorized);
    }

    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    if !read_state(|s| s.stablecoin_registry.contains_key(&token_ledger)) {
        return Err(StabilityPoolError::TokenNotAccepted { ledger: token_ledger });
    }

    mutate_state(|s| s.distribute_interest_revenue(token_ledger, amount, collateral_type));

    log!(INFO, "Distributed {} interest for token {} (collateral: {:?}) from backend", amount, token_ledger, collateral_type);
    Ok(())
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

// ─── ICRC-21: Canister Call Consent Messages ───

#[update]
pub fn icrc21_canister_call_consent_message(
    request: Icrc21ConsentMessageRequest,
) -> Icrc21ConsentMessageResponse {
    let message_text = match request.method.as_str() {
        "deposit" => {
            match candid::decode_args::<(Principal, u64)>(&request.arg) {
                Ok((token_ledger, amount)) => {
                    let symbol = read_state(|s| {
                        s.stablecoin_registry
                            .get(&token_ledger)
                            .map(|c| c.symbol.clone())
                            .unwrap_or_else(|| format!("token {}", token_ledger))
                    });
                    let formatted = format_token_amount(amount);
                    format!(
                        "## Deposit to Stability Pool\n\n\
                         You are depositing **{} {}** into the Rumi Protocol Stability Pool.\n\n\
                         Your deposit earns liquidation rewards proportional to your share of the pool.",
                        formatted, symbol
                    )
                }
                Err(_) => "Deposit stablecoins into the Rumi Protocol Stability Pool.".to_string(),
            }
        }
        "withdraw" => {
            match candid::decode_args::<(Principal, u64)>(&request.arg) {
                Ok((token_ledger, amount)) => {
                    let symbol = read_state(|s| {
                        s.stablecoin_registry
                            .get(&token_ledger)
                            .map(|c| c.symbol.clone())
                            .unwrap_or_else(|| format!("token {}", token_ledger))
                    });
                    let formatted = format_token_amount(amount);
                    format!(
                        "## Withdraw from Stability Pool\n\n\
                         You are withdrawing **{} {}** from the Rumi Protocol Stability Pool.",
                        formatted, symbol
                    )
                }
                Err(_) => "Withdraw stablecoins from the Rumi Protocol Stability Pool.".to_string(),
            }
        }
        "claim_collateral" => {
            match candid::decode_args::<(Principal,)>(&request.arg) {
                Ok((collateral_ledger,)) => {
                    let symbol = read_state(|s| {
                        s.collateral_registry
                            .get(&collateral_ledger)
                            .map(|c| c.symbol.clone())
                            .unwrap_or_else(|| format!("collateral {}", collateral_ledger))
                    });
                    format!(
                        "## Claim Collateral Rewards\n\n\
                         You are claiming your **{}** collateral rewards from the Rumi Protocol Stability Pool.",
                        symbol
                    )
                }
                Err(_) => "Claim collateral rewards from the Rumi Protocol Stability Pool.".to_string(),
            }
        }
        "claim_all_collateral" => {
            "## Claim All Collateral Rewards\n\n\
             You are claiming **all** of your collateral rewards from the Rumi Protocol Stability Pool."
                .to_string()
        }
        "opt_out_collateral" => {
            "## Opt Out of Collateral\n\n\
             You are opting out of receiving a specific collateral type from future liquidations."
                .to_string()
        }
        "opt_in_collateral" => {
            "## Opt In to Collateral\n\n\
             You are opting back in to receiving a specific collateral type from future liquidations."
                .to_string()
        }
        "deposit_as_3usd" => {
            match candid::decode_args::<(Principal, u64)>(&request.arg) {
                Ok((token_ledger, amount)) => {
                    let symbol = read_state(|s| {
                        s.stablecoin_registry
                            .get(&token_ledger)
                            .map(|c| c.symbol.clone())
                            .unwrap_or_else(|| format!("token {}", token_ledger))
                    });
                    let formatted = format_token_amount(amount);
                    format!(
                        "## Deposit as 3USD\n\n\
                         You are depositing **{} {}** into the Rumi Protocol Stability Pool \
                         via the 3pool. Your tokens will be converted to 3USD LP tokens, \
                         which earn swap fees while backing liquidations.",
                        formatted, symbol
                    )
                }
                Err(_) => "Deposit stablecoins into the Stability Pool via the 3pool as 3USD LP tokens.".to_string(),
            }
        }
        _ => {
            return Icrc21ConsentMessageResponse::Err(Icrc21Error::UnsupportedCanisterCall(
                Icrc21ErrorInfo {
                    description: format!(
                        "Method '{}' is not a supported user-facing call.",
                        request.method
                    ),
                },
            ));
        }
    };

    Icrc21ConsentMessageResponse::Ok(Icrc21ConsentInfo {
        consent_message: Icrc21ConsentMessage::GenericDisplayMessage(message_text),
        metadata: Icrc21ConsentMessageResponseMetadata {
            language: request.user_preferences.metadata.language.clone(),
            utc_offset_minutes: request.user_preferences.metadata.utc_offset_minutes,
        },
    })
}

// ─── ICRC-10: Supported Standards ───

#[query]
pub fn icrc10_supported_standards() -> Vec<Icrc10SupportedStandard> {
    vec![
        Icrc10SupportedStandard {
            name: "ICRC-21".to_string(),
            url: "https://github.com/dfinity/ICRC/blob/main/ICRCs/ICRC-21/ICRC-21.md".to_string(),
        },
        Icrc10SupportedStandard {
            name: "ICRC-10".to_string(),
            url: "https://github.com/dfinity/ICRC/blob/main/ICRCs/ICRC-10/ICRC-10.md".to_string(),
        },
    ]
}

/// Format an e8s token amount as a human-readable string.
fn format_token_amount(amount_e8s: u64) -> String {
    let whole = amount_e8s / 100_000_000;
    let frac = amount_e8s % 100_000_000;
    if frac == 0 {
        format!("{}", whole)
    } else {
        let frac_str = format!("{:08}", frac);
        let trimmed = frac_str.trim_end_matches('0');
        format!("{}.{}", whole, trimmed)
    }
}

// ─── Admin: Configuration ───

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

/// Admin: correct a depositor's stablecoin balance to match actual ledger state.
/// Use when internal state tracks tokens that were never actually transferred on-chain.
#[update]
pub fn admin_correct_balance(
    user: Principal,
    token_ledger: Principal,
    correct_amount: u64,
) -> Result<String, StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    let msg = mutate_state(|s| s.correct_balance(user, token_ledger, correct_amount));
    log!(INFO, "Admin balance correction by {}: {}", caller, msg);
    Ok(msg)
}

#[update]
pub fn admin_correct_collateral_gain(
    user: Principal,
    collateral_ledger: Principal,
    correct_amount: u64,
) -> Result<String, StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    let msg = mutate_state(|s| s.correct_collateral_gain(user, collateral_ledger, correct_amount));
    log!(INFO, "Admin collateral gain correction by {}: {}", caller, msg);
    Ok(msg)
}
