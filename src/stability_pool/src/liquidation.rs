use std::collections::BTreeMap;
use candid::Principal;
use ic_canister_log::log;
use ic_cdk::call;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::approve::{ApproveArgs, ApproveError};

use crate::logs::INFO;
use crate::types::*;
use crate::state::{read_state, mutate_state};

/// Called by the backend when it detects liquidatable vaults (push model).
/// Processes each vault sequentially, consuming stablecoins and distributing collateral.
pub async fn notify_liquidatable_vaults(vaults: Vec<LiquidatableVaultInfo>) -> Vec<LiquidationResult> {
    if read_state(|s| s.configuration.emergency_pause) {
        log!(INFO, "Pool is paused — ignoring {} liquidatable vaults", vaults.len());
        return vec![];
    }

    log!(INFO, "Received push notification: {} liquidatable vaults", vaults.len());

    let max_batch = read_state(|s| s.configuration.max_liquidations_per_batch) as usize;

    let mut results = Vec::new();
    for vault_info in vaults.into_iter().take(max_batch) {
        // Skip if already in-flight
        if read_state(|s| s.in_flight_liquidations.contains(&vault_info.vault_id)) {
            log!(INFO, "Vault {} already in-flight, skipping", vault_info.vault_id);
            continue;
        }

        // Check effective pool coverage for this collateral type
        let effective_pool = read_state(|s| s.effective_pool_for_collateral(&vault_info.collateral_type));
        if effective_pool < vault_info.debt_amount {
            log!(INFO, "Insufficient pool coverage for vault {}: need {} e8s, have {} e8s",
                vault_info.vault_id, vault_info.debt_amount, effective_pool);
            continue;
        }

        // Mark as in-flight
        mutate_state(|s| { s.in_flight_liquidations.insert(vault_info.vault_id); });

        let result = execute_single_liquidation(&vault_info).await;

        // Clear in-flight
        mutate_state(|s| { s.in_flight_liquidations.remove(&vault_info.vault_id); });

        if result.success {
            log!(INFO, "Liquidated vault {}: gained {} collateral",
                vault_info.vault_id, result.collateral_gained);
        } else {
            log!(INFO, "Liquidation failed for vault {}: {}",
                vault_info.vault_id, result.error_message.as_deref().unwrap_or("unknown"));
        }

        results.push(result);
    }

    results
}

/// Public fallback: anyone can call this to trigger a liquidation for a specific vault.
/// Per-caller guard is enforced at the lib.rs level.
pub async fn execute_liquidation(vault_id: u64) -> Result<LiquidationResult, StabilityPoolError> {
    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    if read_state(|s| s.in_flight_liquidations.contains(&vault_id)) {
        return Err(StabilityPoolError::SystemBusy);
    }

    // Fetch vault info from backend
    let protocol_id = read_state(|s| s.protocol_canister_id);

    let (vaults,): (Vec<rumi_protocol_backend::vault::CandidVault>,) = call(
        protocol_id, "get_liquidatable_vaults", ()
    ).await.map_err(|_e| StabilityPoolError::InterCanisterCallFailed {
        target: "Protocol".to_string(),
        method: "get_liquidatable_vaults".to_string(),
    })?;
    let target_vault = vaults.into_iter().find(|v| v.vault_id == vault_id);

    let vault = match target_vault {
        Some(v) => v,
        None => return Err(StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "Vault not found in liquidatable list".to_string(),
        }),
    };

    let vault_info = LiquidatableVaultInfo {
        vault_id: vault.vault_id,
        collateral_type: vault.collateral_type,
        debt_amount: vault.borrowed_icusd_amount,
        collateral_amount: vault.icp_margin_amount,
    };

    // Check pool coverage
    let effective_pool = read_state(|s| s.effective_pool_for_collateral(&vault_info.collateral_type));
    if effective_pool < vault_info.debt_amount {
        return Err(StabilityPoolError::InsufficientPoolBalance);
    }

    mutate_state(|s| { s.in_flight_liquidations.insert(vault_id); });
    let result = execute_single_liquidation(&vault_info).await;
    mutate_state(|s| { s.in_flight_liquidations.remove(&vault_id); });

    Ok(result)
}

