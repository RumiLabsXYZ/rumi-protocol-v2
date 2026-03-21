use candid::{CandidType, Deserialize, Nat};
use ic_cdk_macros::{init, post_upgrade, pre_upgrade, query, update};
use ic_canister_log::{declare_log_buffer, log};
use icrc_ledger_types::icrc1::account::Account;

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

// ─── Test Functions ───

#[derive(CandidType, Deserialize)]
pub struct TestSwapResult {
    pub icp_input_e8s: u64,
    pub stable_output_native: u64,
    pub stable_route: String,
    pub icusd_output_e8s: u64,
    pub icusd_sent_to: String,
}

#[derive(CandidType, Deserialize)]
pub struct TestForceResult {
    pub vault_id: u64,
    pub collateral_received_e8s: u64,
    pub debt_covered_e8s: u64,
    pub stable_output_native: u64,
    pub stable_route: String,
    pub icusd_output_e8s: u64,
    pub icusd_deposited_to_reserves: bool,
    pub icp_to_treasury_e8s: u64,
}

fn require_admin() {
    let caller = ic_cdk::api::caller();
    let is_admin = state::read_state(|s| {
        s.config.as_ref().map(|c| c.admin == caller).unwrap_or(false)
    });
    if !is_admin {
        ic_cdk::trap("Unauthorized: only admin can call test functions");
    }
}

/// Test the swap pipeline: ICP → ckStable → icUSD.
/// Send ICP to the bot first, then call this. The icUSD output is sent back to the caller.
#[update]
async fn test_swap_pipeline(amount_e8s: u64) -> TestSwapResult {
    require_admin();

    let config = state::read_state(|s| s.config.clone())
        .expect("Bot not configured");

    log!(INFO, "[test_swap_pipeline] Starting with {} e8s ICP", amount_e8s);

    // Step 1: ICP → ckStable (KongSwap)
    let stable_result = swap::swap_icp_for_stable(&config, amount_e8s).await;
    let (stable_amount, _stable_token, route) = match stable_result {
        Ok(r) => {
            log!(INFO, "[test_swap_pipeline] KongSwap OK: {} native via {}", r.output_amount, r.route);
            (r.output_amount, r.target_token, r.route)
        }
        Err(e) => ic_cdk::trap(&format!("KongSwap failed: {}", e)),
    };

    // Step 2: ckStable → icUSD (3pool)
    let icusd_amount = match swap::swap_stable_for_icusd(&config, stable_amount, _stable_token).await {
        Ok(amount) => {
            log!(INFO, "[test_swap_pipeline] 3pool OK: {} e8s icUSD", amount);
            amount
        }
        Err(e) => ic_cdk::trap(&format!("3pool swap failed: {}", e)),
    };

    // Step 3: Send icUSD back to caller
    let caller = ic_cdk::api::caller();
    let transfer_args = icrc_ledger_types::icrc1::transfer::TransferArg {
        from_subaccount: None,
        to: Account { owner: caller, subaccount: None },
        amount: Nat::from(icusd_amount),
        fee: None,
        memo: None,
        created_at_time: None,
    };

    let sent_to = match ic_cdk::call::<_, (Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError>,)>(
        config.icusd_ledger, "icrc1_transfer", (transfer_args,)
    ).await {
        Ok((Ok(_),)) => {
            log!(INFO, "[test_swap_pipeline] Sent {} e8s icUSD to {}", icusd_amount, caller);
            format!("{}", caller)
        }
        Ok((Err(e),)) => {
            log!(INFO, "[test_swap_pipeline] icUSD transfer failed: {:?}, keeping in bot", e);
            "bot (transfer failed)".to_string()
        }
        Err((code, msg)) => {
            log!(INFO, "[test_swap_pipeline] icUSD transfer call failed: {:?} {}", code, msg);
            "bot (call failed)".to_string()
        }
    };

    TestSwapResult {
        icp_input_e8s: amount_e8s,
        stable_output_native: stable_amount,
        stable_route: route,
        icusd_output_e8s: icusd_amount,
        icusd_sent_to: sent_to,
    }
}

