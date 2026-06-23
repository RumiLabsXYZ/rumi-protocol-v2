use std::collections::BTreeMap;
use candid::{Nat, Principal};
use ic_canister_log::log;
use ic_cdk::call;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{Memo, TransferArg, TransferError};
use icrc_ledger_types::icrc2::approve::{ApproveArgs, ApproveError};
use num_traits::ToPrimitive;

use crate::logs::INFO;
use crate::types::*;
use crate::state::{read_state, mutate_state};

/// Conservative fallback for a collateral ledger's transfer fee, used only when
/// the live `icrc1_fee` query fails (SP-104). Set to the common ICRC fee
/// (10_000 e8s, as on ICP/ckBTC-class ledgers). Over-estimating the fee
/// under-credits depositors slightly (solvency-safe) rather than over-crediting
/// them as a fee=0 fallback would. The next successful liquidation reconciles.
/// Shared with `claim_collateral`'s fee lookup (ICRC-004 / SP-203).
pub(crate) const FALLBACK_COLLATERAL_FEE_E8S: u64 = 10_000;

pub(crate) const CHAIN_WRITEDOWN_MEMO_PREFIX: &[u8] = b"RUMI-LIQ-004:";

pub fn encode_chain_writedown_memo(vault_id: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(CHAIN_WRITEDOWN_MEMO_PREFIX.len() + 8);
    out.extend_from_slice(CHAIN_WRITEDOWN_MEMO_PREFIX);
    out.extend_from_slice(&vault_id.to_be_bytes());
    out
}

pub fn build_icusd_burn_transfer_arg(
    minting_account: Account,
    amount_e8s: u64,
    vault_id: u64,
    created_at_time: u64,
) -> TransferArg {
    TransferArg {
        from_subaccount: None,
        to: minting_account,
        fee: None,
        created_at_time: Some(created_at_time),
        memo: Some(Memo::from(encode_chain_writedown_memo(vault_id))),
        amount: Nat::from(amount_e8s),
    }
}

pub fn build_icusd_burn_proof(
    block_index: u64,
    vault_id: u64,
) -> rumi_protocol_backend::icrc3_proof::SpWritedownProof {
    rumi_protocol_backend::icrc3_proof::SpWritedownProof {
        block_index,
        ledger_kind: rumi_protocol_backend::icrc3_proof::SpProofLedger::IcusdBurn,
        vault_id_memo: vault_id,
    }
}

pub async fn burn_icusd_for_chain_writedown(
    icusd_ledger: Principal,
    amount_e8s: u64,
    vault_id: u64,
) -> Result<rumi_protocol_backend::icrc3_proof::SpWritedownProof, StabilityPoolError> {
    if amount_e8s == 0 {
        return Err(StabilityPoolError::AmountTooLow { minimum_e8s: 1 });
    }

    let minting_account = match call::<(), (Option<Account>,)>(
        icusd_ledger,
        "icrc1_minting_account",
        (),
    ).await {
        Ok((Some(account),)) => account,
        Ok((None,)) => {
            return Err(StabilityPoolError::LedgerTransferFailed {
                reason: "icUSD ledger has no minting account; cannot burn".to_string(),
            });
        }
        Err(_) => {
            return Err(StabilityPoolError::InterCanisterCallFailed {
                target: format!("{}", icusd_ledger),
                method: "icrc1_minting_account".to_string(),
            });
        }
    };

    let transfer_arg = build_icusd_burn_transfer_arg(
        minting_account,
        amount_e8s,
        vault_id,
        ic_cdk::api::time(),
    );

    let result: Result<(Result<Nat, TransferError>,), _> = call(
        icusd_ledger,
        "icrc1_transfer",
        (transfer_arg,),
    ).await;

    let block_index = match result {
        Ok((Ok(block_index),)) => nat_block_index_to_u64(block_index)?,
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            nat_block_index_to_u64(duplicate_of)?
        }
        Ok((Err(error),)) => {
            return Err(StabilityPoolError::LedgerTransferFailed {
                reason: format!("{:?}", error),
            });
        }
        Err(_) => {
            return Err(StabilityPoolError::InterCanisterCallFailed {
                target: format!("{}", icusd_ledger),
                method: "icrc1_transfer".to_string(),
            });
        }
    };

    Ok(build_icusd_burn_proof(block_index, vault_id))
}