/// Core liquidation logic for a single vault.
async fn execute_single_liquidation(vault_info: &LiquidatableVaultInfo) -> LiquidationResult {
    let protocol_id = read_state(|s| s.protocol_canister_id);

    // Step 1: Compute token draw
    let token_draw = read_state(|s| s.compute_token_draw(vault_info.debt_amount, &vault_info.collateral_type));

    if token_draw.is_empty() {
        return LiquidationResult {
            vault_id: vault_info.vault_id,
            stables_consumed: BTreeMap::new(),
            collateral_gained: 0,
            collateral_type: vault_info.collateral_type,
            success: false,
            error_message: Some("No stablecoins available for liquidation".to_string()),
        };
    }

    log!(INFO, "Token draw for vault {}: {:?}", vault_info.vault_id, token_draw);

    // Step 2: Approve backend for each token and call appropriate liquidation endpoint
    let mut total_collateral_gained: u64 = 0;
    let mut actual_consumed: BTreeMap<Principal, u64> = BTreeMap::new();

    // Get stablecoin configs for classification
    let stablecoin_configs: BTreeMap<Principal, StablecoinConfig> = read_state(|s| s.stablecoin_registry.clone());
    let icusd_ledger = stablecoin_configs.iter()
        .find(|(_, c)| c.symbol == "icUSD")
        .map(|(id, _)| *id);

    for (token_ledger, amount) in &token_draw {
        let is_icusd = icusd_ledger.map(|id| id == *token_ledger).unwrap_or(false);

        // Approve backend to spend this token
        let approve_args = ApproveArgs {
            from_subaccount: None,
            spender: Account { owner: protocol_id, subaccount: None },
            amount: candid::Nat::from(*amount as u128 * 2), // 2x buffer for fees
            expected_allowance: None,
            expires_at: Some(ic_cdk::api::time() + 300_000_000_000), // 5 min
            fee: None,
            memo: None,
            created_at_time: Some(ic_cdk::api::time()),
        };

        let approve_result: Result<(Result<candid::Nat, ApproveError>,), _> = call(
            *token_ledger, "icrc2_approve", (approve_args,)
        ).await;

        match approve_result {
            Ok((Ok(_),)) => {},
            Ok((Err(e),)) => {
                log!(INFO, "Approve failed for {}: {:?}", token_ledger, e);
                continue;
            },
            Err(e) => {
                log!(INFO, "Approve call failed for {}: {:?}", token_ledger, e);
                continue;
            }
        }

        // Call the appropriate backend endpoint
        let liq_result = if is_icusd {
            // liquidate_vault_partial(vault_id, amount_e8s)
            let call_result: Result<(Result<rumi_protocol_backend::SuccessWithFee, rumi_protocol_backend::ProtocolError>,), _> = call(
                protocol_id,
                "liquidate_vault_partial",
                (rumi_protocol_backend::vault::VaultArg {
                    vault_id: vault_info.vault_id,
                    amount: *amount,
                },)
            ).await;
            call_result.map(|(r,)| r)
        } else {
            // Determine StableTokenType from ledger principal
            let token_type = determine_stable_token_type(*token_ledger, &stablecoin_configs);
            match token_type {
                Some(tt) => {
                    let call_result: Result<(Result<rumi_protocol_backend::SuccessWithFee, rumi_protocol_backend::ProtocolError>,), _> = call(
                        protocol_id,
                        "liquidate_vault_partial_with_stable",
                        (rumi_protocol_backend::VaultArgWithToken {
                            vault_id: vault_info.vault_id,
                            amount: *amount,
                            token_type: tt,
                        },)
                    ).await;
                    call_result.map(|(r,)| r)
                },
                None => {
                    log!(INFO, "Unknown stable token type for {}, skipping", token_ledger);
                    continue;
                }
            }
        };

        match liq_result {
            Ok(Ok(success)) => {
                let collateral = success.collateral_amount_received.unwrap_or(success.fee_amount_paid);
                log!(INFO, "Liquidation call succeeded for vault {} with token {}: collateral={}, fee={}",
                    vault_info.vault_id, token_ledger, collateral, success.fee_amount_paid);
                actual_consumed.insert(*token_ledger, *amount);
                total_collateral_gained += collateral;
            },
            Ok(Err(protocol_error)) => {
                log!(INFO, "Protocol rejected liquidation for vault {} with token {}: {:?}",
                    vault_info.vault_id, token_ledger, protocol_error);
            },
            Err(call_error) => {
                log!(INFO, "Liquidation call failed for vault {} with token {}: {:?}",
                    vault_info.vault_id, token_ledger, call_error);
            }
        }
    }

    // Step 3: If any liquidation calls succeeded, process gains
    if !actual_consumed.is_empty() && total_collateral_gained > 0 {
        mutate_state(|s| {
            s.process_liquidation_gains(
                vault_info.vault_id,
                vault_info.collateral_type,
                &actual_consumed,
                total_collateral_gained,
            );
        });

        LiquidationResult {
            vault_id: vault_info.vault_id,
            stables_consumed: actual_consumed,
            collateral_gained: total_collateral_gained,
            collateral_type: vault_info.collateral_type,
            success: true,
            error_message: None,
        }
    } else {
        LiquidationResult {
            vault_id: vault_info.vault_id,
            stables_consumed: BTreeMap::new(),
            collateral_gained: 0,
            collateral_type: vault_info.collateral_type,
            success: false,
            error_message: Some("All liquidation calls failed".to_string()),
        }
    }
}

/// Thin translation layer: map a ledger principal to the backend's StableTokenType enum.
/// This goes away when the backend gets a dynamic stablecoin registry.
fn determine_stable_token_type(
    ledger: Principal,
    configs: &BTreeMap<Principal, StablecoinConfig>,
) -> Option<rumi_protocol_backend::StableTokenType> {
    let config = configs.get(&ledger)?;
    match config.symbol.as_str() {
        "ckUSDT" => Some(rumi_protocol_backend::StableTokenType::CKUSDT),
        "ckUSDC" => Some(rumi_protocol_backend::StableTokenType::CKUSDC),
        _ => None, // icUSD uses a different endpoint, other tokens not yet mapped
    }
}
