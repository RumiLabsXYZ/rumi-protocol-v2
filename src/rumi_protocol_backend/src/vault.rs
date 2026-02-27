use crate::event::{
    record_add_margin_to_vault, record_borrow_from_vault, record_open_vault,
    record_redemption_on_vaults, record_repayed_to_vault,
};
use ic_cdk::update;
use crate::guard::GuardPrincipal;
use crate::GuardError;
use crate::logs::INFO;
use crate::management::{mint_icusd, transfer_collateral_from, transfer_icusd_from, transfer_stable_from};
use crate::numeric::{ICUSD, ICP, UsdIcp};
use crate::{
    mutate_state, read_state, ProtocolError, SuccessWithFee, MIN_ICP_AMOUNT, MIN_ICUSD_AMOUNT,
    DUST_THRESHOLD,
    StableTokenType, VaultArgWithToken,
};
use candid::{CandidType, Deserialize, Principal};
use ic_canister_log::log;
use icrc_ledger_types::icrc2::transfer_from::TransferFromError;
use serde::Serialize;
use crate::DEBUG;
use crate::management;
use crate::PendingMarginTransfer;
use rust_decimal::prelude::ToPrimitive;

use crate::compute_collateral_ratio;



#[derive(CandidType, Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct OpenVaultSuccess {
    pub vault_id: u64,
    pub block_index: u64,
}

#[derive(CandidType, Deserialize)]
pub struct VaultArg {
    pub vault_id: u64,
    pub amount: u64,
}

/// Returns `Principal::anonymous()` as sentinel for old events missing `collateral_type`.
/// The replay handler replaces this with the actual ICP ledger principal.
pub(crate) fn default_collateral_type() -> Principal {
    Principal::anonymous()
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Deserialize, Serialize, PartialOrd, Ord)]
pub struct Vault {
    pub owner: Principal,
    pub borrowed_icusd_amount: ICUSD,
    /// Raw collateral amount in token's native precision (e.g., e8s for ICP).
    /// Renamed from `icp_margin_amount`; serde alias handles old events.
    #[serde(alias = "icp_margin_amount")]
    pub collateral_amount: u64,
    pub vault_id: u64,
    /// Ledger canister ID identifying the collateral token.
    /// Old events lack this field; serde default → Principal::anonymous(),
    /// fixed up to ICP ledger principal during event replay.
    #[serde(default = "default_collateral_type")]
    pub collateral_type: Principal,
}

#[derive(CandidType, Serialize, Deserialize, Debug)]
pub struct CandidVault {
    pub owner: Principal,
    pub borrowed_icusd_amount: u64,
    /// Kept for frontend backward compatibility
    pub icp_margin_amount: u64,
    pub vault_id: u64,
    /// Raw collateral amount (same value as icp_margin_amount for ICP vaults)
    pub collateral_amount: u64,
    /// Ledger canister ID of the collateral token
    pub collateral_type: Principal,
}

impl From<Vault> for CandidVault {
    fn from(vault: Vault) -> Self {
        Self {
            owner: vault.owner,
            borrowed_icusd_amount: vault.borrowed_icusd_amount.to_u64(),
            icp_margin_amount: vault.collateral_amount,
            vault_id: vault.vault_id,
            collateral_amount: vault.collateral_amount,
            collateral_type: vault.collateral_type,
        }
    }
}

/// Redeem icUSD for ckStable tokens from the protocol's reserves.
/// Two-tier system: reserves first (flat fee), then vault spillover (dynamic fee).
pub async fn redeem_reserves(icusd_amount_raw: u64, preferred_token: Option<Principal>) -> Result<crate::ReserveRedemptionResult, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let _guard_principal = GuardPrincipal::new(caller, "redeem_reserves")?;

    let icusd_amount: ICUSD = icusd_amount_raw.into();

    if icusd_amount < MIN_ICUSD_AMOUNT {
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: MIN_ICUSD_AMOUNT.to_u64(),
        });
    }

    // Check reserve redemptions are enabled
    let (enabled, reserve_fee_ratio, ckusdt_ledger, ckusdc_ledger, treasury) = read_state(|s| (
        s.reserve_redemptions_enabled,
        s.reserve_redemption_fee,
        s.ckusdt_ledger_principal,
        s.ckusdc_ledger_principal,
        s.treasury_principal,
    ));

    if !enabled {
        return Err(ProtocolError::GenericError(
            "Reserve redemptions are currently disabled.".to_string(),
        ));
    }

    // Determine which ledger to use
    let stable_ledger = if let Some(pref) = preferred_token {
        // Validate it's one of our known stable ledgers
        if Some(pref) == ckusdt_ledger || Some(pref) == ckusdc_ledger {
            pref
        } else {
            return Err(ProtocolError::GenericError(
                "Preferred token is not a supported reserve token.".to_string(),
            ));
        }
    } else {
        // Default: try ckUSDT first, then ckUSDC
        ckusdt_ledger
            .or(ckusdc_ledger)
            .ok_or_else(|| ProtocolError::GenericError("No reserve token ledgers configured.".to_string()))?
    };

    // Calculate fee (flat rate)
    let fee_icusd = icusd_amount * reserve_fee_ratio;
    let net_icusd = icusd_amount - fee_icusd;

    // Convert e8s (icUSD) to e6s (ckStable): divide by 100
    let net_e6s = net_icusd.to_u64() / 100;
    let fee_e6s = fee_icusd.to_u64() / 100;

    if net_e6s == 0 {
        return Err(ProtocolError::GenericError(
            "Redemption amount too small after fee.".to_string(),
        ));
    }

    // Check reserve balance before pulling icUSD
    let reserve_balance = management::get_token_balance(stable_ledger).await
        .map_err(|e| ProtocolError::TemporarilyUnavailable(format!("Cannot query reserve balance: {}", e)))?;

    // Determine how much can come from reserves vs vault spillover.
    // Each ICRC-1 transfer also costs a ledger fee (deducted from sender balance).
    // Query the actual fee from the ledger rather than hardcoding.
    let ledger_fee = management::get_ledger_fee(stable_ledger).await
        .unwrap_or(10_000); // fallback to 10_000 e6s (0.01 USD) if query fails
    let fee_budget = if fee_e6s > 0 { ledger_fee * 2 } else { ledger_fee };
    let total_needed_e6s = net_e6s + fee_e6s + fee_budget;
    let available_for_user = if reserve_balance >= total_needed_e6s {
        net_e6s
    } else if reserve_balance > fee_e6s + fee_budget {
        // Partial: reserve can cover some but not all
        reserve_balance - fee_e6s - fee_budget
    } else {
        0
    };

    let spillover_e6s = net_e6s - available_for_user;
    let spillover_e8s = spillover_e6s * 100; // convert back to icUSD e8s

    // Pull icUSD from caller (effectively burns it)
    let icusd_block_index = transfer_icusd_from(icusd_amount, caller).await
        .map_err(|e| ProtocolError::TransferFromError(e, icusd_amount.to_u64()))?;

    // Transfer ckStable to user from reserves.
    // CRITICAL: If this fails we MUST refund the icUSD — otherwise the user
    // loses funds with nothing received. ICP inter-canister calls are NOT
    // atomic, so we implement the saga/compensation pattern manually.
    if available_for_user > 0 {
        if let Err(transfer_err) = management::transfer_collateral(available_for_user, caller, stable_ledger).await {
            log!(crate::INFO,
                "[redeem_reserves] ckStable transfer failed for {}: {:?}. Refunding {} icUSD.",
                caller, transfer_err, icusd_amount.to_u64()
            );
            // Attempt to refund icUSD (minus ledger fee which is deducted by the ledger)
            match management::transfer_icusd(icusd_amount, caller).await {
                Ok(refund_block) => {
                    log!(crate::INFO,
                        "[redeem_reserves] Refunded {} icUSD to {} (block {})",
                        icusd_amount.to_u64(), caller, refund_block
                    );
                }
                Err(refund_err) => {
                    // Both the ckStable send AND the refund failed.
                    // Log a critical error — admin must manually resolve.
                    log!(crate::INFO,
                        "[redeem_reserves] CRITICAL: ckStable transfer failed AND icUSD refund failed for {}! \
                         Amount: {} icUSD. ckStable error: {:?}. Refund error: {:?}. \
                         Manual intervention required.",
                        caller, icusd_amount.to_u64(), transfer_err, refund_err
                    );
                }
            }
            return Err(ProtocolError::GenericError(
                format!("Reserve transfer failed — your icUSD has been refunded. Error: {:?}", transfer_err),
            ));
        }
    }

    // Transfer fee to treasury (if configured), otherwise fee stays in reserves
    if fee_e6s > 0 {
        if let Some(treasury_principal) = treasury {
            let _ = management::transfer_collateral(fee_e6s, treasury_principal, stable_ledger).await;
            // If treasury transfer fails, fee stays in reserves — not critical
        }
    }

    // Record the reserve redemption event
    crate::event::record_reserve_redemption(
        caller,
        icusd_amount,
        fee_icusd,
        stable_ledger,
        available_for_user,
        fee_e6s,
        icusd_block_index,
    );

    // Handle vault spillover if reserves didn't cover everything
    if spillover_e8s > 0 {
        // Use ICP vault redemption for the remainder with dynamic fee
        let icp_ledger = read_state(|s| s.icp_collateral_type());
        let collateral_price = read_state(|s| s.get_collateral_price_decimal(&icp_ledger))
            .ok_or(ProtocolError::TemporarilyUnavailable("No ICP price for vault spillover".to_string()))?;
        let current_price = UsdIcp::from(collateral_price);

        mutate_state(|s| {
            let spillover_icusd = ICUSD::from(spillover_e8s);
            let base_fee = s.get_redemption_fee(spillover_icusd);
            s.current_base_rate = base_fee;
            s.last_redemption_time = ic_cdk::api::time();
            let vault_fee = spillover_icusd * base_fee;

            record_redemption_on_vaults(
                s,
                caller,
                spillover_icusd - vault_fee,
                vault_fee,
                current_price,
                icusd_block_index,
            );
        });
        ic_cdk_timers::set_timer(std::time::Duration::from_secs(0), || {
            ic_cdk::spawn(crate::process_pending_transfer())
        });
    }

    log!(INFO, "[redeem_reserves] {} redeemed {} icUSD: {} e6s from reserves, {} e8s vault spillover, fee {} e6s",
        caller, icusd_amount.to_u64(), available_for_user, spillover_e8s, fee_e6s);

    Ok(crate::ReserveRedemptionResult {
        icusd_block_index,
        stable_amount_sent: available_for_user,
        fee_amount: fee_icusd.to_u64(),
        stable_token_used: stable_ledger,
        vault_spillover_amount: spillover_e8s,
    })
}

/// Thin wrapper for backward compatibility. Calls `redeem_collateral` with ICP.
pub async fn redeem_icp(icusd_amount: u64) -> Result<SuccessWithFee, ProtocolError> {
    let icp_ledger = read_state(|s| s.icp_collateral_type());
    redeem_collateral(icp_ledger, icusd_amount).await
}

