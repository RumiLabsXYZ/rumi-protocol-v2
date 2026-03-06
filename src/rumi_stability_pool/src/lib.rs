mod types;
mod pool;
mod monitor;

use crate::pool::*;
use crate::types::*;
use crate::monitor::*;
use candid::{candid_method, export_service};
use ic_cdk::*;

// Initialize canister
#[init]
fn init(init_args: InitArgs) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        *state = PoolState {
            protocol_owner: init_args.protocol_owner,
            liquidation_discount: init_args.liquidation_discount,
            max_ltv_ratio: init_args.max_ltv_ratio,
            next_liquidation_id: 1,
            paused: false,
        };
    });
}

// Public API endpoints

#[update]
#[candid_method(update)]
fn deposit(amount: u64) -> DepositResult {
    if is_paused() {
        return DepositResult {
            success: false,
            new_balance: 0,
            message: "Protocol is paused".to_string(),
        };
    }
    
    deposit_icusd(amount)
}

#[update]
#[candid_method(update)]
fn withdraw(amount: u64) -> WithdrawResult {
    if is_paused() {
        return WithdrawResult {
            success: false,
            remaining_balance: 0,
            message: "Protocol is paused".to_string(),
        };
    }
    
    withdraw_icusd(amount)
}

#[update]
#[candid_method(update)]
fn claim_collateral_rewards(liquidation_ids: Vec<u64>) -> ClaimResult {
    claim_collateral(liquidation_ids)
}

#[query]
#[candid_method(query)]
fn get_user_deposit() -> Option<UserDeposit> {
    let user = caller();
    DEPOSITS.with(|deposits| {
        deposits.borrow().get(&user).cloned()
    })
}

#[query]
#[candid_method(query)]
fn get_total_pool_info() -> PoolInfo {
    let total_icusd = get_total_pool_size();
    let total_depositors = DEPOSITS.with(|deposits| {
        deposits.borrow().len() as u64
    });
    
    PoolInfo {
        total_icusd_deposited: total_icusd,
        total_depositors,
        pool_utilization: calculate_pool_utilization(),
    }
}

#[query]
#[candid_method(query)]
fn get_liquidation_history(limit: Option<u64>) -> Vec<LiquidationRecord> {
    let limit = limit.unwrap_or(10).min(100); // Cap at 100
    
    LIQUIDATIONS.with(|liquidations| {
        let liquidations = liquidations.borrow();
        let mut records: Vec<_> = liquidations.values().cloned().collect();
        records.sort_by(|a, b| b.liquidation_time.cmp(&a.liquidation_time));
        records.into_iter().take(limit as usize).collect()
    })
}

#[query]
#[candid_method(query)]
fn get_pool_state() -> PoolState {
    STATE.with(|state| state.borrow().clone())
}

// Admin functions

#[update]
#[candid_method(update)]
fn execute_liquidation(
    vault_id: u64,
    liquidated_debt: u64,
    collateral_received: u64,
    collateral_type: CollateralType,
) -> bool {
    // Only protocol backend can call this
    let _caller = caller();
    STATE.with(|state| {
        let _state = state.borrow();
        // TODO: Add proper authorization check
        // For now, we'll allow any caller for testing
        
        process_liquidation(vault_id, liquidated_debt, collateral_received, collateral_type)
    })
}

#[update]
#[candid_method(update)]
fn pause_protocol() -> bool {
    let caller = caller();
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if caller != state.protocol_owner {
            return false;
        }
        state.paused = true;
        true
    })
}

#[update]
#[candid_method(update)]
fn unpause_protocol() -> bool {
    let caller = caller();
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if caller != state.protocol_owner {
            return false;
        }
        state.paused = false;
        true
    })
}

#[update]
#[candid_method(update)]
fn update_liquidation_discount(new_discount: u8) -> bool {
    let caller = caller();
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if caller != state.protocol_owner {
            return false;
        }
        if new_discount > 50 { // Max 50% discount
            return false;
        }
        state.liquidation_discount = new_discount;
        true
    })
}

// Monitoring and automated liquidation functions

#[update]
#[candid_method(update)]
fn start_monitoring() -> bool {
    let caller = caller();
    STATE.with(|state| {
        let state = state.borrow();
        if caller != state.protocol_owner {
            return false;
        }
        
        // Start the monitoring timer
        start_monitoring_timer();
        true
    })
}