/// Force-liquidate a vault (bypasses health ratio check on backend).
/// Uses the two-phase claim/confirm/cancel pattern.
#[update]
async fn test_force_liquidate(vault_id: u64) -> TestForceResult {
    require_admin();

    let config = state::read_state(|s| s.config.clone())
        .expect("Bot not configured");

    log!(INFO, "[test_force_liquidate] Force-liquidating vault #{}", vault_id);

    // Step 1: Call dev_force_bot_liquidate (now uses claim pattern — locks vault, gets collateral)
    let liq_result: Result<(process::BackendResult<process::BotLiquidationResult>,), _> =
        ic_cdk::call(config.backend_principal, "dev_force_bot_liquidate", (vault_id,)).await;

    let (collateral_amount, debt_covered, collateral_price) = match liq_result {
        Ok((process::BackendResult::Ok(r),)) => {
            log!(INFO, "[test_force_liquidate] Claimed {} e8s collateral, {} e8s debt", r.collateral_amount, r.debt_covered);
            (r.collateral_amount, r.debt_covered, r.collateral_price_e8s)
        }
        Ok((process::BackendResult::Err(e),)) => ic_cdk::trap(&format!("dev_force_bot_liquidate error: {}", e)),
        Err((code, msg)) => ic_cdk::trap(&format!("dev_force_bot_liquidate call failed: {:?} {}", code, msg)),
    };

    // Step 2: Swap ICP → ckStable
    let swap_amount = process::calculate_swap_amount(collateral_amount, debt_covered, collateral_price);
    let stable_result = swap::swap_icp_for_stable(&config, swap_amount).await;
    let (stable_amount, stable_token, route) = match stable_result {
        Ok(r) => {
            log!(INFO, "[test_force_liquidate] KongSwap OK: {} native via {}", r.output_amount, r.route);
            (r.output_amount, r.target_token, r.route)
        }
        Err(e) => {
            // Cancel and trap
            let _ = process::call_bot_cancel_liquidation_pub(&config, vault_id).await;
            ic_cdk::trap(&format!("KongSwap failed (claim cancelled): {}", e));
        }
    };

    // Step 3: ckStable → icUSD
    let icusd_amount = match swap::swap_stable_for_icusd(&config, stable_amount, stable_token).await {
        Ok(amount) => amount,
        Err(e) => {
            let _ = process::call_bot_cancel_liquidation_pub(&config, vault_id).await;
            ic_cdk::trap(&format!("3pool swap failed (claim cancelled): {}", e));
        }
    };

    // Step 4: Deposit icUSD to backend reserves
    let deposited = match process::call_bot_deposit_to_reserves_pub(&config, icusd_amount).await {
        Ok(()) => true,
        Err(e) => {
            log!(INFO, "[test_force_liquidate] Deposit failed: {}", e);
            false
        }
    };

    // Step 5: CONFIRM the liquidation (finalize vault state)
    let confirmed = match process::call_bot_confirm_liquidation_pub(&config, vault_id).await {
        Ok(()) => {
            log!(INFO, "[test_force_liquidate] Confirmed liquidation for vault #{}", vault_id);
            true
        }
        Err(e) => {
            log!(INFO, "[test_force_liquidate] Confirm failed: {}", e);
            false
        }
    };

    // Step 6: Send remaining ICP to treasury
    let icp_to_treasury = collateral_amount.saturating_sub(swap_amount);
    if icp_to_treasury > 0 {
        let _ = process::transfer_icp_to_treasury_pub(&config, icp_to_treasury).await;
    }

    // Log event
    state::mutate_state(|s| {
        s.stats.events_count += 1;
        s.liquidation_events.push(BotLiquidationEvent {
            timestamp: ic_cdk::api::time(),
            vault_id,
            debt_covered_e8s: debt_covered,
            collateral_received_e8s: collateral_amount,
            icusd_burned_e8s: icusd_amount,
            collateral_to_treasury_e8s: icp_to_treasury,
            swap_route: route.clone(),
            effective_price_e8s: 0,
            slippage_bps: 0,
            success: confirmed && deposited,
            error_message: if confirmed { Some("test_force_liquidate".to_string()) } else { Some("confirm failed".to_string()) },
        });
    });

    TestForceResult {
        vault_id,
        collateral_received_e8s: collateral_amount,
        debt_covered_e8s: debt_covered,
        stable_output_native: stable_amount,
        stable_route: route,
        icusd_output_e8s: icusd_amount,
        icusd_deposited_to_reserves: deposited,
        icp_to_treasury_e8s: icp_to_treasury,
    }
}