/// Generic collateral redemption: burn icUSD and receive collateral tokens.
/// Currently the redemption logic (vault sorting, pending transfers) is ICP-centric,
/// but the API surface supports any collateral type. The internal logic will be
/// generalized per-collateral when a second collateral type is actually added.
pub async fn redeem_collateral(collateral_type: Principal, _icusd_amount: u64) -> Result<SuccessWithFee, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let _guard_principal = GuardPrincipal::new(caller, "redeem_collateral")?;

    let icusd_amount: ICUSD = _icusd_amount.into();

    if icusd_amount < MIN_ICUSD_AMOUNT {
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: MIN_ICUSD_AMOUNT.to_u64(),
        });
    }

    // Check collateral status allows redemption
    let collateral_status = read_state(|s| s.get_collateral_status(&collateral_type));
    if let Some(status) = collateral_status {
        if !status.allows_redemption() {
            return Err(ProtocolError::GenericError(
                format!("Redemption is not allowed for collateral type {}.", collateral_type),
            ));
        }
    } else {
        return Err(ProtocolError::GenericError(
            format!("Collateral type {} not found.", collateral_type),
        ));
    }

    let collateral_price = read_state(|s| s.get_collateral_price_decimal(&collateral_type))
        .ok_or(ProtocolError::TemporarilyUnavailable("No price available for collateral".to_string()))?;
    let current_collateral_price = UsdIcp::from(collateral_price);

    match transfer_icusd_from(icusd_amount, caller).await {
        Ok(block_index) => {
            let fee_amount = mutate_state(|s| {
                let base_fee = s.get_redemption_fee(icusd_amount);
                s.current_base_rate = base_fee;
                s.last_redemption_time = ic_cdk::api::time();
                let fee_amount = icusd_amount * base_fee;

                record_redemption_on_vaults(
                    s,
                    caller,
                    icusd_amount - fee_amount,
                    fee_amount,
                    current_collateral_price,
                    block_index,
                );
                fee_amount
            });
            ic_cdk_timers::set_timer(std::time::Duration::from_secs(0), || {
                ic_cdk::spawn(crate::process_pending_transfer())
            });
            Ok(SuccessWithFee {
                block_index,
                fee_amount_paid: fee_amount.to_u64(),
            })
        }
        Err(transfer_from_error) => Err(ProtocolError::TransferFromError(
            transfer_from_error,
            icusd_amount.to_u64(),
        )),
    }
}

pub async fn open_vault(collateral_amount_raw: u64, collateral_type_opt: Option<Principal>) -> Result<OpenVaultSuccess, ProtocolError> {
    let caller = ic_cdk::api::caller();
    // Pass operation name to guard for better tracking
    let guard_principal = match GuardPrincipal::new(caller, "open_vault") {
        Ok(guard) => guard,
        Err(GuardError::AlreadyProcessing) => {
            log!(INFO, "[open_vault] Principal {:?} already has an ongoing operation", caller);
            return Err(ProtocolError::AlreadyProcessing);
        },
        Err(GuardError::StaleOperation) => {
            log!(INFO, "[open_vault] Principal {:?} has a stale operation that's being cleaned up", caller);
            return Err(ProtocolError::TemporarilyUnavailable(
                "Previous operation is being cleaned up. Please try again in a few seconds.".to_string()
            ));
        },
        Err(err) => return Err(err.into()),
    };

    // Resolve collateral type: default to ICP if not specified
    let collateral_type = collateral_type_opt.unwrap_or_else(|| read_state(|s| s.icp_collateral_type()));

    // Look up CollateralConfig; check status is Active
    let (config_ledger, config_status) = read_state(|s| {
        match s.get_collateral_config(&collateral_type) {
            Some(config) => Ok((config.ledger_canister_id, config.status)),
            None => Err(ProtocolError::GenericError("Collateral type not supported.".to_string())),
        }
    })?;

    if !config_status.allows_open() {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Collateral type is not accepting new vaults.".to_string(),
        ));
    }

    let icp_margin_amount: ICP = collateral_amount_raw.into();

    if icp_margin_amount < MIN_ICP_AMOUNT {
        // Mark operation as failed since it didn't meet requirements
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: MIN_ICP_AMOUNT.to_u64(),
        });
    }

    match transfer_collateral_from(collateral_amount_raw, caller, config_ledger).await {
        Ok(block_index) => {
            let vault_id = mutate_state(|s| {
                let vault_id = s.increment_vault_id();
                record_open_vault(
                    s,
                    Vault {
                        owner: caller,
                        borrowed_icusd_amount: 0.into(),
                        collateral_amount: collateral_amount_raw,
                        vault_id,
                        collateral_type,
                    },
                    block_index,
                );
                vault_id
            });
            log!(INFO, "[open_vault] opened vault with id: {vault_id}");

            // Mark operation as successfully completed
            guard_principal.complete();

            Ok(OpenVaultSuccess {
                vault_id,
                block_index,
            })
        }
        Err(transfer_from_error) => {
            // Explicitly mark as failed when an error occurs
            guard_principal.fail();

            if let TransferFromError::BadFee { expected_fee } = transfer_from_error.clone() {
                mutate_state(|s| {
                    if let Ok(fee) = u64::try_from(expected_fee.0) {
                        if let Some(config) = s.get_collateral_config_mut(&collateral_type) {
                            config.ledger_fee = fee;
                        }
                    }
                });
            };
            Err(ProtocolError::TransferFromError(
                transfer_from_error,
                icp_margin_amount.to_u64(),
            ))
        }
    }
}

/// Compound open-vault-and-borrow in a single canister call.
///
/// Uses ICRC-2 `transfer_from` (like `open_vault`) to pull collateral, creates
/// the vault, then immediately borrows `borrow_amount_raw` icUSD — all under a
/// single guard.  This allows Oisy / ICRC-112 signer wallets to batch
/// `icrc2_approve` + `open_vault_and_borrow` into **one** popup instead of the
/// three sequential popups that separate `open_vault` + `borrow_from_vault`
/// would require.
pub async fn open_vault_and_borrow(
    collateral_amount_raw: u64,
    borrow_amount_raw: u64,
    collateral_type_opt: Option<Principal>,
) -> Result<OpenVaultSuccess, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal = match GuardPrincipal::new(caller, "open_vault_and_borrow") {
        Ok(guard) => guard,
        Err(GuardError::AlreadyProcessing) => {
            log!(INFO, "[open_vault_and_borrow] Principal {:?} already has an ongoing operation", caller);
            return Err(ProtocolError::AlreadyProcessing);
        },
        Err(GuardError::StaleOperation) => {
            log!(INFO, "[open_vault_and_borrow] Principal {:?} has a stale operation being cleaned up", caller);
            return Err(ProtocolError::TemporarilyUnavailable(
                "Previous operation is being cleaned up. Please try again in a few seconds.".to_string()
            ));
        },
        Err(err) => return Err(err.into()),
    };

    // Resolve collateral type: default to ICP if not specified
    let collateral_type = collateral_type_opt.unwrap_or_else(|| read_state(|s| s.icp_collateral_type()));

    // Look up CollateralConfig; check status is Active
    let (config_ledger, config_status) = read_state(|s| {
        match s.get_collateral_config(&collateral_type) {
            Some(config) => Ok((config.ledger_canister_id, config.status)),
            None => Err(ProtocolError::GenericError("Collateral type not supported.".to_string())),
        }
    })?;

    if !config_status.allows_open() {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Collateral type is not accepting new vaults.".to_string(),
        ));
    }

    let icp_margin_amount: ICP = collateral_amount_raw.into();

    if icp_margin_amount < MIN_ICP_AMOUNT {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: MIN_ICP_AMOUNT.to_u64(),
        });
    }

    // Pull collateral via ICRC-2 transfer_from (caller must have approved first)
    let block_index = match transfer_collateral_from(collateral_amount_raw, caller, config_ledger).await {
        Ok(bi) => bi,
        Err(transfer_from_error) => {
            guard_principal.fail();
            if let TransferFromError::BadFee { expected_fee } = transfer_from_error.clone() {
                mutate_state(|s| {
                    if let Ok(fee) = u64::try_from(expected_fee.0) {
                        if let Some(config) = s.get_collateral_config_mut(&collateral_type) {
                            config.ledger_fee = fee;
                        }
                    }
                });
            };
            return Err(ProtocolError::TransferFromError(
                transfer_from_error,
                icp_margin_amount.to_u64(),
            ));
        }
    };

    // Create the vault
    let vault_id = mutate_state(|s| {
        let vault_id = s.increment_vault_id();
        record_open_vault(
            s,
            Vault {
                owner: caller,
                borrowed_icusd_amount: 0.into(),
                collateral_amount: collateral_amount_raw,
                vault_id,
                collateral_type,
            },
            block_index,
        );
        vault_id
    });

    log!(INFO, "[open_vault_and_borrow] opened vault {vault_id}, now borrowing {borrow_amount_raw}");

    // Borrow icUSD — reuse internal fn to avoid guard conflict
    if borrow_amount_raw > 0 {
        match borrow_from_vault_internal(caller, VaultArg { vault_id, amount: borrow_amount_raw }).await {
            Ok(borrow_result) => {
                log!(INFO, "[open_vault_and_borrow] vault {} borrow of {} succeeded (fee: {})",
                    vault_id, borrow_amount_raw, borrow_result.fee_amount_paid);
            },
            Err(e) => {
                guard_principal.fail();
                return Err(ProtocolError::GenericError(format!(
                    "Vault created (id={}) but borrow of {} failed: {:?}. You can borrow separately.",
                    vault_id, borrow_amount_raw, e
                )));
            }
        }
    }

    guard_principal.complete();
    Ok(OpenVaultSuccess { vault_id, block_index })
}

