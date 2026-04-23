use candid::{CandidType, Deserialize};
use ic_canister_log::log;

use crate::history::{self, LiquidationRecordV1, LiquidationRecordVersioned, LiquidationStatus};
use crate::state::{self, BotConfig};
use crate::swap;

const CONFIRM_ATTEMPTS: u8 = 5;

/// Result returned by the backend's `bot_claim_liquidation` endpoint.
#[derive(CandidType, Deserialize, Debug)]
pub struct BotLiquidationResult {
    pub vault_id: u64,
    pub collateral_amount: u64,
    pub debt_covered: u64,
    pub collateral_price_e8s: u64,
}

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
    let _guard = match crate::ProcessingGuard::acquire() {
        Ok(g) => g,
        Err(_) => return, // Another liquidation is already in flight
    };

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
    let record_id = history::next_id();
    let timestamp = ic_cdk::api::time();

    // -- Phase 1: CLAIM --
    let liq_result = call_bot_claim_liquidation(&config, vault.vault_id).await;
    let (collateral_amount, debt_covered, collateral_price) = match liq_result {
        Ok(r) => (r.collateral_amount, r.debt_covered, r.collateral_price_e8s),
        Err(e) => {
            log!(crate::INFO, "Claim failed for vault #{}: {}", vault.vault_id, e);
            write_record(LiquidationRecordV1 {
                id: record_id, vault_id: vault.vault_id, timestamp,
                status: LiquidationStatus::ClaimFailed,
                collateral_claimed_e8s: 0, debt_to_cover_e8s: 0, icp_swapped_e8s: 0,
                ckusdc_received_e6: 0, ckusdc_transferred_e6: 0, icp_to_treasury_e8s: 0,
                oracle_price_e8s: 0, effective_price_e8s: 0, slippage_bps: 0,
                error_message: Some(e), confirm_retry_count: 0,
            });
            return;
        }
    };

    // -- Phase 2: SWAP ICP -> ckUSDC --
    let swap_amount = calculate_swap_amount(collateral_amount, debt_covered, collateral_price);
    let swap_result = swap::swap_icp_for_ckusdc(&config, swap_amount).await;

    let (ckusdc_received, effective_price) = match swap_result {
        Ok(r) => (r.ckusdc_received_e6, r.effective_price_e8s),
        Err(e) => {
            log!(crate::INFO, "Swap failed for vault #{}: {}. Returning ICP.", vault.vault_id, e);
            let _ = swap::return_collateral_to_backend(&config, collateral_amount, config.icp_ledger).await;
            let _ = call_bot_cancel_liquidation(&config, vault.vault_id).await;
            write_record(LiquidationRecordV1 {
                id: record_id, vault_id: vault.vault_id, timestamp,
                status: LiquidationStatus::SwapFailed,
                collateral_claimed_e8s: collateral_amount, debt_to_cover_e8s: debt_covered,
                icp_swapped_e8s: swap_amount, ckusdc_received_e6: 0, ckusdc_transferred_e6: 0,
                icp_to_treasury_e8s: 0, oracle_price_e8s: collateral_price,
                effective_price_e8s: 0, slippage_bps: 0,
                error_message: Some(e), confirm_retry_count: 0,
            });
            return;
        }
    };

    let slippage_bps = calculate_slippage(effective_price, collateral_price);

    // -- Phase 3: TRANSFER ckUSDC to backend (NO RETRY) --
    let transfer_result = swap::transfer_ckusdc_to_backend(&config, ckusdc_received).await;

    let ckusdc_transferred = match transfer_result {
        Ok(actual_sent) => actual_sent,
        Err(e) => {
            log!(crate::INFO,
                "STUCK: ckUSDC transfer failed for vault #{}. Bot holding {} ckUSDC e6. Error: {}. Needs admin resolution.",
                vault.vault_id, ckusdc_received, e);
            write_record(LiquidationRecordV1 {
                id: record_id, vault_id: vault.vault_id, timestamp,
                status: LiquidationStatus::TransferFailed,
                collateral_claimed_e8s: collateral_amount, debt_to_cover_e8s: debt_covered,
                icp_swapped_e8s: swap_amount, ckusdc_received_e6: ckusdc_received,
                ckusdc_transferred_e6: 0, icp_to_treasury_e8s: 0,
                oracle_price_e8s: collateral_price, effective_price_e8s: effective_price,
                slippage_bps, error_message: Some(e), confirm_retry_count: 0,
            });
            return;
        }
    };

    // -- Phase 4: CONFIRM (with retry, idempotent) --
    let mut confirm_ok = false;
    let mut confirm_retries: u8 = 0;
    let mut last_confirm_err = String::new();

    for attempt in 0..CONFIRM_ATTEMPTS {
        match call_bot_confirm_liquidation(&config, vault.vault_id).await {
            Ok(()) => {
                confirm_ok = true;
                confirm_retries = attempt + 1;
                break;
            }
            Err(e) => {
                last_confirm_err = e;
                confirm_retries = attempt + 1;
                if attempt + 1 < CONFIRM_ATTEMPTS {
                    log!(crate::INFO, "Confirm attempt {}/{} failed for vault #{}: {}. Retrying.",
                        attempt + 1, CONFIRM_ATTEMPTS, vault.vault_id, last_confirm_err);
                }
            }
        }
    }

    if !confirm_ok {
        log!(crate::INFO,
            "STUCK: Confirm failed after {} attempts for vault #{}. ckUSDC is in backend but debt not written down. Error: {}. Needs admin resolution.",
            CONFIRM_ATTEMPTS, vault.vault_id, last_confirm_err);
        write_record(LiquidationRecordV1 {
            id: record_id, vault_id: vault.vault_id, timestamp,
            status: LiquidationStatus::ConfirmFailed,
            collateral_claimed_e8s: collateral_amount, debt_to_cover_e8s: debt_covered,
            icp_swapped_e8s: swap_amount, ckusdc_received_e6: ckusdc_received,
            ckusdc_transferred_e6: ckusdc_transferred, icp_to_treasury_e8s: 0,
            oracle_price_e8s: collateral_price, effective_price_e8s: effective_price,
            slippage_bps, error_message: Some(last_confirm_err), confirm_retry_count: confirm_retries,
        });
        return;
    }

    // -- Phase 5: TREASURY (liquidation bonus) --
    let icp_to_treasury = collateral_amount.saturating_sub(swap_amount);
    if icp_to_treasury > 0 {
        let _ = swap::transfer_icp_to_treasury(&config, icp_to_treasury).await;
    }

    // -- Phase 6: SUCCESS --
    log!(crate::INFO, "Vault #{} liquidated: debt={} e8s, ckUSDC={} e6, treasury={} e8s ICP",
        vault.vault_id, debt_covered, ckusdc_received, icp_to_treasury);

    write_record(LiquidationRecordV1 {
        id: record_id, vault_id: vault.vault_id, timestamp,
        status: LiquidationStatus::Completed,
        collateral_claimed_e8s: collateral_amount, debt_to_cover_e8s: debt_covered,
        icp_swapped_e8s: swap_amount, ckusdc_received_e6: ckusdc_received,
        ckusdc_transferred_e6: ckusdc_transferred, icp_to_treasury_e8s: icp_to_treasury,
        oracle_price_e8s: collateral_price, effective_price_e8s: effective_price,
        slippage_bps, error_message: None, confirm_retry_count: confirm_retries,
    });

    // Update legacy stats for backward compat with explorer UI
    state::mutate_state(|s| {
        s.stats.total_debt_covered_e8s += debt_covered;
        s.stats.total_collateral_received_e8s += collateral_amount;
        s.stats.total_collateral_to_treasury_e8s += icp_to_treasury;
        s.stats.events_count += 1;
    });
}