#[update]
#[candid_method(update)]
async fn manual_liquidation_check() -> ManualLiquidationResult {
    let caller = caller();
    
    // Allow any caller for now, but could be restricted to admin
    match monitor_and_liquidate().await {
        Ok(count) => ManualLiquidationResult {
            success: true,
            liquidations_executed: count,
            message: format!("Successfully executed {} liquidations", count),
        },
        Err(e) => ManualLiquidationResult {
            success: false,
            liquidations_executed: 0,
            message: format!("Failed to execute liquidations: {}", e),
        }
    }
}

#[update]
#[candid_method(update)]
fn set_protocol_backend(backend_canister: candid::Principal) -> bool {
    let caller = caller();
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if caller != state.protocol_owner {
            return false;
        }
        
        // Store the backend canister for monitoring
        // For now we'll store it in protocol_owner field as a workaround
        // In production, add a proper field to PoolState
        state.protocol_owner = backend_canister;
        true
    })
}

// ── ICRC-21: Canister Call Consent Messages ──

#[update]
#[candid_method(update)]
fn icrc21_canister_call_consent_message(
    request: Icrc21ConsentMessageRequest,
) -> Icrc21ConsentMessageResponse {
    let message_text = match request.method.as_str() {
        "deposit" => {
            // Decode the arg to extract the amount
            match candid::decode_one::<u64>(&request.arg) {
                Ok(amount) => {
                    let formatted = format_token_amount(amount);
                    format!(
                        "## Deposit to Stability Pool\n\n\
                         You are depositing **{} icUSD** into the Rumi Protocol Stability Pool.\n\n\
                         Your deposit earns liquidation rewards proportional to your share of the pool.",
                        formatted
                    )
                }
                Err(_) => "Deposit icUSD into the Rumi Protocol Stability Pool.".to_string(),
            }
        }
        "withdraw" => {
            match candid::decode_one::<u64>(&request.arg) {
                Ok(amount) => {
                    let formatted = format_token_amount(amount);
                    format!(
                        "## Withdraw from Stability Pool\n\n\
                         You are withdrawing **{} icUSD** from the Rumi Protocol Stability Pool.",
                        formatted
                    )
                }
                Err(_) => "Withdraw icUSD from the Rumi Protocol Stability Pool.".to_string(),
            }
        }
        "claim_collateral_rewards" => {
            match candid::decode_one::<Vec<u64>>(&request.arg) {
                Ok(ids) => {
                    format!(
                        "## Claim Collateral Rewards\n\n\
                         You are claiming liquidation rewards from **{}** liquidation event(s).",
                        ids.len()
                    )
                }
                Err(_) => "Claim collateral rewards from liquidation events.".to_string(),
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
            language: request.user_preferences.language.clone(),
            utc_offset_minutes: request.user_preferences.utc_offset_minutes,
        },
    })
}

// ── ICRC-10: Supported Standards ──

#[query]
#[candid_method(query)]
fn icrc10_supported_standards() -> Vec<Icrc10SupportedStandard> {
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

// Helper functions

fn format_token_amount(amount_e8s: u64) -> String {
    let whole = amount_e8s / 100_000_000;
    let frac = amount_e8s % 100_000_000;
    if frac == 0 {
        format!("{}", whole)
    } else {
        // Trim trailing zeros from fractional part
        let frac_str = format!("{:08}", frac);
        let trimmed = frac_str.trim_end_matches('0');
        format!("{}.{}", whole, trimmed)
    }
}

fn is_paused() -> bool {
    STATE.with(|state| state.borrow().paused)
}

fn calculate_pool_utilization() -> f64 {
    // This would calculate how much of the pool is currently being used for liquidations
    // For now, return a placeholder
    0.0
}

// Export candid interface
export_service!();

#[query(name = "__get_candid_interface_tmp_hack")]
fn export_candid() -> String {
    __export_service()
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_liquidation_share_calculation() {
        let user_icusd = 1000;
        let total_pool = 10000;
        let collateral = 500;
        
        let share = calculate_liquidation_share(user_icusd, total_pool, collateral);
        assert_eq!(share, 50); // 10% of 500
    }
}