/// Internal borrow logic without guard management.
/// Called by both `borrow_from_vault` (which acquires its own guard) and
/// `open_vault_with_deposit` (which already holds a guard for the same principal).
async fn borrow_from_vault_internal(caller: Principal, arg: VaultArg) -> Result<SuccessWithFee, ProtocolError> {
    let amount: ICUSD = arg.amount.into();

    if amount < MIN_ICUSD_AMOUNT {
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: MIN_ICUSD_AMOUNT.to_u64(),
        });
    }

    let (vault, collateral_price, config_decimals) = read_state(|s| {
        match s.vault_id_to_vaults.get(&arg.vault_id) {
            Some(vault) => {
                let price = s.get_collateral_price_decimal(&vault.collateral_type)
                    .ok_or("No price available for collateral. Price feed may be down.")?;
                let decimals = s.get_collateral_config(&vault.collateral_type)
                    .map(|c| c.decimals)
                    .unwrap_or(8);
                Ok((vault.clone(), price, decimals))
            },
            None => {
                Err("Vault not found. Please check the vault ID.")
            }
        }
    }).map_err(|msg: &str| ProtocolError::GenericError(msg.to_string()))?;

    // Check collateral status allows borrowing
    let collateral_status = read_state(|s| s.get_collateral_status(&vault.collateral_type));
    if let Some(status) = collateral_status {
        if !status.allows_borrow() {
            return Err(ProtocolError::GenericError(
                "Borrowing is not allowed for this collateral type.".to_string(),
            ));
        }
    }

    if caller != vault.owner {
        return Err(ProtocolError::CallerNotOwner);
    }

    // Check debt ceiling
    let current_debt = read_state(|s| s.total_debt_for_collateral(&vault.collateral_type));
    let debt_ceiling = read_state(|s| {
        s.get_collateral_config(&vault.collateral_type)
            .map(|c| c.debt_ceiling)
            .unwrap_or(u64::MAX)
    });
    if current_debt.to_u64() + amount.to_u64() > debt_ceiling {
        return Err(ProtocolError::GenericError(format!(
            "Borrow would exceed debt ceiling ({} + {} > {})",
            current_debt.to_u64(), amount.to_u64(), debt_ceiling
        )));
    }

    let collateral_value = crate::numeric::collateral_usd_value(vault.collateral_amount, collateral_price, config_decimals);
    let min_ratio = read_state(|s| s.get_min_collateral_ratio_for(&vault.collateral_type));
    let max_borrowable_amount: ICUSD = collateral_value / min_ratio;

    if vault.borrowed_icusd_amount + amount > max_borrowable_amount {
        return Err(ProtocolError::GenericError(format!(
            "failed to borrow from vault, max borrowable: {max_borrowable_amount}, borrowed: {}, requested: {amount}",
            vault.borrowed_icusd_amount
        )));
    }

    let fee: ICUSD = read_state(|s| amount * s.get_borrowing_fee_for(&vault.collateral_type));

    match mint_icusd(amount - fee, caller).await {
        Ok(block_index) => {
            mutate_state(|s| {
                record_borrow_from_vault(s, arg.vault_id, amount, fee, block_index);
            });

            Ok(SuccessWithFee {
                block_index,
                fee_amount_paid: fee.to_u64(),
            })
        }
        Err(mint_error) => {
            Err(ProtocolError::TransferError(mint_error))
        }
    }
}

pub async fn borrow_from_vault(arg: VaultArg) -> Result<SuccessWithFee, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal = match GuardPrincipal::new(caller, &format!("borrow_vault_{}", arg.vault_id)) {
        Ok(guard) => guard,
        Err(GuardError::AlreadyProcessing) => {
            log!(INFO, "[borrow_from_vault] Principal {:?} already has an ongoing operation", caller);
            return Err(ProtocolError::AlreadyProcessing);
        },
        Err(err) => return Err(err.into()),
    };

    match borrow_from_vault_internal(caller, arg).await {
        Ok(result) => {
            guard_principal.complete();
            Ok(result)
        }
        Err(e) => {
            guard_principal.fail();
            Err(e)
        }
    }
}

pub async fn repay_to_vault(arg: VaultArg) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal = GuardPrincipal::new(caller, &format!("repay_vault_{}", arg.vault_id))?;
    let amount: ICUSD = arg.amount.into();
    let vault = match read_state(|s| s.vault_id_to_vaults.get(&arg.vault_id).cloned()) {
        Some(v) => v,
        None => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError("Vault not found".to_string()));
        }
    };

    // Check collateral status allows repayment
    let collateral_status = read_state(|s| s.get_collateral_status(&vault.collateral_type));
    if let Some(status) = collateral_status {
        if !status.allows_repay() {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "Repayment is not allowed for this collateral type.".to_string(),
            ));
        }
    }

    if caller != vault.owner {
        guard_principal.fail();
        return Err(ProtocolError::CallerNotOwner);
    }

    if amount < MIN_ICUSD_AMOUNT {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: MIN_ICUSD_AMOUNT.to_u64(),
        });
    }

    if vault.borrowed_icusd_amount < amount {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(format!(
            "cannot repay more than borrowed: {} ICUSD, repay: {} ICUSD",
            vault.borrowed_icusd_amount, amount
        )));
    }

    match transfer_icusd_from(amount, caller).await {
        Ok(block_index) => {
            mutate_state(|s| record_repayed_to_vault(s, arg.vault_id, amount, block_index));
            guard_principal.complete(); // Mark as completed
            Ok(block_index)
        }
        Err(transfer_from_error) => {
            guard_principal.fail(); // Mark as failed
            Err(ProtocolError::TransferFromError(
                transfer_from_error,
                amount.to_u64(),
            ))
        }
    }
}

/// Repay vault debt using ckUSDT or ckUSDC (1:1 with icUSD, plus configurable fee)
pub async fn repay_to_vault_with_stable(arg: VaultArgWithToken) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal = GuardPrincipal::new(caller, &format!("repay_vault_stable_{}", arg.vault_id))?;

    // Check if the selected stable token is enabled
    let is_enabled = read_state(|s| match arg.token_type {
        StableTokenType::CKUSDT => s.ckusdt_enabled,
        StableTokenType::CKUSDC => s.ckusdc_enabled,
    });
    if !is_enabled {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(format!("{:?} repayments are currently disabled", arg.token_type)));
    }

    // Depeg protection: fetch fresh stablecoin price and reject if outside $0.95–$1.05
    if let Err(e) = crate::xrc::ensure_stable_not_depegged(&arg.token_type).await {
        guard_principal.fail();
        return Err(e);
    }

    // Truncate to nearest 100 e8s for clean 8→6 decimal conversion
    let raw_amount_e8s = arg.amount - (arg.amount % 100);
    let amount: ICUSD = raw_amount_e8s.into();

    let vault = match read_state(|s| s.vault_id_to_vaults.get(&arg.vault_id).cloned()) {
        Some(v) => v,
        None => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError("Vault not found".to_string()));
        }
    };

    // Check collateral status allows repayment
    let collateral_status = read_state(|s| s.get_collateral_status(&vault.collateral_type));
    if let Some(status) = collateral_status {
        if !status.allows_repay() {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "Repayment is not allowed for this collateral type.".to_string(),
            ));
        }
    }

    if caller != vault.owner {
        guard_principal.fail();
        return Err(ProtocolError::CallerNotOwner);
    }

    if amount < MIN_ICUSD_AMOUNT {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: MIN_ICUSD_AMOUNT.to_u64(),
        });
    }

    if vault.borrowed_icusd_amount < amount {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(format!(
            "cannot repay more than borrowed: {} ICUSD, repay: {} ICUSD",
            vault.borrowed_icusd_amount, amount
        )));
    }

    // Convert e8s (icUSD) to e6s (ckstable) and add fee surcharge
    let base_stable_e6s = raw_amount_e8s / 100;
    let fee_rate = read_state(|s| s.ckstable_repay_fee);
    let fee_e6s = (rust_decimal::Decimal::from(base_stable_e6s) * fee_rate.0)
        .to_u64().unwrap_or(0);
    let total_pull_e6s = base_stable_e6s + fee_e6s;

    // Transfer the stable token from user (in 6-decimal units)
    match transfer_stable_from(arg.token_type.clone(), total_pull_e6s, caller).await {
        Ok(block_index) => {
            mutate_state(|s| record_repayed_to_vault(s, arg.vault_id, amount, block_index));
            guard_principal.complete();
            Ok(block_index)
        }
        Err(transfer_from_error) => {
            guard_principal.fail();
            Err(ProtocolError::TransferFromError(
                transfer_from_error,
                total_pull_e6s,
            ))
        }
    }
}

pub async fn add_margin_to_vault(arg: VaultArg) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal = GuardPrincipal::new(caller, &format!("add_margin_vault_{}", arg.vault_id))?;
    let amount: ICP = arg.amount.into();

    if amount < MIN_ICP_AMOUNT {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: MIN_ICP_AMOUNT.to_u64(),
        });
    }

    let (vault, config_ledger) = match read_state(|s| {
        match s.vault_id_to_vaults.get(&arg.vault_id) {
            Some(v) => {
                let ledger = s.get_collateral_config(&v.collateral_type)
                    .map(|c| c.ledger_canister_id)
                    .ok_or("Collateral type not configured")?;
                Ok((v.clone(), ledger))
            },
            None => Err("Vault not found"),
        }
    }) {
        Ok(result) => result,
        Err(msg) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(msg.to_string()));
        }
    };

    // Check collateral status allows adding collateral
    let collateral_status = read_state(|s| s.get_collateral_status(&vault.collateral_type));
    if let Some(status) = collateral_status {
        if !status.allows_add_collateral() {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "Adding collateral is not allowed for this collateral type.".to_string(),
            ));
        }
    }

    if caller != vault.owner {
        guard_principal.fail();
        return Err(ProtocolError::CallerNotOwner);
    }

    match transfer_collateral_from(arg.amount, caller, config_ledger).await {
        Ok(block_index) => {
            mutate_state(|s| record_add_margin_to_vault(s, arg.vault_id, amount, block_index));
            guard_principal.complete();
            Ok(block_index)
        }
        Err(error) => {
            if let TransferFromError::BadFee { expected_fee } = error.clone() {
                mutate_state(|s| {
                    if let Ok(fee) = u64::try_from(expected_fee.0) {
                        if let Some(config) = s.get_collateral_config_mut(&vault.collateral_type) {
                            config.ledger_fee = fee;
                        }
                    }
                });
            };
            guard_principal.fail();
            Err(ProtocolError::TransferFromError(error, amount.to_u64()))
        }
    }
}

// ─── Push-deposit vault operations (Oisy wallet integration) ───
//
// These mirror open_vault / add_margin_to_vault but instead of pulling funds
// via ICRC-2 transfer_from, they sweep funds that the user already pushed to
// a deterministic deposit subaccount. This avoids sequential signer popups
// that Oisy's ICRC-21/25 consent flow may trigger (whether ICRC-2 approve
// actually works through Oisy is unconfirmed).

