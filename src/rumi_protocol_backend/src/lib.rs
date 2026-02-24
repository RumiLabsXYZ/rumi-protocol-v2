use ic_cdk::{query, update, init};
use serde::{Serialize};
use ic_stable_structures::memory_manager::{MemoryId, MemoryManager};
use ic_stable_structures::DefaultMemoryImpl;
use icrc_ledger_types::icrc::generic_metadata_value::MetadataValue;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{BlockIndex, Memo, TransferArg, TransferError};
use icrc_ledger_types::icrc2::allowance::{Allowance, AllowanceArgs};
use icrc_ledger_types::icrc2::approve::{ApproveArgs, ApproveError};
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
use icrc_ledger_types::icrc3::transactions::{Approve, Burn, Mint, Transaction, Transfer};
use std::cell::RefCell;
use crate::state::PendingMarginTransfer;

use crate::event::{record_liquidate_vault, record_redistribute_vault};
use crate::guard::GuardError;
use crate::logs::{DEBUG, INFO};
use crate::numeric::{Ratio, ICUSD, ICP, UsdIcp};
use crate::state::{mutate_state, read_state, Mode};
use crate::vault::Vault;
use candid::{CandidType, Deserialize, Principal};
use ic_canister_log::log;
use rust_decimal::prelude::FromPrimitive;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;


pub mod dashboard;
pub mod event;
pub mod guard;
pub mod icrc21;
pub mod liquidity_pool;
pub mod logs;
pub mod management;
pub mod numeric;
pub mod state;
pub mod storage;
pub mod vault;
pub mod xrc;

#[cfg(any(test, feature = "test_endpoints"))]
pub mod test_helpers; 

#[cfg(test)]
mod tests;

pub const SEC_NANOS: u64 = 1_000_000_000;
pub const E8S: u64 = 100_000_000;

pub const MIN_LIQUIDITY_AMOUNT: ICUSD = ICUSD::new(1_000_000_000);
pub const MIN_ICP_AMOUNT: ICP = ICP::new(100_000);  // Instead of MIN_CKBTC_AMOUNT
pub const MIN_ICUSD_AMOUNT: ICUSD = ICUSD::new(10_000_000); // 0.1 icUSD minimum for all stablecoin operations
pub const DUST_THRESHOLD: ICUSD = ICUSD::new(100); // 0.000001 icUSD - dust threshold for vault closing

// Update collateral ratios per whitepaper
pub const RECOVERY_COLLATERAL_RATIO: Ratio = Ratio::new(dec!(1.5));  // 150%
pub const MINIMUM_COLLATERAL_RATIO: Ratio = Ratio::new(dec!(1.33));  // 133%

/// Stable token types accepted for vault repayment (1:1 with icUSD)
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StableTokenType {
    /// ckUSDT stablecoin
    CKUSDT,
    /// ckUSDC stablecoin
    CKUSDC,
}

