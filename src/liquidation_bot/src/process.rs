use candid::{CandidType, Deserialize, Nat, Principal};
use ic_canister_log::log;
use icrc_ledger_types::icrc1::account::Account;

use crate::state::{self, BotConfig, BotLiquidationEvent, LiquidatableVaultInfo};
use crate::swap;

/// Result returned by the backend's `bot_claim_liquidation` and `dev_force_bot_liquidate` endpoints.
#[derive(CandidType, Deserialize, Debug)]
pub struct BotLiquidationResult {
    pub vault_id: u64,
    pub collateral_amount: u64,
    pub debt_covered: u64,
    pub collateral_price_e8s: u64,
}

/// Wrapper for backend Result variant.
#[derive(CandidType, Deserialize, Debug)]
pub enum BackendResult<T> {
    #[serde(rename = "Ok")]
    Ok(T),
    #[serde(rename = "Err")]
    Err(BackendError),
}

#[derive(CandidType, Deserialize, Debug)]
pub enum BackendError {
    GenericError(String),
    TemporarilyUnavailable(String),
    AnonymousCallerNotAllowed,
    AmountTooLow { minimum_amount: u64 },
    InsufficientFunds { balance: u64 },
    VaultNotFound { vault_id: u64 },
    TransferError(String),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

pub async fn process_pending() {
    let vault = state::mutate_state(|s| s.pending_vaults.pop());
    let Some(vault) = vault else { return };

    let config = match state::read_state(|s| s.config.clone()) {
        Some(c) => c,
        None => {
            log!(crate::INFO, "Bot not configured, skipping vault #{}", vault.vault_id);
            return;
        }
    };

    log!(crate::INFO, "Processing vault #{}", vault.vault_id);

    // Phase 1: CLAIM the vault (gets collateral, locks vault, but debt unchanged)
    let liq_result = call_bot_claim_liquidation(&config, vault.vault_id).await;
    let (collateral_amount, debt_covered, collateral_price) = match liq_result {
        Ok(r) => (r.collateral_amount, r.debt_covered, r.collateral_price_e8s),
        Err(e) => {
            log_failed_event(&vault, &format!("bot_claim_liquidation failed: {}", e));
            return;
        }
    };

    // Phase 2: Try to swap collateral → icUSD
    let swap_amount = calculate_swap_amount_internal(collateral_amount, debt_covered, collateral_price);
    let swap_result = swap::swap_icp_for_stable(&config, swap_amount).await;

    let (stable_amount, stable_token, route) = match swap_result {
        Ok(r) => (r.output_amount, r.target_token, r.route),
        Err(e) => {
            // SWAP FAILED — return collateral and cancel the claim
            log!(crate::INFO, "DEX swap failed for vault #{}: {}. Returning collateral and cancelling.", vault.vault_id, e);

            if let Err(return_err) = return_collateral_to_backend(&config, collateral_amount, vault.collateral_type).await {
                log!(crate::INFO, "WARNING: Failed to return collateral for vault #{}: {}", vault.vault_id, return_err);
            }

            if let Err(cancel_err) = call_bot_cancel_liquidation(&config, vault.vault_id).await {
                log!(crate::INFO, "WARNING: Failed to cancel claim for vault #{}: {}", vault.vault_id, cancel_err);
            }

            log_failed_event(&vault, &format!("DEX swap failed (claim cancelled): {}", e));
            return;
        }
    };

    // Phase 2b: ckStable → icUSD (3pool)
    let icusd_result = swap::swap_stable_for_icusd(&config, stable_amount, stable_token).await;
    let icusd_amount = match icusd_result {
        Ok(amount) => amount,
        Err(e) => {
            // 3pool swap failed — we already swapped ICP to ckStable, can't easily reverse that.
            // Cancel the claim so the stability pool can handle the vault.
            // The bot keeps the ckStable (can be manually recovered).
            log!(crate::INFO, "3pool swap failed for vault #{}: {}. Cancelling claim.", vault.vault_id, e);

            if let Err(cancel_err) = call_bot_cancel_liquidation(&config, vault.vault_id).await {
                log!(crate::INFO, "WARNING: Failed to cancel claim for vault #{}: {}", vault.vault_id, cancel_err);
            }

            log_failed_event(&vault, &format!("3pool swap failed (claim cancelled, ckStable held by bot): {}", e));
            return;
        }
    };

    // Phase 2c: Deposit icUSD to backend reserves
    if let Err(e) = call_bot_deposit_to_reserves(&config, icusd_amount).await {
        log!(crate::INFO, "deposit_to_reserves failed for vault #{}: {}. Cancelling claim.", vault.vault_id, e);
        if let Err(cancel_err) = call_bot_cancel_liquidation(&config, vault.vault_id).await {
            log!(crate::INFO, "WARNING: Failed to cancel claim for vault #{}: {}", vault.vault_id, cancel_err);
        }
        log_failed_event(&vault, &format!("deposit_to_reserves failed (claim cancelled): {}", e));
        return;
    }

    // Phase 3: CONFIRM — everything succeeded, finalize the liquidation
    if let Err(e) = call_bot_confirm_liquidation(&config, vault.vault_id).await {
        log!(crate::INFO, "CRITICAL: bot_confirm_liquidation failed for vault #{}: {}. icUSD already deposited!", vault.vault_id, e);
        log_failed_event(&vault, &format!("CRITICAL: confirm failed after deposit: {}", e));
        return;
    }

    // Phase 4: Send remaining ICP to treasury (liquidation bonus)
    let icp_to_treasury = collateral_amount.saturating_sub(swap_amount);
    if icp_to_treasury > 0 {
        let _ = transfer_icp_to_treasury(&config, icp_to_treasury).await;
    }

    // Phase 5: Log success
    let effective_price = if swap_amount > 0 {
        (stable_amount as u128 * 100_000_000 / swap_amount as u128) as u64
    } else {
        0
    };
    let slippage_bps = calculate_slippage(effective_price, collateral_price);

    state::mutate_state(|s| {
        s.stats.total_debt_covered_e8s += debt_covered;
        s.stats.total_icusd_burned_e8s += icusd_amount;
        s.stats.total_collateral_received_e8s += collateral_amount;
        s.stats.total_collateral_to_treasury_e8s += icp_to_treasury;
        s.stats.events_count += 1;
        s.liquidation_events.push(BotLiquidationEvent {
            timestamp: ic_cdk::api::time(),
            vault_id: vault.vault_id,
            debt_covered_e8s: debt_covered,
            collateral_received_e8s: collateral_amount,
            icusd_burned_e8s: icusd_amount,
            collateral_to_treasury_e8s: icp_to_treasury,
            swap_route: route,
            effective_price_e8s: effective_price,
            slippage_bps,
            success: true,
            error_message: None,
        });
    });

    log!(
        crate::INFO,
        "Vault #{} liquidated: debt={}, collateral={}, icUSD={}, treasury={}",
        vault.vault_id, debt_covered, collateral_amount, icusd_amount, icp_to_treasury
    );
}

// ─── Helper Functions ───

async fn call_bot_claim_liquidation(config: &BotConfig, vault_id: u64) -> Result<BotLiquidationResult, String> {
    let result: Result<(BackendResult<BotLiquidationResult>,), _> =
        ic_cdk::call(config.backend_principal, "bot_claim_liquidation", (vault_id,)).await;

    match result {
        Ok((BackendResult::Ok(r),)) => Ok(r),
        Ok((BackendResult::Err(e),)) => Err(format!("{}", e)),
        Err((code, msg)) => Err(format!("{:?}: {}", code, msg)),
    }
}

async fn call_bot_confirm_liquidation(config: &BotConfig, vault_id: u64) -> Result<(), String> {
    let result: Result<(BackendResult<()>,), _> =
        ic_cdk::call(config.backend_principal, "bot_confirm_liquidation", (vault_id,)).await;

    match result {
        Ok((BackendResult::Ok(()),)) => Ok(()),
        Ok((BackendResult::Err(e),)) => Err(format!("{}", e)),
        Err((code, msg)) => Err(format!("{:?}: {}", code, msg)),
    }
}

async fn call_bot_cancel_liquidation(config: &BotConfig, vault_id: u64) -> Result<(), String> {
    let result: Result<(BackendResult<()>,), _> =
        ic_cdk::call(config.backend_principal, "bot_cancel_liquidation", (vault_id,)).await;

    match result {
        Ok((BackendResult::Ok(()),)) => Ok(()),
        Ok((BackendResult::Err(e),)) => Err(format!("{}", e)),
        Err((code, msg)) => Err(format!("{:?}: {}", code, msg)),
    }
}

/// Transfer collateral back to the backend canister via direct icrc1_transfer.
/// No approve needed — the bot is sending from its own account.
async fn return_collateral_to_backend(config: &BotConfig, amount: u64, collateral_ledger: Principal) -> Result<(), String> {
    let transfer_args = icrc_ledger_types::icrc1::transfer::TransferArg {
        from_subaccount: None,
        to: Account {
            owner: config.backend_principal,
            subaccount: None,
        },
        amount: Nat::from(amount),
        fee: None,
        memo: None,
        created_at_time: None,
    };

    let result: Result<(Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError>,), _> =
        ic_cdk::call(collateral_ledger, "icrc1_transfer", (transfer_args,)).await;

    match result {
        Ok((Ok(_),)) => Ok(()),
        Ok((Err(e),)) => Err(format!("Transfer error: {:?}", e)),
        Err((code, msg)) => Err(format!("Transfer call failed: {:?} {}", code, msg)),
    }
}

async fn call_bot_deposit_to_reserves(config: &BotConfig, amount_e8s: u64) -> Result<(), String> {
    // First approve the backend to spend our icUSD
    let approve_args = icrc_ledger_types::icrc2::approve::ApproveArgs {
        from_subaccount: None,
        spender: Account {
            owner: config.backend_principal,
            subaccount: None,
        },
        amount: Nat::from(amount_e8s * 2),
        expected_allowance: None,
        expires_at: Some(ic_cdk::api::time() + 300_000_000_000),
        fee: None,
        memo: None,
        created_at_time: None,
    };

    let approve_result: Result<(Result<Nat, icrc_ledger_types::icrc2::approve::ApproveError>,), _> =
        ic_cdk::call(config.icusd_ledger, "icrc2_approve", (approve_args,)).await;

    match approve_result {
        Ok((Ok(_),)) => {}
        Ok((Err(e),)) => return Err(format!("icUSD approve failed: {:?}", e)),
        Err((code, msg)) => return Err(format!("icUSD approve call failed: {:?} {}", code, msg)),
    }

    // Call bot_deposit_to_reserves on backend
    let result: Result<(BackendResult<()>,), _> =
        ic_cdk::call(config.backend_principal, "bot_deposit_to_reserves", (amount_e8s,)).await;

    match result {
        Ok((BackendResult::Ok(()),)) => Ok(()),
        Ok((BackendResult::Err(e),)) => Err(format!("{}", e)),
        Err((code, msg)) => Err(format!("{:?}: {}", code, msg)),
    }
}

async fn transfer_icp_to_treasury(config: &BotConfig, amount_e8s: u64) -> Result<(), String> {
    let transfer_args = icrc_ledger_types::icrc1::transfer::TransferArg {
        from_subaccount: None,
        to: Account {
            owner: config.treasury_principal,
            subaccount: None,
        },
        amount: Nat::from(amount_e8s),
        fee: None,
        memo: None,
        created_at_time: None,
    };

    let result: Result<(Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError>,), _> =
        ic_cdk::call(config.icp_ledger, "icrc1_transfer", (transfer_args,)).await;

    match result {
        Ok((Ok(_),)) => {
            log!(crate::INFO, "Transferred {} e8s ICP to treasury", amount_e8s);
            Ok(())
        }
        Ok((Err(e),)) => Err(format!("ICP transfer to treasury failed: {:?}", e)),
        Err((code, msg)) => Err(format!("ICP transfer call failed: {:?} {}", code, msg)),
    }
}

fn calculate_swap_amount_internal(collateral_e8s: u64, debt_e8s: u64, collateral_price_e8s: u64) -> u64 {
    if collateral_price_e8s == 0 {
        return collateral_e8s;
    }
    let icp_needed = (debt_e8s as u128 * 100_000_000 / collateral_price_e8s as u128) as u64;
    let with_buffer = icp_needed.saturating_mul(105) / 100;
    with_buffer.min(collateral_e8s)
}

/// Calculate slippage in basis points between effective price and oracle price.
fn calculate_slippage(effective_price_e8s: u64, oracle_price_e8s: u64) -> i32 {
    if oracle_price_e8s == 0 || effective_price_e8s == 0 {
        return 0;
    }
    let diff = oracle_price_e8s as i64 - effective_price_e8s as i64;
    (diff * 10_000 / oracle_price_e8s as i64) as i32
}

// ─── Public wrappers for test functions ───

pub async fn call_bot_deposit_to_reserves_pub(config: &BotConfig, amount_e8s: u64) -> Result<(), String> {
    call_bot_deposit_to_reserves(config, amount_e8s).await
}

pub async fn transfer_icp_to_treasury_pub(config: &BotConfig, amount_e8s: u64) -> Result<(), String> {
    transfer_icp_to_treasury(config, amount_e8s).await
}

pub fn calculate_swap_amount(collateral_e8s: u64, debt_e8s: u64, collateral_price_e8s: u64) -> u64 {
    calculate_swap_amount_internal(collateral_e8s, debt_e8s, collateral_price_e8s)
}

pub async fn call_bot_cancel_liquidation_pub(config: &BotConfig, vault_id: u64) -> Result<(), String> {
    call_bot_cancel_liquidation(config, vault_id).await
}

pub async fn call_bot_confirm_liquidation_pub(config: &BotConfig, vault_id: u64) -> Result<(), String> {
    call_bot_confirm_liquidation(config, vault_id).await
}

fn log_failed_event(vault: &LiquidatableVaultInfo, error: &str) {
    log!(crate::INFO, "FAILED vault #{}: {}", vault.vault_id, error);
    state::mutate_state(|s| {
        s.stats.events_count += 1;
        s.liquidation_events.push(BotLiquidationEvent {
            timestamp: ic_cdk::api::time(),
            vault_id: vault.vault_id,
            debt_covered_e8s: 0,
            collateral_received_e8s: 0,
            icusd_burned_e8s: 0,
            collateral_to_treasury_e8s: 0,
            swap_route: String::new(),
            effective_price_e8s: 0,
            slippage_bps: 0,
            success: false,
            error_message: Some(error.to_string()),
        });
    });
}