pub async fn open_vault_with_deposit(
    borrow_amount_raw: u64,
    collateral_type_opt: Option<Principal>,
) -> Result<OpenVaultSuccess, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal = match GuardPrincipal::new(caller, "open_vault_with_deposit") {
        Ok(guard) => guard,
        Err(GuardError::AlreadyProcessing) => {
            log!(INFO, "[open_vault_with_deposit] Principal {:?} already has an ongoing operation", caller);
            return Err(ProtocolError::AlreadyProcessing);
        },
        Err(err) => return Err(err.into()),
    };

    // Resolve collateral type: default to ICP if not specified
    let collateral_type = collateral_type_opt.unwrap_or_else(|| read_state(|s| s.icp_collateral_type()));

    // Look up CollateralConfig
    let (config_ledger, config_status, config_fee) = read_state(|s| {
        match s.get_collateral_config(&collateral_type) {
            Some(config) => Ok((config.ledger_canister_id, config.status, config.ledger_fee)),
            None => Err(ProtocolError::GenericError("Collateral type not supported.".to_string())),
        }
    })?;

    if !config_status.allows_open() {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Collateral type is not accepting new vaults.".to_string(),
        ));
    }

    // Sweep funds from the caller's deposit subaccount
    let (collateral_amount, sweep_block_index) = match management::sweep_deposit(&caller, config_ledger, config_fee).await {
        Ok(result) => result,
        Err(e) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                format!("Push-deposit sweep failed: {}. Did you transfer collateral to your deposit account first?", e),
            ));
        }
    };

    let icp_margin_amount: ICP = collateral_amount.into();
    if icp_margin_amount < MIN_ICP_AMOUNT {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: MIN_ICP_AMOUNT.to_u64(),
        });
    }

    // Open the vault with the swept collateral (same logic as open_vault post-transfer)
    let vault_id = mutate_state(|s| {
        let vault_id = s.increment_vault_id();
        record_open_vault(
            s,
            Vault {
                owner: caller,
                borrowed_icusd_amount: 0.into(),
                collateral_amount,
                vault_id,
                collateral_type,
            },
            sweep_block_index,
        );
        vault_id
    });

    log!(INFO, "[open_vault_with_deposit] opened vault {} for {} with {} collateral via push-deposit (sweep block {})",
        vault_id, caller, collateral_amount, sweep_block_index);

    // If the caller also requested an initial borrow, do it now.
    // Use borrow_from_vault_internal to avoid GuardPrincipal conflict —
    // this function already holds the guard for `caller`.
    if borrow_amount_raw > 0 {
        match borrow_from_vault_internal(caller, VaultArg { vault_id, amount: borrow_amount_raw }).await {
            Ok(borrow_result) => {
                log!(INFO, "[open_vault_with_deposit] vault {} initial borrow of {} succeeded (fee: {})",
                    vault_id, borrow_amount_raw, borrow_result.fee_amount_paid);
            },
            Err(e) => {
                guard_principal.fail();
                return Err(ProtocolError::GenericError(format!(
                    "Vault created (id={}) but initial borrow of {} failed: {:?}. You can borrow in a separate call.",
                    vault_id, borrow_amount_raw, e
                )));
            }
        }
    }

    guard_principal.complete();
    Ok(OpenVaultSuccess {
        vault_id,
        block_index: sweep_block_index,
    })
}

pub async fn add_margin_with_deposit(vault_id: u64) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal = GuardPrincipal::new(caller, &format!("add_margin_deposit_{}", vault_id))?;

    let (vault, config_ledger, config_fee) = match read_state(|s| {
        match s.vault_id_to_vaults.get(&vault_id) {
            Some(v) => {
                let config = s.get_collateral_config(&v.collateral_type)
                    .ok_or("Collateral type not configured")?;
                Ok((v.clone(), config.ledger_canister_id, config.ledger_fee))
            },
            None => Err("Vault not found"),
        }
    }) {
        Ok(result) => result,
        Err(msg) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(msg.to_string()));
        }
    };

    // Check collateral status
    let collateral_status = read_state(|s| s.get_collateral_status(&vault.collateral_type));
    if let Some(status) = collateral_status {
        if !status.allows_add_collateral() {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "Adding collateral is not allowed for this collateral type.".to_string(),
            ));
        }
    }

    if caller != vault.owner {
        guard_principal.fail();
        return Err(ProtocolError::CallerNotOwner);
    }

    // Sweep funds from deposit subaccount
    let (collateral_amount, sweep_block_index) = match management::sweep_deposit(&caller, config_ledger, config_fee).await {
        Ok(result) => result,
        Err(e) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                format!("Push-deposit sweep failed: {}. Did you transfer collateral to your deposit account first?", e),
            ));
        }
    };

    let margin_added: ICP = collateral_amount.into();
    if margin_added < MIN_ICP_AMOUNT {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: MIN_ICP_AMOUNT.to_u64(),
        });
    }

    mutate_state(|s| record_add_margin_to_vault(s, vault_id, margin_added, sweep_block_index));

    log!(INFO, "[add_margin_with_deposit] added {} collateral to vault {} via push-deposit (sweep block {})",
        collateral_amount, vault_id, sweep_block_index);

    guard_principal.complete();
    Ok(sweep_block_index)
}

pub async fn close_vault(vault_id: u64) -> Result<Option<u64>, ProtocolError> {
    let caller = ic_cdk::caller();
    let _guard_principal = GuardPrincipal::new(caller, &format!("close_vault_{}", vault_id))?;
    
    // Check rate limits first
    mutate_state(|s| s.check_close_vault_rate_limit(caller))?;
    
    // Record the close request for rate limiting
    mutate_state(|s| s.record_close_vault_request(caller));
    
    // Check if the vault exists first
    let vault_exists = read_state(|s| s.vault_id_to_vaults.contains_key(&vault_id));
    
    if !vault_exists {
        mutate_state(|s| s.complete_close_vault_request());
        log!(
            INFO,
            "[close_vault] Vault #{} not found for principal {}",
            vault_id,
            caller
        );
        return Err(ProtocolError::GenericError(format!("Vault #{} not found", vault_id)));
    }
    
    // Get the vault
    let vault = read_state(|s| {
        s.vault_id_to_vaults
            .get(&vault_id)
            .cloned()
            .ok_or(ProtocolError::GenericError("Vault not found".to_string()))
    })?;

    // Check collateral status allows closing
    let collateral_status = read_state(|s| s.get_collateral_status(&vault.collateral_type));
    if let Some(status) = collateral_status {
        if !status.allows_close() {
            mutate_state(|s| s.complete_close_vault_request());
            return Err(ProtocolError::GenericError(
                "Closing vaults is not allowed for this collateral type.".to_string(),
            ));
        }
    }

    // Verify caller is the owner
    if caller != vault.owner {
        mutate_state(|s| s.complete_close_vault_request());
        log!(
            INFO,
            "[close_vault] Principal {} is not the owner of vault #{}",
            caller,
            vault_id
        );
        return Err(ProtocolError::CallerNotOwner);
    }

    // Handle dust amounts - if debt is very small, forgive it
    if vault.borrowed_icusd_amount <= DUST_THRESHOLD {
        log!(
            INFO,
            "[close_vault] Forgiving dust debt of {} icUSD for vault #{}",
            vault.borrowed_icusd_amount,
            vault_id
        );
        
        // Record dust forgiveness
        mutate_state(|s| {
            s.dust_forgiven_total += vault.borrowed_icusd_amount;
            s.repay_to_vault(vault_id, vault.borrowed_icusd_amount);
        });
        
        // Record dust forgiveness event
        crate::storage::record_event(&crate::event::Event::DustForgiven {
            vault_id,
            amount: vault.borrowed_icusd_amount,
        });
    } else if vault.borrowed_icusd_amount > ICUSD::new(0) {
        mutate_state(|s| s.complete_close_vault_request());
        log!(
            INFO,
            "[close_vault] Cannot close vault #{} with outstanding debt: {}",
            vault_id,
            vault.borrowed_icusd_amount
        );
        return Err(ProtocolError::GenericError(
            "Cannot close vault with outstanding debt. Repay all debt first.".to_string()
        ));
    }

    // Verify there's no remaining collateral
    if vault.collateral_amount > 0 {
        mutate_state(|s| s.complete_close_vault_request());
        log!(
            INFO,
            "[close_vault] Cannot close vault #{} with remaining collateral: {}",
            vault_id,
            vault.collateral_amount
        );
        return Err(ProtocolError::GenericError(
            "Cannot close vault with remaining collateral. Withdraw collateral first.".to_string()
        ));
    }

    // Simply close the vault - no transfers needed
    mutate_state(|s| {
        // Make sure vault exists before attempting to remove
        if s.vault_id_to_vaults.contains_key(&vault_id) {
            // Remove from vault_id_to_vaults map
            s.vault_id_to_vaults.remove(&vault_id);
            
            // Remove from principal_to_vault_ids map
            if let Some(vault_ids) = s.principal_to_vault_ids.get_mut(&vault.owner) {
                vault_ids.remove(&vault_id);
                // If this was the user's last vault, remove the principal entry
                if vault_ids.is_empty() {
                    s.principal_to_vault_ids.remove(&vault.owner);
                }
            }
            
            // Record the close vault event
            crate::event::record_close_vault(s, vault_id, None);
            
            // Complete the close request
            s.complete_close_vault_request();
            
            log!(
                INFO,
                "[close_vault] Successfully closed vault #{} for principal {}",
                vault_id,
                caller
            );
        } else {
            // Log that we tried to close a vault that was already gone
            log!(
                INFO,
                "[close_vault] Attempted to close vault #{} that was already removed",
                vault_id
            );
            s.complete_close_vault_request();
        }
    });
    
    // Return success with no block index (since no transfer was made)
    Ok(None)
}