/// Arguments for repaying vault with a stable token
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultArgWithToken {
    pub vault_id: u64,
    pub amount: u64,
    pub token_type: StableTokenType,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtocolArg {
    Init(InitArg),
    Upgrade(UpgradeArg),
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitArg {
    pub xrc_principal: Principal,
    pub icusd_ledger_principal: Principal,
    pub icp_ledger_principal: Principal,
    pub fee_e8s: u64,
    pub developer_principal: Principal,
    pub treasury_principal: Option<Principal>,
    pub stability_pool_principal: Option<Principal>,
    pub ckusdt_ledger_principal: Option<Principal>,
    pub ckusdc_ledger_principal: Option<Principal>,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeArg {
    pub mode: Option<Mode>,
}

#[derive(CandidType, Deserialize, Debug)]
pub struct ProtocolStatus {
    pub last_icp_rate: f64,
    pub last_icp_timestamp: u64,
    pub total_icp_margin: u64,
    pub total_icusd_borrowed: u64,
    pub total_collateral_ratio: f64,
    pub mode: Mode,
    pub liquidation_bonus: f64,
    pub recovery_target_cr: f64,
    pub recovery_mode_threshold: f64,
    pub recovery_liquidation_buffer: f64,
    pub reserve_redemptions_enabled: bool,
    pub reserve_redemption_fee: f64,
}

#[derive(CandidType, Deserialize, Debug)]
pub struct ReserveRedemptionResult {
    pub icusd_block_index: u64,
    pub stable_amount_sent: u64,
    pub fee_amount: u64,
    pub stable_token_used: Principal,
    pub vault_spillover_amount: u64,
}

#[derive(CandidType, Deserialize, Debug)]
pub struct ReserveBalance {
    pub ledger: Principal,
    pub balance: u64,
    pub symbol: String,
}

#[derive(CandidType, Deserialize, Debug)]
pub struct Fees {
    pub borrowing_fee: f64,
    pub redemption_fee: f64,
}

#[derive(CandidType, Deserialize, Debug)]
pub struct SuccessWithFee {
    pub block_index: u64,
    pub fee_amount_paid: u64,
}

#[derive(candid::CandidType, Deserialize)]
pub struct GetEventsArg {
    pub start: u64,
    pub length: u64,
}

#[derive(CandidType, Deserialize, Debug)]
pub struct LiquidityStatus {
    pub liquidity_provided: u64,
    pub total_liquidity_provided: u64,
    pub liquidity_pool_share: f64,
    pub available_liquidity_reward: u64,
    pub total_available_returns: u64,
}

/// Argument for adding a new collateral type via admin endpoint.
#[derive(CandidType, Clone, Debug, Deserialize)]
pub struct AddCollateralArg {
    /// ICRC-1 ledger canister ID for the new collateral token
    pub ledger_canister_id: Principal,
    /// How to fetch the USD price (e.g., XRC with specific asset pair)
    pub price_source: state::PriceSource,
    /// Below this ratio, the vault can be liquidated (e.g., 1.33)
    pub liquidation_ratio: f64,
    /// Below this ratio, recovery mode triggers (e.g., 1.5)
    pub borrow_threshold_ratio: f64,
    /// Bonus multiplier for liquidators (e.g., 1.15)
    pub liquidation_bonus: f64,
    /// One-time fee at borrow/mint time (e.g., 0.005)
    pub borrowing_fee: f64,
    /// Maximum total debt for this collateral (u64::MAX = no cap)
    pub debt_ceiling: u64,
    /// Minimum vault debt (dust threshold)
    pub min_vault_debt: u64,
    /// Token transfer fee in native units
    pub ledger_fee: u64,
    /// Target CR after recovery liquidation (e.g., 1.55)
    pub recovery_target_cr: f64,
}

#[derive(CandidType, Debug, Clone, Deserialize)]
pub enum ProtocolError {
    TransferFromError(TransferFromError, u64),
    TransferError(TransferError),
    TemporarilyUnavailable(String),
    AlreadyProcessing,
    AnonymousCallerNotAllowed,
    CallerNotOwner,
    AmountTooLow { minimum_amount: u64 },
    GenericError(String),
}

impl From<GuardError> for ProtocolError {
    fn from(e: GuardError) -> Self {
        match e {
            GuardError::AlreadyProcessing => Self::AlreadyProcessing,
            GuardError::TooManyConcurrentRequests => {
                Self::TemporarilyUnavailable("too many concurrent requests".to_string())
            },
            GuardError::StaleOperation => {
                Self::TemporarilyUnavailable("previous operation is being cleaned up".to_string())
            }
        }
    }
}

pub fn check_vaults() {
    let dummy_rate = read_state(|s| {
        s.last_icp_rate.unwrap_or_else(|| {
            log!(INFO, "[check_vaults] No ICP rate available, using default rate");
            UsdIcp::from(dec!(1.0))
        })
    });
    
    // Only identify unhealthy vaults but don't liquidate them
    let (unhealthy_vaults, healthy_vaults) = read_state(|s| {
        let mut unhealthy_vaults: Vec<Vault> = vec![];
        let mut healthy_vaults: Vec<Vault> = vec![];
        for vault in s.vault_id_to_vaults.values() {
            if compute_collateral_ratio(vault, dummy_rate, s)
                < s.get_min_liquidation_ratio_for(&vault.collateral_type)
            {
                unhealthy_vaults.push(vault.clone());
            } else {
                healthy_vaults.push(vault.clone())
            }
        }
        (unhealthy_vaults, healthy_vaults)
    });

    // Log unhealthy vaults but don't liquidate them
    if !unhealthy_vaults.is_empty() {
        log!(
            INFO,
            "[check_vaults] Found {} liquidatable vaults. Waiting for external liquidators.", 
            unhealthy_vaults.len()
        );
        
        // Log detailed information about each unhealthy vault
        for vault in unhealthy_vaults {
            let (ratio, min_ratio) = read_state(|s| {
                (
                    compute_collateral_ratio(&vault, dummy_rate, s),
                    s.get_min_liquidation_ratio_for(&vault.collateral_type),
                )
            });
            log!(
                INFO,
                "[check_vaults] Liquidatable vault #{}: owner={}, borrowed={}, collateral={}, ratio={:.2}%, min_ratio={:.2}%",
                vault.vault_id,
                vault.owner,
                vault.borrowed_icusd_amount,
                vault.collateral_amount,
                ratio.to_f64() * 100.0,
                min_ratio.to_f64() * 100.0
            );
        }
    } else {
        log!(
            DEBUG,
            "[check_vaults] All vaults are healthy at the current ICP rate: {}", 
            dummy_rate.to_f64()
        );
    }
    
    // No longer calling record_liquidate_vault to trigger automatic liquidations
}

/// Compute collateral ratio for a vault using per-collateral price and decimals.
/// Returns Ratio::ZERO when price or config is unavailable — callers must
/// independently check `last_price.is_some()` before performing operations.
pub fn compute_collateral_ratio(vault: &Vault, _rate: UsdIcp, state: &state::State) -> Ratio {
    if vault.borrowed_icusd_amount == 0 {
        return Ratio::from(Decimal::MAX);
    }
    let margin_value: ICUSD = if let Some(config) = state.get_collateral_config(&vault.collateral_type) {
        if let Some(price) = config.last_price {
            let price_dec = Decimal::from_f64(price).unwrap_or(Decimal::ZERO);
            numeric::collateral_usd_value(vault.collateral_amount, price_dec, config.decimals)
        } else {
            // No price available — return zero ratio (conservative / safe direction).
            // Operations must independently check last_price.is_some() and error out.
            return Ratio::from(Decimal::ZERO);
        }
    } else {
        // No config — return zero ratio. This vault's collateral type is unknown.
        return Ratio::from(Decimal::ZERO);
    };
    margin_value / vault.borrowed_icusd_amount
}

pub(crate) async fn process_pending_transfer() {
    let _guard = match crate::guard::TimerLogicGuard::new() {
        Some(guard) => guard,
        None => {
            log!(INFO, "[process_pending_transfer] double entry.");
            return;
        }
    };

    // Process pending margin transfers
    let pending_transfers = read_state(|s| {
        // Log for visibility
        if !s.pending_margin_transfers.is_empty() {
            log!(INFO, "[process_pending_transfer] Found {} pending margin transfers", 
                 s.pending_margin_transfers.len());
        }
        
        s.pending_margin_transfers
            .iter()
            .map(|(vault_id, margin_transfer)| (*vault_id, *margin_transfer))
            .collect::<Vec<(u64, PendingMarginTransfer)>>()
    });
    for (vault_id, transfer) in pending_transfers {
        // Look up per-collateral config for ledger and fee; fall back to global ICP defaults
        let (ledger, transfer_fee) = read_state(|s| {
            match s.get_collateral_config(&transfer.collateral_type) {
                Some(config) => (config.ledger_canister_id, ICP::from(config.ledger_fee)),
                None => (s.icp_ledger_principal, s.icp_ledger_fee),
            }
        });

        if transfer.margin <= transfer_fee {
            log!(INFO, "[transfering_margins] Skipping vault {} - margin {} <= fee {}, removing", vault_id, transfer.margin, transfer_fee);
            mutate_state(|s| { s.pending_margin_transfers.remove(&vault_id); });
            continue;
        }
        match crate::management::transfer_collateral(
            (transfer.margin - transfer_fee).to_u64(),
            transfer.owner,
            ledger,
        )
        .await
        {
            Ok(block_index) => {
                log!(
                    INFO,
                    "[transfering_margins] successfully transferred: {} to {} via ledger {}",
                    transfer.margin,
                    transfer.owner,
                    ledger
                );
                mutate_state(|s| crate::event::record_margin_transfer(s, vault_id, block_index));
            }
            Err(error) => {
                // Improved error logging with more details
                log!(
                    DEBUG,
                    "[transfering_margins] failed to transfer margin: {}, to principal: {}, via ledger: {}, with error: {}",
                    transfer.margin,
                    transfer.owner,
                    ledger,
                    error
                );

                // If there was a transfer fee error, update the fee in collateral config
                if let TransferError::BadFee { expected_fee } = error {
                    log!(INFO, "[transfering_margins] Updating transfer fee to: {:?}", expected_fee);
                    mutate_state(|s| {
                        let expected_fee_u64: u64 = expected_fee
                            .0
                            .try_into()
                            .expect("failed to convert Nat to u64");
                        if let Some(config) = s.get_collateral_config_mut(&transfer.collateral_type) {
                            config.ledger_fee = expected_fee_u64;
                        }
                        // Also update global icp_ledger_fee if this is the ICP collateral
                        let icp_ct = s.icp_collateral_type();
                        let resolved_ct = if transfer.collateral_type == candid::Principal::anonymous() {
                            icp_ct
                        } else {
                            transfer.collateral_type
                        };
                        if resolved_ct == icp_ct {
                            s.icp_ledger_fee = ICP::from(expected_fee_u64);
                        }
                    });

                    // After updating the fee, we should retry this transfer next time
                } else {
                    // For other errors, we still keep the transfer pending for retry
                    log!(INFO, "[transfering_margins] Will retry this transfer later");
                }
            }
        }
    }

    // Process pending excess collateral transfers (from full liquidations)
    let pending_excess = read_state(|s| {
        s.pending_excess_transfers
            .iter()
            .map(|(vault_id, transfer)| (*vault_id, *transfer))
            .collect::<Vec<(u64, PendingMarginTransfer)>>()
    });

    for (vault_id, transfer) in pending_excess {
        let (ledger, transfer_fee) = read_state(|s| {
            match s.get_collateral_config(&transfer.collateral_type) {
                Some(config) => (config.ledger_canister_id, ICP::from(config.ledger_fee)),
                None => (s.icp_ledger_principal, s.icp_ledger_fee),
            }
        });

        if transfer.margin <= transfer_fee {
            log!(INFO, "[transfering_excess] Skipping vault {} - margin {} <= fee {}, removing", vault_id, transfer.margin, transfer_fee);
            mutate_state(|s| { s.pending_excess_transfers.remove(&vault_id); });
            continue;
        }
        match crate::management::transfer_collateral(
            (transfer.margin - transfer_fee).to_u64(),
            transfer.owner,
            ledger,
        )
        .await
        {
            Ok(_block_index) => {
                log!(
                    INFO,
                    "[transfering_excess] successfully transferred excess collateral: {} to {} via ledger {}",
                    transfer.margin,
                    transfer.owner,
                    ledger
                );
                mutate_state(|s| { s.pending_excess_transfers.remove(&vault_id); });
            }
            Err(error) => {
                log!(
                    DEBUG,
                    "[transfering_excess] failed to transfer excess collateral: {}, to principal: {}, via ledger: {}, with error: {}",
                    transfer.margin,
                    transfer.owner,
                    ledger,
                    error
                );
            }
        }
    }

    // Similar improved logic for redemption transfers
    let pending_redemptions = read_state(|s| {
        s.pending_redemption_transfer
            .iter()
            .map(|(icusd_block_index, margin_transfer)| (*icusd_block_index, *margin_transfer))
            .collect::<Vec<(u64, PendingMarginTransfer)>>()
    });

    for (icusd_block_index, pending_transfer) in pending_redemptions {
        let (ledger, transfer_fee) = read_state(|s| {
            match s.get_collateral_config(&pending_transfer.collateral_type) {
                Some(config) => (config.ledger_canister_id, ICP::from(config.ledger_fee)),
                None => (s.icp_ledger_principal, s.icp_ledger_fee),
            }
        });

        if pending_transfer.margin <= transfer_fee {
            log!(INFO, "[transfering_redemptions] Skipping redemption {} - margin {} <= fee {}, removing", icusd_block_index, pending_transfer.margin, transfer_fee);
            mutate_state(|s| { s.pending_redemption_transfer.remove(&icusd_block_index); });
            continue;
        }
        match crate::management::transfer_collateral(
            (pending_transfer.margin - transfer_fee).to_u64(),
            pending_transfer.owner,
            ledger,
        )
        .await
        {
            Ok(block_index) => {
                log!(
                    INFO,
                    "[transfering_redemptions] successfully transferred: {} to {} via ledger {}",
                    pending_transfer.margin,
                    pending_transfer.owner,
                    ledger
                );
                mutate_state(|s| {
                    crate::event::record_redemption_transfered(s, icusd_block_index, block_index)
                });
            }
            Err(error) => log!(
                DEBUG,
                "[transfering_redemptions] failed to transfer margin: {}, via ledger: {}, with error: {}",
                pending_transfer.margin,
                ledger,
                error
            ),
        }
    }

    // Schedule another run if needed, but with better timing
    if read_state(|s| {
        !s.pending_margin_transfers.is_empty() || !s.pending_excess_transfers.is_empty() || !s.pending_redemption_transfer.is_empty()
    }) {
        // Schedule another check in 5 seconds
        log!(INFO, "[process_pending_transfer] Scheduling another transfer attempt in 5 seconds");
        ic_cdk_timers::set_timer(std::time::Duration::from_secs(5), || {
            ic_cdk::spawn(crate::process_pending_transfer())
        });
    } else {
        log!(INFO, "[process_pending_transfer] No more pending transfers");
    }
}