fn nat_block_index_to_u64(block_index: Nat) -> Result<u64, StabilityPoolError> {
    block_index.0.to_u64().ok_or_else(|| StabilityPoolError::LedgerTransferFailed {
        reason: format!("ledger block index {} does not fit in u64", block_index),
    })
}

/// Called by the backend when it detects liquidatable vaults (push model).
/// Processes each vault sequentially, consuming stablecoins and distributing collateral.
pub async fn notify_liquidatable_vaults(vaults: Vec<LiquidatableVaultInfo>) -> Vec<LiquidationResult> {
    if read_state(|s| s.configuration.emergency_pause) {
        log!(INFO, "Pool is paused — ignoring {} liquidatable vaults", vaults.len());
        return vec![];
    }

    // SP-102: hold the per-pool liquidation lock across the whole batch so
    // deposit/withdraw/claim cannot land between a vault's snapshot and its
    // burn apportionment (which would let a withdrawer escape their share).
    // If another liquidation is already running, skip this batch (no retry —
    // the backend re-notifies on its next tick).
    let _liq_guard = match crate::pool_guard::SpLiquidationGuard::new() {
        Ok(g) => g,
        Err(_) => {
            log!(INFO, "notify_liquidatable_vaults: a liquidation is already in flight; skipping this batch");
            return vec![];
        }
    };

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

/// Public fallback: anyone (except the anonymous principal) can call this to
/// trigger a liquidation for a specific vault.
///
/// SP-111 (audit 2026-06-05): the previous comment claimed a per-caller guard
/// was enforced at the lib.rs level — there was none. Concurrency is now
/// serialized by the per-pool `SpLiquidationGuard` acquired below (SP-102), and
/// the anonymous principal is rejected here to keep the permissionless trigger
/// from being driven by unauthenticated cycle-griefing callers.
pub async fn execute_liquidation(vault_id: u64) -> Result<LiquidationResult, StabilityPoolError> {
    if ic_cdk::api::caller() == Principal::anonymous() {
        return Err(StabilityPoolError::Unauthorized);
    }

    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    if read_state(|s| s.in_flight_liquidations.contains(&vault_id)) {
        return Err(StabilityPoolError::SystemBusy);
    }

    // SP-102: hold the per-pool liquidation lock across snapshot -> await ->
    // apportion so deposit/withdraw/claim cannot race the apportionment.
    let _liq_guard = crate::pool_guard::SpLiquidationGuard::new()?;

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
        recommended_liquidation_amount: 0,
        collateral_price_e8s: 0,
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
///
/// Strategy:
/// 1. Non-LP stablecoins (icUSD, ckUSDC, ckUSDT): approve backend → call liquidate_vault_partial
/// 2. LP tokens (3USD): burn on 3pool via authorized_redeem_and_burn → call backend
///    stability_pool_liquidate_debt_burned to write down debt and release collateral
///
/// No circuit breaker / suspension mechanism — if a token fails, we skip it and try the
/// next one. If they all fail, the liquidation simply doesn't happen this round.
async fn execute_single_liquidation(vault_info: &LiquidatableVaultInfo) -> LiquidationResult {
    let protocol_id = read_state(|s| s.protocol_canister_id);

    // Step 1: Compute token draw
    // Use recommended_liquidation_amount (partial cap) if available, otherwise full debt
    let draw_amount = if vault_info.recommended_liquidation_amount > 0 {
        vault_info.recommended_liquidation_amount
    } else {
        vault_info.debt_amount
    };
    let token_draw = read_state(|s| s.compute_token_draw(draw_amount, &vault_info.collateral_type));

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

    // Step 2: Process each token in the draw
    let mut total_collateral_gained: u64 = 0;
    let mut actual_consumed: BTreeMap<Principal, u64> = BTreeMap::new();

    let stablecoin_configs: BTreeMap<Principal, StablecoinConfig> = read_state(|s| s.stablecoin_registry.clone());
    let icusd_ledger = stablecoin_configs.iter()
        .find(|(_, c)| c.symbol == "icUSD")
        .map(|(id, _)| *id);

    // --- Non-LP tokens: approve + liquidate_vault_partial ---
    for (token_ledger, amount) in &token_draw {
        // Skip LP tokens — handled separately below
        if stablecoin_configs.get(token_ledger).map(|c| c.is_lp_token.unwrap_or(false)).unwrap_or(false) {
            continue;
        }

        let is_icusd = icusd_ledger.map(|id| id == *token_ledger).unwrap_or(false);
        let token_decimals = stablecoin_configs.get(token_ledger).map(|c| c.decimals).unwrap_or(8);

        // Pre-check: backend minimum is 10_000_000 e8s (0.1 icUSD)
        let amount_e8s_check = if is_icusd { *amount } else { crate::types::normalize_to_e8s(*amount, token_decimals) };
        if amount_e8s_check < 10_000_000 {
            log!(INFO, "Skipping token {}: amount {} e8s below backend minimum (0.1)", token_ledger, amount_e8s_check);
            continue;
        }

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
            Ok((Ok(_),)) => {
                // Deduct the approve fee from tracked balances
                if let Some(fee) = stablecoin_configs.get(token_ledger).and_then(|c| c.transfer_fee) {
                    if fee > 0 {
                        mutate_state(|s| s.deduct_fee_from_pool(*token_ledger, fee));
                    }
                }
            },
            Ok((Err(e),)) => {
                log!(INFO, "Approve failed for {}: {:?}", token_ledger, e);
                continue;
            },
            Err(e) => {
                log!(INFO, "Approve call failed for {}: {:?}", token_ledger, e);
                continue;
            }
        }

        // No pre-deduct of depositor balances: `process_liquidation_gains` is the
        // single point of truth for stablecoin bookkeeping on a successful
        // liquidation (SP-001 regression fix, audit 2026-04-22-28e9896). Calling
        // `deduct_burned_lp_from_balances` here previously caused depositor balances
        // and the aggregate total to be decremented twice per liquidation — once
        // pre-call, once inside `process_liquidation_gains_at` — leaving phantom
        // tokens in the pool account per liquidation.

        // Call the appropriate backend endpoint
        let liq_result = if is_icusd {
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
            let token_type = determine_stable_token_type(*token_ledger, &stablecoin_configs);
            match token_type {
                Some(tt) => {
                    let amount_e8s = crate::types::normalize_to_e8s(*amount, token_decimals);
                    let call_result: Result<(Result<rumi_protocol_backend::SuccessWithFee, rumi_protocol_backend::ProtocolError>,), _> = call(
                        protocol_id,
                        "liquidate_vault_partial_with_stable",
                        (rumi_protocol_backend::VaultArgWithToken {
                            vault_id: vault_info.vault_id,
                            amount: amount_e8s,
                            token_type: tt,
                        },)
                    ).await;
                    call_result.map(|(r,)| r)
                },
                None => {
                    // Backend was never called; no bookkeeping to roll back.
                    log!(INFO, "Unknown stable token type for {}, skipping", token_ledger);
                    continue;
                }
            }
        };

        match liq_result {
            Ok(Ok(success)) => {
                let collateral = success.collateral_amount_received.unwrap_or(success.fee_amount_paid);
                log!(INFO, "Liquidation succeeded for vault {} with token {}: collateral={}, fee={}",
                    vault_info.vault_id, token_ledger, collateral, success.fee_amount_paid);
                // SP-101 / SP-110: debit by what the backend ACTUALLY pulled from
                // the pool, not the amount we requested, so the tracked aggregate
                // never drifts from the real ledger balance. `process_liquidation_gains`
                // debits depositor balances exactly once, after this loop.
                //   - icUSD path: the backend pulled exactly the realized debt
                //     (`debt_liquidated_e8s`), no surcharge.
                //   - ckStable path: the backend pulled `base + repay-fee surcharge`
                //     (`stable_pulled_e6s`). Using only the base-debt conversion
                //     left the surcharge un-debited and the aggregate above the
                //     ledger (SP-110). Prefer the exact `stable_pulled_e6s`; fall
                //     back to the base conversion for an older backend wasm.
                let realized_consumed = match (success.debt_liquidated_e8s, is_icusd) {
                    (Some(debt_e8), true) => debt_e8,
                    (Some(debt_e8), false) => success
                        .stable_pulled_e6s
                        .unwrap_or_else(|| crate::types::denormalize_from_e8s(debt_e8, token_decimals)),
                    (None, _) => *amount,
                };
                actual_consumed.insert(*token_ledger, realized_consumed);
                total_collateral_gained += collateral;
                // Bug 7: one token per vault per round — vault state changed, remaining draws are stale
                break;
            },
            Ok(Err(protocol_error)) => {
                // Backend explicitly rejected; nothing was pre-deducted, so no rollback needed.
                log!(INFO, "Protocol rejected liquidation for vault {} with token {}: {:?}",
                    vault_info.vault_id, token_ledger, protocol_error);
            },
            Err(call_error) => {
                // Inter-canister call failed; outcome is unknown. We do NOT mutate
                // depositor bookkeeping here — the previous "conservative deduct" path
                // (SP-005) caused permanent depositor loss when the backend was in
                // fact a no-op. If the backend rolled forward (took the tokens via
                // transfer_from but failed to reply), the next liquidation or a manual
                // `correct_balance` reconciliation against `icrc1_balance_of(pool)`
                // will reconcile the divergence. Log loudly so operators notice.
                log!(INFO, "Liquidation call failed for vault {} with token {}: {:?}. \
                      No bookkeeping change; ledger balance should be reconciled if \
                      tokens moved silently.",
                    vault_info.vault_id, token_ledger, call_error);
            }
        }
    }

    // --- LP tokens (3USD): approve + backend pull (atomic) ---
    for (token_ledger, amount) in &token_draw {
        let config = match stablecoin_configs.get(token_ledger) {
            Some(c) if c.is_lp_token.unwrap_or(false) => c,
            _ => continue,
        };

        // Calculate icUSD equivalent using cached virtual price
        let vp = read_state(|s| {
            s.virtual_prices().get(token_ledger).copied().unwrap_or(1_000_000_000_000_000_000)
        });
        let icusd_equiv_e8s = lp_to_usd_e8s(*amount, vp);

        if icusd_equiv_e8s < 10_000_000 {
            log!(INFO, "Skipping LP token {}: icUSD equivalent {} e8s below backend minimum", token_ledger, icusd_equiv_e8s);
            continue;
        }

        // Step A: Approve backend to pull 3USD (same pattern as non-LP tokens)
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
            Ok((Ok(_),)) => {
                // Deduct the approve fee from tracked balances
                if let Some(fee) = config.transfer_fee {
                    if fee > 0 {
                        mutate_state(|s| s.deduct_fee_from_pool(*token_ledger, fee));
                    }
                }
            },
            Ok((Err(e),)) => {
                log!(INFO, "3USD approve failed for vault {}: {:?}", vault_info.vault_id, e);
                continue;
            },
            Err(e) => {
                log!(INFO, "3USD approve call failed for vault {}: {:?}", vault_info.vault_id, e);
                continue;
            }
        }

        // Step B: Ask backend to pull 3USD + write down debt atomically.
        // `process_liquidation_gains` runs once after this loop and is the single
        // point of truth for bookkeeping — no pre-deduct (SP-001 regression fix,
        // audit 2026-04-22-28e9896).

        let liq_result: Result<(Result<StabilityPoolLiquidationResult, rumi_protocol_backend::ProtocolError>,), _> = call(
            protocol_id,
            "stability_pool_liquidate_with_reserves",
            (vault_info.vault_id, icusd_equiv_e8s, *amount, *token_ledger),
        ).await;

        match liq_result {
            Ok((Ok(success),)) => {
                // VER-002 (audit 2026-06-05): the backend caps the writedown to
                // the vault's current debt and refunds the proportional excess
                // 3USD (see stability_pool_liquidate_with_reserves). Record only
                // the REALIZED 3USD using the SAME floor formula the backend
                // refund uses, so the SP's tracked aggregate and its ledger
                // balance both net to exactly the realized amount (no drift).
                // `icusd_equiv_e8s` here equals the `icusd_debt_covered_e8s` the
                // backend received, so the two formulas are identical.
                let realized_3usd = if icusd_equiv_e8s > 0 && success.liquidated_debt < icusd_equiv_e8s {
                    ((*amount as u128).saturating_mul(success.liquidated_debt as u128)
                        / icusd_equiv_e8s as u128) as u64
                } else {
                    *amount
                };
                actual_consumed.insert(*token_ledger, realized_3usd);
                total_collateral_gained += success.collateral_received;
                log!(INFO, "3USD reserves liquidation succeeded for vault {}: {} collateral, {} 3USD consumed (requested {})",
                    vault_info.vault_id, success.collateral_received, realized_3usd, amount);
                break; // one token per vault per round
            }
            Ok((Err(e),)) => {
                // Backend explicitly rejected; approval expires harmlessly and nothing
                // was pre-deducted, so there is no bookkeeping to roll back.
                log!(INFO, "Backend rejected 3USD reserves liquidation for vault {}: {:?}",
                    vault_info.vault_id, e);
            }
            Err(e) => {
                // Inter-canister call failed; outcome unknown. We do NOT mutate
                // depositor bookkeeping (SP-005 regression fix). If the backend
                // pulled the 3USD silently, operator reconciliation against
                // `icrc1_balance_of(pool)` will reconcile.
                log!(INFO, "3USD reserves liquidation call failed for vault {}: {:?}. \
                      No bookkeeping change; ledger balance should be reconciled if \
                      tokens moved silently.",
                    vault_info.vault_id, e);
            }
        }
    }

    // Record liquidation event
    let stables_consumed_e8s: u64 = actual_consumed.values().sum();
    let liq_success = !actual_consumed.is_empty() && total_collateral_gained > 0;
    mutate_state(|s| {
        s.push_event(
            s.protocol_canister_id,
            PoolEventType::LiquidationExecuted {
                vault_id: vault_info.vault_id,
                stables_consumed_e8s,
                collateral_gained: total_collateral_gained,
                collateral_type: vault_info.collateral_type,
                success: liq_success,
            },
        );
    });

    // Step 3: If any liquidation calls succeeded, process gains
    if !actual_consumed.is_empty() && total_collateral_gained > 0 {
        // Deduct the collateral ledger's transfer fee from gains — the backend reports
        // gross collateral but the transfer to the SP deducts one fee.
        let collateral_fee: u64 = match call::<(), (candid::Nat,)>(
            vault_info.collateral_type, "icrc1_fee", ()
        ).await {
            Ok((fee_nat,)) => {
                let fee: u128 = fee_nat.0.try_into().unwrap_or(0);
                fee as u64
            },
            Err(e) => {
                // SP-104 (audit 2026-06-05): do NOT fall back to fee=0. The actual
                // payout transfer deducts the real ledger fee, so crediting the full
                // gross over-credits depositors and leaves the pool short by one fee.
                // Use a conservative fallback so we under- rather than over-credit
                // (solvency-safe); the next successful interaction reconciles.
                log!(INFO, "icrc1_fee query failed for collateral {}: {:?}; using conservative fallback {} e8s",
                    vault_info.collateral_type, e, FALLBACK_COLLATERAL_FEE_E8S);
                FALLBACK_COLLATERAL_FEE_E8S
            },
        };
        let net_collateral = total_collateral_gained.saturating_sub(collateral_fee);

        mutate_state(|s| {
            s.process_liquidation_gains(
                vault_info.vault_id,
                vault_info.collateral_type,
                &actual_consumed,
                net_collateral,
                vault_info.collateral_price_e8s,
            );
        });

        LiquidationResult {
            vault_id: vault_info.vault_id,
            stables_consumed: actual_consumed,
            collateral_gained: net_collateral,
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
fn determine_stable_token_type(
    ledger: Principal,
    configs: &BTreeMap<Principal, StablecoinConfig>,
) -> Option<rumi_protocol_backend::StableTokenType> {
    let config = configs.get(&ledger)?;
    match config.symbol.as_str() {
        "ckUSDT" => Some(rumi_protocol_backend::StableTokenType::CKUSDT),
        "ckUSDC" => Some(rumi_protocol_backend::StableTokenType::CKUSDC),
        _ => None,
    }
}

/// Backend result type for debt-already-burned liquidations.
#[derive(candid::CandidType, candid::Deserialize, Debug)]
struct StabilityPoolLiquidationResult {
    pub success: bool,
    pub vault_id: u64,
    pub liquidated_debt: u64,
    pub collateral_received: u64,
    pub collateral_type: String,
    pub block_index: u64,
    pub fee: u64,
    pub collateral_price_e8s: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use candid::Nat;
    use icrc_ledger_types::icrc1::transfer::Memo;

    fn principal(byte: u8) -> Principal {
        Principal::from_slice(&[byte])
    }

    #[test]
    fn chain_writedown_memo_matches_backend_liq_004_shape() {
        let vault_id: u64 = 0x0102_0304_0506_0708;
        let memo = encode_chain_writedown_memo(vault_id);

        assert_eq!(&memo[..13], b"RUMI-LIQ-004:");
        assert_eq!(&memo[13..], &vault_id.to_be_bytes());
        assert_eq!(
            rumi_protocol_backend::icrc3_proof::decode_writedown_memo(&memo),
            Ok(vault_id),
            "SP burn memo must be accepted by backend proof verifier",
        );
    }

    #[test]
    fn icusd_burn_request_targets_minting_account_and_builds_proof() {
        let minting_account = Account { owner: principal(90), subaccount: None };
        let amount_e8s = 12_345_00000000;
        let vault_id = 77;
        let created_at_time = 123_456_789;
        let block_index = 999;

        let transfer = build_icusd_burn_transfer_arg(
            minting_account,
            amount_e8s,
            vault_id,
            created_at_time,
        );

        assert_eq!(transfer.to, minting_account);
        assert_eq!(transfer.amount, Nat::from(amount_e8s));
        assert_eq!(transfer.fee, None, "ICRC-1 burns to the minting account have zero fee");
        assert_eq!(transfer.from_subaccount, None);
        assert_eq!(transfer.created_at_time, Some(created_at_time));
        assert_eq!(
            transfer.memo,
            Some(Memo::from(encode_chain_writedown_memo(vault_id))),
        );

        let proof = build_icusd_burn_proof(block_index, vault_id);
        assert_eq!(proof.block_index, block_index);
        assert_eq!(proof.ledger_kind, rumi_protocol_backend::icrc3_proof::SpProofLedger::IcusdBurn);
        assert_eq!(proof.vault_id_memo, vault_id);
    }
}