pub async fn withdraw_collateral(vault_id: u64) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::caller();
    let _guard_principal = GuardPrincipal::new(caller, &format!("withdraw_collateral_{}", vault_id))?;
    
    log!(
        INFO,
        "[withdraw_collateral] Request to withdraw collateral from vault #{} by principal {}",
        vault_id,
        caller
    );
    
    // Check vault exists and caller is owner
    let vault = read_state(|state| {
        state.vault_id_to_vaults
            .get(&vault_id)
            .cloned()
            .ok_or(ProtocolError::GenericError("Vault not found".to_string()))
    })?;

    // Check collateral status allows withdrawal
    let collateral_status = read_state(|s| s.get_collateral_status(&vault.collateral_type));
    if let Some(status) = collateral_status {
        if !status.allows_withdraw() {
            return Err(ProtocolError::GenericError(
                "Withdrawal is not allowed for this collateral type.".to_string(),
            ));
        }
    }

    if caller != vault.owner {
        log!(
            INFO,
            "[withdraw_collateral] Caller {} is not the owner of vault #{}",
            caller,
            vault_id
        );
        return Err(ProtocolError::CallerNotOwner);
    }
    
    // Check there's no debt
    if vault.borrowed_icusd_amount > ICUSD::new(0) {
        log!(
            INFO,
            "[withdraw_collateral] Vault #{} has outstanding debt of {} icUSD",
            vault_id,
            vault.borrowed_icusd_amount
        );
        return Err(ProtocolError::GenericError(format!(
            "Vault has {} icUSD debt. You must repay all debt before withdrawing collateral.",
            vault.borrowed_icusd_amount
        )));
    }
    
    // Check there's collateral to withdraw
    if vault.collateral_amount == 0 {
        log!(
            INFO,
            "[withdraw_collateral] Vault #{} has no collateral to withdraw",
            vault_id
        );
        return Err(ProtocolError::GenericError("No collateral to withdraw".to_string()));
    }

    // Look up per-collateral config
    let (ledger_canister_id, ledger_fee) = read_state(|s| {
        let config = s.get_collateral_config(&vault.collateral_type)
            .ok_or(ProtocolError::GenericError("Collateral type not configured".to_string()))?;
        Ok::<_, ProtocolError>((config.ledger_canister_id, config.ledger_fee))
    })?;

    // Get the amount to transfer
    let amount_to_transfer = ICP::from(vault.collateral_amount);
    log!(
        INFO,
        "[withdraw_collateral] Withdrawing {} from vault #{}",
        amount_to_transfer,
        vault_id
    );

    // Set margin to zero in vault BEFORE transferring to avoid reentrancy issues
    mutate_state(|state| {
        if let Some(vault) = state.vault_id_to_vaults.get_mut(&vault_id) {
            vault.collateral_amount = 0;
        }
    });

    // Make the collateral transfer with appropriate fee deduction
    let fee = ICP::from(ledger_fee);
    let transfer_amount = amount_to_transfer - fee;

    log!(
        INFO,
        "[withdraw_collateral] Transferring {} (after fee deduction) to {}",
        transfer_amount,
        caller
    );

    match management::transfer_collateral(transfer_amount.to_u64(), caller, ledger_canister_id).await {
        Ok(block_index) => {
            // Fix for the lifetime issue - we need to use a separate mutate_state call
            // Rather than passing a mutable reference to the state
            mutate_state(|s| crate::event::record_collateral_withdrawn(s, vault_id, amount_to_transfer, block_index));

            log!(
                INFO,
                "[withdraw_collateral] Successfully withdrew {} from vault #{}, transfer block_index: {}",
                amount_to_transfer,
                vault_id,
                block_index
            );

            Ok(block_index)
        },
        Err(error) => {
            // If the transfer fails, we need to restore the collateral in the vault
            mutate_state(|state| {
                if let Some(vault) = state.vault_id_to_vaults.get_mut(&vault_id) {
                    vault.collateral_amount = amount_to_transfer.to_u64();
                }
            });

            log!(
                DEBUG,
                "[withdraw_collateral] Failed to transfer {} to {}, error: {}",
                transfer_amount,
                caller,
                error
            );

            Err(ProtocolError::TransferError(error))
        }
    }
}
pub async fn withdraw_partial_collateral(vault_id: u64, amount: u64) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::caller();
    let _guard_principal = GuardPrincipal::new(caller, &format!("withdraw_partial_{}", vault_id))?;

    let withdraw_amount: ICP = ICP::new(amount);

    if withdraw_amount < MIN_ICP_AMOUNT {
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: MIN_ICP_AMOUNT.to_u64(),
        });
    }

    log!(
        INFO,
        "[withdraw_partial_collateral] Request to withdraw {} from vault #{} by principal {}",
        withdraw_amount,
        vault_id,
        caller
    );

    // Read vault, per-collateral price + config from state
    let (vault, collateral_price, config_decimals, ledger_canister_id, ledger_fee) = match read_state(|s| {
        match s.vault_id_to_vaults.get(&vault_id) {
            Some(vault) => {
                let price = s.get_collateral_price_decimal(&vault.collateral_type)
                    .ok_or("No price available for collateral. Price feed may be down.")?;
                let config = s.get_collateral_config(&vault.collateral_type)
                    .ok_or("Collateral type not configured.")?;
                Ok((vault.clone(), price, config.decimals, config.ledger_canister_id, config.ledger_fee))
            },
            None => Err("Vault not found. Please check the vault ID.")
        }
    }) {
        Ok(result) => result,
        Err(msg) => return Err(ProtocolError::GenericError(msg.to_string())),
    };

    // Check collateral status allows withdrawal
    let collateral_status = read_state(|s| s.get_collateral_status(&vault.collateral_type));
    if let Some(status) = collateral_status {
        if !status.allows_withdraw() {
            return Err(ProtocolError::GenericError(
                "Withdrawal is not allowed for this collateral type.".to_string(),
            ));
        }
    }

    if caller != vault.owner {
        return Err(ProtocolError::CallerNotOwner);
    }

    let vault_collateral = ICP::from(vault.collateral_amount);

    if vault_collateral == ICP::new(0) {
        return Err(ProtocolError::GenericError("No collateral to withdraw".to_string()));
    }

    // Calculate max withdrawable amount that keeps CR >= minimum
    let max_withdrawable = if vault.borrowed_icusd_amount == ICUSD::new(0) {
        // No debt — can withdraw everything
        vault_collateral
    } else {
        // min_collateral_value = debt * min_ratio
        // min_collateral_amount = icusd_to_collateral_amount(min_collateral_value, price, decimals)
        // max_withdrawable = current_collateral - min_collateral_amount
        let min_ratio = read_state(|s| s.get_min_collateral_ratio_for(&vault.collateral_type));
        let min_collateral_value: ICUSD = vault.borrowed_icusd_amount * min_ratio;
        let min_collateral_raw = crate::numeric::icusd_to_collateral_amount(min_collateral_value, collateral_price, config_decimals);
        let min_collateral = ICP::from(min_collateral_raw);

        if vault_collateral <= min_collateral {
            return Err(ProtocolError::GenericError(
                "No excess collateral to withdraw. Your vault is already at or below the minimum collateral ratio.".to_string()
            ));
        }

        vault_collateral - min_collateral
    };

    if withdraw_amount > max_withdrawable {
        return Err(ProtocolError::GenericError(format!(
            "Withdrawal amount exceeds maximum. Max withdrawable: {} (keeps CR above minimum).",
            max_withdrawable
        )));
    }

    log!(
        INFO,
        "[withdraw_partial_collateral] Max withdrawable: {}, requested: {} from vault #{}",
        max_withdrawable,
        withdraw_amount,
        vault_id
    );

    // Reduce margin BEFORE transferring to avoid reentrancy
    mutate_state(|state| {
        if let Some(vault) = state.vault_id_to_vaults.get_mut(&vault_id) {
            vault.collateral_amount -= withdraw_amount.to_u64();
        }
    });

    let fee = ICP::from(ledger_fee);
    let transfer_amount = withdraw_amount - fee;

    log!(
        INFO,
        "[withdraw_partial_collateral] Transferring {} (after fee) to {}",
        transfer_amount,
        caller
    );

    match management::transfer_collateral(transfer_amount.to_u64(), caller, ledger_canister_id).await {
        Ok(block_index) => {
            mutate_state(|s| crate::event::record_partial_collateral_withdrawn(s, vault_id, withdraw_amount, block_index));

            log!(
                INFO,
                "[withdraw_partial_collateral] Successfully withdrew {} from vault #{}, block_index: {}",
                withdraw_amount,
                vault_id,
                block_index
            );

            Ok(block_index)
        },
        Err(error) => {
            // Restore collateral on transfer failure
            mutate_state(|state| {
                if let Some(vault) = state.vault_id_to_vaults.get_mut(&vault_id) {
                    vault.collateral_amount += withdraw_amount.to_u64();
                }
            });

            log!(
                DEBUG,
                "[withdraw_partial_collateral] Failed to transfer {} to {}, error: {}",
                transfer_amount,
                caller,
                error
            );

            Err(ProtocolError::TransferError(error))
        }
    }
}

pub async fn withdraw_and_close_vault(vault_id: u64) -> Result<Option<u64>, ProtocolError> {
    let caller = ic_cdk::caller();
    // Use a specific name for better tracking
    let _guard_principal = GuardPrincipal::new(caller, &format!("withdraw_and_close_{}", vault_id))?;
    
    log!(
        INFO,
        "[withdraw_and_close] Request for vault #{} by principal {}",
        vault_id,
        caller
    );
    
    // Check if the vault exists first
    let vault = read_state(|s| {
        s.vault_id_to_vaults
            .get(&vault_id)
            .cloned()
            .ok_or(ProtocolError::GenericError(format!("Vault #{} not found", vault_id)))
    })?;

    // Check collateral status allows withdraw + close
    let collateral_status = read_state(|s| s.get_collateral_status(&vault.collateral_type));
    if let Some(status) = collateral_status {
        if !status.allows_withdraw() || !status.allows_close() {
            return Err(ProtocolError::GenericError(
                "Withdraw-and-close is not allowed for this collateral type.".to_string(),
            ));
        }
    }

    // Verify caller is the owner
    if caller != vault.owner {
        log!(
            INFO,
            "[withdraw_and_close] Principal {} is not the owner of vault #{}",
            caller,
            vault_id
        );
        return Err(ProtocolError::CallerNotOwner);
    }

    // Check there's no debt
    if vault.borrowed_icusd_amount > ICUSD::new(0) {
        log!(
            INFO,
            "[withdraw_and_close] Vault #{} has outstanding debt of {} icUSD",
            vault_id,
            vault.borrowed_icusd_amount
        );
        return Err(ProtocolError::GenericError(format!(
            "Vault has {} icUSD debt. You must repay all debt before withdrawing and closing.",
            vault.borrowed_icusd_amount
        )));
    }
    
    // Look up per-collateral config
    let (ledger_canister_id, ledger_fee) = read_state(|s| {
        let config = s.get_collateral_config(&vault.collateral_type)
            .ok_or(ProtocolError::GenericError("Collateral type not configured".to_string()))?;
        Ok::<_, ProtocolError>((config.ledger_canister_id, config.ledger_fee))
    })?;

    // If there's collateral, withdraw it first
    let mut block_index: Option<u64> = None;
    let amount_to_transfer = ICP::from(vault.collateral_amount);

    if amount_to_transfer > ICP::new(0) {
        log!(
            INFO,
            "[withdraw_and_close] Withdrawing {} from vault #{}",
            amount_to_transfer,
            vault_id
        );

        // Set margin to zero in vault BEFORE transferring to avoid reentrancy issues
        mutate_state(|state| {
            if let Some(vault) = state.vault_id_to_vaults.get_mut(&vault_id) {
                vault.collateral_amount = 0;
            }
        });

        // Make the collateral transfer with appropriate fee deduction
        let fee = ICP::from(ledger_fee);
        let transfer_amount = amount_to_transfer - fee;

        log!(
            INFO,
            "[withdraw_and_close] Transferring {} (after fee deduction) to {}",
            transfer_amount,
            caller
        );

        match management::transfer_collateral(transfer_amount.to_u64(), caller, ledger_canister_id).await {
            Ok(idx) => {
                // Record the withdrawal event
                mutate_state(|s| crate::event::record_collateral_withdrawn(s, vault_id, amount_to_transfer, idx));

                log!(
                    INFO,
                    "[withdraw_and_close] Successfully withdrew {} from vault #{}, block_index: {}",
                    amount_to_transfer,
                    vault_id,
                    idx
                );

                block_index = Some(idx);
            },
            Err(error) => {
                // CRITICAL: If the transfer fails, restore the collateral and exit WITHOUT closing the vault
                mutate_state(|state| {
                    if let Some(vault) = state.vault_id_to_vaults.get_mut(&vault_id) {
                        vault.collateral_amount = amount_to_transfer.to_u64();
                    }
                });

                log!(
                    DEBUG,
                    "[withdraw_and_close] Failed to transfer {} to {}, error: {}",
                    transfer_amount,
                    caller,
                    error
                );

                return Err(ProtocolError::TransferError(error));
            }
        }
    } else {
        log!(INFO, "[withdraw_and_close] Vault #{} has no collateral to withdraw", vault_id);
    };
    
    // Now close the vault - only if we've successfully transferred any funds
    // or if there were no funds to transfer
    mutate_state(|s| {
        // Make sure vault exists before attempting to remove
        if s.vault_id_to_vaults.contains_key(&vault_id) {
            // Record the combined withdraw and close event
            crate::event::record_withdraw_and_close_vault(s, vault_id, amount_to_transfer, block_index);
            
            log!(
                INFO,
                "[withdraw_and_close] Successfully closed vault #{} for principal {}",
                vault_id,
                caller
            );
        } else {
            // Log that we tried to close a vault that was already gone
            log!(
                INFO,
                "[withdraw_and_close] Attempted to close vault #{} that was already removed",
                vault_id
            );
        }
    });
    
    // Return the block index if we did a transfer, otherwise None
    Ok(block_index)
}