// -- Helpers --

fn write_record(record: LiquidationRecordV1) {
    history::insert_record(LiquidationRecordVersioned::V1(record));
}

async fn call_bot_claim_liquidation(
    config: &BotConfig,
    vault_id: u64,
) -> Result<BotLiquidationResult, String> {
    let result: Result<(BackendResult<BotLiquidationResult>,), _> =
        ic_cdk::call(config.backend_principal, "bot_claim_liquidation", (vault_id,)).await;

    match result {
        Ok((BackendResult::Ok(r),)) => Ok(r),
        Ok((BackendResult::Err(e),)) => Err(format!("{}", e)),
        Err((code, msg)) => Err(format!("{:?}: {}", code, msg)),
    }
}

pub async fn call_bot_confirm_liquidation(
    config: &BotConfig,
    vault_id: u64,
) -> Result<(), String> {
    let result: Result<(BackendResult<()>,), _> =
        ic_cdk::call(config.backend_principal, "bot_confirm_liquidation", (vault_id,)).await;

    match result {
        Ok((BackendResult::Ok(()),)) => Ok(()),
        Ok((BackendResult::Err(e),)) => Err(format!("{}", e)),
        Err((code, msg)) => Err(format!("{:?}: {}", code, msg)),
    }
}

async fn call_bot_cancel_liquidation(
    config: &BotConfig,
    vault_id: u64,
) -> Result<(), String> {
    let result: Result<(BackendResult<()>,), _> =
        ic_cdk::call(config.backend_principal, "bot_cancel_liquidation", (vault_id,)).await;

    match result {
        Ok((BackendResult::Ok(()),)) => Ok(()),
        Ok((BackendResult::Err(e),)) => Err(format!("{}", e)),
        Err((code, msg)) => Err(format!("{:?}: {}", code, msg)),
    }
}

pub fn calculate_swap_amount(collateral_e8s: u64, debt_e8s: u64, collateral_price_e8s: u64) -> u64 {
    if collateral_price_e8s == 0 {
        return collateral_e8s;
    }
    let icp_needed = (debt_e8s as u128 * 100_000_000 / collateral_price_e8s as u128) as u64;
    let with_buffer = icp_needed.saturating_mul(105) / 100;
    with_buffer.min(collateral_e8s)
}

fn calculate_slippage(effective_price_e8s: u64, oracle_price_e8s: u64) -> i32 {
    if oracle_price_e8s == 0 || effective_price_e8s == 0 {
        return 0;
    }
    let diff = oracle_price_e8s as i64 - effective_price_e8s as i64;
    (diff * 10_000 / oracle_price_e8s as i64) as i32
}
