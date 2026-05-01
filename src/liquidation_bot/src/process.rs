use candid::{CandidType, Deserialize};
use ic_canister_log::log;

use crate::history::{self, LiquidationRecordV1, LiquidationRecordVersioned, LiquidationStatus};
use crate::state::{self, BotConfig};
use crate::swap;

const CONFIRM_ATTEMPTS: u8 = 5;
const CANCEL_ATTEMPTS: u8 = 3;

/// Outcome of the swap-failure cleanup path. Pure data, produced by
/// `decide_swap_failure_outcome` and consumed by `process_pending` to write
/// the LiquidationRecord and emit the STUCK log line if applicable.
#[derive(Debug, PartialEq)]
pub(crate) struct SwapFailureOutcome {
    pub status: history::LiquidationStatus,
    pub error_message: String,
    /// Some(line) when the bot is leaving a claim active that the protocol
    /// will reconcile (Wave-11 auto-cancel after 10 min, or admin via
    /// `admin_resolve_stuck_claim`). None when both cleanup steps succeeded.
    pub stuck_log: Option<String>,
}

/// Wave 13 (BOT-002): decide which `LiquidationStatus` and `error_message`
/// to record after a swap failure, given the outcomes of the return-collateral
/// transfer and the cancel-claim retry loop.
///
/// Before Wave 13 the bot ignored both call results with `let _ = ...` and
/// always wrote `SwapFailed`, even when the protocol's claim was still active
/// (budget unrestored, vault still flagged). With the Wave-12 BOT-001b balance
/// gate a failed return guarantees the cancel rejects, so the bot record must
/// reflect that the claim is stuck pending Wave-11 auto-cancel.
///
/// Status mapping reuses existing variants (no `.did` change):
///   * Both cleanup steps OK     -> SwapFailed (happy cleanup)
///   * Return failed             -> TransferFailed (bot couldn't return ICP)
///   * Return OK, cancel stuck   -> ConfirmFailed (protocol-side bookkeeping stuck)
///
/// `return_err` takes priority over `cancel_err` defensively: when the return
/// fails the integration never attempts cancel, so cancel_err should be None,
/// but the helper still picks deterministically if both are somehow set.
pub(crate) fn decide_swap_failure_outcome(
    vault_id: u64,
    swap_err: &str,
    return_err: Option<&str>,
    cancel_err: Option<(u8, &str)>,
) -> SwapFailureOutcome {
    if let Some(rerr) = return_err {
        return SwapFailureOutcome {
            status: history::LiquidationStatus::TransferFailed,
            error_message: format!("swap: {} | return: {}", swap_err, rerr),
            stuck_log: Some(format!(
                "STUCK: ICP return failed after swap failure for vault #{}; claim still active. Wave-11 auto-cancel will fire in 10 min, or admin can run admin_resolve_stuck_claim.",
                vault_id
            )),
        };
    }

    if let Some((attempts, cerr)) = cancel_err {
        return SwapFailureOutcome {
            status: history::LiquidationStatus::ConfirmFailed,
            error_message: format!(
                "swap: {} | cancel after {} retries: {}",
                swap_err, attempts, cerr
            ),
            stuck_log: Some(format!(
                "STUCK: cancel failed after {} attempts for vault #{}; ICP returned but claim still active. Wave-11 auto-cancel will fire in 10 min.",
                attempts, vault_id
            )),
        };
    }

    SwapFailureOutcome {
        status: history::LiquidationStatus::SwapFailed,
        error_message: swap_err.to_string(),
        stuck_log: None,
    }
}

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
        Err(swap_err) => {
            log!(crate::INFO, "Swap failed for vault #{}: {}. Returning ICP.", vault.vault_id, swap_err);

            // Step 1: return seized ICP to the backend.
            let return_result = swap::return_collateral_to_backend(
                &config,
                collateral_amount,
                config.icp_ledger,
            )
            .await;

            // Step 2: cancel the protocol-side claim, only if the return succeeded.
            // The Wave-12 BOT-001b balance gate rejects cancel until the protocol's
            // collateral balance is back to (>=) `claim.collateral_amount - fee`,
            // so attempting cancel after a failed return is pointless and would
            // just produce noisy `[BOT-001b] cancel rejected` log lines.
            let cancel_err = if return_result.is_ok() {
                let mut last_err = String::new();
                let mut succeeded = false;
                let mut attempts: u8 = 0;
                for attempt in 0..CANCEL_ATTEMPTS {
                    attempts = attempt + 1;
                    match call_bot_cancel_liquidation(&config, vault.vault_id).await {
                        Ok(()) => {
                            succeeded = true;
                            break;
                        }
                        Err(e) => {
                            last_err = e;
                            if attempt + 1 < CANCEL_ATTEMPTS {
                                log!(
                                    crate::INFO,
                                    "Cancel attempt {}/{} failed for vault #{}: {}. Retrying.",
                                    attempt + 1,
                                    CANCEL_ATTEMPTS,
                                    vault.vault_id,
                                    last_err
                                );
                            }
                        }
                    }
                }
                if succeeded { None } else { Some((attempts, last_err)) }
            } else {
                None
            };

            let outcome = decide_swap_failure_outcome(
                vault.vault_id,
                &swap_err,
                return_result.as_ref().err().map(|s| s.as_str()),
                cancel_err.as_ref().map(|(n, e)| (*n, e.as_str())),
            );

            if let Some(line) = &outcome.stuck_log {
                log!(crate::INFO, "{}", line);
            }

            write_record(LiquidationRecordV1 {
                id: record_id, vault_id: vault.vault_id, timestamp,
                status: outcome.status,
                collateral_claimed_e8s: collateral_amount, debt_to_cover_e8s: debt_covered,
                icp_swapped_e8s: swap_amount, ckusdc_received_e6: 0, ckusdc_transferred_e6: 0,
                icp_to_treasury_e8s: 0, oracle_price_e8s: collateral_price,
                effective_price_e8s: 0, slippage_bps: 0,
                error_message: Some(outcome.error_message), confirm_retry_count: 0,
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

#[cfg(test)]
mod tests {
    use super::*;

    const SWAP_ERR: &str = "Quote returned zero output";
    const RETURN_ERR: &str = "Transfer error: BadFee";
    const CANCEL_ERR: &str = "GenericError(\"Cannot cancel claim for vault #7: protocol collateral balance 0 < required 99990000\")";

    #[test]
    fn swap_failure_clean_cleanup_records_swap_failed() {
        let outcome = decide_swap_failure_outcome(7, SWAP_ERR, None, None);
        assert_eq!(outcome.status, LiquidationStatus::SwapFailed);
        assert_eq!(outcome.error_message, SWAP_ERR);
        assert!(outcome.stuck_log.is_none(), "happy cleanup must not log STUCK");
    }

    #[test]
    fn swap_failure_with_failed_return_records_transfer_failed_and_logs_stuck() {
        let outcome = decide_swap_failure_outcome(7, SWAP_ERR, Some(RETURN_ERR), None);
        assert_eq!(outcome.status, LiquidationStatus::TransferFailed);
        assert!(outcome.error_message.contains("swap: "));
        assert!(outcome.error_message.contains(SWAP_ERR));
        assert!(outcome.error_message.contains("return: "));
        assert!(outcome.error_message.contains(RETURN_ERR));
        let log = outcome.stuck_log.expect("must surface STUCK log");
        assert!(log.contains("STUCK"));
        assert!(log.contains("vault #7"));
        assert!(log.contains("ICP return failed"));
    }

    #[test]
    fn swap_failure_with_stuck_cancel_records_confirm_failed_and_logs_stuck() {
        let outcome = decide_swap_failure_outcome(7, SWAP_ERR, None, Some((3, CANCEL_ERR)));
        assert_eq!(outcome.status, LiquidationStatus::ConfirmFailed);
        assert!(outcome.error_message.contains("swap: "));
        assert!(outcome.error_message.contains(SWAP_ERR));
        assert!(outcome.error_message.contains("cancel after 3 retries: "));
        assert!(outcome.error_message.contains(CANCEL_ERR));
        let log = outcome.stuck_log.expect("must surface STUCK log");
        assert!(log.contains("STUCK"));
        assert!(log.contains("vault #7"));
        assert!(log.contains("3 attempts"));
    }

    #[test]
    fn return_error_takes_priority_over_cancel_error() {
        // Defensive: integration code should never hand both, but the helper
        // must still pick deterministically. Return failure dominates because
        // the cancel never actually happened in that branch.
        let outcome = decide_swap_failure_outcome(
            42,
            SWAP_ERR,
            Some(RETURN_ERR),
            Some((3, CANCEL_ERR)),
        );
        assert_eq!(outcome.status, LiquidationStatus::TransferFailed);
        assert!(outcome.error_message.contains("return: "));
        assert!(!outcome.error_message.contains("cancel after"));
    }
}