pub async fn liquidate_vault_partial(vault_id: u64, icusd_amount: u64) -> Result<SuccessWithFee, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal = GuardPrincipal::new(caller, &format!("liquidate_vault_partial_{}", vault_id))?;
    
    let liquidation_amount: ICUSD = icusd_amount.into();
    
    if liquidation_amount < MIN_ICUSD_AMOUNT {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: MIN_ICUSD_AMOUNT.to_u64(),
        });
    }
    
    // Step 1: Validate vault is liquidatable and get partial liquidation amounts
    let (vault, collateral_price, config_decimals, collateral_price_usd, _mode, max_liquidatable_debt, collateral_to_liquidator) = match read_state(|s| {
        match s.vault_id_to_vaults.get(&vault_id) {
            Some(vault) => {
                // Check collateral status allows liquidation
                if let Some(status) = s.get_collateral_status(&vault.collateral_type) {
                    if !status.allows_liquidation() {
                        return Err("Liquidation is not allowed for this collateral type.".to_string());
                    }
                }

                let price = s.get_collateral_price_decimal(&vault.collateral_type)
                    .ok_or_else(|| "No price available for collateral. Price feed may be down.".to_string())?;
                let decimals = s.get_collateral_config(&vault.collateral_type)
                    .map(|c| c.decimals)
                    .unwrap_or(8);
                let collateral_price_usd = UsdIcp::from(price);
                let ratio = compute_collateral_ratio(vault, collateral_price_usd, s);
                let min_liq_ratio = s.get_min_liquidation_ratio_for(&vault.collateral_type);

                if ratio >= min_liq_ratio {
                    Err(format!(
                        "Vault #{} is not liquidatable. Current ratio: {}, minimum: {}",
                        vault_id,
                        ratio.to_f64(),
                        min_liq_ratio.to_f64()
                    ))
                } else {
                    // Cap at the amount needed to restore vault CR to recovery_target_cr
                    let max_liquidatable = s.compute_partial_liquidation_cap(vault, collateral_price_usd);

                    // Ensure requested amount doesn't exceed maximum
                    let actual_liquidation_amount = liquidation_amount.min(max_liquidatable).min(vault.borrowed_icusd_amount);

                    if actual_liquidation_amount == ICUSD::new(0) {
                        return Err("Cannot liquidate zero amount".to_string());
                    }

                    // Calculate collateral to transfer (debt + liquidation bonus)
                    let liq_bonus = s.get_liquidation_bonus_for(&vault.collateral_type);
                    let collateral_raw = crate::numeric::icusd_to_collateral_amount(actual_liquidation_amount, price, decimals);
                    let collateral_with_bonus = ICP::from(collateral_raw) * liq_bonus;
                    let collateral_to_transfer = collateral_with_bonus.min(ICP::from(vault.collateral_amount));

                    Ok((vault.clone(), price, decimals, collateral_price_usd, s.mode, actual_liquidation_amount, collateral_to_transfer))
                }
            },
            None => Err(format!("Vault #{} not found", vault_id)),
        }
    }) {
        Ok(result) => result,
        Err(msg) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(msg));
        }
    };

    log!(INFO,
        "[liquidate_vault_partial] Vault #{}: liquidating {} icUSD (max: {}), getting {} ICP collateral",
        vault_id,
        max_liquidatable_debt.to_u64(),
        vault.borrowed_icusd_amount.to_u64(),
        collateral_to_liquidator.to_u64()
    );

    // Step 2: Take icUSD from liquidator
    let icusd_block_index = match transfer_icusd_from(max_liquidatable_debt, caller).await {
        Ok(block_index) => {
            log!(INFO, "[liquidate_vault_partial] Received {} icUSD from liquidator", max_liquidatable_debt.to_u64());
            block_index
        },
        Err(transfer_from_error) => {
            guard_principal.fail();
            return Err(ProtocolError::TransferFromError(transfer_from_error, max_liquidatable_debt.to_u64()));
        }
    };

    // Step 3: Update protocol state (partial liquidation)
    mutate_state(|s| {
        // Reduce vault debt and collateral directly
        if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
            vault.borrowed_icusd_amount -= max_liquidatable_debt;
            vault.collateral_amount -= collateral_to_liquidator.to_u64();
        }

        // Record the partial liquidation event
        let event = crate::event::Event::PartialLiquidateVault {
            vault_id,
            liquidator_payment: max_liquidatable_debt,
            icp_to_liquidator: collateral_to_liquidator,
            liquidator: Some(caller),
            icp_rate: Some(collateral_price_usd),
        };
        crate::storage::record_event(&event);

        // Create pending transfer for liquidator reward
        s.pending_margin_transfers.insert(
            vault_id,
            PendingMarginTransfer {
                owner: caller,
                margin: collateral_to_liquidator,
                collateral_type: vault.collateral_type,
            },
        );

        log!(INFO, "[liquidate_vault_partial] Partial liquidation completed, {} pending transfers created", 1);
    });
    
    // Step 4: Process transfer (same as complete liquidation)
    match try_process_pending_transfers_immediate(vault_id).await {
        Ok(processed_count) => {
            log!(INFO, "[liquidate_vault_partial] Successfully processed {} transfers immediately", processed_count);
        },
        Err(e) => {
            log!(INFO, "[liquidate_vault_partial] Immediate processing failed: {}. Transfers will be retried via timer", e);
            schedule_transfer_retry(vault_id, 0);
        }
    }
    
    ic_cdk_timers::set_timer(std::time::Duration::from_secs(2), move || {
        ic_cdk::spawn(async move {
            log!(INFO, "[liquidate_vault_partial] Backup timer processing transfers for vault #{}", vault_id);
            let _ = crate::process_pending_transfer().await;
        })
    });
    
    guard_principal.complete();
    
    // Calculate fee (liquidator bonus)
    let liquidator_value_received = crate::numeric::collateral_usd_value(collateral_to_liquidator.to_u64(), collateral_price, config_decimals);
    let fee_amount = if liquidator_value_received > max_liquidatable_debt {
        liquidator_value_received - max_liquidatable_debt
    } else {
        ICUSD::new(0)
    };

    log!(INFO, "[liquidate_vault_partial] Partial liquidation completed. Block index: {}, Fee: {}",
         icusd_block_index, fee_amount.to_u64());

    Ok(SuccessWithFee {
        block_index: icusd_block_index,
        fee_amount_paid: fee_amount.to_u64(),
    })
}

/// Liquidate a vault using ckUSDT or ckUSDC (1:1 with icUSD, plus configurable fee)
pub async fn liquidate_vault_partial_with_stable(
    vault_id: u64,
    stable_amount: u64,
    token_type: StableTokenType
) -> Result<SuccessWithFee, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal = GuardPrincipal::new(caller, &format!("liquidate_vault_stable_{}", vault_id))?;

    // Check if the selected stable token is enabled
    let is_enabled = read_state(|s| match token_type {
        StableTokenType::CKUSDT => s.ckusdt_enabled,
        StableTokenType::CKUSDC => s.ckusdc_enabled,
    });
    if !is_enabled {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(format!("{:?} liquidations are currently disabled", token_type)));
    }

    // Depeg protection: fetch fresh stablecoin price and reject if outside $0.95–$1.05
    if let Err(e) = crate::xrc::ensure_stable_not_depegged(&token_type).await {
        guard_principal.fail();
        return Err(e);
    }

    // Truncate to nearest 100 e8s for clean 8→6 decimal conversion
    let raw_amount_e8s = stable_amount - (stable_amount % 100);
    let liquidation_amount: ICUSD = raw_amount_e8s.into();

    if liquidation_amount < MIN_ICUSD_AMOUNT {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: MIN_ICUSD_AMOUNT.to_u64(),
        });
    }

    // Step 1: Validate vault is liquidatable and get partial liquidation amounts
    let (vault, collateral_price, config_decimals, collateral_price_usd, _mode, max_liquidatable_debt, collateral_to_liquidator) = match read_state(|s| {
        match s.vault_id_to_vaults.get(&vault_id) {
            Some(vault) => {
                // Check collateral status allows liquidation
                if let Some(status) = s.get_collateral_status(&vault.collateral_type) {
                    if !status.allows_liquidation() {
                        return Err("Liquidation is not allowed for this collateral type.".to_string());
                    }
                }

                let price = s.get_collateral_price_decimal(&vault.collateral_type)
                    .ok_or_else(|| "No price available for collateral. Price feed may be down.".to_string())?;
                let decimals = s.get_collateral_config(&vault.collateral_type)
                    .map(|c| c.decimals)
                    .unwrap_or(8);
                let collateral_price_usd = UsdIcp::from(price);
                let ratio = compute_collateral_ratio(vault, collateral_price_usd, s);
                let min_liq_ratio = s.get_min_liquidation_ratio_for(&vault.collateral_type);

                if ratio >= min_liq_ratio {
                    Err(format!(
                        "Vault #{} is not liquidatable. Current ratio: {}, minimum: {}",
                        vault_id,
                        ratio.to_f64(),
                        min_liq_ratio.to_f64()
                    ))
                } else {
                    // Cap at the amount needed to restore vault CR to recovery_target_cr
                    let max_liquidatable = s.compute_partial_liquidation_cap(vault, collateral_price_usd);

                    let actual_liquidation_amount = liquidation_amount.min(max_liquidatable).min(vault.borrowed_icusd_amount);

                    if actual_liquidation_amount == ICUSD::new(0) {
                        return Err("Cannot liquidate zero amount".to_string());
                    }

                    let liq_bonus = s.get_liquidation_bonus_for(&vault.collateral_type);
                    let collateral_raw = crate::numeric::icusd_to_collateral_amount(actual_liquidation_amount, price, decimals);
                    let collateral_with_bonus = ICP::from(collateral_raw) * liq_bonus;
                    let collateral_to_transfer = collateral_with_bonus.min(ICP::from(vault.collateral_amount));

                    Ok((vault.clone(), price, decimals, collateral_price_usd, s.mode, actual_liquidation_amount, collateral_to_transfer))
                }
            },
            None => Err(format!("Vault #{} not found", vault_id)),
        }
    }) {
        Ok(result) => result,
        Err(msg) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(msg));
        }
    };

    log!(INFO,
        "[liquidate_vault_stable] Vault #{}: liquidating {} {:?} (max: {}), getting {} ICP collateral",
        vault_id,
        max_liquidatable_debt.to_u64(),
        token_type,
        vault.borrowed_icusd_amount.to_u64(),
        collateral_to_liquidator.to_u64()
    );

    // Step 2: Convert e8s to e6s and add fee surcharge, then take stable token from liquidator
    let debt_e8s = max_liquidatable_debt.to_u64();
    let base_stable_e6s = debt_e8s / 100;
    let fee_rate = read_state(|s| s.ckstable_repay_fee);
    let fee_e6s = (rust_decimal::Decimal::from(base_stable_e6s) * fee_rate.0)
        .to_u64().unwrap_or(0);
    let total_pull_e6s = base_stable_e6s + fee_e6s;

    let stable_block_index = match transfer_stable_from(token_type.clone(), total_pull_e6s, caller).await {
        Ok(block_index) => {
            log!(INFO, "[liquidate_vault_stable] Received {} e6s {:?} from liquidator (fee: {} e6s)", total_pull_e6s, token_type, fee_e6s);
            block_index
        },
        Err(transfer_from_error) => {
            guard_principal.fail();
            return Err(ProtocolError::TransferFromError(transfer_from_error, total_pull_e6s));
        }
    };

    // Step 3: Update protocol state (partial liquidation)
    mutate_state(|s| {
        // Reduce vault debt and collateral directly
        if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
            vault.borrowed_icusd_amount -= max_liquidatable_debt;
            vault.collateral_amount -= collateral_to_liquidator.to_u64();
        }

        // Record the partial liquidation event
        let event = crate::event::Event::PartialLiquidateVault {
            vault_id,
            liquidator_payment: max_liquidatable_debt,
            icp_to_liquidator: collateral_to_liquidator,
            liquidator: Some(caller),
            icp_rate: Some(collateral_price_usd),
        };
        crate::storage::record_event(&event);

        // Create pending transfer for liquidator reward
        s.pending_margin_transfers.insert(
            vault_id,
            PendingMarginTransfer {
                owner: caller,
                margin: collateral_to_liquidator,
                collateral_type: vault.collateral_type,
            },
        );

        log!(INFO, "[liquidate_vault_stable] Partial liquidation completed, pending transfer created");
    });

    // Step 4: Process transfer
    match try_process_pending_transfers_immediate(vault_id).await {
        Ok(processed_count) => {
            log!(INFO, "[liquidate_vault_stable] Successfully processed {} transfers immediately", processed_count);
        },
        Err(e) => {
            log!(INFO, "[liquidate_vault_stable] Immediate processing failed: {}. Transfers will be retried via timer", e);
            schedule_transfer_retry(vault_id, 0);
        }
    }

    ic_cdk_timers::set_timer(std::time::Duration::from_secs(2), move || {
        ic_cdk::spawn(async move {
            log!(INFO, "[liquidate_vault_stable] Backup timer processing transfers for vault #{}", vault_id);
            let _ = crate::process_pending_transfer().await;
        })
    });

    guard_principal.complete();

    // Calculate fee (liquidator bonus)
    let liquidator_value_received = crate::numeric::collateral_usd_value(collateral_to_liquidator.to_u64(), collateral_price, config_decimals);
    let fee_amount = if liquidator_value_received > max_liquidatable_debt {
        liquidator_value_received - max_liquidatable_debt
    } else {
        ICUSD::new(0)
    };

    log!(INFO, "[liquidate_vault_stable] Liquidation completed. Block index: {}, Fee: {}",
         stable_block_index, fee_amount.to_u64());

    Ok(SuccessWithFee {
        block_index: stable_block_index,
        fee_amount_paid: fee_amount.to_u64(),
    })
}

pub async fn liquidate_vault(vault_id: u64) -> Result<SuccessWithFee, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal = GuardPrincipal::new(caller, &format!("liquidate_vault_{}", vault_id))?;
    
    // Step 1: Validate vault is liquidatable
    let (vault, collateral_price, config_decimals, collateral_price_usd, mode) = match read_state(|s| {
        match s.vault_id_to_vaults.get(&vault_id) {
            Some(vault) => {
                // Check collateral status allows liquidation
                if let Some(status) = s.get_collateral_status(&vault.collateral_type) {
                    if !status.allows_liquidation() {
                        return Err("Liquidation is not allowed for this collateral type.".to_string());
                    }
                }

                let price = s.get_collateral_price_decimal(&vault.collateral_type)
                    .ok_or_else(|| "No price available for collateral. Price feed may be down.".to_string())?;
                let decimals = s.get_collateral_config(&vault.collateral_type)
                    .map(|c| c.decimals)
                    .unwrap_or(8);
                let collateral_price_usd = UsdIcp::from(price);
                let ratio = compute_collateral_ratio(vault, collateral_price_usd, s);
                let min_liq_ratio = s.get_min_liquidation_ratio_for(&vault.collateral_type);

                if ratio >= min_liq_ratio {
                    Err(format!(
                        "Vault #{} is not liquidatable. Current ratio: {}, minimum: {}",
                        vault_id,
                        ratio.to_f64(),
                        min_liq_ratio.to_f64()
                    ))
                } else {
                    Ok((vault.clone(), price, decimals, collateral_price_usd, s.mode))
                }
            },
            None => Err(format!("Vault #{} not found", vault_id)),
        }
    }) {
        Ok(result) => result,
        Err(msg) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(msg));
        }
    };

    // Step 2: Calculate liquidation amounts
    // Check if this is a recovery-mode targeted liquidation (vault CR between 133-150%)
    let vault_collateral = ICP::from(vault.collateral_amount);
    let (debt_amount, icp_to_liquidator, excess_collateral, is_recovery_partial) = read_state(|s| {
        let liq_bonus = s.get_liquidation_bonus_for(&vault.collateral_type);
        if let Some(repay_cap) = s.compute_recovery_repay_cap(&vault, collateral_price_usd) {
            // Recovery mode: only liquidate enough to restore CR to target
            let collateral_raw = crate::numeric::icusd_to_collateral_amount(repay_cap, collateral_price, config_decimals);
            let collateral_seized = (ICP::from(collateral_raw) * liq_bonus).min(vault_collateral);
            (repay_cap, collateral_seized, ICP::new(0), true)
        } else {
            // Normal full liquidation
            let debt = vault.borrowed_icusd_amount;
            let collateral_raw = crate::numeric::icusd_to_collateral_amount(debt, collateral_price, config_decimals);
            let icp_with_bonus = ICP::from(collateral_raw) * liq_bonus;
            let icp_to_liq = icp_with_bonus.min(vault_collateral);
            let excess = vault_collateral.saturating_sub(icp_to_liq);
            (debt, icp_to_liq, excess, false)
        }
    });

    log!(INFO,
        "[liquidate_vault] Vault #{}: debt_to_repay={} icUSD, liquidator gets {} ICP, excess={} ICP, recovery_partial={}",
        vault_id,
        debt_amount.to_u64(),
        icp_to_liquidator.to_u64(),
        excess_collateral.to_u64(),
        is_recovery_partial
    );

    // Step 3: Take icUSD from liquidator (this must succeed for liquidation to proceed)
    let icusd_block_index = match transfer_icusd_from(debt_amount, caller).await {
        Ok(block_index) => {
            log!(INFO, "[liquidate_vault] Received {} icUSD from liquidator", debt_amount.to_u64());
            block_index
        },
        Err(transfer_from_error) => {
            guard_principal.fail();
            return Err(ProtocolError::TransferFromError(transfer_from_error, debt_amount.to_u64()));
        }
    };

    // Step 4: Update protocol state ATOMICALLY (this is the critical section)
    mutate_state(|s| {
        // Execute the liquidation in state first (this must happen)
        s.liquidate_vault(vault_id, mode, collateral_price_usd);

        // Record the liquidation event
        let event = crate::event::Event::LiquidateVault {
            vault_id,
            mode,
            icp_rate: collateral_price_usd,
            liquidator: Some(caller),
        };
        crate::storage::record_event(&event);

        // Create pending transfer for liquidator reward
        s.pending_margin_transfers.insert(
            vault_id,
            PendingMarginTransfer {
                owner: caller,
                margin: icp_to_liquidator,
                collateral_type: vault.collateral_type,
            },
        );

        // Create pending transfer for excess collateral to vault owner (if any)
        // (only for full liquidations, not recovery partial)
        if !is_recovery_partial && excess_collateral > ICP::new(0) {
            log!(INFO, "[liquidate_vault] Scheduling excess collateral return to vault owner");
            s.pending_excess_transfers.insert(
                vault_id,
                PendingMarginTransfer {
                    owner: vault.owner,
                    margin: excess_collateral,
                    collateral_type: vault.collateral_type,
                },
            );
        }

        log!(INFO, "[liquidate_vault] Protocol state updated, {} pending transfers created",
             if !is_recovery_partial && excess_collateral > ICP::new(0) { 2 } else { 1 });
    });
    
    // Step 5: Attempt immediate transfer processing (best effort)
    log!(INFO, "[liquidate_vault] Attempting immediate transfer processing...");
    
    // Try to process transfers immediately
    match try_process_pending_transfers_immediate(vault_id).await {
        Ok(processed_count) => {
            log!(INFO, "[liquidate_vault] Successfully processed {} transfers immediately", processed_count);
        },
        Err(e) => {
            log!(INFO, "[liquidate_vault] Immediate processing failed: {}. Transfers will be retried via timer", e);
            
            // Schedule retry with exponential backoff
            schedule_transfer_retry(vault_id, 0);
        }
    }
    
    // Step 6: Always schedule a backup timer (in case immediate processing failed)
    ic_cdk_timers::set_timer(std::time::Duration::from_secs(2), move || {
        ic_cdk::spawn(async move {
            log!(INFO, "[liquidate_vault] Backup timer processing transfers for vault #{}", vault_id);
            let _ = crate::process_pending_transfer().await;
        })
    });
    
    // Step 7: Liquidation is successful (protocol state is consistent)
    guard_principal.complete();
    
    // Calculate fee
    let liquidator_value_received = crate::numeric::collateral_usd_value(icp_to_liquidator.to_u64(), collateral_price, config_decimals);
    let fee_amount = if liquidator_value_received > debt_amount {
        liquidator_value_received - debt_amount
    } else {
        ICUSD::new(0)
    };

    log!(INFO, "[liquidate_vault] Liquidation completed successfully. Block index: {}, Fee: {}",
         icusd_block_index, fee_amount.to_u64());
    
    Ok(SuccessWithFee {
        block_index: icusd_block_index,
        fee_amount_paid: fee_amount.to_u64(),
    })
}

// Helper function to attempt immediate transfer processing
async fn try_process_pending_transfers_immediate(vault_id: u64) -> Result<u32, String> {
    let mut processed_count = 0;

    // Get pending transfers for this liquidation
    let transfers_to_process = read_state(|s| {
        let mut transfers = Vec::new();

        // Primary transfer (liquidator reward)
        if let Some(transfer) = s.pending_margin_transfers.get(&vault_id) {
            transfers.push(("margin", vault_id, transfer.clone()));
        }

        // Excess collateral transfer (if exists)
        if let Some(transfer) = s.pending_excess_transfers.get(&vault_id) {
            transfers.push(("excess", vault_id, transfer.clone()));
        }

        transfers
    });

    // Process each transfer
    for (transfer_type, transfer_id, transfer) in transfers_to_process {
        // Look up per-collateral ledger fee and canister ID
        let (ledger_fee, ledger_canister_id) = read_state(|s| {
            match s.get_collateral_config(&transfer.collateral_type) {
                Some(config) => (ICP::from(config.ledger_fee), config.ledger_canister_id),
                None => (s.icp_ledger_fee, s.icp_ledger_principal),
            }
        });

        if transfer.margin <= ledger_fee {
            log!(INFO, "[immediate_transfer] Skipping {} transfer {} - margin {} <= fee {}, removing", transfer_type, transfer_id, transfer.margin.to_u64(), ledger_fee.to_u64());
            mutate_state(|s| {
                match transfer_type {
                    "margin" => { s.pending_margin_transfers.remove(&transfer_id); },
                    "excess" => { s.pending_excess_transfers.remove(&transfer_id); },
                    _ => {}
                }
            });
            processed_count += 1;
            continue;
        }
        let transfer_amount = transfer.margin - ledger_fee;

        log!(INFO, "[immediate_transfer] Processing {} transfer {} of {} collateral to {}",
             transfer_type, transfer_id, transfer_amount.to_u64(), transfer.owner);

        match management::transfer_collateral(transfer_amount.to_u64(), transfer.owner, ledger_canister_id).await {
            Ok(block_index) => {
                log!(INFO, "[immediate_transfer] Transfer {} successful, block: {}", transfer_id, block_index);

                // Remove from the appropriate pending map
                mutate_state(|s| {
                    match transfer_type {
                        "margin" => { s.pending_margin_transfers.remove(&transfer_id); },
                        "excess" => { s.pending_excess_transfers.remove(&transfer_id); },
                        _ => {}
                    }
                });

                processed_count += 1;
            },
            Err(error) => {
                log!(INFO, "[immediate_transfer] Transfer {} failed: {}. Will retry later", transfer_id, error);
                // Leave in pending transfers for retry
                return Err(format!("Transfer {} failed: {}", transfer_id, error));
            }
        }
    }
    
    Ok(processed_count)
}

// Helper function to schedule transfer retries with exponential backoff
fn schedule_transfer_retry(vault_id: u64, retry_count: u32) {
    let max_retries = 5;
    if retry_count >= max_retries {
        log!(INFO, "[retry_scheduler] Max retries reached for vault #{}", vault_id);
        return;
    }
    
    // Exponential backoff: 1s, 2s, 4s, 8s, 16s
    let delay_seconds = 1u64 << retry_count;
    
    log!(INFO, "[retry_scheduler] Scheduling retry #{} for vault #{} in {}s", 
         retry_count + 1, vault_id, delay_seconds);
    
    ic_cdk_timers::set_timer(std::time::Duration::from_secs(delay_seconds), move || {
        ic_cdk::spawn(async move {
            log!(INFO, "[retry_scheduler] Retry #{} executing for vault #{}", retry_count + 1, vault_id);
            
            match try_process_pending_transfers_immediate(vault_id).await {
                Ok(processed) => {
                    log!(INFO, "[retry_scheduler] Retry #{} successful, processed {} transfers", retry_count + 1, processed);
                },
                Err(_) => {
                    log!(INFO, "[retry_scheduler] Retry #{} failed, scheduling next retry", retry_count + 1);
                    schedule_transfer_retry(vault_id, retry_count + 1);
                }
            }
        })
    });
}

pub async fn partial_repay_to_vault(arg: VaultArg) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal = GuardPrincipal::new(caller, &format!("partial_repay_vault_{}", arg.vault_id))?;
    let amount: ICUSD = arg.amount.into();
    let vault = match read_state(|s| s.vault_id_to_vaults.get(&arg.vault_id).cloned()) {
        Some(v) => v,
        None => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(format!("Vault #{} not found", arg.vault_id)));
        }
    };

    // Check collateral status allows repayment
    let collateral_status = read_state(|s| s.get_collateral_status(&vault.collateral_type));
    if let Some(status) = collateral_status {
        if !status.allows_repay() {
            guard_principal.fail();
            return Err(ProtocolError::TemporarilyUnavailable(
                format!("Collateral is {:?}, repayment not allowed", status)
            ));
        }
    }

    if caller != vault.owner {
        guard_principal.fail();
        return Err(ProtocolError::CallerNotOwner);
    }

    if amount < MIN_ICUSD_AMOUNT {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: MIN_ICUSD_AMOUNT.to_u64(),
        });
    }

    if vault.borrowed_icusd_amount < amount {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(format!(
            "cannot repay more than borrowed: {} ICUSD, repay: {} ICUSD",
            vault.borrowed_icusd_amount, amount
        )));
    }

    match transfer_icusd_from(amount, caller).await {
        Ok(block_index) => {
            mutate_state(|s| record_repayed_to_vault(s, arg.vault_id, amount, block_index));
            guard_principal.complete(); // Mark as completed
            Ok(block_index)
        }
        Err(transfer_from_error) => {
            guard_principal.fail(); // Mark as failed
            Err(ProtocolError::TransferFromError(
                transfer_from_error,
                amount.to_u64(),
            ))
        }
    }
}

pub async fn partial_liquidate_vault(arg: VaultArg) -> Result<SuccessWithFee, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal = GuardPrincipal::new(caller, &format!("partial_liquidate_vault_{}", arg.vault_id))?;
    let liquidator_payment: ICUSD = arg.amount.into();
    
    // Step 1: Validate vault is liquidatable
    let (vault, collateral_price, config_decimals, collateral_price_usd, _mode) = match read_state(|s| {
        match s.vault_id_to_vaults.get(&arg.vault_id) {
            Some(vault) => {
                // Check collateral status allows liquidation
                if let Some(status) = s.get_collateral_status(&vault.collateral_type) {
                    if !status.allows_liquidation() {
                        return Err("Liquidation is not allowed for this collateral type.".to_string());
                    }
                }

                let price = s.get_collateral_price_decimal(&vault.collateral_type)
                    .ok_or_else(|| "No price available for collateral. Price feed may be down.".to_string())?;
                let decimals = s.get_collateral_config(&vault.collateral_type)
                    .map(|c| c.decimals)
                    .unwrap_or(8);
                let collateral_price_usd = UsdIcp::from(price);
                let ratio = compute_collateral_ratio(vault, collateral_price_usd, s);
                let min_liq_ratio = s.get_min_liquidation_ratio_for(&vault.collateral_type);

                if ratio >= min_liq_ratio {
                    Err(format!(
                        "Vault #{} is not liquidatable. Current ratio: {}, minimum: {}",
                        arg.vault_id,
                        ratio.to_f64(),
                        min_liq_ratio.to_f64()
                    ))
                } else {
                    Ok((vault.clone(), price, decimals, collateral_price_usd, s.mode))
                }
            },
            None => Err(format!("Vault #{} not found", arg.vault_id)),
        }
    }) {
        Ok(result) => result,
        Err(msg) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(msg));
        }
    };

    // Step 2: Validate liquidator payment amount
    if liquidator_payment < MIN_ICUSD_AMOUNT {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: MIN_ICUSD_AMOUNT.to_u64(),
        });
    }

    // In recovery mode, cap the payment at the amount needed to restore CR to target
    let liquidator_payment = read_state(|s| {
        let cap = s.compute_partial_liquidation_cap(&vault, collateral_price_usd);
        liquidator_payment.min(cap)
    });

    if liquidator_payment > vault.borrowed_icusd_amount {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(format!(
            "cannot liquidate more than borrowed: {} ICUSD, liquidate: {} ICUSD",
            vault.borrowed_icusd_amount, liquidator_payment
        )));
    }

    // Step 3: Calculate liquidation amounts with liquidation bonus
    let liq_bonus = read_state(|s| s.get_liquidation_bonus_for(&vault.collateral_type));
    let collateral_raw = crate::numeric::icusd_to_collateral_amount(liquidator_payment, collateral_price, config_decimals);
    let icp_to_liquidator = ICP::from(collateral_raw) * liq_bonus;

    // Ensure we don't give more collateral than available
    let actual_icp_to_liquidator = icp_to_liquidator.min(ICP::from(vault.collateral_amount));

    log!(INFO,
        "[partial_liquidate_vault] Vault #{}: liquidator pays {} icUSD, gets {} ICP (bonus: {})",
        arg.vault_id,
        liquidator_payment.to_u64(),
        actual_icp_to_liquidator.to_u64(),
        liq_bonus.to_f64()
    );
    
    // Step 4: Take icUSD from liquidator
    let icusd_block_index = match transfer_icusd_from(liquidator_payment, caller).await {
        Ok(block_index) => {
            log!(INFO, "[partial_liquidate_vault] Received {} icUSD from liquidator", liquidator_payment.to_u64());
            block_index
        },
        Err(transfer_from_error) => {
            guard_principal.fail();
            return Err(ProtocolError::TransferFromError(transfer_from_error, liquidator_payment.to_u64()));
        }
    };
    
    // Step 5: Update protocol state ATOMICALLY
    mutate_state(|s| {
        // Reduce the vault's debt by the liquidator payment amount
        if let Some(vault) = s.vault_id_to_vaults.get_mut(&arg.vault_id) {
            vault.borrowed_icusd_amount -= liquidator_payment;
            vault.collateral_amount -= actual_icp_to_liquidator.to_u64();
        }
        
        // Record the partial liquidation event
        let event = crate::event::Event::PartialLiquidateVault {
            vault_id: arg.vault_id,
            liquidator_payment,
            icp_to_liquidator: actual_icp_to_liquidator,
            liquidator: Some(caller),
            icp_rate: Some(collateral_price_usd),
        };
        crate::storage::record_event(&event);
        
        // Create pending transfer for liquidator reward
        s.pending_margin_transfers.insert(
            arg.vault_id, 
            PendingMarginTransfer {
                owner: caller,
                margin: actual_icp_to_liquidator,
                collateral_type: vault.collateral_type,
            },
        );

        log!(INFO, "[partial_liquidate_vault] Protocol state updated, pending transfer created");
    });
    
    // Step 6: Attempt immediate transfer processing
    log!(INFO, "[partial_liquidate_vault] Attempting immediate transfer processing...");
    
    match try_process_pending_transfers_immediate(arg.vault_id).await {
        Ok(processed_count) => {
            log!(INFO, "[partial_liquidate_vault] Successfully processed {} transfers immediately", processed_count);
        },
        Err(e) => {
            log!(INFO, "[partial_liquidate_vault] Immediate processing failed: {}. Transfers will be retried via timer", e);
            schedule_transfer_retry(arg.vault_id, 0);
        }
    }
    
    // Step 7: Schedule backup timer
    ic_cdk_timers::set_timer(std::time::Duration::from_secs(2), move || {
        ic_cdk::spawn(async move {
            log!(INFO, "[partial_liquidate_vault] Backup timer processing transfers for vault #{}", arg.vault_id);
            let _ = crate::process_pending_transfer().await;
        })
    });
    
    // Step 8: Liquidation is successful
    guard_principal.complete();
    
    // Calculate fee (the 10% discount is the fee)
    let liquidator_value_received = crate::numeric::collateral_usd_value(actual_icp_to_liquidator.to_u64(), collateral_price, config_decimals);
    let fee_amount = if liquidator_value_received > liquidator_payment {
        liquidator_value_received - liquidator_payment
    } else {
        ICUSD::new(0)
    };

    log!(INFO, "[partial_liquidate_vault] Partial liquidation completed successfully. Block index: {}, Fee: {}",
         icusd_block_index, fee_amount.to_u64());
    
    Ok(SuccessWithFee {
        block_index: icusd_block_index,
        fee_amount_paid: fee_amount.to_u64(),
    })
}